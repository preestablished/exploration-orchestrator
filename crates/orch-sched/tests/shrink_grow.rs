//! M3 accept bar: the slot pool shrinks to 1 and regrows to 8 mid-run
//! without deadlock, and the run makes progress throughout (asserted with a
//! virtual-time timeout plus tokio-test polling on the lease path).

mod support;

use orch_core::types::{NodeId, SchedMode};
use orch_fakes::fault::{FaultPlan, LatencyFault};
use orch_sched::{
    pipeline::{Batch, JobOutcome, Pipeline, PipelineConfig},
    retry::RetryPolicy,
};
use std::time::Duration;
use support::{action_bursts, bootstrap_spec, harness, HarnessSpec, PlanProbe};

const BATCHES: u64 = 10;

#[tokio::test(start_paused = true)]
async fn pool_shrinks_to_one_and_regrows_without_deadlock() {
    let harness = harness(HarnessSpec {
        slots: 8,
        hypervisor_probe: Some(PlanProbe::hypervisor(
            FaultPlan::disabled(3).with_latency(LatencyFault::new(20, 20)),
            &["run", "take_snapshot"],
        )),
        ..HarnessSpec::default()
    })
    .await;
    let root = harness
        .driver
        .bootstrap(&bootstrap_spec())
        .await
        .expect("bootstrap");

    let mut pipeline = Pipeline::spawn(
        harness.driver.clone(),
        PipelineConfig {
            mode: SchedMode::Fast,
            max_inflight_batches: 2,
            retry: RetryPolicy {
                job_timeout: Duration::from_secs(120),
                retry_max: 3,
                backoff_base: Duration::from_millis(10),
            },
        },
        0,
    );

    // Script: shrink to 1 slot shortly after the run starts, grow back to 8
    // later; both surface through worker_info during WatchSlots drains.
    let controller = harness.hypervisor.clone();
    let script = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        controller.service().lock().await.set_slots_total(1);
        tokio::time::sleep(Duration::from_millis(600)).await;
        controller.service().lock().await.set_slots_total(8);
    });

    let submitter = pipeline.submitter();
    let snapshot = root.snapshot;
    let producer = tokio::spawn(async move {
        for seq in 0..BATCHES {
            submitter
                .submit(Batch {
                    seq,
                    parent: NodeId::ROOT,
                    parent_snapshot: snapshot,
                    required_class: None,
                    bursts: action_bursts(),
                })
                .await
                .expect("submit");
        }
    });
    pipeline.close();

    // Progress bar: every batch completes within a bounded virtual-time
    // budget. A lease-path deadlock would leave only timers pending, the
    // clock would auto-advance to the deadline, and this would fail loudly.
    let mut completed = 0u64;
    let all = tokio::time::timeout(Duration::from_secs(600), async {
        while let Some(result) = pipeline.next_completed().await.expect("batch") {
            for job in &result.jobs {
                assert!(matches!(job, JobOutcome::Completed(_)), "{job:?}");
            }
            completed += 1;
        }
    })
    .await;
    all.expect("pipeline must not deadlock across shrink/grow");
    assert_eq!(completed, BATCHES);
    script.await.expect("script");
    producer.await.expect("producer");

    // The view converged back to the full pool.
    let snapshot = harness.slots.snapshot();
    assert_eq!(snapshot.capacity, 8);
    assert_eq!(snapshot.reserved, 0);
    harness.drain.abort();
}

/// tokio-test on the lease path: with the pool shrunk to a single occupied
/// slot an acquire stays pending, and regrowing the pool wakes it — polled
/// explicitly, no timers involved.
#[tokio::test(start_paused = true)]
async fn acquire_is_pending_at_shrunken_capacity_and_wakes_on_grow() {
    let harness = harness(HarnessSpec {
        slots: 2,
        ..HarnessSpec::default()
    })
    .await;

    let first = harness.slots.acquire(None).await.expect("first slot");
    harness.hypervisor.service().lock().await.set_slots_total(1);
    tokio::time::sleep(Duration::from_millis(20)).await; // drain refresh

    let view = harness.slots.clone();
    let mut waiter = tokio_test::task::spawn(async move { view.acquire(None).await });
    assert!(waiter.poll().is_pending(), "no free slot at capacity 1");

    harness.hypervisor.service().lock().await.set_slots_total(4);
    tokio::time::sleep(Duration::from_millis(20)).await; // drain refresh

    match waiter.poll() {
        std::task::Poll::Ready(result) => {
            let permit = result.expect("grow admits the waiter");
            drop(permit);
        }
        std::task::Poll::Pending => panic!("waiter must wake after the pool grows"),
    }
    drop(first);
    harness.drain.abort();
}
