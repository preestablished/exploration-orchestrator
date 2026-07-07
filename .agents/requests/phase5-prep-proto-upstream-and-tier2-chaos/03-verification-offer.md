# Cross-Repo Verification

## Control-Plane's Half (Proto)

The companion request
(`../control-plane/.agents/requests/phase4-proto-freeze-tag-and-breaking-gate/`)
asks control-plane to add `buf lint`/`buf breaking` CI, aarch64 coverage,
and the `proto-v*` tag their plan requires. Verification of item 1 is
therefore two-sided and cheap:

- their CI goes green with your `orchestrator/v1` files in the tree and
  the breaking gate active;
- your CI goes green consuming the canonical location;
- one demonstration `buf breaking` failure (scratch branch deleting a
  field) proves the gate actually guards your surface.

Whichever repo lands second runs the demonstration and records it in both
request dirs.

## Phases-Track Check (Chaos)

For item 2 the phases track will re-run the Tier-2 harness from a clean
checkout at a seed of our choosing and confirm the bit-identical
continuation independently — same discipline as the M3/M4 evidence
re-verification. Append your `04-resolution.md` here with the harness
invocation and evidence paths; we respond with `05-verification.md`.

## Handback Shape

Same convention as `phase5-entry-m3-m4-runner-on-fakes/`: resolution here
(`04-`), with git SHAs (both repos for item 1), bead dispositions,
evidence paths, and any disclosed reinterpretations — the D5-style
disclosure discipline from the M3/M4 resolution worked well; keep it.

## Contact / Tracking

- Beads covered: `exploration-orchestrator-777`, `exploration-orchestrator-6ft`.
- Companion request: control-plane
  `phase4-proto-freeze-tag-and-breaking-gate` (filed the same day).
- Downstream consumers to notify on item 1 landing: observatory (M1
  ingest, when that repo activates) — a pointer in the control-plane
  request dir suffices for now.
