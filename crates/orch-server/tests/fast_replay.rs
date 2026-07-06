//! M4 accept bar: fast mode, same seed twice — trees may differ, but every
//! committed node's trajectory re-derives bit-identically: re-running its
//! producing burst from its parent's snapshot on the FakeHypervisor
//! reproduces the stored state hash and snapshot ref (edge-wise induction
//! over GetPath).

mod support;

use orch_checkpoint::ExperimentState;
use orch_clients::{
    hypervisor::{
        DestroyVmRequest, HypervisorWorkerClient, RestoreSnapshotRequest, RunRequest, RunUntil,
        TakeSnapshotRequest,
    },
    input_synth::BurstBody,
    snapshot_store::{GetPathRequest, OrderBy, QueryNodesRequest, SnapshotStoreClient},
};
use orch_core::types::SchedMode;
use orch_driver::node_attrs::decode_node_attrs;
use orch_fakes::grid::GridWorld;
use orch_sched::driver::burst_events;
use orch_server::experiment::{ExperimentRunner, RunnerConfig};
use support::{config_hash, grid_config, sources, FakeWorld, EXPERIMENT_ID};

fn fast_config(seed: u64) -> RunnerConfig {
    let mut config = grid_config(seed);
    config.scheduling.mode = SchedMode::Fast;
    config.scheduling.max_inflight_batches = 2;
    config.validate().expect("valid");
    let hash = config_hash(&config);
    RunnerConfig {
        experiment_id: EXPERIMENT_ID.to_owned(),
        run_id: EXPERIMENT_ID.to_owned(),
        producer_id: "orchestratord-test".to_owned(),
        config,
        config_hash: hash,
    }
}

async fn run_fast(seed: u64) -> (FakeWorld, u64) {
    let world = FakeWorld::new(GridWorld::three_room());
    let (runner, _handle, _mode) = ExperimentRunner::start(
        fast_config(seed),
        sources(),
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        world.observatory(),
        None,
    )
    .await
    .expect("runner starts");
    let outcome = runner.run().await.expect("run completes");
    assert_eq!(outcome.state, ExperimentState::GoalReached, "{outcome:?}");
    (world, outcome.nodes)
}

#[tokio::test(start_paused = true)]
async fn every_committed_node_re_derives_from_its_parent() {
    let (world, nodes) = run_fast(0x5EED).await;
    let (_world_b, _nodes_b) = run_fast(0x5EED).await; // may differ; must both replay

    let store = world.store.service();
    let store = store.lock().await;
    let hypervisor = world.hypervisor.service();
    let mut hypervisor = hypervisor.lock().await;

    let mut all = store
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
    all.sort_by_key(|node| node.node_id);
    assert_eq!(all.len() as u64, nodes);

    let by_id: std::collections::BTreeMap<_, _> =
        all.iter().map(|node| (node.node_id, node)).collect();
    let mut replayed = 0usize;
    for node in &all {
        let Some(parent_id) = node.parent_node_id else {
            continue; // root
        };
        // GetPath sanity: the chain ends at this node.
        let path = store
            .get_path(GetPathRequest {
                experiment_id: EXPERIMENT_ID.to_owned(),
                node_id: node.node_id,
                include_input_logs: false,
            })
            .expect("path");
        assert_eq!(
            path.nodes.last().map(|meta| meta.node_id),
            Some(node.node_id)
        );

        let attrs = decode_node_attrs(&node.attrs).expect("attrs");
        let burst = attrs
            .synth
            .created_by_burst
            .as_ref()
            .expect("committed child has a producing burst");
        let BurstBody::Pad(pad) = &burst.burst.body else {
            panic!("pad bursts only");
        };
        let parent = by_id[&parent_id];

        // Re-run the burst from the parent's snapshot.
        let restored = hypervisor
            .restore_snapshot(RestoreSnapshotRequest {
                snapshot: parent.snapshot_ref,
                entropy_seed: None,
            })
            .expect("restore parent");
        let (events, budget) = burst_events(pad, restored.frame_counter).expect("events");
        hypervisor
            .inject_inputs(orch_clients::hypervisor::InjectInputsRequest {
                lease: restored.lease,
                events,
            })
            .expect("inject");
        hypervisor
            .run(RunRequest {
                lease: restored.lease,
                until: RunUntil::FrameBudget(budget),
                hard_icount_cap: None,
                capture: None,
            })
            .expect("run");
        let snapshot = hypervisor
            .take_snapshot(TakeSnapshotRequest {
                lease: restored.lease,
                seal_input_log: true,
                capture: None,
            })
            .expect("snapshot");
        hypervisor
            .destroy_vm(DestroyVmRequest {
                lease: restored.lease,
            })
            .expect("destroy");

        assert_eq!(
            snapshot.state_hash,
            attrs.synth.state_hash,
            "node {} state hash re-derives",
            node.node_id.get()
        );
        assert_eq!(
            snapshot.snapshot,
            node.snapshot_ref,
            "node {} snapshot ref re-derives (content-addressed)",
            node.node_id.get()
        );
        replayed += 1;
    }
    assert!(replayed > 0);
    println!("fast-replay: re-derived {replayed} trajectories");
}
