use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};
use tauri::{
    AppHandle, Emitter, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};

use crate::app::{AppState, StreamHubRuntimeStatus};
use crate::diagnostics::record_operation;
use crate::error::{AppError, IpcError};
use crate::library::{
    is_promotional_media_record, is_promotional_metadata, is_promotional_name, is_promotional_path,
    mark_promotional_media, scan_directory, PromotionalCleanupReport, ScanOptions, ScanReport,
};
use crate::media::{
    audio_output_capabilities, choose_audio_track, plan_playback, probe_media,
    AudioOutputCapabilities, EnhancementRuntimeStatus, MediaProbe, PlaybackMode, PlaybackPlan,
};
use crate::metadata::{
    apply_adult_classification, clear_classifier_adult, repair_adult_isolation, scrape_media,
    set_manual_adult,
    ScrapeOptions, ScrapeProgress, ScrapeProgressSink, ScrapeReport,
};
use crate::openlist::{
    OpenListAccountInfo, OpenListClient, OpenListFilePage, OpenListRuntimeStatus, OpenListStorage,
    OpenListStorageInput, OpenListStorageSchema,
};
use crate::playback::{
    LibMpvBackend, MpvConfig, PlaybackActor, PlaybackCommand, PlaybackSnapshot,
};
use crate::providers::{
    DeviceCode, DevicePollRequest, FilePage, ListFilesRequest, MediaItem, MediaKind,
    PlaybackDescriptor, PlaybackRequest, PollResult, ProviderCapabilities, ProviderError,
    ProviderSubtitle, ProviderSubtitleSearchRequest, QrLoginSession, Session, SmsLoginRequest,
    SourceDescriptor, TokenImport, VideoQuality,
};
use crate::runtime::{
    discover_resource_dir, EnhancementPlan, RuntimeDiagnostics, RuntimePaths, RuntimeResourceKind,
};
use crate::security::CredentialStore;
use crate::storage::{MediaFilter, MediaRecord};

type CommandResult<T> = Result<T, IpcError>;

const GUANGYA_REFRESH_LEEWAY_SECS: i64 = 600;

fn unix_now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub favorite_count: u64,
    pub watched_seconds: f64,
    pub library_count: u64,
    pub storage_bytes: Option<u64>,
}

fn app_error(error: AppError) -> IpcError {
    error.into()
}

fn provider_error(error: ProviderError) -> IpcError {
    IpcError {
        code: error.code().to_owned(),
        message: error.to_string(),
        retryable: error.retryable(),
        request_id: None,
        details: serde_json::Value::Object(Default::default()),
    }
}

async fn restore_saved_session(state: &AppState, provider_id: &str) -> CommandResult<bool> {
    let key = format!("provider.session.{provider_id}");
    let Some(mut session) = CredentialStore::new(&state.database)
        .load_json::<Session>(&key)
        .map_err(app_error)?
    else {
        return Ok(false);
    };
    if session.is_expired() {
        session = refresh_saved_session(state, provider_id, session).await?;
    }
    state
        .providers
        .restore_session(provider_id, session)
        .await
        .map_err(provider_error)?;
    Ok(true)
}

async fn refresh_saved_session(
    state: &AppState,
    provider_id: &str,
    current: Session,
) -> CommandResult<Session> {
    let _refresh_guard = state
        .lock_session_refresh(provider_id, current.account_id.as_deref())
        .await;
    let key = format!("provider.session.{provider_id}");
    let expected_access_token = current.access_token.clone();
    let latest = CredentialStore::new(&state.database)
        .load_json::<Session>(&key)
        .map_err(app_error)?
        .unwrap_or(current);
    if !latest.is_expired() && latest.access_token != expected_access_token {
        state
            .providers
            .restore_session(provider_id, latest.clone())
            .await
            .map_err(provider_error)?;
        return Ok(latest);
    }
    let refreshed = state
        .providers
        .refresh_session(provider_id, &latest)
        .await
        .map_err(provider_error)?;
    persist_session(state, &refreshed)?;
    state
        .providers
        .restore_session(provider_id, refreshed.clone())
        .await
        .map_err(provider_error)?;
    Ok(refreshed)
}

async fn retry_after_session_expired(state: &AppState, provider_id: &str) -> CommandResult<()> {
    let key = format!("provider.session.{provider_id}");
    let current: Session = CredentialStore::new(&state.database)
        .load_json(&key)
        .map_err(app_error)?
        .ok_or_else(|| app_error(AppError::Provider("no saved provider session".into())))?;
    refresh_saved_session(state, provider_id, current)
        .await
        .map(|_| ())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub provider_id: String,
    pub account_id: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSessionStatus {
    pub provider_id: String,
    pub connection: String,
    pub account_id: Option<String>,
    pub expires_at: Option<i64>,
    pub capabilities: ProviderCapabilities,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuangyaOAuthStatus {
    pub configured: bool,
    pub missing_fields: Vec<String>,
    pub config_file: PathBuf,
    pub device_code_login: bool,
    pub session_refresh: bool,
    pub refresh_available: bool,
    pub browse_files: bool,
    pub connection: String,
    pub account_id: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum PollResultSummary {
    Pending { interval: Option<u64> },
    SlowDown { interval: u64 },
    Authorized(SessionSummary),
    Denied,
    Expired,
}

fn persist_session(state: &AppState, session: &Session) -> CommandResult<SessionSummary> {
    CredentialStore::new(&state.database)
        .save_json(
            &format!("provider.session.{}", session.provider_id),
            session,
        )
        .map_err(app_error)?;
    Ok(SessionSummary {
        provider_id: session.provider_id.clone(),
        account_id: session.account_id.clone(),
        expires_at: session.expires_at,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaPageInput {
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub library_id: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default = "default_page_size")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryMediaInput {
    pub media_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySourceDeleteInput {
    pub source_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryMoveInput {
    pub media_id: String,
    #[serde(default)]
    pub library_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryArtworkInput {
    pub media_id: String,
    pub art_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPreviewInput {
    pub media_id: String,
    pub art_url: String,
    #[serde(default)]
    pub duration_seconds: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryScrapeInput {
    #[serde(default)]
    pub media_ids: Vec<String>,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub include_adult: bool,
    /// `fast` = JavBus only (two-phase first pass); default `full` = all six
    /// adult sources for the leftovers.
    #[serde(default)]
    pub jav_scope: Option<String>,
    #[serde(default = "default_metadata_scrape_limit")]
    pub limit: u32,
}

fn default_metadata_scrape_limit() -> u32 {
    100
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataProviderStatus {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub requires_configuration: bool,
}

fn default_page_size() -> u32 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchInput {
    pub query: String,
    #[serde(default = "default_page_size")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryInput {
    pub media_id: String,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    #[serde(default)]
    pub completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPageInput {
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub page_token: Option<String>,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default)]
    pub query: Option<String>,
    /// Optional batch 18+ decision for sync operations. `Some(true)` stamps an
    /// authoritative 18+ flag; `Some(false)` skips auto-flagging but does **not**
    /// hide later JAV matches; `None` runs automatic classification.
    #[serde(default)]
    pub mark_adult: Option<bool>,
    /// Cloud folder path being imported (`电视剧/英剧/万物生灵`). Used as the
    /// series title / search haystack when the file name itself is `S01E01`.
    #[serde(default)]
    pub folder_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerLoadInput {
    pub url: String,
    #[serde(default)]
    pub media_id: Option<String>,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    /// CENC 内容密钥（hex），仅 libmpv 路径生效。
    #[serde(default)]
    pub decryption_key: Option<String>,
    #[serde(default)]
    pub mode: Option<PlaybackMode>,
    #[serde(default)]
    pub audio_track: Option<i64>,
    #[serde(default)]
    pub subtitle_track: Option<i64>,
    #[serde(default)]
    pub preferred_audio_language: Option<String>,
    #[serde(default)]
    pub preferred_subtitle_language: Option<String>,
    #[serde(default)]
    pub interpolation: Option<bool>,
    #[serde(default)]
    pub hdr: Option<bool>,
    #[serde(default)]
    pub audio_passthrough: Option<bool>,
    /// Optional probe supplied by the caller so playback does not need to
    /// repeat an expensive remote inspection. The backend still validates
    /// the source when the probe is absent.
    #[serde(default)]
    pub media: Option<MediaProbe>,
    #[serde(default)]
    pub resume_position_seconds: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePlayerOpenInput {
    pub url: String,
    #[serde(default)]
    pub media_id: Option<String>,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub resume_position_seconds: Option<f64>,
    #[serde(default)]
    pub audio_passthrough: Option<bool>,
    #[serde(default)]
    pub mode: Option<PlaybackMode>,
    #[serde(default)]
    pub media: Option<MediaProbe>,
    #[serde(default)]
    pub audio_track: Option<i64>,
    #[serde(default)]
    pub subtitle_track: Option<i64>,
    #[serde(default)]
    pub preferred_audio_language: Option<String>,
    #[serde(default)]
    pub preferred_subtitle_language: Option<String>,
    #[serde(default)]
    pub interpolation: Option<bool>,
    #[serde(default)]
    pub hdr: Option<bool>,
    /// CENC 内容密钥（hex）。红果锁定集直链流播时由 libmpv 经
    /// demuxer-lavf-o 注入；普通媒体不传。
    #[serde(default)]
    pub decryption_key: Option<String>,
    /// Frontend playback session that owns this actor. Optional for callers
    /// from older builds; session-aware callers prevent stale closes.
    #[serde(default)]
    pub session_id: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NativePlayerCloseInput {
    /// When present, only close the actor belonging to this session. Omitting
    /// it preserves compatibility with the legacy native-player page.
    #[serde(default)]
    pub session_id: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaProbeInput {
    pub url: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WindowPipInput {
    #[serde(default)]
    pub video_width: Option<i32>,
    #[serde(default)]
    pub video_height: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSearchInput {
    pub url: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub file_hash: Option<String>,
    #[serde(default)]
    pub year: Option<i64>,
    #[serde(default)]
    pub season: Option<i64>,
    #[serde(default)]
    pub episode: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSearchResult {
    pub path: String,
    pub language: Option<String>,
    pub source: String,
    pub downloaded: bool,
    pub file_id: Option<String>,
    pub release: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleDownloadInput {
    pub file_id: String,
    pub media_id: Option<String>,
    pub file_name: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleCredentialsInput {
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleAttachInput {
    pub path: String,
    #[serde(default)]
    pub select: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleImportInput {
    pub file_name: String,
    pub content: String,
    #[serde(default)]
    pub select: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleRemoveInput {
    pub track_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataNfoWriteInput {
    pub media_id: String,
    #[serde(default)]
    pub fields: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerPrepareInput {
    pub url: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerPreviewFrameInput {
    pub url: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    pub position_seconds: f64,
    #[serde(default)]
    pub decryption_key: Option<String>,
    #[serde(default)]
    pub media_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerPreviewFrame {
    pub data_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedPlayback {
    pub url: String,
    pub cached: bool,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySource {
    pub path: String,
    #[serde(default = "default_source_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalPlayer {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalPlayerOpenInput {
    pub player_id: String,
    pub url: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerCapabilities {
    pub libmpv: bool,
    pub hdr: bool,
    pub shaders: bool,
    pub interpolation: bool,
    pub playlist: bool,
    pub external_players: bool,
    pub controls: Vec<String>,
}

fn external_player_candidates() -> Vec<(&'static str, Vec<PathBuf>)> {
    let mut roots = Vec::new();
    for key in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(value) = std::env::var_os(key) {
            roots.push(PathBuf::from(value));
        }
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let mut candidates = vec![
        (
            "potplayer",
            vec![
                PathBuf::from("PotPlayerMini64.exe"),
                PathBuf::from("PotPlayer64.exe"),
            ],
        ),
        ("vlc", vec![PathBuf::from("vlc.exe")]),
        (
            "mpc-hc",
            vec![PathBuf::from("mpc-hc64.exe"), PathBuf::from("mpc-hc.exe")],
        ),
        (
            "mpc-be",
            vec![PathBuf::from("mpc-be64.exe"), PathBuf::from("mpc-be.exe")],
        ),
        ("mpv", vec![PathBuf::from("mpv.exe")]),
    ];
    for (_, paths) in candidates.iter_mut() {
        let names = paths.clone();
        paths.clear();
        for name in names {
            if name.is_absolute() {
                paths.push(name);
                continue;
            }
            for root in &roots {
                paths.push(root.join(&name));
            }
            if let Some(root) = &local {
                paths.push(root.join("Programs").join(&name));
            }
        }
    }
    candidates
}

fn discover_external_players() -> Vec<ExternalPlayer> {
    external_player_candidates()
        .into_iter()
        .filter_map(|(id, paths)| {
            paths
                .into_iter()
                .find(|path| path.is_file())
                .map(|path| ExternalPlayer {
                    id: id.into(),
                    path,
                })
        })
        .collect()
}

fn default_source_enabled() -> bool {
    true
}

#[tauri::command]
fn health() -> &'static str {
    "ok"
}

#[tauri::command]
fn runtime_status(state: State<'_, AppState>) -> RuntimeDiagnostics {
    state.runtime.clone()
}

#[tauri::command]
fn runtime_diagnostics(state: State<'_, AppState>) -> RuntimeDiagnostics {
    state.runtime.clone()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamHubHealth {
    pub base_url: String,
    pub reachable: bool,
    pub http_status: Option<u16>,
    pub latency_ms: Option<u64>,
    pub status: Option<String>,
    pub message: String,
}

#[tauri::command]
fn streamhub_status(state: State<'_, AppState>) -> StreamHubRuntimeStatus {
    state.streamhub.status()
}

#[tauri::command]
fn streamhub_start(state: State<'_, AppState>) -> CommandResult<StreamHubRuntimeStatus> {
    state.streamhub.start().map_err(|message| IpcError {
        code: "streamhub_start_failed".into(),
        message,
        retryable: true,
        request_id: None,
        details: serde_json::Value::Object(Default::default()),
    })
}

#[tauri::command]
fn streamhub_stop(state: State<'_, AppState>) -> CommandResult<StreamHubRuntimeStatus> {
    state.streamhub.stop().map_err(|message| IpcError {
        code: "streamhub_stop_failed".into(),
        message,
        retryable: false,
        request_id: None,
        details: serde_json::Value::Object(Default::default()),
    })
}

#[tauri::command]
async fn streamhub_health(state: State<'_, AppState>) -> CommandResult<StreamHubHealth> {
    let base_url = state.streamhub.base_url().trim_end_matches('/').to_owned();
    let endpoint = format!("{base_url}/api/system/health");
    let started = std::time::Instant::now();
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return Ok(StreamHubHealth {
                base_url,
                reachable: false,
                http_status: None,
                latency_ms: None,
                status: None,
                message: format!("无法创建健康检查客户端：{error}"),
            })
        }
    };
    match client.get(endpoint).send().await {
        Ok(response) => {
            let http_status = response.status().as_u16();
            let reachable = response.status().is_success();
            let body = response.json::<serde_json::Value>().await.ok();
            let status = body
                .as_ref()
                .and_then(|value| value.get("status"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            Ok(StreamHubHealth {
                base_url,
                reachable,
                http_status: Some(http_status),
                latency_ms: Some(started.elapsed().as_millis() as u64),
                status,
                message: if reachable {
                    "StreamHub 健康检查通过".into()
                } else {
                    format!("StreamHub 返回 HTTP {http_status}")
                },
            })
        }
        Err(error) => Ok(StreamHubHealth {
            base_url,
            reachable: false,
            http_status: None,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            status: None,
            message: format!("StreamHub 尚未响应：{error}"),
        }),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConnectionReport {
    pub provider_id: String,
    pub capabilities: ProviderCapabilities,
    pub authenticated: bool,
    pub reachable: bool,
    pub item_count: u64,
    pub latency_ms: u64,
    pub message: String,
}

#[tauri::command]
async fn provider_test_connection(
    provider_id: String,
    state: State<'_, AppState>,
) -> CommandResult<ProviderConnectionReport> {
    let capabilities = state
        .providers
        .capabilities(&provider_id)
        .map_err(provider_error)?;
    let started = std::time::Instant::now();
    let result = provider_list_files_impl(
        &provider_id,
        ProviderPageInput {
            parent_id: None,
            page_token: None,
            page_size: 1,
            query: None,
            mark_adult: None,
            folder_path: None,
        },
        &state,
    )
    .await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let authenticated = CredentialStore::new(&state.database)
        .load_json::<Session>(&format!("provider.session.{provider_id}"))
        .map_err(app_error)?
        .is_some();
    Ok(match result {
        Ok(page) => ProviderConnectionReport {
            provider_id,
            capabilities,
            authenticated: authenticated || capabilities.token_import == false,
            reachable: true,
            item_count: page.total.unwrap_or(page.files.len() as u64),
            latency_ms,
            message: "来源连通且可读取目录".into(),
        },
        Err(error) => {
            let reachable = error.code != "provider_network_error";
            ProviderConnectionReport {
                provider_id,
                capabilities,
                authenticated,
                reachable,
                item_count: 0,
                latency_ms,
                message: error.message,
            }
        }
    })
}

#[tauri::command]
fn provider_list(state: State<'_, AppState>) -> Vec<String> {
    state.providers.ids()
}

#[tauri::command]
fn source_catalog() -> Vec<SourceDescriptor> {
    crate::providers::source_catalog()
}

fn openlist_client(state: &AppState) -> CommandResult<OpenListClient> {
    let stored = CredentialStore::new(&state.database)
        .load_json::<OpenListSession>("openlist.session")
        .map_err(app_error)?
        .map(|session| session.token);
    let token = stored.or_else(|| std::env::var("TTV_OPENLIST_TOKEN").ok());
    OpenListClient::new(state.openlist.base_url())
        .map(|client| client.with_token(token))
        .map_err(app_error)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenListSession {
    token: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListStatusReport {
    pub runtime: OpenListRuntimeStatus,
    pub reachable: bool,
    pub version: Option<String>,
    pub authenticated: bool,
}

#[tauri::command]
async fn openlist_status(state: State<'_, AppState>) -> CommandResult<OpenListStatusReport> {
    let runtime = state.openlist.status();
    let client = openlist_client(&state)?;
    let (reachable, version) = client.health().await.map_err(app_error)?;
    let authenticated = if reachable && client.token().is_some() {
        // A stored token is only a credential candidate. Confirm it against a
        // protected admin endpoint before reporting an authenticated session.
        client.storage_list().await.is_ok()
    } else {
        false
    };
    Ok(OpenListStatusReport {
        runtime: OpenListRuntimeStatus {
            reachable,
            version: version.clone(),
            ..runtime
        },
        reachable,
        version,
        authenticated,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListLoginInput {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListLoginResult {
    pub authenticated: bool,
    pub username: String,
}

#[tauri::command]
async fn openlist_login(
    input: OpenListLoginInput,
    state: State<'_, AppState>,
) -> CommandResult<OpenListLoginResult> {
    if input.username.trim().is_empty() || input.password.is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "OpenList 用户名和密码不能为空".into(),
        )));
    }
    let client = OpenListClient::new(state.openlist.base_url()).map_err(app_error)?;
    let token = client
        .login(&input.username, &input.password)
        .await
        .map_err(app_error)?;
    CredentialStore::new(&state.database)
        .save_json("openlist.session", &OpenListSession { token })
        .map_err(app_error)?;
    Ok(OpenListLoginResult {
        authenticated: true,
        username: input.username,
    })
}

#[tauri::command]
fn openlist_logout(state: State<'_, AppState>) -> CommandResult<bool> {
    CredentialStore::new(&state.database)
        .delete("openlist.session")
        .map_err(app_error)
}

#[tauri::command]
fn openlist_start(state: State<'_, AppState>) -> CommandResult<OpenListRuntimeStatus> {
    state
        .openlist
        .start()
        .map_err(|error| app_error(AppError::Runtime(error)))
}

#[tauri::command]
fn openlist_stop(state: State<'_, AppState>) -> CommandResult<OpenListRuntimeStatus> {
    state
        .openlist
        .stop()
        .map_err(|error| app_error(AppError::Runtime(error)))
}

#[tauri::command]
fn openlist_restart(state: State<'_, AppState>) -> CommandResult<OpenListRuntimeStatus> {
    state
        .openlist
        .restart()
        .map_err(|error| app_error(AppError::Runtime(error)))
}

#[tauri::command]
async fn openlist_session_status(
    state: State<'_, AppState>,
) -> CommandResult<OpenListStatusReport> {
    openlist_status(state).await
}

#[tauri::command]
async fn openlist_storage_schema(
    driver: String,
    state: State<'_, AppState>,
) -> CommandResult<OpenListStorageSchema> {
    openlist_client(&state)?
        .storage_schema(&driver)
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn openlist_storage_list(state: State<'_, AppState>) -> CommandResult<Vec<OpenListStorage>> {
    openlist_client(&state)?
        .storage_list()
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn openlist_storage_get(
    id: String,
    state: State<'_, AppState>,
) -> CommandResult<Option<OpenListStorage>> {
    let items = openlist_client(&state)?
        .storage_list()
        .await
        .map_err(app_error)?;
    Ok(items.into_iter().find(|item| item.id == id))
}

#[tauri::command]
async fn openlist_storage_save(
    input: OpenListStorageInput,
    state: State<'_, AppState>,
) -> CommandResult<OpenListStorage> {
    openlist_client(&state)?
        .storage_save(&input)
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn openlist_storage_delete(id: String, state: State<'_, AppState>) -> CommandResult<bool> {
    openlist_client(&state)?
        .storage_delete(&id)
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn openlist_storage_test(
    id: String,
    state: State<'_, AppState>,
) -> CommandResult<OpenListStorage> {
    openlist_client(&state)?
        .storage_test(&id)
        .await
        .map_err(app_error)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListAuthResult {
    pub status: String,
    pub message: String,
    pub authorization_url: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListAuthInput {
    pub storage_id: String,
    #[serde(default)]
    pub fields: serde_json::Value,
}

#[tauri::command]
async fn openlist_begin_auth(
    input: OpenListAuthInput,
    state: State<'_, AppState>,
) -> CommandResult<OpenListAuthResult> {
    let _ = (input, state);
    Ok(OpenListAuthResult {
        status: "manual".into(),
        message: "请保存存储配置后按 OpenList 驱动要求完成授权".into(),
        authorization_url: None,
        session_id: None,
    })
}

#[tauri::command]
async fn openlist_finish_auth(
    input: OpenListAuthInput,
    state: State<'_, AppState>,
) -> CommandResult<OpenListAuthResult> {
    let client = openlist_client(&state)?;
    let _ = client
        .account_info(&input.storage_id)
        .await
        .map_err(app_error)?;
    Ok(OpenListAuthResult {
        status: "authorized".into(),
        message: "OpenList 账号信息已刷新".into(),
        authorization_url: None,
        session_id: None,
    })
}

fn default_root_path() -> String {
    "/".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListListInput {
    pub storage_id: String,
    #[serde(default = "default_root_path")]
    pub path: String,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
}

#[tauri::command]
async fn openlist_list_files(
    input: OpenListListInput,
    state: State<'_, AppState>,
) -> CommandResult<OpenListFilePage> {
    openlist_client(&state)?
        .list_files(
            &input.storage_id,
            &input.path,
            input.page_size,
            input.cursor.as_deref(),
            input.query.as_deref(),
        )
        .await
        .map_err(app_error)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListPlaybackInput {
    pub storage_id: String,
    pub path: String,
    #[serde(default)]
    pub media_id: Option<String>,
    #[serde(default)]
    pub quality: Option<String>,
}

#[tauri::command]
async fn openlist_resolve_playback(
    input: OpenListPlaybackInput,
    state: State<'_, AppState>,
) -> CommandResult<PlaybackDescriptor> {
    if let Some(media_id) = input.media_id.as_deref() {
        if state
            .database
            .get_media(media_id)
            .map_err(app_error)?
            .is_some_and(|media| is_promotional_media_record(&media))
        {
            return Err(app_error(AppError::InvalidInput(
                "promotional media cannot be played".into(),
            )));
        }
    }
    if is_promotional_path(&input.path) {
        return Err(app_error(AppError::InvalidInput(
            "promotional media cannot be played".into(),
        )));
    }
    let url = openlist_client(&state)?
        .resolve_playback(&input.path)
        .await
        .map_err(app_error)?;
    Ok(PlaybackDescriptor {
        source: format!("openlist:{}", input.storage_id),
        url,
        headers: Default::default(),
        quality: input.quality,
        expires_at: None,
        media_id: input.media_id.unwrap_or_else(|| input.path.clone()),
        outcome: "resolved".into(),
        qualities: None,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListSyncInput {
    pub storage_id: String,
    #[serde(default = "default_root_path")]
    pub path: String,
    #[serde(default = "default_recursive_limit")]
    pub max_items: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListSyncReport {
    pub storage_id: String,
    pub fetched: u64,
    pub imported: u64,
    pub skipped: u64,
    pub skipped_promotional: u64,
    pub skipped_non_video: u64,
    pub folders: u64,
    pub truncated: bool,
}

#[tauri::command]
async fn openlist_sync_library(
    input: OpenListSyncInput,
    state: State<'_, AppState>,
) -> CommandResult<OpenListSyncReport> {
    state.reset_task_cancel();
    let client = openlist_client(&state)?;
    let mut pending = vec![input.path.clone()];
    let mut fetched = 0_u64;
    let mut imported = 0_u64;
    let mut skipped = 0_u64;
    let mut skipped_promotional = 0_u64;
    let mut skipped_non_video = 0_u64;
    let mut folders = 0_u64;
    let mut truncated = false;
    let mut cancelled = false;
    while let Some(path) = pending.pop() {
        if state.tasks_cancelled() {
            cancelled = true;
            break;
        }
        let page = client
            .list_files(&input.storage_id, &path, 200, None, None)
            .await
            .map_err(app_error)?;
        for item in page.files {
            fetched += 1;
            if fetched > u64::from(input.max_items.clamp(1, 100_000)) {
                truncated = true;
                break;
            }
            if item.is_folder {
                folders += 1;
                if is_promotional_name(&item.name) {
                    skipped += 1;
                    skipped_promotional += 1;
                    continue;
                }
                pending.push(item.path);
                continue;
            }
            let is_video = item
                .mime_type
                .as_deref()
                .is_some_and(|mime| mime.starts_with("video/"))
                || [".mp4", ".mkv", ".webm", ".avi", ".mov", ".m4v", ".ts"]
                    .iter()
                    .any(|ext| item.name.to_ascii_lowercase().ends_with(ext));
            if is_promotional_name(&item.name) {
                skipped += 1;
                skipped_promotional += 1;
                continue;
            }
            if !is_video {
                skipped += 1;
                skipped_non_video += 1;
                continue;
            }
            let folder_path = item
                .path
                .rsplit_once('/')
                .map(|(parent, _)| parent.trim_end_matches('/'))
                .filter(|parent| !parent.is_empty() && *parent != "/");
            let mut media = MediaRecord::new(
                format!("openlist:{}:{}", input.storage_id, item.id),
                "video",
                item.name.clone(),
            );
            media.source_type = "openlist".into();
            media.remote_path = Some(item.path.clone());
            media.art_url = item.thumbnail_url.clone();
            media.payload = Some(json!({
                "providerId":"openlist",
                "storageId":input.storage_id,
                "fileId":item.id,
                "path":item.path,
                "mimeType":item.mime_type,
                "folderPath": folder_path,
                "folderName": folder_path.and_then(|path| path.rsplit('/').find(|part| !part.is_empty())),
            }));
            apply_adult_classification(&mut media);
            state.database.upsert_media(&media).map_err(app_error)?;
            imported += 1;
        }
        if truncated {
            break;
        }
    }
    Ok(OpenListSyncReport {
        storage_id: input.storage_id,
        fetched,
        imported,
        skipped,
        skipped_promotional,
        skipped_non_video,
        folders,
        truncated: truncated || cancelled,
    })
}

#[tauri::command]
async fn openlist_account_info(
    storage_id: String,
    state: State<'_, AppState>,
) -> CommandResult<OpenListAccountInfo> {
    openlist_client(&state)?
        .account_info(&storage_id)
        .await
        .map_err(app_error)
}

/// Returns a credential-safe Guangya OAuth health summary. The stored access
/// and refresh tokens never leave the DPAPI-backed credential store.
#[tauri::command]
async fn guangya_oauth_status(state: State<'_, AppState>) -> CommandResult<GuangyaOAuthStatus> {
    const KEY: &str = "provider.session.guangya";
    let mut saved_session: Option<Session> = CredentialStore::new(&state.database)
        .load_json(KEY)
        .map_err(app_error)?;
    if saved_session.as_ref().is_some_and(|session| {
        session.is_expired_at(unix_now_seconds(), GUANGYA_REFRESH_LEEWAY_SECS)
    }) {
        if let Some(current) = saved_session.clone() {
            if refresh_saved_session(&state, "guangya", current)
                .await
                .is_ok()
            {
                saved_session = CredentialStore::new(&state.database)
                    .load_json(KEY)
                    .map_err(app_error)?;
            }
        }
    }
    let (connection, account_id, expires_at, refresh_available) = match saved_session {
        Some(session) if session.is_expired() => (
            "expired".into(),
            session.account_id,
            session.expires_at,
            session
                .refresh_token
                .as_deref()
                .is_some_and(|token| !token.trim().is_empty()),
        ),
        Some(session) => (
            "connected".into(),
            session.account_id,
            session.expires_at,
            session
                .refresh_token
                .as_deref()
                .is_some_and(|token| !token.trim().is_empty()),
        ),
        None => ("notConnected".into(), None, None, false),
    };
    let capabilities = state
        .providers
        .capabilities("guangya")
        .map_err(provider_error)?;
    Ok(GuangyaOAuthStatus {
        configured: state.guangya_oauth_missing_fields.is_empty(),
        missing_fields: state.guangya_oauth_missing_fields.clone(),
        config_file: state.paths.data_dir.join(crate::config::CONFIG_FILE_NAME),
        device_code_login: capabilities.device_code_login,
        session_refresh: capabilities.session_refresh,
        refresh_available,
        browse_files: capabilities.browse_files,
        connection,
        account_id,
        expires_at,
    })
}

#[tauri::command]
fn provider_capabilities(
    provider_id: String,
    state: State<'_, AppState>,
) -> CommandResult<ProviderCapabilities> {
    state
        .providers
        .capabilities(&provider_id)
        .map_err(provider_error)
}

/// Open a first-party provider page in an isolated Tauri WebView.
///
/// Several cloud vendors expose a QR flow only inside their own web
/// application (and keep the polling state in that page).  We deliberately
/// load the vendor page as-is instead of copying cookies or private QR APIs.
#[tauri::command]
fn provider_open_official_page<R: Runtime>(
    app: AppHandle<R>,
    provider_id: String,
    page: String,
) -> CommandResult<String> {
    let (url, title) = match (provider_id.as_str(), page.as_str()) {
        ("baidu", "login") => ("https://pan.baidu.com/", "百度网盘登录"),
        ("aliyun", "login") => ("https://www.aliyundrive.com/sign/in", "阿里云盘登录"),
        ("tianyi", "login") => ("https://cloud.189.cn/web/login.html", "天翼云盘登录"),
        ("cloud123", "login") => (
            "https://user.123pan.cn/centerlogin?redirect_url=https%3A%2F%2Fyun.123pan.cn%2F%3Fnotoken%3D1&source_page=website",
            "123云盘登录",
        ),
        ("quark", "login") => ("https://pan.quark.cn/", "夸克网盘登录"),
        ("115", "login") => ("https://115.com/", "115网盘登录"),
        ("guangya", "login") => ("https://www.guangyapan.com/", "光鸭云盘登录"),
        ("baidu", "workspace") => ("https://pan.baidu.com/disk/main", "百度网盘文件"),
        ("aliyun", "workspace") => ("https://www.aliyundrive.com/drive", "阿里云盘文件"),
        ("tianyi", "workspace") => ("https://cloud.189.cn/main.html", "天翼云盘文件"),
        ("cloud123", "workspace") => ("https://yun.123pan.cn/", "123云盘文件"),
        ("quark", "workspace") => ("https://pan.quark.cn/list", "夸克网盘文件"),
        ("115", "workspace") => ("https://115.com/?ct=7", "115网盘文件"),
        ("guangya", "workspace") => ("https://www.guangyapan.com/", "光鸭云盘文件"),
        _ => {
            return Err(IpcError {
                code: "unsupported_provider_page".into(),
                message: "该网盘没有受支持的官方页面".into(),
                retryable: false,
                request_id: None,
                details: serde_json::Value::Object(Default::default()),
            });
        }
    };
    let parsed = url.parse::<Url>().map_err(|error| IpcError {
        code: "invalid_provider_url".into(),
        message: error.to_string(),
        retryable: false,
        request_id: None,
        details: serde_json::Value::Object(Default::default()),
    })?;
    let label = format!("provider-{provider_id}-{page}");
    if let Some(window) = app.get_webview_window(&label) {
        window.navigate(parsed).map_err(|error| IpcError {
            code: "provider_window_navigation_failed".into(),
            message: error.to_string(),
            retryable: true,
            request_id: None,
            details: serde_json::Value::Object(Default::default()),
        })?;
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(label);
    }
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed))
        .title(title)
        .inner_size(980.0, 760.0)
        .min_inner_size(760.0, 560.0)
        .center()
        .resizable(true)
        .build()
        .map_err(|error| IpcError {
            code: "provider_window_create_failed".into(),
            message: error.to_string(),
            retryable: true,
            request_id: None,
            details: serde_json::Value::Object(Default::default()),
        })?;
    Ok(label)
}

#[tauri::command]
fn provider_session_status(
    provider_id: String,
    state: State<'_, AppState>,
) -> CommandResult<ProviderSessionStatus> {
    let capabilities = state
        .providers
        .capabilities(&provider_id)
        .map_err(provider_error)?;
    let session: Option<Session> = CredentialStore::new(&state.database)
        .load_json(&format!("provider.session.{provider_id}"))
        .map_err(app_error)?;
    let (connection, account_id, expires_at) = match session {
        Some(session) if session.is_expired() => {
            ("expired".into(), session.account_id, session.expires_at)
        }
        Some(session) => ("connected".into(), session.account_id, session.expires_at),
        None => ("notConnected".into(), None, None),
    };
    Ok(ProviderSessionStatus {
        provider_id,
        connection,
        account_id,
        expires_at,
        capabilities,
    })
}

#[tauri::command]
async fn provider_device_code(
    provider_id: String,
    state: State<'_, AppState>,
) -> CommandResult<DeviceCode> {
    state
        .providers
        .login_device_code(&provider_id)
        .await
        .map_err(provider_error)
}

#[tauri::command]
async fn provider_qr_login_create(
    provider_id: String,
    state: State<'_, AppState>,
) -> CommandResult<QrLoginSession> {
    state
        .providers
        .create_qr_login(&provider_id)
        .await
        .map_err(provider_error)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QrLoginPollInput {
    pub session_id: String,
}

#[tauri::command]
async fn provider_qr_login_poll(
    provider_id: String,
    input: QrLoginPollInput,
    state: State<'_, AppState>,
) -> CommandResult<PollResultSummary> {
    if input.session_id.trim().is_empty() {
        return Err(IpcError {
            code: "invalid_input".into(),
            message: "QR login session_id is required".into(),
            retryable: false,
            request_id: None,
            details: serde_json::Value::Object(Default::default()),
        });
    }
    let result = state
        .providers
        .poll_device_token(
            &provider_id,
            DevicePollRequest {
                device_code: input.session_id,
            },
        )
        .await
        .map_err(provider_error)?;
    match result {
        PollResult::Pending { interval } => Ok(PollResultSummary::Pending { interval }),
        PollResult::SlowDown { interval } => Ok(PollResultSummary::SlowDown { interval }),
        PollResult::Authorized(session) => Ok(PollResultSummary::Authorized(persist_session(
            &state, &session,
        )?)),
        PollResult::Denied => Ok(PollResultSummary::Denied),
        PollResult::Expired => Ok(PollResultSummary::Expired),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthAuthorizationInput {
    #[serde(default)]
    pub state: Option<String>,
}

#[tauri::command]
fn provider_oauth_authorization_url(
    provider_id: String,
    input: OAuthAuthorizationInput,
    state: State<'_, AppState>,
) -> CommandResult<String> {
    state
        .providers
        .authorization_url(&provider_id, input.state.as_deref())
        .map_err(provider_error)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCodeInput {
    pub code: String,
}

#[tauri::command]
async fn provider_oauth_exchange_code(
    provider_id: String,
    input: OAuthCodeInput,
    state: State<'_, AppState>,
) -> CommandResult<SessionSummary> {
    let session = state
        .providers
        .exchange_authorization_code(&provider_id, input.code)
        .await
        .map_err(provider_error)?;
    persist_session(&state, &session)
}

#[tauri::command]
async fn provider_poll(
    provider_id: String,
    request: DevicePollRequest,
    state: State<'_, AppState>,
) -> CommandResult<PollResultSummary> {
    let result = state
        .providers
        .poll_device_token(&provider_id, request)
        .await
        .map_err(provider_error)?;
    match result {
        PollResult::Pending { interval } => Ok(PollResultSummary::Pending { interval }),
        PollResult::SlowDown { interval } => Ok(PollResultSummary::SlowDown { interval }),
        PollResult::Authorized(session) => Ok(PollResultSummary::Authorized(persist_session(
            &state, &session,
        )?)),
        PollResult::Denied => Ok(PollResultSummary::Denied),
        PollResult::Expired => Ok(PollResultSummary::Expired),
    }
}

#[tauri::command]
async fn provider_sms_login(
    provider_id: String,
    request: SmsLoginRequest,
    state: State<'_, AppState>,
) -> CommandResult<SessionSummary> {
    let session = state
        .providers
        .login_sms(&provider_id, request)
        .await
        .map_err(provider_error)?;
    persist_session(&state, &session)
}

#[tauri::command]
async fn provider_import_token(
    provider_id: String,
    input: TokenImport,
    state: State<'_, AppState>,
) -> CommandResult<SessionSummary> {
    let session = state
        .providers
        .import_token(&provider_id, input)
        .await
        .map_err(provider_error)?;
    persist_session(&state, &session)
}

#[tauri::command]
async fn provider_refresh(
    provider_id: String,
    state: State<'_, AppState>,
) -> CommandResult<SessionSummary> {
    let key = format!("provider.session.{provider_id}");
    let current: Session = CredentialStore::new(&state.database)
        .load_json(&key)
        .map_err(app_error)?
        .ok_or_else(|| app_error(AppError::Provider("no saved provider session".into())))?;
    let refreshed = refresh_saved_session(&state, &provider_id, current).await?;
    Ok(SessionSummary {
        provider_id: refreshed.provider_id,
        account_id: refreshed.account_id,
        expires_at: refreshed.expires_at,
    })
}

#[tauri::command]
async fn provider_logout(provider_id: String, state: State<'_, AppState>) -> CommandResult<bool> {
    let removed = CredentialStore::new(&state.database)
        .delete(&format!("provider.session.{provider_id}"))
        .map_err(app_error)?;
    state
        .providers
        .clear_session(&provider_id)
        .await
        .map_err(provider_error)?;
    Ok(removed)
}

#[tauri::command]
async fn provider_list_files(
    provider_id: String,
    input: ProviderPageInput,
    state: State<'_, AppState>,
) -> CommandResult<FilePage> {
    provider_list_files_impl(&provider_id, input, &state).await
}

async fn provider_list_files_impl(
    provider_id: &str,
    input: ProviderPageInput,
    state: &AppState,
) -> CommandResult<FilePage> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let started = std::time::Instant::now();
    restore_saved_session(state, provider_id).await?;
    let request = ListFilesRequest {
        parent_id: input.parent_id,
        page_token: input.page_token,
        page_size: Some(input.page_size.min(500)),
        query: input.query.filter(|query| !query.trim().is_empty()),
    };
    let result = state
        .providers
        .list_files(provider_id, request.clone())
        .await;
    let mut retry_count = 0;
    let result = match result {
        Ok(page) => Ok(page),
        Err(ProviderError::SessionExpired) => {
            retry_count = 1;
            retry_after_session_expired(state, provider_id).await?;
            state.providers.list_files(provider_id, request).await
        }
        Err(error) => Err(error),
    };
    record_provider_result(
        "list_files",
        &request_id,
        provider_id,
        started.elapsed(),
        retry_count,
        &result,
    );
    result.map_err(provider_error)
}

#[tauri::command]
async fn provider_resolve_playback(
    provider_id: String,
    request: PlaybackRequest,
    state: State<'_, AppState>,
) -> CommandResult<PlaybackDescriptor> {
    provider_resolve_playback_impl(&provider_id, request, &state).await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSyncReport {
    pub provider_id: String,
    pub fetched: u64,
    pub imported: u64,
    pub skipped: u64,
    pub skipped_promotional: u64,
    pub skipped_non_video: u64,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRecursiveSyncInput {
    #[serde(default)]
    pub root_id: Option<String>,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default = "default_recursive_limit")]
    pub max_items: u32,
    /// Optional batch 18+ decision applied to every imported item (see
    /// [`ProviderPageInput::mark_adult`]).
    #[serde(default)]
    pub mark_adult: Option<bool>,
}

fn default_recursive_limit() -> u32 {
    10_000
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRecursiveSyncReport {
    pub provider_id: String,
    pub fetched: u64,
    pub imported: u64,
    pub skipped: u64,
    pub skipped_promotional: u64,
    pub skipped_non_video: u64,
    pub folders: u64,
    pub truncated: bool,
}

#[tauri::command]
async fn provider_sync_library(
    provider_id: String,
    input: ProviderPageInput,
    state: State<'_, AppState>,
) -> CommandResult<ProviderSyncReport> {
    state.reset_task_cancel();
    let mark_adult = input.mark_adult;
    let folder_path = input.folder_path.clone();
    let page = provider_list_files_impl(&provider_id, input, &state).await?;
    let mut imported = 0_u64;
    let mut skipped = 0_u64;
    let mut skipped_promotional = 0_u64;
    let mut skipped_non_video = 0_u64;
    for item in &page.files {
        if state.tasks_cancelled() {
            break;
        }
        if item.kind != MediaKind::File {
            skipped = skipped.saturating_add(1);
            continue;
        }
        if is_promotional_media_item(&provider_id, item) {
            skipped = skipped.saturating_add(1);
            skipped_promotional = skipped_promotional.saturating_add(1);
            continue;
        }
        if !is_video_media_item(&provider_id, item) {
            skipped = skipped.saturating_add(1);
            skipped_non_video = skipped_non_video.saturating_add(1);
            continue;
        }
        let Some(mut media) =
            provider_media_record(&provider_id, item, folder_path.as_deref())
        else {
            skipped = skipped.saturating_add(1);
            continue;
        };
        apply_import_adult_choice(&mut media, mark_adult);
        state.database.upsert_media(&media).map_err(app_error)?;
        imported = imported.saturating_add(1);
    }
    Ok(ProviderSyncReport {
        provider_id,
        fetched: page.files.len() as u64,
        imported,
        skipped,
        skipped_promotional,
        skipped_non_video,
        next_page_token: page.next_page_token,
    })
}

#[tauri::command]
async fn provider_sync_library_recursive<R: Runtime>(
    provider_id: String,
    input: ProviderRecursiveSyncInput,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> CommandResult<ProviderRecursiveSyncReport> {
    state.reset_task_cancel();
    let mark_adult = input.mark_adult;
    let page_size = input.page_size.clamp(1, 500);
    let max_items = u64::from(input.max_items.clamp(1, 100_000));
    // The stack carries (parent_id, page_token, display_name) so progress
    // ticks can name the folder being walked.
    let task_key = format!(
        "{}:{}",
        provider_id,
        input.root_id.as_deref().unwrap_or("root")
    );
    let mut pending = vec![(
        input.root_id,
        None::<String>,
        String::from("根目录"),
    )];
    let mut seen_dirs = std::collections::HashSet::new();
    let mut fetched = 0_u64;
    let mut imported = 0_u64;
    let mut skipped = 0_u64;
    let mut skipped_promotional = 0_u64;
    let mut skipped_non_video = 0_u64;
    let mut folders = 0_u64;
    let mut truncated = false;

    while let Some((parent_id, page_token, folder_name)) = pending.pop() {
        if state.tasks_cancelled() {
            break;
        }
        let dir_key = format!(
            "{}:{}",
            parent_id.as_deref().unwrap_or("root"),
            page_token.as_deref().unwrap_or("")
        );
        if !seen_dirs.insert(dir_key) {
            continue;
        }
        let page = provider_list_files_impl(
            &provider_id,
            ProviderPageInput {
                parent_id: parent_id.clone(),
                page_token,
                page_size,
                query: None,
                mark_adult: None,
                folder_path: None,
            },
            &state,
        )
        .await?;
        fetched = fetched.saturating_add(page.files.len() as u64);
        for item in &page.files {
            if fetched > max_items {
                truncated = true;
                break;
            }
            match item.kind {
                MediaKind::Folder => {
                    folders = folders.saturating_add(1);
                    if is_promotional_name(&item.name) {
                        skipped = skipped.saturating_add(1);
                        skipped_promotional = skipped_promotional.saturating_add(1);
                        continue;
                    }
                    let child_path = if folder_name == "根目录" {
                        item.name.clone()
                    } else {
                        format!("{folder_name}/{}", item.name)
                    };
                    pending.push((Some(item.id.clone()), None, child_path));
                }
                MediaKind::File if is_promotional_media_item(&provider_id, item) => {
                    skipped = skipped.saturating_add(1);
                    skipped_promotional = skipped_promotional.saturating_add(1);
                }
                MediaKind::File if is_video_media_item(&provider_id, item) => {
                    if let Some(mut media) =
                        provider_media_record(&provider_id, item, Some(folder_name.as_str()))
                    {
                        apply_import_adult_choice(&mut media, mark_adult);
                        state.database.upsert_media(&media).map_err(app_error)?;
                        imported = imported.saturating_add(1);
                    } else {
                        skipped = skipped.saturating_add(1);
                    }
                }
                MediaKind::File => {
                    skipped = skipped.saturating_add(1);
                    skipped_non_video = skipped_non_video.saturating_add(1);
                }
            }
        }
        if truncated {
            break;
        }
        if let Some(next) = page.next_page_token {
            pending.push((parent_id, Some(next), folder_name.clone()));
        }
        // Live progress tick per directory page: the import phase runs for
        // minutes on large clouds, and without these events the frontend
        // progress card sits at zero the whole time.
        let _ = app.emit(
            "library://scan-progress",
            &serde_json::json!({
                "taskKey": task_key,
                "currentFolder": folder_name,
                "fetched": fetched,
                "imported": imported,
                "skipped": skipped,
                "skippedPromotional": skipped_promotional,
                "skippedNonVideo": skipped_non_video,
                "folders": folders,
                "truncated": truncated,
            }),
        );
    }
    Ok(ProviderRecursiveSyncReport {
        provider_id,
        fetched,
        imported,
        skipped,
        skipped_promotional,
        skipped_non_video,
        folders,
        truncated,
    })
}

fn apply_import_adult_choice(media: &mut MediaRecord, mark_adult: Option<bool>) {
    match mark_adult {
        Some(true) => set_manual_adult(media, true),
        Some(false) => {}
        None => {
            apply_adult_classification(media);
        }
    }
}

fn provider_media_record(
    provider_id: &str,
    item: &MediaItem,
    folder_path: Option<&str>,
) -> Option<MediaRecord> {
    let metadata = item.metadata.clone();
    let nested = metadata.get("streamhub").unwrap_or(&metadata);
    let id = format!("provider:{provider_id}:{}", item.id);
    let mut media = MediaRecord::new(id, "video", item.name.clone());
    media.source_type = format!("provider:{provider_id}");
    media.art_url = item.thumbnail_url.clone();
    media.duration_seconds = item
        .duration_seconds
        .map(|value| value.max(0.0).round() as i64);
    media.year = nested.get("year").and_then(serde_json::Value::as_i64);
    media.rating = nested.get("rating").and_then(serde_json::Value::as_f64);
    media.sort_key = Some(item.name.to_lowercase());
    let folder_path = folder_path
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "根目录");
    let folder_name = folder_path.and_then(|path| {
        path.rsplit(['/', '\\'])
            .find(|part| !part.is_empty() && *part != "根目录")
            .map(str::to_string)
    });
    if let Some(path) = folder_path {
        media.remote_path = Some(format!("{}/{}", path.trim_end_matches(['/', '\\']), item.name));
    }
    media.payload = Some(serde_json::json!({
        "providerId": provider_id,
        "mediaId": item.id,
        "metadata": metadata,
        "folderPath": folder_path,
        "folderName": folder_name,
    }));
    Some(media)
}

fn is_video_media_item(provider_id: &str, item: &MediaItem) -> bool {
    if provider_id != "guangya" {
        return true;
    }
    if item
        .mime_type
        .as_deref()
        .map(|mime| mime.trim().to_ascii_lowercase().starts_with("video/"))
        .unwrap_or(false)
    {
        return true;
    }
    let name = item.name.to_ascii_lowercase();
    const VIDEO_EXTENSIONS: &[&str] = &[
        ".mp4", ".mkv", ".webm", ".avi", ".mov", ".m4v", ".ts", ".m2ts", ".flv", ".wmv", ".rm",
        ".rmvb", ".3gp", ".mpeg", ".mpg", ".vob", ".ogv",
    ];
    VIDEO_EXTENSIONS
        .iter()
        .any(|extension| name.ends_with(extension))
        || item
            .metadata
            .get("mediaType")
            .and_then(serde_json::Value::as_str)
            .map(|value| value.to_ascii_lowercase().contains("video"))
            .unwrap_or(false)
}

fn is_promotional_media_item(_provider_id: &str, item: &MediaItem) -> bool {
    is_promotional_name(&item.name)
        || is_promotional_metadata(&item.metadata)
        || ["path", "remotePath", "fileName", "filename", "url"]
            .iter()
            .filter_map(|key| item.metadata.get(*key).and_then(serde_json::Value::as_str))
            .any(is_promotional_path)
}

async fn provider_resolve_playback_impl(
    provider_id: &str,
    request: PlaybackRequest,
    state: &AppState,
) -> CommandResult<PlaybackDescriptor> {
    if state
        .database
        .get_media(&request.media_id)
        .map_err(app_error)?
        .is_some_and(|media| is_promotional_media_record(&media))
    {
        return Err(app_error(AppError::InvalidInput(
            "promotional media cannot be played".into(),
        )));
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let started = std::time::Instant::now();
    restore_saved_session(state, provider_id).await?;
    let result = state
        .providers
        .resolve_playback(provider_id, request.clone())
        .await;
    let mut retry_count = 0;
    let result = match result {
        Ok(descriptor) => Ok(descriptor),
        Err(ProviderError::SessionExpired) => {
            retry_count = 1;
            retry_after_session_expired(state, provider_id).await?;
            state.providers.resolve_playback(provider_id, request).await
        }
        Err(error) => Err(error),
    };
    record_provider_result(
        "resolve_playback",
        &request_id,
        provider_id,
        started.elapsed(),
        retry_count,
        &result,
    );
    result.map_err(provider_error)
}

fn record_provider_result<T>(
    operation: &'static str,
    request_id: &str,
    provider_id: &str,
    duration: std::time::Duration,
    retry_count: u32,
    result: &Result<T, ProviderError>,
) {
    let (outcome, error_code) = match result {
        Ok(_) => ("ok", None),
        Err(error) => ("error", Some(error.code())),
    };
    record_operation(
        "provider",
        operation,
        request_id,
        Some(provider_id),
        outcome,
        duration,
        retry_count,
        error_code,
    );
}

#[tauri::command]
async fn guangya_device_code(state: State<'_, AppState>) -> CommandResult<DeviceCode> {
    provider_device_code("guangya".into(), state).await
}

#[tauri::command]
async fn guangya_poll(
    request: DevicePollRequest,
    state: State<'_, AppState>,
) -> CommandResult<PollResultSummary> {
    provider_poll("guangya".into(), request, state).await
}

#[tauri::command]
async fn guangya_sms_login(
    request: SmsLoginRequest,
    state: State<'_, AppState>,
) -> CommandResult<SessionSummary> {
    provider_sms_login("guangya".into(), request, state).await
}

#[tauri::command]
async fn guangya_list_files(
    input: ProviderPageInput,
    state: State<'_, AppState>,
) -> CommandResult<FilePage> {
    provider_list_files("guangya".into(), input, state).await
}

#[tauri::command]
async fn guangya_resolve_playback(
    request: PlaybackRequest,
    state: State<'_, AppState>,
) -> CommandResult<PlaybackDescriptor> {
    provider_resolve_playback("guangya".into(), request, state).await
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMediaInput {
    pub media_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSubtitleSearchInput {
    pub media_id: String,
    /// 视频文件名(非标题);光鸭在线字幕库按文件名匹配。
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSubtitleDownloadInput {
    /// search 返回的完整字幕条目原样回传,避免前端解析 id 约定。
    pub subtitle: ProviderSubtitle,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSubtitleDownloadedPath {
    pub path: String,
    pub source: String,
    pub name: String,
}

#[tauri::command]
async fn provider_video_qualities(
    provider_id: String,
    input: ProviderMediaInput,
    state: State<'_, AppState>,
) -> CommandResult<Vec<VideoQuality>> {
    restore_saved_session(&state, &provider_id).await?;
    let result = state
        .providers
        .video_qualities(&provider_id, &input.media_id)
        .await;
    let result = match result {
        Ok(qualities) => Ok(qualities),
        Err(ProviderError::SessionExpired) => {
            retry_after_session_expired(&state, &provider_id).await?;
            state
                .providers
                .video_qualities(&provider_id, &input.media_id)
                .await
        }
        Err(error) => Err(error),
    };
    result.map_err(provider_error)
}

#[tauri::command]
async fn provider_subtitle_search(
    provider_id: String,
    input: ProviderSubtitleSearchInput,
    state: State<'_, AppState>,
) -> CommandResult<Vec<ProviderSubtitle>> {
    restore_saved_session(&state, &provider_id).await?;
    let request = ProviderSubtitleSearchRequest {
        media_id: input.media_id,
        name: input.name,
        duration_seconds: input.duration_seconds,
    };
    let result = state
        .providers
        .search_subtitles(&provider_id, request.clone())
        .await;
    let result = match result {
        Ok(subtitles) => Ok(subtitles),
        Err(ProviderError::SessionExpired) => {
            retry_after_session_expired(&state, &provider_id).await?;
            state
                .providers
                .search_subtitles(&provider_id, request)
                .await
        }
        Err(error) => Err(error),
    };
    result.map_err(provider_error)
}

#[tauri::command]
async fn provider_subtitle_download(
    provider_id: String,
    input: ProviderSubtitleDownloadInput,
    state: State<'_, AppState>,
) -> CommandResult<ProviderSubtitleDownloadedPath> {
    restore_saved_session(&state, &provider_id).await?;
    let subtitle = input.subtitle;
    let result = state
        .providers
        .download_subtitle(&provider_id, &subtitle)
        .await;
    let downloaded = match result {
        Ok(downloaded) => downloaded,
        Err(ProviderError::SessionExpired) => {
            retry_after_session_expired(&state, &provider_id).await?;
            match state
                .providers
                .download_subtitle(&provider_id, &subtitle)
                .await
            {
                Ok(downloaded) => downloaded,
                Err(error) => return Err(provider_error(error)),
            }
        }
        Err(error) => return Err(provider_error(error)),
    };
    let cache_dir = state
        .paths
        .cache_dir
        .join("subtitles")
        .join(format!("provider-{provider_id}"));
    std::fs::create_dir_all(&cache_dir)
        .map_err(|error| app_error(AppError::Storage(format!("subtitle cache: {error}"))))?;
    let target = cache_dir.join(&downloaded.file_name);
    std::fs::write(&target, &downloaded.bytes)
        .map_err(|error| app_error(AppError::Storage(format!("subtitle cache write: {error}"))))?;
    Ok(ProviderSubtitleDownloadedPath {
        path: target.display().to_string(),
        source: subtitle.source,
        name: subtitle.name,
    })
}

#[tauri::command]
fn media_probe(input: MediaProbeInput, state: State<'_, AppState>) -> CommandResult<MediaProbe> {
    if input.url.trim().is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "media URL cannot be empty".into(),
        )));
    }
    probe_media(&state, &input.url, &input.headers).map_err(app_error)
}

#[tauri::command]
fn playback_plan(
    input: MediaProbeInput,
    state: State<'_, AppState>,
) -> CommandResult<PlaybackPlan> {
    if input.url.trim().is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "media URL cannot be empty".into(),
        )));
    }
    let probe = probe_media(&state, &input.url, &input.headers).map_err(app_error)?;
    Ok(plan_playback(&probe, input.headers))
}

#[tauri::command]
fn player_tracks(input: MediaProbeInput, state: State<'_, AppState>) -> CommandResult<MediaProbe> {
    media_probe(input, state)
}

#[tauri::command]
fn player_audio_capabilities() -> AudioOutputCapabilities {
    audio_output_capabilities()
}

#[tauri::command]
async fn subtitle_search(
    input: SubtitleSearchInput,
    state: State<'_, AppState>,
) -> CommandResult<Vec<SubtitleSearchResult>> {
    let probe = probe_media(&state, &input.url, &input.headers).map_err(app_error)?;
    let mut results = probe
        .external_subtitles
        .into_iter()
        .filter(|path| {
            input.language.as_deref().is_none_or(|language| {
                path.to_ascii_lowercase()
                    .contains(&language.to_ascii_lowercase())
            })
        })
        .map(|path| SubtitleSearchResult {
            path,
            language: input.language.clone(),
            source: "local".into(),
            downloaded: true,
            file_id: None,
            release: None,
        })
        .collect::<Vec<_>>();

    let api_key = subtitle_api_key(&state)?;
    let Some(api_key) = api_key else {
        return Ok(results);
    };
    let query = input
        .query
        .clone()
        .or_else(|| {
            Path::new(&input.url)
                .file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| input.url.clone());
    let client = reqwest::Client::builder()
        .user_agent("TTV-Box/0.1")
        .build()
        .map_err(|error| app_error(AppError::Runtime(format!("subtitle client: {error}"))))?;
    let mut request = client
        .get("https://api.opensubtitles.com/api/v1/subtitles")
        .header("Api-Key", api_key)
        .header("Content-Type", "application/json")
        .query(&[
            ("query", query.as_str()),
            ("languages", input.language.as_deref().unwrap_or("en")),
        ]);
    if let Some(hash) = input
        .file_hash
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        request = request.query(&[("moviehash", hash)]);
    }
    if let Some(season) = input.season {
        request = request.query(&[("season_number", &season.to_string())]);
    }
    if let Some(episode) = input.episode {
        request = request.query(&[("episode_number", &episode.to_string())]);
    }
    if let Some(year) = input.year {
        request = request.query(&[("year", &year.to_string())]);
    }
    if let Some(year) = probe.nfo.get("year") {
        request = request.query(&[("year", year.as_str())]);
    }
    let response = request.send().await.map_err(|error| {
        app_error(AppError::Provider(format!(
            "subtitle search failed: {error}"
        )))
    })?;
    if response.status().is_success() {
        let payload: serde_json::Value = response.json().await.map_err(|error| {
            app_error(AppError::Provider(format!(
                "subtitle response invalid: {error}"
            )))
        })?;
        if let Some(items) = payload.get("data").and_then(serde_json::Value::as_array) {
            results.extend(items.iter().filter_map(|item| {
                let attributes = item.get("attributes")?;
                let language = attributes
                    .get("language")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                let file_id = attributes
                    .get("files")?
                    .as_array()?
                    .first()?
                    .get("file_id")?
                    .as_i64()?
                    .to_string();
                Some(SubtitleSearchResult {
                    path: String::new(),
                    language,
                    source: "opensubtitles".into(),
                    downloaded: false,
                    file_id: Some(file_id),
                    release: attributes
                        .get("release")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                })
            }));
        }
    }
    Ok(results)
}

#[tauri::command]
async fn subtitle_download(
    input: SubtitleDownloadInput,
    state: State<'_, AppState>,
) -> CommandResult<SubtitleSearchResult> {
    if input.file_id.trim().is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "subtitle file id is empty".into(),
        )));
    }
    let api_key = subtitle_api_key(&state)?.ok_or_else(|| {
        app_error(AppError::Provider(
            "OpenSubtitles API key is not configured".into(),
        ))
    })?;
    let client = reqwest::Client::builder()
        .user_agent("TTV-Box/0.1")
        .build()
        .map_err(|error| app_error(AppError::Runtime(format!("subtitle client: {error}"))))?;
    let mut payload: Option<serde_json::Value> = None;
    let mut last_error = None;
    for attempt in 0..3 {
        match client
            .post("https://api.opensubtitles.com/api/v1/download")
            .header("Api-Key", &api_key)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({"file_id": input.file_id}))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => match response.json().await {
                Ok(value) => {
                    payload = Some(value);
                    break;
                }
                Err(error) => last_error = Some(format!("subtitle response invalid: {error}")),
            },
            Ok(response) => {
                last_error = Some(format!(
                    "subtitle download returned HTTP {}",
                    response.status()
                ));
            }
            Err(error) => last_error = Some(format!("subtitle download failed: {error}")),
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(250 * (attempt + 1) as u64)).await;
        }
    }
    let payload = payload.ok_or_else(|| {
        app_error(AppError::Provider(
            last_error.unwrap_or_else(|| "subtitle download failed".into()),
        ))
    })?;
    let link = payload
        .get("link")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            app_error(AppError::Provider(
                "OpenSubtitles did not return a download link".into(),
            ))
        })?;
    let mut bytes: Option<Vec<u8>> = None;
    let mut last_error = None;
    for attempt in 0..3 {
        match client.get(link).send().await {
            Ok(response) if response.status().is_success() => match response.bytes().await {
                Ok(value) => {
                    bytes = Some(value.to_vec());
                    break;
                }
                Err(error) => last_error = Some(format!("subtitle file read failed: {error}")),
            },
            Ok(response) => {
                last_error = Some(format!(
                    "subtitle file request returned HTTP {}",
                    response.status()
                ));
            }
            Err(error) => last_error = Some(format!("subtitle file request failed: {error}")),
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(250 * (attempt + 1) as u64)).await;
        }
    }
    let bytes = bytes.ok_or_else(|| {
        app_error(AppError::Provider(
            last_error.unwrap_or_else(|| "subtitle file request failed".into()),
        ))
    })?;
    let cache_dir = state.paths.cache_dir.join("subtitles");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|error| app_error(AppError::Storage(format!("subtitle cache: {error}"))))?;
    let media_prefix = input.media_id.clone().unwrap_or_else(|| "media".into());
    let requested_name = input
        .file_name
        .unwrap_or_else(|| format!("{}.srt", input.file_id));
    let safe_name = Path::new(&requested_name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("subtitle.srt");
    let target = cache_dir.join(format!("{}-{}", media_prefix, safe_name));
    let extracted = extract_subtitle_payload(&bytes, safe_name, &cache_dir, &media_prefix)
        .map_err(app_error)?;
    let target = extracted.unwrap_or(target);
    if !target.is_file() {
        std::fs::write(&target, bytes).map_err(|error| {
            app_error(AppError::Storage(format!("subtitle cache write: {error}")))
        })?;
    }
    Ok(SubtitleSearchResult {
        path: target.display().to_string(),
        language: input.language,
        source: "opensubtitles".into(),
        downloaded: true,
        file_id: Some(input.file_id),
        release: None,
    })
}

fn extract_subtitle_payload(
    bytes: &[u8],
    fallback_name: &str,
    cache_dir: &Path,
    media_prefix: &str,
) -> Result<Option<PathBuf>, AppError> {
    if !bytes.starts_with(b"PK\x03\x04") {
        return Ok(None);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| AppError::Storage(format!("subtitle archive is invalid: {error}")))?;
    let mut selected = None;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| AppError::Storage(format!("subtitle archive entry: {error}")))?;
        let entry_name = file.name().to_owned();
        let ext = Path::new(&entry_name)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "srt" | "ass" | "ssa" | "vtt" | "sub" | "sup") {
            continue;
        }
        let safe = Path::new(&entry_name)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(fallback_name);
        let target = cache_dir.join(format!("{}-{}", media_prefix, safe));
        let mut output = Vec::new();
        file.read_to_end(&mut output)
            .map_err(|error| AppError::Storage(format!("subtitle archive read: {error}")))?;
        std::fs::write(&target, output)
            .map_err(|error| AppError::Storage(format!("subtitle archive extract: {error}")))?;
        selected = Some(target);
        break;
    }
    Ok(selected)
}

fn subtitle_api_key(state: &AppState) -> CommandResult<Option<String>> {
    if let Some(value) = CredentialStore::new(&state.database)
        .load_json::<String>("subtitle.opensubtitles.api_key")
        .map_err(app_error)?
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(Some(value));
    }
    Ok(std::env::var("TTV_OPENSUBTITLES_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty()))
}

#[tauri::command]
fn subtitle_credentials_set(
    input: SubtitleCredentialsInput,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    if input.api_key.trim().is_empty() || input.api_key.contains('\0') {
        return Err(app_error(AppError::InvalidInput(
            "subtitle API key cannot be empty".into(),
        )));
    }
    CredentialStore::new(&state.database)
        .save_json("subtitle.opensubtitles.api_key", &input.api_key)
        .map_err(app_error)
}

#[tauri::command]
fn subtitle_credentials_clear(state: State<'_, AppState>) -> CommandResult<bool> {
    CredentialStore::new(&state.database)
        .delete("subtitle.opensubtitles.api_key")
        .map_err(app_error)
}

#[tauri::command]
fn subtitle_import(
    input: SubtitleImportInput,
    state: State<'_, AppState>,
) -> CommandResult<String> {
    let file_name = Path::new(&input.file_name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("subtitle.srt");
    let extension = Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("srt")
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "srt" | "ass" | "ssa" | "vtt" | "sub") {
        return Err(app_error(AppError::InvalidInput(
            "unsupported subtitle format".into(),
        )));
    }
    if input.content.trim().is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "subtitle content is empty".into(),
        )));
    }
    let cache_dir = state.paths.cache_dir.join("subtitles");
    std::fs::create_dir_all(&cache_dir).map_err(app_error_io)?;
    let safe_name = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let path = cache_dir.join(format!(
        "import-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        safe_name
    ));
    std::fs::write(&path, input.content.as_bytes()).map_err(app_error_io)?;
    let path_text = path.display().to_string();
    subtitle_attach(
        SubtitleAttachInput {
            path: path_text.clone(),
            select: input.select,
        },
        state,
    )?;
    Ok(path_text)
}

#[tauri::command]
fn subtitle_attach(input: SubtitleAttachInput, state: State<'_, AppState>) -> CommandResult<()> {
    let supported = Path::new(&input.path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "srt" | "ass" | "ssa" | "vtt" | "sub" | "sup"
            )
        })
        .unwrap_or(false);
    if input.path.trim().is_empty()
        || input.path.contains('\0')
        || !Path::new(&input.path).is_file()
        || !supported
    {
        return Err(app_error(AppError::InvalidInput(
            "subtitle path is invalid".into(),
        )));
    }
    let command = crate::playback::PlaybackCommand::RawCommand {
        args: vec![
            "sub-add".into(),
            input.path,
            if input.select {
                "select".into()
            } else {
                "auto".into()
            },
        ],
    };
    player_command(command, state)
}

#[tauri::command]
fn subtitle_cache_cleanup(
    max_age_days: Option<u64>,
    state: State<'_, AppState>,
) -> CommandResult<u64> {
    let root = state.paths.cache_dir.join("subtitles");
    if !root.is_dir() {
        return Ok(0);
    }
    let max_age = Duration::from_secs(max_age_days.unwrap_or(30).clamp(1, 365) * 86_400);
    let now = SystemTime::now();
    let mut removed = 0_u64;
    for entry in std::fs::read_dir(root).map_err(app_error_io)?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if stale && std::fs::remove_file(path).is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    Ok(removed)
}

#[tauri::command]
fn subtitle_remove(input: SubtitleRemoveInput, state: State<'_, AppState>) -> CommandResult<()> {
    if input.track_id < 0 {
        return Err(app_error(AppError::InvalidInput(
            "subtitle track id is invalid".into(),
        )));
    }
    if input.track_id == 0 {
        player_command(
            crate::playback::PlaybackCommand::SetSubtitleTrack { track_id: None },
            state,
        )
    } else {
        player_command(
            crate::playback::PlaybackCommand::RawCommand {
                args: vec!["sub-remove".into(), input.track_id.to_string()],
            },
            state,
        )
    }
}

#[tauri::command]
fn player_state(state: State<'_, AppState>) -> CommandResult<PlaybackSnapshot> {
    let playback = state
        .playback
        .lock()
        .map_err(|_| AppError::Playback("playback state lock poisoned".into()))?;
    playback
        .as_ref()
        .map(|actor| actor.snapshot())
        .ok_or_else(|| app_error(AppError::Playback("playback engine is unavailable".into())))
}

#[tauri::command]
fn player_capabilities(state: State<'_, AppState>) -> PlayerCapabilities {
    PlayerCapabilities {
        libmpv: state.runtime.playback_available,
        hdr: state.runtime.playback_available,
        shaders: state.runtime.upscaling_available,
        interpolation: state.runtime.interpolation_available,
        playlist: state.runtime.playback_available,
        external_players: !discover_external_players().is_empty(),
        controls: vec![
            "play-pause".into(),
            "seek".into(),
            "volume".into(),
            "speed".into(),
            "audio-track".into(),
            "subtitle-track".into(),
            "screenshot".into(),
            "playlist".into(),
            "fullscreen".into(),
            "always-on-top".into(),
        ],
    }
}

#[tauri::command]
fn player_command(command: PlaybackCommand, state: State<'_, AppState>) -> CommandResult<()> {
    let playback = state
        .playback
        .lock()
        .map_err(|_| AppError::Playback("playback state lock poisoned".into()))?;
    playback
        .as_ref()
        .ok_or_else(|| app_error(AppError::Playback("playback engine is unavailable".into())))?
        .dispatch(command)
        .map_err(app_error)
}

#[tauri::command]
fn player_load(input: PlayerLoadInput, state: State<'_, AppState>) -> CommandResult<()> {
    if input.url.trim().is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "playback URL cannot be empty".into(),
        )));
    }
    if let Some(media_id) = input.media_id.as_deref() {
        if state
            .database
            .get_media(media_id)
            .map_err(app_error)?
            .is_some_and(|media| is_promotional_media_record(&media))
        {
            return Err(app_error(AppError::InvalidInput(
                "promotional media cannot be played".into(),
            )));
        }
    }
    if matches!(input.mode, Some(PlaybackMode::External)) {
        let external = discover_external_players().into_iter().next().ok_or_else(|| {
            app_error(AppError::Playback(
                "external playback was requested, but no supported external player is installed".into(),
            ))
        })?;
        external_player_open(ExternalPlayerOpenInput {
            player_id: external.id,
            url: input.url,
            headers: input.headers,
        })?;
        return Ok(());
    }
    // libmpv can discover tracks while opening the media. A synchronous
    // FFprobe here makes remote MP4/MKV files wait for metadata before the
    // player even receives the signed URL, so only use probe data the caller
    // already has and let playback start immediately otherwise.
    let probe = input.media.clone();
    let selected_audio = input.audio_track.or_else(|| {
        probe
            .as_ref()
            .and_then(|value| choose_audio_track(value, input.preferred_audio_language.as_deref()))
    });
    let selected_subtitle = input.subtitle_track.or_else(|| {
        probe.as_ref().and_then(|value| {
            value
                .subtitles
                .iter()
                .find(|track| {
                    input
                        .preferred_subtitle_language
                        .as_deref()
                        .is_some_and(|language| {
                            track
                                .language
                                .as_deref()
                                .is_some_and(|value| value.eq_ignore_ascii_case(language))
                        })
                })
                .or_else(|| value.subtitles.iter().find(|track| track.default))
                .map(|track| track.index)
        })
    });
    let history_position = input
        .media_id
        .as_deref()
        .map(|media_id| {
            state
                .database
                .latest_watch_history(media_id)
                .map_err(app_error)
        })
        .transpose()?
        .flatten()
        .filter(|history| !history.completed)
        .and_then(|history| {
            if !history.position_seconds.is_finite() || history.position_seconds <= 0.0 {
                return None;
            }
            let position = if history.duration_seconds.is_finite() && history.duration_seconds > 0.0
            {
                history.position_seconds.min(history.duration_seconds)
            } else {
                history.position_seconds
            };
            (position > 0.0).then_some(position)
        });
    let resume_position = input
        .resume_position_seconds
        .filter(|value| value.is_finite() && *value >= 0.0)
        .or(history_position);

    let playback = state
        .playback
        .lock()
        .map_err(|_| AppError::Playback("playback state lock poisoned".into()))?;
    let actor = playback
        .as_ref()
        .ok_or_else(|| app_error(AppError::Playback("playback engine is unavailable".into())))?;
    actor.dispatch(PlaybackCommand::Load {
        media_id: input.media_id,
        url: input.url,
        headers: input.headers,
        decryption_key: input.decryption_key.clone(),
        resume_position_seconds: resume_position,
        audio_track: selected_audio,
        subtitle_track: selected_subtitle,
    })?;
    actor.set_playback_backend(match input.mode {
        Some(PlaybackMode::Browser) => "browser",
        Some(PlaybackMode::Transcode) => "ffmpeg-hls",
        Some(PlaybackMode::External) => "external",
        _ => "libmpv-headless",
    });
    if let Some(hdr) = input.hdr {
        actor.dispatch(PlaybackCommand::RawCommand {
            args: vec![
                "set".into(),
                "target-colorspace-hint".into(),
                if hdr { "yes".into() } else { "no".into() },
            ],
        })?;
    }
    if let Some(interpolation) = input.interpolation {
        actor.dispatch(PlaybackCommand::SetFrameInterpolation {
            enabled: interpolation,
            mode: "display-resample".into(),
        })?;
    }
    Ok(())
}

#[tauri::command]
async fn player_native_open<R: Runtime>(
    app: AppHandle<R>,
    input: NativePlayerOpenInput,
    state: State<'_, AppState>,
) -> CommandResult<bool> {
    #[cfg(not(windows))]
    {
        let _ = (app, input, state);
        return Ok(false);
    }
    #[cfg(windows)]
    {
        if input.url.trim().is_empty() {
            return Err(app_error(AppError::InvalidInput(
                "native player URL cannot be empty".into(),
            )));
        }
        // Async commands run on the runtime instead of the main thread, so the
        // builder can dispatch creation to the event loop. A sync command here
        // deadlocks: build() waits on the very loop that is blocked on us, and
        // every later IPC (including closing the window) queues behind it.
        //
        // 单窗口嵌入（对齐参考实现 LumiPlayer 的 wid 方案）：libmpv 直接渲染到
        // 主窗口 WebView2 之下的子表面。主 webview 在 tauri.conf.json 中透明，
        // 前端播放视图把画布清空后视频即可透出，控件仍按普通 DOM 叠加。
        let window = app
            .get_webview_window("main")
            .ok_or_else(|| app_error(AppError::Playback("main window unavailable".into())))?;
        attach_mpv_surface_sync(app.clone());
        let started = open_native_mpv(input, &state, &window)?;
        if started {
            schedule_mpv_surface_fit(&window);
        }
        Ok(started)
    }
}

/// mpv embeds its video surface as a child window created after the WebView2
/// sibling, which lands it above the HTML overlay. Pin every mpv child to the
/// bottom of the player's z-order so the transparent overlay stays on top and
/// the video shows through it.
#[cfg(windows)]
fn pin_mpv_surface_below_overlay(parent_hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    const HWND_BOTTOM: isize = 1;
    let mpv_class: [u16; 4] = [0x6D, 0x70, 0x76, 0]; // "mpv"
    let mut after: isize = 0;
    unsafe {
        loop {
            let child = FindWindowExW(
                parent_hwnd as _,
                after as _,
                mpv_class.as_ptr(),
                std::ptr::null(),
            );
            if child.is_null() {
                break;
            }
            SetWindowPos(
                child,
                HWND_BOTTOM as _,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            after = child as isize;
        }
    }
}

/// 去重标记：保证主窗口的 resize→mpv 尺寸监听只装一次，避免多次打开播放时叠加重复回调。
#[cfg(windows)]
static MPV_SURFACE_SYNC_ATTACHED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 把 `parent_hwnd` 下所有 mpv 子表面移动到其客户区大小。Windows 上 libmpv
/// 的嵌入子窗口不会自动跟随宿主缩放，几何必须由宿主负责（启动适配 + resize）。
#[cfg(windows)]
fn fit_mpv_surface_to_client(parent_hwnd: isize) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetClientRect, MoveWindow};
    const MPV_CLASS: [u16; 4] = [0x6D, 0x70, 0x76, 0]; // "mpv"
    unsafe {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(parent_hwnd as _, &mut rect) == 0 {
            return;
        }
        let width = (rect.right - rect.left).max(1);
        let height = (rect.bottom - rect.top).max(1);
        let mut after: isize = 0;
        loop {
            let child = FindWindowExW(
                parent_hwnd as _,
                after as _,
                MPV_CLASS.as_ptr(),
                std::ptr::null(),
            );
            if child.is_null() {
                break;
            }
            MoveWindow(child, 0, 0, width, height, 1);
            after = child as isize;
        }
    }
}

/// 首次调用时在主窗口上安装 resize 监听：缩放/最大化/还原后重新钉底并适配
/// mpv 表面尺寸，保证单窗口嵌入时长播画面始终充满客户区。
#[cfg(windows)]
fn attach_mpv_surface_sync<R: Runtime>(app: AppHandle<R>) {
    use std::sync::atomic::Ordering;
    if MPV_SURFACE_SYNC_ATTACHED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let app_for_events = app.clone();
        window.on_window_event(move |event| {
            if matches!(event, WindowEvent::Resized(_)) {
                if let Some(main) = app_for_events.get_webview_window("main") {
                    if let Ok(hwnd) = main.hwnd() {
                        pin_mpv_surface_below_overlay(hwnd.0 as isize);
                        fit_mpv_surface_to_client(hwnd.0 as isize);
                    }
                }
            }
        });
    }
}

/// mpv 子表面在首个 loadfile 之后才异步创建，启动后分几次重试适配；
/// 之后由 resize 钩子接管几何同步。
#[cfg(windows)]
fn schedule_mpv_surface_fit<R: Runtime>(window: &WebviewWindow<R>) {
    let Ok(hwnd) = window.hwnd() else { return };
    let parent_hwnd = hwnd.0 as isize;
    let _ = std::thread::Builder::new()
        .name("mpv-surface-fit".into())
        .spawn(move || {
            for gap_ms in [250_u64, 750, 1_800] {
                std::thread::sleep(Duration::from_millis(gap_ms));
                pin_mpv_surface_below_overlay(parent_hwnd);
                fit_mpv_surface_to_client(parent_hwnd);
            }
        });
}

/// Dispatch a `Load` (and optional HDR / interpolation flags) onto an already
/// running native actor. Episode switches reuse this path so libmpv is not
/// torn down and rebuilt between files.
#[cfg(windows)]
fn load_into_existing_native_actor(
    actor: &PlaybackActor,
    input: &NativePlayerOpenInput,
    selected_audio: Option<i64>,
    selected_subtitle: Option<i64>,
) -> CommandResult<()> {
    actor.set_playback_backend("libmpv-native");
    actor
        .dispatch(PlaybackCommand::Load {
            media_id: input.media_id.clone(),
            url: input.url.clone(),
            headers: input.headers.clone(),
            decryption_key: input.decryption_key.clone(),
            resume_position_seconds: input.resume_position_seconds,
            audio_track: selected_audio,
            subtitle_track: selected_subtitle,
        })
        .map_err(app_error)?;
    // loadfile replace 不会清掉片尾 keep-open 留下的 pause=yes。
    actor.dispatch(PlaybackCommand::Play).map_err(app_error)?;
    if let Some(hdr) = input.hdr {
        actor
            .dispatch(PlaybackCommand::RawCommand {
                args: vec![
                    "set".into(),
                    "target-colorspace-hint".into(),
                    if hdr { "yes".into() } else { "no".into() },
                ],
            })
            .map_err(app_error)?;
    }
    if let Some(interpolation) = input.interpolation {
        actor
            .dispatch(PlaybackCommand::SetFrameInterpolation {
                enabled: interpolation,
                mode: "display-resample".into(),
            })
            .map_err(app_error)?;
    }
    Ok(())
}

/// Heavy half of `player_native_open`: resolve tracks, start the libmpv actor
/// and swap it into the shared playback slot. The window passed in is the
/// embed target (the main window); its HWND becomes mpv's `wid`.
#[cfg(windows)]
fn open_native_mpv<R: Runtime>(
    input: NativePlayerOpenInput,
    state: &State<'_, AppState>,
    window: &WebviewWindow<R>,
) -> CommandResult<bool> {
    let session_id = input.session_id.unwrap_or(0);
    let hwnd = window
        .hwnd()
        .map_err(|error| app_error(AppError::Playback(format!("native player handle: {error}"))))?;
    // Match the official client's fast path: hand the signed URL to the
    // native engine immediately instead of probing the remote file first.
    let probe = input.media.clone();
    let selected_audio = input.audio_track.or_else(|| {
        probe
            .as_ref()
            .and_then(|value| choose_audio_track(value, input.preferred_audio_language.as_deref()))
    });
    let selected_subtitle = input.subtitle_track.or_else(|| {
        probe.as_ref().and_then(|value| {
            input
                .preferred_subtitle_language
                .as_deref()
                .and_then(|language| {
                    value.subtitles.iter().find(|track| {
                        track
                            .language
                            .as_deref()
                            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(language))
                    })
                })
                .or_else(|| value.subtitles.iter().find(|track| track.default))
                .map(|track| track.index)
        })
    });
    {
        let playback = state
            .playback
            .lock()
            .map_err(|_| app_error(AppError::Playback("playback state lock poisoned".into())))?;
        if let Some(actor) = playback.as_ref() {
            if actor.snapshot().playback_backend.as_deref() == Some("libmpv-native") {
                let interpolation_already = actor.snapshot().interpolation_enabled;
                load_into_existing_native_actor(
                    actor,
                    &input,
                    selected_audio,
                    selected_subtitle,
                )?;
                state
                    .native_playback_session
                    .store(session_id, std::sync::atomic::Ordering::Release);
                drop(playback);
                pin_mpv_surface_below_overlay(hwnd.0 as isize);
                schedule_mpv_surface_fit(window);
                if let Ok(mode) = persisted_enhancement_mode(state) {
                    let want_interpolation = matches!(mode, 2 | 3 | 5);
                    if want_interpolation != interpolation_already {
                        let _ = apply_enhancement_plan(state, mode);
                    }
                }
                return Ok(true);
            }
        }
    }
    let mut roots = vec![
        state.paths.resource_dir.join("mpv"),
        state.paths.resource_dir.clone(),
    ];
    if let Some(resource_dir) = discover_resource_dir(Some(&state.paths.resource_dir)) {
        roots.push(resource_dir.join("mpv"));
        roots.push(resource_dir.clone());
    }
    if let Some(path) = state
        .runtime
        .resource(RuntimeResourceKind::LibMpv)
        .and_then(|value| value.path.clone())
    {
        if let Some(parent) = path.parent() {
            roots.push(parent.to_owned());
        }
    }
    let backend = LibMpvBackend::load_from_roots(roots)
        .map_err(AppError::Playback)
        .map_err(app_error)?;
    let mut mpv_config = MpvConfig {
        window_id: Some(hwnd.0 as u64),
        ..MpvConfig::default()
    };
    if let Some(language) = input
        .preferred_audio_language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        mpv_config.options.insert("alang".into(), language.into());
    }
    if let Some(language) = input
        .preferred_subtitle_language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        mpv_config.options.insert("slang".into(), language.into());
    }
    if input
        .audio_passthrough
        .unwrap_or_else(|| audio_output_capabilities().passthrough)
    {
        mpv_config
            .options
            .insert("audio-spdif".into(), "ac3,eac3,dts,dts-hd,truehd".into());
    }

    let actor = PlaybackActor::start(backend, mpv_config).map_err(app_error)?;
    pin_mpv_surface_below_overlay(hwnd.0 as isize);
    load_into_existing_native_actor(&actor, &input, selected_audio, selected_subtitle)?;
    pin_mpv_surface_below_overlay(hwnd.0 as isize);
    let mut playback = state
        .playback
        .lock()
        .map_err(|_| app_error(AppError::Playback("playback state lock poisoned".into())))?;
    let previous = playback.replace(actor);
    state
        .native_playback_session
        .store(session_id, std::sync::atomic::Ordering::Release);
    drop(playback);
    if let Some(previous) = previous {
        previous.detach();
    }
    // 新播放 actor 启动后自动套用已保存的增强设置（补帧/超分/着色器），
    // 否则每次开视频都会从"全部关闭"重新开始，用户保存的开关等于失效。
    // 应用失败不阻断播放：增强属于可选能力，降级由状态反馈暴露。
    if let Ok(mode) = persisted_enhancement_mode(&state) {
        if mode != 0 {
            let _ = apply_enhancement_plan(&state, mode);
        }
    }
    Ok(true)
}

#[tauri::command]
fn player_native_close<R: Runtime>(
    app: AppHandle<R>,
    input: Option<NativePlayerCloseInput>,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    // 单窗口嵌入后不再有独立播放窗口；保留旧标签查找是为了兼容升级前
    // 残留的窗口，正常路径下取 None 直接跳过。回收靠 take 掉 actor：
    // PlaybackActor drop → mpv_terminate_destroy → 子视频窗口随之销毁。
    if let Some(window) = app.get_webview_window("native-player") {
        let _ = window.close();
    }
    let requested_session = input.and_then(|value| value.session_id);
    let mut playback = state
        .playback
        .lock()
        .map_err(|_| app_error(AppError::Playback("playback state lock poisoned".into())))?;
    let current_session = state
        .native_playback_session
        .load(std::sync::atomic::Ordering::Acquire);
    if let Some(session_id) = requested_session {
        // A zero token denotes an actor created by a pre-session build. Treat
        // it as unknown so a current UI close can still clean up legacy audio;
        // non-zero tokens remain strict to protect a newer playback session.
        if current_session != 0 && session_id != current_session {
            return Ok(());
        }
    }
    let is_native = playback
        .as_ref()
        .and_then(|actor| actor.snapshot().playback_backend)
        .as_deref()
        == Some("libmpv-native");
    if is_native {
        // Stop/Unload are queued before Shutdown. Detach instead of joining:
        // a stuck mpv_terminate_destroy must not freeze the UI thread.
        if let Some(actor) = playback.as_ref() {
            let _ = actor.dispatch(PlaybackCommand::Stop);
            let _ = actor.dispatch(PlaybackCommand::Unload);
        }
        if let Some(actor) = playback.take() {
            actor.detach();
        }
        state
            .native_playback_session
            .store(0, std::sync::atomic::Ordering::Release);
    }
    Ok(())
}

/// Convert formats that WebView2 commonly cannot decode (MKV, HEVC Main10,
/// TrueHD, etc.) into a browser-friendly H.264/AAC HLS stream. FFmpeg writes
/// short fragments so playback can begin without waiting for the whole movie.
#[tauri::command]
async fn player_prepare_browser_media(
    input: PlayerPrepareInput,
    state: State<'_, AppState>,
) -> CommandResult<PreparedPlayback> {
    let local_source = decode_local_media_path(&input.url);
    let remote_source = input.url.starts_with("http://") || input.url.starts_with("https://");
    if local_source.is_none() && !remote_source {
        return Err(app_error(AppError::InvalidInput(
            "browser preparation requires a local file or HTTP(S) media URL".into(),
        )));
    }
    let metadata = local_source
        .as_ref()
        .and_then(|source| std::fs::metadata(source).ok());
    if let Some(metadata) = &metadata {
        if !metadata.is_file() {
            return Err(app_error(AppError::InvalidInput(
                "media source is not a regular file".into(),
            )));
        }
    }

    let ffmpeg = state
        .runtime
        .resource(RuntimeResourceKind::Ffmpeg)
        .and_then(|resource| resource.path.clone())
        .or_else(|| {
            discover_resource_dir(Some(&state.paths.resource_dir))
                .map(|root| root.join("mpv/ffmpeg.exe"))
        })
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            app_error(AppError::Playback(
                "bundled FFmpeg is unavailable for browser fallback".into(),
            ))
        })?;

    let modified = metadata
        .as_ref()
        .and_then(|value| value.modified().ok())
        .and_then(|value| value.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|value| value.as_secs())
        .unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    // Bump this when the browser transcode stream layout changes so an older
    // cached HLS manifest without audio cannot be reused.
    "browser-hls-faststart-v3".hash(&mut hasher);
    input.url.hash(&mut hasher);
    for (name, value) in &input.headers {
        name.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    metadata
        .as_ref()
        .map(|value| value.len())
        .unwrap_or_default()
        .hash(&mut hasher);
    modified.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());
    let output_dir = state.paths.cache_dir.join("playback").join(&key);
    let manifest = output_dir.join("index.m3u8");
    if hls_manifest_ready(&output_dir) {
        return Ok(PreparedPlayback {
            url: manifest.to_string_lossy().into_owned(),
            cached: true,
            format: "hls".into(),
        });
    }

    // MKV is only a container. When its primary video stream is already a
    // browser-compatible 8-bit H.264 stream, keep the video bit-for-bit and
    // only normalize the audio/container. This starts much faster than
    // needlessly encoding the entire picture again.
    let ffmpeg_source = local_source
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.url.clone());
    let preferred_encoder = if source_has_browser_h264(&ffmpeg, &ffmpeg_source, &input.headers) {
        "copy"
    } else {
        preferred_h264_encoder(&ffmpeg)
    };
    let mut using_cpu_fallback = preferred_encoder == "libx264";
    let mut already_running = false;
    {
        let mut jobs = state.browser_transcode_jobs.lock().map_err(|_| {
            app_error(AppError::Playback(
                "browser transcode job lock poisoned".into(),
            ))
        })?;
        if let Some(child) = jobs.get_mut(&key) {
            if child.try_wait().map_err(app_error_io)?.is_none() {
                already_running = true;
            } else {
                jobs.remove(&key);
            }
        }
        if !already_running {
            let _ = std::fs::remove_dir_all(&output_dir);
            let child = spawn_hls_transcode(
                &ffmpeg,
                &ffmpeg_source,
                &input.headers,
                &output_dir,
                preferred_encoder,
            )
            .map_err(app_error)?;
            jobs.insert(key.clone(), child);
        }
    }

    for _ in 0..120 {
        if hls_manifest_ready(&output_dir) {
            return Ok(PreparedPlayback {
                url: manifest.to_string_lossy().into_owned(),
                cached: false,
                format: "hls".into(),
            });
        }

        let exited = {
            let mut jobs = state.browser_transcode_jobs.lock().map_err(|_| {
                app_error(AppError::Playback(
                    "browser transcode job lock poisoned".into(),
                ))
            })?;
            match jobs.get_mut(&key) {
                Some(child) => child.try_wait().map_err(app_error_io)?,
                None => None,
            }
        };
        if let Some(status) = exited {
            state
                .browser_transcode_jobs
                .lock()
                .map_err(|_| {
                    app_error(AppError::Playback(
                        "browser transcode job lock poisoned".into(),
                    ))
                })?
                .remove(&key);
            if !status.success() && !using_cpu_fallback {
                using_cpu_fallback = true;
                let _ = std::fs::remove_dir_all(&output_dir);
                let child = spawn_hls_transcode(
                    &ffmpeg,
                    &ffmpeg_source,
                    &input.headers,
                    &output_dir,
                    "libx264",
                )
                .map_err(app_error)?;
                state
                    .browser_transcode_jobs
                    .lock()
                    .map_err(|_| {
                        app_error(AppError::Playback(
                            "browser transcode job lock poisoned".into(),
                        ))
                    })?
                    .insert(key.clone(), child);
            } else {
                return Err(app_error(AppError::Playback(
                    "FFmpeg could not start the browser-compatible video stream".into(),
                )));
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    Err(app_error(AppError::Playback(
        "timed out preparing the browser-compatible video stream".into(),
    )))
}

/// Capture a small seek-preview frame without exposing a local path or a
/// signed playback URL to the WebView. The image stays in-memory as a data URL.
#[tauri::command]
async fn player_preview_frame(
    input: PlayerPreviewFrameInput,
    state: State<'_, AppState>,
) -> CommandResult<PlayerPreviewFrame> {
    if input.url.trim().is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "preview requires a media URL".into(),
        )));
    }
    let ffmpeg = state
        .runtime
        .resource(RuntimeResourceKind::Ffmpeg)
        .and_then(|resource| resource.path.clone())
        .or_else(|| {
            discover_resource_dir(Some(&state.paths.resource_dir))
                .map(|root| root.join("mpv/ffmpeg.exe"))
        })
        .filter(|path| path.is_file());
    let source = resolve_preview_source(
        &input.url,
        input.media_id.as_deref(),
        &state.paths.data_dir,
    );
    let headers = input.headers;
    let decryption_key = input
        .decryption_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let position = input.position_seconds.max(0.0).min(86_400.0);
    if let Some(ffmpeg) = ffmpeg {
        if Path::new(&source).is_file() {
        let source_for_ffmpeg = source.clone();
        let jpeg = tokio::time::timeout(
            Duration::from_millis(1_200),
            tokio::task::spawn_blocking(move || {
                capture_seek_preview_jpeg(
                    &ffmpeg,
                    &source_for_ffmpeg,
                    &headers,
                    decryption_key.as_deref(),
                    position,
                )
            }),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(Result::ok);
        if let Some(jpeg) = jpeg {
            return Ok(PlayerPreviewFrame {
                data_url: format!("data:image/jpeg;base64,{}", BASE64_STANDARD.encode(jpeg)),
            });
        }
        }
    }
    if let Some(jpeg) = capture_live_mpv_jpeg(&state).await {
        return Ok(PlayerPreviewFrame {
            data_url: format!("data:image/jpeg;base64,{}", BASE64_STANDARD.encode(jpeg)),
        });
    }
    Err(app_error(AppError::Playback(
        "FFmpeg could not capture the requested seek preview".into(),
    )))
}

async fn capture_live_mpv_jpeg(state: &State<'_, AppState>) -> Option<Vec<u8>> {
    let out_dir = std::env::temp_dir().join("ttv-seek-preview");
    let _ = std::fs::create_dir_all(&out_dir);
    let out_path = out_dir.join(format!("live-{}.jpg", uuid::Uuid::new_v4()));
    let path_str = out_path.to_string_lossy().replace('\\', "/");
    {
        let playback = state.playback.lock().ok()?;
        let actor = playback.as_ref()?;
        let _ = actor.dispatch(PlaybackCommand::RawCommand {
            args: vec![
                "set".into(),
                "screenshot-format".into(),
                "jpg".into(),
            ],
        });
        let _ = actor.dispatch(PlaybackCommand::RawCommand {
            args: vec!["set".into(), "screenshot-sw".into(), "yes".into()],
        });
        actor
            .dispatch(PlaybackCommand::Screenshot { path: path_str })
            .ok()?;
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if let Ok(bytes) = std::fs::read(&out_path) {
            if let Some(jpeg) = jpeg_payload(&bytes).filter(|item| item.len() > 128) {
                let owned = jpeg.to_vec();
                let _ = std::fs::remove_file(&out_path);
                return Some(owned);
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = std::fs::remove_file(&out_path);
    None
}

fn jpeg_payload(bytes: &[u8]) -> Option<&[u8]> {
    bytes
        .windows(2)
        .position(|window| window == [0xFF, 0xD8])
        .map(|index| &bytes[index..])
}

const SEEK_PREVIEW_SCALE: &str =
    "scale=224:224:force_original_aspect_ratio=decrease:force_divisible_by=2";

fn ffmpeg_http_headers(headers: &std::collections::BTreeMap<String, String>) -> Option<String> {
    if headers.is_empty() {
        return None;
    }
    let mut value = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    // FFmpeg's HTTP demuxer reads headers until a terminating CRLF. Without
    // it the child waits on stdin-like header parsing and the seek preview
    // spinner never resolves.
    if !value.ends_with("\r\n") {
        value.push_str("\r\n");
    }
    Some(value)
}

fn wait_child_with_timeout(
    child: &mut Child,
    timeout: Duration,
) -> Result<Option<std::process::ExitStatus>, AppError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let pid = child.id();
                    let _ = child.kill();
                    kill_process_tree(pid);
                    let _ = child.wait();
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(AppError::Runtime(format!(
                    "wait FFmpeg preview: {error}"
                )))
            }
        }
    }
}

fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_child_window(&mut command);
        let _ = command.status();
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
    }
}

fn resolve_preview_source(url: &str, media_id: Option<&str>, data_dir: &Path) -> String {
    if let Some(path) = decode_local_media_path(url) {
        if path.is_file() {
            return path.to_string_lossy().into_owned();
        }
    }
    if let Some(media_id) = media_id {
        if let Some(vid) = media_id.rsplit(':').next() {
            let safe = !vid.is_empty()
                && vid
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
            if safe {
                let cache = data_dir.join("short-drama-cache");
                for rel in [
                    format!("short-series/{vid}.mp4"),
                    format!("motion-comic/{vid}.mp4"),
                    format!("{vid}.mp4"),
                ] {
                    let candidate = cache.join(&rel);
                    if candidate.is_file() {
                        return candidate.to_string_lossy().into_owned();
                    }
                }
            }
        }
    }
    url.to_owned()
}

fn apply_preview_http_options(
    command: &mut Command,
    source: &str,
    headers: &std::collections::BTreeMap<String, String>,
) {
    if !source.starts_with("http://") && !source.starts_with("https://") {
        return;
    }
    command.args(["-probesize", "65536", "-analyzeduration", "500000"]);
    let mut leftover = std::collections::BTreeMap::new();
    for (name, value) in headers {
        match name.to_ascii_lowercase().as_str() {
            "user-agent" => {
                command.args(["-user_agent", value]);
            }
            "referer" => {
                command.args(["-referer", value]);
            }
            _ => {
                leftover.insert(name.clone(), value.clone());
            }
        }
    }
    if let Some(block) = ffmpeg_http_headers(&leftover) {
        command.args(["-headers", block.as_str()]);
    }
}

fn capture_seek_preview_jpeg(
    ffmpeg: &Path,
    source: &str,
    headers: &std::collections::BTreeMap<String, String>,
    decryption_key: Option<&str>,
    position: f64,
) -> Result<Vec<u8>, AppError> {
    let out_dir = std::env::temp_dir().join("ttv-seek-preview");
    let _ = std::fs::create_dir_all(&out_dir);
    let out_path = out_dir.join(format!("{}.jpg", uuid::Uuid::new_v4()));
    let stamp = format!("{position:.3}");
    // One fast input-seek only. A second accurate pass can decode from the
    // start of an encrypted HTTP stream and freeze the UI spinner forever.
    let mut command = Command::new(ffmpeg);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-an");
    apply_preview_http_options(&mut command, source, headers);
    if let Some(key) = decryption_key {
        command.args(["-decryption_key", key]);
    }
    command.arg("-ss").arg(&stamp).arg("-i").arg(source);
    command.args([
        "-frames:v",
        "1",
        "-vf",
        SEEK_PREVIEW_SCALE,
        "-q:v",
        "3",
        "-y",
    ]);
    command.arg(&out_path);
    hide_child_window(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = std::fs::remove_file(&out_path);
            return Err(AppError::Runtime(format!("spawn FFmpeg preview: {error}")));
        }
    };
    let _ = wait_child_with_timeout(&mut child, Duration::from_millis(1_400))?;
    let captured = std::fs::read(&out_path).ok().and_then(|bytes| {
        jpeg_payload(&bytes)
            .filter(|jpeg| jpeg.len() > 128)
            .map(|jpeg| jpeg.to_vec())
    });
    let _ = std::fs::remove_file(&out_path);
    captured.ok_or_else(|| {
        AppError::Playback("FFmpeg could not capture the requested seek preview".into())
    })
}

fn hls_manifest_ready(output_dir: &Path) -> bool {
    output_dir.join("index.m3u8").is_file()
        && output_dir.join("init.mp4").is_file()
        && std::fs::read_dir(output_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("m4s"))
}

fn preferred_h264_encoder(ffmpeg: &Path) -> &'static str {
    static ENCODER: OnceLock<&'static str> = OnceLock::new();
    ENCODER.get_or_init(|| {
        for encoder in ["h264_nvenc", "h264_qsv", "h264_amf"] {
            let status = Command::new(ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=size=64x64:rate=1",
                    "-frames:v",
                    "1",
                    "-c:v",
                    encoder,
                    "-f",
                    "null",
                    "-",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if status.map(|value| value.success()).unwrap_or(false) {
                return encoder;
            }
        }
        "libx264"
    })
}

fn source_has_browser_h264(
    ffmpeg: &Path,
    source: &str,
    headers: &std::collections::BTreeMap<String, String>,
) -> bool {
    let mut command = Command::new(ffmpeg);
    command.args(["-hide_banner"]);
    if source.starts_with("http") {
        if let Some(value) = ffmpeg_http_headers(headers) {
            command.args(["-headers", value.as_str()]);
        }
    }
    command
        .args(["-i", source])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_child_window(&mut command);
    let Ok(output) = command.output() else {
        return false;
    };
    let probe = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    probe.lines().any(|line| {
        line.contains("video: h264")
            && !line.contains("yuv420p10")
            && !line.contains("yuv422")
            && !line.contains("yuv444")
    })
}

fn spawn_hls_transcode(
    ffmpeg: &Path,
    source: &str,
    headers: &std::collections::BTreeMap<String, String>,
    output_dir: &Path,
    encoder: &str,
) -> Result<Child, AppError> {
    std::fs::create_dir_all(output_dir)?;
    let mut command = Command::new(ffmpeg);
    command
        .current_dir(output_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // Do not persist FFmpeg diagnostics: input errors can echo signed
        // URLs or request headers. The user-facing command returns a generic
        // sanitized failure and the structured diagnostics layer carries the
        // safe error category.
        .stderr(Stdio::null())
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("warning")
        .arg("-y")
        .arg("-i");
    if source.starts_with("http") {
        if let Some(value) = ffmpeg_http_headers(headers) {
            command.args(["-headers", value.as_str()]);
        }
    }
    command
        .arg(source)
        // Keep one deterministic default audio program for WebView2/HLS.
        // Mapping every MKV audio stream can leave the browser without a
        // selectable default track when TrueHD/commentary tracks coexist.
        .args(["-map", "0:v:0", "-map", "0:a:0?"])
        .arg("-c:v")
        .arg(encoder);
    if encoder == "copy" {
        // Preserve the source keyframes. The event playlist is published as
        // soon as the first independently decodable fragment is available.
    } else if encoder == "h264_nvenc" {
        command.args([
            "-preset", "p4", "-tune", "hq", "-rc", "vbr", "-cq", "23", "-b:v", "0",
        ]);
    } else {
        command.args(["-preset", "ultrafast", "-crf", "23"]);
    }
    if encoder != "copy" {
        command.args([
            "-pix_fmt",
            "yuv420p",
            "-force_key_frames",
            "expr:gte(t,n_forced*2)",
        ]);
    }
    command
        .args([
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-af",
            "aresample=async=1:first_pts=0",
            "-disposition:a:0",
            "default",
        ])
        .args([
            "-f",
            "hls",
            "-hls_time",
            "2",
            "-hls_list_size",
            "0",
            "-hls_playlist_type",
            "event",
            "-hls_segment_type",
            "fmp4",
            "-hls_fmp4_init_filename",
            "init.mp4",
            "-hls_flags",
            "independent_segments+temp_file",
            "-hls_segment_filename",
            "segment-%06d.m4s",
            "index.m3u8",
        ]);
    hide_child_window(&mut command);
    command
        .spawn()
        .map_err(|error| AppError::Playback(format!("failed to start FFmpeg: {error}")))
}

#[cfg(windows)]
fn hide_child_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_child_window(_command: &mut Command) {}

fn app_error_io(error: std::io::Error) -> IpcError {
    app_error(AppError::Playback(error.to_string()))
}

fn decode_local_media_path(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return None;
    }
    let path = if trimmed.starts_with("file:///") {
        let without_scheme = trimmed.trim_start_matches("file:///");
        let decoded = percent_decode(without_scheme)?;
        if decoded.len() >= 2 && decoded.as_bytes()[1] == b':' {
            decoded
        } else {
            format!("/{decoded}")
        }
    } else {
        trimmed.to_owned()
    };
    Some(PathBuf::from(path))
}

fn percent_decode(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let raw = value.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            if index + 2 >= raw.len() {
                return None;
            }
            let high = (raw[index + 1] as char).to_digit(16)? as u8;
            let low = (raw[index + 2] as char).to_digit(16)? as u8;
            bytes.push((high << 4) | low);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

#[tauri::command]
fn player_screenshot(path: String, state: State<'_, AppState>) -> CommandResult<()> {
    if path.trim().is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "screenshot path cannot be empty".into(),
        )));
    }
    player_command(PlaybackCommand::Screenshot { path }, state)
}

#[tauri::command]
fn external_player_list() -> Vec<ExternalPlayer> {
    discover_external_players()
}

#[tauri::command]
fn external_player_open(input: ExternalPlayerOpenInput) -> CommandResult<bool> {
    if input.url.trim().is_empty() || input.url.contains('\0') {
        return Err(app_error(AppError::InvalidInput(
            "external player URL cannot be empty".into(),
        )));
    }
    let player = discover_external_players()
        .into_iter()
        .find(|item| item.id == input.player_id)
        .ok_or_else(|| {
            app_error(AppError::NotFound(format!(
                "external player not found: {}",
                input.player_id
            )))
        })?;
    let mut command = std::process::Command::new(player.path);
    for (name, value) in input.headers {
        if name.contains(['\0', '\r', '\n']) || value.contains(['\0', '\r', '\n']) {
            return Err(app_error(AppError::InvalidInput(
                "external player headers contain invalid characters".into(),
            )));
        }
        command.arg(format!("--http-header-fields={name}: {value}"));
    }
    command.arg(input.url).spawn().map_err(|error| {
        app_error(AppError::Playback(format!(
            "failed to start external player: {error}"
        )))
    })?;
    Ok(true)
}

#[tauri::command]
fn library_page(
    input: MediaPageInput,
    state: State<'_, AppState>,
) -> CommandResult<Vec<MediaRecord>> {
    let filter = MediaFilter {
        account_id: input.account_id.as_deref(),
        library_id: input.library_id.as_deref(),
        kind: input.kind.as_deref(),
    };
    state
        .database
        // 大库（数万条）靠 200/页的旧上限要串行拉上百次 IPC，启动时先渲染的
        // 首页子集会"卡"在半量数据上好几秒。放宽到 5000，与 library_scrape 一致。
        .list_media(filter, input.limit.min(5000), input.offset)
        .map_err(app_error)
}

#[tauri::command]
fn library_search(
    input: SearchInput,
    state: State<'_, AppState>,
) -> CommandResult<Vec<MediaRecord>> {
    if input.query.trim().is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "search query cannot be empty".into(),
        )));
    }
    state
        .database
        .search_media(&input.query, input.limit.min(500), input.offset)
        .map_err(app_error)
}

#[tauri::command]
fn library_upsert(media: MediaRecord, state: State<'_, AppState>) -> CommandResult<()> {
    let mut media = media;
    if is_promotional_media_record(&media) {
        mark_promotional_media(&mut media, "content-filter");
    }
    state.database.upsert_media(&media).map_err(app_error)
}

#[tauri::command]
fn library_delete(input: LibraryMediaInput, state: State<'_, AppState>) -> CommandResult<bool> {
    state
        .database
        .delete_media(&input.media_id)
        .map_err(app_error)
}

/// `library://source-delete-progress` 的载荷。phase: count → db → covers → done。
#[derive(Debug, Clone, serde::Serialize)]
struct SourceDeleteProgress {
    phase: String,
    processed: u64,
    total: u64,
}

#[tauri::command]
async fn library_delete_source<R: Runtime>(
    app: AppHandle<R>,
    input: LibrarySourceDeleteInput,
    state: State<'_, AppState>,
) -> CommandResult<u64> {
    state.cancel_tasks();
    let source_type = input.source_type.trim().to_owned();
    if source_type.is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "source_type cannot be empty".into(),
        )));
    }
    let database = std::sync::Arc::clone(&state.database);
    let cover_dir = state.paths.data_dir.join("covers");
    // 批量大小同时决定写锁窗口的上限：批越小，删除进行中刷新页面时
    // 同步分页查询等待的时间越短，窗口不会进入"未响应"。
    const DELETE_BATCH_SIZE: i64 = 1_000;
    tokio::task::spawn_blocking(move || {
        let emit_progress = |phase: &str, processed: u64, total: u64| {
            let _ = app.emit(
                "library://source-delete-progress",
                &SourceDeleteProgress {
                    phase: phase.to_owned(),
                    processed,
                    total,
                },
            );
        };
        let total = database.count_media_by_source(&source_type)?;
        emit_progress("count", 0, total);
        // 只取 id + art_url：整库 list_media 会把 payload 全部反序列化，
        // 长时间占住连接锁，是刷新卡死窗口的主因。
        let art_entries = database.list_media_art_by_source(&source_type)?;
        let mut removed_total = 0u64;
        loop {
            let batch = database.delete_media_by_source_batch(&source_type, DELETE_BATCH_SIZE)?;
            removed_total += batch;
            emit_progress("db", removed_total, total);
            if batch == 0 {
                break;
            }
        }
        // 同一张封面会被剧集/多版本条目共用，去重后再删文件。
        let mut seen = std::collections::HashSet::new();
        let cover_files = art_entries
            .into_iter()
            .filter_map(|(_, art_url)| art_url)
            .filter(|art_url| seen.insert(art_url.clone()))
            .map(std::path::PathBuf::from)
            .filter(|path| path.starts_with(&cover_dir) && path.is_file())
            .collect::<Vec<_>>();
        let cover_total = cover_files.len() as u64;
        emit_progress("covers", 0, cover_total);
        for (index, path) in cover_files.iter().enumerate() {
            let _ = std::fs::remove_file(path);
            let done = (index + 1) as u64;
            if done % 100 == 0 || done == cover_total {
                emit_progress("covers", done, cover_total);
            }
        }
        emit_progress("done", removed_total, total);
        Ok::<u64, AppError>(removed_total)
    })
    .await
    .map_err(|error| {
        app_error(AppError::Internal(format!(
            "source delete task failed: {error}"
        )))
    })?
    .map_err(app_error)
}

#[tauri::command]
fn library_move(input: LibraryMoveInput, state: State<'_, AppState>) -> CommandResult<bool> {
    state
        .database
        .move_media(&input.media_id, input.library_id.as_deref())
        .map_err(app_error)
}

#[tauri::command]
fn library_set_art(input: LibraryArtworkInput, state: State<'_, AppState>) -> CommandResult<bool> {
    state
        .database
        .set_media_art_url(&input.media_id, &input.art_url)
        .map_err(app_error)
}

/// Persist the dedicated wide poster used by the home hero. Card artwork
/// remains untouched so library cards keep their original cover ratio/style.
#[tauri::command]
fn library_set_home_poster(
    input: LibraryArtworkInput,
    state: State<'_, AppState>,
) -> CommandResult<bool> {
    state
        .database
        .set_media_backdrop_url(&input.media_id, &input.art_url)
        .map_err(app_error)
}

#[tauri::command]
fn library_set_preview(
    input: LibraryPreviewInput,
    state: State<'_, AppState>,
) -> CommandResult<bool> {
    state
        .database
        .set_media_preview(&input.media_id, &input.art_url, input.duration_seconds)
        .map_err(app_error)
}

#[tauri::command]
fn library_clear(state: State<'_, AppState>) -> CommandResult<u64> {
    state.database.clear_media().map_err(app_error)
}

/// Ask the running scan / scrape / cloud-import job to stop at its next
/// checkpoint. Cooperative: the backend loops check this flag between items,
/// so the command returns immediately and the job unwinds on its own.
#[tauri::command]
fn tasks_cancel(state: State<'_, AppState>) -> CommandResult<bool> {
    state.cancel_tasks();
    Ok(true)
}

/// Whether a cooperative cancellation has been requested since the last job
/// started. Lets the frontend show "已取消" instead of "失败" when the invoke
/// promise rejects with the cancellation error.
#[tauri::command]
fn tasks_cancelled(state: State<'_, AppState>) -> CommandResult<bool> {
    Ok(state.tasks_cancelled())
}

#[tauri::command]
async fn library_scan<R: Runtime>(
    root: String,
    max_files: Option<u64>,
    mark_adult: Option<bool>,
    state: State<'_, AppState>,
    app: AppHandle<R>,
) -> CommandResult<ScanReport> {
    state.reset_task_cancel();
    let root_buf = std::path::PathBuf::from(&root);
    let database = std::sync::Arc::clone(&state.database);
    let cancel = std::sync::Arc::clone(&state.task_cancel);
    let app_for_progress = app.clone();
    let progress = {
        let cancel = std::sync::Arc::clone(&cancel);
        let root_for_key = root.clone();
        std::sync::Arc::new(
            move |scanned: u64,
                  imported: u64,
                  skipped: u64,
                  skipped_promotional: u64,
                  skipped_non_video: u64| {
                if cancel.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                let _ = app_for_progress.emit(
                    "library://scan-progress",
                    &serde_json::json!({
                        "taskKey": format!("local:{}", root_for_key),
                        "currentFolder": "",
                        "fetched": scanned,
                        "imported": imported,
                        "skipped": skipped,
                        "skippedPromotional": skipped_promotional,
                        "skippedNonVideo": skipped_non_video,
                        "folders": 0,
                    }),
                );
            },
        ) as std::sync::Arc<dyn Fn(u64, u64, u64, u64, u64) + Send + Sync>
    };
    let options = ScanOptions {
        max_files: max_files
            .unwrap_or_else(|| ScanOptions::default().max_files)
            .clamp(1, 1_000_000),
        mark_adult,
        cancel: Some(std::sync::Arc::clone(&cancel)),
        progress: Some(progress),
    };
    tokio::task::spawn_blocking(move || {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(AppError::Internal("library scan cancelled".into()));
        }
        scan_directory(&database, root_buf, options)
    })
    .await
    .map_err(|error| {
        app_error(AppError::Internal(format!(
            "library scan task failed: {error}"
        )))
    })?
    .map_err(app_error)
}

/// Hide promotional/junk records that were imported before the unified filter
/// existed. The original files remain untouched.
#[tauri::command]
fn library_cleanup_promotional(
    state: State<'_, AppState>,
) -> CommandResult<PromotionalCleanupReport> {
    crate::library::cleanup_promotional_media(&state.database).map_err(app_error)
}

/// Whether TMDB is usable: a token saved in settings wins, otherwise fall back
/// to the environment variables.
fn tmdb_enabled_from(database: &crate::storage::Database) -> bool {
    if let Ok(Some(token)) = database.kv_get("metadata.tmdb.token") {
        if !token.trim().is_empty() {
            return true;
        }
    }
    ["TTV_TMDB_READ_TOKEN", "TTV_TMDB_API_KEY"]
        .iter()
        .any(|key| {
            std::env::var(key)
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
        })
}

fn metadata_provider_list(tmdb_enabled: bool) -> Vec<MetadataProviderStatus> {
    vec![
        MetadataProviderStatus {
            id: "tmdb".into(),
            name: "TMDB".into(),
            enabled: tmdb_enabled,
            requires_configuration: true,
        },
        MetadataProviderStatus {
            id: "douban".into(),
            name: "豆瓣".into(),
            enabled: true,
            requires_configuration: false,
        },
        MetadataProviderStatus {
            id: "tvmaze".into(),
            name: "TVMaze".into(),
            enabled: true,
            requires_configuration: false,
        },
        MetadataProviderStatus {
            id: "jav".into(),
            name: "JAV (JavBus/JavDB/Avmoo/JavLibrary/Jav321)".into(),
            enabled: true,
            requires_configuration: false,
        },
    ]
}

#[tauri::command]
fn metadata_providers(state: State<'_, AppState>) -> Vec<MetadataProviderStatus> {
    metadata_provider_list(tmdb_enabled_from(&state.database))
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataTmdbSetInput {
    pub token: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataTmdbStatus {
    pub configured: bool,
    /// Where the active credential comes from: `"settings"`, `"env"` or `"none"`.
    /// The token itself is never returned to the frontend.
    pub source: String,
}

/// Save (or clear, when empty) the TMDB token from the settings screen. The
/// value is stored in the local KV store and read back by the scraper; it is
/// never echoed to the frontend.
#[tauri::command]
fn metadata_tmdb_set(input: MetadataTmdbSetInput, state: State<'_, AppState>) -> CommandResult<()> {
    let token = input.token.trim().to_string();
    if token.is_empty() {
        state
            .database
            .kv_delete("metadata.tmdb.token")
            .map_err(app_error)?;
    } else {
        state
            .database
            .kv_set("metadata.tmdb.token", &token)
            .map_err(app_error)?;
    }
    Ok(())
}

/// Report whether TMDB is configured and where the credential comes from,
/// without ever exposing the token plaintext.
#[tauri::command]
fn metadata_tmdb_status(state: State<'_, AppState>) -> CommandResult<MetadataTmdbStatus> {
    if let Ok(Some(token)) = state.database.kv_get("metadata.tmdb.token") {
        if !token.trim().is_empty() {
            return Ok(MetadataTmdbStatus {
                configured: true,
                source: "settings".into(),
            });
        }
    }
    let from_env = ["TTV_TMDB_READ_TOKEN", "TTV_TMDB_API_KEY"]
        .iter()
        .any(|key| {
            std::env::var(key)
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
        });
    if from_env {
        return Ok(MetadataTmdbStatus {
            configured: true,
            source: "env".into(),
        });
    }
    Ok(MetadataTmdbStatus {
        configured: false,
        source: "none".into(),
    })
}

#[tauri::command]
fn metadata_nfo_write(
    input: MetadataNfoWriteInput,
    state: State<'_, AppState>,
) -> CommandResult<String> {
    let media = state
        .database
        .get_media(&input.media_id)
        .map_err(app_error)?
        .ok_or_else(|| app_error(AppError::InvalidInput("media item not found".into())))?;
    let source = media
        .remote_path
        .as_deref()
        .filter(|value| !value.starts_with("http://") && !value.starts_with("https://"))
        .ok_or_else(|| {
            app_error(AppError::InvalidInput(
                "NFO write requires a local media path".into(),
            ))
        })?;
    let source_path = Path::new(source);
    let parent = source_path.parent().ok_or_else(|| {
        app_error(AppError::Storage(
            "media parent directory is unavailable".into(),
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(app_error_io)?;
    let stem = source_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("movie");
    let target = parent.join(format!("{stem}.nfo"));
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("title".into(), media.title.clone());
    if let Some(value) = media.original_title.clone() {
        fields.insert("originaltitle".into(), value);
    }
    if let Some(value) = media.year {
        fields.insert("year".into(), value.to_string());
    }
    if let Some(value) = media.rating {
        fields.insert("rating".into(), value.to_string());
    }
    if let Some(payload) = media.payload.as_ref().and_then(|value| value.as_object()) {
        for key in ["plot", "premiered", "season", "episode"] {
            if let Some(value) = payload.get(key).and_then(serde_json::Value::as_str) {
                fields.insert(key.into(), value.into());
            }
        }
    }
    fields.extend(input.fields.into_iter().filter(|(key, value)| {
        matches!(
            key.as_str(),
            "title"
                | "originaltitle"
                | "year"
                | "rating"
                | "plot"
                | "premiered"
                | "season"
                | "episode"
        ) && !value.contains('\0')
    }));
    let mut xml =
        String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<movie>\n");
    for key in [
        "title",
        "originaltitle",
        "year",
        "rating",
        "plot",
        "premiered",
        "season",
        "episode",
    ] {
        if let Some(value) = fields.get(key) {
            xml.push_str(&format!("  <{key}>{}</{key}>\n", escape_xml(value)));
        }
    }
    xml.push_str("</movie>\n");
    let temp = target.with_extension("nfo.tmp");
    std::fs::write(&temp, xml).map_err(app_error_io)?;
    std::fs::rename(&temp, &target).map_err(app_error_io)?;
    Ok(target.display().to_string())
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[tauri::command]
async fn library_scrape<R: Runtime>(
    input: LibraryScrapeInput,
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CommandResult<ScrapeReport> {
    let available = metadata_provider_list(tmdb_enabled_from(&state.database));
    state.reset_task_cancel();
    let mut providers = if input.providers.is_empty() {
        available
            .iter()
            .filter(|provider| provider.enabled)
            .map(|provider| provider.id.clone())
            .collect::<Vec<_>>()
    } else {
        input.providers
    };
    providers.retain(|id| {
        available
            .iter()
            .any(|provider| provider.id == *id && provider.enabled)
    });
    if providers.is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "no configured metadata providers are available".into(),
        )));
    }
    let media = if !input.media_ids.is_empty() {
        input
            .media_ids
            .iter()
            .filter_map(|id| state.database.get_media(id).transpose())
            .collect::<Result<Vec<_>, _>>()
            .map_err(app_error)?
    } else if input.overwrite {
        state
            .database
            .list_media(MediaFilter::default(), input.limit.clamp(1, 5_000), 0)
            .map_err(app_error)?
    } else {
        // Target the unscraped backlog so repeated runs make forward progress
        // instead of re-processing the same first page of already-scraped rows.
        state
            .database
            .list_media_unscraped(input.limit.clamp(1, 5_000), 0)
            .map_err(app_error)?
    };
    // Stream per-item progress to the frontend as a Tauri event so the scrape
    // UI can show live counters instead of one blocking spinner.
    let app_handle = app.clone();
    let progress: ScrapeProgressSink = std::sync::Arc::new(move |tick: ScrapeProgress| {
        let _ = app_handle.emit("library://scrape-progress", &tick);
    });
    let jav_scope = match input.jav_scope.as_deref() {
        Some("fast") => crate::adult::JavScope::Fast,
        _ => crate::adult::JavScope::Full,
    };
    scrape_media(
        &state.database,
        media,
        ScrapeOptions {
            providers,
            overwrite: input.overwrite,
            include_adult: input.include_adult,
            cover_dir: Some(state.paths.data_dir.join("covers")),
            cancel: Some(std::sync::Arc::clone(&state.task_cancel)),
            jav_scope,
        },
        Some(progress),
    )
    .await
    .map_err(app_error)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdultReclassifyReport {
    pub scanned: u64,
    pub flagged: u64,
}

/// Reclassify the whole library for 18+ content.
///
/// Adult detection used to run only during scraping, so items imported but not
/// yet scraped (or imported before classification existed) could leak into the
/// main library. This sweep runs [`apply_adult_classification`] over every
/// record and persists newly-flagged ones, repairing existing leaks without any
/// network access.
#[tauri::command]
async fn library_reclassify_adult(
    state: State<'_, AppState>,
) -> CommandResult<AdultReclassifyReport> {
    let database = std::sync::Arc::clone(&state.database);
    tokio::task::spawn_blocking(move || {
        let batch_size = 500_u32;
        let mut offset = 0_u32;
        let mut scanned = 0_u64;
        let mut flagged = 0_u64;
        loop {
            let batch = database.list_media(MediaFilter::default(), batch_size, offset)?;
            let count = batch.len();
            if count == 0 {
                break;
            }
            for mut item in batch {
                scanned = scanned.saturating_add(1);
                if apply_adult_classification(&mut item) {
                    database.upsert_media(&item)?;
                    flagged = flagged.saturating_add(1);
                }
            }
            if (count as u32) < batch_size {
                break;
            }
            offset = offset.saturating_add(batch_size);
        }
        Ok::<_, AppError>(AdultReclassifyReport { scanned, flagged })
    })
    .await
    .map_err(|error| {
        app_error(AppError::Internal(format!(
            "adult reclassify task failed: {error}"
        )))
    })?
    .map_err(app_error)
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct LibrarySetAdultInput {
    pub media_id: String,
    pub adult: bool,
}

/// Record an explicit user decision about whether an item is 18+. This is the
/// authoritative path: it stamps `payload.adultManual` so the import-time
/// classifier, scraping and the reclassify sweep never override the choice
/// again. `adult = false` is how a wrongly-flagged normal video is moved out of
/// the 18+ view permanently.
#[tauri::command]
fn library_set_adult(input: LibrarySetAdultInput, state: State<'_, AppState>) -> CommandResult<()> {
    let mut media = state
        .database
        .get_media(&input.media_id)
        .map_err(app_error)?
        .ok_or_else(|| app_error(AppError::NotFound("media item not found".into())))?;
    set_manual_adult(&mut media, input.adult);
    state.database.upsert_media(&media).map_err(app_error)
}

/// Rebuild 18+ flags from scratch to clear stale false positives. Unlike the
/// additive [`library_reclassify_adult`] sweep (which only ever adds flags),
/// this re-evaluates every record with the current classifier and can also
/// UN-flag items an older, looser heuristic wrongly marked as 18+ (e.g. a
/// normal "Level 03" video). Manual decisions (`adultManual`) and records whose
/// adult flag came from a real scrape are authoritative and left untouched.
#[tauri::command]
async fn library_rebuild_adult(state: State<'_, AppState>) -> CommandResult<AdultReclassifyReport> {
    let database = std::sync::Arc::clone(&state.database);
    tokio::task::spawn_blocking(move || {
        let batch_size = 500_u32;
        let mut offset = 0_u32;
        let mut scanned = 0_u64;
        let mut flagged = 0_u64;
        loop {
            let batch = database.list_media(MediaFilter::default(), batch_size, offset)?;
            let count = batch.len();
            if count == 0 {
                break;
            }
            for mut item in batch {
                scanned = scanned.saturating_add(1);
                let manual = item
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("adultManual"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let scraped = item
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("scrapedBy"))
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty());
                if manual || scraped {
                    continue;
                }
                let before = item.payload.clone();
                clear_classifier_adult(&mut item);
                apply_adult_classification(&mut item);
                if item.payload != before {
                    let now_adult = item
                        .payload
                        .as_ref()
                        .and_then(|payload| payload.get("adult"))
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    database.upsert_media(&item)?;
                    if now_adult {
                        flagged = flagged.saturating_add(1);
                    }
                }
            }
            if (count as u32) < batch_size {
                break;
            }
            offset = offset.saturating_add(batch_size);
        }
        Ok::<_, AppError>(AdultReclassifyReport { scanned, flagged })
    })
    .await
    .map_err(|error| {
        app_error(AppError::Internal(format!(
            "adult rebuild task failed: {error}"
        )))
    })?
    .map_err(app_error)
}

#[tauri::command]
async fn library_repair_adult_isolation(
    state: State<'_, AppState>,
) -> CommandResult<AdultReclassifyReport> {
    let database = std::sync::Arc::clone(&state.database);
    tokio::task::spawn_blocking(move || {
        let batch_size = 500_u32;
        let mut offset = 0_u32;
        let mut scanned = 0_u64;
        let mut flagged = 0_u64;
        loop {
            let batch = database.list_media_raw(batch_size, offset)?;
            let count = batch.len();
            if count == 0 {
                break;
            }
            for mut item in batch {
                scanned = scanned.saturating_add(1);
                if repair_adult_isolation(&mut item) {
                    database.upsert_media(&item)?;
                    flagged = flagged.saturating_add(1);
                }
            }
            if (count as u32) < batch_size {
                break;
            }
            offset = offset.saturating_add(batch_size);
        }
        Ok::<_, AppError>(AdultReclassifyReport { scanned, flagged })
    })
    .await
    .map_err(|error| {
        app_error(AppError::Internal(format!(
            "adult isolation repair failed: {error}"
        )))
    })?
    .map_err(app_error)
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AdultCoverFetchInput {
    pub media_id: String,
    #[serde(default)]
    pub force: bool,
}

/// On-demand cover download for a JAV item (lazy-load retry path): the 18+
/// view calls this when a card's image is missing or failed to load. Uses the
/// stored cover URL when available, otherwise re-queries the adult sources.
#[tauri::command]
async fn adult_cover_fetch(
    input: AdultCoverFetchInput,
    state: State<'_, AppState>,
) -> CommandResult<Option<String>> {
    let mut media = state
        .database
        .get_media(&input.media_id)
        .map_err(app_error)?
        .ok_or_else(|| app_error(AppError::NotFound("media item not found".into())))?;
    let jav = media
        .payload
        .as_ref()
        .and_then(|payload| payload.get("jav"))
        .cloned()
        .ok_or_else(|| app_error(AppError::InvalidInput("item has no jav metadata".into())))?;
    let code = jav
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if code.trim().is_empty() {
        return Err(app_error(AppError::InvalidInput(
            "jav code is empty".into(),
        )));
    }

    let cover_dir = state.paths.data_dir.join("covers");
    if !input.force {
        if let Some(existing) = crate::adult::cover::find_existing_cover(&cover_dir, &code) {
            media.art_url = Some(existing.display().to_string());
            state.database.upsert_media(&media).map_err(app_error)?;
            return Ok(media.art_url);
        }
    }

    let client = crate::adult::build_client().map_err(app_error)?;
    let stored_url = jav
        .get("coverUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned);
    let cover_url = match stored_url {
        Some(url) => url,
        None => {
            let mut codes = crate::adult::code::extract_codes_from_name(
                media.remote_path.as_deref().unwrap_or(&media.title),
            );
            if !codes.iter().any(|candidate| *candidate == code) {
                codes.insert(0, code.clone());
            }
            crate::adult::lookup_jav(&client, &codes)
                .await
                .map_err(app_error)?
                .and_then(|matched| matched.cover_url)
                .ok_or_else(|| app_error(AppError::NotFound("no cover url available".into())))?
        }
    };

    let referer = crate::adult::cover::referer_for_provider(
        jav.get("provider")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
    );
    let path = crate::adult::cover::download_cover(
        &client,
        &cover_dir,
        &code,
        &cover_url,
        input.force,
        referer,
    )
    .await
    .map_err(app_error)?;
    media.art_url = Some(path.display().to_string());
    state.database.upsert_media(&media).map_err(app_error)?;
    Ok(media.art_url)
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AdultFirstFrameInput {
    pub media_id: String,
    #[serde(default)]
    pub force: bool,
}

/// On-demand "first frame as cover" for 18+ items that have no JAV metadata
/// and no usable cover. Resolves the provider playback URL for the cloud file,
/// then uses the bundled FFmpeg to grab an early frame (~1s in, to skip the
/// pure-black frame 0) into `{cover_dir}/firstframe-{hash}.jpg`, and stores it
/// as the item's art. Returns the stored art path, or `None` when the item has
/// no resolvable provider source.
#[tauri::command]
async fn adult_first_frame_cover(
    input: AdultFirstFrameInput,
    state: State<'_, AppState>,
) -> CommandResult<Option<String>> {
    let mut media = state
        .database
        .get_media(&input.media_id)
        .map_err(app_error)?
        .ok_or_else(|| app_error(AppError::NotFound("media item not found".into())))?;

    // Resolve the provider + provider media id needed to look up a playable URL.
    let payload = media
        .payload
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    let provider_id = payload
        .get("providerId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let provider_media_id = payload
        .get("fileId")
        .or_else(|| payload.get("mediaId"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if provider_id.is_empty() || provider_media_id.is_empty() {
        return Ok(None);
    }

    let cover_dir = state.paths.data_dir.join("covers");
    let target = cover_dir.join(format!(
        "firstframe-{}.jpg",
        first_frame_key(&input.media_id)
    ));
    if !input.force && target.is_file() {
        media.art_url = Some(target.display().to_string());
        state.database.upsert_media(&media).map_err(app_error)?;
        return Ok(media.art_url);
    }

    // Resolve a direct playable URL for the cloud file.
    let descriptor = provider_resolve_playback_impl(
        &provider_id,
        PlaybackRequest {
            media_id: provider_media_id,
            quality: None,
        },
        &state,
    )
    .await?;
    let source_url = descriptor.url.clone();

    let ffmpeg = state
        .runtime
        .resource(RuntimeResourceKind::Ffmpeg)
        .and_then(|resource| resource.path.clone())
        .or_else(|| {
            discover_resource_dir(Some(&state.paths.resource_dir))
                .map(|root| root.join("mpv/ffmpeg.exe"))
        })
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            app_error(AppError::Playback(
                "bundled FFmpeg is unavailable for first-frame cover".into(),
            ))
        })?;

    let target_path = target.clone();
    let captured = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&cover_dir)
            .map_err(|error| AppError::Storage(format!("create cover dir: {error}")))?;
        let output = Command::new(&ffmpeg)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(&source_url)
            .arg("-ss")
            .arg("1")
            .arg("-frames:v")
            .arg("1")
            .arg("-q:v")
            .arg("3")
            .arg("-y")
            .arg(&target_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| AppError::Runtime(format!("spawn ffmpeg: {error}")))?;
        if !output.status.success() || !target_path.is_file() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Runtime(format!(
                "ffmpeg first-frame capture failed: {}",
                stderr.trim()
            )));
        }
        Ok::<PathBuf, AppError>(target_path)
    })
    .await
    .map_err(|error| {
        app_error(AppError::Internal(format!(
            "first frame task failed: {error}"
        )))
    })?
    .map_err(app_error)?;

    media.art_url = Some(captured.display().to_string());
    state.database.upsert_media(&media).map_err(app_error)?;
    Ok(media.art_url)
}

/// Stable, path-safe key for a first-frame cover derived from the media id.
fn first_frame_key(media_id: &str) -> String {
    let mut hasher = DefaultHasher::new();
    media_id.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[tauri::command]
fn library_sources_get(state: State<'_, AppState>) -> CommandResult<Vec<LibrarySource>> {
    let value = state
        .database
        .kv_get("library.sources")
        .map_err(app_error)?;
    Ok(value
        .map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map_err(|error| {
            app_error(AppError::Storage(format!(
                "invalid library sources: {error}"
            )))
        })?
        .unwrap_or_default())
}

#[tauri::command]
fn library_sources_set(
    sources: Vec<LibrarySource>,
    state: State<'_, AppState>,
) -> CommandResult<Vec<LibrarySource>> {
    let mut normalized = Vec::new();
    for source in sources {
        let path = std::path::PathBuf::from(source.path.trim());
        if path.as_os_str().is_empty() {
            continue;
        }
        let path = path.canonicalize().map_err(|error| {
            app_error(AppError::NotFound(format!(
                "library source is unavailable: {error}"
            )))
        })?;
        if !path.is_dir() {
            return Err(app_error(AppError::InvalidInput(format!(
                "library source is not a directory: {}",
                path.display()
            ))));
        }
        let path = path.to_string_lossy().into_owned();
        if !normalized
            .iter()
            .any(|item: &LibrarySource| item.path == path)
        {
            normalized.push(LibrarySource {
                path,
                enabled: source.enabled,
            });
        }
    }
    let raw = serde_json::to_string(&normalized)
        .map_err(|error| app_error(AppError::Internal(error.to_string())))?;
    state
        .database
        .kv_set("library.sources", &raw)
        .map_err(app_error)?;
    Ok(normalized)
}

#[tauri::command]
async fn library_scan_sources(state: State<'_, AppState>) -> CommandResult<Vec<ScanReport>> {
    let value = state
        .database
        .kv_get("library.sources")
        .map_err(app_error)?;
    let sources: Vec<LibrarySource> = value
        .map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map_err(|error| {
            app_error(AppError::Storage(format!(
                "invalid library sources: {error}"
            )))
        })?
        .unwrap_or_default();
    let database = std::sync::Arc::clone(&state.database);
    let reports = tokio::task::spawn_blocking(move || {
        sources
            .into_iter()
            .filter(|source| source.enabled)
            .map(|source| {
                crate::library::scan_directory(&database, source.path, ScanOptions::default())
            })
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(|error| {
        app_error(AppError::Internal(format!(
            "library source scan failed: {error}"
        )))
    })?
    .map_err(app_error)?;
    Ok(reports)
}

#[tauri::command]
fn history_save(
    media_id: String,
    position_seconds: f64,
    duration_seconds: f64,
    completed: bool,
    state: State<'_, AppState>,
) -> CommandResult<i64> {
    state
        .database
        .save_watch_history(&media_id, position_seconds, duration_seconds, completed)
        .map_err(app_error)
}

#[tauri::command]
fn history_get(
    media_id: String,
    state: State<'_, AppState>,
) -> CommandResult<Option<crate::storage::WatchHistory>> {
    state
        .database
        .latest_watch_history(&media_id)
        .map_err(app_error)
}

#[tauri::command]
fn favorites_toggle(
    media_id: String,
    favorite: bool,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    state
        .database
        .set_favorite(&media_id, favorite)
        .map_err(app_error)
}

#[tauri::command]
fn favorites_list(
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> CommandResult<Vec<MediaRecord>> {
    state
        .database
        .list_favorite_media(limit.unwrap_or(500).clamp(1, 5_000))
        .map_err(app_error)
}

#[tauri::command]
fn library_stats(state: State<'_, AppState>) -> CommandResult<LibraryStats> {
    Ok(LibraryStats {
        favorite_count: state.database.favorite_count().map_err(app_error)?,
        watched_seconds: state.database.watched_seconds().map_err(app_error)?,
        library_count: state.database.media_count().map_err(app_error)?,
        // The media schema does not persist a trustworthy byte size for every
        // source, so the UI must not invent a storage total.
        storage_bytes: None,
    })
}

#[tauri::command]
fn settings_get(key: String, state: State<'_, AppState>) -> CommandResult<Option<String>> {
    state.database.kv_get(&key).map_err(app_error)
}

#[tauri::command]
fn settings_set(key: String, value: String, state: State<'_, AppState>) -> CommandResult<()> {
    state.database.kv_set(&key, &value).map_err(app_error)
}

#[tauri::command]
fn playback_cache_clear(state: State<'_, AppState>) -> CommandResult<u64> {
    let playback_dir = state.paths.cache_dir.join("playback");
    if !playback_dir.is_dir() {
        return Ok(0);
    }
    let active = state
        .browser_transcode_jobs
        .lock()
        .map_err(|_| app_error(AppError::Playback("transcode job lock poisoned".into())))?;
    let mut removed = 0_u64;
    for entry in std::fs::read_dir(&playback_dir)
        .map_err(app_error_io)?
        .flatten()
    {
        let key = entry.file_name().to_string_lossy().into_owned();
        if active.contains_key(&key) {
            continue;
        }
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if result.is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    Ok(removed)
}

/// 从 KV 读取已保存的增强开关并换算成增强模式（0-5）。
/// 供 enhancement_set 与"新播放 actor 启动后自动套用设置"共用，
/// 保证两处对模式的定义永远一致。
fn persisted_enhancement_mode(state: &AppState) -> Result<u8, AppError> {
    let flag = |key: &str| -> Result<bool, AppError> {
        Ok(state.database.kv_get(key)?.as_deref() == Some("1"))
    };
    let glsl = flag("enhancement.glsl")?;
    let interpolation = flag("enhancement.rife")? || flag("enhancement.vapoursynth")?;
    let uai = flag("enhancement.uai")?;
    Ok(if uai && interpolation {
        5
    } else if uai {
        4
    } else if glsl && interpolation {
        3
    } else if interpolation {
        2
    } else if glsl {
        1
    } else {
        0
    })
}

#[tauri::command]
fn enhancement_set(name: String, enabled: bool, state: State<'_, AppState>) -> CommandResult<()> {
    if !matches!(name.as_str(), "glsl" | "rife" | "vapoursynth" | "uai") {
        return Err(app_error(AppError::InvalidInput(
            "unknown enhancement name".into(),
        )));
    }
    state
        .database
        .kv_set(
            &format!("enhancement.{name}"),
            if enabled { "1" } else { "0" },
        )
        .map_err(app_error)?;
    let _ = apply_enhancement_plan(&state, persisted_enhancement_mode(&state)?)?;
    Ok(())
}

#[tauri::command]
fn enhancement_apply(mode: u8, state: State<'_, AppState>) -> CommandResult<EnhancementPlan> {
    apply_enhancement_plan(&state, mode).map_err(app_error)
}

#[tauri::command]
fn enhancement_status(state: State<'_, AppState>) -> CommandResult<EnhancementRuntimeStatus> {
    let flag = |key: &str| -> CommandResult<bool> {
        Ok(state.database.kv_get(key).map_err(app_error)?.as_deref() == Some("1"))
    };
    let glsl = flag("enhancement.glsl")?;
    let rife = flag("enhancement.rife")?;
    let vapoursynth = flag("enhancement.vapoursynth")?;
    let uai = flag("enhancement.uai")?;
    let snapshot = state
        .playback
        .lock()
        .map_err(|_| app_error(AppError::Playback("playback state lock poisoned".into())))?
        .as_ref()
        .map(|actor| actor.snapshot());
    let runtime_fallback = snapshot
        .as_ref()
        .and_then(|value| value.interpolation_status.as_deref())
        .is_some_and(|value| value.eq_ignore_ascii_case("fallback"));
    let enabled = rife
        || vapoursynth
        || snapshot
            .as_ref()
            .is_some_and(|value| value.interpolation_enabled);
    Ok(EnhancementRuntimeStatus {
        enabled,
        mode: snapshot
            .as_ref()
            .and_then(|value| value.interpolation_status.clone())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                if vapoursynth || rife {
                    "rife".into()
                } else {
                    "off".into()
                }
            }),
        fallback_active: runtime_fallback || (enabled && !state.runtime.interpolation_available),
        reason: snapshot
            .as_ref()
            .and_then(|value| value.degradation_reason.clone())
            .or_else(|| {
                (!state.runtime.interpolation_available)
                    .then(|| "补帧运行时资源不可用，已回退到原始帧率".into())
            }),
        actual_fps: snapshot.as_ref().and_then(|value| value.actual_fps),
        display_fps: snapshot.as_ref().and_then(|value| value.display_fps),
        glsl_enabled: glsl,
        rife_enabled: rife,
        uai_enabled: uai,
    })
}

fn apply_enhancement_plan(state: &AppState, mode: u8) -> Result<EnhancementPlan, AppError> {
    let paths = if let Some(bundled) = discover_resource_dir(Some(&state.paths.resource_dir)) {
        RuntimePaths::from_resource_dir(bundled)
    } else {
        RuntimePaths::from_root(state.paths.data_dir.clone())
    };
    let plan = state.runtime.enhancement_plan(mode, &paths);
    let playback = state
        .playback
        .lock()
        .map_err(|_| AppError::Playback("playback state lock poisoned".into()))?;
    if let Some(actor) = playback.as_ref() {
        // Live 补帧必须走 mpv 自带 display-resample + interpolation。
        // ffmpeg minterpolate 要软件 YUV，当前 Windows 硬解输出是 d3d11 纹理，
        // lavfi 链会报 Impossible to convert，vf 命令却仍返回成功，画面看起来
        // 和关闭补帧一模一样。
        actor.dispatch(PlaybackCommand::SetVideoFilter { filter: None })?;
        if plan.interpolation_enabled {
            actor.dispatch(PlaybackCommand::SetFrameInterpolation {
                enabled: true,
                mode: "display-resample".into(),
            })?;
        } else {
            actor.dispatch(PlaybackCommand::SetFrameInterpolation {
                enabled: false,
                mode: "off".into(),
            })?;
        }
        actor.dispatch(PlaybackCommand::SetVideoFilters {
            shaders: plan
                .shader_paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        })?;
    }
    Ok(plan)
}

#[tauri::command]
fn app_window_minimize<R: tauri::Runtime>(window: tauri::Window<R>) -> CommandResult<()> {
    window.minimize().map_err(|e| IpcError {
        code: "window_error".into(),
        message: e.to_string(),
        retryable: false,
        request_id: None,
        details: serde_json::Value::Object(Default::default()),
    })
}

#[tauri::command]
fn app_window_toggle_maximize<R: tauri::Runtime>(window: tauri::Window<R>) -> CommandResult<()> {
    if let Ok(maximized) = window.is_maximized() {
        if maximized {
            let _ = window.unmaximize();
        } else {
            let _ = window.maximize();
        }
    } else {
        let _ = window.maximize();
    }
    Ok(())
}

const WINDOW_MODE_NORMAL: u8 = 0;
const WINDOW_MODE_FULLSCREEN: u8 = 1;
const WINDOW_MODE_PIP: u8 = 2;

#[derive(Clone, Copy, Debug)]
struct SavedWindowRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    maximized: bool,
}

#[derive(Clone, Copy, Debug)]
struct WindowChromeState {
    mode: u8,
    restore: Option<SavedWindowRect>,
}

fn window_chrome_state() -> &'static std::sync::Mutex<WindowChromeState> {
    static STATE: OnceLock<std::sync::Mutex<WindowChromeState>> = OnceLock::new();
    STATE.get_or_init(|| {
        std::sync::Mutex::new(WindowChromeState {
            mode: WINDOW_MODE_NORMAL,
            restore: None,
        })
    })
}

fn window_error(message: impl Into<String>) -> IpcError {
    IpcError {
        code: "window_error".into(),
        message: message.into(),
        retryable: false,
        request_id: None,
        details: serde_json::Value::Object(Default::default()),
    }
}

fn capture_window_rect<R: Runtime>(window: &tauri::Window<R>) -> Option<SavedWindowRect> {
    let maximized = window.is_maximized().ok().unwrap_or(false);
    let pos = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    Some(SavedWindowRect {
        x: pos.x,
        y: pos.y,
        width: size.width.max(320),
        height: size.height.max(180),
        maximized,
    })
}

fn restore_window_rect<R: Runtime>(
    window: &tauri::Window<R>,
    saved: SavedWindowRect,
) -> CommandResult<()> {
    let _ = window.set_fullscreen(false);
    let _ = window.set_always_on_top(false);
    if window.is_maximized().ok().unwrap_or(false) {
        let _ = window.unmaximize();
    }
    window
        .set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(
            saved.x, saved.y,
        )))
        .map_err(|e| window_error(e.to_string()))?;
    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize::new(
            saved.width,
            saved.height,
        )))
        .map_err(|e| window_error(e.to_string()))?;
    if saved.maximized {
        let _ = window.maximize();
    }
    Ok(())
}

fn pip_content_size(
    video_width: i32,
    video_height: i32,
    avail_w: i32,
    avail_h: i32,
) -> (i32, i32) {
    const LONG: i32 = 480;
    const MIN_W: i32 = 180;
    const MIN_H: i32 = 120;
    let vw = if video_width > 0 { video_width } else { 16 };
    let vh = if video_height > 0 { video_height } else { 9 };
    let (mut w, mut h) = if vw >= vh {
        let w = LONG;
        let h = ((w as i64 * vh as i64) / vw as i64) as i32;
        (w, h.max(MIN_H))
    } else {
        let h = LONG;
        let w = ((h as i64 * vw as i64) / vh as i64) as i32;
        (w.max(MIN_W), h)
    };
    let max_w = avail_w.max(MIN_W);
    let max_h = avail_h.max(MIN_H);
    if w > max_w {
        h = ((h as i64 * max_w as i64) / w as i64).max(MIN_H as i64) as i32;
        w = max_w;
    }
    if h > max_h {
        w = ((w as i64 * max_h as i64) / h as i64).max(MIN_W as i64) as i32;
        h = max_h;
    }
    (w, h)
}

fn pip_outer_rect(
    work_left: i32,
    work_top: i32,
    work_right: i32,
    work_bottom: i32,
    video_width: i32,
    video_height: i32,
) -> (i32, i32, i32, i32) {
    const MARGIN: i32 = 24;
    let avail_w = (work_right - work_left - MARGIN * 2).max(160);
    let avail_h = (work_bottom - work_top - MARGIN * 2).max(90);
    let (w, h) = pip_content_size(video_width, video_height, avail_w, avail_h);
    let x = (work_right - w - MARGIN).max(work_left);
    let y = (work_bottom - h - MARGIN).max(work_top);
    (x, y, w, h)
}

#[cfg(windows)]
fn hwnd_of<R: Runtime>(window: &tauri::Window<R>) -> CommandResult<isize> {
    window
        .hwnd()
        .map(|hwnd| hwnd.0 as isize)
        .map_err(|e| window_error(format!("native window handle: {e}")))
}

#[cfg(windows)]
fn monitor_rects(hwnd: isize) -> Option<(i32, i32, i32, i32, i32, i32, i32, i32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let monitor = MonitorFromWindow(hwnd as _, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            rcWork: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            dwFlags: 0,
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        Some((
            info.rcMonitor.left,
            info.rcMonitor.top,
            info.rcMonitor.right,
            info.rcMonitor.bottom,
            info.rcWork.left,
            info.rcWork.top,
            info.rcWork.right,
            info.rcWork.bottom,
        ))
    }
}

#[cfg(windows)]
fn place_hwnd(hwnd: isize, x: i32, y: i32, width: i32, height: i32, topmost: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_SHOWWINDOW,
    };
    let insert_after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    unsafe {
        SetWindowPos(
            hwnd as _,
            insert_after as _,
            x,
            y,
            width.max(160),
            height.max(90),
            SWP_SHOWWINDOW | SWP_FRAMECHANGED,
        );
        pin_mpv_surface_below_overlay(hwnd);
        fit_mpv_surface_to_client(hwnd);
    }
}

fn apply_fullscreen<R: Runtime>(window: &tauri::Window<R>) -> CommandResult<()> {
    let _ = window.set_always_on_top(false);
    if window.is_maximized().ok().unwrap_or(false) {
        let _ = window.unmaximize();
    }
    let _ = window.set_fullscreen(true);
    #[cfg(windows)]
    {
        let hwnd = hwnd_of(window)?;
        if let Some((left, top, right, bottom, ..)) = monitor_rects(hwnd) {
            place_hwnd(hwnd, left, top, right - left, bottom - top, true);
        }
    }
    Ok(())
}

fn apply_pip<R: Runtime>(
    window: &tauri::Window<R>,
    video_width: i32,
    video_height: i32,
) -> CommandResult<()> {
    let _ = window.set_fullscreen(false);
    if window.is_maximized().ok().unwrap_or(false) {
        let _ = window.unmaximize();
    }
    #[cfg(windows)]
    {
        let hwnd = hwnd_of(window)?;
        if let Some((_, _, _, _, work_left, work_top, work_right, work_bottom)) = monitor_rects(hwnd)
        {
            let (x, y, w, h) = pip_outer_rect(
                work_left,
                work_top,
                work_right,
                work_bottom,
                video_width,
                video_height,
            );
            place_hwnd(hwnd, x, y, w, h, true);
        }
    }
    let _ = window.set_always_on_top(true);
    Ok(())
}

#[tauri::command]
fn app_window_toggle_fullscreen<R: tauri::Runtime>(
    window: tauri::Window<R>,
) -> CommandResult<bool> {
    let mut state = window_chrome_state()
        .lock()
        .map_err(|_| window_error("window chrome state lock poisoned"))?;
    if state.mode == WINDOW_MODE_FULLSCREEN {
        if let Some(saved) = state.restore.take() {
            restore_window_rect(&window, saved)?;
        } else {
            let _ = window.set_fullscreen(false);
            let _ = window.set_always_on_top(false);
        }
        state.mode = WINDOW_MODE_NORMAL;
        return Ok(false);
    }
    if state.mode == WINDOW_MODE_NORMAL {
        state.restore = capture_window_rect(&window);
    }
    apply_fullscreen(&window)?;
    state.mode = WINDOW_MODE_FULLSCREEN;
    Ok(true)
}

#[tauri::command]
fn app_window_toggle_pip<R: tauri::Runtime>(
    window: tauri::Window<R>,
    input: Option<WindowPipInput>,
) -> CommandResult<bool> {
    let mut state = window_chrome_state()
        .lock()
        .map_err(|_| window_error("window chrome state lock poisoned"))?;
    if state.mode == WINDOW_MODE_PIP {
        if let Some(saved) = state.restore.take() {
            restore_window_rect(&window, saved)?;
        } else {
            let _ = window.set_always_on_top(false);
            let _ = window.maximize();
        }
        state.mode = WINDOW_MODE_NORMAL;
        return Ok(false);
    }
    if state.mode == WINDOW_MODE_NORMAL {
        state.restore = capture_window_rect(&window);
    }
    let input = input.unwrap_or_default();
    apply_pip(
        &window,
        input.video_width.unwrap_or(0),
        input.video_height.unwrap_or(0),
    )?;
    state.mode = WINDOW_MODE_PIP;
    Ok(true)
}

#[tauri::command]
fn app_window_close<R: tauri::Runtime>(window: tauri::Window<R>) -> CommandResult<()> {
    window.close().map_err(|e| IpcError {
        code: "window_error".into(),
        message: e.to_string(),
        retryable: false,
        request_id: None,
        details: serde_json::Value::Object(Default::default()),
    })
}

/* ============ 糖心影院（深夜档在线目录，只读浏览 + 显式开映） ============ */

#[tauri::command]
async fn tangxin_discover(
    state: State<'_, AppState>,
) -> CommandResult<Vec<crate::adult::tangxin::TangxinSection>> {
    crate::adult::tangxin::fetch_discover(&state.paths.data_dir)
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn tangxin_search(
    state: State<'_, AppState>,
    request: crate::adult::tangxin::TangxinSearchRequest,
) -> CommandResult<crate::adult::tangxin::TangxinSearchResult> {
    crate::adult::tangxin::search_catalog(&state.paths.data_dir, &request)
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn tangxin_detail(
    state: State<'_, AppState>,
    movie_id: String,
) -> CommandResult<crate::adult::tangxin::TangxinDetail> {
    crate::adult::tangxin::fetch_detail(&state.paths.data_dir, movie_id.trim())
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn tangxin_poster(poster_url: String) -> CommandResult<String> {
    crate::adult::tangxin::decrypt_poster(poster_url.trim())
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn tangxin_play(
    state: State<'_, AppState>,
    movie_id: String,
    allow_buy: Option<bool>,
) -> CommandResult<crate::adult::tangxin::TangxinPlayResult> {
    crate::adult::tangxin::resolve_playback(
        &state.paths.data_dir,
        movie_id.trim(),
        allow_buy.unwrap_or(false),
    )
    .await
    .map_err(app_error)
}

/* ---- 糖心账号池：账号密码 / token 凭证（含群内共享导入）/ 二维码凭证 ---- */

#[tauri::command]
async fn tangxin_account_add(
    state: State<'_, AppState>,
    label: Option<String>,
    username: Option<String>,
    password: Option<String>,
    device_id: Option<String>,
    user_token: Option<String>,
    qrcode: Option<String>,
) -> CommandResult<crate::adult::tangxin::TangxinAccountView> {
    crate::adult::tangxin::account_add(
        &state.paths.data_dir,
        crate::adult::tangxin::TangxinAccountInput {
            label: label.unwrap_or_default(),
            username: username.unwrap_or_default(),
            password: password.unwrap_or_default(),
            device_id: device_id.unwrap_or_default(),
            user_token: user_token.unwrap_or_default(),
            qrcode: qrcode.unwrap_or_default(),
        },
    )
    .await
    .map_err(app_error)
}

#[tauri::command]
async fn tangxin_account_list(
    state: State<'_, AppState>,
) -> CommandResult<Vec<crate::adult::tangxin::TangxinAccountView>> {
    crate::adult::tangxin::account_list(&state.paths.data_dir)
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn tangxin_account_remove(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<()> {
    crate::adult::tangxin::account_remove(&state.paths.data_dir, id.trim())
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn tangxin_account_select(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<()> {
    crate::adult::tangxin::account_select(&state.paths.data_dir, id.trim())
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn tangxin_account_verify(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<crate::adult::tangxin::TangxinAccountView> {
    crate::adult::tangxin::account_verify(&state.paths.data_dir, id.trim())
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn tangxin_cloud_sync(
    state: State<'_, AppState>,
) -> CommandResult<crate::adult::tangxin::TangxinPoolSnapshot> {
    crate::adult::tangxin::account_sync_cloud(&state.paths.data_dir)
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn tangxin_cloud_upload(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<crate::adult::tangxin::TangxinPoolSnapshot> {
    crate::adult::tangxin::account_upload_cloud(&state.paths.data_dir, id.trim())
        .await
        .map_err(app_error)
}

#[tauri::command]
async fn tangxin_cloud_config_get(
    state: State<'_, AppState>,
) -> CommandResult<crate::adult::tangxin::TangxinRemoteConfig> {
    Ok(crate::adult::tangxin::remote_config_get(&state.paths.data_dir))
}

#[tauri::command]
async fn tangxin_cloud_config_set(
    state: State<'_, AppState>,
    base_url: Option<String>,
    enabled: Option<bool>,
    account_source_mode: Option<String>,
) -> CommandResult<crate::adult::tangxin::TangxinRemoteConfig> {
    let current = crate::adult::tangxin::remote_config_get(&state.paths.data_dir);
    crate::adult::tangxin::remote_config_set(
        &state.paths.data_dir,
        crate::adult::tangxin::TangxinRemoteConfig {
            base_url: base_url.unwrap_or(current.base_url),
            enabled: enabled.unwrap_or(current.enabled),
            account_source_mode: account_source_mode.unwrap_or(current.account_source_mode),
            fallback_local: current.fallback_local,
            last_sync_at: current.last_sync_at,
            last_error: current.last_error,
        },
    )
    .map_err(app_error)
}

/// Keep all IPC registrations in one place so the frontend has a stable list.
pub fn invoke_handler<R: tauri::Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool {
    tauri::generate_handler![
        health,
        runtime_status,
        runtime_diagnostics,
        streamhub_status,
        streamhub_start,
        streamhub_stop,
        streamhub_health,
        provider_list,
        source_catalog,
        provider_test_connection,
        openlist_status,
        openlist_login,
        openlist_logout,
        openlist_start,
        openlist_stop,
        openlist_restart,
        openlist_session_status,
        openlist_storage_schema,
        openlist_storage_list,
        openlist_storage_get,
        openlist_storage_save,
        openlist_storage_delete,
        openlist_storage_test,
        openlist_begin_auth,
        openlist_finish_auth,
        openlist_list_files,
        openlist_resolve_playback,
        openlist_sync_library,
        openlist_account_info,
        guangya_oauth_status,
        provider_capabilities,
        provider_open_official_page,
        provider_session_status,
        provider_device_code,
        provider_qr_login_create,
        provider_qr_login_poll,
        provider_oauth_authorization_url,
        provider_oauth_exchange_code,
        provider_poll,
        provider_sms_login,
        provider_import_token,
        provider_refresh,
        provider_logout,
        provider_list_files,
        provider_resolve_playback,
        provider_sync_library,
        provider_sync_library_recursive,
        guangya_device_code,
        guangya_poll,
        guangya_sms_login,
        guangya_list_files,
        guangya_resolve_playback,
        provider_video_qualities,
        provider_subtitle_search,
        provider_subtitle_download,
        media_probe,
        playback_plan,
        player_tracks,
        player_audio_capabilities,
        subtitle_search,
        subtitle_download,
        subtitle_cache_cleanup,
        subtitle_credentials_set,
        subtitle_credentials_clear,
        subtitle_attach,
        subtitle_import,
        subtitle_remove,
        player_state,
        player_capabilities,
        player_command,
        player_load,
        player_native_open,
        player_native_close,
        player_prepare_browser_media,
        player_preview_frame,
        player_screenshot,
        external_player_list,
        external_player_open,
        library_page,
        library_search,
        library_upsert,
        library_delete,
        library_delete_source,
        library_move,
        library_set_art,
        library_set_home_poster,
        library_set_preview,
        library_clear,
        library_scan,
        library_cleanup_promotional,
        tasks_cancel,
        tasks_cancelled,
        metadata_providers,
        metadata_tmdb_set,
        metadata_tmdb_status,
        metadata_nfo_write,
        library_scrape,
        library_reclassify_adult,
        library_set_adult,
        library_rebuild_adult,
        library_repair_adult_isolation,
        adult_cover_fetch,
        adult_first_frame_cover,
        library_sources_get,
        library_sources_set,
        library_scan_sources,
        history_get,
        history_save,
        favorites_toggle,
        favorites_list,
        library_stats,
        settings_get,
        settings_set,
        playback_cache_clear,
        enhancement_set,
        enhancement_apply,
        enhancement_status,
        app_window_minimize,
        app_window_toggle_maximize,
        app_window_toggle_fullscreen,
        app_window_toggle_pip,
        app_window_close,
        crate::short_drama::short_drama_stream,
        crate::short_drama::short_drama_detail,
        crate::short_drama::short_drama_play,
        crate::comic_drama::comic_drama_stream,
        crate::comic_drama::comic_drama_detail,
        crate::comic_drama::comic_drama_play,
        crate::short_drama_app::short_drama_app_resolve,
        crate::short_drama_app::short_drama_app_stream,
        crate::short_drama_app::short_drama_app_album,
        crate::short_drama_app::short_drama_app_status,
        crate::short_drama_app::short_drama_app_set_device,
        crate::short_drama_app::short_drama_app_cache_clear,
        tangxin_discover,
        tangxin_search,
        tangxin_detail,
        tangxin_poster,
        tangxin_play,
        tangxin_account_add,
        tangxin_account_list,
        tangxin_account_remove,
        tangxin_account_select,
        tangxin_account_verify,
        tangxin_cloud_sync,
        tangxin_cloud_upload,
        tangxin_cloud_config_get,
        tangxin_cloud_config_set
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[cfg(windows)]
    use std::sync::Arc;

    #[test]
    fn jpeg_payload_skips_leading_junk() {
        let mut bytes = vec![0, 1, 2];
        bytes.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0, 10, 20]);
        assert_eq!(
            jpeg_payload(&bytes),
            Some(&[0xFF, 0xD8, 0xFF, 0xE0, 10, 20][..])
        );
        assert_eq!(jpeg_payload(&[1, 2, 3]), None);
    }

    #[test]
    fn ffmpeg_headers_terminate_with_crlf() {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("User-Agent".into(), "com.phoenix.read/71332".into());
        headers.insert("Referer".into(), "https://example.invalid/".into());
        let value = ffmpeg_http_headers(&headers).unwrap();
        assert!(value.contains("User-Agent: com.phoenix.read/71332"));
        assert!(value.contains("Referer: https://example.invalid/"));
        assert!(value.ends_with("\r\n"), "{value:?}");
        assert!(ffmpeg_http_headers(&std::collections::BTreeMap::new()).is_none());
    }

    #[test]
    fn wait_child_timeout_kills_runaway_ffmpeg() {
        let ffmpeg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mpv/ffmpeg.exe");
        if !ffmpeg.is_file() {
            return;
        }
        let mut command = Command::new(&ffmpeg);
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "color=size=16x16:rate=1:duration=30",
            "-f",
            "null",
            "-",
        ]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_child_window(&mut command);
        let mut child = command.spawn().unwrap();
        let started = Instant::now();
        let status = wait_child_with_timeout(&mut child, Duration::from_millis(250)).unwrap();
        assert!(status.is_none(), "runaway FFmpeg should be killed");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(windows)]
    #[test]
    fn bundled_ffmpeg_captures_portrait_seek_preview_jpeg() {
        let ffmpeg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mpv/ffmpeg.exe");
        if !ffmpeg.is_file() {
            return;
        }
        let root = std::env::temp_dir().join(format!("ttv-preview-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("portrait.mp4");
        // 540x1170 scales to an odd width under 224px; force_divisible_by=2
        // must still produce a JPEG instead of failing yuv420p.
        let generated = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=540x1170:rate=12",
                "-t",
                "2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-an",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(generated.success());
        let started = Instant::now();
        let jpeg = capture_seek_preview_jpeg(
            &ffmpeg,
            source.to_string_lossy().as_ref(),
            &std::collections::BTreeMap::new(),
            None,
            1.2,
        )
        .expect("seek preview jpeg");
        assert!(jpeg.starts_with(&[0xFF, 0xD8]), "missing JPEG SOI");
        assert!(jpeg.len() > 128);
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pip_outer_rect_sits_in_work_area_corner() {
        let (x, y, w, h) = pip_outer_rect(0, 0, 1920, 1040, 1920, 1080);
        assert_eq!((w, h), (480, 270));
        assert_eq!(x, 1920 - 480 - 24);
        assert_eq!(y, 1040 - 270 - 24);
        let (x, y, w, h) = pip_outer_rect(0, 0, 400, 200, 16, 9);
        assert!(x >= 0 && y >= 0);
        assert!(w <= 400 && h <= 200);
        assert!(x + w <= 400 && y + h <= 200);
    }

    #[test]
    fn pip_outer_rect_follows_portrait_video() {
        let (x, y, w, h) = pip_outer_rect(0, 0, 1920, 1080, 1080, 1920);
        assert_eq!((w, h), (270, 480));
        assert!(h > w, "portrait pip must be taller than wide");
        assert_eq!(x, 1920 - 270 - 24);
        assert_eq!(y, 1080 - 480 - 24);
        let (x, y, w, h) = pip_outer_rect(0, 0, 500, 400, 1080, 2340);
        assert!(h > w);
        assert!(x >= 0 && y >= 0);
        assert!(x + w <= 500 && y + h <= 400);
    }

    #[test]
    fn session_summary_never_serializes_token_fields() {
        let summary = SessionSummary {
            provider_id: "mock".into(),
            account_id: Some("account".into()),
            expires_at: Some(123),
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("accessToken"));
        assert!(!json.contains("refreshToken"));
        assert!(json.contains("providerId"));
    }

    #[test]
    fn guangya_library_import_accepts_videos_only() {
        assert!(is_video_media_item(
            "guangya",
            &MediaItem::file("1", "movie.mkv")
        ));
        assert!(!is_video_media_item(
            "guangya",
            &MediaItem::file("2", "cover.jpg")
        ));
        let mut item = MediaItem::file("3", "extensionless");
        item.mime_type = Some("video/mp4".into());
        assert!(is_video_media_item("guangya", &item));
    }

    #[test]
    fn poll_summary_has_stable_tagged_shape() {
        let value = serde_json::to_value(PollResultSummary::Authorized(SessionSummary {
            provider_id: "mock".into(),
            account_id: None,
            expires_at: None,
        }))
        .unwrap();
        assert_eq!(
            value.get("status").and_then(|item| item.as_str()),
            Some("authorized")
        );
        assert!(value.get("providerId").is_some());
    }

    #[test]
    fn preview_source_prefers_short_drama_cache_file() {
        let root = std::env::temp_dir().join(format!("ttv-preview-cache-{}", uuid::Uuid::new_v4()));
        let cache = root.join("short-drama-cache/short-series");
        std::fs::create_dir_all(&cache).unwrap();
        let vid = "7676328260963159065";
        let file = cache.join(format!("{vid}.mp4"));
        std::fs::write(&file, b"not-a-real-mp4").unwrap();
        let resolved = resolve_preview_source(
            "https://example.invalid/play.m3u8",
            Some(&format!("shortdrama:series:{vid}")),
            &root,
        );
        assert_eq!(PathBuf::from(&resolved), file);
        let remote = resolve_preview_source(
            "https://example.invalid/play.m3u8",
            Some("shortdrama:series:missing"),
            &root,
        );
        assert_eq!(remote, "https://example.invalid/play.m3u8");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_media_path_decodes_file_urls_and_rejects_network_sources() {
        assert_eq!(
            decode_local_media_path("file:///D:/Media/My%20Movie.mkv"),
            Some(PathBuf::from(r"D:\Media\My Movie.mkv"))
        );
        assert_eq!(
            decode_local_media_path(r"D:\Media\Movie.mkv"),
            Some(PathBuf::from(r"D:\Media\Movie.mkv"))
        );
        assert!(decode_local_media_path("https://example.invalid/movie.mkv").is_none());
        assert!(decode_local_media_path("").is_none());
    }

    #[test]
    fn percent_decode_handles_utf8_paths_and_invalid_sequences() {
        assert_eq!(
            percent_decode("D:/%E8%A7%86%E9%A2%91/%E7%94%B5%E5%BD%B1.mkv").as_deref(),
            Some("D:/视频/电影.mkv")
        );
        assert!(percent_decode("D:/bad%2").is_none());
        assert!(percent_decode("D:/bad%ZZ").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn bundled_ffmpeg_creates_browser_hls_from_hevc_mkv() {
        let ffmpeg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mpv/ffmpeg.exe");
        if !ffmpeg.is_file() {
            return;
        }
        let root = std::env::temp_dir().join(format!("ttv-hls-test-{}", uuid::Uuid::new_v4()));
        let source = root.join("source-hevc.mkv");
        let output = root.join("hls");
        std::fs::create_dir_all(&root).unwrap();
        let generated = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=12",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=48000",
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-t",
                "2",
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p10le",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(generated.success());

        let mut child = spawn_hls_transcode(
            &ffmpeg,
            source.to_string_lossy().as_ref(),
            &std::collections::BTreeMap::new(),
            &output,
            "libx264",
        )
        .unwrap();
        let mut ready = false;
        for _ in 0..80 {
            if hls_manifest_ready(&output) {
                ready = true;
                break;
            }
            if child.try_wait().unwrap().is_some() {
                ready = hls_manifest_ready(&output);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(ready);
        assert!(std::fs::read_to_string(output.join("index.m3u8"))
            .unwrap()
            .contains("#EXT-X-MAP"));
        let audio_probe = Command::new(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(output.join("index.m3u8"))
            .args(["-map", "0:a:0", "-t", "0.5", "-f", "null", "-"])
            .status()
            .unwrap();
        assert!(
            audio_probe.success(),
            "HLS output did not expose an audio track"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn bundled_ffmpeg_creates_browser_dash_manifest() {
        let ffmpeg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mpv/ffmpeg.exe");
        if !ffmpeg.is_file() {
            return;
        }
        let root = std::env::temp_dir().join(format!("ttv-dash-test-{}", uuid::Uuid::new_v4()));
        let source = root.join("source.mp4");
        let manifest = root.join("stream.mpd");
        std::fs::create_dir_all(&root).unwrap();
        let generated = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=12",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-t",
                "1",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(generated.success());
        let packaged = Command::new(&ffmpeg)
            .current_dir(&root)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&source)
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-c",
                "copy",
                "-seg_duration",
                "1",
                "-use_template",
                "1",
                "-use_timeline",
                "1",
                "-init_seg_name",
                "init-$RepresentationID$.m4s",
                "-media_seg_name",
                "chunk-$RepresentationID$-$Number%05d$.m4s",
                "-f",
                "dash",
            ])
            .arg(&manifest)
            .status()
            .unwrap();
        assert!(packaged.success());
        assert!(manifest.is_file());
        assert!(root.join("init-0.m4s").is_file());
        assert!(root.join("chunk-0-00001.m4s").is_file());
        assert!(std::fs::read_to_string(&manifest).unwrap().contains("<MPD"));
        let inspected = Command::new(&ffmpeg)
            .current_dir(&root)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg("stream.mpd")
            .args(["-map", "0:v:0", "-t", "0.2", "-f", "null", "-"])
            .status()
            .unwrap();
        assert!(inspected.success());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn expired_provider_response_refreshes_and_replays_once() {
        let database = crate::storage::Database::open_in_memory().unwrap();
        let paths = crate::app::AppPaths::from_data_dir(
            std::env::temp_dir().join(format!("ttv-command-test-{}", uuid::Uuid::new_v4())),
        );
        let router = crate::providers::ProviderRouter::new();
        let provider = Arc::new(crate::providers::MockProvider::new().with_expire_next_list());
        let registered: Arc<dyn crate::providers::MediaProvider> = provider.clone();
        router.register_arc(registered).unwrap();
        let runtime = crate::runtime::probe_runtime(crate::runtime::RuntimePaths::from_root(
            paths.data_dir.clone(),
        ));
        let streamhub = crate::app::StreamHubRuntime::new(
            crate::providers::StreamHubConfig::default(),
            paths.data_dir.clone(),
        );
        let openlist = crate::openlist::OpenListRuntime::new(paths.data_dir.clone());
        let state = AppState::new(paths, database, router, runtime, None, streamhub, openlist);
        let session = Session::new("mock", "access", Some("refresh".into()), Some(3600));
        CredentialStore::new(&state.database)
            .save_json("provider.session.mock", &session)
            .unwrap();

        let page = provider_list_files_impl(
            "mock",
            ProviderPageInput {
                parent_id: None,
                page_token: None,
                page_size: 100,
                query: None,
                mark_adult: None,
                folder_path: None,
            },
            &state,
        )
        .await
        .unwrap();

        assert!(!page.files.is_empty());
        assert_eq!(provider.operation_counts(), (1, 2));
    }
}
