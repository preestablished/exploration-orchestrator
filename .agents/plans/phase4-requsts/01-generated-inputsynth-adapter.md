# 01 Generated InputSynth Adapter

Goal: implement a real generated-proto client for `determinism.inputsynth.v1.InputSynthesizer` while preserving the existing transport-free trait boundary.

Prerequisite:

- Control-plane must publish the complete generated Rust/prost/tonic facade for `determinism.inputsynth.v1`, including the tonic client module, all request/response messages, all enums, `NodeContext`, `ProvenancedBurst`, `ScoredBurst`, and `MineMacros` messages.
- Do not treat the current `determinism_proto::inputsynth::v1` stub as usable. Block until the facade exposes the real tonic client, likely `input_synthesizer_client::InputSynthesizerClient`, plus all v1 messages/enums.
- If that facade is not present, stop this slice after adding a clear Beads blocker that names the missing generated symbols. Do not invent local proto replacements.

Crate placement:

- Update `crates/orch-proto` to re-export the generated input-synth v1 module under an explicit namespace such as `orch_proto::inputsynth::v1`. Do not add a second ambiguous top-level `v1`.
- Implement the adapter in `crates/orch-driver/src/input_synth.rs`.
- Keep `orch-clients` DTOs and traits synchronous and transport-free.
- Let `orch-driver` own tonic, tokio runtime/handle usage, deadlines, and status mapping.

Adapter shape:

- Add a `GeneratedInputSynthClient` or similarly named wrapper around the tonic `InputSynthesizerClient<Channel>`.
- Implement `orch_clients::input_synth::InputSynthClient` for that wrapper.
- Bridge async tonic calls from the existing sync trait with an owned or borrowed runtime handle in `orch-driver`. Avoid pulling `tokio`, `tonic`, or generated service clients into `orch-clients`.
- Map tonic errors into `ClientErrorKind` consistently:
  - `invalid_argument` -> `InvalidRequest`
  - `failed_precondition` -> `FailedPrecondition`
  - `not_found` -> `NotFound`
  - `already_exists` -> `AlreadyExists`
  - `resource_exhausted` -> `ResourceExhausted`
  - `unavailable`, `deadline_exceeded`, transport connect failures -> `Unavailable`
  - malformed successful responses, wrong lengths, seed echo mismatch, or deterministic contract violations -> `DataLoss`
  - everything else -> `Internal`

Required public helpers:

- DTO to wire request converters for `LoadMacroPackRequest`, `HealthRequest`, `ProposeBurstsRequest`, and `MineMacrosRequest`.
- Wire to DTO response converters for `LoadMacroPackResponse`, `HealthResponse`, `ProposeBurstsResponse`, and `MineMacrosResponse`.
- Implement the adapter method for `mine_macros`. If the generated facade lacks `MineMacros`, return a deliberate `ClientErrorKind::FailedPrecondition` or block the slice; do not leave a placeholder that compiles but silently drops the call.
- A small adapter config type containing endpoint, deadline, and retry budget. Keep retries outside the raw tonic conversion helpers so tests can exercise response validation without network.

Out of scope for this slice:

- Search scheduling.
- Snapshot-store writes.
- Fingerprint retry policy. That belongs in `03-bringup-seeds-fingerprints.md`.
- Any fake transport server unless needed for adapter integration tests.

Definition of done:

- `orch-driver` can compile against the real generated input-synth facade.
- The generated adapter implements the existing `InputSynthClient` trait.
- `orch-clients` and `orch-fakes` have no tonic/tokio dependency.
- Error mapping is covered by unit tests using direct converter/status helpers, with network tests behind an opt-in feature if needed.
