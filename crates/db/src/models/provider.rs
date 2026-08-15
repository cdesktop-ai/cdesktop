use std::collections::HashMap;

use chrono::{DateTime, Utc};
use executors::{env::CodexProviderInjection, executors::BaseCodingAgent};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
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

/// Hardcoded `model_providers.<id>` slug for the cdesktop-injected Codex
/// provider. Plan §3.2: the injected provider id is `cdt`, which keeps the
/// applier's emitted keys identical for every preset (`model_providers.cdt.*`)
/// without depending on user-record naming.
pub const CODEX_INJECTED_PROVIDER_ID: &str = "cdt";

/// Env-var name carrying the user's API key into Codex's `env_key`-driven
/// auth path. Plan §3.2 fixes this to `CDT_API_KEY` so the spawn applier
/// can wire `model_providers.cdt.env_key=CDT_API_KEY` once and never
/// re-derive the variable name.
pub const CODEX_INJECTED_API_KEY_ENV: &str = "CDT_API_KEY";

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
        per_agent_enabled: &HashMap<String, bool>,
        claude: &ClaudePayload,
        codex: &CodexPayload,
        opencode: &OpencodePayload,
        deepseek_tui: &DeepseekTuiPayload,
        gemini: &GeminiPayload,
        hermes: &HermesPayload,
    ) -> Result<(), ProviderError> {
        if matches!(kind, AiProviderKind::Default) {
            return Ok(());
        }
        let is_on = |k: &str| per_agent_enabled.get(k).copied().unwrap_or(false);
        let has = |s: &Option<String>| s.as_deref().map_or(false, |v| !v.is_empty());
        let missing = |agent: &str, ok: bool| {
            if ok {
                Ok(())
            } else {
                Err(ProviderError::EnabledAgentMissingBaseUrl(agent.to_string()))
            }
        };
        if is_on("CLAUDE_CODE") {
            missing("CLAUDE_CODE", has(&claude.base_url))?;
        }
        if is_on("CODEX") {
            missing("CODEX", has(&codex.base_url))?;
        }
        if is_on("OPENCODE") {
            missing("OPENCODE", has(&opencode.base_url))?;
        }
        if is_on("DEEPSEEK_TUI") {
            missing("DEEPSEEK_TUI", has(&deepseek_tui.base_url))?;
        }
        if is_on("GEMINI") {
            missing("GEMINI", has(&gemini.base_url))?;
        }
        if is_on("HERMES") {
            missing("HERMES", has(&hermes.base_url))?;
        }
        Ok(())
    }

    pub async fn create(
        pool: &SqlitePool,
        id: Uuid,
        data: &CreateProvider,
    ) -> Result<Self, ProviderError> {
        Self::validate_enabled_models(&data.kind, &data.enabled_models)?;
        Self::validate_enabled_agent_payloads(
            &data.kind,
            &data.per_agent_enabled,
            &data.claude,
            &data.codex,
            &data.opencode,
            &data.deepseek_tui,
            &data.gemini,
            &data.hermes,
        )?;

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
        Self::validate_enabled_agent_payloads(
            &existing.kind,
            &data.per_agent_enabled,
            &data.claude,
            &data.codex,
            &data.opencode,
            &data.deepseek_tui,
            &data.gemini,
            &data.hermes,
        )?;

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

    /// OpenCode's session API requires `(provider_id, model_id)`; its only
    /// signal for that split is `model.split_once('/')`. Cdesktop's
    /// non-Default provider records store `enabled_models` as the raw vendor
    /// ids (e.g. `openai/gpt-5.4-mini` for OpenRouter), so the picker emits
    /// a single-slash string that splits to the wrong provider. Prefix with
    /// the provider's OpenCode slug (`preset_id` or `"custom"` — matches
    /// `build_opencode_injection`) so the first-slash split recovers
    /// `(slug, full_vendor_model_id)`.
    ///
    /// No-op for: non-OpenCode agents, Default mode (picker already emits
    /// prefixed ids sourced from executor discovery), and ids that already
    /// carry the slug prefix.
    pub fn prefix_opencode_model_id(&self, agent: BaseCodingAgent, model_id: &str) -> String {
        if agent != BaseCodingAgent::Opencode || self.kind == AiProviderKind::Default {
            return model_id.to_string();
        }
        let slug = self.preset_id.as_deref().unwrap_or("custom");
        let prefix = format!("{slug}/");
        if model_id.starts_with(&prefix) {
            return model_id.to_string();
        }
        format!("{slug}/{model_id}")
    }

    /// Per-agent credential resolution. Each per-agent payload may carry an
    /// `api_key` override; when present (and non-empty) it wins over the
    /// top-level `Provider::api_key`. Aggregators like Packy Code use this to
    /// issue distinct keys per backing agent. The common case (one key for
    /// every agent) leaves payload overrides empty and falls through.
    pub fn resolved_api_key(&self, agent: BaseCodingAgent) -> Option<&str> {
        let override_key: Option<&str> = match agent {
            BaseCodingAgent::ClaudeCode => self.claude.api_key.as_deref(),
            BaseCodingAgent::Codex => self.codex.api_key.as_deref(),
            BaseCodingAgent::Opencode => self.opencode.api_key.as_deref(),
            BaseCodingAgent::Gemini => self.gemini.api_key.as_deref(),
            BaseCodingAgent::DeepseekTui => self.deepseek_tui.api_key.as_deref(),
            BaseCodingAgent::Hermes => self.hermes.api_key.as_deref(),
            // Other agents (Amp, Cursor, Qwen, Copilot, Droid, QaMock) have
            // no per-agent payload.
            _ => None,
        };
        override_key
            .filter(|s| !s.is_empty())
            .or_else(|| self.api_key.as_deref().filter(|s| !s.is_empty()))
    }

    /// Build the Claude-side env map to inject at process spawn time.
    ///
    /// For Default (ambient auth): empty map — Claude CLI uses its own config.
    /// For Preset/Custom: derives `ANTHROPIC_BASE_URL` from `claude.base_url`,
    /// names the credential env var per `claude.api_key_field`, sets
    /// `ANTHROPIC_DEFAULT_HAIKU_MODEL` from `claude.haiku_model` (or follows
    /// the picker model when unset), routes the picker's selected model into
    /// `CLAUDE_CODE_SUBAGENT_MODEL` for Task-tool / sub-agent routing, and
    /// overlays `claude.env` for vendor quirks.
    /// `ANTHROPIC_MODEL` / `ANTHROPIC_DEFAULT_SONNET_MODEL` /
    /// `ANTHROPIC_DEFAULT_OPUS_MODEL` are stripped because they conflict with
    /// the `--model` CLI flag the executor passes for the *main* model. See
    /// plan §3.2 spawn-time applier.
    ///
    /// Codex / OpenCode / Gemini / DeepSeek TUI / Hermes appliers ship via
    /// their own `build_*_injection` methods (Phases C / D / F / E / G).
    pub fn build_spawn_env(&self, model_id: &str) -> HashMap<String, String> {
        if self.kind == AiProviderKind::Default {
            return HashMap::new();
        }

        let mut env = self.claude.env.clone();
        // Ensure no stray conflicting keys leaked into claude.env.
        env.remove("ANTHROPIC_MODEL");
        env.remove("ANTHROPIC_DEFAULT_SONNET_MODEL");
        env.remove("ANTHROPIC_DEFAULT_OPUS_MODEL");
        env.remove("ANTHROPIC_DEFAULT_HAIKU_MODEL");

        if let Some(base_url) = &self.claude.base_url {
            env.insert("ANTHROPIC_BASE_URL".to_string(), base_url.clone());
        }

        let api_key_field = self
            .claude
            .api_key_field
            .as_deref()
            .unwrap_or("ANTHROPIC_AUTH_TOKEN");
        if let Some(key) = self.resolved_api_key(BaseCodingAgent::ClaudeCode) {
            env.insert(api_key_field.to_string(), key.to_string());
        }

        if !model_id.is_empty() {
            env.insert(
                "CLAUDE_CODE_SUBAGENT_MODEL".to_string(),
                model_id.to_string(),
            );
        }

        let haiku = self.claude.haiku_model.clone().or_else(|| {
            if model_id.is_empty() {
                None
            } else {
                Some(model_id.to_string())
            }
        });
        if let Some(h) = haiku {
            env.insert("ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(), h);
        }

        env
    }

    /// Build the Codex-side spawn injection: the env-var map carrying
    /// `CDT_API_KEY`, plus the dotted-path config overrides + model-provider
    /// id that get merged into Codex's `ThreadStartParams.config` and
    /// `ThreadStartParams.model_provider` respectively.
    ///
    /// Per plan §3.2 the emitted overrides are:
    /// ```text
    /// model_providers.cdt.name      = record.name
    /// model_providers.cdt.base_url  = record.codex.baseUrl
    /// model_providers.cdt.env_key   = "CDT_API_KEY"
    /// model_providers.cdt.wire_api  = "responses"
    /// ```
    /// `requires_openai_auth` is left at its default (false), so codex's
    /// auth path reads only the `env_key` env var and never consults
    /// `~/.codex/auth.json` for our provider — matching the plan's
    /// "user's home dir is read-only from cdesktop" rule.
    ///
    /// Errors:
    /// - `MissingApiKey("CODEX")` when `record.api_key` is empty.
    /// - `EnabledAgentMissingBaseUrl("CODEX")` when `record.codex.base_url` is empty.
    ///
    /// Both checks duplicate Phase B's save-time `validate_enabled_agent_payloads`.
    /// They re-run here to guard records edited (or whose secret got cleared)
    /// between save and spawn — the save-time validator can't see a
    /// post-save edit that goes through a different path.
    ///
    /// `record.codex.env` is overlaid first so the `CDT_API_KEY` we set
    /// last cannot be silently clobbered by a vendor-quirk env entry.
    /// Returns `Ok(None)` for the Default provider (ambient auth path).
    pub fn build_codex_injection(
        &self,
    ) -> Result<Option<(HashMap<String, String>, CodexProviderInjection)>, ProviderError> {
        if self.kind == AiProviderKind::Default {
            return Ok(None);
        }

        let api_key = self
            .resolved_api_key(BaseCodingAgent::Codex)
            .ok_or_else(|| ProviderError::MissingApiKey("CODEX".to_string()))?;
        let base_url = self
            .codex
            .base_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::EnabledAgentMissingBaseUrl("CODEX".to_string()))?;

        let mut env = self.codex.env.clone();
        // `CDT_API_KEY` set last so vendor-quirk env (set above) can't
        // accidentally overwrite the credential. Plan §3.2 is silent on
        // ordering; defensive choice.
        env.insert(CODEX_INJECTED_API_KEY_ENV.to_string(), api_key.to_string());

        let prefix = format!("model_providers.{CODEX_INJECTED_PROVIDER_ID}");
        let mut config_overrides = HashMap::new();
        config_overrides.insert(
            format!("{prefix}.name"),
            JsonValue::String(self.name.clone()),
        );
        config_overrides.insert(
            format!("{prefix}.base_url"),
            JsonValue::String(base_url.to_string()),
        );
        config_overrides.insert(
            format!("{prefix}.env_key"),
            JsonValue::String(CODEX_INJECTED_API_KEY_ENV.to_string()),
        );
        config_overrides.insert(
            format!("{prefix}.wire_api"),
            JsonValue::String("responses".to_string()),
        );

        Ok(Some((
            env,
            CodexProviderInjection {
                config_overrides,
                model_provider_id: CODEX_INJECTED_PROVIDER_ID.to_string(),
            },
        )))
    }

    /// Build the OpenCode-side spawn injection — a single env var
    /// `OPENCODE_CONFIG_CONTENT` whose value is the provider/model JSON
    /// OpenCode loads after the user's global + project configs (so our
    /// keys win without touching `~/.config/opencode/`). Plan §3.2 lines
    /// 192-213 describes the wire shape; `name` / `apiKey` / `models` are
    /// synthesized at spawn from the user record's top-level fields, so
    /// the catalog payload itself stays small.
    ///
    /// The provider id key is `presetId` when the record came from the
    /// catalog, else `"custom"` — same as plan §3.2 line 197.
    ///
    /// Errors:
    /// - `MissingApiKey("OPENCODE")` when the resolved API key is empty.
    /// - `EnabledAgentMissingBaseUrl("OPENCODE")` when `record.opencode.base_url` is empty.
    /// - `EmptyEnabledModels` when `record.enabled_models` is empty —
    ///   OpenCode's runtime deletes any provider whose `models` map is empty
    ///   (`provider.ts:1393`), silently breaking the agent. Defense in depth
    ///   even though the form's `≥1 model checked` rule should already catch this.
    ///
    /// `record.opencode.env` is overlaid first so the caller's
    /// `OPENCODE_CONFIG_CONTENT` we set last cannot be silently clobbered.
    /// Returns `Ok(None)` for the Default provider (ambient auth path).
    pub fn build_opencode_injection(
        &self,
    ) -> Result<Option<HashMap<String, String>>, ProviderError> {
        if self.kind == AiProviderKind::Default {
            return Ok(None);
        }

        let api_key = self
            .resolved_api_key(BaseCodingAgent::Opencode)
            .ok_or_else(|| ProviderError::MissingApiKey("OPENCODE".to_string()))?;
        let base_url = self
            .opencode
            .base_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::EnabledAgentMissingBaseUrl("OPENCODE".to_string()))?;

        if self.enabled_models.is_empty() {
            return Err(ProviderError::EmptyEnabledModels);
        }

        // Provider id slug: presetId for catalog-backed records, "custom" for
        // standalone ones. Mirrors plan §3.2 line 197.
        let provider_slug = self.preset_id.as_deref().unwrap_or("custom");

        // options = { ...record.opencode.options, baseURL, apiKey }
        // Vendor-quirk options (catalog's `setCacheKey`, `region`, etc.) are
        // overlaid first; baseURL + apiKey are inserted LAST so they always
        // win, since silent shadowing of the credential or endpoint by an
        // `options` entry would be confusing for users editing the form.
        let mut options_map: serde_json::Map<String, JsonValue> = serde_json::Map::new();
        for (k, v) in &self.opencode.options {
            options_map.insert(k.clone(), v.clone());
        }
        options_map.insert(
            "baseURL".to_string(),
            JsonValue::String(base_url.to_string()),
        );
        options_map.insert("apiKey".to_string(), JsonValue::String(api_key.to_string()));

        // models = { id: {} } for each enabled model
        let mut models_map: serde_json::Map<String, JsonValue> = serde_json::Map::new();
        for m in &self.enabled_models {
            models_map.insert(m.id.clone(), JsonValue::Object(serde_json::Map::new()));
        }

        // provider.<slug> = { npm, name, options, models }
        let mut provider_inner: serde_json::Map<String, JsonValue> = serde_json::Map::new();
        if let Some(npm) = &self.opencode.npm {
            provider_inner.insert("npm".to_string(), JsonValue::String(npm.clone()));
        }
        provider_inner.insert("name".to_string(), JsonValue::String(self.name.clone()));
        provider_inner.insert("options".to_string(), JsonValue::Object(options_map));
        provider_inner.insert("models".to_string(), JsonValue::Object(models_map));

        let mut providers_map: serde_json::Map<String, JsonValue> = serde_json::Map::new();
        providers_map.insert(provider_slug.to_string(), JsonValue::Object(provider_inner));

        let mut root: serde_json::Map<String, JsonValue> = serde_json::Map::new();
        root.insert("provider".to_string(), JsonValue::Object(providers_map));

        let json = serde_json::to_string(&JsonValue::Object(root))?;

        let mut env = self.opencode.env.clone();
        // Set last so vendor-quirk env overlay can't accidentally clobber it.
        env.insert("OPENCODE_CONFIG_CONTENT".to_string(), json);
        Ok(Some(env))
    }

    /// Build the Gemini-side spawn injection: env-only, same shape as
    /// Claude (plan §3.2 lines 231-237).
    ///
    /// For Default (ambient auth): returns `Ok(None)` — gemini-cli reads
    /// its own `~/.gemini/oauth_creds.json` / `GEMINI_API_KEY` env. For
    /// Preset/Custom: derives `GOOGLE_GEMINI_BASE_URL` from
    /// `record.gemini.base_url`, sets `GEMINI_API_KEY` from the resolved
    /// API key, and overlays `record.gemini.env` for vendor quirks.
    ///
    /// Errors:
    /// - `MissingApiKey("GEMINI")` when the resolved API key is empty.
    /// - `EnabledAgentMissingBaseUrl("GEMINI")` when `record.gemini.base_url` is empty.
    ///
    /// `record.gemini.env` is overlaid first so the credential-bearing
    /// `GOOGLE_GEMINI_BASE_URL` / `GEMINI_API_KEY` set last cannot be
    /// silently clobbered by a vendor-quirk env entry — same defensive
    /// ordering as Phase C/D. Plan §3.2 line 236 lists overlay order
    /// loosely; the credential-last choice mirrors the Codex/OpenCode
    /// appliers.
    ///
    /// Catalog ships no Gemini presets (per plan §3.1 — Default's ambient
    /// auth covers official Google routing, remaining cc-switch presets
    /// have upstream issues). The applier therefore exists exclusively
    /// for Custom records pointing at user-supplied
    /// Google-API-compatible endpoints.
    ///
    /// `GOOGLE_API_KEY` (gemini-cli's alternate credential env var,
    /// `contentGenerator.ts:156`) is **not** set or cleared by this
    /// applier. `gemini-cli`'s `getAuthTypeFromEnv` (`:76-93`) prefers
    /// `GEMINI_API_KEY` first, so the credential we inject wins; an
    /// ambient `GOOGLE_API_KEY` (shell or vendor `record.gemini.env`)
    /// passes through to the child untouched.
    pub fn build_gemini_injection(&self) -> Result<Option<HashMap<String, String>>, ProviderError> {
        if self.kind == AiProviderKind::Default {
            return Ok(None);
        }

        let api_key = self
            .resolved_api_key(BaseCodingAgent::Gemini)
            .ok_or_else(|| ProviderError::MissingApiKey("GEMINI".to_string()))?;
        let base_url = self
            .gemini
            .base_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::EnabledAgentMissingBaseUrl("GEMINI".to_string()))?;

        let mut env = self.gemini.env.clone();
        // GOOGLE_GEMINI_BASE_URL + GEMINI_API_KEY set last so vendor-quirk
        // env (set above) can't accidentally overwrite either.
        env.insert("GOOGLE_GEMINI_BASE_URL".to_string(), base_url.to_string());
        env.insert("GEMINI_API_KEY".to_string(), api_key.to_string());
        Ok(Some(env))
    }

    /// Build the DeepSeek TUI spawn injection: env-only.
    ///
    /// The runtime binary (`deepseek-tui`) reads provider/credentials/model
    /// configuration exclusively from env vars — its own Cli struct does not
    /// expose `--provider`/`--base-url`/`--api-key`/`--model` flags (those
    /// live only on the dispatcher binary `deepseek`, which forwards them
    /// to the runtime as env vars anyway). So our applier emits env vars
    /// directly. Every cdesktop preset routes through the runtime's
    /// `openai` provider mode (the documented OpenAI-compat path); when
    /// that mode is active the runtime reads exclusively from the `openai`
    /// secret + base-url slots (`related/DeepSeek-TUI/crates/secrets/src/lib.rs:460-487`
    /// maps `"openai" => &["OPENAI_API_KEY"]`), so `DEEPSEEK_API_KEY` /
    /// `DEEPSEEK_BASE_URL` are not consulted and we don't waste bytes
    /// setting them.
    ///
    /// Picker-selected model is injected as `OPENAI_MODEL` — also the
    /// `openai`-mode slot.
    ///
    /// Returns `Ok(None)` for the Default provider (the runtime uses its
    /// own `~/.deepseek/config.toml` / keyring credentials in that path).
    ///
    /// Errors:
    /// - `MissingApiKey("DEEPSEEK_TUI")` when the resolved API key is empty.
    /// - `EnabledAgentMissingBaseUrl("DEEPSEEK_TUI")` when `record.deepseek_tui.base_url` is empty.
    ///
    /// `record.deepseek_tui.env` is overlaid first so the credential-bearing
    /// vars we set last cannot be silently clobbered by a vendor-quirk env
    /// entry — same defensive ordering as Phase C/D/F.
    pub fn build_deepseek_tui_injection(
        &self,
        model_id: &str,
    ) -> Result<Option<HashMap<String, String>>, ProviderError> {
        if self.kind == AiProviderKind::Default {
            return Ok(None);
        }

        let api_key = self
            .resolved_api_key(BaseCodingAgent::DeepseekTui)
            .ok_or_else(|| ProviderError::MissingApiKey("DEEPSEEK_TUI".to_string()))?;
        let base_url = self
            .deepseek_tui
            .base_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::EnabledAgentMissingBaseUrl("DEEPSEEK_TUI".to_string()))?;

        let mut env = self.deepseek_tui.env.clone();
        // Credential + endpoint env set LAST so vendor-quirk overlays above
        // can't accidentally shadow them. Only the `openai`-mode slots are
        // populated; `DEEPSEEK_PROVIDER=openai` selects the mode itself.
        env.insert("DEEPSEEK_PROVIDER".to_string(), "openai".to_string());
        env.insert("OPENAI_BASE_URL".to_string(), base_url.to_string());
        env.insert("OPENAI_API_KEY".to_string(), api_key.to_string());
        if !model_id.is_empty() {
            env.insert("OPENAI_MODEL".to_string(), model_id.to_string());
        }
        Ok(Some(env))
    }

    /// Build the Hermes spawn injection: env-only.
    ///
    /// Hermes ACP (`hermes acp`) takes no CLI flags — provider selection
    /// happens via env vars read at startup by `hermes_cli/runtime_provider.py`.
    /// We force the active carrier with `HERMES_INFERENCE_PROVIDER=<slug>`
    /// (resolution at `:311` of that file), then override that carrier's
    /// hardcoded base URL with `<SLUG>_BASE_URL` (e.g. `OPENROUTER_BASE_URL`
    /// at `:577`; the `*_BASE_URL` env-var fallback short-circuits the
    /// hardcoded constant). Auth env var name matches the carrier slug's
    /// own convention (e.g. `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`).
    ///
    /// `HERMES_INFERENCE_MODEL` is read by the oneshot/TUI paths
    /// (`hermes_cli/oneshot.py:228`) but **not** by `acp_adapter/`. The ACP
    /// path applies model selection via `set_session_model`
    /// (`acp_adapter/server.py:1651`), which `AcpAgentHarness` does not
    /// currently emit — so this env var is a no-op for the ACP spawn until
    /// the harness gains that call. We set it anyway so the same injection
    /// stays correct if a future profile invokes Hermes via a non-ACP path.
    ///
    /// `apiMode` → carrier slug mapping (matches the cc-switch preset
    /// distribution: 25 chat_completions + 19 anthropic_messages + 0
    /// codex_responses presets at 2026-05-10 scan):
    /// - `chat_completions` → openrouter
    /// - `anthropic_messages` → anthropic
    /// - `codex_responses` → xai
    ///
    /// Returns `Ok(None)` for the Default provider (Hermes uses its own
    /// `~/.hermes/config.yaml` defaults in that path).
    pub fn build_hermes_injection(
        &self,
        model_id: &str,
    ) -> Result<Option<HashMap<String, String>>, ProviderError> {
        if self.kind == AiProviderKind::Default {
            return Ok(None);
        }

        let api_key = self
            .resolved_api_key(BaseCodingAgent::Hermes)
            .ok_or_else(|| ProviderError::MissingApiKey("HERMES".to_string()))?;
        let base_url = self
            .hermes
            .base_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ProviderError::EnabledAgentMissingBaseUrl("HERMES".to_string()))?;
        let api_mode = self
            .hermes
            .api_mode
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("chat_completions");

        let (slug, url_env, key_env) = match api_mode {
            "anthropic_messages" => ("anthropic", "ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY"),
            "codex_responses" => ("xai", "XAI_BASE_URL", "XAI_API_KEY"),
            // chat_completions (default) and any unknown mode route through
            // openrouter — the most common chat-completions carrier.
            _ => ("openrouter", "OPENROUTER_BASE_URL", "OPENROUTER_API_KEY"),
        };

        let mut env = self.hermes.env.clone();
        // Carrier + credential + endpoint set LAST so vendor-quirk overlays
        // above can't accidentally shadow them.
        env.insert("HERMES_INFERENCE_PROVIDER".to_string(), slug.to_string());
        env.insert(url_env.to_string(), base_url.to_string());
        env.insert(key_env.to_string(), api_key.to_string());
        if !model_id.is_empty() {
            env.insert("HERMES_INFERENCE_MODEL".to_string(), model_id.to_string());
        }
        Ok(Some(env))
    }

    /// Per-agent spawn injection — the single dispatch point that route
    /// handlers call when a user picks a provider for a given message.
    ///
    /// Each agent gets its own applier (Codex via `build_codex_injection`,
    /// OpenCode via `build_opencode_injection`, Gemini via
    /// `build_gemini_injection`, DeepSeek TUI via
    /// `build_deepseek_tui_injection`, Hermes via `build_hermes_injection`,
    /// Claude via `build_spawn_env`). Adding a new agent only adds a match
    /// arm here and a field on `AgentInjection`; route handlers stay put.
    ///
    /// The catch-all arm calls `build_spawn_env` (Claude env). Agents
    /// without their own applier (Amp, Cursor, Qwen, Copilot, Droid)
    /// land there with the Claude-shaped env, picker-gated off via
    /// `record.perAgentEnabled` until each grows a dedicated applier.
    pub fn build_agent_injection(
        &self,
        agent: BaseCodingAgent,
        model_id: &str,
    ) -> Result<AgentInjection, ProviderError> {
        match agent {
            BaseCodingAgent::Codex => match self.build_codex_injection()? {
                Some((env, codex)) => Ok(AgentInjection {
                    env: Some(env),
                    codex: Some(codex),
                }),
                None => Ok(AgentInjection::default()),
            },
            BaseCodingAgent::Opencode => Ok(AgentInjection {
                env: self.build_opencode_injection()?,
                codex: None,
            }),
            BaseCodingAgent::Gemini => Ok(AgentInjection {
                env: self.build_gemini_injection()?,
                codex: None,
            }),
            BaseCodingAgent::DeepseekTui => Ok(AgentInjection {
                env: self.build_deepseek_tui_injection(model_id)?,
                codex: None,
            }),
            BaseCodingAgent::Hermes => Ok(AgentInjection {
                env: self.build_hermes_injection(model_id)?,
                codex: None,
            }),
            _ => {
                let env = self.build_spawn_env(model_id);
                let env = if env.is_empty() { None } else { Some(env) };
                Ok(AgentInjection { env, codex: None })
            }
        }
    }
}

/// Per-agent spawn injection bundle — env vars + agent-specific structured
/// payloads. One slot per agent that has a non-trivial spawn-time applier.
/// Phases D/E/F add more slots; route handlers remain agent-agnostic.
#[derive(Debug, Clone, Default)]
pub struct AgentInjection {
    /// Process env to merge into `ExecutionEnv.provider_vars` for spawn.
    /// Carries `CDT_API_KEY` for Codex, native auth env for every other
    /// agent (e.g. `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` for Claude).
    pub env: Option<HashMap<String, String>>,
    /// Codex-only: dotted-path overrides + `model_provider` id merged into
    /// `ThreadStartParams` at spawn (see `Codex::build_thread_start_params`).
    pub codex: Option<CodexProviderInjection>,
    // OpenCode (Phase D) ships its config via the `OPENCODE_CONFIG_CONTENT`
    // env var inside `env` — no dedicated slot needed. Gemini (Phase F)
    // and DeepSeek TUI (Phase E) are env-only too — the DeepSeek TUI
    // runtime intentionally exposes no provider/credential CLI flags, so
    // it never needs a dedicated argv slot.
}

#[cfg(test)]
mod codex_injection_tests {
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

    #[test]
    fn default_returns_none() {
        let p = provider_with_codex(
            AiProviderKind::Default,
            Some("ignored"),
            Some("ignored"),
            HashMap::new(),
        );
        assert!(p.build_codex_injection().unwrap().is_none());
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
            p.build_codex_injection(),
            Err(ProviderError::MissingApiKey(_))
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
            p.build_codex_injection(),
            Err(ProviderError::MissingApiKey(_))
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
            p.build_codex_injection(),
            Err(ProviderError::EnabledAgentMissingBaseUrl(_))
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
        let (env, injection) = p.build_codex_injection().unwrap().unwrap();

        assert_eq!(env.get("CDT_API_KEY").map(String::as_str), Some("sk-test"));
        assert_eq!(injection.model_provider_id, "cdt");

        let cfg = &injection.config_overrides;
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
        let (env, _) = p.build_codex_injection().unwrap().unwrap();
        assert_eq!(env.get("CDT_API_KEY").map(String::as_str), Some("real-key"));
        assert_eq!(
            env.get("OPENAI_TIMEOUT_MS").map(String::as_str),
            Some("30000")
        );
    }
}

#[cfg(test)]
mod opencode_injection_tests {
    use serde_json::json;

    use super::*;

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
        assert!(p.build_opencode_injection().unwrap().is_none());
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
            p.build_opencode_injection(),
            Err(ProviderError::MissingApiKey(_))
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
            p.build_opencode_injection(),
            Err(ProviderError::EnabledAgentMissingBaseUrl(_))
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
            p.build_opencode_injection(),
            Err(ProviderError::EmptyEnabledModels)
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

        let env = p.build_opencode_injection().unwrap().unwrap();
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
        let env = p.build_opencode_injection().unwrap().unwrap();
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

        let env = p.build_opencode_injection().unwrap().unwrap();
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
        let env = p.build_opencode_injection().unwrap().unwrap();
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
        let env = p.build_opencode_injection().unwrap().unwrap();
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

    #[test]
    fn default_returns_none() {
        let p = provider_with_gemini(
            AiProviderKind::Default,
            Some("ignored"),
            Some("ignored"),
            HashMap::new(),
        );
        assert!(p.build_gemini_injection().unwrap().is_none());
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
            p.build_gemini_injection(),
            Err(ProviderError::MissingApiKey(_))
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
            p.build_gemini_injection(),
            Err(ProviderError::MissingApiKey(_))
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
            p.build_gemini_injection(),
            Err(ProviderError::EnabledAgentMissingBaseUrl(_))
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
        let env = p.build_gemini_injection().unwrap().unwrap();
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

        let env = p.build_gemini_injection().unwrap().unwrap();
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
        let env = p.build_gemini_injection().unwrap().unwrap();
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
        let env = p.build_gemini_injection().unwrap().unwrap();
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
        // build_agent_injection(Gemini, _) must hit build_gemini_injection,
        // not the catch-all that emits Claude env vars. Regression guard
        // against a non-Default provider used with Gemini accidentally
        // injecting ANTHROPIC_BASE_URL into the gemini-cli child.
        let p = provider_with_gemini(
            AiProviderKind::Custom,
            Some("sk-real"),
            Some("https://generativelanguage.googleapis.com"),
            HashMap::new(),
        );
        let inj = p
            .build_agent_injection(BaseCodingAgent::Gemini, "gemini-3-pro-preview")
            .unwrap();
        let env = inj.env.expect("Custom Gemini emits env");
        assert!(env.contains_key("GEMINI_API_KEY"));
        assert!(env.contains_key("GOOGLE_GEMINI_BASE_URL"));
        assert!(!env.contains_key("ANTHROPIC_BASE_URL"));
        assert!(!env.contains_key("ANTHROPIC_AUTH_TOKEN"));
        assert!(inj.codex.is_none());
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
        let inj = p
            .build_agent_injection(BaseCodingAgent::Gemini, "gemini-3-pro-preview")
            .unwrap();
        assert!(inj.env.is_none());
        assert!(inj.codex.is_none());
    }
}
