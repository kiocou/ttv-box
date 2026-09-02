//! Diagnostic probe: reproduces the exact provider calls behind the player's
//! subtitle search and quality switch against the user's real saved session.
//! Read-only: never refreshes or writes the stored credential.
//!
//! Usage: cargo run --example guangya_probe [-- <data_dir>]

use ttv_backend::config::AppConfig;
use ttv_backend::providers::{
    GuangyaProvider, MediaProvider, PlaybackRequest, ProviderSubtitleSearchRequest, Session,
};
use ttv_backend::security::CredentialStore;
use ttv_backend::storage::{Database, MediaFilter};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let data_dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("TTV_DATA_DIR").map(std::path::PathBuf::from))
        .unwrap_or_else(|| dirs_data().expect("no data dir; pass one as argv[1]"));
    let db_path = data_dir.join("ttv.db");
    println!("== data dir: {}", data_dir.display());

    let database = Database::open(&db_path).expect("open ttv.db");
    let session: Session = CredentialStore::new(&database)
        .load_json("provider.session.guangya")
        .expect("load credential")
        .unwrap_or_else(|| panic!("no saved guangya session in {}", db_path.display()));
    println!(
        "== session: account={:?} expires_at={:?} (now {})",
        session.account_id,
        session.expires_at,
        chrono_now()
    );

    let config = AppConfig::load(&data_dir).unwrap_or_default();
    let provider = GuangyaProvider::new(config.guangya.clone()).expect("build guangya provider");
    provider
        .restore_session(session.clone())
        .await
        .expect("restore session");

    // Pick a guangya media record the same way the library feeds the player.
    let records = database
        .list_media(
            MediaFilter {
                account_id: None,
                library_id: None,
                kind: None,
            },
            500,
            0,
        )
        .expect("list media");
    let media = records
        .iter()
        .find(|record| {
            record.id.starts_with("provider:guangya:")
                || record
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("providerId"))
                    .and_then(|value| value.as_str())
                    .is_some_and(|value| value == "guangya")
        })
        .unwrap_or_else(|| panic!("no guangya media among {} records", records.len()));
    scan_quality_spread(
        &session.access_token,
        &config.guangya.api_base_url,
        &records,
    )
    .await;
    println!(
        "== media: id={} title={} remote_path={:?}",
        media.id, media.title, media.remote_path
    );
    let media_id = media
        .payload
        .as_ref()
        .and_then(|payload| payload.get("providerMediaId"))
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .or_else(|| {
            media
                .id
                .strip_prefix("provider:guangya:")
                .map(str::to_owned)
        })
        .expect("resolve providerMediaId");
    println!("== media_id given to provider calls: {media_id}");

    // --- 1. subtitle search: the exact call behind 字幕弹窗 → 搜索云盘字幕 ---
    println!("\n== search_subtitles(media_id, name=None, duration=None)");
    match provider
        .search_subtitles(ProviderSubtitleSearchRequest {
            media_id: media_id.clone(),
            name: None,
            duration_seconds: None,
        })
        .await
    {
        Ok(subtitles) => {
            println!("   ok: {} subtitles", subtitles.len());
            for subtitle in subtitles.iter().take(10) {
                println!(
                    "   - {} source={} ext={} url?{} fileId?{}",
                    subtitle.name,
                    subtitle.source,
                    subtitle.ext,
                    subtitle.url.is_some(),
                    subtitle.file_id.is_some()
                );
            }
        }
        Err(error) => println!("   ERROR: {error:?}"),
    }

    // Also try with the file name, mirroring what the real player knows.
    let file_name = media
        .payload
        .as_ref()
        .and_then(|payload| payload.get("sourceTitle"))
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    println!("\n== search_subtitles(name={file_name:?})");
    match provider
        .search_subtitles(ProviderSubtitleSearchRequest {
            media_id: media_id.clone(),
            name: file_name.clone(),
            duration_seconds: media.duration_seconds.map(|value| value as f64),
        })
        .await
    {
        Ok(subtitles) => println!("   ok: {} subtitles", subtitles.len()),
        Err(error) => println!("   ERROR: {error:?}"),
    }

    // --- 2. quality switch: resolve twice with different gcids, compare URLs ---
    // Prefer a multi-variant file so the switch is observable.
    let target_record = records
        .iter()
        .find(|record| record.title.contains("PTGF"))
        .unwrap_or(media);
    let target_media_id = target_record
        .id
        .strip_prefix("provider:guangya:")
        .map(str::to_owned)
        .or_else(|| {
            target_record
                .payload
                .as_ref()
                .and_then(|payload| payload.get("providerMediaId"))
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| media_id.clone());
    println!(
        "\n== target for quality-switch test: {} ({})",
        target_record.title, target_media_id
    );

    println!("\n== video_qualities(target)");
    let qualities = match provider.video_qualities(&target_media_id).await {
        Ok(qualities) => {
            for quality in &qualities {
                println!(
                    "   - gcid={} displayName={:?} resolutionName={:?} shortName={:?} needVipType={:?} definitionId={} default={}",
                    quality.gcid, quality.display_name, quality.resolution_name, quality.short_name, quality.need_vip_type, quality.definition_id, quality.is_default
                );
            }
            qualities
        }
        Err(error) => {
            println!("   ERROR: {error:?}");
            Vec::new()
        }
    };

    println!("\n== resolve_playback(quality=None)");
    match provider
        .resolve_playback(PlaybackRequest {
            media_id: target_media_id.clone(),
            quality: None,
        })
        .await
    {
        Ok(descriptor) => {
            println!("   url: {}", shorten(&descriptor.url));
            println!("   quality: {:?}", descriptor.quality);
            println!(
                "   qualities returned: {}",
                descriptor.qualities.as_ref().map(Vec::len).unwrap_or(0)
            );
        }
        Err(error) => println!("   ERROR: {error:?}"),
    }

    for quality in qualities.iter().take(4) {
        println!("\n== resolve_playback(quality=gcid {})", quality.gcid);
        match provider
            .resolve_playback(PlaybackRequest {
                media_id: target_media_id.clone(),
                quality: Some(quality.gcid.clone()),
            })
            .await
        {
            Ok(descriptor) => {
                println!("   url: {}", shorten(&descriptor.url));
                println!("   quality: {:?}", descriptor.quality);
            }
            Err(error) => println!("   ERROR: {error:?}"),
        }
    }

    // --- 3. raw API dumps: see exactly what guangya returns before parsing ---
    raw_post(
        &session.access_token,
        &config.guangya.api_base_url,
        "/userres/v1/file/get_file_detail",
        serde_json::json!({"fileId": media_id}),
    )
    .await;

    let gcid_for_subtitles = qualities
        .first()
        .map(|quality| quality.gcid.clone())
        .unwrap_or_default();
    let duration = media.duration_seconds.map(|value| value as f64);
    raw_post(
        &session.access_token,
        &config.guangya.api_base_url,
        "/misc/v1/get_subtitles",
        serde_json::json!({
            "gcid": gcid_for_subtitles,
            "name": file_name.clone().unwrap_or_default(),
            "duration": duration.map(|value| value.floor() as i64).unwrap_or(0),
        }),
    )
    .await;

    // Same call with the backend's fallback values: real fileName + probed duration.
    raw_post(
        &session.access_token,
        &config.guangya.api_base_url,
        "/misc/v1/get_subtitles",
        serde_json::json!({
            "gcid": gcid_for_subtitles,
            "name": "#00年小可爱_12698.mp4",
            "duration": 1782,
        }),
    )
    .await;
}

async fn raw_post(token: &str, api_base: &str, path: &str, body: serde_json::Value) {
    println!("\n== RAW POST {path} {body}");
    let trace_id = uuid::Uuid::new_v4().simple().to_string();
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/{}", api_base.trim_end_matches('/'), path.trim_start_matches('/')))
        .bearer_auth(token)
        .header("accept", "application/json, text/plain, */*")
        .header("content-type", "application/json")
        .header("origin", "https://www.guangyapan.com")
        .header("referer", "https://www.guangyapan.com/")
        .header("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/147.0.0.0 Safari/537.36")
        .header("dt", "4")
        .header("did", uuid::Uuid::new_v4().to_string())
        .header("traceparent", format!("00-{trace_id}-{}-01", &trace_id[..16]))
        .json(&body)
        .send()
        .await
        .expect("raw POST");
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    println!("   status: {status}");
    println!("   body: {}", &text[..text.len().min(4000)]);
}

fn shorten(url: &str) -> String {
    if url.len() <= 110 {
        url.to_owned()
    } else {
        format!("{}…<{} chars>", &url[..110], url.len())
    }
}

/// Count how many distinct quality variants guangya exposes for a sample of
/// library files: tells us whether "only one option in the quality menu" is a
/// data reality or a parsing bug.
async fn scan_quality_spread(
    token: &str,
    api_base: &str,
    records: &[ttv_backend::storage::MediaRecord],
) {
    use std::collections::BTreeMap;
    let mut spread: BTreeMap<usize, usize> = BTreeMap::new();
    let mut multi: Vec<(String, Vec<String>)> = Vec::new();
    let mut checked = 0usize;
    for record in records
        .iter()
        .filter(|record| record.id.starts_with("provider:guangya:"))
        .take(12)
    {
        let media_id = match record.id.strip_prefix("provider:guangya:") {
            Some(id) => id.to_owned(),
            None => continue,
        };
        let body = match raw_post_json(
            token,
            api_base,
            "/userres/v1/file/get_file_detail",
            serde_json::json!({"fileId": media_id}),
        )
        .await
        {
            Some(body) => body,
            None => continue,
        };
        let resources = body
            .pointer("/data/videoResource")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let names: Vec<String> = resources
            .iter()
            .map(|resource| {
                resource
                    .pointer("/info/resolutionName")
                    .and_then(|value| value.as_str())
                    .unwrap_or("?")
                    .to_owned()
            })
            .collect();
        *spread.entry(resources.len()).or_insert(0) += 1;
        if resources.len() > 1 && multi.len() < 3 {
            multi.push((record.title.clone(), names.clone()));
        }
        checked += 1;
    }
    println!("\n== quality spread over {checked} files (videoResource length → count): {spread:?}");
    for (title, names) in multi {
        println!("   multi: {title} → {names:?}");
    }
}

async fn raw_post_json(
    token: &str,
    api_base: &str,
    path: &str,
    body: serde_json::Value,
) -> Option<serde_json::Value> {
    let trace_id = uuid::Uuid::new_v4().simple().to_string();
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/{}", api_base.trim_end_matches('/'), path.trim_start_matches('/')))
        .bearer_auth(token)
        .header("accept", "application/json, text/plain, */*")
        .header("content-type", "application/json")
        .header("origin", "https://www.guangyapan.com")
        .header("referer", "https://www.guangyapan.com/")
        .header("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/147.0.0.0 Safari/537.36")
        .header("dt", "4")
        .header("did", uuid::Uuid::new_v4().to_string())
        .header("traceparent", format!("00-{trace_id}-{}-01", &trace_id[..16]))
        .json(&body)
        .send()
        .await
        .ok()?;
    response.json::<serde_json::Value>().await.ok()
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

fn dirs_data() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|base| std::path::PathBuf::from(base).join("com.ttv.player"))
}
