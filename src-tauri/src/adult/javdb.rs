//! JavDB scraper (second adult source).
//!
//! Searches `https://javdb.com/search?q={code}` and follows the first exact
//! code match into the detail page. JavDB is often the most complete source
//! for actors, studio, rating and cover when JavBus is behind driver-verify.

use reqwest::StatusCode;
use scraper::{Html, Selector};
use std::time::Duration;

use super::{resolve_url, JavMatch, RateLimiter, CHROME_UA};
use crate::error::AppError;

const BASE_URL: &str = "https://javdb.com";

static RATE: RateLimiter = RateLimiter::new(Duration::from_millis(800));

pub async fn lookup(client: &reqwest::Client, code: &str) -> Result<Option<JavMatch>, AppError> {
    let code = code.trim();
    if code.is_empty() {
        return Ok(None);
    }
    let search_url = format!(
        "{BASE_URL}/search?q={}&f=all",
        urlencoding::encode_or_raw(code)
    );
    RATE.wait().await;
    let response = client
        .get(&search_url)
        .header("User-Agent", CHROME_UA)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Cookie", "locale=zh; over18=1")
        .header("Referer", format!("{BASE_URL}/"))
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("javdb search {search_url}: {error}")))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if is_blocked(status, response.url().as_str()) {
        return Err(AppError::Provider(format!(
            "javdb blocked ({status}) at {}",
            response.url()
        )));
    }
    if !status.is_success() {
        return Err(AppError::Provider(format!(
            "javdb search non-200 for {search_url}: {status}"
        )));
    }

    let final_url = response.url().to_string();
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Provider(format!("javdb search body: {error}")))?;
    if looks_like_challenge(&body) {
        return Err(AppError::Provider("javdb returned an anti-bot page".into()));
    }

    // A unique match often 302s onto the detail page (`/v/...`).
    if final_url.contains("/v/") {
        return parse_detail(client, &final_url, &body, code);
    }

    let Some(detail_url) = ({
        let document = Html::parse_document(&body);
        find_search_hit(&document, &final_url, code)
    }) else {
        return Ok(None);
    };
    RATE.wait().await;
    let response = client
        .get(&detail_url)
        .header("User-Agent", CHROME_UA)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Cookie", "locale=zh; over18=1")
        .header("Referer", &search_url)
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("javdb detail {detail_url}: {error}")))?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if is_blocked(status, response.url().as_str()) || !status.is_success() {
        return Err(AppError::Provider(format!(
            "javdb detail {status} for {detail_url}"
        )));
    }
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Provider(format!("javdb detail body: {error}")))?;
    if looks_like_challenge(&body) {
        return Err(AppError::Provider("javdb returned an anti-bot page".into()));
    }
    parse_detail(client, &detail_url, &body, code)
}

fn parse_detail(
    _client: &reqwest::Client,
    page_url: &str,
    body: &str,
    fallback_code: &str,
) -> Result<Option<JavMatch>, AppError> {
    let document = Html::parse_document(body);
    let title = first_text(
        &document,
        &[
            "h2.title strong",
            "h2.title",
            ".current-title",
            "strong.current-title",
            "title",
        ],
    );
    let title = strip_site_suffix(&title);
    if title.is_empty() {
        return Ok(None);
    }

    let mut info = JavMatch::new(String::new(), title, "javdb");
    let fields = collect_panel_fields(&document);
    info.code = first_field(&fields, &["番號", "番号", "id"])
        .or_else(|| extract_code_from_title(&info.title))
        .unwrap_or_else(|| fallback_code.to_owned());
    info.release_date = first_field(&fields, &["日期", "date"]);
    info.duration_min = first_field(&fields, &["時長", "时长", "duration"]).and_then(parse_minutes);
    info.director = first_field(&fields, &["導演", "导演", "director"]);
    info.studio = first_field(&fields, &["片商", "製作商", "制作商", "studio", "maker"]);
    info.label = first_field(&fields, &["發行", "发行", "發行商", "label"]);
    info.series = first_field(&fields, &["系列", "series"]);
    info.tags = split_field(first_field(&fields, &["類別", "类别", "tags", "genres"]));
    if info.tags.is_empty() {
        info.tags = collect_links(&document, "span.tag a, .genre-tags a, a[href*='/tags/']");
    }
    info.actors = split_field(first_field(&fields, &["演員", "演员", "actors"]));
    if info.actors.is_empty() {
        info.actors = collect_links(&document, r#"a[href*="/actors/"]"#);
    }
    info.rating = first_field(&fields, &["評分", "评分", "score"])
        .as_deref()
        .and_then(parse_rating);
    if info.rating.is_none() {
        info.rating = parse_rating(&first_text(
            &document,
            &[".score-stars", "span.score", ".value"],
        ));
    }
    let cover = first_attr(
        &document,
        &[
            (r#"meta[property="og:image"]"#, "content"),
            (".column-video-cover img", "src"),
            (".video-cover img", "src"),
            ("img.video-cover", "src"),
        ],
    );
    if !cover.is_empty() {
        info.cover_url = Some(resolve_url(page_url, &cover));
    }
    info.title = strip_code_prefix(&info.title, &info.code);
    if info.code.is_empty() || info.title.is_empty() {
        return Ok(None);
    }
    Ok(Some(info))
}

fn find_search_hit(document: &Html, page_url: &str, code: &str) -> Option<String> {
    let want = normalize_code(code);
    let selector = sel("a.box, .movie-list .item a, .grid-item a, a[href*='/v/']");
    for link in document.select(&selector) {
        let href = link.value().attr("href").unwrap_or_default();
        if !href.contains("/v/") {
            continue;
        }
        let text = clean_text(&link.text().collect::<String>());
        let uid = first_text_in(&link, &[".uid", ".video-title strong", "strong"]);
        let candidate = if uid.is_empty() { text.clone() } else { uid };
        if normalize_code(&candidate) == want || normalize_code(&text).contains(&want) {
            let resolved = resolve_url(page_url, href);
            if !resolved.is_empty() {
                return Some(resolved);
            }
        }
    }
    None
}

fn collect_panel_fields(document: &Html) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let block = sel(".movie-panel-info .panel-block, nav.panel .panel-block");
    for item in document.select(&block) {
        let label = first_text_in(&item, &["strong"]);
        let value = {
            let full = clean_text(&item.text().collect::<String>());
            full.strip_prefix(&label)
                .unwrap_or(&full)
                .trim()
                .trim_start_matches(':')
                .trim_start_matches('：')
                .trim()
                .to_owned()
        };
        if !label.is_empty() && !value.is_empty() {
            out.push((label, value));
        }
    }
    out
}

fn first_field(fields: &[(String, String)], labels: &[&str]) -> Option<String> {
    for (label, value) in fields {
        let lower = label.to_lowercase();
        if labels.iter().any(|want| {
            let want = want.to_lowercase();
            lower == want || lower.trim_end_matches([':', '：', ' ']) == want
        }) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn split_field(value: Option<String>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for part in value.split([',', '，', '/', '|', '、', '\n']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if seen.insert(part.to_owned()) {
            out.push(part.to_owned());
        }
    }
    out
}

fn collect_links(document: &Html, css: &str) -> Vec<String> {
    let Ok(selector) = Selector::parse(css) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for link in document.select(&selector) {
        let text = clean_text(&link.text().collect::<String>());
        if text.is_empty() {
            continue;
        }
        if seen.insert(text.clone()) {
            out.push(text);
        }
    }
    out
}

fn first_text(document: &Html, css: &[&str]) -> String {
    for selector in css {
        let Ok(parsed) = Selector::parse(selector) else {
            continue;
        };
        if let Some(element) = document.select(&parsed).next() {
            let text = clean_text(&element.text().collect::<String>());
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

fn first_text_in(element: &scraper::ElementRef<'_>, css: &[&str]) -> String {
    for selector in css {
        let Ok(parsed) = Selector::parse(selector) else {
            continue;
        };
        if let Some(child) = element.select(&parsed).next() {
            let text = clean_text(&child.text().collect::<String>());
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

fn first_attr(document: &Html, candidates: &[(&str, &str)]) -> String {
    for (css, attr) in candidates {
        let Ok(selector) = Selector::parse(css) else {
            continue;
        };
        if let Some(element) = document.select(&selector).next() {
            if let Some(value) = element.value().attr(attr) {
                if !value.trim().is_empty() {
                    return value.trim().to_owned();
                }
            }
        }
    }
    String::new()
}

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("static selector must parse")
}

fn clean_text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_site_suffix(raw: &str) -> String {
    let mut title = raw.trim().to_string();
    for suffix in [" | JavDB", " - JavDB", " | JAVDB", " - JAVDB"] {
        if let Some(stripped) = title.strip_suffix(suffix) {
            title = stripped.trim().to_string();
        }
    }
    title
}

fn strip_code_prefix(title: &str, code: &str) -> String {
    let title = title.trim();
    if code.is_empty() {
        return title.to_owned();
    }
    let upper = title.to_uppercase();
    let code_upper = code.to_uppercase();
    if let Some(rest) = upper.strip_prefix(&code_upper) {
        let skip = title.len().saturating_sub(rest.len());
        return title[skip..]
            .trim()
            .trim_start_matches(['-', ':', '：', ' '])
            .trim()
            .to_owned();
    }
    title.to_owned()
}

fn extract_code_from_title(title: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?i)\b([a-z]{2,6}[-_ ]?\d{2,5}[a-z]?)\b").ok()?;
    re.captures(title)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().trim().to_uppercase().replace(' ', "-"))
}

fn parse_minutes(raw: String) -> Option<u32> {
    let re = regex::Regex::new(r"(\d{1,4})").ok()?;
    re.captures(&raw)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
        .filter(|value| *value > 0)
}

fn parse_rating(raw: &str) -> Option<f64> {
    let re = regex::Regex::new(r"(\d+(?:\.\d+)?)").ok()?;
    let value: f64 = re.captures(raw)?.get(1)?.as_str().parse().ok()?;
    if value <= 0.0 {
        return None;
    }
    // JavDB scores are typically x.xx / 5.0; keep the raw number.
    Some(value)
}

fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
}

fn is_blocked(status: StatusCode, url: &str) -> bool {
    status == StatusCode::FORBIDDEN
        || status == StatusCode::TOO_MANY_REQUESTS
        || url.contains("challenge")
        || url.contains("cdn-cgi")
}

fn looks_like_challenge(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("cf-browser-verification")
        || lower.contains("just a moment")
        || lower.contains("attention required")
        || (lower.contains("cloudflare") && lower.contains("challenge"))
}

/// Tiny URL-encoder that only percent-encodes non-unreserved characters.
mod urlencoding {
    pub fn encode_or_raw(value: &str) -> String {
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
}
