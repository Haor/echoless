//! echoless — 跨平台 reference-based AEC 工具 CLI。
//!
//! 当前可用:`processors` / `devices` / `doctor audio` / `offline` / `run` / `nvafx doctor/install/download-install`。
//! 实时主路径走 cpal;主线走经典 AEC3(sonora)保真,LocalVQE 作为独立可选处理器。

#[cfg(feature = "realtime")]
mod realtime;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::fs::{create_dir_all, remove_dir_all, File};
use std::io::{copy, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zip::ZipArchive;

use echoless_audio_io::file::{WavFileSink, WavFileSource};
use echoless_core::{
    apply_reference_channels_to_chain, default_near_delay_ms, default_output_level, run_offline,
    DiagnosticsConfig, PipelineConfig, ReferenceChannels, MAX_NEAR_DELAY_MS, MAX_OUTPUT_LEVEL,
    MIN_OUTPUT_LEVEL, OUTPUT_LEVEL_CURVE_EXPONENT, OUTPUT_LEVEL_MAX_BOOST_DB,
    OUTPUT_LEVEL_MAX_GAIN, UNITY_OUTPUT_LEVEL,
};
use echoless_processors::{registry, NodeConfig};

const DEFAULT_NVAFX_RELEASE_TAG: &str = "rtx-aec-runtime-win64-2.1.0-aec48-preview.1";
const NVAFX_RELEASE_DOWNLOAD_BASE: &str = "https://github.com/Haor/echoless/releases/download";
const NVAFX_COMMON_RUNTIME_ASSET: &str = "echoless-rtx-aec-common-runtime-win64-2.1.0.zip";

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
    /// 跨平台环境诊断
    Doctor(DoctorArgs),
    /// 配置文件工具
    Config(ConfigArgs),
    /// 实时运行
    Run(RunArgs),
    /// 主动侦测 reference 与 mic 的近端对齐延迟
    ProbeDelay(ProbeDelayArgs),
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
struct DoctorArgs {
    #[command(subcommand)]
    cmd: DoctorCmd,
}

#[derive(Subcommand)]
enum DoctorCmd {
    /// 检查虚拟音频设备、reference 可用性和音频权限状态
    Audio(DoctorAudioArgs),
}

#[derive(Args)]
struct DoctorAudioArgs {
    /// 输出 JSON,供 GUI onboarding 消费
    #[arg(long)]
    json: bool,
    /// macOS:显式触发一次系统音频录制权限请求/探测;不会在普通 doctor 中隐式弹窗
    #[arg(long)]
    request_system_audio: bool,
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
    /// 最终输出电平:0=静音,50=原声,100=3x 增益
    #[arg(long)]
    output_level: Option<u32>,
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
    /// near/mic 进入处理器前的人为对齐延迟(ms);macOS 默认 25,其他平台默认 0
    #[arg(long)]
    near_delay_ms: Option<u32>,
    /// 最终输出电平:0=静音,50=原声,100=3x 增益
    #[arg(long)]
    output_level: Option<u32>,
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
struct ProbeDelayArgs {
    /// 近端麦克风设备 selector
    #[arg(long, default_value = "MacBook Pro麦克风")]
    mic: String,
    /// reference selector;macOS 默认 system(Process Tap),Windows 默认 system(WASAPI loopback)
    #[arg(long, default_value = "system")]
    reference: String,
    /// Echoless 输出设备;建议虚拟音频设备,避免把处理后人声送回外放
    #[arg(long, default_value = "BlackHole 2ch")]
    output: String,
    /// 保留 diagnostics session 的输出目录;不传则使用临时目录并在分析后清理
    #[arg(long)]
    out_dir: Option<PathBuf>,
    /// 即使未指定 --out-dir,也保留本次 diagnostics session
    #[arg(long)]
    keep_session: bool,
    /// 开始播放蜂鸣前等待实时管线稳定的秒数
    #[arg(long, default_value_t = 4.0)]
    startup_delay: f64,
    /// 蜂鸣个数
    #[arg(long, default_value_t = 12)]
    beeps: u32,
    /// 蜂鸣音量(0.0-1.0)
    #[arg(long, default_value_t = 0.35)]
    volume: f32,
    /// 仅分析已有 diagnostics session
    #[arg(long)]
    analyze_only: Option<PathBuf>,
    /// 保留生成的蜂鸣 WAV
    #[arg(long)]
    keep_beep: Option<PathBuf>,
    /// 输出机器可读 JSON
    #[arg(long)]
    json: bool,
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
    /// 从 Echoless GitHub public release 下载并安装 RTX AEC runtime 与当前 GPU 模型
    DownloadInstall(NvafxDownloadInstallArgs),
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

#[derive(Args)]
struct NvafxDownloadInstallArgs {
    /// 覆盖安装根目录;Windows 默认 %LOCALAPPDATA%\Echoless\nvafx\2.1.0
    #[arg(long)]
    runtime_dir: Option<PathBuf>,
    /// GitHub release tag;默认使用 Echoless RTX AEC public preview release
    #[arg(long, default_value = DEFAULT_NVAFX_RELEASE_TAG)]
    tag: String,
    /// 输出 { ok, report } JSON,供 GUI installer 消费
    #[arg(long)]
    json: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Offline(a) => cmd_offline(a),
        Cmd::Processors(a) => cmd_processors(a),
        Cmd::Devices(a) => cmd_devices(a),
        Cmd::Doctor(a) => cmd_doctor(a),
        Cmd::Config(a) => cmd_config(a),
        Cmd::Run(a) => cmd_run(a),
        Cmd::ProbeDelay(a) => cmd_probe_delay(a),
        Cmd::Nvafx(a) => cmd_nvafx(a),
    }
}

fn cmd_offline(a: OfflineArgs) -> Result<()> {
    let (rate, frame_ms, config_output_level, chain): (u32, u32, u32, Vec<NodeConfig>) =
        if let Some(cfg_path) = &a.config {
            let s = std::fs::read_to_string(cfg_path)?;
            let pc: PipelineConfig = toml::from_str(&s)?;
            (pc.sample_rate, pc.frame_ms, pc.output_level, pc.chain)
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
            (a.rate, a.frame_ms, default_output_level(), chain)
        };
    let output_level = a.output_level.unwrap_or(config_output_level);

    let cfg = PipelineConfig {
        mic: a.mic.clone(),
        reference: a.reference.clone(),
        output: a.out.clone(),
        sample_rate: rate,
        frame_ms,
        reference_channels: ReferenceChannels::Mono,
        near_delay_ms: 0,
        output_level,
        diagnostics: DiagnosticsConfig::default(),
        chain,
    };
    let validation_errors = validate_pipeline_config(&cfg);
    if let Some(error) = validation_errors.first() {
        bail!("配置无效: {}: {}", error.path, error.message);
    }
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
        "采样率 {} Hz · 帧 {} ms · output_level={} · 链: {}",
        rate, frame_ms, output_level, chain_desc
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
        "pipeline": {
            "params": {
                "sample_rate": { "type": "number", "default": 48000 },
                "frame_ms": { "type": "number", "default": 10 },
                "reference_channels": {
                    "type": "select",
                    "values": ["mono", "stereo"],
                    "default": "mono"
                },
                "near_delay_ms": {
                    "type": "number",
                    "default": default_near_delay_ms(),
                    "min": 0,
                    "max": MAX_NEAR_DELAY_MS,
                    "advanced": true,
                    "calibratable": true
                },
                "output_level": {
                    "type": "number",
                    "default": default_output_level(),
                    "min": MIN_OUTPUT_LEVEL,
                    "max": MAX_OUTPUT_LEVEL,
                    "unity": UNITY_OUTPUT_LEVEL,
                    "mute": MIN_OUTPUT_LEVEL,
                    "curve": "power",
                    "exponent": OUTPUT_LEVEL_CURVE_EXPONENT,
                    "max_gain": OUTPUT_LEVEL_MAX_GAIN,
                    "max_boost_db": OUTPUT_LEVEL_MAX_BOOST_DB
                }
            }
        },
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
    let value: toml::Value = match toml::from_str(&contents) {
        Ok(cfg) => cfg,
        Err(err) => {
            return ConfigValidationReport::new(vec![ConfigValidationError::new(
                "config",
                format!("解析 TOML 失败: {err}"),
            )])
        }
    };
    let shape_errors = validate_config_shape(&value);
    if !shape_errors.is_empty() {
        return ConfigValidationReport::new(shape_errors);
    }
    let cfg: PipelineConfig = match value.try_into() {
        Ok(cfg) => cfg,
        Err(err) => {
            return ConfigValidationReport::new(vec![ConfigValidationError::new(
                "config",
                format!("解析配置失败: {err}"),
            )])
        }
    };
    ConfigValidationReport::new(validate_pipeline_config(&cfg))
}

fn validate_config_shape(value: &toml::Value) -> Vec<ConfigValidationError> {
    let mut errors = Vec::new();
    let Some(table) = value.as_table() else {
        return vec![ConfigValidationError::new(
            "config",
            "config must be a TOML table",
        )];
    };
    expect_top_string(table, "mic", &mut errors);
    expect_top_string(table, "reference", &mut errors);
    expect_top_string(table, "output", &mut errors);
    expect_top_i64(table, "sample_rate", &mut errors);
    expect_top_i64(table, "frame_ms", &mut errors);
    if let Some(value) = table.get("near_delay_ms") {
        match value.as_integer() {
            Some(v) if (0..=i64::from(MAX_NEAR_DELAY_MS)).contains(&v) => {}
            Some(_) => errors.push(ConfigValidationError::new(
                "near_delay_ms",
                format!("near_delay_ms must be between 0 and {MAX_NEAR_DELAY_MS}"),
            )),
            None => errors.push(ConfigValidationError::new(
                "near_delay_ms",
                "near_delay_ms must be an integer",
            )),
        }
    }
    if let Some(value) = table.get("output_level") {
        match value.as_integer() {
            Some(v) if (i64::from(MIN_OUTPUT_LEVEL)..=i64::from(MAX_OUTPUT_LEVEL)).contains(&v) => {
            }
            Some(_) => errors.push(ConfigValidationError::new(
                "output_level",
                format!("output_level must be between {MIN_OUTPUT_LEVEL} and {MAX_OUTPUT_LEVEL}"),
            )),
            None => errors.push(ConfigValidationError::new(
                "output_level",
                "output_level must be an integer",
            )),
        }
    }
    if let Some(value) = table.get("reference_channels") {
        match value.as_str() {
            Some(value) if matches!(value.to_ascii_lowercase().as_str(), "mono" | "stereo") => {}
            Some(_) => errors.push(ConfigValidationError::new(
                "reference_channels",
                "reference_channels must be mono or stereo",
            )),
            None => errors.push(ConfigValidationError::new(
                "reference_channels",
                "reference_channels must be a string",
            )),
        }
    }
    if let Some(value) = table.get("diagnostics") {
        if let Some(diagnostics) = value.as_table() {
            expect_top_string(diagnostics, "diagnostics.record_dir", &mut errors);
            expect_top_i64(diagnostics, "diagnostics.max_seconds", &mut errors);
        } else {
            errors.push(ConfigValidationError::new(
                "diagnostics",
                "diagnostics must be a table",
            ));
        }
    }
    if let Some(value) = table.get("chain") {
        let Some(nodes) = value.as_array() else {
            errors.push(ConfigValidationError::new(
                "chain",
                "chain must be an array of tables",
            ));
            return errors;
        };
        for (index, node) in nodes.iter().enumerate() {
            let base = format!("chain[{index}]");
            let Some(node) = node.as_table() else {
                errors.push(ConfigValidationError::new(
                    base,
                    "chain entry must be a table",
                ));
                continue;
            };
            match node.get("kind").and_then(toml::Value::as_str) {
                Some(kind) if !kind.trim().is_empty() => {}
                Some(_) => errors.push(ConfigValidationError::new(
                    format!("{base}.kind"),
                    "kind must not be empty",
                )),
                None if node.contains_key("kind") => errors.push(ConfigValidationError::new(
                    format!("{base}.kind"),
                    "kind must be a string",
                )),
                None => errors.push(ConfigValidationError::new(
                    format!("{base}.kind"),
                    "kind is required",
                )),
            }
        }
    }
    errors
}

fn expect_top_string(table: &toml::Table, key: &str, errors: &mut Vec<ConfigValidationError>) {
    if table
        .get(key.rsplit('.').next().unwrap_or(key))
        .is_some_and(|value| value.as_str().is_none())
    {
        errors.push(ConfigValidationError::new(
            key,
            format!("{key} must be a string"),
        ));
    }
}

fn expect_top_i64(table: &toml::Table, key: &str, errors: &mut Vec<ConfigValidationError>) {
    if table
        .get(key.rsplit('.').next().unwrap_or(key))
        .is_some_and(|value| value.as_integer().is_none())
    {
        errors.push(ConfigValidationError::new(
            key,
            format!("{key} must be an integer"),
        ));
    }
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
    if cfg.near_delay_ms > MAX_NEAR_DELAY_MS {
        errors.push(ConfigValidationError::new(
            "near_delay_ms",
            format!("near_delay_ms must be <= {MAX_NEAR_DELAY_MS}"),
        ));
    }
    if cfg.output_level > MAX_OUTPUT_LEVEL {
        errors.push(ConfigValidationError::new(
            "output_level",
            format!("output_level must be <= {MAX_OUTPUT_LEVEL}"),
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
    let runtime_dir = params
        .get("runtime_dir")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("auto"))
        .map(Path::new);
    match echoless_processors::nvafx::doctor_report(runtime_dir) {
        Ok(report) if report.ok() => {}
        Ok(report) => {
            let detail = report
                .checks
                .iter()
                .find(|check| check.status.is_problem())
                .map(|check| format!("{}: {}", check.name, check.detail))
                .unwrap_or_else(|| "doctor did not pass".to_string());
            errors.push(ConfigValidationError::new(
                format!("{base}.doctor"),
                format!("nvidia_afx_aec doctor failed: {detail}"),
            ));
        }
        Err(err) => errors.push(ConfigValidationError::new(
            format!("{base}.doctor"),
            format!("nvidia_afx_aec doctor failed: {err:#}"),
        )),
    }
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
        NvafxCmd::DownloadInstall(a) => cmd_nvafx_download_install(a),
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
        near_delay_ms: 0,
        output_level: default_output_level(),
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
    let report = install_nvafx_runtime(NvafxInstallRequest {
        common_zip: &a.common_zip,
        model_zip: &a.model_zip,
        runtime_dir: a.runtime_dir.as_deref(),
        common_sha256: a.common_sha256.as_deref(),
        model_sha256: a.model_sha256.as_deref(),
        install_source: json!({ "kind": "local-zip" }),
        log_to_stderr: false,
    })?;
    print_nvafx_doctor_report(&report);
    if !report.ok() {
        bail!("runtime 已解压,但 doctor 仍未通过");
    }
    Ok(())
}

fn cmd_nvafx_download_install(a: NvafxDownloadInstallArgs) -> Result<()> {
    ensure_nvafx_windows_command("nvafx download-install")?;
    let tag = a.tag.trim();
    if tag.is_empty() {
        bail!("--tag 不能为空");
    }

    let preflight = echoless_processors::nvafx::doctor_report(a.runtime_dir.as_deref())?;
    let arch = preflight.selected_arch.with_context(|| {
        "无法从 nvafx doctor 判断 GPU 架构;请先确认 nvidia-smi、driver 和 RTX GPU 可用"
    })?;
    let model_asset = arch.model_asset_name();
    let download_dir = nvafx_download_cache_dir(tag);
    create_dir_all(&download_dir)
        .with_context(|| format!("创建下载缓存目录失败: {}", download_dir.display()))?;

    install_log(
        a.json,
        format!(
            "RTX AEC 下载源: GitHub release {tag} · arch={}",
            arch.as_str()
        ),
    );

    let release_sha256sums = match fetch_release_sha256sums(tag, &download_dir, a.json) {
        Ok(sums) => sums,
        Err(err) => {
            install_log(
                a.json,
                format!("读取 SHA256SUMS.txt 失败: {err:#}; 将使用内置哈希或仅记录实际哈希"),
            );
            HashMap::new()
        }
    };

    let common_zip = download_dir.join(NVAFX_COMMON_RUNTIME_ASSET);
    let model_zip = download_dir.join(&model_asset);
    let common_url = nvafx_release_asset_url(tag, NVAFX_COMMON_RUNTIME_ASSET);
    let model_url = nvafx_release_asset_url(tag, &model_asset);
    let common_expected =
        expected_sha256_for_release_asset(tag, &release_sha256sums, NVAFX_COMMON_RUNTIME_ASSET);
    let model_expected = expected_sha256_for_release_asset(tag, &release_sha256sums, &model_asset);

    download_release_asset(
        &common_url,
        &common_zip,
        common_expected.as_deref(),
        "common runtime",
        a.json,
    )?;
    download_release_asset(
        &model_url,
        &model_zip,
        model_expected.as_deref(),
        "model",
        a.json,
    )?;

    let report = install_nvafx_runtime(NvafxInstallRequest {
        common_zip: &common_zip,
        model_zip: &model_zip,
        runtime_dir: a.runtime_dir.as_deref(),
        common_sha256: common_expected.as_deref(),
        model_sha256: model_expected.as_deref(),
        install_source: json!({
            "kind": "github-release",
            "tag": tag,
            "arch": arch.as_str(),
            "common_url": common_url,
            "model_url": model_url,
        }),
        log_to_stderr: a.json,
    })?;

    if a.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": report.ok(),
                "report": report,
            }))?
        );
    } else {
        print_nvafx_doctor_report(&report);
    }
    if !report.ok() {
        bail!("runtime 已下载并解压,但 doctor 仍未通过");
    }
    Ok(())
}

struct NvafxInstallRequest<'a> {
    common_zip: &'a Path,
    model_zip: &'a Path,
    runtime_dir: Option<&'a Path>,
    common_sha256: Option<&'a str>,
    model_sha256: Option<&'a str>,
    install_source: serde_json::Value,
    log_to_stderr: bool,
}

fn install_nvafx_runtime(
    request: NvafxInstallRequest<'_>,
) -> Result<echoless_processors::nvafx::DoctorReport> {
    let (runtime_dir, runtime_dir_source) =
        echoless_processors::nvafx::resolve_runtime_dir(request.runtime_dir);
    create_dir_all(&runtime_dir)
        .with_context(|| format!("创建 runtime 目录失败: {}", runtime_dir.display()))?;

    let common_expected = request
        .common_sha256
        .or_else(|| expected_sha256_for_asset(request.common_zip));
    let model_expected = request
        .model_sha256
        .or_else(|| expected_sha256_for_asset(request.model_zip));
    let common_hash = verify_zip_sha256(
        request.common_zip,
        common_expected,
        "common runtime",
        request.log_to_stderr,
    )?;
    let model_hash = verify_zip_sha256(
        request.model_zip,
        model_expected,
        "model",
        request.log_to_stderr,
    )?;

    install_log(
        request.log_to_stderr,
        format!("解压 common runtime 到 {}", runtime_dir.display()),
    );
    extract_zip(request.common_zip, &runtime_dir)?;
    install_log(
        request.log_to_stderr,
        format!("解压 model 到 {}", runtime_dir.display()),
    );
    extract_zip(request.model_zip, &runtime_dir)?;

    let installed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 UNIX_EPOCH")?
        .as_secs();
    let manifest = json!({
        "sdk_version": echoless_processors::nvafx::SDK_VERSION,
        "runtime_file_version": echoless_processors::nvafx::RUNTIME_FILE_VERSION,
        "installed_at_unix": installed_at,
        "runtime_dir_source": runtime_dir_source,
        "common_zip": request.common_zip.display().to_string(),
        "common_sha256": common_hash,
        "model_zip": request.model_zip.display().to_string(),
        "model_sha256": model_hash,
        "install_source": request.install_source,
    });
    let manifest_path = runtime_dir.join("echoless-runtime-install-manifest.json");
    let mut file = File::create(&manifest_path)
        .with_context(|| format!("写入安装 manifest 失败: {}", manifest_path.display()))?;
    file.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    file.write_all(b"\n")?;

    install_log(
        request.log_to_stderr,
        format!("安装 manifest: {}", manifest_path.display()),
    );
    echoless_processors::nvafx::doctor_report(Some(&runtime_dir))
}

fn ensure_nvafx_windows_command(command: &str) -> Result<()> {
    if !cfg!(windows) {
        bail!("{command} 目前只支持 Windows x64; macOS artifact 只能用于 AEC3/LocalVQE 路径");
    }
    Ok(())
}

fn install_log(log_to_stderr: bool, message: impl AsRef<str>) {
    if log_to_stderr {
        eprintln!("{}", message.as_ref());
    } else {
        println!("{}", message.as_ref());
    }
}

fn nvafx_download_cache_dir(tag: &str) -> PathBuf {
    env::temp_dir()
        .join("echoless-nvafx-download")
        .join(sanitize_release_tag(tag))
}

fn sanitize_release_tag(tag: &str) -> String {
    let sanitized = tag
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "release".to_string()
    } else {
        sanitized
    }
}

fn nvafx_release_asset_url(tag: &str, asset: &str) -> String {
    format!(
        "{}/{}/{}",
        NVAFX_RELEASE_DOWNLOAD_BASE,
        encode_url_path_segment(tag),
        encode_url_path_segment(asset)
    )
}

fn encode_url_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn fetch_release_sha256sums(
    tag: &str,
    download_dir: &Path,
    log_to_stderr: bool,
) -> Result<HashMap<String, String>> {
    let path = download_dir.join("SHA256SUMS.txt");
    let url = nvafx_release_asset_url(tag, "SHA256SUMS.txt");
    download_file(&url, &path, "SHA256SUMS.txt", log_to_stderr)?;
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("读取 SHA256SUMS.txt 失败: {}", path.display()))?;
    Ok(parse_sha256sums(&contents))
}

fn parse_sha256sums(contents: &str) -> HashMap<String, String> {
    let mut sums = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(asset) = parts.next() else {
            continue;
        };
        if hash.len() == 64 && hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
            sums.insert(
                asset.trim_start_matches('*').to_string(),
                hash.to_ascii_lowercase(),
            );
        }
    }
    sums
}

fn expected_sha256_for_release_asset(
    tag: &str,
    release_sha256sums: &HashMap<String, String>,
    asset: &str,
) -> Option<String> {
    release_sha256sums.get(asset).cloned().or_else(|| {
        (tag == DEFAULT_NVAFX_RELEASE_TAG)
            .then(|| expected_sha256_for_asset(Path::new(asset)).map(str::to_string))
            .flatten()
    })
}

fn download_release_asset(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    label: &str,
    log_to_stderr: bool,
) -> Result<()> {
    if dest.exists() {
        match expected_sha256 {
            Some(expected) => {
                let actual = sha256_file(dest)
                    .with_context(|| format!("校验已有下载失败: {}", dest.display()))?;
                if actual.eq_ignore_ascii_case(expected) {
                    install_log(
                        log_to_stderr,
                        format!("{label} 已在缓存中且 SHA256 ok: {}", dest.display()),
                    );
                    return Ok(());
                }
                install_log(
                    log_to_stderr,
                    format!("{label} 缓存 SHA256 不匹配,重新下载: {}", dest.display()),
                );
            }
            None => {
                install_log(
                    log_to_stderr,
                    format!(
                        "{label} 已在缓存中,未提供期望 SHA256,将重新下载: {}",
                        dest.display()
                    ),
                );
            }
        }
    }
    download_file(url, dest, label, log_to_stderr)
}

fn download_file(url: &str, dest: &Path, label: &str, log_to_stderr: bool) -> Result<()> {
    if let Some(parent) = dest.parent() {
        create_dir_all(parent)
            .with_context(|| format!("创建下载目录失败: {}", parent.display()))?;
    }
    install_log(log_to_stderr, format!("下载 {label}: {url}"));
    match download_with_powershell(url, dest) {
        Ok(()) => Ok(()),
        Err(power_shell_err) => {
            install_log(
                log_to_stderr,
                format!("PowerShell 下载失败,尝试 curl.exe: {power_shell_err:#}"),
            );
            download_with_curl(url, dest)
                .with_context(|| format!("PowerShell 下载也失败: {power_shell_err:#}"))
        }
    }
}

fn download_with_powershell(url: &str, dest: &Path) -> Result<()> {
    let output = Command::new("powershell.exe")
        .arg("-NoLogo")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg("$ProgressPreference = 'SilentlyContinue'; Invoke-WebRequest -Uri $args[0] -OutFile $args[1] -UseBasicParsing")
        .arg(url)
        .arg(dest)
        .output()
        .context("启动 powershell.exe 失败")?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "powershell.exe exit={}; stderr={}; stdout={}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim(),
        String::from_utf8_lossy(&output.stdout).trim()
    )
}

fn download_with_curl(url: &str, dest: &Path) -> Result<()> {
    let output = Command::new("curl.exe")
        .arg("-L")
        .arg("--fail")
        .arg("--retry")
        .arg("2")
        .arg("--output")
        .arg(dest)
        .arg(url)
        .output()
        .context("启动 curl.exe 失败")?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "curl.exe exit={}; stderr={}; stdout={}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim(),
        String::from_utf8_lossy(&output.stdout).trim()
    )
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

fn verify_zip_sha256(
    path: &Path,
    expected: Option<&str>,
    label: &str,
    log_to_stderr: bool,
) -> Result<String> {
    let actual = sha256_file(path)?;
    match expected {
        Some(expected) if actual.eq_ignore_ascii_case(expected) => {
            install_log(log_to_stderr, format!("{label} SHA256 ok: {actual}"));
        }
        Some(expected) => bail!(
            "{label} SHA256 不匹配: actual={actual}, expected={expected}, file={}",
            path.display()
        ),
        None => {
            install_log(
                log_to_stderr,
                format!(
                    "{label} SHA256: {actual} (未找到官方期望值,仅记录;建议传 --{}-sha256)",
                    if label.starts_with("common") {
                        "common"
                    } else {
                        "model"
                    }
                ),
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
fn cmd_doctor(args: DoctorArgs) -> Result<()> {
    match args.cmd {
        DoctorCmd::Audio(a) => cmd_doctor_audio(a),
    }
}

#[cfg(feature = "realtime")]
fn cmd_doctor_audio(args: DoctorAudioArgs) -> Result<()> {
    let report = realtime::audio_doctor_json_with_options(realtime::AudioDoctorOptions {
        request_system_audio: args.request_system_audio,
    })?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("Audio doctor");
    println!("  ok: {}", report["ok"]);
    println!(
        "  virtual_output_detected: {}",
        report["virtual_output_detected"]
    );
    println!("  recommended_driver: {}", report["recommended_driver"]);
    println!("  install_status: {}", report["install_status"]);
    println!("Use --json for GUI-readable details.");
    Ok(())
}

#[cfg(not(feature = "realtime"))]
fn cmd_doctor(args: DoctorArgs) -> Result<()> {
    match args.cmd {
        DoctorCmd::Audio(a) => cmd_doctor_audio(a),
    }
}

#[cfg(not(feature = "realtime"))]
fn cmd_doctor_audio(args: DoctorAudioArgs) -> Result<()> {
    let report = json!({
        "ok": false,
        "platform": std::env::consts::OS,
        "error": "audio doctor 需 realtime 特性(cpal);当前构建未启用。",
        "virtual_output_detected": false,
        "candidate_outputs": [],
        "candidate_inputs": [],
        "recommended_driver": recommended_audio_driver(),
        "install_status": "unknown",
        "needs_reboot": false,
        "permission_state": "unknown",
        "system_audio_permission": "unknown",
        "system_audio_permission_probe": if args.request_system_audio {
            json!({
                "requested": true,
                "ok": false,
                "state": "unknown",
                "detail": "audio doctor 需 realtime 特性(cpal);当前构建未启用。"
            })
        } else {
            json!(null)
        },
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("audio doctor 需 realtime 特性(cpal);当前构建未启用。");
    }
    Ok(())
}

#[cfg(not(feature = "realtime"))]
fn recommended_audio_driver() -> &'static str {
    if cfg!(windows) {
        "vb-cable"
    } else if cfg!(target_os = "macos") {
        "blackhole-2ch"
    } else {
        "unknown"
    }
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

fn cmd_probe_delay(a: ProbeDelayArgs) -> Result<()> {
    if !cfg!(feature = "realtime") {
        bail!("probe-delay 需 realtime 特性(cpal)");
    }
    if !(cfg!(target_os = "macos") || cfg!(windows)) {
        bail!("probe-delay 当前只支持 macOS Process Tap reference 与 Windows WASAPI loopback reference");
    }
    if !a.startup_delay.is_finite() || a.startup_delay < 0.0 {
        bail!("--startup-delay 必须是非负有限数");
    }
    if !a.volume.is_finite() || !(0.0..=1.0).contains(&a.volume) {
        bail!("--volume 必须在 0.0..=1.0");
    }
    if a.beeps == 0 {
        bail!("--beeps 必须大于 0");
    }

    let (result, cleanup_dirs, session_retained) = if let Some(session_dir) = &a.analyze_only {
        (analyze_probe_session(&a, session_dir)?, Vec::new(), true)
    } else {
        let (beep_path, temp_dir) = probe_beep_path(&a)?;
        let (probe_out_dir, probe_temp_dir, retain_session) = probe_output_dir(&a)?;
        let beep_duration = write_probe_beep_train(&a, &beep_path)?;
        probe_log(a.json, format!("beep_duration_s: {beep_duration:.2}"));
        let session_dir = run_native_delay_probe(&a, &probe_out_dir, &beep_path, beep_duration)?;
        let result = analyze_probe_session(&a, &session_dir)?;
        let mut cleanup_dirs = Vec::new();
        if let Some(temp_dir) = temp_dir {
            cleanup_dirs.push(temp_dir);
        }
        if !retain_session {
            if let Some(probe_temp_dir) = probe_temp_dir {
                cleanup_dirs.push(probe_temp_dir);
            }
        }
        (result, cleanup_dirs, retain_session)
    };

    emit_probe_result(&result, a.json, session_retained)?;
    for dir in cleanup_dirs {
        let _ = remove_dir_all(dir);
    }
    if let Some(warning) = result.warnings.first() {
        bail!("near delay probe warning: {warning}");
    }
    Ok(())
}

const PROBE_SAMPLE_RATE: u32 = 48_000;
const PROBE_PRE_ROLL_S: f64 = 0.5;
const PROBE_POST_ROLL_S: f64 = 0.8;
const PROBE_BEEP_MS: f64 = 70.0;
const PROBE_GAP_MS: f64 = 650.0;
const PROBE_MAX_LAG_MS: f64 = 250.0;
const PROBE_SAFETY_MS: f64 = 8.0;
const PROBE_ENV_STEP_MS: f64 = 0.5;

#[derive(Clone, Debug)]
struct ProbeLag {
    index: usize,
    time_s: f64,
    lag_ms: f64,
    corr: f64,
}

#[derive(Clone, Debug)]
struct ProbeResult {
    session_dir: PathBuf,
    ref_dbfs: f64,
    mic_dbfs: f64,
    global_lag_ms: f64,
    global_corr: f64,
    event_count: usize,
    event_detected: usize,
    event_lag_mean_ms: f64,
    event_lag_stddev_ms: f64,
    event_lag_drift_ms: f64,
    recommended_near_delay_ms: u32,
    per_beep_lags: Vec<ProbeLag>,
    warnings: Vec<String>,
}

fn probe_beep_path(a: &ProbeDelayArgs) -> Result<(PathBuf, Option<PathBuf>)> {
    if let Some(path) = &a.keep_beep {
        return Ok((path.clone(), None));
    }
    let dir = env::temp_dir().join(format!(
        "echoless-beep-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("系统时间早于 UNIX_EPOCH")?
            .as_nanos()
    ));
    create_dir_all(&dir).with_context(|| format!("创建 probe 临时目录失败: {}", dir.display()))?;
    Ok((dir.join("near-delay-beeps.wav"), Some(dir)))
}

fn probe_output_dir(a: &ProbeDelayArgs) -> Result<(PathBuf, Option<PathBuf>, bool)> {
    if let Some(out_dir) = &a.out_dir {
        return Ok((out_dir.clone(), None, true));
    }
    let dir = env::temp_dir().join(format!(
        "echoless-near-delay-probe-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("系统时间早于 UNIX_EPOCH")?
            .as_nanos()
    ));
    Ok((dir.clone(), Some(dir), a.keep_session))
}

fn write_probe_beep_train(a: &ProbeDelayArgs, path: &Path) -> Result<f64> {
    let beep_frames = frames_for_ms(PROBE_BEEP_MS).max(1);
    let gap_frames = frames_for_ms(PROBE_GAP_MS).max(1);
    let pre_frames = (PROBE_SAMPLE_RATE as f64 * PROBE_PRE_ROLL_S).round() as usize;
    let post_frames = (PROBE_SAMPLE_RATE as f64 * PROBE_POST_ROLL_S).round() as usize;
    let ramp_frames = frames_for_ms(4.0).max(1);
    let freqs = [880.0, 1320.0, 1760.0, 1100.0];

    if let Some(parent) = path.parent() {
        create_dir_all(parent)
            .with_context(|| format!("创建蜂鸣 WAV 目录失败: {}", parent.display()))?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: PROBE_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("创建 {} 失败", path.display()))?;

    let mut total_frames = 0usize;
    for _ in 0..pre_frames {
        writer.write_sample(0i16)?;
    }
    total_frames += pre_frames;

    for i in 0..a.beeps as usize {
        let freq = freqs[i % freqs.len()];
        for n in 0..beep_frames {
            let ramp = if n < ramp_frames {
                n as f64 / ramp_frames as f64
            } else if n >= beep_frames.saturating_sub(ramp_frames) {
                (beep_frames - n - 1) as f64 / ramp_frames as f64
            } else {
                1.0
            };
            let sample = f64::from(a.volume)
                * ramp
                * (2.0 * std::f64::consts::PI * freq * n as f64 / PROBE_SAMPLE_RATE as f64).sin();
            writer.write_sample(f32_to_i16(sample as f32))?;
        }
        for _ in 0..gap_frames {
            writer.write_sample(0i16)?;
        }
        total_frames += beep_frames + gap_frames;
    }
    for _ in 0..post_frames {
        writer.write_sample(0i16)?;
    }
    total_frames += post_frames;
    writer.finalize()?;
    Ok(total_frames as f64 / PROBE_SAMPLE_RATE as f64)
}

fn frames_for_ms(ms: f64) -> usize {
    (PROBE_SAMPLE_RATE as f64 * ms / 1000.0).round() as usize
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

fn run_native_delay_probe(
    a: &ProbeDelayArgs,
    out_dir: &Path,
    beep_path: &Path,
    beep_duration_s: f64,
) -> Result<PathBuf> {
    create_dir_all(out_dir)
        .with_context(|| format!("创建 diagnostics 输出目录失败: {}", out_dir.display()))?;
    let diagnostic_seconds = (a.startup_delay + beep_duration_s + 1.0).ceil().max(1.0) as u32;
    let current_exe = env::current_exe().context("定位当前 echoless 可执行文件失败")?;
    let mut child = Command::new(current_exe)
        .arg("run")
        .arg("--processor")
        .arg("passthrough")
        .arg("--mic")
        .arg(&a.mic)
        .arg("--reference")
        .arg(&a.reference)
        .arg("--output")
        .arg(&a.output)
        .arg("--sample-rate")
        .arg(PROBE_SAMPLE_RATE.to_string())
        .arg("--frame-ms")
        .arg("10")
        .arg("--reference-channels")
        .arg("mono")
        .arg("--near-delay-ms")
        .arg("0")
        .arg("--diagnostic-dir")
        .arg(out_dir)
        .arg("--diagnostic-seconds")
        .arg(diagnostic_seconds.to_string())
        .arg("--verbose")
        .arg("--stats-interval-ms")
        .arg("1000")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("启动 echoless run probe 子进程失败")?;

    let stdout = child.stdout.take().context("probe 子进程 stdout 未捕获")?;
    let rx = spawn_probe_line_reader(stdout);
    let mut session_dir: Option<PathBuf> = None;
    let mut saw_done = false;

    let startup_deadline = Instant::now() + Duration::from_secs_f64(a.startup_delay);
    while Instant::now() < startup_deadline {
        drain_probe_output(&rx, a.json, &mut session_dir, &mut saw_done);
        if let Some(status) = child.try_wait()? {
            bail!("echoless run probe 过早退出: {status}");
        }
        thread::sleep(Duration::from_millis(50));
    }

    probe_log(
        a.json,
        format!("playing beep train: {}", beep_path.display()),
    );
    let play_status = play_probe_beep(beep_path)?;
    if !play_status.success() {
        bail!("播放蜂鸣 WAV 失败: {play_status}");
    }

    let finish_deadline = Instant::now() + Duration::from_secs(u64::from(diagnostic_seconds) + 2);
    while Instant::now() < finish_deadline {
        drain_probe_output(&rx, a.json, &mut session_dir, &mut saw_done);
        if saw_done {
            break;
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("echoless run probe 失败: {status}");
            }
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    stop_probe_child(&mut child)?;
    drain_probe_output(&rx, a.json, &mut session_dir, &mut saw_done);

    session_dir
        .filter(|path| path.is_dir())
        .or_else(|| newest_probe_session(out_dir).ok())
        .with_context(|| format!("未找到 diagnostics session: {}", out_dir.display()))
}

fn play_probe_beep(beep_path: &Path) -> Result<std::process::ExitStatus> {
    if cfg!(target_os = "macos") {
        return Command::new("afplay")
            .arg(beep_path)
            .status()
            .context("播放蜂鸣 WAV 失败(需要 macOS afplay)");
    }
    if cfg!(windows) {
        return Command::new("powershell.exe")
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg("$player = New-Object System.Media.SoundPlayer $args[0]; $player.Load(); $player.PlaySync()")
            .arg(beep_path)
            .status()
            .context("播放蜂鸣 WAV 失败(需要 Windows PowerShell SoundPlayer)");
    }
    bail!("当前平台没有 probe-delay 蜂鸣播放实现");
}

fn spawn_probe_line_reader<R>(reader: R) -> Receiver<String>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = channel();
    thread::spawn(move || {
        for line in BufReader::new(reader)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    receiver
}

fn drain_probe_output(
    rx: &Receiver<String>,
    json_mode: bool,
    session_dir: &mut Option<PathBuf>,
    saw_done: &mut bool,
) {
    while let Ok(line) = rx.try_recv() {
        if !json_mode {
            println!("{line}");
        }
        if let Some((_, dir)) = line.split_once("诊断录制目录:") {
            *session_dir = Some(PathBuf::from(dir.trim()));
        }
        if let Some((_, dir)) = line.split_once("诊断录制完成") {
            if let Some((_, path)) = dir.rsplit_once(": ") {
                *session_dir = Some(PathBuf::from(path.trim()));
            }
            *saw_done = true;
        }
    }
}

fn stop_probe_child(child: &mut std::process::Child) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    let _ = Command::new("kill")
        .arg("-INT")
        .arg(child.id().to_string())
        .status();

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    child.kill().context("停止 probe 子进程失败")?;
    let _ = child.wait();
    Ok(())
}

fn newest_probe_session(out_dir: &Path) -> Result<PathBuf> {
    let mut sessions = std::fs::read_dir(out_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_session = path
                .file_name()
                .map(|name| name.to_string_lossy().starts_with("session-"))
                .unwrap_or(false);
            if !path.is_dir() || !is_session {
                return None;
            }
            entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|modified| (modified, path))
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|(modified, _)| *modified);
    sessions
        .pop()
        .map(|(_, path)| path)
        .with_context(|| format!("{} 下没有 session-*", out_dir.display()))
}

fn analyze_probe_session(a: &ProbeDelayArgs, session_dir: &Path) -> Result<ProbeResult> {
    let ref_path = session_dir.join("ref.wav");
    let mic_path = session_dir.join("mic.wav");
    let (ref_rate, reference) = read_wav_mono(&ref_path)?;
    let (mic_rate, mic) = read_wav_mono(&mic_path)?;
    if ref_rate != PROBE_SAMPLE_RATE || mic_rate != PROBE_SAMPLE_RATE {
        bail!(
            "probe 只支持 48k diagnostics,实际 ref={} mic={}",
            ref_rate,
            mic_rate
        );
    }

    let step_frames = frames_for_ms(PROBE_ENV_STEP_MS).max(1);
    let ref_env = envelope(&reference, step_frames);
    let mic_env = envelope(&mic, step_frames);
    let (global_lag_ms, global_corr) =
        estimate_probe_lag(&ref_env, &mic_env, PROBE_ENV_STEP_MS, PROBE_MAX_LAG_MS);
    let events = find_ref_events(&ref_env, PROBE_ENV_STEP_MS, a.beeps as usize);
    let event_lags = per_event_lags(
        &ref_env,
        &mic_env,
        &events,
        PROBE_ENV_STEP_MS,
        PROBE_MAX_LAG_MS,
    );
    let valid_lags = event_lags
        .iter()
        .filter(|(_, _, corr)| corr.abs() > 0.15)
        .map(|(_, lag, _)| *lag)
        .collect::<Vec<_>>();
    let (mean, stddev, drift) = if valid_lags.is_empty() {
        (global_lag_ms, 0.0, 0.0)
    } else {
        let mean = valid_lags.iter().sum::<f64>() / valid_lags.len() as f64;
        let variance =
            valid_lags.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / valid_lags.len() as f64;
        let drift = valid_lags.last().unwrap() - valid_lags.first().unwrap();
        (mean, variance.sqrt(), drift)
    };
    let ref_dbfs = rms_db(&reference);
    let mic_dbfs = rms_db(&mic);
    let mut warnings = Vec::new();
    if ref_dbfs < -45.0 {
        warnings.push("ref is very quiet; play the beep through the system output".to_string());
    }

    Ok(ProbeResult {
        session_dir: session_dir.to_path_buf(),
        ref_dbfs,
        mic_dbfs,
        global_lag_ms,
        global_corr,
        event_count: valid_lags.len(),
        event_detected: events.len(),
        event_lag_mean_ms: mean,
        event_lag_stddev_ms: stddev,
        event_lag_drift_ms: drift,
        recommended_near_delay_ms: recommended_near_delay_ms(mean, PROBE_SAFETY_MS),
        per_beep_lags: event_lags
            .into_iter()
            .enumerate()
            .map(|(index, (event_index, lag_ms, corr))| ProbeLag {
                index: index + 1,
                time_s: event_index as f64 * PROBE_ENV_STEP_MS / 1000.0,
                lag_ms,
                corr,
            })
            .collect(),
        warnings,
    })
}

fn read_wav_mono(path: &Path) -> Result<(u32, Vec<f32>)> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("读取 WAV 失败: {}", path.display()))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels.max(1));
    let values = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int if spec.bits_per_sample <= 16 => reader
            .samples::<i16>()
            .map(|sample| sample.map(|v| v as f32 / i16::MAX as f32))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int if spec.bits_per_sample <= 32 => {
            let scale = (1_i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|v| v as f32 / scale))
                .collect::<std::result::Result<Vec<_>, _>>()?
        }
        _ => bail!(
            "{} 不支持的 WAV 格式: {:?} {}bit",
            path.display(),
            spec.sample_format,
            spec.bits_per_sample
        ),
    };
    if channels == 1 {
        return Ok((spec.sample_rate, values));
    }
    let mono = values
        .chunks(channels)
        .filter(|frame| frame.len() == channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect();
    Ok((spec.sample_rate, mono))
}

fn envelope(samples: &[f32], step_frames: usize) -> Vec<f64> {
    samples
        .chunks(step_frames.max(1))
        .map(|chunk| {
            let energy = chunk
                .iter()
                .map(|sample| f64::from(*sample) * f64::from(*sample))
                .sum::<f64>();
            (energy / chunk.len().max(1) as f64).sqrt()
        })
        .collect()
}

fn rms_db(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return -120.0;
    }
    let energy = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>();
    20.0 * ((energy / samples.len() as f64).sqrt() + 1e-12).log10()
}

fn standardize(values: &[f64]) -> Vec<f64> {
    if values.is_empty() {
        return Vec::new();
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let centered = values.iter().map(|v| v - mean).collect::<Vec<_>>();
    let energy = centered.iter().map(|v| v * v).sum::<f64>().sqrt().max(1.0);
    centered.into_iter().map(|v| v / energy).collect()
}

fn estimate_probe_lag(reference: &[f64], mic: &[f64], step_ms: f64, max_lag_ms: f64) -> (f64, f64) {
    let n = reference.len().min(mic.len());
    let reference = standardize(&reference[..n]);
    let mic = standardize(&mic[..n]);
    let max_lag = (max_lag_ms / step_ms).round().max(1.0) as isize;
    let mut best_lag = 0isize;
    let mut best_corr = 0.0f64;

    for lag in -max_lag..=max_lag {
        let (ref_start, mic_start, len) = if lag >= 0 {
            let lag = lag as usize;
            (0usize, lag, n.saturating_sub(lag))
        } else {
            let lag = (-lag) as usize;
            (lag, 0usize, n.saturating_sub(lag))
        };
        if len < 10 {
            continue;
        }
        let corr = (0..len)
            .map(|i| reference[ref_start + i] * mic[mic_start + i])
            .sum::<f64>();
        if corr.abs() > best_corr.abs() {
            best_corr = corr;
            best_lag = lag;
        }
    }
    (best_lag as f64 * step_ms, best_corr)
}

fn find_ref_events(reference: &[f64], step_ms: f64, expected: usize) -> Vec<usize> {
    if reference.is_empty() || expected == 0 {
        return Vec::new();
    }
    let mut sorted = reference.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    let peak = reference.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let threshold = median + (peak - median) * 0.35;
    let min_gap = (180.0 / step_ms).round().max(1.0) as usize;
    let mut events = Vec::new();
    let mut i = 0usize;
    while i < reference.len() {
        if reference[i] < threshold {
            i += 1;
            continue;
        }
        let mut best_i = i;
        let mut best_v = reference[i];
        while i < reference.len() && reference[i] >= threshold {
            if reference[i] > best_v {
                best_i = i;
                best_v = reference[i];
            }
            i += 1;
        }
        if events
            .last()
            .is_none_or(|last| best_i.saturating_sub(*last) >= min_gap)
        {
            events.push(best_i);
        } else if reference[best_i] > reference[*events.last().unwrap()] {
            *events.last_mut().unwrap() = best_i;
        }
        if events.len() >= expected {
            break;
        }
    }
    events
}

fn per_event_lags(
    reference: &[f64],
    mic: &[f64],
    events: &[usize],
    step_ms: f64,
    max_lag_ms: f64,
) -> Vec<(usize, f64, f64)> {
    let half_window = (160.0 / step_ms).round().max(20.0) as usize;
    events
        .iter()
        .map(|event| {
            let start = event.saturating_sub(half_window);
            let end = reference.len().min(event + half_window);
            let (lag_ms, corr) = estimate_probe_lag(
                &reference[start..end],
                &mic[start..end],
                step_ms,
                max_lag_ms,
            );
            (*event, lag_ms, corr)
        })
        .collect()
}

fn recommended_near_delay_ms(lag_ms: f64, safety_ms: f64) -> u32 {
    if lag_ms >= 0.0 {
        return 0;
    }
    ((-lag_ms + safety_ms) / 5.0).round().max(0.0) as u32 * 5
}

fn emit_probe_result(result: &ProbeResult, json_mode: bool, session_retained: bool) -> Result<()> {
    if json_mode {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "session_dir": result.session_dir.display().to_string(),
                "session_retained": session_retained,
                "ref_dbfs": round2(result.ref_dbfs),
                "mic_dbfs": round2(result.mic_dbfs),
                "global_lag_ms": round2(result.global_lag_ms),
                "global_corr": round4(result.global_corr),
                "event_count": result.event_count,
                "event_detected": result.event_detected,
                "event_lag_mean_ms": round2(result.event_lag_mean_ms),
                "event_lag_stddev_ms": round2(result.event_lag_stddev_ms),
                "event_lag_drift_ms": round2(result.event_lag_drift_ms),
                "recommended_near_delay_ms": result.recommended_near_delay_ms,
                "per_beep_lags": result.per_beep_lags.iter().map(|lag| json!({
                    "index": lag.index,
                    "time_s": round3(lag.time_s),
                    "lag_ms": round2(lag.lag_ms),
                    "corr": round4(lag.corr),
                })).collect::<Vec<_>>(),
                "warnings": result.warnings,
            }))?
        );
        return Ok(());
    }

    println!("\n=== near delay probe result ===");
    println!("session_dir: {}", result.session_dir.display());
    println!("session_retained: {}", session_retained);
    println!("ref_dbfs: {:.1}", result.ref_dbfs);
    println!("mic_dbfs: {:.1}", result.mic_dbfs);
    println!(
        "global_lag_ms: {:+.2}  corr={:+.3}",
        result.global_lag_ms, result.global_corr
    );
    println!(
        "event_count: {}/{}",
        result.event_count, result.event_detected
    );
    println!("event_lag_mean_ms: {:+.2}", result.event_lag_mean_ms);
    println!("event_lag_stddev_ms: {:.2}", result.event_lag_stddev_ms);
    println!("event_lag_drift_ms: {:+.2}", result.event_lag_drift_ms);
    println!(
        "recommended_near_delay_ms: {}",
        result.recommended_near_delay_ms
    );
    println!("\nper-beep lags:");
    for lag in &result.per_beep_lags {
        println!(
            "  {:02}  t={:6.2}s  lag={:+7.2}ms  corr={:+.3}",
            lag.index, lag.time_s, lag.lag_ms, lag.corr
        );
    }
    for warning in &result.warnings {
        println!("\nwarning: {warning}");
    }
    Ok(())
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn round4(value: f64) -> f64 {
    (value * 10000.0).round() / 10000.0
}

fn probe_log(json_mode: bool, message: impl AsRef<str>) {
    if !json_mode {
        println!("{}", message.as_ref());
    }
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
    if let Some(v) = a.near_delay_ms {
        cfg.near_delay_ms = v;
    }
    if let Some(v) = a.output_level {
        cfg.output_level = v;
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
            near_delay_ms: None,
            output_level: None,
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

    fn probe_delay_args() -> ProbeDelayArgs {
        ProbeDelayArgs {
            mic: "MacBook Pro麦克风".to_string(),
            reference: "system".to_string(),
            output: "BlackHole 2ch".to_string(),
            out_dir: None,
            keep_session: false,
            startup_delay: 4.0,
            beeps: 12,
            volume: 0.35,
            analyze_only: None,
            keep_beep: None,
            json: true,
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
        args.near_delay_ms = Some(25);
        args.output_level = Some(75);
        args.processor = vec!["sonora_aec3".into()];
        args.ns_level = Some("high".into());
        args.tail_ms = Some(120);

        let cfg = apply_run_overrides(PipelineConfig::default(), &args).unwrap();

        assert_eq!(cfg.mic, "4");
        assert_eq!(cfg.reference, "system");
        assert_eq!(cfg.output, "CABLE Input");
        assert_eq!(cfg.sample_rate, 48_000);
        assert_eq!(cfg.frame_ms, 10);
        assert_eq!(cfg.near_delay_ms, 25);
        assert_eq!(cfg.output_level, 75);
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
    fn probe_recommendation_uses_only_mic_lead() {
        assert_eq!(recommended_near_delay_ms(-18.5, 8.0), 25);
        assert_eq!(recommended_near_delay_ms(-2.0, 8.0), 10);
        assert_eq!(recommended_near_delay_ms(12.0, 8.0), 0);
    }

    #[test]
    fn probe_output_dir_is_temporary_by_default() {
        let args = probe_delay_args();
        let (out_dir, temp_dir, retained) = probe_output_dir(&args).unwrap();
        assert!(!retained);
        assert_eq!(temp_dir.as_ref(), Some(&out_dir));
        assert!(out_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("echoless-near-delay-probe-"));

        let mut keep_args = probe_delay_args();
        keep_args.keep_session = true;
        let (out_dir, temp_dir, retained) = probe_output_dir(&keep_args).unwrap();
        assert!(retained);
        assert_eq!(temp_dir.as_ref(), Some(&out_dir));

        let mut explicit_args = probe_delay_args();
        explicit_args.out_dir = Some(PathBuf::from("/tmp/echoless-probe-explicit"));
        let (out_dir, temp_dir, retained) = probe_output_dir(&explicit_args).unwrap();
        assert!(retained);
        assert!(temp_dir.is_none());
        assert_eq!(out_dir, PathBuf::from("/tmp/echoless-probe-explicit"));
    }

    #[test]
    fn native_probe_lag_estimator_detects_mic_leading_reference() {
        let mut reference = vec![0.0; 1_100];
        let mut mic = vec![0.0; 1_100];
        for index in [100usize, 500, 900] {
            reference[index] = 1.0;
            mic[index - 4] = 1.0;
        }

        let events = find_ref_events(&reference, 0.5, 3);
        let event_lags = per_event_lags(&reference, &mic, &events, 0.5, 20.0);
        let (global_lag_ms, corr) = estimate_probe_lag(&reference, &mic, 0.5, 20.0);

        assert_eq!(events, vec![100, 500, 900]);
        assert_eq!(global_lag_ms, -2.0);
        assert!(corr > 0.95);
        assert!(event_lags
            .iter()
            .all(|(_, lag_ms, corr)| *lag_ms == -2.0 && *corr > 0.95));
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
        assert_eq!(
            manifest["pipeline"]["params"]["near_delay_ms"]["default"],
            json!(default_near_delay_ms())
        );
        assert_eq!(
            manifest["pipeline"]["params"]["near_delay_ms"]["max"],
            json!(MAX_NEAR_DELAY_MS)
        );
        assert_eq!(
            manifest["pipeline"]["params"]["output_level"]["default"],
            json!(default_output_level())
        );
        assert_eq!(
            manifest["pipeline"]["params"]["output_level"]["unity"],
            json!(UNITY_OUTPUT_LEVEL)
        );
        assert_eq!(
            manifest["pipeline"]["params"]["output_level"]["max_gain"],
            json!(OUTPUT_LEVEL_MAX_GAIN)
        );
        assert_eq!(
            manifest["pipeline"]["params"]["output_level"]["curve"],
            json!("power")
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
    fn config_deserialization_defaults_device_fields() {
        let cfg: PipelineConfig = toml::from_str(
            r#"
            sample_rate = 48000
            frame_ms = 10

            [[chain]]
            kind = "sonora_aec3"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.mic, "default");
        assert_eq!(cfg.reference, "system");
        assert_eq!(cfg.output, "default");
        assert_eq!(cfg.near_delay_ms, default_near_delay_ms());
        assert_eq!(cfg.output_level, default_output_level());
    }

    #[test]
    fn config_shape_validation_reports_field_paths() {
        let value: toml::Value = toml::from_str(
            r#"
            mic = 1
            near_delay_ms = "bad"
            output_level = "loud"
            reference_channels = "surround"
            diagnostics = "bad"
            chain = [{}]
            "#,
        )
        .unwrap();

        let errors = validate_config_shape(&value);
        let paths = errors
            .iter()
            .map(|error| error.path.as_str())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"mic"));
        assert!(paths.contains(&"near_delay_ms"));
        assert!(paths.contains(&"output_level"));
        assert!(paths.contains(&"reference_channels"));
        assert!(paths.contains(&"diagnostics"));
        assert!(paths.contains(&"chain[0].kind"));
    }

    #[test]
    fn config_validation_reports_frontend_safe_errors() {
        let mut bad_params = toml::Table::new();
        bad_params.insert("tail_ms".into(), toml::Value::Integer(1));
        bad_params.insert("ns".into(), toml::Value::String("yes".into()));
        let cfg = PipelineConfig {
            sample_rate: 44_100,
            near_delay_ms: MAX_NEAR_DELAY_MS + 1,
            output_level: MAX_OUTPUT_LEVEL + 1,
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
        assert!(paths.contains(&"near_delay_ms"));
        assert!(paths.contains(&"output_level"));
        assert!(paths.contains(&"reference_channels"));
        assert!(paths.contains(&"chain[1].doctor"));
    }

    #[test]
    fn nvafx_release_asset_url_uses_public_release_base() {
        let url = nvafx_release_asset_url(
            DEFAULT_NVAFX_RELEASE_TAG,
            "echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip",
        );

        assert_eq!(
            url,
            "https://github.com/Haor/echoless/releases/download/rtx-aec-runtime-win64-2.1.0-aec48-preview.1/echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip"
        );
    }

    #[test]
    fn nvafx_release_asset_url_encodes_tag_path_segments() {
        let url = nvafx_release_asset_url("preview/a b", "asset.zip");

        assert_eq!(
            url,
            "https://github.com/Haor/echoless/releases/download/preview%2Fa%20b/asset.zip"
        );
    }

    #[test]
    fn parse_sha256sums_accepts_common_formats() {
        let sums = parse_sha256sums(
            r#"
            # comment
            DCACAC954B7973AE18369B252D13F24B973B10114D00E5293EAB0713601C7BCB  echoless-rtx-aec-common-runtime-win64-2.1.0.zip
            0e75bb7442d317990ef0d5a6477105f86b9bbae1c2c5e4a6bdfb8d4e9f42df5b *echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip
            invalid line
            "#,
        );

        assert_eq!(
            sums["echoless-rtx-aec-common-runtime-win64-2.1.0.zip"],
            "dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb"
        );
        assert_eq!(
            sums["echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip"],
            "0e75bb7442d317990ef0d5a6477105f86b9bbae1c2c5e4a6bdfb8d4e9f42df5b"
        );
    }

    #[test]
    fn expected_sha256_prefers_release_sums_then_default_embedded_values() {
        let mut sums = HashMap::new();
        sums.insert(
            NVAFX_COMMON_RUNTIME_ASSET.to_string(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        );

        assert_eq!(
            expected_sha256_for_release_asset(
                DEFAULT_NVAFX_RELEASE_TAG,
                &sums,
                NVAFX_COMMON_RUNTIME_ASSET
            )
            .as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(
            expected_sha256_for_release_asset(
                DEFAULT_NVAFX_RELEASE_TAG,
                &HashMap::new(),
                NVAFX_COMMON_RUNTIME_ASSET,
            )
            .as_deref(),
            Some("dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb")
        );
        assert_eq!(
            expected_sha256_for_release_asset(
                "custom-tag",
                &HashMap::new(),
                NVAFX_COMMON_RUNTIME_ASSET
            ),
            None
        );
    }
}
