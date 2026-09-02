//! Playback state and actor abstractions.
//!
//! The actor owns the (potentially blocking) mpv backend on one dedicated
//! thread. This module intentionally does not link libmpv; production code
//! can implement [`MpvBackend`] around the dynamically loaded C ABI while
//! tests use [`MockMpvBackend`].

pub mod mpv;
pub use mpv::LibMpvBackend;

use std::collections::BTreeMap;
use std::sync::{
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
    Arc, RwLock,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

const START_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackStatus {
    Idle,
    Loading,
    Playing,
    Paused,
    Buffering,
    Ended,
    Error,
}

impl Default for PlaybackStatus {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MpvTrackInfo {
    /// mpv track id — the value `aid`/`sid` accepts.
    pub id: i64,
    /// `audio` | `sub` | `video`.
    pub kind: String,
    pub lang: Option<String>,
    pub title: Option<String>,
    pub codec: Option<String>,
    pub selected: bool,
    pub default: bool,
    pub forced: bool,
    /// Container stream index (ffprobe `index`), for cross-referencing probe data.
    pub ff_index: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSnapshot {
    pub status: PlaybackStatus,
    pub media_id: Option<String>,
    pub source: Option<String>,
    pub position_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub volume: f64,
    pub speed: f64,
    pub audio_track: Option<i64>,
    pub subtitle_track: Option<i64>,
    /// mpv track-list 拆分后的音轨列表(语言/标题/编码/是否选中),用于音轨(语言)选择菜单。
    #[serde(default)]
    pub audio_tracks: Vec<MpvTrackInfo>,
    /// mpv track-list 拆分后的字幕轨道列表,用于字幕选择菜单。
    #[serde(default)]
    pub subtitle_tracks: Vec<MpvTrackInfo>,
    pub video_quality: Option<String>,
    pub video_preset: Option<String>,
    pub audio_preset: Option<String>,
    pub subtitle_style: Option<String>,
    pub presentation: Option<String>,
    pub interpolation_enabled: bool,
    pub fullscreen: bool,
    pub always_on_top: bool,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub renderer: Option<String>,
    pub decoder: Option<String>,
    pub buffered_percent: Option<f64>,
    /// mpv 滤镜链输出帧率（estimated-vf-fps）。补帧开启后应接近目标帧率。
    pub actual_fps: Option<f64>,
    /// 容器/解复用标称帧率（container-fps），补帧前的片源帧率。
    #[serde(default)]
    pub source_fps: Option<f64>,
    /// mpv 当前视频输出所匹配的显示器刷新率（display-fps）。
    pub display_fps: Option<f64>,
    /// mpv 报告的累计渲染丢帧数（frame-drop-count），用于补帧性能熔断与前端展示。
    pub dropped_frames: Option<i64>,
    /// 当前视频像素宽（mpv dwidth），用于进度条预览按竖/横屏适配。
    #[serde(default)]
    pub video_width: Option<i64>,
    /// 当前视频像素高（mpv dheight）。
    #[serde(default)]
    pub video_height: Option<i64>,
    /// 首帧是否已真正渲染（mpv playback-restart 之后）。前端用它门控"画布变透明"，
    /// 避免首帧绘出前就把 WebView 清空造成加载期间透视窗口。
    pub first_frame_ready: bool,
    pub playback_backend: Option<String>,
    pub degradation_reason: Option<String>,
    pub interpolation_status: Option<String>,
    pub playlist_index: usize,
    pub playlist_length: usize,
    pub error: Option<String>,
    pub updated_at_ms: u64,
    /// 补帧丢帧熔断的内部窗口起点（毫秒），不参与序列化。
    #[serde(skip)]
    pub drop_window_start_ms: Option<u64>,
    /// 补帧丢帧熔断窗口起点的累计丢帧数，不参与序列化。
    #[serde(skip)]
    pub drop_window_start_count: Option<i64>,
    /// 首帧检测的位置基准（秒）：用来判断 time-pos 是否真实前进，不参与序列化。
    #[serde(skip)]
    pub last_observed_position: Option<f64>,
}

impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Idle,
            media_id: None,
            source: None,
            position_seconds: 0.0,
            duration_seconds: None,
            volume: 100.0,
            speed: 1.0,
            audio_track: None,
            subtitle_track: None,
            audio_tracks: Vec::new(),
            subtitle_tracks: Vec::new(),
            video_quality: None,
            video_preset: None,
            audio_preset: None,
            subtitle_style: None,
            presentation: None,
            interpolation_enabled: false,
            fullscreen: false,
            always_on_top: false,
            video_codec: None,
            audio_codec: None,
            renderer: None,
            decoder: None,
            buffered_percent: None,
            actual_fps: None,
            source_fps: None,
            display_fps: None,
            dropped_frames: None,
            video_width: None,
            video_height: None,
            first_frame_ready: false,
            playback_backend: None,
            degradation_reason: None,
            interpolation_status: None,
            playlist_index: 0,
            playlist_length: 0,
            error: None,
            updated_at_ms: now_ms(),
            drop_window_start_ms: None,
            drop_window_start_count: None,
            last_observed_position: None,
        }
    }
}

impl PlaybackSnapshot {
    fn touch(&mut self) {
        self.updated_at_ms = now_ms();
    }
    fn fail(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.status = PlaybackStatus::Error;
        self.degradation_reason = Some(classify_playback_failure(&message));
        self.error = Some(message);
        self.touch();
    }
}

fn classify_playback_failure(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("http")
        || lower.contains("network")
        || lower.contains("connection")
        || lower.contains("timeout")
    {
        "network".into()
    } else if lower.contains("permission")
        || lower.contains("access")
        || lower.contains("401")
        || lower.contains("403")
    {
        "permission".into()
    } else if lower.contains("track") || lower.contains("audio") || lower.contains("subtitle") {
        "track".into()
    } else if lower.contains("render") || lower.contains("gpu") || lower.contains("vo-") {
        "renderer".into()
    } else {
        "format".into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// 枚举上的 rename_all 只作用于变体名；带字段的变体（Seek/SetVolume 等）
// 需要 rename_all_fields 才能匹配前端发送的 camelCase 键名。
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum PlaybackCommand {
    Load {
        media_id: Option<String>,
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        /// CENC 内容密钥（hex，红果锁定集直链流播用）。
        #[serde(default)]
        decryption_key: Option<String>,
        #[serde(default)]
        resume_position_seconds: Option<f64>,
        #[serde(default)]
        audio_track: Option<i64>,
        #[serde(default)]
        subtitle_track: Option<i64>,
    },
    Play,
    Pause,
    TogglePause,
    Stop,
    Unload,
    Seek {
        position_seconds: f64,
    },
    SetVolume {
        volume: f64,
    },
    SetSpeed {
        speed: f64,
    },
    SetAudioTrack {
        track_id: Option<i64>,
    },
    SetSubtitleTrack {
        track_id: Option<i64>,
    },
    SetVideoFilters {
        shaders: Vec<String>,
    },
    SetVideoFilter {
        filter: Option<String>,
    },
    SetVideoFilterWithFallback {
        primary: Option<String>,
        fallback: Option<String>,
    },
    SetVideoQuality {
        quality: String,
    },
    SetVideoPreset {
        preset: String,
    },
    SetAudioPreset {
        preset: String,
    },
    SetSubtitleStyle {
        style: String,
    },
    SetPresentation {
        presentation: String,
    },
    SetFrameInterpolation {
        enabled: bool,
        mode: String,
    },
    ToggleFullscreen,
    SetAlwaysOnTop {
        enabled: bool,
    },
    Screenshot {
        path: String,
    },
    PlaylistAppend {
        url: String,
        headers: BTreeMap<String, String>,
    },
    PlaylistClear,
    PlaylistIndex {
        index: usize,
    },
    RawCommand {
        args: Vec<String>,
    },
}

impl PlaybackCommand {
    pub fn validate(&self) -> Result<(), AppError> {
        match self {
            Self::Load { url, .. } if url.trim().is_empty() => Err(AppError::InvalidInput(
                "playback URL cannot be empty".into(),
            )),
            Self::Seek { position_seconds }
                if !position_seconds.is_finite() || position_seconds.is_sign_negative() =>
            {
                Err(AppError::InvalidInput(
                    "seek position must be finite".into(),
                ))
            }
            Self::SetVolume { volume }
                if !volume.is_finite() || !(0.0..=100.0).contains(volume) =>
            {
                Err(AppError::InvalidInput(
                    "volume must be between 0 and 100".into(),
                ))
            }
            Self::SetSpeed { speed } if !speed.is_finite() || !(0.25..=4.0).contains(speed) => Err(
                AppError::InvalidInput("speed must be between 0.25 and 4".into()),
            ),
            Self::RawCommand { args }
                if args.is_empty()
                    || args.len() > 16
                    || args.iter().any(|arg| arg.contains('\0')) =>
            {
                Err(AppError::InvalidInput("raw mpv command is invalid".into()))
            }
            Self::Screenshot { path } if path.trim().is_empty() || path.contains('\0') => {
                Err(AppError::InvalidInput("screenshot path is invalid".into()))
            }
            Self::PlaylistIndex { index } if *index > 100_000 => {
                Err(AppError::InvalidInput("playlist index is too large".into()))
            }
            _ => Ok(()),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Load { .. } => "load",
            Self::Play => "play",
            Self::Pause => "pause",
            Self::TogglePause => "togglePause",
            Self::Stop => "stop",
            Self::Unload => "unload",
            Self::Seek { .. } => "seek",
            Self::SetVolume { .. } => "setVolume",
            Self::SetSpeed { .. } => "setSpeed",
            Self::SetAudioTrack { .. } => "setAudioTrack",
            Self::SetSubtitleTrack { .. } => "setSubtitleTrack",
            Self::SetVideoFilters { .. } => "setVideoFilters",
            Self::SetVideoFilter { .. } => "setVideoFilter",
            Self::SetVideoFilterWithFallback { .. } => "setVideoFilterWithFallback",
            Self::SetVideoQuality { .. } => "setVideoQuality",
            Self::SetVideoPreset { .. } => "setVideoPreset",
            Self::SetAudioPreset { .. } => "setAudioPreset",
            Self::SetSubtitleStyle { .. } => "setSubtitleStyle",
            Self::SetPresentation { .. } => "setPresentation",
            Self::SetFrameInterpolation { .. } => "setFrameInterpolation",
            Self::ToggleFullscreen => "toggleFullscreen",
            Self::SetAlwaysOnTop { .. } => "setAlwaysOnTop",
            Self::Screenshot { .. } => "screenshot",
            Self::PlaylistAppend { .. } => "playlistAppend",
            Self::PlaylistClear => "playlistClear",
            Self::PlaylistIndex { .. } => "playlistIndex",
            Self::RawCommand { .. } => "rawCommand",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpvConfig {
    pub options: BTreeMap<String, String>,
    pub window_id: Option<u64>,
}

impl Default for MpvConfig {
    fn default() -> Self {
        let mut options = BTreeMap::new();
        for (key, value) in [
            ("terminal", "no"),
            ("msg-level", "all=warn"),
            ("keep-open", "yes"),
            ("idle", "yes"),
            ("load-scripts", "no"),
            // The current Tauri shell does not pass a native video surface.
            // Keep libmpv headless so it cannot open a separate "No file - mpv" window.
            ("force-window", "no"),
            ("vo", "null"),
            ("gpu-api", "d3d11"),
            ("d3d11-output-format", "rgba16f"),
            ("fbo-format", "rgba16hf"),
            ("target-colorspace-hint", "yes"),
            ("target-colorspace-hint-mode", "target"),
            ("tone-mapping", "spline"),
            ("hdr-compute-peak", "yes"),
            // LumiPlayer calibration: peak detection percentile + gamut mapping
            // keep highlights stable across HDR10 / Dolby ViSion masters.
            ("hdr-peak-percentile", "99.995"),
            ("gamut-mapping-mode", "auto"),
            ("hdr-contrast-recovery", "0.30"),
            ("hdr-contrast-smoothness", "3.5"),
            ("dither-depth", "auto"),
            ("dither", "fruit"),
            ("temporal-dither", "yes"),
            // LumiPlayer scaling baseline: lanczos upscaling + mitchell
            // downscaling with anti-ringing keeps posters/text crisp.
            ("scale", "ewa_lanczos"),
            ("cscale", "ewa_lanczos"),
            ("dscale", "mitchell"),
            ("correct-downscaling", "yes"),
            ("sigmoid-upscaling", "yes"),
            ("scale-antiring", "0.7"),
            ("cache", "yes"),
            ("hwdec", "auto-safe"),
            ("cache-on-disk", "yes"),
            ("network-timeout", "15"),
            // 短剧单集很短、切集频繁：更大的读缓冲优先吸收网络抖动，
            // 但仍关闭首帧强等待，保证点开后立即开始解码。
            ("stream-buffer-size", "4MiB"),
            ("cache-secs", "45"),
            ("demuxer-readahead-secs", "35"),
            ("demuxer-max-bytes", "192MiB"),
            ("demuxer-max-back-bytes", "64MiB"),
            ("cache-pause-wait", "0.35"),
            // Begin decoding as soon as the first playable packets arrive;
            // the normal cache continues filling behind playback.
            ("cache-pause-initial", "no"),
            (
                "stream-lavf-o",
                "reconnect=1,reconnect_streamed=1,reconnect_delay_max=2",
            ),
            // Motion smoothing baseline (LumiPlayer quality-mode design): the
            // interpolation flag toggles this to display-resample at runtime.
            ("video-sync", "audio"),
            ("tscale", "oversample"),
            // Chinese-subtitle defaults borrowed from LumiPlayer's mpv.conf.
            ("slang", "chi,zh-CN,zh-Hans,zh,chs"),
            ("alang", "chi,zh-CN,zh-Hans,zh,chs"),
            ("sub-auto", "fuzzy"),
            ("sub-ass-override", "force"),
            ("sub-font-provider", "auto"),
            ("sub-font-size", "42"),
            ("sub-bold", "yes"),
            ("sub-border-size", "3.5"),
            ("sub-shadow-offset", "1.2"),
            ("sub-shadow-color", "#A0000000"),
            ("volume", "100"),
            ("volume-max", "150"),
            ("audio-channels", "auto-safe"),
            ("audio-stream-silence", "yes"),
            ("audio-fallback-to-null", "yes"),
        ] {
            options.insert(key.into(), value.into());
        }
        Self {
            options,
            window_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MpvCommand {
    LoadFile {
        url: String,
        headers: BTreeMap<String, String>,
        /// CENC 内容密钥（hex）。有值时经 demuxer-lavf-o 注入 mov demuxer，
        /// 直接流播加密 mp4；普通文件必须传 None 清掉残留选项。
        decryption_key: Option<String>,
    },
    SetProperty {
        name: &'static str,
        value: String,
    },
    Seek {
        position_seconds: f64,
    },
    Stop,
    Unload,
    SetShaders(Vec<String>),
    SetVideoFilter(Option<String>),
    Command(Vec<String>),
}

/// Boundary for a dynamically loaded libmpv implementation.
pub trait MpvBackend: Send + 'static {
    fn initialize(&mut self, config: &MpvConfig) -> Result<(), String>;
    fn command(&mut self, command: MpvCommand) -> Result<(), String>;
    fn terminate(&mut self);
    fn poll_events(&mut self) -> Vec<PlaybackEvent> {
        Vec::new()
    }
}

#[derive(Debug, Default)]
pub struct MockMpvBackend {
    pub initialized: bool,
    pub commands: Vec<MpvCommand>,
}

impl MpvBackend for MockMpvBackend {
    fn initialize(&mut self, _config: &MpvConfig) -> Result<(), String> {
        self.initialized = true;
        Ok(())
    }
    fn command(&mut self, command: MpvCommand) -> Result<(), String> {
        self.commands.push(command);
        Ok(())
    }
    fn terminate(&mut self) {
        self.initialized = false;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PlaybackEvent {
    FileLoaded {
        duration_seconds: Option<f64>,
    },
    DurationChanged {
        duration_seconds: f64,
    },
    TimePosition {
        position_seconds: f64,
    },
    PauseChanged {
        paused: bool,
    },
    Buffering {
        active: bool,
        percent: Option<f64>,
    },
    Ended,
    /// mpv playback-restart：解码/滤镜链初始化完成、首帧真正开始输出。
    PlaybackRestarted,
    Failed {
        message: String,
    },
    EnhancementFailed {
        message: String,
    },
    RuntimeInfo {
        video_codec: Option<String>,
        audio_codec: Option<String>,
        decoder: Option<String>,
        renderer: Option<String>,
        actual_fps: Option<f64>,
        source_fps: Option<f64>,
        display_fps: Option<f64>,
        dropped_frames: Option<i64>,
        video_width: Option<i64>,
        video_height: Option<i64>,
    },
    TracksChanged {
        audio_track: Option<i64>,
        subtitle_track: Option<i64>,
    },
    /// mpv track-list 变化(新文件加载、sub-add 外挂字幕等)时携带完整轨道表。
    TrackListChanged {
        tracks: Vec<MpvTrackInfo>,
    },
}

#[derive(Debug)]
enum ActorMessage {
    Command(PlaybackCommand),
    Event(PlaybackEvent),
    Shutdown,
}

/// Handle to a single dedicated playback thread.
#[derive(Debug)]
pub struct PlaybackActor {
    sender: Sender<ActorMessage>,
    state: Arc<RwLock<PlaybackSnapshot>>,
    join: Option<JoinHandle<()>>,
}

impl PlaybackActor {
    pub fn start<B: MpvBackend>(mut backend: B, config: MpvConfig) -> Result<Self, AppError> {
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let state = Arc::new(RwLock::new(PlaybackSnapshot::default()));
        let state_for_thread = Arc::clone(&state);
        let join = thread::Builder::new()
            .name("ttv-playback".into())
            .spawn(move || {
                if let Err(message) = backend.initialize(&config) {
                    if let Ok(mut snapshot) = state_for_thread.write() {
                        snapshot.fail(message.clone());
                    }
                    let _ = ready_sender.send(Err(message));
                    return;
                }
                let _ = ready_sender.send(Ok(()));
                actor_loop(&mut backend, receiver, state_for_thread);
            })
            .map_err(|error| {
                AppError::Playback(format!("failed to start playback thread: {error}"))
            })?;
        match ready_receiver.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                sender,
                state,
                join: Some(join),
            }),
            Ok(Err(message)) => {
                let _ = join.join();
                Err(AppError::Playback(message))
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ = join.join();
                Err(AppError::Playback(
                    "timed out initializing playback engine".into(),
                ))
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = join.join();
                Err(AppError::Playback(
                    "playback engine exited during initialization".into(),
                ))
            }
        }
    }

    pub fn dispatch(&self, command: PlaybackCommand) -> Result<(), AppError> {
        command.validate()?;
        self.sender
            .send(ActorMessage::Command(command))
            .map_err(|_| AppError::Playback("playback actor is not running".into()))
    }
    pub fn publish_event(&self, event: PlaybackEvent) -> Result<(), AppError> {
        self.sender
            .send(ActorMessage::Event(event))
            .map_err(|_| AppError::Playback("playback actor is not running".into()))
    }
    pub fn snapshot(&self) -> PlaybackSnapshot {
        self.state
            .read()
            .map(|state| state.clone())
            .unwrap_or_default()
    }
    pub fn set_interpolation_status(&self, enabled: bool, status: impl Into<String>) {
        if let Ok(mut snapshot) = self.state.write() {
            snapshot.interpolation_enabled = enabled;
            snapshot.interpolation_status = Some(status.into());
            if enabled {
                // 补帧开启时重置丢帧窗口,熔断宽限期从生效时刻重新计时
                snapshot.drop_window_start_ms = None;
                snapshot.drop_window_start_count = None;
            }
            snapshot.touch();
        }
    }
    pub fn set_playback_backend(&self, backend: impl Into<String>) {
        if let Ok(mut snapshot) = self.state.write() {
            snapshot.playback_backend = Some(backend.into());
            snapshot.touch();
        }
    }
    pub fn shutdown(mut self) {
        let _ = self.sender.send(ActorMessage::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    /// Ask the playback thread to exit without joining it.
    ///
    /// `mpv_terminate_destroy` can block for seconds while a remote load is
    /// stuck. Joining that work on the Tauri command thread freezes IPC and
    /// leaves the UI on the compatibility-fallback overlay. Callers that are
    /// replacing or closing a live native actor must detach instead of drop.
    pub fn detach(mut self) {
        let _ = self.sender.send(ActorMessage::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = thread::Builder::new()
                .name("ttv-playback-shutdown".into())
                .spawn(move || {
                    let _ = join.join();
                });
        }
    }
}

impl Drop for PlaybackActor {
    fn drop(&mut self) {
        let _ = self.sender.send(ActorMessage::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn actor_loop<B: MpvBackend>(
    backend: &mut B,
    receiver: Receiver<ActorMessage>,
    state: Arc<RwLock<PlaybackSnapshot>>,
) {
    let mut pending_load: Option<(Option<f64>, Option<i64>, Option<i64>)> = None;
    loop {
        let message = match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => {
                for event in backend.poll_events() {
                    process_backend_event(backend, &state, event);
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match message {
            ActorMessage::Command(command) => {
                process_command(backend, &state, command, &mut pending_load)
            }
            ActorMessage::Event(event) => {
                if matches!(&event, PlaybackEvent::FileLoaded { .. }) {
                    if let Some((resume, audio_track, subtitle_track)) = pending_load.take() {
                        if let Some(position_seconds) = resume {
                            if let Err(message) =
                                backend.command(MpvCommand::Seek { position_seconds })
                            {
                                if let Ok(mut snapshot) = state.write() {
                                    snapshot.fail(message);
                                }
                                continue;
                            }
                            if let Ok(mut snapshot) = state.write() {
                                snapshot.position_seconds = position_seconds;
                            }
                        }
                        if let Some(track_id) = audio_track {
                            if let Err(message) = backend.command(MpvCommand::SetProperty {
                                name: "aid",
                                value: track_id.to_string(),
                            }) {
                                if let Ok(mut snapshot) = state.write() {
                                    snapshot.fail(message);
                                }
                            }
                        }
                        if let Some(track_id) = subtitle_track {
                            if let Err(message) = backend.command(MpvCommand::SetProperty {
                                name: "sid",
                                value: track_id.to_string(),
                            }) {
                                if let Ok(mut snapshot) = state.write() {
                                    snapshot.fail(message);
                                }
                            }
                        }
                    }
                }
                process_backend_event(backend, &state, event)
            }
            ActorMessage::Shutdown => break,
        }
    }
    backend.terminate();
}

fn process_backend_event<B: MpvBackend>(
    backend: &mut B,
    state: &Arc<RwLock<PlaybackSnapshot>>,
    event: PlaybackEvent,
) {
    if matches!(event, PlaybackEvent::EnhancementFailed { .. }) {
        let _ = backend.command(MpvCommand::SetVideoFilter(None));
        let _ = backend.command(MpvCommand::SetShaders(Vec::new()));
    }
    apply_event(state, event);
    check_interpolation_circuit_breaker(backend, state);
}

/// Live 补帧滤镜：ffmpeg minterpolate，把滤镜链输出拉到目标帧率。
///
/// 不走 VapourSynth/RIFE：那条路径会在播放线程里加载 ONNX，可能把当前帧卡死。
/// 主路径用运动补偿（mci），失败时回退到帧混合（blend）。两者都会提高
/// `estimated-vf-fps`，帧率面板上的「实际帧率」能直接反映补帧是否生效。
pub fn live_interpolation_filters(target_fps: u32) -> (String, String) {
    let fps = target_fps.clamp(48, 60);
    (
        format!(
            "@ttv-interp:lavfi=[minterpolate=fps={fps}:mi_mode=mci:mc_mode=obmc:me_mode=bidir:vsbmc=0]"
        ),
        format!("@ttv-interp:lavfi=[minterpolate=fps={fps}:mi_mode=blend]"),
    )
}

fn audio_preset_filter(preset: &str) -> String {
    match preset.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "disable" | "原声" => String::new(),
        "movie" | "电影" => {
            "lavfi=[acompressor=threshold=-18dB:ratio=3:attack=20:release=250,loudnorm=I=-16:TP=-1.5:LRA=11]"
                .into()
        }
        "music" | "音乐" => {
            "lavfi=[equalizer=f=80:t=q:w=1:g=2,equalizer=f=10000:t=q:w=1:g=2]".into()
        }
        "night" | "夜听" => "lavfi=[dynaudnorm=f=150:g=15,acompressor=threshold=-20dB:ratio=4]".into(),
        "voice" | "人声" => {
            "lavfi=[highpass=f=120,lowpass=f=8000,equalizer=f=2500:t=q:w=1:g=4]".into()
        }
        "surround" | "环绕" => "lavfi=[extrastereo=m=2.5]".into(),
        other => other.to_owned(),
    }
}

fn process_command<B: MpvBackend>(
    backend: &mut B,
    state: &Arc<RwLock<PlaybackSnapshot>>,
    command: PlaybackCommand,
    pending_load: &mut Option<(Option<f64>, Option<i64>, Option<i64>)>,
) {
    let result = match &command {
        PlaybackCommand::Load {
            url,
            headers,
            media_id,
            decryption_key,
            resume_position_seconds,
            audio_track,
            subtitle_track,
        } => backend
            .command(MpvCommand::LoadFile {
                url: url.clone(),
                headers: headers.clone(),
                decryption_key: decryption_key.clone(),
            })
            .and_then(|_| {
                backend.command(MpvCommand::SetProperty {
                    name: "pause",
                    value: "no".into(),
                })
            })
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.status = PlaybackStatus::Loading;
                    snapshot.media_id = media_id.clone();
                    snapshot.source = Some(url.clone());
                    snapshot.position_seconds = 0.0;
                    snapshot.duration_seconds = None;
                    snapshot.first_frame_ready = false;
                    snapshot.last_observed_position = None;
                    snapshot.error = None;
                    snapshot.touch();
                }
                *pending_load = Some((
                    resume_position_seconds.filter(|value| value.is_finite() && *value >= 0.0),
                    *audio_track,
                    *subtitle_track,
                ));
            }),
        PlaybackCommand::Play => backend
            .command(MpvCommand::SetProperty {
                name: "pause",
                value: "no".into(),
            })
            .map(|_| set_status(state, PlaybackStatus::Playing)),
        PlaybackCommand::Pause => backend
            .command(MpvCommand::SetProperty {
                name: "pause",
                value: "yes".into(),
            })
            .map(|_| set_status(state, PlaybackStatus::Paused)),
        PlaybackCommand::TogglePause => {
            let paused = state
                .read()
                .map(|s| s.status == PlaybackStatus::Paused)
                .unwrap_or(false);
            let value = if paused { "no" } else { "yes" };
            backend
                .command(MpvCommand::SetProperty {
                    name: "pause",
                    value: value.into(),
                })
                .map(|_| {
                    set_status(
                        state,
                        if paused {
                            PlaybackStatus::Playing
                        } else {
                            PlaybackStatus::Paused
                        },
                    )
                })
        }
        PlaybackCommand::Stop => backend
            .command(MpvCommand::Stop)
            .map(|_| set_status(state, PlaybackStatus::Idle)),
        PlaybackCommand::Unload => backend.command(MpvCommand::Unload).map(|_| {
            if let Ok(mut snapshot) = state.write() {
                *snapshot = PlaybackSnapshot::default();
            }
        }),
        PlaybackCommand::Seek { position_seconds } => backend
            .command(MpvCommand::Seek {
                position_seconds: *position_seconds,
            })
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.position_seconds = *position_seconds;
                    snapshot.touch();
                }
            }),
        PlaybackCommand::SetVolume { volume } => backend
            .command(MpvCommand::SetProperty {
                name: "volume",
                value: volume.to_string(),
            })
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.volume = *volume;
                    snapshot.touch();
                }
            }),
        PlaybackCommand::SetSpeed { speed } => backend
            .command(MpvCommand::SetProperty {
                name: "speed",
                value: speed.to_string(),
            })
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.speed = *speed;
                    snapshot.touch();
                }
            }),
        PlaybackCommand::SetAudioTrack { track_id } => backend
            .command(MpvCommand::SetProperty {
                name: "aid",
                value: track_id.map_or_else(|| "no".into(), |id| id.to_string()),
            })
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.audio_track = *track_id;
                    snapshot.touch();
                }
            }),
        PlaybackCommand::SetSubtitleTrack { track_id } => backend
            .command(MpvCommand::SetProperty {
                name: "sid",
                value: track_id.map_or_else(|| "no".into(), |id| id.to_string()),
            })
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.subtitle_track = *track_id;
                    snapshot.touch();
                }
            }),
        PlaybackCommand::SetVideoFilters { shaders } => {
            backend.command(MpvCommand::SetShaders(shaders.clone()))
        }
        PlaybackCommand::SetVideoFilter { filter } => {
            backend.command(MpvCommand::SetVideoFilter(filter.clone()))
        }
        PlaybackCommand::SetVideoFilterWithFallback { primary, fallback } => {
            match backend.command(MpvCommand::SetVideoFilter(primary.clone())) {
                Ok(()) => {
                    if let Ok(mut snapshot) = state.write() {
                        snapshot.interpolation_enabled = true;
                        snapshot.interpolation_status = Some("minterpolate".into());
                        snapshot.degradation_reason = None;
                        snapshot.drop_window_start_ms = None;
                        snapshot.drop_window_start_count = None;
                        snapshot.touch();
                    }
                    Ok(())
                }
                Err(_primary_error) => {
                    let fallback_result = if fallback.is_some() {
                        backend.command(MpvCommand::SetVideoFilter(fallback.clone()))
                    } else {
                        Err("no interpolation fallback filter".into())
                    };
                    match fallback_result {
                    Ok(()) => {
                        if let Ok(mut snapshot) = state.write() {
                            snapshot.interpolation_enabled = true;
                            snapshot.interpolation_status = Some("minterpolate".into());
                            snapshot.degradation_reason = None;
                            snapshot.touch();
                        }
                        Ok(())
                    }
                    Err(fallback_error) => {
                        let _ = backend.command(MpvCommand::SetVideoFilter(None));
                        let _ = backend.command(MpvCommand::SetProperty {
                            name: "video-sync",
                            value: "display-resample".into(),
                        });
                        let _ = backend.command(MpvCommand::SetProperty {
                            name: "interpolation",
                            value: "yes".into(),
                        });
                        let _ = backend.command(MpvCommand::SetProperty {
                            name: "tscale",
                            value: "mitchell".into(),
                        });
                        if let Ok(mut snapshot) = state.write() {
                            snapshot.interpolation_enabled = true;
                            snapshot.interpolation_status = Some("display-resample".into());
                            snapshot.degradation_reason = Some(format!(
                                "神经网络补帧不可用，已改用内置补帧 ({fallback_error})"
                            ));
                            snapshot.touch();
                        }
                        Ok(())
                    }
                    }
                }
            }
        }
        PlaybackCommand::SetVideoQuality { quality } => {
            // 画质切换的真实机制是前端按新画质重新解析播放地址并重开播放器，
            // mpv 侧没有可设置的对应属性（此前误接到 video-zoom，quality 标签并非
            // 数值 zoom，会导致无效设置，已移除）。这里仅记录画质标签供状态展示。
            set_string_state(state, |s| s.video_quality = Some(quality.clone()));
            Ok(())
        }
        PlaybackCommand::SetVideoPreset { preset } => backend
            .command(MpvCommand::SetProperty {
                name: "profile",
                value: preset.clone(),
            })
            .map(|_| set_string_state(state, |s| s.video_preset = Some(preset.clone()))),
        PlaybackCommand::SetAudioPreset { preset } => {
            let filter = audio_preset_filter(preset);
            backend
                .command(MpvCommand::SetProperty {
                    name: "af",
                    value: filter,
                })
                .map(|_| set_string_state(state, |s| s.audio_preset = Some(preset.clone())))
        }
        PlaybackCommand::SetSubtitleStyle { style } => backend
            .command(MpvCommand::SetProperty {
                name: "sub-font",
                value: style.clone(),
            })
            .map(|_| set_string_state(state, |s| s.subtitle_style = Some(style.clone()))),
        PlaybackCommand::SetPresentation { presentation } => backend
            .command(MpvCommand::SetProperty {
                name: "video-sync",
                value: presentation.clone(),
            })
            .map(|_| set_string_state(state, |s| s.presentation = Some(presentation.clone()))),
        PlaybackCommand::SetFrameInterpolation { enabled, mode } => {
            // Two live interpolation paths:
            // - `rife` / `vapoursynth`: VapourSynth generates frames; mpv only
            //   resamples presentation to the display refresh.
            // - `display-resample`: mpv's own temporal interpolator (`tscale`).
            //   `tscale=oversample` only duplicates/drops frames and looks like
            //   补帧 never started, so the builtin path uses `mitchell`.
            let mode = mode.trim().to_ascii_lowercase();
            let neural = *enabled && matches!(mode.as_str(), "rife" | "vapoursynth");
            let filter_interp = *enabled && matches!(mode.as_str(), "minterpolate" | "lavfi");
            let video_sync = if *enabled { "display-resample" } else { "audio" };
            // Filter-based interpolation already emits extra frames. mpv's
            // tscale interpolator is only used when no vf is attached.
            let use_mpv_interp = *enabled && !neural && !filter_interp;
            let tscale = if use_mpv_interp { "mitchell" } else { "oversample" };
            let status = if !*enabled {
                "off"
            } else if neural {
                "rife"
            } else if filter_interp {
                "minterpolate"
            } else {
                "display-resample"
            };
            backend
                .command(MpvCommand::SetProperty {
                    name: "video-sync",
                    value: video_sync.into(),
                })
                .and_then(|_| {
                    backend.command(MpvCommand::SetProperty {
                        name: "interpolation",
                        value: if use_mpv_interp { "yes" } else { "no" }.into(),
                    })
                })
                .and_then(|_| {
                    backend.command(MpvCommand::SetProperty {
                        name: "tscale",
                        value: tscale.into(),
                    })
                })
                .map(|_| {
                    if let Ok(mut snapshot) = state.write() {
                        snapshot.interpolation_enabled = *enabled;
                        snapshot.interpolation_status = Some(status.into());
                        snapshot.degradation_reason = None;
                        snapshot.drop_window_start_ms = None;
                        snapshot.drop_window_start_count = None;
                        snapshot.touch();
                    }
                })
        }
        PlaybackCommand::ToggleFullscreen => backend
            .command(MpvCommand::Command(vec![
                "cycle".into(),
                "fullscreen".into(),
            ]))
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.fullscreen = !snapshot.fullscreen;
                    snapshot.touch();
                }
            }),
        PlaybackCommand::SetAlwaysOnTop { enabled } => backend
            .command(MpvCommand::SetProperty {
                name: "ontop",
                value: if *enabled { "yes" } else { "no" }.into(),
            })
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.always_on_top = *enabled;
                    snapshot.touch();
                }
            }),
        PlaybackCommand::Screenshot { path } => backend.command(MpvCommand::Command(vec![
            "screenshot-to-file".into(),
            path.clone(),
            "video".into(),
        ])),
        PlaybackCommand::PlaylistAppend { url, headers } => backend
            .command(MpvCommand::LoadFile {
                url: url.clone(),
                headers: headers.clone(),
                decryption_key: None,
            })
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.playlist_length += 1;
                    snapshot.touch();
                }
            }),
        PlaybackCommand::PlaylistClear => backend
            .command(MpvCommand::Command(vec!["playlist-clear".into()]))
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.playlist_length = 0;
                    snapshot.playlist_index = 0;
                    snapshot.touch();
                }
            }),
        PlaybackCommand::PlaylistIndex { index } => backend
            .command(MpvCommand::Command(vec![
                "playlist-play-index".into(),
                index.to_string(),
            ]))
            .map(|_| {
                if let Ok(mut snapshot) = state.write() {
                    snapshot.playlist_index = *index;
                    snapshot.touch();
                }
            }),
        PlaybackCommand::RawCommand { args } => backend.command(MpvCommand::Command(args.clone())),
    };
    if let Err(message) = result {
        if let Ok(mut snapshot) = state.write() {
            snapshot.fail(message);
        }
    }
}

fn set_status(state: &Arc<RwLock<PlaybackSnapshot>>, status: PlaybackStatus) {
    if let Ok(mut snapshot) = state.write() {
        snapshot.status = status;
        snapshot.error = None;
        snapshot.touch();
    }
}

fn set_string_state(
    state: &Arc<RwLock<PlaybackSnapshot>>,
    update: impl FnOnce(&mut PlaybackSnapshot),
) {
    if let Ok(mut snapshot) = state.write() {
        update(&mut snapshot);
        snapshot.touch();
    }
}

fn apply_event(state: &Arc<RwLock<PlaybackSnapshot>>, event: PlaybackEvent) {
    if let Ok(mut snapshot) = state.write() {
        match event {
            PlaybackEvent::FileLoaded { duration_seconds } => {
                snapshot.duration_seconds = duration_seconds;
                snapshot.status = PlaybackStatus::Playing;
                // mpv 的 frame-drop-count 随新文件重置,熔断窗口同步清零
                snapshot.dropped_frames = None;
                snapshot.drop_window_start_ms = None;
                snapshot.drop_window_start_count = None;
                // 新文件首帧尚未渲染,透明门控与位置基准复位
                snapshot.first_frame_ready = false;
                snapshot.last_observed_position = None;
            }
            PlaybackEvent::PlaybackRestarted => {
                // mpv emits playback-restart after video output has resumed and
                // a frame is ready for presentation. This also works when the
                // file opens paused at 0, where time-pos never advances.
                snapshot.first_frame_ready = true;
            }
            PlaybackEvent::DurationChanged { duration_seconds }
                if duration_seconds.is_finite() && duration_seconds > 0.0 =>
            {
                snapshot.duration_seconds = Some(duration_seconds);
            }
            PlaybackEvent::TimePosition { position_seconds } if position_seconds.is_finite() => {
                let position = position_seconds.max(0.0);
                snapshot.position_seconds = position;
                // 首帧门控:播放位置真实前进才证明有帧被渲染上屏。比 playback-restart
                // 更准——补帧/超分首次推理的 GPU 预热发生在 restart 之后,只有 time-pos
                // 前进才能确定画面已经透出,此时再点亮透明画布不会透视。
                if !snapshot.first_frame_ready {
                    if let Some(previous) = snapshot.last_observed_position {
                        if position > previous {
                            snapshot.first_frame_ready = true;
                        }
                    }
                    snapshot.last_observed_position = Some(position);
                }
            }
            PlaybackEvent::PauseChanged { paused } => {
                snapshot.status = if paused {
                    PlaybackStatus::Paused
                } else {
                    PlaybackStatus::Playing
                }
            }
            PlaybackEvent::Buffering { active, percent } => {
                snapshot.status = if active {
                    PlaybackStatus::Buffering
                } else {
                    PlaybackStatus::Playing
                };
                snapshot.buffered_percent = percent.filter(|value| value.is_finite());
            }
            PlaybackEvent::Ended => snapshot.status = PlaybackStatus::Ended,
            PlaybackEvent::Failed { message } => snapshot.fail(message),
            PlaybackEvent::EnhancementFailed { message } => {
                snapshot.interpolation_enabled = false;
                snapshot.interpolation_status = Some("fallback".into());
                snapshot.degradation_reason = Some(message);
            }
            PlaybackEvent::RuntimeInfo {
                video_codec,
                audio_codec,
                decoder,
                renderer,
                actual_fps,
                source_fps,
                display_fps,
                dropped_frames,
                video_width,
                video_height,
            } => {
                snapshot.video_codec = video_codec;
                snapshot.audio_codec = audio_codec;
                snapshot.decoder = decoder;
                snapshot.renderer = renderer;
                snapshot.actual_fps = actual_fps.filter(|value| value.is_finite() && *value > 0.0);
                snapshot.source_fps = source_fps.filter(|value| value.is_finite() && *value > 0.0);
                snapshot.display_fps =
                    display_fps.filter(|value| value.is_finite() && *value > 0.0);
                snapshot.dropped_frames = dropped_frames.filter(|value| *value >= 0);
                snapshot.video_width = video_width.filter(|value| *value > 0);
                snapshot.video_height = video_height.filter(|value| *value > 0);
            }
            PlaybackEvent::TracksChanged {
                audio_track,
                subtitle_track,
            } => {
                snapshot.audio_track = audio_track;
                snapshot.subtitle_track = subtitle_track;
            }
            PlaybackEvent::TrackListChanged { tracks } => {
                snapshot.audio_tracks = tracks
                    .iter()
                    .filter(|track| track.kind == "audio")
                    .cloned()
                    .collect();
                snapshot.subtitle_tracks = tracks
                    .iter()
                    .filter(|track| track.kind == "sub")
                    .cloned()
                    .collect();
            }
            PlaybackEvent::TimePosition { .. } => {}
            PlaybackEvent::DurationChanged { .. } => {}
        }
        snapshot.touch();
    }
}

/// 补帧熔断器:丢帧统计滑动窗口长度(毫秒)。
const INTERP_DROP_WINDOW_MS: u64 = 6000;
/// 补帧熔断器:窗口内允许的丢帧速率上限(帧/秒),超过即判定补帧拖垮播放。
const INTERP_DROP_RATE_LIMIT: f64 = 6.0;

/// 补帧熔断器:补帧运行期间若 mpv 丢帧速率持续过高,自动关闭实时补帧并降级为普通播放。
///
/// 仅在内置 RIFE / display-resample 补帧开启且正常播放时评估;
/// 暂停/缓冲期间的丢帧(网络卡顿等)不计入。窗口在文件加载、补帧开启时重置,
/// 因此首次评估天然带有 INTERP_DROP_WINDOW_MS 的宽限期,避免起播瞬间误熔断。
fn check_interpolation_circuit_breaker<B: MpvBackend>(
    backend: &mut B,
    state: &Arc<RwLock<PlaybackSnapshot>>,
) {
    let tripped = {
        let mut snapshot = match state.write() {
            Ok(snapshot) => snapshot,
            Err(_) => return,
        };
        if !snapshot.interpolation_enabled
            || !matches!(
                snapshot.interpolation_status.as_deref(),
                Some("display-resample") | Some("rife") | Some("minterpolate")
            )
            || snapshot.status != PlaybackStatus::Playing
        {
            return;
        }
        let Some(total_drops) = snapshot.dropped_frames else {
            return;
        };
        let now = now_ms();
        let (window_start_ms, window_start_count) = match (
            snapshot.drop_window_start_ms,
            snapshot.drop_window_start_count,
        ) {
            (Some(start_ms), Some(start_count)) => (start_ms, start_count),
            _ => {
                // 首次评估:初始化窗口(宽限期从此刻开始)
                snapshot.drop_window_start_ms = Some(now);
                snapshot.drop_window_start_count = Some(total_drops);
                return;
            }
        };
        let elapsed_ms = now.saturating_sub(window_start_ms);
        if elapsed_ms < INTERP_DROP_WINDOW_MS {
            return;
        }
        let dropped_in_window = (total_drops - window_start_count).max(0);
        let drop_rate = dropped_in_window as f64 / (elapsed_ms as f64 / 1000.0);
        // 无论是否熔断都滑动窗口,避免历史丢帧持续计入后续评估
        snapshot.drop_window_start_ms = Some(now);
        snapshot.drop_window_start_count = Some(total_drops);
        if drop_rate < INTERP_DROP_RATE_LIMIT {
            return;
        }
        snapshot.interpolation_enabled = false;
        snapshot.interpolation_status = Some("fallback".into());
        snapshot.degradation_reason = Some(format!(
            "补帧期间丢帧过多(约 {drop_rate:.1} 帧/秒),已自动关闭补帧"
        ));
        snapshot.drop_window_start_ms = None;
        snapshot.drop_window_start_count = None;
        snapshot.touch();
        true
    };
    if tripped {
        // 关闭内置补帧滤镜与显示重采样，保留 GLSL 超分着色器作为画质兜底。
        let _ = backend.command(MpvCommand::SetVideoFilter(None));
        let _ = backend.command(MpvCommand::SetProperty {
            name: "video-sync",
            value: "audio".into(),
        });
        let _ = backend.command(MpvCommand::SetProperty {
            name: "interpolation",
            value: "no".into(),
        });
        let _ = backend.command(MpvCommand::SetProperty {
            name: "tscale",
            value: "oversample".into(),
        });
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_validation_rejects_invalid_values() {
        assert!(PlaybackCommand::Load {
            media_id: None,
            url: " ".into(),
            headers: BTreeMap::new(),
            decryption_key: None,
            resume_position_seconds: None,
            audio_track: None,
            subtitle_track: None,
        }
        .validate()
        .is_err());
        assert!(PlaybackCommand::SetSpeed { speed: 0.1 }.validate().is_err());
        assert!(PlaybackCommand::SetVolume { volume: 101.0 }
            .validate()
            .is_err());
    }
    #[test]
    fn actor_updates_state_and_accepts_events() {
        let actor = PlaybackActor::start(MockMpvBackend::default(), MpvConfig::default()).unwrap();
        actor
            .dispatch(PlaybackCommand::Load {
                media_id: Some("m1".into()),
                url: "https://example.invalid/video".into(),
                headers: BTreeMap::new(),
                decryption_key: None,
                resume_position_seconds: Some(8.0),
                audio_track: None,
                subtitle_track: None,
            })
            .unwrap();
        actor
            .dispatch(PlaybackCommand::SetSpeed { speed: 1.5 })
            .unwrap();
        actor
            .publish_event(PlaybackEvent::FileLoaded {
                duration_seconds: Some(90.0),
            })
            .unwrap();
        actor
            .publish_event(PlaybackEvent::TimePosition {
                position_seconds: 12.0,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let snapshot = actor.snapshot();
        assert_eq!(snapshot.media_id.as_deref(), Some("m1"));
        assert_eq!(snapshot.status, PlaybackStatus::Playing);
        assert_eq!(snapshot.duration_seconds, Some(90.0));
        assert_eq!(snapshot.position_seconds, 12.0);
        assert_eq!(snapshot.speed, 1.5);
    }
    #[test]
    fn frame_interpolation_uses_display_resample_and_restores_audio_sync() {
        let state = Arc::new(RwLock::new(PlaybackSnapshot::default()));
        let mut backend = MockMpvBackend::default();
        let mut pending_load = None;

        process_command(
            &mut backend,
            &state,
            PlaybackCommand::SetFrameInterpolation {
                enabled: true,
                mode: "display-resample".into(),
            },
            &mut pending_load,
        );
        assert!(matches!(
            backend.commands.as_slice(),
            [
                MpvCommand::SetProperty { name: "video-sync", value },
                MpvCommand::SetProperty { name: "interpolation", value: interpolation },
                MpvCommand::SetProperty { name: "tscale", value: tscale }
            ] if value == "display-resample" && interpolation == "yes" && tscale == "mitchell"
        ));
        assert_eq!(
            state.read().unwrap().interpolation_status.as_deref(),
            Some("display-resample")
        );

        backend.commands.clear();
        process_command(
            &mut backend,
            &state,
            PlaybackCommand::SetFrameInterpolation {
                enabled: false,
                mode: "display-resample".into(),
            },
            &mut pending_load,
        );
        assert!(matches!(
            backend.commands.as_slice(),
            [
                MpvCommand::SetProperty { name: "video-sync", value },
                MpvCommand::SetProperty { name: "interpolation", value: interpolation },
                MpvCommand::SetProperty { name: "tscale", value: tscale }
            ] if value == "audio" && interpolation == "no" && tscale == "oversample"
        ));
        assert!(!state.read().unwrap().interpolation_enabled);
    }
    #[test]
    fn minterpolate_mode_uses_display_resample_without_mpv_tscale() {
        let state = Arc::new(RwLock::new(PlaybackSnapshot::default()));
        let mut backend = MockMpvBackend::default();
        let mut pending_load = None;

        process_command(
            &mut backend,
            &state,
            PlaybackCommand::SetFrameInterpolation {
                enabled: true,
                mode: "minterpolate".into(),
            },
            &mut pending_load,
        );
        assert!(matches!(
            backend.commands.as_slice(),
            [
                MpvCommand::SetProperty { name: "video-sync", value },
                MpvCommand::SetProperty { name: "interpolation", value: interpolation },
                MpvCommand::SetProperty { name: "tscale", value: tscale }
            ] if value == "display-resample" && interpolation == "no" && tscale == "oversample"
        ));
        assert_eq!(
            state.read().unwrap().interpolation_status.as_deref(),
            Some("minterpolate")
        );
        let (primary, fallback) = live_interpolation_filters(60);
        assert!(primary.contains("mi_mode=mci"));
        assert!(fallback.contains("mi_mode=blend"));
    }
    #[test]
    fn neural_interpolation_keeps_display_resample_without_mpv_tscale() {
        let state = Arc::new(RwLock::new(PlaybackSnapshot::default()));
        let mut backend = MockMpvBackend::default();
        let mut pending_load = None;

        process_command(
            &mut backend,
            &state,
            PlaybackCommand::SetFrameInterpolation {
                enabled: true,
                mode: "rife".into(),
            },
            &mut pending_load,
        );
        assert!(matches!(
            backend.commands.as_slice(),
            [
                MpvCommand::SetProperty { name: "video-sync", value },
                MpvCommand::SetProperty { name: "interpolation", value: interpolation },
                MpvCommand::SetProperty { name: "tscale", value: tscale }
            ] if value == "display-resample" && interpolation == "no" && tscale == "oversample"
        ));
        assert_eq!(
            state.read().unwrap().interpolation_status.as_deref(),
            Some("rife")
        );
        assert!(state.read().unwrap().interpolation_enabled);
    }
    #[test]
    fn interpolation_circuit_breaker_restores_audio_sync() {
        let state = Arc::new(RwLock::new(PlaybackSnapshot::default()));
        {
            let mut snapshot = state.write().unwrap();
            snapshot.status = PlaybackStatus::Playing;
            snapshot.interpolation_enabled = true;
            snapshot.interpolation_status = Some("display-resample".into());
            snapshot.dropped_frames = Some(80);
            snapshot.drop_window_start_ms = Some(now_ms().saturating_sub(7_000));
            snapshot.drop_window_start_count = Some(0);
        }
        let mut backend = MockMpvBackend::default();

        check_interpolation_circuit_breaker(&mut backend, &state);

        assert!(matches!(
            backend.commands.as_slice(),
            [
                MpvCommand::SetVideoFilter(None),
                MpvCommand::SetProperty { name: "video-sync", value },
                MpvCommand::SetProperty { name: "interpolation", value: interpolation },
                MpvCommand::SetProperty { name: "tscale", value: tscale }
            ] if value == "audio" && interpolation == "no" && tscale == "oversample"
        ));
        let snapshot = state.read().unwrap();
        assert!(!snapshot.interpolation_enabled);
        assert_eq!(snapshot.interpolation_status.as_deref(), Some("fallback"));
    }
    #[test]
    fn playback_restart_releases_first_frame_gate() {
        let actor = PlaybackActor::start(MockMpvBackend::default(), MpvConfig::default()).unwrap();
        actor
            .publish_event(PlaybackEvent::FileLoaded {
                duration_seconds: Some(60.0),
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        // FILE_LOADED 已把状态置为 Playing，但首帧尚未渲染，透明门控必须保持关闭
        let snapshot = actor.snapshot();
        assert_eq!(snapshot.status, PlaybackStatus::Playing);
        assert!(!snapshot.first_frame_ready);
        // playback-restart means mpv has resumed video output, including when
        // playback opens paused and time-pos cannot advance.
        actor
            .publish_event(PlaybackEvent::PlaybackRestarted)
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(actor.snapshot().first_frame_ready);
        // Later duration discovery must not masquerade as another file load and
        // close the gate again.
        actor
            .publish_event(PlaybackEvent::DurationChanged {
                duration_seconds: 60.0,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(actor.snapshot().first_frame_ready);
        // 加载新文件时门控复位
        actor
            .publish_event(PlaybackEvent::FileLoaded {
                duration_seconds: Some(30.0),
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(!actor.snapshot().first_frame_ready);
    }
    #[test]
    fn default_mpv_config_contains_safe_options() {
        let config = MpvConfig::default();
        assert_eq!(
            config.options.get("load-scripts").map(String::as_str),
            Some("no")
        );
        assert_eq!(
            config.options.get("gpu-api").map(String::as_str),
            Some("d3d11")
        );
        assert_eq!(
            config.options.get("force-window").map(String::as_str),
            Some("no")
        );
        assert_eq!(config.options.get("vo").map(String::as_str), Some("null"));
    }

    struct SlowTerminateBackend {
        delay: Duration,
    }

    impl MpvBackend for SlowTerminateBackend {
        fn initialize(&mut self, _config: &MpvConfig) -> Result<(), String> {
            Ok(())
        }
        fn command(&mut self, _command: MpvCommand) -> Result<(), String> {
            Ok(())
        }
        fn terminate(&mut self) {
            thread::sleep(self.delay);
        }
    }

    #[test]
    fn detach_returns_before_slow_terminate() {
        let actor = PlaybackActor::start(
            SlowTerminateBackend {
                delay: Duration::from_millis(800),
            },
            MpvConfig::default(),
        )
        .unwrap();
        let started = std::time::Instant::now();
        actor.detach();
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "detach joined a blocking mpv terminate"
        );
        thread::sleep(Duration::from_millis(1000));
    }

    #[test]
    fn second_load_reuses_actor_and_resets_first_frame() {
        let actor = PlaybackActor::start(MockMpvBackend::default(), MpvConfig::default()).unwrap();
        actor
            .dispatch(PlaybackCommand::Load {
                media_id: Some("ep1".into()),
                url: "https://example.invalid/ep1.mp4".into(),
                headers: BTreeMap::new(),
                decryption_key: None,
                resume_position_seconds: None,
                audio_track: None,
                subtitle_track: None,
            })
            .unwrap();
        actor
            .publish_event(PlaybackEvent::PlaybackRestarted)
            .unwrap();
        thread::sleep(Duration::from_millis(20));
        assert!(actor.snapshot().first_frame_ready);
        actor
            .dispatch(PlaybackCommand::Load {
                media_id: Some("ep2".into()),
                url: "https://example.invalid/ep2.mp4".into(),
                headers: BTreeMap::new(),
                decryption_key: Some("aabb".into()),
                resume_position_seconds: None,
                audio_track: None,
                subtitle_track: None,
            })
            .unwrap();
        thread::sleep(Duration::from_millis(20));
        let snapshot = actor.snapshot();
        assert_eq!(snapshot.media_id.as_deref(), Some("ep2"));
        assert_eq!(snapshot.status, PlaybackStatus::Loading);
        assert!(!snapshot.first_frame_ready);
        assert_eq!(
            snapshot.source.as_deref(),
            Some("https://example.invalid/ep2.mp4")
        );
    }

    #[test]
    fn load_unpauses_after_keep_open_end() {
        let state = Arc::new(RwLock::new(PlaybackSnapshot::default()));
        {
            let mut snapshot = state.write().unwrap();
            snapshot.status = PlaybackStatus::Ended;
            snapshot.first_frame_ready = true;
            snapshot.media_id = Some("ep1".into());
            snapshot.position_seconds = 110.0;
        }
        let mut backend = MockMpvBackend::default();
        let mut pending_load = None;
        process_command(
            &mut backend,
            &state,
            PlaybackCommand::Load {
                media_id: Some("ep2".into()),
                url: "https://example.invalid/ep2.mp4".into(),
                headers: BTreeMap::new(),
                decryption_key: Some("aabb".into()),
                resume_position_seconds: None,
                audio_track: None,
                subtitle_track: None,
            },
            &mut pending_load,
        );
        assert!(matches!(
            backend.commands.as_slice(),
            [
                MpvCommand::LoadFile { url, decryption_key, .. },
                MpvCommand::SetProperty { name: "pause", value }
            ] if url == "https://example.invalid/ep2.mp4"
                && decryption_key.as_deref() == Some("aabb")
                && value == "no"
        ));
        let snapshot = state.read().unwrap();
        assert_eq!(snapshot.media_id.as_deref(), Some("ep2"));
        assert_eq!(snapshot.status, PlaybackStatus::Loading);
        assert!(!snapshot.first_frame_ready);
        assert_eq!(snapshot.position_seconds, 0.0);
    }
}
