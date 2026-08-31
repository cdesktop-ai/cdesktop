# OpenCode failure fixtures

Every file here is verbatim output captured from a live `opencode serve`
(CLI 1.15.10, `opencode server listening on ...`), not a hand-written shape.

| File | How it was produced |
| --- | --- |
| `session_error_api_401.json` | `anthropic/claude-sonnet-4-5-20250929` against a real `api.anthropic.com` with a deliberately invalid `ANTHROPIC_API_KEY`. |
| `session_error_aborted.json` | `POST /session/{id}/abort` while a turn on `opencode/x-preview-f-free` was streaming. |
| `session_error_unknown.json` | A provider/model pair the server cannot resolve. OpenCode reports this as `UnknownError`, so it stays unclassified by design. |
| `session_status_retry_rate_limit.json` | Upstream returning HTTP 429; captured from the `/event` stream. |
| `session_status_retry_overloaded.json` | Upstream returning HTTP 503; captured from the `/event` stream. |
| `permission_asked_bash_echo.json` | A `bash` tool call under `OPENCODE_PERMISSION` `{"bash":"ask"}`; captured from the `/event` stream. |
| `permission_asked_bash_ls.json` | The same session after `reply: "always"` was given for the `echo` ask - `ls -la` still asks, with its own `always` rule. |
| `permission_replied_always.json` | `POST /permission/{id}/reply {"reply":"always"}`, which returned `true`. |
| `permission_replied_once.json` | The same call with `{"reply":"once"}`, for contrast. |

Note on the two retry fixtures: OpenCode retries a *retryable* provider error
internally with capped exponential backoff and does not emit a terminal
`session.error` for it. A rate-limited turn was observed still retrying at
attempt 17 roughly eleven minutes in. Rate limiting therefore surfaces as a
long-running turn, never as a classifiable terminal outcome - which is why
`normalized_session_failure` has no rate-limit branch and the retry status is
surfaced into the log stream instead.

The two `permission_asked` fixtures are the evidence for `always` being
session-scoped and pattern-bounded: after `always` on `echo w2-first` (rule
`echo *`), a later `echo` in that session ran with no new ask, while `ls -la`
raised the second fixture. Recorded alongside it, because it is not what the
verb names suggest: replying `once` to the `echo` ask also let a later,
different `echo` through on 1.15.10.
