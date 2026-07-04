use clap::{Args, Parser, Subcommand};

use crate::config_validate::ConfigArgs;
use crate::nvafx_install::NvafxArgs;
use crate::probe_delay::ProbeDelayArgs;
use echoless_core::ReferenceChannels;

#[derive(Parser)]
#[command(name = "echoless", about = "跨平台 reference-based AEC 工具", version)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) cmd: Cmd,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
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
pub(crate) struct ProcessorsArgs {
    /// 输出 JSON manifest,供 GUI 消费
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct DevicesArgs {
    /// 输出 JSON,供 GUI 消费
    #[arg(long)]
    pub(crate) json: bool,
    /// 快速枚举:只查询设备身份,跳过可能被驱动卡住的完整格式范围探测
    #[arg(long)]
    pub(crate) fast: bool,
}

#[derive(Args)]
pub(crate) struct DoctorArgs {
    #[command(subcommand)]
    pub(crate) cmd: DoctorCmd,
}

#[derive(Subcommand)]
pub(crate) enum DoctorCmd {
    /// 检查虚拟音频设备、reference 可用性和音频权限状态
    Audio(DoctorAudioArgs),
}

#[derive(Args)]
pub(crate) struct DoctorAudioArgs {
    /// 输出 JSON,供 GUI onboarding 消费
    #[arg(long)]
    pub(crate) json: bool,
    /// 快速枚举音频设备,避免坏驱动在完整格式范围探测中卡住
    #[arg(long)]
    pub(crate) fast_devices: bool,
    /// macOS:显式触发一次系统音频录制权限请求/探测;不会在普通 doctor 中隐式弹窗
    #[arg(long)]
    pub(crate) request_system_audio: bool,
}

#[derive(Args)]
pub(crate) struct OfflineArgs {
    /// 近端麦克风 WAV
    #[arg(long)]
    pub(crate) mic: String,
    /// far-end 参考 WAV
    #[arg(long)]
    pub(crate) reference: String,
    /// 输出 WAV
    #[arg(long)]
    pub(crate) out: String,
    /// 处理链 TOML 配置(含 [[chain]]);给了则用其 chain/rate/frame_ms
    #[arg(long)]
    pub(crate) config: Option<String>,
    /// 快捷处理器 kind,如 "aec3" 或 "localvqe"
    #[arg(long)]
    pub(crate) chain: Option<String>,
    #[arg(long, default_value_t = 48000)]
    pub(crate) rate: u32,
    #[arg(long, default_value_t = 10)]
    pub(crate) frame_ms: u32,
    /// 最终输出电平:0=静音,50=原声,100=3x 增益
    #[arg(long)]
    pub(crate) output_level: Option<u32>,
}

#[derive(Args)]
pub(crate) struct RunArgs {
    /// 管线 TOML 配置;不给则从默认配置开始,再应用命令行覆盖
    #[arg(long)]
    pub(crate) config: Option<String>,
    /// 覆盖麦克风设备:default、索引或名称片段
    #[arg(long)]
    pub(crate) mic: Option<String>,
    /// 覆盖 far-end 参考源:system、none、output:<名>、input:<名>、索引或名称片段
    #[arg(long)]
    pub(crate) reference: Option<String>,
    /// 覆盖输出设备:default、索引或名称片段
    #[arg(long)]
    pub(crate) output: Option<String>,
    /// 覆盖采样率
    #[arg(long)]
    pub(crate) sample_rate: Option<u32>,
    /// 覆盖帧长(ms)
    #[arg(long)]
    pub(crate) frame_ms: Option<u32>,
    /// reference 送进 AEC 的声道模式:mono 或 stereo
    #[arg(long, value_parser = parse_reference_channels)]
    pub(crate) reference_channels: Option<ReferenceChannels>,
    /// near/mic 进入处理器前的人为对齐延迟(ms);macOS 默认 25,其他平台默认 0
    #[arg(long)]
    pub(crate) near_delay_ms: Option<u32>,
    /// 最终输出电平:0=静音,50=原声,100=3x 增益
    #[arg(long)]
    pub(crate) output_level: Option<u32>,
    /// 覆盖处理器,可重复或逗号分隔;默认建议 aec3
    #[arg(long, value_delimiter = ',')]
    pub(crate) processor: Vec<String>,
    /// 开启 aec3 降噪
    #[arg(long)]
    pub(crate) ns: bool,
    /// 关闭 aec3 降噪
    #[arg(long)]
    pub(crate) no_ns: bool,
    /// 覆盖 aec3 降噪强度:low/moderate/high/veryhigh
    #[arg(long)]
    pub(crate) ns_level: Option<String>,
    /// 覆盖 aec3 echo tail 长度(ms)
    #[arg(long)]
    pub(crate) tail_ms: Option<u32>,
    /// 每秒打印滚动实时统计
    #[arg(long)]
    pub(crate) verbose: bool,
    /// 自定义滚动统计间隔(ms);隐含 --verbose
    #[arg(long)]
    pub(crate) stats_interval_ms: Option<u64>,
    /// 输出 JSONL runtime status,供 GUI/sidecar 消费
    #[arg(long)]
    pub(crate) status_json: bool,
    /// 保存实时诊断录音的目录;会在其下创建 timestamp session
    #[arg(long)]
    pub(crate) diagnostic_dir: Option<String>,
    /// 诊断录制秒数上限;不给则录到停止
    #[arg(long)]
    pub(crate) diagnostic_seconds: Option<u32>,
}

fn parse_reference_channels(s: &str) -> Result<ReferenceChannels, String> {
    match s.to_ascii_lowercase().as_str() {
        "mono" | "1" | "1ch" => Ok(ReferenceChannels::Mono),
        "stereo" | "2" | "2ch" => Ok(ReferenceChannels::Stereo),
        _ => Err("必须是 mono 或 stereo".to_string()),
    }
}
