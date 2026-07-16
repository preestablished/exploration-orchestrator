# Request: Conform The Event Emitter To The Canonical Observatory Contract Before M5's Real Sink

## Who Is Asking

Observatory M1 ingest design, executing the reconciliation your repo
flagged as bead `exploration-orchestrator-75z` (cited verbatim below) and
that `phase5-prep-proto-upstream-and-tier2-chaos` item 1 deferred to us
("Flag, don't fix, the EventEnvelope divergence"). Filed 2026-07-16.

The bead:

> orch-clients/src/observatory.rs EventEnvelope (postcard payload map,
> producer_id, ts_logical, seq-excluded-from-hash per D6) diverges from
> control-plane proto/determinism/observatory/v1/events.proto (payload_json
> string; no producer_id/ts_logical). Owner of the reconciliation:
> observatory M1 ingest design — the canonical proto likely needs
> producer_id + ts_logical and a decision on payload encoding; our emitter
> then converts at the wire boundary. Do not change orch-clients DTO
> semantics unilaterally. Flagged from request
> phase5-prep-proto-upstream-and-tier2-chaos item 1.

The reconciliation is now executed. Decisions live in
`observatory/docs/event-contract-reconciliation-v1.md` (D1–D9); the real
`observatory.proto` (full envelope, both RPCs) replaces the 5-field
placeholder in control-plane as part of the same wave.

## Why Now

Your M5 hardening on fakes is done; the next observatory-facing step on
your side is the real tonic `EventSink` (the wire adapter your
`observatory.rs` header anticipates). **Conforming before that adapter
exists is free — after it exists it is a wire migration.** Every payload
builder brought to catalog shape now is one the adapter never has to
translate.

Two of the divergences are conceded to you (no orchestrator change asked):

- `node-pruned` without a `node_id` — the catalog now makes `node_id`
  optional and adds `parent_id` (decision D4). Your id-less pre-commit
  prune emissions are legal as-is.
- Gap-tolerant acks — the canonical ack wording now matches your
  drop-oldest ring's behavior (decision D7). Your
  `emitter_never_blocks_and_drops_oldest_on_outage` test is correct
  against the amended contract; no code change expected.

The rest (envelope fields, payload field shapes, JSON encoding at the wire
boundary) is asked of you in `02-requested-work.md` — your own API.md §6
already promises "Payload field shapes are the catalog's", so this is
closing a gap against your own spec, not accepting a new obligation.

## Contents

- `01-current-state.md` — the field-by-field divergence audit (envelope +
  all ten payload builders), file:line evidence at your `7f97fca`.
- `02-requested-work.md` — the three asks, with the conceded items marked.
- `03-verification-offer.md` — what observatory provides to verify
  conformance cheaply.
- `04-resolution.md` — yours to write.
