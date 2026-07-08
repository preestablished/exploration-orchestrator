# Plan: M5 Hardening on Fakes

Answers `.agents/requests/phase5-m5-hardening-on-fakes/`.
Baseline for this plan: `main` at `f80b0fe` (2026-07-08), clean tree
before planning. This is newer than the request filing: the sibling proto
upstream and Tier-2 true-SIGKILL work is already resolved (`777` and `6ft`
closed, evidence under `evidence/phase5-tier2-chaos/`). M5 should therefore
use that infrastructure where it helps, but must not reopen or duplicate it.

Plan authority remains the docs snapshot at
`/home/infra-admin/.agents/projects/determinism/docs/exploration-orchestrator/`:
`IMPLEMENTATION-PLAN.md` section M5, `API.md` sections 7 and 8,
`ARCHITECTURE.md` section 10, plus the phase doc's "M1-M5 never wait on
the platform" instruction. Everything here runs against in-repo fakes.

## Objective

Land M5 as four linked hardening surfaces:

- **Config validation matrix:** every rejectable `ExperimentConfig` shape from
  `API.md` section 7 is rejected deterministically. Config rejections are
  `INVALID_ARGUMENT` / CLI config errors with exact field messages. They are
  not runtime `FAILED` reason strings.
- **Prometheus metrics completeness:** `/metrics` exports the complete
  `ARCHITECTURE.md` section 10 surface, including the prose-only
  `orch_observatory_dropped_total`, and a test freezes the required metric
  families plus documented label values.
- **Single-writer CAS ownership loss:** a competing writer can take ownership
  mid-run; the loser detects the checkpoint generation loss, fails with the
  stable runtime reason string, and performs no writes after the loss is
  detected. Cover both the checkpoint-CAS window and a node-commit window.
- **24 h fault-injected soak on fakes at K=64:** one full manual evidence run
  with deterministic fake faults active, using the same harness as the CI smoke
  at a shortened duration. Assert checkpoint cadence, RSS/leak behavior, and
  the snapshot-refcount/GC invariant. Commit the soak runbook of every runtime
  `FAILED` reason string.

## Files in this plan

| File | Contents |
|---|---|
| `01-decisions.md` | Decisions and constraints that should be reviewed before coding |
| `02-config-validation.md` | Work items W5.1-W5.5 for the API.md section 7 validation matrix and string-freeze doc |
| `03-metrics.md` | Work items W5.6-W5.8 for the Prometheus surface and metrics completeness tests |
| `04-cas-ownership.md` | Work items W5.9-W5.11 for CAS ownership-loss injection and no-post-loss-write assertions |
| `05-soak-and-runbook.md` | Work items W5.12-W5.16 for the soak harness, fault plan, leak/GC assertions, and runbook |
| `06-verification.md` | Acceptance mapping, CI/evidence expectations, and handback shape |

## Sequencing

```
W5.0 bead filing and doc correction
  -> W5.1 string modules and docs generated from constants
  -> W5.2 full wire/core/YAML config matrix
  -> W5.3 shared served/standalone validator
  -> W5.4 feature-map decoded_features validation/materialization
  -> W5.5 config evidence diff

W5.6 metrics source/renderer
  -> W5.7 complete /metrics wiring
  -> W5.8 metrics completeness tests

W5.9 ownership guard seam
  -> W5.10 checkpoint-window CAS-loss test
  -> W5.11 node-commit-window CAS-loss test

W5.12 soak harness + fake fault-plan config
  -> W5.13 snapshot-refcount/GC inspection
  -> W5.14 CI smoke lane
  -> W5.15 full 24 h evidence run
  -> W5.16 resolution and observatory handoff
```

Metrics and CAS work can proceed in parallel after W5.1. The soak comes last:
it consumes the final validation strings, runtime failure taxonomy, metrics,
CAS path, and fake GC assertions.

## Tracking beads

Create beads before implementation starts. Recommended breakdown:

```bash
PARENT=$(bd create --title="M5: hardening on fakes" \
  --description="Request .agents/requests/phase5-m5-hardening-on-fakes/. Plan .agents/plans/phase5-m5-hardening-on-fakes/. Config validation matrix, Prometheus metrics, CAS ownership-loss path, and 24 h fault-injected soak on fakes at K=64." \
  --type=epic --priority=1 --silent)
CFG=$(bd create --title="M5 config validation matrix and string freeze" \
  --description="Implement W5.1-W5.5: API.md section 7 matrix, shared gRPC/YAML validation path, exact config rejection messages, and committed doc drift tests." \
  --type=task --priority=1 --silent)
MET=$(bd create --title="M5 Prometheus metrics completeness" \
  --description="Implement W5.6-W5.8: ARCHITECTURE section 10 metric surface including observatory drops, renderer, /metrics wiring, and completeness test." \
  --type=task --priority=1 --silent)
CAS=$(bd create --title="M5 CAS ownership-loss path" \
  --description="Implement W5.9-W5.11: competing-writer fault scenarios for checkpoint-CAS and node-commit windows, stable FAILED reason, and no stale loser writes after ownership loss." \
  --type=task --priority=1 --silent)
SOAK=$(bd create --title="M5 24h fault-injected fake soak and runbook" \
  --description="Implement W5.12-W5.16: K=64 soak harness, CI smoke using same binary, leak and snapshot-refcount/GC assertions, evidence, runbook, resolution." \
  --type=task --priority=1 --silent)
bd dep add $MET $CFG
bd dep add $CAS $CFG
bd dep add $SOAK $CFG
bd dep add $SOAK $MET
bd dep add $SOAK $CAS
bd dep add $PARENT $CFG
bd dep add $PARENT $MET
bd dep add $PARENT $CAS
bd dep add $PARENT $SOAK
```

Close each child bead with evidence pointers, then close the parent.

## Out of scope

- M6/M7/M8 real-platform integration, throughput tuning, or first-boss work.
- Reworking the canonical proto beyond using the already-upstreamed
  `determinism-proto` surface.
- Resolving `exploration-orchestrator-75z` EventEnvelope divergence. M5 must
  report the current surface and not create a third event shape.
- The leftover beads `5em`, `isj`, `a78`, `cww`, `w1v`, except that M5 may note
  when a touched file makes a small follow-on more obvious. Do not bundle them
  into M5.
