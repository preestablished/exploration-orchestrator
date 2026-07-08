# Prometheus Metrics

Scope: `ARCHITECTURE.md` section 10 plus the prose-only observatory drop counter.
Current state has scheduler atomics in `orch-sched/src/metrics.rs`, placeholder
stats in `orch-server/src/service.rs`, and `orchestratord` serving only
`orchestratord_up 1`.

## W5.6 - Define a complete metric catalog and renderer

Add a catalog in code for the required metric families:

- `orch_expansions_total`
- `orch_nodes_total{verdict="kept"}`
- `orch_nodes_total{verdict="dup"}`
- `orch_nodes_total{verdict="regression"}`
- `orch_best_score`
- `orch_frontier_size`
- `orch_archive_cells`
- `orch_escalation_level`
- `orch_slot_utilization`
- `orch_pipeline_queue_depth{stage="submit"}`
- `orch_pipeline_queue_depth{stage="complete"}`
- `orch_jobs_failed_total`
- `orch_batch_latency_seconds{stage="select"}`
- `orch_batch_latency_seconds{stage="execute"}`
- `orch_batch_latency_seconds{stage="commit"}`
- `orch_observatory_dropped_total`

Add an explicit shared `MetricsRegistry` or equivalent snapshot path. It must be
passed into `ExperimentRunner`, `Pipeline::spawn`/`PipelineConfig`,
`EventEmitter`, `OrchestratorService`, and `serve_http`; current handles expose
status only, pipeline gauges are local to `Pipeline::spawn`, and `/metrics`
currently has no service/registry handle. Implement a renderer in `orch-server`
that emits Prometheus text from that registry. Keep the renderer
dependency-light; a hand-written renderer is fine if tests parse the output.
The registry/snapshot should be populated from:

- `StatusSnapshot` for expansions, nodes, frontier, best score, escalation,
  discarded children, and guest instruction stats.
- Scheduler `Gauges` for queue depths, job failures, and execute latency, wired
  from the same shared registry rather than lost inside the pipeline.
- New select-stage and commit-stage latency observations around request build /
  policy selection and C-stage commit work.
- `SlotView::utilization()` or a runner-owned sample for slot utilization.
- `EventEmitter::dropped_total()` / shared sink or an explicit atomic in the
  registry for observatory drops.
- Scorer/archive state for `orch_archive_cells`, if available; otherwise add
  runner bookkeeping when `ScoreBatch`/`ReplayCommits` updates the archive.

Acceptance:

- The catalog is code-visible and reused by tests.
- The metrics registry is reachable from the runner, pipeline, event emitter,
  served service, and HTTP responder without global mutable state.
- The renderer emits `# HELP`/`# TYPE` lines and numeric samples for every family.
- Histograms include `_bucket`, `_sum`, and `_count` for all required stages.

## W5.7 - Wire `/metrics` to live service state

Replace the placeholder in `bins/orchestratord/src/main.rs` with the renderer.
The HTTP handler needs access to live state:

- In `--simulate`, pass an `Arc<OrchestratorService<...>>` or a lightweight
  `MetricsRegistry` into `serve_http`.
- In `--experiment`, either serve metrics while the run is active if `--http` is
  supplied, or make the soak harness collect final metrics through the shared
  registry. Prefer serving during standalone too, because the soak should scrape
  live counters over 24 h.

Update `stats_to_wire` in `orch-server/src/service.rs` while in the same area:
the current `archive_cells`, `best_stage`, and `slots_utilization` placeholders
should be backed by real runner status where possible, or a documented zero only
when the fake layer truly cannot provide it yet.

Acceptance:

- `GET /metrics` returns the M5 surface while a fake run is active.
- A test starts the served fake daemon or service, performs a short run, scrapes
  `/metrics`, and sees nonzero `orch_expansions_total` and `orch_nodes_total`.
- Observatory drop counter increments when the fake sink is configured to reject
  or stall and the emitter ring overflows.

## W5.8 - Add metrics completeness tests and before/after evidence

Add a test that parses Prometheus text into families and label sets. Do not use
substring-only checks.

Suggested tests:

- `crates/orch-server/tests/metrics_surface.rs::metrics_catalog_is_complete`
- `crates/orch-server/tests/metrics_surface.rs::observatory_drops_are_exported`
- `bins/orchestratord/tests/http_metrics.rs::simulate_metrics_endpoint_serves_catalog`

Record a before/after metric-name diff in
`evidence/phase5-m5-hardening/metrics-diff.txt`. Before should show only
`orchestratord_up`; after should show the full family set.

Acceptance:

- Tests fail if a required metric family or required label value disappears.
- The evidence diff is committed and linked from the resolution.
