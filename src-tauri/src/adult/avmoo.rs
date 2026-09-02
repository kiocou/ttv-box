//! Avmoo scraper (structured-JSON fallback source).
//!
//! Ported from JavBoss `internal/jav/avmoo.go`. Avmoo exposes a JSON API that
//! requires a CSRF token plus session cookie obtained from a normal search
//! page. The session is cached for 30 minutes and refreshed automatically when
//! the API answers with an auth-style status (400/401/403/419).

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

use super::{JavMatch, RateLimiter, CHROME_UA};
use crate::error::AppError;

const BASE_URL: &str = "https://avmoo.shop";
const API_LANGUAGE: &str = "cn";
const API_SEARCH_LIMIT: u32 = 30;
const API_TRIES: u32 = 3;
const API_RETRY_DELAY: Duration = Duration::from_secs(2);
const SESSION_TTL: Duration = Duration::from_secs(30 * 60);

static RATE: RateLimiter = RateLimiter::new(Duration::from_millis(1500));

#[derive(Debug, Clone)]
struct Session {
    csrf_token: String,
    cookie: String,
    referer: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiEnvelope {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiMovie {
    #[serde(default, rename = "movieId")]
    movie_id: String,
    #[serde(default, rename = "movieFanHao")]
    movie_fan_hao: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "title_ja")]
    title_ja: String,
    #[serde(default, rename = "title_en")]
    title_en: String,
    #[serde(default, rename = "title_cn")]
    title_cn: String,
    #[serde(default, rename = "title_tw")]
    title_tw: String,
    #[serde(default, rename = "releaseDate")]
    release_date: String,
    #[serde(default)]
    length: u32,
    #[serde(default, rename = "posterSmall")]
    poster_small: String,
    #[serde(default, rename = "posterLarge")]
    poster_large: String,
    #[serde(default)]
    studio: Option<ApiStudio>,
    #[serde(default)]
    series: Option<ApiSeries>,
    #[serde(default)]
    genre: Vec<ApiGenre>,
    #[serde(default)]
    star: Vec<ApiStar>,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiStudio {
    #[serde(default, rename = "studioName")]
    studio_name: String,
    #[serde(default, rename = "studioName_cn")]
    studio_name_cn: String,
    #[serde(default, rename = "studioName_tw")]
    studio_name_tw: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiSeries {
    #[serde(default, rename = "seriesName")]
    series_name: String,
    #[serde(default, rename = "seriesName_cn")]
    series_name_cn: String,
    #[serde(default, rename = "seriesName_tw")]
    series_name_tw: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiGenre {
    #[serde(default, rename = "genreName")]
    genre_name: String,
    #[serde(default, rename = "genreName_cn")]
    genre_name_cn: String,
    #[serde(default, rename = "genreName_tw")]
    genre_name_tw: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiStar {
    #[serde(default, rename = "starName")]
    star_name: String,
    #[serde(default, rename = "starName_cn")]
    star_name_cn: String,
    #[serde(default, rename = "starName_tw")]
    star_name_tw: String,
}

/// Look up a code on Avmoo. Returns `Ok(None)` for a confirmed not-found,
/// `Err` on transient failures so the caller can treat the round as best-effort.
pub async fn lookup(client: &reqwest::Client, code: &str) -> Result<Option<JavMatch>, AppError> {
    let code = code.trim();
    if code.is_empty() {
        return Ok(None);
    }
    let movie = match fetch_movie_by_code(client, code).await {
        Ok(movie) => movie,
        // Confirmed not-found from the API; transient errors propagate.
        Err(AppError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(movie) = movie else {
        return Ok(None);
    };
    let mut info = movie_into_match(&movie);
    if info.code.is_empty() {
        info.code = code.to_owned();
    }
    Ok(Some(info))
}

async fn fetch_movie_by_code(
    client: &reqwest::Client,
    code: &str,
) -> Result<Option<ApiMovie>, AppError> {
    let session = cached_session(client, code).await?;
    match fetch_movie_with_session(client, &session, code).await {
        Ok(movie) => Ok(movie),
        Err(error) if is_session_auth_error(&error) => {
            invalidate_session(&session);
            let session = refresh_session(client, code).await?;
            fetch_movie_with_session(client, &session, code).await
        }
        Err(error) => Err(error),
    }
}

async fn fetch_movie_with_session(
    client: &reqwest::Client,
    session: &Session,
    code: &str,
) -> Result<Option<ApiMovie>, AppError> {
    let search_payload = json!([
        { "search": code, "lang": API_LANGUAGE },
        API_SEARCH_LIMIT,
        1
    ]);
    let results: Vec<ApiMovie> =
        post_api(client, session, "/jav/data/api/search", &search_payload).await?;
    let Some(result) = find_search_result(&results, code) else {
        return Ok(None);
    };
    if result.movie_id.trim().is_empty() {
        return Ok(None);
    }
    let detail_payload = json!([result.movie_id, API_LANGUAGE]);
    let mut movie: ApiMovie =
        post_api(client, session, "/jav/data/api/getMovie", &detail_payload).await?;
    if movie.movie_fan_hao.trim().is_empty() {
        movie.movie_fan_hao = result.movie_fan_hao.clone();
    }
    if movie.movie_id.trim().is_empty() {
        movie.movie_id = result.movie_id.clone();
    }
    Ok(Some(movie))
}

fn find_search_result(results: &[ApiMovie], code: &str) -> Option<ApiMovie> {
    let want = normalize_code(code);
    results
        .iter()
        .find(|result| {
            normalize_code(&result.movie_fan_hao) == want && !result.movie_id.trim().is_empty()
        })
        .cloned()
}

fn normalize_code(code: &str) -> String {
    code.trim()
        .to_uppercase()
        .chars()
        .filter(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
        .collect()
}

fn movie_into_match(movie: &ApiMovie) -> JavMatch {
    let title = first_non_empty([
        &movie.title_cn,
        &movie.title_tw,
        &movie.title,
        &movie.title_ja,
        &movie.title_en,
    ]);
    let series = movie.series.as_ref().map(|series| {
        first_non_empty([
            &series.series_name_cn,
            &series.series_name_tw,
            &series.series_name,
        ])
    });
    let studio = movie.studio.as_ref().map(|studio| {
        first_non_empty([
            &studio.studio_name_cn,
            &studio.studio_name_tw,
            &studio.studio_name,
        ])
    });
    let tags = dedupe(movie.genre.iter().map(|genre| {
        first_non_empty([
            &genre.genre_name_cn,
            &genre.genre_name_tw,
            &genre.genre_name,
        ])
    }));
    let actors =
        dedupe(movie.star.iter().map(|star| {
            first_non_empty([&star.star_name_cn, &star.star_name_tw, &star.star_name])
        }));
    let cover_url = first_non_empty([&movie.poster_large, &movie.poster_small]);
    JavMatch {
        code: movie.movie_fan_hao.trim().to_owned(),
        title,
        series: none_if_empty(series.unwrap_or_default()),
        studio: none_if_empty(studio.unwrap_or_default()),
        director: None,
        label: None,
        release_date: none_if_empty(movie.release_date.trim().to_owned()),
        duration_min: if movie.length == 0 {
            None
        } else {
            Some(movie.length)
        },
        tags,
        actors,
        cover_url: none_if_empty(cover_url),
        uncensored: Some(false),
        summary: None,
        rating: None,
        provider: "avmoo".into(),
    }
}

fn none_if_empty(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = &'a String>) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    String::new()
}

fn dedupe(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for value in values {
        let value = value.trim().to_owned();
        if !value.is_empty() && seen.insert(value.clone()) {
            out.push(value);
        }
    }
    out
}

fn is_session_auth_error(error: &AppError) -> bool {
    let message = error.to_string();
    ["code 400", "code 401", "code 403", "code 419"]
        .iter()
        .any(|marker| message.contains(marker))
}

async fn post_api<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    session: &Session,
    path: &str,
    payload: &serde_json::Value,
) -> Result<T, AppError> {
    let mut last_error = None;
    // Bulk fast mode: one attempt instead of three — retries double/triple
    // the wall time of unreachable-source items in large batches.
    let tries = if super::fast_mode() { 1 } else { API_TRIES };
    for attempt in 1..=tries {
        match post_api_once(client, session, path, payload).await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let retryable = is_retryable(&error);
                last_error = Some(error);
                if attempt == tries || !retryable {
                    break;
                }
                tokio::time::sleep(API_RETRY_DELAY).await;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| AppError::Provider("avmoo api failed".into())))
}

fn is_retryable(error: &AppError) -> bool {
    let message = error.to_string();
    if message.contains("timeout") || message.contains("timed out") {
        return true;
    }
    if message.contains("code 429") {
        return true;
    }
    ["code 500", "code 502", "code 503", "code 504"]
        .iter()
        .any(|marker| message.contains(marker))
}

async fn post_api_once<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    session: &Session,
    path: &str,
    payload: &serde_json::Value,
) -> Result<T, AppError> {
    let target = format!("{BASE_URL}{path}");
    RATE.wait().await;
    let response = client
        .post(&target)
        .header("User-Agent", CHROME_UA)
        .header("Accept", "application/json, text/plain, */*")
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Content-Type", "application/json")
        .header("Origin", BASE_URL)
        .header("Referer", &session.referer)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("X-CSRF-Token", &session.csrf_token)
        .header("Cookie", &session.cookie)
        .json(payload)
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("avmoo request {target}: {error}")))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(AppError::NotFound("avmoo 404".into()));
    }
    if !status.is_success() {
        return Err(AppError::Provider(format!(
            "avmoo http code {}",
            status.as_u16()
        )));
    }

    let envelope: ApiEnvelope = response
        .json()
        .await
        .map_err(|error| AppError::Provider(format!("avmoo parse envelope: {error}")))?;
    if envelope.code == 404 {
        return Err(AppError::NotFound("avmoo api 404".into()));
    }
    if envelope.code != 200 {
        return Err(AppError::Provider(format!(
            "avmoo api code {}: {}",
            envelope.code, envelope.message
        )));
    }
    if envelope.data.is_null() {
        return Err(AppError::NotFound("avmoo api empty data".into()));
    }
    serde_json::from_value(envelope.data)
        .map_err(|error| AppError::Provider(format!("avmoo parse data: {error}")))
}

// --- session cache ---------------------------------------------------------

static SESSION_CACHE: std::sync::Mutex<Option<(Session, Instant)>> = std::sync::Mutex::new(None);

async fn cached_session(client: &reqwest::Client, code: &str) -> Result<Session, AppError> {
    if let Some((session, expires_at)) = SESSION_CACHE.lock().expect("session cache").as_ref() {
        if Instant::now() < *expires_at {
            return Ok(session.clone());
        }
    }
    refresh_session(client, code).await
}

async fn refresh_session(client: &reqwest::Client, code: &str) -> Result<Session, AppError> {
    let session = fetch_session(client, code).await?;
    *SESSION_CACHE.lock().expect("session cache") =
        Some((session.clone(), Instant::now() + SESSION_TTL));
    Ok(session)
}

fn invalidate_session(session: &Session) {
    let mut cache = SESSION_CACHE.lock().expect("session cache");
    if let Some((cached, _)) = cache.as_ref() {
        if cached.csrf_token == session.csrf_token && cached.cookie == session.cookie {
            *cache = None;
        }
    }
}

async fn fetch_session(client: &reqwest::Client, code: &str) -> Result<Session, AppError> {
    let page_url = format!("{BASE_URL}/{API_LANGUAGE}/search/{}", urlencoding(code));
    RATE.wait().await;
    let response = client
        .get(&page_url)
        .header("User-Agent", CHROME_UA)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Referer", BASE_URL)
        .header("Sec-Fetch-Site", "none")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Dest", "document")
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("avmoo session request: {error}")))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(AppError::NotFound("avmoo search page 404".into()));
    }
    if !status.is_success() {
        return Err(AppError::Provider(format!(
            "avmoo session http code {}",
            status.as_u16()
        )));
    }

    let cookie = cookie_header(&response);
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Provider(format!("avmoo session read: {error}")))?;
    let token = extract_csrf_token(&body);
    if token.is_empty() || cookie.is_empty() {
        return Err(AppError::Provider("avmoo missing csrf session".into()));
    }
    Ok(Session {
        csrf_token: token,
        cookie,
        referer: page_url,
    })
}

fn urlencoding(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
                ch.to_string()
            } else {
                let mut buf = [0u8; 4];
                ch.encode_utf8(&mut buf)
                    .bytes()
                    .map(|byte| format!("%{byte:02X}"))
                    .collect::<String>()
            }
        })
        .collect()
}

fn extract_csrf_token(body: &str) -> String {
    let re = regex::Regex::new(r#"<meta\s+name=["']csrf-token["']\s+content=["']([^"']+)["']"#)
        .expect("regex");
    re.captures(body)
        .and_then(|caps| caps.get(1))
        .map(|token| token.as_str().trim().to_owned())
        .unwrap_or_default()
}

fn cookie_header(response: &reqwest::Response) -> String {
    response
        .cookies()
        .filter(|cookie| !cookie.name().trim().is_empty())
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect::<Vec<_>>()
        .join("; ")
}
