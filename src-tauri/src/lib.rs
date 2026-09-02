pub mod adult;
pub mod app;
pub mod comic_drama;
pub mod commands;
pub mod config;
pub mod diagnostics;
pub mod error;
pub mod library;
pub mod media;
pub mod metadata;
pub mod openlist;
pub mod playback;
pub mod providers;
pub mod runtime;
pub mod security;
pub mod short_drama;
pub mod short_drama_app;
pub mod storage;

use tracing_subscriber::EnvFilter;

use crate::app::{AppPaths, AppState, StreamHubRuntime};
use crate::config::AppConfig;
use crate::openlist::OpenListRuntime;
use crate::playback::{LibMpvBackend, MpvConfig, PlaybackActor};
use crate::providers::{GuangyaProvider, OAuthProvider, ProviderRouter, StreamHubProvider};
use crate::runtime::{
    discover_resource_dir, prepare_runtime_environment, probe_runtime, RuntimePaths,
};
use crate::storage::Database;

/// Start the Tauri application. The backend modules remain usable in unit tests
/// without constructing a desktop window.
pub fn run() {
    configure_webview2_runtime();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let data_dir = std::env::var_os("TTV_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_data_dir);
    let paths = AppPaths::from_data_dir(data_dir);
    // One-time move of any legacy cwd-relative `.ttv-data` library into the stable
    // data directory, so existing media survives the switch away from cwd storage.
    Database::migrate_legacy_into(&paths.data_dir);
    let config = AppConfig::load(&paths.data_dir).unwrap_or_else(|error| {
        tracing::warn!(error = %error, "configuration could not be loaded; Guangya OAuth remains disabled");
        AppConfig::default()
    });
    let guangya_oauth_missing_fields = config
        .guangya
        .oauth_missing_fields()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let database = Database::open(paths.data_dir.join("ttv.db"))
        .or_else(|_| Database::open_in_memory())
        .expect("failed to initialize TTV database");
    let cleanup_version = database
        .kv_get("maintenance.promotional-filter.version")
        .ok()
        .flatten();
    if cleanup_version.as_deref() != Some("1") {
        match crate::library::cleanup_promotional_media(&database) {
            Ok(report) => {
                let _ = database.kv_set("maintenance.promotional-filter.version", "1");
                if report.hidden > 0 {
                    tracing::info!(
                        scanned = report.scanned,
                        hidden = report.hidden,
                        "promotional media cleanup completed"
                    );
                }
            }
            Err(error) => tracing::warn!(error = %error, "promotional media cleanup failed"),
        }
    }
    let providers = ProviderRouter::new();
    providers
        .register(
            GuangyaProvider::new(config.guangya)
                .expect("configured Guangya HTTP client must build"),
        )
        .expect("failed to register Guangya provider");
    providers
        .register(
            StreamHubProvider::new(config.streamhub.clone())
                .expect("configured StreamHub HTTP client must build"),
        )
        .expect("failed to register StreamHub provider");
    for (provider_id, oauth_config) in config.oauth {
        if let Ok(provider) =
            OAuthProvider::new(Box::leak(provider_id.into_boxed_str()), oauth_config)
        {
            providers
                .register(provider)
                .expect("failed to register OAuth provider");
        }
    }
    let bundled_resources = discover_resource_dir(Some(&paths.resource_dir));
    let mut runtime = if let Some(resource_dir) = bundled_resources.as_deref() {
        let mut diagnostics =
            probe_runtime(RuntimePaths::from_resource_dir(resource_dir.to_owned()));
        diagnostics
            .warnings
            .extend(prepare_runtime_environment(resource_dir));
        diagnostics
    } else {
        probe_runtime(RuntimePaths::from_root(paths.data_dir.clone()))
    };
    let playback = LibMpvBackend::load_from_roots([
        bundled_resources
            .as_ref()
            .map(|path| path.join("mpv"))
            .unwrap_or_default(),
        paths.resource_dir.join("mpv"),
        paths.resource_dir.clone(),
    ])
    .ok()
    .and_then(|backend| PlaybackActor::start(backend, MpvConfig::default()).ok());
    if playback.is_none() {
        runtime.playback_available = false;
        runtime
            .warnings
            .push("libmpv could not be loaded or initialized".into());
    }
    let streamhub = StreamHubRuntime::new(config.streamhub.clone(), paths.data_dir.clone());
    let openlist = OpenListRuntime::new(paths.data_dir.clone());
    if std::env::var("TTV_OPENLIST_AUTO_START")
        .map(|value| value != "0")
        .unwrap_or(true)
    {
        let _ = openlist.start();
    }
    if config.streamhub.auto_start {
        if let Err(error) = streamhub.start() {
            tracing::warn!(error = %error, "configured StreamHub auto-start failed");
        }
    }
    let webview_data_dir = paths.data_dir.join("webview");
    let state = AppState::new(
        paths, database, providers, runtime, playback, streamhub, openlist,
    )
    .with_guangya_oauth_missing_fields(guangya_oauth_missing_fields);

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(commands::invoke_handler())
        .setup(move |app| {
            use tauri::Manager;
            tracing::info!("Initializing Tauri desktop main window...");
            if let Some(window) = app.get_webview_window("main") {
                if let Err(err) = window.show() {
                    tracing::error!(error = %err, "Failed to show main webview window");
                }
                if let Err(err) = window.set_focus() {
                    tracing::warn!(error = %err, "Failed to focus main window");
                }
                tracing::info!("Main webview window displayed successfully");
            } else {
                tracing::warn!(
                    "'main' window not found from configuration; building window programmatically"
                );
                let win_builder =
                    tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
                        .title("LumiPlayer")
                        .inner_size(1360.0, 840.0)
                        .min_inner_size(1080.0, 680.0)
                        .center()
                        .data_directory(webview_data_dir)
                        .decorations(false)
                        .shadow(true)
                        .transparent(true)
                        .maximized(true)
                        .visible(true);
                match win_builder.build() {
                    Ok(win) => {
                        let _ = win.show();
                        let _ = win.set_focus();
                        tracing::info!("Programmatic main window successfully created and shown");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to build main window programmatically");
                    }
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running TTV application");
}

/// Resolve the default, working-directory-independent data directory.
///
/// The previous fallback was `current_dir().join(".ttv-data")`, which tied the
/// database location to wherever the app happened to be launched from. Launching
/// from different terminals (or a packaged install) silently fragmented the
/// library across several `.ttv-data` folders and made media appear to vanish.
/// Anchor on the OS local-data directory instead (Windows: `%LOCALAPPDATA%\com.ttv.player`).
fn default_data_dir() -> std::path::PathBuf {
    if let Some(base) = dirs::data_local_dir() {
        return base.join("com.ttv.player");
    }
    // `data_local_dir` is effectively always available; keep the old behaviour as
    // a last resort rather than failing to start.
    std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(".ttv-data")
}

#[cfg(windows)]
fn configure_webview2_runtime() {
    // 部分系统的 Evergreen 注册项会在更新后短暂失效；使用经过 exe 和架构
    // loader 双重校验的本机 Runtime 目录，避免窗口创建时直接报 0x80070002。
    if let Some(path) = discover_webview2_runtime() {
        if std::env::var_os("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER").is_none() {
            std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", &path);
        }
        tracing::debug!(path = %path.display(), "Detected installed WebView2 runtime");
    } else {
        tracing::warn!("No installed WebView2 runtime was found");
    }
}

#[cfg(not(windows))]
fn configure_webview2_runtime() {}

#[cfg(windows)]
fn discover_webview2_runtime() -> Option<std::path::PathBuf> {
    let mut application_roots = Vec::new();
    for variable in ["ProgramFiles(x86)", "ProgramFiles", "LocalAppData"] {
        if let Some(base) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
            application_roots.push(
                std::path::PathBuf::from(base)
                    .join("Microsoft")
                    .join("EdgeWebView")
                    .join("Application"),
            );
        }
    }

    // A runtime folder is only usable when the host exe AND the architecture
    // loader DLL are both present. During an Evergreen update the secondary
    // root (ProgramFiles) can briefly contain a versioned folder that has the
    // exe but not `EBWebView/<arch>/EmbeddedBrowserWebView.dll`; selecting it
    // fails webview creation with 0x80070002. Validate before ranking.
    let is_usable = |directory: &std::path::Path| -> bool {
        if !directory.join("msedgewebview2.exe").is_file() {
            return false;
        }
        let loader = directory.join("EBWebView");
        loader
            .join("x64")
            .join("EmbeddedBrowserWebView.dll")
            .is_file()
            || loader
                .join("x86")
                .join("EmbeddedBrowserWebView.dll")
                .is_file()
    };

    let mut runtimes = Vec::new();
    for application_root in application_roots {
        let Ok(entries) = std::fs::read_dir(application_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let directory = entry.path();
            if !is_usable(&directory) {
                continue;
            }
            let Some(file_name) = directory.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let version = file_name
                .split('.')
                .map(|part| part.parse::<u64>().unwrap_or(0))
                .collect::<Vec<_>>();
            runtimes.push((version, directory));
        }
    }

    runtimes.sort_by(|left, right| right.0.cmp(&left.0));
    runtimes.into_iter().map(|(_, path)| path).next()
}
