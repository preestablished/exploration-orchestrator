mod support;

use std::sync::{Arc, Mutex};

use orch_checkpoint::ExperimentState;
use orch_clients::snapshot_store::{
    GetMetadataRequest, MetadataExpectation, MetadataKey, OrderBy, PutMetadataRequest,
    QueryNodesRequest, SnapshotStoreClient,
};
use orch_core::types::NodeStatus;
use orch_fakes::{grid::GridWorld, snapshot_store::InMemorySnapshotStore};
use orch_sched::ports::SyncAdapter;
use orch_server::experiment::{
    CrashPoint, CrashPolicy, ExperimentRunner, REASON_CAS_OWNERSHIP_LOST,
};
use support::{runner_config, sources, FakeWorld, EXPERIMENT_ID};

const WINNER_VALUE: &[u8] = b"m5-competing-writer-checkpoint";

#[derive(Clone, Debug, Default)]
struct TakeoverRecord {
    fired: bool,
    nodes_at_takeover: usize,
    generation_after_takeover: Option<u64>,
}

struct CompetingCheckpointWriter {
    point: CrashPoint,
    remaining: u32,
    store: SyncAdapter<InMemorySnapshotStore>,
    record: Arc<Mutex<TakeoverRecord>>,
}

impl CrashPolicy for CompetingCheckpointWriter {
    fn should_crash(&mut self, point: CrashPoint) -> bool {
        if point != self.point || self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        if self.remaining > 0 {
            return false;
        }

        let response = self
            .store
            .with_service_sync(|store| {
                let nodes = count_nodes(store);
                let response = store.put_metadata(PutMetadataRequest {
                    key: MetadataKey::checkpoint(EXPERIMENT_ID),
                    value: WINNER_VALUE.to_vec(),
                    expected_generation: MetadataExpectation::unconditional(),
                })?;
                Ok::<_, orch_clients::ClientError>((nodes, response.generation.get()))
            })
            .expect("store uncontended at ownership hook")
            .expect("competing writer takes checkpoint key");
        let mut record = self.record.lock().expect("record lock");
        record.fired = true;
        record.nodes_at_takeover = response.0;
        record.generation_after_takeover = Some(response.1);
        false
    }
}

fn count_nodes(store: &InMemorySnapshotStore) -> usize {
    store
        .query_nodes(QueryNodesRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            statuses: Vec::<NodeStatus>::new(),
            min_progress: None,
            max_progress: None,
            min_novelty: None,
            min_depth: None,
            max_depth: None,
            created_after: None,
            updated_after: None,
            order_by: OrderBy::CreatedAt,
            limit: None,
        })
        .expect("query nodes")
        .nodes
        .len()
}

async fn run_with_takeover(point: CrashPoint, remaining: u32) -> (FakeWorld, TakeoverRecord) {
    let world = FakeWorld::new(GridWorld::three_room());
    let record = Arc::new(Mutex::new(TakeoverRecord::default()));
    let policy: Option<Box<dyn CrashPolicy>> = Some(Box::new(CompetingCheckpointWriter {
        point,
        remaining,
        store: world.store.clone(),
        record: Arc::clone(&record),
    }));
    let (runner, _handle, _mode) = ExperimentRunner::start(
        runner_config(0xCA5),
        sources(),
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        world.observatory(),
        policy,
    )
    .await
    .expect("runner starts");

    let outcome = runner.run().await.expect("runner returns terminal outcome");
    assert_eq!(outcome.state, ExperimentState::Failed);
    let reason = outcome.failure_reason.expect("failure reason");
    assert!(
        reason.starts_with(REASON_CAS_OWNERSHIP_LOST),
        "unexpected failure reason: {reason}"
    );

    let record = record.lock().expect("record lock").clone();
    assert!(record.fired, "competing writer did not run");
    (world, record)
}

fn assert_competitor_still_owns_checkpoint(world: &FakeWorld, record: &TakeoverRecord) {
    let store = world.store.service();
    let store = store.try_lock().expect("store idle after run");
    let response = store
        .get_metadata(GetMetadataRequest {
            key: MetadataKey::checkpoint(EXPERIMENT_ID),
        })
        .expect("checkpoint metadata exists");
    assert_eq!(response.value, WINNER_VALUE);
    assert_eq!(
        Some(response.generation.get()),
        record.generation_after_takeover
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checkpoint_cas_window_loser_fails_without_overwriting_winner() {
    let (world, record) = run_with_takeover(CrashPoint::BeforeCasPut, 2).await;
    assert_competitor_still_owns_checkpoint(&world, &record);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_commit_window_loser_fails_before_tree_writes() {
    let (world, record) = run_with_takeover(CrashPoint::BeforeCommitOwnerCheck, 1).await;
    assert_competitor_still_owns_checkpoint(&world, &record);

    let store = world.store.service();
    let store = store.try_lock().expect("store idle after run");
    assert_eq!(
        count_nodes(&store),
        record.nodes_at_takeover,
        "stale loser wrote nodes after ownership loss"
    );
}
