//! Plain atomic gauges behind a small struct; M4's server wires them into
//! the Prometheus surface. Metric names are fixed now (M5 forward-design).

use std::sync::atomic::{AtomicU64, Ordering};

/// `orch_pipeline_queue_depth{stage="submit"}` — batches queued S->E.
pub const QUEUE_DEPTH_SUBMIT: &str = "orch_pipeline_queue_depth{stage=\"submit\"}";
/// `orch_pipeline_queue_depth{stage="complete"}` — results queued E->C.
pub const QUEUE_DEPTH_COMPLETE: &str = "orch_pipeline_queue_depth{stage=\"complete\"}";
/// `orch_slot_utilization` — busy fraction of the slot pool.
pub const SLOT_UTILIZATION: &str = "orch_slot_utilization";
/// `orch_jobs_failed_total` — jobs abandoned after retry exhaustion.
pub const JOBS_FAILED_TOTAL: &str = "orch_jobs_failed_total";
/// `orch_batch_latency_seconds{stage="execute"}` — E-stage batch latency.
pub const BATCH_LATENCY_EXECUTE: &str = "orch_batch_latency_seconds{stage=\"execute\"}";

/// Shared pipeline gauges. All plain atomics: safe to read from a metrics
/// endpoint while the pipeline runs.
#[derive(Debug, Default)]
pub struct Gauges {
    /// Current depth of the submit (S->E) queue.
    pub queue_depth_submit: AtomicU64,
    /// Peak depth ever observed on the submit queue.
    pub queue_depth_submit_peak: AtomicU64,
    /// Current depth of the completed (E->C) queue.
    pub queue_depth_complete: AtomicU64,
    /// Peak depth ever observed on the completed queue.
    pub queue_depth_complete_peak: AtomicU64,
    /// Jobs abandoned after retry exhaustion (fast mode).
    pub jobs_failed_total: AtomicU64,
    /// Sum of E-stage batch latencies in virtual milliseconds.
    pub batch_latency_ms_sum: AtomicU64,
    /// Count of completed batches contributing to the latency sum.
    pub batch_latency_count: AtomicU64,
}

impl Gauges {
    pub(crate) fn enqueue(depth: &AtomicU64, peak: &AtomicU64) {
        let now = depth.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(now, Ordering::SeqCst);
    }

    pub(crate) fn dequeue(depth: &AtomicU64) {
        // Saturating: a decrement racing a failed enqueue must not wrap to
        // u64::MAX (review finding).
        let _ = depth.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            Some(value.saturating_sub(1))
        });
    }
}
