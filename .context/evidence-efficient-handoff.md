# Native execution efficiency handoff

## Scope and base

- Base: `cf555c5b23b52bbd2f8eb6f10b30af72c68be58f`
- Branch: `cdt/9e93-sm-ev-efficient`
- Initial implementation commit: `119d0958af548c05098923a460dfb36a86e4f8a9`.
- Draft PR: https://github.com/cdesktop-ai/cdesktop/pull/21
- Fork target when publishing is authorized: `clarkipeng/cdesktop`

## Implemented

- Ordinary Codex follow-ups use `thread/resume` followed by `turn/start` on
  the recorded thread, rather than `thread/fork`.
- `resume_params_from` preserves model, provider, cwd, approval policy,
  sandbox, config, base/developer instructions, and service tier while leaving
  `history` and `path` absent. A failed resume propagates; it never falls back
  to start or fork.
- Resume uses the existing fresh-start disk/admission guard. Fork reservation
  remains applied only to the review isolation path.
- `/compact` now resumes in place before native compaction. `/fast` no longer
  creates an unused fork. The deliberate review path still forks.
- Static append-prompt material is joined with configured developer guidance
  in native `developer_instructions` on start and resume. Follow-ups carry
  only new task input. `base_instructions` remains unset without an explicit
  base, so Codex resolves its stored or model-default instructions normally.

## Evidence and limits

- The pinned `codex-app-server-protocol` checkout at
  `be6e8eac029b183056b7e4402879f15d2c85f61b` declares
  `ThreadResume -> thread/resume`; official OpenAI app-server documentation
  likewise distinguishes `thread/resume` (continue) from `thread/fork`
  (branch).
- Local source evidence: no ordinary path retains a `thread_fork` call;
  `thread_fork` is reached only by `codex/review.rs`.
- Protocol fixtures prove two ordinary continuations emit
  `thread/resume`/`turn/start` pairs on one thread with zero fork requests;
  the review fixture emits `thread/fork`. They also prove that absent explicit
  base guidance remains null at the start-request seam and that changed or
  cleared append guidance is reflected by resume parameters. Cancellation
  leaves the turn outcome unknown for reconciliation, rather than retrying it.
- Normalization keys usage by native thread and turn, so repeated usage updates
  for one turn replace its entry rather than replaying old usage.
- Measured local copy/startup result: not yet available. No provider/cache or
  latency claim is made. The live authenticated canary remains root-owned.

## Verification

- `git diff --check`: passed.
- Passed isolated command:
  `/usr/bin/sandbox-exec -f /Users/clarkpeng/conductor/workspaces/sightmesh-v1/ankara/.context/evidence-test-isolation.sb cargo test -p executors --lib`
  Exit `0`; 110 passed.
- `cargo clippy -p executors --tests -- -D warnings`: exit `0`.
- `pnpm run format`: exit `0` after `pnpm install --frozen-lockfile`; no source
  formatting changes outside this lane.
- All runtime checks used the required state/network isolation profile where
  applicable. No provider request or live state mutation occurred.

## Publication state

The revision is committed and force-pushed to the existing draft PR. No merge,
provider inference, cdesktop install/activation, or service restart was
performed.
