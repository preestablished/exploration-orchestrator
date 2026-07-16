# Runtime Terminal Reasons

Runtime terminal reasons are stable prefixes for experiment terminal outcomes.
They are not config validation rejection strings.

## FAILED Prefixes

- `checkpoint-cas-ownership-lost`
- `scorer-archive-seq-mismatch`
- `synth-fingerprint-mismatch`
- `job-retries-exhausted`
- `determinism-class-mismatch`
- `runtime-error`

## Catalog

| Reason prefix | Status | Meaning | Operator action | How exercised |
|---|---|---|---|---|
| `checkpoint-cas-ownership-lost` | `FAILED` | Another orchestrator owns the checkpoint key. | Stand down the loser; inspect the winner and resume from the latest checkpoint. | M5 CAS ownership-loss tests. |
| `scorer-archive-seq-mismatch` | `FAILED` | Scorer checkpoint/archive sequence diverged from the runner applied count. | Stop, preserve evidence, and file a scorer/orchestrator consistency bug. | Restore/checkpoint tests and M5 runbook audit. |
| `synth-fingerprint-mismatch` | `FAILED` | Synth config fingerprint diverged from the checkpointed bring-up fingerprint. | Stop and compare synth config and macro-pack inputs. | Fingerprint guard tests. |
| `job-retries-exhausted` | `FAILED` | Deterministic job retry budget exhausted. | Inspect worker and fault logs; rerun after fixing deterministic worker faults. | Scheduler retry tests. |
| `determinism-class-mismatch` | `FAILED` | Worker class is incompatible and mismatch is disallowed. | Route to a matching worker or explicitly allow mismatch only when safe. | SlotView and driver class-mismatch tests. |
| `runtime-error` | `FAILED` | Documented passthrough class: a terminal client error whose message matches no dedicated prefix is wrapped as `runtime-error: <message>`. | Inspect the wrapped message; recurring shapes are candidates for a dedicated cataloged prefix. | orch-core `runtime_reasons` wrap tests. |
| `frontier-exhausted` | `BUDGET_EXHAUSTED` | No frontier nodes remain. | Check config, pruning, and scoring; this may be legitimate terminal exhaustion. | Existing frontier exhaustion path. |
