//! Media probing and playback planning shared by the desktop player.
//!
//! The probe intentionally uses FFprobe's JSON output instead of parsing the
//! human-readable banner. This keeps codec, track and HDR decisions stable
//! across FFmpeg versions and gives the UI one normalized contract.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::AppState;
use crate::error::AppError;
use crate::runtime::RuntimeResourceKind;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VideoTrack {
    pub index: i64,
    pub codec: Option<String>,
    pub profile: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<f64>,
    pub bit_depth: Option<u8>,
    pub pixel_format: Option<String>,
    pub hdr: bool,
    pub default: bool,
    pub title: Option<String>,
    /// Conservative hint for the planner; runtime libmpv still decides the
    /// actual decoder and may fall back when the GPU cannot handle it.
    pub hardware_decode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    pub index: i64,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channels: Option<u16>,
    pub sample_rate: Option<u32>,
    pub bitrate: Option<u64>,
    pub default: bool,
    pub forced: bool,
    pub title: Option<String>,
    pub passthrough_capable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTrack {
    pub index: i64,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub default: bool,
    pub forced: bool,
    pub title: Option<String>,
    pub text_based: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaProbe {
    pub source: String,
    pub format: Option<String>,
    pub duration_seconds: Option<f64>,
    pub bitrate: Option<u64>,
    pub size_bytes: Option<u64>,
    pub video: Vec<VideoTrack>,
    pub audio: Vec<AudioTrack>,
    pub subtitles: Vec<SubtitleTrack>,
    pub chapters: u32,
    pub external_subtitles: Vec<String>,
    pub nfo_path: Option<String>,
    pub nfo: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackMode {
    Native,
    Browser,
    Transcode,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackFailure {
    pub kind: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhancementRuntimeStatus {
    pub enabled: bool,
    pub mode: String,
    pub fallback_active: bool,
    pub reason: Option<String>,
    /// 解码/滤镜链处理帧率，不等同于显示器刷新率。
    pub actual_fps: Option<f64>,
    /// 当前视频输出匹配到的显示器刷新率。
    pub display_fps: Option<f64>,
    /// 已保存的各增强开关，前端用于恢复按钮点亮状态。
    pub glsl_enabled: bool,
    pub rife_enabled: bool,
    pub uai_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackPlan {
    pub mode: PlaybackMode,
    pub reason: String,
    pub browser_compatible: bool,
    pub audio_track: Option<i64>,
    pub subtitle_track: Option<i64>,
    pub audio_mode: String,
    pub headers: BTreeMap<String, String>,
    pub passthrough_codecs: Vec<String>,
    pub audio_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioOutputCapabilities {
    pub passthrough: bool,
    pub codecs: Vec<String>,
    pub device: String,
    pub reason: String,
}

pub fn probe_media(
    state: &AppState,
    source: &str,
    headers: &BTreeMap<String, String>,
) -> Result<MediaProbe, AppError> {
    let path = local_path(source);
    let Some(ffprobe) = ffprobe_path(state) else {
        return probe_with_ffmpeg_banner(state, source, headers);
    };
    let mut command = Command::new(ffprobe);
    if !headers.is_empty() && source.starts_with("http") {
        let value = headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect::<Vec<_>>()
            .join("\r\n");
        command.args(["-headers", value.as_str()]);
    }
    command
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_chapters",
        ])
        .arg(path.as_deref().unwrap_or(source))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_child_window(&mut command);
    let output = command
        .output()
        .map_err(|error| AppError::Runtime(format!("failed to start FFprobe: {error}")))?;
    if !output.status.success() {
        return Err(AppError::Playback(format!(
            "FFprobe could not inspect media: {}",
            crate::diagnostics::redact_text(String::from_utf8_lossy(&output.stderr).trim())
        )));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| AppError::Runtime(format!("invalid FFprobe JSON: {error}")))?;
    Ok(normalize_probe(source, path.as_deref(), &value))
}

fn probe_with_ffmpeg_banner(
    state: &AppState,
    source: &str,
    headers: &BTreeMap<String, String>,
) -> Result<MediaProbe, AppError> {
    let ffmpeg = state
        .runtime
        .resource(RuntimeResourceKind::Ffmpeg)
        .and_then(|resource| resource.path.clone())
        .or_else(|| std::env::var("TTV_FFMPEG_PATH").ok().map(PathBuf::from))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            AppError::Runtime("FFprobe/FFmpeg is unavailable for media inspection".into())
        })?;
    let mut command = Command::new(ffmpeg);
    command.args(["-hide_banner"]);
    if !headers.is_empty() && source.starts_with("http") {
        let value = headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect::<Vec<_>>()
            .join("\r\n");
        command.args(["-headers", value.as_str()]);
    }
    command
        .args(["-i", source, "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    hide_child_window(&mut command);
    let output = command
        .output()
        .map_err(|error| AppError::Runtime(format!("failed to start FFmpeg probe: {error}")))?;
    let banner = String::from_utf8_lossy(&output.stderr);
    let mut probe = MediaProbe {
        source: source.into(),
        duration_seconds: banner.lines().find_map(parse_duration_line),
        format: banner.lines().find_map(|line| {
            line.strip_prefix("Input #")
                .map(|value| value.split(',').next().unwrap_or(value).trim().to_owned())
        }),
        ..Default::default()
    };
    for line in banner.lines().filter(|line| line.contains("Stream #")) {
        let lower = line.to_ascii_lowercase();
        let index = line
            .split_once("Stream #")
            .and_then(|(_, value)| value.split(':').nth(1))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(probe.video.len() as i64 + probe.audio.len() as i64);
        let details = line
            .split_once(':')
            .map(|(_, value)| value.trim())
            .unwrap_or(line);
        if lower.contains("video:") {
            let codec = lower
                .split("video:")
                .nth(1)
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .map(str::to_owned);
            let dimensions = details
                .split_whitespace()
                .find(|value| value.contains('x') && value.chars().any(|c| c.is_ascii_digit()));
            let (width, height) = dimensions
                .and_then(|value| value.split_once('x'))
                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                .unwrap_or((0, 0));
            probe.video.push(VideoTrack {
                index,
                codec,
                width: (width > 0).then_some(width),
                height: (height > 0).then_some(height),
                ..Default::default()
            });
        } else if lower.contains("audio:") {
            let codec = lower
                .split("audio:")
                .nth(1)
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .map(str::to_owned);
            probe.audio.push(AudioTrack {
                index,
                codec: codec.clone(),
                passthrough_capable: matches!(
                    codec.as_deref(),
                    Some("aac" | "ac3" | "eac3" | "dts" | "truehd" | "flac")
                ),
                ..Default::default()
            });
        } else if lower.contains("subtitle:") {
            let codec = lower
                .split("subtitle:")
                .nth(1)
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .map(str::to_owned);
            probe.subtitles.push(SubtitleTrack {
                index,
                codec,
                text_based: true,
                ..Default::default()
            });
        }
    }
    if let Some(local) = local_path(source) {
        let path = Path::new(&local);
        probe.size_bytes = std::fs::metadata(path).ok().map(|metadata| metadata.len());
        probe.external_subtitles = external_subtitles(path);
        probe.nfo_path = find_nfo(path).map(|value| value.display().to_string());
    }
    if probe.video.is_empty() && probe.audio.is_empty() && probe.subtitles.is_empty() {
        return Err(AppError::Playback(
            "FFmpeg could not identify any media streams".into(),
        ));
    }
    Ok(probe)
}

fn parse_duration_line(line: &str) -> Option<f64> {
    let value = line
        .split_once("Duration:")?
        .1
        .trim()
        .split(',')
        .next()?
        .trim();
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let hours: f64 = parts[0].parse().ok()?;
    let minutes: f64 = parts[1].parse().ok()?;
    let seconds: f64 = parts[2].parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

pub fn plan_playback(probe: &MediaProbe, headers: BTreeMap<String, String>) -> PlaybackPlan {
    let browser_compatible = probe.video.first().is_some_and(|video| {
        matches!(video.codec.as_deref(), Some("h264" | "vp8" | "vp9" | "av1"))
            && video.bit_depth.unwrap_or(8) <= 8
            && probe.audio.first().is_none_or(|audio| {
                matches!(
                    audio.codec.as_deref(),
                    Some("aac" | "opus" | "vorbis" | "mp3")
                )
            })
    });
    let mode = if probe.source.starts_with("http://") || probe.source.starts_with("https://") {
        if probe.source.to_ascii_lowercase().contains(".m3u8")
            || probe.source.to_ascii_lowercase().contains(".mpd")
        {
            PlaybackMode::Browser
        } else {
            PlaybackMode::Native
        }
    } else {
        PlaybackMode::Native
    };
    PlaybackPlan {
        mode,
        reason: if browser_compatible {
            "媒体包含浏览器常见的视频和音频编码".into()
        } else {
            "优先使用 libmpv，浏览器路径需要兼容转码".into()
        },
        browser_compatible,
        audio_track: choose_audio_track(probe, None),
        subtitle_track: probe
            .subtitles
            .iter()
            .find(|track| track.default)
            .map(|track| track.index),
        audio_mode: probe
            .audio
            .first()
            .map(|track| {
                if track.passthrough_capable && audio_output_capabilities().passthrough {
                    "passthrough"
                } else {
                    "transcode"
                }
                .into()
            })
            .unwrap_or_else(|| "none".into()),
        headers,
        passthrough_codecs: audio_output_capabilities().codecs,
        audio_device: audio_output_capabilities().device,
    }
}

pub fn audio_output_capabilities() -> AudioOutputCapabilities {
    let passthrough = std::env::var("TTV_AUDIO_PASSTHROUGH")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);
    let codecs = if passthrough {
        vec!["ac3", "eac3", "dts", "dts-hd", "truehd"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    AudioOutputCapabilities {
        passthrough,
        codecs,
        device: std::env::var("TTV_AUDIO_DEVICE").unwrap_or_else(|_| "系统默认输出设备".into()),
        reason: if passthrough {
            "已通过 TTV_AUDIO_PASSTHROUGH 启用 HDMI/IEC61937 直通".into()
        } else {
            "默认使用系统混音输出；未启用数字直通".into()
        },
    }
}

pub fn choose_audio_track(probe: &MediaProbe, preferred_language: Option<&str>) -> Option<i64> {
    let language_matches = |track: &AudioTrack| {
        preferred_language.is_some_and(|language| {
            track
                .language
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(language))
        })
    };
    let best = preferred_language
        .filter(|_| probe.audio.iter().any(language_matches))
        .and_then(|_| {
            probe
                .audio
                .iter()
                .filter(|track| language_matches(track))
                .max_by_key(|track| audio_rank(track))
        })
        .or_else(|| {
            probe
                .audio
                .iter()
                .filter(|track| track.default)
                .max_by_key(|track| audio_rank(track))
        })
        .or_else(|| probe.audio.iter().max_by_key(|track| audio_rank(track)));
    best.map(|track| track.index)
}

fn audio_rank(track: &AudioTrack) -> (u8, u16, u64) {
    (
        u8::from(track.default),
        track.channels.unwrap_or_default(),
        track.bitrate.unwrap_or_default(),
    )
}

fn normalize_probe(source: &str, local: Option<&str>, value: &Value) -> MediaProbe {
    let format = value.get("format").unwrap_or(&Value::Null);
    let streams = value
        .get("streams")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut probe = MediaProbe {
        source: source.into(),
        format: format
            .get("format_name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        duration_seconds: value_f64(format, "duration"),
        bitrate: value_u64(format, "bit_rate"),
        size_bytes: value_u64(format, "size")
            .or_else(|| local.and_then(|path| std::fs::metadata(path).ok().map(|m| m.len()))),
        chapters: value
            .get("chapters")
            .and_then(Value::as_array)
            .map_or(0, |items| items.len() as u32),
        ..Default::default()
    };
    for stream in streams {
        let kind = stream
            .get("codec_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "video" => probe.video.push(VideoTrack {
                index: value_i64(&stream, "index").unwrap_or_default(),
                codec: stream
                    .get("codec_name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                profile: stream
                    .get("profile")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                width: value_u64(&stream, "width").map(|v| v as u32),
                height: value_u64(&stream, "height").map(|v| v as u32),
                frame_rate: parse_ratio(stream.get("r_frame_rate").and_then(Value::as_str)),
                bit_depth: value_u64(&stream, "bits_per_raw_sample").map(|v| v as u8),
                pixel_format: stream
                    .get("pix_fmt")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                hdr: stream
                    .get("color_transfer")
                    .and_then(Value::as_str)
                    .is_some_and(|v| matches!(v, "smpte2084" | "arib-std-b67")),
                default: disposition_default(&stream),
                title: stream_title(&stream),
                hardware_decode: stream
                    .get("codec_name")
                    .and_then(Value::as_str)
                    .is_some_and(|codec| {
                        matches!(codec, "h264" | "hevc" | "vp9" | "av1" | "mpeg2video")
                    }),
            }),
            "audio" => {
                let codec = stream
                    .get("codec_name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                probe.audio.push(AudioTrack {
                    index: value_i64(&stream, "index").unwrap_or_default(),
                    passthrough_capable: matches!(
                        codec.as_deref(),
                        Some("aac" | "ac3" | "eac3" | "dts" | "truehd" | "flac")
                    ),
                    codec,
                    language: stream_language(&stream),
                    channels: value_u64(&stream, "channels").map(|v| v as u16),
                    sample_rate: value_u64(&stream, "sample_rate").map(|v| v as u32),
                    bitrate: value_u64(&stream, "bit_rate"),
                    default: disposition_default(&stream),
                    forced: disposition_forced(&stream),
                    title: stream_title(&stream),
                });
            }
            "subtitle" => {
                let codec = stream
                    .get("codec_name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                probe.subtitles.push(SubtitleTrack {
                    index: value_i64(&stream, "index").unwrap_or_default(),
                    text_based: matches!(
                        codec.as_deref(),
                        Some("subrip" | "ass" | "ssa" | "webvtt" | "mov_text" | "text")
                    ),
                    codec,
                    language: stream_language(&stream),
                    default: disposition_default(&stream),
                    forced: disposition_forced(&stream),
                    title: stream_title(&stream),
                });
            }
            _ => {}
        }
    }
    if let Some(path) = local {
        let path = Path::new(path);
        probe.external_subtitles = external_subtitles(path);
        if let Some(nfo) = find_nfo(path) {
            probe.nfo_path = Some(nfo.display().to_string());
            probe.nfo = read_nfo(&nfo);
        }
    }
    probe
}

fn read_nfo(path: &Path) -> BTreeMap<String, String> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let mut result = BTreeMap::new();
    for tag in [
        "title",
        "originaltitle",
        "year",
        "plot",
        "premiered",
        "rating",
        "uniqueid",
        "season",
        "episode",
    ] {
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        let Some(start) = contents.to_ascii_lowercase().find(&open) else {
            continue;
        };
        let Some(content_start) = contents[start..].find('>') else {
            continue;
        };
        let content_start = start + content_start + 1;
        let Some(end) = contents[content_start..].to_ascii_lowercase().find(&close) else {
            continue;
        };
        let value = contents[content_start..content_start + end].trim();
        if !value.is_empty() {
            result.insert(tag.into(), value.to_owned());
        }
    }
    result
}

fn ffprobe_path(state: &AppState) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("TTV_FFPROBE_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(path) = state
        .runtime
        .resource(RuntimeResourceKind::Ffprobe)
        .and_then(|resource| resource.path.clone())
    {
        return Some(path);
    }
    let ffmpeg = state
        .runtime
        .resource(RuntimeResourceKind::Ffmpeg)
        .and_then(|r| r.path.clone());
    let mut candidates = Vec::new();
    if let Some(path) = ffmpeg {
        if let Some(parent) = path.parent() {
            candidates.push(parent.join(if cfg!(windows) {
                "ffprobe.exe"
            } else {
                "ffprobe"
            }));
        }
    }
    candidates.extend([
        state.paths.resource_dir.join("mpv/ffprobe.exe"),
        state.paths.resource_dir.join("ffprobe.exe"),
    ]);
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| {
            let command = if cfg!(windows) { "where.exe" } else { "which" };
            std::process::Command::new(command)
                .arg("ffprobe")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .and_then(|value| value.lines().next().map(PathBuf::from))
                .filter(|path| path.is_file())
        })
}

fn local_path(source: &str) -> Option<String> {
    let value = source.trim();
    if value.is_empty() || value.starts_with("http://") || value.starts_with("https://") {
        return None;
    }
    Some(
        value
            .strip_prefix("file:///")
            .map(|v| v.replace('/', "\\"))
            .unwrap_or_else(|| value.into()),
    )
}

fn external_subtitles(path: &Path) -> Vec<String> {
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let stem = path
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    let supported = ["srt", "ass", "ssa", "vtt", "sub", "sup"];
    let mut matches = std::fs::read_dir(parent)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.flatten())
        .filter_map(|entry| {
            let candidate = entry.path();
            if !candidate.is_file() {
                return None;
            }
            let candidate_stem = candidate.file_stem()?.to_str()?;
            let extension = candidate.extension()?.to_str()?.to_ascii_lowercase();
            if !supported.contains(&extension.as_str()) {
                return None;
            }
            let same_base = candidate_stem == stem
                || candidate_stem.strip_prefix(&format!("{stem}.")).is_some();
            same_base.then(|| candidate.display().to_string())
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches
}

fn find_nfo(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let stem = path.file_stem()?.to_str()?;
    [
        parent.join(format!("{stem}.nfo")),
        parent.join("movie.nfo"),
        parent.join("tvshow.nfo"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn value_f64(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
}
fn value_u64(value: &Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
}
fn value_i64(value: &Value, key: &str) -> Option<i64> {
    value
        .get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
}
fn parse_ratio(value: Option<&str>) -> Option<f64> {
    let (a, b) = value?.split_once('/')?;
    let numerator: f64 = a.parse().ok()?;
    let denominator: f64 = b.parse().ok()?;
    (denominator > 0.0).then_some(numerator / denominator)
}
fn disposition_default(value: &Value) -> bool {
    value
        .pointer("/disposition/default")
        .and_then(Value::as_u64)
        == Some(1)
}
fn disposition_forced(value: &Value) -> bool {
    value.pointer("/disposition/forced").and_then(Value::as_u64) == Some(1)
}
fn stream_language(value: &Value) -> Option<String> {
    value
        .pointer("/tags/language")
        .and_then(Value::as_str)
        .map(str::to_owned)
}
fn stream_title(value: &Value) -> Option<String> {
    value
        .pointer("/tags/title")
        .and_then(Value::as_str)
        .map(str::to_owned)
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
    use super::*;
    #[test]
    fn normalizes_tracks_and_browser_compatibility() {
        let value = serde_json::json!({"format":{"format_name":"matroska","duration":"42","size":"100"},"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"r_frame_rate":"24/1","pix_fmt":"yuv420p","disposition":{"default":1}},{"index":1,"codec_type":"audio","codec_name":"aac","channels":2,"disposition":{"default":1}},{"index":2,"codec_type":"subtitle","codec_name":"subrip","tags":{"language":"en"}}]});
        let probe = normalize_probe("movie.mkv", Some("movie.mkv"), &value);
        assert_eq!(probe.video[0].width, Some(1920));
        assert_eq!(probe.audio[0].codec.as_deref(), Some("aac"));
        assert!(plan_playback(&probe, BTreeMap::new()).browser_compatible);
    }
}
