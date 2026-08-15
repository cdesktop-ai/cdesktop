use chrono::{DateTime, Utc};
use executors::profile::ExecutorConfig;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool, Type, types::Json};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Type, Serialize, Deserialize, PartialEq, TS)]
#[sqlx(type_name = "session_command_intent", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[ts(use_ts_enum)]
pub enum SessionCommandIntent {
    Continue,
    Replace,
}

#[derive(Debug, Clone, Type, Serialize, Deserialize, PartialEq, TS)]
#[sqlx(type_name = "session_command_state", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[ts(use_ts_enum)]
pub enum SessionCommandState {
    Pending,
    Claimed,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct SessionCommandConfig {
    pub executor_config: ExecutorConfig,
    #[serde(default)]
    #[ts(optional)]
    pub selected_provider_id: Option<Uuid>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, TS)]
pub struct SessionCommand {
    pub id: Uuid,
    pub session_id: Uuid,
    pub dedupe_key: Option<String>,
    pub intent: SessionCommandIntent,
    pub body: String,
    #[ts(type = "SessionCommandConfig")]
    pub config: Json<SessionCommandConfig>,
    pub state: SessionCommandState,
    pub execution_process_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

pub struct NewSessionCommand {
    pub session_id: Uuid,
    pub dedupe_key: Option<String>,
    pub intent: SessionCommandIntent,
    pub body: String,
    pub config: SessionCommandConfig,
}

impl SessionCommand {
    pub async fn find_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM session_commands WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    pub async fn enqueue(
        pool: &SqlitePool,
        command: NewSessionCommand,
    ) -> Result<(Self, bool), sqlx::Error> {
        let id = Uuid::new_v4();
        let inserted = sqlx::query_as::<_, Self>(
            r#"INSERT INTO session_commands (
                   id, session_id, dedupe_key, intent, body, config
               ) VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(session_id, dedupe_key) WHERE dedupe_key IS NOT NULL DO NOTHING
               RETURNING *"#,
        )
        .bind(id)
        .bind(command.session_id)
        .bind(command.dedupe_key.as_deref())
        .bind(command.intent)
        .bind(command.body)
        .bind(Json(command.config))
        .fetch_optional(pool)
        .await?;

        if let Some(inserted) = inserted {
            return Ok((inserted, true));
        }

        let existing = sqlx::query_as::<_, Self>(
            "SELECT * FROM session_commands WHERE session_id = ? AND dedupe_key = ?",
        )
        .bind(command.session_id)
        .bind(command.dedupe_key)
        .fetch_one(pool)
        .await?;
        Ok((existing, false))
    }

    pub async fn pending(pool: &SqlitePool, session_id: Uuid) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            "SELECT * FROM session_commands \
             WHERE session_id = ? AND state = 'pending' \
             ORDER BY rowid",
        )
        .bind(session_id)
        .fetch_all(pool)
        .await
    }

    pub async fn has_pending(pool: &SqlitePool, session_id: Uuid) -> Result<bool, sqlx::Error> {
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM session_commands WHERE session_id = ? AND state = 'pending')",
        )
        .bind(session_id)
        .fetch_one(pool)
        .await?;
        Ok(exists != 0)
    }

    pub async fn claim_pending(
        pool: &SqlitePool,
        session_id: Uuid,
        execution_process_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        let mut transaction = pool.begin().await?;
        let pending = sqlx::query_as::<_, Self>(
            r#"SELECT * FROM session_commands
               WHERE session_id = ? AND state = 'pending'
               ORDER BY rowid"#,
        )
        .bind(session_id)
        .fetch_all(&mut *transaction)
        .await?;
        let Some(first) = pending.first() else {
            transaction.commit().await?;
            return Ok(Vec::new());
        };
        let ids: Vec<_> = pending
            .iter()
            .take_while(|command| command.config == first.config)
            .map(|command| command.id)
            .collect();
        let mut update = QueryBuilder::<Sqlite>::new(
            "UPDATE session_commands SET state = 'claimed', execution_process_id = ",
        );
        update
            .push_bind(execution_process_id)
            .push(" WHERE id IN (");
        let mut separated = update.separated(", ");
        for id in &ids {
            separated.push_bind(id);
        }
        separated.push_unseparated(") AND state = 'pending'");
        update.build().execute(&mut *transaction).await?;
        let claimed = sqlx::query_as::<_, Self>(
            r#"SELECT * FROM session_commands
               WHERE execution_process_id = ? AND state = 'claimed'
               ORDER BY rowid"#,
        )
        .bind(execution_process_id)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(claimed)
    }

    pub async fn pending_session_ids(pool: &SqlitePool) -> Result<Vec<Uuid>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT session_id FROM session_commands WHERE state = 'pending' \
             GROUP BY session_id ORDER BY MIN(rowid)",
        )
        .fetch_all(pool)
        .await
    }

    pub async fn release_execution(
        pool: &SqlitePool,
        execution_process_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE session_commands SET state = 'pending', execution_process_id = NULL \
             WHERE execution_process_id = ? AND state = 'claimed'",
        )
        .bind(execution_process_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn ensure_claimed(
        pool: &SqlitePool,
        session_id: Uuid,
        execution_process_id: Uuid,
        body: String,
        config: SessionCommandConfig,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"INSERT INTO session_commands (
                   id, session_id, intent, body, config, state, execution_process_id
               )
               SELECT ?, ?, 'continue', ?, ?, 'claimed', ?
               WHERE NOT EXISTS (
                   SELECT 1 FROM session_commands WHERE execution_process_id = ?
               )"#,
        )
        .bind(Uuid::new_v4())
        .bind(session_id)
        .bind(body)
        .bind(Json(config))
        .bind(execution_process_id)
        .bind(execution_process_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn finish_execution(
        pool: &SqlitePool,
        execution_process_id: Uuid,
        succeeded: bool,
    ) -> Result<(), sqlx::Error> {
        let state = if succeeded { "done" } else { "failed" };
        sqlx::query(
            "UPDATE session_commands SET state = ?, finished_at = datetime('now', 'subsec') \
             WHERE execution_process_id = ? AND state = 'claimed'",
        )
        .bind(state)
        .bind(execution_process_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn cancel_pending(pool: &SqlitePool, session_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE session_commands SET state = 'cancelled', finished_at = datetime('now', 'subsec') \
             WHERE session_id = ? AND state = 'pending'",
        )
        .bind(session_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn cancel_pending_except(
        pool: &SqlitePool,
        session_id: Uuid,
        command_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE session_commands SET state = 'cancelled', finished_at = datetime('now', 'subsec') \
             WHERE session_id = ? AND id != ? AND state = 'pending'",
        )
        .bind(session_id)
        .bind(command_id)
        .execute(pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"CREATE TABLE session_commands (
                id BLOB PRIMARY KEY NOT NULL,
                session_id BLOB NOT NULL,
                dedupe_key TEXT,
                intent TEXT NOT NULL,
                body TEXT NOT NULL,
                config TEXT,
                state TEXT NOT NULL DEFAULT 'pending',
                execution_process_id BLOB,
                created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
                finished_at TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE UNIQUE INDEX dedupe ON session_commands(session_id, dedupe_key) \
             WHERE dedupe_key IS NOT NULL",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn command(session_id: Uuid, body: &str, dedupe_key: Option<&str>) -> NewSessionCommand {
        NewSessionCommand {
            session_id,
            dedupe_key: dedupe_key.map(str::to_owned),
            intent: SessionCommandIntent::Continue,
            body: body.to_owned(),
            config: SessionCommandConfig {
                executor_config: ExecutorConfig::new(
                    executors::executors::BaseCodingAgent::ClaudeCode,
                ),
                selected_provider_id: None,
            },
        }
    }

    #[tokio::test]
    async fn enqueue_is_append_only_and_idempotent_when_keyed() {
        let pool = pool().await;
        let session_id = Uuid::new_v4();
        let (first, first_inserted) =
            SessionCommand::enqueue(&pool, command(session_id, "first", Some("a")))
                .await
                .unwrap();
        let (duplicate, duplicate_inserted) =
            SessionCommand::enqueue(&pool, command(session_id, "ignored duplicate", Some("a")))
                .await
                .unwrap();
        SessionCommand::enqueue(&pool, command(session_id, "second", None))
            .await
            .unwrap();

        assert_eq!(duplicate.id, first.id);
        assert!(first_inserted);
        assert!(!duplicate_inserted);
        assert_eq!(
            SessionCommand::pending(&pool, session_id)
                .await
                .unwrap()
                .len(),
            2
        );

        let other_session = Uuid::new_v4();
        let (other, inserted) =
            SessionCommand::enqueue(&pool, command(other_session, "other", Some("a")))
                .await
                .unwrap();
        assert_ne!(other.id, first.id);
        assert!(inserted);
    }

    #[tokio::test]
    async fn claim_batches_pending_commands_in_order() {
        let pool = pool().await;
        let session_id = Uuid::new_v4();
        SessionCommand::enqueue(&pool, command(session_id, "first", None))
            .await
            .unwrap();
        SessionCommand::enqueue(&pool, command(session_id, "second", None))
            .await
            .unwrap();

        let execution_id = Uuid::new_v4();
        let claimed = SessionCommand::claim_pending(&pool, session_id, execution_id)
            .await
            .unwrap();

        assert_eq!(
            claimed
                .iter()
                .map(|item| item.body.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert!(
            claimed
                .iter()
                .all(|item| item.execution_process_id == Some(execution_id))
        );
        assert!(
            SessionCommand::pending(&pool, session_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn claim_stops_before_a_config_change() {
        let pool = pool().await;
        let session_id = Uuid::new_v4();
        SessionCommand::enqueue(&pool, command(session_id, "claude", None))
            .await
            .unwrap();
        let mut codex = command(session_id, "codex", None);
        codex.config.executor_config =
            ExecutorConfig::new(executors::executors::BaseCodingAgent::Codex);
        SessionCommand::enqueue(&pool, codex).await.unwrap();

        let claimed = SessionCommand::claim_pending(&pool, session_id, Uuid::new_v4())
            .await
            .unwrap();

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].body, "claude");
        assert_eq!(
            SessionCommand::pending(&pool, session_id).await.unwrap()[0].body,
            "codex"
        );
    }

    #[tokio::test]
    async fn interrupted_execution_returns_to_pending() {
        let pool = pool().await;
        let session_id = Uuid::new_v4();
        SessionCommand::enqueue(&pool, command(session_id, "recover", None))
            .await
            .unwrap();
        let execution_id = Uuid::new_v4();
        SessionCommand::claim_pending(&pool, session_id, execution_id)
            .await
            .unwrap();

        SessionCommand::release_execution(&pool, execution_id)
            .await
            .unwrap();

        let pending = SessionCommand::pending(&pool, session_id).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].body, "recover");
        assert_eq!(pending[0].execution_process_id, None);
    }

    #[tokio::test]
    async fn replace_cancels_only_older_pending_commands() {
        let pool = pool().await;
        let session_id = Uuid::new_v4();
        SessionCommand::enqueue(&pool, command(session_id, "older", None))
            .await
            .unwrap();
        let mut replacement = command(session_id, "replace", Some("replacement"));
        replacement.intent = SessionCommandIntent::Replace;
        let (replacement, _) = SessionCommand::enqueue(&pool, replacement).await.unwrap();

        SessionCommand::cancel_pending_except(&pool, session_id, replacement.id)
            .await
            .unwrap();

        let pending = SessionCommand::pending(&pool, session_id).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, replacement.id);
    }
}
