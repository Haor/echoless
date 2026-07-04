//! 处理器注册表:kind 字符串 → `Box<dyn EchoProcessor>`。新增方案在此登记一行。

use crate::{
    aec3::Aec3Engine, localvqe::LocalVqe, nvafx::NvidiaAfxAec, passthrough::Passthrough,
    EchoProcessor,
};

pub fn build(kind: &str) -> anyhow::Result<Box<dyn EchoProcessor>> {
    Ok(match kind {
        "passthrough" => Box::new(Passthrough::new()),
        "aec3" => Box::new(Aec3Engine::new()),
        "sonora_aec3" => Box::new(Aec3Engine::new()), // legacy alias, remove after 2 releases
        "localvqe" => Box::new(LocalVqe::new()),
        "nvidia_afx_aec" => Box::new(NvidiaAfxAec::new()),
        other => anyhow::bail!(
            "未知处理器 kind: {other}(可用: passthrough / aec3 / localvqe / nvidia_afx_aec)"
        ),
    })
}

/// 已注册的处理器种类(供 CLI/前端列出)。
pub fn kinds() -> &'static [&'static str] {
    &["passthrough", "aec3", "localvqe", "nvidia_afx_aec"]
}
