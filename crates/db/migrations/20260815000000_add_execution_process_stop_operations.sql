-- A keyed stop is retained for the lifetime of its execution process. This
-- makes a lost HTTP response replayable across server restarts; deleting an
-- execution process is the explicit retention boundary.
CREATE TABLE execution_process_stop_operations (
    execution_process_id BLOB NOT NULL,
    dedupe_key TEXT NOT NULL,
    owner_instance_id BLOB NOT NULL,
    outcome TEXT CHECK (outcome IN ('accepted', 'rejected', 'interrupted')),
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    completed_at TEXT,
    PRIMARY KEY (execution_process_id, dedupe_key),
    FOREIGN KEY (execution_process_id) REFERENCES execution_processes(id) ON DELETE CASCADE
);
