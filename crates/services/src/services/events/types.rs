use anyhow::Error as AnyhowError;
use db::models::{
    execution_process::ExecutionProcess, routine::Routine, routine_run::RoutineRun,
    scratch::Scratch, session::Session, workspace::Workspace,
};
use serde::{Deserialize, Serialize};
use sqlx::Error as SqlxError;
use strum_macros::{Display, EnumString};
use thiserror::Error;
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum EventError {
    #[error(transparent)]
    Sqlx(#[from] SqlxError),
    #[error(transparent)]
    Parse(#[from] serde_json::Error),
    #[error(transparent)]
    Other(#[from] AnyhowError), // Catches any unclassified errors
}

#[derive(EnumString, Display)]
pub enum HookTables {
    #[strum(to_string = "workspaces")]
    Workspaces,
    #[strum(to_string = "execution_processes")]
    ExecutionProcesses,
    #[strum(to_string = "scratch")]
    Scratch,
    #[strum(to_string = "sessions")]
    Sessions,
    #[strum(to_string = "routines")]
    Routines,
    #[strum(to_string = "routine_runs")]
    RoutineRuns,
}

#[derive(Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecordTypes {
    Workspace(Workspace),
    ExecutionProcess(ExecutionProcess),
    Scratch(Box<Scratch>),
    Session(Session),
    Routine(Routine),
    RoutineRun(RoutineRun),
    DeletedWorkspace {
        rowid: i64,
    },
    DeletedExecutionProcess {
        rowid: i64,
        session_id: Option<Uuid>,
        process_id: Option<Uuid>,
    },
    DeletedScratch {
        rowid: i64,
        scratch_id: Option<Uuid>,
        scratch_type: Option<String>,
    },
    DeletedSession {
        rowid: i64,
        session_id: Option<Uuid>,
        workspace_id: Option<Uuid>,
    },
    DeletedRoutine {
        rowid: i64,
        routine_id: Option<Uuid>,
    },
    DeletedRoutineRun {
        rowid: i64,
        routine_run_id: Option<Uuid>,
        routine_id: Option<Uuid>,
    },
}

#[derive(Serialize, Deserialize, TS)]
pub struct EventPatchInner {
    pub(crate) db_op: String,
    pub(crate) record: RecordTypes,
}

#[derive(Serialize, Deserialize, TS)]
pub struct EventPatch {
    pub(crate) op: String,
    pub(crate) path: String,
    pub(crate) value: EventPatchInner,
}
