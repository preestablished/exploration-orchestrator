# Request: Land M3+M4 (Scheduler + Experiment Runner on Fakes) — Phase 5's Entry Assumption

## Who Is Asking

The determinism program phases track
(`~/.agents/projects/determinism/phases/`), the same coordination surface
behind the Phase 3 request series (guest-sdk Ms4, the hypervisor
framebuffer contract, reference-workload M4 follow-ups, snapshot-store
M7 — all resolved). No Phase 5 consumer repo exists yet to file this,
so the phases track files it directly. Filed 2026-07-06.

## Why exploration-orchestrator, Why Now

The program sits between Phases 3 and 4. Phase 3's exit gate is nearly
cleared (snapshot-store M7 landed 2026-07-03; guest-sdk Ms4 accepted
2026-07-02, with the Ms5 `determinism_replay` CI gate and the Ms3/Ms5
CI lanes in flight), and Phase 4's chains (state-scorer,
input-synthesizer) run in other repos. Both phase docs name your repo as
the parallel track that must not slip:

- Phase 3 (`phases/phase-3-workload-in-the-box.md`): "**Opportunistic
  parallel track (zero platform deps, keeps Phase 5 short):**
  `exploration-orchestrator` M1–M4 (pure core, fakes, scheduler,
  end-to-end runner on a synthetic grid-world)."
- Phase 4 (`phases/phase-4-scoring-and-inputs.md`): "If Phase 3's
  opportunistic orchestrator-on-fakes work (M1–M4) hasn't happened, **run
  it now** as a third parallel track; **Phase 5 assumes it is done**."
- Phase 5 (`phases/phase-5-closed-loop.md`) lists "Orchestrator M1–M4 on
  fakes" as an entry requirement, and its own work list annotates M1–M4
  with "they should already be done; listed here for completeness."

**They are not done.** M1–M2 are genuinely complete (see `01-…` for the
verified evidence — the ralph run through iteration 35 was an M1–M2
final validation, not an M1–M4 one), and the Phase 4 input-synth adapter
work landed on top. But there is no `orch-sched` crate, no worker-driver
lease composition, no S→E→C pipeline, no `ExperimentRunner`, no
checkpoint/WAL/resume, and no served gRPC surface anywhere in the
workspace. Nothing in your beads tracks M3 or M4. If this waits until
Phase 5 opens, the critical path of the program's go/no-go milestone
(first boss) starts with two full milestones of unstarted work.

## The Ask In One Paragraph

Implement M3 and M4 as your `IMPLEMENTATION-PLAN.md` already specifies
them — M3: the `orch-sched` worker driver (lease-API composition, verdict
mapping, determinism-class gating), `SlotView` with
`ListSlots`/`WatchSlots`, the S→E→C pipeline with bounded queues, retry
policy, fast + deterministic modes; M4: the `ExperimentRunner` main loop
with bring-up, bootstrap against `FakeHypervisor`'s Ready event, the
plateau detector + full escalation ladder, `CheckpointV1` save/load + WAL
with scorer-archive drain-lockstep, the served gRPC surface (all six
RPCs), observatory event emission, and ExperimentConfig validation
including standalone YAML — entirely against your existing `orch-fakes`
(zero platform dependencies), with the acceptance bars quoted in `02-…`
(the plan of record governs where the quotes are condensed). Note that
real pieces of M4 scope already exist in `orch-core` and `orch-driver` —
`01-…` inventories them so nothing gets rebuilt.

## What This Request Is Not

- Not M5 (hardening/soak) or M6 (real substrate) — those stay sequenced
  behind this per the plan. M5's soak is called out in `02-…` only so you
  can keep its hooks in mind while building.
- Not a redesign. The plan, architecture doc, and acceptance bars are
  yours already; this request only moves their start date.

## Files In This Request

| File | Contents |
|---|---|
| `01-current-state.md` | Verified repo state: what M1–M2 + the Phase 4 adapter actually cover, and the concrete M3/M4 gaps |
| `02-milestones-m3-m4.md` | Scope and acceptance bars (quoted from your IMPLEMENTATION-PLAN), ordering, and which Phase 5 gate items they feed |
| `03-verification-offer.md` | Evidence conventions, what the phases track will verify on handback, suggested bead shape |
