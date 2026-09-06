# Native execution efficiency: completion checkpoint

## Now

- Base verified: `cf555c5b23b52bbd2f8eb6f10b30af72c68be58f`.
- Ordinary Codex follow-ups now use native `thread/resume`, preserving the
  recorded thread id and effective launch settings without supplying history
  or a rollout path.
- `/compact` resumes the same thread before native compaction instead of
  creating a copied fork. Reviews remain the intentional fork path.
- Follow-up user input contains only the new task; configured static guidance
  is passed separately as native thread instructions.
- `/fast` no longer creates an unused fork after changing its existing setting.

## Review correction

- `append_prompt` becomes native `developer_instructions` on both start and
  resume, joined after any configured developer guidance. This keeps it out of
  repeated user-turn input without replacing Codex's default base instructions.
- Protocol-path fixtures cover repeated resumes, explicit fork isolation,
  cancellation/restart, turn/usage evidence, absent explicit base guidance,
  and changed/cleared append guidance.

## Verification

- Passed: `cargo test -p executors continuation_tests --lib` under
  `evidence-test-isolation.sb` with a temporary `TMPDIR` (2 passed).

## Completed proof

- Repeated continuation fixtures emit `thread/resume`, then `turn/start`, for
  the same recorded thread and emit no fork request.
- The explicit review path fixture retains `thread/fork`.
- Cancellation returns an unknown `turn/start` outcome for command
  reconciliation rather than retrying it blindly; the next continuation is a
  fresh `thread/resume` + `turn/start` pair.
- Usage updates retain `thread_id` and `turn_id`, and a second notification for
  the same turn replaces its normalized entry instead of replaying old usage.

## Constraints retained

- No fallback from resume to fork/start.
- `thread_resume` performs the same start-admission check as a new thread;
  fork-specific reservation remains limited to actual forks.
- No DB, migration, lockfile, provider, or SightMesh wake files were changed.
