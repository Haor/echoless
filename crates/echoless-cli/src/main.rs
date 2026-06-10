//! echoless — 跨平台 reference-based AEC 工具 CLI。
//!
//! 当前可用:`processors` / `devices` / `doctor audio` / `offline` / `run` / `nvafx doctor/install/download-install`。
//! 实时主路径走 cpal;主线走经典 AEC3(sonora)保真,LocalVQE 作为独立可选处理器。

mod config_validate;
mod nvafx_install;
mod probe_delay;
#[cfg(feature = "realtime")]
mod realtime;

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

use config_validate::{cmd_config, validate_pipeline_config, ConfigArgs};
use nvafx_install::{cmd_nvafx, validate_nvafx_constraints, NvafxArgs};
use probe_delay::{cmd_probe_delay, ProbeDelayArgs};

use echoless_audio_io::file::{WavFileSink, WavFileSource};
use echoless_core::{
    apply_reference_channels_to_chain, default_near_delay_ms, default_output_level, run_offline,
    DiagnosticsConfig, PipelineConfig, ReferenceChannels, MAX_NEAR_DELAY_MS, MAX_OUTPUT_LEVEL,
    MIN_OUTPUT_LEVEL, OUTPUT_LEVEL_CURVE_EXPONENT, OUTPUT_LEVEL_MAX_BOOST_DB,
    OUTPUT_LEVEL_MAX_GAIN, UNITY_OUTPUT_LEVEL,
};
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
}
