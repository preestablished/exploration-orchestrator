# 05 Smoke, Validation, and Handoff

Goal: prove the whole request with focused tests before any live Phase 5 run depends on it.

Concrete M3 smoke:

- Add a test that commits a parent with at least two children, A and B.
- A and B must have distinct `ProvenancedBurst` values and distinct scores.
- Expand A through the synth request-building path.
- Capture the `ProposeBurstsRequest` sent to the synth client.
- Assert:
  - `request.node_context.parent_burst == A.created_by_burst`
  - `request.node_context.sibling_bursts.len() == 1`
  - the sibling burst is B's `created_by_burst`
  - sibling `score_delta == B.score - parent.score`
  - mutation-only synth config does not degrade as `no_parent_burst`
  - returned bursts with good fingerprints can proceed to commit
- Add a paired mismatch test where the synth response fingerprint changes unexpectedly and no child commit call occurs.

Suggested test mechanics:

- Use `InMemorySnapshotStore` for parent/child metadata.
- Use the new attrs helpers from `04-node-attrs-context.md`.
- Use a small recording synth client wrapper around `FakeSynth` or a purpose-built test client implementing `InputSynthClient`.
- Keep this as a driver/fakes integration test rather than adding runtime/network dependencies.

Quality gates:

- `cargo fmt --check`
- `cargo test -p orch-clients`
- `cargo test -p orch-driver`
- `cargo test -p orch-fakes`
- `cargo test --workspace`

Gate matrix:

- If the generated input-synth facade is unavailable, leave no partial generated-adapter code in the workspace and keep the workspace compiling.
- For each unblocked local slice, run the package tests it touches before closing that slice.
- Run `cargo test --workspace` before closing the parent request only when generated-dependent code is not blocked.
- If generated-dependent code is blocked, run all non-blocked local gates and keep the parent request issue blocked/open with a final handoff that names the blocked adapter work.

Handoff expectations:

- Update Beads issue status for every implemented slice.
- File follow-up issues for:
  - generated facade unavailable
  - cross-repo docs still disagree on synth seed derivation
  - recent-input tail intentionally left empty
  - mid-experiment pack loading or mined-pack adoption, which are out of scope for this request
- Run the repo close protocol from `AGENTS.md`, including `bd dolt push`, `git push`, and final `git status`.

Definition of done:

- The smoke proves parent and sibling burst context is present on a later expansion.
- Fingerprint mismatch commits no children.
- All non-blocked tests pass locally.
- Remaining blocked work is captured in Beads, not in loose markdown notes.
- The whole request is complete only after the real generated adapter and generated wire contract tests are implemented; otherwise the correct final state is blocked with the parent issue still open/blocked.
