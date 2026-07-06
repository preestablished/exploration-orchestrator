//! M4 accept bar / CI determinism gate: deterministic mode, same seed
//! twice => identical canonical tree hash and identical event-sequence
//! hash over (ts_logical, event_type, payload); different seeds differ.

mod support;

use orch_checkpoint::ExperimentState;
use orch_clients::snapshot_store::{OrderBy, QueryNodesRequest, SnapshotStoreClient};
use orch_driver::node_attrs::decode_node_attrs;
use orch_fakes::grid::GridWorld;
use orch_server::experiment::ExperimentRunner;
use support::{event_sequence_hash, runner_config, sources, FakeWorld, SharedSink, EXPERIMENT_ID};

/// Canonical tree hash: (node_id, parent, state_hash, score, cell_key) in
/// id order.
async fn run_and_hash(seed: u64) -> ([u8; 32], [u8; 32]) {
    let world = FakeWorld::new(GridWorld::three_room());
    let sink = SharedSink::default();
    let (runner, _handle, _mode) = ExperimentRunner::start(
        runner_config(seed),
        sources(),
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        sink.clone(),
        None,
    )
    .await
    .expect("runner starts");
    let outcome = runner.run().await.expect("run completes");
    assert_eq!(outcome.state, ExperimentState::GoalReached, "{outcome:?}");

    let store = world.store.service();
    let store = store.lock().await;
    let mut nodes = store
        .query_nodes(QueryNodesRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            statuses: Vec::new(),
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
        .expect("query")
        .nodes;
    nodes.sort_by_key(|node| node.node_id);
    let mut hasher = blake3::Hasher::new();
    for node in &nodes {
        let attrs = decode_node_attrs(&node.attrs).expect("attrs");
        hasher.update(&node.node_id.get().to_le_bytes());
        hasher.update(
            &node
                .parent_node_id
                .map_or(u64::MAX, |parent| parent.get())
                .to_le_bytes(),
        );
        hasher.update(attrs.synth.state_hash.as_bytes());
        hasher.update(&node.progress_score.get().to_le_bytes());
        hasher.update(&attrs.synth.cell_key.get().to_le_bytes());
    }
    (*hasher.finalize().as_bytes(), event_sequence_hash(&sink))
}

#[tokio::test(start_paused = true)]
async fn same_seed_twice_is_bit_identical_and_different_seeds_differ() {
    let (tree_a, events_a) = run_and_hash(0x5EED).await;
    let (tree_b, events_b) = run_and_hash(0x5EED).await;
    let (tree_c, events_c) = run_and_hash(0x5EED + 1).await;

    assert_eq!(tree_a, tree_b, "same-seed tree hashes must match");
    assert_eq!(events_a, events_b, "same-seed event sequences must match");
    assert_ne!(tree_a, tree_c, "different seeds must differ");
    assert_ne!(events_a, events_c);
    println!("seed-gate: tree={} events={}", hex(&tree_a), hex(&events_a));
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
