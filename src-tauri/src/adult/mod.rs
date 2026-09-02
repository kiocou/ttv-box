//! Adult (JAV) metadata scraping.
//!
//! Architecture ported from JavBoss (reference clone in `.tmp-javboss/`):
//! file-name code extraction (`code`), per-source scrapers and cover
//! downloading (`cover`). Sources are tried in order
//! JavBus -> JavDB -> Avmoo -> JavLibrary -> Jav321 -> sehuatang; each source
//! returns the shared [`JavMatch`]. Extra sources exist so a single anti-bot
//! page or CDN outage does not drop a title. sehuatang sits last: it only
//! gets consulted when every mainstream source came up empty, and its forum
//! thread titles (the user's main source) can still supply a Chinese title /
//! cover for codes those sources don't list.

pub mod avmoo;
pub mod code;
pub mod cover;
pub mod curl_fetch;
pub mod jav321;
pub mod javbus;
pub mod javdb;
pub mod javlibrary;
pub mod sehuatang;
pub mod tangxin;

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// Shared metadata shape returned by every adult source. Mirrors JavBoss
/// `JavInfo` plus the fields the TTV detail page actually renders (plot,
/// director, label, rating). New fields are `#[serde(default)]` so older
/// cache rows still deserialize.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavMatch {
    pub code: String,
    pub title: String,
    pub series: Option<String>,
    pub studio: Option<String>,
    #[serde(default)]
    pub director: Option<String>,
    /// 发行商 / label, distinct from the studio (制作商) when the source
    /// provides both.
    #[serde(default)]
    pub label: Option<String>,
    /// Release date as `YYYY-MM-DD`.
    pub release_date: Option<String>,
    pub duration_min: Option<u32>,
    pub tags: Vec<String>,
    pub actors: Vec<String>,
    pub cover_url: Option<String>,
    pub uncensored: Option<bool>,
    /// Plot / 紹介 when the source has one; otherwise [`compose_summary`]
    /// builds a structured blurb from the other fields.
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub rating: Option<f64>,
    /// Which source produced this match (`javbus` / `javdb` / `avmoo` / …).
    pub provider: String,
}

impl JavMatch {
    pub fn new(
        code: impl Into<String>,
        title: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            title: title.into(),
            series: None,
            studio: None,
            director: None,
            label: None,
            release_date: None,
            duration_min: None,
            tags: Vec::new(),
            actors: Vec::new(),
            cover_url: None,
            uncensored: None,
            summary: None,
            rating: None,
            provider: provider.into(),
        }
    }
}

/// Which adult sources [`lookup_jav`] consults. `Fast` = JavBus only (primary
/// source, one request, highest hit rate) so a first scrape pass over a large
/// library costs seconds per item; `Full` = the whole six-source chain for the
/// leftovers, run as a separate later pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavScope {
    Fast,
    Full,
}

/// Look up a JAV code against all sources in priority order.
pub async fn lookup_jav(
    client: &reqwest::Client,
    codes: &[String],
) -> Result<Option<JavMatch>, AppError> {
    lookup_jav_scoped(client, codes, JavScope::Full).await
}

/// Look up a JAV code against all sources in priority order.
///
/// - `Ok(Some(match))` — a source produced metadata.
/// - `Ok(None)` — every source that answered *definitively* reported the code
///   as unknown (HTTP 404 / api-404) and none failed transiently; callers may
///   negative-cache this.
/// - `Err(_)` — at least one source failed transiently (network, rate limit,
///   anti-bot page) and none produced a match; callers must NOT negative-cache
///   and should retry on a later round.
pub async fn lookup_jav_scoped(
    client: &reqwest::Client,
    codes: &[String],
    scope: JavScope,
) -> Result<Option<JavMatch>, AppError> {
    if codes.is_empty() {
        return Ok(None);
    }
    let try_slow = scope == JavScope::Full;
    let mut confirmed_not_found = false;
    let mut any_transient = false;
    let mut best: Option<JavMatch> = None;
    for code in codes {
        absorb(
            javbus::lookup(client, code).await,
            "javbus",
            code,
            &mut best,
            &mut confirmed_not_found,
            &mut any_transient,
        );
        if !try_slow {
            // Fast scope: JavBus alone per code; leftovers go to a later
            // full-scope pass.
            continue;
        }
        // JavBus pages often omit score / plot / a complete actress list.
        // Always continue to JavDB so the detail page can show rating and actors.
        absorb(
            javdb::lookup(client, code).await,
            "javdb",
            code,
            &mut best,
            &mut confirmed_not_found,
            &mut any_transient,
        );
        if is_complete(best.as_ref()) {
            return Ok(best);
        }
        absorb(
            avmoo::lookup(client, code).await,
            "avmoo",
            code,
            &mut best,
            &mut confirmed_not_found,
            &mut any_transient,
        );
        if is_complete(best.as_ref()) {
            return Ok(best);
        }
        absorb(
            javlibrary::lookup(client, code).await,
            "javlibrary",
            code,
            &mut best,
            &mut confirmed_not_found,
            &mut any_transient,
        );
        if is_complete(best.as_ref()) {
            return Ok(best);
        }
        absorb(
            jav321::lookup(client, code).await,
            "jav321",
            code,
            &mut best,
            &mut confirmed_not_found,
            &mut any_transient,
        );
        // sehuatang last: its search answers "confirmed not found" only when
        // the guest search lists nothing, and its titles are the user's own
        // library's naming source. It builds its own client (per-lookup
        // cookie session for the Discuz gate/search flow).
        absorb(
            sehuatang::lookup(code).await,
            "sehuatang",
            code,
            &mut best,
            &mut confirmed_not_found,
            &mut any_transient,
        );
        if best.is_some() {
            return Ok(best);
        }
    }
    if best.is_some() {
        Ok(best)
    } else if any_transient {
        Err(AppError::Provider(
            "all adult sources failed transiently".into(),
        ))
    } else if confirmed_not_found {
        Ok(None)
    } else {
        Err(AppError::Provider(
            "all adult sources failed transiently".into(),
        ))
    }
}

fn absorb(
    result: Result<Option<JavMatch>, AppError>,
    source: &str,
    code: &str,
    best: &mut Option<JavMatch>,
    confirmed_not_found: &mut bool,
    any_transient: &mut bool,
) {
    match result {
        Ok(Some(mut matched)) => {
            if matched.code.is_empty() {
                matched.code = code.to_owned();
            }
            matched.actors = sanitize_names(matched.actors);
            matched.tags = sanitize_names(matched.tags);
            *best = Some(merge_jav(best.take(), matched));
        }
        Ok(None) => *confirmed_not_found = true,
        Err(error) => {
            *any_transient = true;
            tracing::warn!(
                code = %code,
                source,
                error = %error,
                "adult source lookup failed; trying next source"
            );
        }
    }
}

fn merge_jav(existing: Option<JavMatch>, incoming: JavMatch) -> JavMatch {
    let Some(mut base) = existing else {
        return incoming;
    };
    if prefer_chinese(&base.title, &incoming.title) {
        base.title = incoming.title;
    }
    if base.code.is_empty() {
        base.code = incoming.code.clone();
    }
    base.series = prefer_chinese_opt(base.series, incoming.series);
    base.studio = prefer_chinese_opt(base.studio, incoming.studio);
    base.director = prefer_chinese_opt(base.director, incoming.director);
    base.label = prefer_chinese_opt(base.label, incoming.label);
    if base.release_date.is_none() {
        base.release_date = incoming.release_date;
    }
    if base.duration_min.is_none() {
        base.duration_min = incoming.duration_min;
    }
    if base.cover_url.is_none() {
        base.cover_url = incoming.cover_url;
    }
    if base.uncensored.is_none() {
        base.uncensored = incoming.uncensored;
    }
    base.summary = prefer_chinese_opt(base.summary, incoming.summary);
    if base.rating.is_none() {
        base.rating = incoming.rating;
    }
    base.tags = prefer_chinese_list(base.tags, incoming.tags);
    base.actors = prefer_chinese_list(base.actors, incoming.actors);
    if !incoming.provider.is_empty() && !base.provider.contains(&incoming.provider) {
        base.provider = format!("{},{}", base.provider, incoming.provider);
    }
    base
}

fn chinese_score(text: &str) -> i32 {
    let mut cjk = 0;
    let mut kana = 0;
    for ch in text.chars() {
        match ch {
            '\u{3040}'..='\u{30FF}' => kana += 1,
            '\u{4E00}'..='\u{9FFF}' => cjk += 1,
            _ => {}
        }
    }
    cjk * 2 - kana * 4
}

fn prefer_chinese(current: &str, incoming: &str) -> bool {
    let current = current.trim();
    let incoming = incoming.trim();
    if incoming.is_empty() {
        return false;
    }
    if current.is_empty() {
        return true;
    }
    chinese_score(incoming) > chinese_score(current)
}

fn prefer_chinese_opt(current: Option<String>, incoming: Option<String>) -> Option<String> {
    match (current, incoming) {
        (None, incoming) => incoming,
        (current, None) => current,
        (Some(current), Some(incoming)) => {
            if prefer_chinese(&current, &incoming) {
                Some(incoming)
            } else {
                Some(current)
            }
        }
    }
}

fn prefer_chinese_list(current: Vec<String>, incoming: Vec<String>) -> Vec<String> {
    let current = sanitize_names(current);
    let incoming = sanitize_names(incoming);
    if incoming.is_empty() {
        return current;
    }
    if current.is_empty() {
        return incoming;
    }
    let current_score: i32 = current.iter().map(|value| chinese_score(value)).sum();
    let incoming_score: i32 = incoming.iter().map(|value| chinese_score(value)).sum();
    if incoming_score > current_score {
        merge_unique(incoming, current)
    } else {
        merge_unique(current, incoming)
    }
}

/// Drop page chrome, ads and scripts that HTML scrapers sometimes capture as
/// a single "actor" or "tag". Real names/tags are short; a whole detail page
/// dumped into one field is never valid metadata.
fn sanitize_names(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for value in values {
        let text = value.split_whitespace().collect::<Vec<_>>().join(" ");
        if !is_plausible_metadata_name(&text) {
            continue;
        }
        if seen.insert(text.to_lowercase()) {
            out.push(text);
        }
    }
    out
}

fn is_plausible_metadata_name(text: &str) -> bool {
    if text.is_empty() || text.chars().count() > 40 {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    const GARBAGE: &[&str] = &[
        "function(",
        "adsby",
        "javascript",
        "magnet",
        "画像を拡大",
        "磁力",
        "window.",
        "addclass",
        "jquery",
        "document.",
        "var ",
        "{",
        "}",
        ";",
        "=",
    ];
    !GARBAGE
        .iter()
        .any(|marker| lower.contains(marker) || text.contains(marker))
}

fn merge_unique(mut left: Vec<String>, right: Vec<String>) -> Vec<String> {
    for value in right {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !left
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            left.push(trimmed.to_owned());
        }
    }
    left
}

fn is_complete(matched: Option<&JavMatch>) -> bool {
    let Some(matched) = matched else {
        return false;
    };
    !matched.title.is_empty()
        && matched.cover_url.is_some()
        && !matched.actors.is_empty()
        && (matched.studio.is_some() || matched.series.is_some())
        && (matched.release_date.is_some() || matched.duration_min.is_some())
        && matched.rating.filter(|value| *value > 0.0).is_some()
}

/// Build a human-readable plot for the detail page. Prefers the source plot
/// when present; otherwise composes a structured blurb from the scraped
/// fields so the UI never falls back to a generic "imported from disk"
/// placeholder.
pub fn compose_summary(matched: &JavMatch) -> String {
    if let Some(plot) = matched
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return plot.to_owned();
    }
    let mut parts = Vec::new();
    if !matched.title.is_empty() {
        parts.push(matched.title.clone());
    }
    let mut facts = Vec::new();
    if !matched.code.is_empty() {
        facts.push(format!("番号 {}", matched.code));
    }
    if let Some(studio) = matched.studio.as_deref().filter(|value| !value.is_empty()) {
        facts.push(format!("制作 {studio}"));
    }
    if let Some(label) = matched.label.as_deref().filter(|value| !value.is_empty()) {
        facts.push(format!("发行 {label}"));
    }
    if let Some(series) = matched.series.as_deref().filter(|value| !value.is_empty()) {
        facts.push(format!("系列 {series}"));
    }
    if let Some(date) = matched
        .release_date
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        facts.push(format!("发行日 {date}"));
    }
    if let Some(minutes) = matched.duration_min.filter(|value| *value > 0) {
        facts.push(format!("时长 {minutes} 分钟"));
    }
    if !matched.actors.is_empty() {
        facts.push(format!("出演 {}", matched.actors.join("、")));
    }
    if !matched.tags.is_empty() {
        facts.push(format!(
            "标签 {}",
            matched
                .tags
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    if !facts.is_empty() {
        parts.push(facts.join(" · "));
    }
    parts.join("。")
}

/// Bulk-scrape fast mode: when one scrape run targets a large library (the
/// 5,000-item backlog batches), per-source timeouts shrink, Avmoo drops its
/// retries, and the sehuatang last-resort (31s Discuz guest-search throttle)
/// is skipped entirely — otherwise every unmatched adult-named item burns
/// 1-2 minutes across six sources and a 5,000-item batch would take days.
/// Set by `scrape_media` for the duration of one run.
static FAST_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_fast_mode(on: bool) {
    FAST_MODE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn fast_mode() -> bool {
    FAST_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Shared HTTP client for adult sources: plain TLS, proxy support
/// from the environment (HTTPS_PROXY etc. via reqwest defaults) and a
/// browser-like default UA; per-source headers are set per request.
pub fn build_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .use_rustls_tls() // pinned: native-tls exists only for sehuatang's CF bypass
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(CHROME_UA)
        .build()
        .map_err(|error| AppError::Runtime(format!("adult scraper client: {error}")))
}

/// Bulk-run variant: fail fast (4s connect / 10s total) so a large batch of
/// mostly-unmatchable items does not pay full 20s timeouts per source.
pub fn build_fast_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(4))
        .user_agent(CHROME_UA)
        .build()
        .map_err(|error| AppError::Runtime(format!("adult fast client: {error}")))
}

pub(crate) const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// Global per-source rate limiter: one request per `interval` across the
/// whole process, mirroring JavBoss' mutex token limiter.
pub(crate) struct RateLimiter {
    state: std::sync::Mutex<Option<std::time::Instant>>,
    interval: Duration,
}

impl RateLimiter {
    pub const fn new(interval: Duration) -> Self {
        Self {
            state: std::sync::Mutex::new(None),
            interval,
        }
    }

    pub async fn wait(&self) {
        let wait = {
            let mut next = self.state.lock().expect("rate limiter poisoned");
            let now = std::time::Instant::now();
            match *next {
                Some(slot) if slot > now => {
                    let wait = slot - now;
                    *next = Some(slot + self.interval);
                    wait
                }
                _ => {
                    *next = Some(now + self.interval);
                    Duration::ZERO
                }
            }
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

/// Resolve a possibly-relative URL against a page URL.
pub(crate) fn resolve_url(base: &str, candidate: &str) -> String {
    let candidate = candidate.trim();
    if candidate.is_empty() || candidate.starts_with("javascript:") || candidate.starts_with('#') {
        return String::new();
    }
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        return candidate.to_owned();
    }
    match reqwest::Url::parse(base) {
        Ok(parsed) => parsed
            .join(candidate)
            .map(|url| url.to_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{compose_summary, merge_jav, prefer_chinese, sanitize_names, JavMatch};

    #[test]
    fn compose_summary_prefers_source_plot() {
        let mut matched = JavMatch::new("ABP-356", "作品标题", "javbus");
        matched.summary = Some("  源站剧情简介  ".into());
        assert_eq!(compose_summary(&matched), "源站剧情简介");
    }

    #[test]
    fn compose_summary_builds_structured_blurb() {
        let mut matched = JavMatch::new("ABP-356", "作品标题", "javbus");
        matched.studio = Some("Prestige".into());
        matched.actors = vec!["女優A".into()];
        matched.tags = vec!["巨乳".into(), "痴女".into()];
        matched.release_date = Some("2016-01-01".into());
        let summary = compose_summary(&matched);
        assert!(summary.contains("作品标题"));
        assert!(summary.contains("ABP-356"));
        assert!(summary.contains("Prestige"));
        assert!(summary.contains("女優A"));
        assert!(summary.contains("巨乳"));
    }

    #[test]
    fn merge_fills_missing_fields_from_later_source() {
        let mut first = JavMatch::new("ABP-356", "标题", "javbus");
        first.cover_url = Some("https://example.com/a.jpg".into());
        let mut second = JavMatch::new("ABP-356", "标题", "javdb");
        second.actors = vec!["女優A".into()];
        second.rating = Some(4.2);
        let merged = merge_jav(Some(first), second);
        assert_eq!(merged.actors, vec!["女優A".to_string()]);
        assert_eq!(merged.rating, Some(4.2));
        assert_eq!(
            merged.cover_url.as_deref(),
            Some("https://example.com/a.jpg")
        );
        assert!(merged.provider.contains("javbus"));
        assert!(merged.provider.contains("javdb"));
    }

    #[test]
    fn sanitize_names_drops_page_garbage() {
        let names = vec![
            "波多野結衣".into(),
            "画像を拡大する".into(),
            "演員 磁力連結投稿 × magnet地址 function($ { adsbyjuicy".into(),
            "AIKA".into(),
            "window.adsbyjuicy.push".into(),
        ];
        assert_eq!(
            sanitize_names(names),
            vec!["波多野結衣".to_string(), "AIKA".to_string()]
        );
    }

    #[test]
    fn merge_prefers_chinese_title_over_japanese() {
        let mut first = JavMatch::new(
            "SIRO-5658",
            "サキュバスのように貪欲に快楽を求めるスレンダー美女現る！",
            "javbus",
        );
        first.tags = vec!["美少女".into()];
        let mut second =
            JavMatch::new("SIRO-5658", "像魅魔一样贪婪追求快感的纤细美女现身", "avmoo");
        second.tags = vec!["美少女".into(), "苗条".into()];
        let merged = merge_jav(Some(first), second);
        assert_eq!(merged.title, "像魅魔一样贪婪追求快感的纤细美女现身");
        assert!(prefer_chinese(
            "サキュバスのように貪欲に快楽を求める",
            "像魅魔一样贪婪追求快感的纤细美女现身"
        ));
        assert!(
            merged.tags.contains(&"苗条".to_string())
                || merged.tags.contains(&"美少女".to_string())
        );
    }
}
