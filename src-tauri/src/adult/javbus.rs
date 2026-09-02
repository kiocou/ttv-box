//! JavBus scraper (primary adult source).
//!
//! Ported from JavBoss `internal/jav/javbus.go`. Fetches the detail page for a
//! code and parses title / code / series / release date / duration / tags /
//! actors / cover from the HTML. Handles the JavBus-specific prefix rewrites
//! (`gana` -> `200gana`, ...) and the `driver-verify` anti-bot redirect.

use reqwest::StatusCode;
use scraper::{Html, Selector};
use std::time::Duration;

use super::{resolve_url, JavMatch, RateLimiter, CHROME_UA};
use crate::error::AppError;

const BASE_URL: &str = "https://www.javbus.com";

static RATE: RateLimiter = RateLimiter::new(Duration::from_millis(500));

struct CodeRewrite {
    input_prefix: &'static str,
    request_prefix: &'static str,
}

const REWRITES: &[CodeRewrite] = &[
    CodeRewrite {
        input_prefix: "gana",
        request_prefix: "200gana",
    },
    CodeRewrite {
        input_prefix: "mium",
        request_prefix: "300mium",
    },
    CodeRewrite {
        input_prefix: "luxu",
        request_prefix: "259luxu",
    },
];

/// Look up a code on JavBus. Returns `Ok(None)` when the code is a confirmed
/// 404, `Err` on transient/network failures so the caller can fall through.
pub async fn lookup(client: &reqwest::Client, code: &str) -> Result<Option<JavMatch>, AppError> {
    let code = code.trim();
    if code.is_empty() {
        return Ok(None);
    }
    let (lookup_code, rewrite) = javbus_lookup_code(code);
    let url = format!("{BASE_URL}/{lookup_code}");

    RATE.wait().await;
    let response = client
        .get(&url)
        .header("User-Agent", CHROME_UA)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("Referer", format!("{BASE_URL}/"))
        .header("Cookie", "age=verified; existmag=mag")
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("javbus request {url}: {error}")))?;

    if response.url().as_str().contains("driver-verify") {
        return Err(AppError::Provider(
            "javbus requires browser verification (driver-verify)".into(),
        ));
    }

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Provider(format!(
            "javbus non-200 for {url}: {status}"
        )));
    }

    let body = response
        .text()
        .await
        .map_err(|error| AppError::Provider(format!("javbus read body: {error}")))?;
    let document = Html::parse_document(&body);

    let Some(mut info) = parse_document(&document) else {
        return Ok(None);
    };
    let cover = parse_cover_url(&document, &url);
    if !cover.is_empty() {
        info.cover_url = Some(cover);
    }
    if info.code.is_empty() {
        info.code = lookup_code.clone();
    }
    if let Some(rewrite) = rewrite {
        info.code = strip_request_prefix(&info.code, rewrite);
        info.title = clean_title(&strip_request_prefix(&info.title, rewrite));
    }
    if info.code.is_empty() || info.title.is_empty() {
        return Ok(None);
    }
    Ok(Some(info))
}

fn javbus_lookup_code(code: &str) -> (String, Option<&'static CodeRewrite>) {
    let code = code.trim();
    for rewrite in REWRITES {
        if code_has_prefix(code, rewrite.input_prefix) {
            let rest = &code[rewrite.input_prefix.len()..];
            return (format!("{}{}", rewrite.request_prefix, rest), Some(rewrite));
        }
    }
    (code.to_owned(), None)
}

fn code_has_prefix(code: &str, prefix: &str) -> bool {
    let Some(head) = code.get(..prefix.len()) else {
        return false;
    };
    if !head.eq_ignore_ascii_case(prefix) {
        return false;
    }
    let Some(next) = code.as_bytes().get(prefix.len()) else {
        return false;
    };
    matches!(next, b'-' | b'_' | b' ') || next.is_ascii_digit()
}

fn strip_request_prefix(value: &str, rewrite: &CodeRewrite) -> String {
    let value = value.trim();
    if !code_has_prefix(value, rewrite.request_prefix) {
        return value.to_owned();
    }
    let added = rewrite.request_prefix.len() - rewrite.input_prefix.len();
    if added == 0 || value.len() <= added {
        return value.to_owned();
    }
    let Some(rest) = value.get(added..) else {
        return value.to_owned();
    };
    rest.trim().to_owned()
}

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("static selector must parse")
}

fn clean_text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_document(document: &Html) -> Option<JavMatch> {
    let raw_title = first_text_by_tag(document, "h3");
    let raw_title = if raw_title.is_empty() {
        first_text_by_tag(document, "title")
    } else {
        raw_title
    };
    let title = clean_title(&raw_title);
    let code = extract_field(document, &["識別碼", "识别码", "id:"]);
    let series = extract_field(document, &["系列"]);
    let studio = extract_field(document, &["製作商", "制作商", "studio"]);
    let label = extract_field(document, &["發行商", "发行商", "label"]);
    let director = extract_field(document, &["導演", "导演", "director"]);
    let (release_date, duration_min) = extract_details(document);
    let tags = collect_genres(document);
    let actors = collect_actors(document);
    let uncensored = parse_is_uncensored(document);
    let rating = parse_score(document);

    if title.is_empty() && tags.is_empty() && actors.is_empty() {
        return None;
    }
    Some(JavMatch {
        code,
        title,
        series: none_if_empty(series),
        studio: none_if_empty(studio),
        director: none_if_empty(director),
        label: none_if_empty(label),
        release_date: none_if_empty(release_date),
        duration_min,
        tags,
        actors,
        cover_url: None,
        uncensored: Some(uncensored),
        summary: None,
        rating,
        provider: "javbus".into(),
    })
}

fn none_if_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn first_text_by_tag(document: &Html, tag: &str) -> String {
    let Ok(selector) = Selector::parse(tag) else {
        return String::new();
    };
    document
        .select(&selector)
        .next()
        .map(|element| clean_text(&element.text().collect::<String>()))
        .unwrap_or_default()
}

fn clean_title(raw: &str) -> String {
    let mut title = raw.trim().to_string();
    if let Some(stripped) = title.strip_suffix("- JavBus") {
        title = stripped.trim().to_string();
    }
    let re = regex::Regex::new(r"(?i)^[a-z]{2,6}[-_ ]?\d{2,5}\s*").expect("regex");
    re.replace(&title, "").trim().to_string()
}

fn normalize_label(raw: &str) -> String {
    let mut label = raw.trim().to_lowercase();
    for suffix in [":", "："] {
        if let Some(stripped) = label.strip_suffix(suffix) {
            label = stripped.to_string();
        }
    }
    label.trim().to_string()
}

fn extract_field(document: &Html, labels: &[&str]) -> String {
    let normalized: Vec<String> = labels.iter().map(|label| normalize_label(label)).collect();
    let span_selector = sel("span");
    for span in document.select(&span_selector) {
        let label_text = normalize_label(&span.text().collect::<String>());
        let matches = normalized
            .iter()
            .any(|want| !want.is_empty() && (label_text == *want || label_text.contains(want)));
        if !matches {
            continue;
        }
        let mut value = span
            .next_sibling()
            .and_then(scraper::ElementRef::wrap)
            .map(|sibling| clean_text(&sibling.text().collect::<String>()))
            .unwrap_or_default();
        if value.is_empty() {
            if let Some(parent) = span.parent().and_then(scraper::ElementRef::wrap) {
                let parent_text = clean_text(&parent.text().collect::<String>());
                let label_raw = clean_text(&span.text().collect::<String>());
                value = parent_text
                    .strip_prefix(&label_raw)
                    .unwrap_or(&parent_text)
                    .trim()
                    .to_string();
            }
        }
        if !value.is_empty() {
            return value;
        }
    }
    String::new()
}

fn extract_details(document: &Html) -> (String, Option<u32>) {
    let date_re = regex::Regex::new(r"\d{4}-\d{2}-\d{2}").expect("regex");
    let duration_re = regex::Regex::new(r"(\d{1,4})\s*(分鐘|分钟|分|分間|min)?").expect("regex");
    let paragraph_selector = sel("p");

    let mut release = String::new();
    let mut duration: Option<u32> = None;
    for paragraph in document.select(&paragraph_selector) {
        let text = clean_text(&paragraph.text().collect::<String>());
        let lower = text.to_lowercase();
        if release.is_empty()
            && (text.contains("發行日期") || text.contains("発売日") || lower.contains("release"))
        {
            if let Some(found) = date_re.find(&text) {
                release = found.as_str().to_string();
            }
        }
        if duration.is_none()
            && (text.contains("長度")
                || text.contains("時長")
                || text.contains("時間")
                || lower.contains("length")
                || lower.contains("duration"))
        {
            if let Some(caps) = duration_re.captures(&text) {
                if let Some(number) = caps.get(1) {
                    if let Ok(value) = number.as_str().trim().parse::<u32>() {
                        duration = Some(value);
                    }
                }
            }
        }
        if !release.is_empty() && duration.is_some() {
            break;
        }
    }
    (release, duration)
}

fn collect_genres(document: &Html) -> Vec<String> {
    let genre_selector = sel("span.genre a");
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for link in document.select(&genre_selector) {
        let href = link.value().attr("href").unwrap_or_default();
        if href.contains("/star/") {
            continue;
        }
        let text = clean_text(&link.text().collect::<String>());
        if !text.is_empty() && seen.insert(text.clone()) {
            out.push(text);
        }
    }
    out
}

fn collect_actors(document: &Html) -> Vec<String> {
    let actor_selector = sel(r#"a[href*="/star/"]"#);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for link in document.select(&actor_selector) {
        let href = link.value().attr("href").unwrap_or_default();
        if !href.contains("/star/") {
            continue;
        }
        let text = clean_text(&link.text().collect::<String>());
        if text.is_empty()
            || text.eq_ignore_ascii_case("n/a")
            || text.chars().count() > 40
            || text.contains("画像")
        {
            continue;
        }
        if seen.insert(text.clone()) {
            out.push(text);
        }
    }
    out
}

fn parse_score(document: &Html) -> Option<f64> {
    let raw = extract_field(document, &["評分", "评分", "score"]);
    let candidate = if raw.is_empty() {
        let Ok(selector) = Selector::parse("span.score, .score") else {
            return None;
        };
        document
            .select(&selector)
            .next()
            .map(|element| clean_text(&element.text().collect::<String>()))
            .unwrap_or_default()
    } else {
        raw
    };
    let re = regex::Regex::new(r"(\d+(?:\.\d+)?)").ok()?;
    let value: f64 = re.captures(&candidate)?.get(1)?.as_str().parse().ok()?;
    (value > 0.0).then_some(value)
}

fn parse_is_uncensored(document: &Html) -> bool {
    let link_selector = sel("li.active a");
    for link in document.select(&link_selector) {
        let href = link.value().attr("href").unwrap_or_default().to_lowercase();
        let text = clean_text(&link.text().collect::<String>()).to_lowercase();
        if href.contains("/uncensored")
            || text.contains("無碼")
            || text.contains("无码")
            || text.contains("uncensored")
        {
            return true;
        }
    }
    false
}

fn parse_cover_url(document: &Html, page_url: &str) -> String {
    let candidates: [(&str, &str); 3] = [
        (r#"meta[property="og:image"]"#, "content"),
        ("a.bigImage", "href"),
        ("img.cover, .bigImage img", "src"),
    ];
    for (css, attr) in candidates {
        let Ok(selector) = Selector::parse(css) else {
            continue;
        };
        let Some(element) = document.select(&selector).next() else {
            continue;
        };
        let candidate = element.value().attr(attr).unwrap_or_default();
        let cover = resolve_url(page_url, candidate);
        if !cover.is_empty() {
            return cover;
        }
    }
    String::new()
}
