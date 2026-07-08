//! M4 accept bar: "kill -9 anywhere" — Tier-1 in-process crash lattice
//! (plan D5). Every crash point in the loop is hit deterministically; after
//! each simulated SIGKILL the harness reclaims the dead runner's leases
//! (FakeHypervisor::reclaim_session, standing in for the worker observing
//! its connection drop), constructs a fresh runner against the surviving
//! fakes, and resumes per §8.2. Invariants: the run completes, no node id
//! is ever reused with different content (the store's idempotent CreateNode
//! rejects divergent re-creates), no FRONTIER row is stranded, no double
//! commits, and the deterministic-mode final tree hash equals the
//! uninterrupted run's.
//!
//! `CHAOS_SEED` overrides the seed set (the phases track's fresh-seed
//! spot-check); `CHAOS_SEEDS_PER_POINT` widens the lattice for the evidence
//! pass (default keeps CI runtime sane).

mod support;

use orch_checkpoint::ExperimentState;
use orch_fakes::{grid::GridWorld, snapshot_store::InMemorySnapshotStore};
use orch_server::experiment::{
    CrashPoint, CrashPolicy, ExperimentRunner, RunOutcome, CRASHED_MARKER,
};
use support::{runner_config, sources, FakeWorld, EXPERIMENT_ID};

/// Crashes the runner the `nth` time it reaches `point`; inert afterwards.
struct CrashOnce {
    point: CrashPoint,
    remaining: u32,
}

impl CrashPolicy for CrashOnce {
    fn should_crash(&mut self, point: CrashPoint) -> bool {
        if point != self.point || self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        self.remaining == 0
    }
}

// The comparator is shared with the Tier-2 harness (extracted to
// orch-simstate in W2.2; assertions identical).
fn store_tree_hash(store: &InMemorySnapshotStore) -> [u8; 32] {
    orch_simstate::compare::store_tree_hash(store, EXPERIMENT_ID)
}

fn assert_no_stranded_frontier(store: &InMemorySnapshotStore) {
    orch_simstate::compare::assert_no_stranded_frontier(store, EXPERIMENT_ID);
}

async fn uninterrupted_run(seed: u64) -> ([u8; 32], RunOutcome) {
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
    let outcome = runner.run().await.expect("uninterrupted run");
    assert_eq!(outcome.state, ExperimentState::GoalReached, "{outcome:?}");
    let store = world.store.service();
    let store = store.lock().await;
    (store_tree_hash(&store), outcome)
}

/// Runs one experiment to completion under repeated crashes at `point`,
/// resuming with a fresh runner each time against the surviving fakes.
async fn chaos_run(seed: u64, point: CrashPoint) -> ([u8; 32], RunOutcome, u32) {
    let world = FakeWorld::new(GridWorld::three_room());
    let mut crashes = 0u32;
    let mut incarnation = 0u32;
    loop {
        incarnation += 1;
        // First few incarnations crash at the target point (progressively
        // later occurrences); after that the run gets a clean finish so the
        // lattice always converges.
        let policy: Option<Box<dyn CrashPolicy>> = if crashes < 3 {
            Some(Box::new(CrashOnce {
                point,
                remaining: 1 + (crashes + seed as u32) % 3,
            }))
        } else {
            None
        };
        let started = ExperimentRunner::start(
            runner_config(seed),
            sources(),
            world.hypervisor.clone(),
            world.scorer.clone(),
            world.store.clone(),
            world.synth.clone(),
            world.observatory(),
            policy,
        )
        .await;
        let (runner, _handle, _mode) = match started {
            Ok(parts) => parts,
            // The bootstrap/initial checkpoint contains crash points too: a
            // SIGKILL during start() is just another crash incarnation.
            Err(error) if error.message().starts_with(CRASHED_MARKER) => {
                crashes += 1;
                world.hypervisor.service().lock().await.reclaim_session();
                continue;
            }
            Err(error) => panic!("non-crash start failure under chaos: {error}"),
        };
        match runner.run().await {
            Err(error) if error.message().starts_with(CRASHED_MARKER) => {
                crashes += 1;
                // The worker notices the dead session and reclaims leases.
                world.hypervisor.service().lock().await.reclaim_session();
            }
            Err(error) => panic!("non-crash failure under chaos: {error}"),
            Ok(outcome) => {
                assert_eq!(
                    outcome.state,
                    ExperimentState::GoalReached,
                    "chaos run must still reach the goal: {outcome:?}"
                );
                let store = world.store.service();
                let store = store.lock().await;
                assert_no_stranded_frontier(&store);
                return (store_tree_hash(&store), outcome, crashes);
            }
        }
        assert!(
            incarnation < 64,
            "chaos run failed to converge at {point:?} seed {seed}"
        );
    }
}

fn chaos_seeds() -> Vec<u64> {
    if let Ok(value) = std::env::var("CHAOS_SEED") {
        return vec![value.parse().expect("CHAOS_SEED must be a u64")];
    }
    let per_point: u64 = std::env::var("CHAOS_SEEDS_PER_POINT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2);
    (0..per_point).map(|index| 0x5EED + index * 7).collect()
}

#[tokio::test(start_paused = true)]
async fn crash_lattice_resumes_bit_identically_at_every_point() {
    let seeds = chaos_seeds();
    let mut references = std::collections::BTreeMap::new();
    for &seed in &seeds {
        references.insert(seed, uninterrupted_run(seed).await);
    }

    let mut total_crashes = 0u32;
    for point in CrashPoint::ALL {
        for &seed in &seeds {
            let (reference_hash, reference_outcome) = &references[&seed];
            let (hash, outcome, crashes) = chaos_run(seed, point).await;
            assert!(
                crashes > 0,
                "crash point {point:?} was never reached for seed {seed:#x} — lattice hole"
            );
            total_crashes += crashes;
            assert_eq!(
                &hash, reference_hash,
                "det-mode final tree diverged after crashes at {point:?} seed {seed:#x}"
            );
            assert_eq!(outcome.nodes, reference_outcome.nodes);
            assert_eq!(outcome.goal_nodes, reference_outcome.goal_nodes);
        }
    }
    assert!(total_crashes >= CrashPoint::ALL.len() as u32 * seeds.len() as u32);
}
