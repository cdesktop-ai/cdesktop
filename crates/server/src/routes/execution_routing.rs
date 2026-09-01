//! Read-only projection of sightmesh's execution routing settings.
//!
//! sightmesh owns these settings and writes them to a versioned JSON file;
//! cdesktop only reads them so the Settings dialog can show what routing is
//! actually configured instead of a fixture. Write-back is deliberately not
//! offered here - the owning process must stay the only writer.
//!
//! The file holds routing policy only. Credentials live in sightmesh's
//! separate pool token store, which this module never reads, so the whole
//! projection is secrets-free by construction rather than by filtering.

use std::path::{Path, PathBuf};

use axum::{Router, response::Json as ResponseJson, routing::get};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use utils::response::ApiResponse;

use crate::{DeploymentImpl, error::ApiError};

/// The settings-file layout version this projection understands. A file
/// written by a newer sightmesh is reported as unreadable rather than
/// silently misprojected.
const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRoutingRoute {
    pub id: String,
    pub executor: String,
    pub model: String,
    pub billing_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_pool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// The routing policy sightmesh has persisted.
///
/// Fields the settings file does not carry - a route's live health, its
/// display label, the resolved provider - are absent on purpose: they are
/// runtime state, not settings, and the dashboard keeps labelling those as
/// fixtures until something real backs them.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRoutingSettings {
    pub enabled: bool,
    pub routes: Vec<ExecutionRoutingRoute>,
    pub metered_fallback: String,
    pub same_route_retries: u32,
    pub transient_backoff_seconds: Vec<u32>,
    pub approval_timeout_minutes: u32,
    pub all_routes_exhausted: String,
    pub notify_on_swap: bool,
    pub expose_account_alias: bool,
    pub fallback_on_free_failure: bool,
}

#[derive(Debug, Deserialize)]
struct SettingsFile {
    version: u32,
    #[serde(rename = "executionRouting")]
    execution_routing: ExecutionRoutingSettings,
}

fn settings_path() -> PathBuf {
    std::env::var_os("SIGHTMESH_EXECUTION_ROUTING")
        .map(PathBuf::from)
        .or_else(|| {
            dirs::home_dir().map(|home| home.join(".config/sightmesh/execution_routing.json"))
        })
        .unwrap_or_else(|| PathBuf::from(".config/sightmesh/execution_routing.json"))
}

/// `Ok(None)` means sightmesh has never written routing settings on this host,
/// which is the ordinary case for a cdesktop running without it. A file that
/// exists but cannot be projected is an error rather than a `None`, so a
/// broken or newer-versioned file never reads as "not configured".
fn read_settings(path: &Path) -> Result<Option<ExecutionRoutingSettings>, ApiError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(ApiError::BadGateway(format!(
                "Cannot read sightmesh routing settings at {}: {err}",
                path.display()
            )));
        }
    };

    let file: SettingsFile = serde_json::from_str(&contents).map_err(|err| {
        ApiError::BadGateway(format!(
            "sightmesh routing settings at {} are not readable: {err}",
            path.display()
        ))
    })?;

    if file.version != SETTINGS_VERSION {
        return Err(ApiError::BadGateway(format!(
            "Unsupported sightmesh routing settings version {} at {}",
            file.version,
            path.display()
        )));
    }

    Ok(Some(file.execution_routing))
}

async fn get_execution_routing_settings()
-> Result<ResponseJson<ApiResponse<Option<ExecutionRoutingSettings>>>, ApiError> {
    Ok(ResponseJson(ApiResponse::success(read_settings(
        &settings_path(),
    )?)))
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new().route(
        "/execution-routing/settings",
        get(get_execution_routing_settings),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured verbatim from `ExecutionRoutingStore.save` in sightmesh
    /// (src/sightmesh/execution_routing.py), so this asserts against the real
    /// on-disk contract rather than a hand-written guess at it.
    const REAL_SETTINGS: &str = r#"{
  "executionRouting": {
    "allRoutesExhausted": "block",
    "approvalTimeoutMinutes": 0,
    "enabled": true,
    "exposeAccountAlias": true,
    "fallbackOnFreeFailure": false,
    "meteredFallback": "ask",
    "notifyOnSwap": true,
    "routes": [
      {
        "accountPool": "codex",
        "billingClass": "subscription",
        "executor": "CODEX",
        "id": "codex-subs",
        "model": "gpt-5-codex"
      },
      {
        "account": "acct-luna",
        "billingClass": "metered",
        "executor": "CLAUDE_CODE",
        "id": "claude-metered",
        "model": "claude-opus-5"
      },
      {
        "billingClass": "free",
        "executor": "OPENCODE",
        "id": "opencode-free",
        "model": "grok-code"
      }
    ],
    "sameRouteRetries": 2,
    "transientBackoffSeconds": [
      5,
      20
    ]
  },
  "version": 1
}
"#;

    fn write(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("execution_routing.json");
        std::fs::write(&path, contents).unwrap();
        (dir, path)
    }

    #[test]
    fn projects_real_sightmesh_settings() {
        let (_dir, path) = write(REAL_SETTINGS);

        let settings = read_settings(&path).unwrap().expect("settings present");

        assert!(settings.enabled);
        assert_eq!(settings.metered_fallback, "ask");
        assert_eq!(settings.same_route_retries, 2);
        assert_eq!(settings.transient_backoff_seconds, vec![5, 20]);
        assert_eq!(settings.routes.len(), 3);
        assert_eq!(settings.routes[0].id, "codex-subs");
        assert_eq!(settings.routes[0].account_pool.as_deref(), Some("codex"));
        assert_eq!(settings.routes[1].account.as_deref(), Some("acct-luna"));
        // A free route names neither, and must project as neither.
        assert_eq!(settings.routes[2].account_pool, None);
        assert_eq!(settings.routes[2].account, None);
    }

    #[test]
    fn absent_settings_are_not_an_error() {
        let dir = tempfile::tempdir().unwrap();

        assert!(
            read_settings(&dir.path().join("missing.json"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unreadable_settings_do_not_read_as_unconfigured() {
        let (_dir, path) = write("{ not json");

        assert!(read_settings(&path).is_err());
    }

    #[test]
    fn newer_settings_version_is_refused() {
        let (_dir, path) = write(r#"{"version": 2, "executionRouting": {}}"#);

        assert!(read_settings(&path).is_err());
    }
}
