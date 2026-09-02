//! Provider abstractions used by the backend.
//!
//! The first release keeps the protocol boundary deliberately transport agnostic.
//! A real provider can use its HTTP client of choice without making the command
//! layer depend on that client. The test-only [`MockProvider`] keeps protocol
//! and command tests deterministic without exposing a fake provider at runtime.

pub mod guangya;
pub use guangya::{GuangyaConfig, GuangyaProvider};
pub mod streamhub;
pub use streamhub::{StreamHubConfig, StreamHubProvider};
pub mod oauth;
pub use oauth::{default_oauth_configs, OAuthProvider, OAuthProviderConfig};

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub device_code_login: bool,
    pub sms_login: bool,
    pub token_import: bool,
    pub session_refresh: bool,
    pub browse_files: bool,
    pub playback_resolution: bool,
}

/// Static source catalog shared by the desktop UI and connection workflow.
/// `implemented` describes the adapter shipped in this build; it is never
/// inferred from a card label, so the UI cannot claim a source is connected
/// before a real protocol adapter and credentials are present.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceDescriptor {
    pub id: String,
    pub name: String,
    pub category: String,
    pub protocol: String,
    pub login_mode: String,
    #[serde(default)]
    pub list_endpoint: Option<String>,
    #[serde(default)]
    pub playback_endpoint: Option<String>,
    #[serde(default)]
    pub icon_asset: Option<String>,
    pub implemented: bool,
    pub browse_files: bool,
    pub playback_resolution: bool,
    pub requires_configuration: bool,
}

pub fn source_catalog() -> Vec<SourceDescriptor> {
    let mut sources = vec![
        source(
            "local",
            "本地磁盘",
            "local",
            "filesystem",
            "folder-picker",
            None,
            None,
            None,
            true,
            true,
            true,
            false,
        ),
        source(
            "streamhub",
            "StreamHub",
            "local",
            "http",
            "local-session",
            Some("/api/library/browse"),
            Some("/api/stream/media-files/{id}/playable"),
            None,
            true,
            true,
            true,
            true,
        ),
        source(
            "openlist",
            "OpenList",
            "network",
            "webdav",
            "basic-or-token",
            Some("PROPFIND /dav"),
            Some("GET /dav/{path}"),
            None,
            false,
            false,
            false,
            true,
        ),
        source(
            "webdav",
            "WebDAV",
            "network",
            "webdav",
            "basic-or-token",
            Some("PROPFIND {baseUrl}"),
            Some("GET {resourceUrl}"),
            None,
            false,
            false,
            false,
            true,
        ),
        source(
            "smb",
            "SMB / NAS",
            "network",
            "smb",
            "username-password",
            Some("SMB directory enumeration"),
            Some("SMB file open"),
            None,
            false,
            false,
            false,
            true,
        ),
        source(
            "sftp",
            "SFTP",
            "network",
            "sftp",
            "ssh-key-or-password",
            Some("SFTP readdir"),
            Some("SFTP open/read"),
            None,
            false,
            false,
            false,
            true,
        ),
        source(
            "cloud123",
            "123云盘",
            "directCloud",
            "cloud",
            "unconfirmed-oauth",
            Some("POST https://api.123278.com/b/api/file/list/new"),
            Some("POST https://api.123278.com/b/api/file/download_info"),
            Some("assets/cloud-providers/123pan.ico"),
            false,
            false,
            false,
            true,
        ),
        source(
            "baidu",
            "百度网盘",
            "directCloud",
            "cloud",
            "browser-oauth",
            Some("GET https://pan.baidu.com/rest/2.0/xpan/file?method=list"),
            Some("GET https://pan.baidu.com/rest/2.0/xpan/multimedia"),
            Some("assets/cloud-providers/baidu.svg"),
            true,
            true,
            true,
            true,
        ),
        source(
            "aliyun",
            "阿里云盘",
            "directCloud",
            "cloud",
            "browser-oauth",
            Some("POST https://openapi.alipan.com/adrive/v1.0/openFile/list"),
            Some("POST https://openapi.alipan.com/adrive/v1.0/openFile/getDownloadUrl"),
            Some("assets/cloud-providers/aliyun.svg"),
            true,
            true,
            true,
            true,
        ),
        source(
            "quark",
            "夸克网盘",
            "directCloud",
            "cloud",
            "unsupported-private-cas",
            Some("GET https://drive-pc.quark.cn/1/clouddrive/file/sort"),
            Some("POST https://drive-pc.quark.cn/1/clouddrive/file/v2/play"),
            Some("assets/cloud-providers/quark.ico"),
            false,
            false,
            false,
            true,
        ),
        source(
            "115",
            "115网盘",
            "directCloud",
            "cloud",
            "unsupported-private-session",
            Some("GET https://aps.115.com/natsort/files.php"),
            Some("POST https://aps.115.com/nd.bizuserres.s/v1/get_res_download_url"),
            Some("assets/cloud-providers/115.ico"),
            false,
            false,
            false,
            true,
        ),
        source(
            "tianyi",
            "天翼云盘",
            "directCloud",
            "cloud",
            "browser-oauth",
            Some("GET https://cloud.189.cn/api/open/file/listFiles.action"),
            Some("GET https://cloud.189.cn/api/open/file/getFileDownloadUrl.action"),
            Some("assets/cloud-providers/tianyi.ico"),
            false,
            false,
            false,
            true,
        ),
        source(
            "guangya",
            "光鸭云盘",
            "directCloud",
            "oauth",
            "device-code-oauth",
            Some("POST https://api.guangyapan.com/userres/v1/file/get_file_list"),
            Some("POST https://api.guangyapan.com/userres/v1/file/get_vod_download_url"),
            Some("assets/cloud-providers/guangya.png"),
            true,
            true,
            true,
            true,
        ),
    ];
    // All non-Guangya sources are represented by OpenList storage drivers.
    // The actual connected/readable state is resolved from OpenList at runtime.
    for source in &mut sources {
        if source.id != "guangya" && source.id != "local" && source.id != "streamhub" {
            source.protocol = "openlist".into();
            source.login_mode = "openlist-storage".into();
            source.list_endpoint = Some("POST /api/fs/list".into());
            source.playback_endpoint = Some("POST /api/fs/get".into());
            source.implemented = true;
            source.browse_files = true;
            source.playback_resolution = true;
        }
    }
    sources
}

fn source(
    id: &str,
    name: &str,
    category: &str,
    protocol: &str,
    login_mode: &str,
    list_endpoint: Option<&str>,
    playback_endpoint: Option<&str>,
    icon_asset: Option<&str>,
    implemented: bool,
    browse_files: bool,
    playback_resolution: bool,
    requires_configuration: bool,
) -> SourceDescriptor {
    SourceDescriptor {
        id: id.into(),
        name: name.into(),
        category: category.into(),
        protocol: protocol.into(),
        login_mode: login_mode.into(),
        list_endpoint: list_endpoint.map(str::to_owned),
        playback_endpoint: playback_endpoint.map(str::to_owned),
        icon_asset: icon_asset.map(str::to_owned),
        implemented,
        browse_files,
        playback_resolution,
        requires_configuration,
    }
}

impl ProviderCapabilities {
    #[cfg(test)]
    pub const fn mock() -> Self {
        Self {
            device_code_login: true,
            sms_login: true,
            token_import: true,
            session_refresh: true,
            browse_files: true,
            playback_resolution: true,
        }
    }
}

#[derive(Debug, Clone, Error, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProviderError {
    #[error("provider does not support: {0}")]
    UnsupportedOperation(String),
    #[error("provider authentication is required")]
    NotAuthenticated,
    #[error("provider authorization is pending")]
    AuthenticationPending,
    #[error("provider authorization was denied")]
    AuthorizationDenied,
    #[error("provider session has expired")]
    SessionExpired,
    #[error("invalid provider input: {0}")]
    InvalidInput(String),
    #[error("provider resource was not found: {0}")]
    NotFound(String),
    #[error("provider rate limit exceeded")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("provider network error: {0}")]
    Network(String),
    #[error("provider protocol error ({code}): {message}")]
    Protocol { code: String, message: String },
    #[error("provider operation was cancelled")]
    Cancelled,
    #[error("provider internal error: {0}")]
    Internal(String),
}

impl From<ProviderError> for crate::error::AppError {
    fn from(value: ProviderError) -> Self {
        Self::Provider(value.to_string())
    }
}

impl ProviderError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedOperation(_) => "provider_unsupported_operation",
            Self::NotAuthenticated => "provider_not_authenticated",
            Self::AuthenticationPending => "provider_authentication_pending",
            Self::AuthorizationDenied => "provider_authorization_denied",
            Self::SessionExpired => "provider_session_expired",
            Self::InvalidInput(_) => "provider_invalid_input",
            Self::NotFound(_) => "provider_not_found",
            Self::RateLimited { .. } => "provider_rate_limited",
            Self::Network(_) => "provider_network_error",
            Self::Protocol { .. } => "provider_protocol_error",
            Self::Cancelled => "provider_cancelled",
            Self::Internal(_) => "provider_internal_error",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::AuthenticationPending
                | Self::RateLimited { .. }
                | Self::Network(_)
                | Self::SessionExpired
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    #[serde(default = "default_poll_interval")]
    pub interval: u64,
}

/// Provider-neutral QR login payload. `qr_text` is the exact value that the
/// frontend may encode; it is never synthesized from a provider name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QrLoginSession {
    pub provider_id: String,
    pub session_id: String,
    pub qr_text: String,
    #[serde(default)]
    pub qr_image: Option<String>,
    pub expires_in: u64,
    #[serde(default = "default_poll_interval")]
    pub interval: u64,
    pub mode: String,
}

impl QrLoginSession {
    pub fn from_device_code(provider_id: &str, device: DeviceCode) -> Self {
        let qr_text = device
            .verification_uri_complete
            .clone()
            .unwrap_or_else(|| device.verification_uri.clone());
        Self {
            provider_id: provider_id.into(),
            session_id: device.device_code,
            qr_text,
            qr_image: None,
            expires_in: device.expires_in,
            interval: device.interval,
            mode: "device-code".into(),
        }
    }
}

fn default_poll_interval() -> u64 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DevicePollRequest {
    pub device_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PollResult {
    Pending { interval: Option<u64> },
    SlowDown { interval: u64 },
    Authorized(Session),
    Denied,
    Expired,
}

impl PollResult {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Authorized(_) | Self::Denied | Self::Expired)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub provider_id: String,
    #[serde(default)]
    pub account_id: Option<String>,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix timestamp in seconds at which the access token expires.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

impl Session {
    pub fn new(
        provider_id: impl Into<String>,
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        expires_in_secs: Option<u64>,
    ) -> Self {
        let expires_at = expires_in_secs.map(|seconds| now_unix().saturating_add(seconds as i64));
        Self {
            provider_id: provider_id.into(),
            account_id: None,
            access_token: access_token.into(),
            refresh_token,
            expires_at,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.is_expired_at(now_unix(), 0)
    }

    pub fn is_expired_at(&self, now: i64, leeway_secs: i64) -> bool {
        self.expires_at
            .map(|expires_at| expires_at <= now.saturating_add(leeway_secs))
            .unwrap_or(false)
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Session")
            .field("provider_id", &self.provider_id)
            .field("account_id", &self.account_id)
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenImport {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

impl fmt::Debug for TokenImport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenImport")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("account_id", &self.account_id)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SmsLoginRequest {
    pub phone: String,
    pub code: String,
    #[serde(default)]
    pub verification_id: Option<String>,
    #[serde(default)]
    pub verification_token: Option<String>,
    #[serde(default)]
    pub captcha_token: Option<String>,
}

impl fmt::Debug for SmsLoginRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmsLoginRequest")
            .field("phone", &self.phone)
            .field("code", &"[REDACTED]")
            .field("verification_id", &self.verification_id)
            .field(
                "verification_token",
                &self.verification_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "captcha_token",
                &self.captcha_token.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListFilesRequest {
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub page_token: Option<String>,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    File,
    Folder,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub id: String,
    pub name: String,
    pub kind: MediaKind,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub duration_seconds: Option<f64>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl MediaItem {
    pub fn file(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind: MediaKind::File,
            parent_id: None,
            size_bytes: None,
            mime_type: None,
            duration_seconds: None,
            thumbnail_url: None,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn folder(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: MediaKind::Folder,
            ..Self::file(id, name)
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FilePage {
    pub files: Vec<MediaItem>,
    #[serde(default)]
    pub next_page_token: Option<String>,
    #[serde(default)]
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackRequest {
    pub media_id: String,
    #[serde(default)]
    pub quality: Option<String>,
}

/// One selectable resolution/quality variant of a cloud video file.
///
/// Guangya exposes every resolution as an independent `gcid` resource, so a
/// quality switch means re-resolving the signed URL for another gcid and
/// seeking back to the current position (mirrors the official desktop player).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VideoQuality {
    /// Resource id that `PlaybackRequest::quality` must carry to select this
    /// variant when re-resolving playback.
    pub gcid: String,
    /// Numeric definition id following the official client mapping
    /// (1080 / 2000 / 4000 / 10000 for 原画 …), 0 when unknown.
    pub definition_id: u64,
    pub resolution_name: Option<String>,
    /// Human label such as “1080P 超清”.
    pub display_name: String,
    pub short_name: Option<String>,
    /// 0 unknown, 1 free/limited, 2 requires VIP, 3 free.
    pub need_vip_type: Option<i64>,
    /// 0 原画, 1 转码, 2 转码plus, 3 无损.
    pub source: Option<i64>,
    pub duration_seconds: Option<f64>,
    pub is_default: bool,
}

/// A subtitle candidate discovered by a provider (online catalog or files
/// stored next to the video in the cloud drive).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSubtitle {
    /// Opaque id the caller must pass back to [`MediaProvider::download_subtitle`].
    pub id: String,
    pub name: String,
    /// `online` (server-side catalog) or `cloud` (subtitle file in the drive).
    pub source: String,
    pub ext: String,
    /// Direct URL, when the provider exposes one without another round trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderSubtitleSearchRequest {
    pub media_id: String,
    /// File name (not title) of the video; the Guangya online catalog matches
    /// on it.
    pub name: Option<String>,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ProviderSubtitleDownloaded {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackDescriptor {
    pub source: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    pub media_id: String,
    pub outcome: String,
    /// All selectable variants discovered for this media, so the UI can offer
    /// a real quality switch without another detail round trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualities: Option<Vec<VideoQuality>>,
}

impl fmt::Debug for PlaybackDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers = self.headers.keys().map(String::as_str).collect::<Vec<_>>();
        formatter
            .debug_struct("PlaybackDescriptor")
            .field("source", &self.source)
            .field("url", &"[REDACTED]")
            .field("headers", &headers)
            .field("quality", &self.quality)
            .field("expires_at", &self.expires_at)
            .field("media_id", &self.media_id)
            .field("outcome", &self.outcome)
            .field(
                "qualities",
                &self.qualities.as_ref().map(|items| items.len()),
            )
            .finish()
    }
}

#[async_trait]
pub trait MediaProvider: Send + Sync {
    fn id(&self) -> &'static str;
    /// Alias retained for callers that use the architecture document's
    /// `provider_id` terminology.
    fn provider_id(&self) -> &'static str {
        self.id()
    }
    fn capabilities(&self) -> ProviderCapabilities;

    fn authorization_url(&self, _state: Option<&str>) -> Result<String, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "provider does not expose OAuth authorization URL".into(),
        ))
    }

    async fn exchange_authorization_code(&self, _code: String) -> Result<Session, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "provider does not expose OAuth authorization-code exchange".into(),
        ))
    }

    /// Restore a previously persisted session after application restart.
    /// Providers without in-memory session state may keep the default no-op.
    async fn restore_session(&self, _session: Session) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn clear_session(&self) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn login_device_code(&self) -> Result<DeviceCode, ProviderError>;
    async fn request_device_code(&self) -> Result<DeviceCode, ProviderError> {
        self.login_device_code().await
    }
    async fn poll_device_token(
        &self,
        request: DevicePollRequest,
    ) -> Result<PollResult, ProviderError>;
    async fn login_sms(&self, request: SmsLoginRequest) -> Result<Session, ProviderError>;
    async fn import_token(&self, input: TokenImport) -> Result<Session, ProviderError>;
    async fn refresh_session(&self, session: &Session) -> Result<Session, ProviderError>;
    async fn list_files(&self, request: ListFilesRequest) -> Result<FilePage, ProviderError>;
    async fn resolve_playback(
        &self,
        request: PlaybackRequest,
    ) -> Result<PlaybackDescriptor, ProviderError>;

    /// Enumerate selectable quality variants for a media file. Providers that
    /// only expose a single stream return `UnsupportedOperation`.
    async fn video_qualities(&self, _media_id: &str) -> Result<Vec<VideoQuality>, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "provider does not expose selectable video qualities".into(),
        ))
    }

    /// Search provider-side subtitles: an online catalog and/or subtitle
    /// files stored next to the video inside the cloud drive.
    async fn search_subtitles(
        &self,
        _request: ProviderSubtitleSearchRequest,
    ) -> Result<Vec<ProviderSubtitle>, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "provider does not expose subtitle search".into(),
        ))
    }

    /// Fetch the bytes of one subtitle candidate returned by
    /// [`MediaProvider::search_subtitles`]. Writing to the local cache is the
    /// caller's responsibility.
    async fn download_subtitle(
        &self,
        _subtitle: &ProviderSubtitle,
    ) -> Result<ProviderSubtitleDownloaded, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "provider does not expose subtitle download".into(),
        ))
    }
}

#[derive(Clone, Default)]
pub struct ProviderRouter {
    providers: Arc<RwLock<HashMap<&'static str, Arc<dyn MediaProvider>>>>,
}

impl fmt::Debug for ProviderRouter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRouter")
            .field("providers", &self.ids())
            .finish()
    }
}

impl ProviderRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<P>(&self, provider: P) -> Result<(), ProviderError>
    where
        P: MediaProvider + 'static,
    {
        self.register_arc(Arc::new(provider))
    }

    pub fn register_arc(&self, provider: Arc<dyn MediaProvider>) -> Result<(), ProviderError> {
        let id = provider.id();
        if id.trim().is_empty() {
            return Err(ProviderError::InvalidInput(
                "provider id cannot be empty".to_owned(),
            ));
        }
        self.providers
            .write()
            .map_err(|_| ProviderError::Internal("provider registry lock poisoned".to_owned()))?
            .insert(id, provider);
        Ok(())
    }

    pub fn remove(&self, provider_id: &str) -> Result<bool, ProviderError> {
        Ok(self
            .providers
            .write()
            .map_err(|_| ProviderError::Internal("provider registry lock poisoned".to_owned()))?
            .remove(provider_id)
            .is_some())
    }

    pub fn ids(&self) -> Vec<String> {
        let Ok(providers) = self.providers.read() else {
            return Vec::new();
        };
        let mut ids = providers
            .keys()
            .map(|id| (*id).to_owned())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub fn capabilities(&self, provider_id: &str) -> Result<ProviderCapabilities, ProviderError> {
        Ok(self.provider(provider_id)?.capabilities())
    }

    pub fn authorization_url(
        &self,
        provider_id: &str,
        state: Option<&str>,
    ) -> Result<String, ProviderError> {
        self.provider(provider_id)?.authorization_url(state)
    }

    pub async fn exchange_authorization_code(
        &self,
        provider_id: &str,
        code: String,
    ) -> Result<Session, ProviderError> {
        self.provider(provider_id)?
            .exchange_authorization_code(code)
            .await
    }

    pub fn provider(&self, provider_id: &str) -> Result<Arc<dyn MediaProvider>, ProviderError> {
        self.providers
            .read()
            .map_err(|_| ProviderError::Internal("provider registry lock poisoned".to_owned()))?
            .get(provider_id)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound(format!("provider '{provider_id}'")))
    }

    pub async fn login_device_code(&self, provider_id: &str) -> Result<DeviceCode, ProviderError> {
        self.provider(provider_id)?.login_device_code().await
    }

    pub async fn create_qr_login(
        &self,
        provider_id: &str,
    ) -> Result<QrLoginSession, ProviderError> {
        let device = self.provider(provider_id)?.login_device_code().await?;
        Ok(QrLoginSession::from_device_code(provider_id, device))
    }

    pub async fn restore_session(
        &self,
        provider_id: &str,
        session: Session,
    ) -> Result<(), ProviderError> {
        self.provider(provider_id)?.restore_session(session).await
    }

    pub async fn clear_session(&self, provider_id: &str) -> Result<(), ProviderError> {
        self.provider(provider_id)?.clear_session().await
    }

    pub async fn poll_device_token(
        &self,
        provider_id: &str,
        request: DevicePollRequest,
    ) -> Result<PollResult, ProviderError> {
        self.provider(provider_id)?.poll_device_token(request).await
    }

    pub async fn login_sms(
        &self,
        provider_id: &str,
        request: SmsLoginRequest,
    ) -> Result<Session, ProviderError> {
        self.provider(provider_id)?.login_sms(request).await
    }

    pub async fn import_token(
        &self,
        provider_id: &str,
        input: TokenImport,
    ) -> Result<Session, ProviderError> {
        self.provider(provider_id)?.import_token(input).await
    }

    pub async fn refresh_session(
        &self,
        provider_id: &str,
        session: &Session,
    ) -> Result<Session, ProviderError> {
        self.provider(provider_id)?.refresh_session(session).await
    }

    pub async fn list_files(
        &self,
        provider_id: &str,
        request: ListFilesRequest,
    ) -> Result<FilePage, ProviderError> {
        self.provider(provider_id)?.list_files(request).await
    }

    pub async fn resolve_playback(
        &self,
        provider_id: &str,
        request: PlaybackRequest,
    ) -> Result<PlaybackDescriptor, ProviderError> {
        self.provider(provider_id)?.resolve_playback(request).await
    }

    pub async fn video_qualities(
        &self,
        provider_id: &str,
        media_id: &str,
    ) -> Result<Vec<VideoQuality>, ProviderError> {
        self.provider(provider_id)?.video_qualities(media_id).await
    }

    pub async fn search_subtitles(
        &self,
        provider_id: &str,
        request: ProviderSubtitleSearchRequest,
    ) -> Result<Vec<ProviderSubtitle>, ProviderError> {
        self.provider(provider_id)?.search_subtitles(request).await
    }

    pub async fn download_subtitle(
        &self,
        provider_id: &str,
        subtitle: &ProviderSubtitle,
    ) -> Result<ProviderSubtitleDownloaded, ProviderError> {
        self.provider(provider_id)?
            .download_subtitle(subtitle)
            .await
    }
}

#[cfg(test)]
const MOCK_PROVIDER_ID: &str = "mock";

#[cfg(test)]
#[derive(Debug)]
struct MockState {
    session: Option<Session>,
    device_code: Option<DeviceCode>,
    poll_count: usize,
    polls_before_authorized: usize,
    files: Vec<MediaItem>,
    expire_next_list: bool,
    refresh_count: usize,
    list_count: usize,
}

#[cfg(test)]
pub struct MockProvider {
    state: RwLock<MockState>,
}

#[cfg(test)]
impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl fmt::Debug for MockProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MockProvider")
            .field("id", &self.id())
            .finish()
    }
}

#[cfg(test)]
impl MockProvider {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(MockState {
                session: None,
                device_code: None,
                poll_count: 0,
                polls_before_authorized: 1,
                files: vec![
                    MediaItem::folder("demo-folder", "Demo Library"),
                    MediaItem::file("demo-video", "Demo Video.mkv"),
                ],
                expire_next_list: false,
                refresh_count: 0,
                list_count: 0,
            }),
        }
    }

    pub fn with_files(self, files: Vec<MediaItem>) -> Self {
        if let Ok(mut state) = self.state.write() {
            state.files = files;
        }
        self
    }

    pub fn with_polls_before_authorized(self, attempts: usize) -> Self {
        if let Ok(mut state) = self.state.write() {
            state.polls_before_authorized = attempts;
        }
        self
    }

    pub fn with_session(self, session: Session) -> Self {
        if let Ok(mut state) = self.state.write() {
            state.session = Some(session);
        }
        self
    }

    pub fn with_expire_next_list(self) -> Self {
        if let Ok(mut state) = self.state.write() {
            state.expire_next_list = true;
        }
        self
    }

    pub fn operation_counts(&self) -> (usize, usize) {
        self.state
            .read()
            .map(|state| (state.refresh_count, state.list_count))
            .unwrap_or_default()
    }

    pub fn session(&self) -> Option<Session> {
        self.state
            .read()
            .ok()
            .and_then(|state| state.session.clone())
    }

    fn read_state(&self) -> Result<std::sync::RwLockReadGuard<'_, MockState>, ProviderError> {
        self.state
            .read()
            .map_err(|_| ProviderError::Internal("mock provider lock poisoned".to_owned()))
    }

    fn write_state(&self) -> Result<std::sync::RwLockWriteGuard<'_, MockState>, ProviderError> {
        self.state
            .write()
            .map_err(|_| ProviderError::Internal("mock provider lock poisoned".to_owned()))
    }

    fn require_session(&self) -> Result<Session, ProviderError> {
        let session = self
            .read_state()?
            .session
            .clone()
            .ok_or(ProviderError::NotAuthenticated)?;
        if session.is_expired() {
            return Err(ProviderError::SessionExpired);
        }
        Ok(session)
    }
}

#[cfg(test)]
#[async_trait]
impl MediaProvider for MockProvider {
    fn id(&self) -> &'static str {
        MOCK_PROVIDER_ID
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::mock()
    }

    async fn restore_session(&self, session: Session) -> Result<(), ProviderError> {
        if session.provider_id != self.id() {
            return Err(ProviderError::InvalidInput(
                "session belongs to another provider".into(),
            ));
        }
        self.write_state()?.session = Some(session);
        Ok(())
    }

    async fn login_device_code(&self) -> Result<DeviceCode, ProviderError> {
        let response = DeviceCode {
            device_code: "mock-device-code".to_owned(),
            user_code: "MOCK-1234".to_owned(),
            verification_uri: "https://mock.invalid/device".to_owned(),
            verification_uri_complete: Some(
                "https://mock.invalid/device?code=MOCK-1234".to_owned(),
            ),
            expires_in: 600,
            interval: 1,
        };
        let mut state = self.write_state()?;
        state.device_code = Some(response.clone());
        state.poll_count = 0;
        Ok(response)
    }

    async fn poll_device_token(
        &self,
        request: DevicePollRequest,
    ) -> Result<PollResult, ProviderError> {
        let mut state = self.write_state()?;
        let expected = state
            .device_code
            .as_ref()
            .ok_or_else(|| ProviderError::InvalidInput("device code was not requested".to_owned()))?
            .clone();
        if request.device_code != expected.device_code {
            return Err(ProviderError::InvalidInput(
                "unknown device code".to_owned(),
            ));
        }
        state.poll_count = state.poll_count.saturating_add(1);
        if state.poll_count <= state.polls_before_authorized {
            return Ok(PollResult::Pending {
                interval: Some(expected.interval),
            });
        }
        let session = Session::new(
            self.id(),
            "mock-access-token",
            Some("mock-refresh-token".to_owned()),
            Some(3600),
        );
        state.session = Some(session.clone());
        Ok(PollResult::Authorized(session))
    }

    async fn login_sms(&self, request: SmsLoginRequest) -> Result<Session, ProviderError> {
        if request.phone.trim().is_empty() || request.code.trim().is_empty() {
            return Err(ProviderError::InvalidInput(
                "phone and verification code are required".to_owned(),
            ));
        }
        let session = Session::new(
            self.id(),
            "mock-sms-access-token",
            Some("mock-sms-refresh-token".to_owned()),
            Some(3600),
        );
        self.write_state()?.session = Some(session.clone());
        Ok(session)
    }

    async fn import_token(&self, input: TokenImport) -> Result<Session, ProviderError> {
        if input.access_token.trim().is_empty() {
            return Err(ProviderError::InvalidInput(
                "access token is required".to_owned(),
            ));
        }
        let session = Session {
            provider_id: self.id().to_owned(),
            account_id: input.account_id,
            access_token: input.access_token,
            refresh_token: input.refresh_token,
            expires_at: input.expires_at,
        };
        self.write_state()?.session = Some(session.clone());
        Ok(session)
    }

    async fn refresh_session(&self, session: &Session) -> Result<Session, ProviderError> {
        if session.provider_id != self.id() {
            return Err(ProviderError::InvalidInput(
                "session belongs to another provider".to_owned(),
            ));
        }
        if session.refresh_token.is_none() {
            return Err(ProviderError::SessionExpired);
        }
        {
            let mut state = self.write_state()?;
            state.refresh_count = state.refresh_count.saturating_add(1);
        }
        let refreshed = Session::new(
            self.id(),
            format!("{}-refreshed", session.access_token),
            session.refresh_token.clone(),
            Some(3600),
        );
        self.write_state()?.session = Some(refreshed.clone());
        Ok(refreshed)
    }

    async fn list_files(&self, request: ListFilesRequest) -> Result<FilePage, ProviderError> {
        self.require_session()?;
        {
            let mut state = self.write_state()?;
            state.list_count = state.list_count.saturating_add(1);
            if state.expire_next_list {
                state.expire_next_list = false;
                return Err(ProviderError::SessionExpired);
            }
        }
        let state = self.read_state()?;
        let query = request.query.as_deref().map(str::to_lowercase);
        let page_size = request.page_size.unwrap_or(100).clamp(1, 500) as usize;
        let mut files = state
            .files
            .iter()
            .filter(|file| request.parent_id.as_deref() == file.parent_id.as_deref())
            .filter(|file| {
                query
                    .as_deref()
                    .map(|query| file.name.to_lowercase().contains(query))
                    .unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        let offset = request
            .page_token
            .as_deref()
            .and_then(|token| token.parse::<usize>().ok())
            .unwrap_or(0);
        let total = files.len() as u64;
        let next_page_token = if offset.saturating_add(page_size) < files.len() {
            Some((offset + page_size).to_string())
        } else {
            None
        };
        files = files.into_iter().skip(offset).take(page_size).collect();
        Ok(FilePage {
            files,
            next_page_token,
            total: Some(total),
        })
    }

    async fn resolve_playback(
        &self,
        request: PlaybackRequest,
    ) -> Result<PlaybackDescriptor, ProviderError> {
        self.require_session()?;
        let state = self.read_state()?;
        let item = state
            .files
            .iter()
            .find(|file| file.id == request.media_id)
            .ok_or_else(|| ProviderError::NotFound(request.media_id.clone()))?;
        if item.kind != MediaKind::File {
            return Err(ProviderError::InvalidInput(
                "folders cannot be played".to_owned(),
            ));
        }
        Ok(PlaybackDescriptor {
            source: self.id().to_owned(),
            url: format!("https://mock.invalid/stream/{}", item.id),
            headers: BTreeMap::new(),
            quality: request.quality,
            expires_at: Some(now_unix().saturating_add(300)),
            media_id: item.id.clone(),
            outcome: "ok".to_owned(),
            qualities: None,
        })
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_catalog_contains_all_lumiplayer_sources_without_claiming_unimplemented_adapters() {
        let ids = source_catalog()
            .into_iter()
            .map(|source| source.id)
            .collect::<std::collections::BTreeSet<_>>();
        for id in [
            "local",
            "streamhub",
            "openlist",
            "webdav",
            "smb",
            "sftp",
            "cloud123",
            "baidu",
            "aliyun",
            "quark",
            "115",
            "tianyi",
            "guangya",
        ] {
            assert!(ids.contains(id), "missing source {id}");
        }
    }

    fn session() -> Session {
        Session::new("mock", "access", Some("refresh".to_owned()), Some(3600))
    }

    #[test]
    fn session_debug_redacts_tokens() {
        let output = format!("{:?}", session());
        assert!(!output.contains("\"access\""));
        assert!(!output.contains("\"refresh\""));
        assert!(output.contains("REDACTED"));
    }

    #[tokio::test]
    async fn mock_device_login_pending_then_authorized() {
        let provider = MockProvider::new().with_polls_before_authorized(1);
        let device = provider.login_device_code().await.unwrap();
        let pending = provider
            .poll_device_token(DevicePollRequest {
                device_code: device.device_code.clone(),
            })
            .await
            .unwrap();
        assert_eq!(pending, PollResult::Pending { interval: Some(1) });
        let authorized = provider
            .poll_device_token(DevicePollRequest {
                device_code: device.device_code,
            })
            .await
            .unwrap();
        assert!(matches!(authorized, PollResult::Authorized(_)));
    }

    #[tokio::test]
    async fn router_qr_login_wraps_device_code_without_exposing_tokens() {
        let router = ProviderRouter::new();
        router.register(MockProvider::new()).unwrap();
        let qr = router.create_qr_login("mock").await.unwrap();
        assert_eq!(qr.provider_id, "mock");
        assert_eq!(qr.mode, "device-code");
        assert!(qr.qr_text.starts_with("https://mock.invalid/device"));
        assert!(!qr.qr_text.contains("access"));
        assert!(!qr.qr_text.contains("refresh"));
    }

    #[tokio::test]
    async fn mock_lists_and_resolves_files_after_login() {
        let provider = MockProvider::new().with_session(session());
        let page = provider
            .list_files(ListFilesRequest {
                query: Some("video".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.files.len(), 1);
        let descriptor = provider
            .resolve_playback(PlaybackRequest {
                media_id: "demo-video".to_owned(),
                quality: Some("1080p".to_owned()),
            })
            .await
            .unwrap();
        assert_eq!(descriptor.source, "mock");
        assert_eq!(descriptor.quality.as_deref(), Some("1080p"));
    }

    #[tokio::test]
    async fn mock_restores_persisted_session() {
        let provider = MockProvider::new();
        provider.restore_session(session()).await.unwrap();
        let page = provider
            .list_files(ListFilesRequest::default())
            .await
            .unwrap();
        assert!(!page.files.is_empty());
    }

    #[tokio::test]
    async fn router_dispatches_and_reports_unknown_provider() {
        let router = ProviderRouter::new();
        router
            .register(MockProvider::new().with_session(session()))
            .unwrap();
        assert_eq!(router.ids(), vec!["mock"]);
        assert!(router.capabilities("mock").unwrap().playback_resolution);
        let error = router
            .list_files("missing", ListFilesRequest::default())
            .await
            .unwrap_err();
        assert!(matches!(error, ProviderError::NotFound(_)));
    }

    #[test]
    fn provider_error_maps_to_app_error() {
        let error: crate::error::AppError = ProviderError::SessionExpired.into();
        assert_eq!(error.code(), "provider_error");
        assert!(error.retryable());
    }
}
