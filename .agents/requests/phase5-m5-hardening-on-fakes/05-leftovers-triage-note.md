# Round-3 Triage Note: The Leftover Beads (5em / isj / a78) — No Request Filed, On Purpose

Filed 2026-07-07 (round 3) after a focused assessment. The phases track
considered a "leftovers closeout" request bundling `5em`, `isj`, `a78`
and decided **against** filing it. Rationale, so the decision isn't
re-litigated from scratch next round:

- **`5em` (bounded recent-input tails) needs no request.** It is
  self-contained, fakes-only, and its design is already fully written:
  `.agents/plans/phase4-requsts/04-node-attrs-context.md` (append
  parent tail + child burst, cap at the documented window) and
  input-synthesizer INTEGRATION.md §(1) (≤600-frame pad window; the
  event/token window cap is the one open one-line design call). An
  idle agent takes it as ordinary bead work — or as a "while you're in
  `node_attrs.rs`" rider during this M5 request's execution (payload
  logic, distinct from M5's CAS-discipline concern in the same region).
- **`isj` (seed-rule docs fix) has nowhere to land.** Its target is
  `input-synthesizer/INTEGRATION.md:81` (the stale
  `blake3(experiment_seed ‖ node_id ‖ expansion_counter)[..8]` rule vs
  the implemented `derive_synth_request_seed` /
  `DeterministicRng::synth(experiment_seed, batch_seq)` first draw,
  `crates/orch-core/src/rng.rs:156`) — and the input-synthesizer repo
  does not exist. Annotate the bead blocked-on-repo-creation; roll the
  fix into whatever bootstraps that repo's docs.
- **`a78` (mid-experiment macro-pack loading plan) is soft-blocked on
  the same missing repo** — its subject is input-synthesizer M2/M3
  machinery (`LoadMacroPack` hot-loading, mined-pack adoption,
  fingerprint transition policy). Draftable against doc snapshots,
  unvalidatable until the repo exists. Stays parked with `cww`/`w1v`.

The program-level gap underneath two of the three — **the
input-synthesizer (and state-scorer) repos don't exist** — remains the
operator escalation recorded in
`~/git/preestablished/REQUEST-WORK-ORDER-2026-07-07.md`.
