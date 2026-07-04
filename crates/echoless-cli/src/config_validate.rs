use std::path::Path;

use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use serde_json::json;

use echoless_core::{
    PipelineConfig, ReferenceChannels, MAX_INITIAL_DELAY_MS, MAX_NEAR_DELAY_MS, MAX_OUTPUT_LEVEL,
    MIN_OUTPUT_LEVEL,
};
use echoless_processors::{registry, NodeConfig};

#[derive(Args)]
pub(crate) struct ConfigArgs {
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

pub(crate) fn cmd_config(args: ConfigArgs) -> Result<()> {
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
pub(crate) struct ConfigValidationError {
    pub(crate) path: String,
    pub(crate) message: String,
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

pub(crate) fn validate_pipeline_config(cfg: &PipelineConfig) -> Vec<ConfigValidationError> {
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
    if !is_known_processor_kind(&node.kind) {
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
        "aec3" => validate_aec3_node(&base, &node.params, errors),
        "sonora_aec3" => validate_aec3_node(&base, &node.params, errors), // legacy alias, remove after 2 releases
        "localvqe" => validate_localvqe_node(&base, &node.params, errors),
        "nvidia_afx_aec" => validate_nvafx_node(cfg, &base, &node.params, errors),
        "passthrough" => {}
        _ => {}
    }
}

fn is_known_processor_kind(kind: &str) -> bool {
    registry::kinds().contains(&kind) || kind == "sonora_aec3" // legacy alias, remove after 2 releases
}

fn validate_aec3_node(base: &str, params: &toml::Table, errors: &mut Vec<ConfigValidationError>) {
    expect_bool(params, base, "ns", errors);
    expect_bool(params, base, "agc", errors);
    expect_bool(params, base, "linear_stable_echo_path", errors);
    expect_i64_range(
        params,
        base,
        "initial_delay_ms",
        0,
        i64::from(MAX_INITIAL_DELAY_MS),
        errors,
    );
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

fn expect_i64_range(
    params: &toml::Table,
    base: &str,
    key: &str,
    min: i64,
    max: i64,
    errors: &mut Vec<ConfigValidationError>,
) {
    if let Some(value) = params.get(key) {
        match value.as_integer() {
            Some(v) if (min..=max).contains(&v) => {}
            Some(_) => errors.push(ConfigValidationError::new(
                format!("{base}.{key}"),
                format!("{key} must be between {min} and {max}"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use echoless_core::{default_near_delay_ms, default_output_level};

    #[test]
    fn config_validation_accepts_default_aec3_baseline() {
        let cfg = PipelineConfig {
            chain: vec![NodeConfig {
                kind: "aec3".into(),
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
            kind = "aec3"
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
        bad_params.insert(
            "initial_delay_ms".into(),
            toml::Value::Integer(i64::from(MAX_INITIAL_DELAY_MS) + 1),
        );
        bad_params.insert("tail_ms".into(), toml::Value::Integer(1));
        bad_params.insert("ns".into(), toml::Value::String("yes".into()));
        let cfg = PipelineConfig {
            sample_rate: 44_100,
            near_delay_ms: MAX_NEAR_DELAY_MS + 1,
            output_level: MAX_OUTPUT_LEVEL + 1,
            reference_channels: ReferenceChannels::Stereo,
            chain: vec![
                NodeConfig {
                    kind: "aec3".into(),
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
        assert!(paths.contains(&"chain[0].initial_delay_ms"));
        assert!(paths.contains(&"chain[0].ns"));
        assert!(paths.contains(&"chain[2].kind"));
        assert!(paths.contains(&"sample_rate"));
        assert!(paths.contains(&"near_delay_ms"));
        assert!(paths.contains(&"output_level"));
        assert!(paths.contains(&"reference_channels"));
        assert!(paths.contains(&"chain[1].doctor"));
    }
}
