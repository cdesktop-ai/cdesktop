# Approvals across harnesses

cdesktop brokers two unrelated gates that both get called "approval".
Keeping them apart is the point of this document.

**The metered gate** decides whether a session command may spend on a metered account.
It lives in `MeteredApproval::gate`, is consulted by the session-command dispatcher before any executor is spawned, and is `auto` / `ask` / `never`.
It is allow-once by construction: an `ask` approval is stamped with the execution process that consumed it, so it authorizes exactly one attempt.
Nothing in it is per-harness, and a test pins that (`the_metered_gate_decides_the_same_way_for_every_harness`).

**The tool gate** decides whether a running agent may make a particular tool call.
It lives in `utils::approvals`, reaches the operator through `ExecutorApprovalBridge` and `POST /approvals/{id}/respond`, and each adapter translates the operator's answer into its harness's own protocol.

A session-scoped tool approval never authorizes another metered launch.
The two gates share no state and no vocabulary beyond the word "approval".

## The shape every harness maps onto

A tool decision is an `ApprovalOutcome`, and an approval carries an `ApprovalScope`:

- `once` - authorize this request only.
- `session` - authorize it and ask the harness to remember, bounded by what the operator was shown.

Each harness cdesktop brokers approvals for has some native "and stop asking" form, so `session` is one request they can all be asked for - though not all of them offer it on every request.
An adapter with nothing to persist for a given request degrades to `once`; never the reverse.
The worst an operator can be surprised by is being asked again.

Alongside the decision, an approval carries `ApprovalPatterns`:

- `request` - what this one call covers.
- `session` - what a `session` decision would allow for the rest of the run.

These are different widths, and both are load-bearing.
OpenCode answers a `bash` request for `echo w2-first` with `always: ["echo *"]`.
An operator shown only the narrow list would read one command while granting a whole verb.

`session` being empty is the one invariant the UI consults: an adapter leaves it empty exactly when it has no session-scoped form for that request, and the session action is not offered.

## What each harness actually provides

| harness | request scope | session grant | session offered |
| --- | --- | --- | --- |
| OpenCode | `PermissionRequest.patterns` | `reply: "always"`, which installs `PermissionRequest.always` | always |
| Codex | the command, or the write root it asked for | `acceptForSession` on the command and file-change decisions | always |
| Claude Code | not reported | the CLI's `permission_suggestions`, re-pinned to `destination: "session"` | only when suggestions arrive |
| ACP agents | not reported | the agent's own `allow_always` option | only when the agent offers it |

### OpenCode

The richest of the three: a queryable pending list, structured scope on every ask, and a persistent reply verb.
`reply: "always"` is self-contained - the request's `always` patterns are the rule, so nothing is echoed back with it.

Verified live against `opencode serve` 1.15.10.
Replying `always` to a `bash` ask carrying `always: ["echo *"]` let a later `echo` in the same session run untouched, and still stopped a later `ls -la`, which raised a fresh ask with `always: ["ls *"]`.
The rule installed is exactly the width the operator was shown.

One observation recorded rather than relied on: replying `once` to that same ask *also* let a later, different `echo` through.
For `bash` on 1.15.10, `once` appeared to grant at least the `always` pattern width too.
Nothing here depends on the two verbs differing - `always` is the reply that installs the rule, and the operator is shown that rule either way - but an operator reading "approve once" is being granted more than the words suggest.

### Codex

Codex does raise discrete approval requests: `FileChangeRequestApproval` and `CommandExecutionRequestApproval`, each with its own `acceptForSession` decision.
Its degradation is not a missing prompt. It is that **whether those requests arrive at all is a property of the sandbox, not of the operator's approval policy.**

`ThreadStartParams` carries `sandbox` and `approval_policy` together, and cdesktop derives both from one `PermissionPolicy`:

- `BypassPermissions` sets `askForApproval: never`, and Codex raises no approval request at all.
- `Supervised`, `AcceptEdits` and `Auto` all collapse to `askForApproval: unlessTrusted`.
- With no explicit sandbox, `sandbox: workspaceWrite` is paired with `askForApproval: onRequest`.

So a run configured for `acceptEdits` and a run configured for `supervised` ask identically, and a run that widened its sandbox stops asking about the things the sandbox now permits.
Codex decides what is worth interrupting for; cdesktop only answers.

There is no repair for this at the adapter, and cdesktop does not synthesise the prompts Codex chose not to send.
What the adapter does do is honour a `session` decision when a request *does* arrive, by sending `acceptForSession` instead of `accept` - a capability the protocol has always had and cdesktop previously reached only on its auto-approve path.

Two further protocol facts, recorded because they bound what an approval means here:
`FileChangeApprovalDecision::AcceptForSession` is documented as covering "the same files", and `grantRoot` - Codex's request to widen writes for the session - is marked unstable, with its own protocol noting it is unclear whether it is honoured today.

### Claude Code

Claude Code has a real session-scoped mechanism: a `PermissionUpdate` with `destination: "session"`, which cdesktop already uses to switch modes after `ExitPlanMode`.

The gap is upstream of that. cdesktop's approval path is a `PreToolUse` hook that answers `permissionDecision: "ask"`, which makes the CLI raise `can_use_tool`.
Captured from Claude Code 2.1.232 driven with cdesktop's own flags and hooks, that request arrives with **no** `permission_suggestions` field - the fixture is checked in.
Two other launch shapes were tried, without the hook and without `initialize`; both ran the tool with no `can_use_tool` at all.

The only rules cdesktop will persist for Claude Code are the CLI's own suggestions.
Deriving one from `tool_input` instead would mean choosing a width - `Bash(echo w2-claude-first)` or `Bash(echo *)` - and silently granting whichever guess was wrong.
So when no suggestion arrives, no session action is offered and a session decision stays one-shot.

When suggestions *do* arrive, their destination is forced to `session` before being applied.
A suggestion may name `userSettings` or `projectSettings`, and honouring that would write a rule into the operator's own files that outlives the run they approved it for.

## Adding a harness

Fill `ApprovalPatterns` from what the harness reports and map `ApprovalScope::Session` onto its persistent form.
If it has none for a request, leave `patterns.session` empty; the operator is then only offered `once`, which every harness can honour.
Do not invent a scope the harness did not name.
