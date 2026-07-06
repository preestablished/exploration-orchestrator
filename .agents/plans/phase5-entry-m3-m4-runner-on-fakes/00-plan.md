# Plan: Land M3 + M4 (Scheduler + Experiment Runner on Fakes)

Answers `.agents/requests/phase5-entry-m3-m4-runner-on-fakes/`. Plan of
record: `~/.agents/projects/determinism/docs/exploration-orchestrator/`
(IMPLEMENTATION-PLAN.md §M3/§M4, ARCHITECTURE.md, API.md). Baseline:
`main` @ `f783e20` (working tree verified at `3caa493` by the request;
`f783e20` only adds the request files).

## Objective

- **M3** — `orch-sched`: worker driver (lease composition per API.md
  §2.2, verdict mapping, determinism-class gating), `SlotView` over
  `ListSlots`/`WatchSlots`, S→E→C pipeline with bounded queues, retry
  policy (purity for jobs, `client_batch_id` for scoring), fast +
  deterministic modes. Accept bars quoted in request `02-…` §M3.
- **M4** — `ExperimentRunner` end-to-end on fakes: bring-up, bootstrap
  on FakeHypervisor's Ready event, plateau ladder L1–L4 wired into a
  real loop, `CheckpointV1` + WAL + scorer-archive drain-lockstep,
  resume per the binding sequence (ARCHITECTURE.md §8.2), served gRPC
  surface (six RPCs), observatory `EventEnvelope` emission,
  ExperimentConfig validation incl. standalone YAML. Accept bars in
  request `02-…` §M4.

Everything runs against `orch-fakes` — zero platform dependencies.
M5/M6 scope stays out (but queue gauges and FAILED-reason strings are
designed so M5's runbook bar needs no refactor).

## What already exists (do not rebuild)

Per request `01-current-state.md` and a fresh code survey:

- `orch-core` already holds `ExperimentConfig` + full `validate()`
  matrix, the plateau/ladder state machine (`plateau.rs`), commit rules
  (`commit_batch`), `DeterministicRng` substreams with
  `derive_synth_request_seed`, feature-map compilation + `coarsen_l4`,
  policies, frontier/tree/mirror. **M3/M4 wire these; they do not
  reimplement them.** Named deltas from review: an accumulating
  `validate_all()` (the served surface must list every bad field —
  W4.6) and two new RNG purpose tags (`"boot"`, `"entropy"` — W3.2).
- Caveat (review): `orch-fakes` is *nearly* finished but four targeted
  hardening items sit on the M3/M4 critical path — fault-injector
  attempt salt (W3.0a), lease reclamation on session teardown (W4.4a),
  data-driven grid worlds (W4.2a), and (Tier-2 only) whole-world
  persistence (D5).
- `orch-clients` has sync traits + DTOs for all four boundaries,
  including `MetadataKey::checkpoint`/`::wal` and generation-CAS
  expectations. Missing: `observatory.rs` (`EventSink`).
- `orch-fakes` covers the needed API subsets: FakeHypervisor
  (ListSlots/WatchSlots, Fork with parent freeze, frame_budget,
  Ready-event bootstrap), FakeScorer (batch-id dedup replay,
  Checkpoint/Restore/ReplayCommits, rebin), InMemorySnapshotStore
  (idempotent CreateNode, QueryNodes, metadata CAS), FaultPlan
  (latency ticks, error/timeout rates, partial, fingerprint flip).
  All single-threaded synchronous; latency is expressed in **ticks**.
- `orch-driver` holds the generated input-synth adapter, the
  fingerprint guard, `build_propose_bursts_request`, and
  `OrchNodeAttrsV1` + `build_input_synth_node_context` — M3's synth
  stage and M4's attrs plumbing reuse these as-is.
- `orch-fakes/tests/search_loop.rs` is a working hand-rolled
  select→expand→score→commit loop — the reference for the pipeline's
  semantics and the seed for M3's integration tests.

## Files in this plan

| File | Contents |
|---|---|
| `01-decisions.md` | The six decision points (crate layout, concurrency model, tonic version, served-proto authorship, chaos-test shape, observatory boundary) with rationale — reviewable before code |
| `02-m3.md` | M3 work items W3.0–W3.6 with per-item acceptance mapping |
| `03-m4.md` | M4 work items W4.1–W4.9 with per-item acceptance mapping |
| `04-verification.md` | Test matrix vs. the quoted accept bars, evidence conventions, CI changes, staged-out items |

## Review status

Two independent reviews completed 2026-07-06 (contract-fidelity lens;
feasibility/test-design lens). Both endorsed decisions D1–D6 and the
bar→test traceability. All accepted findings are folded into these
files: fault-injector attempt salt (W3.0a), lease reclamation (W4.4a),
grid-world refactor promoted to W4.2a, `validate_all()`, adapter
sleep/lock/probe mechanics + tokio footguns (D2), fork discipline and
`at_frame` strict-future base (W3.2), utilization sensitivity control
(W3.6), checkpoint-on-goal + full event vocabulary (W4.3), event-hash
exclusions (D6), Tier-2 whole-world persistence scope + descope
trigger (D5), CI dedup, `bins/*` workspace member, `"boot"` RNG tag.

## Sequencing

```
W3.0 (workspace prep: tonic unification, orch-driver grpc feature
      gate, orch-sched skeleton) + W3.0a (fake fault attempt salt)
  → W3.1 slots → W3.2 driver → W3.3 pipeline → W3.4 retry → W3.5 modes
  → W3.6 M3 acceptance suite  ──────────────── M3 done, gate for M4
W4.1 (EventSink boundary — can start parallel to late M3)
W4.2 orch-checkpoint → W4.2a grid-world refactor
  → W4.3a bring-up/bootstrap → W4.3b loop+commit+events
  → W4.3c checkpoint lockstep+pause/stop → W4.4 resume + W4.4a lease
  reclamation → W4.5 ladder wiring → W4.6 proto+server+bin
  → W4.7 YAML/standalone → W4.8 M4 acceptance suite
  → W4.9 docs/README/resolution
```

One commit per work item (or small coherent group) on `main`, SHAs
recorded in `04-resolution.md` per request `03-verification-offer.md`.

## Tracking (beads)

Created when implementation starts, per the request's suggested shape:
parent epic + one bead per milestone; add one bead per W-item only if
sessions end mid-milestone. Known disclosed-bead candidates (see
`04-verification.md`): aarch64 local-only leg if no runner exists at
handback; true-SIGKILL harness tier if reviewers accept the in-process
crash-lattice as the primary vehicle.

```bash
PARENT=$(bd create "Phase 5 entry: land M3+M4 runner-on-fakes" \
  -d "Request .agents/requests/phase5-entry-m3-m4-runner-on-fakes/. Plan .agents/plans/phase5-entry-m3-m4-runner-on-fakes/. M3 orch-sched + M4 ExperimentRunner per IMPLEMENTATION-PLAN, acceptance bars verbatim." \
  -p 1 -l impl -t epic --silent)
M3=$(bd create "M3: orch-sched worker driver, S-E-C pipeline, retries" \
  -d "Plan items W3.0-W3.6. Lease composition, SlotView, bounded queues, retry policy, fast+det modes. Accept bars in request 02. Includes re-exercising the input-synth context contract through the real expansion path." \
  -p 1 -l impl --silent)
M4=$(bd create "M4: ExperimentRunner, checkpoint/WAL/resume, gRPC surface" \
  -d "Plan items W4.1-W4.9. Runner loop, bring-up, plateau ladder, CheckpointV1+WAL with archive lockstep, six RPCs, event emission, standalone YAML. Accept bars in request 02." \
  -p 1 -l impl --silent)
bd dep add $M4 $M3
bd dep add $PARENT $M3; bd dep add $PARENT $M4
```

## Out of scope (explicit)

- M5 hardening (config-matrix, full Prometheus surface, CAS
  ownership-loss path exercised under contention, 24 h soak) and M6
  real-substrate work.
- Redesigning anything in `orch-core` — pure-core boundaries and the
  `purity_guard` allowlist are unchanged; `orch-clients`/`orch-fakes`
  stay free of tokio/tonic/filesystem/wall-clock.
- Upstreaming the served proto into control-plane (disclosed follow-up,
  see `01-decisions.md` D4).
