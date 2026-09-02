//! sehuatang.net (98堂[原色花堂]) scraper — the user's main 18+ source.
//!
//! Every unauthenticated request is fronted by a JS gate page embedding
//! `var safeid='<16 chars>'`. The gate JS only sets `_safe=<safeid>` on the
//! enter-button click and reloads, so replaying that cookie on a second
//! request passes the gate without executing any JS (curl-verified 2026-08).
//!
//! The guest search (`search.php?mod=forum&srchtxt=<code>&searchsubmit=yes`)
//! follows the Discuz submit pattern: after the gate clears, the search
//! submit answers 302 with Discuz session cookies (saltkey / lastvisit /
//! lastact) and redirects back to the same URL; the redirect hop must carry
//! those cookies or Discuz counts a second search and answers the
//! "搜索过于频繁" throttle page. So each lookup runs with its own curl
//! cookie-jar file: `_safe` is appended after the gate page and curl replays
//! it together with the session cookies through the 302 (live-verified
//! 2026-08-29).
//!
//! Why a curl.exe subprocess instead of reqwest: the site fronts Cloudflare,
//! which answers a "Just a moment" 403 to the rustls fingerprint AND to the
//! hyper stack even with native-tls (Schannel) + HTTP/1.1, while the OS
//! curl.exe (Schannel, HTTP/1.1, curl's header layout) gets through —
//! verified A/B 2026-08-29. Requests are paced by rate limiters and the
//! subprocess cost is irrelevant at last-resort lookup frequency. Thread
//! titles carry the site's own Chinese/localized forms.

use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use scraper::{ElementRef, Html, Selector};

use super::curl_fetch::CurlFetch;
use super::{resolve_url, JavMatch, RateLimiter, CHROME_UA};
use crate::error::AppError;

const BASE_URL: &str = "https://sehuatang.net";

/// Gate-clear / thread-page requests are cheap; keep the shared pacing.
static RATE: RateLimiter = RateLimiter::new(Duration::from_millis(1200));

/// Discuz throttles guest searches ("搜索过于频繁，请30秒后再试"), so
/// search.php submissions get their own generous interval.
static SEARCH_RATE: RateLimiter = RateLimiter::new(Duration::from_secs(31));

/// Look up a code on sehuatang. Returns `Ok(None)` when the guest search has
/// no thread matching the code, `Err` on transient failures (gate refused the
/// replay, throttled, network) so the caller knows not to negative-cache.
pub async fn lookup(code: &str) -> Result<Option<JavMatch>, AppError> {
    let code = code.trim();
    if code.is_empty() {
        return Ok(None);
    }
    if super::fast_mode() {
        // Bulk scrape: the Discuz guest-search throttle (≈31s/query) makes
        // this last-resort source unusable for large batches — skipping it
        // lets unmatched items fail in seconds instead of minutes. Small
        // scrapes and the 刮削缺失项 flow still use it.
        return Ok(None);
    }

    let session = CurlFetch::new()?;

    let search_url = format!(
        "{}/search.php?mod=forum&srchtxt={}&searchsubmit=yes",
        BASE_URL,
        urlencode(code)
    );

    SEARCH_RATE.wait().await;
    let body = fetch_through_gate(&session, &search_url).await?;
    // scraper::Html is not Send, so it must not be held across the cover
    // fetch below.
    let hit = {
        let document = Html::parse_document(&body);
        find_thread(&document, code)
    };
    let Some(hit) = hit else {
        return Ok(None);
    };

    let mut matched = JavMatch::new(code.to_owned(), hit.title.clone(), "sehuatang");
    matched.uncensored = infer_uncensored(&hit.title, &hit.forum);
    match fetch_thread_cover(&session, &hit.href).await {
        Ok(Some(cover)) => matched.cover_url = Some(cover),
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(code = %code, error = %error, "sehuatang thread cover fetch failed");
        }
    }
    Ok(Some(matched))
}

/// GET an HTML page from sehuatang, paced by the shared rate limiter.
async fn get_html(session: &CurlFetch, url: &str, follow: bool) -> Result<(u16, String), AppError> {
    RATE.wait().await;
    let headers = vec![
        ("User-Agent".to_owned(), CHROME_UA.to_owned()),
        (
            "Accept-Language".to_owned(),
            "zh-CN,zh;q=0.9,en;q=0.8".to_owned(),
        ),
    ];
    let (status, body) = session.get(url, follow, &headers).await?;
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

/// GET a sehuatang URL, transparently minting the `_safe` gate cookie when
/// the first response is the JS gate page. Cloudflare challenge pages are
/// surfaced as transient errors even when they arrive with HTTP 200.
async fn fetch_through_gate(session: &CurlFetch, url: &str) -> Result<String, AppError> {
    let (status, body) = get_html(session, url, false).await?;
    if status != 200 {
        return Err(non_200_error(url, status, &body));
    }
    if looks_like_challenge(&body) {
        return Err(challenge_error(url));
    }
    if let Some(safeid) = extract_safeid(&body) {
        tracing::debug!(url = %url, "sehuatang gate page hit; injecting _safe cookie");
        // The jar then carries `_safe` plus the Discuz session cookies from
        // the search 302 through the redirect hop.
        session.inject_cookie("sehuatang.net", "_safe", &safeid);
        let (status, body) = get_html(session, url, true).await?;
        if status != 200 {
            return Err(non_200_error(url, status, &body));
        }
        if looks_like_challenge(&body) {
            return Err(challenge_error(url));
        }
        if extract_safeid(&body).is_some() {
            return Err(AppError::Provider(format!(
                "sehuatang gate did not clear after _safe retry: {url}"
            )));
        }
        if is_throttle_message(&body) {
            return Err(AppError::Provider(format!(
                "sehuatang throttled or login required: {url}"
            )));
        }
        return Ok(body);
    }
    if is_throttle_message(&body) {
        return Err(AppError::Provider(format!(
            "sehuatang throttled or login required: {url}"
        )));
    }
    Ok(body)
}

/// Keep a short body excerpt in non-200 errors: sehuatang fronts Cloudflare,
/// and the difference between a fingerprint 403 and a real block lives in
/// the page, not the status line.
fn non_200_error(url: &str, status: u16, body: &str) -> AppError {
    let snippet = body.chars().take(200).collect::<String>();
    AppError::Provider(format!(
        "sehuatang non-200 for {url}: {status} body: {snippet}"
    ))
}

/// Cloudflare also serves its managed-challenge page with HTTP 200. It lists
/// no threads, so without this check a fingerprint block would be misread as
/// "code not found" and negative-cached. Markers must not appear on real
/// pages: the site embeds `/cdn-cgi/challenge-platform/` snippets on every
/// page, so the challenge script path alone is not a marker.
fn looks_like_challenge(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("just a moment")
        || lower.contains("attention required")
        || lower.contains("_cf_chl_opt")
        || lower.contains("cf-browser-verification")
}

fn challenge_error(url: &str) -> AppError {
    AppError::Provider(format!("sehuatang cloudflare challenge page at {url}"))
}

/// Pull the 16-char safeid out of the gate page's inline script
/// (`var safeid='…';`). Absent on every real page.
fn extract_safeid(body: &str) -> Option<String> {
    const MARKER: &str = "var safeid='";
    let start = body.find(MARKER)? + MARKER.len();
    let rest = &body[start..];
    let end = rest.find('\'')?;
    let id = &rest[..end];
    (!id.is_empty()).then(|| id.to_owned())
}

/// Discuz guest search throttles and some boards require login; both render a
/// system-message page instead of the result list. Treat as transient so the
/// caller retries later instead of negative-caching a false "not found".
fn is_throttle_message(body: &str) -> bool {
    body.contains("alert_error") || body.contains("alert_info")
}

struct ThreadHit {
    title: String,
    href: String,
    forum: String,
}

fn find_thread(document: &Html, code: &str) -> Option<ThreadHit> {
    let selector = Selector::parse("a[href*='mod=viewthread'], a[href*='thread-']").ok()?;
    for anchor in document.select(&selector) {
        let href = anchor.value().attr("href").unwrap_or_default();
        if href.is_empty() {
            continue;
        }
        let raw: String = anchor.text().collect();
        let title = clean_text(&raw);
        if title.is_empty() || !title_contains_code(&title, code) {
            continue;
        }
        return Some(ThreadHit {
            title: clean_title(&title, code),
            href: resolve_url(&format!("{BASE_URL}/"), href),
            forum: ancestor_forum(anchor),
        });
    }
    None
}

/// The board name (亚洲无码原创 / 亚洲有码原创 / …) sits next to the thread
/// link inside the same result row; the guest search renders it as a
/// rewrite link (`forum-36-1.html`), other pages as `forum.php?…forumdisplay`.
fn ancestor_forum(anchor: ElementRef<'_>) -> String {
    let Ok(forum_selector) = Selector::parse("a[href*='forumdisplay'], a[href^='forum-']") else {
        return String::new();
    };
    for node in anchor.ancestors() {
        let Some(element) = ElementRef::wrap(node) else {
            continue;
        };
        if element.value().name() == "li" {
            if let Some(forum) = element.select(&forum_selector).next() {
                return clean_text(&forum.text().collect::<String>());
            }
            break;
        }
    }
    String::new()
}

/// Thread pages are visited only for the first-post cover attachment; the rest
/// of the metadata on them is free text. Failures are non-fatal.
async fn fetch_thread_cover(
    session: &CurlFetch,
    thread_url: &str,
) -> Result<Option<String>, AppError> {
    let body = fetch_through_gate(session, thread_url).await?;
    let cover = {
        let document = Html::parse_document(&body);
        parse_cover(&document, thread_url)
    };
    Ok(cover)
}

/// First-pass cover extraction: Discuz attachment <img>s carry a
/// zoomfile/file attribute; everything else in #postlist (avatars, emoticons,
/// UI icons) only has src, and the first of those is the poster's avatar.
/// Fallback: bare src images inside the post body (some posts inline the
/// cover as a plain <img>).
fn parse_cover(document: &Html, thread_url: &str) -> Option<String> {
    for css in ["td.t_f img", "#postlist img", ".pct img"] {
        let Ok(selector) = Selector::parse(css) else {
            continue;
        };
        for image in document.select(&selector) {
            for attribute in ["zoomfile", "file"] {
                let Some(value) = image.value().attr(attribute) else {
                    continue;
                };
                let resolved = resolve_url(thread_url, value);
                if is_cover_candidate(&resolved) {
                    return Some(resolved);
                }
            }
        }
    }
    for css in ["td.t_f img"] {
        let Ok(selector) = Selector::parse(css) else {
            continue;
        };
        for image in document.select(&selector) {
            let Some(value) = image.value().attr("src") else {
                continue;
            };
            let resolved = resolve_url(thread_url, value);
            if is_cover_candidate(&resolved) {
                return Some(resolved);
            }
        }
    }
    None
}

fn is_cover_candidate(url: &str) -> bool {
    url.starts_with("http")
        && !url.contains("smiley")
        && !url.contains("avatar")
        && !url.contains("uc_server")
        && !url.contains("/image/common/")
        && !url.contains("/common/")
}

/// Censored vs uncensored from the board name and title markers; `None` when
/// nothing on the row says either way.
fn infer_uncensored(title: &str, forum: &str) -> Option<bool> {
    if title.contains("无码破解") || title.contains("無碼破解") {
        return Some(true);
    }
    if forum.contains("无码") || forum.contains("無碼") {
        return Some(true);
    }
    if forum.contains("有码") || forum.contains("有碼") {
        return Some(false);
    }
    None
}

fn title_contains_code(title: &str, code: &str) -> bool {
    code_token_regex(code).is_match(title)
}

/// Turn a raw thread title into a display title: drop size/quality markers
/// (`[HD/4.34G]`, `[HD]`, `[2.3GB]` …) and the code token itself, then trim
/// the leftover separator noise. Content markers like `[无码破解]` /
/// `[中文字幕]` are real information and stay. Falls back to the raw title
/// when everything gets stripped.
fn clean_title(raw: &str, code: &str) -> String {
    let without_markers = size_marker_regex().replace_all(raw, " ");
    let without_code = code_token_regex(code).replace_all(&without_markers, "$1 $3");
    let cleaned = clean_text(&without_code);
    if cleaned.is_empty() {
        return clean_text(raw);
    }
    cleaned
        .trim_matches(|ch: char| matches!(ch, '-' | '_' | '·' | '|' | '/' | ','))
        .trim()
        .to_owned()
}

fn size_marker_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)[\[【]\s*(?:hd|uhd|4k|2160p?|1080p?|720p?|60fps)?\s*(?:[/／x×*]?\s*[\d.]+\s*[mg]b?)?\s*[\]】]"#)
            .expect("valid size marker regex")
    })
}

/// Match the code with `-`/`_`/spacing flexibility: `SIRO-5658` also matches
/// `SIRO 5658` and `SIRO5658`; `FC2-PPV-1234567` matches `FC2PPV-1234567`.
///
/// Boundaries are ASCII-alphanumeric based, NOT `\b`: the regex crate's `\b`
/// is Unicode-aware and kana/kanji count as word characters, so
/// `\bSIRO-5658\b` would fail on "SIRO-5658のタイトル" — thread titles glue
/// the code straight onto CJK text all the time. The regex crate has no
/// lookaround, so the boundary groups consume one neighbouring character;
/// `clean_title` puts both back via `$1 $3` and drops only the code itself.
fn code_token_regex(code: &str) -> &'static Regex {
    // Codes repeat across lookups, but the cache is bounded by app lifetime
    // and the set of codes per session is small.
    static CACHE: std::sync::Mutex<Option<(String, &'static Regex)>> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().expect("code token regex cache");
    if let Some((cached_code, regex)) = cache.as_ref() {
        if cached_code == code {
            return regex;
        }
    }
    let parts: Vec<String> = code
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| regex::escape(part))
        .collect();
    let pattern = if parts.is_empty() {
        regex::escape(code)
    } else {
        format!(
            r#"(?i)(^|[^0-9A-Za-z])({})([^0-9A-Za-z]|$)"#,
            parts.join(r"[\s\-_]*")
        )
    };
    let compiled: &'static Regex = Box::leak(Box::new(
        Regex::new(&pattern).expect("valid code token regex"),
    ));
    *cache = Some((code.to_owned(), compiled));
    compiled
}

fn clean_text(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_safeid_reads_gate_script() {
        let gate = r#"<div style="display: none;">最好的报复就是巨大的成功。</div>
<script>var safeid='i514Y11kQN85YOKE';</script>
<script src="static/safe/js/mainv2.js"></script>"#;
        assert_eq!(extract_safeid(gate).as_deref(), Some("i514Y11kQN85YOKE"));
    }

    #[test]
    fn extract_safeid_absent_on_real_pages() {
        let page = r#"<title>搜索 - 98堂[原色花堂] - Powered by Discuz!</title>
<script>var STYLEID = '1';</script>"#;
        assert_eq!(extract_safeid(page), None);
    }

    #[test]
    fn clean_title_strips_size_markers_and_code() {
        assert_eq!(
            clean_title(
                "[HD/4.34G]SIRO-5658 初撮り五十路妻ドキュメント 井上祥子",
                "SIRO-5658"
            ),
            "初撮り五十路妻ドキュメント 井上祥子"
        );
        assert_eq!(
            clean_title("SIRO-5658 タイトル [HD/2.3GB]", "SIRO-5658"),
            "タイトル"
        );
        assert_eq!(clean_title("【4K】ABP-984 作品名", "ABP-984"), "作品名");
    }

    #[test]
    fn clean_title_keeps_content_markers() {
        let cleaned = clean_title("[無碼破解]FC2-PPV-3312555 中文字幕", "FC2-PPV-3312555");
        assert!(cleaned.contains("無碼破解"));
        assert!(cleaned.contains("中文字幕"));
        assert!(!cleaned.contains("FC2"));
    }

    #[test]
    fn clean_title_falls_back_to_raw_when_only_code() {
        assert_eq!(clean_title("SIRO-5658", "SIRO-5658"), "SIRO-5658");
        assert_eq!(
            clean_title("[HD] SIRO-5658 [4.3G]", "SIRO-5658"),
            "[HD] SIRO-5658 [4.3G]"
        );
    }

    #[test]
    fn title_contains_code_allows_separator_flexibility() {
        assert!(title_contains_code("SIRO 5658 初撮り", "SIRO-5658"));
        assert!(title_contains_code("SIRO5658 初撮り", "SIRO-5658"));
        assert!(title_contains_code(
            "FC2PPV-3312555 中文字幕",
            "FC2-PPV-3312555"
        ));
        assert!(!title_contains_code("完全无关的标题", "SIRO-5658"));
    }

    #[test]
    fn title_contains_code_matches_cjk_glued_code() {
        // Unicode \b would treat kana/kanji as word characters and miss these.
        assert!(title_contains_code("SIRO-5658のタイトル", "SIRO-5658"));
        assert!(title_contains_code("初撮りSIRO-5658五十路", "SIRO-5658"));
        // Longer alphanumeric runs still do not match.
        assert!(!title_contains_code("CSIRO-5658 関連", "SIRO-5658"));
        assert!(!title_contains_code("SIRO-56580 の続き", "SIRO-5658"));
    }

    #[test]
    fn clean_title_strips_cjk_glued_code() {
        assert_eq!(
            clean_title("[无码破解]SIRO-5658初撮り五十路妻ドキュメント", "SIRO-5658"),
            "[无码破解] 初撮り五十路妻ドキュメント"
        );
        assert_eq!(
            clean_title("SIRO-5658のタイトル", "SIRO-5658"),
            "のタイトル"
        );
    }

    #[test]
    fn looks_like_challenge_flags_cf_pages_not_real_ones() {
        let challenge = r#"<title>Just a moment...</title>
        <script>window._cf_chl_opt = {cType: 'managed'};</script>"#;
        assert!(looks_like_challenge(challenge));
        // The gate page embeds a challenge-platform snippet too — not a marker.
        let gate = r#"<script>var safeid='O7EiXNxL5eL5eIzP';</script>
        <script src="static/safe/js/mainv2.js"></script>
        <script>a.src='/cdn-cgi/challenge-platform/scripts/jsd/main.js';</script>"#;
        assert!(!looks_like_challenge(gate));
        // Real pages carry the same snippet plus real content.
        let real = r#"<title>搜索 - 98堂[原色花堂]</title>
        <script>window.__CF$cv$params={r:'a32aa04a9bc6fda8'};a.src='/cdn-cgi/challenge-platform/scripts/jsd/main.js';</script>"#;
        assert!(!looks_like_challenge(real));
    }

    #[test]
    fn parse_cover_prefers_attachment_image_and_skips_avatar() {
        // Markup mirrors a live thread page: the avatar sits in #postlist, the
        // cover is the first attachment img inside td.t_f.
        let page = r#"
        <div id="postlist">
        <table><tr><td class="plc">
          <a href="home.php?mod=space&amp;uid=560813"><img src="uc_server/avatar.php?uid=560813&amp;size=middle" /></a>
          <table><tr><td class="t_f" id="postmessage_1">
            <img id="aimg_9xYyZ" zoomfile="https://tu.djhdhs.us/data/attachment/forum/202604/27/091512abc123.jpg"
                 file="data/attachment/forum/202604/27/091512abc123.jpg"
                 src="data/attachment/forum/202604/27/091512abc123.jpg.thumb.jpg" />
            <img src="static/image/smiley/tongue.gif" />
          </td></tr></table>
        </td></tr></table>
        </div>"#;
        let document = Html::parse_document(page);
        let cover = parse_cover(
            &document,
            "https://sehuatang.net/forum.php?mod=viewthread&tid=3463226",
        );
        assert_eq!(
            cover.as_deref(),
            Some("https://tu.djhdhs.us/data/attachment/forum/202604/27/091512abc123.jpg")
        );
    }

    #[test]
    fn parse_cover_falls_back_to_inline_post_image() {
        // Some posts inline the cover as a plain <img> (relative src).
        let page = r#"
        <table><tr><td class="t_f" id="postmessage_2">
          <img src="data/attachment/forum/202605/01/inline.jpg" />
        </td></tr></table>"#;
        let document = Html::parse_document(page);
        let cover = parse_cover(
            &document,
            "https://sehuatang.net/forum.php?mod=viewthread&tid=1",
        );
        assert_eq!(
            cover.as_deref(),
            Some("https://sehuatang.net/data/attachment/forum/202605/01/inline.jpg")
        );
    }

    #[test]
    fn parse_cover_none_when_only_avatar() {
        let page = r#"<div id="postlist"><img src="uc_server/avatar.php?uid=1" /></div>"#;
        let document = Html::parse_document(page);
        let cover = parse_cover(
            &document,
            "https://sehuatang.net/forum.php?mod=viewthread&tid=1",
        );
        assert_eq!(cover, None);
    }

    #[test]
    fn find_thread_picks_matching_thread_row() {
        // Markup copied verbatim from a live guest search result page
        // (2026-08-29, fix2.html): li id is the tid, the code is wrapped in
        // <strong><font>, and the board link uses the rewrite form.
        let page = r##"
        <div class="slst mtw" id="threadlist">
        <ul><li class="pbw" id="3463226">
        <h3 class="xs3">
        <a href="forum.php?mod=viewthread&amp;tid=3463226&amp;highlight=" target="_blank" ><strong><font color="#ff0000">SIRO-5658</font></strong> [无码破解] サキュバスのように貪欲に快楽を求めるスレンダー美女現る！みさと 27歳</a>
        </h3>
        <p class="xg1">149 个回复 - 53742 次查看</p>
        <p><span>2026-04-27 08:06</span> - <span><a href="space-uid-560813.html" target="_blank">82h8oeddq</a></span> - <span><a href="forum-36-1.html" target="_blank" class="xi1">亚洲无码原创</a></span></p>
        </li>
        <li class="pbw" id="3461451">
        <h3 class="xs3">
        <a href="forum.php?mod=viewthread&amp;tid=3461451&amp;highlight=" target="_blank" ><strong><font color="#ff0000">SIRO-5658</font></strong> サキュバスのように貪欲に快楽を求めるスレンダー美女現る！ただひらすらに快感と！</a>
        </h3>
        <p><a href="forum-37-1.html" target="_blank" class="xi1">亚洲有码原创</a></p>
        </li></ul>
        </div>"##;
        let document = Html::parse_document(page);
        let hit = find_thread(&document, "SIRO-5658").expect("thread hit");
        assert_eq!(
            hit.title,
            "[无码破解] サキュバスのように貪欲に快楽を求めるスレンダー美女現る！みさと 27歳"
        );
        assert_eq!(
            hit.href,
            "https://sehuatang.net/forum.php?mod=viewthread&tid=3463226&highlight="
        );
        assert_eq!(hit.forum, "亚洲无码原创");
    }

    #[test]
    fn find_thread_none_when_no_match() {
        let page = r#"<a href="thread-111-1-1.html">完全无关的标题</a>"#;
        let document = Html::parse_document(page);
        assert!(find_thread(&document, "SIRO-5658").is_none());
    }

    #[test]
    fn infer_uncensored_from_board_and_title() {
        assert_eq!(infer_uncensored("标题", "亚洲无码原创"), Some(true));
        assert_eq!(infer_uncensored("标题", "亚洲有码原创"), Some(false));
        assert_eq!(infer_uncensored("[無碼破解] 标题", "国产原创"), Some(true));
        assert_eq!(infer_uncensored("普通标题", ""), None);
    }

    #[test]
    fn urlencode_percent_encodes_utf8() {
        assert_eq!(urlencode("SIRO-5658"), "SIRO-5658");
        assert_eq!(urlencode("中 文"), "%E4%B8%AD%20%E6%96%87");
    }
}
