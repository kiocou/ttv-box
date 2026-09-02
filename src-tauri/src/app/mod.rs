use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::openlist::OpenListRuntime;
use crate::playback::PlaybackActor;
use crate::providers::ProviderRouter;
use crate::providers::StreamHubConfig;
use crate::runtime::RuntimeDiagnostics;
use crate::storage::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub resource_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamHubRuntimeStatus {
    pub configured: bool,
    pub base_url: String,
    pub jar_path: Option<String>,
    pub java_path: Option<String>,
    pub running: bool,
    pub started_at_ms: Option<u64>,
    pub message: String,
}

#[derive(Debug)]
struct StreamHubProcessState {
    child: Option<Child>,
    started_at_ms: Option<u64>,
    last_message: String,
}

#[derive(Debug)]
pub struct StreamHubRuntime {
    config: StreamHubConfig,
    data_dir: PathBuf,
    state: Mutex<StreamHubProcessState>,
}

impl StreamHubRuntime {
    pub fn new(config: StreamHubConfig, data_dir: PathBuf) -> Self {
        Self {
            config,
            data_dir,
            state: Mutex::new(StreamHubProcessState {
                child: None,
                started_at_ms: None,
                last_message: "未启动".into(),
            }),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    fn jar_path(&self) -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if let Some(path) = self
            .config
            .jar_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
        {
            candidates.push(PathBuf::from(path));
        }
        candidates.extend([
            self.data_dir.join("resources/streamhub-local-api.jar"),
            self.data_dir
                .join("resources/streamhub-local-api/streamhub-local-api-0.1.0.jar"),
            PathBuf::from("src-tauri/resources/streamhub-local-api.jar"),
            PathBuf::from("逆向分析工程/2026-08-17-22-07-44/analysis/streamhub_run/streamhub-local-api-0.1.0.jar"),
        ]);
        if let Ok(current) = std::env::current_dir() {
            candidates.push(current.join("resources/streamhub-local-api.jar"));
        }
        candidates.into_iter().find(|path| path.is_file())
    }

    fn java_command(&self) -> String {
        self.config
            .java_path
            .clone()
            .or_else(|| std::env::var("TTV_JAVA_PATH").ok())
            .filter(|path| !path.trim().is_empty())
            .unwrap_or_else(|| "java".into())
    }

    fn refresh_child_locked(state: &mut StreamHubProcessState) -> bool {
        if let Some(child) = state.child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    state.child = None;
                    state.started_at_ms = None;
                    state.last_message = format!("StreamHub 已退出：{status}");
                    false
                }
                Ok(None) => true,
                Err(error) => {
                    state.last_message = format!("无法检查 StreamHub 进程：{error}");
                    false
                }
            }
        } else {
            false
        }
    }

    pub fn start(&self) -> Result<StreamHubRuntimeStatus, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "StreamHub 状态锁已损坏".to_owned())?;
        if Self::refresh_child_locked(&mut state) {
            state.last_message = "StreamHub 已在运行".into();
            return Ok(self.status_locked(&state));
        }
        let jar = self.jar_path().ok_or_else(|| {
            "未找到 streamhub-local-api.jar，请在 config.json 配置 jarPath".to_owned()
        })?;
        let port = reqwest::Url::parse(&self.config.base_url)
            .ok()
            .and_then(|url| url.port_or_known_default())
            .unwrap_or(18400);
        let java = self.java_command();
        let streamhub_data = self.data_dir.join("streamhub");
        std::fs::create_dir_all(&streamhub_data)
            .map_err(|error| format!("无法创建 StreamHub 数据目录：{error}"))?;
        let mut command = Command::new(&java);
        command
            .args([
                "-Dserver.address=127.0.0.1",
                &format!("-Dserver.port={port}"),
                "-Dstreamhub.desktop-proxy.enabled=true",
            ])
            .arg("-jar")
            .arg(&jar)
            .current_dir(&streamhub_data)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command
            .spawn()
            .map_err(|error| format!("无法启动 Java/StreamHub：{error}"))?;
        state.child = Some(child);
        state.started_at_ms = Some(now_ms());
        state.last_message = format!(
            "已启动 StreamHub，等待 {}/api/system/health",
            self.config.base_url
        );
        Ok(self.status_locked(&state))
    }

    pub fn stop(&self) -> Result<StreamHubRuntimeStatus, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "StreamHub 状态锁已损坏".to_owned())?;
        if let Some(mut child) = state.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        state.started_at_ms = None;
        state.last_message = "已停止 StreamHub".into();
        Ok(self.status_locked(&state))
    }

    pub fn status(&self) -> StreamHubRuntimeStatus {
        let Ok(mut state) = self.state.lock() else {
            return StreamHubRuntimeStatus {
                configured: false,
                base_url: self.config.base_url.clone(),
                jar_path: self.jar_path().map(|path| path.display().to_string()),
                java_path: Some(self.java_command()),
                running: false,
                started_at_ms: None,
                message: "StreamHub 状态锁已损坏".into(),
            };
        };
        Self::refresh_child_locked(&mut state);
        self.status_locked(&state)
    }

    fn status_locked(&self, state: &StreamHubProcessState) -> StreamHubRuntimeStatus {
        StreamHubRuntimeStatus {
            configured: self.jar_path().is_some(),
            base_url: self.config.base_url.clone(),
            jar_path: self.jar_path().map(|path| path.display().to_string()),
            java_path: Some(self.java_command()),
            running: state.child.is_some(),
            started_at_ms: state.started_at_ms,
            message: state.last_message.clone(),
        }
    }
}

impl Drop for StreamHubRuntime {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut() {
            if let Some(child) = state.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

impl AppPaths {
    pub fn from_data_dir(data_dir: PathBuf) -> Self {
        Self {
            cache_dir: data_dir.join("cache"),
            resource_dir: data_dir.join("resources"),
            data_dir,
        }
    }
}

#[derive(Debug)]
pub struct AppState {
    pub paths: AppPaths,
    pub database: Arc<Database>,
    pub providers: ProviderRouter,
    pub guangya_oauth_missing_fields: Vec<String>,
    pub runtime: RuntimeDiagnostics,
    pub streamhub: StreamHubRuntime,
    pub openlist: OpenListRuntime,
    pub playback: Mutex<Option<PlaybackActor>>,
    /// Frontend session that owns the current embedded libmpv actor. Native
    /// open/close can overlap while a signed URL is resolving; the token keeps
    /// a late close from an older playback session away from a newer actor.
    pub native_playback_session: AtomicU64,
    pub browser_transcode_jobs: Mutex<HashMap<String, Child>>,
    pub task_cancel: Arc<AtomicBool>,
    session_refresh_locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl AppState {
    pub fn new(
        paths: AppPaths,
        database: Database,
        providers: ProviderRouter,
        runtime: RuntimeDiagnostics,
        playback: Option<PlaybackActor>,
        streamhub: StreamHubRuntime,
        openlist: OpenListRuntime,
    ) -> Self {
        Self {
            paths,
            database: Arc::new(database),
            providers,
            guangya_oauth_missing_fields: Vec::new(),
            runtime,
            streamhub,
            openlist,
            playback: Mutex::new(playback),
            native_playback_session: AtomicU64::new(0),
            browser_transcode_jobs: Mutex::new(HashMap::new()),
            task_cancel: Arc::new(AtomicBool::new(false)),
            session_refresh_locks: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn with_guangya_oauth_missing_fields(mut self, missing_fields: Vec<String>) -> Self {
        self.guangya_oauth_missing_fields = missing_fields;
        self
    }

    pub async fn lock_session_refresh(
        &self,
        provider_id: &str,
        account_id: Option<&str>,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let key = format!("{provider_id}:{}", account_id.unwrap_or("default"));
        let lock = {
            let mut locks = self.session_refresh_locks.lock().await;
            Arc::clone(
                locks
                    .entry(key)
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        lock.lock_owned().await
    }

    pub fn reset_task_cancel(&self) {
        self.task_cancel.store(false, Ordering::Release);
    }

    pub fn cancel_tasks(&self) {
        self.task_cancel.store(true, Ordering::Release);
    }

    pub fn tasks_cancelled(&self) -> bool {
        self.task_cancel.load(Ordering::Acquire)
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Ok(jobs) = self.browser_transcode_jobs.get_mut() {
            for child in jobs.values_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}
