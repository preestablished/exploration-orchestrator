use std::path::Path;

use orch_core::{
    config_rejections,
    types::{ExperimentConfig, OnGoal, PolicyKind, PruneAction, SchedMode},
};

struct RejectCase {
    name: &'static str,
    mutate: fn(&mut ExperimentConfig),
    expected: &'static str,
}

fn valid_config() -> ExperimentConfig {
    let mut config = ExperimentConfig::new(
        0x5EED,
        "workload://grid",
        "featmap://grid",
        "score://grid",
        "synth://grid",
    );
    config.scheduling.mode = SchedMode::Fast;
    config.scheduling.max_inflight_batches = 2;
    config
}

fn assert_rejects(case: &RejectCase) {
    let mut config = valid_config();
    (case.mutate)(&mut config);

    let messages: Vec<String> = config
        .validate_all()
        .into_iter()
        .map(|error| error.to_string())
        .collect();

    assert!(
        messages.iter().any(|message| message == case.expected),
        "{} expected {:?}, got {:?}",
        case.name,
        case.expected,
        messages
    );
}

#[test]
fn materialized_config_rejects_api_section_7_invalid_shapes() {
    let cases = [
        RejectCase {
            name: "version",
            mutate: |config| config.version = 2,
            expected: "invalid config version 2",
        },
        RejectCase {
            name: "seed",
            mutate: |config| config.seed = 0,
            expected: "missing required field seed",
        },
        RejectCase {
            name: "workload_image_ref",
            mutate: |config| config.workload_image_ref.clear(),
            expected: "missing required field workload_image_ref",
        },
        RejectCase {
            name: "feature_map_ref",
            mutate: |config| config.feature_map_ref.clear(),
            expected: "missing required field feature_map_ref",
        },
        RejectCase {
            name: "scoring_program_ref",
            mutate: |config| config.scoring_program_ref.clear(),
            expected: "missing required field scoring_program_ref",
        },
        RejectCase {
            name: "synth_config_ref",
            mutate: |config| config.synth_config_ref.clear(),
            expected: "missing required field synth_config_ref",
        },
        RejectCase {
            name: "selection.alpha",
            mutate: |config| config.selection.alpha = f64::NAN,
            expected: "field out of range selection.alpha",
        },
        RejectCase {
            name: "selection.beta",
            mutate: |config| config.selection.beta = -1.0,
            expected: "field out of range selection.beta",
        },
        RejectCase {
            name: "selection.gamma",
            mutate: |config| config.selection.gamma = f64::INFINITY,
            expected: "field out of range selection.gamma",
        },
        RejectCase {
            name: "selection.delta",
            mutate: |config| config.selection.delta = -0.1,
            expected: "field out of range selection.delta",
        },
        RejectCase {
            name: "selection.temperature",
            mutate: |config| config.selection.temperature = 0.0,
            expected: "field out of range selection.temperature",
        },
        RejectCase {
            name: "selection.ucb_c",
            mutate: |config| config.selection.ucb_c = f64::NAN,
            expected: "field out of range selection.ucb_c",
        },
        RejectCase {
            name: "selection.staged.inner",
            mutate: |config| {
                config.selection.policy = PolicyKind::Staged;
                config.selection.staged.inner = PolicyKind::Staged;
            },
            expected: "staged inner policy cannot be staged",
        },
        RejectCase {
            name: "selection.staged.epsilon_regress",
            mutate: |config| config.selection.staged.epsilon_regress = 1.1,
            expected: "field out of range selection.staged.epsilon_regress",
        },
        RejectCase {
            name: "selection.max_visits_per_node",
            mutate: |config| config.selection.max_visits_per_node = 0,
            expected: "field out of range selection.max_visits_per_node",
        },
        RejectCase {
            name: "selection.exhaust_after_dup_expansions",
            mutate: |config| config.selection.exhaust_after_dup_expansions = 0,
            expected: "field out of range selection.exhaust_after_dup_expansions",
        },
        RejectCase {
            name: "burst.k_per_expansion",
            mutate: |config| config.burst.k_per_expansion = 257,
            expected: "field out of range burst.k_per_expansion",
        },
        RejectCase {
            name: "burst.base_burst_len_frames",
            mutate: |config| {
                config.burst.base_burst_len_frames = config.burst.max_burst_len_frames + 1
            },
            expected: "field out of range burst.base_burst_len_frames",
        },
        RejectCase {
            name: "burst.max_burst_len_frames",
            mutate: |config| config.burst.max_burst_len_frames = 0,
            expected: "field out of range burst.max_burst_len_frames",
        },
        RejectCase {
            name: "plateau.window_n",
            mutate: |config| config.plateau.window_n = 9,
            expected: "field out of range plateau.window_n",
        },
        RejectCase {
            name: "plateau.epsilon_s",
            mutate: |config| config.plateau.epsilon_s = f64::NAN,
            expected: "field out of range plateau.epsilon_s",
        },
        RejectCase {
            name: "plateau.ladder.burst_len_factor",
            mutate: |config| config.plateau.ladder.burst_len_factor = 0.99,
            expected: "field out of range plateau.ladder.burst_len_factor",
        },
        RejectCase {
            name: "plateau.ladder.temp_factor",
            mutate: |config| config.plateau.ladder.temp_factor = f64::INFINITY,
            expected: "field out of range plateau.ladder.temp_factor",
        },
        RejectCase {
            name: "plateau.ladder.macro_weight_hot",
            mutate: |config| config.plateau.ladder.macro_weight_hot = -0.1,
            expected: "field out of range plateau.ladder.macro_weight_hot",
        },
        RejectCase {
            name: "plateau.ladder.backtrack_kappa",
            mutate: |config| config.plateau.ladder.backtrack_kappa = f64::NAN,
            expected: "field out of range plateau.ladder.backtrack_kappa",
        },
        RejectCase {
            name: "plateau.ladder.backtrack_depth_quantile",
            mutate: |config| config.plateau.ladder.backtrack_depth_quantile = -0.1,
            expected: "field out of range plateau.ladder.backtrack_depth_quantile",
        },
        RejectCase {
            name: "plateau.ladder.radius_factor",
            mutate: |config| config.plateau.ladder.radius_factor = 0.5,
            expected: "field out of range plateau.ladder.radius_factor",
        },
        RejectCase {
            name: "plateau.ladder.max_level",
            mutate: |config| config.plateau.ladder.max_level = 5,
            expected: "field out of range plateau.ladder.max_level",
        },
        RejectCase {
            name: "scheduling.max_inflight_batches fast",
            mutate: |config| {
                config.scheduling.mode = SchedMode::Fast;
                config.scheduling.max_inflight_batches = 0;
            },
            expected: "field out of range scheduling.max_inflight_batches",
        },
        RejectCase {
            name: "scheduling.max_inflight_batches deterministic",
            mutate: |config| {
                config.scheduling.mode = SchedMode::Deterministic;
                config.scheduling.max_inflight_batches = 2;
            },
            expected: "field out of range scheduling.max_inflight_batches",
        },
        RejectCase {
            name: "scheduling.job_timeout_s",
            mutate: |config| config.scheduling.job_timeout_s = 0,
            expected: "field out of range scheduling.job_timeout_s",
        },
        RejectCase {
            name: "checkpoint.every_commits",
            mutate: |config| config.checkpoint.every_commits = 0,
            expected: "field out of range checkpoint.every_commits",
        },
        RejectCase {
            name: "checkpoint.every_seconds",
            mutate: |config| config.checkpoint.every_seconds = 0,
            expected: "field out of range checkpoint.every_seconds",
        },
    ];

    for case in cases {
        assert_rejects(&case);
    }
}

#[test]
fn materialized_config_documents_non_rejectable_v1_shapes() {
    let mut config = valid_config();
    config.macro_pack_refs.push(String::new());
    config.budgets.max_nodes = 0;
    config.budgets.max_wall_clock_s = 0;
    config.budgets.max_guest_instructions = 0;
    config.budgets.max_expansions = 0;
    config.burst.max_guest_instructions_per_job = 0;
    config.scheduling.retry_max = 0;
    config.scheduling.retry_backoff_ms = 0;
    config.scheduling.hypervisor_endpoints.push(String::new());
    config.prune_action = PruneAction::Drop;
    config.on_goal = OnGoal::Continue;

    assert_eq!(config.validate_all(), Vec::new());
}

#[test]
fn config_rejection_doc_matches_code_catalog() {
    let doc_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("config-validation-rejections.md");
    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", doc_path.display()));

    let mut in_catalog = false;
    let entries: Vec<String> = doc
        .lines()
        .filter_map(|line| {
            if line == "## Catalog" {
                in_catalog = true;
                return None;
            }
            if in_catalog && line.starts_with("## ") {
                in_catalog = false;
            }
            if !in_catalog {
                return None;
            }
            line.strip_prefix("- `")
                .and_then(|rest| rest.strip_suffix('`'))
                .map(str::to_owned)
        })
        .collect();

    assert_eq!(entries, config_rejections::CATALOG);
}
