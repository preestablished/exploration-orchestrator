# Decision Points

Request `02-…` §Sequencing item 2 asks for placement/drift decisions to
be stated and reviewable. These six decisions shape everything in
`02-m3.md`/`03-m4.md`. Each records the chosen option, the rejected
alternative, and what would force a revisit.

## D1 — Crate layout: follow ARCHITECTURE.md §1 names

**Decision:** create `orch-sched` (M3), `orch-checkpoint` (M4, pure),
`orch-server` (M4, runner + tonic surface), and `bins/orchestratord`
(M4), exactly as ARCHITECTURE.md §1 names them. `orch-driver` keeps its
current, narrower role — the input-synth gRPC adapter + node-attrs
envelope — and is *documented* as such (README + a note in
`04-resolution.md`) rather than renamed or absorbed.

**Rejected:** extending `orch-driver` into a catch-all runner crate.
It would put the served tonic server, the checkpoint logic, and a
client adapter in one crate, blur the dependency rules (ARCHITECTURE.md
§1 wants sched/server depending on traits, not gRPC impls), and make
the doc drift worse instead of better.

**Note on drift:** ARCHITECTURE.md's `orch-sched/src/driver.rs` is the
*worker* driver (hypervisor composition) — unrelated to the existing
`orch-driver` crate name. The worker driver lands in
`orch-sched/src/driver.rs` per the doc; the name collision is
README-documented.

## D2 — Concurrency model: tokio in orch-sched/orch-server, sync traits kept, virtual time on fakes

The repo grew **sync** client traits (`orch-clients`), single-threaded
fakes whose injected latency is expressed in **ticks**, while
ARCHITECTURE.md §6 assumes tokio mpsc pipelines and the M3 accept bar
says "loom or tokio-test exercised on the lease path".

**Decision:**
- `orch-clients` traits stay sync and remain the contract source of
  truth (no churn to M1–M2 surfaces, purity guard untouched).
- `orch-sched` defines its own thin async ports (one async trait per
  boundary it drives: hypervisor, scorer, synth, store) plus a
  `SyncAdapter<T>` that wraps any sync `orch-clients` impl. The adapter
  maps the fakes' `FaultDecision::latency_ticks` to `tokio::time::sleep`
  — 1 tick = 1 virtual millisecond.
- Tests run on a **current-thread tokio runtime with paused time**
  (`tokio::time::pause()`): K jobs overlap in virtual time, execution
  is serial in real time, so every M3 test is bit-deterministic while
  still exercising the real async pipeline code. Utilization,
  jitter (±50%), and latency histogram bars are measured in virtual
  time. `tokio-test` covers the lease/shrink-grow deadlock bar (loom is
  overkill for mpsc + a slot semaphore; revisit only if a real
  interleaving bug survives tokio-test).
- Production path (M6) swaps in real async gRPC impls behind the same
  ports on a multi-thread runtime; nothing in the pipeline changes.

**Rejected:** (a) a bespoke synchronous simulated-time scheduler — it
would be rewritten wholesale at M6 and would not satisfy the
"tokio-test on the lease path" bar in spirit; (b) converting
`orch-clients` to `#[async_trait]` — invalidates M1–M2 fakes/tests for
zero functional gain, since the adapter achieves the same seam.

**Concurrency vs. `&mut self` fakes:** the fakes are one logical
server; the adapter holds them behind a single async mutex. Under
paused time this serializes *execution* but not *virtual overlap* —
provided the mechanics below are followed. Slot capacity (8 default
slots) is still enforced by the fake itself.

**Adapter mechanics (review findings, binding):**
- **Sleep outside the lock, sleep *before* the call:** the adapter
  sleeps the virtual latency first, then takes the lock and makes the
  (instant) sync call — so the fake's state change lands at the
  *response* instant, which is what a caller of a real endpoint
  observes, and K jobs genuinely overlap in virtual time. Sleeping
  inside the lock would serialize virtual time (K·L) and fail the
  utilization bar structurally.
- **Latency is not observable through the sync traits** — it surfaces
  only via each fake's inherent `last_fault()`. `orch-sched/src`
  therefore defines a small `LatencyProbe` seam (constructor-supplied
  closure/trait); the fake-specific probe impls live in the **test
  tree**, so `orch-sched` never depends on `orch-fakes` outside
  dev-deps. Since latency must be known *before* the call
  (sleep-first), the probe pre-computes the pending decision (fakes'
  fault decisions are pure functions of the request — same derivation,
  computed ahead).
- **Timeout faults pay their duration:** a `FaultTerminal::Timeout`
  today returns `DeadlineExceeded` instantly; the adapter charges the
  caller's configured timeout in virtual time before surfacing it, so
  retry/backoff timing and the backpressure test are realistic.
- **Tokio determinism footguns:** the pipeline uses no unbiased
  `tokio::select!` (thread-RNG branch order — use `biased;` or avoid
  `select!`); task spawn order is fixed; `tokio = { features = [...,
  "test-util"] }` is required for `time::pause()` (currently absent —
  W3.0).

## D3 — Tonic version unification: workspace moves to 0.14

`Cargo.toml` pins `tonic = "0.12"`, but `determinism-proto` generates
against tonic 0.14.6/prost 0.14. `orch-driver` currently straddles
both. Adding a *served* surface on 0.12 while clients speak 0.14 means
two hyper/h2 stacks in one binary.

**Decision:** first work item (W3.0) bumps workspace `tonic` to 0.14.x
(matching determinism-proto), fixes `orch-driver`'s transport imports,
and verifies `cargo test --workspace` + `purity_guard` (orch-core never
sees tonic, so the guard is unaffected). Done before any M3 code so the
skew never lands in new code.

## D4 — Served proto: author `determinism.orchestrator.v1` service here

Upstream `determinism-proto`'s `orchestrator::v1` is hand-written Rust
containing only a `StartExperimentRequest{experiment_id, spec:
ExperimentSpec}` — no service, no tonic codegen, and a *different
StartExperimentRequest shape* than API.md §1.

**Decision:** author the full `ExplorationOrchestrator` proto (six
RPCs + ExperimentConfig message per API.md §1/§7) in this repo —
`orch-proto/protos/determinism/orchestrator/v1/orchestrator.proto`
with a `build.rs` (tonic-prost 0.14, server + client codegen). This is
exactly the fallback IMPLEMENTATION-PLAN §M0 already sanctions ("author
only the served `determinism.orchestrator.v1` here per API.md §1,
upstreaming later"). The name collision with upstream's placeholder
module is contained by keeping our generated module under
`orch_proto::orchestrator_v1` and *dropping* the re-export of
upstream's `orchestrator` module (it has no consumer in this
workspace). Upstreaming + reconciling `ExperimentSpec` vs
`ExperimentConfig` is filed as a disclosed bead at handback — it needs
a control-plane-side change and is not on the M4 critical path.

**Config mapping:** proto `ExperimentConfig` converts to/from
`orch_core::types::ExperimentConfig` (the validation authority);
`config_hash` = blake3 over the *effective* (defaults-materialized)
canonical proto bytes, per API.md §7.

## D5 — "Kill -9 anywhere" bar: two tiers, crash-lattice primary

The fakes are in-memory; a real SIGKILL vaporizes the "durable" store,
so the bar cannot be run literally against stock fakes.

**Decision:**
- **Tier 1 (primary, exhaustive): in-process crash-lattice.** The
  runner takes an injectable `CrashPolicy` (test-only) that aborts the
  run at named crash points — every point in the loop where state
  visibility changes: after WAL write / after dispatch / mid-batch /
  after CreateNode / before WAL delete / after WAL delete / mid-
  checkpoint (before and after `CheckpointArchive`, before and after
  the CAS put) / after commit before checkpoint. The harness keeps the
  fakes alive, constructs a **fresh runner instance** (no carried
  state) against the surviving fakes, resumes per §8.2, and asserts the
  full invariant list from the accept bar. 50+ randomized-seed runs ×
  systematic sweep of every crash point. This is *stronger* than random
  SIGKILL sampling: every crash point is hit deterministically.
- **Tier 2 (fidelity): true SIGKILL harness.** A test-support
  persistence wrapper (living in the M4 test tree, NOT in `orch-fakes`,
  which stays fs-free) journals the fake store + scorer archive to a
  temp dir; `orchestratord --simulate` runs against it; the harness
  SIGKILLs the child process at random wall-clock points across 50
  runs and re-launches. This proves the process-level claim (signal
  handling, no reliance on in-memory runner state) end-to-end.

**Tier-1 prerequisite — lease reclamation (review finding, binding):**
a crashed runner's leases are unreclaimable today: `DestroyVm`
requires the dead runner's exact `Lease{slot_id, token}`, the fake has
no expiry, and a frozen fork-parent unfreezes only when its children
are destroyed — so crash points after dispatch would permanently leak
up to K of 8 slots and wedge the lattice. Fix (W4.4a): model
**session-teardown reclamation** in FakeHypervisor — a
`reclaim_session()` hook the harness invokes on crash (standing in
for the real worker observing its client connection drop), which
destroys live leased slots child-first and unfreezes parents.
Faithful to the real system's direction (worker survives orchestrator
death and must reclaim); the real hypervisor's actual orphan-lease
semantics are its owner doc's territory — disclosed in
`04-resolution.md` and re-verified at M6.

**Tier-2 true scope (review finding):** journaling "store + scorer"
is not enough — all snapshots live in FakeHypervisor's in-memory map,
and synth fingerprint state matters too. Tier 2 therefore requires
whole-fake-world persistence (hypervisor snapshots + slot table,
scorer archive + batch cache, store, synth state), with the journal
itself crash-consistent (append-only records, per-record checksums,
truncate-torn-tail on reload). Sized honestly as its own work item.
**Descope trigger, pre-agreed:** if at the M4 midpoint Tier 2
threatens the critical path, it moves to a disclosed bead and Tier 1
stands alone — the reinterpretation the request's evidence
conventions explicitly allow.

## D6 — Observatory boundary: `EventSink` trait in orch-clients + fake

**Decision:** add `orch-clients/src/observatory.rs` — transport-free
DTO `EventEnvelope` (per observatory's envelope contract: run_id,
source_service, producer_id, seq, ts_logical, event_type, payload)
plus a **sync** `EventSink` trait (`emit(&mut self, EventEnvelope)`),
matching the style of the other four boundaries; and
`orch-fakes/src/observatory.rs` — a recording fake with configurable
ack behavior. The *bounded ring, drop-oldest, never-blocks* semantics
(API.md §6) are implemented on the orchestrator side (`orch-server`
emitter), not in the trait — the trait models the wire, the emitter
models the producer rules. Event-sequence determinism (the M4 bar)
is asserted over the fake's recorded envelope list.

**Payload note:** payload schemas are observatory's contract; on fakes
we emit the v1 vocabulary (API.md §6 list) with the documented field
shapes as serde-serialized maps. No proto dependency needed until a
real observatory exists (its determinism-proto feature carries no
tonic today anyway).

**Event-sequence determinism hash (review finding):** API.md §6 makes
`producer_id = "orchestratord-<startup_unix>"` wall-clock-derived and
restarts `seq` per session — both nondeterministic run-to-run by
design. The seed-gate's "identical event sequence" hash is therefore
defined over `(ts_logical, event_type, canonical payload)` per
envelope, **excluding** `producer_id`/`seq`; the exclusion is
disclosed in `04-resolution.md`. Test harnesses additionally inject a
deterministic producer identity so full-envelope golden tests stay
possible where wanted.
