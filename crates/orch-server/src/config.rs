//! Proto <-> core ExperimentConfig conversion and the config hash.
//!
//! Proto3 semantics per API.md §7: an unset field reads as zero/empty and
//! the documented default is materialized at validation time; the
//! *effective* (defaults-materialized) config is what gets validated and
//! hashed. `config_hash` = blake3 over the canonical proto encoding of the
//! effective config, identical for the gRPC and standalone-YAML paths.

use orch_core::types::{
    Budgets, BurstConfig, CheckpointConfig, ExperimentConfig, LadderConfig, OnGoal, PlateauConfig,
    PolicyKind, PruneAction, SchedMode, SchedulingConfig, SelectionConfig, StagedConfig,
};
use orch_proto::orchestrator_v1 as wire;

fn or_default_u32(value: u32, default: u32) -> u32 {
    if value == 0 {
        default
    } else {
        value
    }
}

fn or_default_u64(value: u64, default: u64) -> u64 {
    if value == 0 {
        default
    } else {
        value
    }
}

fn or_default_f64(value: f64, default: f64) -> f64 {
    if value == 0.0 {
        default
    } else {
        value
    }
}

fn policy_kind(value: i32) -> PolicyKind {
    match wire::PolicyKind::try_from(value) {
        Ok(wire::PolicyKind::Ucb) => PolicyKind::Ucb,
        Ok(wire::PolicyKind::Staged) => PolicyKind::Staged,
        _ => PolicyKind::Softmax,
    }
}

/// Materializes the effective core config from the wire message. Zero /
/// empty proto3 fields take their documented defaults; DETERMINISTIC mode
/// coerces `max_inflight_batches` to 1 (API.md §7 validation rules).
pub fn effective_config(config: &wire::ExperimentConfig) -> ExperimentConfig {
    let defaults = ExperimentConfig::new(0, "", "", "", "");
    let budgets = config.budgets.unwrap_or_default();
    let selection = config.selection.unwrap_or_default();
    let staged = selection.staged.unwrap_or_default();
    let burst = config.burst.unwrap_or_default();
    let plateau = config.plateau.unwrap_or_default();
    let ladder = plateau.ladder.unwrap_or_default();
    let scheduling = config.scheduling.clone().unwrap_or_default();
    let checkpoint = config.checkpoint.unwrap_or_default();

    let mode = match wire::SchedMode::try_from(scheduling.mode) {
        Ok(wire::SchedMode::Deterministic) => SchedMode::Deterministic,
        _ => SchedMode::Fast,
    };
    let max_inflight = if mode == SchedMode::Deterministic {
        1
    } else {
        or_default_u32(
            scheduling.max_inflight_batches,
            defaults.scheduling.max_inflight_batches,
        )
    };

    ExperimentConfig {
        version: config.version,
        seed: config.seed,
        workload_image_ref: config.workload_image_ref.clone(),
        feature_map_ref: config.feature_map_ref.clone(),
        scoring_program_ref: config.scoring_program_ref.clone(),
        synth_config_ref: config.synth_config_ref.clone(),
        macro_pack_refs: config.macro_pack_refs.clone(),
        budgets: Budgets {
            max_nodes: if config.budgets.is_none() {
                defaults.budgets.max_nodes
            } else {
                budgets.max_nodes
            },
            max_wall_clock_s: or_default_u64(
                budgets.max_wall_clock_s,
                defaults.budgets.max_wall_clock_s,
            ),
            max_guest_instructions: budgets.max_guest_instructions,
            max_expansions: budgets.max_expansions,
        },
        selection: SelectionConfig {
            policy: policy_kind(selection.policy),
            alpha: or_default_f64(selection.alpha, defaults.selection.alpha),
            beta: or_default_f64(selection.beta, defaults.selection.beta),
            gamma: or_default_f64(selection.gamma, defaults.selection.gamma),
            delta: or_default_f64(selection.delta, defaults.selection.delta),
            temperature: or_default_f64(selection.temperature, defaults.selection.temperature),
            ucb_c: or_default_f64(selection.ucb_c, defaults.selection.ucb_c),
            staged: StagedConfig {
                inner: policy_kind(staged.inner),
                epsilon_regress: or_default_f64(
                    staged.epsilon_regress,
                    defaults.selection.staged.epsilon_regress,
                ),
            },
            max_visits_per_node: or_default_u32(
                selection.max_visits_per_node,
                defaults.selection.max_visits_per_node,
            ),
            exhaust_after_dup_expansions: or_default_u32(
                selection.exhaust_after_dup_expansions,
                defaults.selection.exhaust_after_dup_expansions,
            ),
        },
        burst: BurstConfig {
            k_per_expansion: or_default_u32(burst.k_per_expansion, defaults.burst.k_per_expansion),
            base_burst_len_frames: or_default_u32(
                burst.base_burst_len_frames,
                defaults.burst.base_burst_len_frames,
            ),
            max_burst_len_frames: or_default_u32(
                burst.max_burst_len_frames,
                defaults.burst.max_burst_len_frames,
            ),
            max_guest_instructions_per_job: burst.max_guest_instructions_per_job,
        },
        plateau: PlateauConfig {
            window_n: or_default_u32(plateau.window_n, defaults.plateau.window_n),
            epsilon_s: or_default_f64(plateau.epsilon_s, defaults.plateau.epsilon_s),
            ladder: LadderConfig {
                burst_len_factor: or_default_f64(
                    ladder.burst_len_factor,
                    defaults.plateau.ladder.burst_len_factor,
                ),
                temp_factor: or_default_f64(
                    ladder.temp_factor,
                    defaults.plateau.ladder.temp_factor,
                ),
                macro_weight_hot: or_default_f64(
                    ladder.macro_weight_hot,
                    defaults.plateau.ladder.macro_weight_hot,
                ),
                backtrack_kappa: or_default_f64(
                    ladder.backtrack_kappa,
                    defaults.plateau.ladder.backtrack_kappa,
                ),
                backtrack_depth_quantile: or_default_f64(
                    ladder.backtrack_depth_quantile,
                    defaults.plateau.ladder.backtrack_depth_quantile,
                ),
                radius_factor: or_default_f64(
                    ladder.radius_factor,
                    defaults.plateau.ladder.radius_factor,
                ),
                max_level: if plateau.ladder.is_none() {
                    defaults.plateau.ladder.max_level
                } else {
                    ladder.max_level
                },
            },
        },
        scheduling: SchedulingConfig {
            mode,
            max_inflight_batches: max_inflight,
            job_timeout_s: or_default_u32(
                scheduling.job_timeout_s,
                defaults.scheduling.job_timeout_s,
            ),
            retry_max: or_default_u32(scheduling.retry_max, defaults.scheduling.retry_max),
            retry_backoff_ms: or_default_u32(
                scheduling.retry_backoff_ms,
                defaults.scheduling.retry_backoff_ms,
            ),
            hypervisor_endpoints: scheduling.hypervisor_endpoints.clone(),
            allow_class_mismatch: scheduling.allow_class_mismatch,
        },
        checkpoint: CheckpointConfig {
            every_commits: or_default_u32(
                checkpoint.every_commits,
                defaults.checkpoint.every_commits,
            ),
            every_seconds: or_default_u32(
                checkpoint.every_seconds,
                defaults.checkpoint.every_seconds,
            ),
        },
        prune_action: match wire::PruneAction::try_from(config.prune_action) {
            Ok(wire::PruneAction::Drop) => PruneAction::Drop,
            _ => PruneAction::Exhausted,
        },
        on_goal: match wire::OnGoal::try_from(config.on_goal) {
            Ok(wire::OnGoal::Continue) => OnGoal::Continue,
            _ => OnGoal::Stop,
        },
        decoded_features: config.decoded_features.clone(),
    }
}

/// The wire encoding of an effective core config (canonical: prost encodes
/// fields in tag order).
pub fn to_wire(config: &ExperimentConfig) -> wire::ExperimentConfig {
    wire::ExperimentConfig {
        version: config.version,
        seed: config.seed,
        workload_image_ref: config.workload_image_ref.clone(),
        feature_map_ref: config.feature_map_ref.clone(),
        scoring_program_ref: config.scoring_program_ref.clone(),
        synth_config_ref: config.synth_config_ref.clone(),
        macro_pack_refs: config.macro_pack_refs.clone(),
        budgets: Some(wire::Budgets {
            max_nodes: config.budgets.max_nodes,
            max_wall_clock_s: config.budgets.max_wall_clock_s,
            max_guest_instructions: config.budgets.max_guest_instructions,
            max_expansions: config.budgets.max_expansions,
        }),
        selection: Some(wire::SelectionConfig {
            policy: match config.selection.policy {
                PolicyKind::Softmax => wire::PolicyKind::Softmax as i32,
                PolicyKind::Ucb => wire::PolicyKind::Ucb as i32,
                PolicyKind::Staged => wire::PolicyKind::Staged as i32,
            },
            alpha: config.selection.alpha,
            beta: config.selection.beta,
            gamma: config.selection.gamma,
            delta: config.selection.delta,
            temperature: config.selection.temperature,
            ucb_c: config.selection.ucb_c,
            staged: Some(wire::StagedConfig {
                inner: match config.selection.staged.inner {
                    PolicyKind::Softmax => wire::PolicyKind::Softmax as i32,
                    PolicyKind::Ucb => wire::PolicyKind::Ucb as i32,
                    PolicyKind::Staged => wire::PolicyKind::Staged as i32,
                },
                epsilon_regress: config.selection.staged.epsilon_regress,
            }),
            max_visits_per_node: config.selection.max_visits_per_node,
            exhaust_after_dup_expansions: config.selection.exhaust_after_dup_expansions,
        }),
        burst: Some(wire::BurstConfig {
            k_per_expansion: config.burst.k_per_expansion,
            base_burst_len_frames: config.burst.base_burst_len_frames,
            max_burst_len_frames: config.burst.max_burst_len_frames,
            max_guest_instructions_per_job: config.burst.max_guest_instructions_per_job,
        }),
        plateau: Some(wire::PlateauConfig {
            window_n: config.plateau.window_n,
            epsilon_s: config.plateau.epsilon_s,
            ladder: Some(wire::LadderConfig {
                burst_len_factor: config.plateau.ladder.burst_len_factor,
                temp_factor: config.plateau.ladder.temp_factor,
                macro_weight_hot: config.plateau.ladder.macro_weight_hot,
                backtrack_kappa: config.plateau.ladder.backtrack_kappa,
                backtrack_depth_quantile: config.plateau.ladder.backtrack_depth_quantile,
                radius_factor: config.plateau.ladder.radius_factor,
                max_level: config.plateau.ladder.max_level,
            }),
        }),
        scheduling: Some(wire::SchedulingConfig {
            mode: match config.scheduling.mode {
                SchedMode::Fast => wire::SchedMode::Fast as i32,
                SchedMode::Deterministic => wire::SchedMode::Deterministic as i32,
            },
            max_inflight_batches: config.scheduling.max_inflight_batches,
            job_timeout_s: config.scheduling.job_timeout_s,
            retry_max: config.scheduling.retry_max,
            retry_backoff_ms: config.scheduling.retry_backoff_ms,
            hypervisor_endpoints: config.scheduling.hypervisor_endpoints.clone(),
            allow_class_mismatch: config.scheduling.allow_class_mismatch,
        }),
        checkpoint: Some(wire::CheckpointConfig {
            every_commits: config.checkpoint.every_commits,
            every_seconds: config.checkpoint.every_seconds,
        }),
        prune_action: match config.prune_action {
            PruneAction::Exhausted => wire::PruneAction::Exhausted as i32,
            PruneAction::Drop => wire::PruneAction::Drop as i32,
        },
        on_goal: match config.on_goal {
            OnGoal::Stop => wire::OnGoal::Stop as i32,
            OnGoal::Continue => wire::OnGoal::Continue as i32,
        },
        decoded_features: config.decoded_features.clone(),
    }
}

/// blake3 over the canonical proto bytes of the effective config (API.md §7).
pub fn config_hash(config: &ExperimentConfig) -> [u8; 32] {
    use prost::Message;
    let bytes = to_wire(config).encode_to_vec();
    *blake3::hash(&bytes).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_materialize_and_round_trip_through_the_wire() {
        let sparse = wire::ExperimentConfig {
            version: 1,
            seed: 42,
            workload_image_ref: "w".to_owned(),
            feature_map_ref: "f".to_owned(),
            scoring_program_ref: "s".to_owned(),
            synth_config_ref: "y".to_owned(),
            ..Default::default()
        };

        let effective = effective_config(&sparse);
        assert!(effective.validate().is_ok());
        assert_eq!(effective.selection.alpha, 1.0);
        assert_eq!(effective.burst.k_per_expansion, 16);
        assert_eq!(effective.checkpoint.every_commits, 64);
        assert_eq!(effective.budgets.max_nodes, 1_000_000);

        // Effective -> wire -> effective is a fixpoint, so the hash is
        // stable no matter which side supplied the defaults.
        let round = effective_config(&to_wire(&effective));
        assert_eq!(round, effective);
        assert_eq!(config_hash(&round), config_hash(&effective));
    }

    #[test]
    fn deterministic_mode_coerces_inflight_to_one() {
        let mut sparse = wire::ExperimentConfig {
            version: 1,
            seed: 1,
            workload_image_ref: "w".to_owned(),
            feature_map_ref: "f".to_owned(),
            scoring_program_ref: "s".to_owned(),
            synth_config_ref: "y".to_owned(),
            ..Default::default()
        };
        sparse.scheduling = Some(wire::SchedulingConfig {
            mode: wire::SchedMode::Deterministic as i32,
            max_inflight_batches: 4,
            ..Default::default()
        });

        let effective = effective_config(&sparse);
        assert_eq!(effective.scheduling.max_inflight_batches, 1);
        assert!(effective.validate().is_ok());
    }
}

// ── standalone YAML (API.md §8, W4.7) ───────────────────────────────────────
//
// The file is the YAML encoding of the identical ExperimentConfig proto;
// unset fields default exactly as unset proto3 fields do, because the YAML
// deserializes into the same sparse wire message before the shared
// defaults-materialization + validation path runs.

mod yaml {
    use serde::Deserialize;

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ExperimentConfigYaml {
        #[serde(default)]
        pub version: u32,
        #[serde(default)]
        pub seed: u64,
        #[serde(default)]
        pub workload_image_ref: String,
        #[serde(default)]
        pub feature_map_ref: String,
        #[serde(default)]
        pub scoring_program_ref: String,
        #[serde(default)]
        pub synth_config_ref: String,
        #[serde(default)]
        pub macro_pack_refs: Vec<String>,
        #[serde(default)]
        pub budgets: Option<BudgetsYaml>,
        #[serde(default)]
        pub selection: Option<SelectionYaml>,
        #[serde(default)]
        pub burst: Option<BurstYaml>,
        #[serde(default)]
        pub plateau: Option<PlateauYaml>,
        #[serde(default)]
        pub scheduling: Option<SchedulingYaml>,
        #[serde(default)]
        pub checkpoint: Option<CheckpointYaml>,
        #[serde(default)]
        pub prune_action: Option<String>,
        #[serde(default)]
        pub on_goal: Option<String>,
        #[serde(default)]
        pub decoded_features: Vec<String>,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct BudgetsYaml {
        #[serde(default)]
        pub max_nodes: u64,
        #[serde(default)]
        pub max_wall_clock_s: u64,
        #[serde(default)]
        pub max_guest_instructions: u64,
        #[serde(default)]
        pub max_expansions: u64,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct SelectionYaml {
        #[serde(default)]
        pub policy: Option<String>,
        #[serde(default)]
        pub alpha: f64,
        #[serde(default)]
        pub beta: f64,
        #[serde(default)]
        pub gamma: f64,
        #[serde(default)]
        pub delta: f64,
        #[serde(default)]
        pub temperature: f64,
        #[serde(default)]
        pub ucb_c: f64,
        #[serde(default)]
        pub staged: Option<StagedYaml>,
        #[serde(default)]
        pub max_visits_per_node: u32,
        #[serde(default)]
        pub exhaust_after_dup_expansions: u32,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct StagedYaml {
        #[serde(default)]
        pub inner: Option<String>,
        #[serde(default)]
        pub epsilon_regress: f64,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct BurstYaml {
        #[serde(default)]
        pub k_per_expansion: u32,
        #[serde(default)]
        pub base_burst_len_frames: u32,
        #[serde(default)]
        pub max_burst_len_frames: u32,
        #[serde(default)]
        pub max_guest_instructions_per_job: u64,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PlateauYaml {
        #[serde(default)]
        pub window_n: u32,
        #[serde(default)]
        pub epsilon_s: f64,
        #[serde(default)]
        pub ladder: Option<LadderYaml>,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct LadderYaml {
        #[serde(default)]
        pub burst_len_factor: f64,
        #[serde(default)]
        pub temp_factor: f64,
        #[serde(default)]
        pub macro_weight_hot: f64,
        #[serde(default)]
        pub backtrack_kappa: f64,
        #[serde(default)]
        pub backtrack_depth_quantile: f64,
        #[serde(default)]
        pub radius_factor: f64,
        #[serde(default)]
        pub max_level: u32,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct SchedulingYaml {
        #[serde(default)]
        pub mode: Option<String>,
        #[serde(default)]
        pub max_inflight_batches: u32,
        #[serde(default)]
        pub job_timeout_s: u32,
        #[serde(default)]
        pub retry_max: u32,
        #[serde(default)]
        pub retry_backoff_ms: u32,
        #[serde(default)]
        pub hypervisor_endpoints: Vec<String>,
        #[serde(default)]
        pub allow_class_mismatch: bool,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct CheckpointYaml {
        #[serde(default)]
        pub every_commits: u32,
        #[serde(default)]
        pub every_seconds: u32,
    }
}

fn enum_tag(value: &Option<String>) -> String {
    value
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace(['-', '_'], "")
}

/// Parses a standalone YAML config into the sparse wire message, exactly as
/// an unset-field proto would arrive over gRPC.
pub fn wire_config_from_yaml(bytes: &[u8]) -> Result<wire::ExperimentConfig, String> {
    let parsed: yaml::ExperimentConfigYaml =
        serde_yaml::from_slice(bytes).map_err(|error| format!("yaml parse failed: {error}"))?;

    let policy = |tag: &Option<String>| -> Result<i32, String> {
        Ok(match enum_tag(tag).as_str() {
            "" | "softmax" => wire::PolicyKind::Softmax as i32,
            "ucb" => wire::PolicyKind::Ucb as i32,
            "staged" => wire::PolicyKind::Staged as i32,
            other => return Err(format!("unknown policy '{other}'")),
        })
    };

    Ok(wire::ExperimentConfig {
        version: parsed.version,
        seed: parsed.seed,
        workload_image_ref: parsed.workload_image_ref,
        feature_map_ref: parsed.feature_map_ref,
        scoring_program_ref: parsed.scoring_program_ref,
        synth_config_ref: parsed.synth_config_ref,
        macro_pack_refs: parsed.macro_pack_refs,
        budgets: parsed.budgets.map(|budgets| wire::Budgets {
            max_nodes: budgets.max_nodes,
            max_wall_clock_s: budgets.max_wall_clock_s,
            max_guest_instructions: budgets.max_guest_instructions,
            max_expansions: budgets.max_expansions,
        }),
        selection: match parsed.selection {
            None => None,
            Some(selection) => Some(wire::SelectionConfig {
                policy: policy(&selection.policy)?,
                alpha: selection.alpha,
                beta: selection.beta,
                gamma: selection.gamma,
                delta: selection.delta,
                temperature: selection.temperature,
                ucb_c: selection.ucb_c,
                staged: match selection.staged {
                    None => None,
                    Some(staged) => Some(wire::StagedConfig {
                        inner: policy(&staged.inner)?,
                        epsilon_regress: staged.epsilon_regress,
                    }),
                },
                max_visits_per_node: selection.max_visits_per_node,
                exhaust_after_dup_expansions: selection.exhaust_after_dup_expansions,
            }),
        },
        burst: parsed.burst.map(|burst| wire::BurstConfig {
            k_per_expansion: burst.k_per_expansion,
            base_burst_len_frames: burst.base_burst_len_frames,
            max_burst_len_frames: burst.max_burst_len_frames,
            max_guest_instructions_per_job: burst.max_guest_instructions_per_job,
        }),
        plateau: parsed.plateau.map(|plateau| wire::PlateauConfig {
            window_n: plateau.window_n,
            epsilon_s: plateau.epsilon_s,
            ladder: plateau.ladder.map(|ladder| wire::LadderConfig {
                burst_len_factor: ladder.burst_len_factor,
                temp_factor: ladder.temp_factor,
                macro_weight_hot: ladder.macro_weight_hot,
                backtrack_kappa: ladder.backtrack_kappa,
                backtrack_depth_quantile: ladder.backtrack_depth_quantile,
                radius_factor: ladder.radius_factor,
                max_level: ladder.max_level,
            }),
        }),
        scheduling: match parsed.scheduling {
            None => None,
            Some(scheduling) => Some(wire::SchedulingConfig {
                mode: match enum_tag(&scheduling.mode).as_str() {
                    "" | "fast" => wire::SchedMode::Fast as i32,
                    "deterministic" => wire::SchedMode::Deterministic as i32,
                    other => return Err(format!("unknown scheduling mode '{other}'")),
                },
                max_inflight_batches: scheduling.max_inflight_batches,
                job_timeout_s: scheduling.job_timeout_s,
                retry_max: scheduling.retry_max,
                retry_backoff_ms: scheduling.retry_backoff_ms,
                hypervisor_endpoints: scheduling.hypervisor_endpoints,
                allow_class_mismatch: scheduling.allow_class_mismatch,
            }),
        },
        checkpoint: parsed.checkpoint.map(|checkpoint| wire::CheckpointConfig {
            every_commits: checkpoint.every_commits,
            every_seconds: checkpoint.every_seconds,
        }),
        prune_action: match enum_tag(&parsed.prune_action).as_str() {
            "" | "exhausted" | "pruneactionexhausted" => wire::PruneAction::Exhausted as i32,
            "drop" | "pruneactiondrop" => wire::PruneAction::Drop as i32,
            other => return Err(format!("unknown prune_action '{other}'")),
        },
        on_goal: match enum_tag(&parsed.on_goal).as_str() {
            "" | "stop" | "ongoalstop" => wire::OnGoal::Stop as i32,
            "continue" | "ongoalcontinue" => wire::OnGoal::Continue as i32,
            other => return Err(format!("unknown on_goal '{other}'")),
        },
        decoded_features: parsed.decoded_features,
    })
}

#[cfg(test)]
mod yaml_tests {
    use super::*;

    #[test]
    fn yaml_and_grpc_paths_agree_on_the_effective_config_and_hash() {
        let yaml = br#"
version: 1
seed: 42
workload_image_ref: w
feature_map_ref: f
scoring_program_ref: s
synth_config_ref: y
scheduling:
  mode: deterministic
selection:
  temperature: 8.0
"#;
        let sparse = wire_config_from_yaml(yaml).expect("yaml parses");
        let effective = effective_config(&sparse);
        assert!(effective.validate().is_ok());
        assert_eq!(
            effective.scheduling.mode,
            orch_core::types::SchedMode::Deterministic
        );
        assert_eq!(effective.scheduling.max_inflight_batches, 1);
        assert_eq!(effective.selection.temperature, 8.0);
        assert_eq!(effective.selection.alpha, 1.0); // default materialized

        // Identical sparse config over gRPC hashes identically.
        let grpc_effective = effective_config(&sparse.clone());
        assert_eq!(config_hash(&effective), config_hash(&grpc_effective));
    }

    #[test]
    fn yaml_rejects_unknown_fields_and_enums() {
        assert!(wire_config_from_yaml(b"bogus_field: 1\n").is_err());
        assert!(wire_config_from_yaml(b"scheduling:\n  mode: warp\n").is_err());
    }
}
