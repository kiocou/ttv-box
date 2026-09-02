//! StreamHub local media-center adapter.
//!
//! The supported contract is recovered from the locally archived StreamHub
//! application. It talks only to an explicitly configured StreamHub instance;
//! credentials are supplied by the user through the existing token-import flow.

use std::sync::RwLock;

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    DeviceCode, DevicePollRequest, FilePage, ListFilesRequest, MediaItem, MediaKind, MediaProvider,
    PlaybackDescriptor, PlaybackRequest, PollResult, ProviderCapabilities, ProviderError, Session,
    SmsLoginRequest, TokenImport,
};

pub const PROVIDER_ID: &str = "streamhub";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct StreamHubConfig {
    /// The StreamHub process is expected to be user-managed. LumiPlayer starts
    /// it on a dynamic loopback port; standalone deployments can set this URL.
    pub base_url: String,
    pub user_agent: Option<String>,
    /// Start the bundled/local StreamHub JAR from the desktop process.
    #[serde(default)]
    pub auto_start: bool,
    /// Optional explicit path to streamhub-local-api.jar.
    #[serde(default)]
    pub jar_path: Option<String>,
    /// Optional Java executable path. Defaults to `java` on PATH.
    #[serde(default)]
    pub java_path: Option<String>,
}

impl Default for StreamHubConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:18400".into(),
            user_agent: Some("TTV/0.1 StreamHub adapter".into()),
            auto_start: false,
            jar_path: None,
            java_path: None,
        }
    }
}

impl StreamHubConfig {
    fn endpoint(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

#[derive(Debug, Default)]
struct StreamHubState {
    session: Option<Session>,
}

pub struct StreamHubProvider {
    client: Client,
    config: StreamHubConfig,
    state: RwLock<StreamHubState>,
}

impl std::fmt::Debug for StreamHubProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StreamHubProvider")
            .field("id", &PROVIDER_ID)
            .field("base_url", &self.config.base_url)
            .finish()
    }
}

impl StreamHubProvider {
    pub fn new(config: StreamHubConfig) -> Result<Self, ProviderError> {
        if !(config.base_url.starts_with("http://") || config.base_url.starts_with("https://")) {
            return Err(ProviderError::InvalidInput(
                "StreamHub base_url must start with http:// or https://".into(),
            ));
        }
        let mut builder = Client::builder().timeout(std::time::Duration::from_secs(20));
        if let Some(user_agent) = config
            .user_agent
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            builder = builder.user_agent(user_agent.to_owned());
        }
        let client = builder.build().map_err(|error| {
            ProviderError::Internal(format!("cannot build StreamHub client: {error}"))
        })?;
        Ok(Self {
            client,
            config,
            state: RwLock::new(StreamHubState::default()),
        })
    }

    fn session(&self) -> Result<Option<Session>, ProviderError> {
        self.state
            .read()
            .map_err(|_| ProviderError::Internal("StreamHub session lock poisoned".into()))
            .map(|state| state.session.clone())
    }

    fn request(&self, path: &str) -> Result<reqwest::RequestBuilder, ProviderError> {
        let request = self.client.get(self.config.endpoint(path));
        match self.session()? {
            Some(session) if session.is_expired() => Err(ProviderError::SessionExpired),
            Some(session) => Ok(request.bearer_auth(session.access_token)),
            None => Ok(request),
        }
    }

    async fn json_response(&self, response: reqwest::Response) -> Result<Value, ProviderError> {
        let status = response.status();
        let body = response.json::<Value>().await.map_err(|error| {
            ProviderError::Network(format!("invalid StreamHub response: {error}"))
        })?;
        if status.is_success() {
            return Ok(body);
        }
        Err(map_http_error(status, &body))
    }

    fn media_file_id(detail: &Value, quality: Option<&str>) -> Option<String> {
        let versions = detail.get("versions").and_then(Value::as_array);
        let from_versions = versions.and_then(|versions| {
            quality
                .and_then(|quality| {
                    versions.iter().find(|version| {
                        version
                            .get("qualityLabel")
                            .and_then(Value::as_str)
                            .is_some_and(|label| label.eq_ignore_ascii_case(quality))
                    })
                })
                .or_else(|| {
                    versions.iter().find(|version| {
                        version.get("selected").and_then(Value::as_bool) == Some(true)
                    })
                })
                .or_else(|| versions.first())
                .and_then(|version| version.get("mediaFileId"))
                .and_then(value_id)
        });
        from_versions.or_else(|| detail.get("mediaFileId").and_then(value_id))
    }

    async fn list_show_episodes(
        &self,
        parent_id: &str,
        page_token: Option<&str>,
        page_size: usize,
    ) -> Result<FilePage, ProviderError> {
        let (media_type, show_id) = parent_id.split_once(':').ok_or_else(|| {
            ProviderError::InvalidInput("StreamHub parent id must be show:<id>".into())
        })?;
        if !matches!(media_type, "show" | "tv" | "series" | "tv_show") {
            return Ok(FilePage::default());
        }
        let detail = self
            .json_response(
                self.request(&format!("/api/library/shows/{show_id}"))?
                    .send()
                    .await
                    .map_err(network_error)?,
            )
            .await?;
        let show_title = detail
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut episodes = detail
            .get("episodes")
            .and_then(Value::as_array)
            .map_or(&[][..], |items| items.as_slice())
            .iter()
            .filter_map(|episode| self.episode_to_item(show_id, show_title, episode))
            .collect::<Vec<_>>();
        episodes.sort_by_key(episode_sort_key);

        let total = episodes.len();
        let offset = page_token
            .and_then(|token| token.parse::<usize>().ok())
            .unwrap_or(0)
            .min(total);
        let files = episodes
            .into_iter()
            .skip(offset)
            .take(page_size)
            .collect::<Vec<_>>();
        let next_offset = offset.saturating_add(files.len());
        Ok(FilePage {
            files,
            next_page_token: (next_offset < total).then(|| next_offset.to_string()),
            total: Some(total as u64),
        })
    }

    fn episode_to_item(
        &self,
        show_id: &str,
        show_title: &str,
        episode: &Value,
    ) -> Option<MediaItem> {
        let file_id = Self::media_file_id(episode, None)?;
        let provider_media_id = format!("file:{file_id}");
        let season_number = episode
            .get("seasonNumber")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let episode_number = episode
            .get("episodeNumber")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let name = episode
            .get("title")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| match (season_number, episode_number) {
                (season, number) if season > 0 && number > 0 => {
                    format!("S{season:02}E{number:02}")
                }
                (_, number) if number > 0 => format!("第 {number:02} 集"),
                _ => "未命名剧集".into(),
            });
        let thumbnail_url = episode
            .get("stillPath")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(|path| {
                if path.starts_with("http://") || path.starts_with("https://") {
                    path.to_owned()
                } else {
                    self.config.endpoint(path)
                }
            });
        let mut streamhub = episode.clone();
        if let Some(metadata) = streamhub.as_object_mut() {
            metadata.insert("mediaType".into(), Value::String("episode".into()));
            metadata.insert("showId".into(), Value::String(show_id.into()));
            metadata.insert("showTitle".into(), Value::String(show_title.into()));
            metadata.insert(
                "providerMediaId".into(),
                Value::String(provider_media_id.clone()),
            );
        }
        Some(MediaItem {
            id: provider_media_id.clone(),
            name,
            kind: MediaKind::File,
            parent_id: Some(format!("show:{show_id}")),
            size_bytes: None,
            mime_type: Some("video/*".into()),
            duration_seconds: episode.get("durationSeconds").and_then(Value::as_f64),
            thumbnail_url,
            metadata: json!({
                "streamhub": streamhub,
                "providerId": PROVIDER_ID,
                "mediaId": provider_media_id,
            }),
        })
    }

    async fn resolve_media_file_id(
        &self,
        media_id: &str,
        quality: Option<&str>,
    ) -> Result<String, ProviderError> {
        if let Some(file_id) = media_id.strip_prefix("file:") {
            return non_empty(file_id, "StreamHub media file id is empty");
        }
        let (media_type, item_id) = media_id.split_once(':').ok_or_else(|| {
            ProviderError::InvalidInput(
                "StreamHub media id must be file:<id>, movie:<id>, or show:<id>".into(),
            )
        })?;
        let path = match media_type {
            "movie" => format!("/api/library/movies/{item_id}"),
            "show" | "tv" | "series" | "tv_show" => {
                format!("/api/library/shows/{item_id}")
            }
            _ => {
                return Err(ProviderError::InvalidInput(
                    "unsupported StreamHub media type".into(),
                ))
            }
        };
        let detail = self
            .json_response(self.request(&path)?.send().await.map_err(network_error)?)
            .await?;
        Self::media_file_id(&detail, quality).ok_or_else(|| {
            ProviderError::NotFound("StreamHub item has no playable media-file version".into())
        })
    }
}

#[async_trait]
impl MediaProvider for StreamHubProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            token_import: true,
            browse_files: true,
            playback_resolution: true,
            ..ProviderCapabilities::default()
        }
    }

    async fn restore_session(&self, session: Session) -> Result<(), ProviderError> {
        if session.provider_id != PROVIDER_ID {
            return Err(ProviderError::InvalidInput(
                "session belongs to another provider".into(),
            ));
        }
        self.state
            .write()
            .map_err(|_| ProviderError::Internal("StreamHub session lock poisoned".into()))?
            .session = Some(session);
        Ok(())
    }

    async fn login_device_code(&self) -> Result<DeviceCode, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "StreamHub uses its own local login form; import an access token when desktop proxy is disabled".into(),
        ))
    }

    async fn poll_device_token(
        &self,
        _request: DevicePollRequest,
    ) -> Result<PollResult, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "StreamHub does not expose device-code login".into(),
        ))
    }

    async fn login_sms(&self, _request: SmsLoginRequest) -> Result<Session, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "StreamHub does not expose SMS login".into(),
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
            refresh_token: None,
            expires_at: input.expires_at,
        };
        self.restore_session(session.clone()).await?;
        Ok(session)
    }

    async fn refresh_session(&self, _session: &Session) -> Result<Session, ProviderError> {
        Err(ProviderError::UnsupportedOperation(
            "StreamHub token refresh uses an HttpOnly refresh cookie and is intentionally not replicated by this adapter".into(),
        ))
    }

    async fn list_files(&self, request: ListFilesRequest) -> Result<FilePage, ProviderError> {
        let page_size = request.page_size.unwrap_or(100).clamp(1, 500) as usize;
        if let Some(parent_id) = request.parent_id.as_deref() {
            return self
                .list_show_episodes(parent_id, request.page_token.as_deref(), page_size)
                .await;
        }
        let offset = request
            .page_token
            .as_deref()
            .and_then(|token| token.parse::<usize>().ok())
            .unwrap_or(0);
        let page = offset / page_size + 1;
        let mut url =
            reqwest::Url::parse(&self.config.endpoint("/api/library/browse")).map_err(|error| {
                ProviderError::InvalidInput(format!("invalid StreamHub base_url: {error}"))
            })?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("page", &page.to_string());
            query.append_pair("pageSize", &page_size.to_string());
            if let Some(keyword) = request
                .query
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                query.append_pair("keyword", keyword);
            }
        }
        let session = self.session()?;
        if session.as_ref().is_some_and(Session::is_expired) {
            return Err(ProviderError::SessionExpired);
        }
        let mut http_request = self.client.get(url);
        if let Some(session) = session {
            http_request = http_request.bearer_auth(session.access_token);
        }
        let body = self
            .json_response(http_request.send().await.map_err(network_error)?)
            .await?;
        let files = body
            .get("items")
            .and_then(Value::as_array)
            .map_or(&[][..], |items| items.as_slice())
            .iter()
            .filter_map(|card| self.card_to_item(card))
            .collect::<Vec<_>>();
        let total = body.get("total").and_then(Value::as_u64);
        let has_more = body
            .get("hasMore")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let next_page_token = has_more.then(|| (offset + files.len()).to_string());
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
        let file_id = self
            .resolve_media_file_id(&request.media_id, request.quality.as_deref())
            .await?;
        let body = self
            .json_response(
                self.request(&format!("/api/stream/media-files/{file_id}/playable"))?
                    .send()
                    .await
                    .map_err(network_error)?,
            )
            .await?;
        let raw_url = body
            .get("url")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ProviderError::Protocol {
                code: "missing_playable_url".into(),
                message: "StreamHub playable response did not contain url".into(),
            })?;
        let url = if raw_url.starts_with("http://") || raw_url.starts_with("https://") {
            raw_url.to_owned()
        } else {
            self.config.endpoint(raw_url)
        };
        let mut headers = std::collections::BTreeMap::new();
        if let Some(session) = self.session()? {
            headers.insert(
                "Authorization".into(),
                format!("Bearer {}", session.access_token),
            );
        }
        Ok(PlaybackDescriptor {
            source: PROVIDER_ID.into(),
            url,
            headers,
            quality: request.quality,
            expires_at: None,
            media_id: request.media_id,
            outcome: "streamhub-playable".into(),
            qualities: None,
        })
    }
}

impl StreamHubProvider {
    fn card_to_item(&self, card: &Value) -> Option<MediaItem> {
        let numeric_id = card.get("id").and_then(value_id)?;
        let media_type = card
            .get("mediaType")
            .and_then(Value::as_str)
            .unwrap_or("movie")
            .to_ascii_lowercase();
        let id = format!("{media_type}:{numeric_id}");
        let name = card
            .get("title")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())?
            .to_owned();
        let thumbnail_url = card
            .get("posterPath")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(|path| {
                if path.starts_with("http://") || path.starts_with("https://") {
                    path.to_owned()
                } else {
                    self.config.endpoint(path)
                }
            });
        Some(MediaItem {
            id,
            name,
            kind: MediaKind::File,
            parent_id: None,
            size_bytes: None,
            mime_type: Some("video/*".into()),
            duration_seconds: card.get("durationSeconds").and_then(Value::as_f64),
            thumbnail_url,
            metadata: json!({
                "streamhub": card,
                "providerId": PROVIDER_ID,
            }),
        })
    }
}

fn value_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
}

fn episode_sort_key(item: &MediaItem) -> (i64, i64, String) {
    let episode = item.metadata.get("streamhub").unwrap_or(&item.metadata);
    (
        episode
            .get("seasonNumber")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        episode
            .get("episodeNumber")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        item.id.clone(),
    )
}

fn non_empty(value: &str, message: &str) -> Result<String, ProviderError> {
    (!value.trim().is_empty())
        .then(|| value.to_owned())
        .ok_or_else(|| ProviderError::InvalidInput(message.into()))
}

fn network_error(error: reqwest::Error) -> ProviderError {
    ProviderError::Network(error.to_string())
}

fn map_http_error(status: StatusCode, body: &Value) -> ProviderError {
    match status {
        StatusCode::UNAUTHORIZED => ProviderError::NotAuthenticated,
        StatusCode::FORBIDDEN => ProviderError::AuthorizationDenied,
        StatusCode::NOT_FOUND => ProviderError::NotFound(
            body.get("message")
                .and_then(Value::as_str)
                .unwrap_or("StreamHub resource")
                .into(),
        ),
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited {
            retry_after_secs: None,
        },
        _ => ProviderError::Protocol {
            code: status.as_u16().to_string(),
            message: body
                .get("message")
                .or_else(|| body.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("StreamHub request failed")
                .into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_browse_cards_to_provider_items() {
        let provider = StreamHubProvider::new(StreamHubConfig::default()).unwrap();
        let item = provider
            .card_to_item(&json!({
                "id": 42,
                "mediaType": "movie",
                "title": "A Real Movie",
                "durationSeconds": 600,
                "posterPath": "/cache/images/poster.jpg"
            }))
            .unwrap();
        assert_eq!(item.id, "movie:42");
        assert_eq!(item.name, "A Real Movie");
        assert_eq!(item.duration_seconds, Some(600.0));
        assert_eq!(
            item.thumbnail_url.as_deref(),
            Some("http://127.0.0.1:18400/cache/images/poster.jpg")
        );
    }

    #[test]
    fn chooses_selected_or_first_media_version() {
        let detail = json!({
            "mediaFileId": 1,
            "versions": [
                {"mediaFileId": 2, "qualityLabel": "1080P"},
                {"mediaFileId": 3, "qualityLabel": "4K", "selected": true}
            ]
        });
        assert_eq!(
            StreamHubProvider::media_file_id(&detail, Some("4k")),
            Some("3".into())
        );
        assert_eq!(
            StreamHubProvider::media_file_id(&detail, None),
            Some("3".into())
        );
    }

    #[test]
    fn show_container_does_not_silently_resolve_to_first_episode() {
        let detail = json!({
            "id": 10,
            "mediaType": "show",
            "episodes": [
                {"id": 101, "episodeNumber": 1, "mediaFileId": 201},
                {"id": 102, "episodeNumber": 2, "mediaFileId": 202}
            ]
        });
        assert_eq!(StreamHubProvider::media_file_id(&detail, None), None);
    }

    #[test]
    fn maps_episode_to_its_own_playable_media_file() {
        let provider = StreamHubProvider::new(StreamHubConfig::default()).unwrap();
        let item = provider
            .episode_to_item(
                "10",
                "A Real Show",
                &json!({
                    "id": 102,
                    "seasonNumber": 1,
                    "episodeNumber": 2,
                    "title": "Second Episode",
                    "durationSeconds": 2700,
                    "stillPath": "/cache/images/episode-2.jpg",
                    "mediaFileId": 202
                }),
            )
            .unwrap();

        assert_eq!(item.id, "file:202");
        assert_eq!(item.parent_id.as_deref(), Some("show:10"));
        assert_eq!(item.name, "Second Episode");
        assert_eq!(item.duration_seconds, Some(2700.0));
        assert_eq!(
            item.metadata["streamhub"]["episodeNumber"].as_i64(),
            Some(2)
        );
        assert_eq!(
            item.metadata["streamhub"]["providerMediaId"].as_str(),
            Some("file:202")
        );
    }
}
