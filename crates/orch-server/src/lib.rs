#![forbid(unsafe_code)]

//! Experiment runner and served control surface (plan M4).

pub mod bringup;
pub mod config;
pub mod events;
pub mod experiment;
pub mod metrics;
pub mod service;

use orch_clients::{
    input_synth::ProposeBurstsRequest, snapshot_store::SnapshotStoreClient, ClientResult,
};
use orch_driver::input_synth::{build_propose_bursts_request, ProposeBurstsBuildSpec};
use orch_sched::ports::SyncAdapter;

/// Startup/loop-local sync access to the snapshot store for the S stage's
/// request builder (`build_propose_bursts_request` is sync trait-generic).
/// The runner is the store's only writer, so the store lock is uncontended
/// at build time; real transports replace this seam at M6.
pub trait SyncStoreAccess {
    fn build_propose_bursts(
        &self,
        spec: ProposeBurstsBuildSpec,
    ) -> ClientResult<ProposeBurstsRequest>;
}

impl<T> SyncStoreAccess for SyncAdapter<T>
where
    T: SnapshotStoreClient + Send + 'static,
{
    fn build_propose_bursts(
        &self,
        spec: ProposeBurstsBuildSpec,
    ) -> ClientResult<ProposeBurstsRequest> {
        self.with_service_sync(|store| build_propose_bursts_request(&*store, spec))?
    }
}
