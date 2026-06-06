//! aec — 跨平台 reference-based AEC 工具 CLI(骨架)。
//!
//! 当前可用:`offline`(mic.wav + ref.wav 经处理链 → out.wav)。
//! 待平台 HAL:`devices` / `run`(实时)。处理方案 = 经典 AEC3(sonora)+ LocalVQE,可单开/串联/组合。

#[cfg(not(feature = "realtime"))]
mod backends;
#[cfg(feature = "realtime")]
mod realtime;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use echoless_core::{run_offline, PipelineConfig};
use echoless_hal::file::{WavFileSink, WavFileSource};
use echoless_processors::{registry, NodeConfig};

#[derive(Parser)]
#[command(name = "echoless", about = "跨平台 reference-based AEC 工具(骨架)", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 离线:mic.wav + ref.wav 经处理链 → out.wav(当前可用)
    Offline(OfflineArgs),
    /// 列出可用处理器种类(设备枚举依赖平台 HAL,TODO)
    Processors,
    /// 列出音频设备(平台 HAL,TODO)
    Devices,
    /// 实时运行(平台 HAL,TODO)
    Run(RunArgs),
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
    /// 快捷处理链(逗号分隔),如 "sonora_aec3,localvqe";与 --config 二选一
    #[arg(long)]
    chain: Option<String>,
    #[arg(long, default_value_t = 48000)]
    rate: u32,
    #[arg(long, default_value_t = 10)]
    frame_ms: u32,
}

#[derive(Args)]
struct RunArgs {
    /// 管线 TOML 配置
    #[arg(long)]
    config: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Offline(a) => cmd_offline(a),
        Cmd::Processors => cmd_processors(),
        Cmd::Devices => cmd_devices(),
        Cmd::Run(a) => cmd_run(a),
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
            .map(|k| NodeConfig { kind: k.to_string(), params: toml::Table::new() })
            .collect();
        (a.rate, a.frame_ms, chain)
    };

    let cfg = PipelineConfig {
        mic: a.mic.clone(),
        reference: a.reference.clone(),
        output: a.out.clone(),
        sample_rate: rate,
        frame_ms,
        chain,
    };

    let frame = cfg.frame_size();
    let mic = WavFileSource::new(&a.mic, frame)?;
    let reference = WavFileSource::new(&a.reference, frame)?;
    let sink = WavFileSink::new(&a.out);

    let chain_desc = if cfg.chain.is_empty() {
        "直通(passthrough)".to_string()
    } else {
        cfg.chain.iter().map(|n| n.kind.clone()).collect::<Vec<_>>().join(" → ")
    };
    println!("离线运行: {} + {} → {}", a.mic, a.reference, a.out);
    println!("采样率 {} Hz · 帧 {} ms · 链: {}", rate, frame_ms, chain_desc);

    let rep = run_offline(&cfg, mic, reference, sink)?;
    println!(
        "完成: {} 帧 (~{:.2}s) · 链 [{}] · 累计算法延迟 {:.1} ms",
        rep.frames,
        rep.seconds,
        rep.chain.join(", "),
        rep.total_latency_ms
    );
    for s in &rep.node_stats {
        println!("  - {}: ERLE {:.1} dB, delay {} ms, diverged={}", s.name, s.erle_db, s.estimated_delay_ms, s.diverged);
    }
    Ok(())
}

fn cmd_processors() -> Result<()> {
    println!("可用处理器种类:");
    for k in registry::kinds() {
        println!("  - {k}");
    }
    println!("(在 --chain 或 config 的 [[chain]] 里按 kind 引用;可单开/串联/组合)");
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
    let s = std::fs::read_to_string(&a.config)?;
    let cfg: PipelineConfig = toml::from_str(&s)?;
    println!("实时运行配置: mic={} ref={} out={}", cfg.mic, cfg.reference, cfg.output);
    realtime::run(&cfg)
}

#[cfg(not(feature = "realtime"))]
fn cmd_run(_a: RunArgs) -> Result<()> {
    let _ = backends::make_mic("default");
    anyhow::bail!("实时管线需 realtime 特性(cpal);当前构建未启用")
}
