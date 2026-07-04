//! echoless — 跨平台 reference-based AEC 工具 CLI。
//!
//! 当前可用:`processors` / `devices` / `doctor audio` / `offline` / `run` / `nvafx doctor/install/download-install`。
//! 实时主路径走 cpal;主线走经典 AEC3(aec3)保真,LocalVQE 作为独立可选处理器。

mod audio_commands;
mod cli;
mod config_validate;
mod dsp;
mod nvafx_install;
mod offline;
mod probe_delay;
mod processor_manifest;
#[cfg(feature = "realtime")]
mod realtime;
mod run_command;

use anyhow::Result;
use audio_commands::{cmd_devices, cmd_doctor};
use clap::Parser;
use cli::{Cli, Cmd};
use config_validate::cmd_config;
use nvafx_install::cmd_nvafx;
use offline::cmd_offline;
use probe_delay::cmd_probe_delay;
use processor_manifest::cmd_processors;
use run_command::cmd_run;

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
