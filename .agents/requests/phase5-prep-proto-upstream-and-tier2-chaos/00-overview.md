# Request: Pay The Proto Debt And Harden The Resume Gate While The Platform Catches Up

## Who Is Asking

The phases track, on behalf of the two downstream consumers of the
orchestrator's contracts: `control-plane` (owner of the canonical proto
tree) and the future `observatory` event-ingest work. Filed 2026-07-07.

## Why exploration-orchestrator, Why Now

Your allowed early-start scope is **complete**: M0–M4 on fakes landed,
dual-reviewed with all findings applied (`main` at `bf5b7b3`), and the
`phase5-entry-m3-m4-runner-on-fakes` request is resolved. M5+ is gated —
Phase 5's entry needs the Phase 3 exit gate, which lives in other repos —
so the question is what this repo does with the gap. Two of your own open
beads answer it, and both get harder the longer they wait:

1. **The proto promise (`exploration-orchestrator-777`, P2).** Decision D4
   authored `determinism.orchestrator.v1` locally in `orch-proto` as a
   temporary measure with an explicit promise to upstream. The phases
   standing rule is unambiguous: "any cross-service schema change lands in
   `control-plane/proto/` before the consuming code" (`phases/README.md`).
   Control-plane's own IMPLEMENTATION-PLAN names a descriptor-equality CI
   check between its `ExperimentSpec` and your `ExperimentConfig`, and a
   **placeholder `orchestrator/v1/orchestrator.proto` with an incompatible
   shape already sits in their tree** waiting to be replaced. Every week
   the local copy drifts is contract-drift risk at the M6/M7 integration,
   priced at the worst possible time — the first-boss gate run. And the
   window is closing: the companion control-plane request adds a
   `buf breaking` gate and tags `proto-v*` — the placeholder must be
   replaced *before* that baseline is cut, or the replacement becomes a
   formally breaking change the new gate rejects.
2. **The resume gate's known soft spot (`exploration-orchestrator-6ft`,
   P2).** Phase 5 exit gate 2: SIGKILL mid-run → resume from checkpoint →
   search still reaches the gate. Your repo plan's M4 acceptance says it
   plainly — the harness "SIGKILLs the process" — so Tier-2 is paying back
   a disclosed reinterpretation of early-start scope, not pulling M5
   forward. Your M4 chaos evidence is the in-process Tier-1 crash lattice;
   the M3/M4 resolution's "Disclosed follow-up beads" section (per
   decision D5) records that true-SIGKILL/whole-fake-world persistence was
   descoped on a pre-agreed trigger. That trigger is now: the gate run
   this hardening protects is the program's go/no-go milestone, and
   fakes-only work is free right now while it will compete with
   integration firefighting once M6 opens.

## The Ask In One Paragraph

Upstream `determinism.orchestrator.v1` into `control-plane/proto/` and cut
`orch-proto` over to consume the canonical copy (coordinate with the
control-plane request filed alongside this one — they own the tag/breaking-
gate half), then build the Tier-2 chaos harness: a true-SIGKILL of the
whole orchestrator process with the fake world persisted out-of-process,
resume from checkpoint, and the same bit-identical-continuation standard
the Tier-1 lattice already enforces. Both are fakes-only/proto-only — no
gated Phase 5 platform work is touched.

## Files In This Request

| File | Contents |
|---|---|
| `01-current-state.md` | Evidence: what's done, the two beads, what's gated and stays untouched |
| `02-requested-work.md` | The ask, sequencing, acceptance criteria, out of scope |
| `03-verification-offer.md` | Cross-repo verification: control-plane's side, and the phases-track check |
