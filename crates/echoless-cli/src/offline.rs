use anyhow::{bail, Result};

use crate::cli::OfflineArgs;
use crate::config_validate::validate_pipeline_config;
use crate::nvafx_install::validate_nvafx_constraints;
use echoless_audio_io::file::{WavFileSink, WavFileSource};
use echoless_core::{
    default_output_level, run_offline, DiagnosticsConfig, PipelineConfig, ReferenceChannels,
};
use echoless_processors::NodeConfig;

pub(crate) fn cmd_offline(a: OfflineArgs) -> Result<()> {
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
        bypass: false,
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
