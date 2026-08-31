use db::models::provider::Provider;
use executors::{profile::ExecutorConfig, provider::ProviderInjection};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::ApiError;

/// Build the spawn-time provider injection for a selected provider id.
/// Mirrors what `workspaces::create::create_and_start_workspace` builds before
/// calling `start_workspace`; shared so the routine spawn path behaves
/// identically.
///
/// Mutates `executor_config.model_id` to the id form the target harness
/// addresses models by.
///
/// Returns the default (empty) injection when no provider id is supplied.
pub async fn build_injection_for_provider(
    pool: &SqlitePool,
    selected_provider_id: Option<Uuid>,
    executor_config: &mut ExecutorConfig,
) -> Result<ProviderInjection, ApiError> {
    let Some(provider_id) = selected_provider_id else {
        return Ok(ProviderInjection::default());
    };

    let provider = Provider::find_by_id(pool, provider_id)
        .await
        .map_err(|_| ApiError::BadRequest(format!("Provider '{provider_id}' not found")))?;
    if !provider.enabled {
        return Err(ApiError::BadRequest(format!(
            "Provider '{}' is disabled",
            provider.name
        )));
    }
    let (model_id, injection) = provider
        .resolve_injection(
            executor_config.executor,
            executor_config.model_id.as_deref().unwrap_or(""),
        )
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if executor_config.model_id.is_some() {
        executor_config.model_id = Some(model_id);
    }
    Ok(injection)
}
