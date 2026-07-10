# Plan: M5 Review Follow-ups

Addresses the two-subagent review findings filed after M5 closeout. Tracking
authority is beads:

| Bead | Priority | Finding |
|---|---:|---|
| `exploration-orchestrator-5v5` | P1 | M5 soak counts latency decisions but does not charge adapter latency. |
| `exploration-orchestrator-f6n` | P1 | Config, metrics, and CAS evidence stamp stale provenance. |
| `exploration-orchestrator-cju` | P1 | Checkpoint cadence and periodic retention are not strongly evidenced. |
| `exploration-orchestrator-nul` | P2 | RSS evidence can pass with too few evaluated samples. |
| `exploration-orchestrator-ggy` | P3 | One-shot fake fault attempt semantics are undocumented/untested. |

## Objective

Repair the false-positive evidence risks without widening M5 into a new platform
lane. The result should make the smoke harness prove the same properties it
claims, regenerate stale evidence from a reproducible commit, and clearly
separate full 24 h evidence from smoke-only follow-up evidence.

## Decisions

### D1 - Use the existing `LatencyProbe` seam

`orch-sched::ports::SyncAdapter` already supports caller-provided latency
probes and documents that fake-specific probes live in tests. Add a test-only
probe in `crates/orch-server/tests/support/mod.rs` that owns a cloned
`FaultInjector`, uses the same `(target, operation, request_identity)` stream as
the wrapped fake, and consumes its mirror stream with `decide` inside
`pending_call`.

Do not use `peek` by itself: `peek` does not consume attempts, while the fake
service does. The probe must also record charged-call counters and charged-tick
totals so evidence can distinguish actual adapter sleeps from fake decision
counts.

Use it only for fake-backed test worlds. Do not make `orch-sched` depend on
`orch-fakes` outside dev/test code.

### D2 - Keep observatory latency semantics explicit

The observatory fake is a synchronous `EventSink`, not a `SyncAdapter`. The M5
soak will not add blocking sleeps to `SharedSink`, because the event emitter
contract is synchronous and never-blocking. Narrow the evidence wording to say
adapter latency is charged for the four async service adapters while observatory
latency remains a fake decision/stat.

### D3 - Assert cadence with checkpoint metadata generation

The fake store already increments checkpoint metadata generation on each
successful checkpoint write. The soak can assert and print:

- final `checkpoint_generation`
- `checkpoint_min_generation`
- `checkpoint_every_commits`

Minimum generation should cover initial checkpoint, commit-cadence checkpoints,
and final stop checkpoint. Time-based checkpoints may add more writes, so assert
`generation >= min`, not equality.

### D4 - Periodic retention must be exercised by smoke

Change both the Rust test default and the script default for
`M5_SOAK_GC_EVERY_COMMITS` to `4`, matching the 24 h lane that passed. Assert
nonzero periodic retention/compaction counters for the soak test so a passing
direct `cargo test` or script smoke cannot skip that path. Keep final fake GC
equality as the stronger end-state invariant.

### D5 - RSS is required only when declared

Add an explicit script flag, defaulting to required for 24 h lanes and not
required for smoke lanes:

- `M5_SOAK_REQUIRE_RSS_EVIDENCE=1|0`
- `M5_SOAK_MIN_RSS_EVALUATED_SAMPLES=4`

When RSS evidence is required, fail unless enough post-warmup samples exist and
the tolerance passes. When it is not required, still print the RSS summary and
record that RSS evidence was not required.

### D6 - Preserve 24 h manifest when running smoke

Smoke reruns should not overwrite the 24 h manifest/census that backs the M5
resolution. Add lane-specific manifest/census files for smoke follow-up evidence
and keep `run-manifest.md` / `failed-reason-census.txt` as the 24 h closeout
files unless a 24 h lane is being run.

### D7 - Evidence provenance requires an implementation commit

Regenerated evidence scripts stamp `git rev-parse HEAD`. To make the stamped
commit reproducible, first commit the plan and code/test/script changes, then
run the evidence scripts from that commit, then commit the regenerated evidence
and any resolution addendum.

Add a clean-tree guard to every M5 evidence script so future evidence cannot be
generated from uncommitted code while still stamping a clean commit. Evidence
files themselves may be dirty from the script write; the guard must run before
overwriting outputs.

## Work Items

### W1 - Plan review

Write this plan and have two subagents review it before implementation. Revise
the plan if either review finds a blocking gap.

### W2 - Real latency charging

Implement mirror fake latency probes for `FakeWorld::with_service_plans` or a
soak-specific constructor. Cover hypervisor, scorer, snapshot store, and synth.
Keep observatory as fake-decision latency only.

Expose probe accounting to the soak test and print a separate
`M5_SOAK_LATENCY_CHARGED` line with per-adapter charged calls and charged ticks.
Assert the charged counts are nonzero for the four async adapters. Add or extend
tests so multi-call probe accounting consumes the same attempt stream shape as
the fake fault injector.

### W3 - Checkpoint cadence and retention assertions

In `m5_soak.rs`, assert:

- checkpoint metadata generation is at least the expected commit-cadence lower
  bound
- periodic retention ran
- watch-event and observatory-event compaction ran

Print the new checkpoint fields in `M5_SOAK_SUMMARY`.

### W4 - RSS script hardening

In `scripts/evidence-m5-soak.sh`, add RSS-required/min-sample controls and
fail required lanes with too few evaluated samples. Make smoke evidence explicit
about RSS not being a leak-proof lane unless the env opts in.

### W5 - One-shot semantics

Document the current `with_one_shot_error` semantics and add a unit test pinning
the intentional behavior: one-shot errors fire on the second call of the
`(target, operation)` stream (`attempt == 1`), not the second retry of a specific
request identity. If implementation discovers first-call behavior works better
without breaking bring-up, update code and tests instead.

### W6 - Evidence regeneration

After the implementation commit:

- regenerate config validation evidence
- regenerate metrics evidence
- regenerate CAS ownership evidence
- run a short M5 soak evidence lane with real adapter latency and periodic
  retention exercised

Do not rerun the 24 h lane as part of this follow-up unless the smoke lane
surfaces a defect that invalidates the prior 24 h result. If the old 24 h
evidence remains limited to pre-fix latency semantics, add a resolution addendum
stating exactly what the follow-up smoke proves.

### W7 - Verification and closeout

Run:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- focused tests touched by the change
- `cargo test --workspace --all-features` if runtime permits after focused
  tests pass

Before evidence regeneration, verify the implementation commit is clean with
`git status --short --branch`. After smoke regeneration, verify
`run-manifest.md` and `failed-reason-census.txt` still point at the 24 h lane
unless the rerun was a 24 h lane.

Close the five follow-up beads with evidence pointers, run `bd dolt push`, and
commit/push all relevant changes.

## Expected Evidence

New or updated evidence should include:

- regenerated `config-validation.txt`, `metrics-diff.txt`, `cas-ownership.txt`
  with a commit that contains the tested code
- updated `soak-smoke.txt` proving real adapter latency, checkpoint generation
  lower bound, periodic retention, and compaction
- `M5_SOAK_LATENCY_CHARGED` in smoke evidence with nonzero hypervisor, scorer,
  store, and synth charged calls/ticks
- lane-specific smoke manifest/census if smoke is rerun after the 24 h closeout
- an addendum in `.agents/requests/phase5-m5-hardening-on-fakes/04-resolution.md`
  or a sibling follow-up note explaining the post-review evidence
