use clap::{Args, Parser, Subcommand};

use crate::config_validate::ConfigArgs;
use crate::nvafx_install::NvafxArgs;
use crate::probe_delay::ProbeDelayArgs;
use echoless_core::ReferenceChannels;

#[derive(Parser)]
#[command(
    name = "echoless",
    about = "Cross-platform reference-based AEC tool",
    version
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) cmd: Cmd,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
    /// Offline: mic.wav + ref.wav through processing chain -> out.wav
    Offline(OfflineArgs),
    /// List available processor kinds
    Processors(ProcessorsArgs),
    /// List audio devices
    Devices(DevicesArgs),
    /// Cross-platform environment diagnostics
    Doctor(DoctorArgs),
    /// Config file utilities
    Config(ConfigArgs),
    /// Realtime run
    Run(RunArgs),
    /// Actively probe near-end alignment delay between reference and mic
    ProbeDelay(ProbeDelayArgs),
    /// NVIDIA AFX / RTX AEC runtime tooling
    Nvafx(NvafxArgs),
}

#[derive(Args)]
pub(crate) struct ProcessorsArgs {
    /// Emit JSON manifest for GUI consumers
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct DevicesArgs {
    /// Emit JSON for GUI consumers
    #[arg(long)]
    pub(crate) json: bool,
    /// Fast enumeration: query only device identity, skipping full format-range probing that can hang on some drivers
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
    /// Check virtual audio devices, reference availability, and audio permission state
    Audio(DoctorAudioArgs),
}

#[derive(Args)]
pub(crate) struct DoctorAudioArgs {
    /// Emit JSON for GUI onboarding consumers
    #[arg(long)]
    pub(crate) json: bool,
    /// Fast device enumeration; avoids bad drivers hanging in full format-range probing
    #[arg(long)]
    pub(crate) fast_devices: bool,
    /// macOS: explicitly trigger a system audio recording permission request/probe; never prompted implicitly by regular doctor
    #[arg(long)]
    pub(crate) request_system_audio: bool,
}

#[derive(Args)]
pub(crate) struct OfflineArgs {
    /// Near-end microphone WAV
    #[arg(long)]
    pub(crate) mic: String,
    /// Far-end reference WAV
    #[arg(long)]
    pub(crate) reference: String,
    /// Output WAV
    #[arg(long)]
    pub(crate) out: String,
    /// Processing chain TOML config (with [[chain]]); when provided, its chain/rate/frame_ms are used
    #[arg(long)]
    pub(crate) config: Option<String>,
    /// Shortcut processor kind, e.g. "aec3" or "localvqe"
    #[arg(long)]
    pub(crate) chain: Option<String>,
    #[arg(long, default_value_t = 48000)]
    pub(crate) rate: u32,
    #[arg(long, default_value_t = 10)]
    pub(crate) frame_ms: u32,
    /// Final output level: 0=mute, 50=unity, 100=3x gain
    #[arg(long)]
    pub(crate) output_level: Option<u32>,
}

#[derive(Args)]
pub(crate) struct RunArgs {
    /// Pipeline TOML config; when omitted, starts from defaults and applies CLI overrides
    #[arg(long)]
    pub(crate) config: Option<String>,
    /// Override microphone device: default, index, or name substring
    #[arg(long)]
    pub(crate) mic: Option<String>,
    /// Override far-end reference source: system, none, output:<name>, input:<name>, index, or name substring
    #[arg(long)]
    pub(crate) reference: Option<String>,
    /// Override output device: default, index, or name substring
    #[arg(long)]
    pub(crate) output: Option<String>,
    /// Override sample rate
    #[arg(long)]
    pub(crate) sample_rate: Option<u32>,
    /// Override frame length (ms)
    #[arg(long)]
    pub(crate) frame_ms: Option<u32>,
    /// Reference channel mode fed into AEC: mono or stereo
    #[arg(long, value_parser = parse_reference_channels)]
    pub(crate) reference_channels: Option<ReferenceChannels>,
    /// Manual alignment delay (ms) applied to near/mic before the processor; macOS defaults to 25, other platforms default to 0
    #[arg(long)]
    pub(crate) near_delay_ms: Option<u32>,
    /// Final output level: 0=mute, 50=unity, 100=3x gain
    #[arg(long)]
    pub(crate) output_level: Option<u32>,
    /// Override processors; repeatable or comma-separated; aec3 is the default recommendation
    #[arg(long, value_delimiter = ',')]
    pub(crate) processor: Vec<String>,
    /// Enable aec3 noise suppression
    #[arg(long)]
    pub(crate) ns: bool,
    /// Disable aec3 noise suppression
    #[arg(long)]
    pub(crate) no_ns: bool,
    /// Override aec3 noise suppression level: low/moderate/high/veryhigh
    #[arg(long)]
    pub(crate) ns_level: Option<String>,
    /// Override aec3 echo tail length (ms)
    #[arg(long)]
    pub(crate) tail_ms: Option<u32>,
    /// Print rolling realtime stats every second
    #[arg(long)]
    pub(crate) verbose: bool,
    /// Custom rolling stats interval (ms); implies --verbose
    #[arg(long)]
    pub(crate) stats_interval_ms: Option<u64>,
    /// Emit JSONL runtime status for GUI/sidecar consumers
    #[arg(long)]
    pub(crate) status_json: bool,
    /// Directory to save realtime diagnostic recordings; a timestamped session is created beneath it
    #[arg(long)]
    pub(crate) diagnostic_dir: Option<String>,
    /// Diagnostic recording cap in seconds; records until stopped when omitted
    #[arg(long)]
    pub(crate) diagnostic_seconds: Option<u32>,
}

fn parse_reference_channels(s: &str) -> Result<ReferenceChannels, String> {
    match s.to_ascii_lowercase().as_str() {
        "mono" | "1" | "1ch" => Ok(ReferenceChannels::Mono),
        "stereo" | "2" | "2ch" => Ok(ReferenceChannels::Stereo),
        _ => Err("must be mono or stereo".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use clap::{error::ErrorKind, Parser};

    use super::Cli;

    #[test]
    fn nvafx_commands_reject_removed_runtime_dir_option() {
        let cases = [
            vec!["echoless", "nvafx", "doctor", "--runtime-dir", "custom"],
            vec![
                "echoless",
                "nvafx",
                "offline",
                "--mic",
                "mic.wav",
                "--reference",
                "ref.wav",
                "--out",
                "out.wav",
                "--runtime-dir",
                "custom",
            ],
            vec![
                "echoless",
                "nvafx",
                "install",
                "--common-zip",
                "common.zip",
                "--model-zip",
                "model.zip",
                "--runtime-dir",
                "custom",
            ],
            vec![
                "echoless",
                "nvafx",
                "download-install",
                "--runtime-dir",
                "custom",
            ],
        ];

        for args in cases {
            let error = match Cli::try_parse_from(args) {
                Ok(_) => panic!("removed --runtime-dir option was accepted"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), ErrorKind::UnknownArgument);
        }
    }
}
