//! Official OAuth2 adapter used by cloud providers that expose a documented
//! authorization-code and refresh-token flow. It deliberately does not
//! emulate private QR/CAS cookies or vendor request signatures.

use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    DeviceCode, DevicePollRequest, FilePage, ListFilesRequest, MediaProvider, PlaybackDescriptor,
    PlaybackRequest, PollResult, ProviderCapabilities, ProviderError, Session, SmsLoginRequest,
    TokenImport,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct OAuthProviderConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    /// Optional RFC 8628 device authorization endpoint. It is intentionally
    /// unset by defaults because QR/device endpoints must be confirmed by
    /// each provider's official documentation.
    pub device_code_endpoint: Option<String>,
    #[serde(default)]
    pub device_code_grant_type: Option<String>,
    #[serde(default = "default_refresh_grant_type")]
    pub refresh_grant_type: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub scope: Option<String>,
}

impl Default for OAuthProviderConfig {
    fn default() -> Self {
        Self {
            authorization_endpoint: String::new(),
            token_endpoint: String::new(),
            device_code_endpoint: None,
            device_code_grant_type: None,
            refresh_grant_type: Some("refresh_token".into()),
            client_id: None,
            client_secret: None,
            redirect_uri: "http://127.0.0.1:49215/oauth/callback".into(),
            scope: None,
        }
    }
}

fn default_refresh_grant_type() -> Option<String> {
    Some("refresh_token".into())
}

pub fn default_oauth_configs() -> std::collections::BTreeMap<String, OAuthProviderConfig> {
    let mut configs = std::collections::BTreeMap::new();
    configs.insert(
        "baidu".into(),
        config(
            "https://openapi.baidu.com/oauth/2.0/authorize",
            "https://openapi.baidu.com/oauth/2.0/token",
        ),
    );
    configs.insert(
        "aliyun".into(),
        config(
            "https://openapi.alipan.com/oauth/authorize",
            "https://openapi.alipan.com/oauth/access_token",
        ),
    );
    configs.insert(
        "cloud123".into(),
        config(
            "https://www.123pan.com/oauth/authorize",
            "https://www.123pan.com/oauth/token",
        ),
    );
    configs.insert(
        "tianyi".into(),
        config(
            "https://open.e.189.cn/api/logbox/oauth2/authorize",
            "https://open.e.189.cn/api/logbox/oauth2/token",
        ),
    );
    configs
}

fn config(authorization_endpoint: &str, token_endpoint: &str) -> OAuthProviderConfig {
    OAuthProviderConfig {
        authorization_endpoint: authorization_endpoint.into(),
        token_endpoint: token_endpoint.into(),
        ..OAuthProviderConfig::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenWire {
    access_token: Option<String>,
    access_token_snake: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceCodeWire {
    #[serde(alias = "device_code")]
    device_code: String,
    #[serde(default)]
    #[serde(alias = "user_code")]
    user_code: Option<String>,
    #[serde(default)]
    #[serde(alias = "verification_uri")]
    verification_uri: Option<String>,
    #[serde(default)]
    #[serde(alias = "verification_url")]
    verification_url: Option<String>,
    #[serde(default)]
    #[serde(alias = "verification_uri_complete")]
    verification_uri_complete: Option<String>,
    #[serde(default = "default_device_expires_in")]
    #[serde(alias = "expires_in")]
    expires_in: u64,
    #[serde(default = "default_device_interval")]
    #[serde(alias = "interval")]
    interval: u64,
}

fn default_device_expires_in() -> u64 {
    600
}
fn default_device_interval() -> u64 {
    5
}

impl TokenWire {
    fn from_value(value: Value) -> Result<Self, ProviderError> {
        let access_token = value
            .get("accessToken")
            .or_else(|| value.get("access_token"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let refresh_token = value
            .get("refreshToken")
            .or_else(|| value.get("refresh_token"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let expires_in = value
            .get("expiresIn")
            .or_else(|| value.get("expires_in"))
            .and_then(Value::as_u64);
        let token_type = value
            .get("tokenType")
            .or_else(|| value.get("token_type"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let access_token = access_token.filter(|token| !token.trim().is_empty());
        if access_token.is_none() {
            return Err(ProviderError::Protocol {
                code: "oauth_missing_access_token".into(),
                message: "OAuth token response did not contain an access token".into(),
            });
        }
        Ok(Self {
            access_token,
            access_token_snake: None,
            refresh_token,
            expires_in,
            token_type,
        })
    }
}

pub struct OAuthProvider {
    id: &'static str,
    config: OAuthProviderConfig,
    client: Client,
    session: RwLock<Option<Session>>,
}

impl std::fmt::Debug for OAuthProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthProvider")
            .field("id", &self.id)
            .field(
                "authorization_endpoint",
                &self.config.authorization_endpoint,
            )
            .field("token_endpoint", &self.config.token_endpoint)
            .finish()
    }
}

impl OAuthProvider {
    pub fn new(id: &'static str, config: OAuthProviderConfig) -> Result<Self, ProviderError> {
        if config.authorization_endpoint.trim().is_empty()
            || config.token_endpoint.trim().is_empty()
        {
            return Err(ProviderError::InvalidInput(
                "OAuth endpoints are required".into(),
            ));
        }
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|error| {
                ProviderError::Internal(format!("cannot build OAuth client: {error}"))
            })?;
        Ok(Self {
            id,
            config,
            client,
            session: RwLock::new(None),
        })
    }

    fn configured(&self) -> Result<&str, ProviderError> {
        self.config
            .client_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ProviderError::InvalidInput(format!(
                    "OAuth client_id for '{}' is not configured",
                    self.id
                ))
            })
    }

    fn device_configured(&self) -> bool {
        self.config
            .device_code_endpoint
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            && self
                .config
                .device_code_grant_type
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && self
                .config
                .client_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }

    fn supports_drive_api(&self) -> bool {
        matches!(self.id, "baidu" | "aliyun")
    }

    fn active_session(&self) -> Result<Session, ProviderError> {
        let session = self
            .session
            .read()
            .map_err(|_| ProviderError::Internal("OAuth session lock poisoned".into()))?
            .clone()
            .ok_or(ProviderError::NotAuthenticated)?;
        if session.is_expired() {
            return Err(ProviderError::SessionExpired);
        }
        Ok(session)
    }

    async fn response_json(
        &self,
        response: reqwest::Response,
        operation: &str,
    ) -> Result<Value, ProviderError> {
        let status = response.status();
        let body = response.json::<Value>().await.map_err(|error| {
            ProviderError::Network(format!("invalid {operation} response: {error}"))
        })?;
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::SessionExpired);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::RateLimited {
                retry_after_secs: None,
            });
        }
        if !status.is_success() {
            return Err(ProviderError::Protocol {
                code: status.as_u16().to_string(),
                message: oauth_api_message(&body, operation),
            });
        }
        Ok(body)
    }

    async fn baidu_list_files(
        &self,
        request: ListFilesRequest,
        session: &Session,
    ) -> Result<FilePage, ProviderError> {
        let directory = request
            .parent_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("/");
        let limit = request.page_size.unwrap_or(100).clamp(1, 1000);
        let start = request
            .page_token
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        let response = self
            .client
            .get("https://pan.baidu.com/rest/2.0/xpan/file")
            .query(&[
                ("method", "list".to_owned()),
                ("access_token", session.access_token.clone()),
                ("dir", directory.to_owned()),
                ("start", start.to_string()),
                ("limit", limit.to_string()),
                ("order", "name".to_owned()),
                ("desc", "0".to_owned()),
                ("web", "1".to_owned()),
                ("folder", "0".to_owned()),
                ("showempty", "0".to_owned()),
            ])
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let body = self.response_json(response, "Baidu file list").await?;
        let errno = body
            .get("errno")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        if errno != 0 {
            return Err(map_baidu_error(errno, &body));
        }
        let raw_items = body
            .get("list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let files = raw_items
            .iter()
            .filter(|item| {
                request.query.as_deref().is_none_or(|query| {
                    value_string(item, &["server_filename", "name"])
                        .is_some_and(|name| name.to_lowercase().contains(&query.to_lowercase()))
                })
            })
            .map(|item| baidu_media_item(item, directory))
            .collect::<Vec<_>>();
        let next_page_token = (raw_items.len() as u32 >= limit)
            .then(|| start.saturating_add(raw_items.len() as u64).to_string());
        Ok(FilePage {
            files,
            next_page_token,
            total: None,
        })
    }

    async fn baidu_resolve_playback(
        &self,
        request: PlaybackRequest,
        session: &Session,
    ) -> Result<PlaybackDescriptor, ProviderError> {
        if request.media_id.trim().is_empty() {
            return Err(ProviderError::InvalidInput("media_id is required".into()));
        }
        let fsids = serde_json::to_string(&[request.media_id.clone()])
            .map_err(|error| ProviderError::Internal(error.to_string()))?;
        let response = self
            .client
            .get("https://pan.baidu.com/rest/2.0/xpan/multimedia")
            .query(&[
                ("method", "filemetas".to_owned()),
                ("access_token", session.access_token.clone()),
                ("fsids", fsids),
                ("dlink", "1".to_owned()),
                ("thumb", "1".to_owned()),
                ("extra", "1".to_owned()),
            ])
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let body = self.response_json(response, "Baidu file metadata").await?;
        let errno = body
            .get("errno")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        if errno != 0 {
            return Err(map_baidu_error(errno, &body));
        }
        let item = body
            .get("list")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .ok_or_else(|| ProviderError::NotFound(request.media_id.clone()))?;
        let dlink = value_string(item, &["dlink", "download_link"]).ok_or_else(|| {
            ProviderError::Protocol {
                code: "baidu_missing_dlink".into(),
                message: "Baidu did not return a downloadable URL".into(),
            }
        })?;
        let mut url = reqwest::Url::parse(&dlink).map_err(|error| ProviderError::Protocol {
            code: "baidu_invalid_dlink".into(),
            message: error.to_string(),
        })?;
        if !url.query_pairs().any(|(key, _)| key == "access_token") {
            url.query_pairs_mut()
                .append_pair("access_token", &session.access_token);
        }
        let mut headers = BTreeMap::new();
        headers.insert("User-Agent".into(), "pan.baidu.com".into());
        Ok(PlaybackDescriptor {
            source: self.id.into(),
            url: url.to_string(),
            headers,
            quality: request.quality,
            expires_at: None,
            media_id: request.media_id,
            outcome: "direct".into(),
            qualities: None,
        })
    }

    async fn aliyun_drive_id(&self, session: &Session) -> Result<String, ProviderError> {
        let response = self
            .client
            .post("https://openapi.alipan.com/adrive/v1.0/user/getDriveInfo")
            .bearer_auth(&session.access_token)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let body = self.response_json(response, "Aliyun drive info").await?;
        value_string(
            &body,
            &["default_drive_id", "defaultDriveId", "resource_drive_id"],
        )
        .ok_or_else(|| ProviderError::Protocol {
            code: "aliyun_missing_drive_id".into(),
            message: "Aliyun did not return a default drive id".into(),
        })
    }

    async fn aliyun_list_files(
        &self,
        request: ListFilesRequest,
        session: &Session,
    ) -> Result<FilePage, ProviderError> {
        let drive_id = self.aliyun_drive_id(session).await?;
        let parent_file_id = request
            .parent_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("root");
        let limit = request.page_size.unwrap_or(100).clamp(1, 200);
        let mut payload = serde_json::json!({
            "drive_id": drive_id,
            "parent_file_id": parent_file_id,
            "limit": limit,
            "order_by": "name",
            "order_direction": "ASC"
        });
        if let Some(marker) = request
            .page_token
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            payload["marker"] = Value::String(marker.to_owned());
        }
        let response = self
            .client
            .post("https://openapi.alipan.com/adrive/v1.0/openFile/list")
            .bearer_auth(&session.access_token)
            .json(&payload)
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let body = self.response_json(response, "Aliyun file list").await?;
        if body.get("code").and_then(Value::as_str).is_some() {
            return Err(map_aliyun_error(&body));
        }
        let files = body
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| {
                request.query.as_deref().is_none_or(|query| {
                    value_string(item, &["name"])
                        .is_some_and(|name| name.to_lowercase().contains(&query.to_lowercase()))
                })
            })
            .map(aliyun_media_item)
            .collect::<Vec<_>>();
        Ok(FilePage {
            total: body.get("total_count").and_then(Value::as_u64),
            next_page_token: value_string(&body, &["next_marker", "nextMarker"])
                .filter(|value| !value.is_empty()),
            files,
        })
    }

    async fn aliyun_resolve_playback(
        &self,
        request: PlaybackRequest,
        session: &Session,
    ) -> Result<PlaybackDescriptor, ProviderError> {
        if request.media_id.trim().is_empty() {
            return Err(ProviderError::InvalidInput("media_id is required".into()));
        }
        let drive_id = self.aliyun_drive_id(session).await?;
        let response = self
            .client
            .post("https://openapi.alipan.com/adrive/v1.0/openFile/getDownloadUrl")
            .bearer_auth(&session.access_token)
            .json(&serde_json::json!({
                "drive_id": drive_id,
                "file_id": request.media_id
            }))
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let body = self.response_json(response, "Aliyun download URL").await?;
        if body.get("code").and_then(Value::as_str).is_some() {
            return Err(map_aliyun_error(&body));
        }
        let url =
            value_string(&body, &["url", "download_url", "downloadUrl"]).ok_or_else(|| {
                ProviderError::Protocol {
                    code: "aliyun_missing_download_url".into(),
                    message: "Aliyun did not return a downloadable URL".into(),
                }
            })?;
        Ok(PlaybackDescriptor {
            source: self.id.into(),
            url,
            headers: BTreeMap::new(),
            quality: request.quality,
            expires_at: body
                .get("expiration")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339_unix),
            media_id: request.media_id,
            outcome: "direct".into(),
            qualities: None,
        })
    }

    async fn exchange(&self, form: Vec<(&str, String)>) -> Result<Session, ProviderError> {
        let response = self
            .client
            .post(&self.config.token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let status = response.status();
        let body = response
            .json::<Value>()
            .await
            .map_err(|error| ProviderError::Network(format!("invalid OAuth response: {error}")))?;
        if !status.is_success() {
            return Err(ProviderError::Protocol {
                code: status.as_u16().to_string(),
                message: body
                    .get("error_description")
                    .or_else(|| body.get("message"))
                    .or_else(|| body.get("error"))
                    .and_then(Value::as_str)
                    .unwrap_or("OAuth token exchange failed")
                    .into(),
            });
        }
        let token = TokenWire::from_value(body)?;
        let session = Session {
            provider_id: self.id.into(),
            account_id: None,
            access_token: token.access_token.unwrap_or_default(),
            refresh_token: token.refresh_token,
            expires_at: token
                .expires_in
                .map(|seconds| super::now_unix().saturating_add(seconds as i64)),
        };
        self.session
            .write()
            .map_err(|_| ProviderError::Internal("OAuth session lock poisoned".into()))?
            .replace(session.clone());
        Ok(session)
    }

    pub fn authorization_url(&self, state: Option<&str>) -> Result<String, ProviderError> {
        let client_id = self.configured()?;
        let mut url =
            reqwest::Url::parse(&self.config.authorization_endpoint).map_err(|error| {
                ProviderError::InvalidInput(format!(
                    "invalid OAuth authorization endpoint: {error}"
                ))
            })?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("response_type", "code");
            query.append_pair("client_id", client_id);
            query.append_pair("redirect_uri", &self.config.redirect_uri);
            if let Some(scope) = self
                .config
                .scope
                .as_deref()
                .filter(|scope| !scope.trim().is_empty())
            {
                query.append_pair("scope", scope);
            }
            if let Some(state) = state.filter(|state| !state.trim().is_empty()) {
                query.append_pair("state", state);
            }
        }
        Ok(url.to_string())
    }

    pub async fn exchange_authorization_code(
        &self,
        code: String,
    ) -> Result<Session, ProviderError> {
        if code.trim().is_empty() {
            return Err(ProviderError::InvalidInput(
                "OAuth authorization code is required".into(),
            ));
        }
        let client_id = self.configured()?.to_owned();
        let mut form = vec![
            ("grant_type", "authorization_code".into()),
            ("code", code),
            ("client_id", client_id),
            ("redirect_uri", self.config.redirect_uri.clone()),
        ];
        if let Some(secret) = self
            .config
            .client_secret
            .as_deref()
            .filter(|secret| !secret.trim().is_empty())
        {
            form.push(("client_secret", secret.into()));
        }
        self.exchange(form).await
    }
}

fn map_oauth_error(status: reqwest::StatusCode, body: &Value) -> ProviderError {
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("oauth_error");
    let message = body
        .get("error_description")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("OAuth request failed"));
    match error {
        "authorization_pending" => ProviderError::AuthenticationPending,
        "slow_down" => ProviderError::RateLimited {
            retry_after_secs: None,
        },
        "access_denied" => ProviderError::AuthorizationDenied,
        "expired_token" => ProviderError::SessionExpired,
        "invalid_grant" | "invalid_token" => ProviderError::SessionExpired,
        _ => ProviderError::Protocol {
            code: error.into(),
            message: message.into(),
        },
    }
}

fn poll_error(error: ProviderError) -> Result<PollResult, ProviderError> {
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

#[async_trait]
impl MediaProvider for OAuthProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn authorization_url(&self, state: Option<&str>) -> Result<String, ProviderError> {
        OAuthProvider::authorization_url(self, state)
    }

    async fn exchange_authorization_code(&self, code: String) -> Result<Session, ProviderError> {
        OAuthProvider::exchange_authorization_code(self, code).await
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            device_code_login: self.device_configured(),
            token_import: true,
            session_refresh: true,
            browse_files: self.supports_drive_api(),
            playback_resolution: self.supports_drive_api(),
            ..ProviderCapabilities::default()
        }
    }

    async fn restore_session(&self, session: Session) -> Result<(), ProviderError> {
        if session.provider_id != self.id {
            return Err(ProviderError::InvalidInput(
                "session belongs to another provider".into(),
            ));
        }
        self.session
            .write()
            .map_err(|_| ProviderError::Internal("OAuth session lock poisoned".into()))?
            .replace(session);
        Ok(())
    }

    async fn clear_session(&self) -> Result<(), ProviderError> {
        self.session
            .write()
            .map_err(|_| ProviderError::Internal("OAuth session lock poisoned".into()))?
            .take();
        Ok(())
    }

    async fn login_device_code(&self) -> Result<DeviceCode, ProviderError> {
        if !self.device_configured() {
            return Err(ProviderError::UnsupportedOperation(
                "OAuth device-code endpoint, grant type, and client_id must be configured from official provider documentation".into(),
            ));
        }
        let endpoint = self.config.device_code_endpoint.as_deref().unwrap();
        let client_id = self.configured()?.to_owned();
        let mut form = vec![("client_id", client_id)];
        if let Some(scope) = self
            .config
            .scope
            .as_deref()
            .filter(|v| !v.trim().is_empty())
        {
            form.push(("scope", scope.to_owned()));
        }
        let response = self
            .client
            .post(endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let status = response.status();
        let body = response.json::<Value>().await.map_err(|error| {
            ProviderError::Network(format!("invalid device-code response: {error}"))
        })?;
        if !status.is_success() {
            return Err(map_oauth_error(status, &body));
        }
        let wire: DeviceCodeWire =
            serde_json::from_value(body).map_err(|error| ProviderError::Protocol {
                code: "invalid_device_code_response".into(),
                message: error.to_string(),
            })?;
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
            user_code: wire.user_code.unwrap_or_default(),
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
        if !self.device_configured() {
            return Err(ProviderError::UnsupportedOperation(
                "OAuth device-code endpoint is not configured".into(),
            ));
        }
        if request.device_code.trim().is_empty() {
            return Err(ProviderError::InvalidInput(
                "device_code is required".into(),
            ));
        }
        let client_id = self.configured()?.to_owned();
        let grant_type = self.config.device_code_grant_type.as_deref().unwrap();
        let response = self
            .client
            .post(&self.config.token_endpoint)
            .form(&[
                ("grant_type", grant_type.to_owned()),
                ("device_code", request.device_code),
                ("client_id", client_id),
            ])
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;
        let status = response.status();
        let body = response
            .json::<Value>()
            .await
            .map_err(|error| ProviderError::Network(format!("invalid OAuth response: {error}")))?;
        if !status.is_success() {
            return poll_error(map_oauth_error(status, &body));
        }
        let token = TokenWire::from_value(body)?;
        let session = Session {
            provider_id: self.id.into(),
            account_id: None,
            access_token: token.access_token.unwrap_or_default(),
            refresh_token: token.refresh_token,
            expires_at: token
                .expires_in
                .map(|seconds| super::now_unix().saturating_add(seconds as i64)),
        };
        self.session
            .write()
            .map_err(|_| ProviderError::Internal("OAuth session lock poisoned".into()))?
            .replace(session.clone());
        Ok(PollResult::Authorized(session))
    }
    async fn login_sms(&self, _request: SmsLoginRequest) -> Result<Session, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "OAuth provider does not expose SMS login".into(),
        ))
    }

    async fn import_token(&self, input: TokenImport) -> Result<Session, ProviderError> {
        if input.access_token.trim().is_empty() {
            return Err(ProviderError::InvalidInput(
                "access token is required".into(),
            ));
        }
        let session = Session {
            provider_id: self.id.into(),
            account_id: input.account_id,
            access_token: input.access_token,
            refresh_token: input.refresh_token,
            expires_at: input.expires_at,
        };
        self.restore_session(session.clone()).await?;
        Ok(session)
    }

    async fn refresh_session(&self, session: &Session) -> Result<Session, ProviderError> {
        let refresh_token = session
            .refresh_token
            .clone()
            .ok_or(ProviderError::SessionExpired)?;
        let client_id = self.configured()?.to_owned();
        let mut form = vec![
            (
                "grant_type",
                self.config
                    .refresh_grant_type
                    .clone()
                    .unwrap_or_else(|| "refresh_token".into()),
            ),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ];
        if let Some(secret) = self
            .config
            .client_secret
            .as_deref()
            .filter(|secret| !secret.trim().is_empty())
        {
            form.push(("client_secret", secret.into()));
        }
        self.exchange(form).await
    }

    async fn list_files(&self, request: ListFilesRequest) -> Result<FilePage, ProviderError> {
        let session = self.active_session()?;
        match self.id {
            "baidu" => self.baidu_list_files(request, &session).await,
            "aliyun" => self.aliyun_list_files(request, &session).await,
            _ => Err(ProviderError::UnsupportedOperation(
                "OAuth authentication is available; this provider has no verified public file API adapter".into(),
            )),
        }
    }
    async fn resolve_playback(
        &self,
        request: PlaybackRequest,
    ) -> Result<PlaybackDescriptor, ProviderError> {
        let session = self.active_session()?;
        match self.id {
            "baidu" => self.baidu_resolve_playback(request, &session).await,
            "aliyun" => self.aliyun_resolve_playback(request, &session).await,
            _ => Err(ProviderError::UnsupportedOperation(
                "OAuth authentication is available; this provider has no verified public playback adapter".into(),
            )),
        }
    }
}

fn value_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|field| match field {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
    })
}

fn value_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|field| {
            field
                .as_u64()
                .or_else(|| field.as_str().and_then(|value| value.parse::<u64>().ok()))
        })
    })
}

fn value_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|field| {
            field
                .as_f64()
                .or_else(|| field.as_str().and_then(|value| value.parse::<f64>().ok()))
        })
    })
}

fn baidu_media_item(value: &Value, parent: &str) -> super::MediaItem {
    let is_folder = value_u64(value, &["isdir", "is_dir"]).unwrap_or_default() == 1;
    let path = value_string(value, &["path"]).unwrap_or_default();
    let id = if is_folder {
        path.clone()
    } else {
        value_string(value, &["fs_id", "fsId"]).unwrap_or_else(|| path.clone())
    };
    let category = value_u64(value, &["category"]).unwrap_or_default();
    let mime_type = value_string(value, &["mime_type", "mimeType"])
        .or_else(|| (category == 1).then(|| "video/*".to_owned()));
    let thumbnail_url = value
        .get("thumbs")
        .and_then(|thumbs| value_string(thumbs, &["url3", "url2", "url1"]))
        .or_else(|| value_string(value, &["thumb_url", "thumbnail"]));
    super::MediaItem {
        id,
        name: value_string(value, &["server_filename", "name"])
            .unwrap_or_else(|| "未命名文件".into()),
        kind: if is_folder {
            super::MediaKind::Folder
        } else {
            super::MediaKind::File
        },
        parent_id: Some(parent.into()),
        size_bytes: value_u64(value, &["size"]),
        mime_type,
        duration_seconds: value_f64(value, &["duration"]),
        thumbnail_url,
        metadata: serde_json::json!({
            "path": path,
            "category": category,
            "fsId": value_string(value, &["fs_id", "fsId"])
        }),
    }
}

fn aliyun_media_item(value: &Value) -> super::MediaItem {
    let is_folder =
        value_string(value, &["type"]).is_some_and(|kind| kind.eq_ignore_ascii_case("folder"));
    let category = value_string(value, &["category"]);
    let mime_type = value_string(value, &["mime_type", "mimeType"]).or_else(|| {
        category
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("video"))
            .then(|| "video/*".to_owned())
    });
    let duration_seconds = value
        .get("video_media_metadata")
        .or_else(|| value.get("videoMediaMetadata"))
        .and_then(|metadata| value_f64(metadata, &["duration"]));
    super::MediaItem {
        id: value_string(value, &["file_id", "fileId"]).unwrap_or_default(),
        name: value_string(value, &["name"]).unwrap_or_else(|| "未命名文件".into()),
        kind: if is_folder {
            super::MediaKind::Folder
        } else {
            super::MediaKind::File
        },
        parent_id: value_string(value, &["parent_file_id", "parentFileId"]),
        size_bytes: value_u64(value, &["size"]),
        mime_type,
        duration_seconds,
        thumbnail_url: value_string(value, &["thumbnail", "thumbnail_url", "thumbnailUrl"]),
        metadata: serde_json::json!({
            "category": category,
            "driveId": value_string(value, &["drive_id", "driveId"]),
            "fileExtension": value_string(value, &["file_extension", "fileExtension"])
        }),
    }
}

fn oauth_api_message(body: &Value, operation: &str) -> String {
    value_string(
        body,
        &[
            "message",
            "error_description",
            "error",
            "error_msg",
            "errorMessage",
        ],
    )
    .unwrap_or_else(|| format!("{operation} failed"))
}

fn map_baidu_error(errno: i64, body: &Value) -> ProviderError {
    if matches!(errno, -6 | 111 | 110) {
        return ProviderError::SessionExpired;
    }
    ProviderError::Protocol {
        code: format!("baidu_{errno}"),
        message: oauth_api_message(body, "Baidu API request"),
    }
}

fn map_aliyun_error(body: &Value) -> ProviderError {
    let code = value_string(body, &["code", "error"]).unwrap_or_else(|| "aliyun_error".into());
    if code.to_ascii_lowercase().contains("token")
        || code.to_ascii_lowercase().contains("unauthorized")
    {
        return ProviderError::SessionExpired;
    }
    ProviderError::Protocol {
        code,
        message: oauth_api_message(body, "Aliyun API request"),
    }
}

fn parse_rfc3339_unix(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_url_contains_standard_oauth_parameters() {
        let provider = OAuthProvider::new(
            "baidu",
            OAuthProviderConfig {
                authorization_endpoint: "https://example.test/authorize".into(),
                token_endpoint: "https://example.test/token".into(),
                client_id: Some("client-123".into()),
                redirect_uri: "http://127.0.0.1/callback".into(),
                scope: Some("files.read".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let url = provider.authorization_url(Some("state-1")).unwrap();
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-123"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback"));
        assert!(url.contains("scope=files.read"));
        assert!(url.contains("state=state-1"));
    }

    #[test]
    fn token_wire_accepts_camel_and_snake_case() {
        let camel = TokenWire::from_value(
            serde_json::json!({"accessToken":"a","refreshToken":"r","expiresIn":60}),
        )
        .unwrap();
        assert_eq!(camel.access_token.as_deref(), Some("a"));
        let snake = TokenWire::from_value(
            serde_json::json!({"access_token":"a","refresh_token":"r","expires_in":60}),
        )
        .unwrap();
        assert_eq!(snake.access_token.as_deref(), Some("a"));
    }

    #[test]
    fn device_capability_requires_explicit_official_endpoint() {
        let provider = OAuthProvider::new(
            "baidu",
            OAuthProviderConfig {
                authorization_endpoint: "https://example.test/authorize".into(),
                token_endpoint: "https://example.test/token".into(),
                client_id: Some("client".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!provider.capabilities().device_code_login);
    }

    #[test]
    fn public_drive_capabilities_are_enabled_only_for_verified_adapters() {
        let baidu = OAuthProvider::new(
            "baidu",
            OAuthProviderConfig {
                authorization_endpoint: "https://example.test/authorize".into(),
                token_endpoint: "https://example.test/token".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let tianyi = OAuthProvider::new(
            "tianyi",
            OAuthProviderConfig {
                authorization_endpoint: "https://example.test/authorize".into(),
                token_endpoint: "https://example.test/token".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(baidu.capabilities().browse_files);
        assert!(baidu.capabilities().playback_resolution);
        assert!(!tianyi.capabilities().browse_files);
        assert!(!tianyi.capabilities().playback_resolution);
    }

    #[test]
    fn baidu_items_keep_folder_paths_and_file_ids() {
        let folder = baidu_media_item(
            &serde_json::json!({
                "fs_id": 10,
                "path": "/Movies",
                "server_filename": "Movies",
                "isdir": 1
            }),
            "/",
        );
        let file = baidu_media_item(
            &serde_json::json!({
                "fs_id": 11,
                "path": "/Movies/demo.mp4",
                "server_filename": "demo.mp4",
                "isdir": 0,
                "category": 1,
                "size": 1024
            }),
            "/Movies",
        );
        assert_eq!(folder.id, "/Movies");
        assert_eq!(folder.kind, super::super::MediaKind::Folder);
        assert_eq!(file.id, "11");
        assert_eq!(file.mime_type.as_deref(), Some("video/*"));
    }

    #[test]
    fn aliyun_video_metadata_is_normalized() {
        let item = aliyun_media_item(&serde_json::json!({
            "file_id": "file-1",
            "parent_file_id": "root",
            "name": "demo.mkv",
            "type": "file",
            "category": "video",
            "size": 2048,
            "thumbnail": "https://example.test/thumb.jpg",
            "video_media_metadata": {"duration": 42.5}
        }));
        assert_eq!(item.id, "file-1");
        assert_eq!(item.mime_type.as_deref(), Some("video/*"));
        assert_eq!(item.duration_seconds, Some(42.5));
    }
}
