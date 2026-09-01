//! Durable metered-fallback approval state machine (plan §12).
//!
//! When a session command's config declares metered execution, the command
//! dispatcher consults [`MeteredApproval::gate`] before claiming:
//!
//! - `auto`: launch proceeds; the winning claim records a durable
//!   `auto_started` row as the notification of metered spend.
//! - `ask`: the gate durably creates one pending approval and holds the
//!   command; an approval authorizes exactly one attempt (consumption is
//!   stamped with the claimed execution process id), a denial leaves the
//!   command blocked with its checkpoint intact.
//! - `never`: the command blocks with a durable `routes_exhausted` record and
//!   metered work never starts.
//!
//! All rows survive restart; resume flows through the normal dispatcher whose
//! native single-winner claim guarantees exactly-once launch.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool, Type};
use ts_rs::TS;
use uuid::Uuid;

use super::session_command::SessionCommand;

pub const ROUTES_EXHAUSTED_REASON: &str = "routes_exhausted";

#[derive(Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS)]
#[sqlx(type_name = "metered_approval_policy", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[ts(use_ts_enum)]
pub enum MeteredApprovalPolicy {
    Auto,
    Ask,
    Never,
}

#[derive(Debug, Clone, Copy, Type, Serialize, Deserialize, PartialEq, Eq, TS)]
#[sqlx(type_name = "metered_approval_state", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(use_ts_enum)]
pub enum MeteredApprovalState {
    Pending,
    Approved,
    Denied,
    AutoStarted,
    Blocked,
}

/// Safe metered-execution declaration carried inside `SessionCommandConfig`.
/// Contains policy plus display-only metadata; never credential material.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct MeteredExecution {
    pub policy: MeteredApprovalPolicy,
    #[serde(default)]
    #[ts(optional)]
    pub account_alias: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct MeteredApproval {
    pub id: Uuid,
    pub session_command_id: Uuid,
    pub policy: MeteredApprovalPolicy,
    pub state: MeteredApprovalState,
    pub account_alias: Option<String>,
    pub reason: Option<String>,
    /// Set when an approval (or auto start) was consumed by a claimed
    /// attempt — the allow-once linkage.
    pub execution_process_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeteredGateDecision {
    /// Launch may proceed through the normal claim path.
    Proceed,
    /// A durable approval is pending; hold the command without claiming.
    AwaitApproval,
    /// Metered execution is not allowed; the command stays queued with its
    /// checkpoint intact and never launches under the current policy.
    Blocked,
}

impl MeteredApproval {
    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM metered_approvals WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    pub async fn list_pending(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM metered_approvals WHERE state = 'pending' ORDER BY rowid")
            .fetch_all(pool)
            .await
    }

    pub async fn find_latest_for_command(
        pool: &SqlitePool,
        session_command_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM metered_approvals WHERE session_command_id = ? \
             ORDER BY rowid DESC LIMIT 1",
        )
        .bind(session_command_id)
        .fetch_optional(pool)
        .await
    }

    /// Decide whether the head-of-queue command may launch. Durable and
    /// idempotent: repeated calls converge on the same decision, creating at
    /// most one pending approval row (partial unique index) for `ask`.
    pub async fn gate(
        pool: &SqlitePool,
        command: &SessionCommand,
    ) -> Result<MeteredGateDecision, sqlx::Error> {
        let Some(metered) = &command.config.0.metered else {
            return Ok(MeteredGateDecision::Proceed);
        };

        match metered.policy {
            MeteredApprovalPolicy::Auto => Ok(MeteredGateDecision::Proceed),
            MeteredApprovalPolicy::Ask => {
                match Self::find_latest_for_command(pool, command.id).await? {
                    Some(latest) if latest.state == MeteredApprovalState::Pending => {
                        Ok(MeteredGateDecision::AwaitApproval)
                    }
                    Some(latest)
                        if latest.state == MeteredApprovalState::Approved
                            && latest.execution_process_id.is_none() =>
                    {
                        Ok(MeteredGateDecision::Proceed)
                    }
                    Some(latest) if latest.state == MeteredApprovalState::Denied => {
                        Ok(MeteredGateDecision::Blocked)
                    }
                    // No history, a consumed approval, or a stale
                    // auto/blocked record from an earlier policy: this
                    // attempt needs its own approval.
                    _ => {
                        Self::ensure_pending(pool, command.id, metered.account_alias.as_deref())
                            .await?;
                        Ok(MeteredGateDecision::AwaitApproval)
                    }
                }
            }
            MeteredApprovalPolicy::Never => {
                Self::ensure_blocked(pool, command.id, metered.account_alias.as_deref()).await?;
                Ok(MeteredGateDecision::Blocked)
            }
        }
    }

    /// Create the single pending approval for a command if none is open.
    async fn ensure_pending(
        pool: &SqlitePool,
        session_command_id: Uuid,
        account_alias: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO metered_approvals (id, session_command_id, policy, state, account_alias) \
             VALUES (?, ?, 'ask', 'pending', ?) \
             ON CONFLICT(session_command_id) WHERE state = 'pending' DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(session_command_id)
        .bind(account_alias)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Durably record that `never` policy blocked this command. One record
    /// per command is enough; repeats are no-ops.
    async fn ensure_blocked(
        pool: &SqlitePool,
        session_command_id: Uuid,
        account_alias: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO metered_approvals \
             (id, session_command_id, policy, state, account_alias, reason, resolved_at) \
             SELECT ?, ?, 'never', 'blocked', ?, ?, datetime('now', 'subsec') \
             WHERE NOT EXISTS (\
                 SELECT 1 FROM metered_approvals \
                 WHERE session_command_id = ? AND state = 'blocked'\
             )",
        )
        .bind(Uuid::new_v4())
        .bind(session_command_id)
        .bind(account_alias)
        .bind(ROUTES_EXHAUSTED_REASON)
        .bind(session_command_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Resolve a pending approval. Single-winner: returns `false` when the
    /// approval was already resolved (or does not exist), so a duplicate
    /// response can never flip an earlier decision.
    pub async fn respond(
        pool: &SqlitePool,
        id: Uuid,
        approved: bool,
        reason: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let state = if approved {
            MeteredApprovalState::Approved
        } else {
            MeteredApprovalState::Denied
        };
        let result = sqlx::query(
            "UPDATE metered_approvals \
             SET state = ?, reason = ?, resolved_at = datetime('now', 'subsec') \
             WHERE id = ? AND state = 'pending'",
        )
        .bind(state)
        .bind(reason)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Stamp the unconsumed approval for a command with the claimed
    /// execution process — the approval now authorizes exactly this attempt
    /// and no future one (allow-once).
    pub async fn consume_approval(
        pool: &SqlitePool,
        session_command_id: Uuid,
        execution_process_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE metered_approvals SET execution_process_id = ? \
             WHERE session_command_id = ? AND state = 'approved' \
             AND execution_process_id IS NULL",
        )
        .bind(execution_process_id)
        .bind(session_command_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Durable notification that `auto` policy started metered execution for
    /// a claimed attempt. Keyed by execution process, so each launched
    /// attempt records exactly one auto-start.
    pub async fn record_auto_start(
        pool: &SqlitePool,
        session_command_id: Uuid,
        execution_process_id: Uuid,
        account_alias: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO metered_approvals \
             (id, session_command_id, policy, state, account_alias, execution_process_id, resolved_at) \
             SELECT ?, ?, 'auto', 'auto_started', ?, ?, datetime('now', 'subsec') \
             WHERE NOT EXISTS (\
                 SELECT 1 FROM metered_approvals WHERE execution_process_id = ?\
             )",
        )
        .bind(Uuid::new_v4())
        .bind(session_command_id)
        .bind(account_alias)
        .bind(execution_process_id)
        .bind(execution_process_id)
        .execute(pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use executors::profile::ExecutorConfig;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::models::session_command::{
        NewSessionCommand, SessionCommandConfig, SessionCommandIntent,
    };

    const SCHEMA: [&str; 5] = [
        r#"CREATE TABLE session_commands (
            id BLOB PRIMARY KEY NOT NULL,
            session_id BLOB NOT NULL,
            dedupe_key TEXT,
            intent TEXT NOT NULL,
            body TEXT NOT NULL,
            config TEXT,
            state TEXT NOT NULL DEFAULT 'pending',
            execution_process_id BLOB,
            attempt_number INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            finished_at TEXT
        )"#,
        "CREATE UNIQUE INDEX dedupe ON session_commands(session_id, dedupe_key) \
         WHERE dedupe_key IS NOT NULL",
        r#"CREATE TABLE metered_approvals (
            id BLOB PRIMARY KEY NOT NULL,
            session_command_id BLOB NOT NULL,
            policy TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'pending',
            account_alias TEXT,
            reason TEXT,
            execution_process_id BLOB,
            created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
            resolved_at TEXT
        )"#,
        "CREATE UNIQUE INDEX metered_approvals_one_pending \
         ON metered_approvals (session_command_id) WHERE state = 'pending'",
        "CREATE INDEX metered_approvals_by_command \
         ON metered_approvals (session_command_id)",
    ];

    async fn apply_schema(pool: &SqlitePool) {
        for statement in SCHEMA {
            sqlx::query(statement).execute(pool).await.unwrap();
        }
    }

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        apply_schema(&pool).await;
        pool
    }

    fn metered_command(
        session_id: Uuid,
        policy: Option<MeteredApprovalPolicy>,
    ) -> NewSessionCommand {
        metered_command_for(
            session_id,
            policy,
            executors::executors::BaseCodingAgent::ClaudeCode,
        )
    }

    fn metered_command_for(
        session_id: Uuid,
        policy: Option<MeteredApprovalPolicy>,
        executor: executors::executors::BaseCodingAgent,
    ) -> NewSessionCommand {
        NewSessionCommand {
            session_id,
            dedupe_key: None,
            intent: SessionCommandIntent::Continue,
            body: "resume the task".to_owned(),
            config: SessionCommandConfig {
                executor_config: ExecutorConfig::new(executor),
                selected_provider_id: None,
                auth_binding_id: None,
                metered: policy.map(|policy| MeteredExecution {
                    policy,
                    account_alias: Some("max-a".to_owned()),
                }),
            },
        }
    }

    async fn enqueue(pool: &SqlitePool, policy: Option<MeteredApprovalPolicy>) -> SessionCommand {
        let (command, _) = SessionCommand::enqueue(pool, metered_command(Uuid::new_v4(), policy))
            .await
            .unwrap();
        command
    }

    /// The metered gate reads the command's policy and nothing else. Tool
    /// approvals are brokered per harness and differ widely between them; this
    /// gate runs above the adapters, before any executor is spawned, so it has
    /// to be blind to which harness the command names.
    #[tokio::test]
    async fn the_metered_gate_decides_the_same_way_for_every_harness() {
        use executors::executors::BaseCodingAgent::{ClaudeCode, Codex, Opencode};

        let pool = pool().await;

        for (policy, expected) in [
            (None, MeteredGateDecision::Proceed),
            (
                Some(MeteredApprovalPolicy::Auto),
                MeteredGateDecision::Proceed,
            ),
            (
                Some(MeteredApprovalPolicy::Ask),
                MeteredGateDecision::AwaitApproval,
            ),
            (
                Some(MeteredApprovalPolicy::Never),
                MeteredGateDecision::Blocked,
            ),
        ] {
            for executor in [ClaudeCode, Codex, Opencode] {
                let (command, _) = SessionCommand::enqueue(
                    &pool,
                    metered_command_for(Uuid::new_v4(), policy, executor),
                )
                .await
                .unwrap();

                assert_eq!(
                    MeteredApproval::gate(&pool, &command).await.unwrap(),
                    expected,
                    "policy {policy:?} decided differently for {executor:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn unmetered_command_proceeds_without_rows() {
        let pool = pool().await;
        let command = enqueue(&pool, None).await;

        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::Proceed
        );
        assert!(
            MeteredApproval::list_pending(&pool)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn auto_policy_proceeds_and_records_durable_auto_start_once() {
        let pool = pool().await;
        let command = enqueue(&pool, Some(MeteredApprovalPolicy::Auto)).await;

        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::Proceed
        );

        let execution_id = Uuid::new_v4();
        MeteredApproval::record_auto_start(&pool, command.id, execution_id, Some("max-a"))
            .await
            .unwrap();
        MeteredApproval::record_auto_start(&pool, command.id, execution_id, Some("max-a"))
            .await
            .unwrap();

        let latest = MeteredApproval::find_latest_for_command(&pool, command.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.state, MeteredApprovalState::AutoStarted);
        assert_eq!(latest.execution_process_id, Some(execution_id));
        assert_eq!(latest.account_alias.as_deref(), Some("max-a"));
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metered_approvals")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn ask_policy_holds_until_approved_then_authorizes_exactly_one_attempt() {
        let pool = pool().await;
        let command = enqueue(&pool, Some(MeteredApprovalPolicy::Ask)).await;

        // First gate creates the single pending approval; repeats are stable.
        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::AwaitApproval
        );
        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::AwaitApproval
        );
        let pending = MeteredApproval::list_pending(&pool).await.unwrap();
        assert_eq!(pending.len(), 1);

        // Approval flips the gate open.
        assert!(
            MeteredApproval::respond(&pool, pending[0].id, true, None)
                .await
                .unwrap()
        );
        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::Proceed
        );

        // The claimed attempt consumes the approval (allow-once)...
        let execution_id = Uuid::new_v4();
        assert!(
            MeteredApproval::consume_approval(&pool, command.id, execution_id)
                .await
                .unwrap()
        );
        assert!(
            !MeteredApproval::consume_approval(&pool, command.id, Uuid::new_v4())
                .await
                .unwrap()
        );

        // ...so a later attempt must ask again.
        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::AwaitApproval
        );
        assert_eq!(MeteredApproval::list_pending(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ask_policy_denial_blocks_and_keeps_command_checkpoint_intact() {
        let pool = pool().await;
        let command = enqueue(&pool, Some(MeteredApprovalPolicy::Ask)).await;

        MeteredApproval::gate(&pool, &command).await.unwrap();
        let pending = MeteredApproval::list_pending(&pool).await.unwrap();
        assert!(
            MeteredApproval::respond(&pool, pending[0].id, false, Some("operator declined"))
                .await
                .unwrap()
        );

        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::Blocked
        );
        // The command itself is untouched: still pending, never claimed.
        let stored = SessionCommand::find_by_id(&pool, command.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.state,
            crate::models::session_command::SessionCommandState::Pending
        );
        assert_eq!(stored.execution_process_id, None);
    }

    #[tokio::test]
    async fn respond_is_single_winner() {
        let pool = pool().await;
        let command = enqueue(&pool, Some(MeteredApprovalPolicy::Ask)).await;
        MeteredApproval::gate(&pool, &command).await.unwrap();
        let pending = MeteredApproval::list_pending(&pool).await.unwrap();

        assert!(
            MeteredApproval::respond(&pool, pending[0].id, false, None)
                .await
                .unwrap()
        );
        // A late duplicate response cannot flip the decision.
        assert!(
            !MeteredApproval::respond(&pool, pending[0].id, true, None)
                .await
                .unwrap()
        );
        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::Blocked
        );
    }

    #[tokio::test]
    async fn never_policy_blocks_with_durable_routes_exhausted_record() {
        let pool = pool().await;
        let command = enqueue(&pool, Some(MeteredApprovalPolicy::Never)).await;

        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::Blocked
        );
        // Idempotent: repeated dispatch attempts do not spam records.
        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::Blocked
        );

        let latest = MeteredApproval::find_latest_for_command(&pool, command.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.state, MeteredApprovalState::Blocked);
        assert_eq!(latest.reason.as_deref(), Some(ROUTES_EXHAUSTED_REASON));
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM metered_approvals")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn pending_approval_survives_restart_and_resumes_after_approval() {
        // A real file-backed database: the first pool is dropped entirely to
        // simulate a service/machine restart before the approval resolves.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("cdesktop-test.sqlite");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());

        let command_id;
        {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await
                .unwrap();
            apply_schema(&pool).await;
            let (command, _) = SessionCommand::enqueue(
                &pool,
                metered_command(Uuid::new_v4(), Some(MeteredApprovalPolicy::Ask)),
            )
            .await
            .unwrap();
            command_id = command.id;
            assert_eq!(
                MeteredApproval::gate(&pool, &command).await.unwrap(),
                MeteredGateDecision::AwaitApproval
            );
            pool.close().await;
        }

        // "Restart": fresh pool over the same durable file.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let command = SessionCommand::find_by_id(&pool, command_id)
            .await
            .unwrap()
            .unwrap();
        let pending = MeteredApproval::list_pending(&pool).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].session_command_id, command_id);

        // Approval after restart opens the gate; the normal dispatcher
        // claim path (single-winner) then resumes the command exactly once.
        assert!(
            MeteredApproval::respond(&pool, pending[0].id, true, None)
                .await
                .unwrap()
        );
        assert_eq!(
            MeteredApproval::gate(&pool, &command).await.unwrap(),
            MeteredGateDecision::Proceed
        );
        let execution_id = Uuid::new_v4();
        let claimed = SessionCommand::claim_pending(&pool, command.session_id)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert!(
            MeteredApproval::consume_approval(&pool, command_id, execution_id)
                .await
                .unwrap()
        );
    }
}
