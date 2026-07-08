#![forbid(unsafe_code)]

//! Crash-consistent persistence for the fake world (plan Tier-2, D-T1/D-T2).
//!
//! `orch-fakes` stays free of filesystem and wall-clock dependencies (plan
//! D5); this crate wraps the four fakes in journaling [`world::Persistent`]
//! adapters backed by a write-ahead op [`journal::Journal`], so a real
//! SIGKILL at any instruction leaves a state-dir from which
//! [`world::PersistentServices::reload`] rebuilds the exact committed world
//! by re-invoking the logged ops against fresh fakes.
//!
//! **Soundness invariant (D-T4):** the wrappers require a *disabled*
//! `FaultPlan` in every wrapped fake. `FaultInjector::decide` advances a
//! per-`(target, operation)` attempt counter on every call including
//! read-only ops, and read-only ops are not replayed — any nonzero plan
//! would diverge under replay. With the plan disabled all decisions are
//! counter-independent.
//!
//! Test-only env hooks (documented, not for production):
//! - `ORCH_SIM_TORN_AT=<wal-append|ckpt-put>:<nth>` — on the nth matching
//!   `put_metadata` append, write a torn frame prefix, print
//!   `TIER2_CHAOS_HANG kind=<kind>`, and park so a harness can SIGKILL
//!   mid-write deterministically (plan D-T3).
//! - `ORCH_SIM_BREAK=perturb-node|drop-scorer-replay` — honored by
//!   `orchestratord`, mapped to [`world::BreakMode`] for the negative
//!   control (plan W2.5).

pub mod compare;
pub mod journal;
pub mod records;
pub mod world;

pub use journal::{Journal, LoadStats, RecordKind};
pub use records::JournalRecord;
pub use world::{BreakMode, Persistent, PersistentServices, PersistentWorld};
