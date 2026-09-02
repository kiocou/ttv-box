use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Url;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenListRuntimeStatus {
    pub running: bool,
    pub reachable: bool,
    pub base_url: String,
    pub version: Option<String>,
    pub message: String,
    pub binary_path: Option<String>,
    pub started_at_ms: Option<u64>,
}

#[derive(Debug)]
struct ProcessState {
    child: Option<Child>,
    started_at_ms: Option<u64>,
    message: String,
}

#[derive(Debug)]
pub struct OpenListRuntime {
    data_dir: PathBuf,
    base_url: String,
    binary_path: Option<PathBuf>,
    state: Mutex<ProcessState>,
}

impl OpenListRuntime {
    pub fn new(data_dir: PathBuf) -> Self {
        let base_url = std::env::var("TTV_OPENLIST_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:5244".to_owned());
        let binary_path = std::env::var_os("TTV_OPENLIST_BIN")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .or_else(|| Self::discover_binary());
        Self {
            data_dir,
            base_url,
            binary_path,
            state: Mutex::new(ProcessState {
                child: None,
                started_at_ms: None,
                message: "OpenList 尚未启动".into(),
            }),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn start(&self) -> Result<OpenListRuntimeStatus, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "OpenList 状态锁已损坏".to_owned())?;
        if Self::refresh_child_locked(&mut state) {
            state.message = "OpenList 已在运行".into();
            return Ok(self.status_locked(&state, false, None));
        }
        let Some(binary) = self.binary_path.clone() else {
            state.message =
                "未找到 OpenList 可执行文件；请配置 TTV_OPENLIST_BIN 或放入 resources/openlist"
                    .into();
            return Ok(self.status_locked(&state, false, None));
        };
        let data_dir = self.data_dir.join("openlist");
        std::fs::create_dir_all(&data_dir)
            .map_err(|error| format!("无法创建 OpenList 数据目录：{error}"))?;
        let port = Url::parse(&self.base_url)
            .ok()
            .and_then(|url| url.port())
            .unwrap_or(5244);
        prepare_openlist_config(&data_dir, port)?;
        ensure_default_admin(&binary, &data_dir)?;
        let mut command = Command::new(&binary);
        // OpenList v4 reads the listen port from its config file. The scheme
        // object has no envPrefix mapping in v4.2.5, so prepare the config
        // before spawning and keep the env values for compatible releases.
        command
            .args(["server", "--data", data_dir.to_string_lossy().as_ref()])
            .env("OPENLIST_SCHEME_ADDRESS", "127.0.0.1")
            .env("OPENLIST_SCHEME_HTTP_PORT", port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_child_window(&mut command);
        match command.spawn() {
            Ok(child) => {
                state.child = Some(child);
                state.started_at_ms = Some(now_ms());
                state.message = format!("已启动 OpenList，等待 {}", self.base_url);
            }
            Err(error) => state.message = format!("无法启动 OpenList：{error}"),
        }
        Ok(self.status_locked(&state, false, None))
    }

    pub fn stop(&self) -> Result<OpenListRuntimeStatus, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "OpenList 状态锁已损坏".to_owned())?;
        if let Some(mut child) = state.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        state.started_at_ms = None;
        state.message = "OpenList 已停止".into();
        Ok(self.status_locked(&state, false, None))
    }

    pub fn restart(&self) -> Result<OpenListRuntimeStatus, String> {
        let _ = self.stop()?;
        self.start()
    }

    pub fn status(&self) -> OpenListRuntimeStatus {
        let Ok(mut state) = self.state.lock() else {
            return OpenListRuntimeStatus {
                running: false,
                reachable: false,
                base_url: self.base_url.clone(),
                version: None,
                message: "OpenList 状态锁已损坏".into(),
                binary_path: self
                    .binary_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                started_at_ms: None,
            };
        };
        Self::refresh_child_locked(&mut state);
        self.status_locked(&state, false, None)
    }

    fn status_locked(
        &self,
        state: &ProcessState,
        reachable: bool,
        version: Option<String>,
    ) -> OpenListRuntimeStatus {
        OpenListRuntimeStatus {
            running: state.child.is_some(),
            reachable,
            base_url: self.base_url.clone(),
            version,
            message: state.message.clone(),
            binary_path: self
                .binary_path
                .as_ref()
                .map(|path| path.display().to_string()),
            started_at_ms: state.started_at_ms,
        }
    }

    fn refresh_child_locked(state: &mut ProcessState) -> bool {
        if let Some(child) = state.child.as_mut() {
            match child.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) => {
                    state.child = None;
                    state.started_at_ms = None;
                    state.message = format!("OpenList 已退出：{status}");
                    false
                }
                Err(error) => {
                    state.message = format!("无法检查 OpenList 进程：{error}");
                    false
                }
            }
        } else {
            false
        }
    }

    fn discover_binary() -> Option<PathBuf> {
        let mut candidates = Vec::new();
        let names = if cfg!(windows) {
            ["openlist.exe", "openlist"]
        } else {
            ["openlist", "openlist.exe"]
        };
        if let Ok(current) = std::env::current_dir() {
            for name in names {
                candidates.push(current.join("resources/openlist").join(name));
                candidates.push(current.join("src-tauri/resources/openlist").join(name));
            }
        }
        if let Ok(executable) = std::env::current_exe() {
            if let Some(parent) = executable.parent() {
                for name in names {
                    candidates.push(parent.join("resources/openlist").join(name));
                    candidates.push(parent.join("openlist").join(name));
                }
            }
        }
        candidates.into_iter().find(|path| path.is_file())
    }
}

impl Drop for OpenListRuntime {
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
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}

fn prepare_openlist_config(data_dir: &std::path::Path, port: u16) -> Result<(), String> {
    let config_path = data_dir.join("config.json");
    let mut config = if config_path.is_file() {
        let bytes = std::fs::read(&config_path)
            .map_err(|error| format!("无法读取 OpenList 配置：{error}"))?;
        serde_json::from_slice::<Value>(&bytes)
            .map_err(|error| format!("OpenList 配置格式无效：{error}"))?
    } else {
        json!({})
    };
    let root = config
        .as_object_mut()
        .ok_or_else(|| "OpenList 配置根节点必须是对象".to_owned())?;
    let scheme = root
        .entry("scheme")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "OpenList scheme 配置必须是对象".to_owned())?;
    scheme.insert("address".into(), Value::String("127.0.0.1".into()));
    scheme.insert("http_port".into(), Value::from(port));
    std::fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config)
            .map_err(|error| format!("无法序列化 OpenList 配置：{error}"))?,
    )
    .map_err(|error| format!("无法写入 OpenList 配置：{error}"))
}

/// OpenList creates its first administrator through the CLI. Only initialize
/// a brand-new data directory; an existing database may contain a user-set
/// password and must never be overwritten on subsequent launches.
fn ensure_default_admin(
    binary: &std::path::Path,
    data_dir: &std::path::Path,
) -> Result<(), String> {
    if data_dir.join("data.db").is_file() {
        return Ok(());
    }
    let status = Command::new(binary)
        .args([
            "admin",
            "set",
            "admin",
            "--data",
            data_dir.to_string_lossy().as_ref(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("无法初始化 OpenList 管理员账号：{error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("OpenList 管理员初始化失败：{status}"))
    }
}

#[cfg(windows)]
fn hide_child_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_child_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::{ensure_default_admin, prepare_openlist_config};
    use serde_json::Value;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn listener_config_preserves_existing_values_and_updates_port() {
        let root = std::env::temp_dir().join(format!(
            "ttv-openlist-config-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("config.json"),
            r#"{"jwt_secret":"keep-me","scheme":{"http_port":5244}}"#,
        )
        .unwrap();

        prepare_openlist_config(&root, 5255).unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(root.join("config.json")).unwrap()).unwrap();
        assert_eq!(value["jwt_secret"], "keep-me");
        assert_eq!(value["scheme"]["address"], "127.0.0.1");
        assert_eq!(value["scheme"]["http_port"], 5255);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_admin_initialization_is_skipped_for_existing_database() {
        let root = std::env::temp_dir().join(format!(
            "ttv-openlist-admin-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("data.db"), b"existing").unwrap();
        let missing = root.join("missing-openlist.exe");
        ensure_default_admin(&missing, &root).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
