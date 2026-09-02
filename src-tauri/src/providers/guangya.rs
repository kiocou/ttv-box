//! Configuration-driven Guangya OAuth provider.
//!
//! Only the device-code and refresh portions are implemented here. Guangya
//! file-list and direct-link payloads are intentionally left behind the
//! provider boundary until they are confirmed by a captured response.

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use super::{
    DeviceCode, DevicePollRequest, FilePage, ListFilesRequest, MediaProvider, PlaybackDescriptor,
    PlaybackRequest, PollResult, ProviderCapabilities, ProviderError, ProviderSubtitle,
    ProviderSubtitleDownloaded, ProviderSubtitleSearchRequest, Session, SmsLoginRequest,
    TokenImport, VideoQuality,
};

const PROVIDER_ID: &str = "guangya";
const PLAYBACK_DETAIL_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone)]
struct CachedPlaybackDetail {
    resources: Vec<VideoQuality>,
    fallback_gcid: Option<String>,
    cached_at: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct GuangyaConfig {
    pub account_base_url: String,
    pub api_base_url: String,
    pub device_code_path: String,
    pub token_path: String,
    pub client_id: Option<String>,
    /// Public OAuth client used by the Guangya web login page. This is
    /// separate from the developer client id used by token-upload APIs.
    pub oauth_client_id: Option<String>,
    pub scope: Option<String>,
    /// Confirmed by the public Guangya web client (`/v1/auth/token`).
    pub device_code_grant_type: Option<String>,
    /// Confirmed by the public Guangya web client (`/v1/auth/token`).
    pub refresh_grant_type: Option<String>,
    pub user_agent: Option<String>,
    pub headers: BTreeMap<String, String>,
}

impl Default for GuangyaConfig {
    fn default() -> Self {
        Self {
            account_base_url: "https://account.guangyapan.com".into(),
            api_base_url: "https://api.guangyapan.com".into(),
            device_code_path: "/v1/auth/device/code".into(),
            token_path: "/v1/auth/token".into(),
            client_id: None,
            oauth_client_id: Some("aMe-8VSlkrbQXpUR".into()),
            scope: None,
            device_code_grant_type: Some("urn:ietf:params:oauth:grant-type:device_code".into()),
            refresh_grant_type: Some("refresh_token".into()),
            user_agent: None,
            headers: BTreeMap::new(),
        }
    }
}

impl GuangyaConfig {
    fn effective_oauth_client_id(&self) -> Option<&str> {
        self.oauth_client_id
            .as_deref()
            .or(self.client_id.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub fn oauth_configured(&self) -> bool {
        self.oauth_missing_fields().is_empty()
    }

    /// Only reports configuration keys, never their values. This lets the UI
    /// guide setup without exposing OAuth metadata or account credentials.
    pub fn oauth_missing_fields(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.effective_oauth_client_id().is_none() {
            missing.push("clientId");
        }
        if self.device_code_path.trim().is_empty() {
            missing.push("deviceCodePath");
        }
        if self.token_path.trim().is_empty() {
            missing.push("tokenPath");
        }
        if self
            .device_code_grant_type
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
        {
            missing.push("deviceCodeGrantType");
        }
        if self
            .refresh_grant_type
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
        {
            missing.push("refreshGrantType");
        }
        missing
    }

    fn endpoint(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.account_base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    fn api_endpoint(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.api_base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

#[derive(Clone)]
pub struct GuangyaProvider {
    client: Client,
    config: GuangyaConfig,
    session: Arc<RwLock<Option<Session>>>,
    playback_details: Arc<RwLock<HashMap<String, CachedPlaybackDetail>>>,
    device_id: String,
}

impl std::fmt::Debug for GuangyaProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuangyaProvider")
            .field("id", &PROVIDER_ID)
            .field("account_base_url", &self.config.account_base_url)
            .field("oauth_configured", &self.config.oauth_configured())
            .finish()
    }
}

impl Default for GuangyaProvider {
    fn default() -> Self {
        Self::new(GuangyaConfig::default()).expect("default Guangya HTTP client must build")
    }
}

impl GuangyaProvider {
    pub fn new(config: GuangyaConfig) -> Result<Self, ProviderError> {
        let mut builder = Client::builder().timeout(Duration::from_secs(15));
        if let Some(user_agent) = config
            .user_agent
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            builder = builder.user_agent(user_agent.to_owned());
        }
        let client = builder.build().map_err(|error| {
            ProviderError::Internal(format!("cannot build HTTP client: {error}"))
        })?;
        Ok(Self {
            client,
            config,
            session: Arc::new(RwLock::new(None)),
            playback_details: Arc::new(RwLock::new(HashMap::new())),
            device_id: uuid::Uuid::new_v4().to_string(),
        })
    }

    fn cached_playback_detail(
        &self,
        media_id: &str,
    ) -> Result<Option<CachedPlaybackDetail>, ProviderError> {
        let mut cache = self
            .playback_details
            .write()
            .map_err(|_| ProviderError::Internal("Guangya playback cache lock poisoned".into()))?;
        let fresh = cache
            .get(media_id)
            .filter(|entry| entry.cached_at.elapsed() < PLAYBACK_DETAIL_CACHE_TTL)
            .cloned();
        if fresh.is_none() {
            cache.remove(media_id);
        }
        Ok(fresh)
    }

    fn cache_playback_detail(
        &self,
        media_id: &str,
        detail: CachedPlaybackDetail,
    ) -> Result<(), ProviderError> {
        self.playback_details
            .write()
            .map_err(|_| ProviderError::Internal("Guangya playback cache lock poisoned".into()))?
            .insert(media_id.to_owned(), detail);
        Ok(())
    }

    async fn playback_detail(&self, media_id: &str) -> Result<CachedPlaybackDetail, ProviderError> {
        if let Some(detail) = self.cached_playback_detail(media_id)? {
            return Ok(detail);
        }
        let detail = self
            .api_post(
                "/userres/v1/file/get_file_detail",
                json!({"fileId": media_id}),
            )
            .await?;
        let cached = CachedPlaybackDetail {
            resources: parse_video_resources(&detail),
            fallback_gcid: detail
                .get("fileInfo")
                .and_then(|info| value_string(info, &["gcid", "mediaId", "media_id"])),
            cached_at: Instant::now(),
        };
        self.cache_playback_detail(media_id, cached.clone())?;
        Ok(cached)
    }

    fn request_builder(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let request = request
            .header("accept", "application/json, text/plain, */*")
            .header("content-type", "application/json")
            .header("origin", "https://www.guangyapan.com")
            .header("referer", "https://www.guangyapan.com/")
            .header("user-agent", self.config.user_agent.as_deref().unwrap_or(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/147.0.0.0 Safari/537.36",
            ));
        self.config
            .headers
            .iter()
            .fold(request, |request, (name, value)| {
                request.header(name, value)
            })
    }

    fn authenticated_request(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let session = self
            .session
            .read()
            .map_err(|_| ProviderError::Internal("Guangya session lock poisoned".into()))?
            .clone()
            .ok_or(ProviderError::NotAuthenticated)?;
        if session.is_expired() {
            return Err(ProviderError::SessionExpired);
        }
        let trace_id = uuid::Uuid::new_v4().simple().to_string();
        let span_id = &trace_id[..16];
        Ok(self
            .request_builder(request)
            .bearer_auth(session.access_token)
            .header("dt", "4")
            .header("did", self.device_id.clone())
            .header("traceparent", format!("00-{trace_id}-{span_id}-01")))
    }

    fn set_session(&self, session: Session) -> Result<(), ProviderError> {
        if session.provider_id != PROVIDER_ID {
            return Err(ProviderError::InvalidInput(
                "session belongs to another provider".into(),
            ));
        }
        *self
            .session
            .write()
            .map_err(|_| ProviderError::Internal("Guangya session lock poisoned".into()))? =
            Some(session);
        Ok(())
    }

    async fn api_post(&self, path: &str, body: Value) -> Result<Value, ProviderError> {
        let request = self
            .authenticated_request(self.client.post(self.config.api_endpoint(path)))?
            .json(&body);
        let response = request
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let status = response.status();
        let body = response.json::<Value>().await.map_err(|error| {
            ProviderError::Network(format!("invalid Guangya API response: {error}"))
        })?;
        let code = body.get("code").and_then(value_i64).unwrap_or(0);
        if !status.is_success() || code != 0 {
            return Err(map_protocol_error(status, &body));
        }
        Ok(body.get("data").cloned().unwrap_or(body))
    }

    fn require_oauth_config(&self) -> Result<&str, ProviderError> {
        if !self.config.oauth_configured() {
            return Err(ProviderError::UnsupportedOperation(
                "Guangya OAuth protocol is not configured".into(),
            ));
        }
        self.config.effective_oauth_client_id().ok_or_else(|| {
            ProviderError::UnsupportedOperation("Guangya client_id is not configured".into())
        })
    }

    fn device_code_payload(&self) -> Result<serde_json::Map<String, Value>, ProviderError> {
        let client_id = self.require_oauth_config()?;
        let mut payload = serde_json::Map::new();
        payload.insert("client_id".into(), Value::String(client_id.into()));
        if let Some(scope) = self
            .config
            .scope
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            payload.insert("scope".into(), Value::String(scope.into()));
        }
        Ok(payload)
    }

    fn configured_grant_type<'a>(grant_type: Option<&'a str>) -> Result<&'a str, ProviderError> {
        grant_type
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ProviderError::UnsupportedOperation(
                    "Guangya OAuth grant type is not configured".into(),
                )
            })
    }

    async fn response_value(&self, response: reqwest::Response) -> Result<Value, ProviderError> {
        let status = response.status();
        let body = response.json::<Value>().await.map_err(|error| {
            ProviderError::Network(format!("invalid Guangya response: {error}"))
        })?;
        response_payload(status, body)
    }

    async fn response_json<T: for<'de> Deserialize<'de>>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, ProviderError> {
        let body = self.response_value(response).await?;
        serde_json::from_value(body).map_err(|error| ProviderError::Protocol {
            code: "invalid_response".into(),
            message: error.to_string(),
        })
    }
}

fn response_payload(status: StatusCode, body: Value) -> Result<Value, ProviderError> {
    let code = body.get("code").and_then(value_i64);
    let has_error = body
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    if !status.is_success() || has_error || code.is_some_and(|value| value != 0) {
        return Err(map_protocol_error(status, &body));
    }
    Ok(body
        .get("data")
        .filter(|value| value.is_object() || value.is_array())
        .cloned()
        .unwrap_or(body))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceCodeWire {
    #[serde(alias = "device_code")]
    device_code: String,
    #[serde(alias = "user_code")]
    user_code: String,
    #[serde(default)]
    #[serde(alias = "verification_uri")]
    verification_uri: Option<String>,
    #[serde(default)]
    #[serde(alias = "verification_url")]
    verification_url: Option<String>,
    #[serde(default)]
    #[serde(alias = "verification_uri_complete")]
    verification_uri_complete: Option<String>,
    #[serde(default = "default_expires_in")]
    #[serde(alias = "expires_in")]
    expires_in: u64,
    #[serde(default = "default_interval")]
    #[serde(alias = "interval")]
    interval: u64,
}

fn default_expires_in() -> u64 {
    600
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenWire {
    #[serde(alias = "access_token")]
    access_token: String,
    #[serde(default, alias = "refresh_token")]
    refresh_token: Option<String>,
    #[serde(default, alias = "expires_at")]
    expires_at: Option<i64>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default, alias = "account_id", alias = "user_id")]
    account_id: Option<String>,
}

impl TokenWire {
    fn from_value(value: Value) -> Result<Self, ProviderError> {
        let access_token =
            value_string(&value, &["accessToken", "access_token"]).ok_or_else(|| {
                ProviderError::Protocol {
                    code: "missing_access_token".into(),
                    message: "token response did not contain an access token".into(),
                }
            })?;
        Ok(Self {
            access_token,
            refresh_token: value_string(&value, &["refreshToken", "refresh_token"]),
            expires_at: value
                .get("expiresAt")
                .or_else(|| value.get("expires_at"))
                .and_then(value_i64),
            expires_in: value
                .get("expiresIn")
                .or_else(|| value.get("expires_in"))
                .and_then(value_u64),
            account_id: value_string(&value, &["accountId", "account_id", "userId", "user_id"]),
        })
    }

    fn into_session(self) -> Result<Session, ProviderError> {
        if self.access_token.trim().is_empty() {
            return Err(ProviderError::Protocol {
                code: "missing_access_token".into(),
                message: "token response did not contain an access token".into(),
            });
        }
        let mut session = Session::new(
            PROVIDER_ID,
            self.access_token,
            self.refresh_token,
            self.expires_in,
        );
        session.expires_at = self.expires_at.or(session.expires_at);
        session.account_id = self.account_id;
        Ok(session)
    }
}

#[async_trait]
impl MediaProvider for GuangyaProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn capabilities(&self) -> ProviderCapabilities {
        let oauth_configured = self.config.oauth_configured();
        ProviderCapabilities {
            device_code_login: oauth_configured,
            sms_login: false,
            token_import: true,
            session_refresh: oauth_configured,
            browse_files: oauth_configured,
            playback_resolution: oauth_configured,
        }
    }

    async fn restore_session(&self, session: Session) -> Result<(), ProviderError> {
        if session.provider_id != PROVIDER_ID {
            return Err(ProviderError::InvalidInput(
                "session belongs to another provider".into(),
            ));
        }
        self.set_session(session)
    }

    async fn clear_session(&self) -> Result<(), ProviderError> {
        self.session
            .write()
            .map_err(|_| ProviderError::Internal("Guangya session lock poisoned".into()))?
            .take();
        Ok(())
    }

    async fn login_device_code(&self) -> Result<DeviceCode, ProviderError> {
        let payload = self.device_code_payload()?;
        let response = self
            .request_builder(
                self.client
                    .post(self.config.endpoint(&self.config.device_code_path)),
            )
            .json(&Value::Object(payload))
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let wire: DeviceCodeWire = self.response_json(response).await?;
        let verification_uri = wire
            .verification_uri
            .or(wire.verification_url)
            .or_else(|| wire.verification_uri_complete.clone())
            .ok_or_else(|| ProviderError::Protocol {
                code: "missing_verification_uri".into(),
                message: "device-code response did not contain a verification URI".into(),
            })?;
        Ok(DeviceCode {
            device_code: wire.device_code,
            user_code: wire.user_code,
            verification_uri,
            verification_uri_complete: wire.verification_uri_complete,
            expires_in: wire.expires_in,
            interval: wire.interval,
        })
    }

    async fn poll_device_token(
        &self,
        request: DevicePollRequest,
    ) -> Result<PollResult, ProviderError> {
        let client_id = self.require_oauth_config()?;
        let grant_type =
            Self::configured_grant_type(self.config.device_code_grant_type.as_deref())?;
        let response = self
            .request_builder(
                self.client
                    .post(self.config.endpoint(&self.config.token_path)),
            )
            .json(&json!({
                "grant_type": grant_type,
                "device_code": request.device_code,
                "client_id": client_id,
            }))
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let body = match self.response_value(response).await {
            Ok(body) => body,
            Err(error) => return poll_error_result(error),
        };
        let session = TokenWire::from_value(body)?.into_session()?;
        self.set_session(session.clone())?;
        Ok(PollResult::Authorized(session))
    }

    async fn login_sms(&self, _request: SmsLoginRequest) -> Result<Session, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "Guangya SMS field mapping is not frozen".into(),
        ))
    }

    async fn import_token(&self, input: TokenImport) -> Result<Session, ProviderError> {
        if input.access_token.trim().is_empty() {
            return Err(ProviderError::InvalidInput(
                "access token is required".into(),
            ));
        }
        let session = Session {
            provider_id: PROVIDER_ID.into(),
            account_id: input.account_id,
            access_token: input.access_token,
            refresh_token: input.refresh_token,
            expires_at: input.expires_at,
        };
        self.set_session(session.clone())?;
        Ok(session)
    }

    async fn refresh_session(&self, session: &Session) -> Result<Session, ProviderError> {
        let client_id = self.require_oauth_config()?;
        let grant_type = Self::configured_grant_type(self.config.refresh_grant_type.as_deref())?;
        let refresh_token = session
            .refresh_token
            .as_deref()
            .filter(|token| !token.trim().is_empty())
            .ok_or(ProviderError::SessionExpired)?;
        let response = self
            .request_builder(
                self.client
                    .post(self.config.endpoint(&self.config.token_path)),
            )
            .json(&json!({
                "grant_type": grant_type,
                "refresh_token": refresh_token,
                "client_id": client_id,
            }))
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let refreshed = self.response_value(response).await?;
        let mut refreshed = TokenWire::from_value(refreshed)?.into_session()?;
        if refreshed.account_id.is_none() {
            refreshed.account_id = session.account_id.clone();
        }
        if refreshed.refresh_token.is_none() {
            refreshed.refresh_token = session.refresh_token.clone();
        }
        self.set_session(refreshed.clone())?;
        Ok(refreshed)
    }

    async fn list_files(&self, request: ListFilesRequest) -> Result<FilePage, ProviderError> {
        let page = request
            .page_token
            .as_deref()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let page_size = request.page_size.unwrap_or(100).clamp(1, 500);
        let (path, body) = if let Some(query) = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            (
                "/userres/v1/file/search_files",
                json!({
                    "name": query,
                    "page": page,
                    "pageSize": page_size,
                }),
            )
        } else {
            (
                "/userres/v1/file/get_file_list",
                json!({
                    "parentId": request.parent_id.unwrap_or_default(),
                    "page": page,
                    "pageSize": page_size,
                    "orderBy": 3,
                    "sortType": 1,
                    "fileTypes": [],
                    "needSubFolderStat": true,
                }),
            )
        };
        let data = self.api_post(path, body).await?;
        let list = extract_file_list(&data);
        let files = list.iter().filter_map(map_media_item).collect::<Vec<_>>();
        let total = extract_total(&data);
        let has_more =
            extract_bool(&data, &["hasMore", "has_more", "hasNext", "has_next"]).unwrap_or(false);
        let next_page_token = if has_more
            || total.is_some_and(|total| total > ((page as u64 + 1) * page_size as u64))
            || (total.is_none() && list.len() >= page_size as usize)
        {
            Some((page + 1).to_string())
        } else {
            None
        };
        Ok(FilePage {
            files,
            next_page_token,
            total,
        })
    }

    async fn resolve_playback(
        &self,
        request: PlaybackRequest,
    ) -> Result<PlaybackDescriptor, ProviderError> {
        let detail = self.playback_detail(&request.media_id).await?;
        let file_id = request.media_id.clone();
        let selected = select_resource(&detail.resources, request.quality.as_deref());
        let gcid = selected
            .map(|resource| resource.gcid.clone())
            .or_else(|| detail.fallback_gcid.clone())
            .ok_or_else(|| ProviderError::Protocol {
                code: "missing_gcid".into(),
                message: "Guangya file detail did not contain a playable resource id".into(),
            })?;
        let quality = selected.map(|resource| resource.display_name.clone());
        let qualities = (!detail.resources.is_empty()).then(|| detail.resources.clone());
        let playback = self
            .api_post(
                "/userres/v1/file/get_vod_download_url",
                json!({"fileId": file_id, "gcid": gcid}),
            )
            .await?;
        let url = value_string(&playback, &["signedURL", "signedUrl", "url"]).ok_or_else(|| {
            ProviderError::Protocol {
                code: "missing_signed_url".into(),
                message: "Guangya playback response did not contain signedURL".into(),
            }
        })?;
        Ok(PlaybackDescriptor {
            source: PROVIDER_ID.into(),
            url,
            headers: BTreeMap::new(),
            quality,
            expires_at: playback_expiry(&playback),
            media_id: request.media_id,
            outcome: "resolved".into(),
            qualities,
        })
    }

    async fn video_qualities(&self, media_id: &str) -> Result<Vec<VideoQuality>, ProviderError> {
        Ok(self.playback_detail(media_id).await?.resources)
    }

    async fn search_subtitles(
        &self,
        request: ProviderSubtitleSearchRequest,
    ) -> Result<Vec<ProviderSubtitle>, ProviderError> {
        let detail = self
            .api_post(
                "/userres/v1/file/get_file_detail",
                json!({"fileId": request.media_id}),
            )
            .await?;
        let file_info = detail.get("fileInfo").cloned().unwrap_or(Value::Null);
        let file_name = request
            .name
            .clone()
            .or_else(|| value_string(&file_info, &["fileName", "file_name", "name"]));
        let parent_id = value_string(&file_info, &["parentId", "parent_id"]);
        let gcid = value_string(&file_info, &["gcid", "mediaId", "media_id"]);
        let duration = request
            .duration_seconds
            .or_else(|| default_resource_duration(&detail));

        let mut results = Vec::new();
        // 在线字幕库：官方播放器用 {gcid, name, duration(秒)} 匹配。
        // 冷门片 / 缺参会返回 code 112，不能因此丢掉同目录云盘字幕。
        if let Some(gcid) = gcid.as_deref().filter(|value| !value.trim().is_empty()) {
            match self
                .api_post(
                    "/misc/v1/get_subtitles",
                    json!({
                        "gcid": gcid,
                        "name": file_name.clone().unwrap_or_default(),
                        "duration": duration.map(|value| value.floor() as i64).unwrap_or(0),
                    }),
                )
                .await
            {
                Ok(data) => {
                    for (index, item) in extract_file_list(&data).into_iter().enumerate() {
                        let url = value_string(&item, &["url", "signedURL", "signedUrl"]);
                        let Some(url) = url.filter(|value| !value.trim().is_empty()) else {
                            continue;
                        };
                        let ext = value_string(&item, &["ext", "extension", "format"])
                            .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
                            .unwrap_or_else(|| "srt".into());
                        let name = value_string(&item, &["name", "title", "fileName"])
                            .unwrap_or_else(|| format!("在线字幕 {index}"));
                        let id = value_string(&item, &["cid", "gcid", "id"])
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| format!("online-{index}"));
                        results.push(ProviderSubtitle {
                            id: format!("online:{id}"),
                            name,
                            source: "online".into(),
                            ext,
                            url: Some(url),
                            file_id: None,
                            language: None,
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Guangya online subtitle catalog failed");
                }
            }
        }
        // 云盘字幕：字幕文件与视频同目录，官方用 fileTypes:[6] 列出。
        // 官方客户端页码从 1 开始；部分列表接口仍接受 0。先按官方翻页，空结果再试第 0 页。
        if let Some(parent_id) = parent_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            let mut cloud = collect_cloud_subtitles(self, parent_id, 1..=5).await;
            if cloud.is_empty() {
                cloud = collect_cloud_subtitles(self, parent_id, 0..=0).await;
            }
            results.extend(cloud);
        }
        Ok(results)
    }

    async fn download_subtitle(
        &self,
        subtitle: &ProviderSubtitle,
    ) -> Result<ProviderSubtitleDownloaded, ProviderError> {
        match subtitle.source.as_str() {
            "online" => {
                let url = subtitle
                    .url
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| ProviderError::Protocol {
                        code: "missing_subtitle_url".into(),
                        message: "Guangya online subtitle did not carry a download URL".into(),
                    })?;
                let bytes = self.download_bytes(url).await?;
                let file_name = sanitize_subtitle_name(&subtitle.name, &subtitle.ext);
                Ok(ProviderSubtitleDownloaded { file_name, bytes })
            }
            "cloud" => {
                let file_id = subtitle
                    .file_id
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| ProviderError::Protocol {
                        code: "missing_subtitle_file_id".into(),
                        message: "Guangya cloud subtitle did not carry a file id".into(),
                    })?;
                let playback = self
                    .api_post(
                        "/userres/v1/get_res_download_url",
                        json!({"fileId": file_id}),
                    )
                    .await?;
                let url = value_string(&playback, &["signedURL", "signedUrl", "url"]).ok_or_else(
                    || ProviderError::Protocol {
                        code: "missing_signed_url".into(),
                        message: "Guangya subtitle download did not contain signedURL".into(),
                    },
                )?;
                let bytes = self.download_bytes(&url).await?;
                let file_name = sanitize_subtitle_name(&subtitle.name, &subtitle.ext);
                Ok(ProviderSubtitleDownloaded { file_name, bytes })
            }
            other => Err(ProviderError::InvalidInput(format!(
                "unknown Guangya subtitle source: {other}"
            ))),
        }
    }
}

impl GuangyaProvider {
    async fn download_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        let response = self
            .client
            .get(url)
            .header(
                "user-agent",
                self.config.user_agent.as_deref().unwrap_or(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/147.0.0.0 Safari/537.36",
                ),
            )
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ProviderError::Network(format!(
                "subtitle download returned HTTP {status}"
            )));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| ProviderError::Network(format!("subtitle body read failed: {error}")))
    }
}

/// Official desktop mapping of the VOD response TTL: `urlDuration` is a
/// lifetime in seconds, so the absolute expiry is now + urlDuration.
fn playback_expiry(playback: &Value) -> Option<i64> {
    let expires_at = playback
        .get("expiresAt")
        .or_else(|| playback.get("expires_at"))
        .and_then(value_i64);
    if expires_at.is_some() {
        return expires_at;
    }
    let ttl = playback
        .get("urlDuration")
        .or_else(|| playback.get("url_duration"))
        .and_then(value_i64)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some(now + ttl.max(0))
}

fn sanitize_subtitle_name(name: &str, ext: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .filter(|character| {
            !matches!(
                character,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'
            )
        })
        .collect();
    let trimmed = cleaned.trim();
    let base = if trimmed.is_empty() {
        "subtitle".to_owned()
    } else {
        trimmed.to_owned()
    };
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    if base
        .rsplit_once('.')
        .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case(&ext))
    {
        base
    } else if ext.is_empty() {
        format!("{base}.srt")
    } else {
        format!("{base}.{ext}")
    }
}

/// `videoResource[]` → selectable qualities, following the official client:
/// source 0 (原画) is definition 10000; otherwise the definition number is
/// parsed out of `resolutionName`.
fn parse_video_resources(detail: &Value) -> Vec<VideoQuality> {
    let resources = detail
        .get("videoResource")
        .or_else(|| detail.get("video_resource"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    resources
        .iter()
        .filter_map(|resource| {
            let gcid = value_string(resource, &["gcid", "mediaId", "media_id"])
                .filter(|value| !value.trim().is_empty())?;
            let info = resource.get("info").filter(|value| value.is_object());
            let flat = || resource;
            let read = |keys: &[&str]| -> Option<String> {
                info.and_then(|info| value_string(info, keys))
                    .or_else(|| value_string(flat(), keys))
            };
            let read_i64 = |keys: &[&str]| -> Option<i64> {
                info.and_then(|info| {
                    keys.iter()
                        .find_map(|key| info.get(*key).and_then(value_i64))
                })
                .or_else(|| {
                    keys.iter()
                        .find_map(|key| flat().get(*key).and_then(value_i64))
                })
            };
            let source = read_i64(&["source", "category", "videoSource"]);
            let resolution_name = read(&["resolutionName", "resolution_name", "name"]);
            let definition_id = definition_id_for(source, resolution_name.as_deref());
            let display_name = display_name_for(source, resolution_name.as_deref());
            let short_name = short_name_for(source, resolution_name.as_deref());
            let is_default = info
                .and_then(|info| {
                    info.get("defaultResolution")
                        .or_else(|| info.get("default_resolution"))
                })
                .and_then(value_bool_str)
                .or_else(|| {
                    resource
                        .get("defaultResolution")
                        .or_else(|| resource.get("default_resolution"))
                        .and_then(value_bool_str)
                })
                .unwrap_or(false);
            Some(VideoQuality {
                gcid,
                definition_id,
                short_name,
                display_name,
                resolution_name,
                need_vip_type: read_i64(&["needVipType", "need_vip_type"]),
                source,
                duration_seconds: read(&["duration"])
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|value| value.is_finite() && *value > 0.0),
                is_default,
            })
        })
        .collect()
}

fn value_bool_str(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => Some(number.as_i64() == Some(1)),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Official `fb()` mapping: 原画=10000, then 240/360/480/720/1080/2K/4K/8K.
fn definition_id_for(source: Option<i64>, resolution_name: Option<&str>) -> u64 {
    if source == Some(0) {
        return 10_000;
    }
    let Some(name) = resolution_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return 0;
    };
    if name.contains("8K") {
        8_000
    } else if name.contains("4K") {
        4_000
    } else if name.contains("2K") {
        2_000
    } else if name.contains("1080") {
        1_080
    } else if name.contains("720") {
        720
    } else if name.contains("480") {
        480
    } else if name.contains("360") {
        360
    } else if name.contains("240") {
        240
    } else {
        0
    }
}

/// Official display-name table (240P 极速 … 8K 超高清, 原画, 无损画质).
fn display_name_for(source: Option<i64>, resolution_name: Option<&str>) -> String {
    match source {
        Some(0) => return "原画".into(),
        Some(3) => return "无损画质".into(),
        _ => {}
    }
    let Some(name) = resolution_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return "自动".into();
    };
    if name.contains("8K") {
        "8K 超高清".into()
    } else if name.contains("4K") {
        "4K 超高清".into()
    } else if name.contains("2K") {
        "2K 超高清".into()
    } else if name.contains("1080") {
        "1080P 超清".into()
    } else if name.contains("720") {
        "720P 高清".into()
    } else if name.contains("480") {
        "480P 标清".into()
    } else if name.contains("360") {
        "360P 流畅".into()
    } else if name.contains("240") {
        "240P 极速".into()
    } else {
        name.to_owned()
    }
}

fn short_name_for(source: Option<i64>, resolution_name: Option<&str>) -> Option<String> {
    match source {
        Some(0) => return Some("原画".into()),
        Some(3) => return Some("无损".into()),
        _ => {}
    }
    let name = resolution_name?;
    if name.contains("8K") {
        Some("8K".into())
    } else if name.contains("4K") {
        Some("4K".into())
    } else if name.contains("2K") {
        Some("2K".into())
    } else if name.contains("1080") {
        Some("1080P".into())
    } else if name.contains("720") {
        Some("720P".into())
    } else if name.contains("480") {
        Some("480P".into())
    } else if name.contains("360") {
        Some("360P".into())
    } else if name.contains("240") {
        Some("240P".into())
    } else {
        None
    }
}

/// Quality selection shared by `resolve_playback`: match the requested
/// gcid/definition/name first, then the resource flagged `defaultResolution`,
/// then the first resource (falls back to `fileInfo.gcid` at the caller).
fn select_resource<'a>(
    resources: &'a [VideoQuality],
    quality: Option<&str>,
) -> Option<&'a VideoQuality> {
    let requested = quality.map(str::trim).filter(|value| !value.is_empty());
    if let Some(requested) = requested {
        if let Some(resource) = resources.iter().find(|resource| {
            resource.gcid == requested
                || resource.resolution_name.as_deref() == Some(requested)
                || resource.display_name == requested
                || resource.short_name.as_deref() == Some(requested)
                || definition_matches(resource, requested)
        }) {
            return Some(resource);
        }
    }
    resources
        .iter()
        .find(|resource| resource.is_default)
        .or_else(|| resources.first())
}

fn definition_matches(resource: &VideoQuality, requested: &str) -> bool {
    let Some(number) = requested.trim_end_matches('p').parse::<u64>().ok() else {
        return false;
    };
    resource.definition_id == number
}

fn default_resource_duration(detail: &Value) -> Option<f64> {
    let resources = detail
        .get("videoResource")
        .or_else(|| detail.get("video_resource"))
        .and_then(Value::as_array)?;
    let duration = resources
        .iter()
        .filter_map(|resource| {
            let info = resource.get("info")?;
            info.get("duration")
                .or_else(|| resource.get("duration"))
                .and_then(|value| {
                    value
                        .as_f64()
                        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
                })
        })
        .find(|value| value.is_finite() && *value > 0.0)?;
    Some(duration)
}

fn value_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|value| match value {
            Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
    })
}

async fn collect_cloud_subtitles(
    provider: &GuangyaProvider,
    parent_id: &str,
    pages: impl IntoIterator<Item = u32>,
) -> Vec<ProviderSubtitle> {
    let mut results = Vec::new();
    for page in pages {
        let data = match provider
            .api_post(
                "/userres/v1/file/get_file_list",
                json!({
                    "parentId": parent_id,
                    "page": page,
                    "pageSize": 100,
                    "fileTypes": [6],
                    "orderBy": 0,
                    "sortType": 1,
                }),
            )
            .await
        {
            Ok(data) => data,
            Err(error) => {
                tracing::warn!(page, error = %error, "Guangya cloud subtitle list failed");
                break;
            }
        };
        let items = extract_file_list(&data);
        let count = items.len();
        for item in items {
            let name = value_string(&item, &["fileName", "file_name", "name"]).unwrap_or_default();
            let ext = name
                .rsplit_once('.')
                .map(|(_, extension)| extension.to_ascii_lowercase())
                .unwrap_or_default();
            if !matches!(
                ext.as_str(),
                "srt" | "ass" | "ssa" | "sub" | "vtt" | "smi" | "sami" | "sup"
            ) {
                continue;
            }
            let Some(file_id) = value_string(&item, &["fileId", "file_id", "id"])
                .filter(|value| !value.trim().is_empty())
            else {
                continue;
            };
            results.push(ProviderSubtitle {
                id: format!("cloud:{file_id}"),
                name,
                source: "cloud".into(),
                ext,
                url: None,
                file_id: Some(file_id),
                language: None,
            });
        }
        let total = extract_total(&data).unwrap_or(0);
        let fetched = u64::from(page.max(1)) * 100;
        if count < 100 || (total > 0 && fetched >= total) {
            break;
        }
    }
    results
}

fn extract_file_list(value: &Value) -> Vec<Value> {
    if let Some(items) = value.as_array() {
        return items.clone();
    }
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    for key in ["list", "fileList", "file_list", "files", "items", "records"] {
        if let Some(items) = object.get(key).and_then(Value::as_array) {
            return items.clone();
        }
    }
    for key in ["data", "result", "payload"] {
        if let Some(nested) = object.get(key) {
            let items = extract_file_list(nested);
            if !items.is_empty() {
                return items;
            }
        }
    }
    Vec::new()
}

fn extract_total(value: &Value) -> Option<u64> {
    if let Some(total) = ["total", "totalCount", "total_count", "count"]
        .iter()
        .find_map(|key| value.get(*key).and_then(value_u64))
    {
        return Some(total);
    }
    ["data", "result", "payload"]
        .iter()
        .find_map(|key| value.get(*key).and_then(extract_total))
}

fn extract_bool(value: &Value, keys: &[&str]) -> Option<bool> {
    if let Some(result) = keys
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
    {
        return Some(result);
    }
    ["data", "result", "payload"].iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|nested| extract_bool(nested, keys))
    })
}

fn value_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn value_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn map_media_item(value: &Value) -> Option<super::MediaItem> {
    let id = value_string(
        value,
        &[
            "fileId",
            "file_id",
            "fileID",
            "resourceId",
            "resource_id",
            "resId",
            "res_id",
            "id",
            "folderId",
            "folder_id",
            "fid",
        ],
    )?;
    let name = value_string(
        value,
        &[
            "fileName",
            "file_name",
            "filename",
            "displayName",
            "display_name",
            "resName",
            "resourceName",
            "fileFullName",
            "fullName",
            "name",
            "title",
        ],
    )?;
    let res_type = [
        "resType",
        "res_type",
        "resourceType",
        "resource_type",
        "folderType",
        "folder_type",
        "dirType",
        "dir_type",
        "fileType",
        "file_type",
        "type",
        "kind",
    ]
    .iter()
    .find_map(|key| {
        value.get(*key).and_then(|field| {
            value_i64(field).or_else(|| {
                field
                    .as_str()
                    .and_then(|text| match text.to_ascii_lowercase().as_str() {
                        "folder" | "dir" | "directory" => Some(2),
                        "file" => Some(1),
                        _ => None,
                    })
            })
        })
    });
    let folder_flag = [
        "isDir",
        "is_dir",
        "isFolder",
        "is_folder",
        "folder",
        "directory",
        "isDirectory",
        "dir",
        "hasChildren",
        "has_children",
    ]
    .iter()
    .any(|key| {
        value
            .get(*key)
            .map(|field| {
                field.as_bool() == Some(true)
                    || value_i64(field) == Some(1)
                    || field.as_str() == Some("true")
            })
            .unwrap_or(false)
    });
    let has_children_hint = ["childrenCount", "childCount", "dirCount", "folderCount"]
        .iter()
        .any(|key| value.get(*key).is_some());
    let has_video_extension = name.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "mp4"
                | "mkv"
                | "avi"
                | "mov"
                | "m4v"
                | "ts"
                | "webm"
                | "flv"
                | "wmv"
                | "rmvb"
                | "mpeg"
                | "mpg"
        )
    });
    let is_folder = res_type == Some(2)
        || (res_type != Some(1) && !has_video_extension && (folder_flag || has_children_hint));
    let kind = if is_folder {
        super::MediaKind::Folder
    } else {
        super::MediaKind::File
    };
    Some(super::MediaItem {
        id,
        name,
        kind,
        parent_id: value_string(
            value,
            &[
                "parentId",
                "parent_id",
                "pid",
                "pId",
                "dirId",
                "dir_id",
                "folderId",
                "folder_id",
                "parentResourceId",
                "parent_resource_id",
            ],
        ),
        size_bytes: ["fileSize", "file_size", "sizeBytes", "size_bytes", "size"]
            .iter()
            .find_map(|key| value.get(*key).and_then(value_u64)),
        mime_type: value_string(
            value,
            &[
                "mimeType",
                "mineType",
                "mime_type",
                "contentType",
                "content_type",
            ],
        ),
        duration_seconds: value
            .get("duration")
            .or_else(|| value.get("durationSeconds"))
            .and_then(|value| {
                value
                    .as_f64()
                    .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            }),
        thumbnail_url: value_string(
            value,
            &[
                "thumbnailUrl",
                "thumbnail_url",
                "thumbUrl",
                "thumb_url",
                "coverUrl",
                "cover_url",
                // 光鸭文件列表接口的缩略图字段名就是 "thumbnail"
                "thumbnail",
            ],
        ),
        metadata: value.clone(),
    })
}

fn map_protocol_error(status: StatusCode, body: &Value) -> ProviderError {
    let numeric_code = body.get("code").and_then(value_i64);
    if numeric_code == Some(117) {
        return ProviderError::SessionExpired;
    }
    let error_code = body
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| body.get("code").and_then(Value::as_str).map(str::to_owned))
        .or_else(|| numeric_code.map(|code| code.to_string()))
        .unwrap_or_else(|| "http_error".into());
    let message = body
        .get("error_description")
        .and_then(Value::as_str)
        .or_else(|| body.get("msg").and_then(Value::as_str))
        .or_else(|| body.get("message").and_then(Value::as_str))
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("request failed"));
    match error_code.as_str() {
        "authorization_pending" => ProviderError::AuthenticationPending,
        "slow_down" => ProviderError::RateLimited {
            retry_after_secs: None,
        },
        "invalid_grant" | "invalid_token" => ProviderError::SessionExpired,
        "access_denied" => ProviderError::AuthorizationDenied,
        _ => ProviderError::Protocol {
            code: error_code,
            message: message.to_owned(),
        },
    }
}

fn poll_error_result(error: ProviderError) -> Result<PollResult, ProviderError> {
    match error {
        ProviderError::AuthenticationPending => Ok(PollResult::Pending { interval: None }),
        ProviderError::RateLimited { retry_after_secs } => Ok(PollResult::SlowDown {
            interval: retry_after_secs.unwrap_or(5),
        }),
        ProviderError::AuthorizationDenied => Ok(PollResult::Denied),
        ProviderError::SessionExpired => Ok(PollResult::Expired),
        other => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_wire_accepts_camel_and_snake_case_fields() {
        let wire: TokenWire = serde_json::from_value(json!({
            "accessToken": "a",
            "refresh_token": "r",
            "expiresIn": 120,
            "user_id": "u"
        }))
        .unwrap();
        let session = wire.into_session().unwrap();
        assert_eq!(session.provider_id, PROVIDER_ID);
        assert_eq!(session.account_id.as_deref(), Some("u"));
        assert!(session.refresh_token.is_some());
    }

    #[test]
    fn token_response_accepts_zero_code_and_nested_data() {
        let payload = response_payload(
            StatusCode::OK,
            json!({
                "code": 0,
                "data": {
                    "accessToken": "a",
                    "refreshToken": "r",
                    "expiresIn": 120
                }
            }),
        )
        .unwrap();
        let session = TokenWire::from_value(payload)
            .unwrap()
            .into_session()
            .unwrap();
        assert_eq!(session.access_token, "a");
        assert_eq!(session.refresh_token.as_deref(), Some("r"));
    }

    #[test]
    fn token_response_rejects_nonzero_code_but_accepts_code_free_payload() {
        assert!(matches!(
            response_payload(StatusCode::OK, json!({"code": 117, "msg": "expired"})),
            Err(ProviderError::SessionExpired)
        ));
        assert!(response_payload(StatusCode::OK, json!({"accessToken": "a"})).is_ok());
    }

    #[test]
    fn oauth_errors_map_to_provider_states() {
        assert!(matches!(
            map_protocol_error(
                StatusCode::BAD_REQUEST,
                &json!({"error": "authorization_pending"})
            ),
            ProviderError::AuthenticationPending
        ));
        assert!(matches!(
            map_protocol_error(StatusCode::BAD_REQUEST, &json!({"error": "invalid_grant"})),
            ProviderError::SessionExpired
        ));
        assert!(matches!(
            poll_error_result(ProviderError::AuthenticationPending),
            Ok(PollResult::Pending { interval: None })
        ));
        assert!(matches!(
            poll_error_result(ProviderError::RateLimited {
                retry_after_secs: Some(9)
            }),
            Ok(PollResult::SlowDown { interval: 9 })
        ));
    }

    #[test]
    fn device_code_accepts_complete_uri_as_fallback() {
        let wire: DeviceCodeWire = serde_json::from_value(json!({
            "device_code": "d",
            "user_code": "u",
            "verification_uri_complete": "https://example.invalid/complete"
        }))
        .unwrap();
        assert_eq!(
            wire.verification_uri.or(wire.verification_uri_complete),
            Some("https://example.invalid/complete".into())
        );
    }

    #[test]
    fn file_mapping_recognizes_folder_variants() {
        let item = map_media_item(&json!({
            "fileId": "folder-1",
            "fileName": "Movies",
            "resourceType": "folder"
        }))
        .unwrap();
        assert_eq!(item.kind, crate::providers::MediaKind::Folder);
    }

    #[test]
    fn file_mapping_accepts_web_client_aliases_and_nested_lists() {
        let payload = json!({
            "fileList": [{
                "id": "video-1",
                "name": "Movie.mp4",
                "file_size": 1024,
                "file_type": "file",
                "folderId": "root"
            }],
            "totalCount": 1
        });
        let list = extract_file_list(&payload);
        assert_eq!(list.len(), 1);
        let item = map_media_item(&list[0]).unwrap();
        assert_eq!(item.id, "video-1");
        assert_eq!(item.size_bytes, Some(1024));
        assert_eq!(item.parent_id.as_deref(), Some("root"));
        assert_eq!(extract_total(&payload), Some(1));
    }

    fn sample_detail() -> Value {
        json!({
            "fileInfo": {"fileId": "f1", "gcid": "file-gcid", "parentId": "parent-1", "fileName": "电影.mp4"},
            "videoResource": [
                {"gcid": "g-720", "info": {"source": 1, "resolutionName": "高清 720P", "needVipType": 3, "duration": 5400}},
                {"gcid": "g-1080", "info": {"source": 1, "resolutionName": "超清 1080P", "needVipType": 1, "defaultResolution": true, "duration": 5400}},
                {"gcid": "g-4k", "info": {"source": 1, "resolutionName": "4K 超高清", "needVipType": 2}},
                {"gcid": "g-src", "info": {"source": 0, "resolutionName": "原始画质", "needVipType": 2}}
            ]
        })
    }

    #[test]
    fn video_resources_map_official_definition_ids() {
        let resources = parse_video_resources(&sample_detail());
        assert_eq!(resources.len(), 4);
        assert_eq!(resources[0].definition_id, 720);
        assert_eq!(resources[0].display_name, "720P 高清");
        assert_eq!(resources[1].definition_id, 1080);
        assert_eq!(resources[1].display_name, "1080P 超清");
        assert!(resources[1].is_default);
        assert!(!resources[0].is_default);
        assert_eq!(resources[2].definition_id, 4_000);
        assert_eq!(resources[3].definition_id, 10_000);
        assert_eq!(resources[3].display_name, "原画");
        assert_eq!(resources[1].duration_seconds, Some(5400.0));
        assert_eq!(resources[2].need_vip_type, Some(2));
    }

    #[test]
    fn quality_selection_prefers_request_then_default_then_first() {
        let resources = parse_video_resources(&sample_detail());
        // 按 gcid
        assert_eq!(
            select_resource(&resources, Some("g-4k")).unwrap().gcid,
            "g-4k"
        );
        // 按显示名 / 分辨率名 / 短名 / 定义数字
        assert_eq!(
            select_resource(&resources, Some("1080P 超清"))
                .unwrap()
                .gcid,
            "g-1080"
        );
        assert_eq!(
            select_resource(&resources, Some("高清 720P")).unwrap().gcid,
            "g-720"
        );
        assert_eq!(
            select_resource(&resources, Some("4K")).unwrap().gcid,
            "g-4k"
        );
        assert_eq!(
            select_resource(&resources, Some("1080")).unwrap().gcid,
            "g-1080"
        );
        // 未命中 → defaultResolution 优先，否则第一个
        assert_eq!(
            select_resource(&resources, Some("8K")).unwrap().gcid,
            "g-1080"
        );
        let no_default = vec![resources[0].clone(), resources[2].clone()];
        assert_eq!(select_resource(&no_default, None).unwrap().gcid, "g-720");
    }

    #[test]
    fn playback_expiry_converts_urlduration_ttl() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let expires_at =
            playback_expiry(&json!({"signedURL": "https://cdn", "urlDuration": 3600})).unwrap();
        assert!((expires_at - (now + 3600)).abs() <= 2);
        // 显式 expiresAt 优先
        assert_eq!(
            playback_expiry(&json!({"urlDuration": 3600, "expiresAt": 1234})),
            Some(1234)
        );
        assert_eq!(playback_expiry(&json!({"signedURL": "https://cdn"})), None);
    }

    #[test]
    fn subtitle_names_are_sanitized_and_ext_normalized() {
        assert_eq!(
            sanitize_subtitle_name("Movie.chs.srt", "SRT"),
            "Movie.chs.srt"
        );
        assert_eq!(
            sanitize_subtitle_name("bad:name?.ass", "ass"),
            "badname.ass"
        );
        assert_eq!(sanitize_subtitle_name("", "srt"), "subtitle.srt");
        assert_eq!(sanitize_subtitle_name("无后缀", ""), "无后缀.srt");
    }

    #[test]
    fn online_subtitle_items_map_from_list_payload() {
        let data = json!({
            "list": [
                {"url": "https://cdn/sub1.srt", "ext": "srt", "name": "Movie.chs.srt", "cid": "c1"},
                {"ext": "ass", "name": "无地址"},
                {"url": "https://cdn/sub2.ass", "name": "Movie.cht.ass"}
            ]
        });
        let items = extract_file_list(&data);
        assert_eq!(items.len(), 3);
        assert!(value_string(&items[0], &["url"]).is_some());
    }

    #[test]
    fn protocol_error_reads_numeric_code_and_msg() {
        let error = map_protocol_error(
            reqwest::StatusCode::OK,
            &json!({"code": 112, "msg": "参数错误"}),
        );
        match error {
            ProviderError::Protocol { code, message } => {
                assert_eq!(code, "112");
                assert_eq!(message, "参数错误");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
