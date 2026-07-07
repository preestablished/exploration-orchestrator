//! M3 accept bar: backpressure with flat memory. A C stage whose scorer is
//! far slower than jobs must not let queues grow: the bounded submit and
//! completed queues cap at their configured bounds and stay there, and the
//! only in-flight structures are those bounds (asserted via peak gauges —
//! "memory flat" is structural on fakes; RSS belongs to M5's soak).

mod support;

use orch_clients::scorer::{ArchiveUpdateMode, ScoreBatchRequest, StateInput};
use orch_core::types::{NodeId, SchedMode};
use orch_fakes::fault::{FaultPlan, LatencyFault};
use orch_sched::{
    pipeline::{Batch, JobOutcome, Pipeline, PipelineConfig},
    ports::AsyncScorer,
    retry::RetryPolicy,
};
use std::{sync::atomic::Ordering, time::Duration};
use support::{action_bursts, bootstrap_spec, harness, HarnessSpec, PlanProbe, EXPERIMENT_ID};

const BATCHES: u64 = 12;
const MAX_INFLIGHT: u32 = 2;

#[tokio::test(start_paused = true)]
async fn queues_cap_at_configured_bounds_under_a_slow_scorer() {
    // Job latency ~10 virtual ms; scorer latency ~1000 virtual ms.
    let harness = harness(HarnessSpec {
        hypervisor_probe: Some(PlanProbe::hypervisor(
            FaultPlan::disabled(7).with_latency(LatencyFault::new(10, 0)),
            &["run", "take_snapshot"],
        )),
        scorer_probe: Some(PlanProbe::scorer(
            FaultPlan::disabled(8).with_latency(LatencyFault::new(1_000, 0)),
            &["score_batch"],
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
            max_inflight_batches: MAX_INFLIGHT,
            retry: RetryPolicy {
                job_timeout: Duration::from_secs(120),
                retry_max: 3,
                backoff_base: Duration::from_millis(50),
            },
        },
        0,
    );
    let gauges = pipeline.gauges();

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

    // Slow C stage: every completed batch goes through the slow scorer
    // before the next result is taken. The whole drain is bounded in
    // virtual time so a hang-class deadlock fails loudly instead of
    // hanging the suite (review suggestion; cf. shrink_grow.rs).
    let mut completed = 0u64;
    let drain = async {
        while let Some(result) = pipeline.next_completed().await.expect("batch") {
            let states: Vec<StateInput> = result
                .jobs
                .iter()
                .map(|job| match job {
                    JobOutcome::Completed(job) => StateInput {
                        node_ref: format!("b{}-j{}", result.seq, job.job_idx),
                        feature_bytes: job
                            .capture
                            .as_ref()
                            .expect("capture")
                            .feature_bytes
                            .clone()
                            .expect("features"),
                        framebuffer: None,
                        fb_meta: None,
                    },
                    JobOutcome::Abandoned { job_idx, reason } => {
                        panic!("job {job_idx} abandoned: {reason}")
                    }
                })
                .collect();
            harness
                .scorer
                .score_batch(ScoreBatchRequest {
                    experiment_id: EXPERIMENT_ID.to_owned(),
                    states,
                    archive_update: ArchiveUpdateMode::ScoreOnly,
                    client_batch_id: format!("b{}", result.seq),
                    return_decoded: false,
                })
                .await
                .expect("score");
            completed += 1;
        }
    };
    tokio::time::timeout(Duration::from_secs(600), drain)
        .await
        .expect("backpressure drain must not deadlock");
    assert_eq!(completed, BATCHES);
    producer.await.expect("producer");

    // Bounded structures only: the submit queue holds at most its cap plus
    // the one producer blocked in send; the completed queue likewise. If
    // anything buffered unboundedly ahead of the slow scorer, these peaks
    // would scale with BATCHES (12) instead of the caps (1).
    // Bounds: channel capacity plus the senders that can block on it —
    // one producer for the submit queue, max_inflight batch tasks for the
    // completed queue. Both are configuration constants, independent of
    // BATCHES.
    let submit_bound = u64::from(MAX_INFLIGHT - 1) + 1;
    let complete_bound = 1 + u64::from(MAX_INFLIGHT);
    let submit_peak = gauges.queue_depth_submit_peak.load(Ordering::SeqCst);
    let complete_peak = gauges.queue_depth_complete_peak.load(Ordering::SeqCst);
    assert!(
        submit_peak <= submit_bound,
        "submit queue peak {submit_peak} exceeds bound {submit_bound}"
    );
    assert!(
        complete_peak <= complete_bound,
        "completed queue peak {complete_peak} exceeds bound {complete_bound}"
    );
    // The queues did actually fill (backpressure engaged, not just idle).
    assert!(submit_peak >= submit_bound);
    assert!(complete_peak >= complete_bound);
    // And nothing is left queued.
    assert_eq!(gauges.queue_depth_submit.load(Ordering::SeqCst), 0);
    assert_eq!(gauges.queue_depth_complete.load(Ordering::SeqCst), 0);
    harness.drain.abort();
}
