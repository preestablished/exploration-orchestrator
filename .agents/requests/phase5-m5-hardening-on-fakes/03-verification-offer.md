# Verification And Downstream Handoff

## Phases-Track Verification

On your resolution we will:

1. re-run the matrix and metrics-completeness tests plus the CI soak
   smoke from a clean checkout, and audit matrix *coverage against
   API.md §7* (every field/oneof represented — not just "the tests
   pass");
2. audit the soak evidence for window integrity (continuous
   timestamps, assertions evaluated over the whole 24 h, not a
   truncated run silently passed off);
3. spot-check three FAILED reason strings end-to-end: force the
   condition, observe the exact string, find it in the runbook.

## Observatory Handoff Note

M5's event stream is observatory M1's integration input
(`phase-5-closed-loop.md`). Observatory has no local repo yet — so the
handoff is a note: when M5 resolves, record in the resolution (a) where
the canonical event-stream surface lives post-M5, (b) the current
status of the EventEnvelope divergence (postcard struct vs
`observatory/v1` proto), so observatory's first request starts from
facts instead of archaeology.

## Relationship To Round-1

Independent scopes, one caveat each direction:

- If round-1's Tier-2 harness lands first, run the soak under it and
  note the upgrade; otherwise Tier-1 fault plans are the documented
  basis.
- If M5's metrics/event work touches any type that `777`'s proto
  upstream will canonicalize, coordinate naming with the control-plane
  request rather than inventing a third shape.

## Handback Shape

Append `04-resolution.md` here (bead ids, commits, evidence paths, the
runbook location, the observatory handoff facts); we respond with
`05-verification.md`.

## Contact / Tracking

- Plan authority: IMPLEMENTATION-PLAN §M5; `phase-5-closed-loop.md`
  M5 line + parallelism note.
- Sibling open request: `phase5-prep-proto-upstream-and-tier2-chaos/`
  (beads `777`, `6ft`).
- Prior evidence discipline to match: `evidence/phase5-m3-m4/`.
