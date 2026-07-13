//! Product-level external noise-suppression compatibility rules.

use crate::{registry, NodeConfig};

pub const WEBRTC_MODE: &str = "webrtc";
pub const RNNOISE_MODE: &str = "rnnoise";
pub const OFF_MODE: &str = "off";
pub const WEBRTC_PROCESSOR_KIND: &str = "webrtc_ns";
pub const RNNOISE_PROCESSOR_KIND: &str = "rnnoise";

pub const LOCALVQE_V12_MODEL: &str = "localvqe-v1.2-1.3M-f32.gguf";
pub const LOCALVQE_V13_MODEL: &str = "localvqe-v1.3-4.8M-f32.gguf";
pub const LOCALVQE_V14_MODEL: &str = "localvqe-v1.4-aec-200K-f32.gguf";

pub const ALL_NOISE_MODES: &[&str] = &[WEBRTC_MODE, RNNOISE_MODE, OFF_MODE];
pub const OFF_ONLY_NOISE_MODES: &[&str] = &[OFF_MODE];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalVqeModelCapability {
    BuiltInNoiseSuppression,
    PureAec,
    Unknown,
}

impl LocalVqeModelCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BuiltInNoiseSuppression => "built_in_ns",
            Self::PureAec => "pure_aec",
            Self::Unknown => "unknown",
        }
    }

    pub fn allowed_noise_modes(self) -> &'static [&'static str] {
        match self {
            Self::PureAec => ALL_NOISE_MODES,
            Self::BuiltInNoiseSuppression | Self::Unknown => OFF_ONLY_NOISE_MODES,
        }
    }

    pub fn allows_external_noise_suppression(self) -> bool {
        self == Self::PureAec
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoiseChainError {
    pub node_index: usize,
    pub message: String,
}

pub fn localvqe_model_capability(model_path: &str) -> LocalVqeModelCapability {
    let file_name = model_path.rsplit(['/', '\\']).next().unwrap_or(model_path);
    if file_name.eq_ignore_ascii_case(LOCALVQE_V12_MODEL)
        || file_name.eq_ignore_ascii_case(LOCALVQE_V13_MODEL)
    {
        LocalVqeModelCapability::BuiltInNoiseSuppression
    } else if file_name.eq_ignore_ascii_case(LOCALVQE_V14_MODEL) {
        LocalVqeModelCapability::PureAec
    } else {
        LocalVqeModelCapability::Unknown
    }
}

pub fn is_external_noise_suppression_kind(kind: &str) -> bool {
    matches!(
        registry::canonical_kind(kind),
        WEBRTC_PROCESSOR_KIND | RNNOISE_PROCESSOR_KIND
    )
}

pub fn validate_noise_suppression_chain(nodes: &[NodeConfig]) -> Vec<NoiseChainError> {
    let ns_indices = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| is_external_noise_suppression_kind(&node.kind).then_some(index))
        .collect::<Vec<_>>();
    let mut errors = Vec::new();

    for &index in ns_indices.iter().skip(1) {
        errors.push(NoiseChainError {
            node_index: index,
            message: "only one external noise suppression node is allowed".into(),
        });
    }

    if let Some(&ns_index) = ns_indices.first() {
        for node in nodes {
            if registry::canonical_kind(&node.kind) != "localvqe" {
                continue;
            }
            let model = node
                .params
                .get("model")
                .and_then(toml::Value::as_str)
                .unwrap_or_default();
            let capability = localvqe_model_capability(model);
            if !capability.allows_external_noise_suppression() {
                errors.push(NoiseChainError {
                    node_index: ns_index,
                    message: format!(
                        "LocalVQE model {model:?} does not allow external noise suppression"
                    ),
                });
                break;
            }
        }
    }

    for &index in &ns_indices {
        let compatible = index
            .checked_sub(1)
            .and_then(|engine_index| nodes.get(engine_index))
            .is_some_and(engine_allows_external_noise_suppression);
        if !compatible {
            errors.push(NoiseChainError {
                node_index: index,
                message: format!(
                    "{} must immediately follow a compatible AEC engine",
                    nodes[index].kind
                ),
            });
        }
    }

    errors
}

fn engine_allows_external_noise_suppression(node: &NodeConfig) -> bool {
    match registry::canonical_kind(&node.kind) {
        "aec3" | "nvidia_afx_aec" => true,
        "localvqe" => node
            .params
            .get("model")
            .and_then(toml::Value::as_str)
            .map(localvqe_model_capability)
            .is_some_and(LocalVqeModelCapability::allows_external_noise_suppression),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: &str) -> NodeConfig {
        NodeConfig {
            kind: kind.into(),
            params: toml::Table::new(),
        }
    }

    fn localvqe(model: &str) -> NodeConfig {
        let mut node = node("localvqe");
        node.params
            .insert("model".into(), toml::Value::String(model.into()));
        node
    }

    #[test]
    fn classifies_only_declared_localvqe_models_as_external_ns_capable() {
        assert_eq!(
            localvqe_model_capability(&format!("C:\\models\\{LOCALVQE_V12_MODEL}")),
            LocalVqeModelCapability::BuiltInNoiseSuppression
        );
        assert_eq!(
            localvqe_model_capability(&format!("/models/{LOCALVQE_V13_MODEL}")),
            LocalVqeModelCapability::BuiltInNoiseSuppression
        );
        assert_eq!(
            localvqe_model_capability(LOCALVQE_V14_MODEL),
            LocalVqeModelCapability::PureAec
        );
        assert_eq!(
            localvqe_model_capability("custom-aec-model.gguf"),
            LocalVqeModelCapability::Unknown
        );
    }

    #[test]
    fn accepts_external_ns_after_each_compatible_engine() {
        for engine in [
            node("aec3"),
            node("nvidia_afx_aec"),
            localvqe(LOCALVQE_V14_MODEL),
        ] {
            for ns_kind in [WEBRTC_PROCESSOR_KIND, RNNOISE_PROCESSOR_KIND] {
                assert!(
                    validate_noise_suppression_chain(&[engine.clone(), node(ns_kind)]).is_empty()
                );
            }
        }
    }

    #[test]
    fn rejects_external_ns_for_built_in_or_unknown_localvqe_models() {
        for model in [LOCALVQE_V12_MODEL, LOCALVQE_V13_MODEL, "custom.gguf"] {
            let errors =
                validate_noise_suppression_chain(&[localvqe(model), node(WEBRTC_PROCESSOR_KIND)]);
            assert!(!errors.is_empty(), "model {model} unexpectedly accepted");
        }
    }

    #[test]
    fn rejects_orphaned_and_multiple_external_ns_nodes() {
        assert!(!validate_noise_suppression_chain(&[node(WEBRTC_PROCESSOR_KIND)]).is_empty());
        assert!(!validate_noise_suppression_chain(&[
            node("aec3"),
            node(WEBRTC_PROCESSOR_KIND),
            node(RNNOISE_PROCESSOR_KIND),
        ])
        .is_empty());
    }
}
