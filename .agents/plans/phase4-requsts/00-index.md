# Phase 4 Request Plan: Input Synth v1 Client and Context Persistence

Source request:
`/home/infra-admin/.agents/projects/exploration-orchestrator/requests/input-synth-v1-client-context/README.md`

Reference docs:

- `/home/infra-admin/.agents/projects/determinism/docs/input-synthesizer/API.md`
- `/home/infra-admin/.agents/projects/determinism/docs/input-synthesizer/INTEGRATION.md`
- `/home/infra-admin/.agents/projects/determinism/docs/exploration-orchestrator/API.md`
- `/home/infra-admin/.agents/projects/determinism/docs/exploration-orchestrator/ARCHITECTURE.md`

Current repo shape:

- `orch-clients` owns transport-free DTOs and sync client traits.
- `orch-fakes` owns deterministic fakes and contract tests.
- `orch-driver` is the right repo-local owner for runtime/client adapter code.
- `orch-proto` currently re-exports only orchestrator proto types.
- The adjacent control-plane `determinism-proto` input-synth surface is still a stub; the real generated `determinism.inputsynth.v1` facade is a prerequisite for the real adapter.

Preflight rule for `/goal implement .agents/plans/phase4-requsts/`:

- First check whether `determinism-proto` exposes the real generated `inputsynth` v1 tonic/prost facade. The current stub module is not usable.
- If the facade is missing, create a Beads blocker naming the missing generated symbols and skip only the generated adapter and generated wire-golden parts of files `01` and `02`.
- Continue all independent work that can compile without the real facade: synth seed helper/tests, bring-up/fingerprint state helpers against the `InputSynthClient` trait, attrs encode/decode, `NodeContext` reconstruction, fake-backed smoke tests, and docs.
- Do not report the whole request as complete while the real generated adapter is blocked. Keep a parent implementation issue open or blocked until the adapter and generated wire tests are finished.

Implementation sequence:

1. Add generated input-synth proto access and the real transport adapter in `orch-driver`.
2. Add DTO-to-wire conversion goldens and shared fake/adapter contract tests.
3. Implement experiment synth bring-up, seed derivation, and fingerprint guard rails.
4. Define the versioned node attrs envelope and build future `NodeContext` from stored tree metadata.
5. Add the concrete M3 smoke that proves parent and sibling bursts reach the synth call.

Execution notes for the implementing agent:

- Use Beads for implementation tracking. Create new implementation issues for these slices rather than reusing planning/review issues.
- Suggested dependency shape: parent Phase 4 request issue; generated-facade blocker; adapter issue blocked by facade; generated wire-goldens blocked by facade; seed/fingerprint issue; attrs/context issue; smoke issue blocked by seed/fingerprint and attrs/context, and partially blocked by adapter only for real-network coverage.
- Keep `orch-clients` and `orch-fakes` transport-free.
- Do not move generator logic into orchestrator.
- Do not add request fields that are not in the generated owner API. Ladder changes use `config_overrides_yaml`.
- Preserve deterministic behavior: no wall-clock, thread RNG, hash map iteration, or unordered sibling/context assembly in deterministic paths.
