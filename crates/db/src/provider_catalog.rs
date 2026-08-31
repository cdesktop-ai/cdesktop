use serde::{Deserialize, Serialize};

use crate::provider_payloads::{
    ClaudePayload, CodexPayload, DeepseekTuiPayload, GeminiPayload, HermesPayload, OpencodePayload,
};

const CATALOG_JSON: &str = include_str!("provider_catalog.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCatalog {
    pub schema_version: u32,
    pub cc_switch_source_sha: String,
    pub presets: Vec<CatalogPreset>,
}

/// A catalog preset — the shipped recommendation for a provider.
/// `agents[]` is the recommended set; user records use this to seed `perAgentEnabled`.
/// Per-agent payload slots are always present (some may be empty/null when the preset
/// doesn't recommend that agent).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPreset {
    pub id: String,
    pub name: String,
    pub agents: Vec<String>,
    pub claude: ClaudePayload,
    pub codex: CodexPayload,
    pub opencode: OpencodePayload,
    pub deepseek_tui: DeepseekTuiPayload,
    pub gemini: GeminiPayload,
    pub hermes: HermesPayload,
    pub enabled_models: Vec<String>,
}

pub fn load_catalog() -> &'static ProviderCatalog {
    use std::sync::OnceLock;
    static CATALOG: OnceLock<ProviderCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON).expect("provider_catalog.json is malformed")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads() {
        let catalog = load_catalog();
        assert!(
            !catalog.presets.is_empty(),
            "catalog must have at least one preset"
        );
        assert!(
            !catalog.cc_switch_source_sha.is_empty(),
            "catalog must have a SHA pin"
        );
    }
}
