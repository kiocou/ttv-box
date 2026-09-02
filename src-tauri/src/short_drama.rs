//! 红果短剧公开目录 + 站内播放集成（hongguoduanju.com H5）。
//!
//! 官网三个页面（目录/详情/播放）都是 SSR，把数据放在 `window._ROUTER_DATA`
//! 的 JSON 里，因此这里不做 HTML 选择器解析，直接提取 JSON：
//! - 目录页  `loaderData.category_page`：`recommendList[]`（每页 24 条完整卡片）
//!           + `pagination{totalPages}` + `selectorList[]`（官方中文分类树）。
//! - 详情页  `loaderData.detail_page`：`seriesDetail`（简介/标签/演员/vid_list）
//!           + `videoList`（50 部相关推荐，可反哺无限流）。
//! - 播放页  `/player/{series_id}/{vid}`：`video_player_info.main_url` 是免签名
//!           头、支持 Range 的直链 mp4（qznovelvod CDN），`<video>`/libmpv 直播。
//!
//! 版权边界（如实透出，不做绕过）：官网 H5 每部剧只放开前
//! `accessible_episode_cnt` 集（普遍为 3 集）网页直链，之后会 404。
//! 游标 = "{facet_idx}:{page}"，facet 队列 ~180 个分类面（背景/设定/题材/性别/
//! 时间/排序 × sort_type），翻完一页自动推进；一个面翻尽跳下一个面，全部翻完
//! 回卷到头（内容由前端按 id 去重，重复轮会被前端去重后自然轮换）。

use crate::adult::curl_fetch::CurlFetch;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const HONGGUO_BASE: &str = "https://hongguoduanju.com";
const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
/// 单次 stream 调用最多翻几个目录页（正常一次命中；空页/翻尽才多翻）。
const MAX_FETCHES_PER_CALL: usize = 6;
/// 单个分类面最多翻页数（官网目录普遍 ≤21 页，留裕量防脏数据死循环）。
const MAX_PAGES_PER_FACET: u32 = 40;

// ============ 数据结构 ============

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaCard {
    pub id: String,
    pub title: String,
    pub cover_url: String,
    pub episodes: String,
    pub category: String,
    pub source: String,
    pub source_url: String,
    pub description: String,
    pub total_episodes: u32,
    pub playable_episodes: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaStreamInput {
    #[serde(default)]
    pub cursor: Option<String>,
    /// 服务端分类过滤：形如 `background=cate_1`、`topic=cate_165&sort_type=1`
    /// 的官方目录查询串；空/缺省 = 全部内容池。
    #[serde(default)]
    pub facet: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaStreamPage {
    pub items: Vec<ShortDramaCard>,
    pub next_cursor: Option<String>,
    pub source: String,
    pub fetched_at: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaDetailInput {
    pub series_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaCastMember {
    pub name: String,
    pub role: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaDetail {
    pub id: String,
    pub title: String,
    pub cover_url: String,
    pub intro: String,
    pub tags: Vec<String>,
    pub episodes_text: String,
    pub total_episodes: u32,
    pub playable_episodes: u32,
    pub vids: Vec<String>,
    pub cast: Vec<ShortDramaCastMember>,
    pub recommendations: Vec<ShortDramaCard>,
    pub source_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaPlayInput {
    pub series_id: String,
    pub vid: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaPlayback {
    pub series_id: String,
    pub vid: String,
    pub url: String,
    pub poster_url: String,
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub series_name: String,
    pub cover_url: String,
    /// 1-based 集数（vid 在 vid_list 中的位置；未知为 0）。
    pub episode: u32,
    pub total_episodes: u32,
    pub playable_episodes: u32,
    pub vids: Vec<String>,
    /// 下一集 vid（没有则 None，前端据此停止自动连播）。
    pub next_vid: Option<String>,
}

// ============ 分类面游标 ============

/// 目录页官方分类树（category 页 selectorList 快照；官网 rarely 变动，
/// 变了也不影响：旧面会返回空页并跳过）。同一分类面用两种排序各铺一遍。
fn facet_queries() -> Vec<String> {
    const BACKGROUNDS: &[&str] = &[
        "cate_757",
        "cate_1",
        "cate_758",
        "cate_11",
        "cate_79",
        "cate_452",
        "cate_127",
        "cate_390",
        "cate_4",
        "cate_1153",
        "cate_1162",
    ];
    const SETTINGS: &[&str] = &[
        "cate_1051",
        "cate_1207",
        "cate_760",
        "cate_266",
        "cate_36",
        "cate_37",
        "cate_19",
        "cate_265",
        "cate_862",
        "cate_1010",
        "cate_475",
        "cate_20",
        "cate_936",
        "cate_1045",
        "cate_598",
        "cate_1007",
        "cate_1008",
        "cate_487",
        "cate_1049",
        "cate_1044",
        "cate_96",
        "cate_43",
        "cate_26",
        "cate_387",
        "cate_762",
        "cate_929",
        "cate_616",
        "cate_1293",
        "cate_477",
        "cate_1291",
        "cate_1287",
        "cate_1042",
        "cate_428",
        "cate_1255",
        "cate_1200",
        "cate_615",
        "cate_831",
        "cate_380",
        "cate_1191",
        "cate_826",
        "cate_582",
        "cate_375",
    ];
    const TOPICS: &[&str] = &[
        "cate_1021",
        "cate_1048",
        "cate_262",
        "cate_1020",
        "cate_1019",
        "cate_439",
        "cate_1038",
        "cate_246",
        "cate_1013",
        "cate_1047",
        "cate_1180",
        "cate_1022",
        "cate_165",
        "cate_303",
        "cate_297",
        "cate_1027",
        "cate_1025",
        "cate_751",
        "cate_1235",
        "cate_1136",
        "cate_1148",
        "cate_504",
        "cate_1172",
        "cate_1240",
        "cate_302",
        "cate_1168",
        "cate_1092",
        "cate_1219",
        "cate_1225",
    ];
    let mut facets = Vec::new();
    let mut push = |query: String| facets.push(query);
    push("sort_type=1".into());
    push("sort_type=2".into());
    push("gender=1&sort_type=1".into());
    push("gender=0&sort_type=1".into());
    for time in 1..=4 {
        push(format!("time={time}&sort_type=1"));
    }
    for cate in BACKGROUNDS {
        push(format!("background={cate}&sort_type=1"));
        push(format!("background={cate}&sort_type=2"));
    }
    for cate in TOPICS {
        push(format!("topic={cate}&sort_type=1"));
    }
    for cate in SETTINGS {
        push(format!("setting={cate}&sort_type=1"));
    }
    facets
}

/// 纯函数：算出下一游标。当前页翻尽 → 下一页；分类面翻尽 → 下一个面；
/// 队尾回卷到 0（无限流：官网目录每日更新，回卷后内容也会轮换）。
fn advance_cursor(idx: usize, page: u32, total_pages: u32, facet_len: usize) -> (usize, u32) {
    let capped = total_pages.clamp(1, MAX_PAGES_PER_FACET);
    if page < capped {
        (idx, page + 1)
    } else if idx + 1 < facet_len {
        (idx + 1, 1)
    } else {
        (0, 1)
    }
}

fn parse_cursor(cursor: Option<&str>, facet_len: usize) -> (usize, u32) {
    let Some(cursor) = cursor
        .map(|value| value.trim().to_owned())
        .filter(|v| !v.is_empty())
    else {
        return (0, 1);
    };
    let mut parts = cursor.splitn(2, ':');
    let idx = parts
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| if facet_len == 0 { 0 } else { value % facet_len })
        .unwrap_or(0);
    let page = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or(1);
    (idx, page)
}

// ============ 网络层 ============

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

/// reqwest 被拦（WAF 验证页/网络问题）时退回 OS curl.exe —— 与 sehuatang 共用的
/// 传输层，会自动探测本地代理；hongguo 无 Cloudflare 门槛，这条通常用不上。
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

/// 抓页面并从 `window._ROUTER_DATA = {…}` 提取 SSR JSON。
async fn fetch_router_data(path: &str) -> Result<Value, String> {
    let url = format!("{HONGGUO_BASE}{path}");
    let body = match fetch_via_reqwest(&url).await {
        Ok(body) => body,
        Err(reqwest_error) => fetch_via_curl(&url).await.map_err(|curl_error| {
            format!("短剧页面请求失败（reqwest：{reqwest_error}；curl：{curl_error}）")
        })?,
    };
    extract_router_data(&body)
        .ok_or_else(|| "页面缺少 _ROUTER_DATA 数据（可能触发了反爬验证）".to_owned())
}

/// 从 SSR HTML 里抽出第一个 `_ROUTER_DATA = {…}` JSON（花括号深度扫描，
/// 感知字符串与转义；JSON 里可能再嵌 `</script>` 之外的任意字符）。
fn extract_router_data(body: &str) -> Option<Value> {
    let marker = body.find("_ROUTER_DATA")?;
    let start = body[marker..].find('{')? + marker;
    let bytes = body.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if in_string {
            match byte {
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_slice(&bytes[start..=start + offset]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

// ============ 卡片映射 ============

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn u32_field(value: &Value, key: &str) -> u32 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0) as u32
}

fn card_from_series(series: &Value) -> Option<ShortDramaCard> {
    let id = string_field(series, "series_id");
    if id.is_empty() {
        return None;
    }
    let title = {
        let name = string_field(series, "series_name");
        if name.is_empty() {
            format!("短剧 {id}")
        } else {
            name
        }
    };
    let tags = series
        .get("tags")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_default();
    let total = u32_field(series, "episode_cnt");
    let intro = string_field(series, "series_intro");
    Some(ShortDramaCard {
        id: id.clone(),
        title,
        cover_url: string_field(series, "series_cover"),
        episodes: {
            let text = string_field(series, "episode_right_text");
            if text.is_empty() {
                if total > 0 {
                    format!("全{total}集")
                } else {
                    "短剧".into()
                }
            } else {
                text
            }
        },
        category: if tags.is_empty() {
            "短剧".into()
        } else {
            tags
        },
        source: "红果短剧".into(),
        source_url: format!("{HONGGUO_BASE}/detail?series_id={id}"),
        description: if intro.is_empty() {
            "来自红果短剧公开目录的短剧条目。".into()
        } else {
            intro
        },
        total_episodes: total,
        playable_episodes: u32_field(series, "accessible_episode_cnt"),
    })
}

/// 相关推荐/目录列表兼容两种形态：JSON 数组或以 "0".."49" 为键的对象。
fn series_list(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(Value::Object(map)) => {
            let mut entries: Vec<(usize, &Value)> = map
                .iter()
                .filter_map(|(key, value)| key.parse::<usize>().ok().map(|index| (index, value)))
                .collect();
            entries.sort_by_key(|(index, _)| *index);
            entries.into_iter().map(|(_, value)| value).collect()
        }
        _ => Vec::new(),
    }
}

// ============ 命令 ============

/// 单个 facet 键值对白名单（防 URL 注入：facet 会拼进请求路径查询串）。
fn facet_param_allowed(key: &str, value: &str) -> bool {
    match key {
        "background" | "setting" | "topic" => {
            value.starts_with("cate_") && value[5..].chars().all(|c| c.is_ascii_digit())
        }
        "gender" => matches!(value, "0" | "1"),
        "time" => matches!(value, "1" | "2" | "3" | "4"),
        "sort_type" => matches!(value, "1" | "2"),
        _ => false,
    }
}

/// 校验并规范化前端传来的 facet 查询串；非法输入回退 None（全部内容池）。
fn sanitize_facet(facet: Option<&str>) -> Option<String> {
    let facet = facet.map(str::trim).filter(|value| !value.is_empty())?;
    let pairs = facet
        .split('&')
        .map(|pair| {
            let mut parts = pair.splitn(2, '=');
            (
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    if pairs.is_empty()
        || pairs
            .iter()
            .any(|(key, value)| !facet_param_allowed(key, value))
    {
        return None;
    }
    // 保证 sort_type 在最后（官网接受任意顺序，这里统一便于游标复用）。
    let mut sorted = pairs;
    sorted.sort_by_key(|(key, _)| *key == "sort_type");
    Some(
        sorted
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&"),
    )
}

#[tauri::command]
pub async fn short_drama_stream(
    input: ShortDramaStreamInput,
) -> Result<ShortDramaStreamPage, String> {
    let mut facets = facet_queries();
    if let Some(facet) = sanitize_facet(input.facet.as_deref()) {
        // 指定分类面：只用该面（热门/最新两种排序铺页），游标逻辑不变。
        facets = vec![
            format!("{facet}&sort_type=1"),
            format!("{facet}&sort_type=2"),
        ];
    }
    let (mut idx, mut page) = parse_cursor(input.cursor.as_deref(), facets.len());
    for _ in 0..MAX_FETCHES_PER_CALL {
        // 官网目录第 1 页 = 不带 page 参数（page=1 会被 301 到不存在的
        // /category/category 路径），分页参数从 page=2 开始。
        let path = if page <= 1 {
            format!("/category?{}", facets[idx])
        } else {
            format!("/category?{}&page={page}", facets[idx])
        };
        let Ok(data) = fetch_router_data(&path).await else {
            // 单面抓取失败：跳到下一个面重试；连败几次则交还给前端稍后再取。
            (idx, page) = ((idx + 1) % facets.len(), 1);
            continue;
        };
        let category_page = data
            .pointer("/loaderData/category_page")
            .cloned()
            .unwrap_or(Value::Null);
        let batch = series_list(category_page.get("recommendList"))
            .iter()
            .filter_map(|series| card_from_series(series))
            .collect::<Vec<_>>();
        if batch.is_empty() {
            (idx, page) = ((idx + 1) % facets.len(), 1);
            continue;
        }
        let total_pages = category_page
            .pointer("/pagination/totalPages")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        let (next_idx, next_page) = advance_cursor(idx, page, total_pages, facets.len());
        return Ok(ShortDramaStreamPage {
            items: batch,
            next_cursor: Some(format!("{next_idx}:{next_page}")),
            source: "红果短剧官网".into(),
            fetched_at: chrono::Utc::now().timestamp(),
        });
    }
    // 连续 6 个面都拿不到数据（网络异常或目录真被清空）→ 让前端稍后重试。
    Ok(ShortDramaStreamPage {
        items: Vec::new(),
        next_cursor: Some(format!("{idx}:{page}")),
        source: "红果短剧官网".into(),
        fetched_at: chrono::Utc::now().timestamp(),
    })
}

#[tauri::command]
pub async fn short_drama_detail(input: ShortDramaDetailInput) -> Result<ShortDramaDetail, String> {
    let series_id = input.series_id.trim();
    if series_id.is_empty() || !series_id.chars().all(|c| c.is_ascii_digit()) {
        return Err("缺少有效的短剧 ID。".into());
    }
    let data = fetch_router_data(&format!("/detail?series_id={series_id}")).await?;
    let detail_page = data
        .pointer("/loaderData/detail_page")
        .filter(|value| value.is_object())
        .ok_or_else(|| "详情页数据为空。".to_owned())?;
    let series = detail_page
        .get("seriesDetail")
        .cloned()
        .unwrap_or(Value::Null);
    let card = card_from_series(&series).ok_or_else(|| "详情页缺少剧集信息。".to_owned())?;
    let cast = series
        .get("celebrities")
        .and_then(Value::as_array)
        .map(|members| {
            members
                .iter()
                .map(|member| ShortDramaCastMember {
                    name: string_field(member, "nickname"),
                    role: string_field(member, "sub_title"),
                })
                .filter(|member| !member.name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let vids = series
        .get("vid_list")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let intro = string_field(&series, "series_intro");
    let tags = series
        .get("tags")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let recommendations = series_list(detail_page.get("videoList"))
        .iter()
        .filter_map(|series| card_from_series(series))
        .collect();
    Ok(ShortDramaDetail {
        id: card.id.clone(),
        title: card.title.clone(),
        cover_url: card.cover_url.clone(),
        intro: if intro.is_empty() {
            card.description.clone()
        } else {
            intro
        },
        tags,
        episodes_text: card.episodes.clone(),
        total_episodes: card.total_episodes,
        playable_episodes: card.playable_episodes,
        vids,
        cast,
        recommendations,
        source_url: card.source_url.clone(),
    })
}

#[tauri::command]
pub async fn short_drama_play(input: ShortDramaPlayInput) -> Result<ShortDramaPlayback, String> {
    let series_id = input.series_id.trim();
    let vid = input.vid.trim();
    if series_id.is_empty() || !series_id.chars().all(|c| c.is_ascii_digit()) {
        return Err("缺少有效的短剧 ID。".into());
    }
    let data = fetch_router_data(&format!("/player/{series_id}/{vid}")).await?;
    let page_payload = data
        .get("loaderData")
        .and_then(Value::as_object)
        .and_then(|loader| {
            loader
                .values()
                .find(|value| {
                    value
                        .get("video_player_info")
                        .map(|info| info.is_object())
                        .unwrap_or(false)
                })
                .cloned()
                .or_else(|| {
                    loader
                        .iter()
                        .find(|(key, value)| key.contains("page") && value.is_object())
                        .map(|(_, value)| value.clone())
                })
        })
        .ok_or_else(|| "播放页数据为空，该集可能仅限红果 App 内观看。".to_owned())?;
    let player_info = page_payload
        .get("video_player_info")
        .cloned()
        .unwrap_or(Value::Null);
    let url = string_field(&player_info, "main_url");
    if url.is_empty() {
        let accessible = page_payload
            .pointer("/seriesDetail/accessible_episode_cnt")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let episodes_text = if accessible > 0 {
            format!("官网网页端仅开放第 1-{accessible} 集，本集需在红果 App 内观看。")
        } else {
            "该集需要在红果 App 内观看（网页端未开放直链）。".into()
        };
        return Err(episodes_text);
    }
    let series = page_payload
        .get("seriesDetail")
        .cloned()
        .unwrap_or(Value::Null);
    let vids: Vec<String> = series
        .get("vid_list")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let episode = vids
        .iter()
        .position(|candidate| candidate == vid)
        .map(|index| (index + 1) as u32)
        .unwrap_or(0);
    Ok(ShortDramaPlayback {
        series_id: series_id.to_owned(),
        vid: vid.to_owned(),
        url,
        poster_url: string_field(&player_info, "poster_url"),
        duration_seconds: player_info
            .get("duration")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        width: u32_field(&player_info, "width"),
        height: u32_field(&player_info, "height"),
        series_name: string_field(&series, "series_name"),
        cover_url: string_field(&series, "series_cover"),
        episode,
        total_episodes: vids.len() as u32,
        playable_episodes: u32_field(&series, "accessible_episode_cnt"),
        next_vid: vids.get(episode as usize).cloned(),
        vids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_router_data_with_nested_quotes() {
        let body = r#"<script>_ROUTER_DATA = {"loaderData":{"detail_page":{"seriesDetail":{"series_name":"他说：\"好\"","tags":["战{}神"]}}}};</script>"#;
        let data = extract_router_data(body).expect("router data");
        assert_eq!(
            data.pointer("/loaderData/detail_page/seriesDetail/series_name")
                .and_then(Value::as_str),
            Some("他说：\"好\""),
        );
        assert_eq!(
            data.pointer("/loaderData/detail_page/seriesDetail/tags/0")
                .and_then(Value::as_str),
            Some("战{}神"),
        );
    }

    #[test]
    fn extracts_router_data_returns_none_without_marker() {
        assert!(extract_router_data("<html>no data</html>").is_none());
    }

    #[test]
    fn cards_from_array_and_object_forms() {
        let series = serde_json::json!({
            "series_id": "123",
            "series_name": "示例短剧",
            "series_cover": "https://img.test/a.jpg",
            "series_intro": "简介",
            "episode_cnt": 12,
            "accessible_episode_cnt": 3,
            "tags": ["都市", "爱情"],
            "episode_right_text": "全12集"
        });
        let as_array = serde_json::json!([series]);
        let cards_array = series_list(Some(&as_array));
        assert_eq!(cards_array.len(), 1);
        let card = card_from_series(cards_array[0]).unwrap();
        assert_eq!(card.id, "123");
        assert_eq!(card.episodes, "全12集");
        assert_eq!(card.playable_episodes, 3);
        assert_eq!(card.total_episodes, 12);
        assert!(card.source_url.ends_with("series_id=123"));

        let as_object = serde_json::json!({ "0": series, "1": series });
        let cards_object = series_list(Some(&as_object));
        assert_eq!(cards_object.len(), 2);
    }

    #[test]
    fn detail_parse_reaches_series_and_recommendations() {
        let series = serde_json::json!({
            "series_id": "123", "series_name": "示例短剧", "series_cover": "c",
            "series_intro": "剧情简介", "episode_cnt": 20, "accessible_episode_cnt": 3,
            "tags": ["都市"], "episode_right_text": "全20集",
            "vid_list": ["v1", "v2", "v3", "v4"],
            "celebrities": [{"nickname": "演员甲", "sub_title": "饰 主角"}]
        });
        let page = serde_json::json!({
            "loaderData": {
                "detail_layout": null,
                "detail_page": { "isSuccess": true, "seriesDetail": series, "videoList": {"0": series} }
            }
        });
        let detail_page = page.pointer("/loaderData/detail_page").unwrap();
        let recs = series_list(detail_page.get("videoList"));
        assert_eq!(recs.len(), 1);
        assert_eq!(card_from_series(recs[0]).unwrap().id, "123");
        let vids: Vec<String> = series
            .get("vid_list")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        assert_eq!(vids, vec!["v1", "v2", "v3", "v4"]);
    }

    #[test]
    fn play_payload_missing_video_player_info_is_detectable() {
        let page = serde_json::json!({
            "loaderData": {
                "player_layout": null,
                "player_page": { "isSuccess": true, "seriesDetail": {"accessible_episode_cnt": 3} }
            }
        });
        let loader = page.get("loaderData").unwrap();
        assert!(loader
            .as_object()
            .map(|map| map
                .values()
                .all(|value| value.get("video_player_info").is_none()))
            .unwrap_or(false));
    }

    #[test]
    fn cursor_advances_pages_then_facets_then_wraps() {
        // 页未翻尽 → 下一页
        assert_eq!(advance_cursor(3, 5, 21, 10), (3, 6));
        // 页翻尽 → 下一面
        assert_eq!(advance_cursor(3, 21, 21, 10), (4, 1));
        // 超大 totalPages 被钳制
        assert_eq!(advance_cursor(3, 40, 9999, 10), (4, 1));
        // 队尾回卷
        assert_eq!(advance_cursor(9, 30, 30, 10), (0, 1));
        // 面只支持 1 页时直接跳面
        assert_eq!(advance_cursor(2, 1, 1, 10), (3, 1));
    }

    #[test]
    fn cursor_parses_and_wraps_input() {
        let facets = facet_queries();
        assert!(!facets.is_empty());
        assert_eq!(parse_cursor(None, facets.len()), (0, 1));
        assert_eq!(parse_cursor(Some("5:7"), facets.len()), (5, 7));
        assert_eq!(
            parse_cursor(Some("9999:2"), facets.len()),
            (9999 % facets.len(), 2)
        );
        assert_eq!(parse_cursor(Some("garbage"), facets.len()), (0, 1));
        assert_eq!(parse_cursor(Some("3:0"), facets.len()), (3, 1));
    }

    #[test]
    fn facet_param_whitelist_rejects_injection() {
        assert_eq!(
            sanitize_facet(Some("background=cate_1&sort_type=2")).as_deref(),
            Some("background=cate_1&sort_type=2")
        );
        assert_eq!(
            sanitize_facet(Some("gender=1")).as_deref(),
            Some("gender=1")
        );
        // 组合键重排：sort_type 挪到最后
        assert_eq!(
            sanitize_facet(Some("sort_type=2&topic=cate_165")).as_deref(),
            Some("topic=cate_165&sort_type=2")
        );
        // 非法键 / 非法值 / 任意路径注入
        assert_eq!(sanitize_facet(Some("evil=1")), None);
        assert_eq!(sanitize_facet(Some("background=../admin")), None);
        assert_eq!(sanitize_facet(Some("background=cate_x&sort_type=1")), None);
        assert_eq!(sanitize_facet(Some("")), None);
        assert_eq!(sanitize_facet(None), None);
    }
}
