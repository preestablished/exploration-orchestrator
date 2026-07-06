# Verification, Evidence, CI

Conventions per request `03-verification-offer.md` (snapshot-store M7
precedent: plan → two-reviewer pass → work-item commits → evidence →
resolution file).

## Acceptance-bar → test traceability

Every quoted bar in request `02-…` maps to exactly one named test (see
tables in `02-m3.md` W3.6 and `03-m4.md` W4.8). No bar is satisfied
"implicitly by other tests". Reinterpretations, stated up front:

1. **"memory flat" (M3 backpressure)** — asserted structurally
   (bounded queues, bounded in-flight sets, gauge caps) rather than by
   RSS sampling; RSS belongs to M5's 24 h soak. Disclosed in
   `04-resolution.md`.
2. **"Kill -9 anywhere" (M4)** — Tier-1 exhaustive in-process crash
   lattice is the primary vehicle; Tier-2 true-SIGKILL harness proves
   the process-level claim (D5). If Tier 2 is descoped by review, it
   becomes a disclosed bead.
3. **aarch64 legs** — CI already runs an `ubuntu-24.04-arm` matrix
   arm, so the cross-arch seed gate is expected to run in CI, not
   local-only. If the arm runner is unavailable at handback, the leg is
   verified locally-only and disclosed as a bead (posture the request
   allows).
4. **Utilization/latency in virtual time** — the >95% bar and ±50%
   jitter are measured on the paused-clock harness (D2), with a
   sensitivity control proving the metric can fail (W3.6). Real
   wall-clock utilization is an M6 bar (>90% on 10⁴ real jobs).
5. **Event-sequence hash exclusions** — `producer_id` and per-session
   `seq` are nondeterministic by the platform's own contract; the
   determinism hash covers `(ts_logical, event_type, payload)` (D6).

## Evidence layout

`evidence/phase5-m3-m4/` (committed, small text only):

- `run-manifest.md` — command lines, seeds, counts, host + toolchain.
- `m3-acceptance.txt`, `m4-acceptance.txt` — summarized cargo test
  output (the M7 evidence-script approach): a
  `scripts/evidence-m3m4.sh` that runs the named suites with pinned
  seeds and tees trimmed summaries.
- `seed-gate.txt` — tree-hash + event-hash pairs for same-seed×2 (both
  arches when CI arm ran), different-seed sanity.
- `chaos.txt` — crash-point matrix × run counts, invariant assertion
  totals, the fresh-random-seed re-run the phases track will repeat.

## CI changes

- The existing `cargo test --workspace` step already picks up the new
  crates — no per-crate duplicate invocations (review finding). Add
  only an explicit named `seed_gate` invocation as the determinism
  gate (both matrix arms). Keep total runtime sane: chaos Tier-1 runs
  a reduced lattice in CI (every crash point × 5 seeds; verify budget
  fit) with the full ×50 behind an env var in the evidence script;
  Tier-2 is evidence-script-only (`#[ignore]`).
- Consider adding `clippy -D warnings` (plan-of-record M0 wanted it;
  currently absent). Low-risk, do it in W3.0 if the existing tree is
  already clean, else file a bead — don't let lint archaeology block
  M3.

## Handback checklist (what the phases track re-verifies)

Matches request `03-…` §1–4:

1. Cold `cargo test --workspace` + named acceptance tests + seed gate
   ×2 from a clean checkout (x86_64; aarch64 per CI availability).
2. Chaos spot-check with a fresh random seed — supported by
   `chaos_resume.rs` taking a `CHAOS_SEED` env override.
3. Expansion-path contract re-exercised through real M3 path —
   `expansion_context.rs` (W3.6).
4. Purity boundaries — `purity_guard` green; no
   tokio/tonic/fs/wall-clock in `orch-clients`/`orch-fakes` (the new
   observatory module included).

## Definition of done

- All W-items landed as reviewed commits on `main`, pushed; beads
  closed; `04-resolution.md` written with SHAs, decisions, staged-out
  beads; evidence directory populated; CI green on both arches.
- Request closes only when the phases track fills
  `05-verification.md` (their step, not ours).
