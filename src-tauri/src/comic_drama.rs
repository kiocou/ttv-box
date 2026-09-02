//! 红果漫剧公开目录（hongguoduanju.com 热播漫剧榜）+ 站内播放。
//!
//! 漫剧与短剧共用同一套 H5 详情/播放页（`/detail?series_id=`、`/player/{id}/{vid}`）
//! 以及 App-API（`content_type=1004` / aid=8704 MotionComic，worker 走 V2 播放模型
//! 并在 sinfonlineb/a/lf 三线故障转移）。
//! 官网没有独立 `/comic` 分类页：`/category?tab=comic` 会被改写成短剧目录。
//! 因此目录流抓 `/rank/hot-comic-drama` 的 SSR HTML 榜单（约 5 页 × 20 部），
//! 详情与直链复用 `short_drama_detail` / `short_drama_play`。

use crate::adult::curl_fetch::CurlFetch;
use crate::short_drama::{
    short_drama_detail, short_drama_play, ShortDramaCard, ShortDramaDetail, ShortDramaDetailInput,
    ShortDramaPlayInput, ShortDramaPlayback, ShortDramaStreamPage,
};
use regex::Regex;
use serde::Deserialize;
use std::sync::OnceLock;
use std::time::Duration;

const HONGGUO_BASE: &str = "https://hongguoduanju.com";
const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const RANK_PATH: &str = "/rank/hot-comic-drama";
const MAX_PAGES: u32 = 8;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComicDramaStreamInput {
    #[serde(default)]
    pub cursor: Option<String>,
    /// 本地题材过滤提示（榜单页无官方 facet；空 = 整榜翻页）。
    #[serde(default)]
    pub facet: Option<String>,
}

fn article_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)<article\b[^>]*class="[^"]*pc-item[^"]*"[^>]*>.*?</article>"#)
            .expect("article re")
    })
}

fn id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?:rank-title-|series_id=)(\d{15,})"#).expect("id re")
    })
}

fn title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)<h2[^>]*id="rank-title-\d+"[^>]*>(.*?)</h2>"#).expect("title re")
    })
}

fn cover_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"https://p\d-novel\.byteimg\.com/[^"'>\s]+"#).expect("cover re")
    })
}

fn episode_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"全\s*(\d+)\s*集"#).expect("episode re"))
}

fn desc_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?s)pc-description[^>]*>(.*?)</p>"#).expect("desc re"))
}

fn cats_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?s)pc-categories-[^"]*">([\s\S]*?)</p>"#).expect("cats re"))
}

fn span_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"<span>([^<]+)</span>"#).expect("span re"))
}

fn page_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"hot-comic-drama\?page=(\d+)"#).expect("page re")
    })
}

fn strip_tags(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    html_unescape(out.trim())
}

fn html_unescape(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn parse_rank_cards(html: &str) -> Vec<ShortDramaCard> {
    let mut cards = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for article in article_re().find_iter(html) {
        let block = article.as_str();
        let Some(id) = id_re().captures(block).map(|cap| cap[1].to_owned()) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let title = title_re()
            .captures(block)
            .map(|cap| strip_tags(&cap[1]))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("漫剧 {id}"));
        let cover = cover_re()
            .find(block)
            .map(|cap| cap.as_str().replace("&amp;", "&"))
            .unwrap_or_default();
        let description = desc_re()
            .captures(block)
            .map(|cap| strip_tags(&cap[1]))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "来自红果漫剧热播榜的条目。".into());
        let category = cats_re()
            .captures(block)
            .map(|cap| {
                span_re()
                    .captures_iter(&cap[1])
                    .map(|item| item[1].trim().to_owned())
                    .filter(|name| !name.is_empty() && !name.chars().all(|ch| ch.is_ascii_digit()))
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "漫剧".into());
        let total_episodes = episode_re()
            .captures(block)
            .and_then(|cap| cap[1].parse::<u32>().ok())
            .unwrap_or(0);
        cards.push(ShortDramaCard {
            id: id.clone(),
            title,
            cover_url: cover,
            episodes: if total_episodes > 0 {
                format!("全{total_episodes}集")
            } else {
                "漫剧".into()
            },
            category,
            source: "红果漫剧".into(),
            source_url: format!("{HONGGUO_BASE}/detail?series_id={id}"),
            description,
            total_episodes,
            playable_episodes: 0,
        });
    }
    cards
}

fn detect_total_pages(html: &str) -> u32 {
    page_link_re()
        .captures_iter(html)
        .filter_map(|cap| cap[1].parse::<u32>().ok())
        .max()
        .unwrap_or(1)
        .clamp(1, MAX_PAGES)
}

async fn fetch_via_reqwest(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent(BROWSER_UA)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("请求失败：{error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("读取响应失败：{error}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    Ok(body)
}

async fn fetch_via_curl(url: &str) -> Result<String, String> {
    let fetch = CurlFetch::new().map_err(|error| error.to_string())?;
    let (status, body) = fetch
        .get(url, false, &[("User-Agent".into(), BROWSER_UA.into())])
        .await
        .map_err(|error| error.to_string())?;
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    String::from_utf8(body).map_err(|error| format!("响应不是 UTF-8：{error}"))
}

async fn fetch_html(path: &str) -> Result<String, String> {
    let url = format!("{HONGGUO_BASE}{path}");
    match fetch_via_reqwest(&url).await {
        Ok(body) => Ok(body),
        Err(reqwest_error) => fetch_via_curl(&url).await.map_err(|curl_error| {
            format!("漫剧页面请求失败（reqwest：{reqwest_error}；curl：{curl_error}）")
        }),
    }
}

fn parse_cursor(cursor: Option<&str>) -> u32 {
    cursor
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or(1)
}

fn rank_path(page: u32) -> String {
    if page <= 1 {
        RANK_PATH.to_owned()
    } else {
        format!("{RANK_PATH}?page={page}")
    }
}

#[tauri::command]
pub async fn comic_drama_stream(
    input: ComicDramaStreamInput,
) -> Result<ShortDramaStreamPage, String> {
    let page = parse_cursor(input.cursor.as_deref());
    let body = fetch_html(&rank_path(page)).await?;
    let items = parse_rank_cards(&body);
    let total_pages = detect_total_pages(&body).max(page).clamp(1, MAX_PAGES);
    let next_page = if page < total_pages { page + 1 } else { 1 };
    Ok(ShortDramaStreamPage {
        items,
        next_cursor: Some(next_page.to_string()),
        source: "红果漫剧热播榜".into(),
        fetched_at: chrono::Utc::now().timestamp(),
    })
}

#[tauri::command]
pub async fn comic_drama_detail(input: ShortDramaDetailInput) -> Result<ShortDramaDetail, String> {
    short_drama_detail(input).await
}

#[tauri::command]
pub async fn comic_drama_play(input: ShortDramaPlayInput) -> Result<ShortDramaPlayback, String> {
    short_drama_play(input).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rank_article() {
        let html = r#"<ol><article class="pc-item-jlQoVk " aria-labelledby="rank-title-7677801492920667198"><a href="/detail?series_id=7677801492920667198" aria-label="查看糯糯下山，师兄们都慌了"><picture><source srcSet="https://p6-novel.byteimg.com/novel-pic/abc~tplv-shrink:640:0.webp"/><img src="https://p6-novel.byteimg.com/novel-pic/abc~tplv-shrink:640:0.image" alt="封面"/></picture></a><h2 id="rank-title-7677801492920667198">糯糯下山，师兄们都慌了</h2><p class="pc-categories-ApVUzG"><span>萌宝</span><span>异界</span><span>205</span></p><p class="pc-description-feL298">六岁那年，云糯糯被师父派下山历练。</p></article></ol><a href="/rank/hot-comic-drama?page=5">5</a>"#;
        let cards = parse_rank_cards(html);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, "7677801492920667198");
        assert_eq!(cards[0].title, "糯糯下山，师兄们都慌了");
        assert_eq!(cards[0].category, "萌宝 / 异界");
        assert!(cards[0].cover_url.contains("byteimg.com"));
        assert_eq!(detect_total_pages(html), 5);
    }
}
