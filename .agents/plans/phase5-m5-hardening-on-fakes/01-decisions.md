# Decision Points

Review these before coding. Each decision is chosen to satisfy the request while
staying inside the current fakes-first architecture.

## D5.1 - Keep two separate string taxonomies

**Decision:** implement two explicit modules and two committed docs:

- Config rejection strings: field-scoped validation messages used by gRPC
  `StartExperiment` (`INVALID_ARGUMENT`) and standalone YAML (`orchestratord
  --experiment` failure). These are not runtime failure reasons.
- Runtime `FAILED` reason strings: stable prefixes used when an experiment
  transitions to `ExperimentState::Failed`.

The config string constants should live next to the owning validator
(`orch-core::types` may keep `ConfigError`, but expose a stable catalog). The
runtime reason constants should be centralized so `orch-server` and `orch-sched`
do not each invent their own list. A small `orch-core::reasons` module is the
least-churn option because both crates already depend on `orch-core`.

Committed docs should be generated or asserted from the constant lists:

- `docs/config-validation-rejections.md`
- `docs/runtime-failed-reasons.md`

Tests compare the docs' string lists to the code constants. Drift fails CI.

**Rejected:** scraping strings from tests or from `Display` output with no
constant list. That keeps reviewers as the drift detector, which is exactly what
the request rejects.

## D5.2 - Validate sparse wire first, then effective config

**Decision:** split config validation into two phases:

1. A sparse-wire validation pass over `wire::ExperimentConfig` catches unknown
   enum numeric values and fields where proto3 defaulting would otherwise hide
   an invalid supplied value.
2. The existing `effective_config(...).validate_all()` pass validates the
   materialized core config.

YAML parsing errors remain parser errors, not config validator errors. The
shared-path proof should use a value that reaches the validator from both paths
such as `burst.k_per_expansion = 257`.

**Seed interpretation:** `API.md` section 7 says `seed` is required. With proto3 scalar
semantics the only representable "unset" value is `0`, so M5 should reject
`seed == 0` with the same required-field taxonomy unless the implementer records
a deliberate API.md correction in the resolution. The current code accepts seed
zero; this is an M5 matrix gap, not a blocker.

**Rejected:** silently mapping unknown enum numbers to documented defaults.
That is current behavior in `effective_config` for several enums, but it makes
invalid wire input unrejectable.

## D5.3 - Freeze metric families and required label values

**Decision:** the metrics completeness test freezes Prometheus family names and
the label values documented by the architecture:

- `orch_nodes_total{verdict="kept|dup|regression"}`
- `orch_pipeline_queue_depth{stage="submit|complete"}`
- `orch_batch_latency_seconds{stage="select|execute|commit"}`

For histograms, freeze the family (`orch_batch_latency_seconds`) and assert that
the text output includes the normal Prometheus `_bucket`, `_sum`, and `_count`
series for each required stage. Do not freeze bucket boundaries unless the
implementation needs them for a documented SLO.

**Rejected:** name-only testing of `orch_batch_latency_seconds`. A future
refactor could keep one stage and drop the rest without tripping a test.

## D5.4 - Add a renderer, not ad hoc HTTP string assembly

**Decision:** add a reusable metrics renderer in `orch-server` and make
`orchestratord`'s HTTP responder call it. The renderer takes a snapshot object:
experiment stats, scheduler gauges, event-drop counts, and any aggregate runner
state. Tests call the renderer directly; HTTP tests only prove `/metrics` serves
that output.

The current `serve_http` function returns only `orchestratord_up 1`; M5 replaces
that placeholder. Keep the responder plain HTTP for now. Pulling in a web
framework for two endpoints is unnecessary.

## D5.5 - CAS ownership loss is detected before tree writes and at checkpoint CAS

**Decision:** add an ownership guard before node-store writes in the commit path,
and keep the existing CAS check on checkpoint `PutMetadata`.

Mechanically: once the runner has a checkpoint generation, `ensure_checkpoint_owner`
can `GetMetadata(orch/ckpt/<exp>)` and compare the generation to
`self.ckpt_generation`. If it differs or the key is absent, fail with
`checkpoint-cas-ownership-lost`. Call this guard immediately before the C-stage
starts creating/updating tree nodes. The checkpoint path still detects the race
where a competing writer wins after the guard but before `PutMetadata`.

This satisfies both requested windows:

- Node-commit window: takeover before C-stage writes, loser fails before
  `CreateNode`.
- Checkpoint-write window: takeover right before checkpoint CAS, loser fails on
  `PutMetadata`.

**Rejected:** relying only on the next checkpoint CAS. That can allow the stale
runner to write tree nodes after a competing writer has taken the checkpoint key.

## D5.6 - Soak is a manual evidence lane plus CI smoke, same entrypoint

**Decision:** implement one soak entrypoint and parameterize duration/scale.
Recommended shape: `orchestratord --experiment <generated-m5-soak.yaml>
--state-dir <dir>` driven by `scripts/evidence-m5-soak.sh`; the script controls
duration and fault settings through env vars. CI runs the same script with a
short duration and reduced evidence capture. The 24 h lane is manual evidence.

The full run uses K=64, deterministic fake faults, and the persistent fake world.
Because Tier-2 is now present, include a documented Tier-2 resume component in
the soak lane: either run the full soak under a journaled state dir with periodic
true-SIGKILL/resume rounds, or run a separate same-config Tier-2 sub-lane if the
service fault plan cannot be made journal-sound.

**Constraint:** current `orch-simstate::fresh_fakes` deliberately disables fault
plans as a journal soundness invariant. If M5 enables service fault plans under a
journal, it must record the fault-plan seed/config in the journal header and
prove reload uses the same deterministic plan. If that is too large, keep
service-fault and Tier-2 kill faults as two lanes and disclose the split.

## D5.7 - Fake snapshot GC remains an inspection/testing seam

**Decision:** add fakes-only inspection helpers to assert the snapshot-refcount
invariant. The runner should not learn fake-only APIs. The soak harness can
inspect the fake world after Stop:

- committed node snapshot refs from `InMemorySnapshotStore::query_nodes`
- all known fake hypervisor snapshots from a new inspection method
- optional fake GC/retention helper that removes unreferenced snapshots

The invariant is: after Stop and fake GC, live snapshots equal exactly the set of
committed node refs; discarded children are absent from that live set and are
listed as unreferenced orphans before GC.

**Rejected:** RSS-only leak assertions. The request explicitly says RSS-flat
alone does not pass.

## D5.8 - Resolution must include the README correction and observatory handoff

**Decision:** implementation should include the one-line phase README correction
noted in the request where that docs tree is available: `(M1-M4)` becomes
`(M1-M5)` for the early-start band. If the phases docs are outside this repo at
implementation time, record that exact external edit requirement in
`04-resolution.md`.

The M5 resolution must also state:

- where the canonical event-stream surface lives after M5
- current status of `exploration-orchestrator-75z` EventEnvelope divergence
- whether M5 changed any event payload or only metrics/failure taxonomy
