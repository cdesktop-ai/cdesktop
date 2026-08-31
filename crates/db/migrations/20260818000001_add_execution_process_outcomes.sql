-- Normalized terminal outcome per execution attempt (plan §9/§14).
-- One row per process, written exactly once by the attempt-completion winner.
-- Kept in a side table so the hot execution_processes queries stay unchanged.
CREATE TABLE execution_process_outcomes (
    execution_process_id BLOB PRIMARY KEY NOT NULL
        REFERENCES execution_processes (id) ON DELETE CASCADE,
    outcome TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
