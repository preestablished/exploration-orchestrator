# Resolution — Proto Upstream + Tier-2 True-SIGKILL Chaos Harness

Executed per `.agents/plans/phase5-prep-proto-upstream-and-tier2-chaos/`
(plan reviewed twice pre-execution; review deltas folded in before work
started). Both items landed 2026-07-08.

## Commit table

| Work item | Repo | SHA | Contents |
|---|---|---|---|
| W1.1 coordination note | control-plane | `2f6f911` | `06-orchestrator-upstream-notes.md` in their request dir: D-P3 mirror scope (full `ExperimentConfig` closure), D-P2 lint posture (three `ignore_only` exemptions + renumbering escape hatch with its persisted-`config_hash` cost), sequencing reminder, EventEnvelope divergence flag |
| W1.2 canonical proto + codegen | control-plane | `9cb1a0c` | Placeholder replaced by the real `determinism.orchestrator.v1` (wire shape byte-identical to the served copy; header comment only); `determinism-proto` `orchestrator` feature → real tonic codegen on the inputsynth/scorer pattern (build.rs gating, packaged copy + staleness guard, facade smoke test); `prost-types` added for `google.protobuf.Timestamp` |
| W1.4 orch-proto cutover | this repo | `52965ad` | `orch-proto` reduced to the D-P1 re-export shim; local `protos/` + `build.rs` deleted; `protos.lock` rewritten; zero source changes outside the crate (verified: workspace + grpc_surface + seed_gate green) |
| W2.1/W2.2 journal + wrappers | this repo | `ce78faf` | `crates/orch-simstate`: crash-consistent op journal, `Persistent<T>` wrappers, replay with `Applied`-digest pairing by `op_id`, shared comparator + `scorer_archive_fingerprint`; `chaos_resume.rs` refactored onto the shared comparator (assertions identical) |
| W2.3 bin wiring | this repo | `3cf7d30` | `orchestratord --state-dir` (both modes, one concrete world type); `ORCH_CHAOS_HANG_AT` hang-hook (`--experiment` only; served path keeps no policy per D-T4); `ORCH_SIM_BREAK`; `CrashPoint` `as_str`/`FromStr` |
| W2.4–W2.6 harness + CI | this repo | `cdc3d40` | `bins/orchestratord/tests/tier2_chaos.rs` (lattice/torn/random kill classes, gRPC resume smoke, negative control), `scripts/evidence-tier2.sh`, `tier2-chaos-smoke` CI job (both arches), M3/M4-resolution addendum |
| W2.7 evidence + resolution | this repo | _see `git log`_ | `evidence/phase5-tier2-chaos/` + this file |

W1.3 (buf lint exemptions): control-plane's item-1 buf gates had not
landed at execution time (no `buf.yaml` in their tree; `buf` not
installed locally — the plan says don't install tooling just for this).
The exact `ignore_only` stanza ships in the W1.1 note (§2), explicitly
marked "fold into your item-1 buf.yaml".

## Bead dispositions

- `exploration-orchestrator-777` — **closed** ("determinism.orchestrator.v1
  canonical in control-plane @ 9cb1a0c; orch-proto reduced to re-export
  (this repo @ 52965ad); no local proto copy remains").
- `exploration-orchestrator-6ft` — **closed** (see closure text; harness,
  wiring, and evidence paths).
- `exploration-orchestrator-75z` — **opened** (W1.6): reconcile the
  runtime `EventEnvelope` vs the canonical `observatory/v1` proto; owner
  is observatory M1 ingest design. Same content flagged in the W1.1 note
  (§4) so observatory finds it next to the proto-freeze context.
- `cww` (async input-synth transport adapter) — **untouched**, per plan
  scope. M5+ beads (`cww`, `w1v`, `isj`, `5em`, `a78`) all stay parked.

## Evidence

- `evidence/phase5-tier2-chaos/run-manifest.md` — date, host, toolchain,
  commit, seed/kill parameters.
- `evidence/phase5-tier2-chaos/tier2-chaos.txt` — full-matrix run
  (`TIER2_SEEDS=5`, all 11 lattice points, both torn kinds, random
  kills), containing the `TIER2_SUMMARY seeds=… kills=… lattice=…
  torn=… random=…` line; kill quota ≥ 50 asserted by the harness itself
  (`TIER2_MIN_KILLS=50`).
- `evidence/phase5-tier2-chaos/negative-control.txt` — the demonstrated
  negative, naming the mutation (`perturb-node`) and the observed
  divergence (`TIER2_NEGATIVE … hash_diverged=true`).

Reproduce: `scripts/evidence-tier2.sh` (env-overridable); phases-track
spot-check: `CHAOS_SEED=<fresh> TIER2_ENABLE=1 cargo test -p
orchestratord --test tier2_chaos -- --nocapture`.

## Cross-repo verification state (item 1)

- Our CI consumes the canonical proto location on x86_64 + aarch64 via
  the existing control-plane sibling checkout; both workspaces built and
  tested green locally (`cargo build/test --workspace --all-features` in
  both repos).
- **Breaking-gate demonstration: pending their gates** — their item-1
  `buf breaking` CI does not exist yet, so the scratch-branch
  demonstration cannot run (plan W1.5 anticipated this: whichever repo
  lands second runs it; their resolution carries the evidence). Not
  silently skipped — see reinterpretation (g).

## Reinterpretations and named deltas (disclosed, numbered)

(a) **Lint posture: exemptions, not renames** (D-P2). The upstream ships
    with three `ignore_only` exemptions proposed in the coordination
    note (`ENUM_ZERO_VALUE_SUFFIX`, `SERVICE_SUFFIX`,
    `RPC_RESPONSE_STANDARD_NAME` — the third found by review against
    the full DEFAULT category, beyond the request's named rules). None
    is fixable wire-compatibly. The renumbering escape hatch (with its
    persisted-`config_hash` cost) is documented in the note, exercised
    only on control-plane's explicit ask — which has not come.
(b) **CI-vs-manual lane** (D-T5): the full matrix is the manual evidence
    lane; CI runs the reduced smoke (1 seed, 3 lattice points, both torn
    kinds, 2 random kills, gRPC smoke, negative control) as the
    dedicated `tier2-chaos-smoke` job on both arches. Measured runtime
    (debug, 2-vCPU ext4 host): ~11.5 s per uninterrupted run without a
    journal, ~225 s with (fsync-per-mutating-op dominates, as D-T5
    predicted; batching syncs would defeat the torn-write points and was
    not done). The full matrix therefore lands well past the ~10-minute
    CI promotion threshold — manual lane it is, with rounds parallelized
    across worker threads (state-dirs independent; fsync-bound).
(c) **Reclaim-on-reload** (D-T4): `PersistentServices::reload` invokes
    `FakeHypervisor::reclaim_session()` after replay and journals it, as
    the Tier-2 stand-in for the worker observing its client connection
    drop — echoing W4.4a's standing disclosure; re-verify semantics at
    M6.
(d) **`Applied`-frame digest design**: as sketched in D-T2, one delta —
    errors digest via a kind-as-string + message mirror struct local to
    `orch-simstate` (no serde on `ClientError`), and replay in
    `reload_broken` skips digest checks entirely (the mutation exists to
    diverge; the end-state comparator is the assertion).
(e) **Fake determinism audit findings**: none. No digest mismatch fired
    across the smoke or evidence runs; no fake-side fix was needed. The
    known-benign `watch_slots` cursor wart (`Cell` behind `&self`) is
    documented in the plan and in code comments, invisible to digests
    as predicted.
(f) **Negative-control mutation replaced** (W2.5): the request's "skip
    WAL replay" example is convergent by design (WAL entries are
    self-healing instructions; intents re-derive from `(seed,
    batch_seq)` + checkpoint frontier) and was replaced by the
    perturbed-node replay mutation (`ORCH_SIM_BREAK=perturb-node`,
    bumping one journaled `create_node`'s `progress_score`). The harness
    asserts the hash arm fires specifically, and that the unbroken
    reload of the same pre-relaunch journal converges — pinning the
    divergence on the mutation.
(g) **Gates→upstream inversion** (W1.5): the upstream landed before
    control-plane's buf gates exist (their item-1 timing is theirs; the
    only hard constraint — upstream before the `proto-v0.2.0` tag — is
    satisfied since no tag exists). Their item 3 ordering ("gates first,
    then upstream as a lint-only concern") is therefore inverted; the
    W1.1 note carries the exemption stanza so their gates land green
    over our file, and the breaking-gate demo transfers to their
    resolution.

## Phases-track handoff

Per `03-verification-offer.md`: respond with `05-verification.md` after
the clean-checkout re-run at a fresh `CHAOS_SEED` (single-seed override
is implemented and exercised — see Evidence).
