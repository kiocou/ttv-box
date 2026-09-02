//! Real metadata scraping for local and provider-backed media.
//!
//! Douban works without credentials and returns Chinese metadata, so it is
//! the default movie/TV provider. TVMaze also works without credentials and
//! covers series not listed on Douban. TMDB is enabled when
//! `TTV_TMDB_READ_TOKEN` or `TTV_TMDB_API_KEY` is present; credentials are
//! never sent to the frontend or persisted in the database.

use regex::Regex;
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha512};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::AppError;
use crate::library::{is_promotional_media_record, mark_promotional_media};
use crate::storage::MediaRecord;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeReport {
    pub requested: u64,
    pub matched: u64,
    pub updated: u64,
    pub unmatched: u64,
    pub covers: u64,
    pub adult_isolated: u64,
    pub providers: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ScrapeOptions {
    pub providers: Vec<String>,
    pub overwrite: bool,
    pub include_adult: bool,
    /// Directory where covers are downloaded (`{data_dir}/covers`). JAV covers
    /// go to `{cover_dir}/{code}{ext}`, normal posters to
    /// `{cover_dir}/normal/{provider}-{id}{ext}`. When `None`, covers are not
    /// downloaded and remote URLs are kept instead.
    pub cover_dir: Option<std::path::PathBuf>,
    pub cancel: Option<Arc<AtomicBool>>,
    /// Two-phase scraping: `Fast` = JavBus only (first pass, seconds per
    /// item); `Full` = all six adult sources for the leftovers (later pass).
    pub jav_scope: crate::adult::JavScope,
}

/// One progress tick emitted while [`scrape_media`] processes the library.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrapeProgress {
    pub phase: String,
    pub current: u64,
    pub total: u64,
    pub percent: f64,
    pub title: String,
    pub provider: String,
    pub matched: u64,
    pub unmatched: u64,
    pub updated: u64,
    pub covers: u64,
    pub adult_isolated: u64,
    pub done: bool,
}

/// Callback used to stream [`ScrapeProgress`] to the caller (the command layer
/// turns this into a Tauri event). Kept as a plain closure so `metadata` does
/// not depend on `tauri`.
pub type ScrapeProgressSink = std::sync::Arc<dyn Fn(ScrapeProgress) + Send + Sync>;

/// TMDB credential resolved from settings or the environment. Read tokens are
/// sent as a bearer token, API keys as the `api_key` query parameter.
#[derive(Debug, Clone)]
enum TmdbCredential {
    ReadToken(String),
    ApiKey(String),
}

/// Per-item outcome used to fold results into the report and emit one progress
/// tick per item regardless of which branch handled it.
#[derive(Debug, Default)]
struct ItemDelta {
    matched: bool,
    updated: bool,
    unmatched: bool,
    cover: bool,
    adult_isolated: bool,
    provider: String,
}

pub async fn scrape_media(
    database: &std::sync::Arc<crate::storage::Database>,
    media: Vec<MediaRecord>,
    options: ScrapeOptions,
    progress: Option<ScrapeProgressSink>,
) -> Result<ScrapeReport, AppError> {
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("TTV-Box/0.1 metadata scraper")
        .build()
        .map_err(|e| AppError::Runtime(format!("metadata client: {e}")))?;
    let total = media.len() as u64;
    let mut report = ScrapeReport {
        requested: total,
        matched: 0,
        updated: 0,
        unmatched: 0,
        covers: 0,
        adult_isolated: 0,
        providers: options.providers.clone(),
    };
    let tmdb_credential = resolve_tmdb_credential(database);
    let jav_enabled = options.providers.iter().any(|provider| provider == "jav");
    // Bulk run: a 5,000-item backlog batch cannot afford the full 6-source
    // JAV pipeline per item (up to 1-2 min each with timeouts + the 31s
    // sehuatang throttle) — switch the adult sources to fail-fast mode.
    let bulk_run = total >= 200;
    crate::adult::set_fast_mode(bulk_run);
    let adult_client = if jav_enabled {
        Some(if bulk_run {
            crate::adult::build_fast_client()?
        } else {
            crate::adult::build_client()?
        })
    } else {
        None
    };
    let concurrency = if jav_enabled { 4 } else { 6 };
    let mut pending = media.into_iter();
    let mut running = tokio::task::JoinSet::new();
    let mut started = 0_u64;
    let database = std::sync::Arc::clone(database);
    let cancel = options.cancel.clone();
    let options = std::sync::Arc::new(options);
    let tmdb_credential = std::sync::Arc::new(tmdb_credential);
    let adult_client = adult_client.map(std::sync::Arc::new);

    let fast_mode_reset = FastModeReset;
    loop {
        while running.len() < concurrency {
            let Some(item) = pending.next() else { break };
            if cancel
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
            {
                break;
            }
            started += 1;
            let current = started;
            let progress_title = item.title.clone();
            if let Some(sink) = progress.as_ref() {
                let percent = if total == 0 {
                    100.0
                } else {
                    (current.saturating_sub(1) as f64 / total as f64) * 100.0
                };
                sink(ScrapeProgress {
                    phase: "scrape".into(),
                    current,
                    total,
                    percent,
                    title: progress_title.clone(),
                    provider: String::new(),
                    matched: report.matched,
                    unmatched: report.unmatched,
                    updated: report.updated,
                    covers: report.covers,
                    adult_isolated: report.adult_isolated,
                    done: false,
                });
            }
            let database = std::sync::Arc::clone(&database);
            let client = client.clone();
            let adult_client = adult_client.clone();
            let options = std::sync::Arc::clone(&options);
            let tmdb_credential = std::sync::Arc::clone(&tmdb_credential);
            running.spawn(async move {
                let mut match_cache = HashMap::new();
                let result = scrape_one(
                    &database,
                    &client,
                    adult_client.as_deref(),
                    &mut match_cache,
                    &options,
                    tmdb_credential.as_ref().as_ref(),
                    item,
                )
                .await;
                (current, progress_title, result)
            });
        }
        if running.is_empty() {
            break;
        }
        let Some(joined) = running.join_next().await else {
            break;
        };
        let (current, progress_title, delta) =
            joined.map_err(|error| AppError::Runtime(format!("scrape task failed: {error}")))?;
        let delta = delta?;
        if delta.matched {
            report.matched += 1;
        }
        if delta.updated {
            report.updated += 1;
        }
        if delta.unmatched {
            report.unmatched += 1;
        }
        if delta.cover {
            report.covers += 1;
        }
        if delta.adult_isolated {
            report.adult_isolated += 1;
        }
        if let Some(sink) = progress.as_ref() {
            let percent = if total == 0 {
                100.0
            } else {
                (current as f64 / total as f64) * 100.0
            };
            sink(ScrapeProgress {
                phase: "scrape".into(),
                current,
                total,
                percent,
                title: progress_title,
                provider: delta.provider.clone(),
                matched: report.matched,
                unmatched: report.unmatched,
                updated: report.updated,
                covers: report.covers,
                adult_isolated: report.adult_isolated,
                done: started >= total && running.is_empty() && pending.len() == 0,
            });
        }
    }
    drop(fast_mode_reset);
    Ok(report)
}

struct FastModeReset;

impl Drop for FastModeReset {
    fn drop(&mut self) {
        crate::adult::set_fast_mode(false);
    }
}

/// Process a single media item through the scrape pipeline and report what
/// happened via an [`ItemDelta`]. Extracted so the outer loop can fold results
/// into the report and emit exactly one progress tick per item.
#[allow(clippy::too_many_arguments)]
async fn scrape_one(
    database: &std::sync::Arc<crate::storage::Database>,
    client: &Client,
    adult_client: Option<&Client>,
    match_cache: &mut HashMap<(String, String), Option<Match>>,
    options: &ScrapeOptions,
    tmdb_credential: Option<&TmdbCredential>,
    mut item: MediaRecord,
) -> Result<ItemDelta, AppError> {
    if is_promotional_media_record(&item) {
        let changed = mark_promotional_media(&mut item, "content-filter");
        if changed {
            database.upsert_media(&item)?;
        }
        return Ok(ItemDelta {
            updated: changed,
            ..Default::default()
        });
    }
    if let Some(nfo) = item.remote_path.as_deref().and_then(read_nfo_fields) {
        apply_nfo_fields(&mut item, &nfo);
        database.upsert_media(&item)?;
        if !options.overwrite {
            return Ok(ItemDelta {
                updated: true,
                ..Default::default()
            });
        }
    }
    if !options.overwrite
        && item
            .payload
            .as_ref()
            .and_then(|p| p.get("scrapedBy"))
            .is_some()
        && !jav_payload_incomplete(item.payload.as_ref())
    {
        return Ok(ItemDelta {
            unmatched: true,
            ..Default::default()
        });
    }
    // Self-healing: a previous scrape may have stamped the *auto* 18+ flag on
    // an ordinary movie whose release name looked code-shaped (the "HDR-010"
    // bug). Strip that stale classification before anything else so the fixed
    // classifier below gets a clean slate; genuinely-coded JAV items are
    // re-flagged from their file name by the same pipeline.
    if clear_stale_jav_unmatched(&mut item) {
        database.upsert_media(&item)?;
    }
    // 轮次化刮削（两套相反的流水线，视频类型判定决定走哪套）：
    // 18+ 条目：①高成功率 18+ 源（JavBus）→ ②其他 18+ 源 → ③普通影视源兜底
    // 普通条目：①普通影视源（TMDB 等）→ ②其他普通源（豆瓣/TVMaze）→ ③18+ 源兜底
    // 每轮任意源命中即终止；三轮全败 → scrapeFailures+1，连续两轮全败 →
    // payload.scraped=false（界面隐藏、文件保留，后台低优先级重试再捡起）。
    let adult_hint = looks_adult_media(&item);
    let scrape_stage = payload_scrape_u64(&item, "scrapeStage").max(1);
    // JAV branch: only a *strong* 18+ filename signal (see `looks_like_jav_name`)
    // routes the item to the adult sources. Extraction is recall-oriented and
    // would otherwise send ordinary titles such as "Level 03" / "Part 2" into
    // the 18+ zone. Adult-flagged items without a JAV code still isolate below
    // and never fall through to TMDB/TVMaze.
    let mut round3_normal_fallback = false;
    if let Some(adult_client) = adult_client.clone() {
        let source_name = item
            .remote_path
            .clone()
            .unwrap_or_else(|| item.title.clone());
        let extracted = crate::adult::code::extract_codes_from_name(&source_name);
        let codes = if crate::adult::code::looks_like_jav_name_relaxed(&source_name)
            || (looks_adult_media(&item) && !extracted.is_empty())
        {
            extracted
        } else {
            Vec::new()
        };
        if !codes.is_empty() {
            // 轮次门槛：快速轮（scope=Fast）只跑第①轮，失败把阶段推到 2，
            // 剩余轮次交给后面的全量轮；全量轮一次跑完 ②③。
            let effective_scope = match options.jav_scope {
                crate::adult::JavScope::Full => crate::adult::JavScope::Full,
                crate::adult::JavScope::Fast if scrape_stage >= 2 => crate::adult::JavScope::Full,
                other => other,
            };
            let delta = match scrape_jav_item(
                database,
                adult_client,
                &codes,
                options.cover_dir.as_deref(),
                effective_scope,
            )
            .await
            {
                Ok(Some(matched)) => {
                    let cover_path =
                        download_jav_cover(adult_client, options.cover_dir.as_deref(), &matched)
                            .await;
                    let had_cover = cover_path.is_some();
                    apply_jav_match(&mut item, &matched, cover_path);
                    database.upsert_media(&item)?;
                    ItemDelta {
                        matched: true,
                        updated: true,
                        cover: had_cover,
                        provider: "jav".into(),
                        ..Default::default()
                    }
                }
                Ok(None) => {
                    mark_jav_unmatched(&mut item, &codes, "not-found");
                    match effective_scope {
                        crate::adult::JavScope::Fast => {
                            // 第①轮失败：阶段推到 2，等下一批（全量轮）继续。
                            set_scrape_stage(&mut item, 2);
                            database.upsert_media(&item)?;
                            ItemDelta {
                                updated: true,
                                unmatched: true,
                                adult_isolated: true,
                                provider: "jav".into(),
                                ..Default::default()
                            }
                        }
                        crate::adult::JavScope::Full => {
                            // 第②轮失败：进入第③轮（普通影视源按标题兜底）。
                            set_scrape_stage(&mut item, 3);
                            database.upsert_media(&item)?;
                            round3_normal_fallback = true;
                            ItemDelta::default()
                        }
                    }
                }
                Err(error) => {
                    // Transient failure: mark 18+ so the item is isolated, but
                    // leave it unscraped so the next round retries.
                    tracing::warn!(
                        codes = ?codes,
                        error = %error,
                        "jav scrape failed transiently; will retry next round"
                    );
                    mark_jav_unmatched(&mut item, &codes, "pending");
                    database.upsert_media(&item)?;
                    ItemDelta {
                        updated: true,
                        unmatched: true,
                        adult_isolated: true,
                        provider: "jav".into(),
                        ..Default::default()
                    }
                }
            };
            if !round3_normal_fallback {
                return Ok(delta);
            }
        }
    }
    let mut queries = query_variants(&item.title);
    if let Some(path) = item.remote_path.as_deref() {
        for extra in query_variants(path) {
            if !queries.contains(&extra) {
                queries.push(extra);
            }
        }
    }
    if let Some(folder) = folder_name_of(&item) {
        if !queries.contains(&folder) {
            queries.insert(0, folder);
        }
    }
    if queries.is_empty() {
        return Ok(ItemDelta {
            unmatched: true,
            ..Default::default()
        });
    }
    // 18+ isolation: adult items are only ever scraped by the JAV sources above
    // and must never be routed to the normal TMDB/TVMaze providers. If JAV did
    // not handle this item (no code / provider disabled / no match), clear any
    // stale normal metadata, mark the item isolated, and stop here — except
    // when轮③ (normal-source fallback after the 18+ rounds failed) is active.
    let manual_override = item
        .payload
        .as_ref()
        .and_then(|payload| payload.as_object())
        .and_then(manual_adult_override);
    // 手动标记优先：用户明确标了 18+ 的条目不做普通源兜底。
    if manual_override.unwrap_or(adult_hint && !round3_normal_fallback) {
        // Drop leftover TMDB/TVMaze metadata, but keep a provider thumbnail
        // (http URL) so we do not blank out a cover that never came from a
        // normal scrape. JAV-scraped items never reach here.
        let provider_cover = item
            .art_url
            .clone()
            .filter(|url| url.trim_start().starts_with("http"));
        if had_remote_metadata(item.payload.as_ref()) {
            clear_remote_metadata(&mut item);
            if provider_cover.is_some() {
                item.art_url = provider_cover;
            }
        }
        let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
        if !payload.is_object() {
            payload = serde_json::json!({});
        }
        let obj = payload.as_object_mut().expect("object payload");
        obj.insert("adult".into(), Value::Bool(true));
        obj.insert("contentRating".into(), Value::String("18+".into()));
        obj.entry("genres")
            .or_insert_with(|| serde_json::json!(["成人"]));
        obj.insert(
            "metadataSource".into(),
            Value::String("local-classifier".into()),
        );
        item.payload = Some(payload);
        database.upsert_media(&item)?;
        return Ok(ItemDelta {
            unmatched: true,
            updated: true,
            adult_isolated: true,
            ..Default::default()
        });
    }
    let mut match_value = None;
    let mut provider_name = None;
    let mut matched_query = String::new();
    if let Some(tmdb_id) =
        extract_tmdb_id(&item.title).or_else(|| item.remote_path.as_deref().and_then(extract_tmdb_id))
    {
        if options.providers.iter().any(|provider| provider == "tmdb") {
            match scrape_tmdb_by_id(client, &tmdb_id, tmdb_credential).await {
                Ok(Some(metadata)) => {
                    match_value = Some(metadata);
                    provider_name = Some("tmdb");
                    matched_query = format!("tmdb:{tmdb_id}");
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        tmdb_id = %tmdb_id,
                        error = %error,
                        "TMDB id lookup failed; falling back to title search"
                    );
                }
            }
        }
    }
    let mut providers = options.providers.clone();
    if match_value.is_none() && queries.iter().any(|query| contains_cjk(query)) {
        providers.sort_by_key(|provider| match provider.as_str() {
            "douban" => 0,
            "tmdb" => 1,
            "tvmaze" => 2,
            _ => 3,
        });
    }
    if match_value.is_none() {
    'providers: for provider in &providers {
        // Try the most specific query variant first; the colon-cut fallback
        // only runs when the full title matched nothing anywhere.
        for query in &queries {
            let cache_key = (provider.clone(), query.to_lowercase());
            let result = if let Some(cached) = match_cache.get(&cache_key) {
                cached.clone()
            } else {
                let persistent_key = format!(
                    "{}:{}:{}",
                    provider,
                    query.to_lowercase(),
                    options.include_adult || adult_hint
                );
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|value| value.as_secs() as i64)
                    .unwrap_or_default();
                let fetched = if let Some((_cached_provider, payload)) =
                    database.metadata_cache_get(&persistent_key, now)?
                {
                    serde_json::from_str::<Option<Match>>(&payload).unwrap_or(None)
                } else {
                    let outcome = match provider.as_str() {
                        "tmdb" => {
                            scrape_tmdb(
                                client,
                                query,
                                options.include_adult || adult_hint,
                                tmdb_credential,
                            )
                            .await
                        }
                        // 轮③：18+ 条目在 18+ 源全部失败后允许普通源按标题兜底。
                        "douban" if !adult_hint || round3_normal_fallback => {
                            scrape_douban(query).await
                        }
                        "tvmaze" if !adult_hint || round3_normal_fallback => {
                            scrape_tvmaze(client, query).await
                        }
                        _ => Ok(None),
                    };
                    match outcome {
                        Ok(fetched) => {
                            let ttl = if fetched.is_some() {
                                30 * 24 * 3600
                            } else {
                                6 * 3600
                            };
                            if let Ok(payload) = serde_json::to_string(&fetched) {
                                let _ = database.metadata_cache_set(
                                    &persistent_key,
                                    provider,
                                    &payload,
                                    now.saturating_add(ttl),
                                );
                            }
                            fetched
                        }
                        Err(error) => {
                            // A single provider's transient network failure (timeout,
                            // DNS, rate limit) must not abort the whole library
                            // scrape. Treat it as "no match from this provider" and
                            // move on; the item stays unscraped and is retried on the
                            // next run. Deliberately not cached, so a later run hits
                            // the network again instead of replaying the failure.
                            tracing::warn!(
                                provider = %provider,
                                query = %query,
                                error = %error,
                                "metadata provider lookup failed; skipping provider for this item"
                            );
                            None
                        }
                    }
                };
                match_cache.insert(cache_key, fetched.clone());
                fetched
            };
            if result.is_some() {
                match_value = result;
                provider_name = Some(provider.as_str());
                matched_query = query.clone();
                break 'providers;
            }
        }
    }
    }
    let Some(metadata) = match_value else {
        // 第③轮（普通条目）：普通源全部未命中，文件名带严格番号信号时用
        // JavBus 做最后兜底（仅快速单源，控制耗时）。
        if !adult_hint {
            if let Some(fallback_client) = adult_client.as_ref() {
                let source_name = item
                    .remote_path
                    .clone()
                    .unwrap_or_else(|| item.title.clone());
                let extracted = crate::adult::code::extract_codes_from_name(&source_name);
                if !extracted.is_empty() && crate::adult::code::looks_like_jav_name(&source_name) {
                    if let Ok(Some(matched)) = crate::adult::lookup_jav_scoped(
                        fallback_client,
                        &extracted,
                        crate::adult::JavScope::Fast,
                    )
                    .await
                    {
                        let cover_path = download_jav_cover(
                            fallback_client,
                            options.cover_dir.as_deref(),
                            &matched,
                        )
                        .await;
                        let had_cover = cover_path.is_some();
                        apply_jav_match(&mut item, &matched, cover_path);
                        database.upsert_media(&item)?;
                        return Ok(ItemDelta {
                            matched: true,
                            updated: true,
                            cover: had_cover,
                            provider: "jav".into(),
                            ..Default::default()
                        });
                    }
                }
            }
        }
        // 三轮全部失败：失败计数 +1；连续两轮全败 → payload.scraped=false
        // （界面隐藏，文件保留，后台低优先级重试会再捡起）。
        record_pipeline_failure(&mut item);
        database.upsert_media(&item)?;
        let had_previous_remote_match = had_remote_metadata(item.payload.as_ref());
        let mut delta = ItemDelta {
            unmatched: true,
            updated: had_previous_remote_match,
            ..Default::default()
        };
        if had_previous_remote_match {
            clear_remote_metadata(&mut item);
            database.upsert_media(&item)?;
        }
        if adult_hint {
            let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
            if !payload.is_object() {
                payload = serde_json::json!({});
            }
            let obj = payload.as_object_mut().expect("object payload");
            // A manual "not 18+" decision wins over the filename hint.
            if manual_adult_override(obj).unwrap_or(true) {
                obj.insert("adult".into(), Value::Bool(true));
                obj.insert("contentRating".into(), Value::String("18+".into()));
                obj.entry("genres")
                    .or_insert_with(|| serde_json::json!(["成人"]));
                obj.insert(
                    "metadataSource".into(),
                    Value::String("local-classifier".into()),
                );
                item.payload = Some(payload);
                database.upsert_media(&item)?;
                delta.updated = true;
                delta.adult_isolated = true;
            } else {
                item.payload = Some(payload);
            }
        }
        return Ok(delta);
    };
    let Match {
        title,
        original_title,
        year,
        rating,
        art,
        backdrop,
        summary,
        external_id,
        media_type,
        adult,
        genres,
    } = metadata;
    let source_title = item.title.clone();
    // 轮③命中：18+ 分类条目被普通源匹配，说明是误分类（HDR-010 案例），
    // 解除隔离让条目回到正常影视库。
    if adult_hint {
        set_manual_adult(&mut item, false);
    }
    let provider_slug = provider_name.unwrap_or("unknown");
    let remote_art = art.clone();
    // Cache the poster locally for normal matches so the grid does not depend
    // on the remote CDN being reachable at render time.
    let mut cover_downloaded = false;
    let mut final_art = art;
    if !(adult || adult_hint) {
        if let (Some(cover_dir), Some(url)) = (options.cover_dir.as_deref(), remote_art.as_deref())
        {
            if url.starts_with("http://") || url.starts_with("https://") {
                let key = format!("{}-{}", provider_slug, external_id);
                if let Some(path) = download_normal_cover(client, cover_dir, &key, url).await {
                    final_art = Some(path.display().to_string());
                    cover_downloaded = true;
                }
            }
        }
    }
    item.title = display_title(&title, &source_title);
    item.sort_key = Some(item.title.to_lowercase());
    item.original_title = original_title;
    item.year = year;
    item.rating = rating;
    item.art_url = final_art;
    item.backdrop_url = backdrop;
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let obj = payload.as_object_mut().expect("object payload");
    obj.insert("scrapedBy".into(), Value::String(provider_slug.into()));
    obj.insert("externalId".into(), Value::String(external_id.clone()));
    obj.insert("mediaType".into(), Value::String(media_type));
    obj.insert("summary".into(), Value::String(summary));
    obj.insert("matchedTitle".into(), Value::String(matched_query));
    obj.insert("sourceTitle".into(), Value::String(source_title));
    // A manual 18+ decision wins over whatever the metadata source claims.
    let effective_adult = manual_adult_override(obj).unwrap_or(adult || adult_hint);
    obj.insert("adult".into(), Value::Bool(effective_adult));
    obj.insert(
        "contentRating".into(),
        Value::String(if effective_adult { "18+" } else { "" }.into()),
    );
    let mut genres = genres;
    if effective_adult && genres.is_empty() {
        genres.push("成人".into());
    }
    obj.insert(
        "genres".into(),
        Value::Array(genres.into_iter().map(Value::String).collect()),
    );
    obj.insert("metadataSource".into(), Value::String(provider_slug.into()));
    obj.insert("metadataConfidence".into(), Value::String("remote".into()));
    obj.insert("remoteMatch".into(), Value::Bool(true));
    if cover_downloaded {
        if let Some(url) = remote_art.as_deref() {
            obj.insert("artUrlRemote".into(), Value::String(url.to_string()));
        }
    }
    item.payload = Some(payload);
    database.upsert_media(&item)?;
    Ok(ItemDelta {
        matched: true,
        updated: true,
        cover: cover_downloaded,
        provider: provider_slug.into(),
        ..Default::default()
    })
}

/// Classify a raw TMDB secret as a read token (JWT, sent as bearer) or an API
/// key (short hex, sent as the `api_key` query parameter). Read tokens contain
/// dots; API keys do not.
fn classify_tmdb_secret(raw: &str) -> TmdbCredential {
    if raw.contains('.') {
        TmdbCredential::ReadToken(raw.to_string())
    } else {
        TmdbCredential::ApiKey(raw.to_string())
    }
}

/// Resolve the TMDB credential. Settings (KV `metadata.tmdb.token`) win over
/// the environment so the in-app settings screen can override without a
/// restart. Returns `None` when nothing is configured, which disables TMDB.
fn resolve_tmdb_credential(database: &crate::storage::Database) -> Option<TmdbCredential> {
    if let Ok(Some(token)) = database.kv_get("metadata.tmdb.token") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Some(classify_tmdb_secret(&token));
        }
    }
    if let Ok(token) = std::env::var("TTV_TMDB_READ_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Some(TmdbCredential::ReadToken(token));
        }
    }
    if let Ok(key) = std::env::var("TTV_TMDB_API_KEY") {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Some(TmdbCredential::ApiKey(key));
        }
    }
    None
}

/// Pick a cover file extension from the URL path or the HTTP content type.
fn normal_cover_ext(url_path: &str, content_type: &str) -> String {
    const KNOWN: [&str; 4] = [".jpg", ".jpeg", ".png", ".webp"];
    if let Some((_, ext)) = url_path.rsplit_once('.') {
        let ext = format!(".{}", ext.to_lowercase());
        if KNOWN.contains(&ext.as_str()) {
            return ext;
        }
    }
    let ct = content_type.to_lowercase();
    if ct.contains("webp") {
        ".webp".into()
    } else if ct.contains("png") {
        ".png".into()
    } else {
        ".jpg".into()
    }
}

/// Minimum size for a normal poster to be considered real (not a placeholder).
const MIN_NORMAL_COVER_BYTES: usize = 5 * 1024;

/// Download a normal (non-JAV) poster into `{cover_dir}/normal/{key}{ext}`.
///
/// Returns the local path on success, or `None` when the download fails or the
/// payload looks like a placeholder. Existing covers are reused without a
/// network round-trip. The caller keeps the remote URL as a fallback when this
/// returns `None`.
async fn download_normal_cover(
    client: &Client,
    cover_dir: &std::path::Path,
    key: &str,
    cover_url: &str,
) -> Option<std::path::PathBuf> {
    let key = key.trim().to_lowercase();
    if key.is_empty() {
        return None;
    }
    let dir = cover_dir.join("normal");
    for ext in [".jpg", ".jpeg", ".png", ".webp"] {
        let path = dir.join(format!("{key}{ext}"));
        if path.is_file() {
            return Some(path);
        }
    }
    let mut request = client
        .get(cover_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36",
        );
    // Douban's image CDN rejects hotlinks that carry a foreign Referer; a
    // same-site one keeps the download path working like a browser visit.
    if cover_url.contains("doubanio.com") {
        request = request.header("Referer", "https://movie.douban.com/");
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let final_path = response.url().path().to_owned();
    let bytes = response.bytes().await.ok()?;
    if bytes.len() < MIN_NORMAL_COVER_BYTES {
        return None;
    }
    let ext = normal_cover_ext(&final_path, &content_type);
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        return None;
    }
    let target = dir.join(format!("{key}{ext}"));
    let tmp = std::path::PathBuf::from(format!("{}.tmp", target.display()));
    if tokio::fs::write(&tmp, &bytes).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return None;
    }
    if tokio::fs::rename(&tmp, &target).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return None;
    }
    Some(target)
}

fn had_remote_metadata(payload: Option<&Value>) -> bool {
    payload
        .and_then(Value::as_object)
        .map(|object| {
            object
                .get("metadataConfidence")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("remote"))
                || ["tmdb", "tvmaze", "douban"].contains(
                    &object
                        .get("scrapedBy")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        })
        .unwrap_or(false)
}

fn clear_remote_metadata(item: &mut MediaRecord) {
    let source_title = item
        .payload
        .as_ref()
        .and_then(|payload| payload.get("sourceTitle"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| item.title.clone());
    item.title = normalize_title(&source_title);
    item.sort_key = Some(item.title.to_lowercase());
    item.original_title = None;
    item.year = None;
    item.rating = None;
    item.art_url = item
        .payload
        .as_ref()
        .and_then(|payload| payload.get("metadata"))
        .and_then(|metadata| {
            metadata
                .get("thumbnailUrl")
                .or_else(|| metadata.get("thumbnail_url"))
                .or_else(|| metadata.get("coverUrl"))
                .or_else(|| metadata.get("thumbnail"))
        })
        .and_then(Value::as_str)
        .map(str::to_owned);
    item.backdrop_url = None;
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let object = payload.as_object_mut().expect("object payload");
    for key in [
        "scrapedBy",
        "externalId",
        "mediaType",
        "summary",
        "matchedTitle",
        "remoteMatch",
    ] {
        object.remove(key);
    }
    object.insert(
        "metadataSource".into(),
        Value::String("cleared-unmatched".into()),
    );
    object.insert("metadataConfidence".into(), Value::String("none".into()));
    item.payload = Some(payload);
}
/// 读取 payload 里的轮次状态字段（缺省 0）。
fn payload_scrape_u64(item: &MediaRecord, key: &str) -> i64 {
    item.payload
        .as_ref()
        .and_then(|payload| payload.get(key))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
}

/// 推进轮次阶段（①=1 高成功率源，②=2 其他同类源，③=3 交叉兜底源）。
fn set_scrape_stage(item: &mut MediaRecord, stage: i64) {
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    payload
        .as_object_mut()
        .expect("object payload")
        .insert("scrapeStage".into(), Value::from(stage));
    item.payload = Some(payload);
}

/// 记录一轮完整流水线失败：scrapeFailures +1、阶段归位；连续两轮全败 →
/// payload.scraped=false。UI 列表过滤该标记（条目隐藏），文件不动，
/// 后台低优先级重试会重新处理这些条目。
fn record_pipeline_failure(item: &mut MediaRecord) {
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let obj = payload.as_object_mut().expect("object payload");
    let failures = obj
        .get("scrapeFailures")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        + 1;
    obj.insert("scrapeFailures".into(), Value::from(failures));
    obj.insert("scrapeStage".into(), Value::from(1));
    if failures >= 2 {
        obj.insert("scraped".into(), Value::Bool(false));
    }
    item.payload = Some(payload);
}

/// Look up a JAV code against the adult sources with persistent caching.
/// Successful matches cache for 90 days, confirmed not-found for 7 days
/// (mirroring JavBoss); transient errors are not cached so the next scrape
/// round retries them. The cache key is scope-aware: a fast-scope (JavBus
/// only) miss must never shadow the later full-scope pass's slow sources.
async fn scrape_jav_item(
    database: &std::sync::Arc<crate::storage::Database>,
    client: &reqwest::Client,
    codes: &[String],
    _cover_dir: Option<&std::path::Path>,
    scope: crate::adult::JavScope,
) -> Result<Option<crate::adult::JavMatch>, AppError> {
    let scope_tag = match scope {
        crate::adult::JavScope::Fast => "f",
        crate::adult::JavScope::Full => "",
    };
    let cache_key = format!("v3:jav{}:{}", scope_tag, codes[0].to_lowercase());
    let miss_ttl: i64 = match scope {
        // Fast misses are re-tried cheaply within the same day, but not on
        // every pass of one batch.
        crate::adult::JavScope::Fast => 6 * 3600,
        crate::adult::JavScope::Full => 7 * 24 * 3600,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default();
    if let Some((_provider, payload)) = database.metadata_cache_get(&cache_key, now)? {
        if let Ok(cached) = serde_json::from_str::<Option<crate::adult::JavMatch>>(&payload) {
            return Ok(cached);
        }
    }
    let result = crate::adult::lookup_jav_scoped(client, codes, scope).await?;
    let ttl = if result.is_some() {
        90 * 24 * 3600
    } else {
        miss_ttl
    };
    if let Ok(payload) = serde_json::to_string(&result) {
        let _ = database.metadata_cache_set(&cache_key, "jav", &payload, now.saturating_add(ttl));
    }
    Ok(result)
}

/// Download the cover for a matched JAV item. Failures are non-fatal: the
/// remote URL is kept so the frontend can still display it.
async fn download_jav_cover(
    client: &reqwest::Client,
    cover_dir: Option<&std::path::Path>,
    matched: &crate::adult::JavMatch,
) -> Option<std::path::PathBuf> {
    let cover_dir = cover_dir?;
    let cover_url = matched.cover_url.as_deref()?;
    match crate::adult::cover::download_cover(
        client,
        cover_dir,
        &matched.code,
        cover_url,
        false,
        crate::adult::cover::referer_for_provider(&matched.provider),
    )
    .await
    {
        Ok(path) => Some(path),
        Err(error) => {
            tracing::warn!(code = %matched.code, error = %error, "jav cover download failed");
            None
        }
    }
}

fn apply_jav_match(
    item: &mut MediaRecord,
    matched: &crate::adult::JavMatch,
    cover_path: Option<std::path::PathBuf>,
) {
    let source_title = item.title.clone();
    if !matched.title.is_empty() {
        item.title = matched.title.clone();
    }
    item.sort_key = Some(item.title.to_lowercase());
    item.original_title = Some(matched.code.clone());
    item.year = matched
        .release_date
        .as_deref()
        .and_then(|date| date.get(..4))
        .and_then(|year| year.parse().ok());
    if let Some(minutes) = matched.duration_min.filter(|value| *value > 0) {
        if item.duration_seconds.unwrap_or(0) <= 0 {
            item.duration_seconds = Some(i64::from(minutes) * 60);
        }
    }
    if let Some(rating) = matched.rating.filter(|value| *value > 0.0) {
        item.rating = Some(rating);
    }
    if let Some(path) = cover_path.as_ref() {
        item.art_url = Some(path.display().to_string());
    } else if item.art_url.is_none() {
        item.art_url = matched.cover_url.clone();
    }
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let obj = payload.as_object_mut().expect("object payload");
    obj.insert("scrapedBy".into(), Value::String("jav".into()));
    obj.insert("externalId".into(), Value::String(matched.code.clone()));
    obj.insert("mediaType".into(), Value::String("video".into()));
    obj.insert("matchedTitle".into(), Value::String(matched.code.clone()));
    obj.insert("sourceTitle".into(), Value::String(source_title));
    let summary = crate::adult::compose_summary(matched);
    if !summary.is_empty() {
        obj.insert("summary".into(), Value::String(summary));
    }
    if let Some(cover) = matched
        .cover_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        obj.insert("artUrlRemote".into(), Value::String(cover.to_owned()));
    }
    // A per-item "this is not 18+" click (`library_set_adult`) still wins. A
    // batch import stamp of adultManual=false must not hide a confirmed JAV
    // hit — that is how 18+ titles leaked into the main library.
    let effective_adult = match manual_adult_override(obj) {
        Some(false) if obj.get("adultManualSource").and_then(Value::as_str) == Some("user") => {
            false
        }
        _ => true,
    };
    if effective_adult {
        obj.remove("adultManual");
        obj.remove("adultManualSource");
    }
    obj.insert("adult".into(), Value::Bool(effective_adult));
    obj.insert(
        "contentRating".into(),
        Value::String(if effective_adult { "18+" } else { "" }.into()),
    );
    obj.insert(
        "genres".into(),
        Value::Array(
            matched
                .tags
                .iter()
                .map(|tag| Value::String(tag.clone()))
                .collect(),
        ),
    );
    obj.insert(
        "metadataSource".into(),
        Value::String(format!("jav:{}", matched.provider)),
    );
    obj.insert("metadataConfidence".into(), Value::String("remote".into()));
    obj.insert("remoteMatch".into(), Value::Bool(true));
    obj.insert(
        "jav".into(),
        serde_json::json!({
            "code": matched.code,
            "title": matched.title,
            "actors": matched.actors,
            "studio": matched.studio,
            "director": matched.director,
            "label": matched.label,
            "series": matched.series,
            "tags": matched.tags,
            "uncensored": matched.uncensored,
            "releaseDate": matched.release_date,
            "durationMin": matched.duration_min,
            "rating": matched.rating,
            "summary": matched.summary,
            "provider": matched.provider,
            "coverUrl": matched.cover_url,
        }),
    );
    item.payload = Some(payload);
}

fn jav_payload_incomplete(payload: Option<&Value>) -> bool {
    let Some(payload) = payload.and_then(Value::as_object) else {
        return false;
    };
    if payload.get("scrapedBy").and_then(Value::as_str) != Some("jav") {
        return false;
    }
    let jav = payload.get("jav").and_then(Value::as_object);
    let actors_empty = jav
        .and_then(|obj| obj.get("actors"))
        .and_then(Value::as_array)
        .map(|actors| actors.is_empty())
        .unwrap_or(true);
    let rating_missing = jav
        .and_then(|obj| obj.get("rating"))
        .and_then(Value::as_f64)
        .filter(|value| *value > 0.0)
        .is_none();
    let provider = jav
        .and_then(|obj| obj.get("provider"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let summary = payload.get("summary").and_then(Value::as_str).unwrap_or("");
    let summary_placeholder = summary.is_empty()
        || summary.contains("已从媒体中心导入")
        || summary.contains("已从本地目录导入")
        || summary.contains("等待 JavBus");
    // Retry for a missing score only until JavDB has already been consulted;
    // some titles simply have no public rating.
    let retry_rating = rating_missing && !provider.contains("javdb");
    actors_empty || retry_rating || summary_placeholder
}

/// Mark an item with a JAV code but no (yet) successful scrape as 18+ so it is
/// isolated from the main library, and keep the code candidates for retry.
fn mark_jav_unmatched(item: &mut MediaRecord, codes: &[String], status: &str) {
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let obj = payload.as_object_mut().expect("object payload");
    // Respect a manual "not 18+" decision even when a JAV code is present.
    if manual_adult_override(obj).unwrap_or(true) {
        obj.insert("adult".into(), Value::Bool(true));
        obj.insert("contentRating".into(), Value::String("18+".into()));
        obj.entry("genres")
            .or_insert_with(|| serde_json::json!(["成人"]));
    }
    obj.insert(
        "metadataSource".into(),
        Value::String("jav-classifier".into()),
    );
    obj.insert(
        "jav".into(),
        serde_json::json!({
            "code": codes[0],
            "codes": codes,
            "status": status,
        }),
    );
    if obj
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .is_empty()
    {
        obj.insert(
            "summary".into(),
            Value::String(format!(
                "已识别番号 {}，等待 JavBus / JavDB / Avmoo / JavLibrary / Jav321 补全作品资料。",
                codes[0]
            )),
        );
    }
    item.payload = Some(payload);
}

/// Drop the *auto* 18+ classification that a previous scrape recorded for a
/// JAV-shaped code that none of the adult sources could confirm
/// (`metadataSource == "jav-classifier"` + `jav.status == "not-found"`).
///
/// Those records used to be final, so ordinary movies whose release name
/// produced a phantom code (the "HDR-010" bug) were stranded in the 18+ zone
/// forever with a black cover. Clearing the flag lets the current scrape
/// re-run the fixed classifier: real JAV names re-isolate from their file
/// name in the same pass, false positives proceed to the normal providers.
/// A manual adult decision (`adultManual`) is never touched. Returns whether
/// the record was modified so the caller persists it.
fn clear_stale_jav_unmatched(item: &mut MediaRecord) -> bool {
    let Some(payload) = item.payload.as_ref().and_then(Value::as_object) else {
        return false;
    };
    if payload.get("metadataSource").and_then(Value::as_str) != Some("jav-classifier") {
        return false;
    }
    if payload
        .get("jav")
        .and_then(|jav| jav.get("status"))
        .and_then(Value::as_str)
        != Some("not-found")
    {
        return false;
    }
    if payload.get("adult").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    if manual_adult_override(payload) == Some(true) {
        return false;
    }
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    let obj = payload.as_object_mut().expect("checked object payload");
    obj.remove("adult");
    obj.remove("contentRating");
    obj.remove("jav");
    obj.remove("metadataSource");
    if obj
        .get("genres")
        .and_then(Value::as_array)
        .is_some_and(|genres| genres.len() == 1 && genres[0] == "成人")
    {
        obj.remove("genres");
    }
    if obj
        .get("summary")
        .and_then(Value::as_str)
        .is_some_and(|summary| summary.starts_with("已识别番号"))
    {
        obj.remove("summary");
    }
    item.payload = Some(payload);
    true
}

fn read_nfo_fields(source: &str) -> Option<HashMap<String, String>> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return None;
    }
    let path = std::path::Path::new(source);
    let parent = path.parent()?;
    let stem = path.file_stem()?.to_str()?;
    let candidate = [
        parent.join(format!("{stem}.nfo")),
        parent.join("movie.nfo"),
        parent.join("tvshow.nfo"),
    ]
    .into_iter()
    .find(|path| path.is_file())?;
    let contents = std::fs::read_to_string(candidate).ok()?;
    let lower = contents.to_ascii_lowercase();
    let mut fields = HashMap::new();
    for tag in [
        "title",
        "originaltitle",
        "year",
        "plot",
        "premiered",
        "rating",
        "season",
        "episode",
    ] {
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        let Some(start) = lower.find(&open) else {
            continue;
        };
        let Some(marker_end) = contents[start..].find('>') else {
            continue;
        };
        let content_start = start + marker_end + 1;
        let Some(end_marker) = lower[content_start..].find(&close) else {
            continue;
        };
        let end = end_marker + content_start;
        let value = contents[content_start..end].trim();
        if !value.is_empty() {
            fields.insert(tag.into(), value.into());
        }
    }
    Some(fields)
}

fn apply_nfo_fields(item: &mut MediaRecord, fields: &HashMap<String, String>) {
    if let Some(title) = fields.get("title") {
        item.title = title.clone();
    }
    if let Some(original) = fields.get("originaltitle") {
        item.original_title = Some(original.clone());
    }
    if let Some(year) = fields.get("year").and_then(|value| value.parse().ok()) {
        item.year = Some(year);
    }
    if let Some(rating) = fields.get("rating").and_then(|value| value.parse().ok()) {
        item.rating = Some(rating);
    }
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let object = payload.as_object_mut().expect("object payload");
    for key in ["plot", "premiered", "season", "episode"] {
        if let Some(value) = fields.get(key) {
            object.insert(key.into(), Value::String(value.clone()));
        }
    }
    object.insert("metadataSource".into(), Value::String("nfo".into()));
    object.insert("metadataConfidence".into(), Value::String("local".into()));
    item.payload = Some(payload);
}

fn normalize_title(raw: &str) -> String {
    normalize_query_title(raw, true)
}

/// Build the scrape query for a media title. When `cut_colon` is set, a
/// "Title: Subtitle" stem is reduced to its prefix — release names put junk
/// ("4K Remastered", "Level 03") after a colon. The cut is skipped when the
/// suffix carries CJK text: Chinese titles legitimately contain a full-width
/// colon ("007：大破量子危机"), and cutting them to the prefix ("007") sends
/// the lookup after the wrong movie.
fn normalize_query_title(raw: &str, cut_colon: bool) -> String {
    let stem = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let stem = stem.rsplit_once('.').map(|(s, _)| s).unwrap_or(stem);
    let stem = if let Some(sep) = stem.find(" - ") {
        let prefix = stem[..sep].trim();
        if prefix.chars().count() >= 2 && stem[sep + 3..].trim().chars().count() >= 2 {
            prefix
        } else {
            stem
        }
    } else {
        stem
    };
    let stem = if let Some(sep) = stem.find('｜').or_else(|| stem.find('|')) {
        let prefix = stem[..sep].trim();
        if prefix.chars().count() >= 2 && stem[sep + '｜'.len_utf8()..].trim().chars().count() >= 2
        {
            prefix
        } else {
            stem
        }
    } else {
        stem
    };
    if cut_colon {
        if let Some(sep) = stem.find('：').or_else(|| stem.find(':')) {
            let suffix = if stem[sep..].starts_with('：') {
                stem[sep + '：'.len_utf8()..].trim()
            } else {
                stem[sep + 1..].trim()
            };
            if !contains_cjk(suffix) {
                let prefix = stem[..sep].trim();
                if prefix.chars().count() >= 2 && suffix.chars().count() >= 2 {
                    return normalize_query_title(prefix, false);
                }
            }
        }
    }
    let stem = stem.replace(['.', '_'], " ");
    let stem = regex_like_strip(&stem);
    stem.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether the text contains at least one CJK ideograph (covers the
/// full-width punctuation used in Chinese titles as well).
fn contains_cjk(value: &str) -> bool {
    value
        .chars()
        .any(|c| matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3000..=0x303F | 0xFF00..=0xFFEF))
}

/// Ordered query candidates for one media item: the most specific cleaned
/// title first (keeps "007：大破量子危机" and "Spider-Man No Way Home"
/// intact), then the colon-cut prefix as the fallback for names whose suffix
/// is junk the full query cannot match through. Deduplicated and capped so a
/// pathological name cannot trigger a provider storm.
fn query_variants(raw_title: &str) -> Vec<String> {
    let stripped = strip_embedded_ids(raw_title);
    let mut out: Vec<String> = Vec::new();
    let mut push = |query: String| {
        if !query.is_empty() && !out.contains(&query) && out.len() < 5 {
            out.push(query);
        }
    };
    if let Some(cjk) = leading_cjk_title(&stripped) {
        push(cjk);
    }
    if let Some(latin) = latin_title_after_cjk(&stripped) {
        push(latin);
    }
    if let Some((series, _, _)) = crate::library::parse_episode_identity(&stripped) {
        push(series);
    }
    push(normalize_query_title(&stripped, false));
    push(normalize_title(&stripped));
    out
}

fn strip_embedded_ids(raw: &str) -> String {
    let re = tmdb_id_regex();
    re.replace_all(raw, " ").into_owned()
}

fn tmdb_id_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)[\{\[\(]?\s*tmdb[-_:]?\s*(\d{2,10})\s*[\}\]\)]?").expect("tmdb id regex")
    })
}

/// `{tmdb-972533}` / `[tmdb-335983]` embedded in release names.
pub fn extract_tmdb_id(raw: &str) -> Option<String> {
    tmdb_id_regex()
        .captures(raw)
        .and_then(|caps| caps.get(1))
        .map(|id| id.as_str().to_string())
}

fn leading_cjk_title(raw: &str) -> Option<String> {
    let stem = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let stem = stem.rsplit_once('.').map(|(s, _)| s).unwrap_or(stem);
    let mut collected = String::new();
    for ch in stem.chars() {
        if matches!(ch, '.' | '_' | '[' | ']' | '{' | '}') {
            if !collected.is_empty() {
                break;
            }
            continue;
        }
        if is_cjk_char(ch) || matches!(ch, '：' | ':' | '·' | '・' | '，' | '、' | '！' | '？') {
            collected.push(ch);
            continue;
        }
        if ch.is_ascii_alphanumeric() {
            break;
        }
        if collected.is_empty() {
            continue;
        }
        break;
    }
    let collected = collected
        .trim_matches(|c: char| matches!(c, '：' | ':' | '·' | '・' | ' ' | '-' | '_'))
        .to_string();
    (collected.chars().count() >= 2).then_some(collected)
}

fn latin_title_after_cjk(raw: &str) -> Option<String> {
    leading_cjk_title(raw)?;
    let stem = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let stem = stem.rsplit_once('.').map(|(s, _)| s).unwrap_or(stem);
    let bytes = stem.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = stem[i..].chars().next()?;
        if is_cjk_char(ch) || matches!(ch, '：' | '·' | '・') {
            i += ch.len_utf8();
            continue;
        }
        break;
    }
    let rest = stem[i..].trim_matches(|c: char| matches!(c, '.' | '_' | ' ' | '-' | ':' | '：'));
    if rest.is_empty() {
        return None;
    }
    let mut words = Vec::new();
    for part in rest.split(|c: char| matches!(c, '.' | '_' | ' ')) {
        if part.is_empty() {
            continue;
        }
        if part.len() == 4 && part.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        if is_release_marker(part) || is_season_episode(&part.to_ascii_lowercase()) {
            break;
        }
        if part.chars().any(is_cjk_char) {
            break;
        }
        words.push(part);
        if words.len() >= 8 {
            break;
        }
    }
    let title = words.join(" ");
    (title.chars().count() >= 3).then_some(title)
}

fn is_cjk_char(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3000..=0x303F | 0xFF00..=0xFFEF)
}

fn regex_like_strip(value: &str) -> String {
    let normalized = value.replace(['[', ']', '(', ')'], " ");
    let mut title = Vec::new();
    for part in normalized.split_whitespace() {
        let lower = part
            .trim_matches(|c: char| c == '-' || c == '.')
            .to_ascii_lowercase();
        if is_season_episode(&lower) || is_release_marker(&lower) {
            break;
        }
        if part.len() == 4 && part.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        title.push(part);
    }
    title.join(" ")
}

fn is_release_marker(value: &str) -> bool {
    let value = value
        .trim_matches(|c: char| c == '{' || c == '}' || c == '[' || c == ']' || c == '(' || c == ')')
        .to_ascii_lowercase();
    if value.starts_with("tmdb") || value.starts_with("imdb") || value.starts_with("crc") {
        return true;
    }
    if value.starts_with('r')
        && value.len() <= 3
        && value[1..].chars().all(|c| c.is_ascii_digit())
    {
        return true;
    }
    if value.ends_with("audio") || value.ends_with("audios") {
        return true;
    }
    [
        "1080p", "1080i", "2160p", "720p", "480p", "webrip", "web-dl", "webdl", "bluray", "brrip",
        "hdtv", "remux", "repack", "proper", "hdr", "hdr10", "hdr10+", "dv", "avc", "hevc", "x264",
        "x265", "h264", "h265", "h265", "hq", "sb", "dvdrip", "ldvdrip", "bdrip", "tvrip", "ldrip",
        "halfcd", "web-dl", "ac3", "dts", "aac", "ddp", "ddp5", "10bit", "8bit", "3audio", "2audio",
        "mnhd", "frds", "nowys", "iNT", "int",
    ]
    .iter()
    .any(|marker| value.eq_ignore_ascii_case(marker))
        || value.ends_with("fps")
            && value[..value.len().saturating_sub(3)]
                .chars()
                .all(|c| c.is_ascii_digit())
}

fn is_season_episode(value: &str) -> bool {
    let Some(episode_marker) = value.find('e') else {
        return false;
    };
    value.starts_with('s')
        && episode_marker > 1
        && episode_marker + 1 < value.len()
        && value[1..episode_marker].chars().all(|c| c.is_ascii_digit())
        && value[episode_marker + 1..]
            .chars()
            .all(|c| c.is_ascii_digit())
}

fn display_title(matched_title: &str, source_title: &str) -> String {
    let normalized = source_title.replace(['.', '_', '[', ']', '(', ')'], " ");
    let episode = normalized
        .split_whitespace()
        .map(|part| part.trim_matches(|c: char| c == '-' || c == '.'))
        .find(|part| is_season_episode(&part.to_ascii_lowercase()));
    match episode {
        Some(code) => format!("{matched_title} · {}", code.to_ascii_uppercase()),
        None => matched_title.to_owned(),
    }
}

fn clean_summary(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut inside_tag = false;
    for character in raw.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => text.push(character),
            _ => {}
        }
    }
    let decoded = text
        .replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Match {
    title: String,
    original_title: Option<String>,
    year: Option<i64>,
    rating: Option<f64>,
    art: Option<String>,
    backdrop: Option<String>,
    summary: String,
    external_id: String,
    media_type: String,
    adult: bool,
    genres: Vec<String>,
}

async fn scrape_tvmaze(client: &Client, query: &str) -> Result<Option<Match>, AppError> {
    let value: Value = client
        .get("https://api.tvmaze.com/search/shows")
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| AppError::Provider(format!("TVMaze request failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Provider(format!("TVMaze response failed: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Provider(format!("TVMaze JSON failed: {e}")))?;
    let Some(show) = value
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.get("show"))
    else {
        return Ok(None);
    };
    let title = show
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(query)
        .to_owned();
    let original = show.get("name").and_then(Value::as_str).map(str::to_owned);
    let year = show
        .get("premiered")
        .and_then(Value::as_str)
        .and_then(|v| v.get(0..4))
        .and_then(|v| v.parse().ok());
    let rating = show
        .get("rating")
        .and_then(|v| v.get("average"))
        .and_then(Value::as_f64);
    let art = show
        .get("image")
        .and_then(|v| v.get("original").or_else(|| v.get("medium")))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let summary = clean_summary(show.get("summary").and_then(Value::as_str).unwrap_or(""));
    let id = show
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or_default()
        .to_string();
    Ok(Some(Match {
        title,
        original_title: original,
        year,
        rating,
        art,
        backdrop: None,
        summary,
        external_id: id,
        media_type: "tv".into(),
        adult: false,
        genres: show
            .get("genres")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    }))
}

/// Browser identity used for the Douban requests. Douban answers plain
/// `curl`-style agents with redirects even on the suggest endpoint.
const DOUBAN_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

/// Douban client with a cookie jar and manual redirects. Redirects are
/// followed by hand because the subject page bounces through a
/// proof-of-work interstitial whose cookies must land in the jar before the
/// page itself is served (see [`fetch_douban_html`]).
fn douban_client() -> Result<Client, AppError> {
    Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .user_agent(DOUBAN_USER_AGENT)
        .build()
        .map_err(|e| AppError::Provider(format!("豆瓣客户端构建失败: {e}")))
}

/// Douban metadata provider. Needs no credentials and returns Chinese
/// titles, genres and summaries, so it is the default fallback for titles
/// TMDB (optionally configured) cannot match. Search uses the public suggest
/// endpoint; details come from the subject page.
async fn scrape_douban(query: &str) -> Result<Option<Match>, AppError> {
    let client = douban_client()?;
    let value: Value = client
        .get("https://movie.douban.com/j/subject_suggest")
        .query(&[("q", query)])
        .header("Referer", "https://movie.douban.com/")
        .send()
        .await
        .map_err(|e| AppError::Provider(format!("豆瓣搜索请求失败: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Provider(format!("豆瓣搜索响应失败: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Provider(format!("豆瓣搜索解析失败: {e}")))?;
    let Some(entry) = value.as_array().and_then(|entries| {
        entries.iter().find(|entry| {
            !entry
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .is_empty()
        })
    }) else {
        return Ok(None);
    };
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let media_type = match entry.get("type").and_then(Value::as_str) {
        Some("tv") => "tv",
        _ => "movie",
    };
    let suggest_year = entry
        .get("year")
        .and_then(Value::as_str)
        .and_then(|year| year.get(0..4))
        .and_then(|year| year.parse::<i64>().ok());
    let subject_url = format!("https://movie.douban.com/subject/{id}/");
    let page = fetch_douban_html(&client, &subject_url).await?;
    let Some(title) = douban_page_capture(&page, douban_title_re()) else {
        // The page could not be parsed (challenge unresolved, layout change):
        // report no match so the remaining providers still get a chance.
        return Ok(None);
    };
    let year = douban_page_capture(&page, douban_year_re())
        .and_then(|year| year.parse::<i64>().ok())
        .or(suggest_year);
    let rating = douban_page_capture(&page, douban_rating_re())
        .and_then(|rating| rating.trim().parse::<f64>().ok())
        .filter(|rating| *rating > 0.0);
    let art = douban_page_capture(&page, douban_poster_re());
    let summary = {
        let raw = douban_page_capture(&page, douban_summary_re()).unwrap_or_default();
        clean_summary(&raw)
    };
    let genres = douban_genre_re()
        .captures_iter(&page)
        .filter_map(|caps| caps.get(1).map(|genre| genre.as_str().trim().to_owned()))
        .filter(|genre| !genre.is_empty())
        .collect::<Vec<_>>();
    Ok(Some(Match {
        title,
        original_title: None,
        year,
        rating,
        art,
        backdrop: None,
        summary,
        external_id: id,
        media_type: media_type.to_owned(),
        adult: false,
        genres,
    }))
}

/// Fetch a Douban page, solving the `sec.douban.com` proof-of-work
/// interstitial when it appears. The flow mirrors what the interstitial's
/// JavaScript does in a browser: GET the guarded page until it answers 302 →
/// fetch the challenge form → find the nonce making `sha512(cha + nonce)`
/// start with four hex zeros → POST the answer → retry the original page,
/// now that the session cookie is in the jar.
async fn fetch_douban_html(client: &Client, url: &str) -> Result<String, AppError> {
    let mut current = url.to_owned();
    for _ in 0..6 {
        let response = client
            .get(&current)
            .header("Referer", "https://movie.douban.com/")
            .send()
            .await
            .map_err(|e| AppError::Provider(format!("豆瓣页面请求失败: {e}")))?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            if location.is_empty() {
                break;
            }
            current = resolve_douban_url(&current, &location)?;
            continue;
        }
        let body = response
            .text()
            .await
            .map_err(|e| AppError::Provider(format!("豆瓣页面读取失败: {e}")))?;
        let Some(cha) = douban_form_field(&body, "cha") else {
            return Ok(body);
        };
        let Some(tok) = douban_form_field(&body, "tok") else {
            return Ok(body);
        };
        let red = douban_form_field(&body, "red").unwrap_or_else(|| url.to_owned());
        let sol = douban_pow_nonce(&cha);
        if sol.is_empty() {
            return Err(AppError::Provider("豆瓣人机验证工作量证明求解失败".into()));
        }
        let post = client
            .post("https://sec.douban.com/c")
            .header("Referer", "https://sec.douban.com/")
            .header("Origin", "https://sec.douban.com")
            .form(&[
                ("tok", tok.as_str()),
                ("cha", cha.as_str()),
                ("sol", sol.as_str()),
                ("red", red.as_str()),
            ])
            .send()
            .await
            .map_err(|e| AppError::Provider(format!("豆瓣验证提交失败: {e}")))?;
        if post.status().is_redirection() {
            let location = post
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            if !location.is_empty() {
                current = resolve_douban_url("https://sec.douban.com/c", &location)?;
                continue;
            }
        }
        // No redirect hint: the session cookie is set by now, so retry the
        // original page directly.
        current = url.to_owned();
    }
    Err(AppError::Provider("豆瓣页面跳转次数过多".into()))
}

/// Resolve a `Location` header against the request URL, accepting absolute
/// and relative targets.
fn resolve_douban_url(base: &str, location: &str) -> Result<String, AppError> {
    if location.starts_with("http://") || location.starts_with("https://") {
        return Ok(location.to_owned());
    }
    reqwest::Url::parse(base)
        .and_then(|base| base.join(location))
        .map(|url| url.to_string())
        .map_err(|e| AppError::Provider(format!("豆瓣跳转地址解析失败: {e}")))
}

/// Solve the interstitial proof of work: smallest nonce where
/// `sha512(cha + nonce)` starts with four hex zeros (difficulty 4, ~65k
/// hashes on average). Returns an empty string when no nonce is found within
/// a generous cap so callers fail fast instead of spinning forever.
fn douban_pow_nonce(cha: &str) -> String {
    const MAX_NONCE: u64 = 20_000_000;
    let cha = cha.as_bytes();
    for nonce in 1..=MAX_NONCE {
        let mut hasher = Sha512::new();
        hasher.update(cha);
        hasher.update(nonce.to_string().as_bytes());
        let digest = hasher.finalize();
        // Four leading hex zeros == two leading zero bytes; only pay for the
        // hex formatting when the cheap byte check passes.
        if digest[0] == 0 && digest[1] == 0 {
            let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            if hex.starts_with("0000") {
                return nonce.to_string();
            }
        }
    }
    String::new()
}

/// Extract a hidden form field value (`id="x" name="x" value="..."`) from the
/// interstitial HTML.
fn douban_form_field(html: &str, field: &str) -> Option<String> {
    let pattern = format!(r#"id="{field}" name="{field}" value="([^"]*)""#);
    Regex::new(&pattern)
        .ok()?
        .captures(html)?
        .get(1)
        .map(|value| value.as_str().to_owned())
}

fn douban_title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"<span property="v:itemreviewed">([^<]+)</span>"#).expect("regex")
    })
}

fn douban_year_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"class="year">\((\d{4})\)</span>"#).expect("regex"))
}

fn douban_rating_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"property="v:average">([^<]*)</strong>"#).expect("regex"))
}

fn douban_poster_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"property="og:image" content="([^"]+)""#).expect("regex"))
}

fn douban_summary_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?s)property="v:summary"[^>]*>(.*?)</span>"#).expect("regex"))
}

fn douban_genre_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"property="v:genre">([^<]+)</span>"#).expect("regex"))
}

/// First capture group of the first match, trimmed.
fn douban_page_capture(page: &str, pattern: &Regex) -> Option<String> {
    pattern
        .captures(page)
        .and_then(|caps| caps.get(1))
        .map(|value| value.as_str().trim().to_owned())
        .filter(|value| !value.is_empty())
}

async fn scrape_tmdb(
    client: &Client,
    query: &str,
    include_adult: bool,
    credential: Option<&TmdbCredential>,
) -> Result<Option<Match>, AppError> {
    let Some(credential) = credential else {
        return Ok(None);
    };
    let mut request = client
        .get("https://api.themoviedb.org/3/search/multi")
        .query(&[
            ("query", query),
            ("language", "zh-CN"),
            (
                "include_adult",
                if include_adult { "true" } else { "false" },
            ),
        ]);
    match credential {
        TmdbCredential::ReadToken(token) => {
            request = request.bearer_auth(token.clone());
        }
        TmdbCredential::ApiKey(key) => {
            request = request.query(&[("api_key", key.clone())]);
        }
    }
    let value: Value = request
        .send()
        .await
        .map_err(|e| AppError::Provider(format!("TMDB request failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Provider(format!("TMDB response failed: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Provider(format!("TMDB JSON failed: {e}")))?;
    let Some(result) = value
        .get("results")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter().find(|v| {
                v.get("media_type")
                    .and_then(Value::as_str)
                    .map(|t| t == "movie" || t == "tv")
                    .unwrap_or(false)
            })
        })
    else {
        return Ok(None);
    };
    let title = result
        .get("title")
        .or_else(|| result.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(query)
        .to_owned();
    let original = result
        .get("original_title")
        .or_else(|| result.get("original_name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let date = result
        .get("release_date")
        .or_else(|| result.get("first_air_date"))
        .and_then(Value::as_str);
    let year = date.and_then(|v| v.get(0..4)).and_then(|v| v.parse().ok());
    let rating = result.get("vote_average").and_then(Value::as_f64);
    let art = result
        .get("poster_path")
        .and_then(Value::as_str)
        .map(|p| format!("https://image.tmdb.org/t/p/w780{p}"));
    let backdrop = result
        .get("backdrop_path")
        .and_then(Value::as_str)
        .map(|p| format!("https://image.tmdb.org/t/p/w1280{p}"));
    let summary = clean_summary(result.get("overview").and_then(Value::as_str).unwrap_or(""));
    let id = result
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or_default()
        .to_string();
    Ok(Some(Match {
        title,
        original_title: original,
        year,
        rating,
        art,
        backdrop,
        summary,
        external_id: id,
        adult: result
            .get("adult")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        media_type: result
            .get("media_type")
            .and_then(Value::as_str)
            .unwrap_or("video")
            .to_owned(),
        genres: result
            .get("genre_ids")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_i64)
                    .filter_map(|id| tmdb_genre_label(id).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    }))
}

async fn scrape_tmdb_by_id(
    client: &Client,
    tmdb_id: &str,
    credential: Option<&TmdbCredential>,
) -> Result<Option<Match>, AppError> {
    let Some(credential) = credential else {
        return Ok(None);
    };
    for media_type in ["movie", "tv"] {
        let mut request = client
            .get(format!("https://api.themoviedb.org/3/{media_type}/{tmdb_id}"))
            .query(&[("language", "zh-CN")]);
        match credential {
            TmdbCredential::ReadToken(token) => {
                request = request.bearer_auth(token.clone());
            }
            TmdbCredential::ApiKey(key) => {
                request = request.query(&[("api_key", key.clone())]);
            }
        }
        let response = request.send().await.map_err(|e| {
            AppError::Provider(format!("TMDB id request failed: {e}"))
        })?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        let value: Value = response
            .error_for_status()
            .map_err(|e| AppError::Provider(format!("TMDB id response failed: {e}")))?
            .json()
            .await
            .map_err(|e| AppError::Provider(format!("TMDB id JSON failed: {e}")))?;
        if value.get("id").and_then(Value::as_i64).is_none() {
            continue;
        }
        let title = value
            .get("title")
            .or_else(|| value.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if title.is_empty() {
            continue;
        }
        let original = value
            .get("original_title")
            .or_else(|| value.get("original_name"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let date = value
            .get("release_date")
            .or_else(|| value.get("first_air_date"))
            .and_then(Value::as_str);
        let year = date.and_then(|v| v.get(0..4)).and_then(|v| v.parse().ok());
        let genres = value
            .get("genres")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.get("id")
                            .and_then(Value::as_i64)
                            .and_then(tmdb_genre_label)
                            .map(str::to_owned)
                            .or_else(|| {
                                item.get("name")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Ok(Some(Match {
            title,
            original_title: original,
            year,
            rating: value.get("vote_average").and_then(Value::as_f64),
            art: value
                .get("poster_path")
                .and_then(Value::as_str)
                .map(|p| format!("https://image.tmdb.org/t/p/w780{p}")),
            backdrop: value
                .get("backdrop_path")
                .and_then(Value::as_str)
                .map(|p| format!("https://image.tmdb.org/t/p/w1280{p}")),
            summary: clean_summary(value.get("overview").and_then(Value::as_str).unwrap_or("")),
            external_id: tmdb_id.to_string(),
            adult: value.get("adult").and_then(Value::as_bool).unwrap_or(false),
            media_type: media_type.to_string(),
            genres,
        }));
    }
    Ok(None)
}

fn folder_name_of(item: &MediaRecord) -> Option<String> {
    let payload = item.payload.as_ref()?.as_object()?;
    if let Some(name) = payload
        .get("folderName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "根目录")
    {
        return Some(name.to_string());
    }
    payload
        .get("folderPath")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|path| {
            path.rsplit(['/', '\\'])
                .find(|part| !part.is_empty() && *part != "根目录")
                .map(str::to_string)
        })
}

/// Re-isolate 18+ records that a batch "mark as normal" import hid in the main
/// library. A confirmed JAV scrape or adult genre list always wins over that
/// stamp; per-item `library_set_adult` decisions (`adultManualSource=user`)
/// are left untouched.
pub fn repair_adult_isolation(item: &mut MediaRecord) -> bool {
    let Some(payload) = item.payload.as_ref().and_then(Value::as_object) else {
        return apply_adult_classification(item);
    };
    if payload.get("adult").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    if payload.get("adultManualSource").and_then(Value::as_str) == Some("user")
        && payload.get("adultManual").and_then(Value::as_bool) == Some(true)
        && payload.get("adult").and_then(Value::as_bool) == Some(false)
    {
        return false;
    }
    let scraped_jav = payload
        .get("scrapedBy")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("jav"))
        || payload.get("jav").is_some_and(Value::is_object);
    let adult_genres = looks_adult_genre(item.payload.as_ref());
    if !scraped_jav && !adult_genres && !looks_adult_media(item) {
        return false;
    }
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let obj = payload.as_object_mut().expect("object payload");
    obj.insert("adult".into(), Value::Bool(true));
    obj.insert("contentRating".into(), Value::String("18+".into()));
    obj.remove("adultManual");
    obj.remove("adultManualSource");
    obj.entry("genres")
        .or_insert_with(|| serde_json::json!(["成人"]));
    item.payload = Some(payload);
    true
}

fn tmdb_genre_label(id: i64) -> Option<&'static str> {
    Some(match id {
        12 => "冒险",
        14 => "奇幻",
        16 => "动画",
        18 => "剧情",
        27 => "恐怖",
        28 => "动作",
        35 => "喜剧",
        36 => "历史",
        37 => "西部",
        53 => "惊悚",
        80 => "犯罪",
        99 => "纪录片",
        878 => "科幻",
        9648 => "悬疑",
        10402 => "音乐",
        10749 => "爱情",
        10751 => "家庭",
        10752 => "战争",
        10770 => "电视电影",
        10759 => "动作冒险",
        10762 => "儿童",
        10763 => "新闻",
        10764 => "真人秀",
        10765 => "科幻奇幻",
        10766 => "肥皂剧",
        10767 => "脱口秀",
        10768 => "战争政治",
        _ => return None,
    })
}

/// Classify a media record as 18+ based on its payload, rating and file name.
///
/// This is the single source of truth for adult detection. It is run during
/// scraping, at import time (local scan + provider sync) and by the
/// reclassification sweep, so that `payload.adult` is reliable even before a
/// network scrape happens.
pub fn looks_adult_media(item: &MediaRecord) -> bool {
    let payload = item.payload.as_ref();
    let metadata = payload
        .and_then(|value| value.get("metadata"))
        .filter(|value| value.is_object());
    let explicit = metadata
        .or(payload)
        .and_then(|value| value.as_object())
        .and_then(|obj| {
            ["adult", "isAdult", "18plus", "is18Plus"]
                .iter()
                .find_map(|key| obj.get(*key).and_then(Value::as_bool))
        })
        .unwrap_or(false);
    if explicit {
        return true;
    }
    let rating = metadata
        .or(payload)
        .and_then(|value| {
            value
                .get("contentRating")
                .or_else(|| value.get("vod_remarks"))
                .or_else(|| value.get("rating"))
        })
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if ["18+", "nc-17", "xxx", "adult", "nsfw"]
        .iter()
        .any(|marker| rating.contains(marker))
    {
        return true;
    }
    if looks_adult_genre(payload) {
        return true;
    }
    // Signed cloud URLs can carry arbitrary values in their query string.
    // Restrict keyword checks to the stable path so transport parameters never
    // classify an otherwise normal film as 18+.
    let source_path = item
        .remote_path
        .as_deref()
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let value = format!("{} {}", item.title, source_path).to_ascii_lowercase();
    if [
        "18+", "porn", "nsfw", "hentai", "xxx", "adult", "无码", "有码", "色情", "成人",
    ]
    .iter()
    .any(|marker| value.contains(marker))
    {
        return true;
    }
    // A JAV-code shape in the file name is a 18+ signal on its own. The
    // relaxed check isolates everything the scrape's JAV routing would claim
    // (including two-digit codes), but guards against ordinary titles like
    // "Level 03" / "Movie 2020". Classifying at import time — instead of
    // waiting for the network-bound scrape — closes the window where 18+
    // content would otherwise leak into the main library.
    let name = item.remote_path.as_deref().unwrap_or(item.title.as_str());
    crate::adult::code::looks_like_jav_name_relaxed(name)
}

/// Genre labels that only occur on adult content in cloud-drive metadata. Any
/// match classifies the record as 18+ even when the file name carries no JAV
/// code or 18+ marker — the cloud provider has already done the work for us.
///
/// Genre-only by design: titles and paths are still judged by the narrower
/// keyword list in [`looks_adult_media`], because words like 巨乳 are safe as a
/// category label but would misclassify ordinary titles as substring matches.
const ADULT_GENRE_MARKERS: &[&str] = &[
    "成人",
    "口交",
    "內射",
    "内射",
    "獨佔動畫",
    "独占动画",
    "舔陰",
    "舔阴",
    "痴女",
    "顏射",
    "颜射",
    "口內射精",
    "口内射精",
    "亂交",
    "乱交",
    "乳交",
    "無毛",
    "无毛",
    "潮吹",
    "吞精",
    "凌辱",
    "淫語",
    "淫语",
    "自慰",
    "手淫",
    "束縛",
    "束缚",
    "露出",
    "巨乳",
    "美乳",
    "微乳",
    "人妻",
    "熟女",
    "女教師",
    "女教师",
    "亂倫",
    "乱伦",
    "蘿莉塔",
    "萝莉塔",
    "蘿莉",
    "萝莉",
    "無碼",
    "无码",
    "有码",
    "素人",
    "裏番",
    "里番",
    "振動",
    "振动",
    "打飛機",
    "打飞机",
    "肛門",
    "肛门",
    "車震",
    "车震",
    "變態",
    "变态",
    "射在外陰",
    "射在外阴",
    "鴨嘴",
    "鸭嘴",
    "奴役",
    "早洩",
    "早泄",
    "色狼",
    "多P",
    "SM",
];

/// Check the genre labels attached to a record against
/// [`ADULT_GENRE_MARKERS`]. Labels are read from the payload and the provider
/// metadata card (`metadata` / `metadata.streamhub`): `genres` / `genreLabels`
/// as string arrays, and `vod_class` / `vodClass` / `vod_class_name` / `genre`
/// as the TVBox-compatible delimited strings cloud-drive providers fill. The
/// exact-match comparison keeps partially-overlapping names like "美腿" (also
/// a normal photography tag) from triggering alone.
fn looks_adult_genre(payload: Option<&Value>) -> bool {
    let Some(payload) = payload.and_then(Value::as_object) else {
        return false;
    };
    let mut labels: Vec<String> = Vec::new();
    let mut collect = |obj: &serde_json::Map<String, Value>| {
        for key in ["genres", "genreLabels"] {
            if let Some(values) = obj.get(key).and_then(Value::as_array) {
                labels.extend(
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|value| value.trim().to_string()),
                );
            }
        }
        for key in ["vod_class", "vodClass", "vod_class_name", "genre"] {
            if let Some(value) = obj.get(key).and_then(Value::as_str) {
                labels.extend(
                    value
                        .split([',', '，', '|', '/'])
                        .map(|part| part.trim().to_string())
                        .filter(|part| !part.is_empty()),
                );
            }
        }
    };
    collect(payload);
    if let Some(metadata) = payload.get("metadata").filter(|value| value.is_object()) {
        collect(metadata.as_object().expect("object metadata"));
        if let Some(card) = metadata
            .get("streamhub")
            .filter(|value| value.is_object())
            .and_then(Value::as_object)
        {
            collect(card);
        }
    }
    labels.iter().any(|label| {
        ADULT_GENRE_MARKERS
            .iter()
            .any(|marker| label.eq_ignore_ascii_case(marker))
    })
}

/// Returns the user's manual 18+ decision if one was recorded
/// (`payload.adultManual == true`), otherwise `None`. A manual choice is
/// authoritative: every scrape/merge path that writes `payload.adult` consults
/// this first so an explicit "is / isn't 18+" decision is never overridden by a
/// later scrape, filename hint, or reclassify sweep.
fn manual_adult_override(obj: &serde_json::Map<String, Value>) -> Option<bool> {
    if obj
        .get("adultManual")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Some(obj.get("adult").and_then(Value::as_bool).unwrap_or(false))
    } else {
        None
    }
}

/// Mark a record as 18+ when [`looks_adult_media`] says so and it is not
/// already flagged. Additive only: it never clears an existing adult flag and
/// never clobbers fields set by a real scrape (summary, scrapedBy, ...). This
/// is safe to run at import time and over the whole library as a reclassify
/// sweep. Returns `true` when the record was newly flagged.
///
/// A manual decision (`payload.adultManual == true`) is authoritative: this
/// function then leaves the record untouched so a user's explicit 18+ / not-18+
/// choice survives scraping and reclassify sweeps.
pub fn apply_adult_classification(item: &mut MediaRecord) -> bool {
    let manual = item
        .payload
        .as_ref()
        .and_then(|value| value.get("adultManual"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if manual {
        return false;
    }
    if !looks_adult_media(item) {
        return false;
    }
    let already = item
        .payload
        .as_ref()
        .and_then(|value| value.get("adult"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if already {
        return false;
    }
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let obj = payload.as_object_mut().expect("object payload");
    obj.insert("adult".into(), Value::Bool(true));
    obj.insert("contentRating".into(), Value::String("18+".into()));
    let has_genre = obj
        .get("genres")
        .and_then(Value::as_array)
        .is_some_and(|genres| !genres.is_empty());
    if !has_genre {
        obj.insert(
            "genres".into(),
            Value::Array(vec![Value::String("成人".into())]),
        );
    }
    obj.entry("metadataSource")
        .or_insert_with(|| Value::String("local-classifier".into()));
    item.payload = Some(payload);
    true
}

/// Record an explicit user decision about whether an item is 18+.
///
/// Sets `payload.adult` to `adult` and stamps `payload.adultManual = true` so
/// [`apply_adult_classification`] (import-time classification, scraping and the
/// reclassify sweep) will never override it again. Passing `adult = false`
/// removes the item from the 18+ view and keeps it out permanently, which is
/// how a wrongly-flagged normal video is corrected.
pub fn set_manual_adult(item: &mut MediaRecord, adult: bool) {
    let mut payload = item.payload.take().unwrap_or_else(|| serde_json::json!({}));
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    let obj = payload.as_object_mut().expect("object payload");
    obj.insert("adult".into(), Value::Bool(adult));
    obj.insert("adultManual".into(), Value::Bool(true));
    obj.insert("adultManualSource".into(), Value::String("user".into()));
    if adult {
        obj.insert("contentRating".into(), Value::String("18+".into()));
        let has_genre = obj
            .get("genres")
            .and_then(Value::as_array)
            .is_some_and(|genres| !genres.is_empty());
        if !has_genre {
            obj.insert(
                "genres".into(),
                Value::Array(vec![Value::String("成人".into())]),
            );
        }
    } else {
        obj.insert("contentRating".into(), Value::String(String::new()));
        if let Some(genres) = obj.get_mut("genres").and_then(Value::as_array_mut) {
            genres.retain(|genre| genre.as_str() != Some("成人"));
        }
    }
    item.payload = Some(payload);
}

/// Undo the adult-classifier's markers on a record so classification can be
/// re-run from a clean slate. Only touches fields the classifier itself writes
/// (`adult`, an `18+` contentRating, the `成人` genre, a classifier
/// metadataSource); real scrape output and manual decisions are left intact.
/// Used by the rebuild sweep to clear stale false positives (e.g. a normal
/// "Level 03" video flagged by an older, looser heuristic).
pub fn clear_classifier_adult(item: &mut MediaRecord) {
    let Some(payload) = item.payload.as_mut() else {
        return;
    };
    let Some(obj) = payload.as_object_mut() else {
        return;
    };
    obj.remove("adult");
    if obj.get("contentRating").and_then(Value::as_str) == Some("18+") {
        obj.remove("contentRating");
    }
    if let Some(genres) = obj.get_mut("genres").and_then(Value::as_array_mut) {
        genres.retain(|genre| genre.as_str() != Some("成人"));
    }
    let jav_classifier =
        obj.get("metadataSource").and_then(Value::as_str) == Some("jav-classifier");
    if matches!(
        obj.get("metadataSource").and_then(Value::as_str),
        Some("local-classifier") | Some("jav-classifier")
    ) {
        obj.remove("metadataSource");
    }
    if jav_classifier {
        // Pending JAV metadata is generated entirely by the classifier. Drop
        // it during a rebuild so an old false positive cannot keep showing a
        // foreign code or placeholder summary on a normal media detail page.
        obj.remove("jav");
        obj.remove("externalId");
        obj.remove("matchedTitle");
        if obj
            .get("summary")
            .and_then(Value::as_str)
            .is_some_and(|summary| summary.starts_with("已识别番号 "))
        {
            obj.remove("summary");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_adult_classification, clean_summary, clear_classifier_adult,
        clear_stale_jav_unmatched, display_title, douban_pow_nonce, extract_tmdb_id,
        looks_adult_media, normalize_title, query_variants, repair_adult_isolation,
        set_manual_adult, tmdb_genre_label,
    };
    use crate::storage::MediaRecord;

    #[test]
    fn normalizes_common_release_names() {
        assert_eq!(
            normalize_title("Breaking.Bad.S01E01.1080p.WEB-DL.mkv"),
            "Breaking Bad"
        );
        assert_eq!(
            normalize_title("The.Matrix.1999.2160p.BluRay.mkv"),
            "The Matrix"
        );
        assert_eq!(normalize_title("  Avatar_2009  "), "Avatar");
    }

    #[test]
    fn normalizes_episode_release_names_before_metadata_lookup() {
        assert_eq!(
            normalize_title(
                "Good.Omens.S03E01.The.Finale.2160p.REPACK.AMZN.WEB-DL.DDP5.1.HDR.H.265-NTb.mkv"
            ),
            "Good Omens"
        );
        assert_eq!(
            normalize_title(
                "House.of.Cards.2013.S01E01.Chapter.1.1080p.BluRay.REMUX.AVC.DTS-HD.MA.5.1-NOGRP.mkv"
            ),
            "House of Cards"
        );
        assert_eq!(
            normalize_title(
                "The.Boys.S02E02.Proper.Preparation.and.Planning.2160p.AMZN.WEB-DL.DDP.5.1.HDR10+.H.265-BlackTV.mkv"
            ),
            "The Boys"
        );
    }

    #[test]
    fn strips_cloud_collection_labels_before_metadata_lookup() {
        assert_eq!(normalize_title("小狐狸分级 - Level 03.mp4"), "小狐狸分级");
        assert_eq!(normalize_title("小狐狸分级｜Level 03.mp4"), "小狐狸分级");
        assert_eq!(normalize_title("小狐狸分级：Level 03.mp4"), "小狐狸分级");
    }
    #[test]
    fn keeps_episode_identity_after_matching_a_series() {
        assert_eq!(
            display_title(
                "Good Omens",
                "Good.Omens.S03E01.The.Finale.2160p.WEB-DL.mkv"
            ),
            "Good Omens · S03E01"
        );
        assert_eq!(
            display_title("The Matrix", "The.Matrix.1999.mkv"),
            "The Matrix"
        );
    }

    #[test]
    fn cleans_catalog_summary_markup() {
        assert_eq!(
            clean_summary("<p>The <i>Nice &amp; Accurate</i> Prophecies&nbsp;return.</p>"),
            "The Nice & Accurate Prophecies return."
        );
    }

    #[test]
    fn classifies_adult_metadata_from_source_payload_and_title() {
        let mut media = MediaRecord::new("adult-1", "video", "Example Release");
        media.payload = Some(serde_json::json!({"contentRating":"18+"}));
        assert!(looks_adult_media(&media));

        let mut title_media = MediaRecord::new("adult-2", "video", "Sample.XXX.1080p.mp4");
        assert!(looks_adult_media(&title_media));
        title_media.title = "Family.Movie.1080p.mp4".into();
        assert!(!looks_adult_media(&title_media));
    }

    #[test]
    fn classifies_adult_from_provider_genres() {
        // Cloud-drive entries carry their genre labels in payload.genres (or the
        // nested provider metadata); that alone must trigger 18+ classification
        // even when the file name has no JAV code or marker at all.
        let mut media = MediaRecord::new("adult-3", "video", "00000.mp4");
        media.payload = Some(serde_json::json!({
            "metadata": {"genres": ["口交", "巨乳", "痴女"]}
        }));
        assert!(looks_adult_media(&media));
        assert!(apply_adult_classification(&mut media));
        assert_eq!(
            media
                .payload
                .as_ref()
                .and_then(|p| p.get("adult"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );

        // TVBox-style vod_class string and top-level genres also count.
        let mut vod_class = MediaRecord::new("adult-4", "video", "00001.mp4");
        vod_class.payload = Some(serde_json::json!({
            "metadata": {"vod_class": "成人,口交|巨乳"}
        }));
        assert!(looks_adult_media(&vod_class));

        let mut top_level = MediaRecord::new("adult-5", "video", "00002.mp4");
        top_level.payload = Some(serde_json::json!({"genres": ["無毛", "潮吹"]}));
        assert!(looks_adult_media(&top_level));

        // StreamHub-style nested card genreLabels count too.
        let mut card = MediaRecord::new("adult-7", "video", "00003.mp4");
        card.payload = Some(serde_json::json!({
            "metadata": {"streamhub": {"genreLabels": ["痴女", "美腿"]}}
        }));
        assert!(looks_adult_media(&card));

        // Non-adult genre labels must not trigger classification.
        let mut normal = MediaRecord::new("adult-6", "video", "Drama.Movie.2020.mp4");
        normal.payload = Some(serde_json::json!({"genres": ["剧情", "科幻", "美腿"]}));
        assert!(!looks_adult_media(&normal));
        assert!(!apply_adult_classification(&mut normal));
    }

    #[test]
    fn apply_adult_classification_flags_once_and_is_additive() {
        // An adult-titled record gets flagged on first pass.
        let mut media = MediaRecord::new("c-1", "video", "Some.Porn.Movie.mp4");
        assert!(apply_adult_classification(&mut media));
        let payload = media.payload.as_ref().expect("payload set");
        assert_eq!(payload.get("adult").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            payload.get("contentRating").and_then(|v| v.as_str()),
            Some("18+")
        );
        // Second pass is a no-op (already flagged) so re-sweeps stay idempotent.
        assert!(!apply_adult_classification(&mut media));

        // A normal record is left untouched.
        let mut normal = MediaRecord::new("c-2", "video", "Family.Movie.1080p.mp4");
        assert!(!apply_adult_classification(&mut normal));
        assert!(normal.payload.is_none());
    }

    #[test]
    fn manual_adult_decision_is_authoritative() {
        // Marking an adult-looking record as NOT adult keeps it out of the 18+
        // view, and the classifier must never re-flag it afterwards.
        let mut media = MediaRecord::new("m-1", "video", "Some.Porn.Movie.mp4");
        assert!(apply_adult_classification(&mut media));
        set_manual_adult(&mut media, false);
        let payload = media.payload.as_ref().expect("payload set");
        assert_eq!(payload.get("adult").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            payload.get("adultManual").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            payload.get("contentRating").and_then(|v| v.as_str()),
            Some("")
        );
        // Reclassify sweep respects the manual choice and does not re-flag.
        assert!(!apply_adult_classification(&mut media));
        assert_eq!(
            media
                .payload
                .as_ref()
                .and_then(|p| p.get("adult"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );

        // Marking a normal record as adult flags it and that also sticks.
        let mut normal = MediaRecord::new("m-2", "video", "Family.Movie.1080p.mp4");
        set_manual_adult(&mut normal, true);
        assert_eq!(
            normal
                .payload
                .as_ref()
                .and_then(|p| p.get("adult"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(!apply_adult_classification(&mut normal));
        assert_eq!(
            normal
                .payload
                .as_ref()
                .and_then(|p| p.get("adult"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn rebuild_clears_stale_false_positives_but_keeps_real_codes() {
        // A normal video wrongly flagged by an older, looser classifier is
        // cleared by the clear + re-classify rebuild pass.
        let mut level = MediaRecord::new("r-1", "video", "小狐狸分级 - Level 03.mp4");
        level.payload = Some(serde_json::json!({
            "adult": true,
            "contentRating": "18+",
            "genres": ["成人"],
            "metadataSource": "local-classifier"
        }));
        clear_classifier_adult(&mut level);
        assert!(!apply_adult_classification(&mut level));
        assert_eq!(
            level
                .payload
                .as_ref()
                .and_then(|p| p.get("adult"))
                .and_then(|v| v.as_bool()),
            None
        );

        // A real JAV code stays flagged after the same clear + re-classify pass.
        let mut jav = MediaRecord::new("r-2", "video", "IPX-633.mp4");
        jav.payload = Some(serde_json::json!({
            "adult": true,
            "contentRating": "18+",
            "genres": ["成人"],
            "metadataSource": "local-classifier"
        }));
        clear_classifier_adult(&mut jav);
        assert!(apply_adult_classification(&mut jav));
        assert_eq!(
            jav.payload
                .as_ref()
                .and_then(|p| p.get("adult"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn import_time_classification_isolates_two_digit_codes() {
        // A two-digit JAV code used to slip past the strict import-time check
        // and only got isolated once the (network-bound) scrape finished — the
        // window where 18+ content leaked onto the home page. The relaxed
        // classifier isolates it at import time.
        let mut two_digit = MediaRecord::new("t-1", "video", "PT-82.mp4");
        assert!(looks_adult_media(&two_digit));
        assert!(apply_adult_classification(&mut two_digit));
        assert_eq!(
            two_digit
                .payload
                .as_ref()
                .and_then(|p| p.get("adult"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );

        // Ordinary "word + number" titles must still stay out of the 18+ zone.
        let mut level = MediaRecord::new("t-2", "video", "小狐狸分级 - Level 03.mp4");
        assert!(!looks_adult_media(&level));
        assert!(!apply_adult_classification(&mut level));
    }

    #[test]
    fn maps_tmdb_genre_ids_to_readable_labels() {
        assert_eq!(tmdb_genre_label(18), Some("剧情"));
        assert_eq!(tmdb_genre_label(878), Some("科幻"));
        assert_eq!(tmdb_genre_label(10765), Some("科幻奇幻"));
        assert_eq!(tmdb_genre_label(99999), None);
    }

    #[test]
    fn chinese_titles_keep_their_full_text_as_the_query() {
        // A full-width colon inside a Chinese title is part of the name, not a
        // release-name separator. Cutting it used to query "007" and match the
        // wrong movie.
        assert_eq!(
            normalize_title("007：大破量子危机 (2008) - 2160p.HDR10+.mkv"),
            "007：大破量子危机"
        );
        assert_eq!(
            normalize_title("大破天幕杀机 (2012) - 1080p.HDR.mkv"),
            "大破天幕杀机"
        );
        // An ASCII colon whose suffix is junk keeps the old cut behavior.
        assert_eq!(normalize_title("小狐狸分级：Level 03.mp4"), "小狐狸分级");
        // English subtitle titles gain a specific-first variant pair.
        assert_eq!(
            query_variants("Spider-Man: No Way Home (2021) - 2160p.WEB-DL.mkv"),
            vec![
                "Spider-Man: No Way Home".to_string(),
                "Spider-Man".to_string()
            ]
        );
        // Plain titles produce exactly one variant.
        assert_eq!(
            query_variants("The.Matrix.1999.2160p.BluRay.mkv"),
            vec!["The Matrix".to_string()]
        );
        assert_eq!(
            extract_tmdb_id("最后的呼吸.2025.2160p.BluRay.{tmdb-972533}.mkv").as_deref(),
            Some("972533")
        );
        let chinese_release =
            query_variants("杨乃武与小白菜.The.Adulteress.1962.R3.SB.DVDRip.x264.mkv");
        assert!(
            chinese_release.iter().any(|query| query == "杨乃武与小白菜"),
            "expected CJK title variant, got {chinese_release:?}"
        );
        assert!(
            chinese_release.iter().any(|query| query == "The Adulteress"),
            "expected English title variant, got {chinese_release:?}"
        );
    }

    #[test]
    fn repair_isolates_jav_hidden_by_batch_normal_stamp() {
        let mut jav = MediaRecord::new("jav-1", "video", "CRC-032.mp4");
        jav.payload = Some(serde_json::json!({
            "adult": false,
            "adultManual": true,
            "scrapedBy": "jav",
            "genres": ["巨乳", "口交"]
        }));
        assert!(repair_adult_isolation(&mut jav));
        let payload = jav.payload.as_ref().and_then(|p| p.as_object()).unwrap();
        assert_eq!(payload.get("adult").and_then(|v| v.as_bool()), Some(true));
        assert!(payload.get("adultManual").is_none());

        let mut kept = MediaRecord::new("user-1", "video", "CRC-032.mp4");
        kept.payload = Some(serde_json::json!({
            "adult": false,
            "adultManual": true,
            "adultManualSource": "user",
            "scrapedBy": "jav"
        }));
        assert!(!repair_adult_isolation(&mut kept));
    }

    #[test]
    fn douban_pow_nonce_solves_known_shape() {
        // The solver must find a nonce whose sha512(cha + nonce) starts with
        // four hex zeros, and it must be deterministic for a given challenge.
        let cha = "0123456789abcdef";
        let nonce = douban_pow_nonce(cha);
        assert!(!nonce.is_empty());
        assert_eq!(nonce, douban_pow_nonce(cha));
        let digest = {
            use sha2::{Digest, Sha512};
            let mut hasher = Sha512::new();
            hasher.update(cha.as_bytes());
            hasher.update(nonce.as_bytes());
            hasher.finalize()
        };
        assert_eq!(&digest[..2], &[0, 0]);
    }

    #[test]
    fn stale_jav_classification_is_cleared_but_manual_decisions_survive() {
        let mut media = MediaRecord::new("stale-1", "video", "007：大破量子危机 (2008).mkv");
        media.remote_path = Some("D:/media/007：大破量子危机 (2008) - 2160p.HDR10.mkv".into());
        media.payload = Some(serde_json::json!({
            "adult": true,
            "contentRating": "18+",
            "genres": ["成人"],
            "metadataSource": "jav-classifier",
            "summary": "已识别番号 HDR-010，等待 JavBus / JavDB / Avmoo / JavLibrary / Jav321 补全作品资料。",
            "jav": {"code": "HDR-010", "status": "not-found"}
        }));
        assert!(clear_stale_jav_unmatched(&mut media));
        let payload = media.payload.as_ref().and_then(|p| p.as_object()).unwrap();
        assert!(payload.get("adult").is_none());
        assert!(payload.get("jav").is_none());
        assert!(payload.get("summary").is_none());
        assert!(!looks_adult_media(&media));

        // A manual decision is authoritative and must never be cleared.
        let mut manual = MediaRecord::new("stale-2", "video", "Some Release");
        manual.payload = Some(serde_json::json!({
            "adult": true,
            "adultManual": true,
            "metadataSource": "jav-classifier",
            "jav": {"code": "ABP-356", "status": "not-found"}
        }));
        assert!(!clear_stale_jav_unmatched(&mut manual));
        assert_eq!(
            manual
                .payload
                .as_ref()
                .and_then(|p| p.get("adult"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );

        // Real JAV items stay classified (their file name still carries the
        // code, so the next scrape re-isolates them immediately).
        let mut real = MediaRecord::new("stale-3", "video", "IPX-633.mp4");
        real.remote_path = Some("D:/media/IPX-633.mp4".into());
        real.payload = Some(serde_json::json!({
            "adult": true,
            "metadataSource": "jav-classifier",
            "jav": {"code": "IPX-633", "status": "not-found"}
        }));
        assert!(clear_stale_jav_unmatched(&mut real));
        assert!(looks_adult_media(&real));
    }
}
