# CAS Ownership Loss

Scope: exercise the single-writer discipline behind checkpoint metadata CAS.
Current state has `REASON_CAS_OWNERSHIP_LOST = "checkpoint-cas-ownership-lost"`
and catches `FailedPrecondition`/`AlreadyExists` on checkpoint `PutMetadata`.
M5 must make the path deliberate, testable, and covered for both requested
windows.

## W5.9 - Add an ownership guard and injection seam

Add an `ensure_checkpoint_owner(window)` helper in `ExperimentRunner`.

Behavior:

- If `self.ckpt_generation` is `Some(generation)`, read
  `MetadataKey::checkpoint(experiment_id)` and compare the returned generation.
- If it differs, is absent, or returns a CAS-style precondition conflict, fail
  with `checkpoint-cas-ownership-lost: ...`.
- If the read has a retryable transient error, use the normal retry policy.
- Mark ownership-loss errors as a special terminal class that skips the normal
  final checkpoint/archive attempt in `run()`. A stale loser must not write one
  more checkpoint after it has discovered that another writer owns the key.

Call sites:

- Immediately before local C-stage mutation (`commit_batch(...)`,
  replay-commit adoption/insertion, or any tree/frontier/seen mutation) for the
  node-commit window. Guarding only `CreateNode` is too late because the runner
  mutates in-memory commit state before store writes.
- Existing checkpoint `PutMetadata` remains the checkpoint-window guard.

Add a test-only fault hook that can perform a competing metadata write at a named
ownership window. Keep it in test support or behind the existing `CrashPolicy`
style seam; do not expose it as production CLI surface unless needed by the soak.

Acceptance:

- Ownership-loss errors use the central runtime reason constant.
- The loser transitions to `ExperimentState::Failed` and publishes the reason.
- No generic store error leaks to the user for the intended CAS path.
- The final-checkpoint path is suppressed for ownership-loss failures.

## W5.10 - Checkpoint-write window scenario

Test: a competing writer updates `orch/ckpt/<exp>` after the runner has built a
checkpoint but before its CAS `PutMetadata`.

Recommended shape:

- Use the existing fake world and a small deterministic config.
- Inject takeover at `CrashPoint::BeforeCasPut` or a new ownership hook adjacent
  to it.
- The competitor writes a valid checkpoint-shaped payload or a sentinel payload
  with the next generation. The payload content is less important than the
  generation conflict; prefer a valid checkpoint if convenient.
- The original runner attempts `PutMetadata` with stale expected generation,
  receives the conflict, and fails cleanly.

Assertions:

- Outcome state is `Failed`.
- `failure_reason` starts with `checkpoint-cas-ownership-lost`.
- The reason appears in the `FAILED` subset of `docs/runtime-terminal-reasons.md`.
- No loser writes occur after the failed CAS, including final checkpoint/archive
  writes. Count store/scorer mutating operations before and after if needed.

## W5.11 - Node-commit window scenario

Test: a competing writer takes the checkpoint key while the original runner is
about to commit a completed batch.

Recommended shape:

- Inject takeover just before the C-stage tree-write block.
- The new ownership guard must detect the generation mismatch before the loser
  calls `commit_batch(...)`, mutates local tree/frontier/seen state, or calls
  `CreateNode`.
- Instrument the fake store or persistent journal to count `create_node`,
  `update_nodes`, `put_metadata`, and `delete_metadata` operations by writer
  incarnation.

Assertions:

- The stale runner fails with `checkpoint-cas-ownership-lost`.
- There are zero loser local commit mutations, `CreateNode`/`UpdateNodes`/
  `ReplayCommits` calls, or final checkpoint/archive writes after the takeover
  injection point.
- The checkpoint key generation remains the competitor's generation.
- A fresh runner can resume from the winner's checkpoint or, if the test uses a
  sentinel payload, the failure is explicitly limited to the loser path and does
  not masquerade as a resume test.

Evidence:

- Capture both tests in `evidence/phase5-m5-hardening/cas-ownership.txt`.
