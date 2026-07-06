//! Runner smoke: a fresh run on the boss+credits world reaches the goal
//! autonomously with the full loop engaged (bootstrap, synth bursts, WAL,
//! commit, events, checkpoints).

mod support;

use orch_checkpoint::ExperimentState;
use orch_fakes::grid::GridWorld;
use orch_server::experiment::{ExperimentRunner, StartMode};
use support::{runner_config, sources, FakeWorld};

#[tokio::test(start_paused = true)]
async fn fresh_run_reaches_credits_and_checkpoints() {
    let world = FakeWorld::new(GridWorld::three_room());
    let sink = world.observatory();

    let (runner, _handle, mode) = ExperimentRunner::start(
        runner_config(0x5EED),
        sources(),
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        sink,
        None,
    )
    .await
    .expect("runner starts");
    assert!(matches!(mode, StartMode::Fresh));

    let outcome = runner.run().await.expect("run completes");

    assert_eq!(
        outcome.state,
        ExperimentState::GoalReached,
        "run must reach credits: {outcome:?}"
    );
    assert!(!outcome.goal_nodes.is_empty());
    assert!(outcome.expansions > 0);
    assert!(outcome.nodes > 1);
}
