//! JavLibrary scraper.
//!
//! Uses the Chinese locale search endpoint
//! `https://www.javlibrary.com/cn/vl_searchbyid.php?keyword={code}`. A unique
//! hit redirects to the detail page; otherwise the first exact-id row is
//! followed. JavLibrary is especially strong on director / maker / label.

use reqwest::StatusCode;
use scraper::{Html, Selector};
use std::time::Duration;

use super::{resolve_url, JavMatch, RateLimiter, CHROME_UA};
use crate::error::AppError;

const BASE_URL: &str = "https://www.javlibrary.com/cn";

static RATE: RateLimiter = RateLimiter::new(Duration::from_millis(900));

pub async fn lookup(client: &reqwest::Client, code: &str) -> Result<Option<JavMatch>, AppError> {
    let code = code.trim();
    if code.is_empty() {
        return Ok(None);
    }
    let search_url = format!("{BASE_URL}/vl_searchbyid.php?keyword={}", encode(code));
    RATE.wait().await;
    let response = client
        .get(&search_url)
        .header("User-Agent", CHROME_UA)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,ja;q=0.8,en;q=0.7")
        .header("Referer", format!("{BASE_URL}/"))
        .header("Cookie", "over18=18")
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("javlibrary search {search_url}: {error}")))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Provider(format!(
            "javlibrary search {status} for {search_url}"
        )));
    }
    let final_url = response.url().to_string();
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Provider(format!("javlibrary search body: {error}")))?;
    if looks_like_age_gate(&body) {
        return Err(AppError::Provider("javlibrary age-gate".into()));
    }

    if final_url.contains("?v=") || body.contains("id=\"video_id\"") {
        return Ok(parse_detail(&final_url, &body, code));
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
        .header("Accept-Language", "zh-CN,zh;q=0.9,ja;q=0.8,en;q=0.7")
        .header("Referer", &search_url)
        .header("Cookie", "over18=18")
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("javlibrary detail {detail_url}: {error}")))?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Provider(format!(
            "javlibrary detail {status} for {detail_url}"
        )));
    }
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Provider(format!("javlibrary detail body: {error}")))?;
    Ok(parse_detail(&detail_url, &body, code))
}

fn parse_detail(page_url: &str, body: &str, fallback_code: &str) -> Option<JavMatch> {
    let document = Html::parse_document(body);
    let raw_title = text_of(
        &document,
        "#video_title a, #video_title, h3.post-title a, title",
    );
    let title = strip_site_suffix(&raw_title);
    if title.is_empty() {
        return None;
    }
    let mut info = JavMatch::new(String::new(), title, "javlibrary");
    info.code = text_of(&document, "#video_id .text, #video_id td.text");
    if info.code.is_empty() {
        info.code = fallback_code.to_owned();
    }
    info.release_date = none_if_empty(text_of(&document, "#video_date .text, #video_date td.text"));
    info.duration_min = parse_minutes(&text_of(
        &document,
        "#video_length .text, #video_length span.text",
    ));
    info.director = none_if_empty(text_of(
        &document,
        "#video_director .text a, #video_director .text",
    ));
    info.studio = none_if_empty(text_of(
        &document,
        "#video_maker .text a, #video_maker .text",
    ));
    info.label = none_if_empty(text_of(
        &document,
        "#video_label .text a, #video_label .text",
    ));
    info.tags = collect_texts(&document, "#video_genres a, span.genre a");
    info.actors = collect_texts(
        &document,
        "#video_cast a.star, #video_cast span.star a, span.star a",
    );
    info.rating = parse_rating(&text_of(&document, "#video_review .score, span.score"));
    let cover = attr_of(
        &document,
        &[
            ("#video_jacket_img", "src"),
            (r#"meta[property="og:image"]"#, "content"),
            ("img#video_jacket_img", "src"),
        ],
    );
    if !cover.is_empty() {
        info.cover_url = Some(resolve_url(page_url, &cover));
    }
    info.title = strip_code_prefix(&info.title, &info.code);
    if info.code.is_empty() || info.title.is_empty() {
        return None;
    }
    Some(info)
}

fn find_search_hit(document: &Html, page_url: &str, code: &str) -> Option<String> {
    let want = normalize_code(code);
    let selector = sel(".video a, div.video > a, a[href*='?v=']");
    for link in document.select(&selector) {
        let href = link.value().attr("href").unwrap_or_default();
        if !href.contains("?v=") && !href.contains("&v=") {
            continue;
        }
        let id = first_child_text(&link, "div.id, .id");
        let text = if id.is_empty() {
            clean_text(&link.text().collect::<String>())
        } else {
            id
        };
        if normalize_code(&text) == want {
            let resolved = resolve_url(page_url, href);
            if !resolved.is_empty() {
                return Some(resolved);
            }
        }
    }
    None
}

fn text_of(document: &Html, css: &str) -> String {
    for selector in css.split(',') {
        let Ok(parsed) = Selector::parse(selector.trim()) else {
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

fn collect_texts(document: &Html, css: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for selector in css.split(',') {
        let Ok(parsed) = Selector::parse(selector.trim()) else {
            continue;
        };
        for element in document.select(&parsed) {
            let text = clean_text(&element.text().collect::<String>());
            if text.is_empty() {
                continue;
            }
            if seen.insert(text.clone()) {
                out.push(text);
            }
        }
    }
    out
}

fn attr_of(document: &Html, candidates: &[(&str, &str)]) -> String {
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

fn first_child_text(element: &scraper::ElementRef<'_>, css: &str) -> String {
    let Ok(selector) = Selector::parse(css) else {
        return String::new();
    };
    element
        .select(&selector)
        .next()
        .map(|child| clean_text(&child.text().collect::<String>()))
        .unwrap_or_default()
}

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("static selector must parse")
}

fn clean_text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn none_if_empty(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn strip_site_suffix(raw: &str) -> String {
    let mut title = raw.trim().to_string();
    for suffix in [" - JAVLibrary", " - JavLibrary", " - javlibrary"] {
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

fn parse_minutes(raw: &str) -> Option<u32> {
    let re = regex::Regex::new(r"(\d{1,4})").ok()?;
    re.captures(raw)?
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
        None
    } else {
        Some(value)
    }
}

fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
}

fn looks_like_age_gate(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("over18") && lower.contains("age") && !lower.contains("video_id")
}

fn encode(value: &str) -> String {
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
