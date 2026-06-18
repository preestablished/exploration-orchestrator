# exploration-orchestrator

Pure-Rust core (`orch-core` has no tokio/tonic).

## M1 Core

`orch-core` is pure synchronous search logic. It owns deterministic value types,
feature-layout compilation, tree/frontier/mirror state, commit decisions,
plateau tracking, deterministic RNG, and selection policies. It does not own
platform service clients, transports, schedulers, or runners.

Core validation commands:

```bash
cargo test -p orch-core
cargo test -p orch-core --test purity_guard
```

## M1-M2 Crate Roles

- `orch-core` owns deterministic in-memory search primitives: config/value types,
  compile-time feature layout checks, tree/frontier/mirror state, commit rules,
  plateau tracking, deterministic RNG, and parent-selection policies.
- `orch-clients` owns transport-free DTOs and client traits for orchestrator
  service boundaries.
- `orch-fakes` owns deterministic service-contract fakes for M2 validation. These
  fakes mirror orchestrator client DTO boundaries for the fake grid world, scorer,
  snapshot store, input synth, hypervisor worker, fault injection, and transcript
  hashing. They are not real platform clients and intentionally avoid transport,
  async runtime, filesystem, network, and wall-clock dependencies.
- `orch-proto` re-exports the owner orchestrator proto surface used by this repo.
- `orch-driver` remains a placeholder during M1-M2. Scheduler/pipeline work is M3
  scope, and checkpoint runner/resume orchestration is M4 scope.

## Validation Commands

```bash
cargo test -p orch-core
cargo test -p orch-fakes
cargo test --workspace
```
