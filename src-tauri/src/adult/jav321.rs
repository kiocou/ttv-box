//! Jav321 scraper (uncensored-friendly fallback).
//!
//! POSTs the code to `https://www.jav321.com/search`. A unique hit lands on
//! `/video/{code}`; otherwise the first matching card is followed. Useful for
//! Caribbean / 1pondo / Heyzo style uncensored titles that JavBus often 404s.

use reqwest::StatusCode;
use scraper::{Html, Selector};
use std::time::Duration;

use super::{resolve_url, JavMatch, RateLimiter, CHROME_UA};
use crate::error::AppError;

const BASE_URL: &str = "https://www.jav321.com";

static RATE: RateLimiter = RateLimiter::new(Duration::from_millis(700));

pub async fn lookup(client: &reqwest::Client, code: &str) -> Result<Option<JavMatch>, AppError> {
    let code = code.trim();
    if code.is_empty() {
        return Ok(None);
    }
    RATE.wait().await;
    let response = client
        .post(format!("{BASE_URL}/search"))
        .header("User-Agent", CHROME_UA)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9,ja;q=0.8,en;q=0.7")
        .header("Referer", format!("{BASE_URL}/"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("sn={}", encode(code)))
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("jav321 search {code}: {error}")))?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Provider(format!("jav321 search {status}")));
    }
    let final_url = response.url().to_string();
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Provider(format!("jav321 search body: {error}")))?;

    if final_url.contains("/video/") {
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
        .header("Referer", format!("{BASE_URL}/"))
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("jav321 detail {detail_url}: {error}")))?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Provider(format!(
            "jav321 detail {status} for {detail_url}"
        )));
    }
    let body = response
        .text()
        .await
        .map_err(|error| AppError::Provider(format!("jav321 detail body: {error}")))?;
    Ok(parse_detail(&detail_url, &body, code))
}

fn parse_detail(page_url: &str, body: &str, fallback_code: &str) -> Option<JavMatch> {
    let document = Html::parse_document(body);
    let raw_title = first_text(
        &document,
        &[
            "div.panel-heading h3",
            "h3",
            "h1",
            r#"meta[property="og:title"]"#,
            "title",
        ],
    );
    let title = strip_site_suffix(&raw_title);
    if title.is_empty() {
        return None;
    }
    let mut info = JavMatch::new(String::new(), title, "jav321");
    let fields = collect_dl_fields(&document);
    info.code = first_field(&fields, &["番号", "品番", "id", "sn"])
        .unwrap_or_else(|| fallback_code.to_owned());
    info.release_date = first_field(&fields, &["发行日期", "発売日", "date"]);
    info.duration_min =
        first_field(&fields, &["播放时长", "収録時間", "时长", "duration"]).and_then(parse_minutes);
    info.studio = first_field(&fields, &["片商", "メーカー", "studio", "maker"]);
    info.series = first_field(&fields, &["系列", "シリーズ", "series"]);
    info.actors = split_field(first_field(&fields, &["女优", "女優", "演员", "actors"]));
    if info.actors.is_empty() {
        info.actors = collect_links(&document, "a[href*='/star/'], a[href*='/actress/']");
    }
    info.tags = split_field(first_field(&fields, &["标签", "ジャンル", "genres"]));
    if info.tags.is_empty() {
        info.tags = collect_links(&document, "a[href*='/genre/'], a[href*='/tag/']");
    }
    let plot = first_text(
        &document,
        &[
            "div.panel-body p",
            "#video_info p",
            ".col-md-12 p",
            r#"meta[name="description"]"#,
        ],
    );
    if !plot.is_empty() && plot.len() > 8 {
        info.summary = Some(plot);
    }
    let cover = first_attr(
        &document,
        &[
            (r#"meta[property="og:image"]"#, "content"),
            ("img.img-responsive", "src"),
            (".col-md-3 img", "src"),
            ("img.img-rounded", "src"),
        ],
    );
    if !cover.is_empty() {
        info.cover_url = Some(resolve_url(page_url, &cover));
    }
    info.uncensored = Some(true);
    info.title = strip_code_prefix(&info.title, &info.code);
    if info.code.is_empty() || info.title.is_empty() {
        return None;
    }
    Some(info)
}

fn find_search_hit(document: &Html, page_url: &str, code: &str) -> Option<String> {
    let want = normalize_code(code);
    let selector = sel("a[href*='/video/']");
    for link in document.select(&selector) {
        let href = link.value().attr("href").unwrap_or_default();
        let text = clean_text(&link.text().collect::<String>());
        if normalize_code(href).contains(&want) || normalize_code(&text) == want {
            let resolved = resolve_url(page_url, href);
            if !resolved.is_empty() {
                return Some(resolved);
            }
        }
    }
    None
}

fn collect_dl_fields(document: &Html) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(row) = Selector::parse("b, strong, th") {
        for label_el in document.select(&row) {
            let label = clean_text(&label_el.text().collect::<String>())
                .trim_end_matches([':', '：'])
                .to_owned();
            if label.is_empty() || label.chars().count() > 12 {
                continue;
            }
            let value = label_el
                .next_sibling()
                .and_then(scraper::ElementRef::wrap)
                .map(|sibling| clean_text(&sibling.text().collect::<String>()))
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    label_el
                        .parent()
                        .and_then(scraper::ElementRef::wrap)
                        .map(|parent| {
                            let full = clean_text(&parent.text().collect::<String>());
                            full.strip_prefix(&label)
                                .unwrap_or(&full)
                                .trim()
                                .trim_start_matches([':', '：'])
                                .trim()
                                .to_owned()
                        })
                })
                .unwrap_or_default();
            if !value.is_empty() {
                out.push((label, value));
            }
        }
    }
    out
}

fn first_field(fields: &[(String, String)], labels: &[&str]) -> Option<String> {
    for (label, value) in fields {
        let lower = label.to_lowercase();
        if labels
            .iter()
            .any(|want| lower.contains(&want.to_lowercase()))
        {
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
            let text = if *selector == r#"meta[property="og:title"]"#
                || *selector == r#"meta[name="description"]"# {
                element
                    .value()
                    .attr("content")
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            } else {
                clean_text(&element.text().collect::<String>())
            };
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
    for suffix in [" - JAV321", " | JAV321", " - jav321"] {
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

fn parse_minutes(raw: String) -> Option<u32> {
    let re = regex::Regex::new(r"(\d{1,4})").ok()?;
    re.captures(&raw)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
        .filter(|value| *value > 0)
}

fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
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
