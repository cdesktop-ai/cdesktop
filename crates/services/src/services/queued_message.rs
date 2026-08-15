use chrono::{DateTime, Utc};
use db::models::{scratch::DraftFollowUpData, session_command::SessionCommand};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct QueuedMessage {
    pub session_id: Uuid,
    pub data: DraftFollowUpData,
    pub queued_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QueueStatus {
    Empty,
    Queued { message: QueuedMessage },
}

impl QueueStatus {
    pub fn from_command(command: SessionCommand) -> Self {
        Self::Queued {
            message: QueuedMessage {
                session_id: command.session_id,
                data: DraftFollowUpData {
                    message: command.body,
                    executor_config: command.config.0.executor_config,
                },
                queued_at: command.created_at,
            },
        }
    }
}
