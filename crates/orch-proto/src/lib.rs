#![forbid(unsafe_code)]

//! Wire types for the exploration orchestrator.
//!
//! `orchestrator_v1` is generated from the locally authored
//! `determinism.orchestrator.v1` proto (plan D4; see `protos.lock`).
//! Upstream determinism-proto's placeholder `orchestrator` module is NOT
//! re-exported — it has no service and a divergent StartExperimentRequest
//! shape; reconciliation is a disclosed follow-up at handback.

pub mod inputsynth {
    pub use determinism_proto::inputsynth::v1;
}

pub mod orchestrator_v1 {
    tonic::include_proto!("determinism.orchestrator.v1");
}
