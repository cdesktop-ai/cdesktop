# Claude Code control-request fixtures

Verbatim frames captured from a live `claude` CLI (2.1.232) driven over
`--input-format=stream-json` with the same flags and control protocol cdesktop
uses.

| File | How it was produced |
| --- | --- |
| `can_use_tool_bash.json` | `initialize` carrying cdesktop's approvals `PreToolUse` hook, then `set_permission_mode: default`, then a prompt asking for one `Bash` call. The hook callback was answered `permissionDecision: "ask"`, which is what makes the CLI raise `can_use_tool`. |

The fixture records the finding this lane acted on: on cdesktop's path the
request arrives with **no** `permission_suggestions` field. Claude Code has a
session-scoped permission mechanism, but for a hook-forwarded request it hands
over no rule to persist, so cdesktop offers no session-scoped approval rather
than inventing a rule width the operator never saw.

Two other shapes were tried and are recorded here because their absence is the
evidence: launching without the `PreToolUse` hook, and launching with
`--permission-mode=default` and no `initialize`. In both the CLI ran `Bash`
with no `can_use_tool` at all.
