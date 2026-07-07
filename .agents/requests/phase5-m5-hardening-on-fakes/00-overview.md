# Request: M5 Hardening On Fakes — The Mislabeled-As-Gated Milestone, Startable Now

## Who Is Asking

The phases track, round 2 (2026-07-07). This corrects a scoping error in
our own round-1 request
(`phase5-prep-proto-upstream-and-tier2-chaos/01-current-state.md`), which
parked M5 under "What Stays Untouched (Gated)." **M5 is not gated.**

## Why exploration-orchestrator, Why Now

The evidence that M5 is fakes-only and startable today:

- The repo plan's own preamble: "milestones M0–M5 run entirely against
  in-repo fakes, so this repo can be built in parallel"
  (IMPLEMENTATION-PLAN, determinism docs). §M5's content is config-matrix
  validation, complete Prometheus metrics per ARCHITECTURE §10, the CAS
  ownership-loss path, and a **24 h fault-injected soak on fakes at
  K=64** — zero platform surface.
- `phase-5-closed-loop.md`: "M5 — hardening: config surface, metrics,
  single-writer discipline, soak on fakes. *Depends on M4.*" M4 is done
  (`bf5b7b3`, resolution + dual review applied). M6 is the first
  platform-gated milestone, not M5. The same doc's parallelism note is an
  instruction: "the fakes-first design exists precisely so M1–M5 never
  wait on the platform — **enforce that**."
- M5 also matters *downstream*: "Orchestrator M5 emits the canonical
  event stream; integrate as soon as both sides exist" — observatory M1
  can be built any time, but its *integration* waits on M5's stream,
  and observatory must be live before the M7 first-boss gate run.

**The counter-authority, confronted rather than skipped:** the phases
README annotates this repo's early-start track as "(M1–M4)" — twice —
under the standing rule that anything else from a later phase waits.
That annotation and the repo plan's "M0–M5 run entirely against
in-repo fakes" band genuinely disagree. As the phases track (the gating
authority), we rule for the repo plan and the phase doc's own
"M1–M5 never wait on the platform — enforce that" instruction: §M5 has
zero platform surface, so the README parenthetical is a
transcription artifact, not a gate. Recorded here as a disclosed
reinterpretation (D5 style); a one-line README correction
("(M1–M4)" → "(M1–M5)") should ride the resolution so the docs stop
disagreeing.

Round-1 (proto upstream + Tier-2 chaos) is unexecuted; this request does
**not** wait for it. M5 touches neither the proto tree nor the chaos
harness — the two proceed in parallel. The one time-boxed coupling
remains round-1's: the proto placeholder must be replaced before
control-plane tags `proto-v*` (window still open — no tag exists as of
this filing).

## The Ask In One Paragraph

Execute M5 per the plan: the full `ExperimentConfig` validation matrix
(every rejectable config shape rejected with a stable, grep-able FAILED
reason string); the complete Prometheus metric surface per
ARCHITECTURE §10; the single-writer/CAS ownership-loss path exercised
under fault injection; and the 24-hour soak on fakes at K=64 with fault
injection, with leak/GC assertions and a runbook enumerating every
FAILED reason string observed — the soak infrastructure and failure
taxonomy the Phase 5 first-boss gate run (and its own 4-hour soak floor)
will stand on.

## Files In This Request

| File | Contents |
|---|---|
| `01-current-state.md` | Evidence: M0–M4 done, the mislabel, bead landscape |
| `02-requested-work.md` | The ask, sequencing, acceptance criteria, out of scope |
| `03-verification-offer.md` | Phases-track verification and the observatory handoff note |
