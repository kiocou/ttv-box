//! 糖心影院 (Tangxin cinema) catalog client for the adult zone.
//!
//! Protocol ported from the open-source tangxin-zhizhe Chrome extension
//! (github.com/lsy5920/tangxin-zhizhe-extension, verified 2026-08-31 against
//! the live site with the same encrypted contract):
//!
//! - `POST https://txh068.com/h5{endpoint}` with `Content-Type: text/plain`,
//!   headers `deviceType: web`, `time` (unix seconds), `version: 4.76`.
//! - Request body = JSON `{data, token, deviceId, device, source, driver}`,
//!   PKCS7-padded, AES-128-**ECB** encrypted (key `fd14f9f8e38808fa`) block by
//!   block, base64 encoded.
//! - Response body = same scheme (base64 of ECB blocks); a plain-JSON body is
//!   accepted too. Top-level `status: "y"` marks success.
//! - Guest sessions: `/system/info` then `/system/menu` returns
//!   `{token, user_id}` → `userToken = "{token}_{user_id}"`. Issuance is
//!   IP-rate-limited, so the session is persisted and reused across runs.
//! - Authorized sessions: set `TANGXIN_JWT` (a site-issued token/JWT) in the
//!   backend environment. Set `TANGXIN_DEVICE_ID` to the device used for that
//!   authorization; set `TANGXIN_USER_ID` when the value is the raw
//!   `/user/findByAccount` token. Authorized sessions are never written to disk.
//! - Account pool (`tangxin-accounts.json`): real site accounts for
//!   full-duration playback, ported from the extension's account-pool rotation.
//!   Credentials come either from password login (`/user/findByAccount`) or
//!   imported token/deviceId (and qrcode) credentials — e.g. ones shared in a
//!   group. Browsing stays on the guest session; only an explicit play gesture
//!   rotates accounts. Coin-locked content is never charged without the
//!   frontend's explicit `allow_buy` confirmation.
//! - Catalog posters are `.bnc` AES-128-**ECB** blobs (key `525202f9149e061d`,
//!   PKCS7) that decrypt to JPG/PNG/GIF/WebP.
//! - Site domains rotate (the CDN distribution can vanish overnight —
//!   verified 2026-09-02 when txh068.com went NXDOMAIN-ish worldwide). Set
//!   `TANGXIN_API_BASE` (e.g. `https://txh069.com`) to point at a new domain
//!   without rebuilding; it overrides Origin/Referer/request URLs.
//! - Browsing is read-only (`/movie/block`, `/movie/search`, whitelisted
//!   fields only). `play_link` is fetched only on an explicit user play
//!   gesture and is a POST-only m3u8, so playback resolves through a local
//!   playlist file handed to the native player.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::adult::curl_fetch::CurlFetch;
use crate::error::AppError;

/// Rotated from txh068.com on 2026-09-02 (the old CloudFront distribution
/// vanished from global DNS). `TANGXIN_API_BASE` overrides future rotations.
const API_BASE_DEFAULT: &str = "https://txh092.com";

/// Site base URL. `TANGXIN_API_BASE` overrides the built-in domain (these
/// sites rotate domains when the old distribution dies); read once per run.
fn api_base() -> &'static str {
    static BASE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BASE.get_or_init(|| {
        std::env::var("TANGXIN_API_BASE")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| API_BASE_DEFAULT.to_owned())
    })
}

const API_AES_KEY: &[u8; 16] = b"fd14f9f8e38808fa";
const POSTER_AES_KEY: &[u8; 16] = b"525202f9149e061d";
const API_VERSION: &str = "4.76";
const API_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
/// Shared cloud account pool worker (from tangxin-zhizhe-extension).
pub const REMOTE_BASE_URL_DEFAULT: &str = "https://txzzsecure.lsy20.top";
/// Bearer token the extension ships for the same worker; required by every
/// /v1/* and /v2/* pool endpoint.
const REMOTE_ACCESS_TOKEN: &str = "txzz_builtin_5b8d0ce4a7f341d99e6c2f183b704ad6_7c15f8a2";
const SESSION_FILE: &str = "tangxin-session.json";
const AUTH_TOKEN_ENV: &str = "TANGXIN_JWT";
const AUTH_USER_ID_ENV: &str = "TANGXIN_USER_ID";
const AUTH_DEVICE_ID_ENV: &str = "TANGXIN_DEVICE_ID";
const MAX_SESSION_ATTEMPTS: u32 = 3;
const MAX_ITEMS: usize = 600;
const MAX_SECTION_ITEMS: usize = 40;
const MAX_SECTIONS: usize = 24;
const MAX_COLLECTION_ITEMS: usize = 120;
const DEFAULT_PAGE_SIZE: u32 = 24;
const MAX_PAGE_SIZE: u32 = 48;
const MAX_POSTER_BYTES: usize = 6 * 1024 * 1024;
const AES_BLOCK: usize = 16;

/* ---------------- AES-128-ECB ---------------- */

/// PKCS7-pad then ECB-encrypt, mirroring the extension block-by-block flow.
fn encrypt_ecb(key: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    use aes::cipher::{BlockEncrypt as _, KeyInit as _};
    let cipher = aes::Aes128::new_from_slice(key).expect("128-bit key");
    let pad = AES_BLOCK - (plain.len() % AES_BLOCK);
    let mut padded = plain.to_vec();
    padded.resize(plain.len() + pad, pad as u8);
    let mut out = Vec::with_capacity(padded.len());
    let mut block = [0u8; AES_BLOCK];
    for chunk in padded.chunks_exact(AES_BLOCK) {
        block.copy_from_slice(chunk);
        let mut block = aes::cipher::generic_array::GenericArray::from_mut_slice(&mut block);
        cipher.encrypt_block(&mut block);
        out.extend_from_slice(&block);
    }
    out
}

/// ECB-decrypt every block, then strip PKCS7 padding once at the end.
fn decrypt_ecb(key: &[u8; 16], cipher: &[u8]) -> Result<Vec<u8>, AppError> {
    use aes::cipher::{BlockDecrypt as _, KeyInit as _};
    if cipher.is_empty() || cipher.len() % AES_BLOCK != 0 {
        return Err(AppError::Provider("加密数据分块长度无效".into()));
    }
    let decryptor = aes::Aes128::new_from_slice(key).expect("128-bit key");
    let mut out = Vec::with_capacity(cipher.len());
    let mut block = [0u8; AES_BLOCK];
    for chunk in cipher.chunks_exact(AES_BLOCK) {
        block.copy_from_slice(chunk);
        let mut block = aes::cipher::generic_array::GenericArray::from_mut_slice(&mut block);
        decryptor.decrypt_block(&mut block);
        out.extend_from_slice(&block);
    }
    let pad = *out
        .last()
        .ok_or_else(|| AppError::Provider("解密结果为空".into()))?;
    if pad == 0 || pad as usize > AES_BLOCK || pad as usize > out.len() {
        return Err(AppError::Provider("PKCS7 填充无效".into()));
    }
    if out[out.len() - pad as usize..]
        .iter()
        .any(|byte| *byte != pad)
    {
        return Err(AppError::Provider("PKCS7 填充不一致".into()));
    }
    out.truncate(out.len() - pad as usize);
    Ok(out)
}

/* ---------------- small text helpers ---------------- */

fn safe_text(value: &Value, max: usize) -> String {
    let text = value.as_str().unwrap_or_default();
    let cleaned: String = text
        .chars()
        .map(|ch| {
            if (ch as u32) < 0x20 || ch as u32 == 0x7f {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let collapsed: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect()
}

fn value_text(value: &Value, keys: &[&str], max: usize) -> String {
    for key in keys {
        let text = safe_text(&value[*key], max);
        if !text.is_empty() {
            return text;
        }
    }
    String::new()
}

fn normalize_asset_url(value: &Value, keys: &[&str]) -> String {
    let text = value_text(value, keys, 1800);
    if text.is_empty() {
        return String::new();
    }
    let parsed = match reqwest::Url::parse(&text)
        .or_else(|_| reqwest::Url::parse(&format!("https://txh068.com/{text}")))
    {
        Ok(url) => url,
        Err(_) => return String::new(),
    };
    match parsed.scheme() {
        "http" | "https" => parsed.to_string(),
        _ => String::new(),
    }
}

fn finite_u64(value: &Value) -> u64 {
    if let Some(num) = value.as_f64() {
        return if num.is_finite() && num > 0.0 {
            num as u64
        } else {
            0
        };
    }
    // Upstream sends counters/prices as numeric strings ("30").
    value
        .as_str()
        .and_then(|text| text.trim().parse::<f64>().ok())
        .filter(|num| num.is_finite() && *num > 0.0)
        .map(|num| num as u64)
        .unwrap_or(0)
}

/// `duration`-ish fields arrive as seconds ("90") or "mm:ss" / "hh:mm:ss".
fn parse_duration_seconds(value: &Value) -> u64 {
    if let Some(num) = value.as_f64() {
        return if num.is_finite() && num > 0.0 {
            num as u64
        } else {
            0
        };
    }
    let text = safe_text(value, 32);
    if text.is_empty() {
        return 0;
    }
    if let Ok(num) = text.parse::<f64>() {
        return if num > 0.0 { num as u64 } else { 0 };
    }
    let parts: Vec<u64> = text
        .split(':')
        .map(|part| part.trim().parse::<u64>().unwrap_or(0))
        .collect();
    match parts.len() {
        2 => parts[0] * 60 + parts[1],
        3 => parts[0] * 3600 + parts[1] * 60 + parts[2],
        _ => 0,
    }
}

fn format_duration(seconds: u64) -> String {
    if seconds == 0 {
        return String::new();
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let rest = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{rest:02}")
    } else {
        format!("{minutes}:{rest:02}")
    }
}

/* ---------------- whitelist model ---------------- */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TangxinMovie {
    pub id: String,
    pub title: String,
    pub poster_url: String,
    pub creator: String,
    pub avatar_url: String,
    pub duration_seconds: u64,
    pub duration_label: String,
    pub orientation: String,
    pub access: String,
    pub price: u64,
    pub views: String,
    pub likes: String,
    pub favorites: String,
    pub score: String,
    pub published_at: String,
    pub badge: String,
    pub is_collection: bool,
}

fn orientation_for(value: &Value) -> String {
    let canvas = safe_text(&value["canvas"], 24).to_lowercase();
    let width = finite_u64(&value["width"]);
    let height = finite_u64(&value["height"]);
    if ["long", "portrait", "vertical"].contains(&canvas.as_str()) {
        return "portrait".into();
    }
    if ["short", "landscape", "horizontal"].contains(&canvas.as_str()) {
        return "landscape".into();
    }
    if canvas == "square" {
        return "square".into();
    }
    if width > 0 && height > 0 {
        let longest = width.max(height);
        if (longest - width.min(height)) as f64 / (longest as f64) < 0.08 {
            return "square".into();
        }
        return if height > width {
            "portrait".into()
        } else {
            "landscape".into()
        };
    }
    "portrait".into()
}

fn access_for(value: &Value) -> String {
    let pay_type = safe_text(&value["pay_type"], 32).to_lowercase();
    if ["money", "coin", "gold"].contains(&pay_type.as_str()) || finite_u64(&value["money"]) > 0 {
        return "coin".into();
    }
    if ["vip", "member"].contains(&pay_type.as_str()) {
        return "vip".into();
    }
    "free".into()
}

/// Explicit field whitelist (extension `normalizeMovie`): unknown upstream
/// fields — including any signed playback URL — never enter catalog state.
fn normalize_movie(value: &Value) -> Option<TangxinMovie> {
    if !value.is_object() {
        return None;
    }
    let id = value_text(value, &["id", "movie_id", "movieId"], 80);
    if id.is_empty() {
        return None;
    }
    let duration_seconds = [
        "durationSeconds",
        "duration",
        "duration_time",
        "time_length",
    ]
    .iter()
    .map(|key| parse_duration_seconds(&value[*key]))
    .max()
    .unwrap_or(0);
    let duration_label = {
        let labelled = value_text(value, &["durationLabel", "duration_time"], 32);
        if labelled.contains(':') {
            labelled
        } else {
            format_duration(duration_seconds)
        }
    };
    let access = access_for(value);
    let price = if access == "coin" {
        finite_u64(&value["money"])
    } else {
        0
    };
    let title = {
        let title = value_text(value, &["name", "title"], 180);
        if title.is_empty() {
            format!("影片 {id}")
        } else {
            title
        }
    };
    let creator = {
        let creator = value_text(
            value,
            &["creator", "nickname", "author_name", "author"],
            100,
        );
        if creator.is_empty() {
            "糖心创作者".into()
        } else {
            creator
        }
    };
    Some(TangxinMovie {
        id,
        title,
        poster_url: normalize_asset_url(value, &["img", "image", "cover", "poster"]),
        creator,
        avatar_url: normalize_asset_url(value, &["headico", "avatar", "avatar_url"]),
        duration_seconds,
        duration_label,
        orientation: orientation_for(value),
        access,
        price,
        views: value_text(value, &["click", "views"], 40),
        likes: value_text(value, &["love", "likes"], 40),
        favorites: value_text(value, &["favorite", "favorites"], 40),
        score: value_text(value, &["score"], 24),
        published_at: value_text(value, &["time", "created_at", "published_at"], 48),
        badge: value_text(value, &["icon", "badge"], 48),
        is_collection: safe_text(&value["is_episode"], 8).eq_ignore_ascii_case("y"),
    })
}

fn append_unique_movies(existing: &mut Vec<TangxinMovie>, incoming: &[Value], max: usize) {
    for raw in incoming {
        let Some(movie) = normalize_movie(raw) else {
            continue;
        };
        if existing.iter().any(|item| item.id == movie.id) {
            continue;
        }
        existing.push(movie);
        if existing.len() >= max {
            return;
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TangxinSection {
    pub id: String,
    pub name: String,
    pub filter_json: Option<String>,
    pub items: Vec<TangxinMovie>,
}

fn normalize_section(value: &Value) -> Option<TangxinSection> {
    if !value.is_object() {
        return None;
    }
    let id = value_text(value, &["id", "block_id"], 80);
    let style = value["style"].as_f64().unwrap_or(0.0);
    if id.is_empty() || style < 0.0 {
        return None;
    }
    let items_raw = value["items"]
        .as_array()
        .or_else(|| value["list"].as_array())
        .or_else(|| value["movies"].as_array())?;
    let mut items = Vec::new();
    append_unique_movies(&mut items, items_raw, MAX_SECTION_ITEMS);
    if items.is_empty() {
        return None;
    }
    let name = {
        let name = value_text(value, &["name", "title"], 100);
        if name.is_empty() {
            "本期推荐".into()
        } else {
            name
        }
    };
    let filter_json = value["filter"]
        .as_str()
        .or_else(|| value["params"].as_str())
        .map(str::to_owned);
    Some(TangxinSection {
        id,
        name,
        filter_json,
        items,
    })
}

fn unwrap_array(value: &Value, keys: &[&str]) -> Vec<Value> {
    if let Some(list) = value.as_array() {
        return list.clone();
    }
    for key in keys {
        if let Some(list) = value[*key].as_array() {
            return list.clone();
        }
    }
    Vec::new()
}

/* ---------------- session ---------------- */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TangxinSession {
    device_id: String,
    user_token: String,
}

fn make_device_id() -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("web_{}", &uuid[..13])
}

fn session_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SESSION_FILE)
}

fn load_session(data_dir: &Path) -> Option<TangxinSession> {
    let text = std::fs::read_to_string(session_path(data_dir)).ok()?;
    let session: TangxinSession = serde_json::from_str(&text).ok()?;
    if session.device_id.is_empty() || session.user_token.is_empty() {
        return None;
    }
    Some(session)
}

fn save_session(data_dir: &Path, session: &TangxinSession) {
    if let Ok(text) = serde_json::to_string(session) {
        let _ = std::fs::write(session_path(data_dir), text);
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn has_composed_user_id_suffix(token: &str) -> bool {
    let Some((prefix, suffix)) = token.rsplit_once('_') else {
        return false;
    };
    !prefix.is_empty() && !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit())
}

/// The extension sends the server-issued credential in the encrypted `token`
/// field. `/user/findByAccount` returns a token plus `user_id`; callers may
/// provide either the already-composed `{token}_{user_id}` value or the raw
/// token together with `TANGXIN_USER_ID`.
fn compose_authorized_token(jwt: &str, user_id: Option<&str>) -> String {
    let jwt = jwt.trim();
    if has_composed_user_id_suffix(jwt) || user_id.is_none() {
        return jwt.to_owned();
    }
    format!("{}_{}", jwt, user_id.unwrap_or_default().trim())
}

/// Read a server-issued authorization session without persisting it. Keeping
/// this credential in the backend environment prevents the frontend from ever
/// receiving or storing it. An absent variable means normal guest mode.
fn configured_auth_session() -> Result<Option<TangxinSession>, AppError> {
    let Some(jwt) = env_non_empty(AUTH_TOKEN_ENV) else {
        return Ok(None);
    };
    if jwt.len() > 8192 {
        return Err(AppError::InvalidInput(format!(
            "{AUTH_TOKEN_ENV} 长度超过限制"
        )));
    }
    let user_id = env_non_empty(AUTH_USER_ID_ENV);
    let user_token = compose_authorized_token(&jwt, user_id.as_deref());
    if user_token.is_empty() {
        return Err(AppError::InvalidInput(format!("{AUTH_TOKEN_ENV} 不能为空")));
    }
    let device_id = env_non_empty(AUTH_DEVICE_ID_ENV).ok_or_else(|| {
        AppError::InvalidInput(format!("配置 {AUTH_DEVICE_ID_ENV} 后才能使用授权播放会话"))
    })?;
    if device_id.len() > 128 {
        return Err(AppError::InvalidInput(format!(
            "{AUTH_DEVICE_ID_ENV} 长度超过限制"
        )));
    }
    Ok(Some(TangxinSession {
        device_id,
        user_token,
    }))
}

/* ---------------- API plumbing ---------------- */

fn api_headers() -> Vec<(String, String)> {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_default();
    vec![
        ("Content-Type".into(), "text/plain".into()),
        ("Accept".into(), "application/json, text/plain, */*".into()),
        ("User-Agent".into(), API_UA.into()),
        ("Origin".into(), api_base().into()),
        ("Referer".into(), format!("{}/", api_base())),
        ("deviceType".into(), "web".into()),
        ("time".into(), time),
        ("version".into(), API_VERSION.into()),
    ]
}

fn encrypted_body(data: Value, session: &TangxinSession) -> Result<String, AppError> {
    let payload = json!({
        "data": data,
        "token": session.user_token,
        "deviceId": session.device_id,
        "device": "Win32",
        "source": "Apple Computer, Inc.",
        "driver": true,
    });
    let plain = serde_json::to_vec(&payload)
        .map_err(|error| AppError::Provider(format!("请求序列化失败：{error}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(encrypt_ecb(API_AES_KEY, &plain)))
}

fn decrypt_response_body(body: &[u8]) -> Result<Value, AppError> {
    let text = String::from_utf8_lossy(body).trim().to_string();
    if text.is_empty() {
        return Err(AppError::Provider("接口返回空响应".into()));
    }
    if let Ok(plain) = serde_json::from_str::<Value>(&text) {
        return Ok(plain);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.as_bytes())
        .map_err(|error| AppError::Provider(format!("响应不是有效密文：{error}")))?;
    let plain = decrypt_ecb(API_AES_KEY, &bytes)?;
    serde_json::from_slice::<Value>(&plain)
        .map_err(|error| AppError::Provider(format!("解密后的响应不是 JSON：{error}")))
}

/// POST one encrypted API call and return the raw HTTP status + body bytes.
/// `endpoint` is the path after the host; a path already starting with `/h5`
/// (e.g. a resolved `play_link`) is used as-is.
async fn api_post_raw(
    client: &CurlFetch,
    session: &TangxinSession,
    endpoint: &str,
    data: Value,
) -> Result<(u16, Vec<u8>), AppError> {
    let path = if endpoint.starts_with("/h5") {
        endpoint.to_owned()
    } else {
        format!("/h5{endpoint}")
    };
    let body = encrypted_body(data, session)?;
    client
        .post(
            &format!("{}{path}", api_base()),
            &api_headers(),
            body.as_bytes(),
        )
        .await
}

async fn api_call(
    client: &CurlFetch,
    session: &TangxinSession,
    endpoint: &str,
    data: Value,
) -> Result<Value, AppError> {
    let (status, response) = api_post_raw(client, session, endpoint, data).await?;
    if status >= 400 || status == 0 {
        return Err(AppError::Provider(format!(
            "接口 HTTP {status}：{}",
            String::from_utf8_lossy(&response)
                .chars()
                .take(160)
                .collect::<String>()
        )));
    }
    let parsed = decrypt_response_body(&response)?;
    let status_flag = safe_text(&parsed["status"], 24);
    if status_flag != "y" {
        let message = value_text(&parsed, &["error", "msg", "message"], 240);
        return Err(AppError::Provider(if message.is_empty() {
            format!("接口返回失败状态 {status_flag}")
        } else {
            message
        }));
    }
    Ok(parsed)
}

async fn create_session(client: &CurlFetch) -> Result<TangxinSession, AppError> {
    let mut last_error = String::from("访客会话创建失败");
    let mut proxy_escalated = false;
    for _ in 0..MAX_SESSION_ATTEMPTS {
        let device_id = make_device_id();
        let bootstrap = TangxinSession {
            device_id: device_id.clone(),
            user_token: String::new(),
        };
        if api_call(client, &bootstrap, "/system/info", json!({}))
            .await
            .is_err()
        {
            last_error = "连接糖心站点失败，请检查网络或代理".into();
            continue;
        }
        let menu = match api_call(
            client,
            &bootstrap,
            "/system/menu",
            json!({"channel_code": "", "share_code": ""}),
        )
        .await
        {
            Ok(menu) => menu,
            Err(error) => {
                last_error = error.to_string();
                continue;
            }
        };
        let data = &menu["data"];
        let token = safe_text(&data["token"], 4096);
        let user_id = safe_text(&data["user_id"], 240);
        if token.is_empty() || user_id.is_empty() {
            // IP-level issuance limit: the direct IP gets no token — escalate
            // once to the local socks proxy (different egress IP) and retry.
            if !proxy_escalated && client.enable_local_socks_fallback() {
                proxy_escalated = true;
                tracing::info!("tangxin: guest token withheld on direct IP; retrying via local socks proxy");
            }
            last_error = "糖心站点未发放访客令牌（可能触发了频控），请稍后再试".into();
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            continue;
        }
        return Ok(TangxinSession {
            device_id,
            user_token: format!("{token}_{user_id}"),
        });
    }
    Err(AppError::Provider(last_error))
}

/// Load the persisted guest session, creating (and saving) one when missing.
async fn ensure_session(client: &CurlFetch, data_dir: &Path) -> Result<TangxinSession, AppError> {
    // Versions with a local account-login UI stored this marker beside the
    // session. Drop that old session once so the login-free client resumes as
    // a visitor instead of silently retaining a user's account token.
    let legacy_identity = data_dir.join("tangxin-identity.json");
    if legacy_identity.exists() {
        let _ = std::fs::remove_file(&legacy_identity);
        let _ = std::fs::remove_file(session_path(data_dir));
    }
    if let Some(session) = configured_auth_session()? {
        return Ok(session);
    }
    if let Some(session) = load_session(data_dir) {
        return Ok(session);
    }
    let session = create_session(client).await?;
    save_session(data_dir, &session);
    Ok(session)
}

/// Drop the persisted session and mint a fresh one (stale-token recovery).
async fn rebuild_session(client: &CurlFetch, data_dir: &Path) -> Result<TangxinSession, AppError> {
    let _ = std::fs::remove_file(session_path(data_dir));
    let session = create_session(client).await?;
    save_session(data_dir, &session);
    Ok(session)
}

fn is_auth_error(error: &AppError) -> bool {
    let message = error.to_string();
    ["登录", "token", "令牌", "身份", "请先", "授权", "过期"]
        .iter()
        .any(|needle| message.contains(needle))
}

/// Whether an error means the shared cloud account pool itself is down, rather
/// than one account's credential being rejected by the site. The worker returns
/// HTTP 500 INTERNAL_ERROR when its qrcode/session backend fails, and HTTP 409
/// ACCOUNT_POOL_EMPTY when no account can be verified.
fn is_cloud_pool_outage(error: &AppError) -> bool {
    let message = error.to_string();
    ["服务内部异常", "账号池没有可用账号", "INTERNAL_ERROR", "ACCOUNT_POOL_EMPTY", "云端账号池", "云端池"]
        .iter()
        .any(|needle| message.contains(needle))
}

/// One read-only API call with persisted-session bootstrap and a single
/// stale-token retry.
async fn sessioned_call(
    client: &CurlFetch,
    data_dir: &Path,
    endpoint: &str,
    data: Value,
) -> Result<Value, AppError> {
    let has_configured_auth = configured_auth_session()?.is_some();
    let session = ensure_session(client, data_dir).await?;
    match api_call(client, &session, endpoint, data.clone()).await {
        Ok(response) => Ok(response),
        Err(error) if !has_configured_auth && is_auth_error(&error) => {
            let session = rebuild_session(client, data_dir).await?;
            api_call(client, &session, endpoint, data).await
        }
        Err(error) => Err(error),
    }
}

/* ---------------- account pool ---------------- */
//
// 站方真实账号会话用于播放解析，取代访客试看。凭据三类（与扩展一致）：
// - 账号密码：/user/findByAccount 登录换取 token（自动刷新并回写落盘）
// - token/deviceId：直接复用已签发凭证（群内共享的凭证也走这里导入）
// - 二维码凭证：/user/findQrcode 找回
// 目录浏览始终走访客会话；只有显式开映才会轮换账号。金币内容默认不扣费，
// 前端二次确认后带 allow_buy 重试才会调用 /movie/doBuy。

pub const ACCOUNTS_FILE: &str = "tangxin-accounts.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TangxinAccount {
    pub id: String,
    pub label: String,
    pub username: String,
    pub password: String,
    pub device_id: String,
    pub user_token: String,
    pub qrcode: String,
    pub nickname: String,
    pub coin: String,
    pub is_vip: bool,
    pub vip_end_time: String,
    pub is_dark_vip: bool,
    pub dark_vip_end_time: String,
    pub available: bool,
    pub unavailable_reason: String,
    pub checked_at: i64,
    /// "" / "local" for user-entered credentials, "cloud" for rows pulled
    /// from the shared worker pool. Defaults keep old files loading.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub cloud_readonly: bool,
    #[serde(default)]
    pub remote_id: String,
}

impl TangxinAccount {
    fn has_password(&self) -> bool {
        !self.username.is_empty() && !self.password.is_empty()
    }

    /// Cloud rows are managed by the worker: they never rotate locally and
    /// their sessions come from the verify endpoint, not stored credentials.
    fn is_cloud(&self) -> bool {
        self.cloud_readonly || self.source == "cloud" || !self.remote_id.is_empty()
    }

    fn has_saved_token(&self) -> bool {
        !self.user_token.is_empty() && !self.device_id.is_empty()
    }

    fn has_credential(&self) -> bool {
        self.has_saved_token() || self.has_password() || !self.qrcode.is_empty()
    }

    fn credential_mode(&self) -> &'static str {
        if self.has_saved_token() {
            "token"
        } else if self.has_password() {
            "password"
        } else if !self.qrcode.is_empty() {
            "qrcode"
        } else {
            "none"
        }
    }

    /// Rotation order: accounts without a coin balance sort last.
    fn coin_value(&self) -> u64 {
        self.coin.trim().parse::<u64>().unwrap_or(u64::MAX)
    }

    fn display_label(&self) -> String {
        for candidate in [&self.label, &self.nickname, &self.username] {
            let trimmed = candidate.trim();
            if !trimmed.is_empty() {
                return trimmed.chars().take(40).collect();
            }
        }
        if self.has_saved_token() {
            return format!("token·{}", mask_secret(&self.user_token, 4, 4));
        }
        if !self.qrcode.is_empty() {
            return format!("凭证·{}", mask_secret(&self.qrcode, 4, 4));
        }
        "未命名账号".into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct TangxinAccountStore {
    accounts: Vec<TangxinAccount>,
    selected_id: String,
    #[serde(default)]
    remote: TangxinRemoteConfig,
}

/// Shared cloud account pool (the tangxin-zhizhe-extension worker). The
/// built-in bearer token ships with the extension source; the pool itself is
/// the source of truth for cloud rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TangxinRemoteConfig {
    pub base_url: String,
    pub enabled: bool,
    /// "cloud" 云端轮班 | "local" 本地值班 | "cloud-first" 云端优先
    pub account_source_mode: String,
    pub fallback_local: bool,
    pub last_sync_at: i64,
    pub last_error: String,
}

impl Default for TangxinRemoteConfig {
    fn default() -> Self {
        Self {
            base_url: REMOTE_BASE_URL_DEFAULT.to_owned(),
            enabled: true,
            account_source_mode: "cloud".to_owned(),
            fallback_local: true,
            last_sync_at: 0,
            last_error: String::new(),
        }
    }
}

impl TangxinRemoteConfig {
    fn normalized(mut self) -> Self {
        self.base_url = self.base_url.trim().trim_end_matches('/').to_owned();
        if self.base_url.is_empty() {
            self.base_url = REMOTE_BASE_URL_DEFAULT.to_owned();
        }
        if !["cloud", "local", "cloud-first"].contains(&self.account_source_mode.as_str()) {
            self.account_source_mode = "cloud".to_owned();
        }
        self
    }
}

/// Masked projection for the UI: secrets never round-trip to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TangxinAccountView {
    pub id: String,
    pub label: String,
    pub credential_mode: &'static str,
    pub credential_hint: String,
    pub nickname: String,
    pub coin: String,
    pub is_vip: bool,
    pub vip_end_time: String,
    pub is_dark_vip: bool,
    pub dark_vip_end_time: String,
    pub available: bool,
    pub unavailable_reason: String,
    pub checked_at: i64,
    pub selected: bool,
    pub is_cloud: bool,
}

/// Everything the user pastes when adding an account.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TangxinAccountInput {
    pub label: String,
    pub username: String,
    pub password: String,
    pub device_id: String,
    pub user_token: String,
    pub qrcode: String,
}

fn mask_secret(value: &str, keep_head: usize, keep_tail: usize) -> String {
    let count = value.chars().count();
    if count == 0 {
        return String::new();
    }
    if count <= keep_head + keep_tail {
        return "*".repeat(count);
    }
    let head: String = value.chars().take(keep_head).collect();
    let tail: String = value.chars().skip(count - keep_tail).collect();
    format!("{head}****{tail}")
}

fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn accounts_path(data_dir: &Path) -> PathBuf {
    data_dir.join(ACCOUNTS_FILE)
}

fn load_account_store(data_dir: &Path) -> TangxinAccountStore {
    std::fs::read_to_string(accounts_path(data_dir))
        .ok()
        .and_then(|text| serde_json::from_str::<TangxinAccountStore>(&text).ok())
        .map(|mut store| {
            store
                .accounts
                .retain(|account| account.has_credential() || account.is_cloud());
            store.remote = store.remote.clone().normalized();
            store
        })
        .unwrap_or_default()
}

fn save_account_store(data_dir: &Path, store: &TangxinAccountStore) {
    if let Ok(text) = serde_json::to_string(store) {
        let _ = std::fs::write(accounts_path(data_dir), text);
    }
}

fn replace_account(store: &mut TangxinAccountStore, account: TangxinAccount) {
    if let Some(slot) = store.accounts.iter_mut().find(|item| item.id == account.id) {
        *slot = account;
    }
}

fn account_to_view(account: &TangxinAccount, selected: bool) -> TangxinAccountView {
    TangxinAccountView {
        id: account.id.clone(),
        label: account.display_label(),
        credential_mode: account.credential_mode(),
        credential_hint: if account.has_password() {
            mask_secret(&account.username, 2, 2)
        } else if account.has_saved_token() {
            mask_secret(&account.device_id, 6, 4)
        } else {
            mask_secret(&account.qrcode, 4, 4)
        },
        nickname: account.nickname.clone(),
        coin: account.coin.clone(),
        is_vip: account.is_vip,
        vip_end_time: account.vip_end_time.clone(),
        is_dark_vip: account.is_dark_vip,
        dark_vip_end_time: account.dark_vip_end_time.clone(),
        available: account.available,
        unavailable_reason: account.unavailable_reason.clone(),
        checked_at: account.checked_at,
        selected,
        is_cloud: account.is_cloud(),
    }
}

fn account_views(store: &TangxinAccountStore) -> Vec<TangxinAccountView> {
    store
        .accounts
        .iter()
        .map(|account| account_to_view(account, account.id == store.selected_id))
        .collect()
}

fn value_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(false),
        Value::String(text) => matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "y" | "yes" | "true" | "1"
        ),
        _ => false,
    }
}

/// Pull `/user/info` fields into the account's cached status and mark it
/// available. The site returns identity plus 普通 VIP (`is_vip`) / 尤物圈
/// (`is_dark_vip`) entitlements and coin balance.
async fn refresh_account_status(
    client: &CurlFetch,
    session: &TangxinSession,
    account: &mut TangxinAccount,
) -> Result<(), AppError> {
    let response = api_call(client, session, "/user/info", json!({})).await?;
    let data = &response["data"];
    account.nickname = value_text(data, &["nickname", "account_name", "username"], 60);
    account.coin = value_text(data, &["coin", "gold", "money"], 32);
    account.is_vip = value_truthy(&data["is_vip"])
        || value_truthy(&data["vip"])
        || value_truthy(&data["has_vip"]);
    account.vip_end_time = value_text(data, &["vip_end_time"], 40);
    account.is_dark_vip = value_truthy(&data["is_dark_vip"])
        || value_truthy(&data["dark_vip"])
        || value_truthy(&data["has_dark_vip"]);
    account.dark_vip_end_time = value_text(data, &["dark_vip_end_time"], 40);
    account.available = true;
    account.unavailable_reason.clear();
    account.checked_at = now_unix_seconds();
    Ok(())
}

fn mark_account_unavailable(account: &mut TangxinAccount, reason: &str) {
    account.available = false;
    account.unavailable_reason = reason.chars().take(240).collect();
    account.checked_at = now_unix_seconds();
}

async fn login_account_by_password(
    client: &CurlFetch,
    data_dir: &Path,
    account: &mut TangxinAccount,
) -> Result<TangxinSession, AppError> {
    let bootstrap = ensure_session(client, data_dir).await?;
    let response = api_call(
        client,
        &bootstrap,
        "/user/findByAccount",
        json!({
            "account_name": account.username,
            "account_password": account.password,
            "type": "login",
        }),
    )
    .await?;
    let token = safe_text(&response["data"]["token"], 4096);
    let user_id = safe_text(&response["data"]["user_id"], 240);
    if token.is_empty() || user_id.is_empty() {
        return Err(AppError::Provider("站点未返回登录令牌".into()));
    }
    let session = TangxinSession {
        device_id: bootstrap.device_id,
        user_token: format!("{token}_{user_id}"),
    };
    refresh_account_status(client, &session, account).await?;
    account.device_id = session.device_id.clone();
    account.user_token = session.user_token.clone();
    Ok(session)
}

async fn restore_account_by_qrcode(
    client: &CurlFetch,
    data_dir: &Path,
    account: &mut TangxinAccount,
) -> Result<TangxinSession, AppError> {
    let bootstrap = ensure_session(client, data_dir).await?;
    let response = api_call(
        client,
        &bootstrap,
        "/user/findQrcode",
        json!({"code": account.qrcode}),
    )
    .await?;
    let token = safe_text(&response["data"]["token"], 4096);
    let user_id = safe_text(&response["data"]["user_id"], 240);
    if token.is_empty() || user_id.is_empty() {
        return Err(AppError::Provider("站点未返回二维码凭证令牌".into()));
    }
    let session = TangxinSession {
        device_id: bootstrap.device_id.clone(),
        user_token: format!("{token}_{user_id}"),
    };
    refresh_account_status(client, &session, account).await?;
    account.device_id = session.device_id.clone();
    account.user_token = session.user_token.clone();
    Ok(session)
}

/// Acquire a verified playback session for one account. Order mirrors the
/// extension: saved token+deviceId → password login → qrcode restore. A stale
/// saved token is dropped so the login path re-issues a fresh one; refreshed
/// credentials are written back into the record (caller persists the store).
async fn acquire_account_session(
    client: &CurlFetch,
    data_dir: &Path,
    account: &mut TangxinAccount,
) -> Result<TangxinSession, AppError> {
    let mut errors: Vec<String> = Vec::new();
    if account.has_saved_token() {
        let session = TangxinSession {
            device_id: account.device_id.clone(),
            user_token: account.user_token.clone(),
        };
        match refresh_account_status(client, &session, account).await {
            Ok(()) => return Ok(session),
            Err(error) => {
                errors.push(format!("已保存凭证无效：{error}"));
                account.user_token.clear();
            }
        }
    }
    if account.has_password() {
        match login_account_by_password(client, data_dir, account).await {
            Ok(session) => return Ok(session),
            Err(error) => errors.push(format!("账号密码登录失败：{error}")),
        }
    }
    if !account.qrcode.is_empty() {
        match restore_account_by_qrcode(client, data_dir, account).await {
            Ok(session) => return Ok(session),
            Err(error) => errors.push(format!("二维码凭证找回失败：{error}")),
        }
    }
    Err(AppError::Provider(if errors.is_empty() {
        "账号没有可用凭据".into()
    } else {
        errors.join("；")
    }))
}

/// Account-session `/movie/detail` with one fresh-login retry on auth errors.
/// Returns the (possibly refreshed) session so playback can keep using it.
async fn account_detail_with_retry(
    client: &CurlFetch,
    data_dir: &Path,
    account: &mut TangxinAccount,
    movie_id: &str,
) -> Result<(TangxinSession, Value), AppError> {
    let session = acquire_account_session(client, data_dir, account).await?;
    match api_call(client, &session, "/movie/detail", json!({"id": movie_id})).await {
        Ok(response) => Ok((session, response)),
        Err(error) if is_auth_error(&error) => {
            account.user_token.clear();
            let refreshed = acquire_account_session(client, data_dir, account).await?;
            let response = api_call(client, &refreshed, "/movie/detail", json!({"id": movie_id})).await?;
            Ok((refreshed, response))
        }
        Err(error) => Err(error),
    }
}

/// Cloud-pool counterpart of `account_detail_with_retry`: cloud rows hold no
/// local credentials, so the worker re-issues a session for this one account
/// (`/v1/accounts/verify`) and playback then proceeds locally on it. One
/// fresh-verify retry on auth errors.
async fn cloud_account_detail_with_retry(
    client: &CurlFetch,
    remote: &TangxinRemoteConfig,
    account: &mut TangxinAccount,
    movie_id: &str,
) -> Result<(TangxinSession, Value), AppError> {
    let session = verify_cloud_account(client, remote, account).await?;
    match api_call(client, &session, "/movie/detail", json!({"id": movie_id})).await {
        Ok(response) => Ok((session, response)),
        Err(error) if is_auth_error(&error) => {
            let refreshed = verify_cloud_account(client, remote, account).await?;
            let response = api_call(client, &refreshed, "/movie/detail", json!({"id": movie_id})).await?;
            Ok((refreshed, response))
        }
        Err(error) => Err(error),
    }
}

const PLAY_LINK_KEYS: &[&str] = &[
    "play_link", "playLink", "play_url", "playUrl", "m3u8", "m3u8_url", "m3u8Url",
    "video_url", "videoUrl", "media_url", "mediaUrl", "backup_link", "backupLink",
    "backup_url", "backupUrl", "second_play_link", "secondPlayLink",
];

/// Mirrors the extension's hasPotentialPlaybackEntitlement: any play-ish field
/// carrying a value means the detail already returned media — never charge
/// coins in that case even if `has_buy` is unset.
fn detail_has_playable_link(data: &Value) -> bool {
    for key in PLAY_LINK_KEYS {
        if !value_text(data, &[key], 16).is_empty() {
            return true;
        }
    }
    if let Some(lines) = data["lines"].as_array() {
        for line in lines {
            if !value_text(line, &["url", "play_link", "link"], 16).is_empty() {
                return true;
            }
        }
    }
    false
}

/// Locked coin content per the extension's isLockedCoinVideo: no media
/// returned, unpaid, priced by coins.
fn detail_locked_coin_price(data: &Value) -> Option<u64> {
    if detail_has_playable_link(data) {
        return None;
    }
    if safe_text(&data["has_buy"], 8) == "y" {
        return None;
    }
    if safe_text(&data["layer_type"], 32) != "money" {
        return None;
    }
    let money = data["money"]
        .as_u64()
        .or_else(|| data["money"].as_str().and_then(|text| text.trim().parse::<u64>().ok()))?;
    (money > 0).then_some(money)
}

fn detail_title(data: &Value, movie_id: &str) -> String {
    let title = value_text(data, &["name", "title"], 180);
    if title.is_empty() {
        format!("影片 {movie_id}")
    } else {
        title
    }
}

fn collect_line_candidates(data: &Value) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    for field in ["play_link", "backup_link"] {
        let url = safe_text(&data[field], 1024);
        if !url.is_empty() {
            candidates.push(url);
        }
    }
    if let Some(lines) = data["lines"].as_array() {
        for line in lines {
            let url = value_text(line, &["url", "play_link", "link"], 1024);
            if !url.is_empty() && !candidates.contains(&url) {
                candidates.push(url);
            }
        }
    }
    candidates
}

struct LineProbe {
    text: String,
    is_preview: bool,
    error: String,
}

/// POST each line candidate (`play_link` is POST-only) and keep the first full
/// (non-preview) playlist; a preview body is retained as a graceful fallback.
async fn probe_play_lines(
    client: &CurlFetch,
    session: &TangxinSession,
    movie_id: &str,
    candidates: &[String],
) -> LineProbe {
    let mut probe = LineProbe {
        text: String::new(),
        is_preview: true,
        error: "所有线路均不可用".into(),
    };
    for candidate in candidates {
        let (status, body) = match api_post_raw(client, session, candidate, json!({"id": movie_id})).await {
            Ok(result) => result,
            Err(error) => {
                probe.error = error.to_string();
                continue;
            }
        };
        let resolved = String::from_utf8_lossy(&body).trim().to_string();
        if status >= 400 || status == 0 || !resolved.starts_with("#EXTM3U") {
            probe.error = format!("线路返回 HTTP {status}");
            continue;
        }
        if !resolved.contains("m3u8-preview") {
            return LineProbe {
                text: resolved,
                is_preview: false,
                error: String::new(),
            };
        }
        if probe.text.is_empty() {
            probe.text = resolved;
        }
        probe.is_preview = true;
        probe.error = "当前会话仅返回试看片段".into();
    }
    probe
}

fn playlist_play_result(
    data: &Value,
    movie_id: &str,
    probe: LineProbe,
    account_label: &str,
) -> Result<TangxinPlayResult, AppError> {
    let path = std::env::temp_dir().join(format!("ttv-tangxin-{}.m3u8", uuid::Uuid::new_v4()));
    std::fs::write(&path, probe.text.as_bytes())
        .map_err(|error| AppError::Provider(format!("写入播放清单失败：{error}")))?;
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_owned(), API_UA.to_owned());
    headers.insert("Referer".to_owned(), format!("{}/", api_base()));
    Ok(TangxinPlayResult {
        playlist_path: path.to_string_lossy().into_owned(),
        title: detail_title(data, movie_id),
        headers,
        is_preview: probe.is_preview,
        playlist: probe.text,
        account_label: account_label.to_owned(),
        needs_purchase: false,
        purchase_price: 0,
        purchase_account_label: String::new(),
    })
}

/// Validate one add-account submission and build the stored record.
fn build_account(
    input: TangxinAccountInput,
    existing: &[TangxinAccount],
) -> Result<TangxinAccount, AppError> {
    let username = input.username.trim().to_owned();
    let password = input.password.trim().to_owned();
    let device_id = input.device_id.trim().to_owned();
    let user_token = input.user_token.trim().to_owned();
    let qrcode = input.qrcode.trim().to_owned();
    if username.len() > 64
        || password.len() > 128
        || device_id.len() > 128
        || user_token.len() > 4096
        || qrcode.len() > 256
    {
        return Err(AppError::InvalidInput("账号字段长度超出限制".into()));
    }
    let mode = if !user_token.is_empty() {
        if device_id.is_empty() {
            return Err(AppError::InvalidInput("token 凭证需要同时填写 deviceId".into()));
        }
        "token"
    } else if !username.is_empty() || !password.is_empty() {
        if username.is_empty() || password.is_empty() {
            return Err(AppError::InvalidInput("账号密码登录需要同时填写用户名和密码".into()));
        }
        "password"
    } else if !qrcode.is_empty() {
        "qrcode"
    } else {
        return Err(AppError::InvalidInput(
            "至少填写一种凭据：账号密码、token/deviceId 或二维码凭证".into(),
        ));
    };
    let duplicate = existing.iter().any(|account| match mode {
        "token" => account.user_token == user_token && account.device_id == device_id,
        "password" => account.username == username && account.password == password,
        _ => account.qrcode == qrcode && !qrcode.is_empty(),
    });
    if duplicate {
        return Err(AppError::InvalidInput("该账号已存在于账号池".into()));
    }
    let label = if input.label.trim().is_empty() {
        if mode == "password" { username.clone() } else { String::new() }
    } else {
        input.label.trim().chars().take(40).collect()
    };
    Ok(TangxinAccount {
        id: uuid::Uuid::new_v4().simple().to_string(),
        label,
        username,
        password,
        device_id,
        user_token,
        qrcode,
        nickname: String::new(),
        coin: String::new(),
        is_vip: false,
        vip_end_time: String::new(),
        is_dark_vip: false,
        dark_vip_end_time: String::new(),
        available: false,
        unavailable_reason: String::new(),
        checked_at: 0,
        source: String::new(),
        cloud_readonly: false,
        remote_id: String::new(),
    })
}

/* ---------------- public helpers for commands ---------------- */

pub async fn fetch_discover(data_dir: &Path) -> Result<Vec<TangxinSection>, AppError> {
    let client = CurlFetch::new()?;
    let response = sessioned_call(
        &client,
        data_dir,
        "/movie/block",
        json!({"position": "app_home_tj"}),
    )
    .await?;
    let blocks = unwrap_array(&response["data"], &["blocks", "items", "list"]);
    let mut sections = Vec::new();
    for raw in &blocks {
        let Some(section) = normalize_section(raw) else {
            continue;
        };
        if sections
            .iter()
            .any(|existing: &TangxinSection| existing.id == section.id)
        {
            continue;
        }
        sections.push(section);
        if sections.len() >= MAX_SECTIONS {
            break;
        }
    }
    Ok(sections)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TangxinSearchRequest {
    #[serde(default)]
    pub keywords: String,
    #[serde(default)]
    pub order: String,
    #[serde(default)]
    pub pay_type: String,
    #[serde(default)]
    pub canvas: String,
    #[serde(default)]
    pub tag_id: String,
    #[serde(default)]
    pub cat_id: String,
    #[serde(default)]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
}

fn default_page_size() -> u32 {
    DEFAULT_PAGE_SIZE
}

/// Whitelisted `/movie/search` body (extension `buildSearchParams`): unknown
/// fields, tokens and playback addresses never reach the upstream.
fn build_search_params(request: &TangxinSearchRequest) -> Value {
    let mut params = json!({});
    let keywords = safe_text(&Value::String(request.keywords.clone()), 80);
    if !keywords.is_empty() {
        params["keywords"] = Value::String(keywords);
    }
    let order = request.order.to_lowercase();
    if ["new", "hot", "click", "love", "favorite", "score"].contains(&order.as_str()) {
        params["order"] = Value::String(order);
    }
    let mut pay_type = request.pay_type.to_lowercase();
    if pay_type == "coin" {
        pay_type = "money".into();
    }
    if ["free", "vip", "money"].contains(&pay_type.as_str()) {
        params["pay_type"] = Value::String(pay_type);
    }
    let mut canvas = request.canvas.to_lowercase();
    if canvas == "portrait" {
        canvas = "long".into();
    }
    if canvas == "landscape" {
        canvas = "short".into();
    }
    if ["long", "short", "square"].contains(&canvas.as_str()) {
        params["canvas"] = Value::String(canvas);
    }
    let tag_id = request.tag_id.trim();
    if !tag_id.is_empty() && tag_id.chars().all(|ch| ch.is_ascii_digit()) {
        params["tag_id"] = Value::String(tag_id.to_owned());
    }
    let cat_id = request.cat_id.trim();
    if !cat_id.is_empty() && cat_id.chars().all(|ch| ch.is_ascii_digit()) {
        params["cat_id"] = Value::String(cat_id.to_owned());
    }
    params["page"] = Value::from(request.page.max(1));
    params["page_size"] = Value::from(request.page_size.clamp(1, MAX_PAGE_SIZE));
    params
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TangxinSearchResult {
    pub items: Vec<TangxinMovie>,
    pub page: u32,
    pub page_size: u32,
    pub has_more: bool,
}

pub async fn search_catalog(
    data_dir: &Path,
    request: &TangxinSearchRequest,
) -> Result<TangxinSearchResult, AppError> {
    let client = CurlFetch::new()?;
    let params = build_search_params(request);
    let response = sessioned_call(&client, data_dir, "/movie/search", params).await?;
    let data = &response["data"];
    let raw_items = unwrap_array(data, &["items", "list", "rows"]);
    let mut items = Vec::new();
    append_unique_movies(&mut items, &raw_items, MAX_ITEMS);
    let total = data["total"]
        .as_u64()
        .or_else(|| data["total"].as_str().and_then(|text| text.parse().ok()))
        .or_else(|| {
            data["count"]
                .as_u64()
                .or_else(|| data["count"].as_str().and_then(|text| text.parse().ok()))
        });
    let page = request.page.max(1);
    let page_size = request.page_size.clamp(1, MAX_PAGE_SIZE);
    let has_more = match total {
        Some(total) => (page as u64) * (page_size as u64) < total,
        None => items.len() as u32 >= page_size,
    };
    Ok(TangxinSearchResult {
        items,
        page,
        page_size,
        has_more,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TangxinDetail {
    pub movie: TangxinMovie,
    pub description: String,
    pub cat_name: String,
    pub tags: Vec<String>,
    pub groups: Vec<TangxinMovie>,
}

/// Whitelisted `/movie/detail`: rebuilds only safe fields, drops any play
/// address, and keeps the parent item inside the episode list.
pub async fn fetch_detail(data_dir: &Path, movie_id: &str) -> Result<TangxinDetail, AppError> {
    let client = CurlFetch::new()?;
    let response =
        sessioned_call(&client, data_dir, "/movie/detail", json!({"id": movie_id})).await?;
    let data = &response["data"];
    let movie = normalize_movie(data)
        .or_else(|| normalize_movie(&json!({"id": movie_id})))
        .ok_or_else(|| AppError::Provider("影片详情无法识别".into()))?;
    let tags = data["tags"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|tag| safe_text(tag, 40))
                .filter(|tag| !tag.is_empty())
                .take(24)
                .collect()
        })
        .unwrap_or_default();
    let groups_raw = data["groups"]
        .as_array()
        .or_else(|| data["list"].as_array())
        .or_else(|| data["items"].as_array())
        .cloned()
        .unwrap_or_default();
    let mut groups: Vec<TangxinMovie> = Vec::new();
    append_unique_movies(&mut groups, &groups_raw, MAX_COLLECTION_ITEMS);
    if !groups.iter().any(|item| item.id == movie.id) {
        if groups.len() >= MAX_COLLECTION_ITEMS {
            groups.truncate(MAX_COLLECTION_ITEMS - 1);
        }
        groups.push(movie.clone());
    }
    Ok(TangxinDetail {
        movie,
        description: safe_text(&data["description"], 600),
        cat_name: safe_text(&data["cat_name"], 60),
        tags,
        groups,
    })
}

/* ---------------- poster decryption ---------------- */

fn private_ipv4(hostname: &str) -> bool {
    let parts: Vec<&str> = hostname.split('.').collect();
    if parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        let octets: Vec<u32> = parts.iter().map(|p| p.parse().unwrap()).collect();
        return octets[0] == 10
            || octets[0] == 127
            || (octets[0] == 169 && octets[1] == 254)
            || (octets[0] == 172 && (16..=31).contains(&octets[1]))
            || (octets[0] == 192 && octets[1] == 168);
    }
    false
}

fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        return Some("image/jpeg");
    }
    if bytes.len() >= 8
        && bytes[0] == 0x89
        && bytes[1] == 0x50
        && bytes[2] == 0x4e
        && bytes[3] == 0x47
    {
        return Some("image/png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Validate, fetch and decrypt one `.bnc` poster into a `data:` URL.
/// Mirrors the extension guards: HTTPS only, no localhost/private hosts,
/// 6 MiB cap, PKCS7 + image magic check.
pub async fn decrypt_poster(poster_url: &str) -> Result<String, AppError> {
    let parsed = reqwest::Url::parse(poster_url)
        .map_err(|_| AppError::InvalidInput("海报地址无效".into()))?;
    if parsed.scheme() != "https" {
        return Err(AppError::InvalidInput("海报必须是 HTTPS 地址".into()));
    }
    if !parsed.path().to_lowercase().ends_with(".bnc") {
        return Err(AppError::InvalidInput("海报不是加密 .bnc 资源".into()));
    }
    let hostname = parsed.host_str().unwrap_or_default().to_lowercase();
    if hostname.is_empty()
        || hostname == "localhost"
        || hostname.ends_with(".localhost")
        || hostname.ends_with(".local")
        || hostname.contains(':')
        || private_ipv4(&hostname)
    {
        return Err(AppError::InvalidInput("海报主机不允许访问".into()));
    }
    let extension = parsed
        .query_pairs()
        .find(|(key, _)| key == "ext")
        .map(|(_, value)| value.trim_start_matches('.').to_lowercase())
        .unwrap_or_else(|| "png".into());
    if !["jpg", "jpeg", "png", "gif", "webp"].contains(&extension.as_str()) {
        return Err(AppError::InvalidInput("海报图片类型不受支持".into()));
    }
    let client = CurlFetch::new()?;
    let (status, encrypted) = client
        .get(poster_url, false, &[("User-Agent".into(), API_UA.into())])
        .await?;
    if status >= 400 || status == 0 {
        return Err(AppError::Provider(format!("海报下载失败（HTTP {status}）")));
    }
    if encrypted.is_empty() || encrypted.len() > MAX_POSTER_BYTES {
        return Err(AppError::Provider("海报大小或分块格式无效".into()));
    }
    let plain = decrypt_ecb(POSTER_AES_KEY, &encrypted)?;
    // Trust the sniffed magic; the ext query hint only gates the whitelist.
    let sniffed = sniff_image_mime(&plain)
        .ok_or_else(|| AppError::Provider("海报解密成功但图片格式不受支持".into()))?;
    Ok(format!(
        "data:{sniffed};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&plain)
    ))
}

/* ---------------- playback ---------------- */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TangxinPlayResult {
    pub playlist_path: String,
    pub title: String,
    pub headers: HashMap<String, String>,
    pub is_preview: bool,
    /// Full m3u8 text. The browser (hls.js) path loads this as a blob URL;
    /// the native mpv path uses `playlist_path` instead.
    pub playlist: String,
    /// Account that served this playback (empty = visitor session).
    pub account_label: String,
    /// True when every pool account is blocked by coin-only content; the
    /// frontend must confirm with the user before retrying with `allow_buy`.
    pub needs_purchase: bool,
    pub purchase_price: u64,
    pub purchase_account_label: String,
}

/// Guest/env-auth flow (no accounts configured): the site decides whether the
/// visitor session receives a full stream or a preview.
async fn resolve_playback_visitor(
    client: &CurlFetch,
    data_dir: &Path,
    movie_id: &str,
) -> Result<TangxinPlayResult, AppError> {
    let response = sessioned_call(client, data_dir, "/movie/detail", json!({"id": movie_id})).await?;
    let data = response["data"].clone();
    let candidates = collect_line_candidates(&data);
    if candidates.is_empty() {
        let pay_type = safe_text(&data["pay_type"], 32);
        let money = safe_text(&data["money"], 32);
        return Err(AppError::Provider(format!(
            "该影片没有可播放线路（{}{}）",
            pay_type,
            if money.is_empty() {
                String::new()
            } else {
                format!(" / 金币 {money}")
            }
        )));
    }
    let session = ensure_session(client, data_dir).await?;
    let probe = probe_play_lines(client, &session, movie_id, &candidates).await;
    if probe.text.is_empty() {
        return Err(AppError::Provider(probe.error));
    }
    if configured_auth_session()?.is_some() && probe.is_preview {
        return Err(AppError::Provider(
            "授权会话只返回试看清单，站点未授予完整播放权益".into(),
        ));
    }
    playlist_play_result(&data, movie_id, probe, "")
}

/// Resolve one movie into a local m3u8 playlist. Called only from an explicit
/// user play gesture. With an account pool, accounts are tried in automatic
/// rotation (selected first, then last-verified-available, then cheapest-coin;
/// cloud-pool rows re-issue their session through the worker) until one
/// returns a full playlist; coin-locked content is bought (`/movie/doBuy`)
/// only when the frontend passes `allow_buy` after an explicit user
/// confirmation.
pub async fn resolve_playback(
    data_dir: &Path,
    movie_id: &str,
    allow_buy: bool,
) -> Result<TangxinPlayResult, AppError> {
    let client = CurlFetch::new()?;
    let mut store = load_account_store(data_dir);
    let remote = store.remote.clone();
    let has_local = store.accounts.iter().any(|account| !account.is_cloud());
    if !has_local && !remote.enabled {
        return resolve_playback_visitor(&client, data_dir, movie_id).await;
    }

    // 云端轮班 / 云端优先：Worker 自己选号、扣费并返回可直接探测的线路，
    // 本地只在云端失败且允许回退时接手。
    let mut cloud_down_reason: Option<String> = None;
    if remote.enabled && remote.account_source_mode != "local" {
        match cloud_playback(&client, data_dir, movie_id, &remote).await {
            Ok(result) => return Ok(result),
            Err(error) => {
                let outage = is_cloud_pool_outage(&error);
                if outage {
                    cloud_down_reason = Some(error.to_string());
                }
                if remote.account_source_mode == "cloud" && !remote.fallback_local {
                    return Err(AppError::Provider(format!("云端账号池播放失败：{error}")));
                }
                // 云端池整体故障且本地没有账号时，逐号重试只会打到同一个坏掉的
                // worker（/v1/accounts/verify 同样返回 500），直接退回访客会话：
                // 免费内容完整播放，VIP/金币内容退化为试看。
                if outage && !has_local {
                    let reason = cloud_down_reason
                        .clone()
                        .unwrap_or_else(|| "云端账号池不可用".to_string());
                    return match resolve_playback_visitor(&client, data_dir, movie_id).await {
                        Ok(result) => Ok(result),
                        Err(visitor_error) => Err(AppError::Provider(format!(
                            "云端账号池当前不可用（{reason}），且访客播放也失败：{visitor_error}"
                        ))),
                    };
                }
                tracing::warn!("tangxin: cloud pool playback failed, falling back local: {error}");
            }
        }
    }

    // 自动选号：云端池账号同样参与本地轮换（worker /verify 逐号重签会话），
    // 云端播放端点失败时不再退化成访客试看。云端池被禁用时其账号取不到
    // 会话，继续跳过。
    let mut ordered: Vec<TangxinAccount> = store
        .accounts
        .iter()
        .filter(|account| !account.is_cloud() || remote.enabled)
        .cloned()
        .collect();
    if ordered.is_empty() {
        return resolve_playback_visitor(&client, data_dir, movie_id).await;
    }
    // 轮换顺序：优先使用 > 最近验证可用 > 金币余额低者在前；全程无需手动选号。
    ordered.sort_by_key(|account| {
        (
            account.id != store.selected_id,
            !account.available,
            account.coin_value(),
        )
    });
    let mut store_dirty = false;

    let mut errors: Vec<String> = Vec::new();
    let mut fallback_preview: Option<(String, String)> = None;
    let mut locked: Vec<(TangxinAccount, TangxinSession, u64, String)> = Vec::new();

    for candidate in &ordered {
        let mut account = candidate.clone();
        let before = account.clone();
        let label = account.display_label();
        let detail_result = if account.is_cloud() {
            cloud_account_detail_with_retry(&client, &remote, &mut account, movie_id).await
        } else {
            account_detail_with_retry(&client, data_dir, &mut account, movie_id).await
        };
        let (session, response) = match detail_result {
            Ok(result) => result,
            Err(error) => {
                mark_account_unavailable(&mut account, &error.to_string());
                if account.is_cloud() && is_cloud_pool_outage(&error) {
                    cloud_down_reason = Some(error.to_string());
                }
                errors.push(format!("账号 {label}：{error}"));
                if account != before {
                    replace_account(&mut store, account.clone());
                    store_dirty = true;
                }
                continue;
            }
        };
        if account != before {
            replace_account(&mut store, account.clone());
            store_dirty = true;
        }
        let data = response["data"].clone();

        if let Some(price) = detail_locked_coin_price(&data) {
            errors.push(format!("账号 {label}：金币内容未解锁（{price} 金币）"));
            locked.push((account, session, price, detail_title(&data, movie_id)));
            continue;
        }
        let candidates = collect_line_candidates(&data);
        if candidates.is_empty() {
            errors.push(format!("账号 {label}：没有可播放线路"));
            continue;
        }
        let probe = probe_play_lines(&client, &session, movie_id, &candidates).await;
        if probe.text.is_empty() {
            errors.push(format!("账号 {label}：{}", probe.error));
            continue;
        }
        if probe.is_preview {
            errors.push(format!("账号 {label}：{}", probe.error));
            if fallback_preview.is_none() {
                fallback_preview = Some((probe.text, detail_title(&data, movie_id)));
            }
            continue;
        }
        if store_dirty {
            save_account_store(data_dir, &store);
        }
        return playlist_play_result(&data, movie_id, probe, &label);
    }

    // Coin unlock: never charged without the explicit allow_buy confirmation.
    if !locked.is_empty() {
        locked.sort_by_key(|entry| entry.0.coin_value());
        let (_, _, price, title) = &locked[0];
        if !allow_buy {
            let purchase_account = locked[0].0.display_label();
            if store_dirty {
                save_account_store(data_dir, &store);
            }
            return Ok(TangxinPlayResult {
                playlist_path: String::new(),
                title: title.clone(),
                headers: HashMap::new(),
                is_preview: true,
                playlist: String::new(),
                account_label: String::new(),
                needs_purchase: true,
                purchase_price: *price,
                purchase_account_label: purchase_account,
            });
        }
        // 扣费结果不确定时立即终止，绝不换号重试，防止重复扣费。
        let (mut account, session, _, _title) = locked.remove(0);
        let before = account.clone();
        let label = account.display_label();
        api_call(&client, &session, "/movie/doBuy", json!({"id": movie_id}))
            .await
            .map_err(|error| {
                if account != before {
                    replace_account(&mut store, account.clone());
                    save_account_store(data_dir, &store);
                }
                AppError::Provider(format!("账号 {label} 解锁扣费失败：{error}"))
            })?;
        let response = api_call(&client, &session, "/movie/detail", json!({"id": movie_id}))
            .await
            .map_err(|error| {
                AppError::Provider(format!("账号 {label} 扣费后获取详情失败：{error}"))
            })?;
        let data = response["data"].clone();
        if detail_locked_coin_price(&data).is_some() {
            return Err(AppError::Provider(format!(
                "账号 {label} 扣费后仍未解锁，请核对站点权益与余额"
            )));
        }
        let candidates = collect_line_candidates(&data);
        let probe = probe_play_lines(&client, &session, movie_id, &candidates).await;
        if probe.text.is_empty() || probe.is_preview {
            let reason = if probe.text.is_empty() {
                probe.error
            } else {
                "扣费后仍只返回试看清单".to_string()
            };
            return Err(AppError::Provider(format!(
                "账号 {label} 扣费后仍无法播放：{reason}"
            )));
        }
        let _ = refresh_account_status(&client, &session, &mut account).await;
        if account != before {
            replace_account(&mut store, account.clone());
            store_dirty = true;
        }
        if store_dirty {
            save_account_store(data_dir, &store);
        }
        return playlist_play_result(&data, movie_id, probe, &label);
    }

    if store_dirty {
        save_account_store(data_dir, &store);
    }
    if let Some((text, title)) = fallback_preview {
        // 所有账号都只拿到试看权益：退回试看流而不是直接失败。
        let path = std::env::temp_dir().join(format!("ttv-tangxin-{}.m3u8", uuid::Uuid::new_v4()));
        std::fs::write(&path, text.as_bytes())
            .map_err(|error| AppError::Provider(format!("写入播放清单失败：{error}")))?;
        let mut headers = HashMap::new();
        headers.insert("User-Agent".to_owned(), API_UA.to_owned());
        headers.insert("Referer".to_owned(), format!("{}/", api_base()));
        return Ok(TangxinPlayResult {
            playlist_path: path.to_string_lossy().into_owned(),
            title,
            headers,
            is_preview: true,
            playlist: text,
            account_label: String::new(),
            needs_purchase: false,
            purchase_price: 0,
            purchase_account_label: String::new(),
        });
    }
    // 云端池整体不可用且本地没有账号时，退回访客会话：免费内容仍可完整播放，
    // VIP/金币内容退化为试看，比直接报错更可用。访客也失败时给出明确提示。
    if !has_local {
        if let Some(reason) = cloud_down_reason {
            match resolve_playback_visitor(&client, data_dir, movie_id).await {
                Ok(result) => return Ok(result),
                Err(visitor_error) => {
                    return Err(AppError::Provider(format!(
                        "云端账号池当前不可用（{reason}），且访客播放也失败：{visitor_error}"
                    )));
                }
            }
        }
    }
    Err(AppError::Provider(if errors.is_empty() {
        "账号池中没有可用账号".into()
    } else {
        format!("未能取得完整播放线路：{}", errors.join("；"))
    }))
}

/// Add one account, verify it immediately (best-effort: a failed verify keeps
/// the record with its failure reason instead of rolling back the save).
pub async fn account_add(
    data_dir: &Path,
    input: TangxinAccountInput,
) -> Result<TangxinAccountView, AppError> {
    let mut store = load_account_store(data_dir);
    let mut account = build_account(input, &store.accounts)?;
    store.accounts.push(account.clone());
    let client = CurlFetch::new()?;
    let before = account.clone();
    if let Err(error) = acquire_account_session(&client, data_dir, &mut account).await {
        mark_account_unavailable(&mut account, &error.to_string());
    }
    let verified_available = account.available;
    if account != before {
        replace_account(&mut store, account.clone());
    }
    save_account_store(data_dir, &store);
    if !verified_available {
        return Err(AppError::Provider(format!(
            "账号已保存，但验证失败：{}",
            account.unavailable_reason
        )));
    }
    Ok(account_to_view(&account, store.selected_id == account.id))
}

pub async fn account_list(data_dir: &Path) -> Result<Vec<TangxinAccountView>, AppError> {
    Ok(account_views(&load_account_store(data_dir)))
}

pub async fn account_remove(data_dir: &Path, id: &str) -> Result<(), AppError> {
    let mut store = load_account_store(data_dir);
    if store
        .accounts
        .iter()
        .any(|account| account.id == id && account.is_cloud())
    {
        // 删了也会在下一次同步时被云端列表拉回来，直接给出明确提示。
        return Err(AppError::InvalidInput(
            "云端账号由云端池管理，本地无法删除；如不想使用请切换账号来源为「本地值班」".into(),
        ));
    }
    let before = store.accounts.len();
    store.accounts.retain(|account| account.id != id);
    if store.accounts.len() == before {
        return Err(AppError::NotFound(format!("未找到账号：{id}")));
    }
    if store.selected_id == id {
        store.selected_id.clear();
    }
    save_account_store(data_dir, &store);
    Ok(())
}

/// Pin one account as the preferred rotation candidate; empty id clears.
pub async fn account_select(data_dir: &Path, id: &str) -> Result<(), AppError> {
    let mut store = load_account_store(data_dir);
    if !id.is_empty() {
        let account = store
            .accounts
            .iter()
            .find(|account| account.id == id)
            .ok_or_else(|| AppError::NotFound(format!("未找到账号：{id}")))?;
        if account.is_cloud() {
            return Err(AppError::InvalidInput(
                "云端账号由云端池自动轮换，不支持本地固定选择".into(),
            ));
        }
    }
    store.selected_id = id.to_owned();
    save_account_store(data_dir, &store);
    Ok(())
}

/// Re-verify one account now: refresh its token (re-login when stale) and
/// cached identity/VIP/coin status.
pub async fn account_verify(data_dir: &Path, id: &str) -> Result<TangxinAccountView, AppError> {
    let mut store = load_account_store(data_dir);
    let index = store
        .accounts
        .iter()
        .position(|account| account.id == id)
        .ok_or_else(|| AppError::NotFound(format!("未找到账号：{id}")))?;
    let mut account = store.accounts[index].clone();
    let before = account.clone();
    let client = CurlFetch::new()?;
    let result = if account.is_cloud() {
        verify_cloud_account(&client, &store.remote.clone().normalized(), &mut account).await
    } else {
        acquire_account_session(&client, data_dir, &mut account).await
    };
    if let Err(error) = &result {
        mark_account_unavailable(&mut account, &error.to_string());
    }
    if account != before {
        replace_account(&mut store, account.clone());
    }
    save_account_store(data_dir, &store);
    let selected = store.selected_id == account.id;
    result.map(|_| account_to_view(&account, selected))
}

/* ---------------- cloud account pool (tangxin-zhizhe-extension worker) ---------------- */

fn remote_headers() -> Vec<(String, String)> {
    vec![
        ("Content-Type".into(), "application/json".into()),
        ("Accept".into(), "application/json".into()),
        ("User-Agent".into(), API_UA.into()),
    ]
}

/// One JSON call against the shared worker pool. Bearer auth mirrors the
/// extension; `ok:false` payloads surface their `error` text.
async fn remote_request_json(
    client: &CurlFetch,
    remote: &TangxinRemoteConfig,
    method: &str,
    endpoint: &str,
    body: Option<Value>,
) -> Result<Value, AppError> {
    let url = format!("{}{endpoint}", remote.base_url);
    let mut headers = remote_headers();
    headers.push(("Authorization".into(), format!("Bearer {REMOTE_ACCESS_TOKEN}")));
    let (status, response) = if method == "GET" {
        client.get(&url, false, &headers).await?
    } else {
        let text = serde_json::to_vec(&body.unwrap_or(Value::Null))
            .map_err(|error| AppError::Provider(format!("云端请求序列化失败：{error}")))?;
        client.post(&url, &headers, &text).await?
    };
    let text = String::from_utf8_lossy(&response);
    let parsed: Value = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(text.trim())
            .map_err(|error| AppError::Provider(format!("云端接口 {endpoint} 返回非 JSON：{error}")))?
    };
    if status >= 400 || parsed["ok"] == Value::Bool(false) {
        let message = value_text(&parsed, &["error", "message"], 240);
        return Err(AppError::Provider(if message.is_empty() {
            format!("云端接口 {endpoint} 请求失败：HTTP {status}")
        } else {
            message
        }));
    }
    Ok(parsed)
}

/// Map one worker row into a cloud-marked local record. The worker may or may
/// not hand out raw credentials; sessions are always re-issued by /verify, so
/// stored credentials on cloud rows are best-effort only.
fn cloud_row_to_account(raw: &Value) -> Option<TangxinAccount> {
    let id = value_text(raw, &["id"], 120);
    if id.is_empty() {
        return None;
    }
    let info = if raw["userInfo"].is_object() {
        raw["userInfo"].clone()
    } else {
        raw.clone()
    };
    let username = value_text(raw, &["username", "account_name"], 64);
    Some(TangxinAccount {
        id: format!("cloud-{id}"),
        label: value_text(raw, &["label", "username", "id"], 40),
        username,
        password: String::new(),
        device_id: value_text(raw, &["deviceId"], 240),
        user_token: value_text(raw, &["userToken", "token"], 4096),
        qrcode: String::new(),
        nickname: value_text(&info, &["nickname", "account_name", "username"], 60),
        coin: value_text(&info, &["coin", "gold", "balance", "money"], 32),
        is_vip: value_truthy(&info["is_vip"]) || value_truthy(&info["vip"]) || value_truthy(&info["has_vip"]),
        vip_end_time: value_text(&info, &["vip_end_time"], 40),
        is_dark_vip: value_truthy(&info["is_dark_vip"])
            || value_truthy(&info["dark_vip"])
            || value_truthy(&info["has_dark_vip"]),
        dark_vip_end_time: value_text(&info, &["dark_vip_end_time"], 40),
        available: true,
        unavailable_reason: String::new(),
        checked_at: now_unix_seconds(),
        source: "cloud".to_owned(),
        cloud_readonly: true,
        remote_id: id,
    })
}

impl TangxinAccount {
    fn pool_id(&self) -> &str {
        if self.remote_id.is_empty() {
            &self.id
        } else {
            &self.remote_id
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TangxinPoolSnapshot {
    pub config: TangxinRemoteConfig,
    pub accounts: Vec<TangxinAccountView>,
}

/// Pull the shared pool and merge it into the local store: user-entered rows
/// are kept as-is, cloud rows are replaced wholesale by the worker's list
/// (the worker is the source of truth for anything cloud-managed).
pub async fn account_sync_cloud(data_dir: &Path) -> Result<TangxinPoolSnapshot, AppError> {
    let mut store = load_account_store(data_dir);
    let remote = store.remote.clone();
    if !remote.enabled {
        return Err(AppError::InvalidInput("云端账号池未启用".into()));
    }
    let client = CurlFetch::new()?;
    let payload = match remote_request_json(&client, &remote, "GET", "/v1/accounts", None).await {
        Ok(payload) => payload,
        Err(error) => {
            store.remote.last_error = error.to_string();
            save_account_store(data_dir, &store);
            return Err(error);
        }
    };
    let rows = payload["accounts"].as_array().cloned().unwrap_or_default();
    let mut merged: Vec<TangxinAccount> = store
        .accounts
        .iter()
        .filter(|account| !account.is_cloud())
        .cloned()
        .collect();
    let mut cloud_count = 0usize;
    for row in &rows {
        if let Some(account) = cloud_row_to_account(row) {
            merged.push(account);
            cloud_count += 1;
        }
    }
    // Selection only drives local rotation; keep it when still valid.
    if !store.selected_id.is_empty()
        && !merged.iter().any(|account| account.id == store.selected_id && !account.is_cloud())
    {
        store.selected_id = merged
            .iter()
            .find(|account| !account.is_cloud() && account.available)
            .map(|account| account.id.clone())
            .unwrap_or_default();
    }
    store.accounts = merged;
    store.remote.last_sync_at = now_unix_seconds();
    store.remote.last_error = String::new();
    save_account_store(data_dir, &store);
    tracing::info!("tangxin: cloud pool synced {cloud_count} cloud accounts");
    Ok(TangxinPoolSnapshot {
        config: store.remote.clone(),
        accounts: account_views(&store),
    })
}

/// Push one user-entered account into the shared pool. Mirrors the extension:
/// the full credential set is uploaded, the local row then becomes a cloud
/// summary and the next sync reconciles it with the worker's copy.
pub async fn account_upload_cloud(data_dir: &Path, id: &str) -> Result<TangxinPoolSnapshot, AppError> {
    let mut store = load_account_store(data_dir);
    let remote = store.remote.clone();
    if !remote.enabled {
        return Err(AppError::InvalidInput("云端账号池未启用".into()));
    }
    let index = store
        .accounts
        .iter()
        .position(|account| account.id == id)
        .ok_or_else(|| AppError::NotFound(format!("未找到账号：{id}")))?;
    let account = &store.accounts[index];
    if account.is_cloud() {
        return Err(AppError::InvalidInput("该账号已是云端账号，不需要重复上传".into()));
    }
    if !account.has_credential() {
        return Err(AppError::InvalidInput(
            "该账号缺少可上传凭据（密码、token/deviceId 或二维码凭证）".into(),
        ));
    }
    let client = CurlFetch::new()?;
    let payload = json!({
        "account": {
            "id": account.id,
            "label": account.display_label(),
            "username": account.username,
            "password": account.password,
            "deviceId": account.device_id,
            "userToken": account.user_token,
            "qrcode": account.qrcode,
            "source": if account.qrcode.is_empty() { "remote" } else { "qrcode" },
        }
    });
    let response = remote_request_json(&client, &remote, "POST", "/v1/accounts/client-upload", Some(payload))
        .await
        .map_err(|error| {
            let mut store = load_account_store(data_dir);
            store.remote.last_error = error.to_string();
            save_account_store(data_dir, &store);
            error
        })?;
    let remote_id = value_text(&response["account"], &["id"], 120);
    if let Some(slot) = store.accounts.iter_mut().find(|item| item.id == id) {
        slot.cloud_readonly = true;
        slot.source = "cloud".to_owned();
        if !remote_id.is_empty() {
            slot.remote_id = remote_id;
        }
    }
    store.remote.last_error = String::new();
    save_account_store(data_dir, &store);
    account_sync_cloud(data_dir).await
}

/// Read the persisted cloud pool configuration.
pub fn remote_config_get(data_dir: &Path) -> TangxinRemoteConfig {
    load_account_store(data_dir).remote
}

/// Persist cloud pool configuration (base url / enabled / source mode).
pub fn remote_config_set(data_dir: &Path, config: TangxinRemoteConfig) -> Result<TangxinRemoteConfig, AppError> {
    let normalized = config.normalized();
    if ["cloud", "cloud-first"].contains(&normalized.account_source_mode.as_str())
        && !normalized.enabled
    {
        return Err(AppError::InvalidInput(
            "当前账号来源依赖云端池，请先把账号来源切到「本地值班」再关闭云端池".into(),
        ));
    }
    let mut store = load_account_store(data_dir);
    store.remote = normalized.clone();
    save_account_store(data_dir, &store);
    Ok(normalized)
}

/// Cloud-rotation playback: the worker picks an account (lowest coins first),
/// purchases coin content itself and returns ready-to-probe sources.
async fn cloud_playback(
    client: &CurlFetch,
    data_dir: &Path,
    movie_id: &str,
    remote: &TangxinRemoteConfig,
) -> Result<TangxinPlayResult, AppError> {
    let payload = remote_request_json(
        client,
        remote,
        "POST",
        "/v2/playback/session",
        Some(json!({
            "movieId": movie_id,
            "movieTitle": "",
            "requestId": uuid::Uuid::new_v4().to_string(),
            "forceRefresh": false,
            "bootstrapSession": Value::Null,
        })),
    )
    .await?;
    let detail = payload["detail"].as_object().is_some().then(|| payload["detail"].clone());
    let session_obj = &payload["session"];
    if session_obj["movieId"].is_null() || session_obj["sources"].as_array().is_none() {
        return Err(AppError::Provider("云端播放接口返回结构不完整".into()));
    }
    let mut candidates: Vec<String> = Vec::new();
    let mut push_candidate = |value: &Value| {
        let url = value.as_str().unwrap_or("").trim().to_owned();
        if url.starts_with("http") && !candidates.contains(&url) {
            candidates.push(url);
        }
    };
    if let Some(sources) = session_obj["sources"].as_array() {
        for source in sources {
            push_candidate(&source["url"]);
        }
    }
    if let Some(detail) = &detail {
        for key in PLAY_LINK_KEYS {
            push_candidate(&detail[key]);
        }
    }
    if candidates.is_empty() {
        return Err(AppError::Provider("云端播放接口未返回可播放线路".into()));
    }
    let session = ensure_session(client, data_dir).await?;
    let probe = probe_play_lines(client, &session, movie_id, &candidates).await;
    if probe.text.is_empty() {
        return Err(AppError::Provider(probe.error));
    }
    if probe.is_preview {
        return Err(AppError::Provider("云端账号仅返回试看片段".into()));
    }
    let label = value_text(&payload["account"], &["label", "username", "nickname", "id"], 60);
    let detail = detail.unwrap_or_else(|| json!({}));
    playlist_play_result(&detail, movie_id, probe, &if label.is_empty() {
        "云端池".to_owned()
    } else {
        format!("云端·{label}")
    })
}

/// Verify a cloud-managed account: the worker re-issues a session for it and
/// we refresh the cached identity/VIP/coin status through /user/info.
async fn verify_cloud_account(
    client: &CurlFetch,
    remote: &TangxinRemoteConfig,
    account: &mut TangxinAccount,
) -> Result<TangxinSession, AppError> {
    let payload = remote_request_json(
        client,
        remote,
        "POST",
        "/v1/accounts/verify",
        Some(json!({"accountId": account.pool_id().to_owned(), "bootstrapSession": Value::Null})),
    )
    .await?;
    let session = TangxinSession {
        device_id: value_text(&payload["session"], &["deviceId"], 240),
        user_token: value_text(&payload["session"], &["userToken", "token"], 4096),
    };
    if session.device_id.is_empty() || session.user_token.is_empty() {
        return Err(AppError::Provider("云端池未返回该账号的可用会话".into()));
    }
    refresh_account_status(client, &session, account).await?;
    Ok(session)
}

/* ---------------- unit tests ---------------- */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecb_roundtrip_matches_extension_scheme() {
        let plain = br#"{"data":{"page":1},"token":"abc_1"}"#;
        let cipher = encrypt_ecb(API_AES_KEY, plain);
        assert_eq!(cipher.len() % AES_BLOCK, 0);
        assert!(cipher.len() > plain.len());
        let decrypted = decrypt_ecb(API_AES_KEY, &cipher).unwrap();
        assert_eq!(decrypted, plain);
    }

    #[test]
    fn pkcs7_bad_padding_rejected() {
        let plain = b"hello world!!";
        let mut cipher = encrypt_ecb(API_AES_KEY, plain);
        // Corrupt the padding byte of the last block.
        *cipher.last_mut().unwrap() ^= 0xff;
        assert!(decrypt_ecb(API_AES_KEY, &cipher).is_err());
    }

    #[test]
    fn normalize_movie_uses_whitelist_and_drops_play_fields() {
        let raw = json!({
            "id": "36248",
            "name": "老师到家中巡视辅导",
            "img": "https://lmm.tbxgdc.cn/media2/x.bnc?ext=.jpg",
            "nickname": "米欧",
            "headico": "https://lmm.tbxgdc.cn/media/headico/38.bnc?ext=.jpg",
            "click": "76.9w",
            "love": "44.7w",
            "score": "10",
            "time": "2026-08-01",
            "icon": "new",
            "is_episode": "n",
            "play_link": "/h5/m3u8/link/secret.m3u8",
            "backup_link": "/h5/m3u8/link/backup.m3u8",
            "m3u8": "https://example.com/signed.m3u8"
        });
        let movie = normalize_movie(&raw).unwrap();
        assert_eq!(movie.id, "36248");
        assert_eq!(movie.title, "老师到家中巡视辅导");
        assert_eq!(movie.creator, "米欧");
        assert_eq!(movie.views, "76.9w");
        assert_eq!(movie.score, "10");
        assert_eq!(movie.access, "free");
        assert!(!movie.is_collection);
        // No field in the model can carry the signed URLs.
        let serialized = serde_json::to_string(&movie).unwrap();
        assert!(!serialized.contains("m3u8"));
        assert!(!serialized.contains("play_link"));
    }

    #[test]
    fn normalize_movie_requires_id_and_reads_pay_type() {
        assert!(normalize_movie(&json!({"name": "no id"})).is_none());
        let movie = normalize_movie(&json!({"id": "1", "pay_type": "vip"})).unwrap();
        assert_eq!(movie.access, "vip");
        let movie =
            normalize_movie(&json!({"id": "1", "pay_type": "money", "money": "30"})).unwrap();
        assert_eq!(movie.access, "coin");
        assert_eq!(movie.price, 30);
    }

    #[test]
    fn normalize_section_drops_ads_and_negative_style() {
        let ad = json!({"id": "", "name": "", "style": "-1", "ad": []});
        assert!(normalize_section(&ad).is_none());
        let empty = json!({"id": "9", "name": "空的", "style": 1, "items": []});
        assert!(normalize_section(&empty).is_none());
        let valid = json!({
            "id": "5",
            "name": "最新上架",
            "style": 2,
            "items": [{"id": "36248", "name": "A"}, {"id": "36248", "name": "A"}]
        });
        let section = normalize_section(&valid).unwrap();
        assert_eq!(section.id, "5");
        assert_eq!(section.items.len(), 1);
    }

    #[test]
    fn search_params_whitelist() {
        let request = TangxinSearchRequest {
            keywords: "糖心".into(),
            order: "HOT".into(),
            pay_type: "coin".into(),
            canvas: "portrait".into(),
            tag_id: "12".into(),
            cat_id: "x".into(),
            page: 0,
            page_size: 500,
        };
        let params = build_search_params(&request);
        assert_eq!(params["keywords"], "糖心");
        assert_eq!(params["order"], "hot");
        assert_eq!(params["pay_type"], "money");
        assert_eq!(params["canvas"], "long");
        assert_eq!(params["tag_id"], "12");
        assert!(params.get("cat_id").is_none());
        assert_eq!(params["page"], 1);
        assert_eq!(params["page_size"], 48);
        assert!(params.as_object().unwrap().len() <= 8);
    }

    #[test]
    fn duration_parsing_and_formatting() {
        assert_eq!(parse_duration_seconds(&json!("01:30")), 90);
        assert_eq!(parse_duration_seconds(&json!("1:02:03")), 3723);
        assert_eq!(parse_duration_seconds(&json!(125.4)), 125);
        assert_eq!(format_duration(3723), "1:02:03");
        assert_eq!(format_duration(90), "1:30");
    }

    #[test]
    fn authorized_token_accepts_raw_jwt_with_user_id_or_precomposed_token() {
        assert_eq!(
            compose_authorized_token("jwt.payload.signature", Some("42")),
            "jwt.payload.signature_42"
        );
        assert_eq!(
            compose_authorized_token("jwt.pay_load.signature", Some("42")),
            "jwt.pay_load.signature_42"
        );
        assert_eq!(
            compose_authorized_token("opaque_42", Some("99")),
            "opaque_42"
        );
        assert_eq!(
            compose_authorized_token("jwt.payload.signature", None),
            "jwt.payload.signature"
        );
    }

    #[test]
    fn locked_coin_requires_unpaid_money_layer_without_media() {
        // 金币锁定：无线路 + 未购 + money 层 + 正价格。
        assert_eq!(
            detail_locked_coin_price(&json!({
                "id": "9", "layer_type": "money", "money": 36, "has_buy": "n"
            })),
            Some(36)
        );
        // 已购或已返回媒体（VIP 直接放行）绝不判锁定，严禁误扣金币。
        assert_eq!(
            detail_locked_coin_price(&json!({
                "id": "9", "layer_type": "money", "money": 36, "has_buy": "y"
            })),
            None
        );
        assert_eq!(
            detail_locked_coin_price(&json!({
                "id": "9", "layer_type": "money", "money": 36,
                "play_link": "/h5/m3u8/link/x.m3u8"
            })),
            None
        );
        assert_eq!(
            detail_locked_coin_price(&json!({
                "id": "9", "layer_type": "vip", "money": 0
            })),
            None
        );
        // 字符串价格同样识别。
        assert_eq!(
            detail_locked_coin_price(&json!({
                "id": "9", "layer_type": "money", "money": "18"
            })),
            Some(18)
        );
        // lines[] 里带地址也算已返回媒体。
        assert_eq!(
            detail_locked_coin_price(&json!({
                "id": "9", "layer_type": "money", "money": 36,
                "lines": [{"name": "主线", "link": "/h5/m3u8/link/y.m3u8"}]
            })),
            None
        );
    }

    #[test]
    fn account_input_requires_one_complete_credential() {
        let base = TangxinAccountInput::default();
        assert!(build_account(base.clone(), &[]).is_err());
        // token 模式缺 deviceId。
        assert!(build_account(
            TangxinAccountInput { user_token: "tok_1".into(), ..base.clone() },
            &[]
        )
        .is_err());
        // 密码模式缺密码。
        assert!(build_account(
            TangxinAccountInput { username: "u".into(), ..base.clone() },
            &[]
        )
        .is_err());
        let token_account = build_account(
            TangxinAccountInput {
                label: "共享号".into(),
                device_id: "web_abc".into(),
                user_token: "tok_1".into(),
                ..base.clone()
            },
            &[],
        )
        .unwrap();
        assert_eq!(token_account.credential_mode(), "token");
        assert_eq!(token_account.label, "共享号");
        // 重复凭据拒绝。
        assert!(build_account(
            TangxinAccountInput {
                device_id: "web_abc".into(),
                user_token: "tok_1".into(),
                ..base.clone()
            },
            std::slice::from_ref(&token_account),
        )
        .is_err());
        // 密码模式构建成功且默认沿用用户名作昵称。
        let password_account = build_account(
            TangxinAccountInput { username: "candy".into(), password: "pw".into(), ..base },
            &[],
        )
        .unwrap();
        assert_eq!(password_account.credential_mode(), "password");
        assert_eq!(password_account.label, "candy");
    }

    #[test]
    fn account_rotation_orders_selected_first_then_cheapest_coin() {
        let make = |id: &str, coin: &str| TangxinAccount {
            id: id.to_owned(),
            coin: coin.to_owned(),
            username: format!("u{id}"),
            password: "p".into(),
            ..TangxinAccount::default()
        };
        let mut ordered = vec![make("a", ""), make("b", "120"), make("c", "30"), make("d", "900")];
        ordered.sort_by_key(|account| account.coin_value());
        assert_eq!(ordered.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["c", "b", "d", "a"]);
        // 选中账号置顶（即使金币更多）。
        if let Some(position) = ordered.iter().position(|account| account.id == "d") {
            let selected = ordered.remove(position);
            ordered.insert(0, selected);
        }
        assert_eq!(ordered[0].id, "d");
    }

    #[test]
    fn account_views_mask_secrets_and_flag_selection() {
        let account = TangxinAccount {
            id: "a1".into(),
            username: "candyuser".into(),
            password: "secret-password".into(),
            coin: "88".into(),
            is_vip: true,
            ..TangxinAccount::default()
        };
        let store = TangxinAccountStore {
            accounts: vec![account],
            selected_id: "a1".into(),
            ..TangxinAccountStore::default()
        };
        let views = account_views(&store);
        assert_eq!(views.len(), 1);
        let view = &views[0];
        assert!(view.selected);
        assert_eq!(view.credential_mode, "password");
        assert_eq!(view.credential_hint, "ca****er");
        assert_eq!(view.label, "candyuser");
        assert!(view.is_vip);
        // 视图从不携带明文密码/token。
        let json = serde_json::to_string(view).unwrap();
        assert!(!json.contains("secret-password"));
    }

    #[test]
    fn account_store_roundtrip_keeps_only_credentialed_rows() {
        let dir = std::env::temp_dir().join(format!("ttv-tangxin-acct-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TangxinAccountStore {
            accounts: vec![TangxinAccount {
                id: "a1".into(),
                username: "candy".into(),
                password: "pw".into(),
                ..TangxinAccount::default()
            }],
            selected_id: "a1".into(),
            ..TangxinAccountStore::default()
        };
        save_account_store(&dir, &store);
        let loaded = load_account_store(&dir);
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(loaded.accounts[0].id, "a1");
        assert_eq!(loaded.selected_id, "a1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_rows_map_from_worker_payload_and_survive_retention() {
        let row = json!({
            "id": "full-candy",
            "label": "糖心·糖糖",
            "username": "candy",
            "deviceId": "web_abcdef1234567",
            "userInfo": {"coin": "256", "is_vip": true, "vip_end_time": "2026-12-31"}
        });
        let account = cloud_row_to_account(&row).expect("cloud row maps");
        assert!(account.is_cloud());
        assert_eq!(account.id, "cloud-full-candy");
        assert_eq!(account.remote_id, "full-candy");
        assert_eq!(account.coin, "256");
        assert!(account.is_vip);
        assert_eq!(account.label, "糖心·糖糖");
        // 云端行可以没有本地凭据，加载时不能被 retention 丢弃。
        let dir = std::env::temp_dir().join(format!("ttv-tangxin-cloud-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TangxinAccountStore {
            accounts: vec![account],
            ..TangxinAccountStore::default()
        };
        save_account_store(&dir, &store);
        let loaded = load_account_store(&dir);
        assert_eq!(loaded.accounts.len(), 1);
        assert!(loaded.accounts[0].is_cloud());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_rows_without_id_are_dropped() {
        assert!(cloud_row_to_account(&json!({"label": "no id"})).is_none());
    }

    #[test]
    fn remote_config_normalizes_mode_and_base_url() {
        let config = TangxinRemoteConfig {
            base_url: "https://example.workers.dev/".into(),
            account_source_mode: "bogus".into(),
            ..TangxinRemoteConfig::default()
        }
        .normalized();
        assert_eq!(config.base_url, "https://example.workers.dev");
        assert_eq!(config.account_source_mode, "cloud");
        assert_eq!(TangxinRemoteConfig::default().base_url, REMOTE_BASE_URL_DEFAULT);
    }

    #[test]
    fn remote_config_persists_across_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ttv-tangxin-remote-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TangxinAccountStore {
            remote: TangxinRemoteConfig {
                account_source_mode: "cloud-first".into(),
                last_sync_at: 12345,
                ..TangxinRemoteConfig::default()
            },
            ..TangxinAccountStore::default()
        };
        save_account_store(&dir, &store);
        let loaded = load_account_store(&dir);
        assert_eq!(loaded.remote.account_source_mode, "cloud-first");
        assert_eq!(loaded.remote.last_sync_at, 12345);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live end-to-end against the real site. Run explicitly:
    /// `cargo test --lib tangxin_e2e -- --ignored --nocapture`
    /// Needs a reachable network; the guest token is staged as a session file
    /// because issuance is IP-rate-limited.
    #[tokio::test]
    #[ignore]
    async fn tangxin_e2e() {
        let data_dir =
            std::env::temp_dir().join(format!("ttv-tangxin-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(
            session_path(&data_dir),
            r#"{"deviceId":"web_full_1788189224z","userToken":"c00b3ea9bae43a8e4912a38eabe30595_128980690"}"#,
        )
        .unwrap();
        let sections = fetch_discover(&data_dir).await.unwrap();
        assert!(!sections.is_empty(), "discover returned no sections");
        assert!(sections.iter().all(|s| !s.items.is_empty()));
        let search = search_catalog(
            &data_dir,
            &TangxinSearchRequest {
                keywords: String::new(),
                order: "new".into(),
                pay_type: String::new(),
                canvas: String::new(),
                tag_id: String::new(),
                cat_id: String::new(),
                page: 1,
                page_size: 12,
            },
        )
        .await
        .unwrap();
        println!("search items: {}", search.items.len());
        assert!(!search.items.is_empty());
        let movie = &search.items[0];
        assert!(!movie.poster_url.is_empty());
        let poster = decrypt_poster(&movie.poster_url).await.unwrap();
        assert!(poster.starts_with("data:image/"));
        println!("poster data url: {} bytes", poster.len());
        let detail = fetch_detail(&data_dir, &movie.id).await.unwrap();
        assert_eq!(detail.movie.id, movie.id);
        let play = resolve_playback(&data_dir, &movie.id, false).await;
        match play {
            Ok(play) => {
                let playlist = std::fs::read_to_string(&play.playlist_path).unwrap();
                assert!(playlist.starts_with("#EXTM3U"));
                println!("playlist ok, preview={}", play.is_preview);
            }
            Err(error) => println!("play not available for this item: {error}"),
        }
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn poster_host_guards() {
        assert!(private_ipv4("192.168.1.5"));
        assert!(private_ipv4("10.0.0.2"));
        assert!(!private_ipv4("lmm.tbxgdc.cn"));
        assert_eq!(
            sniff_image_mime(&[0xff, 0xd8, 0xff, 0xe0]),
            Some("image/jpeg")
        );
    }
}
