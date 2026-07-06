# Verification Offer and Handback Shape

Same conventions as the resolved Phase 3 request series (see
snapshot-store `phase3-m7-gc-exit-gate` for the closest precedent: plan
→ two-reviewer pass → work-item commits → evidence → resolution file).
Verification runs on this host — the same environment that re-verified
M7 for gate item 4. The aarch64 leg of the seed gate is best-effort
pending a runner; if none exists at handback, that leg is verified
locally-only and left as a disclosed bead, the same posture your M4 bar
allows.

## What the phases track will verify on handback

1. **Re-run the acceptance suites cold** from a clean checkout on both
   an x86_64 and (if a runner exists) an aarch64 host:
   `cargo test --workspace` plus the named M3/M4 acceptance tests, and
   the seed-reproducibility gate twice with the same seed.
2. **Chaos spot-check:** re-run the SIGKILL chaos test with a fresh
   random seed (not the CI-pinned one) and confirm the resume
   invariants hold on runs the implementation has never seen.
3. **Expansion-path contract:** confirm the parent/sibling burst +
   `score_delta` contract (already proven at the request-builder level
   in `orch-driver/tests/input_synth_context.rs`) is re-exercised
   through the real M3 expansion path, per `02-…`.
4. **Purity boundaries:** `purity_guard` still green;
   `orch-clients`/`orch-fakes` still free of tokio/tonic/filesystem/
   wall-clock dependencies.

## Evidence conventions

- One commit per work item (or small coherent group) on `main`, SHAs
  listed in the resolution file.
- Test-run evidence (command lines + summarized output, seeds used,
  chaos-run counts) recorded under `target/` or an evidence directory
  named in the resolution — the M7 GC handback's evidence-script
  approach worked well.
- State explicitly any acceptance bar you stage out or reinterpret
  (e.g. if the aarch64 leg of the seed gate has no runner yet, say so
  and leave a bead), the way snapshot-store split its `BM:` benchmark
  bar into its own bead.

## Suggested bead shape

Per the beads conventions: a parent request bead, then one bead per
slice, children blocked on parents —

```bash
PARENT=$(bd create "Phase 5 entry: land M3+M4 runner-on-fakes" \
  -d "Request .agents/requests/phase5-entry-m3-m4-runner-on-fakes/. M3 orch-sched + M4 ExperimentRunner per IMPLEMENTATION-PLAN, acceptance bars verbatim." \
  -p 1 -l impl -t epic --silent)
M3=$(bd create "M3: orch-sched worker driver, S-E-C pipeline, retries" \
  -d "Lease composition, SlotView, bounded queues, retry policy, fast+det modes. Accept bars in request 02. Includes the parked input-synth M3 smoke." \
  -p 1 -l impl --silent)
M4=$(bd create "M4: ExperimentRunner, checkpoint/WAL/resume, gRPC surface" \
  -d "Runner loop, bring-up, plateau ladder, CheckpointV1+WAL with archive lockstep, six RPCs, event emission, standalone YAML. Accept bars in request 02." \
  -p 1 -l impl --silent)
# M4 blocked on M3; the epic blocked on both (closes last)
bd dep add $M4 $M3
bd dep add $PARENT $M3; bd dep add $PARENT $M4
```

## Resolution files

When done, add to this directory in the established pattern:

- `04-resolution.md` — commits, decisions, staged-out items with beads.
- `05-verification.md` — filled in by the phases track after the
  re-verification above; the request is closed only when that file
  records the gates green.

## Priority and timing

P1. Nothing in Phase 4's other chains blocks on this, but Phase 5's
critical path starts here — the phase docs' whole reason for the
"opportunistic parallel track" is that this work has zero platform
dependencies and should never be the thing Phase 5 waits on. The quiet
window between the Phase 3 gate closing and the Phase 4 gate run is the
right time, exactly as it was for snapshot-store's M7.
