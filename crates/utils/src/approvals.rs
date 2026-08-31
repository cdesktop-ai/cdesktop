use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

pub const APPROVAL_TIMEOUT_SECONDS: i64 = 36000; // 10 hours

/// How long an approved decision holds.
///
/// Each harness cdesktop brokers approvals for has some native "and stop
/// asking" form - OpenCode `reply: "always"`, Claude Code a session-destination
/// permission update, Codex `acceptForSession`, an ACP agent's `allow_always`
/// option. `Session` is the one request they can all be asked for, though not
/// all of them offer it on every request; see
/// `docs/design/harness-approvals.md`.
///
/// An adapter with nothing to persist for a given request degrades to
/// [`ApprovalScope::Once`] and never the other way round, so the worst an
/// operator can be surprised by is being asked again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    /// Authorize this request only.
    #[default]
    Once,
    /// Authorize this request and ask the harness to remember the decision for
    /// the rest of its session.
    Session,
}

/// What an operator is being asked to allow, in the harness's own rule syntax.
///
/// The two lists are different widths and both are load-bearing: OpenCode
/// answers a `bash` request with `patterns: ["echo w2-first"]` and
/// `always: ["echo *"]`, so an operator shown only the first would be
/// approving a whole verb while reading one command.
///
/// `session` empty is the single invariant that decides whether
/// [`ApprovalScope::Session`] is on offer at all: an adapter leaves it empty
/// exactly when it has no session-scoped form for this request, and the UI
/// has no other rule to consult.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct ApprovalPatterns {
    /// What this one request covers.
    pub request: Vec<String>,
    /// What approving with [`ApprovalScope::Session`] would allow for the rest
    /// of the run without asking again.
    pub session: Vec<String>,
}

impl ApprovalPatterns {
    /// True when the harness offered no session-scoped form for this request.
    pub fn is_once_only(&self) -> bool {
        self.session.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ApprovalRequest {
    pub id: String,
    pub tool_name: String,
    pub execution_process_id: Uuid,
    pub patterns: ApprovalPatterns,
    pub created_at: DateTime<Utc>,
    pub timeout_at: DateTime<Utc>,
}

impl ApprovalRequest {
    pub fn new(tool_name: String, execution_process_id: Uuid, patterns: ApprovalPatterns) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            tool_name,
            execution_process_id,
            patterns,
            created_at: now,
            timeout_at: now + Duration::seconds(APPROVAL_TIMEOUT_SECONDS),
        }
    }
}

/// Status of a tool permission request (approve/deny for tool execution).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved {
        #[serde(default)]
        scope: ApprovalScope,
    },
    Denied {
        #[ts(optional)]
        reason: Option<String>,
    },
    TimedOut,
}

/// A question–answer pair. `answer` holds one or more selected labels/values.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct QuestionAnswer {
    pub question: String,
    pub answer: Vec<String>,
}

/// Status of a question answer request.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QuestionStatus {
    Answered { answers: Vec<QuestionAnswer> },
    TimedOut,
}

// Tracks both approval and question answers requests
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApprovalOutcome {
    Approved {
        #[serde(default)]
        scope: ApprovalScope,
    },
    Denied {
        #[ts(optional)]
        reason: Option<String>,
    },
    Answered {
        answers: Vec<QuestionAnswer>,
    },
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ApprovalResponse {
    pub execution_process_id: Uuid,
    pub status: ApprovalOutcome,
}
