//! End-to-end verification of the library scrape pipeline on a real
//! (temporary) database. Seeds rows with real-world file names, runs the same
//! `scrape_media` path as the `library_scrape` command, then reports what
//! landed: Chinese titles via douban, JAV metadata via the six adult sources,
//! cover downloads, and 18+ isolation.
//!
//! Usage:
//!   cargo run --example library_scrape_smoke
//!   HTTPS_PROXY=socks5h://127.0.0.1:10808 cargo run --example library_scrape_smoke
//!
//! Uses a throwaway database under the OS temp dir; the real app database is
//! never touched.

use std::sync::Arc;

use ttv_backend::metadata::{scrape_media, ScrapeOptions};
use ttv_backend::storage::{Database, MediaRecord};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();
    let tmp = std::env::temp_dir().join(format!("ttv-scrape-smoke-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let cover_dir = tmp.join("covers");
    let database = Arc::new(Database::open(tmp.join("ttv.db")).expect("open temp db"));

    // (file name, seed title). The JAV case must route through the adult
    // sources; the CJK case must come back with a Chinese douban match; the
    // ASCII case exercises the generic western path.
    let cases: Vec<(&str, &str)> = vec![
        ("SIRO-5658 初撮り五十路妻ドキュメント.mp4", "SIRO-5658"),
        ("千与千寻 2001 1080p.mkv", "千与千寻"),
        ("Shrek (2001).mp4", "Shrek"),
    ];

    let mut media = Vec::new();
    for (index, (file, title)) in cases.iter().enumerate() {
        let mut item = MediaRecord::new(format!("smoke-{index}"), "video", *title);
        item.remote_path = Some(format!("D:\\Media\\{file}"));
        database.upsert_media(&item).expect("insert seed row");
        media.push(item);
    }

    let report = scrape_media(
        &database,
        media,
        ScrapeOptions {
            providers: vec!["douban".into(), "tvmaze".into(), "jav".into()],
            overwrite: false,
            include_adult: true,
            cover_dir: Some(cover_dir.clone()),
            cancel: None,
            jav_scope: ttv_backend::adult::JavScope::Full,
        },
        None,
    )
    .await
    .expect("scrape_media failed");

    println!(
        "report: requested={} matched={} updated={} unmatched={} covers={} adult_isolated={}",
        report.requested,
        report.matched,
        report.updated,
        report.unmatched,
        report.covers,
        report.adult_isolated
    );

    let mut failures = Vec::new();
    for (index, (file, _)) in cases.iter().enumerate() {
        let item = database
            .get_media(&format!("smoke-{index}"))
            .expect("get_media")
            .expect("row present");
        let payload = item.payload.clone().unwrap_or(serde_json::Value::Null);
        let scraped_by = payload
            .get("scrapedBy")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let metadata_source = payload
            .get("metadataSource")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let summary = payload
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .chars()
            .take(60)
            .collect::<String>();
        let adult = payload
            .get("adult")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let art_name = item
            .art_url
            .as_deref()
            .map(std::path::Path::new)
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "NONE".into());
        println!("#{index} [{file}]");
        println!("   title:    {}", item.title);
        println!("   year:     {:?}", item.year);
        println!("   rating:   {:?}", item.rating);
        println!("   source:   scrapedBy={scraped_by:?} metadataSource={metadata_source:?} adult={adult}");
        println!("   art:      {art_name}");
        println!("   summary:  {summary}");

        let is_jav_case = index == 0;
        if is_jav_case && !adult {
            failures.push("SIRO-5658 was not isolated as 18+".into());
        }
        if is_jav_case && art_name == "NONE" {
            failures.push("SIRO-5658 has no downloaded cover".into());
        }
        if index != 0 && adult {
            failures.push(format!("{} wrongly flagged adult", file));
        }
        if index == 1 && !item.title.contains("千与千寻") {
            failures.push("douban CJK match did not keep the Chinese title".into());
        }
    }

    let cover_files: Vec<_> = std::fs::read_dir(&cover_dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    println!("covers on disk: {cover_files:?}");

    if !failures.is_empty() {
        for failure in &failures {
            println!("FAIL: {failure}");
        }
        std::process::exit(1);
    }
    println!("ALL CHECKS PASSED (temp db at {})", tmp.display());
    // Keep the temp dir for post-mortem inspection; the OS cleans temp.
}
