//! echoless — 跨平台 reference-based AEC 工具 CLI。
//!
//! 当前可用:`processors` / `devices` / `offline` / `run` / `nvafx doctor`。
//! 实时 MVP 走 cpal;主线走经典 AEC3(sonora)保真,LocalVQE 作为独立可选处理器。

#[cfg(not(feature = "realtime"))]
mod backends;
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
use echoless_hal::file::{WavFileSink, WavFileSource};
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
    Processors,
    /// 列出音频设备
    Devices,
    /// 实时运行
    Run(RunArgs),
    /// NVIDIA AFX / RTX AEC runtime 工具
    Nvafx(NvafxArgs),
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
    /// 快捷处理链,如 "sonora_aec3" 或 "localvqe";逗号串联仅用于实验
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
    /// 覆盖处理链,可重复或逗号分隔;默认建议单开 sonora_aec3
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
    /// 覆盖 runtime 根目录;默认读 ECHOLESS_NVAFX_RUNTIME_DIR,再退到 %LOCALAPPDATA%\Echoless\nvafx\2.1.0
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
    /// 覆盖安装根目录;默认 %LOCALAPPDATA%\Echoless\nvafx\2.1.0
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
        Cmd::Processors => cmd_processors(),
        Cmd::Devices => cmd_devices(),
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

fn cmd_processors() -> Result<()> {
    println!("可用处理器种类:");
    for k in registry::kinds() {
        println!("  - {k}");
    }
    println!("(在 --chain 或 config 的 [[chain]] 里按 kind 引用;默认建议单开 sonora_aec3,串联仅用于实验)");
    Ok(())
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
fn cmd_devices() -> Result<()> {
    realtime::print_devices()
}

#[cfg(not(feature = "realtime"))]
fn cmd_devices() -> Result<()> {
    println!("设备枚举需 realtime 特性(cpal);当前构建未启用。");
    let _ = backends::make_mic("default");
    Ok(())
}

#[cfg(feature = "realtime")]
fn cmd_run(a: RunArgs) -> Result<()> {
    let cfg = load_run_config(&a)?;
    validate_nvafx_constraints(&cfg)?;
    let opts = runtime_options_from_args(&a)?;
    println!(
        "实时运行配置: mic={} ref={} out={}",
        cfg.mic, cfg.reference, cfg.output
    );
    realtime::run_with_options(&cfg, opts)
}

#[cfg(not(feature = "realtime"))]
fn cmd_run(_a: RunArgs) -> Result<()> {
    let _ = backends::make_mic("default");
    anyhow::bail!("实时管线需 realtime 特性(cpal);当前构建未启用")
}

fn load_run_config(a: &RunArgs) -> Result<PipelineConfig> {
    let cfg = if let Some(path) = &a.config {
        let s = std::fs::read_to_string(path)?;
        toml::from_str(&s)?
    } else {
        PipelineConfig::default()
    };
    apply_run_overrides(cfg, a)
}

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
        stats_interval_ms: a.stats_interval_ms.or_else(|| a.verbose.then_some(1000)),
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
    }

    #[test]
    #[cfg(feature = "realtime")]
    fn runtime_options_reject_zero_interval() {
        let mut args = run_args();
        args.stats_interval_ms = Some(0);

        let err = runtime_options_from_args(&args).unwrap_err();

        assert!(err.to_string().contains("大于 0"));
    }
}
