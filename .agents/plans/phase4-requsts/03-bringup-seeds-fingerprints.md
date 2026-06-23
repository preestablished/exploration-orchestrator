# 03 Bring-Up, Seeds, and Fingerprints

Goal: make synth experiment bring-up and per-expansion fingerprint checks deterministic, retryable, and explicit.

Seed rule:

- Use the orchestrator rule already implemented in `orch-core::rng`: synth request seeds come from `DeterministicRng::synth(experiment_seed, batch_seq)`.
- Add a small public helper such as `derive_synth_request_seed(experiment_seed, batch_seq) -> u64` that returns the first `next_u64()` from that stream.
- Add a golden fixture. Existing `rng.rs` vectors imply:
  - `experiment_seed = 0x0123_4567_89ab_cdef`
  - `batch_seq = 7`
  - expected synth request seed = `8_371_989_289_210_138_313`
- Update local docs or add a cross-repo doc follow-up so the conflicting input-synthesizer integration note using `blake3(experiment_seed || node_id || expansion_counter)[..8]` is reconciled with the orchestrator `derive(seed, "synth", batch_seq)` rule.

Bring-up module:

- Add a driver-level `SynthBringup` helper that accepts:
  - experiment id
  - experiment config source for `LoadMacroPack(kind = EXPERIMENT_CONFIG)`
  - configured macro pack sources for `LoadMacroPack(kind = MACRO_PACK)`
  - optional required pack ids parsed from the synth config's `macro.packs`
- Loading order:
  - load experiment config first
  - load every configured macro pack next
  - collect every `LoadMacroPackResponse.document_id`
  - call `Health()`
  - assert `Health.loaded_packs` covers the union of loaded macro-pack document ids and any pack ids named by parsed `macro.packs`
- The orchestrator may parse only enough synthesizer config YAML to extract required pack ids. Do not parse macro pack semantics.

Fingerprint registry:

- Track expected fingerprints by effective synth profile:
  - experiment id
  - model
  - exact `config_overrides_yaml` bytes or a canonical hash of those bytes
- The first successful `ProposeBursts` for a profile establishes that profile's expected `ConfigFingerprint`.
- A later response for the same profile must match the expected fingerprint.
- Overrides intentionally get their own expected fingerprint. Do not compare override profiles to the base profile.

Per-response guard:

- Validate all wire invariants from `02-dto-wire-contract-goldens.md` before any commit path sees the bursts.
- If a response fingerprint is unexpected:
  - commit no children
  - re-run bring-up by redelivering the experiment config and macro packs
  - retry the same `ProposeBursts` request within a bounded retry budget
  - if it still mismatches, surface `FailedPrecondition` and halt the expansion
- If any burst provenance fingerprint differs from the response fingerprint, treat it as `DataLoss` and commit no children.

Definition of done:

- Seed derivation has one code path and one golden test.
- Bring-up verifies health against document ids returned by `LoadMacroPack` and required ids parsed from config.
- Fingerprint mismatch and per-burst provenance mismatch tests prove no child commit is attempted.
- Retry behavior is deterministic and bounded.

