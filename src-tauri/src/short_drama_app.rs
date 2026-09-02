//! 短剧 App-API 云端解析桥（锁定集直链的下载解密管线）。
//!
//! 红果官网 H5 每部剧只放开前几集网页直链（`accessible_episode_cnt`，普遍 3），
//! 之后官网播放页直接 404。桌面端的"全集可播"通过番茄小说 App 的
//! `multi_video_model` 接口实现：`resources/shortdrama-worker/` 里打包了
//! 嵌入式 Python + liushen 六代签名 + ffmpeg 解密管线，Rust 侧按需拉起
//! 单次进程 `worker.py resolve <vid>`，产出本地 mp4 后交给播放器播放。
//!
//! 设备凭据存放在数据目录 `short-drama-device.json`（deviceId/installId，
//! 来自番茄小说设备注册；与 FRAME 短剧工作台的 config.json 同源同格式）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Runtime};

/// 进度事件名（stage: sign/model/fallback/download/transcode/done）。
pub const RESOLVE_EVENT: &str = "shortdrama://app-resolve";
const WORKER_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaAppResolveInput {
    pub series_id: String,
    pub vid: String,
    #[serde(default)]
    pub content_type: Option<u16>,
    #[serde(default)]
    pub app_id: Option<u32>,
}

/// 红果两个客户端共用播放器接口，但请求模型和 aid 不同。
/// 仅接受 APK 逆向已确认的短剧/漫剧内容类型，避免前端传入任意模型值。
#[derive(Debug, Clone, Copy)]
struct HongguoAppProfile {
    content_type: u16,
    app_id: u32,
    cache_namespace: &'static str,
}

impl HongguoAppProfile {
    fn from_input(content_type: Option<u16>, app_id: Option<u32>) -> Result<Self, String> {
        let content_type = content_type.unwrap_or(1);
        let profile = match content_type {
            1 => Self {
                content_type,
                app_id: 8662,
                cache_namespace: "short-series",
            },
            1004 | 1007 => Self {
                content_type,
                app_id: 8704,
                cache_namespace: "motion-comic",
            },
            _ => {
                return Err(format!(
                    "不支持的红果 contentType={content_type}（仅支持短剧 1、漫剧 1004/1007）。"
                ));
            }
        };
        if let Some(app_id) = app_id {
            if app_id != profile.app_id {
                return Err(format!(
                    "contentType={} 必须使用 aid={}，收到 aid={app_id}。",
                    profile.content_type, profile.app_id
                ));
            }
        }
        Ok(profile)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaAppPlayback {
    pub play_url: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub cached: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaAppStatus {
    pub configured: bool,
    pub python_found: bool,
    pub worker_found: bool,
    pub ffmpeg_found: bool,
    pub cache_dir: String,
}

/// 锁定集直链流播（worker `stream` 子命令的产物）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaAppStream {
    pub url: String,
    /// CENC 内容密钥（hex）；源流未加密时为空串。
    pub decryption_key: String,
    pub width: u32,
    pub height: u32,
    pub download_ua: String,
    pub download_referer: String,
    /// App 播放模型中返回的全部可用清晰度，已在 worker 中按最高档排序。
    pub variants: Vec<ShortDramaAppVariant>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaAppVariant {
    pub id: String,
    pub label: String,
    pub url: String,
    pub decryption_key: String,
    pub width: u32,
    pub height: u32,
    pub bitrate: u64,
}

/// 专辑详情里的单集（worker `album` 子命令，按 vid_index 排序）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaAppEpisode {
    pub vid: String,
    pub index: u32,
    pub title: String,
    pub locked: bool,
    pub disabled: bool,
    pub duration_seconds: f64,
    pub cover: String,
}

/// 专辑详情（全集 vid 顺序 + 真实锁定态，来自 App-API album_detail）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaAppAlbum {
    pub series_id: String,
    pub title: String,
    pub cover: String,
    pub intro: String,
    pub total: u32,
    pub episodes: Vec<ShortDramaAppEpisode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceCredentials {
    device_id: String,
    install_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    device_token: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    cdid: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    openudid: String,
}

fn json_string_field(value: &serde_json::Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(text) = value
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            return text.to_owned();
        }
    }
    String::new()
}

fn explain_hongguo_api_error(detail: &str) -> String {
    if detail.contains("111104") || detail.contains("设备身份无效") {
        return "红果设备身份无效（111104）。请从真机抓包更新 deviceId / installId，并写入服务端下发的 deviceToken（x-tt-dt）。".into();
    }
    if detail.contains("110001") {
        return "红果播放模型异常（110001）。漫剧请使用 App V2 播放模型，或更换设备凭据后重试。".into();
    }
    detail.to_owned()
}

fn data_dir() -> PathBuf {
    if let Some(base) = dirs::data_local_dir() {
        return base.join("com.ttv.player");
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".ttv-data")
}

fn device_config_path() -> PathBuf {
    data_dir().join("short-drama-device.json")
}

fn cache_dir() -> PathBuf {
    data_dir().join("short-drama-cache")
}

/// 与 runtime::discover_resource_dir 相同的候选顺序，但以 worker/python 的
/// 存在为判据（worker 目录随 bundle.resources 复制到可执行文件旁）。
fn resource_base() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(value) = std::env::var_os("TTV_RESOURCE_DIR") {
        candidates.push(PathBuf::from(value));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join("resources"));
            candidates.push(parent.to_owned());
        }
    }
    if let Ok(current) = std::env::current_dir() {
        candidates.push(current.join("resources"));
        candidates.push(current.join("src-tauri/resources"));
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"));
    candidates.into_iter().find(|base| {
        base.join("shortdrama-worker/worker.py").is_file()
            || base.join("python/python.exe").is_file()
    })
}

fn worker_paths() -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let base = resource_base().ok_or_else(|| {
        "未找到短剧解析 worker 资源目录（shortdrama-worker）。请重新安装或完整打包应用。".to_owned()
    })?;
    let python = base.join("python/python.exe");
    let worker = base.join("shortdrama-worker/worker.py");
    let ffmpeg = base.join("mpv/ffmpeg.exe");
    if !python.is_file() {
        return Err(format!("嵌入式 Python 不存在：{}", python.display()));
    }
    if !worker.is_file() {
        return Err(format!("解析 worker 不存在：{}", worker.display()));
    }
    if !ffmpeg.is_file() {
        return Err(format!("ffmpeg 不存在：{}", ffmpeg.display()));
    }
    Ok((python, worker, ffmpeg))
}

fn load_credentials() -> Result<DeviceCredentials, String> {
    let path = device_config_path();
    let text = std::fs::read_to_string(&path).map_err(|error| {
        format!(
            "短剧云端解析未配置设备凭据（{} 读取失败：{error}）。请在短剧页设置 deviceId / installId。",
            path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        format!("设备凭据格式错误（{error}）。应为 deviceId + installId 两个字段。")
    })?;
    let device_id = json_string_field(&value, &["deviceId", "device_id"]);
    let install_id = json_string_field(&value, &["installId", "install_id", "iid"]);
    if device_id.is_empty() || install_id.is_empty() {
        return Err("设备凭据为空（deviceId / installId 均必填）。".to_owned());
    }
    Ok(DeviceCredentials {
        device_id,
        install_id,
        device_token: json_string_field(
            &value,
            &["deviceToken", "device_token", "xTtDt", "x_tt_dt", "x-tt-dt"],
        ),
        cdid: json_string_field(&value, &["cdid"]),
        openudid: json_string_field(&value, &["openudid", "openUdid"]),
    })
}

fn apply_hongguo_worker_env(
    command: &mut tokio::process::Command,
    credentials: &DeviceCredentials,
    profile: HongguoAppProfile,
) {
    command
        .env("TTV_SD_DEVICE_ID", &credentials.device_id)
        .env("TTV_SD_INSTALL_ID", &credentials.install_id)
        .env("TTV_SD_CONTENT_TYPE", profile.content_type.to_string())
        .env("TTV_SD_AID", profile.app_id.to_string());
    if !credentials.device_token.is_empty() {
        command.env("TTV_SD_DEVICE_TOKEN", &credentials.device_token);
    }
    if !credentials.cdid.is_empty() {
        command.env("TTV_SD_CDID", &credentials.cdid);
    }
    if !credentials.openudid.is_empty() {
        command.env("TTV_SD_OPENUDID", &credentials.openudid);
    }
}

#[tauri::command]
pub fn short_drama_app_status() -> ShortDramaAppStatus {
    let (python_found, worker_found, ffmpeg_found) = match worker_paths() {
        Ok((python, worker, ffmpeg)) => (python.is_file(), worker.is_file(), ffmpeg.is_file()),
        Err(_) => (false, false, false),
    };
    ShortDramaAppStatus {
        configured: load_credentials().is_ok(),
        python_found,
        worker_found,
        ffmpeg_found,
        cache_dir: cache_dir().to_string_lossy().to_string(),
    }
}

#[tauri::command]
pub fn short_drama_app_set_device(device_id: String, install_id: String) -> Result<String, String> {
    let device_id = device_id.trim().to_owned();
    let install_id = install_id.trim().to_owned();
    if device_id.is_empty() || install_id.is_empty() {
        return Err("deviceId / installId 均不能为空。".into());
    }
    if !device_id.chars().all(|c| c.is_ascii_digit())
        || !install_id.chars().all(|c| c.is_ascii_digit())
    {
        return Err("deviceId / installId 应为纯数字 ID。".into());
    }
    let path = device_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let existing = load_credentials().ok();
    let payload = DeviceCredentials {
        device_id,
        install_id,
        device_token: existing
            .as_ref()
            .map(|item| item.device_token.clone())
            .unwrap_or_default(),
        cdid: existing
            .as_ref()
            .map(|item| item.cdid.clone())
            .unwrap_or_default(),
        openudid: existing
            .as_ref()
            .map(|item| item.openudid.clone())
            .unwrap_or_default(),
    };
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("写入设备凭据失败：{error}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// 解析一集：命中缓存直接返回；否则拉起 worker.py（下载+解密+转存）并转发进度。
#[tauri::command]
pub async fn short_drama_app_resolve<R: Runtime>(
    app: AppHandle<R>,
    input: ShortDramaAppResolveInput,
) -> Result<ShortDramaAppPlayback, String> {
    let vid = input.vid.trim().to_owned();
    if vid.is_empty() || !vid.chars().all(|c| c.is_ascii_digit()) {
        return Err("缺少有效的集 vid。".into());
    }
    let profile = HongguoAppProfile::from_input(input.content_type, input.app_id)?;
    let (python, worker, ffmpeg) = worker_paths()?;
    let credentials = load_credentials()?;

    let out_path = cache_dir()
        .join(profile.cache_namespace)
        .join(format!("{vid}.mp4"));
    if out_path.is_file()
        && out_path
            .metadata()
            .map(|meta| meta.len() > 0)
            .unwrap_or(false)
    {
        return Ok(ShortDramaAppPlayback {
            play_url: out_path.to_string_lossy().to_string(),
            width: 0,
            height: 0,
            size_bytes: out_path.metadata().map(|meta| meta.len()).unwrap_or(0),
            cached: true,
        });
    }
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建缓存目录失败：{error}"))?;
    }

    let _ = app.emit(
        RESOLVE_EVENT,
        serde_json::json!({"vid": vid, "stage": "start", "message": "正在启动云端解析"}),
    );

    let mut command = tokio::process::Command::new(&python);
    command
        .arg(&worker)
        .arg("resolve")
        .arg(&vid);
    apply_hongguo_worker_env(&mut command, &credentials, profile);
    command
        .env("TTV_SD_FFMPEG", &ffmpeg)
        .env("TTV_SD_OUT", &out_path)
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // CREATE_NO_WINDOW：后台进程不闪控制台窗口。
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动解析 worker 失败：{error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "解析 worker stdout 不可读".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "解析 worker stderr 不可读".to_owned())?;

    let emit_app = app.clone();
    let emit_vid = vid.clone();
    let reader = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(stdout).lines();
        let mut final_payload: Option<serde_json::Value> = None;
        let mut error_text = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                error_text.push_str(trimmed);
                error_text.push('\n');
                continue;
            };
            match value.get("event").and_then(serde_json::Value::as_str) {
                Some("progress") => {
                    let _ = emit_app.emit(
                        RESOLVE_EVENT,
                        serde_json::json!({
                            "vid": emit_vid,
                            "stage": value.get("stage").cloned().unwrap_or(serde_json::Value::Null),
                            "message": value.get("message").cloned().unwrap_or(serde_json::Value::Null),
                            "percent": value.get("percent").cloned().unwrap_or(serde_json::Value::Null),
                        }),
                    );
                }
                Some("done") => final_payload = Some(value),
                _ => error_text.push_str(trimmed),
            }
        }
        (final_payload, error_text)
    });
    // stderr 必须持续排空（否则管道写满会卡死 worker），失败时取尾部辅助定位。
    let stderr_reader = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(stderr).lines();
        let mut collected = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if collected.len() < 8000 {
                collected.push_str(&line);
                collected.push('\n');
            }
        }
        collected
    });

    let wait_result = tokio::time::timeout(WORKER_TIMEOUT, child.wait()).await;
    let status = match wait_result {
        Ok(Ok(status)) => Some(status),
        Ok(Err(error)) => {
            reader.abort();
            return Err(format!("解析 worker 退出异常：{error}"));
        }
        Err(_) => {
            let _ = child.kill().await;
            reader.abort();
            return Err("云端解析超时（300 秒），请稍后重试。".into());
        }
    };
    let (final_payload, error_text) = reader
        .await
        .map_err(|error| format!("读取解析输出失败：{error}"))?;
    let stderr_text = stderr_reader.await.unwrap_or_else(|_| String::new());

    if !status.map(|status| status.success()).unwrap_or(false) || final_payload.is_none() {
        let detail = final_payload
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let mut combined = error_text.trim().to_owned();
                let stderr_tail = stderr_text.trim().to_owned();
                if !stderr_tail.is_empty() {
                    if !combined.is_empty() {
                        combined.push_str(" | ");
                    }
                    combined.push_str(
                        &stderr_tail
                            .lines()
                            .rev()
                            .take(3)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<Vec<_>>()
                            .join(" / "),
                    );
                }
                combined
            });
        let _ = app.emit(
            RESOLVE_EVENT,
            serde_json::json!({"vid": vid, "stage": "error", "message": detail}),
        );
        // 半成品文件清理
        let _ = std::fs::remove_file(&out_path);
        return Err(if detail.is_empty() {
            "云端解析失败（worker 未返回结果）。".to_owned()
        } else {
            format!("云端解析失败：{}", explain_hongguo_api_error(&detail))
        });
    }

    let payload = final_payload.unwrap();
    let width = payload
        .get("width")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let height = payload
        .get("height")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let size = payload
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let _ = app.emit(
        RESOLVE_EVENT,
        serde_json::json!({"vid": vid, "stage": "done", "message": "解析完成"}),
    );
    Ok(ShortDramaAppPlayback {
        play_url: out_path.to_string_lossy().to_string(),
        width,
        height,
        size_bytes: size,
        cached: false,
    })
}

#[tauri::command]
pub fn short_drama_app_cache_clear() -> Result<String, String> {
    let dir = cache_dir();
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|error| format!("清理缓存失败：{error}"))?;
    }
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.to_string_lossy().to_string())
}

// ============ stream / album（签名直链与专辑详情，共用 worker 进程） ============

/// 拉起单次 worker 子命令并收集最终 `{"event":"done",...}` 载荷。
/// 与 resolve 的差别：不需要 ffmpeg/OUT 环境变量，进度事件带 `action` 区分。
async fn run_worker_subcommand<R: Runtime>(
    app: &AppHandle<R>,
    subcommand: &str,
    target: &str,
    action: &'static str,
    profile: HongguoAppProfile,
) -> Result<serde_json::Value, String> {
    let (python, worker, _ffmpeg) = worker_paths()?;
    let credentials = load_credentials()?;

    let mut command = tokio::process::Command::new(&python);
    command
        .arg(&worker)
        .arg(subcommand)
        .arg(target);
    apply_hongguo_worker_env(&mut command, &credentials, profile);
    command
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动解析 worker 失败：{error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "解析 worker stdout 不可读".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "解析 worker stderr 不可读".to_owned())?;

    let emit_app = app.clone();
    let reader = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(stdout).lines();
        let mut final_payload: Option<serde_json::Value> = None;
        let mut error_text = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                error_text.push_str(trimmed);
                error_text.push('\n');
                continue;
            };
            match value.get("event").and_then(serde_json::Value::as_str) {
                Some("progress") => {
                    let _ = emit_app.emit(
                        RESOLVE_EVENT,
                        serde_json::json!({
                            "action": action,
                            "stage": value.get("stage").cloned().unwrap_or(serde_json::Value::Null),
                            "message": value.get("message").cloned().unwrap_or(serde_json::Value::Null),
                        }),
                    );
                }
                Some("done") => final_payload = Some(value),
                _ => error_text.push_str(trimmed),
            }
        }
        (final_payload, error_text)
    });
    let stderr_reader = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(stderr).lines();
        let mut collected = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if collected.len() < 4000 {
                collected.push_str(&line);
                collected.push('\n');
            }
        }
        collected
    });

    let wait_result = tokio::time::timeout(WORKER_TIMEOUT, child.wait()).await;
    let status = match wait_result {
        Ok(Ok(status)) => Some(status),
        Ok(Err(error)) => {
            reader.abort();
            return Err(format!("解析 worker 退出异常：{error}"));
        }
        Err(_) => {
            let _ = child.kill().await;
            reader.abort();
            return Err("云端解析超时（300 秒），请稍后重试。".into());
        }
    };
    let (final_payload, error_text) = reader
        .await
        .map_err(|error| format!("读取解析输出失败：{error}"))?;
    let stderr_text = stderr_reader.await.unwrap_or_else(|_| String::new());

    if !status.map(|status| status.success()).unwrap_or(false) || final_payload.is_none() {
        let mut combined = error_text.trim().to_owned();
        let stderr_tail = stderr_text.trim().to_owned();
        if !stderr_tail.is_empty() {
            if !combined.is_empty() {
                combined.push_str(" | ");
            }
            combined.push_str(
                &stderr_tail
                    .lines()
                    .rev()
                    .take(3)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" / "),
            );
        }
        return Err(if combined.is_empty() {
            format!("worker {subcommand} 失败（未返回结果）。")
        } else {
            format!(
                "worker {subcommand} 失败：{}",
                explain_hongguo_api_error(&combined)
            )
        });
    }
    let payload = final_payload.unwrap();
    if payload.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        let detail = payload
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("未知错误");
        return Err(format!(
            "worker {subcommand} 失败：{}",
            explain_hongguo_api_error(detail)
        ));
    }
    Ok(payload)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaAppStreamInput {
    pub vid: String,
    #[serde(default)]
    pub content_type: Option<u16>,
    #[serde(default)]
    pub app_id: Option<u32>,
}

/// 锁定集秒开：签名取回加密直链 + CENC 密钥，交给 libmpv 流播（不落盘）。
#[tauri::command]
pub async fn short_drama_app_stream<R: Runtime>(
    app: AppHandle<R>,
    input: ShortDramaAppStreamInput,
) -> Result<ShortDramaAppStream, String> {
    let vid = input.vid.trim().to_owned();
    if vid.is_empty() || !vid.chars().all(|c| c.is_ascii_digit()) {
        return Err("缺少有效的集 vid。".into());
    }
    let profile = HongguoAppProfile::from_input(input.content_type, input.app_id)?;
    let payload = run_worker_subcommand(&app, "stream", &vid, "stream", profile).await?;
    let url = payload
        .get("url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if url.is_empty() {
        return Err("云端直链为空，请回退到完整解析。".into());
    }
    let width = payload
        .get("width")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let height = payload
        .get("height")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let variants = payload
        .get("variants")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| {
                    let url = value.get("url").and_then(serde_json::Value::as_str)?.trim();
                    if url.is_empty() {
                        return None;
                    }
                    Some(ShortDramaAppVariant {
                        id: value
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        label: value
                            .get("label")
                            .and_then(serde_json::Value::as_str)
                            .filter(|label| !label.trim().is_empty())
                            .unwrap_or("原始画质")
                            .to_owned(),
                        url: url.to_owned(),
                        decryption_key: value
                            .get("content_key")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        width: value
                            .get("width")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0) as u32,
                        height: value
                            .get("height")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0) as u32,
                        bitrate: value
                            .get("bitrate")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let variants = if variants.is_empty() {
        vec![ShortDramaAppVariant {
            id: "default".to_owned(),
            label: if height > 0 {
                format!("{height}P")
            } else {
                "最高".to_owned()
            },
            url: url.clone(),
            decryption_key: payload
                .get("content_key")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            width,
            height,
            bitrate: 0,
        }]
    } else {
        variants
    };
    Ok(ShortDramaAppStream {
        url,
        decryption_key: payload
            .get("content_key")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        width,
        height,
        download_ua: payload
            .get("download_ua")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("com.phoenix.read/71332")
            .to_owned(),
        download_referer: payload
            .get("download_referer")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("https://novel.snssdk.com/")
            .to_owned(),
        variants,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortDramaAppAlbumInput {
    pub series_id: String,
    #[serde(default)]
    pub content_type: Option<u16>,
    #[serde(default)]
    pub app_id: Option<u32>,
}

/// 专辑详情：全集 vid 顺序 + 真实锁定态（修正官网“前 N 集”的粗粒度徽标）。
#[tauri::command]
pub async fn short_drama_app_album<R: Runtime>(
    app: AppHandle<R>,
    input: ShortDramaAppAlbumInput,
) -> Result<ShortDramaAppAlbum, String> {
    let series_id = input.series_id.trim().to_owned();
    if series_id.is_empty() || !series_id.chars().all(|c| c.is_ascii_digit()) {
        return Err("缺少有效的剧集 ID。".into());
    }
    let profile = HongguoAppProfile::from_input(input.content_type, input.app_id)?;
    let payload = run_worker_subcommand(&app, "album", &series_id, "album", profile).await?;
    let episodes = payload
        .get("episodes")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| {
                    let vid = value.get("vid").and_then(serde_json::Value::as_str)?;
                    Some(ShortDramaAppEpisode {
                        vid: vid.to_owned(),
                        index: value
                            .get("index")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0) as u32,
                        title: value
                            .get("title")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        locked: value
                            .get("locked")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        disabled: value
                            .get("disabled")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        duration_seconds: value
                            .get("duration_seconds")
                            .and_then(serde_json::Value::as_f64)
                            .unwrap_or(0.0),
                        cover: value
                            .get("cover")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if episodes.is_empty() {
        return Err("专辑详情没有分集信息。".into());
    }
    Ok(ShortDramaAppAlbum {
        series_id,
        title: payload
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        cover: payload
            .get("cover")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        intro: payload
            .get("intro")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        total: payload
            .get("total")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(episodes.len() as u64) as u32,
        episodes,
    })
}

#[cfg(test)]
mod tests {
    use super::HongguoAppProfile;

    #[test]
    fn reads_device_token_aliases_from_credentials_json() {
        let value = serde_json::json!({
            "deviceId": "111",
            "install_id": "222",
            "x-tt-dt": "token-from-server",
            "cdid": "cdid-1"
        });
        assert_eq!(super::json_string_field(&value, &["deviceId", "device_id"]), "111");
        assert_eq!(super::json_string_field(&value, &["installId", "install_id"]), "222");
        assert_eq!(
            super::json_string_field(&value, &["deviceToken", "x-tt-dt", "x_tt_dt"]),
            "token-from-server"
        );
        assert_eq!(
            super::explain_hongguo_api_error("worker stream 失败：111104 SERVICE_ERROR"),
            "红果设备身份无效（111104）。请从真机抓包更新 deviceId / installId，并写入服务端下发的 deviceToken（x-tt-dt）。"
        );
    }

    #[test]
    fn matches_confirmed_hongguo_content_profiles() {
        let short = HongguoAppProfile::from_input(Some(1), Some(8662)).unwrap();
        assert_eq!(short.app_id, 8662);
        assert_eq!(short.cache_namespace, "short-series");

        let comic = HongguoAppProfile::from_input(Some(1004), Some(8704)).unwrap();
        assert_eq!(comic.app_id, 8704);
        assert_eq!(comic.cache_namespace, "motion-comic");

        assert!(HongguoAppProfile::from_input(Some(1), Some(8704)).is_err());
        assert!(HongguoAppProfile::from_input(Some(999), None).is_err());
    }
}
