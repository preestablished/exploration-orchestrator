# Requested Work

Target: before your M5 exit (the real tonic `EventSink`). All three items
are producer-side; observatory's ingest tolerates the current shapes
meanwhile (see `01-current-state.md`).

## 1. Conform the payload builders to the catalog as-amended

Bring `crates/orch-server/src/events.rs` builders to API.md §2.1 field
shapes. The catalog moved toward you where reality won:

- `node-pruned`: **no behavioral change needed** — `node_id` is now
  optional and `parent_id` is canonical (decision D4). Only the
  `parent_node_id` → `parent_id` key rename remains.
- `assertion-violated` / `reachability-hit`: the typed shapes stay
  canonical (decision D8). Either decode/enrich the SDK relay to the
  typed fields, or drive the guest-sdk contract to define the decode —
  your call which; until then observatory stores the raw relay and
  projects a degraded finding (`summary = "undecoded guest-sdk relay"`).

Everything else is data you already have (snapshot_ref, depth,
expansion_idx, checkpoint stats, batch kept/dups/regressions/…): emit it
with the catalog's names and types (`cell_key` as string; scores as
`progress_score`/`novelty_score`; etc.).

## 2. Extend the runtime envelope / wire adapter

At the tonic boundary (or in the DTO, your choice per the bead's "do not
change DTO semantics unilaterally" — the adapter can supply these):

- `envelope_version = 1`
- `ts_wall_ns` (producer wall clock, ns — advisory only)
- `payload_version = 1`
- payload encoded as a UTF-8 JSON **object** into `payload_json` bytes
  (≤64 KiB). Your postcard-canonical `Payload` map already has
  deterministic ordering; serialize it to JSON with the same BTreeMap
  ordering and the bytes are canonical too.
- `source_service` as the proto enum
  (`SOURCE_SERVICE_EXPLORATION_ORCHESTRATOR = 1`; values renamed with the
  `SOURCE_SERVICE_` prefix per decision D2).

The canonical proto now exists in full in
`control-plane/proto/determinism/observatory/v1/events.proto` — pin:
see `04-resolution.md` placeholder below; observatory records the merge
SHA + blake3 in `observatory/docs/event-contract-reconciliation-v1.md`
(proto-pin section) once landed. The generated client is
`determinism_proto::observatory::v1::event_ingest_client::EventIngestClient`
behind the `observatory` feature.

## 3. Adopt the amended ack wording (docs only)

Decision D7 legitimizes your drop-oldest behavior: `acked_seq` is now
"highest seq committed in stream order per producer session (gaps
permitted)". Your `emitter_never_blocks_and_drops_oldest_on_outage` test
already assumes exactly this — **no code change expected**. One stale doc
comment is yours to fix alongside: the `EventSink` trait doc
(`crates/orch-clients/src/observatory.rs:59-60`) still says
"acknowledge the highest contiguous sequence".
