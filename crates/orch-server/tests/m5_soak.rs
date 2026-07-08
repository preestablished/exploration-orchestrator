mod support;

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use orch_checkpoint::{decode_checkpoint, ExperimentState};
use orch_clients::{
    snapshot_store::{
        GetMetadataRequest, MetadataKey, OrderBy, QueryNodesRequest, SnapshotStoreClient,
    },
    ClientErrorKind,
};
use orch_core::types::{NodeStatus, OnGoal, SchedMode};
use orch_fakes::{
    fault::{FaultPlan, FaultStats, LatencyFault},
    grid::{GridPos, GridWorld, Room, GRID_HEIGHT, GRID_WIDTH},
    hypervisor::FakeHypervisor,
    observatory::FakeObservatory,
    snapshot_store::InMemorySnapshotStore,
};
use orch_sched::ports::SyncAdapter;
use orch_server::experiment::{Control, CrashPoint, CrashPolicy, ExperimentRunner};
use support::{grid_config, sources, FakeWorld, SharedSink, EXPERIMENT_ID};

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

fn service_fault_plan(seed: u64, operation: &str) -> FaultPlan {
    FaultPlan::disabled(seed)
        .with_latency(LatencyFault::new(1, 3))
        .with_one_shot_error(operation, ClientErrorKind::Unavailable)
}

fn assert_fault_stats(target: &str, stats: FaultStats) {
    assert!(
        stats.decisions_total > 0,
        "{target} fault injector was never exercised"
    );
    assert!(
        stats.latency_faults_total > 0,
        "{target} latency fault never fired: {stats:?}"
    );
    assert!(
        stats.terminal_faults_total > 0,
        "{target} one-shot transient error never fired: {stats:?}"
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn soak_world() -> GridWorld {
    let mut walls = Vec::new();
    for x in 0..GRID_WIDTH {
        for y in 0..GRID_HEIGHT {
            if !(y == 0 && (x == 0 || x == 1)) {
                walls.push(GridPos::new(Room::Start, x, y));
            }
        }
    }
    GridWorld {
        name: "m5-soak-two-cell".to_owned(),
        start: GridPos::new(Room::Start, 0, 0),
        walls,
        doors: Vec::new(),
        key: None,
        boss: None,
        goal: GridPos::new(Room::Boss, 4, 4),
        room_base_score: [0.0, 0.0, 0.0],
        room_x_weight: [1.0, 0.0, 0.0],
        room_y_weight: [0.0, 0.0, 0.0],
        prune_cell: None,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct RetentionStats {
    runs: u64,
    total_orphans_removed: u64,
    max_orphans_removed: u64,
    watch_events_compacted: u64,
    observatory_events_compacted: u64,
    busy_skips: u64,
}

struct PeriodicSnapshotRetention {
    every_commits: u64,
    commits_seen: u64,
    hypervisor: SyncAdapter<FakeHypervisor>,
    store: SyncAdapter<InMemorySnapshotStore>,
    observatory: SharedSink,
    stats: Arc<Mutex<RetentionStats>>,
}

impl CrashPolicy for PeriodicSnapshotRetention {
    fn should_crash(&mut self, point: CrashPoint) -> bool {
        if point != CrashPoint::AfterCommitBeforeCheckpoint || self.every_commits == 0 {
            return false;
        }
        self.commits_seen = self.commits_seen.saturating_add(1);
        if !self.commits_seen.is_multiple_of(self.every_commits) {
            return false;
        }

        let committed_refs = self
            .store
            .with_service_sync(|store| {
                query_all_nodes(store)
                    .into_iter()
                    .map(|node| node.snapshot_ref)
                    .collect::<BTreeSet<_>>()
            })
            .ok();
        let Some(committed_refs) = committed_refs else {
            let mut stats = self.stats.lock().expect("retention stats lock");
            stats.busy_skips = stats.busy_skips.saturating_add(1);
            return false;
        };
        let retained = self.hypervisor.with_service_sync(|hypervisor| {
            let removed = hypervisor.retain_live_snapshots(&committed_refs);
            let watch_events_compacted = hypervisor.compact_consumed_watch_events();
            (removed, watch_events_compacted)
        });
        let Ok((removed, watch_events_compacted)) = retained else {
            let mut stats = self.stats.lock().expect("retention stats lock");
            stats.busy_skips = stats.busy_skips.saturating_add(1);
            return false;
        };
        let removed = u64::try_from(removed.len()).unwrap_or(u64::MAX);
        let watch_events_compacted = u64::try_from(watch_events_compacted).unwrap_or(u64::MAX);
        let observatory_events_compacted = u64::try_from(
            self.observatory
                .0
                .lock()
                .expect("observatory lock")
                .clear_events(),
        )
        .unwrap_or(u64::MAX);
        let mut stats = self.stats.lock().expect("retention stats lock");
        stats.runs = stats.runs.saturating_add(1);
        stats.total_orphans_removed = stats.total_orphans_removed.saturating_add(removed);
        stats.max_orphans_removed = stats.max_orphans_removed.max(removed);
        stats.watch_events_compacted = stats
            .watch_events_compacted
            .saturating_add(watch_events_compacted);
        stats.observatory_events_compacted = stats
            .observatory_events_compacted
            .saturating_add(observatory_events_compacted);
        false
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m5_fault_injected_fake_soak_smoke() {
    let duration = Duration::from_secs(env_u64("M5_SOAK_DURATION_SECONDS", 2));
    let seed = env_u64("M5_SOAK_SEED", 0x5E05);
    let fault_seed = env_u64("M5_SOAK_FAULT_SEED", 0xFA171);
    let k = env_u32("M5_SOAK_K", 64);
    let gc_every_commits = env_u64("M5_SOAK_GC_EVERY_COMMITS", 64);

    let world = FakeWorld::with_service_plans(
        soak_world(),
        service_fault_plan(fault_seed.wrapping_add(1), "run"),
        service_fault_plan(fault_seed.wrapping_add(2), "score_batch"),
        service_fault_plan(fault_seed.wrapping_add(3), "put_metadata"),
        service_fault_plan(fault_seed.wrapping_add(4), "propose_bursts"),
    );
    let observatory = SharedSink(std::sync::Arc::new(std::sync::Mutex::new(
        FakeObservatory::with_fault_plan(service_fault_plan(fault_seed.wrapping_add(5), "emit")),
    )));
    let mut config = grid_config(seed);
    config.burst.k_per_expansion = k;
    config.budgets.max_expansions = 10_000_000;
    config.budgets.max_wall_clock_s = duration.as_secs().saturating_add(60);
    config.budgets.max_nodes = 0;
    config.selection.max_visits_per_node = u32::MAX;
    config.selection.exhaust_after_dup_expansions = u32::MAX;
    config.checkpoint.every_commits = 4;
    config.checkpoint.every_seconds = 1;
    config.on_goal = OnGoal::Continue;
    config.scheduling.mode = SchedMode::Deterministic;
    config.scheduling.max_inflight_batches = 1;
    config.validate().expect("soak config validates");
    let hash = support::config_hash(&config);
    let config_hash = hex(&hash);
    let runner_config = orch_server::experiment::RunnerConfig {
        experiment_id: EXPERIMENT_ID.to_owned(),
        run_id: EXPERIMENT_ID.to_owned(),
        producer_id: "m5-soak".to_owned(),
        config,
        config_hash: hash,
    };
    let retention_stats = Arc::new(Mutex::new(RetentionStats::default()));
    let retention_policy: Option<Box<dyn CrashPolicy>> =
        Some(Box::new(PeriodicSnapshotRetention {
            every_commits: gc_every_commits,
            commits_seen: 0,
            hypervisor: world.hypervisor.clone(),
            store: world.store.clone(),
            observatory: observatory.clone(),
            stats: Arc::clone(&retention_stats),
        }));

    let (runner, handle, _mode) = ExperimentRunner::start(
        runner_config,
        sources(),
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        observatory.clone(),
        retention_policy,
    )
    .await
    .expect("runner starts");
    let mut run = tokio::spawn(runner.run());
    let outcome = tokio::select! {
        () = tokio::time::sleep(duration) => {
            handle.send(Control::Stop).expect("stop runner");
            run.await
                .expect("runner task joins")
                .expect("runner returns outcome")
        }
        early = &mut run => {
            let outcome = early
                .expect("runner task joins")
                .expect("runner returns outcome");
            panic!("soak runner ended before requested duration: {outcome:?}");
        }
    };

    assert_eq!(outcome.state, ExperimentState::Stopped);
    assert!(outcome.expansions > 0, "soak made no progress");
    assert!(outcome.nodes > 1, "soak committed no children");

    let store = world.store.service();
    let store = store.lock().await;
    let nodes = query_all_nodes(&*store);
    let committed_refs: BTreeSet<_> = nodes.iter().map(|node| node.snapshot_ref).collect();
    let checkpoint_response = store
        .get_metadata(GetMetadataRequest {
            key: MetadataKey::checkpoint(EXPERIMENT_ID),
        })
        .expect("final checkpoint metadata exists");
    drop(store);
    let checkpoint =
        decode_checkpoint(&checkpoint_response.value, EXPERIMENT_ID, &hash).expect("checkpoint");
    assert_eq!(checkpoint.status, ExperimentState::Stopped);
    assert_eq!(checkpoint.expansions, outcome.expansions);
    assert_eq!(checkpoint.budgets_used.expansions, outcome.expansions);
    assert_eq!(checkpoint.budgets_used.nodes, outcome.nodes);
    assert_eq!(
        checkpoint.batch_seq, outcome.expansions,
        "final checkpoint covers every completed expansion"
    );

    let hypervisor = world.hypervisor.service();
    let mut hypervisor = hypervisor.lock().await;
    let hypervisor_faults = hypervisor.fault_stats();
    let pre_gc_live = hypervisor.live_snapshot_refs();
    let pre_gc_orphans: BTreeSet<_> = pre_gc_live.difference(&committed_refs).copied().collect();
    let removed = hypervisor.retain_live_snapshots(&committed_refs);
    let post_gc_live = hypervisor.live_snapshot_refs();
    drop(hypervisor);

    let scorer_faults = world.scorer.service().lock().await.fault_stats();
    let store_faults = world.store.service().lock().await.fault_stats();
    let synth_faults = world.synth.service().lock().await.fault_stats();
    let observatory_faults = observatory
        .0
        .lock()
        .expect("observatory lock")
        .fault_stats();
    let retention_stats = *retention_stats.lock().expect("retention stats lock");

    assert_eq!(removed, pre_gc_orphans);
    assert_eq!(post_gc_live, committed_refs);
    assert_fault_stats("hypervisor", hypervisor_faults);
    assert_fault_stats("scorer", scorer_faults);
    assert_fault_stats("store", store_faults);
    assert_fault_stats("synth", synth_faults);
    assert_fault_stats("observatory", observatory_faults);

    println!(
        "M5_SOAK_SUMMARY duration_seconds={} k={} seed={} fault_seed={} config_hash={} expansions={} nodes={} committed_refs={} pre_gc_orphans={} post_gc_live={} periodic_gc_runs={} periodic_gc_orphans={} periodic_gc_max_orphans={} watch_events_compacted={} observatory_events_compacted={} retention_busy_skips={} failed_reason=none",
        duration.as_secs(),
        k,
        seed,
        fault_seed,
        config_hash,
        outcome.expansions,
        outcome.nodes,
        committed_refs.len(),
        pre_gc_orphans.len(),
        post_gc_live.len(),
        retention_stats.runs,
        retention_stats.total_orphans_removed,
        retention_stats.max_orphans_removed,
        retention_stats.watch_events_compacted,
        retention_stats.observatory_events_compacted,
        retention_stats.busy_skips,
    );
    println!(
        "M5_SOAK_FAULT_COUNTS hypervisor_decisions={} hypervisor_latency={} hypervisor_terminal={} scorer_decisions={} scorer_latency={} scorer_terminal={} store_decisions={} store_latency={} store_terminal={} synth_decisions={} synth_latency={} synth_terminal={} observatory_decisions={} observatory_latency={} observatory_terminal={}",
        hypervisor_faults.decisions_total,
        hypervisor_faults.latency_faults_total,
        hypervisor_faults.terminal_faults_total,
        scorer_faults.decisions_total,
        scorer_faults.latency_faults_total,
        scorer_faults.terminal_faults_total,
        store_faults.decisions_total,
        store_faults.latency_faults_total,
        store_faults.terminal_faults_total,
        synth_faults.decisions_total,
        synth_faults.latency_faults_total,
        synth_faults.terminal_faults_total,
        observatory_faults.decisions_total,
        observatory_faults.latency_faults_total,
        observatory_faults.terminal_faults_total,
    );
}
