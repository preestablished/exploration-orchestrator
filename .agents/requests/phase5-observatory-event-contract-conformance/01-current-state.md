# Current State: The Divergence Audit

Audited 2026-07-16 against this repo at `7f97fca` and the canonical
contract (`~/.agents/projects/determinism/docs/observatory/API.md` §1
envelope + §2.1 catalog, as amended by reconciliation decisions D2/D4/D7 —
see `observatory/docs/event-contract-reconciliation-v1.md`).

Your own spec's standard applies throughout: orchestrator API.md §6
(line 568) states "Payload field shapes are the catalog's". The payload
rows below are therefore bugs by your own doc, not contract disputes.

## Envelope (`crates/orch-clients/src/observatory.rs:47-56`)

| Aspect | Canonical (API.md §1) | As-built |
|---|---|---|
| `envelope_version` | `uint32`, MUST be 1 | absent |
| `ts_wall_ns` | `uint64`, advisory | absent |
| `payload_version` | `uint32` per event_type | absent |
| payload | `bytes payload_json` — UTF-8 JSON **object**, ≤64 KiB | structured `Payload = BTreeMap<String, PayloadValue>` (`observatory.rs:38`; postcard-canonical — JSON encoding is the tonic adapter's job, per your own header comment) |
| `source_service` | `enum SourceService` (wire values renamed `SOURCE_SERVICE_*` per decision D2 for buf lint) | `String` (`"EXPLORATION_ORCHESTRATOR"`, `orch-server/src/events.rs:17`) |
| `run_id, seq, ts_logical, event_type, producer_id` | — | match semantically (seq restarts per session, `producer_id = "orchestratord-<startup_unix>"`, `ts_logical` = expansion index) |
| ack semantics | **amended (D7)**: highest seq committed in stream order, gaps permitted | drop-oldest ring creates seq gaps; your test `events.rs:334-357` asserts ack advances across a gap (seqs `[3,4,5,6]` acked 6) — **now correct against the amended contract**. One stale doc comment remains on your side: `observatory.rs:59-60` still says "highest contiguous sequence" |

## Payload builders (`crates/orch-server/src/events.rs`) vs catalog §2.1

| event_type | §2.1 fields (as amended) | as-built fields (builder line) | Delta |
|---|---|---|---|
| `node-added` | `node_id, parent_id?, snapshot_ref, depth, progress_score, novelty_score, cell_key: str?, stage, guest_time_ns, input_delta_bytes, expansion_idx, features?` | `node_id, parent_node_id, score, novelty, cell_key: u64, stage, features` (190-214) | missing `snapshot_ref, depth, guest_time_ns, input_delta_bytes, expansion_idx`; renamed `parent_node_id→parent_id`, `score→progress_score`, `novelty→novelty_score`; `cell_key` typed u64 not str |
| `node-pruned` | `node_id?: str, parent_id: str, reason` (**D4 concession** — optional id + parent_id now canonical) | `parent_node_id, reason, node_id?` (217-228) | rename `parent_node_id→parent_id` only; optionality now conforms |
| `best-score-improved` | `node_id, score, prev_best, expansion_idx` | `node_id, best_score, previous_best_score` (231-237) | renames + missing `expansion_idx` |
| `stall-detected` | `window_expansions, best_score, escalation_level, since_expansion_idx` | `expansions_since_improvement, window` (240-248) | mostly disjoint |
| `escalation-changed` | `from_level, to_level, expansion_idx` | `level, previous_level` (251-259) | renames |
| `goal-reached` | `node_id, goal_id, score, expansion_idx, path_len` | `node_id, score` (262-267) | missing `goal_id, expansion_idx, path_len` |
| `batch-completed` | `batch_seq, kept, dups, regressions, failed_jobs, batch_wall_ms` | `batch_seq, parent_node_id, committed, discarded` (270-282) | mostly disjoint |
| `checkpoint` | `checkpoint_id, expansion_idx, frontier_size, tree_nodes, archive_cells, seen_set_size` | `batch_seq, expansions, archive_seq` (285-291) | disjoint |
| `assertion-violated` | `node_id?, assertion_id, message, guest_pc?, beacon_seq?` | generic `sdk_event_payload`: `node_id, stream, payload: bytes` (296-305) | raw relay, undecoded (see D8) |
| `reachability-hit` | `node_id, reachability_id, first_hit, expansion_idx` | same generic relay | same |

Emission sites verified in `crates/orch-server/src/experiment.rs`:
`node-added` 1619/2045; `node-pruned` 1608/1683/1711/1952/1987/2001/2102
(id-less at 1684/1712/1988/2002/2103); `best-score-improved` 2180;
`stall-detected` 2192; `escalation-changed` 2203; `goal-reached`
1752/2134; `batch-completed` 1722/2112; `checkpoint` 2462;
`assertion-violated` 1637; `reachability-hit` 1639. The ten names match
the catalog exactly — the vocabulary itself has no drift.

## How observatory ingests you TODAY (so nothing is blocked on this request)

Ingest is tolerant by design (decision D3): missing/renamed payload fields
are never rejected — projections extract what they can, fall back to
defaults, and increment `obs_projection_partial_total{event_type}`. Your
current shapes will land and display, degraded (e.g. no tree depth, no
score-curve expansion_idx precision, checkpoint gauges empty). Structural
violations only (`envelope_version != 1`, non-JSON-object payload,
>64 KiB, empty `run_id`) are rejected per-seq. There are deliberately no
rename shims: the partial-projection metric is the drift signal, and the
fix belongs here, on the producer.
