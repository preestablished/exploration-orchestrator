//! Shared fixture for the M4 acceptance suite: fake-backed services, the
//! grid feature-map document, experiment sources, and runner construction.

#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use orch_clients::hypervisor::{BootSpec, Digest32, ElfBoot, HashEpochs, MachineConfig};
use orch_core::{
    compile::{
        Discretize, Feature, FeatureMap, FeatureMapKind, FeatureMapMeta, FeatureRegion,
        FeatureSemantics, FeatureStability, FeatureValueType, RegionLayout, RegionLayouts,
    },
    types::{ExperimentConfig, GuestInstructions, SchedMode},
};
use orch_fakes::{
    fault::{FaultInjector, FaultPlan, FaultRequest, FaultTarget, FaultTerminal},
    grid::GridWorld,
    hypervisor::FakeHypervisor,
    observatory::FakeObservatory,
    scorer::FakeScorer,
    snapshot_store::InMemorySnapshotStore,
    synth::FakeSynth,
};
use orch_sched::ports::{LatencyProbe, PendingCall, SyncAdapter};
use orch_server::{
    bringup::{ExperimentSources, WorkloadSpec},
    experiment::RunnerConfig,
};

pub const EXPERIMENT_ID: &str = "m4-acceptance";

pub fn machine_config() -> MachineConfig {
    MachineConfig {
        version: 1,
        mem_bytes: 128 * 1024 * 1024,
        vcpus: 1,
        clock_num: 1,
        clock_den: 1,
        base_image_hash: Digest32::new([0xAA; 32]),
        boot: BootSpec::Elf(ElfBoot {
            kernel_hash: Digest32::new([0xBB; 32]),
            cmdline: b"console=ttyS0".to_vec(),
        }),
        epoch_len: GuestInstructions::new(50_000_000),
        hash_epochs: HashEpochs::EpochsOn,
        skid_margin: 8192,
    }
}

fn grid_feature(name: &str, offset: u64) -> Feature {
    Feature {
        name: name.to_owned(),
        region: "grid".to_owned(),
        offset,
        value_type: FeatureValueType::U8,
        width: None,
        semantics: FeatureSemantics::new("counter"),
        stability: FeatureStability::Stable,
        discretize: Discretize::Identity,
        valid_when: None,
        extra: BTreeMap::new(),
    }
}

/// The grid world's 5-byte feature document (room, x, y, keys, boss_hp).
pub fn grid_feature_map() -> FeatureMap {
    FeatureMap {
        schema_version: 1,
        kind: FeatureMapKind::FeatureMap,
        meta: FeatureMapMeta {
            name: "grid".to_owned(),
            workload: "grid-world".to_owned(),
            game_revision: "r1".to_owned(),
            version: 1,
            extra: BTreeMap::new(),
        },
        regions: vec![FeatureRegion {
            name: "grid".to_owned(),
            size: 5,
            extra: BTreeMap::new(),
        }],
        features: vec![
            grid_feature("room", 0),
            grid_feature("x", 1),
            grid_feature("y", 2),
            grid_feature("keys", 3),
            grid_feature("boss_hp", 4),
        ],
        extra: BTreeMap::new(),
    }
}

pub fn region_layouts() -> RegionLayouts {
    let mut layouts = RegionLayouts::new();
    layouts.insert(
        "grid".to_owned(),
        RegionLayout {
            size: 5,
            layout_version: 1,
        },
    );
    layouts
}

pub fn synth_config_yaml() -> Vec<u8> {
    format!(
        "version: 1\nkind: experiment_config\nexperiment_id: {EXPERIMENT_ID}\nmodel: pad\nbutton_alphabet: console16-12btn-v1\ngenerator_mix:\n  weighted_random: 1\n  macro: 0\n  mutation: 0\n  policy: 0\n"
    )
    .into_bytes()
}

pub fn sources() -> ExperimentSources {
    ExperimentSources {
        feature_map: grid_feature_map(),
        region_layouts: region_layouts(),
        synth_config_yaml: synth_config_yaml(),
        macro_pack_yamls: Vec::new(),
        workload: WorkloadSpec {
            machine_config: machine_config(),
            bootstrap_icount_cap: Some(GuestInstructions::new(10_000_000)),
            fps: None,
            pad_layout: None,
        },
    }
}

/// Grid-tuned experiment config: short bursts, small expansions, snappy
/// checkpoints. Callers override what their bar needs.
pub fn grid_config(seed: u64) -> ExperimentConfig {
    let mut config = ExperimentConfig::new(
        seed,
        "workload://grid",
        "featmap://grid",
        "score://grid",
        "synth://grid",
    );
    config.burst.k_per_expansion = 8;
    config.burst.base_burst_len_frames = 3;
    config.burst.max_burst_len_frames = 12;
    config.selection.max_visits_per_node = 256;
    config.selection.exhaust_after_dup_expansions = 32;
    config.budgets.max_expansions = 4_096;
    // Virtual clock: generous wall budget so only expansions bound runs.
    config.budgets.max_wall_clock_s = 86_400;
    config.budgets.max_nodes = 0;
    config.checkpoint.every_commits = 16;
    config.checkpoint.every_seconds = 3_600;
    config.selection.temperature = 8.0;
    config.scheduling.mode = SchedMode::Deterministic;
    config.scheduling.max_inflight_batches = 1;
    config.validate().expect("grid config is valid");
    config
}

pub fn config_hash(config: &ExperimentConfig) -> [u8; 32] {
    // blake3 over the canonical (postcard) encoding of the effective config.
    let bytes = postcard::to_allocvec(config).expect("config encodes");
    *blake3::hash(&bytes).as_bytes()
}

pub fn runner_config(seed: u64) -> RunnerConfig {
    let config = grid_config(seed);
    let hash = config_hash(&config);
    RunnerConfig {
        experiment_id: EXPERIMENT_ID.to_owned(),
        run_id: EXPERIMENT_ID.to_owned(),
        producer_id: "orchestratord-test".to_owned(),
        config,
        config_hash: hash,
    }
}

/// One fake service world, shareable across runner incarnations (the chaos
/// harness keeps these alive and constructs fresh runners against them).
pub struct FakeWorld {
    pub hypervisor: SyncAdapter<FakeHypervisor>,
    pub scorer: SyncAdapter<FakeScorer>,
    pub store: SyncAdapter<InMemorySnapshotStore>,
    pub synth: SyncAdapter<FakeSynth>,
    pub latency: FakeWorldLatencyStats,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LatencyChargeStats {
    pub calls_total: u64,
    pub charged_calls_total: u64,
    pub charged_ticks_total: u64,
    pub timeout_charges_total: u64,
}

#[derive(Clone, Debug, Default)]
pub struct SharedLatencyStats(Arc<Mutex<LatencyChargeStats>>);

impl SharedLatencyStats {
    fn record(&self, pending: PendingCall) {
        let mut stats = self.0.lock().expect("latency stats lock");
        stats.calls_total = stats.calls_total.saturating_add(1);
        if pending.latency_ticks > 0 || pending.timeout {
            stats.charged_calls_total = stats.charged_calls_total.saturating_add(1);
        }
        stats.charged_ticks_total = stats
            .charged_ticks_total
            .saturating_add(u64::from(pending.latency_ticks));
        if pending.timeout {
            stats.timeout_charges_total = stats.timeout_charges_total.saturating_add(1);
        }
    }

    pub fn snapshot(&self) -> LatencyChargeStats {
        *self.0.lock().expect("latency stats lock")
    }
}

#[derive(Clone, Debug, Default)]
pub struct FakeWorldLatencyStats {
    pub hypervisor: SharedLatencyStats,
    pub scorer: SharedLatencyStats,
    pub store: SharedLatencyStats,
    pub synth: SharedLatencyStats,
}

pub struct FakeLatencyProbe {
    target: FaultTarget,
    injector: FaultInjector,
    stats: SharedLatencyStats,
}

impl FakeLatencyProbe {
    pub fn new(target: FaultTarget, plan: FaultPlan, stats: SharedLatencyStats) -> Self {
        Self {
            target,
            injector: FaultInjector::new(plan),
            stats,
        }
    }
}

impl LatencyProbe for FakeLatencyProbe {
    fn pending_call(&mut self, operation: &'static str, request_identity: &[u8]) -> PendingCall {
        let decision = self.injector.decide(
            FaultRequest::new(self.target, operation, request_identity),
            0,
        );
        let pending = PendingCall {
            latency_ticks: decision.latency_ticks,
            timeout: matches!(decision.terminal, FaultTerminal::Timeout),
        };
        self.stats.record(pending);
        pending
    }
}

impl FakeWorld {
    pub fn new(world: GridWorld) -> Self {
        Self::with_plans(world, FaultPlan::disabled(0))
    }

    pub fn with_plans(world: GridWorld, hypervisor_plan: FaultPlan) -> Self {
        Self::with_service_plans(
            world,
            hypervisor_plan,
            FaultPlan::disabled(0),
            FaultPlan::disabled(0),
            FaultPlan::disabled(0),
        )
    }

    pub fn with_service_plans(
        world: GridWorld,
        hypervisor_plan: FaultPlan,
        scorer_plan: FaultPlan,
        store_plan: FaultPlan,
        synth_plan: FaultPlan,
    ) -> Self {
        let latency = FakeWorldLatencyStats::default();
        Self {
            hypervisor: SyncAdapter::new(FakeHypervisor::with_world_slots_and_fault_plan(
                world.clone(),
                8,
                hypervisor_plan.clone(),
            ))
            .with_probe(FakeLatencyProbe::new(
                FaultTarget::Hypervisor,
                hypervisor_plan,
                latency.hypervisor.clone(),
            )),
            scorer: SyncAdapter::new(FakeScorer::with_world_and_fault_plan(
                world,
                scorer_plan.clone(),
            ))
            .with_probe(FakeLatencyProbe::new(
                FaultTarget::Scorer,
                scorer_plan,
                latency.scorer.clone(),
            )),
            store: SyncAdapter::new(InMemorySnapshotStore::with_fault_plan(store_plan.clone()))
                .with_probe(FakeLatencyProbe::new(
                    FaultTarget::SnapshotStore,
                    store_plan,
                    latency.store.clone(),
                )),
            synth: SyncAdapter::new(FakeSynth::with_fault_plan(synth_plan.clone())).with_probe(
                FakeLatencyProbe::new(FaultTarget::Synth, synth_plan, latency.synth.clone()),
            ),
            latency,
        }
    }

    pub fn observatory(&self) -> FakeObservatory {
        FakeObservatory::new()
    }
}

/// Event sink handle the test keeps while the runner owns the emitter.
#[derive(Clone, Default)]
pub struct SharedSink(pub std::sync::Arc<std::sync::Mutex<FakeObservatory>>);

impl orch_clients::observatory::EventSink for SharedSink {
    fn emit(
        &mut self,
        envelope: orch_clients::observatory::EventEnvelope,
    ) -> orch_clients::ClientResult<()> {
        self.0.lock().expect("sink lock").emit(envelope)
    }

    fn acked_seq(&self) -> orch_clients::ClientResult<u64> {
        self.0.lock().expect("sink lock").acked_seq()
    }
}

/// Seed-gate event hash: blake3 over each envelope's
/// (ts_logical, event_type, canonical payload) — producer_id and seq are
/// nondeterministic by the platform's own contract (plan D6, disclosed).
pub fn event_sequence_hash(sink: &SharedSink) -> [u8; 32] {
    let sink = sink.0.lock().expect("sink lock");
    let mut hasher = blake3::Hasher::new();
    for event in sink.events() {
        hasher.update(&event.ts_logical.to_le_bytes());
        hasher.update(&(event.event_type.len() as u64).to_le_bytes());
        hasher.update(event.event_type.as_bytes());
        let payload = postcard::to_allocvec(&event.payload).expect("payload encodes");
        hasher.update(&(payload.len() as u64).to_le_bytes());
        hasher.update(&payload);
    }
    *hasher.finalize().as_bytes()
}
