# Cross-Repo Verification

## The Conformance Oracle

Observatory ships `obs-events-gen` (its M1 synthetic producer / load
harness) with two vocabulary profiles:

- `--vocab catalog` — strict API.md §2.1 field shapes as amended. This is
  the reference stream: diff your emitted payload JSON against it
  field-by-field for each event type, or replay it through your own
  decoding to confirm shape agreement.
- `--vocab orch-asbuilt` — byte-shapes of your `events.rs` builders
  today, kept as observatory's tolerance-path regression fixture. When
  you conform, this profile becomes a historical artifact on our side;
  you don't need to keep compatibility with it.

Generation is seeded and deterministic
(`obs-events-gen generate --seed N --vocab catalog --out stream.jsonl`),
so a fixture stream can be pinned by seed in your tests without checking
in bytes.

## Joint Smoke (once your tonic sink exists)

Observatory's ingest server (`EventIngest` on `:7470`) plus
`obs-events-gen verify` gives an end-to-end check: run your emitter
against a live `observatoryd`, then compare counts/checksums and inspect
`obs_projection_partial_total{event_type}` on its `/metrics` — a fully
conformed producer drives that counter to zero for your event types. We
are glad to run this jointly as soon as both sides exist; file the
request back at `observatory/.agents/requests/` or just note it in
`04-resolution.md`.
