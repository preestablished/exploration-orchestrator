mod support;

use std::{collections::BTreeSet, time::Duration};

use orch_checkpoint::ExperimentState;
use orch_clients::snapshot_store::{OrderBy, QueryNodesRequest, SnapshotStoreClient};
use orch_core::types::{NodeStatus, OnGoal, SchedMode};
use orch_fakes::{
    fault::{FaultPlan, LatencyFault},
    grid::GridWorld,
};
use orch_server::experiment::{Control, ExperimentRunner};
use support::{grid_config, sources, FakeWorld, EXPERIMENT_ID};

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn query_all_nodes(
    store: &impl SnapshotStoreClient,
) -> Vec<orch_clients::snapshot_store::NodeMeta> {
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m5_fault_injected_fake_soak_smoke() {
    let duration = Duration::from_secs(env_u64("M5_SOAK_DURATION_SECONDS", 2));
    let seed = env_u64("M5_SOAK_SEED", 0x5E05);
    let fault_seed = env_u64("M5_SOAK_FAULT_SEED", 0xFA171);
    let k = env_u32("M5_SOAK_K", 64);

    let fault_plan = FaultPlan::disabled(fault_seed).with_latency(LatencyFault::new(1, 3));
    let world = FakeWorld::with_plans(GridWorld::three_room(), fault_plan);
    let mut config = grid_config(seed);
    config.burst.k_per_expansion = k;
    config.budgets.max_expansions = 1_000_000;
    config.budgets.max_wall_clock_s = duration.as_secs().saturating_add(60);
    config.budgets.max_nodes = 0;
    config.checkpoint.every_commits = 4;
    config.checkpoint.every_seconds = 1;
    config.on_goal = OnGoal::Continue;
    config.scheduling.mode = SchedMode::Deterministic;
    config.scheduling.max_inflight_batches = 1;
    config.validate().expect("soak config validates");
    let hash = support::config_hash(&config);
    let runner_config = orch_server::experiment::RunnerConfig {
        experiment_id: EXPERIMENT_ID.to_owned(),
        run_id: EXPERIMENT_ID.to_owned(),
        producer_id: "m5-soak".to_owned(),
        config,
        config_hash: hash,
    };

    let (runner, handle, _mode) = ExperimentRunner::start(
        runner_config,
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
    let run = tokio::spawn(runner.run());
    tokio::time::sleep(duration).await;
    handle.send(Control::Stop).expect("stop runner");
    let outcome = run
        .await
        .expect("runner task joins")
        .expect("runner returns outcome");

    assert_eq!(outcome.state, ExperimentState::Stopped);
    assert!(outcome.expansions > 0, "soak made no progress");
    assert!(outcome.nodes > 1, "soak committed no children");

    let store = world.store.service();
    let store = store.lock().await;
    let nodes = query_all_nodes(&*store);
    let committed_refs: BTreeSet<_> = nodes.iter().map(|node| node.snapshot_ref).collect();
    drop(store);

    let hypervisor = world.hypervisor.service();
    let mut hypervisor = hypervisor.lock().await;
    let pre_gc_live = hypervisor.live_snapshot_refs();
    let pre_gc_orphans: BTreeSet<_> = pre_gc_live.difference(&committed_refs).copied().collect();
    let removed = hypervisor.retain_live_snapshots(&committed_refs);
    let post_gc_live = hypervisor.live_snapshot_refs();
    let fault = hypervisor.last_fault();

    assert_eq!(removed, pre_gc_orphans);
    assert_eq!(post_gc_live, committed_refs);
    assert!(
        fault.is_some_and(|decision| decision.latency_ticks > 0),
        "deterministic fake latency fault did not fire"
    );

    println!(
        "M5_SOAK_SUMMARY duration_seconds={} k={} seed={} fault_seed={} expansions={} nodes={} committed_refs={} pre_gc_orphans={} post_gc_live={} failed_reason=none",
        duration.as_secs(),
        k,
        seed,
        fault_seed,
        outcome.expansions,
        outcome.nodes,
        committed_refs.len(),
        pre_gc_orphans.len(),
        post_gc_live.len()
    );
}
