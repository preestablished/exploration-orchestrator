# Decision Points

Same template as the M3/M4 plan: chosen option, rejected alternative,
revisit trigger. D-P* shape `02-proto-upstream.md`; D-T* shape
`03-tier2-chaos.md`.

## D-P1 — Cutover shape: `orch-proto` stays, as a pure re-export shim

**Decision:** after upstreaming, `crates/orch-proto/src/lib.rs` becomes:

```rust
pub mod inputsynth {
    pub use determinism_proto::inputsynth::v1;
}
pub mod orchestrator_v1 {
    pub use determinism_proto::orchestrator::v1::*;
}
```

(`pub use …::*` re-exports the generated nested modules —
`exploration_orchestrator_server`, `exploration_orchestrator_client`,
`progress_event`, etc. — so every existing `orch_proto::orchestrator_v1::…`
path in `orch-server`, `orchestratord`, and `grpc_surface.rs` keeps
compiling unchanged.) `orch-proto/protos/` and `orch-proto/build.rs` are
**deleted**; `protos.lock` is rewritten to record that both families are
now consumed from the canonical control-plane tree (it is referenced by
bead 777 and the M3/M4 resolution — rewrite, don't delete).

**Rejected:** repointing the four consumer crates directly at
`determinism_proto::orchestrator::v1` and deleting `orch-proto`. Slightly
"cleaner" but churns five files across three crates for zero behavior
change, and `orch-proto` remains the natural seam for any future
repo-local wire helpers. Revisit at M6 if the crate is still an empty
shim then.

## D-P2 — buf lint posture: keep the wire shape, exempt the two unfixable rules

The control-plane request names three lint rules (package version
suffix — we pass, `v1` — enum zero-values, service suffix), but their
gate will run a whole DEFAULT-category lint ("expect the existing ~21
placeholder stubs to need cleanup or exemption"). Against DEFAULT, our
file violates three rules (the rest of DEFAULT passes — request/response
uniqueness, enum value prefixes, package/directory match, casing all
verified clean):

- `ENUM_ZERO_VALUE_SUFFIX` — all five enums have semantic zero values
  (`EXPERIMENT_STATE_PENDING = 0`, `PRUNE_ACTION_EXHAUSTED = 0`,
  `ON_GOAL_STOP = 0`, `POLICY_KIND_SOFTMAX = 0`, `SCHED_MODE_FAST = 0`),
  not `*_UNSPECIFIED = 0`.
- `SERVICE_SUFFIX` — the service is `ExplorationOrchestrator`, not
  `…Service`, fixed by API.md §1.
- `RPC_RESPONSE_STANDARD_NAME` — twice: `GetExperimentStatus` returns
  `ExperimentStatus` (deliberately, because the same message is embedded
  in `ProgressEvent.status`; renaming it `GetExperimentStatusResponse`
  would be a semantic lie there, and wrapping it in a new response
  message *is* wire-breaking) and `StreamProgress` returns
  `stream ProgressEvent` (an event stream, not a response envelope).

**Decision:** neither is fixable within the request's own constraint
("style fixes … wire-compatibly"):

- Renumbering to insert `*_UNSPECIFIED = 0` changes wire values — a
  breaking change to the served surface.
- Renaming the zero values to `*_UNSPECIFIED` *is* wire-compatible
  (proto3 enum names are not on the wire) but is a semantic lie: `PENDING`,
  `STOP`, `SOFTMAX`, `FAST` are the real, documented defaults (API.md §7
  defaults land on the zero value deliberately), and the rename churns
  generated Rust variant names through `orch-server`'s conversions.
- Renaming the service changes gRPC method paths
  (`/determinism.orchestrator.v1.ExplorationOrchestrator/…`) — wire-breaking.

So the upstream PR ships `buf.yaml` `ignore_only` entries (or the
equivalent under whatever config their item 1 lands) for
`ENUM_ZERO_VALUE_SUFFIX`, `SERVICE_SUFFIX`, and
`RPC_RESPONSE_STANDARD_NAME`, scoped to
`proto/determinism/orchestrator/v1/orchestrator.proto`, with this
rationale in a comment. Control-plane's request already accepts
exemptions as a mechanism ("cleanup or exemption").

**Escape hatch, pre-agreed:** if control-plane's review insists on full
conformance *before the tag*, renumbering is uniquely cheap right now —
no tag exists and no external consumer exists. But it is **not**
persistence-free: the production `config_hash` is blake3 over the
canonical **proto** encoding (`orch-server/src/config.rs:302`,
`blake3(to_wire(config).encode_to_vec())` — the postcard-over-core-types
hash is only the test helper in `tests/support/mod.rs`), and that hash
is persisted in `CheckpointV1.config_hash` and checked on resume
(`orch-checkpoint/src/lib.rs:93,193`). Renumbering therefore changes the
`config_hash` of any config carrying a non-default enum value — no
checkpoint (including a Tier-2 state-dir) may straddle the renumber.
That's acceptable today (fakes-only, no long-lived state-dirs, no
pinned hash literals in tests), plus the bounded code fallout: proto
enum blocks, `orch-server/src/config.rs` + `service.rs` wire↔core enum
conversions, `grpc_surface.rs` expectations. That path is a *disclosed
deviation* to record in the resolution, taken only on their explicit ask.
Ditto a service rename — but push back hard on that one, since API.md
names the service and their own placeholder already used the un-suffixed
name.

## D-P3 — Mirror scope: descriptor equality covers `ExperimentConfig` and its full closure

Control-plane's item 4 mandates `ExperimentSpec` mirror
`ExperimentConfig` field-for-field, enforced by a descriptor-equality CI
check they own. Our request's obligation: "do not merge the upstream
without agreeing which message shapes the mirror covers."

**Decision:** the agreement note (W1.1) proposes the mirror covers the
**full transitive closure of `ExperimentConfig`**: `ExperimentConfig`
itself plus `Budgets`, `SelectionConfig`, `StagedConfig`, `BurstConfig`,
`PlateauConfig`, `LadderConfig`, `SchedulingConfig`, `CheckpointConfig`,
and enums `PruneAction`, `OnGoal`, `PolicyKind`, `SchedMode` — names,
types, and field numbers all matched, orchestrator side the source of
truth, divergence fixed control-plane-side. Whether their
`ExperimentSpec` *embeds* `determinism.orchestrator.v1.ExperimentConfig`
by import (which makes the equality check trivial and drift structurally
impossible) or duplicates the messages under `controlplane/v1` is their
call to make in their item 4 — the note states we're fine with either
and mildly prefer embedding. Note: today's `controlplane/v1` `Budgets`
and `BurstParams` are a different shape (e.g. `max_wall_clock_secs`,
`guest_seconds_per_job`); the mirror rework replaces them — their tree,
their commit.

**Not merged until:** the note is acknowledged in their request dir (a
one-line reply suffices). The upstream itself does not depend on the
mirror landing — only on the *scope agreement* existing, so a slow
mirror rework does not block W1.2–W1.5.

## D-T1 — Crate placement: new `crates/orch-simstate`; `orch-fakes` stays fs-free

Tier-2 needs the persistence wrapper in two places the M4 test tree
can't serve: the `orchestratord` binary (bead 6ft: "`orchestratord
--simulate` wired to it") and the harness. D5 is binding that
`orch-fakes` stays free of filesystem/wall-clock.

**Decision:** new workspace crate `crates/orch-simstate` — the
crash-consistent journal, the four `Persistent*` wrappers, the reload
path, and the shared comparator. Depends on `orch-clients`, `orch-fakes`,
`orch-core`, `orch-driver` (for `decode_node_attrs` in the comparator),
postcard, blake3. The SIGKILL harness lives in
`bins/orchestratord/tests/tier2_chaos.rs` — integration tests of the bin
package get `CARGO_BIN_EXE_orchestratord`, so the harness always drives
the exact binary under test with zero path guessing.

**Rejected:** (a) persistence inside `orch-fakes` — violates the binding
D5 boundary; (b) wrapper as a private module of `orchestratord` — the
comparator and journal unit tests would be unreachable from anywhere
else, and Tier-1's `chaos_resume.rs` couldn't share the extracted
comparator; (c) harness as a standalone script — loses `cargo test`
integration and the CI story.

## D-T2 — Journal design: write-ahead op log, replay by re-invocation, response digests

There is no durability anywhere today: checkpoint + WAL bytes live in
`InMemorySnapshotStore.metadata` (a `HashMap` behind `PutMetadata`
generation-CAS), and all four fakes are pure in-memory maps. The journal
*is* the durability layer, so its crash consistency is the thing under
test.

**Decision:** one append-only journal file per state-dir
(`<state-dir>/journal.v1`), shared by all four wrappers behind a
`Mutex<Journal>`. Note execution is **not** serialized at op
granularity even in deterministic mode: `run_batch` fans the K sibling
jobs out as concurrent `tokio::spawn` tasks
(`orch-sched/src/driver.rs:342-362,484-502`), so ops on *different*
services interleave. What holds is per-service atomicity — each
wrapper's journal-append + apply happens while that service's
`SyncAdapter` mutex is held — and per-fake state only depends on that
fake's own op order, so the journal's total order is a valid
linearization for replay. Record frame:

```
u32 LE payload length | u64 LE truncated blake3 of payload | payload
```

payload = postcard-encoded `JournalRecord` enum: one variant per
**mutating** client-trait method across the four boundaries. The rule
is mechanical and exact for all four traits: **journal every `&mut self`
trait method, never journal `&self` methods** — synth's
`propose_bursts`/`mine_macros` are `&mut` but pure, journaling them is
harmless; `&self` queries never mutate. Two additional hard rules:
**errored ops are journaled too** (e.g. the hypervisor's `run()` mutates
slot state and pushes watch events even on error), and **ops issued
from background tasks are never journaled** — `list_slots` /
`watch_slots` / `worker_info` fire from the SlotView drain task on a
5 ms real-time timer (`orch-sched/src/slots.rs:208-219`) at
nondeterministic instants; they're `&self` so the rule already excludes
them, but don't "improve" on it. Each op frame carries the request DTO
and a monotonically assigned `op_id` (issued under the journal mutex).

Write path (write-ahead, all under the service's mutex): append op
frame → `File::sync_data()` → apply to the in-memory fake → append an
advisory `Applied { op_id, digest }` frame (no fsync — losing it is
fine) → return the fake's response. The digest is a `u64`
truncated-blake3 of the postcard-encoded response; since `ClientError`
has no serde derives (`#[non_exhaustive]` kind), digest errors via a
small local mirror in `orch-simstate` (kind-as-string + message). A
SIGKILL between append and apply is indistinguishable from "server
executed, response lost" — exactly the crash semantics real clients
face. Reload path: rebuild fresh fakes from `GridWorld::three_room()`,
scan the journal verifying length + checksum per frame, **truncate the
torn tail** (`set_len` + sync) at the first short/corrupt frame, then
re-invoke every op in order, pairing each op frame with its `Applied`
frame **by `op_id`** (never "the next frame" — frames from other
services interleave); where the pair exists, the re-invoked response's
digest must equal it — a mismatch is a loud panic naming the op id
(the tripwire for any hidden nondeterminism in the fakes); a missing
`Applied` frame means the crash landed between append and apply, and
the re-invoked result is authoritative. The dir is fsynced at journal
creation so the file itself survives. Known benign wart to document:
`FakeHypervisor::watch_slots` advances a cursor through a `Cell`
despite `&self`, so after reload the drain task re-sees old events —
convergent (last-write-wins into `SlotView.known`) and invisible to
the digest check, but worth a comment so nobody chases it as a bug.

**Rejected:** (a) journaling full state snapshots per op — O(world) per
write, and it wouldn't exercise replay determinism at all; (b) journaling
responses in full and *installing* them instead of re-invoking — hides
exactly the class of bug Tier-2 exists to catch; (c) no fsync — the
harness kills at torn-write boundaries on purpose; without fsync the
"forced mid-append" points are meaningless. Compaction is deliberately
absent — runs are grid-sized; revisit only if evidence-lane runtime
forces it (disclose if so).

## D-T3 — Kill mechanics: lattice via hang-hooks, torn writes via hold-hooks, plus random timer kills

Random wall-clock SIGKILL sampling alone cannot guarantee the accept
bar's coverage ("all Tier-1 lattice points plus forced
mid-checkpoint-write and mid-WAL-append").

**Decision:** three kill classes, all ending in a real `SIGKILL` from
the harness (never a cooperative path):

1. **Lattice points:** `orchestratord` grows an env-gated hook —
   `ORCH_CHAOS_HANG_AT=<CrashPoint>:<nth>` (debug feature, documented as
   test-only) — that builds a `CrashPolicy` whose `should_crash`, on the
   nth arrival at the named point, prints a marker line
   (`TIER2_CHAOS_HANG point=<…>`) to stdout, flushes, and parks the
   thread (`loop { thread::sleep }`). The harness watches child stdout
   for the marker, then SIGKILLs. This reuses the existing 11
   `CrashPoint` sites unchanged and turns each into a true-SIGKILL
   point. Honored in `--experiment` (standalone) mode; the gRPC-served
   path keeps `None` (see D-T4).
2. **Forced torn writes:** `orch-simstate` honors
   `ORCH_SIM_TORN_AT=<kind>:<nth>` where `<kind>` ∈
   `{wal-append, ckpt-put}` (matched against `put_metadata` records whose
   key is a WAL / checkpoint key respectively): on the nth match it
   writes a **prefix** of the frame, syncs, prints the marker, and parks
   — the harness SIGKILLs, and reload must land exactly on the
   torn-tail-truncation path. This is the request's "mid-WAL-append" and
   "mid-checkpoint-write", made deterministic.
3. **Random kills:** the harness also runs per-seed rounds that sleep a
   random real-time interval and SIGKILL wherever the child happens to
   be. If the child exits first, the round counts as a (redundant)
   control run, not toward the ≥50 kills — the harness loops until the
   kill quota is met.

**Rejected:** promoting `CrashPolicy` erroring (Tier-1 style) into the
bin and calling `abort()` — a process *exit* is not a SIGKILL landing at
an arbitrary instruction with OS-level suddenness, and it would skip the
journal-torn-write cases entirely.

## D-T4 — Harness drive mode: standalone primary, one served-gRPC resume smoke

**Decision:** the kill matrix drives `orchestratord --experiment
<yaml> --experiment-id tier2 --state-dir <dir>` — deterministic
completion signal (exit code 0 + persisted status `GoalReached`), no
client choreography per incarnation. Config mirrors the Tier-1 grid
tuning (`support::grid_config`: deterministic mode, `max_inflight_batches
= 1`, `every_commits = 16`, small bursts) expressed as the standalone
YAML that `wire_config_from_yaml` accepts. `FaultPlan` stays disabled —
and this is a **soundness invariant of the journal, not a tuning
choice**: `FaultInjector::decide` advances a per-`(target, operation)`
attempt counter on every call *including read-only ops*
(`orch-fakes/src/fault.rs:396-420`), and read-only ops are not
replayed, so any nonzero plan would diverge under replay. With the plan
disabled all decisions are counter-independent. State this in
`orch-simstate`'s docs: the wrappers require a disabled fault plan.

One additional smoke case runs `--simulate --state-dir`, starts an
experiment over gRPC, random-kills the process, relaunches, and calls
`StartExperiment(resume_if_exists = true)` — proving the *served* resume
path works over a real process death too. It's one case, not a matrix:
the runner underneath is identical; only the entry point differs.

**Resume-side session reclaim:** Tier-1 calls
`FakeHypervisor::reclaim_session()` after each crash (the worker
observing its client connection drop). Tier-2's equivalent happens at
reload: after replaying the journal, the reload path invokes
`reclaim_session()` on the rebuilt hypervisor **and appends it as a
journal record** (so the next incarnation's replay reproduces it in
order). Same stand-in, same disclosure as W4.4a.

## D-T5 — CI shape: reduced smoke in CI, full matrix as the evidence lane

The full bar (≥5 seeds × (11 lattice + 2 torn + random) ≥ 50 kills, each
kill a real process launch in real time — no `tokio::time::pause()`
across a process boundary) has an unknown wall-clock cost until W2.4
measures it.

**Decision:** the harness scales by env (`TIER2_SEEDS`,
`TIER2_RANDOM_KILLS`, default small). CI gets a dedicated job running
the reduced smoke: 1 seed, 3 lattice points (`AfterWalWrite`,
`BeforeCasPut`, `AfterCasPut`), both torn-write kinds, 2 random kills,
the gRPC smoke, and the negative control — every *mechanism* exercised
on every push, on both arches. The full matrix runs via
`scripts/evidence-tier2.sh` (same discipline as `evidence-m3m4.sh`) and
lands under `evidence/phase5-tier2-chaos/`. **If** the measured full
runtime is under ~10 minutes, promote it into CI and say so in the
resolution; otherwise the evidence lane is the documented manual lane
the request explicitly allows ("say which and why"). Expectation to
plan around: fsync-per-mutating-op dominates — seconds to tens of
seconds per incarnation on CI disks, plausibly 15–40 min for the full
lane (65 forced rounds × multiple incarnations + randoms), a few
minutes for the ~9-round smoke. Batching syncs is **not** an acceptable
mitigation (it would defeat the torn-write points); the only knob is
matrix size.

**Rejected:** full matrix in CI sight-unseen (could double CI time for a
lattice that Tier-1 already sweeps exhaustively in-process), or
CI-skipping Tier-2 entirely (the mechanisms would rot).
