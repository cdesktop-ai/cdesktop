use axum::{
    Json, Router,
    extract::{Path, State},
    response::Json as ResponseJson,
    routing::{delete, get, post, put},
};
use db::{
    models::{
        coding_agent_turn::{CodingAgentTurn, RecentModelProviderPair},
        provider::{CreateProvider, Provider, ProviderError, UpdateProvider},
    },
    provider_catalog::{CatalogPreset, load_catalog},
};
use deployment::Deployment;
use serde::{Deserialize, Serialize};
use services::services::model_fetch::{FetchedModel, fetch_models};
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

impl From<ProviderError> for ApiError {
    fn from(e: ProviderError) -> Self {
        match e {
            ProviderError::NotFound => ApiError::BadRequest("Provider not found".to_string()),
            ProviderError::CannotDeleteDefault => {
                ApiError::BadRequest("Cannot delete the Default provider".to_string())
            }
            ProviderError::EmptyEnabledModels => {
                ApiError::BadRequest("enabledModels must not be empty".to_string())
            }
            ProviderError::Database(e) => ApiError::Database(e),
            e => ApiError::BadRequest(e.to_string()),
        }
    }
}

pub fn router(deployment: &DeploymentImpl) -> Router<DeploymentImpl> {
    Router::new()
        .route("/providers/catalog", get(get_catalog))
        .route("/providers/recents", get(get_recents))
        .route("/providers", get(list_providers))
        .route("/providers", post(create_provider))
        .route("/providers/{id}", get(get_provider))
        .route("/providers/{id}", put(update_provider))
        .route("/providers/{id}", delete(delete_provider))
        .route("/providers/fetch-models", post(fetch_provider_models))
        .with_state(deployment.clone())
}

pub async fn get_catalog() -> ResponseJson<ApiResponse<&'static [CatalogPreset]>> {
    ResponseJson(ApiResponse::success(load_catalog().presets.as_slice()))
}

pub async fn get_recents(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<RecentModelProviderPair>>>, ApiError> {
    let pool = &deployment.db().pool;
    let pairs = CodingAgentTurn::recent_model_provider_pairs(pool, 5)
        .await
        .map_err(ApiError::Database)?;
    Ok(ResponseJson(ApiResponse::success(pairs)))
}

pub async fn list_providers(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<Provider>>>, ApiError> {
    let pool = &deployment.db().pool;
    let providers = Provider::list(pool).await?;
    Ok(ResponseJson(ApiResponse::success(providers)))
}

pub async fn get_provider(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<Provider>>, ApiError> {
    let pool = &deployment.db().pool;
    let provider = Provider::find_by_id(pool, id).await?;
    Ok(ResponseJson(ApiResponse::success(provider)))
}

pub async fn create_provider(
    State(deployment): State<DeploymentImpl>,
    Json(body): Json<CreateProvider>,
) -> Result<ResponseJson<ApiResponse<Provider>>, ApiError> {
    let pool = &deployment.db().pool;
    let id = Uuid::new_v4();
    let provider = Provider::create(pool, id, &body).await?;
    Ok(ResponseJson(ApiResponse::success(provider)))
}

pub async fn update_provider(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateProvider>,
) -> Result<ResponseJson<ApiResponse<Provider>>, ApiError> {
    let pool = &deployment.db().pool;
    let provider = Provider::update(pool, id, &body).await?;
    Ok(ResponseJson(ApiResponse::success(provider)))
}

pub async fn delete_provider(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<ResponseJson<ApiResponse<()>>, ApiError> {
    let pool = &deployment.db().pool;
    Provider::delete(pool, id).await?;
    Ok(ResponseJson(ApiResponse::success(())))
}

#[derive(Debug, Deserialize)]
pub struct FetchModelsRequest {
    pub base_url: String,
    pub api_key: String,
    pub models_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FetchModelsResponse {
    pub models: Vec<FetchedModel>,
}

pub async fn fetch_provider_models(
    Json(body): Json<FetchModelsRequest>,
) -> Result<ResponseJson<ApiResponse<FetchModelsResponse>>, ApiError> {
    let models = fetch_models(&body.base_url, &body.api_key, body.models_url.as_deref())
        .await
        .map_err(ApiError::BadRequest)?;

    Ok(ResponseJson(ApiResponse::success(FetchModelsResponse {
        models,
    })))
}
