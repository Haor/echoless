use anyhow::Result;
use serde_json::json;

use crate::cli::ProcessorsArgs;
use echoless_core::{
    default_near_delay_ms, default_output_level, MAX_INITIAL_DELAY_MS, MAX_NEAR_DELAY_MS,
    MAX_OUTPUT_LEVEL, MIN_OUTPUT_LEVEL, OUTPUT_LEVEL_CURVE_EXPONENT, OUTPUT_LEVEL_MAX_BOOST_DB,
    OUTPUT_LEVEL_MAX_GAIN, UNITY_OUTPUT_LEVEL,
};
use echoless_processors::registry;

pub(crate) fn cmd_processors(args: ProcessorsArgs) -> Result<()> {
    if args.json {
        println!("{}", serde_json::to_string_pretty(&processor_manifest())?);
        return Ok(());
    }
    println!("可用处理器种类:");
    for k in registry::kinds() {
        println!("  - {k}");
    }
    println!("(在 --chain 或 config 的 [[chain]] 里按 kind 引用;默认建议 aec3)");
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
                "kind": "aec3",
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
                        "min": 0,
                        "max": MAX_INITIAL_DELAY_MS,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processor_manifest_exposes_frontend_contract() {
        let manifest = processor_manifest();
        let processors = manifest["processors"].as_array().unwrap();

        let aec3 = processors
            .iter()
            .find(|processor| processor["kind"] == "aec3")
            .unwrap();

        assert_eq!(aec3["default"], true);
        assert_eq!(aec3["params"]["ns"]["default"], false);
        assert_eq!(
            aec3["params"]["reference_channels"]["values"],
            json!(["mono", "stereo"])
        );
        assert_eq!(aec3["params"]["initial_delay_ms"]["min"], json!(0));
        assert_eq!(
            aec3["params"]["initial_delay_ms"]["max"],
            json!(MAX_INITIAL_DELAY_MS)
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
