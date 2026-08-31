//! Launch-time auth binding resolution.
//!
//! A `SessionCommandConfig` carries only opaque identifiers: `auth_binding_id`
//! names the credential binding to launch with, `selected_provider_id` names
//! the provider selection shown in the UI. Resolution to real credential
//! material happens here, immediately before the executor spawns, and the
//! resolved material lives only in the in-memory [`AgentInjection`] — it is
//! never persisted, serialized, or logged (see `ExecutorAction`'s
//! `serde(skip)` provider fields and the redacting `Debug` impls).

use db::models::{
    provider::{Provider, ProviderError},
    session_command::SessionCommandConfig,
};
use executors::{profile::ExecutorConfig, provider::ProviderInjection};
use sqlx::SqlitePool;
use thiserror::Error;
use utils::redact::redact_text;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AuthBindingError {
    #[error("Auth binding '{0}' not found")]
    NotFound(Uuid),
    #[error("Auth binding '{0}' is disabled")]
    Disabled(String),
    /// Message is redacted against the binding's credential material before
    /// construction; safe to log and surface.
    #[error("Auth binding resolution failed: {0}")]
    Resolution(String),
}

/// Credential material and safe metadata for one launch attempt.
///
/// Manual `Debug` below prints only opaque identifiers: the loaded provider
/// record carries a plain credential and must never reach logs.
pub struct ResolvedAuthBinding {
    /// Opaque identifier the injection was resolved from, if any.
    pub binding_id: Option<Uuid>,
    /// Loaded provider record backing the binding (`None` for ambient auth).
    pub provider: Option<Provider>,
    /// Spawn-time credential injection. In-memory only.
    pub injection: ProviderInjection,
}

impl std::fmt::Debug for ResolvedAuthBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedAuthBinding")
            .field("binding_id", &self.binding_id)
            .field("provider_id", &self.provider.as_ref().map(|p| p.id))
            .field("injection", &self.injection)
            .finish()
    }
}

/// Resolve the auth binding for a launch. `auth_binding_id` is authoritative;
/// `selected_provider_id` is the compatibility fallback for configs enqueued
/// before bindings existed. No binding means ambient executor auth.
///
/// Mutates `executor_config.model_id` to the id form the target harness
/// addresses models by, mirroring the route-side resolution.
pub async fn resolve_for_launch(
    pool: &SqlitePool,
    config: &SessionCommandConfig,
    executor_config: &mut ExecutorConfig,
) -> Result<ResolvedAuthBinding, AuthBindingError> {
    let binding_id = config.auth_binding_id.or(config.selected_provider_id);
    let Some(id) = binding_id else {
        return Ok(ResolvedAuthBinding {
            binding_id: None,
            provider: None,
            injection: ProviderInjection::default(),
        });
    };

    let provider = Provider::find_by_id(pool, id)
        .await
        .map_err(|error| match error {
            ProviderError::NotFound => AuthBindingError::NotFound(id),
            other => AuthBindingError::Resolution(other.to_string()),
        })?;
    if !provider.enabled {
        return Err(AuthBindingError::Disabled(provider.name.clone()));
    }

    let (model_id, injection) = provider
        .resolve_injection(
            executor_config.executor,
            executor_config.model_id.as_deref().unwrap_or(""),
        )
        .map_err(|error| {
            // ProviderError messages carry no credential values today; the
            // scrub keeps that true even if a future variant embeds one.
            let resolved = provider.resolved_api_key(executor_config.executor);
            let secrets: Vec<&str> = provider
                .api_key
                .as_deref()
                .into_iter()
                .chain(resolved.as_deref())
                .collect();
            AuthBindingError::Resolution(redact_text(&error.to_string(), secrets))
        })?;
    if executor_config.model_id.is_some() {
        executor_config.model_id = Some(model_id);
    }

    Ok(ResolvedAuthBinding {
        binding_id: Some(id),
        provider: Some(provider),
        injection,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use db::{
        models::provider::{AiProviderKind, CreateProvider, EnabledModel},
        provider_payloads::ClaudePayload,
    };
    use executors::executors::BaseCodingAgent;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        pool
    }

    async fn create_provider(pool: &SqlitePool, name: &str, api_key: &str) -> Provider {
        Provider::create(
            pool,
            Uuid::new_v4(),
            &CreateProvider {
                name: name.to_string(),
                kind: AiProviderKind::Custom,
                preset_id: None,
                api_key: Some(api_key.to_string()),
                per_agent_enabled: HashMap::from([("CLAUDE_CODE".to_string(), true)]),
                claude: ClaudePayload {
                    base_url: Some("https://provider.test/v1".to_string()),
                    ..ClaudePayload::default()
                },
                codex: Default::default(),
                opencode: Default::default(),
                deepseek_tui: Default::default(),
                gemini: Default::default(),
                hermes: Default::default(),
                enabled_models: vec![EnabledModel {
                    id: "claude-fable-5".to_string(),
                    display_name: "Fable".to_string(),
                    owned_by: None,
                }],
            },
        )
        .await
        .unwrap()
    }

    fn config(
        auth_binding_id: Option<Uuid>,
        selected_provider_id: Option<Uuid>,
    ) -> SessionCommandConfig {
        SessionCommandConfig {
            executor_config: ExecutorConfig::new(BaseCodingAgent::ClaudeCode),
            selected_provider_id,
            auth_binding_id,
            metered: None,
        }
    }

    #[tokio::test]
    async fn no_binding_resolves_to_ambient_default() {
        let pool = pool().await;
        let config = config(None, None);
        let mut executor_config = config.executor_config.clone();

        let resolved = resolve_for_launch(&pool, &config, &mut executor_config)
            .await
            .unwrap();

        assert_eq!(resolved.binding_id, None);
        assert!(resolved.provider.is_none());
        assert!(resolved.injection.env.is_none());
        assert!(resolved.injection.structured.is_none());
    }

    #[tokio::test]
    async fn auth_binding_id_is_authoritative_over_selected_provider() {
        let pool = pool().await;
        let bound = create_provider(&pool, "bound-account", "binding-key").await;
        let selected = create_provider(&pool, "selected-account", "selected-key").await;

        let config = config(Some(bound.id), Some(selected.id));
        let mut executor_config = config.executor_config.clone();
        let resolved = resolve_for_launch(&pool, &config, &mut executor_config)
            .await
            .unwrap();

        assert_eq!(resolved.binding_id, Some(bound.id));
        assert_eq!(resolved.provider.as_ref().map(|p| p.id), Some(bound.id));
        let env = resolved.injection.env.expect("injection env");
        assert_eq!(
            env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str),
            Some("binding-key")
        );
    }

    #[tokio::test]
    async fn selected_provider_is_the_compatibility_fallback() {
        let pool = pool().await;
        let selected = create_provider(&pool, "selected-account", "selected-key").await;

        let config = config(None, Some(selected.id));
        let mut executor_config = config.executor_config.clone();
        let resolved = resolve_for_launch(&pool, &config, &mut executor_config)
            .await
            .unwrap();

        assert_eq!(resolved.binding_id, Some(selected.id));
        let env = resolved.injection.env.expect("injection env");
        assert_eq!(
            env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str),
            Some("selected-key")
        );
    }

    #[tokio::test]
    async fn disabled_binding_fails_closed_without_leaking_credentials() {
        let pool = pool().await;
        let provider = create_provider(&pool, "cooled-account", "top-secret-key").await;
        sqlx::query("UPDATE providers SET enabled = 0 WHERE id = ?")
            .bind(provider.id.to_string())
            .execute(&pool)
            .await
            .unwrap();

        let config = config(Some(provider.id), None);
        let mut executor_config = config.executor_config.clone();
        let error = resolve_for_launch(&pool, &config, &mut executor_config)
            .await
            .unwrap_err();

        let message = error.to_string();
        assert!(matches!(error, AuthBindingError::Disabled(_)));
        assert!(message.contains("cooled-account"));
        assert!(!message.contains("top-secret-key"));
    }

    #[tokio::test]
    async fn unknown_binding_fails_with_not_found() {
        let pool = pool().await;
        let missing = Uuid::new_v4();
        let config = config(Some(missing), None);
        let mut executor_config = config.executor_config.clone();

        let error = resolve_for_launch(&pool, &config, &mut executor_config)
            .await
            .unwrap_err();
        assert!(matches!(error, AuthBindingError::NotFound(id) if id == missing));
    }
}
