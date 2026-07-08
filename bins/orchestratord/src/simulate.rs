//! The `--simulate` fake world: the boss+credits grid, its feature-map
//! document, and the synth config (zero platform dependencies).

use std::collections::BTreeMap;

use orch_clients::hypervisor::{BootSpec, Digest32, ElfBoot, HashEpochs, MachineConfig};
use orch_core::compile::{
    Discretize, Feature, FeatureMap, FeatureMapKind, FeatureMapMeta, FeatureRegion,
    FeatureSemantics, FeatureStability, FeatureValueType, RegionLayout, RegionLayouts,
};
use orch_core::types::GuestInstructions;
use orch_server::bringup::{ExperimentSources, WorkloadSpec};
use orch_simstate::world::{BreakMode, PersistentServices, PersistentWorld};

/// The `--simulate` world is always the persistent shape (plan W2.3): one
/// concrete type for both modes, with the journal simply absent when
/// `--state-dir` is not given.
pub type SimulatedWorld = PersistentWorld;

/// Builds the fake world: journal-less without a state dir; otherwise
/// reload-or-create against `<dir>/journal.v1`. `break_mode` is the
/// negative-control mutation (`ORCH_SIM_BREAK`, test-only) and requires an
/// existing journal.
pub fn world(
    state_dir: Option<&str>,
    break_mode: Option<BreakMode>,
) -> Result<SimulatedWorld, String> {
    let Some(dir) = state_dir else {
        if break_mode.is_some() {
            return Err("ORCH_SIM_BREAK requires --state-dir".to_owned());
        }
        return Ok(PersistentServices::ephemeral().into_world());
    };
    let dir = std::path::Path::new(dir);
    let journal_exists = dir.join(orch_simstate::journal::JOURNAL_FILE).exists();
    let services = match (journal_exists, break_mode) {
        (true, Some(mode)) => {
            PersistentServices::reload_broken(dir, mode)
                .map_err(|error| format!("reload {dir:?}: {error}"))?
                .0
        }
        (true, None) => {
            PersistentServices::reload(dir)
                .map_err(|error| format!("reload {dir:?}: {error}"))?
                .0
        }
        (false, Some(_)) => {
            return Err("ORCH_SIM_BREAK requires an existing journal to corrupt".to_owned())
        }
        (false, None) => PersistentServices::create(dir)
            .map_err(|error| format!("create state dir {dir:?}: {error}"))?,
    };
    Ok(services.into_world())
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
