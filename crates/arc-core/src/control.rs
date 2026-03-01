use crate::{compiled::CompiledConfig, config::ArcConfig, SharedConfig};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Control plane shared state.
#[derive(Clone)]
pub struct ControlState {
    pub cfg: Arc<SharedConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigPushRequest {
    pub config: ArcConfig,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigPushResponse {
    pub generation: Uuid,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusResponse {
    pub generation: Uuid,
    pub node_id: String,
}

impl ControlState {
    pub fn apply_config(&self, cfg: ArcConfig) -> anyhow::Result<Uuid> {
        let compiled = CompiledConfig::compile(cfg)?;
        let gen = compiled.generation;
        self.cfg.swap(Arc::new(compiled));
        Ok(gen)
    }
}
