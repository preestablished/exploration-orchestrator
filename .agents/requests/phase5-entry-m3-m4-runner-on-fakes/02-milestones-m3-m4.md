# Scope and Acceptance: M3 then M4

Both milestones are specified in your
`~/.agents/projects/determinism/docs/exploration-orchestrator/IMPLEMENTATION-PLAN.md`;
the acceptance bars below are quoted from it in full so there is no
drift between this request and the plan of record. M4 is gated on M3.
Both run entirely against `orch-fakes` — no platform service, no Intel
box, no other repo's schedule can block this work.

## M3 — Worker driver + pipeline + retries (`orch-sched`)

Scope (plan §M3): the worker-driver module (lease-API composition per
API.md §2.2, including verdict mapping and determinism-class gating),
`SlotView` with `ListSlots`/`WatchSlots` handling, the S→E→C pipeline
with bounded queues, retry policy (purity-based for driver jobs,
`client_batch_id`-based for scoring), and fast + deterministic modes.

**Accept (against fakes with fault injection):**

> - Slot utilization > 95% in fast mode with simulated job latency
>   jitter ±50%.
> - Backpressure: slow FakeScorer ⇒ queue gauges cap at configured
>   bounds, memory flat.
> - Retries: 5% injected job failure rate ⇒ search completes, results
>   identical to a 0%-failure run **in deterministic mode** (purity ⇒
>   retries invisible).
> - Worker-pool shrink/grow mid-run handled without deadlock (loom or
>   tokio-test exercised on the lease path).

Note: the input-synth context smoke
(`orch-driver/tests/input_synth_context.rs`, see `01-…`) already proves
the parent/sibling-burst contract at the request-builder level. Once the
real M3 expansion path exists, re-exercise that contract through it (an
integration test driving expansion end-to-end), so the guarantee holds
where the runner will actually rely on it.

## M4 — Experiment runner end-to-end on fakes + checkpoint/resume

Scope (plan §M4): `ExperimentRunner` main loop; bring-up (compile →
LoadFeatureMap/LoadScoringProgram; LoadMacroPack + fingerprint);
bootstrap against `FakeHypervisor`'s Ready event; plateau detector +
full escalation ladder (L4 = re-bin via `rebin=true` on the fake); goal
handling; `CheckpointV1` save/load + WAL **with the scorer-archive
drain-lockstep** (CheckpointArchive/RestoreArchive + seq assertions);
served gRPC surface (all six RPCs); observatory `EventEnvelope`
emission; ExperimentConfig validation including standalone YAML loading.

**Accept:**

> - **Autonomy on fakes:** default config beats the grid-world "boss"
>   and reaches "credits" with zero scripted inputs, within budget, for
>   10/10 seeds.
> - **Plateau:** a grid world with a key hidden behind a long corridor
>   stalls L0→L1→… and the ladder measurably unsticks it (vs.
>   ladder-disabled control run, which fails the same budget).
> - **Kill -9 anywhere:** a chaos test SIGKILLs the process at random
>   points across 50 runs; resume always follows the binding sequence
>   (ARCHITECTURE.md §8.2: restore checkpoint → replay WAL →
>   `next_node_id = 1 + max(node_id)` from the store → frontier from
>   FRONTIER rows with checkpoint weights → `RestoreArchive` +
>   `archive_seq` assertion + `ReplayCommits`) and completes with a
>   valid tree: zero node-id reuse (no `ALREADY_EXISTS`), no stranded
>   FRONTIER rows (every store FRONTIER row is in the resumed frontier
>   — post-checkpoint commits adopted with recomputed default weights),
>   no double-committed nodes (`CreateNode` idempotency +
>   `client_batch_id` dedup + `ReplayCommits`-driven duplicate verdicts
>   exercised), and in deterministic mode the final tree hash equals
>   the uninterrupted run's.
> - **Seed reproducibility (CI determinism gate, MAP convention):**
>   deterministic mode, same seed, runs twice (and across x86_64 vs
>   aarch64 runners) ⇒ identical tree hash (hash over (node_id, parent,
>   state_hash, score, cell_key) in id order) and identical event
>   sequence. Different seeds ⇒ different trees (sanity).
> - Fast mode: same seed twice ⇒ trees may differ (assert the
>   *trajectory replay* invariant instead: every committed node's path
>   re-derives via FakeHypervisor).
> - Pause→checkpoint→resume across a process restart preserves status
>   and stats.

## Why these bars matter downstream (Phase 5 gate mapping)

- The **kill-anywhere** bar is Phase 5 exit-gate item 2 ("Resumable"),
  rehearsed on fakes before the real gate run.
- The tree-hash and event-sequence determinism bars are what make the
  Phase 5 gate's replay verification (item 3) diagnosable — the tree
  must be "born replay-ready."
- The pipeline/backpressure bars are the precondition for Phase 5's
  throughput floor (item 5, >80% slot busy over a 4-hour soak) and for
  M6's `10⁴ real driver jobs at >90% utilization` bar.
- Observatory `EventEnvelope` emission is what lets observatory M1 start
  integrating "as soon as both sides exist" (Phase 5 work list — which
  attributes the canonical event stream to M5; the implementation plan
  puts emission in M4, and M4 governs here. Either way, earlier is
  better for observatory).

## Sequencing and boundaries

1. M3 first, M4 gated on it (plan ordering).
2. Crate placement is a decision point, not a directive: ARCHITECTURE.md
   §1 names `orch-sched` (scheduler/pipeline), `orch-checkpoint`
   (checkpoint/WAL), `orch-server` (runner + tonic surface), and
   `bins/orchestratord`; the repo's actual layout grew `orch-driver`
   instead, which the architecture doc does not mention. `orch-sched`
   for M3 follows the plan naming; whether M4's runner/checkpoint/gRPC
   code splits per the architecture doc or extends `orch-driver` is
   your call — state the choice and the doc drift in `04-resolution.md`
   so it is reviewable. Keep `orch-core` pure and
   `orch-clients`/`orch-fakes` transport-free regardless.
3. M5 (config-matrix hardening, full Prometheus surface, CAS
   ownership-loss, 24 h soak) is explicitly **out of scope** here — but
   design the queue gauges and FAILED-reason strings so M5's "grep-able
   runbook of every FAILED reason string" bar doesn't force a refactor.
4. Update `README.md`'s placeholder language for `orch-driver` when the
   scope statement stops being true.
