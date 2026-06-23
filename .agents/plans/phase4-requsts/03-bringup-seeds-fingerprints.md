# 03 Bring-Up, Seeds, and Fingerprints

Goal: make synth experiment bring-up and per-expansion fingerprint checks deterministic, retryable, and explicit.

Seed rule:

- Use the orchestrator rule already implemented in `orch-core::rng`: synth request seeds come from `DeterministicRng::synth(experiment_seed, batch_seq)`.
- Add a small public helper such as `derive_synth_request_seed(experiment_seed, batch_seq) -> u64` that returns the first `next_u64()` from that stream.
- Add a golden fixture. Existing `rng.rs` vectors imply:
  - `experiment_seed = 0x0123_4567_89ab_cdef`
  - `batch_seq = 7`
  - expected synth request seed = `8_371_989_289_210_138_313`
- Add local authoritative docs or rustdoc on the helper stating this is the only orchestrator synth request seed rule.
- Add request-building tests proving `derive_synth_request_seed(experiment_seed, batch_seq)` is used and `node_id` is not mixed into the seed.
- File a cross-repo Beads follow-up if the determinism/input-synthesizer docs still mention `blake3(experiment_seed || node_id || expansion_counter)[..8]`; do not leave local behavior undocumented while waiting for that external cleanup.

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
- Implement the registry as an explicit `check_or_insert(profile, response_fingerprint)` style operation:
  - empty profile entry inserts only after all response invariants pass
  - matching entry accepts the response
  - mismatching entry returns a typed mismatch without modifying the expected fingerprint

Per-response guard:

- Validate all wire invariants from `02-dto-wire-contract-goldens.md` before any commit path sees the bursts.
- If a response fingerprint is unexpected:
  - commit no children
  - do not update the expected fingerprint registry
  - re-run bring-up by redelivering the experiment config and macro packs in their original deterministic order
  - retry the exact same `ProposeBursts` request with the same seed, batch sequence, node context, length hint, model, and override bytes within a bounded retry budget
  - if it still mismatches, surface `FailedPrecondition` and halt the expansion
- If any burst provenance fingerprint differs from the response fingerprint, treat it as `DataLoss` and commit no children.
- Keep transport retries separate from fingerprint mismatch retries so the retry budget and logs explain whether the service was unreachable or the effective config changed.

Definition of done:

- Seed derivation has one code path and one golden test.
- Bring-up verifies health against document ids returned by `LoadMacroPack` and required ids parsed from config.
- Fingerprint mismatch and per-burst provenance mismatch tests prove no child commit is attempted.
- Retry behavior is deterministic and bounded.
