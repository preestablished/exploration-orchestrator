//! M4 accept bar: default(-shaped) config, boss+credits world, 10 seeds —
//! 10/10 reach credits within budget with zero scripted inputs (every
//! burst comes from the synthesizer).

mod support;

use orch_checkpoint::ExperimentState;
use orch_fakes::grid::GridWorld;
use orch_server::experiment::ExperimentRunner;
use support::{runner_config, sources, FakeWorld};

#[tokio::test(start_paused = true)]
async fn ten_of_ten_seeds_reach_credits_within_budget() {
    for seed_index in 0..10u64 {
        let seed = 0xA0_0000 + seed_index * 101;
        let world = FakeWorld::new(GridWorld::three_room());
        let (runner, _handle, _mode) = ExperimentRunner::start(
            runner_config(seed),
            sources(),
            world.hypervisor.clone(),
            world.scorer.clone(),
            world.store.clone(),
            world.synth.clone(),
            world.observatory(),
            None,
        )
        .await
        .expect("runner starts");
        let outcome = runner.run().await.expect("run completes");
        assert_eq!(
            outcome.state,
            ExperimentState::GoalReached,
            "seed {seed:#x} must reach credits: {outcome:?}"
        );
        assert!(
            outcome.expansions <= runner_config(seed).config.budgets.max_expansions,
            "seed {seed:#x} within budget"
        );
    }
}
