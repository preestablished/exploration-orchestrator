# exploration-orchestrator

Deterministic exploration orchestrator (Phase 5 of the determinism
platform). Pure-Rust search core; everything through M4 runs against
in-repo fakes with zero platform dependencies.

## Crate map (post-M4)

- `orch-core` — pure synchronous search logic: config/value types (with
  `validate()` / accumulating `validate_all()`), feature-map compilation
  and L4 coarsening, tree/frontier/cell-mirror/seen state, commit rules,
  plateau/stall/escalation-ladder state machines, deterministic ChaCha12
  RNG substreams, and selection policies. No tokio/tonic/fs/wall-clock —
  enforced by `purity_guard`.
- `orch-clients` — transport-free DTOs and sync client traits for the five
  service boundaries (hypervisor worker, state scorer, snapshot store,
  input synthesizer, observatory event sink).
- `orch-fakes` — deterministic service fakes: data-driven grid worlds
  (`three_room` boss+credits, `corridor_hidden_key` plateau fixture),
  hypervisor with fork/freeze and lease-reclamation semantics, scorer with
  batch-id dedup + archive checkpoint/restore/replay, snapshot store with
  metadata generation-CAS, synthesizer, recording observatory, and the
  seed-pure fault injector (latency/error/timeout/partial/fingerprint-flip
  with a per-(target, operation) attempt salt). Single-threaded and
  synchronous; latency is expressed in ticks.
- `orch-proto` — wire types: re-exports the control-plane inputsynth proto
  and generates the locally authored `determinism.orchestrator.v1` service
  (see `protos.lock` for provenance; upstreaming is a disclosed follow-up).
- `orch-driver` — the input-synth request builders, bring-up composition,
  fingerprint registry/guard, and node-attrs envelope. **Naming drift
  note:** despite the name, this crate is *not* ARCHITECTURE.md's "worker
  driver" — that lives in `orch-sched/src/driver.rs`. `orch-driver`'s
  tonic adapter is behind the default-off `grpc` feature so trait-generic
  consumers never link tonic.
- `orch-sched` — M3: async ports + `SyncAdapter` (sleep-before-lock
  virtual-latency model, 1 tick = 1 virtual ms), `SlotView` slot
  accounting with determinism-class gating, the worker driver (lease
  composition per API.md §2.2, fork discipline, verdict mapping,
  bootstrap), the bounded S→E→C pipeline, and the retry policy.
- `orch-checkpoint` — M4, pure: `CheckpointV1` + WAL `ExpansionIntent`
  encode/decode/validate (postcard, golden-pinned).
- `orch-server` — M4: the `ExperimentRunner` loop (bring-up, bootstrap,
  WAL journaling, commit + store writes + observatory events, plateau
  ladder wiring incl. L4 re-bin, budgets, pause/stop, §8 checkpoint
  lockstep, §8.2 resume with replay adoption, crash-lattice hooks), the
  proto↔core config conversion + `config_hash`, the standalone-YAML path,
  and the served `ExplorationOrchestrator` tonic surface.
- `bins/orchestratord` — the daemon: `--simulate` serves gRPC over the
  fake world (`/healthz`, `/metrics`, SIGTERM drain); `--experiment
  <file.yaml>` runs standalone (`run_id = experiment_id`). Example configs
  in `bins/orchestratord/examples/`.

## Concurrency model (plan decision D2)

The sync client traits in `orch-clients` remain the contract source of
truth. `orch-sched` defines thin async ports plus a `SyncAdapter<T>` that
lifts any sync implementation onto tokio: virtual latency (from the fakes'
tick-denominated fault plans, via a constructor-supplied `LatencyProbe`)
is charged by sleeping *before* the service lock, so K in-flight calls
genuinely overlap in virtual time and state changes land at the response
instant. All tests run on a current-thread tokio runtime with paused time:
bit-deterministic, while exercising the real async pipeline. Real
transport adapters replace `SyncAdapter` behind the same ports at M6.

## Validation

```bash
cargo test --workspace --all-features   # everything, incl. the gated wire tests
cargo test -p orch-core --test purity_guard
cargo test -p orch-server --test seed_gate   # the CI determinism gate
./scripts/evidence-m3m4.sh              # the M3+M4 acceptance evidence pass
```

The M3 acceptance suite lives in `crates/orch-sched/tests/` (utilization,
backpressure, retry equivalence, shrink/grow, expansion context) and the
M4 suite in `crates/orch-server/tests/` (autonomy, plateau ladder, Tier-1
chaos lattice, seed gate, fast-mode trajectory replay, pause/resume, gRPC
surface). `CHAOS_SEED` / `CHAOS_SEEDS_PER_POINT` widen the chaos lattice.

## Running the daemon

```bash
cargo run -p orchestratord -- --experiment bins/orchestratord/examples/grid-smoke.yaml
cargo run -p orchestratord -- --simulate --listen 127.0.0.1:7130 --http 127.0.0.1:7131
```
