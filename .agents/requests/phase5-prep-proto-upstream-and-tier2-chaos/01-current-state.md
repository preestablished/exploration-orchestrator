# Current State (Evidence-Based)

Repo `main` at `bf5b7b3`, clean tree, assessed 2026-07-07.

## Done, Reviewed, Resolved

- **M0–M4 complete.** Pure core (`orch-core`), fakes/grid-world
  (`orch-fakes`), scheduler (`orch-sched` — acceptance suite at
  `cba2a5d`), experiment runner + checkpoint/WAL/resume + gRPC
  (`orch-server`, `orch-checkpoint`). Evidence:
  `evidence/phase5-m3-m4/{m3-acceptance,m4-acceptance,chaos,seed-gate}.txt`.
- **Request `phase5-entry-m3-m4-runner-on-fakes` resolved** (2026-07-06,
  `04-resolution.md` at `084892f`); dual review — 0 Critical, 12 unique
  Important findings across the two reviewers (4 + 10, two overlapping) —
  all applied at `bf5b7b3`, see that commit body.
- **Phase 4 input-synth integration** landed via
  `.agents/plans/phase4-requsts/` (adapter, node-attrs, DTO goldens,
  bring-up seeds).

## The Two Beads This Request Covers

1. **`exploration-orchestrator-777` (P2) — upstream the proto.**
   Decision D4 (in `.agents/plans/phase5-entry-m3-m4-runner-on-fakes/01-decisions.md`)
   authored `determinism.orchestrator.v1` inside `crates/orch-proto`
   rather than waiting on control-plane, with upstreaming recorded as the
   payback. What that surface actually is: `crates/orch-proto/protos/
   determinism/orchestrator/v1/orchestrator.proto` — six RPCs, the
   `ProgressEvent` stream, and `ExperimentConfig`. (Note carefully what it
   is *not*: the observatory `EventEnvelope` is a Rust struct in
   `crates/orch-clients/src/observatory.rs`, and a canonical-but-divergent
   `EventEnvelope` proto already exists at
   `control-plane/proto/determinism/observatory/v1/events.proto` — that
   divergence is flagged below, not upstreamed here.) Consumers and
   constraints:
   - control-plane's `controlplane/v1/resources.proto` carries an
     `ExperimentSpec` that their IMPLEMENTATION-PLAN requires to mirror
     your `ExperimentConfig`, enforced by a named descriptor-equality CI
     check;
   - control-plane's tree already holds a **placeholder
     `orchestrator/v1/orchestrator.proto` with an incompatible shape**
     (different `StartExperimentRequest`, empty service) that your upstream
     replaces — and the companion request's breaking gate/tag must not
     freeze the placeholder first;
   - the phases standing rule requires the canonical copy to live in
     `control-plane/proto/` (`phases/README.md`, "The proto crate only
     grows").
   A companion request is being filed in control-plane
   (`../control-plane/.agents/requests/phase4-proto-freeze-tag-and-breaking-gate/`)
   covering their half: buf lint/breaking gates, aarch64 CI, and the
   version tag. This bead is your half: land the `.proto` files there and
   cut `orch-proto` over to build from the canonical location.
2. **`exploration-orchestrator-6ft` (P2) — Tier-2 true-SIGKILL chaos.**
   M4's resumability evidence is the Tier-1 in-process crash lattice
   (5-seed × 11-point, bit-identical continuations — `chaos.txt`).
   Disclosed reinterpretation #12 against decision D5 records the descope:
   Tier-1 does not kill the *process*, so WAL/checkpoint durability under
   a real SIGKILL (half-written files, fsync boundaries, fake-world state
   surviving out-of-process) is untested. Phase 5 exit gate 2 is a literal
   SIGKILL of the orchestrator mid-run. The pre-agreed trigger for
   building Tier-2 was "before the gate run" — with M5+ gated and the
   fakes free, that is now.

## What Stays Untouched (Gated)

M5 (hardening/soak), M6 (real substrate), M7 (first boss), M8 — all wait
on Phase 4/5 entry criteria in other repos. Beads `cww` (async input-synth
adapter, an M6 pre-req), `w1v` (hypervisor lease semantics — re-verify at
M6), `isj` (synth-docs alignment), `5em` (bounded node-attr input tails),
and `a78` (macro-pack planning) stay parked; nothing here starts them.
