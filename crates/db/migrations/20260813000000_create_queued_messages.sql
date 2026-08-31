CREATE TABLE queued_messages (
    session_id BLOB PRIMARY KEY NOT NULL,
    data TEXT NOT NULL,
    queued_at TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'starting')),
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
