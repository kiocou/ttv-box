//! Local media library scanning.
//!
//! Scanning is intentionally provider-agnostic. Remote providers populate the
//! same `media` table through their own paging jobs, while this module handles
//! local filesystem roots only.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::error::AppError;
use crate::metadata::{apply_adult_classification, set_manual_adult};
use crate::storage::{Database, MediaRecord};

const VIDEO_EXTENSIONS: &[&str] = &[
    "avi", "flv", "m2ts", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "rm", "rmvb", "ts", "webm",
    "wmv",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub root: String,
    pub scanned_files: u64,
    pub imported: u64,
    pub skipped: u64,
    pub skipped_promotional: u64,
    pub skipped_non_video: u64,
    pub errors: u64,
}

// `cancel` 持有 Arc<AtomicBool>，不满足 Copy；这里只按引用传递，Clone 足够。
#[derive(Clone)]
pub struct ScanOptions {
    pub max_files: u64,
    /// Explicit 18+ choice for the whole batch, applied as a *manual* decision
    /// (`adultManual`) so it overrides the heuristic and survives later
    /// scraping / reclassify sweeps. `None` falls back to auto-classification.
    pub mark_adult: Option<bool>,
    /// Cooperative cancellation flag. Checked between directories and between
    /// files; when set the scan stops and returns the partial report.
    pub cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Live progress callback (scanned_files, imported, skipped,
    /// skipped_promotional, skipped_non_video), invoked once
    /// per directory and once after the final flush, so the frontend scan card
    /// shows moving counters instead of a stuck zero.
    pub progress: Option<Arc<dyn Fn(u64, u64, u64, u64, u64) + Send + Sync>>,
}

impl std::fmt::Debug for ScanOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanOptions")
            .field("max_files", &self.max_files)
            .field("mark_adult", &self.mark_adult)
            .field("cancel", &self.cancel)
            .field("progress", &self.progress.as_ref().map(|_| "fn"))
            .finish()
    }
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_files: 100_000,
            mark_adult: None,
            cancel: None,
            progress: None,
        }
    }
}

pub fn scan_directory(
    database: &Arc<Database>,
    root: impl AsRef<Path>,
    options: ScanOptions,
) -> Result<ScanReport, AppError> {
    let root = root.as_ref();
    if !root.exists() {
        return Err(AppError::NotFound(format!(
            "scan root does not exist: {}",
            root.display()
        )));
    }
    if !root.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "scan root is not a directory: {}",
            root.display()
        )));
    }

    let root = root
        .canonicalize()
        .map_err(|error| AppError::Storage(format!("cannot resolve scan root: {error}")))?;
    let root_display = root.to_string_lossy().into_owned();
    let mut report = ScanReport {
        root: root_display,
        scanned_files: 0,
        imported: 0,
        skipped: 0,
        skipped_promotional: 0,
        skipped_non_video: 0,
        errors: 0,
    };
    let mut pending = vec![root];
    let mut media_batch = Vec::with_capacity(128);

    while let Some(directory) = pending.pop() {
        if options
            .cancel
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
        {
            break;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                report.errors += 1;
                continue;
            }
        };

        for entry in entries {
            if options
                .cancel
                .as_ref()
                .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
            {
                break;
            }
            if report.scanned_files >= options.max_files {
                if !media_batch.is_empty() {
                    match database.upsert_media_batch(&media_batch) {
                        Ok(()) => report.imported += media_batch.len() as u64,
                        Err(_) => report.errors += media_batch.len() as u64,
                    }
                }
                return Ok(report);
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    report.errors += 1;
                    continue;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    report.errors += 1;
                    continue;
                }
            };
            if file_type.is_dir() {
                if is_promotional_path(path.to_string_lossy().as_ref()) {
                    report.skipped += 1;
                    report.skipped_promotional += 1;
                    continue;
                }
                pending.push(path);
                continue;
            }
            // Do not follow symlinks: this avoids cycles and keeps a scan inside
            // the user-selected root.
            if !file_type.is_file() {
                continue;
            }

            report.scanned_files += 1;
            if is_promotional_path(path.to_string_lossy().as_ref()) {
                report.skipped += 1;
                report.skipped_promotional += 1;
                continue;
            }
            if !is_video_file(&path) {
                report.skipped += 1;
                report.skipped_non_video += 1;
                continue;
            }

            let title = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("Untitled")
                .to_owned();
            let path_text = path.to_string_lossy().into_owned();
            let episode = parse_episode_identity(&title);
            let kind = episode.is_some().then_some("episode").unwrap_or("video");
            let mut media = MediaRecord::new(local_media_id(&path_text), kind, title.clone());
            media.source_type = "local".to_owned();
            media.remote_path = Some(path_text);
            media.sort_key = Some(match episode {
                Some((ref series, season, number)) => {
                    format!("{series} s{season:02}e{number:03}").to_lowercase()
                }
                None => media.title.to_lowercase(),
            });
            if let Some((series, season, number)) = episode {
                media.payload = Some(serde_json::json!({
                    "seriesTitle": series,
                    "season": season,
                    "episode": number,
                    "mediaType": "episode",
                    "metadataConfidence": "filename"
                }));
            }
            // Classify 18+ at import time so the item is isolated from the main
            // library even before any network scrape runs. An explicit batch
            // choice (`options.mark_adult`) wins over the heuristic and is
            // recorded as a manual decision so it is never overridden later.
            match options.mark_adult {
                Some(true) => set_manual_adult(&mut media, true),
                Some(false) => {}
                None => {
                    apply_adult_classification(&mut media);
                }
            }
            media_batch.push(media);
            if media_batch.len() >= 128 {
                match database.upsert_media_batch(&media_batch) {
                    Ok(()) => report.imported += media_batch.len() as u64,
                    Err(_) => report.errors += media_batch.len() as u64,
                }
                media_batch.clear();
            }
        }
        if let Some(progress) = options.progress.as_ref() {
            progress(
                report.scanned_files,
                report.imported,
                report.skipped,
                report.skipped_promotional,
                report.skipped_non_video,
            );
        }
    }

    if !media_batch.is_empty() {
        match database.upsert_media_batch(&media_batch) {
            Ok(()) => report.imported += media_batch.len() as u64,
            Err(_) => report.errors += media_batch.len() as u64,
        }
        media_batch.clear();
    }
    if let Some(progress) = options.progress.as_ref() {
        progress(
            report.scanned_files,
            report.imported,
            report.skipped,
            report.skipped_promotional,
            report.skipped_non_video,
        );
    }

    Ok(report)
}

fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
        .unwrap_or(false)
}

/// Detect common cloud-drive advertising, referral, navigation and promotion
/// entries before they become playable library items.
pub fn is_promotional_name(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase().replace(
        [
            ' ', '_', '-', '·', '。', '，', ',', '.', '【', '】', '[', ']', '(', ')',
        ],
        "",
    );
    if normalized.is_empty() {
        return false;
    }
    [
        "广告",
        "推广",
        "宣传",
        "赞助",
        "片头广告",
        "片尾广告",
        "广告视频",
        "推广视频",
        "宣传片",
        "开屏",
        "弹窗",
        "二维码",
        "扫码",
        "加群",
        "qq群",
        "微信群",
        "公众号",
        "微信",
        "telegram",
        "导航",
        "最新网址",
        "最新地址",
        "备用网址",
        "备用地址",
        "发布页",
        "资源发布",
        "福利",
        "优惠",
        "返利",
        "邀请码",
        "邀请链接",
        "客服",
        "联系站长",
        "观影群",
        "资源站",
        "网站导航",
        "官方网站",
        "官方网页",
        "域名",
        "跳转",
        "推广链接",
        "赞助商",
        "vip购买",
        "充值",
        "成人导航",
        "advert",
        "advertisement",
        "adbanner",
        "promotion",
        "promotional",
        "sponsor",
        "referral",
        "qrscan",
        "join群",
        // 小样/预览/预告等无效片段：不进入刮削流水线，也不列出。
        "sample",
        "trailer",
        "preview",
        "advertising",
        "commercial",
        "sponsored",
        "placeholder",
        "dummyvideo",
        "testvideo",
        "junkvideo",
        "小样",
        "样片",
        "测试视频",
        "测试片",
        "空文件",
        "废片",
        "垃圾",
        "试看",
        "预览",
        "预告片",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

/// Detect promotional payloads exposed by TVBox/OpenList/provider adapters.
/// Only explicitly promotional fields are inspected so ordinary descriptions
/// do not accidentally become hidden from the library.
pub fn is_promotional_metadata(value: &Value) -> bool {
    const KEYS: &[&str] = &[
        "promotional",
        "promoted",
        "vod_remarks",
        "vod_content",
        "remarks",
        "content",
        "description",
        "notice",
        "ad",
        "advertisement",
        "promotion",
        "ad_url",
        "adurl",
        "advert_url",
        "adverturl",
        "source_url",
        "sourceurl",
        "file_name",
        "filename",
        "file_name",
        "path",
        "url",
        "channel",
    ];
    match value {
        Value::Object(object) => object.iter().any(|(key, item)| {
            (KEYS
                .iter()
                .any(|candidate| key.eq_ignore_ascii_case(candidate))
                && (item.as_str().map(is_promotional_name).unwrap_or(false)
                    || item.as_bool().unwrap_or(false)))
                || is_promotional_metadata(item)
        }),
        Value::Array(items) => items.iter().any(is_promotional_metadata),
        _ => false,
    }
}

/// Apply the same advertisement/junk decision to records coming from every
/// source. This is deliberately stricter than the adult classifier: an ad is
/// rejected before metadata scraping can route it into the 18+ library.
pub fn is_promotional_media_record(item: &MediaRecord) -> bool {
    if item
        .payload
        .as_ref()
        .and_then(|payload| payload.get("promotional"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    [
        item.title.as_str(),
        item.original_title.as_deref().unwrap_or_default(),
        item.sort_key.as_deref().unwrap_or_default(),
        item.remote_path.as_deref().unwrap_or_default(),
    ]
    .iter()
    .any(|value| is_promotional_name(value))
        || is_promotional_path(item.remote_path.as_deref().unwrap_or_default())
        || item.payload.as_ref().is_some_and(is_promotional_metadata)
}

/// Mark a record as hidden promotional/junk content without deleting the
/// underlying file. Returns whether the payload changed.
pub fn mark_promotional_media(item: &mut MediaRecord, reason: &str) -> bool {
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let object = payload.as_object_mut().expect("object payload");
    let changed = object.get("promotional").and_then(Value::as_bool) != Some(true)
        || object.get("promotionalReason").and_then(Value::as_str) != Some(reason);
    object.insert("promotional".into(), Value::Bool(true));
    object.insert("promotionalReason".into(), Value::String(reason.into()));
    item.payload = Some(payload);
    changed
}

pub fn is_promotional_path(value: &str) -> bool {
    value
        .split(['/', '\\'])
        .filter(|part| !part.trim().is_empty())
        .any(|part| {
            if is_promotional_name(part) {
                return true;
            }
            let component = part.trim().to_ascii_lowercase();
            let stem = component
                .rsplit_once('.')
                .map(|(value, _)| value)
                .unwrap_or(component.as_str());
            matches!(
                stem,
                "ad" | "ads"
                    | "advert"
                    | "advertising"
                    | "commercial"
                    | "commercials"
                    | "promo"
                    | "promos"
                    | "promotion"
                    | "promotions"
                    | "sample"
                    | "samples"
                    | "trailer"
                    | "trailers"
                    | "preview"
                    | "previews"
                    | "junk"
            )
        })
}

/// Mark existing records as hidden promotional/junk content. The files are
/// intentionally retained so a false positive can be recovered manually.
pub fn cleanup_promotional_media(
    database: &Database,
) -> Result<PromotionalCleanupReport, AppError> {
    let mut report = PromotionalCleanupReport::default();
    let mut offset = 0_u32;
    const BATCH_SIZE: u32 = 500;
    loop {
        let batch = database.list_media_raw(BATCH_SIZE, offset)?;
        let count = batch.len();
        if count == 0 {
            break;
        }
        for mut item in batch {
            report.scanned = report.scanned.saturating_add(1);
            if is_promotional_media_record(&item)
                && mark_promotional_media(&mut item, "content-filter")
            {
                database.upsert_media(&item)?;
                report.hidden = report.hidden.saturating_add(1);
            }
        }
        if (count as u32) < BATCH_SIZE {
            break;
        }
        offset = offset.saturating_add(BATCH_SIZE);
    }
    Ok(report)
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromotionalCleanupReport {
    pub scanned: u64,
    pub hidden: u64,
}

fn local_media_id(path: &str) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_ascii_lowercase().hash(&mut hasher);
    format!("local:{:016x}", hasher.finish())
}

/// Parse common TV naming schemes without trying to fully interpret release
/// names. The returned series title is intentionally conservative so metadata
/// providers can still perform the final match.
pub(crate) fn parse_episode_identity(value: &str) -> Option<(String, i64, i64)> {
    let normalized = value.replace(['.', '_', '-'], " ");
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        let lower = token.to_ascii_lowercase();
        if let Some((season, episode)) = lower.strip_prefix('s').and_then(parse_sxe) {
            let series = tokens[..index].join(" ");
            if !series.is_empty() {
                return Some((series, season, episode));
            }
        }
        if let Some((season, episode)) = lower.split_once('x').and_then(parse_1x02) {
            let series = tokens[..index].join(" ");
            if !series.is_empty() {
                return Some((series, season, episode));
            }
        }
    }
    None
}

fn parse_sxe(value: &str) -> Option<(i64, i64)> {
    let marker = value.find('e')?;
    let season = value.get(1..marker)?.parse().ok()?;
    let episode = value.get(marker + 1..)?.parse().ok()?;
    (season >= 0 && episode >= 0).then_some((season, episode))
}

fn parse_1x02(value: (&str, &str)) -> Option<(i64, i64)> {
    let season = value.0.parse().ok()?;
    let episode = value.1.parse().ok()?;
    (season >= 0 && episode >= 0).then_some((season, episode))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_video_files_and_is_idempotent() {
        let root = std::env::temp_dir().join(format!("ttv-scan-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("movie.MKV"), b"video").unwrap();
        fs::write(root.join("nested").join("clip.mp4"), b"video").unwrap();
        fs::write(root.join("notes.txt"), b"skip").unwrap();

        let database = Arc::new(Database::open_in_memory().unwrap());
        let first = scan_directory(&database, &root, ScanOptions::default()).unwrap();
        let second = scan_directory(&database, &root, ScanOptions::default()).unwrap();
        assert_eq!(first.imported, 2);
        assert_eq!(second.imported, 2);
        assert_eq!(
            database
                .list_media(Default::default(), 10, 0)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(first.skipped, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_file_as_scan_root() {
        let root = std::env::temp_dir().join(format!("ttv-scan-file-{}", std::process::id()));
        fs::write(&root, b"not a directory").unwrap();
        let database = Arc::new(Database::open_in_memory().unwrap());
        assert!(matches!(
            scan_directory(&database, &root, ScanOptions::default()),
            Err(AppError::InvalidInput(_))
        ));
        let _ = fs::remove_file(root);
    }

    #[test]
    fn filters_promotional_names() {
        assert!(is_promotional_name("片头广告_扫码加群.mp4"));
        assert!(is_promotional_name("最新地址与备用网址"));
        assert!(!is_promotional_name("The.Matrix.1999.1080p.mkv"));
        assert!(is_promotional_name("official_promotion_banner.jpg"));
        assert!(is_promotional_name("广告.无码.1080p.mp4"));
        assert!(!is_promotional_name("The Official Movie 2024.mkv"));
    }

    #[test]
    fn detects_promotional_provider_metadata_recursively() {
        let value = serde_json::json!({"streamhub": {"vod_content": "扫码加群获取最新地址"}});
        assert!(is_promotional_metadata(&value));
        assert!(!is_promotional_metadata(
            &serde_json::json!({"vod_content": "A feature film"})
        ));
    }

    #[test]
    fn classifies_record_from_path_and_payload_before_adult_routing() {
        let mut media = MediaRecord::new("ad-1", "video", "ABC-123");
        media.remote_path = Some("/资源站/ABC-123.mp4".into());
        assert!(is_promotional_media_record(&media));
        let mut marked = media.clone();
        assert!(mark_promotional_media(&mut marked, "content-filter"));
        assert!(is_promotional_media_record(&marked));
    }

    #[test]
    fn cleanup_hides_existing_promotional_rows_without_deleting_files() {
        let database = Database::open_in_memory().unwrap();
        let mut ad = MediaRecord::new("ad-1", "video", "广告 ABC-123");
        ad.remote_path = Some("/library/广告 ABC-123.mp4".into());
        let normal = MediaRecord::new("normal-1", "video", "A Real Movie");
        database.upsert_media_batch(&[ad, normal]).unwrap();

        let report = cleanup_promotional_media(&database).unwrap();
        assert_eq!(report.hidden, 1);
        assert_eq!(
            database
                .list_media(Default::default(), 10, 0)
                .unwrap()
                .len(),
            1
        );
        assert!(database.get_media("ad-1").unwrap().is_some());
    }

    #[test]
    fn parses_episode_identity_and_preserves_series_sorting() {
        assert_eq!(
            parse_episode_identity("The.Show.S02E07.1080p"),
            Some(("The Show".into(), 2, 7))
        );
        assert_eq!(
            parse_episode_identity("The Show 1x03"),
            Some(("The Show".into(), 1, 3))
        );
        assert_eq!(parse_episode_identity("Movie.2024.1080p"), None);
    }
}
