# 02 DTO/Wire Contract Goldens

Goal: prove that the orchestrator DTOs and the generated `determinism.inputsynth.v1` wire types round-trip exactly enough for production and fake clients to share one behavioral contract.

Test placement:

- Put pure conversion tests in `crates/orch-driver/tests/input_synth_wire.rs` or an equivalent driver test module.
- Keep fake contract tests in `crates/orch-fakes/tests/contracts.rs`, but refactor shared sample data into reusable helpers if that keeps fake and real adapter assertions aligned.
- Do not make `orch-fakes` depend on generated proto or tonic.

Golden coverage:

- `NodeId` conversion:
  - local `NodeId(u64)` -> decimal string in `NodeContext.node_id`
  - decimal string -> local `NodeId`
  - reject empty, negative, non-decimal, and overflow strings
- `SnapshotRef` conversion:
  - local 32-byte `SnapshotRef` -> lowercase hex string for input-synth `NodeContext.snapshot_ref`, unless the generated facade changes the field to bytes
  - golden fixture must make the chosen encoding explicit
  - reject wrong hex length or invalid characters when decoding is needed
- Enum mappings:
  - `DocumentKind::{ExperimentConfig, MacroPack, EventGrammar}`
  - `HealthStatus::{Serving, Degraded, NotServing}`
  - `ModelKind::{Pad, EventGrammar}`
  - `GeneratorKind::{WeightedRandom, Macro, Mutation, Policy}`
  - unknown or unspecified generated enum values must return `InvalidRequest` for request-side data and `DataLoss` for response-side data
- Oneof mappings:
  - `LoadMacroPackSource::{DocumentYaml, ArtifactRef}`
  - `BurstBody::{Pad, Event}`
  - `FieldValue::{Int, Enum, DurationNs, Bytes}`
  - optional `recent_inputs`, `parent_burst`, `macro_provenance`, `mutation_provenance`, and `policy_provenance`
- Fixed length bytes:
  - 32-byte `BurstId`
  - 32-byte `ConfigFingerprint`
  - reject any generated response with shorter or longer byte arrays as `DataLoss`
- `ProposeBurstsResponse` invariants:
  - response `seed` must equal request `seed`
  - response `bursts.len()` must equal request `k`
  - every returned `burst.provenance.config_fingerprint` must equal response `config_fingerprint`
  - response order is slot order and must be preserved

Shared contract test update:

- Keep the current fake tests that prove deterministic same-request behavior.
- Add a shared "sample request -> validated response" assertion that can run against `FakeSynth` and, separately, against a generated adapter backed by a test server or direct converter fixture.
- The fake is still allowed to be richer than the generated service internally, but externally it must honor the same owner API semantics.

Definition of done:

- Conversion tests fail on every listed encoding, enum, oneof, length, and exact-`k` regression.
- Fake contract tests still pass.
- The generated adapter and fake have one clearly documented common contract surface.

