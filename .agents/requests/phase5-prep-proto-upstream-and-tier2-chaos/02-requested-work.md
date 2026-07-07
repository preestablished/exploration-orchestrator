# Requested Work

## What We Need (Behavioral)

1. **Proto upstream (`exploration-orchestrator-777`).**
   - Land the actual `orch-proto` surface — `orchestrator.proto`: six
     RPCs, the `ProgressEvent` stream, `ExperimentConfig` — in
     `control-plane/proto/determinism/orchestrator/v1/`, **replacing the
     incompatible placeholder file already at that path** (different
     `StartExperimentRequest`, empty `service ExplorationOrchestrator`).
     The placeholder has no consumers; replacing it is safe *only before*
     control-plane's `buf breaking` baseline/tag is cut — this replacement
     must land before `proto-v0.2.0` is tagged. Style fixes their
     `buf lint` demands are yours to make, wire-compatibly, before or
     with the merge.
   - The consumption mechanism, concretely (do not just re-aim
     `orch-proto/build.rs` at the control-plane path): extend
     `determinism-proto`'s `orchestrator` feature to run real tonic
     codegen (it currently pulls only facade deps — the `build.rs` /
     Cargo feature change lands in control-plane with their review, but
     you author it as part of the upstream PR, the way `inputsynth` is
     consumed today), then reduce `crates/orch-proto` to a re-export of
     that feature and **delete `orch-proto/protos/` and its build.rs
     codegen** — no forkable local copy left behind.
   - `ExperimentSpec` ↔ `ExperimentConfig`: control-plane's plan mandates
     a field-for-field mirror enforced by a descriptor-equality CI check,
     divergence fixed control-plane-side. Control-plane owns landing the
     check (their request, item 4); your job is to not merge the upstream
     without agreeing which message shapes the mirror covers.
   - **Flag, don't fix, the EventEnvelope divergence:** your runtime
     envelope (`orch-clients/src/observatory.rs`, postcard,
     `producer_id`/`ts_logical`) differs from the canonical
     `observatory/v1/events.proto` (`payload_json`). Record the
     divergence and its intended resolution (a bead here or a note in the
     control-plane request dir) so observatory M1 doesn't discover it
     cold. It is not part of the `orchestrator/v1` upstream.
2. **Tier-2 chaos harness (`exploration-orchestrator-6ft`).**
   - A harness that runs a real experiment on fakes as a separate process,
     SIGKILLs it (no cooperative shutdown path) at scheduled and random
     points — including mid-checkpoint-write and mid-WAL-append —
     persists the fake world out-of-process, resumes from checkpoint, and
     drives the search to completion. Scope of the persisted fake world
     is enumerated in bead `6ft`'s description (hypervisor snapshots +
     slots, scorer archive + batch cache, store, synth state,
     crash-consistent journal, `orchestratord --simulate` wiring) — treat
     that as the checklist.
   - The standard is the one M4 already set: the resumed run's committed
     tree/archive state must be bit-identical to an uninterrupted control
     run (the Tier-1 lattice's equivalence check, now across a process
     boundary).
   - Cover the WAL-truncation-at-checkpoint design specifically — it was
     disclosed reinterpretation #2 in the M3/M4 resolution and is exactly
     the kind of design a true kill either validates or breaks.

## Suggested Sequencing (Yours To Overrule)

Item 1 first — it is smaller, another repo is adding gates around it, and
observatory/control-plane consumers are downstream of it. Item 2 is
self-contained and can absorb the remaining quiet window.

## Acceptance Criteria

1. `determinism.orchestrator.v1` builds from `control-plane/proto/` in
   both repos' CI; `orch-proto` contains no independent copy of the
   `.proto` definitions; `exploration-orchestrator-777` closed with the
   control-plane commit referenced.
2. The `ExperimentSpec`/`ExperimentConfig` relationship is recorded
   (comment, doc, or CI check) — an agent landing a field in one knows
   what it means for the other.
3. Tier-2 harness in CI (or a documented manual lane if runtime makes CI
   impractical — say which and why): **≥5 seeds and ≥50 SIGKILL runs
   total** (matching the plan's own M4 "50 runs" bar), kill points
   covering all Tier-1 lattice points plus forced mid-checkpoint-write
   and mid-WAL-append; resume; bit-identical continuation vs control.
   Plus one **demonstrated negative**: an intentionally broken resume
   (documented mutation, e.g. WAL replay skipped) must fail the
   comparator — a chaos harness that cannot detect a real divergence
   proves nothing. `exploration-orchestrator-6ft` closed with evidence
   under `evidence/` following the existing discipline.
4. A short note in the D5 reinterpretation trail marking the Tier-2 gap
   closed, so the Phase 5 gate-run checklist can cite it instead of the
   descope.

## Out Of Scope For This Request

- M5 hardening/soak, M6 real-substrate integration, M7 — gated on other
  repos' phase gates; the phase docs are explicit and this request does
  not soften them.
- `cww` (async input-synth transport adapter) — a real M6 pre-req, but
  M6-shaped work; pick it up only if both items above land with window to
  spare, and say so in the resolution if you do.
- Observatory's ingest side — theirs; item 1 just makes your stream's
  schema canonical so they can start clean.
