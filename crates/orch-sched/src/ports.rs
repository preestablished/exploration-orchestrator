//! Async ports over the transport-free sync client traits, plus the
//! [`SyncAdapter`] that lifts any sync implementation (the `orch-fakes`
//! services in tests, and nothing else until M6) onto a tokio runtime.
//!
//! Concurrency model (plan decision D2):
//!
//! - The sync traits in `orch-clients` remain the contract source of truth.
//!   These ports mirror exactly the methods the scheduler and runner drive.
//! - A sync implementation is one logical server held behind a single async
//!   mutex. Virtual latency is charged by sleeping **before** taking the lock
//!   and making the (instant) sync call, so the service state change lands at
//!   the *response* instant and K in-flight calls genuinely overlap in
//!   virtual time.
//! - Latency is not observable through the sync traits; it is supplied by a
//!   caller-provided [`LatencyProbe`]. Fake-specific probes live in test
//!   trees; production adapters at M6 replace [`SyncAdapter`] wholesale.
//! - A probe-predicted timeout charges the caller's configured call timeout
//!   in virtual time before the call is made, so retry and backpressure
//!   timing under injected timeouts is realistic.
//!
//! 1 latency tick = 1 virtual millisecond.

use std::{future::Future, sync::Arc, time::Duration};

use orch_clients::{
    hypervisor::{
        CreateVmRequest, CreateVmResponse, DestroyVmRequest, DestroyVmResponse, ForkRequest,
        ForkResponse, GetWorkerInfoRequest, GetWorkerInfoResponse, HypervisorWorkerClient,
        InjectInputsRequest, InjectInputsResponse, ListSlotsRequest, ListSlotsResponse,
        RestoreSnapshotRequest, RestoreSnapshotResponse, RunRequest, RunResponse,
        TakeSnapshotRequest, TakeSnapshotResponse, WatchSlotsRequest, WatchSlotsResponse,
    },
    input_synth::{
        HealthRequest, HealthResponse, InputSynthClient, LoadMacroPackRequest,
        LoadMacroPackResponse, MineMacrosRequest, MineMacrosResponse, ProposeBurstsRequest,
        ProposeBurstsResponse,
    },
    scorer::{
        CheckpointArchiveRequest, CheckpointArchiveResponse, LoadFeatureMapRequest,
        LoadFeatureMapResponse, LoadScoringProgramRequest, LoadScoringProgramResponse,
        ReplayCommitsRequest, ReplayCommitsResponse, RestoreArchiveRequest, RestoreArchiveResponse,
        ScoreBatchRequest, ScoreBatchResponse, StateScorerClient,
    },
    snapshot_store::{
        CreateNodeRequest, CreateNodeResponse, DeleteMetadataRequest, DeleteMetadataResponse,
        GetChildrenRequest, GetChildrenResponse, GetMetadataRequest, GetMetadataResponse,
        GetNodeRequest, GetNodeResponse, GetPathRequest, GetPathResponse, PruneSubtreeRequest,
        PruneSubtreeResponse, PutMetadataRequest, PutMetadataResponse, QueryNodesRequest,
        QueryNodesResponse, SnapshotStoreClient, UpdateNodesRequest, UpdateNodesResponse,
    },
    ClientResult,
};
use serde::Serialize;
use tokio::sync::Mutex;

/// 1 fake-service latency tick equals 1 virtual millisecond (plan D2).
#[must_use]
pub fn ticks_to_virtual(ticks: u32) -> Duration {
    Duration::from_millis(u64::from(ticks))
}

/// Pre-computed shape of the next call on a `(target, operation)` stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PendingCall {
    /// Virtual latency to charge before the call, in ticks.
    pub latency_ticks: u32,
    /// The upcoming call will terminate as a deterministic timeout fault; the
    /// adapter charges the configured call timeout instead of `latency_ticks`.
    pub timeout: bool,
}

/// Seam supplying the virtual latency of the next sync call.
///
/// Fake fault decisions are deterministic per `(seed, target, operation,
/// request identity, attempt)`; probe implementations (test tree only)
/// typically pre-compute them via `FaultInjector::peek`, or replay the same
/// latency distribution from a cloned plan. `orch-sched` itself never depends
/// on `orch-fakes` outside dev-deps.
pub trait LatencyProbe: Send {
    fn pending_call(&mut self, operation: &'static str, request_identity: &[u8]) -> PendingCall;
}

/// Probe charging zero latency everywhere (the default).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoLatency;

impl LatencyProbe for NoLatency {
    fn pending_call(&mut self, _operation: &'static str, _request_identity: &[u8]) -> PendingCall {
        PendingCall::default()
    }
}

/// Lifts a sync `orch-clients` implementation onto the async ports.
///
/// Cloning shares the underlying service, probe, and configuration, so K
/// concurrent callers contend on one logical server whose slot or capacity
/// limits stay enforced by the implementation itself.
pub struct SyncAdapter<T> {
    inner: Arc<Mutex<T>>,
    probe: Arc<Mutex<Box<dyn LatencyProbe>>>,
    call_timeout: Duration,
}

impl<T> Clone for SyncAdapter<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            probe: Arc::clone(&self.probe),
            call_timeout: self.call_timeout,
        }
    }
}

impl<T: Send + 'static> SyncAdapter<T> {
    pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(10);

    #[must_use]
    pub fn new(inner: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            probe: Arc::new(Mutex::new(Box::new(NoLatency))),
            call_timeout: Self::DEFAULT_CALL_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_probe(mut self, probe: impl LatencyProbe + 'static) -> Self {
        self.probe = Arc::new(Mutex::new(Box::new(probe)));
        self
    }

    #[must_use]
    pub fn with_call_timeout(mut self, call_timeout: Duration) -> Self {
        self.call_timeout = call_timeout;
        self
    }

    /// Shared handle to the wrapped service, for test inspection.
    #[must_use]
    pub fn service(&self) -> Arc<Mutex<T>> {
        Arc::clone(&self.inner)
    }

    /// Charges virtual latency, then takes the service lock and makes the
    /// (instant) sync call. Sleep placement per D2: before the lock, so the
    /// state change lands at the response instant and calls overlap in
    /// virtual time.
    async fn call<R, F>(&self, operation: &'static str, identity: &[u8], f: F) -> ClientResult<R>
    where
        F: FnOnce(&mut T) -> ClientResult<R> + Send,
        R: Send,
    {
        let pending = self.probe.lock().await.pending_call(operation, identity);
        let charge = if pending.timeout {
            self.call_timeout
        } else {
            ticks_to_virtual(pending.latency_ticks)
        };
        if charge > Duration::ZERO {
            tokio::time::sleep(charge).await;
        }
        let mut inner = self.inner.lock().await;
        f(&mut inner)
    }
}

fn identity_bytes<R: Serialize>(request: &R) -> Vec<u8> {
    postcard::to_allocvec(request).unwrap_or_default()
}

macro_rules! async_port {
    (
        $(#[$attr:meta])*
        trait $port:ident lifts $sync:ident {
            $($method:ident($req:ty) -> $resp:ty;)+
        }
    ) => {
        $(#[$attr])*
        pub trait $port: Send + Sync {
            $(
                fn $method(
                    &self,
                    request: $req,
                ) -> impl Future<Output = ClientResult<$resp>> + Send;
            )+
        }

        impl<T> $port for SyncAdapter<T>
        where
            T: $sync + Send + 'static,
        {
            $(
                fn $method(
                    &self,
                    request: $req,
                ) -> impl Future<Output = ClientResult<$resp>> + Send {
                    async move {
                        let identity = identity_bytes(&request);
                        self.call(stringify!($method), &identity, move |inner| {
                            inner.$method(request)
                        })
                        .await
                    }
                }
            )+
        }
    };
}

async_port! {
    /// Async port over [`HypervisorWorkerClient`].
    trait AsyncHypervisor lifts HypervisorWorkerClient {
        create_vm(CreateVmRequest) -> CreateVmResponse;
        restore_snapshot(RestoreSnapshotRequest) -> RestoreSnapshotResponse;
        fork(ForkRequest) -> ForkResponse;
        inject_inputs(InjectInputsRequest) -> InjectInputsResponse;
        run(RunRequest) -> RunResponse;
        take_snapshot(TakeSnapshotRequest) -> TakeSnapshotResponse;
        destroy_vm(DestroyVmRequest) -> DestroyVmResponse;
        list_slots(ListSlotsRequest) -> ListSlotsResponse;
        watch_slots(WatchSlotsRequest) -> WatchSlotsResponse;
        worker_info(GetWorkerInfoRequest) -> GetWorkerInfoResponse;
    }
}

async_port! {
    /// Async port over [`StateScorerClient`].
    trait AsyncScorer lifts StateScorerClient {
        load_feature_map(LoadFeatureMapRequest) -> LoadFeatureMapResponse;
        load_scoring_program(LoadScoringProgramRequest) -> LoadScoringProgramResponse;
        score_batch(ScoreBatchRequest) -> ScoreBatchResponse;
        checkpoint_archive(CheckpointArchiveRequest) -> CheckpointArchiveResponse;
        restore_archive(RestoreArchiveRequest) -> RestoreArchiveResponse;
        replay_commits(ReplayCommitsRequest) -> ReplayCommitsResponse;
    }
}

async_port! {
    /// Async port over [`InputSynthClient`].
    trait AsyncSynth lifts InputSynthClient {
        load_macro_pack(LoadMacroPackRequest) -> LoadMacroPackResponse;
        health(HealthRequest) -> HealthResponse;
        propose_bursts(ProposeBurstsRequest) -> ProposeBurstsResponse;
        mine_macros(MineMacrosRequest) -> MineMacrosResponse;
    }
}

async_port! {
    /// Async port over [`SnapshotStoreClient`].
    trait AsyncStore lifts SnapshotStoreClient {
        create_node(CreateNodeRequest) -> CreateNodeResponse;
        update_nodes(UpdateNodesRequest) -> UpdateNodesResponse;
        get_node(GetNodeRequest) -> GetNodeResponse;
        get_children(GetChildrenRequest) -> GetChildrenResponse;
        get_path(GetPathRequest) -> GetPathResponse;
        query_nodes(QueryNodesRequest) -> QueryNodesResponse;
        put_metadata(PutMetadataRequest) -> PutMetadataResponse;
        get_metadata(GetMetadataRequest) -> GetMetadataResponse;
        delete_metadata(DeleteMetadataRequest) -> DeleteMetadataResponse;
        prune_subtree(PruneSubtreeRequest) -> PruneSubtreeResponse;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_clients::{snapshot_store::MetadataKey, ClientErrorKind};
    use orch_fakes::{
        fault::{FaultPlan, FaultRate, LatencyFault},
        snapshot_store::InMemorySnapshotStore,
    };
    use tokio::time::Instant;

    #[derive(Clone, Copy, Debug)]
    struct FixedProbe {
        latency_ticks: u32,
        timeout: bool,
    }

    impl LatencyProbe for FixedProbe {
        fn pending_call(&mut self, _op: &'static str, _identity: &[u8]) -> PendingCall {
            PendingCall {
                latency_ticks: self.latency_ticks,
                timeout: self.timeout,
            }
        }
    }

    fn checkpoint_get() -> GetMetadataRequest {
        GetMetadataRequest {
            key: MetadataKey::checkpoint("exp-a"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn adapter_charges_probe_latency_in_virtual_time() {
        let adapter = SyncAdapter::new(InMemorySnapshotStore::new()).with_probe(FixedProbe {
            latency_ticks: 250,
            timeout: false,
        });

        let start = Instant::now();
        let error = adapter
            .get_metadata(checkpoint_get())
            .await
            .expect_err("metadata absent");

        assert_eq!(error.kind(), ClientErrorKind::NotFound);
        assert_eq!(start.elapsed(), Duration::from_millis(250));
    }

    #[tokio::test(start_paused = true)]
    async fn adapter_sleeps_before_the_call_so_calls_overlap_in_virtual_time() {
        let adapter = SyncAdapter::new(InMemorySnapshotStore::new()).with_probe(FixedProbe {
            latency_ticks: 100,
            timeout: false,
        });

        let start = Instant::now();
        let (left, right) = tokio::join!(
            adapter.get_metadata(checkpoint_get()),
            adapter.get_metadata(checkpoint_get()),
        );

        left.expect_err("metadata absent");
        right.expect_err("metadata absent");
        // Sleep-before-lock: two overlapping calls take one latency, not two.
        assert_eq!(start.elapsed(), Duration::from_millis(100));
    }

    #[tokio::test(start_paused = true)]
    async fn adapter_charges_configured_timeout_for_predicted_timeout_faults() {
        let store = InMemorySnapshotStore::with_fault_plan(
            FaultPlan::disabled(7).with_timeout(FaultRate::always()),
        );
        let adapter = SyncAdapter::new(store)
            .with_probe(FixedProbe {
                latency_ticks: 3,
                timeout: true,
            })
            .with_call_timeout(Duration::from_secs(2));

        let start = Instant::now();
        let error = adapter
            .get_metadata(checkpoint_get())
            .await
            .expect_err("deterministic timeout fault");

        assert_eq!(error.kind(), ClientErrorKind::Unavailable);
        assert_eq!(start.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn adapter_passes_service_errors_through_unchanged() {
        let store = InMemorySnapshotStore::with_fault_plan(
            FaultPlan::disabled(11).with_error(FaultRate::always(), ClientErrorKind::DataLoss),
        );
        let adapter = SyncAdapter::new(store);

        let start = Instant::now();
        let error = adapter
            .get_metadata(checkpoint_get())
            .await
            .expect_err("injected error");

        assert_eq!(error.kind(), ClientErrorKind::DataLoss);
        assert_eq!(error.message(), "deterministic fake fault");
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[test]
    fn tick_mapping_is_one_virtual_millisecond_per_tick() {
        assert_eq!(ticks_to_virtual(0), Duration::ZERO);
        assert_eq!(ticks_to_virtual(1), Duration::from_millis(1));
        assert_eq!(ticks_to_virtual(1_500), Duration::from_millis(1_500));
    }

    #[test]
    fn probe_seam_can_replay_fake_latency_distribution() {
        use orch_fakes::fault::{FaultInjector, FaultRequest, FaultTarget};

        // A test probe holding a cloned injector peeks the same deterministic
        // latency stream the fake draws from.
        let plan = FaultPlan::disabled(42).with_latency(LatencyFault::new(10, 20));
        let probe_injector = FaultInjector::new(plan.clone());
        let fake_injector = FaultInjector::new(plan);
        let request = FaultRequest::new(FaultTarget::SnapshotStore, "get_metadata", b"exp-a");

        for _ in 0..8 {
            let peeked = probe_injector.peek(request, 0);
            let _ = probe_injector.decide(request, 0);
            assert_eq!(fake_injector.decide(request, 0), peeked);
        }
    }
}
