//! M4 accept bar: the plateau ladder unsticks the corridor-hidden-key
//! world. With the ladder enabled the run escalates (observed L0 -> L1 ->
//! ... events) and reaches the stage gate / goal; the max_level = 0 control
//! run fails the same budget.

mod support;

use orch_checkpoint::ExperimentState;
use orch_fakes::grid::GridWorld;
use orch_server::experiment::{ExperimentRunner, RunOutcome, RunnerConfig};
use support::{config_hash, grid_config, sources, FakeWorld, SharedSink, EXPERIMENT_ID};

fn corridor_config(seed: u64, max_level: u32) -> RunnerConfig {
    let mut config = grid_config(seed);
    // Cold, greedy, novelty-blind selection: the score gradient pins the
    // search at the locked gate (the zero-score climb shaft can never win
    // the priority race), so only the ladder's escalations — hotter
    // temperature and the L3 backtrack bonus — can unstick it.
    config.selection.temperature = 0.3;
    config.selection.beta = 0.05;
    config.selection.gamma = 0.02;
    config.selection.max_visits_per_node = 1_000_000;
    config.selection.exhaust_after_dup_expansions = 1_000_000;
    // Measured solve points for this seed: ladder 222, control 324. The
    // shared budget sits at the midpoint so both outcomes carry >=15%
    // margin (review finding: no single-expansion cliff).
    config.budgets.max_expansions = 273;
    config.plateau.window_n = 12;
    config.plateau.ladder.backtrack_kappa = 5.0;
    config.plateau.ladder.temp_factor = 4.0;
    config.plateau.ladder.burst_len_factor = 2.0;
    config.plateau.ladder.max_level = max_level;
    config.validate().expect("valid corridor config");
    let hash = config_hash(&config);
    RunnerConfig {
        experiment_id: EXPERIMENT_ID.to_owned(),
        run_id: EXPERIMENT_ID.to_owned(),
        producer_id: "orchestratord-test".to_owned(),
        config,
        config_hash: hash,
    }
}

async fn run_corridor(seed: u64, max_level: u32) -> (RunOutcome, SharedSink) {
    let world = FakeWorld::new(GridWorld::corridor_hidden_key());
    let sink = SharedSink::default();
    let (runner, _handle, _mode) = ExperimentRunner::start(
        corridor_config(seed, max_level),
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
    (outcome, sink)
}

#[tokio::test(start_paused = true)]
async fn ladder_unsticks_the_corridor_and_the_control_fails_the_budget() {
    let seed = 0x001A_DDE4;

    // Both arms share the budget; the assertion is the *relative* gap, not
    // a co-tuned cliff (review finding): the ladder run must reach the goal
    // inside the budget the control exhausts, with clear headroom.
    let (control, _) = run_corridor(seed, 0).await;
    assert_ne!(
        control.state,
        ExperimentState::GoalReached,
        "max_level=0 control must fail the budget: {control:?}"
    );

    let (ladder, sink) = run_corridor(seed, 4).await;
    assert_eq!(
        ladder.state,
        ExperimentState::GoalReached,
        "ladder run must unstick: {ladder:?}"
    );
    // Headroom: the ladder solved with at least 15% of the shared budget
    // unspent, so the pass/fail gap is not a single-expansion cliff (the
    // control side carries the same margin by construction).
    let budget = corridor_config(seed, 4).config.budgets.max_expansions;
    assert!(
        ladder.expansions * 100 <= budget * 85,
        "ladder must clear the budget with headroom: {} of {budget}",
        ladder.expansions
    );

    // Escalation assertions (review finding: `contains(&1)` alone was
    // weaker than the claimed L0 -> L1 -> ... path). Levels reset to L0 on
    // improvement, so the raw sequence legitimately restarts; within a
    // stall episode each escalation climbs exactly one level.
    let levels: Vec<u64> = {
        let sink = sink.0.lock().expect("sink");
        sink.events_of_type("escalation-changed")
            .iter()
            .map(
                |event| match event.payload.get("level").expect("level field") {
                    orch_clients::observatory::PayloadValue::U64(level) => *level,
                    other => panic!("unexpected level payload: {other:?}"),
                },
            )
            .collect()
    };
    assert!(
        !levels.is_empty(),
        "escalation-changed events must be observed"
    );
    assert_eq!(levels[0], 1, "the ladder starts at L0 -> L1: {levels:?}");
    let max_level = levels.iter().copied().max().unwrap_or(0);
    assert!(
        max_level >= 2,
        "the run must escalate past L1 to unstick: {levels:?}"
    );
    for pair in levels.windows(2) {
        assert!(
            pair[1] == pair[0] + 1 || pair[1] <= pair[0],
            "escalations climb one level at a time (resets allowed): {levels:?}"
        );
    }
}
