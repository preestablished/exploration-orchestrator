# Plan: Proto Upstream + Tier-2 True-SIGKILL Chaos Harness

Answers `.agents/requests/phase5-prep-proto-upstream-and-tier2-chaos/`.
Baseline: `main` @ `bf5b7b3`, clean tree. Companion request (control-plane's
half): `../control-plane/.agents/requests/phase4-proto-freeze-tag-and-breaking-gate/`.
Beads covered: `exploration-orchestrator-777` (proto upstream),
`exploration-orchestrator-6ft` (Tier-2 chaos).

## Objective

1. **Item 1 — proto upstream (`777`).** Land the real
   `determinism.orchestrator.v1` (six RPCs, `ProgressEvent` stream,
   `ExperimentConfig`) in `control-plane/proto/determinism/orchestrator/v1/`,
   replacing the incompatible placeholder; extend `determinism-proto`'s
   `orchestrator` feature to real tonic codegen (authored by us, reviewed by
   control-plane, the way `inputsynth` is consumed today); reduce
   `crates/orch-proto` to a re-export with **no local `.proto` copy left**.
   Must land **before** control-plane tags `proto-v0.2.0` / activates
   `buf breaking` against a baseline containing the placeholder.
2. **Item 2 — Tier-2 chaos (`6ft`).** True SIGKILL of the whole
   `orchestratord` process mid-run, whole fake world persisted
   out-of-process via a crash-consistent journal, resume from checkpoint,
   bit-identical continuation vs an uninterrupted control run — the Tier-1
   standard (`chaos_resume.rs`) across a process boundary. ≥5 seeds, ≥50
   SIGKILLs total, all 11 Tier-1 lattice points plus forced
   mid-checkpoint-write and mid-WAL-append, plus one demonstrated negative.

Both items are fakes-only/proto-only. M5+ stays untouched (beads `cww`,
`w1v`, `isj`, `5em`, `a78` stay parked).

## What already exists (do not rebuild)

- **The proto surface is done and served.**
  `crates/orch-proto/protos/determinism/orchestrator/v1/orchestrator.proto`
  is the file to upstream *verbatim in wire shape* (comment header changes
  only, plus whatever lint posture D-P2 settles). `orch-server` +
  `bins/orchestratord` consume it only as `orch_proto::orchestrator_v1`;
  `orch-driver` consumes only `orch_proto::inputsynth::v1`. The cutover
  therefore touches `orch-proto` internals and nothing downstream.
- **The consumption pattern is proven.** `determinism-proto`'s
  `inputsynth`/`scorer` features already do gated tonic codegen
  (`build.rs` branches on `CARGO_FEATURE_*`, packaged-copy staleness guard,
  `tonic::include_proto!` in `lib.rs`, facade smoke test). The
  `orchestrator` feature copies that pattern exactly.
- **Both repos' CI already interlock.** Our `ci.yaml` checks out the
  control-plane sibling on x86_64 + aarch64 and runs the workspace; their
  CI is cargo-only today (buf gates are their request's item 1).
- **The runner's resume machinery is complete and Tier-1-proven.**
  `ExperimentRunner::start` auto-detects the checkpoint;
  `load_wal_entries` self-repairs torn truncation; `CrashPolicy` hooks
  exist at all 11 lattice points (`orch-server/src/experiment.rs`);
  `FakeHypervisor::reclaim_session` models the worker reclaiming a dead
  session's leases. **Tier-2 adds durability underneath and a process
  boundary around this — it changes none of it.**
- **The comparator exists.** `store_tree_hash` + dense-id assertion +
  `assert_no_stranded_frontier` in
  `crates/orch-server/tests/chaos_resume.rs:43-121`. Tier-2 extracts and
  reuses it (W2.4), not reinvents it.
- **`orchestratord` already has the two modes Tier-2 needs**:
  `--simulate` (served gRPC over fakes) and `--experiment <yaml>`
  (standalone run-to-completion, exit code). What's missing is
  `--state-dir` persistence and a way to park at a crash point so the
  harness can land a real SIGKILL there.

## Files in this plan

| File | Contents |
|---|---|
| `01-decisions.md` | Eight decision points: three proto-side (D-P1..3), five Tier-2 (D-T1..5), each with rejected alternatives and revisit triggers |
| `02-proto-upstream.md` | Item 1 work items W1.1–W1.6 (both repos' edits, authored here) |
| `03-tier2-chaos.md` | Item 2 work items W2.1–W2.7 (journal crate, orchestratord wiring, harness, negative control, evidence) |
| `04-verification.md` | Acceptance-criteria mapping, evidence conventions, CI changes, resolution/handback shape |

## Sequencing

```
W1.1 (coordination note: mirror scope + lint posture + EventEnvelope flag)
  → W1.2 control-plane edits (proto + feature + build.rs + lib.rs)
  → W1.3 lint posture applied (buf.yaml exemptions or renames, per D-P2
        and control-plane's item-1 state)
  → W1.4 orch-proto cutover (delete local protos/, re-export)
  → W1.5 cross-repo verification + breaking-gate demo + close 777
W1.6 (EventEnvelope divergence bead + note) — any time, independent

W2.1 orch-simstate journal  → W2.2 PersistentWorld wrappers
  → W2.3 orchestratord --state-dir + chaos hooks
  → W2.4 Tier-2 harness (lattice + torn-write + random kills)
  → W2.5 negative control → W2.6 evidence script + CI lane
  → W2.7 D5 trail note + close 6ft + 04-resolution.md
```

Item 1 first (smaller, another repo is adding gates around it, downstream
consumers wait on it), per the request's suggested sequencing. Item 2 is
self-contained; W2.1–W2.2 may start in parallel once W1.1 is filed, since
the two items share no files.

One commit per work item (or small coherent group) on `main`, SHAs
recorded in `04-resolution.md`. Control-plane-side commits land in their
repo via their review; record both repos' SHAs.

## Tracking (beads)

Both beads exist — claim, don't create:

```bash
bd update exploration-orchestrator-777 --claim
bd update exploration-orchestrator-6ft --claim
```

Add child beads only if a session ends mid-item. W1.6 creates one new
bead (EventEnvelope divergence — see `02-proto-upstream.md`).

## Out of scope (explicit)

- M5 hardening/soak, M6 real substrate, M7 — gated; nothing here starts
  them. (A separate `phase5-m5-hardening-on-fakes` request exists; this
  plan does not touch its scope.)
- `cww` (async input-synth transport adapter) — only if both items land
  with window to spare, and then disclosed in the resolution.
- Observatory ingest; reconciling the `EventEnvelope` divergence (item 1
  only *flags* it — W1.6).
- Control-plane's own items: buf gates, aarch64 lane, `ExperimentSpec`
  mirror rework, descriptor-equality check, the `proto-v0.2.0` tag. We
  author the upstream PR content and the agreement note; they land/review
  their half.
- Any change to `orch-core`, `orch-clients` trait semantics, checkpoint
  format (`CHECKPOINT_VERSION` stays 1), or the Tier-1 lattice's
  assertions. `orch-fakes` stays fs-free (D-T1).
