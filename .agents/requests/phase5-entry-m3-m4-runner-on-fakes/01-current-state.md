# Verified Current State (2026-07-06)

Everything below was checked against the working tree at `3caa493`
("Harden generated input synth adapter", tip of `main`).

## What is done

**M1–M2 (complete).** The ralph run (iterations 1–35, merged through
`c513d80` "iteration 35 merge - final validation") built and validated:

- `orch-core` (~6.4k lines, pure/synchronous, no tokio/tonic): types,
  feature-layout compilation (`compile.rs`), tree/frontier/mirror state,
  commit rules, plateau tracking, deterministic RNG (ChaCha12 substreams
  with golden vectors, iteration 4), selection policies, plus a
  `purity_guard` test. Note for M4 scoping: this already includes a full
  `ExperimentConfig` with a `validate()` matrix (`types.rs`, incl.
  `CheckpointConfig` range checks) and the plateau/escalation-ladder
  state machine (`plateau.rs`: `PlateauConfig`, `LadderConfig`,
  `EscalationKnobs`, `EscalationLevel`) — M4 wires these, it does not
  build them.
- `orch-clients`: transport-free DTOs and sync client traits for the
  hypervisor, scorer, snapshot-store, and input-synth boundaries.
- `orch-fakes`: deterministic grid world (`grid.rs`), FakeHypervisor,
  FakeScorer (archive + rebin, iteration 27), FakeSynth (bursts, macros,
  fingerprints, iteration 26), fake snapshot store, fault injection
  (`fault.rs`), transcript hashing (`transcript.rs`), contract tests,
  and a fake search-loop test (`tests/search_loop.rs`, iteration 30).
- `orch-proto`: re-exports of the owner orchestrator proto surface.

**Phase 4 input-synth integration (complete).** Commits
`510dfb3`–`3caa493` landed the generated `determinism.inputsynth.v1`
adapter in `orch-driver` (the control-plane facade it needed shipped in
control-plane `2a97392`/`261141b`), seed derivation, fingerprint guard
rails, the versioned node-attrs envelope, and `NodeContext`
reconstruction. The request-context smoke from that plan also landed:
`orch-driver/tests/input_synth_context.rs` asserts parent/sibling bursts
and `score_delta` reach `ProposeBurstsRequest`, and that a fingerprint
mismatch commits no children (bead `exploration-orchestrator-05w`,
closed 2026-06-23).

## What is missing (the M3/M4 gap)

- **No `orch-sched` crate.** The workspace members are exactly
  `orch-clients`, `orch-core`, `orch-driver`, `orch-fakes`, `orch-proto`.
- **`orch-driver` is only the input-synth adapter**: its `src/` is
  `input_synth.rs`, `node_attrs.rs`, `lib.rs`. No worker driver, no
  pipeline, no runner, no checkpoint code.
- The consuming side of the client traits does not exist: no worker-pool
  scheduler, no runner loop. (Do not over-read this — `orch-core` already
  holds `ExperimentConfig` validation and the escalation-ladder state
  machine, per above; what is missing is the wiring, not those types.)
- No served gRPC surface (the six ExperimentService RPCs), no WAL, no
  `CheckpointV1`, no plateau-ladder wiring into a real run loop, no
  ExperimentConfig standalone-YAML loading.
- No observatory client boundary: `orch-clients/src/` has no
  `observatory.rs` (ARCHITECTURE.md §1 expects an `EventSink` trait
  there) and `orch-fakes` has no fake event sink. M4's `EventEnvelope`
  emission needs that trait + fake created first — transport-free, like
  the other four boundaries.
- Your own `README.md` still says: "`orch-driver` remains a placeholder
  during M1-M2. Scheduler/pipeline work is M3 scope, and checkpoint
  runner/resume orchestration is M4 scope." Accurate — and worth updating
  as part of this work.

## Tracking state

`bd list` shows three open beads (`…-5em` bounded input tails P2,
`…-isj` synth-docs alignment P2, `…-a78` macro-pack planning P3). None
tracks M3 or M4. There is no repo-local `.agents/requests/` history —
this is the first request filed against this repo.

## Cautions

- Keep `orch-core` pure (its `purity_guard` test enforces this) and keep
  `orch-clients`/`orch-fakes` transport-free — the fakes-first design is
  the reason M3/M4 have zero platform dependencies, and Phase 5's
  parallelism notes say to enforce exactly that.
- Preserve deterministic-path discipline (no wall-clock, no thread RNG,
  no hash-map iteration order, ordered sibling/context assembly) — the
  same rules the input-synth plan's execution notes state.
- `determinism-proto` is consumed by path from
  `../control-plane/crates/determinism-proto` with
  `default-features = false`; the orchestrator + inputsynth features you
  need are already published there (control-plane `261141b`).
