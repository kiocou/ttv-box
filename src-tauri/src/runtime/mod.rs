//! Runtime resource discovery and diagnostics.
//!
//! Resource probing is deliberately filesystem-only. It does not load DLLs or
//! start external processes, so startup diagnostics remain safe on developer
//! machines and in CI. The real libmpv loader can consume the resulting paths.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeResourceKind {
    LibMpv,
    Ffmpeg,
    Ffprobe,
    VapourSynth,
    RifeModel,
    Shader,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeResourceStatus {
    Present,
    Missing,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct RuntimeResourceSpec {
    pub kind: RuntimeResourceKind,
    pub name: String,
    pub candidates: Vec<PathBuf>,
    pub required: bool,
}

impl RuntimeResourceSpec {
    pub fn new(
        kind: RuntimeResourceKind,
        name: impl Into<String>,
        candidates: Vec<PathBuf>,
        required: bool,
    ) -> Self {
        Self {
            kind,
            name: name.into(),
            candidates,
            required,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeResource {
    pub kind: RuntimeResourceKind,
    pub name: String,
    pub path: Option<PathBuf>,
    pub required: bool,
    pub status: RuntimeResourceStatus,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostics {
    pub checked_at_ms: u64,
    pub resources: Vec<RuntimeResource>,
    pub playback_available: bool,
    pub upscaling_available: bool,
    pub interpolation_available: bool,
    pub enhancement_available: bool,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhancementPlan {
    pub mode: u8,
    pub interpolation_enabled: bool,
    pub upscaling_enabled: bool,
    pub ai_upscaling_enabled: bool,
    pub shader_paths: Vec<PathBuf>,
    pub interpolation_script: Option<PathBuf>,
    pub interpolation_fallback_script: Option<PathBuf>,
    pub ai_upscaling_script: Option<PathBuf>,
    pub warnings: Vec<String>,
}

impl RuntimeDiagnostics {
    pub fn resource(&self, kind: RuntimeResourceKind) -> Option<&RuntimeResource> {
        self.resources.iter().find(|resource| resource.kind == kind)
    }

    pub fn enhancement_plan(&self, mode: u8, paths: &RuntimePaths) -> EnhancementPlan {
        let mode = mode.min(5);
        let mut warnings = Vec::new();
        let glsl_requested = matches!(mode, 1 | 3);
        let ai_upscaling_requested = matches!(mode, 4 | 5);
        let upscaling_requested = glsl_requested || ai_upscaling_requested;
        let interpolation_requested = matches!(mode, 2 | 3 | 5);
        let upscaling_enabled = upscaling_requested && self.upscaling_available;
        let interpolation_enabled = interpolation_requested && self.interpolation_available;
        if upscaling_requested && !upscaling_enabled {
            warnings.push(
                "realtime upscaling shaders are unavailable; using original rendering".into(),
            );
        }
        if interpolation_requested && !interpolation_enabled {
            warnings.push("mpv playback is unavailable; interpolation disabled".into());
        }
        let shader_paths = if upscaling_enabled {
            [
                "SSimDownscaler.glsl",
                "ArtCNN.glsl",
                "adaptive-sharpen.glsl",
            ]
            .iter()
            .filter_map(|name| {
                [
                    paths.shader_dir.join(name),
                    paths.resource_dir.join("shaders").join(name),
                ]
                .into_iter()
                .find(|path| path.is_file())
            })
            .collect()
        } else {
            Vec::new()
        };
        if upscaling_requested && upscaling_enabled && shader_paths.is_empty() {
            warnings.push("realtime upscaling shader files are missing".into());
        }

        let interpolation_script = if interpolation_enabled {
            first_existing_file([
                paths
                    .resource_dir
                    .join("vapoursynth/scripts/MEMC_RIFE_DML.vpy"),
                paths
                    .root_dir
                    .join("resources/vapoursynth/scripts/MEMC_RIFE_DML.vpy"),
            ])
        } else {
            None
        };
        let interpolation_fallback_script = if interpolation_enabled {
            first_existing_file([
                paths
                    .resource_dir
                    .join("vapoursynth/scripts/MEMC_MVT_LQ.vpy"),
                paths
                    .root_dir
                    .join("resources/vapoursynth/scripts/MEMC_MVT_LQ.vpy"),
            ])
        } else {
            None
        };
        if interpolation_enabled && interpolation_script.is_none() {
            warnings.push(
                "RIFE script missing; live interpolation will use mpv display-resample".into(),
            );
        }

        EnhancementPlan {
            mode,
            interpolation_enabled,
            upscaling_enabled: upscaling_enabled && !shader_paths.is_empty(),
            ai_upscaling_enabled: ai_upscaling_requested && !shader_paths.is_empty(),
            shader_paths,
            interpolation_script,
            interpolation_fallback_script,
            ai_upscaling_script: None,
            warnings,
        }
    }
}

fn first_existing_file(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|path| path.is_file())
}

/// Escape a filesystem path for mpv filter arguments.
///
/// mpv treats `:` as an option separator. Wrapping the path in `[]` keeps
/// Windows drive letters and spaces intact.
pub fn mpv_bracket_path(path: &Path) -> String {
    format!("[{}]", path.to_string_lossy().replace('\\', "/"))
}

/// Build the libmpv `vf` argument that loads a VapourSynth interpolation script.
pub fn vapoursynth_mpv_filter(
    script: &Path,
    buffered_frames: u32,
    concurrent_frames: u32,
) -> String {
    format!(
        "@ttv-interp:vapoursynth=file={}:buffered-frames={buffered_frames}:concurrent-frames={concurrent_frames}",
        mpv_bracket_path(script)
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePaths {
    pub root_dir: PathBuf,
    pub resource_dir: PathBuf,
    pub binary_dir: PathBuf,
    pub shader_dir: PathBuf,
    pub model_dir: PathBuf,
}

/// Resolve resources in both development and a packaged Tauri application.
/// Tauri places mapped resources beside the executable in its resource
/// directory, while development runs from the workspace or Cargo target.
pub fn discover_resource_dir(data_resource_dir: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(value) = env::var_os("TTV_RESOURCE_DIR") {
        candidates.push(PathBuf::from(value));
    }
    if let Ok(executable) = env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join("resources"));
            candidates.push(parent.to_owned());
        }
    }
    if let Ok(current) = env::current_dir() {
        candidates.push(current.join("resources"));
        candidates.push(current.join("src-tauri/resources"));
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"));
    if let Some(path) = data_resource_dir {
        candidates.push(path.to_owned());
    }
    candidates
        .into_iter()
        .find(|path| path.join("mpv").is_dir() || path.join("shaders").is_dir())
}

/// Make native runtimes discoverable before libmpv/VapourSynth are loaded.
/// This is intentionally process-local and prepends paths rather than
/// overwriting a user's existing environment.
pub fn prepare_runtime_environment(resource_dir: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    let vapoursynth_dir = resource_dir.join("vapoursynth");
    preload_windows_runtime(&vapoursynth_dir, &mut warnings);
    let path_entries = [
        resource_dir.join("mpv"),
        vapoursynth_dir.join("vsscript/python/bridge"),
        vapoursynth_dir.clone(),
        vapoursynth_dir.join("python"),
        vapoursynth_dir.join("site-packages/vapoursynth"),
        vapoursynth_dir.join("vs-plugins"),
        vapoursynth_dir.join("vs-plugins/vsort"),
    ];
    let mut path_value = path_entries
        .iter()
        .filter(|path| path.is_dir())
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if let Some(existing) = env::var_os("PATH") {
        path_value.push(existing.to_string_lossy().into_owned());
    }
    if path_value.len() > 1 {
        env::set_var("PATH", path_value.join(";"));
    } else {
        warnings.push("runtime PATH entries are unavailable".into());
    }

    let python_home = vapoursynth_dir.join("python");
    if python_home.is_dir() {
        env::set_var("PYTHONHOME", &python_home);
    } else {
        warnings.push("bundled Python runtime is missing".into());
    }
    let python_paths = [
        vapoursynth_dir.join("site-packages"),
        vapoursynth_dir.join("python"),
    ];
    env::set_var(
        "PYTHONPATH",
        python_paths
            .iter()
            .filter(|path| path.exists())
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(";"),
    );

    let plugin_dir = vapoursynth_dir.join("vs-plugins");
    if plugin_dir.is_dir() {
        let plugin_value = plugin_dir.to_string_lossy().into_owned();
        env::set_var("VAPOURSYNTH_PLUGIN_PATH", &plugin_value);
        env::set_var("VS_PLUGIN_PATH", plugin_value);
    } else {
        warnings.push("VapourSynth plugin directory is missing".into());
    }
    let vsscript_path = vapoursynth_dir.join("vsscript/python/bridge/vsscript.dll");
    if vsscript_path.is_file() {
        env::set_var("VSSCRIPT_PATH", vsscript_path);
    } else {
        warnings.push("portable VSScript bridge is missing".into());
    }
    warnings
}

#[cfg(windows)]
fn preload_windows_runtime(vapoursynth_dir: &Path, warnings: &mut Vec<String>) {
    for name in [
        "msvcp140.dll",
        "msvcp140_1.dll",
        "msvcp140_2.dll",
        "msvcp140_atomic_wait.dll",
        "vcruntime140.dll",
        "vcruntime140_1.dll",
        "vcruntime140_threads.dll",
    ] {
        let path = vapoursynth_dir.join(name);
        if !path.is_file() {
            continue;
        }
        match unsafe { libloading::Library::new(&path) } {
            Ok(library) => std::mem::forget(library),
            Err(error) if name == "msvcp140.dll" => warnings.push(format!(
                "failed to preload bundled Microsoft C++ runtime: {error}"
            )),
            Err(_) => {}
        }
    }
}

#[cfg(not(windows))]
fn preload_windows_runtime(_vapoursynth_dir: &Path, _warnings: &mut Vec<String>) {}

impl RuntimePaths {
    pub fn from_root(root_dir: PathBuf) -> Self {
        Self {
            resource_dir: root_dir.join("resources"),
            binary_dir: root_dir.join("bin"),
            shader_dir: root_dir.join("shaders"),
            model_dir: root_dir.join("models"),
            root_dir,
        }
    }

    pub fn from_resource_dir(resource_dir: PathBuf) -> Self {
        let root_dir = resource_dir
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| resource_dir.clone());
        Self {
            binary_dir: resource_dir.join("mpv"),
            shader_dir: resource_dir.join("shaders"),
            model_dir: resource_dir.join("vapoursynth/vs-plugins/models"),
            resource_dir,
            root_dir,
        }
    }

    pub fn default_specs(&self) -> Vec<RuntimeResourceSpec> {
        let mpv_names: &[&str] = if cfg!(windows) {
            &["libmpv-2.dll", "libmpv-1.dll"]
        } else if cfg!(target_os = "macos") {
            &["libmpv.2.dylib", "libmpv.dylib"]
        } else {
            &["libmpv.so.2", "libmpv.so"]
        };
        let mut mpv_candidates = Vec::new();
        for name in mpv_names {
            mpv_candidates.push(self.resource_dir.join("mpv").join(name));
            mpv_candidates.push(self.resource_dir.join(name));
            mpv_candidates.push(self.binary_dir.join(name));
            mpv_candidates.push(self.root_dir.join(name));
        }

        vec![
            RuntimeResourceSpec::new(RuntimeResourceKind::LibMpv, "libmpv", mpv_candidates, true),
            RuntimeResourceSpec::new(
                RuntimeResourceKind::Ffmpeg,
                "ffmpeg",
                vec![
                    self.resource_dir.join("mpv/ffmpeg.exe"),
                    self.resource_dir.join(if cfg!(windows) {
                        "ffmpeg.dll"
                    } else {
                        "libffmpeg.so"
                    }),
                    self.binary_dir.join("ffmpeg.exe"),
                ],
                false,
            ),
            RuntimeResourceSpec::new(
                RuntimeResourceKind::Ffprobe,
                "ffprobe",
                vec![
                    self.resource_dir.join("mpv/ffprobe.exe"),
                    self.resource_dir.join("ffprobe.exe"),
                    self.binary_dir.join("ffprobe.exe"),
                ],
                false,
            ),
            RuntimeResourceSpec::new(
                RuntimeResourceKind::VapourSynth,
                "vapoursynth",
                vec![
                    self.resource_dir.join("vapoursynth/libvapoursynth.dll"),
                    self.resource_dir
                        .join("vapoursynth/runtime/libvapoursynth.dll"),
                    self.resource_dir.join("vapoursynth/vspipe.exe"),
                    self.resource_dir
                        .join("vapoursynth/python/vapoursynth/vspipe.exe"),
                    self.resource_dir.join(if cfg!(windows) {
                        "vapoursynth.dll"
                    } else {
                        "libvapoursynth.so"
                    }),
                    self.binary_dir.join("vspipe.exe"),
                ],
                false,
            ),
            RuntimeResourceSpec::new(
                RuntimeResourceKind::RifeModel,
                "rife",
                vec![
                    self.resource_dir
                        .join("vapoursynth/vs-plugins/models/rife_v2/rife_v4.25_lite.onnx"),
                    self.resource_dir
                        .join("vapoursynth/models/rife_v2/rife_v4.25_lite.onnx"),
                    self.model_dir.join("rife"),
                    self.model_dir.join("rife.onnx"),
                ],
                false,
            ),
            RuntimeResourceSpec::new(
                RuntimeResourceKind::Shader,
                "upscaling-shader",
                vec![
                    self.resource_dir.join("shaders/SSimDownscaler.glsl"),
                    self.resource_dir.join("shaders/ArtCNN.glsl"),
                    self.shader_dir.join("SSimDownscaler.glsl"),
                    self.shader_dir.join("ArtCNN.glsl"),
                ],
                false,
            ),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeProbe {
    pub paths: RuntimePaths,
    pub specs: Vec<RuntimeResourceSpec>,
}

impl RuntimeProbe {
    pub fn new(paths: RuntimePaths) -> Self {
        let specs = paths.default_specs();
        Self { paths, specs }
    }

    pub fn with_specs(paths: RuntimePaths, specs: Vec<RuntimeResourceSpec>) -> Self {
        Self { paths, specs }
    }

    pub fn probe(&self) -> RuntimeDiagnostics {
        let mut resources = Vec::with_capacity(self.specs.len());
        let mut warnings = Vec::new();
        let mut errors = Vec::new();

        for spec in &self.specs {
            let found = spec
                .candidates
                .iter()
                .find(|path| path.is_file() || path.is_dir());
            let (status, path, size_bytes) = match found {
                Some(path) => match fs::metadata(path) {
                    Ok(metadata) if metadata.is_file() || metadata.is_dir() => (
                        RuntimeResourceStatus::Present,
                        Some(path.clone()),
                        Some(metadata.len()),
                    ),
                    _ => (RuntimeResourceStatus::Invalid, Some(path.clone()), None),
                },
                None => (
                    RuntimeResourceStatus::Missing,
                    spec.candidates.first().cloned(),
                    None,
                ),
            };

            if status != RuntimeResourceStatus::Present {
                let message = format!(
                    "{} resource {}: {:?}",
                    if spec.required {
                        "required"
                    } else {
                        "optional"
                    },
                    spec.name,
                    status
                );
                if spec.required {
                    errors.push(message);
                } else {
                    warnings.push(message);
                }
            }
            resources.push(RuntimeResource {
                kind: spec.kind,
                name: spec.name.clone(),
                path,
                required: spec.required,
                status,
                size_bytes,
            });
        }

        let playback_available = resources.iter().any(|resource| {
            resource.kind == RuntimeResourceKind::LibMpv
                && resource.status == RuntimeResourceStatus::Present
        });
        let upscaling_available = resources.iter().any(|resource| {
            resource.kind == RuntimeResourceKind::Shader
                && resource.status == RuntimeResourceStatus::Present
        });
        // Live interpolation is implemented by the bundled mpv
        // display-resample path. It is available whenever playback is
        // available and does not depend on an external frame-generation app.
        let interpolation_available = playback_available;
        let enhancement_available = upscaling_available || interpolation_available;
        RuntimeDiagnostics {
            checked_at_ms: now_ms(),
            resources,
            playback_available,
            upscaling_available,
            interpolation_available,
            enhancement_available,
            warnings,
            errors,
        }
    }
}

pub fn probe_runtime(paths: RuntimePaths) -> RuntimeDiagnostics {
    RuntimeProbe::new(paths).probe()
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
    use std::fs;

    #[test]
    fn probe_reports_required_and_optional_resources() {
        let root = std::env::temp_dir().join(format!("ttv-runtime-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("resources")).unwrap();
        fs::write(
            root.join("resources").join(if cfg!(windows) {
                "libmpv-2.dll"
            } else {
                "libmpv.so.2"
            }),
            b"mock",
        )
        .unwrap();
        let diagnostics = probe_runtime(RuntimePaths::from_root(root.clone()));
        assert!(diagnostics.playback_available);
        assert!(diagnostics.interpolation_available);
        assert!(diagnostics.enhancement_available);
        assert!(diagnostics
            .resource(RuntimeResourceKind::LibMpv)
            .unwrap()
            .path
            .is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn custom_specs_support_directory_resources() {
        let root = std::env::temp_dir().join(format!("ttv-runtime-dir-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("models")).unwrap();
        let paths = RuntimePaths::from_root(root.clone());
        let specs = vec![RuntimeResourceSpec::new(
            RuntimeResourceKind::RifeModel,
            "model-dir",
            vec![root.join("models")],
            true,
        )];
        let diagnostics = RuntimeProbe::with_specs(paths, specs).probe();
        assert_eq!(
            diagnostics.resources[0].status,
            RuntimeResourceStatus::Present
        );
        assert!(diagnostics.errors.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn enhancement_plan_only_enables_existing_pipeline_files() {
        let root =
            std::env::temp_dir().join(format!("ttv-enhancement-plan-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("resources/vapoursynth/scripts")).unwrap();
        fs::create_dir_all(root.join("resources/shaders")).unwrap();
        fs::write(root.join("resources/shaders/ArtCNN.glsl"), b"shader").unwrap();
        fs::write(
            root.join("resources/vapoursynth/scripts/MEMC_RIFE_DML.vpy"),
            b"script",
        )
        .unwrap();
        let paths = RuntimePaths::from_root(root.clone());
        let diagnostics = RuntimeDiagnostics {
            checked_at_ms: 0,
            resources: vec![],
            playback_available: true,
            upscaling_available: true,
            interpolation_available: true,
            enhancement_available: true,
            warnings: vec![],
            errors: vec![],
        };
        let plan = diagnostics.enhancement_plan(3, &paths);
        assert!(plan.upscaling_enabled);
        assert!(plan.interpolation_enabled);
        assert_eq!(plan.shader_paths.len(), 1);
        assert_eq!(
            plan.interpolation_script.as_deref(),
            Some(
                root.join("resources/vapoursynth/scripts/MEMC_RIFE_DML.vpy")
                    .as_path()
            )
        );
        assert!(plan.interpolation_fallback_script.is_none());
        assert!(plan.ai_upscaling_script.is_none());
        assert_eq!(
            vapoursynth_mpv_filter(
                plan.interpolation_script.as_ref().unwrap(),
                4,
                2
            ),
            format!(
                "@ttv-interp:vapoursynth=file=[{}]:buffered-frames=4:concurrent-frames=2",
                root.join("resources/vapoursynth/scripts/MEMC_RIFE_DML.vpy")
                    .to_string_lossy()
                    .replace('\\', "/")
            )
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn interpolation_only_mode_falls_back_without_vapoursynth_scripts() {
        let root =
            std::env::temp_dir().join(format!("ttv-interpolation-plan-{}", uuid::Uuid::new_v4()));
        let diagnostics = RuntimeDiagnostics {
            checked_at_ms: 0,
            resources: vec![],
            playback_available: true,
            upscaling_available: false,
            interpolation_available: true,
            enhancement_available: true,
            warnings: vec![],
            errors: vec![],
        };

        let plan = diagnostics.enhancement_plan(2, &RuntimePaths::from_root(root.clone()));
        assert!(plan.interpolation_enabled);
        assert!(!plan.upscaling_enabled);
        assert!(plan.shader_paths.is_empty());
        assert!(plan.interpolation_script.is_none());
        assert!(plan.interpolation_fallback_script.is_none());
        assert!(plan.ai_upscaling_script.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn enhancement_modes_four_and_five_enable_ai_upscaling() {
        let root =
            std::env::temp_dir().join(format!("ttv-ai-enhancement-{}", uuid::Uuid::new_v4()));
        let resources = root.join("resources");
        fs::create_dir_all(resources.join("shaders")).unwrap();
        for name in [
            "SSimDownscaler.glsl",
            "ArtCNN.glsl",
            "adaptive-sharpen.glsl",
        ] {
            fs::write(resources.join("shaders").join(name), b"shader").unwrap();
        }

        let diagnostics = RuntimeDiagnostics {
            checked_at_ms: 0,
            resources: vec![],
            playback_available: true,
            upscaling_available: true,
            interpolation_available: true,
            enhancement_available: true,
            warnings: vec![],
            errors: vec![],
        };
        let paths = RuntimePaths::from_root(root.clone());

        let ai_only = diagnostics.enhancement_plan(4, &paths);
        assert!(ai_only.ai_upscaling_enabled);
        assert!(ai_only.upscaling_enabled);
        assert!(!ai_only.interpolation_enabled);
        assert_eq!(ai_only.shader_paths.len(), 3);
        assert!(ai_only.ai_upscaling_script.is_none());

        let combined = diagnostics.enhancement_plan(5, &paths);
        assert!(combined.ai_upscaling_enabled);
        assert!(combined.upscaling_enabled);
        assert!(combined.interpolation_enabled);
        assert_eq!(combined.shader_paths.len(), 3);
        assert!(combined.interpolation_script.is_none());
        assert!(combined.interpolation_fallback_script.is_none());
        assert!(combined.ai_upscaling_script.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn realtime_interpolation_requires_only_mpv_playback() {
        let root = std::env::temp_dir().join(format!("ttv-runtime-stack-{}", uuid::Uuid::new_v4()));
        let resources = root.join("resources");
        fs::create_dir_all(&resources).unwrap();
        let libmpv = resources.join(if cfg!(windows) {
            "libmpv-2.dll"
        } else {
            "libmpv.so.2"
        });
        fs::write(&libmpv, b"runtime").unwrap();

        let diagnostics = probe_runtime(RuntimePaths::from_root(root.clone()));
        assert!(diagnostics.interpolation_available);

        fs::remove_file(libmpv).unwrap();
        let unavailable = probe_runtime(RuntimePaths::from_root(root.clone()));
        assert!(!unavailable.interpolation_available);
        let _ = fs::remove_dir_all(root);
    }
}
