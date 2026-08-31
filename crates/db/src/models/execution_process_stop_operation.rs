use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

/// The durable, caller-visible outcome of a keyed stop request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopExecutionOutcome {
    Accepted,
    Rejected,
    Interrupted,
}

impl StopExecutionOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(value: &str) -> Result<Self, sqlx::Error> {
        match value {
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(sqlx::Error::Protocol(format!(
                "invalid execution process stop outcome: {value}"
            ))),
        }
    }
}

#[derive(Debug, FromRow)]
struct StopExecutionOperationRow {
    owner_instance_id: Uuid,
    outcome: Option<String>,
}

/// A replay-safe state for one `(execution_process_id, dedupe_key)` pair.
pub enum StopExecutionOperationState {
    Owner,
    Pending { owned_by_current_instance: bool },
    Complete(StopExecutionOutcome),
}

pub struct StopExecutionOperation;

impl StopExecutionOperation {
    /// Creates the durable intent before the stop is invoked. Completed rows
    /// are never overwritten, so every replay observes its original result.
    pub async fn begin(
        pool: &SqlitePool,
        execution_process_id: Uuid,
        dedupe_key: &str,
        instance_id: Uuid,
    ) -> Result<StopExecutionOperationState, sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO execution_process_stop_operations \
             (execution_process_id, dedupe_key, owner_instance_id) VALUES (?, ?, ?) \
             ON CONFLICT(execution_process_id, dedupe_key) DO NOTHING",
        )
        .bind(execution_process_id)
        .bind(dedupe_key)
        .bind(instance_id)
        .execute(pool)
        .await?
        .rows_affected()
            != 0;

        if inserted {
            return Ok(StopExecutionOperationState::Owner);
        }

        Self::state(pool, execution_process_id, dedupe_key, instance_id).await
    }

    pub async fn state(
        pool: &SqlitePool,
        execution_process_id: Uuid,
        dedupe_key: &str,
        instance_id: Uuid,
    ) -> Result<StopExecutionOperationState, sqlx::Error> {
        let row = sqlx::query_as::<_, StopExecutionOperationRow>(
            "SELECT owner_instance_id, outcome FROM execution_process_stop_operations \
             WHERE execution_process_id = ? AND dedupe_key = ?",
        )
        .bind(execution_process_id)
        .bind(dedupe_key)
        .fetch_one(pool)
        .await?;

        match row.outcome {
            Some(outcome) => Ok(StopExecutionOperationState::Complete(
                StopExecutionOutcome::parse(&outcome)?,
            )),
            None => Ok(StopExecutionOperationState::Pending {
                owned_by_current_instance: row.owner_instance_id == instance_id,
            }),
        }
    }

    /// First completion wins. This deliberately preserves a definitive reject
    /// instead of turning a replay into a transport retry.
    pub async fn complete(
        pool: &SqlitePool,
        execution_process_id: Uuid,
        dedupe_key: &str,
        outcome: StopExecutionOutcome,
        instance_id: Uuid,
    ) -> Result<StopExecutionOutcome, sqlx::Error> {
        sqlx::query(
            "UPDATE execution_process_stop_operations \
             SET outcome = ?, completed_at = datetime('now', 'subsec') \
             WHERE execution_process_id = ? AND dedupe_key = ? AND outcome IS NULL",
        )
        .bind(outcome.as_str())
        .bind(execution_process_id)
        .bind(dedupe_key)
        .execute(pool)
        .await?;

        match Self::state(pool, execution_process_id, dedupe_key, instance_id).await? {
            StopExecutionOperationState::Complete(outcome) => Ok(outcome),
            StopExecutionOperationState::Owner | StopExecutionOperationState::Pending { .. } => {
                Err(sqlx::Error::Protocol(
                    "execution process stop operation was not completed".into(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        str::FromStr,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use sqlx::{
        SqlitePool,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };
    use tokio::sync::Barrier;

    use super::*;

    async fn pool(url: &str) -> SqlitePool {
        let options = SqliteConnectOptions::from_str(url)
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE execution_process_stop_operations (\
                execution_process_id BLOB NOT NULL, dedupe_key TEXT NOT NULL, \
                owner_instance_id BLOB NOT NULL, \
                outcome TEXT, created_at TEXT, completed_at TEXT, \
                PRIMARY KEY (execution_process_id, dedupe_key))",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn instance_id() -> Uuid {
        Uuid::new_v4()
    }

    #[tokio::test]
    async fn accepted_stop_replays_after_a_lost_response_without_another_stop() {
        let pool = pool("sqlite::memory:").await;
        let process_id = Uuid::new_v4();
        let instance_id = instance_id();
        let mut stop_calls = 0;

        assert!(matches!(
            StopExecutionOperation::begin(&pool, process_id, "stall-1", instance_id)
                .await
                .unwrap(),
            StopExecutionOperationState::Owner
        ));
        stop_calls += 1; // the first response is deliberately "lost"
        StopExecutionOperation::complete(
            &pool,
            process_id,
            "stall-1",
            StopExecutionOutcome::Accepted,
            instance_id,
        )
        .await
        .unwrap();

        assert!(matches!(
            StopExecutionOperation::begin(&pool, process_id, "stall-1", instance_id)
                .await
                .unwrap(),
            StopExecutionOperationState::Complete(StopExecutionOutcome::Accepted)
        ));
        assert_eq!(stop_calls, 1);
    }

    #[tokio::test]
    async fn completed_stop_replays_after_server_restart() {
        let path = std::env::temp_dir().join(format!("cdesktop-stop-{}.sqlite", Uuid::new_v4()));
        let url = format!("sqlite://{}", path.display());
        let process_id = Uuid::new_v4();
        let first_instance_id = instance_id();
        let first_pool = pool(&url).await;
        StopExecutionOperation::begin(&first_pool, process_id, "restart", first_instance_id)
            .await
            .unwrap();
        StopExecutionOperation::complete(
            &first_pool,
            process_id,
            "restart",
            StopExecutionOutcome::Accepted,
            first_instance_id,
        )
        .await
        .unwrap();
        first_pool.close().await;

        let second_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str(&url).unwrap())
            .await
            .unwrap();
        assert!(matches!(
            StopExecutionOperation::begin(&second_pool, process_id, "restart", instance_id())
                .await
                .unwrap(),
            StopExecutionOperationState::Complete(StopExecutionOutcome::Accepted)
        ));
        second_pool.close().await;
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn rejected_stop_replays_as_the_same_definitive_outcome() {
        let pool = pool("sqlite::memory:").await;
        let process_id = Uuid::new_v4();
        let instance_id = instance_id();
        StopExecutionOperation::begin(&pool, process_id, "rejected", instance_id)
            .await
            .unwrap();
        StopExecutionOperation::complete(
            &pool,
            process_id,
            "rejected",
            StopExecutionOutcome::Rejected,
            instance_id,
        )
        .await
        .unwrap();

        assert!(matches!(
            StopExecutionOperation::begin(&pool, process_id, "rejected", instance_id)
                .await
                .unwrap(),
            StopExecutionOperationState::Complete(StopExecutionOutcome::Rejected)
        ));
    }

    #[tokio::test]
    async fn interrupted_stop_replays_as_a_distinct_durable_outcome() {
        let pool = pool("sqlite::memory:").await;
        let process_id = Uuid::new_v4();
        let instance_id = instance_id();
        StopExecutionOperation::begin(&pool, process_id, "interrupted", instance_id)
            .await
            .unwrap();
        StopExecutionOperation::complete(
            &pool,
            process_id,
            "interrupted",
            StopExecutionOutcome::Interrupted,
            instance_id,
        )
        .await
        .unwrap();

        assert!(matches!(
            StopExecutionOperation::begin(&pool, process_id, "interrupted", instance_id)
                .await
                .unwrap(),
            StopExecutionOperationState::Complete(StopExecutionOutcome::Interrupted)
        ));
    }

    #[tokio::test]
    async fn dedupe_key_is_scoped_to_an_execution_process() {
        let pool = pool("sqlite::memory:").await;
        let key = "same-key";
        let instance_id = instance_id();
        assert!(matches!(
            StopExecutionOperation::begin(&pool, Uuid::new_v4(), key, instance_id)
                .await
                .unwrap(),
            StopExecutionOperationState::Owner
        ));
        assert!(matches!(
            StopExecutionOperation::begin(&pool, Uuid::new_v4(), key, instance_id)
                .await
                .unwrap(),
            StopExecutionOperationState::Owner
        ));
    }

    #[tokio::test]
    async fn concurrent_pending_follower_replays_the_owner_outcome_without_stopping() {
        let pool = Arc::new(pool("sqlite::memory:").await);
        let process_id = Uuid::new_v4();
        let instance_id = instance_id();
        let owner_started = Arc::new(Barrier::new(2));
        let before_stop = Arc::new(Barrier::new(2));
        let stop_calls = Arc::new(AtomicUsize::new(0));

        let owner = {
            let pool = pool.clone();
            let owner_started = owner_started.clone();
            let before_stop = before_stop.clone();
            let stop_calls = stop_calls.clone();
            tokio::spawn(async move {
                assert!(matches!(
                    StopExecutionOperation::begin(&pool, process_id, "concurrent", instance_id)
                        .await
                        .unwrap(),
                    StopExecutionOperationState::Owner
                ));
                owner_started.wait().await;
                before_stop.wait().await;
                tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
                stop_calls.fetch_add(1, Ordering::SeqCst);
                StopExecutionOperation::complete(
                    &pool,
                    process_id,
                    "concurrent",
                    StopExecutionOutcome::Accepted,
                    instance_id,
                )
                .await
                .unwrap()
            })
        };
        let follower = {
            let pool = pool.clone();
            let owner_started = owner_started.clone();
            let before_stop = before_stop.clone();
            tokio::spawn(async move {
                owner_started.wait().await;
                assert!(matches!(
                    StopExecutionOperation::begin(&pool, process_id, "concurrent", instance_id)
                        .await
                        .unwrap(),
                    StopExecutionOperationState::Pending {
                        owned_by_current_instance: true
                    }
                ));
                before_stop.wait().await;
                StopExecutionOperation::state(&pool, process_id, "concurrent", instance_id)
                    .await
                    .unwrap()
            })
        };

        assert!(matches!(
            follower.await.unwrap(),
            StopExecutionOperationState::Pending {
                owned_by_current_instance: true
            }
        ));
        assert_eq!(owner.await.unwrap(), StopExecutionOutcome::Accepted);
        assert!(matches!(
            StopExecutionOperation::begin(&pool, process_id, "concurrent", instance_id)
                .await
                .unwrap(),
            StopExecutionOperationState::Complete(StopExecutionOutcome::Accepted)
        ));
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn owner_crash_before_stop_side_effect_replays_interrupted_without_takeover() {
        let pool = pool("sqlite::memory:").await;
        let process_id = Uuid::new_v4();
        let owner = instance_id();
        let restarted_server = instance_id();

        assert!(matches!(
            StopExecutionOperation::begin(&pool, process_id, "crash-boundary", owner)
                .await
                .unwrap(),
            StopExecutionOperationState::Owner
        ));
        // The owner is paused after durable intent and lost before it can
        // cancel or kill. Restart reconciliation must not run that side
        // effect a second time or report acceptance.
        assert!(matches!(
            StopExecutionOperation::begin(&pool, process_id, "crash-boundary", restarted_server)
                .await
                .unwrap(),
            StopExecutionOperationState::Pending {
                owned_by_current_instance: false
            }
        ));
        assert_eq!(
            StopExecutionOperation::complete(
                &pool,
                process_id,
                "crash-boundary",
                StopExecutionOutcome::Interrupted,
                restarted_server,
            )
            .await
            .unwrap(),
            StopExecutionOutcome::Interrupted
        );
        assert!(matches!(
            StopExecutionOperation::begin(&pool, process_id, "crash-boundary", restarted_server)
                .await
                .unwrap(),
            StopExecutionOperationState::Complete(StopExecutionOutcome::Interrupted)
        ));
    }
}
