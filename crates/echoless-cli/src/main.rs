//! echoless — 跨平台 reference-based AEC 工具 CLI。
//!
//! 当前可用:`processors` / `devices` / `offline` / `run` / `nvafx doctor`。
//! 实时主路径走 cpal;主线走经典 AEC3(sonora)保真,LocalVQE 作为独立可选处理器。

#[cfg(feature = "realtime")]
mod realtime;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::{create_dir_all, File};
use std::io::{copy, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use zip::ZipArchive;

use echoless_core::{
    apply_reference_channels_to_chain, run_offline, DiagnosticsConfig, PipelineConfig,
    ReferenceChannels,
};
use echoless_audio_io::file::{WavFileSink, WavFileSource};
use echoless_processors::{registry, NodeConfig};

#[derive(Parser)]
#[command(name = "echoless", about = "跨平台 reference-based AEC 工具", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 离线:mic.wav + ref.wav 经处理链 → out.wav
    Offline(OfflineArgs),
    /// 列出可用处理器种类
    Processors(ProcessorsArgs),
    /// 列出音频设备
    Devices(DevicesArgs),
    /// 配置文件工具
    Config(ConfigArgs),
    /// 实时运行
    Run(RunArgs),
    /// NVIDIA AFX / RTX AEC runtime 工具
    Nvafx(NvafxArgs),
}

#[derive(Args)]
struct ProcessorsArgs {
    /// 输出 JSON manifest,供 GUI 消费
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct DevicesArgs {
    /// 输出 JSON,供 GUI 消费
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ConfigArgs {
    #[command(subcommand)]
    cmd: ConfigCmd,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// 校验管线 TOML 配置
    Validate(ConfigValidateArgs),
}

#[derive(Args)]
struct ConfigValidateArgs {
    /// 管线 TOML 配置
    #[arg(long)]
    config: String,
    /// 输出结构化 JSON 结果
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct OfflineArgs {
    /// 近端麦克风 WAV
    #[arg(long)]
    mic: String,
    /// far-end 参考 WAV
    #[arg(long)]
    reference: String,
    /// 输出 WAV
    #[arg(long)]
    out: String,
    /// 处理链 TOML 配置(含 [[chain]]);给了则用其 chain/rate/frame_ms
    #[arg(long)]
    config: Option<String>,
    /// 快捷处理器 kind,如 "sonora_aec3" 或 "localvqe"
    #[arg(long)]
    chain: Option<String>,
    #[arg(long, default_value_t = 48000)]
    rate: u32,
    #[arg(long, default_value_t = 10)]
    frame_ms: u32,
}

#[derive(Args)]
struct RunArgs {
    /// 管线 TOML 配置;不给则从默认配置开始,再应用命令行覆盖
    #[arg(long)]
    config: Option<String>,
    /// 覆盖麦克风设备:default、索引或名称片段
    #[arg(long)]
    mic: Option<String>,
    /// 覆盖 far-end 参考源:system、none、output:<名>、input:<名>、索引或名称片段
    #[arg(long)]
    reference: Option<String>,
    /// 覆盖输出设备:default、索引或名称片段
    #[arg(long)]
    output: Option<String>,
    /// 覆盖采样率
    #[arg(long)]
    sample_rate: Option<u32>,
    /// 覆盖帧长(ms)
    #[arg(long)]
    frame_ms: Option<u32>,
    /// reference 送进 AEC 的声道模式:mono 或 stereo
    #[arg(long, value_parser = parse_reference_channels)]
    reference_channels: Option<ReferenceChannels>,
    /// 覆盖处理器,可重复或逗号分隔;默认建议 sonora_aec3
    #[arg(long, value_delimiter = ',')]
    processor: Vec<String>,
    /// 开启 sonora_aec3 降噪
    #[arg(long)]
    ns: bool,
    /// 关闭 sonora_aec3 降噪
    #[arg(long)]
    no_ns: bool,
    /// 覆盖 sonora_aec3 降噪强度:low/moderate/high/veryhigh
    #[arg(long)]
    ns_level: Option<String>,
    /// 覆盖 sonora_aec3 echo tail 长度(ms)
    #[arg(long)]
    tail_ms: Option<u32>,
    /// 每秒打印滚动实时统计
    #[arg(long)]
    verbose: bool,
    /// 自定义滚动统计间隔(ms);隐含 --verbose
    #[arg(long)]
    stats_interval_ms: Option<u64>,
    /// 输出 JSONL runtime status,供 GUI/sidecar 消费
    #[arg(long)]
    status_json: bool,
    /// 保存实时诊断录音的目录;会在其下创建 timestamp session
    #[arg(long)]
    diagnostic_dir: Option<String>,
    /// 诊断录制秒数上限;不给则录到停止
    #[arg(long)]
    diagnostic_seconds: Option<u32>,
}

#[derive(Args)]
struct NvafxArgs {
    #[command(subcommand)]
    cmd: NvafxCmd,
}

#[derive(Subcommand)]
enum NvafxCmd {
    /// 检查 RTX AEC runtime、GPU、driver、VC++ runtime 是否可用
    Doctor(NvafxDoctorArgs),
    /// 离线运行 RTX AEC:mic.wav + ref.wav → out.wav
    Offline(NvafxOfflineArgs),
    /// 从本地 zip 安装 Echoless RTX AEC runtime 与模型
    Install(NvafxInstallArgs),
}

#[derive(Args)]
struct NvafxDoctorArgs {
    /// 覆盖 runtime 根目录;Windows 默认读 ECHOLESS_NVAFX_RUNTIME_DIR,再退到 %LOCALAPPDATA%\Echoless\nvafx\2.1.0
    #[arg(long)]
    runtime_dir: Option<PathBuf>,
    /// 输出 JSON,供 GUI/installer 消费
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct NvafxOfflineArgs {
    /// 近端麦克风 WAV
    #[arg(long)]
    mic: String,
    /// far-end 参考 WAV
    #[arg(long)]
    reference: String,
    /// 输出 WAV
    #[arg(long)]
    out: String,
    /// 覆盖 runtime 根目录
    #[arg(long)]
    runtime_dir: Option<PathBuf>,
    /// 覆盖模型路径;默认按 GPU 架构自动选择
    #[arg(long)]
    model_path: Option<PathBuf>,
    /// AFX AEC 强度
    #[arg(long, default_value_t = 1.0)]
    intensity_ratio: f32,
}

#[derive(Args)]
struct NvafxInstallArgs {
    /// common runtime zip
    #[arg(long)]
    common_zip: PathBuf,
    /// 当前 GPU 架构对应的 model zip
    #[arg(long)]
    model_zip: PathBuf,
    /// 覆盖安装根目录;Windows 默认 %LOCALAPPDATA%\Echoless\nvafx\2.1.0
    #[arg(long)]
    runtime_dir: Option<PathBuf>,
    /// 覆盖 common zip 期望 SHA256;不填则按官方 asset 名称自动匹配
    #[arg(long)]
    common_sha256: Option<String>,
    /// 覆盖 model zip 期望 SHA256;不填则按官方 asset 名称自动匹配
    #[arg(long)]
    model_sha256: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Offline(a) => cmd_offline(a),
        Cmd::Processors(a) => cmd_processors(a),
        Cmd::Devices(a) => cmd_devices(a),
        Cmd::Config(a) => cmd_config(a),
        Cmd::Run(a) => cmd_run(a),
        Cmd::Nvafx(a) => cmd_nvafx(a),
    }
}

fn cmd_offline(a: OfflineArgs) -> Result<()> {
    let (rate, frame_ms, chain): (u32, u32, Vec<NodeConfig>) = if let Some(cfg_path) = &a.config {
        let s = std::fs::read_to_string(cfg_path)?;
        let pc: PipelineConfig = toml::from_str(&s)?;
        (pc.sample_rate, pc.frame_ms, pc.chain)
    } else {
        let chain = a
            .chain
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|k| NodeConfig {
                kind: k.to_string(),
                params: toml::Table::new(),
            })
            .collect();
        (a.rate, a.frame_ms, chain)
    };

    let cfg = PipelineConfig {
        mic: a.mic.clone(),
        reference: a.reference.clone(),
        output: a.out.clone(),
        sample_rate: rate,
        frame_ms,
        reference_channels: ReferenceChannels::Mono,
        diagnostics: DiagnosticsConfig::default(),
        chain,
    };
    validate_nvafx_constraints(&cfg)?;

    let frame = cfg.frame_size();
    let mic = WavFileSource::new(&a.mic, frame)?;
    let reference = WavFileSource::new(&a.reference, frame)?;
    let sink = WavFileSink::new(&a.out);

    let chain_desc = if cfg.chain.is_empty() {
        "直通(passthrough)".to_string()
    } else {
        cfg.chain
            .iter()
            .map(|n| n.kind.clone())
            .collect::<Vec<_>>()
            .join(" → ")
    };
    println!("离线运行: {} + {} → {}", a.mic, a.reference, a.out);
    println!(
        "采样率 {} Hz · 帧 {} ms · 链: {}",
        rate, frame_ms, chain_desc
    );

    let rep = run_offline(&cfg, mic, reference, sink)?;
    println!(
        "完成: {} 帧 (~{:.2}s) · 链 [{}] · 累计算法延迟 {:.1} ms",
        rep.frames,
        rep.seconds,
        rep.chain.join(", "),
        rep.total_latency_ms
    );
    for s in &rep.node_stats {
        println!(
            "  - {}: ERLE {:.1} dB, delay {} ms, process {:.2} ms, runtime_errors={}, diverged={}",
            s.name,
            s.erle_db,
            s.estimated_delay_ms,
            s.process_time_ms,
            s.runtime_error_count,
            s.diverged
        );
        if let Some(model) = &s.selected_model {
            println!("      model={model}");
        }
        if let Some(err) = &s.last_backend_error {
            println!("      last_error={err}");
        }
    }
    Ok(())
}

fn cmd_processors(args: ProcessorsArgs) -> Result<()> {
    if args.json {
        println!("{}", serde_json::to_string_pretty(&processor_manifest())?);
        return Ok(());
    }
    println!("可用处理器种类:");
    for k in registry::kinds() {
        println!("  - {k}");
    }
    println!("(在 --chain 或 config 的 [[chain]] 里按 kind 引用;默认建议 sonora_aec3)");
    Ok(())
}

fn processor_manifest() -> serde_json::Value {
    json!({
        "processors": [
            {
                "kind": "passthrough",
                "label": "Passthrough",
                "platforms": ["windows", "macos", "linux"],
                "default": false,
                "experimental": false,
                "diagnostic": true,
                "params": {}
            },
            {
                "kind": "sonora_aec3",
                "label": "AEC3",
                "platforms": ["windows", "macos", "linux"],
                "default": true,
                "experimental": false,
                "constraints": {
                    "preferred_sample_rate": 48000,
                    "preferred_frame_ms": 10
                },
                "params": {
                    "reference_channels": {
                        "type": "select",
                        "values": ["mono", "stereo"],
                        "default": "mono"
                    },
                    "ns": {
                        "type": "bool",
                        "default": false
                    },
                    "ns_level": {
                        "type": "select",
                        "values": ["low", "moderate", "high", "veryhigh"],
                        "default": "low",
                        "requires": { "ns": true }
                    },
                    "agc": {
                        "type": "bool",
                        "default": false,
                        "advanced": true
                    },
                    "initial_delay_ms": {
                        "type": "number",
                        "default": null,
                        "advanced": true
                    },
                    "tail_ms": {
                        "type": "number",
                        "default": null,
                        "min": 4,
                        "advanced": true
                    },
                    "delay_num_filters": {
                        "type": "number",
                        "default": null,
                        "min": 1,
                        "advanced": true
                    },
                    "linear_stable_echo_path": {
                        "type": "bool",
                        "default": false,
                        "advanced": true
                    }
                }
            },
            {
                "kind": "localvqe",
                "label": "LocalVQE",
                "platforms": ["windows", "macos", "linux"],
                "default": false,
                "experimental": true,
                "constraints": {
                    "native_sample_rate": 16000,
                    "native_channels": "mono",
                    "algorithmic_latency_ms": 16.0
                },
                "params": {
                    "model": { "type": "path", "required": true },
                    "library": { "type": "path", "required": false },
                    "backend": { "type": "string", "required": false, "advanced": true },
                    "device": { "type": "number", "required": false, "advanced": true },
                    "threads": { "type": "number", "min": 1, "required": false },
                    "noise_gate": { "type": "bool", "default": false },
                    "noise_gate_threshold_dbfs": {
                        "type": "number",
                        "default": -45.0,
                        "advanced": true
                    }
                }
            },
            {
                "kind": "nvidia_afx_aec",
                "label": "RTX AEC",
                "platforms": ["windows"],
                "default": false,
                "experimental": true,
                "requires_doctor_ok": true,
                "constraints": {
                    "sample_rate": 48000,
                    "frame_ms": 10,
                    "reference_channels": "mono"
                },
                "params": {
                    "runtime_dir": { "type": "path", "required": false },
                    "model_path": { "type": "path", "required": false },
                    "intensity_ratio": { "type": "number", "default": 1.0, "min": 0.0 },
                    "use_default_gpu": { "type": "bool", "default": true, "advanced": true },
                    "disable_cuda_graph": { "type": "bool", "default": false, "advanced": true },
                    "on_runtime_error": {
                        "type": "select",
                        "values": ["silence", "bypass"],
                        "default": "silence",
                        "advanced": true
                    }
                }
            }
        ]
    })
}

fn cmd_config(args: ConfigArgs) -> Result<()> {
    match args.cmd {
        ConfigCmd::Validate(a) => cmd_config_validate(a),
    }
}

fn cmd_config_validate(args: ConfigValidateArgs) -> Result<()> {
    let report = validate_config_file(&args.config);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    } else if report.ok {
        println!("配置校验通过: {}", args.config);
    } else {
        for error in &report.errors {
            eprintln!("{}: {}", error.path, error.message);
        }
    }

    if report.ok {
        Ok(())
    } else {
        bail!("配置校验失败: {} 个问题", report.errors.len())
    }
}

#[derive(Clone, Debug)]
struct ConfigValidationReport {
    ok: bool,
    errors: Vec<ConfigValidationError>,
}

impl ConfigValidationReport {
    fn new(errors: Vec<ConfigValidationError>) -> Self {
        Self {
            ok: errors.is_empty(),
            errors,
        }
    }

    fn to_json(&self) -> serde_json::Value {
        json!({
            "ok": self.ok,
            "errors": self.errors.iter().map(ConfigValidationError::to_json).collect::<Vec<_>>(),
        })
    }
}

#[derive(Clone, Debug)]
struct ConfigValidationError {
    path: String,
    message: String,
}

impl ConfigValidationError {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        json!({
            "path": self.path,
            "message": self.message,
        })
    }
}

fn validate_config_file(path: &str) -> ConfigValidationReport {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) => {
            return ConfigValidationReport::new(vec![ConfigValidationError::new(
                "config",
                format!("读取配置失败: {err}"),
            )])
        }
    };
    let cfg: PipelineConfig = match toml::from_str(&contents) {
        Ok(cfg) => cfg,
        Err(err) => {
            return ConfigValidationReport::new(vec![ConfigValidationError::new(
                "config",
                format!("解析 TOML 失败: {err}"),
            )])
        }
    };
    ConfigValidationReport::new(validate_pipeline_config(&cfg))
}

fn validate_pipeline_config(cfg: &PipelineConfig) -> Vec<ConfigValidationError> {
    let mut errors = Vec::new();
    if cfg.sample_rate == 0 {
        errors.push(ConfigValidationError::new(
            "sample_rate",
            "sample_rate must be greater than 0",
        ));
    }
    if cfg.frame_ms == 0 {
        errors.push(ConfigValidationError::new(
            "frame_ms",
            "frame_ms must be greater than 0",
        ));
    } else if cfg.sample_rate > 0
        && !(u64::from(cfg.sample_rate) * u64::from(cfg.frame_ms)).is_multiple_of(1000)
    {
        errors.push(ConfigValidationError::new(
            "frame_ms",
            "sample_rate * frame_ms must produce an integer sample count",
        ));
    }
    if cfg
        .diagnostics
        .record_dir
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        errors.push(ConfigValidationError::new(
            "diagnostics.record_dir",
            "record_dir must not be empty",
        ));
    }
    if matches!(cfg.diagnostics.max_seconds, Some(0)) {
        errors.push(ConfigValidationError::new(
            "diagnostics.max_seconds",
            "max_seconds must be greater than 0",
        ));
    }

    for (index, node) in cfg.chain.iter().enumerate() {
        validate_chain_node(cfg, index, node, &mut errors);
    }
    errors
}

fn validate_chain_node(
    cfg: &PipelineConfig,
    index: usize,
    node: &NodeConfig,
    errors: &mut Vec<ConfigValidationError>,
) {
    let base = format!("chain[{index}]");
    if !registry::kinds().contains(&node.kind.as_str()) {
        errors.push(ConfigValidationError::new(
            format!("{base}.kind"),
            format!(
                "unknown processor kind {}; available: {}",
                node.kind,
                registry::kinds().join(", ")
            ),
        ));
        return;
    }

    match node.kind.as_str() {
        "sonora_aec3" => validate_sonora_node(&base, &node.params, errors),
        "localvqe" => validate_localvqe_node(&base, &node.params, errors),
        "nvidia_afx_aec" => validate_nvafx_node(cfg, &base, &node.params, errors),
        "passthrough" => {}
        _ => {}
    }
}

fn validate_sonora_node(base: &str, params: &toml::Table, errors: &mut Vec<ConfigValidationError>) {
    expect_bool(params, base, "ns", errors);
    expect_bool(params, base, "agc", errors);
    expect_bool(params, base, "linear_stable_echo_path", errors);
    expect_i64(params, base, "initial_delay_ms", errors);
    expect_i64_min(params, base, "tail_ms", 4, errors);
    expect_i64_min(params, base, "delay_num_filters", 1, errors);
    expect_string_one_of(
        params,
        base,
        "ns_level",
        &[
            "low",
            "moderate",
            "high",
            "veryhigh",
            "very_high",
            "very-high",
        ],
        errors,
    );
    if let Some(value) = params.get("reference_channels") {
        let ok = value.as_integer().is_some_and(|v| matches!(v, 1 | 2))
            || value
                .as_str()
                .map(|s| {
                    matches!(
                        s.to_ascii_lowercase().as_str(),
                        "mono" | "1" | "1ch" | "stereo" | "2" | "2ch"
                    )
                })
                .unwrap_or(false);
        if !ok {
            errors.push(ConfigValidationError::new(
                format!("{base}.reference_channels"),
                "reference_channels must be mono, stereo, 1, or 2",
            ));
        }
    }
}

fn validate_localvqe_node(
    base: &str,
    params: &toml::Table,
    errors: &mut Vec<ConfigValidationError>,
) {
    expect_required_nonempty_string(params, base, "model", errors);
    expect_optional_nonempty_string(params, base, "library", errors);
    expect_optional_nonempty_string(params, base, "backend", errors);
    expect_i64(params, base, "device", errors);
    expect_i64_min(params, base, "threads", 1, errors);
    expect_bool(params, base, "noise_gate", errors);
    expect_finite_number(params, base, "noise_gate_threshold_dbfs", errors);
}

fn validate_nvafx_node(
    cfg: &PipelineConfig,
    base: &str,
    params: &toml::Table,
    errors: &mut Vec<ConfigValidationError>,
) {
    if cfg.sample_rate != echoless_processors::nvafx::NVAFX_SAMPLE_RATE {
        errors.push(ConfigValidationError::new(
            "sample_rate",
            format!(
                "nvidia_afx_aec requires {} Hz",
                echoless_processors::nvafx::NVAFX_SAMPLE_RATE
            ),
        ));
    }
    if cfg.frame_ms != 10 {
        errors.push(ConfigValidationError::new(
            "frame_ms",
            "nvidia_afx_aec requires 10ms frame",
        ));
    }
    if cfg.reference_channels != ReferenceChannels::Mono {
        errors.push(ConfigValidationError::new(
            "reference_channels",
            "nvidia_afx_aec requires mono reference",
        ));
    }
    expect_optional_nonempty_string(params, base, "runtime_dir", errors);
    expect_optional_nonempty_string(params, base, "model_path", errors);
    expect_finite_number_min(params, base, "intensity_ratio", 0.0, errors);
    expect_bool(params, base, "use_default_gpu", errors);
    expect_bool(params, base, "disable_cuda_graph", errors);
    expect_string_one_of(
        params,
        base,
        "on_runtime_error",
        &["silence", "bypass"],
        errors,
    );
}

fn expect_bool(
    params: &toml::Table,
    base: &str,
    key: &str,
    errors: &mut Vec<ConfigValidationError>,
) {
    if params
        .get(key)
        .is_some_and(|value| value.as_bool().is_none())
    {
        errors.push(ConfigValidationError::new(
            format!("{base}.{key}"),
            format!("{key} must be a boolean"),
        ));
    }
}

fn expect_i64(
    params: &toml::Table,
    base: &str,
    key: &str,
    errors: &mut Vec<ConfigValidationError>,
) {
    if params
        .get(key)
        .is_some_and(|value| value.as_integer().is_none())
    {
        errors.push(ConfigValidationError::new(
            format!("{base}.{key}"),
            format!("{key} must be an integer"),
        ));
    }
}

fn expect_i64_min(
    params: &toml::Table,
    base: &str,
    key: &str,
    min: i64,
    errors: &mut Vec<ConfigValidationError>,
) {
    if let Some(value) = params.get(key) {
        match value.as_integer() {
            Some(v) if v >= min => {}
            Some(_) => errors.push(ConfigValidationError::new(
                format!("{base}.{key}"),
                format!("{key} must be >= {min}"),
            )),
            None => errors.push(ConfigValidationError::new(
                format!("{base}.{key}"),
                format!("{key} must be an integer"),
            )),
        }
    }
}

fn expect_required_nonempty_string(
    params: &toml::Table,
    base: &str,
    key: &str,
    errors: &mut Vec<ConfigValidationError>,
) {
    match params.get(key).and_then(toml::Value::as_str) {
        Some(value) if !value.trim().is_empty() => {}
        Some(_) => errors.push(ConfigValidationError::new(
            format!("{base}.{key}"),
            format!("{key} must not be empty"),
        )),
        None => errors.push(ConfigValidationError::new(
            format!("{base}.{key}"),
            format!("{key} is required"),
        )),
    }
}

fn expect_optional_nonempty_string(
    params: &toml::Table,
    base: &str,
    key: &str,
    errors: &mut Vec<ConfigValidationError>,
) {
    if let Some(value) = params.get(key) {
        match value.as_str() {
            Some(s) if !s.trim().is_empty() => {}
            Some(_) => errors.push(ConfigValidationError::new(
                format!("{base}.{key}"),
                format!("{key} must not be empty"),
            )),
            None => errors.push(ConfigValidationError::new(
                format!("{base}.{key}"),
                format!("{key} must be a string"),
            )),
        }
    }
}

fn expect_string_one_of(
    params: &toml::Table,
    base: &str,
    key: &str,
    allowed: &[&str],
    errors: &mut Vec<ConfigValidationError>,
) {
    let Some(value) = params.get(key) else {
        return;
    };
    let Some(value) = value.as_str() else {
        errors.push(ConfigValidationError::new(
            format!("{base}.{key}"),
            format!("{key} must be a string"),
        ));
        return;
    };
    if !allowed
        .iter()
        .any(|allowed| value.eq_ignore_ascii_case(allowed))
    {
        errors.push(ConfigValidationError::new(
            format!("{base}.{key}"),
            format!("{key} must be one of: {}", allowed.join(", ")),
        ));
    }
}

fn expect_finite_number(
    params: &toml::Table,
    base: &str,
    key: &str,
    errors: &mut Vec<ConfigValidationError>,
) {
    if let Some(value) = params.get(key) {
        if toml_number_as_f64(value).is_none() {
            errors.push(ConfigValidationError::new(
                format!("{base}.{key}"),
                format!("{key} must be a finite number"),
            ));
        }
    }
}

fn expect_finite_number_min(
    params: &toml::Table,
    base: &str,
    key: &str,
    min: f64,
    errors: &mut Vec<ConfigValidationError>,
) {
    if let Some(value) = params.get(key) {
        match toml_number_as_f64(value) {
            Some(v) if v >= min => {}
            Some(_) => errors.push(ConfigValidationError::new(
                format!("{base}.{key}"),
                format!("{key} must be >= {min}"),
            )),
            None => errors.push(ConfigValidationError::new(
                format!("{base}.{key}"),
                format!("{key} must be a finite number"),
            )),
        }
    }
}

fn toml_number_as_f64(value: &toml::Value) -> Option<f64> {
    value
        .as_float()
        .or_else(|| value.as_integer().map(|value| value as f64))
        .filter(|value| value.is_finite())
}

fn cmd_nvafx(args: NvafxArgs) -> Result<()> {
    match args.cmd {
        NvafxCmd::Doctor(a) => cmd_nvafx_doctor(a),
        NvafxCmd::Offline(a) => cmd_nvafx_offline(a),
        NvafxCmd::Install(a) => cmd_nvafx_install(a),
    }
}

fn cmd_nvafx_doctor(args: NvafxDoctorArgs) -> Result<()> {
    let report = echoless_processors::nvafx::doctor_report(args.runtime_dir.as_deref())?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": report.ok(),
                "report": report,
            }))?
        );
        return Ok(());
    }

    println!("NVIDIA AFX / RTX AEC doctor");
    println!(
        "SDK {} · runtime file {} · 最低 driver {}",
        echoless_processors::nvafx::SDK_VERSION,
        echoless_processors::nvafx::RUNTIME_FILE_VERSION,
        echoless_processors::nvafx::MIN_DRIVER_VERSION,
    );
    println!(
        "Runtime: {} ({})",
        report.runtime_dir.display(),
        report.runtime_dir_source
    );
    if report.gpus.is_empty() {
        println!("GPU:     未检测到 NVIDIA GPU");
    } else {
        println!("GPU:");
        for (index, gpu) in report.gpus.iter().enumerate() {
            let arch = gpu
                .arch
                .map(|arch| arch.as_str().to_string())
                .unwrap_or_else(|| "unsupported".to_string());
            println!(
                "  [{index}] {} · driver {} · compute_cap {} · arch {}",
                gpu.name, gpu.driver_version, gpu.compute_capability, arch
            );
        }
    }
    if let Some(asset) = report.expected_model_asset() {
        println!("Model asset: {asset}");
    }
    println!();

    let mut problems = 0usize;
    for check in &report.checks {
        if check.status.is_problem() {
            problems += 1;
        }
        println!(
            "[{}] {} — {}",
            check.status.label(),
            check.name,
            check.detail
        );
        if let Some(action) = &check.action {
            println!("      处理: {action}");
        }
    }

    if problems == 0 {
        println!("\nRTX AEC runtime 预检通过。");
    } else {
        println!("\nRTX AEC runtime 预检未通过: {problems} 个问题需要处理。");
    }
    Ok(())
}

fn cmd_nvafx_offline(a: NvafxOfflineArgs) -> Result<()> {
    ensure_nvafx_windows_command("nvafx offline")?;
    if !a.intensity_ratio.is_finite() || a.intensity_ratio < 0.0 {
        bail!("--intensity-ratio 必须是非负有限数");
    }
    let mut params = toml::Table::new();
    if let Some(runtime_dir) = &a.runtime_dir {
        params.insert(
            "runtime_dir".into(),
            toml::Value::String(runtime_dir.display().to_string()),
        );
    }
    if let Some(model_path) = &a.model_path {
        params.insert(
            "model_path".into(),
            toml::Value::String(model_path.display().to_string()),
        );
    }
    params.insert(
        "intensity_ratio".into(),
        toml::Value::Float(a.intensity_ratio as f64),
    );

    let cfg = PipelineConfig {
        mic: a.mic.clone(),
        reference: a.reference.clone(),
        output: a.out.clone(),
        sample_rate: echoless_processors::nvafx::NVAFX_SAMPLE_RATE,
        frame_ms: 10,
        reference_channels: ReferenceChannels::Mono,
        diagnostics: DiagnosticsConfig::default(),
        chain: vec![NodeConfig {
            kind: "nvidia_afx_aec".into(),
            params,
        }],
    };
    validate_nvafx_constraints(&cfg)?;

    let frame = cfg.frame_size();
    let mic = WavFileSource::new(&a.mic, frame)?;
    let reference = WavFileSource::new(&a.reference, frame)?;
    let sink = WavFileSink::new(&a.out);
    println!("RTX AEC 离线运行: {} + {} → {}", a.mic, a.reference, a.out);
    let rep = run_offline(&cfg, mic, reference, sink)?;
    println!(
        "完成: {} 帧 (~{:.2}s) · process 链 [{}]",
        rep.frames,
        rep.seconds,
        rep.chain.join(", ")
    );
    for s in &rep.node_stats {
        println!(
            "  - {}: process {:.2} ms, runtime_errors={}, diverged={}",
            s.name, s.process_time_ms, s.runtime_error_count, s.diverged
        );
        if let Some(arch) = &s.selected_gpu_arch {
            println!("      arch={arch}");
        }
        if let Some(model) = &s.selected_model {
            println!("      model={model}");
        }
        if let Some(err) = &s.last_backend_error {
            println!("      last_error={err}");
        }
    }
    Ok(())
}

fn cmd_nvafx_install(a: NvafxInstallArgs) -> Result<()> {
    ensure_nvafx_windows_command("nvafx install")?;
    let (runtime_dir, runtime_dir_source) =
        echoless_processors::nvafx::resolve_runtime_dir(a.runtime_dir.as_deref());
    create_dir_all(&runtime_dir)
        .with_context(|| format!("创建 runtime 目录失败: {}", runtime_dir.display()))?;

    let common_expected = a
        .common_sha256
        .as_deref()
        .or_else(|| expected_sha256_for_asset(&a.common_zip));
    let model_expected = a
        .model_sha256
        .as_deref()
        .or_else(|| expected_sha256_for_asset(&a.model_zip));
    let common_hash = verify_zip_sha256(&a.common_zip, common_expected, "common runtime")?;
    let model_hash = verify_zip_sha256(&a.model_zip, model_expected, "model")?;

    println!("解压 common runtime 到 {}", runtime_dir.display());
    extract_zip(&a.common_zip, &runtime_dir)?;
    println!("解压 model 到 {}", runtime_dir.display());
    extract_zip(&a.model_zip, &runtime_dir)?;

    let installed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 UNIX_EPOCH")?
        .as_secs();
    let manifest = json!({
        "sdk_version": echoless_processors::nvafx::SDK_VERSION,
        "runtime_file_version": echoless_processors::nvafx::RUNTIME_FILE_VERSION,
        "installed_at_unix": installed_at,
        "runtime_dir_source": runtime_dir_source,
        "common_zip": a.common_zip.display().to_string(),
        "common_sha256": common_hash,
        "model_zip": a.model_zip.display().to_string(),
        "model_sha256": model_hash,
    });
    let manifest_path = runtime_dir.join("echoless-runtime-install-manifest.json");
    let mut file = File::create(&manifest_path)
        .with_context(|| format!("写入安装 manifest 失败: {}", manifest_path.display()))?;
    file.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    file.write_all(b"\n")?;

    println!("安装 manifest: {}", manifest_path.display());
    let report = echoless_processors::nvafx::doctor_report(Some(&runtime_dir))?;
    print_nvafx_doctor_report(&report);
    if !report.ok() {
        bail!("runtime 已解压,但 doctor 仍未通过");
    }
    Ok(())
}

fn ensure_nvafx_windows_command(command: &str) -> Result<()> {
    if !cfg!(windows) {
        bail!("{command} 目前只支持 Windows x64; macOS artifact 只能用于 AEC3/LocalVQE 路径");
    }
    Ok(())
}

fn print_nvafx_doctor_report(report: &echoless_processors::nvafx::DoctorReport) {
    println!("NVIDIA AFX / RTX AEC doctor");
    println!(
        "SDK {} · runtime file {} · 最低 driver {}",
        echoless_processors::nvafx::SDK_VERSION,
        echoless_processors::nvafx::RUNTIME_FILE_VERSION,
        echoless_processors::nvafx::MIN_DRIVER_VERSION,
    );
    println!(
        "Runtime: {} ({})",
        report.runtime_dir.display(),
        report.runtime_dir_source
    );
    if report.gpus.is_empty() {
        println!("GPU:     未检测到 NVIDIA GPU");
    } else {
        println!("GPU:");
        for (index, gpu) in report.gpus.iter().enumerate() {
            let arch = gpu
                .arch
                .map(|arch| arch.as_str().to_string())
                .unwrap_or_else(|| "unsupported".to_string());
            println!(
                "  [{index}] {} · driver {} · compute_cap {} · arch {}",
                gpu.name, gpu.driver_version, gpu.compute_capability, arch
            );
        }
    }
    if let Some(asset) = report.expected_model_asset() {
        println!("Model asset: {asset}");
    }
    println!();

    let mut problems = 0usize;
    for check in &report.checks {
        if check.status.is_problem() {
            problems += 1;
        }
        println!(
            "[{}] {} — {}",
            check.status.label(),
            check.name,
            check.detail
        );
        if let Some(action) = &check.action {
            println!("      处理: {action}");
        }
    }

    if problems == 0 {
        println!("\nRTX AEC runtime 预检通过。");
    } else {
        println!("\nRTX AEC runtime 预检未通过: {problems} 个问题需要处理。");
    }
}

fn expected_sha256_for_asset(path: &Path) -> Option<&'static str> {
    match path.file_name()?.to_str()? {
        "echoless-rtx-aec-common-runtime-win64-2.1.0.zip" => {
            Some("dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb")
        }
        "echoless-rtx-aec-model-win64-2.1.0-turing-aec48.zip" => {
            Some("951e03bb144156f4b27cbf2caa6930f9dabc3f1cb26a0afd9d9523f4d286dae9")
        }
        "echoless-rtx-aec-model-win64-2.1.0-ampere-aec48.zip" => {
            Some("066e06ec18a7d4509675411a1e050e11b0cfc4fee30d69d783871333018c9ab9")
        }
        "echoless-rtx-aec-model-win64-2.1.0-ada-aec48.zip" => {
            Some("92170e6a259f9093397b93cf4385759c36697ecb9e308322405bce1abcb8e3df")
        }
        "echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip" => {
            Some("0e75bb7442d317990ef0d5a6477105f86b9bbae1c2c5e4a6bdfb8d4e9f42df5b")
        }
        _ => None,
    }
}

fn verify_zip_sha256(path: &Path, expected: Option<&str>, label: &str) -> Result<String> {
    let actual = sha256_file(path)?;
    match expected {
        Some(expected) if actual.eq_ignore_ascii_case(expected) => {
            println!("{label} SHA256 ok: {actual}");
        }
        Some(expected) => bail!(
            "{label} SHA256 不匹配: actual={actual}, expected={expected}, file={}",
            path.display()
        ),
        None => {
            println!(
                "{label} SHA256: {actual} (未找到官方期望值,仅记录;建议传 --{}-sha256)",
                if label.starts_with("common") {
                    "common"
                } else {
                    "model"
                }
            );
        }
    }
    Ok(actual)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("打开文件失败: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("读取文件失败: {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file =
        File::open(zip_path).with_context(|| format!("打开 zip 失败: {}", zip_path.display()))?;
    let mut archive =
        ZipArchive::new(file).with_context(|| format!("读取 zip 失败: {}", zip_path.display()))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("读取 zip entry #{index} 失败: {}", zip_path.display()))?;
        let enclosed = entry
            .enclosed_name()
            .with_context(|| format!("zip entry 路径不安全: {}", entry.name()))?;
        let out_path = dest.join(enclosed);
        if entry.is_dir() {
            create_dir_all(&out_path)
                .with_context(|| format!("创建目录失败: {}", out_path.display()))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            create_dir_all(parent)
                .with_context(|| format!("创建目录失败: {}", parent.display()))?;
        }
        let mut out = File::create(&out_path)
            .with_context(|| format!("创建文件失败: {}", out_path.display()))?;
        copy(&mut entry, &mut out)
            .with_context(|| format!("解压文件失败: {}", out_path.display()))?;
    }
    Ok(())
}

#[cfg(feature = "realtime")]
fn cmd_devices(args: DevicesArgs) -> Result<()> {
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&realtime::devices_json()?)?
        );
        return Ok(());
    }
    realtime::print_devices()
}

#[cfg(not(feature = "realtime"))]
fn cmd_devices(args: DevicesArgs) -> Result<()> {
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": false,
                "error": "设备枚举需 realtime 特性(cpal);当前构建未启用。",
                "inputs": [],
                "outputs": [],
                "reference_sources": [
                    { "id": "system", "label": "System audio", "kind": "system" },
                    { "id": "none", "label": "No reference", "kind": "none" }
                ]
            }))?
        );
        return Ok(());
    }
    println!("设备枚举需 realtime 特性(cpal);当前构建未启用。");
    Ok(())
}

#[cfg(feature = "realtime")]
fn cmd_run(a: RunArgs) -> Result<()> {
    let cfg = load_run_config(&a)?;
    validate_nvafx_constraints(&cfg)?;
    let opts = runtime_options_from_args(&a)?;
    let run_config = format!(
        "实时运行配置: mic={} ref={} out={}",
        cfg.mic, cfg.reference, cfg.output
    );
    if opts.status_json {
        eprintln!("{run_config}");
    } else {
        println!("{run_config}");
    }
    realtime::run_with_options(&cfg, opts)
}

#[cfg(not(feature = "realtime"))]
fn cmd_run(_a: RunArgs) -> Result<()> {
    anyhow::bail!("实时管线需 realtime 特性(cpal);当前构建未启用")
}

#[cfg_attr(not(feature = "realtime"), allow(dead_code))]
fn load_run_config(a: &RunArgs) -> Result<PipelineConfig> {
    let cfg = if let Some(path) = &a.config {
        let s = std::fs::read_to_string(path)?;
        toml::from_str(&s)?
    } else {
        PipelineConfig::default()
    };
    apply_run_overrides(cfg, a)
}

#[cfg_attr(not(feature = "realtime"), allow(dead_code))]
fn apply_run_overrides(mut cfg: PipelineConfig, a: &RunArgs) -> Result<PipelineConfig> {
    if let Some(v) = &a.mic {
        cfg.mic = v.clone();
    }
    if let Some(v) = &a.reference {
        cfg.reference = v.clone();
    }
    if let Some(v) = &a.output {
        cfg.output = v.clone();
    }
    if let Some(v) = a.sample_rate {
        cfg.sample_rate = v;
    }
    if let Some(v) = a.frame_ms {
        cfg.frame_ms = v;
    }
    if let Some(v) = a.reference_channels {
        cfg.reference_channels = v;
    }
    if !a.processor.is_empty() {
        cfg.chain = a
            .processor
            .iter()
            .map(|kind| NodeConfig {
                kind: kind.clone(),
                params: toml::Table::new(),
            })
            .collect();
    }
    apply_reference_channels_to_chain(&mut cfg.chain, cfg.reference_channels);

    if a.ns && a.no_ns {
        bail!("--ns 与 --no-ns 不能同时使用");
    }
    if a.ns {
        set_sonora_param(&mut cfg.chain, "ns", toml::Value::Boolean(true))?;
    }
    if a.no_ns {
        set_sonora_param(&mut cfg.chain, "ns", toml::Value::Boolean(false))?;
    }
    if let Some(level) = &a.ns_level {
        set_sonora_param(&mut cfg.chain, "ns", toml::Value::Boolean(true))?;
        set_sonora_param(
            &mut cfg.chain,
            "ns_level",
            toml::Value::String(level.clone()),
        )?;
    }
    if let Some(tail_ms) = a.tail_ms {
        set_sonora_param(
            &mut cfg.chain,
            "tail_ms",
            toml::Value::Integer(tail_ms.into()),
        )?;
    }
    if let Some(dir) = &a.diagnostic_dir {
        if dir.trim().is_empty() {
            bail!("--diagnostic-dir 不能为空");
        }
        cfg.diagnostics.record_dir = Some(dir.clone());
    }
    if let Some(seconds) = a.diagnostic_seconds {
        if seconds == 0 {
            bail!("--diagnostic-seconds 必须大于 0");
        }
        cfg.diagnostics.max_seconds = Some(seconds);
    }

    Ok(cfg)
}

fn parse_reference_channels(s: &str) -> Result<ReferenceChannels, String> {
    match s.to_ascii_lowercase().as_str() {
        "mono" | "1" | "1ch" => Ok(ReferenceChannels::Mono),
        "stereo" | "2" | "2ch" => Ok(ReferenceChannels::Stereo),
        _ => Err("必须是 mono 或 stereo".to_string()),
    }
}

#[cfg_attr(not(feature = "realtime"), allow(dead_code))]
fn set_sonora_param(nodes: &mut [NodeConfig], key: &str, value: toml::Value) -> Result<()> {
    let Some(node) = nodes.iter_mut().find(|node| node.kind == "sonora_aec3") else {
        bail!("{key} 需要配置中存在 sonora_aec3 节点,或使用 --processor sonora_aec3");
    };
    node.params.insert(key.to_string(), value);
    Ok(())
}

fn validate_nvafx_constraints(cfg: &PipelineConfig) -> Result<()> {
    if !cfg.chain.iter().any(|node| node.kind == "nvidia_afx_aec") {
        return Ok(());
    }
    if cfg.sample_rate != echoless_processors::nvafx::NVAFX_SAMPLE_RATE {
        bail!(
            "nvidia_afx_aec v1 只支持 {} Hz,当前 sample_rate={}",
            echoless_processors::nvafx::NVAFX_SAMPLE_RATE,
            cfg.sample_rate
        );
    }
    if cfg.frame_ms != 10 {
        bail!(
            "nvidia_afx_aec v1 只支持 10ms frame,当前 frame_ms={}",
            cfg.frame_ms
        );
    }
    if cfg.reference_channels != ReferenceChannels::Mono {
        bail!("nvidia_afx_aec v1 只支持 mono reference;请设置 reference_channels = \"mono\"");
    }
    Ok(())
}

#[cfg(feature = "realtime")]
fn runtime_options_from_args(a: &RunArgs) -> Result<realtime::RuntimeOptions> {
    if matches!(a.stats_interval_ms, Some(0)) {
        bail!("--stats-interval-ms 必须大于 0");
    }
    Ok(realtime::RuntimeOptions {
        stats_interval_ms: a
            .stats_interval_ms
            .or_else(|| (a.verbose || a.status_json).then_some(1000)),
        status_json: a.status_json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_args() -> RunArgs {
        RunArgs {
            config: None,
            mic: None,
            reference: None,
            output: None,
            sample_rate: None,
            frame_ms: None,
            reference_channels: None,
            processor: Vec::new(),
            ns: false,
            no_ns: false,
            ns_level: None,
            tail_ms: None,
            verbose: false,
            stats_interval_ms: None,
            status_json: false,
            diagnostic_dir: None,
            diagnostic_seconds: None,
        }
    }

    #[test]
    fn run_overrides_replace_devices_and_chain() {
        let mut args = run_args();
        args.mic = Some("4".into());
        args.reference = Some("system".into());
        args.output = Some("CABLE Input".into());
        args.sample_rate = Some(48_000);
        args.frame_ms = Some(10);
        args.reference_channels = Some(echoless_core::ReferenceChannels::Stereo);
        args.processor = vec!["sonora_aec3".into()];
        args.ns_level = Some("high".into());
        args.tail_ms = Some(120);

        let cfg = apply_run_overrides(PipelineConfig::default(), &args).unwrap();

        assert_eq!(cfg.mic, "4");
        assert_eq!(cfg.reference, "system");
        assert_eq!(cfg.output, "CABLE Input");
        assert_eq!(cfg.sample_rate, 48_000);
        assert_eq!(cfg.frame_ms, 10);
        assert_eq!(
            cfg.reference_channels,
            echoless_core::ReferenceChannels::Stereo
        );
        assert_eq!(cfg.chain.len(), 1);
        assert_eq!(cfg.chain[0].kind, "sonora_aec3");
        assert_eq!(
            cfg.chain[0].params["reference_channels"].as_str(),
            Some("stereo")
        );
        assert_eq!(cfg.chain[0].params["ns"].as_bool(), Some(true));
        assert_eq!(cfg.chain[0].params["ns_level"].as_str(), Some("high"));
        assert_eq!(cfg.chain[0].params["tail_ms"].as_integer(), Some(120));
    }

    #[test]
    fn run_overrides_apply_diagnostics() {
        let mut args = run_args();
        args.diagnostic_dir = Some("diag".into());
        args.diagnostic_seconds = Some(30);

        let cfg = apply_run_overrides(PipelineConfig::default(), &args).unwrap();

        assert_eq!(cfg.diagnostics.record_dir.as_deref(), Some("diag"));
        assert_eq!(cfg.diagnostics.max_seconds, Some(30));
    }

    #[test]
    fn run_overrides_reject_sonora_flags_without_sonora_node() {
        let mut args = run_args();
        args.tail_ms = Some(120);

        let err = apply_run_overrides(PipelineConfig::default(), &args).unwrap_err();

        assert!(err.to_string().contains("sonora_aec3"));
    }

    #[test]
    fn run_overrides_reject_zero_diagnostic_seconds() {
        let mut args = run_args();
        args.diagnostic_seconds = Some(0);

        let err = apply_run_overrides(PipelineConfig::default(), &args).unwrap_err();

        assert!(err.to_string().contains("大于 0"));
    }

    #[test]
    #[cfg(feature = "realtime")]
    fn runtime_options_use_verbose_default_interval() {
        let mut args = run_args();
        args.verbose = true;

        let opts = runtime_options_from_args(&args).unwrap();

        assert_eq!(opts.stats_interval_ms, Some(1000));
        assert!(!opts.status_json);
    }

    #[test]
    #[cfg(feature = "realtime")]
    fn runtime_options_use_status_json_default_interval() {
        let mut args = run_args();
        args.status_json = true;

        let opts = runtime_options_from_args(&args).unwrap();

        assert_eq!(opts.stats_interval_ms, Some(1000));
        assert!(opts.status_json);
    }

    #[test]
    #[cfg(feature = "realtime")]
    fn runtime_options_reject_zero_interval() {
        let mut args = run_args();
        args.stats_interval_ms = Some(0);

        let err = runtime_options_from_args(&args).unwrap_err();

        assert!(err.to_string().contains("大于 0"));
    }

    #[test]
    fn processor_manifest_exposes_frontend_contract() {
        let manifest = processor_manifest();
        let processors = manifest["processors"].as_array().unwrap();

        let aec3 = processors
            .iter()
            .find(|processor| processor["kind"] == "sonora_aec3")
            .unwrap();

        assert_eq!(aec3["default"], true);
        assert_eq!(aec3["params"]["ns"]["default"], false);
        assert_eq!(
            aec3["params"]["reference_channels"]["values"],
            json!(["mono", "stereo"])
        );
    }

    #[test]
    fn config_validation_accepts_default_aec3_baseline() {
        let cfg = PipelineConfig {
            chain: vec![NodeConfig {
                kind: "sonora_aec3".into(),
                params: toml::Table::new(),
            }],
            ..PipelineConfig::default()
        };

        let errors = validate_pipeline_config(&cfg);

        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn config_validation_reports_frontend_safe_errors() {
        let mut bad_params = toml::Table::new();
        bad_params.insert("tail_ms".into(), toml::Value::Integer(1));
        bad_params.insert("ns".into(), toml::Value::String("yes".into()));
        let cfg = PipelineConfig {
            sample_rate: 44_100,
            reference_channels: ReferenceChannels::Stereo,
            chain: vec![
                NodeConfig {
                    kind: "sonora_aec3".into(),
                    params: bad_params,
                },
                NodeConfig {
                    kind: "nvidia_afx_aec".into(),
                    params: toml::Table::new(),
                },
                NodeConfig {
                    kind: "missing".into(),
                    params: toml::Table::new(),
                },
            ],
            ..PipelineConfig::default()
        };

        let errors = validate_pipeline_config(&cfg);
        let paths = errors
            .iter()
            .map(|error| error.path.as_str())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"chain[0].tail_ms"));
        assert!(paths.contains(&"chain[0].ns"));
        assert!(paths.contains(&"chain[2].kind"));
        assert!(paths.contains(&"sample_rate"));
        assert!(paths.contains(&"reference_channels"));
    }
}
