# Verification, CI, Evidence, Handback

## Acceptance mapping

| Request acceptance | Plan coverage | Evidence |
|---|---|---|
| M5 beads filed and closed with evidence pointers | W5.0 tracking in `00-plan.md`, W5.16 resolution | bead closure text, `04-resolution.md` |
| Matrix covers every API.md section 7 invalid shape and exact strings | W5.1-W5.5 | `config-validation.txt`, `docs/config-validation-rejections.md` |
| Metrics complete per ARCHITECTURE section 10 with before/after diff | W5.6-W5.8 | `metrics-diff.txt`, metrics tests |
| CAS covers checkpoint-write and node-commit windows | W5.9-W5.11 | `cas-ownership.txt`, runtime reason runbook |
| 24 h K=64 fault-injected soak with leak/GC assertions and CI smoke | W5.12-W5.16 | `soak-24h.txt`, `soak-smoke.txt`, `run-manifest.md` |
| Soak runbook lists every FAILED reason string | W5.15 | `docs/runtime-terminal-reasons.md`, `failed-reason-census.txt` |

## CI changes

Add focused gates without making the 24 h lane part of CI:

- `cargo test -p orch-core config_matrix`
- `cargo test -p orch-server --test config_validation_surface`
- `cargo test -p orch-server --test metrics_surface`
- `cargo test -p orch-server --test cas_ownership_loss`
- `scripts/evidence-m5-soak.sh` with short smoke duration, or a CI wrapper that
  invokes the same script and stores trimmed output

Keep existing gates green:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test -p orch-server --test seed_gate
cargo test -p orchestratord --test tier2_chaos -- --nocapture
```

The full 24 h soak is manual evidence, not PR CI.

## Evidence rules

Evidence files should be committed, small, and grep-friendly:

- include command lines, env vars, seeds, host, rustc, commit SHA
- include final test summary lines
- trim cargo noise
- preserve failure reason census and assertion result summaries
- record absolute start/end timestamps for the 24 h lane

Do not claim a soak passed if timestamps show a shortened run. If a defect stops
the run early, commit the finding evidence, fix, then rerun.

## Phases-track spot checks to expect

The requester said they will re-run:

- matrix and metrics completeness tests from a clean checkout
- soak evidence audit for continuous timestamps and whole-run assertions
- three FAILED reason end-to-end spot checks

Make that easy:

- Keep test names stable and documented in the resolution.
- Add env vars for forcing each runtime FAILED reason where possible.
- Make `docs/runtime-terminal-reasons.md` grep-able by prefix, with a clear
  `FAILED` subset section.

## Final handback shape

Implementation should append
`.agents/requests/phase5-m5-hardening-on-fakes/04-resolution.md`. The phases
track will respond with `05-verification.md`.

Session close remains the repo's normal workflow:

```bash
git status
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
git add <files>
git commit -m "..."
git pull --rebase
bd dolt push
git push
git status
```
