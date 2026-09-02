//! Cover downloading for adult metadata.
//!
//! Ported from JavBoss `internal/manager/cover_manager.go` (single-URL path).
//! Downloads the cover referenced by a [`super::JavMatch`], validates it
//! (>= 30 KB — anything smaller is a placeholder/error image) and writes it
//! atomically (`.tmp` -> rename) to `{cover_dir}/{code-lowercase}{ext}`.
//! JavBus image hosts get browser headers + the `age=verified` cookie to pass
//! their hotlink protection.

use reqwest::StatusCode;
use std::path::{Path, PathBuf};

use super::CHROME_UA;
use crate::error::AppError;

const MIN_VALID_COVER_BYTES: usize = 30 * 1024;
const KNOWN_EXTS: &[&str] = &[".jpg", ".jpeg", ".png", ".webp"];

/// The site a provider's image host requires as Referer for hotlink
/// protection, when known. sehuatang rotates its image CDN domains, so the
/// mapping is keyed on the source rather than the image host.
pub fn referer_for_provider(provider: &str) -> Option<&'static str> {
    if provider
        .split(',')
        .any(|source| source.trim() == "sehuatang")
    {
        Some("https://sehuatang.net/")
    } else {
        None
    }
}

/// Return the on-disk cover for a code if one already exists (any known
/// extension). File names use the lower-cased code, mirroring JavBoss.
pub fn find_existing_cover(cover_dir: &Path, code: &str) -> Option<PathBuf> {
    let code = normalize_code(code);
    if code.is_empty() {
        return None;
    }
    KNOWN_EXTS
        .iter()
        .map(|ext| cover_dir.join(format!("{code}{ext}")))
        .find(|path| path.is_file())
}

/// Download `cover_url` into `cover_dir/{code}{ext}`. Skips the download when
/// a cover already exists unless `force` is set. `referer` carries the
/// referring site for image hosts with hotlink protection (sehuatang's image
/// CDN answers 403 without its site referer — live-verified 2026-08-29).
/// Returns the final path.
pub async fn download_cover(
    client: &reqwest::Client,
    cover_dir: &Path,
    code: &str,
    cover_url: &str,
    force: bool,
    referer: Option<&str>,
) -> Result<PathBuf, AppError> {
    let code = normalize_code(code);
    if code.is_empty() {
        return Err(AppError::InvalidInput("empty jav code".into()));
    }
    let cover_url = cover_url.trim();
    if cover_url.is_empty() {
        return Err(AppError::InvalidInput("empty cover url".into()));
    }
    if !force {
        if let Some(existing) = find_existing_cover(cover_dir, &code) {
            return Ok(existing);
        }
    }

    // Header set shared by the reqwest attempt and the curl.exe fallback
    // below, so the hotlink/WAF expectations stay identical.
    let mut header_pairs: Vec<(String, String)> = Vec::new();
    if let Some(referer) = referer {
        // Known hotlink-protected hosts: a browser-like UA is required too —
        // the TTVCoverBot UA trips their WAF even with a valid referer.
        header_pairs.push(("User-Agent".into(), CHROME_UA.into()));
        header_pairs.push(("Referer".into(), referer.into()));
    }
    if let Ok(url) = reqwest::Url::parse(cover_url) {
        let host = url.host_str().unwrap_or_default().to_lowercase();
        if host == "javbus.com" || host.ends_with(".javbus.com") {
            header_pairs.push(("User-Agent".into(), CHROME_UA.into()));
            header_pairs.push((
                "Accept".into(),
                "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".into(),
            ));
            header_pairs.push(("Accept-Language".into(), "zh-CN,zh;q=0.9,en;q=0.8".into()));
            header_pairs.push(("Referer".into(), "https://www.javbus.com/".into()));
            header_pairs.push(("Cookie".into(), "age=verified; existmag=mag".into()));
        } else if host.contains("javdb") {
            header_pairs.push(("User-Agent".into(), CHROME_UA.into()));
            header_pairs.push((
                "Accept".into(),
                "image/avif,image/webp,image/apng,image/*,*/*;q=0.8".into(),
            ));
            header_pairs.push(("Referer".into(), "https://javdb.com/".into()));
        } else if host.contains("javlibrary") {
            header_pairs.push(("User-Agent".into(), CHROME_UA.into()));
            header_pairs.push((
                "Accept".into(),
                "image/avif,image/webp,image/apng,image/*,*/*;q=0.8".into(),
            ));
            header_pairs.push(("Referer".into(), "https://www.javlibrary.com/".into()));
            header_pairs.push(("Cookie".into(), "over18=18".into()));
        } else if host.contains("jav321") {
            header_pairs.push(("User-Agent".into(), CHROME_UA.into()));
            header_pairs.push((
                "Accept".into(),
                "image/avif,image/webp,image/apng,image/*,*/*;q=0.8".into(),
            ));
            header_pairs.push(("Referer".into(), "https://www.jav321.com/".into()));
        }
    }

    // Dedupe by header name, last write wins: the provider referer and the
    // per-host branch can both emit e.g. "Referer", and conflicting duplicate
    // headers make javbus's WAF answer 403 (live-verified 2026-08-29).
    let mut deduped: Vec<(String, String)> = Vec::new();
    for (name, value) in header_pairs {
        match deduped
            .iter_mut()
            .find(|(existing, _): &&mut (String, String)| existing.eq_ignore_ascii_case(&name))
        {
            Some(entry) => entry.1 = value,
            None => deduped.push((name, value)),
        }
    }
    let header_pairs = deduped;

    let mut request = client
        .get(cover_url)
        .header("User-Agent", "Mozilla/5.0 (compatible; TTVCoverBot/1.0)");
    for (name, value) in &header_pairs {
        request = request.header(name, value);
    }

    // Primary attempt: the shared reqwest client. CF-fronted image hosts
    // (javbus.com) answer this with a "Just a moment" 403 because of the
    // rustls fingerprint, so any failure falls back to the OS curl.exe —
    // the same stack browsers/curl use, which those hosts accept
    // (live-verified 2026-08-29).
    enum Primary {
        Ok {
            content_type: String,
            final_path: String,
            bytes: Vec<u8>,
        },
        NotFound,
        Failed(String),
    }
    let primary = match request.send().await {
        Ok(response) => {
            let status = response.status();
            if status == StatusCode::NOT_FOUND {
                Primary::NotFound
            } else if !status.is_success() {
                Primary::Failed(format!("cover status {status} for {cover_url}"))
            } else {
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                let final_path = response.url().path().to_owned();
                match response.bytes().await {
                    Ok(bytes) => Primary::Ok {
                        content_type,
                        final_path,
                        bytes: bytes.to_vec(),
                    },
                    Err(error) => Primary::Failed(format!("cover read body: {error}")),
                }
            }
        }
        Err(error) => Primary::Failed(format!("cover request {cover_url}: {error}")),
    };

    let (content_type, final_url, bytes) = match primary {
        Primary::Ok {
            content_type,
            final_path,
            bytes,
        } => (content_type, final_path, bytes),
        Primary::NotFound => return Err(AppError::NotFound(format!("cover 404: {cover_url}"))),
        Primary::Failed(reason) => {
            let fetch = super::curl_fetch::CurlFetch::new()?;
            let (status, body) = fetch.get(cover_url, false, &header_pairs).await?;
            if status == 404 {
                return Err(AppError::NotFound(format!("cover 404: {cover_url}")));
            }
            if status != 200 {
                return Err(AppError::Provider(format!(
                    "{reason}; curl fallback status {status} for {cover_url}"
                )));
            }
            tracing::debug!(cover_url = %cover_url, "cover downloaded via curl fallback");
            let content_type = infer_content_type(&body).to_owned();
            (content_type, cover_url.to_owned(), body)
        }
    };

    if bytes.len() < MIN_VALID_COVER_BYTES {
        return Err(AppError::Provider(format!(
            "cover too small ({} bytes < {} minimum): {cover_url}",
            bytes.len(),
            MIN_VALID_COVER_BYTES
        )));
    }

    let ext = extension_for(&final_url, &content_type);
    let target = cover_dir.join(format!("{code}{ext}"));
    tokio::fs::create_dir_all(cover_dir)
        .await
        .map_err(|error| AppError::Storage(format!("create cover dir: {error}")))?;

    let tmp = PathBuf::from(format!("{}.tmp", target.display()));
    if let Err(error) = tokio::fs::write(&tmp, &bytes).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(AppError::Storage(format!("write cover tmp: {error}")));
    }
    // Drop stale copies of the same code under other extensions, then finalize.
    remove_cover_files(cover_dir, &code).await;
    if let Err(error) = tokio::fs::rename(&tmp, &target).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(AppError::Storage(format!("finalize cover: {error}")));
    }
    Ok(target)
}

async fn remove_cover_files(cover_dir: &Path, code: &str) {
    for ext in KNOWN_EXTS {
        let path = cover_dir.join(format!("{code}{ext}"));
        let _ = tokio::fs::remove_file(path).await;
    }
}

fn normalize_code(code: &str) -> String {
    code.trim().to_lowercase()
}

/// Extension from the URL path when it looks like an image extension, else
/// guessed from the Content-Type header, else `.jpg`.
fn extension_for(url_path: &str, content_type: &str) -> String {
    let from_path = url_path
        .rsplit_once('.')
        .map(|(_, ext)| format!(".{}", ext.to_lowercase()))
        .filter(|ext| KNOWN_EXTS.contains(&ext.as_str()));
    if let Some(ext) = from_path {
        return ext;
    }
    guess_ext(content_type).unwrap_or_else(|| ".jpg".into())
}

fn guess_ext(content_type: &str) -> Option<String> {
    let content_type = content_type.to_lowercase();
    if content_type.contains("webp") {
        Some(".webp".into())
    } else if content_type.contains("png") {
        Some(".png".into())
    } else if content_type.contains("jpeg") || content_type.contains("jpg") {
        Some(".jpg".into())
    } else {
        None
    }
}

/// Content-type for the curl.exe fallback path, which has no response headers
/// to read: sniff the image magic bytes, defaulting to JPEG.
fn infer_content_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else {
        "image/jpeg"
    }
}

#[cfg(test)]
mod tests {
    use super::{extension_for, normalize_code};

    #[test]
    fn extension_prefers_url_path() {
        assert_eq!(extension_for("/pics/cover/abc.jpg", "image/webp"), ".jpg");
        assert_eq!(extension_for("/pics/cover/abc", "image/webp"), ".webp");
        assert_eq!(extension_for("/pics/cover/abc", "image/png"), ".png");
        assert_eq!(extension_for("/pics/cover/abc", ""), ".jpg");
        assert_eq!(extension_for("/pics/cover/abc.html", "image/jpeg"), ".jpg");
    }

    #[test]
    fn codes_normalize_to_lowercase() {
        assert_eq!(normalize_code(" IPX-633 "), "ipx-633");
    }
}
