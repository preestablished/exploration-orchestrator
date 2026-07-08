# Soak and Runbook

Scope: 24 h fake soak at K=64 with fault injection, plus a short CI smoke using
the same entrypoint. The soak is manual evidence; the smoke is a regression gate.

## W5.12 - Build one parameterized soak entrypoint

Use one entrypoint for both full and smoke runs. Recommended shape:

- `scripts/evidence-m5-soak.sh` drives `orchestratord --experiment`.
- The script generates or selects an M5 soak YAML with
  `burst.k_per_expansion = 64`.
- The generated soak YAML must keep the run alive for the requested duration:
  set `on_goal: continue`, set `budgets.max_wall_clock_s` to the duration (or
  the corrected v1 representation chosen in W5.2), and set node/expansion
  budgets high enough that they cannot end the run early.
- Duration, seed, state dir, fault plan, and scrape interval are env vars.
- The script writes a manifest and summarized logs under
  `evidence/phase5-m5-hardening/`.

Suggested env:

```bash
M5_SOAK_DURATION_SECONDS=86400
M5_SOAK_SEED=0x0000000000005e05
M5_SOAK_STATE_DIR=target/m5-soak-state
M5_SOAK_FAULT_SEED=0xfa171
M5_SOAK_SCRAPE_INTERVAL_SECONDS=30
M5_SOAK_K=64
```

The CI smoke sets `M5_SOAK_DURATION_SECONDS` to a short value, for example
300-1800 seconds depending on runtime. It must use the same script and binary.

Acceptance:

- The full and smoke lanes do not fork behavior.
- The script exits nonzero on any failed assertion or failed cargo/bin command.
- The script fails if elapsed time is shorter than requested without recording a
  real defect finding.
- The manifest records commit, host, rustc, config hash, K, seeds, duration,
  fault settings, start/end timestamps, and whether Tier-2 persistence/kill
  hooks were used.

## W5.13 - Add deterministic fake fault-plan configuration

The current simulated persistent world builds fakes with disabled fault plans.
M5 needs active fault injection and proof that faults actually fired.

Implement one of these approaches and document which was chosen:

- Preferred: extend `PersistentServices` with a journaled world config header
  containing fault-plan parameters. Reload reconstructs fresh fakes with the same
  deterministic plans, preserving Tier-2 digest soundness.
- Acceptable split lane: keep the journaled Tier-2 lane with service faults
  disabled, and run the continuous 24 h K=64 soak as a journal-less
  service-fault lane using the same experiment config. This must be disclosed as
  a two-lane interpretation.

At least one continuous 24 h K=64 lane must run with Tier-1 service fault
injection active and satisfy the leak/checkpoint/GC assertions. A separate
Tier-2 SIGKILL/journal lane may disable service faults for journal soundness, but
it is additional evidence and does not satisfy the 24 h fault-injected soak by
itself.

Faults should include deterministic latency and transient service errors across
hypervisor, synth, scorer, store, and observatory. Do not enable permanent
invalid-data faults in the 24 h soak unless the expected FAILED reason is part
of the runbook.

Current seams are not enough by themselves: persistent fakes are built with
disabled fault plans, latency is only charged through explicit `SyncAdapter`
probes, and fakes expose `last_fault` rather than cumulative counts. Add
configurable fake fault plans for the soak lane, wire latency probes where the
runner uses sync adapters, and expose cumulative per-target fault counters for
the evidence manifest.

Acceptance:

- Fault-plan settings are recorded in the evidence manifest.
- The CI smoke proves faults actually fired by reporting cumulative per-target
  counts.
- Retryable faults do not create unexplained runtime FAILED reasons.

## W5.14 - Assert leak, checkpoint, and snapshot-refcount invariants

Add soak assertions beyond "the process ran":

- RSS sampled over time is flat within a documented tolerance after warmup.
- `orch_expansions_total` continues increasing until the configured stop/budget.
- Checkpoints appear at the configured cadence (`every_commits` or
  `every_seconds`) and at Stop.
- At Stop, decode the final checkpoint and verify `batch_seq`/stats match the
  observed run.
- Snapshot-refcount invariant:
  - committed node refs = every committed node's `snapshot_ref` in the fake store
  - pre-GC orphans = fake hypervisor snapshots not referenced by committed nodes
  - every discarded child snapshot is in the orphan set before GC
  - after fake GC/retention, live fake snapshots exactly equal committed node refs

Implementation detail: add fakes-only inspection helpers rather than making the
runner depend on fake-only APIs.

Acceptance:

- A failing leak trend, missing checkpoint, or bad snapshot invariant fails the
  soak script.
- The evidence records the final committed-ref count, orphan count, and post-GC
  live count.

## W5.15 - Commit the runtime terminal reason runbook

Add `docs/runtime-terminal-reasons.md` with a status column, and keep a
`FAILED` subset section that is generated from or checked against the runtime
reason catalog. The request asks for the `FAILED` runbook, but current code also
has non-FAILED terminal reasons; the doc should not imply those are `FAILED`.
It should include:

| Reason prefix | Status | Meaning | Operator action | How exercised |
|---|---|---|---|---|
| `checkpoint-cas-ownership-lost` | `FAILED` | Another orchestrator owns the checkpoint key | Stand down loser; inspect winner/resume from latest checkpoint | M5 CAS tests |
| `scorer-archive-seq-mismatch` | `FAILED` | Scorer checkpoint/archive sequence diverged from runner applied count | Stop, preserve evidence, file scorer/orchestrator consistency bug | Existing restore/checkpoint tests or targeted M5 test |
| `synth-fingerprint-mismatch` | `FAILED` | Synth config fingerprint diverged from checkpointed bring-up fingerprint | Stop; compare synth config/macro pack inputs | Existing fault/fingerprint test or targeted M5 test |
| `frontier-exhausted` | `BUDGET_EXHAUSTED` today, or change code before listing as `FAILED` | No frontier nodes remain | Check config/pruning/scoring; this may be legitimate terminal exhaustion | Existing or new small config test |
| `job-retries-exhausted` | `FAILED` in deterministic mode | Deterministic job retry budget exhausted | Inspect worker/fault logs; rerun after fixing deterministic worker fault | Scheduler retry test |
| `determinism-class-mismatch` | `FAILED` when escalated to experiment terminal state | Worker class incompatible and mismatch disallowed | Route to matching worker or set explicit override only if safe | SlotView class-mismatch test |

Audit every assignment to `failure_reason` and every path to
`ExperimentState::Failed`. Each path must map to a stable cataloged prefix, or
the plan must explicitly document and test the exact passthrough string class.
Raw `error.message()` passthroughs are not acceptable unexplained FAILED
strings. If implementation discovers additional runtime terminal prefixes, add
them to the catalog and runbook in the same commit.

Acceptance:

- A doc/code drift test compares the terminal-reason doc prefix list to the
  runtime catalog and fails if any `FAILED` path is uncovered.
- The soak evidence contains a FAILED-string census: either observed strings or
  "none observed"; every possible prefix is still listed in the runbook.

## W5.16 - Full 24 h run and resolution

Run the full lane once after W5.1-W5.15 land.

Evidence layout:

- `evidence/phase5-m5-hardening/run-manifest.md`
- `evidence/phase5-m5-hardening/config-validation.txt`
- `evidence/phase5-m5-hardening/metrics-diff.txt`
- `evidence/phase5-m5-hardening/cas-ownership.txt`
- `evidence/phase5-m5-hardening/soak-24h.txt`
- `evidence/phase5-m5-hardening/soak-smoke.txt`
- `evidence/phase5-m5-hardening/failed-reason-census.txt`

Add `.agents/requests/phase5-m5-hardening-on-fakes/04-resolution.md` with:

- bead ids and dispositions
- commit table
- evidence paths
- soak start/end timestamps and duration
- config hash and K=64 confirmation
- fault settings and whether Tier-2 was used
- leak/GC/checkpoint assertion results
- terminal-reason runbook path and the `FAILED` subset census
- observatory handoff facts from D5.8
- README correction status from D5.8

If the 24 h run surfaces a real defect and stops early, treat it as a successful
finding: record it, fix it, and rerun the full lane.
