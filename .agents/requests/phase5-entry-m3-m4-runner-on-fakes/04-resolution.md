# Resolution: M3 + M4 landed (scheduler + runner on fakes)

Executed per `.agents/plans/phase5-entry-m3-m4-runner-on-fakes/`
(decisions D1–D6 as reviewed). All work-item commits on `main`, pushed;
`cargo test --workspace --all-features`, `clippy -D warnings`, and
`purity_guard` green at head. Evidence in `evidence/phase5-m3-m4/`
(produced by `scripts/evidence-m3m4.sh`).

## Commits (in order)

| SHA | Item |
|---|---|
| `efe169c` | W3.0 — tonic 0.14 unification, orch-driver `grpc` feature gate, CI clippy; absorbs upstream control-plane proto drift (`261141b` changed PadSegment/Burst/MiningParams shapes) |
| `c91bfa2` | W3.0a — fault-injector per-(target, operation) attempt salt + `peek` |
| `3c0b8c2` | W3.0 — orch-sched skeleton: async ports + `SyncAdapter` (D2 mechanics) |
| `7348572` | W3.1 — `SlotView` (+ `FakeHypervisor::set_slots_total` for shrink/grow) |
| `89ad83f` | W3.2 — worker driver, fork discipline, verdict mapping, bootstrap; `boot`/`entropy` RNG tags golden-tested |
| `1ee158a` (+`f47deb7`) | W3.3–W3.5 — pipeline, retry, modes, determinism smoke |
| `cba2a5d` | W3.6 — M3 acceptance suite (all five bars + sensitivity control) |
| `a72c101` | W4.1 — observatory boundary (`EventEnvelope`/`EventSink` + recording fake) |
| `3934e11` | W4.2 — orch-checkpoint (CheckpointV1 + WAL intent, golden-pinned) |
| `68e3bb5` | W4.2a — data-driven grid worlds (three-room + corridor-hidden-key) |
| `a442961` | W4.4a — `FakeHypervisor::reclaim_session` |
| `6e5d67a` | W4.3a–c + W4.5 — ExperimentRunner (bring-up, loop, lockstep, ladder incl. L4) |
| `3df1ca7` | W4.4 — §8.2 resume, checkpoint-scoped WAL, replay adoption; Tier-1 chaos lattice green |
| `33e0976` | W4.8 — autonomy (10 seeds), seed gate, fast-mode trajectory replay |
| `ff251b5` | W4.8 — plateau ladder A/B on the corridor world |
| `9fd2885` | W4.6 + W4.7 — authored proto, `validate_all`, served surface, `orchestratord`, standalone YAML, gRPC surface test |
| (this commit) | W4.9 — docs, evidence, CI seed gate, resolution |

## Decisions as landed

- **D1** — crates exactly as named: `orch-sched`, `orch-checkpoint`,
  `orch-server`, `bins/orchestratord`. `orch-driver` keeps its narrow
  role; the naming drift vs ARCHITECTURE.md's worker driver is
  README-documented.
- **D2** — sync traits kept; async ports + `SyncAdapter` with
  sleep-before-lock, `LatencyProbe` seam (probe impls in test trees),
  timeout-duration charging, paused-time tests, no unbiased `select!`.
  The M3 suite exercises the lease path under tokio-test
  (`shrink_grow.rs` polls an acquire explicitly).
- **D3** — workspace on tonic 0.14. Note: the sibling control-plane
  checkout had already published breaking phase-4 proto changes
  (PadSegment `{buttons, hold_frames}`, Burst field renumber,
  MiningParams plain scalars); W3.0 absorbed them — the pre-existing
  baseline no longer compiled against the moved sibling.
- **D4** — `determinism.orchestrator.v1` authored here
  (`orch-proto/protos/...`, tonic-prost 0.14, vendored protoc);
  generated as `orch_proto::orchestrator_v1`; upstream's placeholder
  module no longer re-exported; provenance in `protos.lock`.
- **D5** — Tier-1 in-process crash lattice is the primary vehicle
  (11 crash points × seeds, `CHAOS_SEED`/`CHAOS_SEEDS_PER_POINT`
  overrides). **Tier 2 (true SIGKILL + whole-world persistence) was
  descoped via the pre-agreed trigger** — filed as a bead (below).
- **D6** — `EventSink` in orch-clients + recording fake; bounded
  drop-oldest ring in orch-server's emitter; the seed-gate event hash
  covers `(ts_logical, event_type, canonical payload)` and excludes
  `producer_id`/`seq` (nondeterministic by the platform's contract);
  test harnesses inject a deterministic producer identity.

## Reinterpretations and named deltas (disclosed)

1. **"memory flat" (M3)** — asserted structurally (bounded queues,
   configuration-derived peak bounds), not RSS; RSS belongs to M5's soak.
2. **WAL lifecycle** — ARCHITECTURE.md §8 says "delete after the batch
   commits"; as sketched, that makes det-mode bit-identical resume
   impossible (commit rule 1's duplicate mirror bumps and route visit
   bumps live nowhere durable). Landed design: **the checkpoint is the
   WAL truncation point** (entries deleted after the CAS put; deletion is
   still strictly after commit), and resume replays every surviving
   batch through a replay-exact commit path that **adopts** children the
   original run already committed (identity: parent + producing burst
   id). Selection weights restore from the checkpoint and every
   post-checkpoint delta is recomputed, so the chaos bar's tree-hash
   equality holds at all 11 crash points. A crash inside the truncation
   window is repaired by finishing the truncation on resume.
3. **Replay duplicate verdicts** — during WAL replay the duplicate
   verdict derives from the orchestrator's own SeenMap (equal to the
   scorer's verdict on fakes, whose dedup is the same seen-set); fresh
   batches keep trusting the scorer verdict.
4. **Store visit/expand counters** — can double-count on replayed
   duplicate routes; diagnostic only, the checkpoint + replay is the
   selection-state authority.
5. **Plateau checkpoint shape** — stores the implementation's stall
   counters (complete resumable state of `StallDetector` /
   `EscalationLadder`) rather than the design sketch's score ring.
6. **Fingerprint pinning** — the synth config fingerprint is a function
   of the effective config, so L3 overrides legitimately change it; the
   checkpoint pins the base (no-overrides) profile and
   `FingerprintRegistry` guards per profile (phase-4 design).
7. **Grid cell keys** — now include the progress dimensions (keys,
   boss_hp), Go-Explore-faithful; without this no generated-input run
   could path back through visited cells after the key pickup (the old
   search_loop reference only solved the world via an ε_keep override).
8. **Corridor world geometry** — solvable without Down: FakeSynth's pad
   alphabet has no Down mask; the detour uses a climb shaft + key-cell
   return door, and the annex is score-flat via per-room axis weights.
9. **Shrink/grow surface** — pool size changes surface through
   `GetWorkerInfo::slots_total` refreshed on each WatchSlots drain (slot
   events carry no capacity field); `FakeHypervisor::set_slots_total`
   drives the drills.
10. **Initial checkpoint after bootstrap** — so a crash before the first
    cadence checkpoint resumes through §8.2 instead of re-bootstrapping
    over a non-empty store.
11. **sdk-event relay** — `assertion-violated`/`reachability-hit` relay
    is wired post-commit but the fakes emit no such events; exercised by
    payload-shape unit tests only, not integration (per plan).
12. **Contract-test posture** — fakes remain transport-free;
    `grpc_surface.rs` is the only served-endpoint test.
    IMPLEMENTATION-PLAN's risk-table mitigation (fakes behind real tonic
    endpoints) is deferred to M6 and must be revisited there.
13. **Stop semantics** — `abandon_inflight` drains on fakes (drain is
    bounded); the real cancel path is M6 work.
14. **named orch-core deltas** — `validate_all()` (accumulating),
    `RngPurpose::{Boot, Entropy}` + derive helpers,
    `PolicyContext::with_backtrack` (L3 κ·ν bonus application point),
    `CommitState::{from_parts, ensure_tracking}` +
    `update_parent_exhaustion` made public (resume/replay),
    `StallDetector::restore` / `EscalationLadder::restore`.

## Disclosed follow-up beads

Filed in the tracker at close:
- Upstream the authored `determinism.orchestrator.v1` proto into
  control-plane and reconcile `ExperimentSpec` vs `ExperimentConfig`.
- Tier-2 true-SIGKILL harness (whole-fake-world persistence wrapper,
  crash-consistent journal) — descoped per the D5 trigger.
  **Addendum 2026-07-08: gap closed.** Bead `exploration-orchestrator-6ft`
  is closed by the phase5-prep plan
  (`.agents/plans/phase5-prep-proto-upstream-and-tier2-chaos/`): the
  `orch-simstate` crash-consistent journal + `PersistentWorld` wrappers,
  `orchestratord --state-dir`, and the true-SIGKILL harness
  `bins/orchestratord/tests/tier2_chaos.rs` (11-point lattice + forced
  torn-WAL/torn-checkpoint writes + random kills + served-gRPC resume
  smoke + demonstrated negative control), with the full-matrix evidence
  in `evidence/phase5-tier2-chaos/` and the reduced smoke in CI
  (`tier2-chaos-smoke`). The Phase 5 gate-run checklist should cite this
  closure, not the descope. The proto-upstream bead
  (`exploration-orchestrator-777`) above is likewise closed —
  `determinism.orchestrator.v1` is canonical in control-plane and
  `orch-proto` is a re-export shim (see that plan's `04-resolution.md`).
- Fake lease-reclamation semantics (`reclaim_session`) pending the
  hypervisor owner-doc confirmation; re-verify at M6.
- M6 constraint: `GeneratedInputSynthClient`'s internal block_on Runtime
  must be replaced by an async transport adapter behind the ports before
  running inside the server.

## aarch64

CI runs the full suite plus the named seed gate on both `x86_64` and
`ubuntu-24.04-arm` matrix arms (`.github/workflows/ci.yaml`). Local
evidence in `evidence/phase5-m3-m4/` is x86_64; the arm leg is expected
from CI per the request's posture.

## Handback verification pointers (request `03-…`)

1. Cold re-run: `cargo test --workspace --all-features` + the named
   suites (see README "Validation") + `cargo test -p orch-server --test
   seed_gate` twice.
2. Chaos spot-check: `CHAOS_SEED=<fresh> cargo test -p orch-server
   --test chaos_resume`.
3. Expansion-path contract: `cargo test -p orch-sched --test
   expansion_context`.
4. Purity: `cargo test -p orch-core --test purity_guard`; the new
   observatory module in orch-clients/orch-fakes carries no
   tokio/tonic/fs/wall-clock deps.

`05-verification.md` is the phases track's step.
