# Verification, Evidence, and Handback

## Acceptance-criteria mapping (request `02-requested-work.md`)

| Accept bar | Satisfied by | Verified how |
|---|---|---|
| 1. `determinism.orchestrator.v1` builds from `control-plane/proto/` in both repos' CI; no independent copy in `orch-proto`; 777 closed w/ control-plane SHA | W1.2 (canonical proto + codegen), W1.4 (cutover + deletions), W1.5 (close) | Both CI runs green; `rg` shows no local `include_proto`/`protos/`; bead closure text carries the SHA |
| 2. `ExperimentSpec` ↔ `ExperimentConfig` relationship recorded | W1.1 note (D-P3 scope list) + control-plane's own descriptor check (their item 4) | Note file in their request dir, acknowledged; our resolution links it |
| 3. Tier-2 harness: ≥5 seeds, ≥50 SIGKILLs, all Tier-1 points + forced mid-checkpoint-write + mid-WAL-append; bit-identical continuation; demonstrated negative; 6ft closed w/ evidence | W2.1–W2.6 | `evidence/phase5-tier2-chaos/` (`TIER2_SUMMARY` line with per-class kill counts; `negative-control.txt` naming the mutation); harness asserts kill quota + hash equality; CI smoke keeps mechanisms green |
| 4. D5 reinterpretation-trail note marking the Tier-2 gap closed | W2.7 | Dated addendum in the M3/M4 `04-resolution.md` |

Cross-repo verification per the request's `03-verification-offer.md`:
their CI green with our files + breaking gate active; our CI green on
the canonical location; one scratch-branch `buf breaking` demonstration
recorded in both request dirs (W1.5 — whichever repo lands second runs
it). For item 2 the phases track re-runs the Tier-2 harness from a clean
checkout at a seed of their choosing: the harness honors `CHAOS_SEED`
the same way `chaos_resume.rs` does (single-seed override) — W2.4 must
implement that env passthrough.

## Test matrix (net-new)

| Test | Where | Gate |
|---|---|---|
| determinism-proto orchestrator facade smoke | control-plane `lib.rs` tests | their CI |
| journal round-trip / torn-tail-at-every-offset / mid-file corruption panic / version header | `orch-simstate` unit | our CI (workspace) |
| PersistentWorld create→drive→reload state equality + live-after-reload | `orch-simstate` unit | our CI |
| replay response-digest mismatch panics | `orch-simstate` unit (deliberately perturbed fake) | our CI |
| Tier-1 lattice unchanged, now on the shared comparator | `chaos_resume.rs` (refactor only — assertions identical) | our CI |
| Tier-2 lattice + torn-writes + random kills + gRPC resume smoke | `bins/orchestratord/tests/tier2_chaos.rs` | CI smoke (reduced) + evidence lane (full) |
| Negative control (perturbed-node replay diverges; drop-WAL rejected as convergent — see W2.5) | same harness file | CI smoke + evidence |

Existing suites must stay untouched and green: the M3/M4 acceptance
suites, `seed_gate`, `grpc_surface` (W1.4 proves the re-export by
compiling them unchanged).

## Evidence conventions

`evidence/phase5-tier2-chaos/` mirrors `evidence/phase5-m3-m4/`:
`run-manifest.md` (date, host, toolchain, commit, seeds, kill counts) +
`tier2-chaos.txt` + `negative-control.txt`, produced by
`scripts/evidence-tier2.sh`, greppable `== section ==` headers, trimmed
to `^test |test result|TIER2_` lines. Evidence is committed, and the
resolution references exact paths.

## CI changes

- New job `tier2-chaos-smoke` (both arches) — reduced matrix per D-T5.
- No change to the existing `rust` job besides the workspace picking up
  `orch-simstate` (its unit tests ride `cargo test --workspace`).
- Control-plane CI changes (buf gates, aarch64) are theirs; we only add
  the two lint exemptions if their gate landed first (W1.3).

## Resolution / handback shape

`.agents/requests/phase5-prep-proto-upstream-and-tier2-chaos/04-resolution.md`,
same convention as the M3/M4 resolution (the request asks for the
D5-style discipline by name):

- Commit table: this repo's SHAs per work item **and** the control-plane
  SHAs for W1.1/W1.2/W1.3 (+ the scratch-branch demo link).
- Bead dispositions: 777 closed, 6ft closed, the new EventEnvelope bead
  open with its id, `cww` explicitly untouched (or disclosed if window
  allowed).
- Evidence paths.
- Reinterpretations and named deltas (disclosed), numbered, expected
  entries at minimum: (a) lint exemptions vs. renames outcome (D-P2 —
  including the escape hatch if taken); (b) CI-vs-manual lane decision
  with measured runtime (D-T5); (c) reclaim-on-reload as the Tier-2
  stand-in for worker session teardown (D-T4, echoing W4.4a's
  disclosure); (d) `Applied`-frame response-digest design if it deviated
  from D-T2's sketch; (e) anything the fakes' determinism audit turned
  up (a digest mismatch forcing a fake-side fix is a finding to
  disclose, not silently patch); (f) the negative-control mutation
  actually shipped — the request's own "e.g. WAL replay skipped"
  example is convergent by design and was replaced (W2.5), which is
  itself a disclosed reinterpretation of the request text; (g) if the
  upstream landed before control-plane's buf gates existed, the
  gates→upstream→tag inversion (W1.5).
- The phases track responds with `05-verification.md` (their clean-
  checkout re-run at a fresh `CHAOS_SEED`).

## Session-close checklist (binding, per CLAUDE.md)

Quality gates (`cargo fmt --check`, `clippy -D warnings`, `cargo test
--workspace --all-features`, `cargo test -p orch-server --test
seed_gate`) before every commit batch; `bd dolt push`, `git push` in
**both** repos at session end; `git status` clean and up to date with
origin in both.
