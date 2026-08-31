ALTER TABLE session_commands
ADD COLUMN attempt_number INTEGER NOT NULL DEFAULT 0;
