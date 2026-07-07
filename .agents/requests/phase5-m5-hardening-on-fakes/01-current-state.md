# Current State (Evidence-Based)

Repo `main` at `438a219` (the round-1 filing commit on top of
`bf5b7b3`), clean tree, assessed 2026-07-07.

## Done

- **M0–M4 complete** at `bf5b7b3`: pure core, fakes/grid-world,
  scheduler (acceptance `cba2a5d`), experiment runner +
  checkpoint/WAL/resume + gRPC. Evidence:
  `evidence/phase5-m3-m4/{m3-acceptance,m4-acceptance,chaos,seed-gate}.txt`;
  M3/M4 request resolved 2026-07-06, dual review (0 Critical, 12 unique
  Important) applied — with one disclosed partial deferral: the
  f64-optional-presence piece of one finding rides bead `777` (the
  proto upstream), per the `bf5b7b3` commit body's "Deferred with doc
  markers."

## Open Requests And Beads

- **Round-1 request** (`phase5-prep-proto-upstream-and-tier2-chaos/`):
  unexecuted — beads `777` (proto upstream) and `6ft` (Tier-2 SIGKILL
  chaos) both open. Its proto window is still open: control-plane has
  cut no `proto-v*` tag and the incompatible placeholder is untouched.
  **This request is independent of it** — M5 touches neither scope.
- Bead board: 7 open, 0 in progress, all `bd ready` (`777`, `6ft`,
  `cww`, `w1v`, `isj`, `5em`, `a78`). None of them *is* M5 — M5 has no
  bead yet; filing its work breakdown is part of this request.

## The Mislabel This Request Corrects

Round-1's `01-current-state.md` grouped M5 with M6–M8 as "gated on
Phase 4/5 entry criteria in other repos." For M6–M8 that's right; for
M5 it contradicts both authorities:

- IMPLEMENTATION-PLAN preamble: M0–M5 named inside the fakes-only band;
  §M5 acceptance is "soak passes; grep-able runbook of every FAILED
  reason string" — no platform dependency anywhere in its text.
- `phase-5-closed-loop.md` M5 line: "*Depends on M4*" — and M4 is done.

## What M5 Consists Of (Plan §M5)

1. **Config validation matrix** — every invalid `ExperimentConfig`
   shape rejected deterministically with a stable FAILED reason string
   (the strings become the soak runbook's vocabulary).
2. **Metrics completeness** — the Prometheus surface per
   ARCHITECTURE §10, wired through orch-sched/orch-server (partial
   metrics exist in `orch-sched/src/metrics.rs`; §10 is the checklist).
3. **Single-writer / CAS ownership-loss path** — the discipline the
   M3/M4 review pressed on, now exercised deliberately: a second writer
   appears, the loser detects ownership loss via CAS and fails cleanly
   with its reason string, no split-brain tree writes.
4. **24 h soak on fakes, K=64, fault injection** — long-run leak/GC
   assertions, checkpoint cadence held, and the runbook: every FAILED
   reason string observed during the soak, catalogued with meaning and
   operator action.

## Why This Matters Downstream

- Phase 5 exit gate 5 is a 4-hour soak with >80% slot utilization; the
  M7 first-boss gate run is "multi-hour ... use the observatory
  dashboard to diagnose stalls rather than guessing." M5's soak
  infrastructure, failure taxonomy, and metrics are those runs'
  instrumentation.
- Observatory M1 integrates against the event stream "as soon as both
  sides exist" — M5 landing is the orchestrator-side trigger.
- The known EventEnvelope divergence (orch-clients postcard struct vs
  `observatory/v1` `payload_json` proto) stays a flagged, recorded item
  per round-1 — M5's metrics/event work must not silently entrench
  either shape without noting it there.
