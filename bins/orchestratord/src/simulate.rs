//! The `--simulate` fake world: the boss+credits grid, its feature-map
//! document, and the synth config (zero platform dependencies).

use std::collections::BTreeMap;

use orch_clients::hypervisor::{BootSpec, Digest32, ElfBoot, HashEpochs, MachineConfig};
use orch_core::compile::{
    Discretize, Feature, FeatureMap, FeatureMapKind, FeatureMapMeta, FeatureRegion,
    FeatureSemantics, FeatureStability, FeatureValueType, RegionLayout, RegionLayouts,
};
use orch_core::types::GuestInstructions;
use orch_fakes::{
    grid::GridWorld, hypervisor::FakeHypervisor, observatory::FakeObservatory, scorer::FakeScorer,
    snapshot_store::InMemorySnapshotStore, synth::FakeSynth,
};
use orch_sched::ports::SyncAdapter;
use orch_server::bringup::{ExperimentSources, WorkloadSpec};

pub struct SimulatedWorld {
    pub hypervisor: SyncAdapter<FakeHypervisor>,
    pub scorer: SyncAdapter<FakeScorer>,
    pub store: SyncAdapter<InMemorySnapshotStore>,
    pub synth: SyncAdapter<FakeSynth>,
}

impl SimulatedWorld {
    pub fn new() -> Self {
        let world = GridWorld::three_room();
        Self {
            hypervisor: SyncAdapter::new(FakeHypervisor::with_world(world.clone())),
            scorer: SyncAdapter::new(FakeScorer::with_world(world)),
            store: SyncAdapter::new(InMemorySnapshotStore::new()),
            synth: SyncAdapter::new(FakeSynth::new()),
        }
    }

    pub fn observatory(&self) -> FakeObservatory {
        FakeObservatory::new()
    }
}

fn machine_config() -> MachineConfig {
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

pub fn sources(experiment_id: &str) -> ExperimentSources {
    let mut region_layouts = RegionLayouts::new();
    region_layouts.insert(
        "grid".to_owned(),
        RegionLayout {
            size: 5,
            layout_version: 1,
        },
    );
    ExperimentSources {
        feature_map: FeatureMap {
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
        },
        region_layouts,
        synth_config_yaml: format!(
            "version: 1\nkind: experiment_config\nexperiment_id: {experiment_id}\nmodel: pad\nbutton_alphabet: console16-12btn-v1\ngenerator_mix:\n  weighted_random: 1\n  macro: 0\n  mutation: 0\n  policy: 0\n"
        )
        .into_bytes(),
        macro_pack_yamls: Vec::new(),
        workload: WorkloadSpec {
            machine_config: machine_config(),
            bootstrap_icount_cap: Some(GuestInstructions::new(10_000_000)),
            fps: None,
            pad_layout: None,
        },
    }
}
