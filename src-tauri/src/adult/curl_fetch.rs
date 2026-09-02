//! Shared OS-curl.exe fetch session for hosts whose Cloudflare/WAF setup
//! blocks the reqwest TLS fingerprint (rustls and hyper+native-tls both get
//! "Just a moment" 403 while OS curl passes — A/B-verified 2026-08-29 on
//! sehuatang.net and javbus.com image hosts).
//!
//! Each session owns a Netscape cookie jar + a body scratch file in the OS
//! temp dir (removed on drop). Requests run through the `curl` executable,
//! paced by the caller. Proxy resolution: explicit HTTPS_PROXY/ALL_PROXY env
//! first; when none is set and the direct attempt dies at the transport layer
//! (DNS / connect / timeout — e.g. DNS pollution on CN networks), the request
//! is retried once through a locally listening socks port and the working
//! proxy is remembered for the rest of the session.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

use crate::error::AppError;

/// Local proxy candidates probed when direct transport fails and no env proxy
/// is configured (sing-box socks / clash-style mixed port).
const FALLBACK_PROXY_ENDPOINTS: &[(&str, u16)] = &[("127.0.0.1", 10808), ("127.0.0.1", 7890)];

/// curl exit codes that mean "never reached the host" and are worth retrying
/// through the local proxy: 6 resolve failure, 7 connect failure, 28 timeout.
const PROXY_RETRY_EXIT_CODES: &[i32] = &[6, 7, 28];

/// One curl session: temp cookie jar + body file, both removed on drop.
pub struct CurlFetch {
    jar: PathBuf,
    body: PathBuf,
    proxy: Option<String>,
    /// Proxy discovered by the transport-failure fallback, shared across the
    /// session's requests (interior mutability: get/post take &self).
    learned_proxy: Mutex<Option<String>>,
}

impl CurlFetch {
    pub fn new() -> Result<Self, AppError> {
        let id = uuid::Uuid::new_v4();
        Ok(Self {
            jar: std::env::temp_dir().join(format!("ttv-curl-{id}.jar")),
            body: std::env::temp_dir().join(format!("ttv-curl-{id}.body")),
            proxy: detect_proxy(),
            learned_proxy: Mutex::new(None),
        })
    }

    fn effective_proxy(&self) -> Option<String> {
        if self.proxy.is_some() {
            return self.proxy.clone();
        }
        self.learned_proxy
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// Force the local socks fallback for the rest of this session. Used when
    /// a site answers fine but withholds guest tokens from the direct IP
    /// (IP-level issuance limits) — proxy egress gets a different IP.
    pub fn enable_local_socks_fallback(&self) -> bool {
        let Some(proxy) = fallback_proxy() else {
            return false;
        };
        if let Ok(mut slot) = self.learned_proxy.lock() {
            *slot = Some(proxy);
            true
        } else {
            false
        }
    }

    /// GET a URL through OS curl.exe. `follow` enables -L for the Discuz
    /// search-submit 302 dance; `headers` are sent on every hop. Returns the
    /// final HTTP status and body bytes.
    pub async fn get(
        &self,
        url: &str,
        follow: bool,
        headers: &[(String, String)],
    ) -> Result<(u16, Vec<u8>), AppError> {
        let jar = self.jar.clone();
        let body_path = self.body.clone();
        let proxy = self.effective_proxy();
        let url = url.to_owned();
        let url_for_error = url.clone();
        let headers = headers.to_vec();
        let learned_slot = Arc::new(Mutex::new(None::<String>));
        let slot = Arc::clone(&learned_slot);
        let (status, body) =
            tokio::task::spawn_blocking(move || -> Result<(u16, Vec<u8>), String> {
                run_with_proxy_fallback(
                    proxy,
                    &slot,
                    &|proxy: Option<&str>| {
                        let mut command = Command::new("curl");
                        command
                            .arg("-sS")
                            .arg("--max-time")
                            .arg("25")
                            .arg("--max-redirs")
                            .arg("5")
                            .arg("-b")
                            .arg(&jar)
                            .arg("-c")
                            .arg(&jar)
                            .arg("-o")
                            .arg(&body_path)
                            .arg("-w")
                            .arg("%{http_code}");
                        if follow {
                            command.arg("-L");
                        }
                        if let Some(proxy) = proxy {
                            command.arg("-x").arg(proxy);
                        }
                        for (name, value) in &headers {
                            command.arg("-H").arg(format!("{name}: {value}"));
                        }
                        command.arg(&url);
                        command
                    },
                    &url,
                )
                .map(|(status, _)| (status, std::fs::read(&body_path).unwrap_or_default()))
            })
            .await
            .map_err(|error| AppError::Provider(format!("curl join {url_for_error}: {error}")))?
            .map_err(|error| AppError::Provider(format!("curl {url_for_error}: {error}")))?;
        remember_learned_proxy(self, &learned_slot);
        Ok((status, body))
    }

    /// POST a body to a URL through OS curl.exe. The body is staged in a temp
    /// file so large payloads never hit the Windows command-line limit. As with
    /// `get`, returns the final HTTP status and response bytes.
    pub async fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), AppError> {
        let id = uuid::Uuid::new_v4();
        let body_in = std::env::temp_dir().join(format!("ttv-curl-{id}.req"));
        let body_in_cleanup = body_in.clone();
        std::fs::write(&body_in, body)
            .map_err(|error| AppError::Provider(format!("curl post stage: {error}")))?;
        let jar = self.jar.clone();
        let body_path = self.body.clone();
        let proxy = self.effective_proxy();
        let url = url.to_owned();
        let url_for_error = url.clone();
        let headers = headers.to_vec();
        let learned_slot = Arc::new(Mutex::new(None::<String>));
        let slot = Arc::clone(&learned_slot);
        let result = tokio::task::spawn_blocking(move || -> Result<(u16, Vec<u8>), String> {
            run_with_proxy_fallback(
                proxy,
                &slot,
                &|proxy: Option<&str>| {
                    let mut command = Command::new("curl");
                    command
                        .arg("-sS")
                        .arg("--max-time")
                        .arg("25")
                        .arg("--max-redirs")
                        .arg("5")
                        .arg("-b")
                        .arg(&jar)
                        .arg("-c")
                        .arg(&jar)
                        .arg("-o")
                        .arg(&body_path)
                        .arg("-w")
                        .arg("%{http_code}")
                        .arg("--data-binary")
                        .arg(format!("@{}", body_in.display()));
                    if let Some(proxy) = proxy {
                        command.arg("-x").arg(proxy);
                    }
                    for (name, value) in &headers {
                        command.arg("-H").arg(format!("{name}: {value}"));
                    }
                    command.arg(&url);
                    command
                },
                &url,
            )
            .map(|(status, _)| (status, std::fs::read(&body_path).unwrap_or_default()))
        })
        .await
        .map_err(|error| AppError::Provider(format!("curl join {url_for_error}: {error}")))?
        .map_err(|error| AppError::Provider(format!("curl {url_for_error}: {error}")));
        let _ = std::fs::remove_file(&body_in_cleanup);
        remember_learned_proxy(self, &learned_slot);
        result
    }

    /// Body bytes of the most recent `get`, for callers that stream to disk.
    pub async fn read_body(&self) -> Vec<u8> {
        let body_path = self.body.clone();
        tokio::task::spawn_blocking(move || std::fs::read(&body_path).unwrap_or_default())
            .await
            .unwrap_or_default()
    }

    /// Append a cookie to the jar in Netscape format so subsequent requests
    /// replay it (e.g. the sehuatang `_safe` gate cookie).
    pub fn inject_cookie(&self, host: &str, name: &str, value: &str) {
        let line = format!("{host}\tFALSE\t/\tFALSE\t0\t{name}\t{value}\n");
        if let Err(error) = {
            use std::io::Write as _;
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&self.jar)
                .and_then(|mut file| file.write_all(line.as_bytes()))
        } {
            tracing::warn!(host = %host, name = %name, error = %error, "curl jar write failed");
        }
    }
}

impl Drop for CurlFetch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.jar);
        let _ = std::fs::remove_file(&self.body);
    }
}

/// Many target hosts are DNS-polluted on CN networks, so honour the standard
/// proxy env vars; socks5h (proxy-side DNS) is what bypasses the pollution.
pub fn detect_proxy() -> Option<String> {
    if let Ok(no_proxy) = std::env::var("NO_PROXY").or_else(|_| std::env::var("no_proxy")) {
        let no_proxy = no_proxy.to_ascii_lowercase();
        if no_proxy.split(',').any(|entry| {
            let entry = entry.trim();
            entry == "*" || entry.contains("sehuatang") || entry.contains("javbus")
        }) {
            return None;
        }
    }
    for key in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim().to_owned();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Local socks fallback probed when direct transport fails without an explicit
/// proxy (sing-box / clash-style listeners). `NO_PROXY="*"` disables it.
fn fallback_proxy() -> Option<String> {
    if let Ok(no_proxy) = std::env::var("NO_PROXY").or_else(|_| std::env::var("no_proxy")) {
        if no_proxy.split(',').any(|entry| entry.trim() == "*") {
            return None;
        }
    }
    for (host, port) in FALLBACK_PROXY_ENDPOINTS {
        let Ok(addr) = format!("{host}:{port}").parse::<std::net::SocketAddr>() else {
            continue;
        };
        if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300))
            .is_ok()
        {
            return Some(format!("socks5h://{host}:{port}"));
        }
    }
    None
}

/// Run one curl attempt; on a never-reached-host transport failure with no
/// explicit proxy configured, retry once through the local socks port.
/// Success on the retry is recorded into `learned_slot` so the caller can pin
/// it for the rest of the session. Error strings keep the historical
/// "curl transport failed: …" shape.
fn run_with_proxy_fallback(
    proxy: Option<String>,
    learned_slot: &Mutex<Option<String>>,
    build_command: &dyn Fn(Option<&str>) -> Command,
    url: &str,
) -> Result<(u16, Vec<u8>), String> {
    // curl returns exit 0 for HTTP-level errors (403 etc.) and non-zero only
    // for transport failures; keep that distinction.
    let output = build_command(proxy.as_deref())
        .output()
        .map_err(|error| error.to_string())?;
    let exit_code = output.status.code().unwrap_or(0);
    if output.status.success() {
        let status: u16 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        return Ok((status, Vec::new())); // body read by caller contract below
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if proxy.is_some() || !PROXY_RETRY_EXIT_CODES.contains(&exit_code) {
        return Err(format!("curl transport failed: {stderr}"));
    }
    let Some(fallback) = fallback_proxy() else {
        return Err(format!("curl transport failed: {stderr}"));
    };
    tracing::info!(url = %url, proxy = %fallback, "curl direct failed; retrying via local socks proxy");
    let retry = build_command(Some(&fallback))
        .output()
        .map_err(|error| error.to_string())?;
    if !retry.status.success() {
        let retry_stderr = String::from_utf8_lossy(&retry.stderr).trim().to_owned();
        return Err(format!("curl transport failed: {retry_stderr}"));
    }
    if let Ok(mut slot) = learned_slot.lock() {
        *slot = Some(fallback);
    }
    let status: u16 = String::from_utf8_lossy(&retry.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    Ok((status, Vec::new()))
}

// The run helper returns only the status code; body bytes live in the shared
// scratch file, so wrap it to preserve the original (status, body) contract.
fn remember_learned_proxy(session: &CurlFetch, learned_slot: &Mutex<Option<String>>) {
    if let Ok(guard) = learned_slot.lock() {
        if let Some(proxy) = guard.clone() {
            if let Ok(mut slot) = session.learned_proxy.lock() {
                *slot = Some(proxy);
            }
        }
    }
}
