mod support;

use orch_proto::orchestrator_v1 as wire;
use orch_proto::orchestrator_v1::exploration_orchestrator_server::ExplorationOrchestrator;
use orch_server::{
    bringup::materialize_decoded_features,
    config::{
        config_validation_failed_detail, effective_config, to_wire, validate_wire_config,
        wire_config_from_yaml,
    },
    events::SharedSink,
    service::OrchestratorService,
};
use support::{grid_config, grid_feature_map, region_layouts, sources, FakeWorld};
use tonic::Request;

fn valid_wire_config() -> wire::ExperimentConfig {
    to_wire(&grid_config(0x5EED))
}

fn messages(config: &wire::ExperimentConfig) -> Vec<String> {
    validate_wire_config(config)
        .into_iter()
        .map(|error| error.to_string())
        .collect()
}

fn assert_rejects(name: &str, mutate: impl FnOnce(&mut wire::ExperimentConfig), expected: &str) {
    let mut config = valid_wire_config();
    mutate(&mut config);
    let messages = messages(&config);
    assert!(
        messages.iter().any(|message| message == expected),
        "{name} expected {expected:?}, got {messages:?}"
    );
}

fn assert_accepts(
    name: &str,
    mutate: impl FnOnce(&mut wire::ExperimentConfig),
    check: impl FnOnce(&orch_core::types::ExperimentConfig),
) {
    let mut config = valid_wire_config();
    mutate(&mut config);
    let messages = messages(&config);
    assert!(messages.is_empty(), "{name} rejected with {messages:?}");
    let effective = effective_config(&config);
    check(&effective);
}

#[test]
fn wire_validation_rejects_unknown_enums_before_defaulting() {
    assert_rejects(
        "selection.policy",
        |config| config.selection.as_mut().unwrap().policy = 99,
        "unknown enum value selection.policy",
    );
    assert_rejects(
        "selection.staged.inner",
        |config| {
            config
                .selection
                .as_mut()
                .unwrap()
                .staged
                .as_mut()
                .unwrap()
                .inner = 99;
        },
        "unknown enum value selection.staged.inner",
    );
    assert_rejects(
        "scheduling.mode",
        |config| config.scheduling.as_mut().unwrap().mode = 99,
        "unknown enum value scheduling.mode",
    );
    assert_rejects(
        "prune_action",
        |config| config.prune_action = 99,
        "unknown enum value prune_action",
    );
    assert_rejects(
        "on_goal",
        |config| config.on_goal = 99,
        "unknown enum value on_goal",
    );
}

#[test]
fn wire_validation_matrix_covers_api_section_7_shapes() {
    assert_rejects(
        "version",
        |config| config.version = 2,
        "invalid config version 2",
    );
    assert_rejects(
        "seed",
        |config| config.seed = 0,
        "missing required field seed",
    );
    assert_rejects(
        "workload_image_ref",
        |config| config.workload_image_ref.clear(),
        "missing required field workload_image_ref",
    );
    assert_rejects(
        "feature_map_ref",
        |config| config.feature_map_ref.clear(),
        "missing required field feature_map_ref",
    );
    assert_rejects(
        "scoring_program_ref",
        |config| config.scoring_program_ref.clear(),
        "missing required field scoring_program_ref",
    );
    assert_rejects(
        "synth_config_ref",
        |config| config.synth_config_ref.clear(),
        "missing required field synth_config_ref",
    );
    assert_rejects(
        "selection.alpha",
        |config| config.selection.as_mut().unwrap().alpha = f64::NAN,
        "field out of range selection.alpha",
    );
    assert_rejects(
        "selection.beta",
        |config| config.selection.as_mut().unwrap().beta = -1.0,
        "field out of range selection.beta",
    );
    assert_rejects(
        "selection.gamma",
        |config| config.selection.as_mut().unwrap().gamma = f64::INFINITY,
        "field out of range selection.gamma",
    );
    assert_rejects(
        "selection.delta",
        |config| config.selection.as_mut().unwrap().delta = -0.1,
        "field out of range selection.delta",
    );
    assert_rejects(
        "selection.temperature",
        |config| config.selection.as_mut().unwrap().temperature = -1.0,
        "field out of range selection.temperature",
    );
    assert_rejects(
        "selection.ucb_c",
        |config| config.selection.as_mut().unwrap().ucb_c = f64::NAN,
        "field out of range selection.ucb_c",
    );
    assert_rejects(
        "selection.staged",
        |config| {
            let selection = config.selection.as_mut().unwrap();
            selection.policy = wire::PolicyKind::Staged as i32;
            selection.staged.as_mut().unwrap().inner = wire::PolicyKind::Staged as i32;
        },
        "staged inner policy cannot be staged",
    );
    assert_rejects(
        "selection.staged.epsilon_regress",
        |config| {
            config
                .selection
                .as_mut()
                .unwrap()
                .staged
                .as_mut()
                .unwrap()
                .epsilon_regress = 1.1
        },
        "field out of range selection.staged.epsilon_regress",
    );
    assert_rejects(
        "burst.k_per_expansion",
        |config| config.burst.as_mut().unwrap().k_per_expansion = 257,
        "field out of range burst.k_per_expansion",
    );
    assert_rejects(
        "burst.base_burst_len_frames",
        |config| {
            let burst = config.burst.as_mut().unwrap();
            burst.base_burst_len_frames = burst.max_burst_len_frames + 1;
        },
        "field out of range burst.base_burst_len_frames",
    );
    assert_rejects(
        "plateau.window_n",
        |config| config.plateau.as_mut().unwrap().window_n = 9,
        "field out of range plateau.window_n",
    );
    assert_rejects(
        "plateau.epsilon_s",
        |config| config.plateau.as_mut().unwrap().epsilon_s = -0.1,
        "field out of range plateau.epsilon_s",
    );
    assert_rejects(
        "plateau.ladder.burst_len_factor",
        |config| {
            config
                .plateau
                .as_mut()
                .unwrap()
                .ladder
                .as_mut()
                .unwrap()
                .burst_len_factor = 0.5
        },
        "field out of range plateau.ladder.burst_len_factor",
    );
    assert_rejects(
        "plateau.ladder.temp_factor",
        |config| {
            config
                .plateau
                .as_mut()
                .unwrap()
                .ladder
                .as_mut()
                .unwrap()
                .temp_factor = f64::NAN
        },
        "field out of range plateau.ladder.temp_factor",
    );
    assert_rejects(
        "plateau.ladder.macro_weight_hot",
        |config| {
            config
                .plateau
                .as_mut()
                .unwrap()
                .ladder
                .as_mut()
                .unwrap()
                .macro_weight_hot = -0.1
        },
        "field out of range plateau.ladder.macro_weight_hot",
    );
    assert_rejects(
        "plateau.ladder.backtrack_kappa",
        |config| {
            config
                .plateau
                .as_mut()
                .unwrap()
                .ladder
                .as_mut()
                .unwrap()
                .backtrack_kappa = -0.1
        },
        "field out of range plateau.ladder.backtrack_kappa",
    );
    assert_rejects(
        "plateau.ladder.backtrack_depth_quantile",
        |config| {
            config
                .plateau
                .as_mut()
                .unwrap()
                .ladder
                .as_mut()
                .unwrap()
                .backtrack_depth_quantile = 2.0;
        },
        "field out of range plateau.ladder.backtrack_depth_quantile",
    );
    assert_rejects(
        "plateau.ladder.radius_factor",
        |config| {
            config
                .plateau
                .as_mut()
                .unwrap()
                .ladder
                .as_mut()
                .unwrap()
                .radius_factor = 0.5
        },
        "field out of range plateau.ladder.radius_factor",
    );
    assert_rejects(
        "plateau.ladder.max_level",
        |config| {
            config
                .plateau
                .as_mut()
                .unwrap()
                .ladder
                .as_mut()
                .unwrap()
                .max_level = 5
        },
        "field out of range plateau.ladder.max_level",
    );

    assert_accepts(
        "macro_pack_refs are opaque refs",
        |config| config.macro_pack_refs.push(String::new()),
        |_| {},
    );
    assert_accepts(
        "explicit budget zeros survive v1 defaulting",
        |config| {
            config.budgets.as_mut().unwrap().max_nodes = 0;
            config.budgets.as_mut().unwrap().max_wall_clock_s = 0;
            config.budgets.as_mut().unwrap().max_guest_instructions = 0;
            config.budgets.as_mut().unwrap().max_expansions = 0;
        },
        |effective| {
            assert_eq!(effective.budgets.max_nodes, 0);
            assert_eq!(effective.budgets.max_wall_clock_s, 0);
            assert_eq!(effective.budgets.max_guest_instructions, 0);
            assert_eq!(effective.budgets.max_expansions, 0);
        },
    );
    assert_accepts(
        "selection zero knobs default over proto3 v1",
        |config| {
            let selection = config.selection.as_mut().unwrap();
            selection.max_visits_per_node = 0;
            selection.exhaust_after_dup_expansions = 0;
        },
        |effective| {
            assert_eq!(effective.selection.max_visits_per_node, 64);
            assert_eq!(effective.selection.exhaust_after_dup_expansions, 8);
        },
    );
    assert_accepts(
        "burst zero knobs default over proto3 v1",
        |config| {
            let burst = config.burst.as_mut().unwrap();
            burst.k_per_expansion = 0;
            burst.max_burst_len_frames = 0;
            burst.max_guest_instructions_per_job = 0;
        },
        |effective| {
            assert_eq!(effective.burst.k_per_expansion, 16);
            assert_eq!(effective.burst.max_burst_len_frames, 600);
            assert_eq!(effective.burst.max_guest_instructions_per_job, 0);
        },
    );
    assert_accepts(
        "deterministic scheduling coerces max inflight",
        |config| {
            let scheduling = config.scheduling.as_mut().unwrap();
            scheduling.mode = wire::SchedMode::Deterministic as i32;
            scheduling.max_inflight_batches = 99;
        },
        |effective| assert_eq!(effective.scheduling.max_inflight_batches, 1),
    );
    assert_accepts(
        "scheduling zero knobs default over proto3 v1",
        |config| {
            let scheduling = config.scheduling.as_mut().unwrap();
            scheduling.mode = wire::SchedMode::Fast as i32;
            scheduling.max_inflight_batches = 0;
            scheduling.job_timeout_s = 0;
            scheduling.retry_max = 0;
            scheduling.retry_backoff_ms = 0;
            scheduling.hypervisor_endpoints.push(String::new());
        },
        |effective| {
            assert_eq!(effective.scheduling.max_inflight_batches, 2);
            assert_eq!(effective.scheduling.job_timeout_s, 120);
            assert_eq!(effective.scheduling.retry_max, 3);
            assert_eq!(effective.scheduling.retry_backoff_ms, 250);
        },
    );
    assert_accepts(
        "checkpoint zero knobs default over proto3 v1",
        |config| {
            config.checkpoint.as_mut().unwrap().every_commits = 0;
            config.checkpoint.as_mut().unwrap().every_seconds = 0;
        },
        |effective| {
            assert_eq!(effective.checkpoint.every_commits, 64);
            assert_eq!(effective.checkpoint.every_seconds, 30);
        },
    );
}

#[test]
fn decoded_features_validate_against_compiled_feature_map_and_default_subset() {
    let mut feature_map = grid_feature_map();
    feature_map.features[0].semantics = orch_core::compile::FeatureSemantics::new("room_id");
    feature_map.features[0].discretize = orch_core::compile::Discretize::None;
    feature_map.features[1].semantics = orch_core::compile::FeatureSemantics::new("position_x");
    feature_map.features[1].discretize = orch_core::compile::Discretize::Grid {
        x: "x".to_owned(),
        y: "y".to_owned(),
        room: "room".to_owned(),
        cell_w: 4,
        cell_h: 4,
    };
    feature_map.features[2].semantics = orch_core::compile::FeatureSemantics::new("position_y");
    feature_map.features[2].discretize = orch_core::compile::Discretize::None;

    let compiled = orch_core::compile::compile_feature_map(&feature_map, &region_layouts())
        .expect("feature map compiles");
    let mut config = grid_config(0x5EED);
    config.decoded_features.clear();

    let selected =
        materialize_decoded_features(&config, &compiled).expect("default subset materializes");
    assert_eq!(selected, ["room", "x", "y"]);

    config.decoded_features = vec!["x".to_owned()];
    let selected =
        materialize_decoded_features(&config, &compiled).expect("configured feature exists");
    assert_eq!(selected, ["x"]);

    config.decoded_features = vec!["does_not_exist".to_owned()];
    let error = materialize_decoded_features(&config, &compiled).expect_err("unknown feature");
    assert_eq!(
        error.to_string(),
        "decoded feature not in feature_map does_not_exist"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_and_yaml_paths_share_validator_detail() {
    let world = FakeWorld::new(orch_fakes::grid::GridWorld::three_room());
    let service = OrchestratorService::new(
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        SharedSink::new(world.observatory()),
        std::sync::Arc::new(|experiment_id: &str| {
            let mut sources = sources();
            sources.synth_config_yaml = format!(
                "version: 1\nkind: experiment_config\nexperiment_id: {experiment_id}\nmodel: pad\nbutton_alphabet: console16-12btn-v1\ngenerator_mix:\n  weighted_random: 1\n  macro: 0\n  mutation: 0\n  policy: 0\n"
            )
            .into_bytes();
            sources
        }),
        "orchestratord-test",
    );

    let mut wire_config = valid_wire_config();
    wire_config.burst.as_mut().unwrap().k_per_expansion = 257;
    let grpc_error = service
        .start_experiment(Request::new(wire::StartExperimentRequest {
            experiment_id: "bad-config".to_owned(),
            config: Some(wire_config),
            resume_if_exists: false,
            run_id: String::new(),
        }))
        .await
        .expect_err("invalid config");
    assert_eq!(grpc_error.code(), tonic::Code::InvalidArgument);

    let yaml = br#"
version: 1
seed: 24301
workload_image_ref: workload://grid
feature_map_ref: featmap://grid
scoring_program_ref: score://grid
synth_config_ref: synth://grid
burst:
  k_per_expansion: 257
"#;
    let yaml_config = wire_config_from_yaml(yaml).expect("yaml parses");
    let standalone_detail = config_validation_failed_detail(&validate_wire_config(&yaml_config));

    assert_eq!(grpc_error.message(), standalone_detail);
    assert_eq!(
        standalone_detail,
        "config validation failed: field out of range burst.k_per_expansion"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_rejects_unknown_decoded_feature_after_feature_map_compile() {
    let world = FakeWorld::new(orch_fakes::grid::GridWorld::three_room());
    let service = OrchestratorService::new(
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        SharedSink::new(world.observatory()),
        std::sync::Arc::new(|experiment_id: &str| {
            let mut sources = sources();
            sources.synth_config_yaml = format!(
                "version: 1\nkind: experiment_config\nexperiment_id: {experiment_id}\nmodel: pad\nbutton_alphabet: console16-12btn-v1\ngenerator_mix:\n  weighted_random: 1\n  macro: 0\n  mutation: 0\n  policy: 0\n"
            )
            .into_bytes();
            sources
        }),
        "orchestratord-test",
    );

    let mut config = valid_wire_config();
    config.decoded_features = vec!["does_not_exist".to_owned()];
    let error = service
        .start_experiment(Request::new(wire::StartExperimentRequest {
            experiment_id: "bad-decoded-feature".to_owned(),
            config: Some(config),
            resume_if_exists: false,
            run_id: String::new(),
        }))
        .await
        .expect_err("invalid decoded feature");

    assert_eq!(error.code(), tonic::Code::InvalidArgument);
    assert_eq!(
        error.message(),
        "config validation failed: decoded feature not in feature_map does_not_exist"
    );
}
