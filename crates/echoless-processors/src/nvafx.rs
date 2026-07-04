//! NVIDIA AFX / RTX AEC runtime discovery, preflight checks, and processor.
//!
//! The backend is optional and Windows-only. It dynamically loads the NVIDIA
//! Audio Effects runtime from an Echoless-managed runtime directory, so normal
//! users do not need CUDA Toolkit, TensorRT SDK, AFX SDK, or NGC CLI.

use std::env;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::{dsp::copy_or_zero, EchoProcessor, IoSpec, ProcessorStats};

pub const SDK_VERSION: &str = "2.1.0";
pub const RUNTIME_FILE_VERSION: &str = "2.1.0.9";
pub const MIN_DRIVER_VERSION: &str = "572.61";
pub const DEFAULT_ENV_VAR: &str = "ECHOLESS_NVAFX_RUNTIME_DIR";
pub const NVAFX_SAMPLE_RATE: u32 = 48_000;
pub const NVAFX_FRAME_SAMPLES: u32 = 480;

const COMMON_REQUIRED_FILES: &[&str] = &[
    "bin/NVAudioEffects.dll",
    "bin/cublas64_12.dll",
    "bin/cublasLt64_12.dll",
    "bin/cufft64_11.dll",
    "bin/libcrypto-3-x64.dll",
    "bin/nvinfer_10.dll",
    "features/nvafxaec/bin/nvafxaec.dll",
];

const VC_RUNTIME_DLLS: &[&str] = &["VCRUNTIME140.dll", "VCRUNTIME140_1.dll", "MSVCP140.dll"];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuArch {
    Turing,
    Ampere,
    Ada,
    Blackwell,
}

impl GpuArch {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Turing => "turing",
            Self::Ampere => "ampere",
            Self::Ada => "ada",
            Self::Blackwell => "blackwell",
        }
    }

    pub fn from_compute_capability(value: &str) -> Option<Self> {
        match normalize_compute_capability(value).as_str() {
            "75" => Some(Self::Turing),
            "80" | "86" => Some(Self::Ampere),
            "89" => Some(Self::Ada),
            "100" | "120" => Some(Self::Blackwell),
            _ => None,
        }
    }

    pub fn model_payload_path(self) -> PathBuf {
        PathBuf::from("features")
            .join("nvafxaec")
            .join("models")
            .join(self.as_str())
            .join("aec_48k.trtpkg")
    }

    pub fn model_asset_name(self) -> String {
        format!(
            "echoless-rtx-aec-model-win64-{SDK_VERSION}-{}-aec48.zip",
            self.as_str()
        )
    }
}

impl fmt::Display for GpuArch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct GpuInfo {
    pub name: String,
    pub driver_version: String,
    pub compute_capability: String,
    pub arch: Option<GpuArch>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warning,
    Missing,
    Unsupported,
}

impl CheckStatus {
    pub fn is_problem(&self) -> bool {
        matches!(self, Self::Missing | Self::Unsupported)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warn",
            Self::Missing => "missing",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    pub action: Option<String>,
}

impl DoctorCheck {
    fn ok(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Ok,
            detail: detail.into(),
            action: None,
        }
    }

    fn warning(
        name: impl Into<String>,
        detail: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Warning,
            detail: detail.into(),
            action: Some(action.into()),
        }
    }

    fn missing(
        name: impl Into<String>,
        detail: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Missing,
            detail: detail.into(),
            action: Some(action.into()),
        }
    }

    fn unsupported(
        name: impl Into<String>,
        detail: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Unsupported,
            detail: detail.into(),
            action: Some(action.into()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub runtime_dir: PathBuf,
    pub runtime_dir_source: String,
    pub gpus: Vec<GpuInfo>,
    pub selected_arch: Option<GpuArch>,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn ok(&self) -> bool {
        !self.checks.iter().any(|check| check.status.is_problem())
    }

    pub fn expected_model_asset(&self) -> Option<String> {
        self.selected_arch.map(GpuArch::model_asset_name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeErrorMode {
    Silence,
    Bypass,
}

impl RuntimeErrorMode {
    fn parse(value: Option<&toml::Value>) -> Result<Self> {
        let Some(value) = value else {
            return Ok(Self::Silence);
        };
        let Some(s) = value.as_str() else {
            bail!("on_runtime_error 必须是 silence 或 bypass");
        };
        match s.to_ascii_lowercase().as_str() {
            "silence" => Ok(Self::Silence),
            "bypass" => Ok(Self::Bypass),
            other => bail!("on_runtime_error 不支持 {other};可用: silence / bypass"),
        }
    }
}

#[derive(Clone, Debug)]
struct AecConfig {
    runtime_dir: Option<PathBuf>,
    model_path: Option<PathBuf>,
    intensity_ratio: f32,
    use_default_gpu: bool,
    disable_cuda_graph: bool,
    on_runtime_error: RuntimeErrorMode,
}

impl Default for AecConfig {
    fn default() -> Self {
        Self {
            runtime_dir: None,
            model_path: None,
            intensity_ratio: 1.0,
            use_default_gpu: true,
            disable_cuda_graph: false,
            on_runtime_error: RuntimeErrorMode::Silence,
        }
    }
}

impl AecConfig {
    fn from_params(params: &toml::Table) -> Result<Self> {
        let mut cfg = Self {
            runtime_dir: parse_path_param(params.get("runtime_dir")),
            model_path: parse_path_param(params.get("model_path")),
            ..Self::default()
        };
        if let Some(v) = params
            .get("intensity_ratio")
            .and_then(toml::Value::as_float)
        {
            cfg.intensity_ratio = v as f32;
        } else if let Some(v) = params
            .get("intensity_ratio")
            .and_then(toml::Value::as_integer)
        {
            cfg.intensity_ratio = v as f32;
        }
        if !cfg.intensity_ratio.is_finite() || cfg.intensity_ratio < 0.0 {
            bail!("intensity_ratio 必须是非负有限数");
        }
        if let Some(v) = params.get("use_default_gpu").and_then(toml::Value::as_bool) {
            cfg.use_default_gpu = v;
        }
        if let Some(v) = params
            .get("disable_cuda_graph")
            .and_then(toml::Value::as_bool)
        {
            cfg.disable_cuda_graph = v;
        }
        cfg.on_runtime_error = RuntimeErrorMode::parse(params.get("on_runtime_error"))?;
        Ok(cfg)
    }
}

fn parse_path_param(value: Option<&toml::Value>) -> Option<PathBuf> {
    value
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("auto"))
        .map(PathBuf::from)
}

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Debug)]
struct RuntimeSelection {
    runtime_dir: PathBuf,
    model_path: PathBuf,
    selected_arch: Option<GpuArch>,
}

#[derive(Default)]
pub struct NvidiaAfxAec {
    cfg: AecConfig,
    selection: Option<RuntimeSelection>,
    runtime_errors: u64,
    last_error: Option<String>,
    last_process_time_ms: f32,
    #[cfg(windows)]
    effect: Option<AfxAecEffect>,
}

impl NvidiaAfxAec {
    pub fn new() -> Self {
        Self::default()
    }

    fn current_stats(&self) -> ProcessorStats {
        let mut stats = ProcessorStats::empty("nvidia_afx_aec");
        stats.diverged = self.runtime_errors > 0 || self.last_error.is_some();
        stats.process_time_ms = self.last_process_time_ms;
        stats.runtime_error_count = self.runtime_errors;
        stats.selected_model = self
            .selection
            .as_ref()
            .map(|selection| selection.model_path.display().to_string());
        stats.selected_gpu_arch = self
            .selection
            .as_ref()
            .and_then(|selection| selection.selected_arch)
            .map(|arch| arch.as_str().to_string());
        stats.last_backend_error = self.last_error.clone();
        stats
    }

    fn handle_runtime_error(&mut self, err: impl Into<String>, near: &[f32], out: &mut [f32]) {
        self.runtime_errors = self.runtime_errors.saturating_add(1);
        self.last_error = Some(err.into());
        match self.cfg.on_runtime_error {
            RuntimeErrorMode::Silence => out.fill(0.0),
            RuntimeErrorMode::Bypass => copy_or_zero(near, out),
        }
    }
}

impl EchoProcessor for NvidiaAfxAec {
    fn name(&self) -> &'static str {
        "nvidia_afx_aec"
    }

    fn io_spec(&self) -> IoSpec {
        IoSpec {
            sample_rate: NVAFX_SAMPLE_RATE,
            near_channels: 1,
            far_channels: 1,
            algorithmic_latency_ms: 0.0,
        }
    }

    fn configure(&mut self, params: &toml::Table) -> Result<()> {
        self.cfg = AecConfig::from_params(params)?;
        self.runtime_errors = 0;
        self.last_error = None;
        self.last_process_time_ms = 0.0;

        let report = doctor_report(self.cfg.runtime_dir.as_deref())?;
        if !report.ok() {
            bail!("nvidia_afx_aec 尚未可用;请先运行 `echoless nvafx doctor` 修复依赖");
        }

        let selection = resolve_runtime_selection(
            self.cfg.runtime_dir.as_deref(),
            self.cfg.model_path.as_deref(),
        )?;

        #[cfg(not(windows))]
        {
            let _ = selection;
            bail!("nvidia_afx_aec 目前只支持 Windows x64");
        }

        #[cfg(windows)]
        {
            let effect =
                AfxAecEffect::new(&selection.runtime_dir, &selection.model_path, &self.cfg)?;
            self.effect = Some(effect);
            self.selection = Some(selection);
            Ok(())
        }
    }

    fn process(&mut self, near: &[f32], far: &[f32], out: &mut [f32], frames: u32) {
        if frames != NVAFX_FRAME_SAMPLES {
            self.handle_runtime_error(
                format!("RTX AEC v1 只支持 {NVAFX_FRAME_SAMPLES} samples/frame,实际 {frames}"),
                near,
                out,
            );
            return;
        }
        let frame = NVAFX_FRAME_SAMPLES as usize;
        if near.len() < frame || far.len() < frame || out.len() < frame {
            self.handle_runtime_error("RTX AEC 输入/输出缓冲长度不足", near, out);
            return;
        }

        #[cfg(not(windows))]
        {
            self.handle_runtime_error("RTX AEC backend 只支持 Windows", near, out);
        }

        #[cfg(windows)]
        {
            let Some(effect) = self.effect.as_mut() else {
                self.handle_runtime_error("RTX AEC effect 未初始化", near, out);
                return;
            };
            let started = Instant::now();
            match effect.run_frame(&near[..frame], &far[..frame], &mut out[..frame]) {
                Ok(()) => {
                    self.last_process_time_ms = started.elapsed().as_secs_f32() * 1000.0;
                    self.last_error = None;
                    if out.len() > frame {
                        out[frame..].fill(0.0);
                    }
                }
                Err(err) => {
                    self.last_process_time_ms = started.elapsed().as_secs_f32() * 1000.0;
                    self.handle_runtime_error(format!("{err:#}"), near, out);
                }
            }
        }
    }

    fn stats(&self) -> ProcessorStats {
        self.current_stats()
    }

    fn reset(&mut self) {
        #[cfg(windows)]
        if let Some(effect) = self.effect.as_mut() {
            if let Err(err) = effect.reset() {
                self.runtime_errors = self.runtime_errors.saturating_add(1);
                self.last_error = Some(format!("{err:#}"));
            }
        }
    }
}

pub fn doctor_report(runtime_dir_override: Option<&Path>) -> Result<DoctorReport> {
    let (runtime_dir, runtime_dir_source) = resolve_runtime_dir(runtime_dir_override);
    let mut checks = Vec::new();

    if cfg!(windows) {
        checks.extend(check_windows_system_dependencies());
    } else {
        checks.push(DoctorCheck::unsupported(
            "platform",
            "NVIDIA AFX AEC runtime 目前只支持 Windows x64",
            "在 Windows RTX 机器上使用 RTX AEC backend",
        ));
    }

    let gpus = detect_gpus().unwrap_or_else(|err| {
        checks.push(DoctorCheck::missing(
            "nvidia-smi",
            format!("无法运行 nvidia-smi: {err:#}"),
            "安装 NVIDIA graphics driver 572.61 或更新版本",
        ));
        Vec::new()
    });
    let selected_arch = gpus.iter().find_map(|gpu| gpu.arch);
    checks.extend(check_gpus(&gpus));
    checks.extend(check_runtime_files(&runtime_dir, selected_arch));

    if cfg!(windows) && !checks.iter().any(|check| check.status.is_problem()) {
        checks.extend(check_afx_smoke(&runtime_dir, selected_arch));
    }

    Ok(DoctorReport {
        runtime_dir,
        runtime_dir_source,
        gpus,
        selected_arch,
        checks,
    })
}

pub fn resolve_runtime_dir(override_dir: Option<&Path>) -> (PathBuf, String) {
    if let Some(dir) = override_dir {
        return (dir.to_path_buf(), "argument".to_string());
    }
    if let Some(dir) = env::var_os(DEFAULT_ENV_VAR).filter(|value| !value.is_empty()) {
        return (PathBuf::from(dir), DEFAULT_ENV_VAR.to_string());
    }

    let (base, source) = echoless_paths::brand_data_root();
    (base.join("nvafx").join(SDK_VERSION), source)
}

fn resolve_runtime_selection(
    runtime_dir_override: Option<&Path>,
    model_path_override: Option<&Path>,
) -> Result<RuntimeSelection> {
    let (runtime_dir, _runtime_dir_source) = resolve_runtime_dir(runtime_dir_override);
    let gpus = detect_gpus().context("检测 NVIDIA GPU 失败")?;
    let selected_arch = gpus.iter().find_map(|gpu| gpu.arch);
    let model_path = if let Some(path) = model_path_override {
        path.to_path_buf()
    } else {
        let arch = selected_arch.context("无法根据 GPU compute capability 选择 RTX AEC 模型")?;
        runtime_dir.join(arch.model_payload_path())
    };
    Ok(RuntimeSelection {
        runtime_dir,
        model_path,
        selected_arch,
    })
}

fn check_windows_system_dependencies() -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    for dll in VC_RUNTIME_DLLS {
        if let Some(path) = find_windows_system_dll(dll) {
            checks.push(DoctorCheck::ok(
                format!("vc-runtime:{dll}"),
                format!("Microsoft VC++ runtime 已存在: {}", path.display()),
            ));
        } else {
            checks.push(DoctorCheck::missing(
                format!("vc-runtime:{dll}"),
                "未找到 Microsoft VC++ runtime DLL",
                "安装 Microsoft Visual C++ 2015-2022 Redistributable x64",
            ));
        }
    }
    if let Some(path) = find_windows_system_dll("nvcuda.dll") {
        checks.push(DoctorCheck::ok(
            "nvcuda.dll",
            format!("CUDA driver DLL: {}", path.display()),
        ));
    } else {
        checks.push(DoctorCheck::missing(
            "nvcuda.dll",
            "未找到 NVIDIA CUDA driver DLL",
            "安装 NVIDIA graphics driver 572.61 或更新版本",
        ));
    }
    checks
}

fn find_windows_system_dll(name: &str) -> Option<PathBuf> {
    windows_system_dll_candidates(name)
        .into_iter()
        .find(|path| path.is_file())
}

fn windows_system_dll_candidates(name: &str) -> Vec<PathBuf> {
    windows_system_dll_candidates_from(name, env::var_os("SystemRoot"), env::var_os("PATH"))
}

fn windows_system_dll_candidates_from(
    name: &str,
    system_root: Option<std::ffi::OsString>,
    path_var: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(system_root) = system_root {
        let root = PathBuf::from(system_root);
        roots.push(root.join("System32"));
        roots.push(root.join("SysWOW64"));
    }
    if let Some(path) = path_var {
        roots.extend(env::split_paths(&path));
    }
    roots.into_iter().map(|root| root.join(name)).collect()
}

fn nvidia_smi_candidates() -> Vec<PathBuf> {
    nvidia_smi_candidates_from(
        env::var_os("ProgramFiles"),
        env::var_os("SystemRoot"),
        env::var_os("PATH"),
    )
}

fn nvidia_smi_candidates_from(
    program_files: Option<std::ffi::OsString>,
    system_root: Option<std::ffi::OsString>,
    path_var: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(program_files) = program_files {
        candidates.push(
            PathBuf::from(program_files)
                .join("NVIDIA Corporation")
                .join("NVSMI")
                .join("nvidia-smi.exe"),
        );
    }
    if let Some(system_root) = system_root {
        candidates.push(
            PathBuf::from(system_root)
                .join("System32")
                .join("nvidia-smi.exe"),
        );
    }
    if let Some(path) = path_var {
        for root in env::split_paths(&path) {
            candidates.push(root.join(if cfg!(windows) {
                "nvidia-smi.exe"
            } else {
                "nvidia-smi"
            }));
        }
    }
    candidates
}

fn resolve_nvidia_smi() -> PathBuf {
    nvidia_smi_candidates()
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("nvidia-smi"))
}

fn detect_gpus() -> Result<Vec<GpuInfo>> {
    let nvidia_smi = resolve_nvidia_smi();
    let output = Command::new(&nvidia_smi)
        .args([
            "--query-gpu=name,driver_version,compute_cap",
            "--format=csv,noheader",
        ])
        .output()
        .with_context(|| format!("运行 nvidia-smi 失败: {}", nvidia_smi.display()))?;
    if !output.status.success() {
        bail!(
            "nvidia-smi ({}) 退出码 {:?}: {}",
            nvidia_smi.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter_map(parse_nvidia_smi_gpu_line)
        .collect())
}

fn parse_nvidia_smi_gpu_line(line: &str) -> Option<GpuInfo> {
    let parts: Vec<_> = line.split(',').map(str::trim).collect();
    let [name, driver_version, compute_capability] = parts.as_slice() else {
        return None;
    };
    Some(GpuInfo {
        name: (*name).to_string(),
        driver_version: (*driver_version).to_string(),
        compute_capability: (*compute_capability).to_string(),
        arch: GpuArch::from_compute_capability(compute_capability),
    })
}

fn check_gpus(gpus: &[GpuInfo]) -> Vec<DoctorCheck> {
    if gpus.is_empty() {
        return vec![DoctorCheck::missing(
            "gpu",
            "未检测到 NVIDIA GPU",
            "确认机器有 RTX / Tensor Core GPU 并已安装 NVIDIA driver",
        )];
    }

    let mut checks = Vec::new();
    for (index, gpu) in gpus.iter().enumerate() {
        let label = format!("gpu:{index}");
        if compare_versions(&gpu.driver_version, MIN_DRIVER_VERSION).is_some_and(|ord| ord.is_lt())
        {
            checks.push(DoctorCheck::missing(
                format!("{label}:driver"),
                format!(
                    "{} driver={} 低于最低要求 {}",
                    gpu.name, gpu.driver_version, MIN_DRIVER_VERSION
                ),
                "更新 NVIDIA graphics driver",
            ));
        } else {
            checks.push(DoctorCheck::ok(
                format!("{label}:driver"),
                format!("{} driver={}", gpu.name, gpu.driver_version),
            ));
        }
        match gpu.arch {
            Some(arch) => checks.push(DoctorCheck::ok(
                format!("{label}:arch"),
                format!(
                    "{} compute_cap={} -> {arch}",
                    gpu.name, gpu.compute_capability
                ),
            )),
            None => checks.push(DoctorCheck::unsupported(
                format!("{label}:arch"),
                format!(
                    "{} compute_cap={} 不在支持列表",
                    gpu.name, gpu.compute_capability
                ),
                "RTX AEC backend 需要 Turing/Ampere/Ada/Blackwell 架构",
            )),
        }
    }
    checks
}

fn check_runtime_files(runtime_dir: &Path, selected_arch: Option<GpuArch>) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    if runtime_dir.is_dir() {
        checks.push(DoctorCheck::ok(
            "runtime-dir",
            format!("runtime 目录: {}", runtime_dir.display()),
        ));
    } else {
        checks.push(DoctorCheck::missing(
            "runtime-dir",
            format!("runtime 目录不存在: {}", runtime_dir.display()),
            "下载并解压 echoless-rtx-aec-common-runtime-win64-2.1.0.zip",
        ));
    }

    for rel in COMMON_REQUIRED_FILES {
        let path = runtime_dir.join(rel);
        if path.is_file() {
            checks.push(DoctorCheck::ok(
                format!("runtime:{rel}"),
                format!("found {}", path.display()),
            ));
        } else {
            checks.push(DoctorCheck::missing(
                format!("runtime:{rel}"),
                format!("missing {}", path.display()),
                "下载并解压 common runtime zip",
            ));
        }
    }

    match selected_arch {
        Some(arch) => {
            let rel = arch.model_payload_path();
            let path = runtime_dir.join(&rel);
            if path.is_file() {
                checks.push(DoctorCheck::ok(
                    "runtime:model",
                    format!("found {}", path.display()),
                ));
            } else {
                checks.push(DoctorCheck::missing(
                    "runtime:model",
                    format!("missing {}", path.display()),
                    format!("下载并解压 {}", arch.model_asset_name()),
                ));
            }
        }
        None => checks.push(DoctorCheck::warning(
            "runtime:model",
            "无法判断应使用哪个 AEC 模型",
            "先修复 GPU/driver 检测",
        )),
    }

    checks
}

fn check_afx_smoke(runtime_dir: &Path, selected_arch: Option<GpuArch>) -> Vec<DoctorCheck> {
    let Some(arch) = selected_arch else {
        return vec![DoctorCheck::warning(
            "afx-smoke",
            "跳过 AFX smoke check: 无法选择 GPU 架构",
            "先修复 GPU/driver 检测",
        )];
    };
    let model_path = runtime_dir.join(arch.model_payload_path());
    #[cfg(windows)]
    {
        match AfxAecEffect::new(runtime_dir, &model_path, &AecConfig::default()) {
            Ok(effect) => vec![
                DoctorCheck::ok(
                    "afx-smoke:load",
                    format!("已加载 NVAudioEffects.dll 并创建 AEC effect ({arch})"),
                ),
                DoctorCheck::ok(
                    "afx-smoke:devices",
                    format!("supported CUDA devices: {:?}", effect.supported_devices()),
                ),
                DoctorCheck::ok(
                    "afx-smoke:frame-sizes",
                    format!(
                        "supported frame sizes: {:?}",
                        effect.supported_frame_sizes()
                    ),
                ),
            ],
            Err(err) => vec![DoctorCheck::unsupported(
                "afx-smoke",
                format!("AFX AEC smoke check 失败: {err:#}"),
                "确认 runtime/model 与 GPU 架构匹配,并安装满足要求的 NVIDIA driver",
            )],
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (runtime_dir, model_path);
        vec![DoctorCheck::unsupported(
            "afx-smoke",
            "AFX smoke check 只在 Windows 执行",
            "在 Windows RTX 机器上运行 doctor",
        )]
    }
}

fn normalize_compute_capability(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect()
}

fn compare_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    let mut left_parts = parse_version_parts(left)?;
    let mut right_parts = parse_version_parts(right)?;
    let len = left_parts.len().max(right_parts.len());
    left_parts.resize(len, 0);
    right_parts.resize(len, 0);
    Some(left_parts.cmp(&right_parts))
}

fn parse_version_parts(value: &str) -> Option<Vec<u32>> {
    let parts: Vec<_> = value
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    (!parts.is_empty()).then_some(parts)
}

#[cfg(windows)]
mod ffi {
    use std::ffi::{c_char, c_float, c_int, c_uint, c_void, CString};
    use std::path::{Path, PathBuf};
    use std::ptr;

    use anyhow::{bail, Context, Result};
    use libloading::Library;

    use super::{AecConfig, NVAFX_FRAME_SAMPLES, NVAFX_SAMPLE_RATE};

    type NvafxStatus = c_int;
    type NvafxHandle = *mut c_void;

    const STATUS_SUCCESS: NvafxStatus = 0;
    const STATUS_OUTPUT_BUFFER_TOO_SMALL: NvafxStatus = 7;

    const EFFECT_AEC: &[u8] = b"aec\0";
    const PARAM_INPUT_SAMPLE_RATE: &[u8] = b"input_sample_rate\0";
    const PARAM_OUTPUT_SAMPLE_RATE: &[u8] = b"output_sample_rate\0";
    const PARAM_NUM_SAMPLES_PER_INPUT_FRAME: &[u8] = b"num_samples_per_input_frame\0";
    const PARAM_NUM_SAMPLES_PER_OUTPUT_FRAME: &[u8] = b"num_samples_per_output_frame\0";
    const PARAM_NUM_INPUT_CHANNELS: &[u8] = b"num_input_channels\0";
    const PARAM_NUM_OUTPUT_CHANNELS: &[u8] = b"num_output_channels\0";
    const PARAM_MODEL_PATH: &[u8] = b"model_path\0";
    const PARAM_INTENSITY_RATIO: &[u8] = b"intensity_ratio\0";
    const PARAM_USE_DEFAULT_GPU: &[u8] = b"use_default_gpu\0";
    const PARAM_DISABLE_CUDA_GRAPH: &[u8] = b"disable_cuda_graph\0";
    const PARAM_SUPPORTED_NUM_SAMPLES_PER_FRAME: &[u8] = b"supported_num_samples_per_frame\0";

    type CreateEffectFn =
        unsafe extern "C" fn(code: *const c_char, effect: *mut NvafxHandle) -> NvafxStatus;
    type DestroyEffectFn = unsafe extern "C" fn(effect: NvafxHandle) -> NvafxStatus;
    type SetU32Fn =
        unsafe extern "C" fn(effect: NvafxHandle, param: *const c_char, val: c_uint) -> NvafxStatus;
    type SetFloatFn = unsafe extern "C" fn(
        effect: NvafxHandle,
        param: *const c_char,
        val: c_float,
    ) -> NvafxStatus;
    type SetStringFn = unsafe extern "C" fn(
        effect: NvafxHandle,
        param: *const c_char,
        val: *const c_char,
    ) -> NvafxStatus;
    type GetU32Fn = unsafe extern "C" fn(
        effect: NvafxHandle,
        param: *const c_char,
        val: *mut c_uint,
    ) -> NvafxStatus;
    type GetU32ListFn = unsafe extern "C" fn(
        effect: NvafxHandle,
        param: *const c_char,
        list: *mut c_uint,
        list_size: *mut c_uint,
    ) -> NvafxStatus;
    type GetSupportedDevicesFn = unsafe extern "C" fn(
        effect: NvafxHandle,
        num: *mut c_int,
        devices: *mut c_int,
    ) -> NvafxStatus;
    type LoadFn = unsafe extern "C" fn(effect: NvafxHandle) -> NvafxStatus;
    type RunFn = unsafe extern "C" fn(
        effect: NvafxHandle,
        input: *const *const c_float,
        output: *mut *mut c_float,
        num_input_samples: c_uint,
        num_input_channels: c_uint,
    ) -> NvafxStatus;
    type ResetFn = unsafe extern "C" fn(effect: NvafxHandle) -> NvafxStatus;

    pub struct AfxAecEffect {
        api: AfxApi,
        handle: NvafxHandle,
        supported_devices: Vec<i32>,
        supported_frame_sizes: Vec<u32>,
    }

    unsafe impl Send for AfxAecEffect {}

    impl AfxAecEffect {
        pub fn new(runtime_dir: &Path, model_path: &Path, cfg: &AecConfig) -> Result<Self> {
            let api = AfxApi::load(runtime_dir)?;
            let mut handle = ptr::null_mut();
            api.check(
                unsafe { (api.create_effect)(EFFECT_AEC.as_ptr().cast(), &mut handle) },
                "NvAFX_CreateEffect(aec)",
            )?;
            if handle.is_null() {
                bail!("NvAFX_CreateEffect(aec) 返回空 handle");
            }

            let mut effect = Self {
                api,
                handle,
                supported_devices: Vec::new(),
                supported_frame_sizes: Vec::new(),
            };
            if let Err(err) = effect.configure_and_load(model_path, cfg) {
                let _ = effect.destroy();
                return Err(err);
            }
            Ok(effect)
        }

        pub fn supported_devices(&self) -> &[i32] {
            &self.supported_devices
        }

        pub fn supported_frame_sizes(&self) -> &[u32] {
            &self.supported_frame_sizes
        }

        pub fn run_frame(&mut self, near: &[f32], far: &[f32], out: &mut [f32]) -> Result<()> {
            let mut input = [near.as_ptr(), far.as_ptr()];
            let mut output = [out.as_mut_ptr()];
            self.api.check(
                unsafe {
                    (self.api.run)(
                        self.handle,
                        input.as_mut_ptr().cast_const(),
                        output.as_mut_ptr(),
                        NVAFX_FRAME_SAMPLES,
                        2,
                    )
                },
                "NvAFX_Run(aec)",
            )
        }

        pub fn reset(&mut self) -> Result<()> {
            self.api
                .check(unsafe { (self.api.reset)(self.handle) }, "NvAFX_Reset")
        }

        fn configure_and_load(&mut self, model_path: &Path, cfg: &AecConfig) -> Result<()> {
            if cfg.disable_cuda_graph {
                self.set_u32(PARAM_DISABLE_CUDA_GRAPH, 1)?;
            }
            if cfg.use_default_gpu {
                self.set_u32(PARAM_USE_DEFAULT_GPU, 1)?;
            }
            self.set_string(PARAM_MODEL_PATH, model_path)?;
            self.set_u32(PARAM_INPUT_SAMPLE_RATE, NVAFX_SAMPLE_RATE)?;
            self.set_u32(PARAM_OUTPUT_SAMPLE_RATE, NVAFX_SAMPLE_RATE)?;
            self.set_float(PARAM_INTENSITY_RATIO, cfg.intensity_ratio)?;

            self.supported_devices = self.get_supported_devices()?;
            self.supported_frame_sizes =
                self.get_u32_list(PARAM_SUPPORTED_NUM_SAMPLES_PER_FRAME)?;
            if !self.supported_frame_sizes.contains(&NVAFX_FRAME_SAMPLES) {
                bail!(
                    "RTX AEC 模型不支持 {} samples/frame;支持列表 {:?}",
                    NVAFX_FRAME_SAMPLES,
                    self.supported_frame_sizes
                );
            }

            self.api
                .check(unsafe { (self.api.load)(self.handle) }, "NvAFX_Load")?;

            let input_rate = self.get_u32(PARAM_INPUT_SAMPLE_RATE)?;
            let output_rate = self.get_u32(PARAM_OUTPUT_SAMPLE_RATE)?;
            let input_channels = self.get_u32(PARAM_NUM_INPUT_CHANNELS)?;
            let output_channels = self.get_u32(PARAM_NUM_OUTPUT_CHANNELS)?;
            let input_frame = self.get_u32(PARAM_NUM_SAMPLES_PER_INPUT_FRAME)?;
            let output_frame = self.get_u32(PARAM_NUM_SAMPLES_PER_OUTPUT_FRAME)?;
            if input_rate != NVAFX_SAMPLE_RATE
                || output_rate != NVAFX_SAMPLE_RATE
                || input_channels != 2
                || output_channels != 1
                || input_frame != NVAFX_FRAME_SAMPLES
                || output_frame != NVAFX_FRAME_SAMPLES
            {
                bail!(
                    "AFX AEC 属性不匹配: in_rate={input_rate}, out_rate={output_rate}, \
                     in_ch={input_channels}, out_ch={output_channels}, in_frame={input_frame}, out_frame={output_frame}"
                );
            }
            Ok(())
        }

        fn set_u32(&self, param: &[u8], value: u32) -> Result<()> {
            self.api.check(
                unsafe { (self.api.set_u32)(self.handle, param.as_ptr().cast(), value) },
                param_label("NvAFX_SetU32", param),
            )
        }

        fn set_float(&self, param: &[u8], value: f32) -> Result<()> {
            self.api.check(
                unsafe { (self.api.set_float)(self.handle, param.as_ptr().cast(), value) },
                param_label("NvAFX_SetFloat", param),
            )
        }

        fn set_string(&self, param: &[u8], value: &Path) -> Result<()> {
            let value = value
                .to_str()
                .with_context(|| format!("AFX path 不是 UTF-8: {}", value.display()))?;
            let value = CString::new(value).context("AFX path 含 NUL 字节")?;
            self.api.check(
                unsafe {
                    (self.api.set_string)(self.handle, param.as_ptr().cast(), value.as_ptr())
                },
                param_label("NvAFX_SetString", param),
            )
        }

        fn get_u32(&self, param: &[u8]) -> Result<u32> {
            let mut value = 0u32;
            self.api.check(
                unsafe { (self.api.get_u32)(self.handle, param.as_ptr().cast(), &mut value) },
                param_label("NvAFX_GetU32", param),
            )?;
            Ok(value)
        }

        fn get_u32_list(&self, param: &[u8]) -> Result<Vec<u32>> {
            let mut size = 0u32;
            let status = unsafe {
                (self.api.get_u32_list)(
                    self.handle,
                    param.as_ptr().cast(),
                    ptr::null_mut(),
                    &mut size,
                )
            };
            if status != STATUS_OUTPUT_BUFFER_TOO_SMALL {
                self.api
                    .check(status, param_label("NvAFX_GetU32List(size)", param))?;
            }
            let mut values = vec![0u32; size as usize];
            self.api.check(
                unsafe {
                    (self.api.get_u32_list)(
                        self.handle,
                        param.as_ptr().cast(),
                        values.as_mut_ptr(),
                        &mut size,
                    )
                },
                param_label("NvAFX_GetU32List", param),
            )?;
            values.truncate(size as usize);
            Ok(values)
        }

        fn get_supported_devices(&self) -> Result<Vec<i32>> {
            let mut size = 0i32;
            let status = unsafe {
                (self.api.get_supported_devices)(self.handle, &mut size, ptr::null_mut())
            };
            if status != STATUS_OUTPUT_BUFFER_TOO_SMALL {
                self.api.check(status, "NvAFX_GetSupportedDevices(size)")?;
            }
            let mut devices = vec![0i32; size.max(0) as usize];
            self.api.check(
                unsafe {
                    (self.api.get_supported_devices)(self.handle, &mut size, devices.as_mut_ptr())
                },
                "NvAFX_GetSupportedDevices",
            )?;
            devices.truncate(size.max(0) as usize);
            Ok(devices)
        }

        fn destroy(&mut self) -> Result<()> {
            if self.handle.is_null() {
                return Ok(());
            }
            let handle = std::mem::replace(&mut self.handle, ptr::null_mut());
            self.api.check(
                unsafe { (self.api.destroy_effect)(handle) },
                "NvAFX_DestroyEffect",
            )
        }
    }

    impl Drop for AfxAecEffect {
        fn drop(&mut self) {
            let _ = self.destroy();
        }
    }

    struct AfxApi {
        _library: Library,
        _dll_dirs: DllDirectoryGuard,
        create_effect: CreateEffectFn,
        destroy_effect: DestroyEffectFn,
        set_u32: SetU32Fn,
        set_float: SetFloatFn,
        set_string: SetStringFn,
        get_u32: GetU32Fn,
        get_u32_list: GetU32ListFn,
        get_supported_devices: GetSupportedDevicesFn,
        load: LoadFn,
        run: RunFn,
        reset: ResetFn,
    }

    unsafe impl Send for AfxApi {}

    impl AfxApi {
        fn load(runtime_dir: &Path) -> Result<Self> {
            let dll_dirs = DllDirectoryGuard::new(&[
                runtime_dir.join("bin"),
                runtime_dir.join("features").join("nvafxaec").join("bin"),
            ])?;
            let dll = runtime_dir.join("bin").join("NVAudioEffects.dll");
            let library = unsafe { Library::new(&dll) }
                .with_context(|| format!("加载 AFX DLL 失败: {}", dll.display()))?;
            Ok(Self {
                create_effect: unsafe { load_symbol(&library, b"NvAFX_CreateEffect\0")? },
                destroy_effect: unsafe { load_symbol(&library, b"NvAFX_DestroyEffect\0")? },
                set_u32: unsafe { load_symbol(&library, b"NvAFX_SetU32\0")? },
                set_float: unsafe { load_symbol(&library, b"NvAFX_SetFloat\0")? },
                set_string: unsafe { load_symbol(&library, b"NvAFX_SetString\0")? },
                get_u32: unsafe { load_symbol(&library, b"NvAFX_GetU32\0")? },
                get_u32_list: unsafe { load_symbol(&library, b"NvAFX_GetU32List\0")? },
                get_supported_devices: unsafe {
                    load_symbol(&library, b"NvAFX_GetSupportedDevices\0")?
                },
                load: unsafe { load_symbol(&library, b"NvAFX_Load\0")? },
                run: unsafe { load_symbol(&library, b"NvAFX_Run\0")? },
                reset: unsafe { load_symbol(&library, b"NvAFX_Reset\0")? },
                _library: library,
                _dll_dirs: dll_dirs,
            })
        }

        fn check(&self, status: NvafxStatus, operation: impl AsRef<str>) -> Result<()> {
            if status == STATUS_SUCCESS {
                Ok(())
            } else {
                bail!(
                    "{} failed: {} ({status})",
                    operation.as_ref(),
                    status_name(status)
                )
            }
        }
    }

    unsafe fn load_symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T> {
        Ok(*library
            .get::<T>(name)
            .with_context(|| format!("加载 AFX symbol 失败: {}", String::from_utf8_lossy(name)))?)
    }

    fn param_label(prefix: &'static str, param: &[u8]) -> String {
        let name = String::from_utf8_lossy(param);
        format!("{prefix}({})", name.trim_end_matches('\0'))
    }

    fn status_name(status: NvafxStatus) -> &'static str {
        match status {
            0 => "NVAFX_STATUS_SUCCESS",
            1 => "NVAFX_STATUS_FAILED",
            2 => "NVAFX_STATUS_INVALID_HANDLE",
            3 => "NVAFX_STATUS_INVALID_PARAM",
            4 => "NVAFX_STATUS_IMMUTABLE_PARAM",
            5 => "NVAFX_STATUS_INSUFFICIENT_DATA",
            6 => "NVAFX_STATUS_EFFECT_NOT_AVAILABLE",
            7 => "NVAFX_STATUS_OUTPUT_BUFFER_TOO_SMALL",
            8 => "NVAFX_STATUS_MODEL_LOAD_FAILED",
            9 => "NVAFX_STATUS_MODEL_NOT_LOADED",
            10 => "NVAFX_STATUS_INCOMPATIBLE_MODEL",
            11 => "NVAFX_STATUS_GPU_UNSUPPORTED",
            12 => "NVAFX_STATUS_NO_SUPPORTED_GPU_FOUND",
            13 => "NVAFX_STATUS_WRONG_GPU",
            14 => "NVAFX_STATUS_CUDA_ERROR",
            15 => "NVAFX_STATUS_INVALID_OPERATION",
            16 => "NVAFX_UNSUPPORTED_RUNTIME",
            17 => "NVAFX_STATUS_32_SERVER_NOT_REGISTERED",
            18 => "NVAFX_STATUS_32_COM_ERROR",
            19 => "NVAFX_STATUS_CUDA_CONTEXT_CREATION_FAILED",
            20 => "NVAFX_STATUS_LIBRARY_ERROR",
            21 => "NVAFX_STATUS_OUT_OF_MEMORY",
            22 => "NVAFX_STATUS_REFERENCE_AUDIO_NOT_SET",
            _ => "NVAFX_STATUS_UNKNOWN",
        }
    }

    struct DllDirectoryGuard {
        cookies: Vec<*mut c_void>,
        _paths: Vec<Vec<u16>>,
    }

    impl DllDirectoryGuard {
        fn new(paths: &[PathBuf]) -> Result<Self> {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::System::LibraryLoader::{
                AddDllDirectory, SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                LOAD_LIBRARY_SEARCH_USER_DIRS,
            };

            let ok = unsafe {
                SetDefaultDllDirectories(
                    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error())
                    .context("设置 Windows DLL 默认搜索路径失败");
            }

            let mut cookies = Vec::new();
            let mut wide_paths = Vec::new();
            for path in paths {
                let wide: Vec<u16> = path
                    .as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();
                let cookie = unsafe { AddDllDirectory(wide.as_ptr()) };
                if cookie.is_null() {
                    return Err(std::io::Error::last_os_error())
                        .with_context(|| format!("加入 DLL 搜索路径失败: {}", path.display()));
                }
                cookies.push(cookie);
                wide_paths.push(wide);
            }
            Ok(Self {
                cookies,
                _paths: wide_paths,
            })
        }
    }

    impl Drop for DllDirectoryGuard {
        fn drop(&mut self) {
            use windows_sys::Win32::System::LibraryLoader::RemoveDllDirectory;
            for cookie in self.cookies.drain(..) {
                unsafe {
                    RemoveDllDirectory(cookie);
                }
            }
        }
    }
}

#[cfg(windows)]
use ffi::AfxAecEffect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_compute_capability_to_arch() {
        assert_eq!(
            GpuArch::from_compute_capability("7.5"),
            Some(GpuArch::Turing)
        );
        assert_eq!(
            GpuArch::from_compute_capability("8.6"),
            Some(GpuArch::Ampere)
        );
        assert_eq!(GpuArch::from_compute_capability("8.9"), Some(GpuArch::Ada));
        assert_eq!(
            GpuArch::from_compute_capability("12.0"),
            Some(GpuArch::Blackwell)
        );
        assert_eq!(
            GpuArch::from_compute_capability("10.0"),
            Some(GpuArch::Blackwell)
        );
        assert_eq!(GpuArch::from_compute_capability("9.0"), None);
    }

    #[test]
    fn parses_nvidia_smi_line() {
        let gpu = parse_nvidia_smi_gpu_line("NVIDIA GeForce RTX 5080, 596.49, 12.0").unwrap();
        assert_eq!(gpu.name, "NVIDIA GeForce RTX 5080");
        assert_eq!(gpu.driver_version, "596.49");
        assert_eq!(gpu.arch, Some(GpuArch::Blackwell));
    }

    #[test]
    fn compares_driver_versions() {
        assert!(compare_versions("596.49", MIN_DRIVER_VERSION)
            .unwrap()
            .is_gt());
        assert!(compare_versions("572.61", MIN_DRIVER_VERSION)
            .unwrap()
            .is_eq());
        assert!(compare_versions("551.86", MIN_DRIVER_VERSION)
            .unwrap()
            .is_lt());
    }

    #[test]
    fn model_asset_and_payload_match_distribution() {
        let arch = GpuArch::Blackwell;
        assert_eq!(
            arch.model_asset_name(),
            "echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip"
        );
        assert_eq!(
            arch.model_payload_path(),
            PathBuf::from("features")
                .join("nvafxaec")
                .join("models")
                .join("blackwell")
                .join("aec_48k.trtpkg")
        );
    }

    #[test]
    fn parses_runtime_error_mode() {
        assert_eq!(
            RuntimeErrorMode::parse(None).unwrap(),
            RuntimeErrorMode::Silence
        );
        assert_eq!(
            RuntimeErrorMode::parse(Some(&toml::Value::String("bypass".into()))).unwrap(),
            RuntimeErrorMode::Bypass
        );
        assert!(RuntimeErrorMode::parse(Some(&toml::Value::String("fail".into()))).is_err());
    }

    #[test]
    fn ignores_auto_path_params() {
        let mut params = toml::Table::new();
        params.insert("runtime_dir".into(), toml::Value::String("auto".into()));
        params.insert("model_path".into(), toml::Value::String(" ".into()));
        let cfg = AecConfig::from_params(&params).unwrap();
        assert!(cfg.runtime_dir.is_none());
        assert!(cfg.model_path.is_none());
    }

    #[test]
    fn system_dll_candidates_prefer_system_root_before_path() {
        let system_root = PathBuf::from("/secure/SystemRoot");
        let untrusted = PathBuf::from("/untrusted");
        let path = env::join_paths([untrusted.clone(), system_root.join("System32")]).unwrap();
        let candidates = windows_system_dll_candidates_from(
            "nvcuda.dll",
            Some(system_root.clone().into_os_string()),
            Some(path),
        );

        assert_eq!(
            candidates[0],
            system_root.join("System32").join("nvcuda.dll")
        );
        assert_eq!(
            candidates[1],
            system_root.join("SysWOW64").join("nvcuda.dll")
        );
        assert!(candidates
            .iter()
            .position(|path| path == &untrusted.join("nvcuda.dll"))
            .is_some_and(|idx| idx > 1));
    }

    #[test]
    fn nvidia_smi_candidates_prefer_standard_install_paths_before_path() {
        let program_files = PathBuf::from("/secure/ProgramFiles");
        let system_root = PathBuf::from("/secure/SystemRoot");
        let untrusted = PathBuf::from("/untrusted");
        let path = env::join_paths([untrusted.clone()]).unwrap();
        let candidates = nvidia_smi_candidates_from(
            Some(program_files.clone().into_os_string()),
            Some(system_root.clone().into_os_string()),
            Some(path),
        );

        assert_eq!(
            candidates[0],
            program_files
                .join("NVIDIA Corporation")
                .join("NVSMI")
                .join("nvidia-smi.exe")
        );
        assert_eq!(
            candidates[1],
            system_root.join("System32").join("nvidia-smi.exe")
        );
        let path_candidate_name = if cfg!(windows) {
            "nvidia-smi.exe"
        } else {
            "nvidia-smi"
        };
        assert!(candidates
            .iter()
            .position(|path| path == &untrusted.join(path_candidate_name))
            .is_some_and(|idx| idx > 1));
    }
}
