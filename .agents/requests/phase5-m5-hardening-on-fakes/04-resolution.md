# M5 Hardening on Fakes Resolution

Resolved: 2026-07-09

## Beads

| Bead | Disposition | Evidence |
|---|---|---|
| `exploration-orchestrator-4rz` | Parent epic, closed by this resolution. | This file plus all evidence below. |
| `exploration-orchestrator-aax` | Closed: config validation matrix and string freeze. | `evidence/phase5-m5-hardening/config-validation.txt`, `docs/config-validation-rejections.md` |
| `exploration-orchestrator-0ro` | Closed: Prometheus metrics completeness. | `evidence/phase5-m5-hardening/metrics-diff.txt` |
| `exploration-orchestrator-2gl` | Closed: CAS ownership-loss path. | `evidence/phase5-m5-hardening/cas-ownership.txt`, `docs/runtime-terminal-reasons.md` |
| `exploration-orchestrator-189` | Closed: 24 h K=64 fault-injected fake soak and runbook. | `evidence/phase5-m5-hardening/soak-24h.txt`, `evidence/phase5-m5-hardening/soak-smoke.txt`, `evidence/phase5-m5-hardening/run-manifest.md`, `evidence/phase5-m5-hardening/failed-reason-census.txt` |

## Commits

| Commit | Subject | Scope |
|---|---|---|
| `8d45361` | Implement M5 hardening surfaces | Config validation matrix, metrics surface, CAS ownership-loss path, runtime terminal reason runbook, initial soak smoke. |
| `ca5cda2` | Harden M5 soak evidence harness | Fake fault counters, one-shot transient fake service errors, RSS/evidence script hardening, periodic fake retention. |
| `c0645cb` | Keep M5 soak alive for duration | Fail-fast early runner exit check and non-exhausting soak limits. |
| `af8b2dd` | Bound M5 soak fake world | Two-cell fake world for bounded long-run state, committed as the provenance commit for the 24 h evidence. |
| This resolution commit | Record M5 24h soak evidence | Adds the completed 24 h evidence, resolution, and bead closeout. |

## Evidence

- `evidence/phase5-m5-hardening/config-validation.txt`
- `evidence/phase5-m5-hardening/metrics-diff.txt`
- `evidence/phase5-m5-hardening/cas-ownership.txt`
- `evidence/phase5-m5-hardening/soak-smoke.txt`
- `evidence/phase5-m5-hardening/soak-24h.txt`
- `evidence/phase5-m5-hardening/run-manifest.md`
- `evidence/phase5-m5-hardening/failed-reason-census.txt`
- `docs/config-validation-rejections.md`
- `docs/runtime-terminal-reasons.md`

## 24h Soak

Evidence file: `evidence/phase5-m5-hardening/soak-24h.txt`

Run facts:

- Provenance commit: `af8b2dd1edf009295b33ddf4588724bc987269d7`
- Start: `2026-07-08T23:00:27Z`
- End: `2026-07-09T23:00:43Z`
- Elapsed wall seconds: `86416`
- Requested duration seconds: `86400`
- K: `64`
- Seed: `24069`
- Fault seed: `1024369`
- Config hash: `a4e431d4e1a528ad60e06647e63bbf313904e112b8e954794f2608bf53ee71eb`
- GC cadence: every `4` commits
- Rust: `rustc 1.96.1 (31fca3adb 2026-06-26)`
- Host: `infra-control`

Fault plan:

- Deterministic latency with base `1` and jitter `3`.
- One-shot transient `Unavailable` faults on `hypervisor:run`, `scorer:score_batch`, `store:put_metadata`, `synth:propose_bursts`, and `observatory:emit`.
- Tier-2 persistence/kill hooks were not used in this 24 h lane.

The chosen W5.13 interpretation is the acceptable split lane: the 24 h soak is a journal-less Tier-1 service-fault lane with deterministic fake service faults active. Tier-2 journal/SIGKILL evidence remains a separate lane because journaled fake reload currently requires disabled service fault plans for digest soundness unless fault plans are persisted in the journal header.

## Soak Assertions

- Full duration held: `elapsed_wall_seconds=86416` is greater than the requested `86400`.
- Runner progress held until Stop: `expansions=10793`, `nodes=2`.
- Final status had no unexplained terminal failure: `failed_reason=none`.
- Final checkpoint was required, decoded by the Rust test, and checked against the observed outcome for status, expansions, node budget usage, expansion budget usage, and batch sequence.
- Fault injection fired on every fake service:
  `hypervisor_terminal=1`, `scorer_terminal=1`, `store_terminal=1`, `synth_terminal=1`, `observatory_terminal=1`, with nonzero decisions and latency counts for all five.
- Snapshot-refcount invariant passed:
  `committed_refs=2`, `pre_gc_orphans=47`, `post_gc_live=2`.
- Periodic fake retention ran:
  `periodic_gc_runs=2698`, `periodic_gc_orphans=467090`, `periodic_gc_max_orphans=197`, `retention_busy_skips=0`.
- Fake event buffers were compacted:
  `watch_events_compacted=2762758`, `observatory_events_compacted=711852`.
- RSS check passed:
  `samples=2851`, `warmup_omitted=120`, `evaluated_samples=2731`, `min_kib=72628`, `max_kib=96408`, `delta_percent=32.74` under the configured `50` percent tolerance.

## Runbook

Runtime terminal reason runbook: `docs/runtime-terminal-reasons.md`

FAILED-string census: `evidence/phase5-m5-hardening/failed-reason-census.txt`

Observed FAILED reasons in the 24 h lane: none observed.

## Final Verification

Final gates run after adding the resolution:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`

All passed on 2026-07-09. The workspace suite included the M5 smoke, config validation surface, metrics surface, CAS ownership-loss, seed gate, and Tier-2 chaos tests.

## Post-Review Follow-Up

On 2026-07-10, two review agents found evidence-harness gaps after the 24 h closeout. Follow-up plan: `./.agents/plans/phase5-m5-review-followups/00-plan.md`.

Follow-up implementation commit: `d38fce153bb9718e193956b68755547b57aeb7a3`.

Follow-up evidence:

- `evidence/phase5-m5-hardening/config-validation.txt`
- `evidence/phase5-m5-hardening/metrics-diff.txt`
- `evidence/phase5-m5-hardening/cas-ownership.txt`
- `evidence/phase5-m5-hardening/soak-smoke.txt`
- `evidence/phase5-m5-hardening/run-manifest-smoke.md`
- `evidence/phase5-m5-hardening/failed-reason-census-smoke.txt`

The regenerated config, metrics, and CAS evidence now stamps `d38fce153bb9718e193956b68755547b57aeb7a3`, which contains the referenced tests. The follow-up smoke lane proves real charged adapter latency for hypervisor/scorer/store/synth via `M5_SOAK_LATENCY_CHARGED`, checkpoint cadence via `checkpoint_generation >= checkpoint_min_generation`, and periodic retention/compaction via nonzero periodic counters. Smoke RSS is explicitly classified as not required (`RSS_EVIDENCE required=0`); the 24 h lane remains the RSS-bounded lane.

This follow-up does not replace the 24 h lane at `af8b2dd1edf009295b33ddf4588724bc987269d7`. It narrows the old latency claim: the 24 h evidence proves fake latency decisions and retry behavior under service faults, while the 2026-07-10 smoke evidence proves the harness now charges adapter sleeps for those latency decisions on the async fake service adapters.

## Observatory Handoff

- The canonical orchestrator-side event stream surface remains the external docs snapshot at `/home/infra-admin/.agents/projects/determinism/docs/exploration-orchestrator/API.md` section 6 plus the local transport-free boundary in `crates/orch-clients/src/observatory.rs`.
- `exploration-orchestrator-75z` remains the unresolved EventEnvelope divergence item: the current local postcard `EventEnvelope` surface and the `observatory/v1` proto `payload_json` envelope are not reconciled by M5.
- M5 changed metrics coverage and runtime failure taxonomy. It did not introduce a third event shape and did not change the event payload schema.

## Phase README Correction

The phase README correction from D5.8 was applied outside this repository at `/home/infra-admin/.agents/projects/determinism/phases/README.md`:

- The early-start prose now says orchestrator fakes span phases 3-5.
- The graph now says `orch M1-M5 on fakes`.
- The repo/phase matrix now lists `exploration-orchestrator` as `(M1-M5)` under P3.

That docs tree is not a git repository, so this repo commit records the external edit but cannot include it.

## Second Review Pass (2026-07-16)

Two further review subagents audited the closeout (bead
`exploration-orchestrator-x6o`). The evidence audit found no blocking
findings; the plan-vs-implementation audit found one unmet acceptance
criterion and several documentation gaps. Fix commit:
`2efeccd3551e441c31d0389801719fc878723e5b`.

Applied:

- **Uncataloged FAILED passthrough (plan W5.15):** the generic terminal arm
  copied raw `error.message()` into `failure_reason`. Uncataloged messages
  are now wrapped as `runtime-error: <message>` via
  `orch_core::runtime_reasons::cataloged_failed_reason` (idempotent;
  cataloged prefixes pass through unchanged). The prefix is in the code
  catalog, `docs/runtime-terminal-reasons.md`, the doc drift test, and new
  wrap unit tests.
- **Soak census capture:** the soak harness now prints an
  `M5_SOAK_TERMINAL ... failed_reason=` line before panicking on early exit
  or a non-Stopped outcome, so `scripts/evidence-m5-soak.sh`'s FAILED-reason
  census can record findings instead of losing them to the panic.
- **Evidence host stamp:** the config/metrics/CAS evidence scripts now emit
  the `host:` line required by the plan's evidence rules.
- **`budgets.max_wall_clock_s = 0`:** the frozen accepted semantics (zero
  disables the wall-clock budget, guarded by `> 0` at the budget check) are
  now recorded in `docs/config-validation-rejections.md`.

Recorded interpretations (no code change):

- **CI gates:** M5 did not add the focused CI steps listed in
  `06-verification.md`; all four test suites plus the 10 s soak smoke run
  under the existing `cargo test --workspace --all-features` CI step. The
  evidence scripts (duration/RSS/census assertion layer) remain manual
  evidence lanes, matching the plan's "script or CI wrapper" option only in
  the workspace-test interpretation.
- **Soak entrypoint substitution:** both soak lanes drive the in-process
  `m5_soak` test via `scripts/evidence-m5-soak.sh`, not
  `orchestratord --experiment` over HTTP. Monotone `orch_expansions_total`
  growth is evidenced indirectly (end-state expansions, checkpoint
  `batch_seq` cross-check, generation lower bound), and RSS sampling reads
  the test process, not `/metrics`.
- **24 h lane caveats:** the two-cell fake world (`af8b2dd`) bounds the
  24 h tree to `nodes=2`, so committed-ref invariants are correspondingly
  narrow even though orphan GC churned 467k snapshots. Charged-call vs
  decision counters may skew slightly (e.g. off-by-one on one-shot terminal
  decisions); the evidence claims only nonzero charged calls/ticks.

Config, metrics, CAS, and soak-smoke evidence were regenerated from
`2efeccd` (rustc 1.97.1). The 24 h closeout files (`soak-24h.txt`,
`run-manifest.md`, `failed-reason-census.txt`) are unchanged and still
stamp the `af8b2dd` lane.
