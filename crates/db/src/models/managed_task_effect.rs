use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct ManagedTaskEffect {
    pub task_id: Uuid,
    pub epoch: i64,
    pub request_hash: String,
    pub kind: String,
    pub state: String,
    pub workspace_id: Uuid,
    pub session_id: Uuid,
    pub owner_instance_id: Uuid,
    pub effect_created: bool,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct NewManagedTaskEffect<'a> {
    pub task_id: Uuid,
    pub epoch: i64,
    pub request_hash: &'a str,
    pub kind: &'a str,
    pub workspace_id: Uuid,
    pub session_id: Uuid,
    pub owner_instance_id: Uuid,
}

#[derive(Debug, Error)]
pub enum ManagedTaskEffectError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("The task epoch already belongs to different launch parameters")]
    Conflict,
    #[error("The task epoch is stale")]
    StaleEpoch,
}

impl ManagedTaskEffect {
    pub async fn begin(
        pool: &SqlitePool,
        effect: NewManagedTaskEffect<'_>,
    ) -> Result<(Self, bool), ManagedTaskEffectError> {
        let inserted = sqlx::query(
            "INSERT INTO managed_task_effects
             (task_id, epoch, request_hash, kind, workspace_id, session_id, owner_instance_id)
             SELECT ?, ?, ?, ?, ?, ?, ?
             WHERE NOT EXISTS (
                 SELECT 1 FROM managed_task_effects WHERE task_id = ? AND epoch > ?
             )
             ON CONFLICT(task_id, epoch) DO NOTHING",
        )
        .bind(effect.task_id)
        .bind(effect.epoch)
        .bind(effect.request_hash)
        .bind(effect.kind)
        .bind(effect.workspace_id)
        .bind(effect.session_id)
        .bind(effect.owner_instance_id)
        .bind(effect.task_id)
        .bind(effect.epoch)
        .execute(pool)
        .await?
        .rows_affected()
            == 1;

        if let Some(row) = Self::find(pool, effect.task_id, effect.epoch).await? {
            if row.request_hash != effect.request_hash || row.kind != effect.kind {
                return Err(ManagedTaskEffectError::Conflict);
            }
            return Ok((row, inserted));
        }

        let newer: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM managed_task_effects WHERE task_id = ? AND epoch > ?)",
        )
        .bind(effect.task_id)
        .bind(effect.epoch)
        .fetch_one(pool)
        .await?;
        if newer != 0 {
            return Err(ManagedTaskEffectError::StaleEpoch);
        }
        Err(ManagedTaskEffectError::Conflict)
    }

    pub async fn find(
        pool: &SqlitePool,
        task_id: Uuid,
        epoch: i64,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM managed_task_effects WHERE task_id = ? AND epoch = ?")
            .bind(task_id)
            .bind(epoch)
            .fetch_optional(pool)
            .await
    }

    pub async fn finish(
        pool: &SqlitePool,
        task_id: Uuid,
        epoch: i64,
        state: &str,
        effect_created: bool,
        reason: Option<&str>,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query(
            "UPDATE managed_task_effects
             SET state = ?, effect_created = ?, reason = ?,
                 updated_at = datetime('now', 'subsec')
             WHERE task_id = ? AND epoch = ? AND state = 'pending'",
        )
        .bind(state)
        .bind(effect_created)
        .bind(reason)
        .bind(task_id)
        .bind(epoch)
        .execute(pool)
        .await?;
        Self::find(pool, task_id, epoch)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }
}

#[cfg(test)]
mod tests {
    use futures::future::join_all;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    fn effect<'a>(task_id: Uuid, epoch: i64, request_hash: &'a str) -> NewManagedTaskEffect<'a> {
        NewManagedTaskEffect {
            task_id,
            epoch,
            request_hash,
            kind: "session",
            workspace_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            owner_instance_id: Uuid::new_v4(),
        }
    }

    #[tokio::test]
    async fn duplicate_epoch_returns_the_original_reserved_effect() {
        let pool = pool().await;
        let task_id = Uuid::new_v4();
        let (original, inserted) = ManagedTaskEffect::begin(&pool, effect(task_id, 1, "same"))
            .await
            .unwrap();
        assert!(inserted);

        let (replayed, inserted) = ManagedTaskEffect::begin(&pool, effect(task_id, 1, "same"))
            .await
            .unwrap();
        assert!(!inserted);
        assert_eq!(replayed.workspace_id, original.workspace_id);
        assert_eq!(replayed.session_id, original.session_id);
    }

    #[tokio::test]
    async fn concurrent_duplicate_wakeups_reserve_one_native_effect() {
        let pool = pool().await;
        let task_id = Uuid::new_v4();
        let outcomes =
            join_all((0..8).map(|_| ManagedTaskEffect::begin(&pool, effect(task_id, 1, "same"))))
                .await;
        let rows = outcomes.into_iter().map(Result::unwrap).collect::<Vec<_>>();

        assert_eq!(rows.iter().filter(|(_, inserted)| *inserted).count(), 1);
        assert!(
            rows.iter()
                .all(|(row, _)| row.session_id == rows[0].0.session_id)
        );
    }

    #[tokio::test]
    async fn epoch_cannot_be_reused_for_different_parameters() {
        let pool = pool().await;
        let task_id = Uuid::new_v4();
        ManagedTaskEffect::begin(&pool, effect(task_id, 1, "first"))
            .await
            .unwrap();

        let error = ManagedTaskEffect::begin(&pool, effect(task_id, 1, "second"))
            .await
            .unwrap_err();
        assert!(matches!(error, ManagedTaskEffectError::Conflict));
    }

    #[tokio::test]
    async fn older_epoch_is_rejected_after_a_newer_one_exists() {
        let pool = pool().await;
        let task_id = Uuid::new_v4();
        ManagedTaskEffect::begin(&pool, effect(task_id, 2, "newer"))
            .await
            .unwrap();

        let error = ManagedTaskEffect::begin(&pool, effect(task_id, 1, "older"))
            .await
            .unwrap_err();
        assert!(matches!(error, ManagedTaskEffectError::StaleEpoch));
    }
}
