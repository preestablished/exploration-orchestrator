# Requested Work

## What We Need (Behavioral)

1. **Bead the milestone first.** File the M5 work breakdown as beads
   (matrix / metrics / CAS path / soak, with dep edges) so progress is
   ledger-visible — this repo's M3/M4 discipline, continued.
2. **Config validation matrix — anchored to API.md §7.** Two distinct
   string taxonomies; don't fuse them: config rejection is
   `INVALID_ARGUMENT` with bad-field messages (API.md §1/§7 semantics),
   while the soak runbook catalogs *runtime* FAILED reason strings
   (CAS loss, worker loss, ...). For the matrix: every `ExperimentConfig`
   field/oneof in API.md §7 gets ≥1 invalid case with a test asserting
   the exact message; one test proves both entry paths (gRPC
   `StartExperiment` and `--experiment` YAML) share the same validator.
   Freeze both string sets mechanically: constants in one module plus a
   test asserting the committed doc list equals the code's constant
   list — drift fails CI, not reviewer memory.
3. **Metrics completeness per ARCHITECTURE §10.** Audit the §10 list
   against what `orch-sched`/`orch-server` already export; land the
   gaps — including `orch_observatory_dropped_total`, which hides in
   §10's *prose* rather than its headline list; one test asserting the
   full expected metric-name set (state whether label sets like
   `verdict=kept|dup|regression` are part of the frozen expectation)
   is exported, so a future refactor can't silently drop one.
4. **CAS ownership-loss path.** A test (or fault-injection scenario)
   where a competing writer takes ownership mid-run: the loser must
   detect via CAS, stop cleanly with its reason string, and the tree
   must show no post-loss writes from the loser. Cover both
   checkpoint-write and node-commit windows.
5. **The 24 h soak.** On fakes, K=64, fault injection active
   (the Tier-1 fault plans; if round-1's Tier-2 harness has landed by
   then, run the soak under it and say so — but do not wait for it).
   The plan's own target, verbatim, is the bar: "zero leaks — RSS
   flat; at Stop, the fake store's live snapshots are exactly the
   committed nodes' refs, with every discarded child an unreferenced
   orphan eligible for GC" — assert the snapshot-refcount invariant
   explicitly; RSS-flat alone does not pass. Checkpoint cadence held,
   no unexplained FAILED strings. Deliverable: the **soak runbook** —
   every FAILED reason string observed (or provably possible from the
   matrix), its meaning, and the operator action.
   (Requester's interpretations, not plan text — adopt or overrule
   with a note: 24 h doesn't fit CI, so define the soak as a
   documented manual lane — any dev host suffices, no M6 hardware is
   implied — with a short ≤30 min CI smoke variant that is the same
   harness binary at a shortened window, not a second code path;
   freeze the reason strings once the runbook cites them; cover both
   checkpoint-write and node-commit CAS windows; assert the exported
   metric-name set.)

## Suggested Sequencing (Yours To Overrule)

1 → 2 → 3 and 4 in either order → 5 last (the soak consumes the matrix
strings, metrics, and CAS path). Round-1's beads (`777`, `6ft`) remain
open in parallel — if you are also the agent holding round-1, do `777`
first regardless (its window is time-boxed by control-plane's tag);
nothing in M5 collides.

## Acceptance Criteria

1. M5 beads filed and closed with evidence pointers.
2. Matrix: every enumerated invalid shape has a test asserting its
   exact string; the string list is committed as a doc (the runbook's
   appendix).
3. Metrics: the §10 completeness test passes; a diff of
   before/after metric names recorded.
4. CAS: both window scenarios pass; the loser's reason string is in
   the matrix doc.
5. Soak: one full 24 h run recorded under `evidence/` (start/end
   timestamps, K, fault plan, leak/GC assertion results, FAILED-string
   census), the CI smoke variant green, and the runbook committed.
   A soak that surfaces a real defect and stops early is a *successful
   finding*, not a failed acceptance — fix, record, rerun.

## Out Of Scope For This Request

- **M6/M7/M8** — genuinely platform-gated (Phase 3/4 exit gates); the
  phase doc's fakes-first instruction cuts both ways.
- Round-1's scope (`777` proto upstream, `6ft` Tier-2 chaos) — parallel,
  not superseded. The soak may *use* Tier-2 if it exists; it must not
  *wait* for it.
- `cww` (async input-synth adapter) — M6-shaped; stays parked per
  round-1.
- Resolving the EventEnvelope divergence — flagged item; note any new
  facts on it, don't fix it here.
