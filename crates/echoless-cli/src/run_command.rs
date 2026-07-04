use anyhow::{bail, Result};

use crate::cli::RunArgs;
use crate::nvafx_install::validate_nvafx_constraints;
#[cfg(feature = "realtime")]
use crate::realtime;
use echoless_core::{apply_reference_channels_to_chain, PipelineConfig};
use echoless_processors::NodeConfig;

#[cfg(feature = "realtime")]
pub(crate) fn cmd_run(a: RunArgs) -> Result<()> {
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
pub(crate) fn cmd_run(_a: RunArgs) -> Result<()> {
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
        set_aec3_param(&mut cfg.chain, "ns", toml::Value::Boolean(true))?;
    }
    if a.no_ns {
        set_aec3_param(&mut cfg.chain, "ns", toml::Value::Boolean(false))?;
    }
    if let Some(level) = &a.ns_level {
        set_aec3_param(&mut cfg.chain, "ns", toml::Value::Boolean(true))?;
        set_aec3_param(
            &mut cfg.chain,
            "ns_level",
            toml::Value::String(level.clone()),
        )?;
    }
    if let Some(tail_ms) = a.tail_ms {
        set_aec3_param(
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

#[cfg_attr(not(feature = "realtime"), allow(dead_code))]
fn set_aec3_param(nodes: &mut [NodeConfig], key: &str, value: toml::Value) -> Result<()> {
    let Some(node) = nodes.iter_mut().find(|node| {
        node.kind == "aec3" || node.kind == "sonora_aec3" // legacy alias, remove after 2 releases
    }) else {
        bail!("{key} 需要配置中存在 aec3 节点,或使用 --processor aec3");
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
        args.processor = vec!["aec3".into()];
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
        assert_eq!(cfg.chain[0].kind, "aec3");
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
    fn run_overrides_reject_aec3_flags_without_aec3_node() {
        let mut args = run_args();
        args.tail_ms = Some(120);

        let err = apply_run_overrides(PipelineConfig::default(), &args).unwrap_err();

        assert!(err.to_string().contains("aec3"));
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
}
