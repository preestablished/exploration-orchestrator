//! M3 accept bar: slot utilization > 95% in fast mode under fault-plan
//! latency jitter (±50%) on Run/TakeSnapshot, measured as the busy-slot
//! integral over virtual time. Includes the sensitivity control: a
//! deliberately serialized configuration must *fail* the bar, proving the
//! metric can fail.

mod support;

use orch_core::types::{NodeId, SchedMode};
use orch_fakes::fault::{FaultPlan, LatencyFault};
use orch_sched::{
    pipeline::{Batch, Pipeline, PipelineConfig},
    retry::RetryPolicy,
};
use std::time::Duration;
use support::{
    action_bursts, bootstrap_spec, harness, pad_burst, HarnessSpec, PlanProbe, BUTTON_RIGHT,
};

const BATCHES: u64 = 24;

/// Latency 50..150 virtual ms (±50% jitter around 100) on the two
/// heavyweight worker ops.
fn jitter_plan() -> FaultPlan {
    FaultPlan::disabled(0x1A7E).with_latency(LatencyFault::new(50, 100))
}

fn retry() -> RetryPolicy {
    RetryPolicy {
        job_timeout: Duration::from_secs(120),
        retry_max: 3,
        backoff_base: Duration::from_millis(50),
    }
}

async fn run_load(slots: u32, jobs_per_batch: usize, max_inflight: u32) -> f64 {
    let harness = harness(HarnessSpec {
        slots,
        hypervisor_probe: Some(PlanProbe::hypervisor(
            jitter_plan(),
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
            max_inflight_batches: max_inflight,
            retry: retry(),
        },
        0,
    );

    // Measure from the moment work exists.
    let baseline = harness.slots.utilization();

    let submitter = pipeline.submitter();
    let producer = tokio::spawn(async move {
        for seq in 0..BATCHES {
            let bursts = if jobs_per_batch == 6 {
                action_bursts()
            } else {
                (0..jobs_per_batch)
                    .map(|slot| pad_burst(slot as u32, BUTTON_RIGHT, 1))
                    .collect()
            };
            submitter
                .submit(Batch {
                    seq,
                    parent: NodeId::ROOT,
                    parent_snapshot: root.snapshot,
                    required_class: None,
                    bursts,
                })
                .await
                .expect("submit");
        }
    });
    pipeline.close();

    let mut completed = 0u64;
    while let Some(_result) = pipeline.next_completed().await.expect("batch") {
        completed += 1;
    }
    assert_eq!(completed, BATCHES);
    producer.await.expect("producer");

    let total = harness.slots.utilization();
    let busy = total.busy - baseline.busy;
    let capacity = total.capacity - baseline.capacity;
    harness.drain.abort();
    busy.as_secs_f64() / capacity.as_secs_f64()
}

#[tokio::test(start_paused = true)]
async fn slot_utilization_exceeds_95_percent_under_latency_jitter() {
    // 8 slots, 8 jobs per batch, 3 batches in flight: the pool stays
    // saturated while work exists.
    let utilization = run_load(8, 8, 3).await;
    assert!(
        utilization > 0.95,
        "busy-slot integral {utilization:.4} must exceed 0.95"
    );
}

#[tokio::test(start_paused = true)]
async fn sensitivity_control_serialized_dispatch_fails_the_bar() {
    // Degraded config: one single-job batch in flight at a time on the same
    // 8-slot pool. If the metric could not fail, this would also pass.
    let utilization = run_load(8, 1, 1).await;
    assert!(
        utilization < 0.95,
        "serialized dispatch reads {utilization:.4}; the metric must be able to fail"
    );
}
