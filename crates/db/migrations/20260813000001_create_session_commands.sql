ALTER TABLE sessions ADD COLUMN parent_session_id BLOB
REFERENCES sessions(id) ON DELETE SET NULL;

CREATE INDEX idx_sessions_parent ON sessions(parent_session_id);

CREATE TABLE session_commands (
    id BLOB PRIMARY KEY NOT NULL,
    session_id BLOB NOT NULL,
    dedupe_key TEXT,
    intent TEXT NOT NULL CHECK (intent IN ('continue', 'replace')),
    body TEXT NOT NULL,
    config TEXT,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'claimed', 'done', 'failed', 'cancelled')),
    execution_process_id BLOB,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    finished_at TEXT,
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (execution_process_id) REFERENCES execution_processes(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX idx_session_commands_dedupe
ON session_commands(session_id, dedupe_key)
WHERE dedupe_key IS NOT NULL;

CREATE INDEX idx_session_commands_pending
ON session_commands(session_id, state);

UPDATE execution_processes
SET status = 'failed',
    completed_at = COALESCE(completed_at, datetime('now', 'subsec')),
    updated_at = datetime('now', 'subsec')
WHERE status = 'running'
  AND run_reason = 'codingagent'
  AND EXISTS (
      SELECT 1
      FROM execution_processes AS newer
      WHERE newer.session_id = execution_processes.session_id
        AND newer.status = 'running'
        AND newer.run_reason = 'codingagent'
        AND (
            newer.started_at > execution_processes.started_at
            OR (
                newer.started_at = execution_processes.started_at
                AND newer.rowid > execution_processes.rowid
            )
        )
  );

CREATE UNIQUE INDEX idx_one_running_coding_agent_per_session
ON execution_processes(session_id)
WHERE status = 'running' AND run_reason = 'codingagent';

INSERT INTO session_commands (id, session_id, intent, body, config, created_at)
SELECT randomblob(16),
       session_id,
       'continue',
       json_extract(data, '$.message'),
       json_object(
           'executor_config', json_extract(data, '$.executor_config'),
           'selected_provider_id', NULL
       ),
       queued_at
FROM queued_messages;

DROP TABLE queued_messages;
