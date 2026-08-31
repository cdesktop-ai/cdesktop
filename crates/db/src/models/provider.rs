use std::collections::HashMap;

use chrono::{DateTime, Utc};
use executors::{
    executors::{BaseCodingAgent, CodingAgent, StandardCodingAgentExecutor},
    provider::{ProviderContext, ProviderInjection, ProviderInjectionError, ProviderPayload},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use thiserror::Error;
use ts_rs::TS;
use uuid::Uuid;

use crate::provider_payloads::{
    ClaudePayload, CodexPayload, DeepseekTuiPayload, GeminiPayload, HermesPayload, OpencodePayload,
};

// Spawn-time provider applier real-world verification status (per
// multi-agent-routing.md verification matrix at §7):
//   - Phase C (Codex): unit tests cover env+config-overrides shape; a real
//     spawn against an OpenRouter Codex provider with diff of `~/.codex/`
//     before/after is still pending.
//   - Phase D (OpenCode): unit tests cover JSON shape + env overlay
//     ordering; a real spawn against an OpenRouter (Anthropic-compat)
//     OpenCode provider with diff of `~/.config/opencode/` before/after is
//     still pending.
//   - Phase F (Gemini): unit tests cover env shape + overlay ordering; a
//     real spawn against a user-supplied Google-API-compatible Custom
//     record with diff of `~/.gemini/` before/after is still pending.
//     Note: catalog ships no Gemini presets (plan §3.1), so verification
//     needs a *manually-created* Custom record — no preset path to
//     instantiate from, unlike Phases C/D.
// Tracked here so the gap is visible from the appliers themselves.

pub const DEFAULT_PROVIDER_ID: &str = "00000000-0000-0000-0000-000000000001";

/// Kind of AI routing provider. PascalCase end-to-end (wire + DB CHECK constraint).
/// Renamed in TypeScript to `AiProviderKind` to avoid collision with the git-host
/// `ProviderKind` already in `shared/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(rename = "AiProviderKind")]
pub enum AiProviderKind {
    Default,
    Preset,
    Custom,
}

impl std::fmt::Display for AiProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiProviderKind::Default => write!(f, "Default"),
            AiProviderKind::Preset => write!(f, "Preset"),
            AiProviderKind::Custom => write!(f, "Custom"),
        }
    }
}

impl std::str::FromStr for AiProviderKind {
    type Err = ProviderError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Default" => Ok(AiProviderKind::Default),
            "Preset" => Ok(AiProviderKind::Preset),
            "Custom" => Ok(AiProviderKind::Custom),
            _ => Err(ProviderError::InvalidKind(s.to_string())),
        }
    }
}

// Keep a type alias so call sites that used `ProviderKind` still compile.
pub type ProviderKind = AiProviderKind;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EnabledModel {
    pub id: String,
    pub display_name: String,
    pub owned_by: Option<String>,
}

/// User provider record — persistent shape stored in the `providers` table.
/// `apiKey` and `perAgentEnabled` are top-level; per-agent payloads are nested.
/// Picker visibility and spawn-time injection both read this struct directly.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: Uuid,
    pub name: String,
    pub kind: AiProviderKind,
    pub preset_id: Option<String>,
    pub enabled: bool,
    pub api_key: Option<String>,
    /// Map<agent_enum_name, bool>. Single source of truth for picker visibility
    /// per plan §3.2. Keys span the full agent enum.
    pub per_agent_enabled: HashMap<String, bool>,
    pub claude: ClaudePayload,
    pub codex: CodexPayload,
    pub opencode: OpencodePayload,
    pub deepseek_tui: DeepseekTuiPayload,
    pub gemini: GeminiPayload,
    pub hermes: HermesPayload,
    pub enabled_models: Vec<EnabledModel>,
    #[ts(type = "Date")]
    pub created_at: DateTime<Utc>,
    #[ts(type = "Date")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CreateProvider {
    pub name: String,
    pub kind: AiProviderKind,
    pub preset_id: Option<String>,
    pub api_key: Option<String>,
    pub per_agent_enabled: HashMap<String, bool>,
    #[serde(default)]
    pub claude: ClaudePayload,
    #[serde(default)]
    pub codex: CodexPayload,
    #[serde(default)]
    pub opencode: OpencodePayload,
    #[serde(default)]
    pub deepseek_tui: DeepseekTuiPayload,
    #[serde(default)]
    pub gemini: GeminiPayload,
    #[serde(default)]
    pub hermes: HermesPayload,
    pub enabled_models: Vec<EnabledModel>,
}

/// Update payload — full replacement (frontend always supplies the complete
/// state). `kind` is intentionally absent (sticky once set).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProvider {
    pub name: String,
    pub preset_id: Option<String>,
    pub enabled: bool,
    pub api_key: Option<String>,
    pub per_agent_enabled: HashMap<String, bool>,
    #[serde(default)]
    pub claude: ClaudePayload,
    #[serde(default)]
    pub codex: CodexPayload,
    #[serde(default)]
    pub opencode: OpencodePayload,
    #[serde(default)]
    pub deepseek_tui: DeepseekTuiPayload,
    #[serde(default)]
    pub gemini: GeminiPayload,
    #[serde(default)]
    pub hermes: HermesPayload,
    pub enabled_models: Vec<EnabledModel>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("Provider not found")]
    NotFound,
    #[error("Invalid provider kind: {0}")]
    InvalidKind(String),
    #[error("Invalid UUID: {0}")]
    InvalidUuid(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Cannot delete the Default provider")]
    CannotDeleteDefault,
    #[error("enabledModels must not be empty")]
    EmptyEnabledModels,
    #[error("agent {0} is enabled but its baseUrl is empty")]
    EnabledAgentMissingBaseUrl(String),
    #[error("agent {0} is enabled but apiKey is empty")]
    MissingApiKey(String),
    #[error("no adapter is registered for agent {0}")]
    UnregisteredAgent(BaseCodingAgent),
    #[error(transparent)]
    Injection(#[from] ProviderInjectionError),
}

// Raw row returned from SQLite — JSON fields stored as TEXT.
#[derive(Debug, FromRow)]
struct ProviderRow {
    id: String,
    name: String,
    kind: String,
    preset_id: Option<String>,
    enabled: bool,
    api_key: Option<String>,
    per_agent_enabled: String,
    claude: String,
    codex: String,
    opencode: String,
    deepseek_tui: String,
    gemini: String,
    hermes: String,
    enabled_models: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

struct EnabledAgentPayloads<'a> {
    per_agent_enabled: &'a HashMap<String, bool>,
    claude: &'a ClaudePayload,
    codex: &'a CodexPayload,
    opencode: &'a OpencodePayload,
    deepseek_tui: &'a DeepseekTuiPayload,
    gemini: &'a GeminiPayload,
    hermes: &'a HermesPayload,
}

impl<'a> From<&'a CreateProvider> for EnabledAgentPayloads<'a> {
    fn from(provider: &'a CreateProvider) -> Self {
        Self {
            per_agent_enabled: &provider.per_agent_enabled,
            claude: &provider.claude,
            codex: &provider.codex,
            opencode: &provider.opencode,
            deepseek_tui: &provider.deepseek_tui,
            gemini: &provider.gemini,
            hermes: &provider.hermes,
        }
    }
}

impl<'a> From<&'a UpdateProvider> for EnabledAgentPayloads<'a> {
    fn from(provider: &'a UpdateProvider) -> Self {
        Self {
            per_agent_enabled: &provider.per_agent_enabled,
            claude: &provider.claude,
            codex: &provider.codex,
            opencode: &provider.opencode,
            deepseek_tui: &provider.deepseek_tui,
            gemini: &provider.gemini,
            hermes: &provider.hermes,
        }
    }
}

impl TryFrom<ProviderRow> for Provider {
    type Error = ProviderError;

    fn try_from(r: ProviderRow) -> Result<Self, ProviderError> {
        let kind: AiProviderKind = r.kind.parse()?;
        let mut enabled_models: Vec<EnabledModel> = serde_json::from_str(&r.enabled_models)?;
        // Default provider has no DB-stored model list; synthesize it from the
        // Claude executor's canonical alias list at read time, so updates ship
        // with the binary instead of needing a migration.
        if matches!(kind, AiProviderKind::Default) && enabled_models.is_empty() {
            enabled_models = executors::executors::claude::DEFAULT_MODEL_IDS
                .iter()
                .map(|(id, name)| EnabledModel {
                    id: (*id).to_string(),
                    display_name: (*name).to_string(),
                    owned_by: Some("anthropic".to_string()),
                })
                .collect();
        }
        Ok(Provider {
            id: r
                .id
                .parse()
                .map_err(|_| ProviderError::InvalidUuid(r.id.clone()))?,
            name: r.name,
            kind,
            preset_id: r.preset_id,
            enabled: r.enabled,
            api_key: r.api_key,
            per_agent_enabled: serde_json::from_str(&r.per_agent_enabled)?,
            claude: serde_json::from_str(&r.claude)?,
            codex: serde_json::from_str(&r.codex)?,
            opencode: serde_json::from_str(&r.opencode)?,
            deepseek_tui: serde_json::from_str(&r.deepseek_tui)?,
            gemini: serde_json::from_str(&r.gemini)?,
            hermes: serde_json::from_str(&r.hermes)?,
            enabled_models,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
    }
}

impl Provider {
    pub async fn list(pool: &SqlitePool) -> Result<Vec<Self>, ProviderError> {
        let rows = sqlx::query_as!(
            ProviderRow,
            r#"SELECT
                id, name, kind, preset_id,
                enabled as "enabled!: bool",
                api_key, per_agent_enabled,
                claude, codex, opencode, deepseek_tui, gemini, hermes,
                enabled_models,
                created_at as "created_at!: DateTime<Utc>",
                updated_at as "updated_at!: DateTime<Utc>"
               FROM providers
               ORDER BY
                CASE kind WHEN 'Default' THEN 0 ELSE 1 END,
                created_at ASC"#
        )
        .fetch_all(pool)
        .await?;

        rows.into_iter().map(Provider::try_from).collect()
    }

    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Self, ProviderError> {
        let id_str = id.to_string();
        let row = sqlx::query_as!(
            ProviderRow,
            r#"SELECT
                id, name, kind, preset_id,
                enabled as "enabled!: bool",
                api_key, per_agent_enabled,
                claude, codex, opencode, deepseek_tui, gemini, hermes,
                enabled_models,
                created_at as "created_at!: DateTime<Utc>",
                updated_at as "updated_at!: DateTime<Utc>"
               FROM providers WHERE id = $1"#,
            id_str
        )
        .fetch_optional(pool)
        .await?
        .ok_or(ProviderError::NotFound)?;

        Provider::try_from(row)
    }

    /// Reject empty `enabledModels` — see plan §3.6 validation rule (defense in
    /// depth so a misbehaving client can't persist a record OpenCode would
    /// silently delete at runtime).
    fn validate_enabled_models(
        kind: &AiProviderKind,
        models: &[EnabledModel],
    ) -> Result<(), ProviderError> {
        // Default singleton carries no model list (synthesized at read).
        if matches!(kind, AiProviderKind::Default) {
            return Ok(());
        }
        if models.is_empty() {
            return Err(ProviderError::EmptyEnabledModels);
        }
        Ok(())
    }

    /// Reject save when an agent is enabled but its `base_url` is missing —
    /// otherwise the spawn applier silently falls through to the user's
    /// ambient config (defeating the point of cdesktop-managed routing).
    /// The Default provider is exempt: it carries no per-agent payloads.
    fn validate_enabled_agent_payloads(
        kind: &AiProviderKind,
        payloads: EnabledAgentPayloads<'_>,
    ) -> Result<(), ProviderError> {
        if matches!(kind, AiProviderKind::Default) {
            return Ok(());
        }
        let is_on = |k: &str| payloads.per_agent_enabled.get(k).copied().unwrap_or(false);
        let has = |s: &Option<String>| s.as_deref().is_some_and(|v| !v.is_empty());
        let missing = |agent: &str, ok: bool| {
            if ok {
                Ok(())
            } else {
                Err(ProviderError::EnabledAgentMissingBaseUrl(agent.to_string()))
            }
        };
        if is_on("CLAUDE_CODE") {
            missing("CLAUDE_CODE", has(&payloads.claude.base_url))?;
        }
        if is_on("CODEX") {
            missing("CODEX", has(&payloads.codex.base_url))?;
        }
        if is_on("OPENCODE") {
            missing("OPENCODE", has(&payloads.opencode.base_url))?;
        }
        if is_on("DEEPSEEK_TUI") {
            missing("DEEPSEEK_TUI", has(&payloads.deepseek_tui.base_url))?;
        }
        if is_on("GEMINI") {
            missing("GEMINI", has(&payloads.gemini.base_url))?;
        }
        if is_on("HERMES") {
            missing("HERMES", has(&payloads.hermes.base_url))?;
        }
        Ok(())
    }

    pub async fn create(
        pool: &SqlitePool,
        id: Uuid,
        data: &CreateProvider,
    ) -> Result<Self, ProviderError> {
        Self::validate_enabled_models(&data.kind, &data.enabled_models)?;
        Self::validate_enabled_agent_payloads(&data.kind, data.into())?;

        let id_str = id.to_string();
        let kind_str = data.kind.to_string();
        let per_agent_enabled = serde_json::to_string(&data.per_agent_enabled)?;
        let claude_str = serde_json::to_string(&data.claude)?;
        let codex_str = serde_json::to_string(&data.codex)?;
        let opencode_str = serde_json::to_string(&data.opencode)?;
        let deepseek_tui_str = serde_json::to_string(&data.deepseek_tui)?;
        let gemini_str = serde_json::to_string(&data.gemini)?;
        let hermes_str = serde_json::to_string(&data.hermes)?;
        let enabled_models_str = serde_json::to_string(&data.enabled_models)?;

        let row = sqlx::query_as!(
            ProviderRow,
            r#"INSERT INTO providers (
                id, name, kind, preset_id, enabled,
                api_key, per_agent_enabled,
                claude, codex, opencode, deepseek_tui, gemini, hermes,
                enabled_models
               )
               VALUES ($1, $2, $3, $4, 1, $5, $6, $7, $8, $9, $10, $11, $12, $13)
               RETURNING
                id, name, kind, preset_id,
                enabled as "enabled!: bool",
                api_key, per_agent_enabled,
                claude, codex, opencode, deepseek_tui, gemini, hermes,
                enabled_models,
                created_at as "created_at!: DateTime<Utc>",
                updated_at as "updated_at!: DateTime<Utc>""#,
            id_str,
            data.name,
            kind_str,
            data.preset_id,
            data.api_key,
            per_agent_enabled,
            claude_str,
            codex_str,
            opencode_str,
            deepseek_tui_str,
            gemini_str,
            hermes_str,
            enabled_models_str,
        )
        .fetch_one(pool)
        .await?;

        Provider::try_from(row)
    }

    pub async fn update(
        pool: &SqlitePool,
        id: Uuid,
        data: &UpdateProvider,
    ) -> Result<Self, ProviderError> {
        // Look up kind to know whether to enforce the non-empty rule (Default exempt).
        let existing = Self::find_by_id(pool, id).await?;
        Self::validate_enabled_models(&existing.kind, &data.enabled_models)?;
        Self::validate_enabled_agent_payloads(&existing.kind, data.into())?;

        let id_str = id.to_string();
        let per_agent_enabled = serde_json::to_string(&data.per_agent_enabled)?;
        let claude_str = serde_json::to_string(&data.claude)?;
        let codex_str = serde_json::to_string(&data.codex)?;
        let opencode_str = serde_json::to_string(&data.opencode)?;
        let deepseek_tui_str = serde_json::to_string(&data.deepseek_tui)?;
        let gemini_str = serde_json::to_string(&data.gemini)?;
        let hermes_str = serde_json::to_string(&data.hermes)?;
        let enabled_models_str = serde_json::to_string(&data.enabled_models)?;

        let row = sqlx::query_as!(
            ProviderRow,
            r#"UPDATE providers
               SET name = $1, preset_id = $2, enabled = $3,
                   api_key = $4, per_agent_enabled = $5,
                   claude = $6, codex = $7, opencode = $8,
                   deepseek_tui = $9, gemini = $10, hermes = $11,
                   enabled_models = $12,
                   updated_at = datetime('now')
               WHERE id = $13
               RETURNING
                id, name, kind, preset_id,
                enabled as "enabled!: bool",
                api_key, per_agent_enabled,
                claude, codex, opencode, deepseek_tui, gemini, hermes,
                enabled_models,
                created_at as "created_at!: DateTime<Utc>",
                updated_at as "updated_at!: DateTime<Utc>""#,
            data.name,
            data.preset_id,
            data.enabled,
            data.api_key,
            per_agent_enabled,
            claude_str,
            codex_str,
            opencode_str,
            deepseek_tui_str,
            gemini_str,
            hermes_str,
            enabled_models_str,
            id_str,
        )
        .fetch_optional(pool)
        .await?
        .ok_or(ProviderError::NotFound)?;

        Provider::try_from(row)
    }

    pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<(), ProviderError> {
        if id.to_string() == DEFAULT_PROVIDER_ID {
            return Err(ProviderError::CannotDeleteDefault);
        }
        let id_str = id.to_string();
        sqlx::query!("DELETE FROM providers WHERE id = $1", id_str)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub fn is_default(&self) -> bool {
        self.kind == AiProviderKind::Default
    }

    /// The adapter registered for `agent`.
    ///
    /// Every harness convention this record feeds — which slot holds its
    /// credentials, what its model ids look like, what its spawn injection
    /// contains — is the adapter's to decide. An unregistered harness has no
    /// adapter, and a record cannot be routed to one.
    fn adapter(agent: BaseCodingAgent) -> Result<CodingAgent, ProviderError> {
        CodingAgent::registered(agent).ok_or(ProviderError::UnregisteredAgent(agent))
    }

    /// This record as `agent`'s adapter sees it.
    ///
    /// The adapter names its own slot; the record only knows how to hand one
    /// over. Credential precedence stays here because it is a property of the
    /// record, not of any harness: a slot-level `apiKey` overrides the
    /// record-level one, which is how aggregators (e.g. Packy Code) issue
    /// distinct keys per harness while the common case sets a single key.
    fn context_for(
        &self,
        adapter: &CodingAgent,
        model_id: &str,
    ) -> Result<ProviderContext, ProviderError> {
        let record = serde_json::to_value(self)?;
        let payload = ProviderPayload::from_slot(record.get(adapter.provider_slot()));
        let api_key = payload
            .api_key
            .clone()
            .or_else(|| self.api_key.clone().filter(|key| !key.is_empty()));
        Ok(ProviderContext {
            ambient: self.kind == AiProviderKind::Default,
            record_name: self.name.clone(),
            slug: self
                .preset_id
                .clone()
                .unwrap_or_else(|| "custom".to_string()),
            api_key,
            payload,
            enabled_models: self.enabled_models.iter().map(|m| m.id.clone()).collect(),
            model_id: model_id.to_string(),
        })
    }

    /// The credential `agent` would spawn with, for redaction.
    pub fn resolved_api_key(&self, agent: BaseCodingAgent) -> Option<String> {
        let adapter = Self::adapter(agent).ok()?;
        self.context_for(&adapter, "").ok()?.api_key
    }

    /// The picker-selected model id in `agent`'s own id form.
    pub fn provider_model_id(
        &self,
        agent: BaseCodingAgent,
        model_id: &str,
    ) -> Result<String, ProviderError> {
        let adapter = Self::adapter(agent)?;
        Ok(adapter.provider_model_id(&self.context_for(&adapter, model_id)?))
    }

    /// Both halves of a spawn against this record: the model id in the
    /// harness's own form, and the injection its adapter builds.
    ///
    /// The model id is resolved first so the injection sees the id the harness
    /// will actually be asked for.
    pub fn resolve_injection(
        &self,
        agent: BaseCodingAgent,
        model_id: &str,
    ) -> Result<(String, ProviderInjection), ProviderError> {
        let adapter = Self::adapter(agent)?;
        let mut ctx = self.context_for(&adapter, model_id)?;
        ctx.model_id = adapter.provider_model_id(&ctx);
        if ctx.ambient {
            // Ambient records stop here, so no adapter can inject over a
            // harness's own credentials by forgetting to check.
            return Ok((ctx.model_id, ProviderInjection::default()));
        }
        let injection = adapter.build_provider_injection(&ctx)?;
        Ok((ctx.model_id, injection))
    }
}

#[cfg(test)]
mod codex_injection_tests {
    use executors::executors::codex::CodexProviderInjection;
    use serde_json::Value as JsonValue;

    use super::*;

    fn provider_with_codex(
        kind: AiProviderKind,
        api_key: Option<&str>,
        base_url: Option<&str>,
        codex_env: HashMap<String, String>,
    ) -> Provider {
        Provider {
            id: Uuid::new_v4(),
            name: "Test Provider".to_string(),
            kind,
            preset_id: None,
            enabled: true,
            api_key: api_key.map(|s| s.to_string()),
            per_agent_enabled: HashMap::new(),
            claude: ClaudePayload::default(),
            codex: CodexPayload {
                base_url: base_url.map(|s| s.to_string()),
                api_key: None,
                env: codex_env,
            },
            opencode: OpencodePayload::default(),
            deepseek_tui: DeepseekTuiPayload::default(),
            gemini: GeminiPayload::default(),
            hermes: HermesPayload::default(),
            enabled_models: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Drive the record through the same path a spawn takes: adapter lookup,
    /// context, injection. Nothing here names a Codex-side convention.
    fn injection(p: &Provider) -> Result<ProviderInjection, ProviderError> {
        p.resolve_injection(BaseCodingAgent::Codex, "")
            .map(|(_, injection)| injection)
    }

    fn env_of(p: &Provider) -> HashMap<String, String> {
        injection(p)
            .expect("injection builds")
            .env
            .expect("non-ambient record emits env")
    }

    #[test]
    fn default_returns_none() {
        let p = provider_with_codex(
            AiProviderKind::Default,
            Some("ignored"),
            Some("ignored"),
            HashMap::new(),
        );
        let injection = injection(&p).unwrap();
        assert!(injection.env.is_none());
        assert!(injection.structured.is_none());
    }

    #[test]
    fn missing_api_key_rejected() {
        let p = provider_with_codex(
            AiProviderKind::Preset,
            None,
            Some("https://example.com/v1"),
            HashMap::new(),
        );
        assert!(matches!(
            injection(&p),
            Err(ProviderError::Injection(
                ProviderInjectionError::MissingApiKey(_)
            ))
        ));
    }

    #[test]
    fn empty_api_key_rejected() {
        let p = provider_with_codex(
            AiProviderKind::Preset,
            Some(""),
            Some("https://example.com/v1"),
            HashMap::new(),
        );
        assert!(matches!(
            injection(&p),
            Err(ProviderError::Injection(
                ProviderInjectionError::MissingApiKey(_)
            ))
        ));
    }

    #[test]
    fn missing_base_url_rejected() {
        let p = provider_with_codex(
            AiProviderKind::Preset,
            Some("sk-test"),
            None,
            HashMap::new(),
        );
        assert!(matches!(
            injection(&p),
            Err(ProviderError::Injection(
                ProviderInjectionError::MissingBaseUrl(_)
            ))
        ));
    }

    #[test]
    fn structural_keys_emitted() {
        let p = provider_with_codex(
            AiProviderKind::Preset,
            Some("sk-test"),
            Some("https://openrouter.ai/api/v1"),
            HashMap::new(),
        );
        let built = injection(&p).unwrap();
        let env = built.env.expect("non-ambient record emits env");
        let codex = built
            .structured
            .as_ref()
            .and_then(|s| s.get::<CodexProviderInjection>(BaseCodingAgent::Codex))
            .expect("codex owns its structured payload");

        assert_eq!(env.get("CDT_API_KEY").map(String::as_str), Some("sk-test"));
        assert_eq!(codex.model_provider_id, "cdt");

        let cfg = &codex.config_overrides;
        assert_eq!(
            cfg.get("model_providers.cdt.name"),
            Some(&JsonValue::String("Test Provider".to_string()))
        );
        assert_eq!(
            cfg.get("model_providers.cdt.base_url"),
            Some(&JsonValue::String(
                "https://openrouter.ai/api/v1".to_string()
            ))
        );
        assert_eq!(
            cfg.get("model_providers.cdt.env_key"),
            Some(&JsonValue::String("CDT_API_KEY".to_string()))
        );
        assert_eq!(
            cfg.get("model_providers.cdt.wire_api"),
            Some(&JsonValue::String("responses".to_string()))
        );
    }

    #[test]
    fn vendor_env_overlaid_first_credential_wins() {
        // Vendor-quirk env in record.codex.env is overlaid first; CDT_API_KEY
        // is set last so a misconfigured vendor entry can't overwrite the
        // credential.
        let mut codex_env = HashMap::new();
        codex_env.insert("OPENAI_TIMEOUT_MS".to_string(), "30000".to_string());
        codex_env.insert(
            "CDT_API_KEY".to_string(),
            "should-be-overridden".to_string(),
        );

        let p = provider_with_codex(
            AiProviderKind::Preset,
            Some("real-key"),
            Some("https://example.com/v1"),
            codex_env,
        );
        let env = env_of(&p);
        assert_eq!(env.get("CDT_API_KEY").map(String::as_str), Some("real-key"));
        assert_eq!(
            env.get("OPENAI_TIMEOUT_MS").map(String::as_str),
            Some("30000")
        );
    }
}

#[cfg(test)]
mod opencode_injection_tests {
    use serde_json::{Value as JsonValue, json};

    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn provider_with_opencode(
        kind: AiProviderKind,
        api_key: Option<&str>,
        preset_id: Option<&str>,
        base_url: Option<&str>,
        npm: Option<&str>,
        opencode_env: HashMap<String, String>,
        opencode_options: HashMap<String, JsonValue>,
        enabled_models: Vec<EnabledModel>,
    ) -> Provider {
        Provider {
            id: Uuid::new_v4(),
            name: "Test Provider".to_string(),
            kind,
            preset_id: preset_id.map(|s| s.to_string()),
            enabled: true,
            api_key: api_key.map(|s| s.to_string()),
            per_agent_enabled: HashMap::new(),
            claude: ClaudePayload::default(),
            codex: CodexPayload::default(),
            opencode: OpencodePayload {
                npm: npm.map(|s| s.to_string()),
                base_url: base_url.map(|s| s.to_string()),
                options: opencode_options,
                api_key: None,
                env: opencode_env,
            },
            deepseek_tui: DeepseekTuiPayload::default(),
            gemini: GeminiPayload::default(),
            hermes: HermesPayload::default(),
            enabled_models,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Drive the record through the same path a spawn takes: adapter lookup,
    /// context, injection. Nothing here names a OpenCode-side convention.
    fn injection(p: &Provider) -> Result<ProviderInjection, ProviderError> {
        p.resolve_injection(BaseCodingAgent::Opencode, "")
            .map(|(_, injection)| injection)
    }

    fn env_of(p: &Provider) -> HashMap<String, String> {
        injection(p)
            .expect("injection builds")
            .env
            .expect("non-ambient record emits env")
    }

    fn parse_config(env: &HashMap<String, String>) -> JsonValue {
        let raw = env
            .get("OPENCODE_CONFIG_CONTENT")
            .expect("OPENCODE_CONFIG_CONTENT must be present");
        serde_json::from_str(raw).expect("config content must parse as JSON")
    }

    #[test]
    fn default_returns_none() {
        let p = provider_with_opencode(
            AiProviderKind::Default,
            Some("ignored"),
            None,
            Some("ignored"),
            None,
            HashMap::new(),
            HashMap::new(),
            vec![],
        );
        let injection = injection(&p).unwrap();
        assert!(injection.env.is_none());
        assert!(injection.structured.is_none());
    }

    #[test]
    fn missing_api_key_rejected() {
        let p = provider_with_opencode(
            AiProviderKind::Preset,
            None,
            Some("openrouter"),
            Some("https://openrouter.ai/api/v1"),
            Some("@ai-sdk/anthropic"),
            HashMap::new(),
            HashMap::new(),
            vec![EnabledModel {
                id: "anthropic/claude-opus-4.7".to_string(),
                display_name: "Opus 4.7".to_string(),
                owned_by: None,
            }],
        );
        assert!(matches!(
            injection(&p),
            Err(ProviderError::Injection(
                ProviderInjectionError::MissingApiKey(_)
            ))
        ));
    }

    #[test]
    fn missing_base_url_rejected() {
        let p = provider_with_opencode(
            AiProviderKind::Preset,
            Some("sk-test"),
            Some("openrouter"),
            None,
            Some("@ai-sdk/anthropic"),
            HashMap::new(),
            HashMap::new(),
            vec![EnabledModel {
                id: "anthropic/claude-opus-4.7".to_string(),
                display_name: "Opus 4.7".to_string(),
                owned_by: None,
            }],
        );
        assert!(matches!(
            injection(&p),
            Err(ProviderError::Injection(
                ProviderInjectionError::MissingBaseUrl(_)
            ))
        ));
    }

    #[test]
    fn empty_models_rejected() {
        let p = provider_with_opencode(
            AiProviderKind::Preset,
            Some("sk-test"),
            Some("openrouter"),
            Some("https://openrouter.ai/api/v1"),
            Some("@ai-sdk/anthropic"),
            HashMap::new(),
            HashMap::new(),
            vec![],
        );
        assert!(matches!(
            injection(&p),
            Err(ProviderError::Injection(
                ProviderInjectionError::EmptyEnabledModels
            ))
        ));
    }

    #[test]
    fn json_shape_matches_plan() {
        // Plan §3.2 lines 192-211: provider.<slug>.{npm,name,options,models}
        // with options carrying baseURL + apiKey + custom options, and models
        // synthesized as { id: {} } for each enabled model.
        let mut options = HashMap::new();
        options.insert("setCacheKey".to_string(), json!(true));

        let p = provider_with_opencode(
            AiProviderKind::Preset,
            Some("sk-real"),
            Some("openrouter"),
            Some("https://openrouter.ai/api/v1"),
            Some("@ai-sdk/anthropic"),
            HashMap::new(),
            options,
            vec![
                EnabledModel {
                    id: "anthropic/claude-opus-4.7".to_string(),
                    display_name: "Opus 4.7".to_string(),
                    owned_by: None,
                },
                EnabledModel {
                    id: "anthropic/claude-sonnet-4.6".to_string(),
                    display_name: "Sonnet 4.6".to_string(),
                    owned_by: None,
                },
            ],
        );

        let env = env_of(&p);
        let cfg = parse_config(&env);

        let provider = &cfg["provider"]["openrouter"];
        assert_eq!(provider["npm"], json!("@ai-sdk/anthropic"));
        assert_eq!(provider["name"], json!("Test Provider"));
        assert_eq!(
            provider["options"]["baseURL"],
            json!("https://openrouter.ai/api/v1")
        );
        assert_eq!(provider["options"]["apiKey"], json!("sk-real"));
        assert_eq!(provider["options"]["setCacheKey"], json!(true));
        assert_eq!(provider["models"]["anthropic/claude-opus-4.7"], json!({}));
        assert_eq!(provider["models"]["anthropic/claude-sonnet-4.6"], json!({}));
    }

    #[test]
    fn custom_record_uses_custom_slug() {
        // Records with no presetId (Custom) emit `provider.custom.*`
        // per plan §3.2 line 197.
        let p = provider_with_opencode(
            AiProviderKind::Custom,
            Some("sk-test"),
            None,
            Some("https://example.com/v1"),
            Some("@ai-sdk/openai-compatible"),
            HashMap::new(),
            HashMap::new(),
            vec![EnabledModel {
                id: "gpt-4".to_string(),
                display_name: "GPT-4".to_string(),
                owned_by: None,
            }],
        );
        let env = env_of(&p);
        let cfg = parse_config(&env);
        assert!(cfg["provider"]["custom"].is_object());
        assert!(cfg["provider"]["openrouter"].is_null());
    }

    #[test]
    fn payload_apikey_overrides_top_level() {
        // Per-agent apiKey override (Packy Code style) wins over top-level.
        let mut p = provider_with_opencode(
            AiProviderKind::Preset,
            Some("top-level-key"),
            Some("openrouter"),
            Some("https://openrouter.ai/api/v1"),
            Some("@ai-sdk/anthropic"),
            HashMap::new(),
            HashMap::new(),
            vec![EnabledModel {
                id: "anthropic/claude-opus-4.7".to_string(),
                display_name: "Opus 4.7".to_string(),
                owned_by: None,
            }],
        );
        p.opencode.api_key = Some("opencode-specific-key".to_string());

        let env = env_of(&p);
        let cfg = parse_config(&env);
        assert_eq!(
            cfg["provider"]["openrouter"]["options"]["apiKey"],
            json!("opencode-specific-key")
        );
    }

    #[test]
    fn vendor_env_overlaid_config_content_wins() {
        // record.opencode.env is overlaid first; OPENCODE_CONFIG_CONTENT is
        // set last so a misconfigured vendor env entry can't clobber the
        // provider config we just built.
        let mut opencode_env = HashMap::new();
        opencode_env.insert("OPENCODE_LOG_LEVEL".to_string(), "debug".to_string());
        opencode_env.insert(
            "OPENCODE_CONFIG_CONTENT".to_string(),
            "should-be-overridden".to_string(),
        );

        let p = provider_with_opencode(
            AiProviderKind::Preset,
            Some("sk-test"),
            Some("openrouter"),
            Some("https://openrouter.ai/api/v1"),
            Some("@ai-sdk/anthropic"),
            opencode_env,
            HashMap::new(),
            vec![EnabledModel {
                id: "m".to_string(),
                display_name: "m".to_string(),
                owned_by: None,
            }],
        );
        let env = env_of(&p);
        // The vendor entry must be fully replaced, not merged into.
        assert_ne!(
            env.get("OPENCODE_CONFIG_CONTENT").map(String::as_str),
            Some("should-be-overridden")
        );
        let cfg = parse_config(&env);
        assert_eq!(
            cfg["provider"]["openrouter"]["options"]["baseURL"],
            json!("https://openrouter.ai/api/v1")
        );
        assert_eq!(
            env.get("OPENCODE_LOG_LEVEL").map(String::as_str),
            Some("debug")
        );
    }

    #[test]
    fn options_apikey_cannot_shadow_resolved_credential() {
        // record.opencode.options is overlaid first, then baseURL+apiKey are
        // inserted last so they always win. Confirms a misconfigured
        // `options.apiKey` can't silently replace the resolved credential.
        let mut bad_options = HashMap::new();
        bad_options.insert("apiKey".to_string(), json!("LEAKED-FROM-OPTIONS"));
        bad_options.insert("baseURL".to_string(), json!("https://wrong.example/v1"));

        let p = provider_with_opencode(
            AiProviderKind::Preset,
            Some("sk-real"),
            Some("openrouter"),
            Some("https://openrouter.ai/api/v1"),
            Some("@ai-sdk/anthropic"),
            HashMap::new(),
            bad_options,
            vec![EnabledModel {
                id: "m".to_string(),
                display_name: "m".to_string(),
                owned_by: None,
            }],
        );
        let env = env_of(&p);
        let cfg = parse_config(&env);
        assert_eq!(
            cfg["provider"]["openrouter"]["options"]["apiKey"],
            json!("sk-real")
        );
        assert_eq!(
            cfg["provider"]["openrouter"]["options"]["baseURL"],
            json!("https://openrouter.ai/api/v1")
        );
    }
}

#[cfg(test)]
mod gemini_injection_tests {
    use super::*;

    fn provider_with_gemini(
        kind: AiProviderKind,
        api_key: Option<&str>,
        base_url: Option<&str>,
        gemini_env: HashMap<String, String>,
    ) -> Provider {
        Provider {
            id: Uuid::new_v4(),
            name: "Test Provider".to_string(),
            kind,
            preset_id: None,
            enabled: true,
            api_key: api_key.map(|s| s.to_string()),
            per_agent_enabled: HashMap::new(),
            claude: ClaudePayload::default(),
            codex: CodexPayload::default(),
            opencode: OpencodePayload::default(),
            deepseek_tui: DeepseekTuiPayload::default(),
            gemini: GeminiPayload {
                base_url: base_url.map(|s| s.to_string()),
                api_key: None,
                env: gemini_env,
            },
            hermes: HermesPayload::default(),
            enabled_models: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Drive the record through the same path a spawn takes: adapter lookup,
    /// context, injection. Nothing here names a Gemini-side convention.
    fn injection(p: &Provider) -> Result<ProviderInjection, ProviderError> {
        p.resolve_injection(BaseCodingAgent::Gemini, "")
            .map(|(_, injection)| injection)
    }

    fn env_of(p: &Provider) -> HashMap<String, String> {
        injection(p)
            .expect("injection builds")
            .env
            .expect("non-ambient record emits env")
    }

    #[test]
    fn default_returns_none() {
        let p = provider_with_gemini(
            AiProviderKind::Default,
            Some("ignored"),
            Some("ignored"),
            HashMap::new(),
        );
        let injection = injection(&p).unwrap();
        assert!(injection.env.is_none());
        assert!(injection.structured.is_none());
    }

    #[test]
    fn missing_api_key_rejected() {
        let p = provider_with_gemini(
            AiProviderKind::Custom,
            None,
            Some("https://generativelanguage.googleapis.com"),
            HashMap::new(),
        );
        assert!(matches!(
            injection(&p),
            Err(ProviderError::Injection(
                ProviderInjectionError::MissingApiKey(_)
            ))
        ));
    }

    #[test]
    fn empty_api_key_rejected() {
        let p = provider_with_gemini(
            AiProviderKind::Custom,
            Some(""),
            Some("https://generativelanguage.googleapis.com"),
            HashMap::new(),
        );
        assert!(matches!(
            injection(&p),
            Err(ProviderError::Injection(
                ProviderInjectionError::MissingApiKey(_)
            ))
        ));
    }

    #[test]
    fn missing_base_url_rejected() {
        let p = provider_with_gemini(
            AiProviderKind::Custom,
            Some("sk-test"),
            None,
            HashMap::new(),
        );
        assert!(matches!(
            injection(&p),
            Err(ProviderError::Injection(
                ProviderInjectionError::MissingBaseUrl(_)
            ))
        ));
    }

    #[test]
    fn structural_keys_emitted() {
        // Plan §3.2 lines 233-236: GOOGLE_GEMINI_BASE_URL + GEMINI_API_KEY
        // are the entire applier output (plus any vendor-quirk env overlay).
        let p = provider_with_gemini(
            AiProviderKind::Custom,
            Some("sk-real"),
            Some("https://generativelanguage.googleapis.com"),
            HashMap::new(),
        );
        let env = env_of(&p);
        assert_eq!(
            env.get("GOOGLE_GEMINI_BASE_URL").map(String::as_str),
            Some("https://generativelanguage.googleapis.com")
        );
        assert_eq!(
            env.get("GEMINI_API_KEY").map(String::as_str),
            Some("sk-real")
        );
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn payload_apikey_overrides_top_level() {
        // Per-agent apiKey override (Packy Code style) wins over top-level.
        let mut p = provider_with_gemini(
            AiProviderKind::Custom,
            Some("top-level-key"),
            Some("https://generativelanguage.googleapis.com"),
            HashMap::new(),
        );
        p.gemini.api_key = Some("gemini-specific-key".to_string());

        let env = env_of(&p);
        assert_eq!(
            env.get("GEMINI_API_KEY").map(String::as_str),
            Some("gemini-specific-key")
        );
    }

    #[test]
    fn vendor_env_overlaid_credential_wins() {
        // record.gemini.env is overlaid first; GOOGLE_GEMINI_BASE_URL +
        // GEMINI_API_KEY are inserted last so a misconfigured vendor entry
        // can't clobber either. Mirrors Phase C/D defensive ordering.
        let mut gemini_env = HashMap::new();
        gemini_env.insert("GEMINI_LOG_LEVEL".to_string(), "debug".to_string());
        gemini_env.insert(
            "GEMINI_API_KEY".to_string(),
            "should-be-overridden".to_string(),
        );
        gemini_env.insert(
            "GOOGLE_GEMINI_BASE_URL".to_string(),
            "https://wrong.example".to_string(),
        );

        let p = provider_with_gemini(
            AiProviderKind::Custom,
            Some("real-key"),
            Some("https://generativelanguage.googleapis.com"),
            gemini_env,
        );
        let env = env_of(&p);
        assert_eq!(
            env.get("GEMINI_API_KEY").map(String::as_str),
            Some("real-key")
        );
        assert_eq!(
            env.get("GOOGLE_GEMINI_BASE_URL").map(String::as_str),
            Some("https://generativelanguage.googleapis.com")
        );
        assert_eq!(
            env.get("GEMINI_LOG_LEVEL").map(String::as_str),
            Some("debug")
        );
    }

    #[test]
    fn google_api_key_in_overlay_survives() {
        // gemini-cli reads `GOOGLE_API_KEY` as an alternate credential
        // (contentGenerator.ts:156) and prefers `GEMINI_API_KEY` when both
        // are set (`getAuthTypeFromEnv`). The applier sets only
        // `GEMINI_API_KEY` and must leave any `GOOGLE_API_KEY` from the
        // vendor-env overlay alone — confirms the documented "passes
        // through untouched" contract.
        let mut gemini_env = HashMap::new();
        gemini_env.insert(
            "GOOGLE_API_KEY".to_string(),
            "ambient-google-key".to_string(),
        );
        let p = provider_with_gemini(
            AiProviderKind::Custom,
            Some("sk-real"),
            Some("https://generativelanguage.googleapis.com"),
            gemini_env,
        );
        let env = env_of(&p);
        assert_eq!(
            env.get("GOOGLE_API_KEY").map(String::as_str),
            Some("ambient-google-key")
        );
        assert_eq!(
            env.get("GEMINI_API_KEY").map(String::as_str),
            Some("sk-real")
        );
    }

    #[test]
    fn dispatch_routes_gemini_to_gemini_applier() {
        // Gemini must resolve to Gemini's own adapter, not the default
        // Anthropic-style applier. Regression guard against a non-Default
        // provider used with Gemini injecting ANTHROPIC_BASE_URL into the
        // gemini-cli child.
        let p = provider_with_gemini(
            AiProviderKind::Custom,
            Some("sk-real"),
            Some("https://generativelanguage.googleapis.com"),
            HashMap::new(),
        );
        let (_, inj) = p
            .resolve_injection(BaseCodingAgent::Gemini, "gemini-3-pro-preview")
            .unwrap();
        let env = inj.env.expect("Custom Gemini emits env");
        assert!(env.contains_key("GEMINI_API_KEY"));
        assert!(env.contains_key("GOOGLE_GEMINI_BASE_URL"));
        assert!(!env.contains_key("ANTHROPIC_BASE_URL"));
        assert!(!env.contains_key("ANTHROPIC_AUTH_TOKEN"));
        assert!(inj.structured.is_none());
    }

    #[test]
    fn dispatch_default_gemini_returns_no_env() {
        // Default provider routes Gemini to ambient ~/.gemini auth; the
        // dispatch must yield env: None so provider_vars stays empty and
        // gemini-cli reads its own oauth_creds.json.
        let p = provider_with_gemini(
            AiProviderKind::Default,
            Some("ignored"),
            None,
            HashMap::new(),
        );
        let (_, inj) = p
            .resolve_injection(BaseCodingAgent::Gemini, "gemini-3-pro-preview")
            .unwrap();
        assert!(inj.env.is_none());
        assert!(inj.structured.is_none());
    }
}

#[cfg(test)]
mod slot_routing_tests {
    use executors::profile::ExecutorConfigs;

    use super::*;

    /// A record with every payload slot present and nothing configured in any
    /// of them. The only place this file enumerates slots — it is the record
    /// schema, not a routing decision.
    fn bare_record() -> Provider {
        Provider {
            id: Uuid::new_v4(),
            name: "Test Provider".to_string(),
            kind: AiProviderKind::Preset,
            preset_id: Some("openrouter".to_string()),
            enabled: true,
            api_key: Some("sk-test".to_string()),
            per_agent_enabled: HashMap::new(),
            claude: ClaudePayload::default(),
            codex: CodexPayload::default(),
            opencode: OpencodePayload::default(),
            deepseek_tui: DeepseekTuiPayload::default(),
            gemini: GeminiPayload::default(),
            hermes: HermesPayload::default(),
            enabled_models: vec![EnabledModel {
                id: "anthropic/claude-opus-4.7".to_string(),
                display_name: "Opus 4.7".to_string(),
                owned_by: None,
            }],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn slot_url(slot: &str) -> String {
        format!("https://{slot}.example/v1")
    }

    /// The same record with a base URL written into the named slots — by wire
    /// key, exactly as an adapter names it.
    fn record_with(slots: &[&str]) -> Provider {
        let mut record = serde_json::to_value(bare_record()).expect("record serializes");
        for slot in slots {
            record[*slot]["baseUrl"] = serde_json::Value::String(slot_url(slot));
        }
        serde_json::from_value(record).expect("record round-trips")
    }

    fn registered_agents() -> Vec<BaseCodingAgent> {
        ExecutorConfigs::get_cached()
            .executors
            .keys()
            .copied()
            .collect()
    }

    /// A record hands each harness the slot that harness declares, and no
    /// other. Nothing here — and nothing in the record — names a harness to
    /// decide that: the slot comes off the adapter, so a harness added
    /// tomorrow is routed by the same code.
    #[test]
    fn injection_follows_the_adapter_declared_slot() {
        let agents = registered_agents();
        assert!(
            !agents.is_empty(),
            "profile registry must register harnesses"
        );

        let all_slots: Vec<&'static str> = agents
            .iter()
            .filter_map(|agent| CodingAgent::registered(*agent))
            .map(|adapter| adapter.provider_slot())
            .collect();

        for agent in agents {
            let adapter = CodingAgent::registered(agent).expect("registered agent has an adapter");
            let slot = adapter.provider_slot();
            let others: Vec<&str> = all_slots.iter().copied().filter(|s| *s != slot).collect();

            // A harness that needs an endpoint must find it in its own slot...
            if record_with(&[]).resolve_injection(agent, "m").is_err() {
                assert!(
                    record_with(&[slot]).resolve_injection(agent, "m").is_ok(),
                    "{agent} must build from slot '{slot}'"
                );
                assert!(
                    record_with(&others).resolve_injection(agent, "m").is_err(),
                    "{agent} must not fall back to another harness's slot"
                );
                continue;
            }

            // ...and one that does not still has to read the right one.
            let (_, injection) = record_with(&all_slots)
                .resolve_injection(agent, "m")
                .expect("injection builds");
            let env = injection.env.expect("non-ambient record emits env");
            assert!(
                env.values().any(|value| *value == slot_url(slot)),
                "{agent} must read slot '{slot}'"
            );
            for other in &others {
                assert!(
                    !env.values().any(|value| *value == slot_url(other)),
                    "{agent} leaked slot '{other}' into its spawn env"
                );
            }
        }
    }

    /// The Default record is ambient auth: no harness injects anything, so a
    /// spawn falls through to the harness's own credentials.
    #[test]
    fn ambient_record_injects_nothing_for_any_harness() {
        let mut record = record_with(&[]);
        record.kind = AiProviderKind::Default;
        for agent in registered_agents() {
            let (_, injection) = record
                .resolve_injection(agent, "m")
                .expect("ambient record resolves");
            assert!(injection.env.is_none(), "{agent} injected env for Default");
            assert!(
                injection.structured.is_none(),
                "{agent} injected a structured payload for Default"
            );
        }
    }
}
