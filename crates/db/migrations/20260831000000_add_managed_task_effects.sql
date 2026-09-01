-- This journal answers which native workspace/session effect was authorized
-- for one SightMesh task epoch. That historical fact cannot be reconstructed
-- after a response loss, server restart, or later workspace deletion.
CREATE TABLE managed_task_effects (
    task_id BLOB NOT NULL,
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    request_hash TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('workspace', 'session')),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'active', 'refused', 'lost')),
    workspace_id BLOB NOT NULL,
    session_id BLOB NOT NULL,
    owner_instance_id BLOB NOT NULL,
    effect_created INTEGER NOT NULL DEFAULT 0,
    reason TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    PRIMARY KEY (task_id, epoch)
);

CREATE INDEX idx_managed_task_effects_latest
    ON managed_task_effects (task_id, epoch DESC);
