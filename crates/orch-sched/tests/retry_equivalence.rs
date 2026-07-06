//! M3 accept bar: retries are invisible under purity. A deterministic-mode
//! small-world search through the full pipeline with a 5% injected
//! terminal-error rate (worker and scorer paths) must produce the identical
//! final tree hash and commit-order transcript as the 0%-failure run.
//! Depends on the fault-injector attempt salt (W3.0a): without it,
//! identity-invariant requests would re-draw the same fault forever.

mod support;

use orch_clients::ClientErrorKind;
use orch_fakes::fault::{FaultPlan, FaultRate};
use orch_sched::retry::RetryPolicy;
use std::time::Duration;
use support::{harness, run_search, HarnessSpec};

const SEED: u64 = 0x0DD5_EED5;
const MAX_EXPANSIONS: u64 = 4_096;

fn retry() -> RetryPolicy {
    RetryPolicy {
        job_timeout: Duration::from_secs(120),
        retry_max: 8,
        backoff_base: Duration::from_millis(10),
    }
}

async fn search_with_error_rate(per_million: u32) -> (u64, [u8; 32], usize) {
    let rate = FaultRate::per_million(per_million).expect("rate");
    let harness = harness(HarnessSpec {
        slots: 8,
        experiment_seed: SEED,
        hypervisor_plan: FaultPlan::disabled(0xFA17).with_error(rate, ClientErrorKind::Unavailable),
        scorer_plan: FaultPlan::disabled(0xFA18).with_error(rate, ClientErrorKind::Unavailable),
        ..HarnessSpec::default()
    })
    .await;

    let outcome = run_search(&harness, SEED, retry(), MAX_EXPANSIONS)
        .await
        .expect("search completes");
    harness.drain.abort();
    (
        outcome.expansions,
        outcome.tree_hash,
        outcome.transcript.len(),
    )
}

#[tokio::test(start_paused = true)]
async fn five_percent_injected_errors_leave_the_search_bit_identical() {
    let clean = search_with_error_rate(0).await;
    let faulty = search_with_error_rate(50_000).await;

    assert_eq!(
        clean.1, faulty.1,
        "final tree hash must match the 0%-failure run"
    );
    assert_eq!(
        clean.0, faulty.0,
        "expansion count (commit transcript length) must match"
    );
    assert_eq!(clean.2, faulty.2);
}

#[tokio::test(start_paused = true)]
async fn different_seeds_still_differ_under_the_same_fault_plan() {
    // Sanity: the equivalence above is not because everything collapses to
    // one outcome.
    let harness_a = harness(HarnessSpec {
        experiment_seed: SEED,
        ..HarnessSpec::default()
    })
    .await;
    let harness_b = harness(HarnessSpec {
        experiment_seed: SEED + 1,
        ..HarnessSpec::default()
    })
    .await;

    let a = run_search(&harness_a, SEED, retry(), MAX_EXPANSIONS)
        .await
        .expect("search a");
    let b = run_search(&harness_b, SEED + 1, retry(), MAX_EXPANSIONS)
        .await
        .expect("search b");
    harness_a.drain.abort();
    harness_b.drain.abort();

    assert_ne!(a.tree_hash, b.tree_hash);
}
