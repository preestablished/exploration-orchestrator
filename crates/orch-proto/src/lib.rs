#![forbid(unsafe_code)]

//! Wire types for the exploration orchestrator.
//!
//! Both proto families are canonical in the control-plane repo and consumed
//! via the `determinism-proto` crate (see `protos.lock`). The
//! `determinism.orchestrator.v1` surface was authored here (plan D4) and
//! upstreamed per bead `exploration-orchestrator-777`; this crate is now a
//! pure re-export shim and the natural seam for any future repo-local wire
//! helpers.

pub mod inputsynth {
    pub use determinism_proto::inputsynth::v1;
}

pub mod orchestrator_v1 {
    pub use determinism_proto::orchestrator::v1::*;
}
