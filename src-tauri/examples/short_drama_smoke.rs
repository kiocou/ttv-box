//! 端到端验证短剧三命令（真实网络，不碰应用数据库）：
//! 1. `short_drama_stream`：默认内容池连续拉 3 页，校验卡片字段与游标推进；
//! 2. facet 过滤：`background=cate_1` 拉一页，校验全部卡片带“都市”类标签；
//! 3. `short_drama_detail`：取第一部剧的详情（vid_list/演员/推荐）；
//! 4. `short_drama_play`：解析第 1 集直链，并 HEAD 下载前 1KB 验证可播 mp4。
//!
//! Usage:
//!   cargo run --example short_drama_smoke
//!   HTTPS_PROXY=socks5h://127.0.0.1:10808 cargo run --example short_drama_smoke

use ttv_backend::short_drama::{
    short_drama_detail, short_drama_play, short_drama_stream, ShortDramaDetailInput,
    ShortDramaPlayInput, ShortDramaStreamInput,
};

#[tokio::main]
async fn main() {
    let mut failures = 0;

    // ---- 1. 无限流：连续 3 页 ----
    let mut cursor: Option<String> = None;
    let mut seen_ids = std::collections::HashSet::new();
    let mut first_series: Option<String> = None;
    for round in 1..=3 {
        match short_drama_stream(ShortDramaStreamInput {
            cursor: cursor.clone(),
            facet: None,
        })
        .await
        {
            Ok(page) => {
                println!(
                    "[stream {round}] items={} next_cursor={:?}",
                    page.items.len(),
                    page.next_cursor
                );
                if page.items.is_empty() {
                    println!("  !! 页面为空");
                    failures += 1;
                }
                for card in &page.items {
                    if card.title.is_empty() || card.cover_url.is_empty() {
                        println!("  !! 卡片字段缺失：{} / {}", card.title, card.cover_url);
                        failures += 1;
                    }
                    if !seen_ids.insert(card.id.clone()) {
                        println!("  !! 重复卡片：{} {}", card.id, card.title);
                        failures += 1;
                    }
                    println!(
                        "  - {} | {} | {} | 可播 {}/{}",
                        card.title,
                        card.episodes,
                        card.category,
                        card.playable_episodes,
                        card.total_episodes
                    );
                }
                if first_series.is_none() {
                    if let Some(card) = page.items.first() {
                        first_series = Some(card.id.clone());
                    }
                }
                cursor = page.next_cursor;
            }
            Err(error) => {
                println!("[stream {round}] 失败：{error}");
                failures += 1;
                break;
            }
        }
    }

    // ---- 2. facet 过滤 ----
    match short_drama_stream(ShortDramaStreamInput {
        cursor: None,
        facet: Some("background=cate_1".into()),
    })
    .await
    {
        Ok(page) => {
            println!("[facet 都市] items={}", page.items.len());
            for card in page.items.iter().take(3) {
                println!("  - {} | {}", card.title, card.category);
            }
        }
        Err(error) => {
            println!("[facet] 失败：{error}");
            failures += 1;
        }
    }

    // ---- 3. 详情 ----
    let Some(series_id) = first_series else {
        println!("!! 没有可用系列，跳过详情/播放验证");
        std::process::exit(if failures > 0 { 1 } else { 0 });
    };
    let detail = match short_drama_detail(ShortDramaDetailInput {
        series_id: series_id.clone(),
    })
    .await
    {
        Ok(detail) => {
            println!(
                "[detail] {} | {} | {} | 集数 {} 可播 {} | vids={} | 演员={} | 推荐={}",
                detail.title,
                detail.id,
                detail.intro.chars().take(24).collect::<String>(),
                detail.episodes_text,
                detail.playable_episodes,
                detail.vids.len(),
                detail.cast.len(),
                detail.recommendations.len()
            );
            if detail.vids.is_empty() || detail.playable_episodes == 0 {
                println!("  !! 详情缺少可播集信息");
                failures += 1;
            }
            Some(detail)
        }
        Err(error) => {
            println!("[detail] 失败：{error}");
            failures += 1;
            None
        }
    };

    // ---- 4. 播放直链 + 可下载性 ----
    if let Some(detail) = detail {
        let vid = detail.vids.first().cloned().unwrap_or_default();
        match short_drama_play(ShortDramaPlayInput {
            series_id: series_id.clone(),
            vid,
        })
        .await
        {
            Ok(playback) => {
                println!(
                    "[play] 第{}集/共{}集 | {}x{} | {:.1}s | url_len={}",
                    playback.episode,
                    playback.total_episodes,
                    playback.width,
                    playback.height,
                    playback.duration_seconds,
                    playback.url.len()
                );
                println!("  next_vid={:?}", playback.next_vid);
                let client = reqwest::Client::builder()
                    .user_agent("TTV Box short-drama smoke")
                    .build()
                    .expect("client");
                let response = client
                    .get(&playback.url)
                    .header("Range", "bytes=0-1023")
                    .send()
                    .await
                    .expect("range request");
                let status = response.status().as_u16();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                let body = response.bytes().await.expect("body");
                println!(
                    "  下载校验：HTTP {status} {content_type} {} bytes",
                    body.len()
                );
                let magic_ok = body.len() >= 12 && &body[4..8] == b"ftyp";
                if status != 206 || !content_type.starts_with("video/") || !magic_ok {
                    println!(
                        "  !! 直链不可播：status={status} type={content_type} magic_ok={magic_ok}"
                    );
                    failures += 1;
                } else {
                    println!("  ✓ mp4 直链可播（Range 206 + video/mp4 + ftyp 头）");
                }
                // 第 3 集（官网放开的边界）也应可解析
                if let Some(third) = detail.vids.get(2) {
                    match short_drama_play(ShortDramaPlayInput {
                        series_id: series_id.clone(),
                        vid: third.clone(),
                    })
                    .await
                    {
                        Ok(ep3) => println!("  ✓ 第 3 集直链解析成功 episode={}", ep3.episode),
                        Err(error) => println!("  !! 第 3 集解析失败：{error}"),
                    }
                }
                // 超出开放范围的集应报错而不是给直链
                if (detail.playable_episodes as usize) < detail.vids.len() {
                    let locked = detail.vids[detail.playable_episodes as usize].clone();
                    if let Ok(playback) = short_drama_play(ShortDramaPlayInput {
                        series_id,
                        vid: locked,
                    })
                    .await
                    {
                        println!("  !! 未开放的集居然返回了直链：{}", playback.url.len());
                        failures += 1;
                    } else {
                        println!("  ✓ 未开放集正确拒绝");
                    }
                }
            }
            Err(error) => {
                println!("[play] 失败：{error}");
                failures += 1;
            }
        }
    }

    println!("\n=== smoke 结束：{failures} 项失败 ===");
    if failures > 0 {
        std::process::exit(1);
    }
}
