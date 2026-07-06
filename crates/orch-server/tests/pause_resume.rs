//! M4 accept bar: Pause -> checkpointed_batch_seq -> drop the runner ->
//! new process-equivalent runner -> Resume; status and stats are preserved
//! across the restart.

mod support;

use orch_checkpoint::ExperimentState;
use orch_core::types::OnGoal;
use orch_fakes::grid::GridWorld;
use orch_server::experiment::{Control, ExperimentRunner, RunnerConfig, StartMode};
use support::{config_hash, grid_config, sources, FakeWorld, EXPERIMENT_ID};

/// A run that never self-terminates quickly: goals continue, huge budget —
/// so Pause can land at a deterministic point.
fn long_runner_config(seed: u64) -> RunnerConfig {
    let mut config = grid_config(seed);
    config.on_goal = OnGoal::Continue;
    config.budgets.max_expansions = 1_000_000;
    config.validate().expect("valid");
    let hash = config_hash(&config);
    RunnerConfig {
        experiment_id: EXPERIMENT_ID.to_owned(),
        run_id: EXPERIMENT_ID.to_owned(),
        producer_id: "orchestratord-test".to_owned(),
        config,
        config_hash: hash,
    }
}

#[tokio::test(start_paused = true)]
async fn pause_survives_a_process_swap_and_resume_finishes_the_run() {
    let world = FakeWorld::new(GridWorld::three_room());

    // First incarnation: run, then pause mid-flight.
    let (runner, handle, mode) = ExperimentRunner::start(
        long_runner_config(0x5EED),
        sources(),
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        world.observatory(),
        None,
    )
    .await
    .expect("first runner starts");
    assert!(matches!(mode, StartMode::Fresh));
    let first = tokio::spawn(runner.run());

    // Let some expansions land, then pause (watch notifications, not
    // timers: under the paused clock timer wakeups starve while the runner
    // has ready work).
    let mut watch = handle.watch();
    while watch.borrow_and_update().expansions < 3 {
        watch.changed().await.expect("runner publishes status");
    }
    handle.send(Control::Pause).expect("pause");
    while watch.borrow_and_update().state != ExperimentState::Paused {
        watch.changed().await.expect("runner publishes status");
    }
    let paused = handle.status();
    assert_eq!(paused.state, ExperimentState::Paused);
    assert!(paused.checkpointed_batch_seq > 0, "pause checkpoints");
    assert!(paused.expansions >= 3);

    // Simulated process death while parked: abort the task. The fakes (the
    // durable world) survive.
    first.abort();
    let _ = first.await;

    // Second incarnation resumes from the durable PAUSED checkpoint.
    let (runner, handle, mode) = ExperimentRunner::start(
        long_runner_config(0x5EED),
        sources(),
        world.hypervisor.clone(),
        world.scorer.clone(),
        world.store.clone(),
        world.synth.clone(),
        world.observatory(),
        None,
    )
    .await
    .expect("second runner resumes");
    match mode {
        StartMode::Resumed {
            checkpoint_batch_seq,
        } => assert_eq!(checkpoint_batch_seq, paused.checkpointed_batch_seq),
        StartMode::Fresh => panic!("second incarnation must resume"),
    }
    let resumed = handle.status();
    assert_eq!(resumed.state, ExperimentState::Paused, "durably PAUSED");
    assert!(
        resumed.expansions >= paused.expansions,
        "stats preserved: {} >= {}",
        resumed.expansions,
        paused.expansions
    );
    assert_eq!(resumed.best_score, paused.best_score);

    let second = tokio::spawn(runner.run());
    // Still parked until Resume.
    tokio::task::yield_now().await;
    assert!(!second.is_finished());
    assert_eq!(handle.status().state, ExperimentState::Paused);

    handle.send(Control::Resume).expect("resume");
    // Let the resumed run make progress, then stop it.
    let mut watch = handle.watch();
    loop {
        let snapshot = watch.borrow_and_update().clone();
        if snapshot.state == ExperimentState::Running && snapshot.expansions > paused.expansions {
            break;
        }
        watch.changed().await.expect("runner publishes status");
    }
    handle.send(Control::Stop).expect("stop");
    let outcome = second
        .await
        .expect("second runner task")
        .expect("second run completes");
    assert_eq!(outcome.state, ExperimentState::Stopped, "{outcome:?}");
    assert!(outcome.expansions > paused.expansions, "stats advanced");
}
