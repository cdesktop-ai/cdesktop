-- Durable metered-fallback approval per logical session command (plan §12).
-- Rows survive service and machine restart; the dispatcher gate reads the
-- latest row for a command to decide launch/wait/block, and stamps
-- execution_process_id when an approval is consumed by a claimed attempt
-- (allow-once). Only safe metadata is stored — never credential material.
CREATE TABLE metered_approvals (
    id BLOB PRIMARY KEY NOT NULL,
    session_command_id BLOB NOT NULL
        REFERENCES session_commands (id) ON DELETE CASCADE,
    policy TEXT NOT NULL, -- auto | ask | never
    state TEXT NOT NULL DEFAULT 'pending', -- pending | approved | denied | auto_started | blocked
    account_alias TEXT,
    reason TEXT,
    execution_process_id BLOB,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    resolved_at TEXT
);

-- At most one open question per logical command.
CREATE UNIQUE INDEX metered_approvals_one_pending
    ON metered_approvals (session_command_id)
    WHERE state = 'pending';

CREATE INDEX metered_approvals_by_command
    ON metered_approvals (session_command_id);
