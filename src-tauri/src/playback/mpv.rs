//! Minimal dynamic libmpv C-ABI adapter.
//!
//! The adapter deliberately owns all unsafe FFI on the playback thread. The
//! rest of the application only sees the safe `MpvBackend` trait.

use std::ffi::{c_char, c_int, c_void, CString};
use std::path::{Path, PathBuf};
use std::ptr;

use libloading::Library;

use super::{MpvBackend, MpvCommand, MpvConfig, MpvTrackInfo, PlaybackEvent};

type MpvHandle = c_void;
type CreateFn = unsafe extern "C" fn() -> *mut MpvHandle;
type InitializeFn = unsafe extern "C" fn(*mut MpvHandle) -> c_int;
type SetOptionStringFn =
    unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int;
type SetPropertyStringFn =
    unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int;
type CommandFn = unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int;
type TerminateDestroyFn = unsafe extern "C" fn(*mut MpvHandle);
type GetPropertyStringFn = unsafe extern "C" fn(*mut MpvHandle, *const c_char) -> *mut c_char;
type FreeFn = unsafe extern "C" fn(*mut c_void);
type WaitEventFn = unsafe extern "C" fn(*mut MpvHandle, f64) -> *const MpvEvent;
type ObservePropertyFn = unsafe extern "C" fn(*mut MpvHandle, u64, *const c_char, c_int) -> c_int;

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEventProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEventLogMessage {
    prefix: *const c_char,
    level: *const c_char,
    text: *const c_char,
    log_level: c_int,
}

#[repr(C)]
struct MpvEventEndFile {
    reason: c_int,
    error: c_int,
    playlist_entry_id: i64,
    playlist_insert_id: i64,
    playlist_insert_num_entries: c_int,
}

const MPV_EVENT_END_FILE: c_int = 7;
const MPV_EVENT_FILE_LOADED: c_int = 8;
const MPV_EVENT_PLAYBACK_RESTART: c_int = 21;
const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;
const MPV_EVENT_LOG_MESSAGE: c_int = 2;
const MPV_FORMAT_STRING: c_int = 1;
const MPV_FORMAT_FLAG: c_int = 3;
const MPV_FORMAT_INT64: c_int = 4;
const MPV_FORMAT_DOUBLE: c_int = 5;
const MPV_LOG_LEVEL_ERROR: c_int = 40;
const MPV_END_FILE_REASON_ERROR: c_int = 4;

pub struct LibMpvBackend {
    _library: Library,
    handle: *mut MpvHandle,
    mpv_initialize: InitializeFn,
    mpv_set_option_string: SetOptionStringFn,
    mpv_set_property_string: SetPropertyStringFn,
    mpv_command: CommandFn,
    mpv_terminate_destroy: TerminateDestroyFn,
    mpv_get_property_string: GetPropertyStringFn,
    mpv_free: FreeFn,
    mpv_wait_event: WaitEventFn,
    mpv_observe_property: Option<ObservePropertyFn>,
    last_duration: Option<f64>,
    last_position: Option<f64>,
    last_pause: Option<bool>,
    last_buffer: Option<f64>,
    last_audio_track: Option<i64>,
    last_subtitle_track: Option<i64>,
    last_track_count: Option<i64>,
}

// libmpv is confined to the actor's dedicated thread by PlaybackActor.
unsafe impl Send for LibMpvBackend {}

impl std::fmt::Debug for LibMpvBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LibMpvBackend")
            .field("initialized", &(self.handle.is_null() == false))
            .finish()
    }
}

impl LibMpvBackend {
    pub fn load_default() -> Result<Self, String> {
        let mut roots = Vec::new();
        if let Ok(executable) = std::env::current_exe() {
            if let Some(parent) = executable.parent() {
                roots.push(parent.to_owned());
                roots.push(parent.join("resources"));
                roots.push(parent.join("resources").join("bin"));
            }
        }
        if let Ok(current) = std::env::current_dir() {
            roots.push(current.clone());
            roots.push(current.join("resources"));
            roots.push(current.join("resources").join("bin"));
        }
        Self::load_from_roots(roots)
    }

    pub fn load_from_roots<I>(roots: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let names = library_names();
        let mut attempted = Vec::new();
        for root in roots {
            for name in &names {
                let path = root.join(name);
                attempted.push(path.display().to_string());
                if path.is_file() {
                    match unsafe { Self::load_from_path(&path) } {
                        Ok(backend) => return Ok(backend),
                        Err(_) => continue,
                    }
                }
            }
        }
        Err(format!(
            "libmpv library not found; attempted {}",
            attempted.join(", ")
        ))
    }

    unsafe fn load_from_path(path: &Path) -> Result<Self, String> {
        let library = Library::new(path).map_err(|error| error.to_string())?;
        let mpv_create: CreateFn = *library
            .get(b"mpv_create\0")
            .map_err(|error| format!("missing mpv_create: {error}"))?;
        let mpv_initialize: InitializeFn = *library
            .get(b"mpv_initialize\0")
            .map_err(|error| format!("missing mpv_initialize: {error}"))?;
        let mpv_set_option_string: SetOptionStringFn = *library
            .get(b"mpv_set_option_string\0")
            .map_err(|error| format!("missing mpv_set_option_string: {error}"))?;
        let mpv_set_property_string: SetPropertyStringFn = *library
            .get(b"mpv_set_property_string\0")
            .map_err(|error| format!("missing mpv_set_property_string: {error}"))?;
        let mpv_command: CommandFn = *library
            .get(b"mpv_command\0")
            .map_err(|error| format!("missing mpv_command: {error}"))?;
        let mpv_terminate_destroy: TerminateDestroyFn = *library
            .get(b"mpv_terminate_destroy\0")
            .map_err(|error| format!("missing mpv_terminate_destroy: {error}"))?;
        let mpv_get_property_string: GetPropertyStringFn = *library
            .get(b"mpv_get_property_string\0")
            .map_err(|error| format!("missing mpv_get_property_string: {error}"))?;
        let mpv_free: FreeFn = *library
            .get(b"mpv_free\0")
            .map_err(|error| format!("missing mpv_free: {error}"))?;
        let mpv_wait_event: WaitEventFn = *library
            .get(b"mpv_wait_event\0")
            .map_err(|error| format!("missing mpv_wait_event: {error}"))?;
        let mpv_observe_property = library
            .get::<ObservePropertyFn>(b"mpv_observe_property\0")
            .ok()
            .map(|symbol| *symbol);
        let handle = mpv_create();
        if handle.is_null() {
            return Err("mpv_create returned a null handle".into());
        }
        Ok(Self {
            _library: library,
            handle,
            mpv_initialize,
            mpv_set_option_string,
            mpv_set_property_string,
            mpv_command,
            mpv_terminate_destroy,
            mpv_get_property_string,
            mpv_free,
            mpv_wait_event,
            mpv_observe_property,
            last_duration: None,
            last_position: None,
            last_pause: None,
            last_buffer: None,
            last_audio_track: None,
            last_subtitle_track: None,
            last_track_count: None,
        })
    }

    fn ensure_handle(&self) -> Result<*mut MpvHandle, String> {
        if self.handle.is_null() {
            Err("libmpv handle is not initialized".into())
        } else {
            Ok(self.handle)
        }
    }

    fn set_option(&self, name: &str, value: &str) -> Result<(), String> {
        let handle = self.ensure_handle()?;
        let name = CString::new(name).map_err(|_| "mpv option name contains NUL".to_owned())?;
        let value = CString::new(value).map_err(|_| "mpv option value contains NUL".to_owned())?;
        let result = unsafe { (self.mpv_set_option_string)(handle, name.as_ptr(), value.as_ptr()) };
        check_result("mpv_set_option_string", result)
    }

    fn set_property(&self, name: &str, value: &str) -> Result<(), String> {
        let handle = self.ensure_handle()?;
        let name = CString::new(name).map_err(|_| "mpv property name contains NUL".to_owned())?;
        let value =
            CString::new(value).map_err(|_| "mpv property value contains NUL".to_owned())?;
        let result =
            unsafe { (self.mpv_set_property_string)(handle, name.as_ptr(), value.as_ptr()) };
        check_result("mpv_set_property_string", result)
    }

    fn send_command(&self, values: &[String]) -> Result<(), String> {
        let handle = self.ensure_handle()?;
        let strings = values
            .iter()
            .map(|value| {
                CString::new(value.as_str()).map_err(|_| "mpv command contains NUL".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut pointers = strings
            .iter()
            .map(|value| value.as_ptr())
            .collect::<Vec<_>>();
        pointers.push(ptr::null());
        let result = unsafe { (self.mpv_command)(handle, pointers.as_ptr()) };
        check_result("mpv_command", result)
    }

    fn property(&self, name: &str) -> Option<String> {
        let handle = self.handle;
        let name = CString::new(name).ok()?;
        let pointer = unsafe { (self.mpv_get_property_string)(handle, name.as_ptr()) };
        if pointer.is_null() {
            return None;
        }
        let value = unsafe { std::ffi::CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.mpv_free)(pointer.cast()) };
        Some(value)
    }
}

impl MpvBackend for LibMpvBackend {
    fn initialize(&mut self, config: &MpvConfig) -> Result<(), String> {
        for (name, value) in &config.options {
            self.set_option(name, value)
                .map_err(|error| format!("invalid mpv option {name}={value}: {error}"))?;
        }
        if let Some(window_id) = config.window_id {
            // A native target opts into the GPU video path; the default config
            // stays headless so libmpv cannot create an auxiliary window.
            self.set_option("force-window", "yes")?;
            self.set_option("vo", "gpu-next,gpu")?;
            self.set_option("wid", &window_id.to_string())?;
        }
        let handle = self.ensure_handle()?;
        let result = unsafe { (self.mpv_initialize)(handle) };
        check_result("mpv_initialize", result)?;
        if let Some(observe) = self.mpv_observe_property {
            for (id, name, format) in [
                (1, "time-pos", MPV_FORMAT_DOUBLE),
                (2, "duration", MPV_FORMAT_DOUBLE),
                (3, "pause", MPV_FORMAT_FLAG),
                (4, "cache-buffering-state", MPV_FORMAT_DOUBLE),
                (5, "video-format", MPV_FORMAT_STRING),
                (6, "audio-codec-name", MPV_FORMAT_STRING),
                (7, "hwdec-current", MPV_FORMAT_STRING),
                (8, "vo-configured", MPV_FORMAT_STRING),
                (9, "estimated-vf-fps", MPV_FORMAT_DOUBLE),
                (10, "aid", MPV_FORMAT_STRING),
                (11, "sid", MPV_FORMAT_STRING),
                (12, "frame-drop-count", MPV_FORMAT_INT64),
                (13, "display-fps", MPV_FORMAT_DOUBLE),
            ] {
                let name =
                    CString::new(name).map_err(|_| "mpv property contains NUL".to_owned())?;
                let result = unsafe { observe(handle, id, name.as_ptr(), format) };
                if result < 0 {
                    return Err(format!(
                        "mpv_observe_property failed for {name:?}: {result}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn command(&mut self, command: MpvCommand) -> Result<(), String> {
        match command {
            MpvCommand::LoadFile {
                url,
                headers,
                decryption_key,
            } => {
                self.last_duration = None;
                self.last_position = None;
                self.last_pause = None;
                self.last_buffer = None;
                self.last_audio_track = None;
                self.last_subtitle_track = None;
                // 新文件加载后强制重新枚举 track-list(旧文件的轨道数可能相同但内容不同)
                self.last_track_count = None;
                if !headers.is_empty() {
                    let header_value = headers
                        .iter()
                        .map(|(name, value)| format!("{name}: {value}"))
                        .collect::<Vec<_>>()
                        .join(",");
                    self.set_property("http-header-fields", &header_value)?;
                } else {
                    // An empty header set must clear the previous file's
                    // headers, or a stale Authorization token gets replayed
                    // against the next (unrelated) host.
                    self.set_property("http-header-fields", "")?;
                }
                // Do not carry a muted/disabled audio state into the next file.
                self.set_property("aid", "auto")?;
                self.set_property("mute", "no")?;
                self.set_property("volume", "100")?;
                // CENC 加密直链（红果锁定集）：decryption_key 走 mov demuxer 的
                // AVOption。每次 Load 都显式设置/清空，避免密钥泄漏到下一个文件
                // （与上方 http-header-fields 的清空策略同理）。
                match decryption_key
                    .as_deref()
                    .filter(|key| !key.trim().is_empty())
                {
                    Some(key) => {
                        self.set_property(
                            "demuxer-lavf-o",
                            &format!("decryption_key={}", key.trim()),
                        )?;
                    }
                    None => {
                        self.set_property("demuxer-lavf-o", "")?;
                    }
                }
                // keep-open=yes 会在片尾把 pause 留在 yes。loadfile replace
                // 不会清掉它，切集后新文件会停在 0:00 且画面仍是上一帧。
                self.set_property("pause", "no")?;
                self.send_command(&["loadfile".into(), url, "replace".into()])?;
                self.set_property("pause", "no")
            }
            MpvCommand::SetProperty { name, value } => self.set_property(name, &value),
            MpvCommand::Seek { position_seconds } => self.send_command(&[
                "seek".into(),
                position_seconds.to_string(),
                "absolute".into(),
            ]),
            MpvCommand::Stop => self.send_command(&["stop".into()]),
            MpvCommand::Unload => self.send_command(&["stop".into()]),
            MpvCommand::SetShaders(shaders) => {
                self.set_property("glsl-shaders", &shaders.join(","))
            }
            MpvCommand::SetVideoFilter(filter) => {
                self.set_property("vf", filter.as_deref().unwrap_or("").trim())
            }
            MpvCommand::Command(args) => self.send_command(&args),
        }
    }

    fn terminate(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.mpv_terminate_destroy)(self.handle) };
            self.handle = ptr::null_mut();
        }
    }

    fn poll_events(&mut self) -> Vec<PlaybackEvent> {
        let mut events = Vec::new();
        for _ in 0..16 {
            let event = unsafe { (self.mpv_wait_event)(self.handle, 0.0) };
            if event.is_null() {
                break;
            }
            match unsafe { (*event).event_id } {
                MPV_EVENT_END_FILE => {
                    let end_file = unsafe { (*event).data.cast::<MpvEventEndFile>().as_ref() };
                    if end_file.is_some_and(|value| {
                        value.reason == MPV_END_FILE_REASON_ERROR || value.error < 0
                    }) {
                        let code = end_file
                            .map(|value| value.error)
                            .unwrap_or(unsafe { (*event).error });
                        events.push(PlaybackEvent::Failed {
                            message: format!("libmpv ended playback with error {code}"),
                        });
                    } else {
                        events.push(PlaybackEvent::Ended);
                    }
                }
                MPV_EVENT_FILE_LOADED => events.push(PlaybackEvent::FileLoaded {
                    duration_seconds: self
                        .property("duration")
                        .and_then(|value| value.parse::<f64>().ok())
                        .filter(|value| value.is_finite() && *value > 0.0),
                }),
                // playback-restart 在 mpv 完成解码/滤镜链初始化、真正开始输出画面时触发，
                // 是"首帧已渲染"的可靠信号；FILE_LOADED 早于首帧，不能用来点亮透明画布。
                MPV_EVENT_PLAYBACK_RESTART => events.push(PlaybackEvent::PlaybackRestarted),
                MPV_EVENT_PROPERTY_CHANGE => {
                    if let Some(property) =
                        unsafe { (*event).data.cast::<MpvEventProperty>().as_ref() }
                    {
                        if let Some(name) = unsafe { property.name.as_ref() }.map(|_| {
                            unsafe { std::ffi::CStr::from_ptr(property.name) }
                                .to_string_lossy()
                                .into_owned()
                        }) {
                            if let Some(value) = self.property_event_value(property) {
                                match name.as_str() {
                                    "time-pos" => {
                                        if let Ok(value) = value.parse::<f64>() {
                                            self.push_position(&mut events, value);
                                        }
                                    }
                                    "duration" => {
                                        if let Ok(value) = value.parse::<f64>() {
                                            self.push_duration(&mut events, value);
                                        }
                                    }
                                    "pause" => self.push_pause(
                                        &mut events,
                                        value == "yes" || value == "true" || value == "1",
                                    ),
                                    "cache-buffering-state" => {
                                        if let Ok(value) = value.parse::<f64>() {
                                            self.push_buffer(&mut events, value);
                                        }
                                    }
                                    "aid" => self.push_track_change(
                                        &mut events,
                                        value.parse::<i64>().ok(),
                                        self.last_subtitle_track,
                                    ),
                                    "sid" => self.push_track_change(
                                        &mut events,
                                        self.last_audio_track,
                                        value.parse::<i64>().ok(),
                                    ),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                MPV_EVENT_LOG_MESSAGE => {
                    if let Some(log) =
                        unsafe { (*event).data.cast::<MpvEventLogMessage>().as_ref() }
                    {
                        if log.log_level >= MPV_LOG_LEVEL_ERROR {
                            let text = if log.text.is_null() {
                                "libmpv reported a playback error".to_owned()
                            } else {
                                crate::diagnostics::redact_text(
                                    unsafe { std::ffi::CStr::from_ptr(log.text) }
                                        .to_string_lossy()
                                        .trim(),
                                )
                            };
                            let lower = text.to_ascii_lowercase();
                            if lower.contains("vapoursynth")
                                || lower.contains("rife")
                                || lower.contains("ttv-interp")
                                || lower.contains("minterpolate")
                                || lower.contains("impossible to convert")
                                || lower.contains("video filter")
                                || lower.contains("vf")
                            {
                                events.push(PlaybackEvent::EnhancementFailed {
                                    message: "补帧滤镜运行失败，已自动卸载并回退原始帧率".into(),
                                });
                            } else {
                                events.push(PlaybackEvent::Failed { message: text });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(position) = self
            .property("time-pos")
            .and_then(|v| v.parse::<f64>().ok())
        {
            self.push_position(&mut events, position);
        }
        if let Some(duration) = self
            .property("duration")
            .and_then(|v| v.parse::<f64>().ok())
        {
            self.push_duration(&mut events, duration);
        }
        if let Some(paused) = self.property("pause") {
            self.push_pause(
                &mut events,
                paused == "yes" || paused == "true" || paused == "1",
            );
        }
        if let Some(buffer) = self
            .property("cache-buffering-state")
            .and_then(|v| v.parse::<f64>().ok())
        {
            self.push_buffer(&mut events, buffer);
        }
        self.push_track_list(&mut events);
        let audio_track = self
            .property("aid")
            .and_then(|value| value.parse::<i64>().ok());
        let subtitle_track = self
            .property("sid")
            .and_then(|value| value.parse::<i64>().ok());
        self.push_track_change(&mut events, audio_track, subtitle_track);
        events.push(PlaybackEvent::RuntimeInfo {
            video_codec: self.property("video-format"),
            audio_codec: self.property("audio-codec-name"),
            decoder: self.property("hwdec-current"),
            renderer: self.property("vo-configured"),
            actual_fps: self
                .property("estimated-display-fps")
                .and_then(|value| value.parse().ok())
                .filter(|value| *value > 1.0)
                .or_else(|| {
                    self.property("estimated-vf-fps")
                        .and_then(|value| value.parse().ok())
                }),
            source_fps: self
                .property("container-fps")
                .and_then(|value| value.parse().ok())
                .or_else(|| {
                    self.property("current-tracks/video/demux-fps")
                        .and_then(|value| value.parse().ok())
                }),
            display_fps: self
                .property("display-fps")
                .and_then(|value| value.parse().ok()),
            dropped_frames: self
                .property("frame-drop-count")
                .and_then(|value| value.parse().ok()),
            video_width: self.property("dwidth").and_then(|value| value.parse().ok()),
            video_height: self.property("dheight").and_then(|value| value.parse().ok()),
        });
        events
    }
}

impl LibMpvBackend {
    fn property_event_value(&self, property: &MpvEventProperty) -> Option<String> {
        if property.data.is_null() {
            return None;
        }
        unsafe {
            match property.format {
                MPV_FORMAT_STRING => {
                    let value = *(property.data.cast::<*const c_char>());
                    (!value.is_null()).then(|| {
                        std::ffi::CStr::from_ptr(value)
                            .to_string_lossy()
                            .into_owned()
                    })
                }
                MPV_FORMAT_FLAG => Some((*(property.data.cast::<c_int>()) != 0).to_string()),
                MPV_FORMAT_INT64 => Some((*(property.data.cast::<i64>())).to_string()),
                MPV_FORMAT_DOUBLE => Some((*(property.data.cast::<f64>())).to_string()),
                _ => None,
            }
        }
    }

    fn push_position(&mut self, events: &mut Vec<PlaybackEvent>, value: f64) {
        if value.is_finite()
            && self
                .last_position
                .map_or(true, |last| (last - value).abs() > 0.02)
        {
            self.last_position = Some(value);
            events.push(PlaybackEvent::TimePosition {
                position_seconds: value,
            });
        }
    }
    fn push_duration(&mut self, events: &mut Vec<PlaybackEvent>, value: f64) {
        if value.is_finite()
            && value > 0.0
            && self
                .last_duration
                .map_or(true, |last| (last - value).abs() > 0.01)
        {
            self.last_duration = Some(value);
            events.push(PlaybackEvent::DurationChanged {
                duration_seconds: value,
            });
        }
    }
    fn push_pause(&mut self, events: &mut Vec<PlaybackEvent>, value: bool) {
        if self.last_pause != Some(value) {
            self.last_pause = Some(value);
            events.push(PlaybackEvent::PauseChanged { paused: value });
        }
    }
    fn push_buffer(&mut self, events: &mut Vec<PlaybackEvent>, value: f64) {
        if value.is_finite()
            && self
                .last_buffer
                .map_or(true, |last| (last - value).abs() > 0.5)
        {
            self.last_buffer = Some(value);
            events.push(PlaybackEvent::Buffering {
                active: value < 100.0,
                percent: Some(value.clamp(0.0, 100.0)),
            });
        }
    }

    fn push_track_change(
        &mut self,
        events: &mut Vec<PlaybackEvent>,
        audio_track: Option<i64>,
        subtitle_track: Option<i64>,
    ) {
        if self.last_audio_track != audio_track || self.last_subtitle_track != subtitle_track {
            self.last_audio_track = audio_track;
            self.last_subtitle_track = subtitle_track;
            events.push(PlaybackEvent::TracksChanged {
                audio_track,
                subtitle_track,
            });
        }
    }

    /// 枚举 mpv `track-list` 并在轨道数量变化时发出 TrackListChanged。
    ///
    /// 只有数量变化才重新逐条读取属性(每条约 8 次 property 调用),常规 250ms
    /// 轮询只付一次 `track-list/count` 的开销。当前选中轨道由 TracksChanged/
    /// 快照的 audio_track/subtitle_track 表达,不依赖 selected 标记。
    fn push_track_list(&mut self, events: &mut Vec<PlaybackEvent>) {
        let Some(count) = self
            .property("track-list/count")
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|count| *count > 0 && *count <= 128)
        else {
            return;
        };
        if self.last_track_count == Some(count) {
            return;
        }
        let mut tracks = Vec::with_capacity(count.max(0) as usize);
        for index in 0..count {
            let prefix = format!("track-list/{index}");
            let Some(id) = self
                .property(&format!("{prefix}/id"))
                .and_then(|value| value.parse::<i64>().ok())
            else {
                continue;
            };
            let flag = |name: &str| {
                matches!(
                    self.property(&format!("{prefix}/{name}")).as_deref(),
                    Some("yes") | Some("true") | Some("1")
                )
            };
            tracks.push(MpvTrackInfo {
                id,
                kind: self.property(&format!("{prefix}/type")).unwrap_or_default(),
                lang: self
                    .property(&format!("{prefix}/lang"))
                    .filter(|value| !value.is_empty()),
                title: self
                    .property(&format!("{prefix}/title"))
                    .filter(|value| !value.is_empty()),
                codec: self
                    .property(&format!("{prefix}/codec"))
                    .filter(|value| !value.is_empty()),
                selected: flag("selected"),
                default: flag("default"),
                forced: flag("forced"),
                ff_index: self
                    .property(&format!("{prefix}/ff-index"))
                    .and_then(|value| value.parse::<i64>().ok()),
            });
        }
        self.last_track_count = Some(count);
        if !tracks.is_empty() {
            events.push(PlaybackEvent::TrackListChanged { tracks });
        }
    }
}

impl Drop for LibMpvBackend {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn check_result(operation: &str, result: c_int) -> Result<(), String> {
    if result >= 0 {
        Ok(())
    } else {
        Err(format!("{operation} failed with mpv error {result}"))
    }
}

fn library_names() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["libmpv-2.dll", "mpv-2.dll", "libmpv.dll"]
    } else if cfg!(target_os = "macos") {
        vec!["libmpv.2.dylib", "libmpv.dylib"]
    } else {
        vec!["libmpv.so.2", "libmpv.so"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_returns_attempted_paths() {
        let error =
            LibMpvBackend::load_from_roots([PathBuf::from("Z:/ttv/does-not-exist")]).unwrap_err();
        assert!(error.contains("libmpv library not found"));
        assert!(error.contains("does-not-exist"));
    }

    #[test]
    fn library_names_match_platform() {
        assert!(!library_names().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn bundled_library_loads_and_initializes() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mpv");
        if !root.join("libmpv-2.dll").is_file() {
            return;
        }
        let backend = LibMpvBackend::load_from_roots([root]).unwrap();
        let actor = crate::playback::PlaybackActor::start(backend, MpvConfig::default()).unwrap();
        actor.shutdown();
    }

    #[cfg(windows)]
    #[test]
    fn bundled_library_plays_smoke_media_and_reports_progress() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mpv");
        let sample = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|path| path.join(".downloads/player-smoke.mp4"));
        let Some(sample) = sample.filter(|path| path.is_file()) else {
            return;
        };
        if !root.join("libmpv-2.dll").is_file() {
            return;
        }
        let backend = LibMpvBackend::load_from_roots([root]).unwrap();
        let actor = crate::playback::PlaybackActor::start(backend, MpvConfig::default()).unwrap();
        actor
            .dispatch(crate::playback::PlaybackCommand::Load {
                media_id: Some("smoke".into()),
                url: sample.to_string_lossy().into_owned(),
                headers: Default::default(),
                decryption_key: None,
                resume_position_seconds: None,
                audio_track: None,
                subtitle_track: None,
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        let mut saw_progress = false;
        let mut saw_end = false;
        while std::time::Instant::now() < deadline {
            let snapshot = actor.snapshot();
            saw_progress |= snapshot.position_seconds > 0.0;
            saw_end |= snapshot.status == crate::playback::PlaybackStatus::Ended;
            if saw_end || snapshot.status == crate::playback::PlaybackStatus::Error {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let snapshot = actor.snapshot();
        assert_ne!(snapshot.status, crate::playback::PlaybackStatus::Error);
        assert!(snapshot.duration_seconds.unwrap_or_default() > 0.0);
        assert!(saw_progress || saw_end);
        actor.shutdown();
    }

    #[cfg(windows)]
    #[test]
    fn bundled_library_attaches_external_subtitle_track() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mpv");
        let sample = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|path| path.join(".downloads/player-smoke.mp4"));
        let Some(sample) = sample.filter(|path| path.is_file()) else {
            return;
        };
        if !root.join("libmpv-2.dll").is_file() {
            return;
        }
        let subtitle = std::env::temp_dir().join(format!(
            "ttv-smoke-{}-{}.srt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(
            &subtitle,
            "1\n00:00:00,000 --> 00:00:00,900\nTTV subtitle smoke\n",
        )
        .unwrap();
        let backend = LibMpvBackend::load_from_roots([root]).unwrap();
        let actor = crate::playback::PlaybackActor::start(backend, MpvConfig::default()).unwrap();
        actor
            .dispatch(crate::playback::PlaybackCommand::Load {
                media_id: Some("subtitle-smoke".into()),
                url: sample.to_string_lossy().into_owned(),
                headers: Default::default(),
                decryption_key: None,
                resume_position_seconds: None,
                audio_track: None,
                subtitle_track: None,
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline
            && actor.snapshot().duration_seconds.unwrap_or_default() <= 0.0
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        actor
            .dispatch(crate::playback::PlaybackCommand::RawCommand {
                args: vec![
                    "sub-add".into(),
                    subtitle.to_string_lossy().into_owned(),
                    "select".into(),
                ],
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline && actor.snapshot().subtitle_track.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(actor.snapshot().subtitle_track.is_some());
        actor.shutdown();
        let _ = std::fs::remove_file(subtitle);
    }
}
