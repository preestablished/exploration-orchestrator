//! Prometheus metric catalog, registry, and text renderer.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
    time::Duration,
};

use orch_sched::metrics::Gauges;

pub const ORCH_EXPANSIONS_TOTAL: &str = "orch_expansions_total";
pub const ORCH_NODES_TOTAL: &str = "orch_nodes_total";
pub const ORCH_BEST_SCORE: &str = "orch_best_score";
pub const ORCH_FRONTIER_SIZE: &str = "orch_frontier_size";
pub const ORCH_ARCHIVE_CELLS: &str = "orch_archive_cells";
pub const ORCH_ESCALATION_LEVEL: &str = "orch_escalation_level";
pub const ORCH_SLOT_UTILIZATION: &str = "orch_slot_utilization";
pub const ORCH_PIPELINE_QUEUE_DEPTH: &str = "orch_pipeline_queue_depth";
pub const ORCH_JOBS_FAILED_TOTAL: &str = "orch_jobs_failed_total";
pub const ORCH_BATCH_LATENCY_SECONDS: &str = "orch_batch_latency_seconds";
pub const ORCH_OBSERVATORY_DROPPED_TOTAL: &str = "orch_observatory_dropped_total";

pub const REQUIRED_METRIC_FAMILIES: &[&str] = &[
    ORCH_EXPANSIONS_TOTAL,
    ORCH_NODES_TOTAL,
    ORCH_BEST_SCORE,
    ORCH_FRONTIER_SIZE,
    ORCH_ARCHIVE_CELLS,
    ORCH_ESCALATION_LEVEL,
    ORCH_SLOT_UTILIZATION,
    ORCH_PIPELINE_QUEUE_DEPTH,
    ORCH_JOBS_FAILED_TOTAL,
    ORCH_BATCH_LATENCY_SECONDS,
    ORCH_OBSERVATORY_DROPPED_TOTAL,
];

pub const NODE_VERDICTS: &[&str] = &["kept", "dup", "regression"];
pub const QUEUE_STAGES: &[&str] = &["submit", "complete"];
pub const BATCH_LATENCY_STAGES: &[BatchLatencyStage] = &[
    BatchLatencyStage::Select,
    BatchLatencyStage::Execute,
    BatchLatencyStage::Commit,
];

const HISTOGRAM_BUCKETS: &[f64] = &[0.001, 0.01, 0.1, 1.0, 10.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BatchLatencyStage {
    Select,
    Execute,
    Commit,
}

impl BatchLatencyStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Execute => "execute",
            Self::Commit => "commit",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistogramSnapshot {
    pub sum_seconds: f64,
    pub count: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetricsSnapshot {
    pub expansions_total: u64,
    pub nodes_kept: u64,
    pub nodes_dup: u64,
    pub nodes_regression: u64,
    pub best_score: f64,
    pub frontier_size: u64,
    pub archive_cells: u64,
    pub escalation_level: u32,
    pub slot_utilization: f64,
    pub pipeline_queue_depth_submit: u64,
    pub pipeline_queue_depth_complete: u64,
    pub jobs_failed_total: u64,
    pub batch_latency: BTreeMap<BatchLatencyStage, HistogramSnapshot>,
    pub observatory_dropped_total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetricsStatus {
    pub expansions_total: u64,
    pub nodes_kept: u64,
    pub nodes_dup: u64,
    pub nodes_regression: u64,
    pub best_score: f64,
    pub frontier_size: u64,
    pub archive_cells: u64,
    pub escalation_level: u32,
    pub slot_utilization: f64,
}

#[derive(Debug, Default)]
pub struct MetricsRegistry {
    snapshot: Mutex<MetricsSnapshot>,
}

impl MetricsRegistry {
    pub fn update_status(&self, status: MetricsStatus) {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        snapshot.expansions_total = status.expansions_total;
        snapshot.nodes_kept = status.nodes_kept;
        snapshot.nodes_dup = status.nodes_dup;
        snapshot.nodes_regression = status.nodes_regression;
        snapshot.best_score = status.best_score;
        snapshot.frontier_size = status.frontier_size;
        snapshot.archive_cells = status.archive_cells;
        snapshot.escalation_level = status.escalation_level;
        snapshot.slot_utilization = status.slot_utilization;
    }

    pub fn update_pipeline_gauges(&self, gauges: &Gauges) {
        use std::sync::atomic::Ordering;

        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        snapshot.pipeline_queue_depth_submit = gauges.queue_depth_submit.load(Ordering::SeqCst);
        snapshot.pipeline_queue_depth_complete = gauges.queue_depth_complete.load(Ordering::SeqCst);
        snapshot.jobs_failed_total = gauges.jobs_failed_total.load(Ordering::SeqCst);
        let execute = snapshot
            .batch_latency
            .entry(BatchLatencyStage::Execute)
            .or_default();
        execute.count = gauges.batch_latency_count.load(Ordering::SeqCst);
        execute.sum_seconds = gauges.batch_latency_ms_sum.load(Ordering::SeqCst) as f64 / 1000.0;
    }

    pub fn observe_batch_latency(&self, stage: BatchLatencyStage, duration: Duration) {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let histogram = snapshot.batch_latency.entry(stage).or_default();
        histogram.count = histogram.count.saturating_add(1);
        histogram.sum_seconds += duration.as_secs_f64();
    }

    pub fn set_observatory_dropped_total(&self, dropped_total: u64) {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .observatory_dropped_total = dropped_total;
    }

    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

pub fn render_prometheus(snapshot: &MetricsSnapshot) -> String {
    let mut output = String::new();
    gauge(
        &mut output,
        ORCH_EXPANSIONS_TOTAL,
        "Total completed expansions.",
        snapshot.expansions_total,
        "counter",
    );
    family_header(
        &mut output,
        ORCH_NODES_TOTAL,
        "Committed or discarded nodes by verdict.",
        "counter",
    );
    sample(
        &mut output,
        ORCH_NODES_TOTAL,
        &[("verdict", "kept")],
        snapshot.nodes_kept,
    );
    sample(
        &mut output,
        ORCH_NODES_TOTAL,
        &[("verdict", "dup")],
        snapshot.nodes_dup,
    );
    sample(
        &mut output,
        ORCH_NODES_TOTAL,
        &[("verdict", "regression")],
        snapshot.nodes_regression,
    );
    gauge(
        &mut output,
        ORCH_BEST_SCORE,
        "Best progress score observed.",
        snapshot.best_score,
        "gauge",
    );
    gauge(
        &mut output,
        ORCH_FRONTIER_SIZE,
        "Current frontier size.",
        snapshot.frontier_size,
        "gauge",
    );
    gauge(
        &mut output,
        ORCH_ARCHIVE_CELLS,
        "Current scorer archive cell count.",
        snapshot.archive_cells,
        "gauge",
    );
    gauge(
        &mut output,
        ORCH_ESCALATION_LEVEL,
        "Current plateau escalation level.",
        snapshot.escalation_level,
        "gauge",
    );
    gauge(
        &mut output,
        ORCH_SLOT_UTILIZATION,
        "Busy-slot utilization fraction.",
        snapshot.slot_utilization,
        "gauge",
    );
    family_header(
        &mut output,
        ORCH_PIPELINE_QUEUE_DEPTH,
        "Current pipeline queue depth by stage.",
        "gauge",
    );
    sample(
        &mut output,
        ORCH_PIPELINE_QUEUE_DEPTH,
        &[("stage", "submit")],
        snapshot.pipeline_queue_depth_submit,
    );
    sample(
        &mut output,
        ORCH_PIPELINE_QUEUE_DEPTH,
        &[("stage", "complete")],
        snapshot.pipeline_queue_depth_complete,
    );
    gauge(
        &mut output,
        ORCH_JOBS_FAILED_TOTAL,
        "Jobs abandoned after retry exhaustion.",
        snapshot.jobs_failed_total,
        "counter",
    );
    render_histograms(&mut output, snapshot);
    gauge(
        &mut output,
        ORCH_OBSERVATORY_DROPPED_TOTAL,
        "Observatory events dropped by the local emitter ring.",
        snapshot.observatory_dropped_total,
        "counter",
    );
    output
}

#[must_use]
pub fn rendered_families(text: &str) -> BTreeSet<String> {
    parse_samples(text).keys().cloned().collect()
}

#[must_use]
pub fn rendered_label_values(text: &str, family: &str, label: &str) -> BTreeSet<String> {
    parse_samples(text)
        .remove(family)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|labels| labels.get(label).cloned())
        .collect()
}

fn render_histograms(output: &mut String, snapshot: &MetricsSnapshot) {
    family_header(
        output,
        ORCH_BATCH_LATENCY_SECONDS,
        "Batch latency by pipeline stage.",
        "histogram",
    );
    for stage in BATCH_LATENCY_STAGES {
        let histogram = snapshot
            .batch_latency
            .get(stage)
            .cloned()
            .unwrap_or_default();
        for bucket in HISTOGRAM_BUCKETS {
            sample_name(
                output,
                &format!("{ORCH_BATCH_LATENCY_SECONDS}_bucket"),
                &[("stage", stage.as_str()), ("le", &bucket.to_string())],
                0u64,
            );
        }
        sample_name(
            output,
            &format!("{ORCH_BATCH_LATENCY_SECONDS}_bucket"),
            &[("stage", stage.as_str()), ("le", "+Inf")],
            histogram.count,
        );
        sample_name(
            output,
            &format!("{ORCH_BATCH_LATENCY_SECONDS}_sum"),
            &[("stage", stage.as_str())],
            histogram.sum_seconds,
        );
        sample_name(
            output,
            &format!("{ORCH_BATCH_LATENCY_SECONDS}_count"),
            &[("stage", stage.as_str())],
            histogram.count,
        );
    }
}

fn gauge(output: &mut String, name: &str, help: &str, value: impl MetricValue, kind: &str) {
    family_header(output, name, help, kind);
    sample(output, name, &[], value);
}

fn family_header(output: &mut String, name: &str, help: &str, kind: &str) {
    output.push_str("# HELP ");
    output.push_str(name);
    output.push(' ');
    output.push_str(help);
    output.push('\n');
    output.push_str("# TYPE ");
    output.push_str(name);
    output.push(' ');
    output.push_str(kind);
    output.push('\n');
}

fn sample(output: &mut String, family: &str, labels: &[(&str, &str)], value: impl MetricValue) {
    sample_name(output, family, labels, value);
}

fn sample_name(output: &mut String, name: &str, labels: &[(&str, &str)], value: impl MetricValue) {
    output.push_str(name);
    if !labels.is_empty() {
        output.push('{');
        for (index, (key, value)) in labels.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str(key);
            output.push_str("=\"");
            output.push_str(value);
            output.push('"');
        }
        output.push('}');
    }
    output.push(' ');
    output.push_str(&value.metric_value());
    output.push('\n');
}

trait MetricValue {
    fn metric_value(&self) -> String;
}

impl MetricValue for u64 {
    fn metric_value(&self) -> String {
        self.to_string()
    }
}

impl MetricValue for u32 {
    fn metric_value(&self) -> String {
        self.to_string()
    }
}

impl MetricValue for f64 {
    fn metric_value(&self) -> String {
        if self.is_finite() {
            format!("{self:.9}")
        } else {
            "0".to_owned()
        }
    }
}

fn parse_samples(text: &str) -> BTreeMap<String, BTreeSet<BTreeMap<String, String>>> {
    let mut parsed: BTreeMap<String, BTreeSet<BTreeMap<String, String>>> = BTreeMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((series, _value)) = line.split_once(' ') else {
            continue;
        };
        let (raw_name, labels) = match series.split_once('{') {
            Some((name, rest)) => {
                let labels = rest
                    .strip_suffix('}')
                    .unwrap_or(rest)
                    .split(',')
                    .filter_map(|pair| {
                        let (key, value) = pair.split_once('=')?;
                        Some((key.to_owned(), value.trim_matches('"').to_owned()))
                    })
                    .collect();
                (name, labels)
            }
            None => (series, BTreeMap::new()),
        };
        parsed
            .entry(normalize_family(raw_name).to_owned())
            .or_default()
            .insert(labels);
    }
    parsed
}

fn normalize_family(name: &str) -> &str {
    name.strip_suffix("_bucket")
        .or_else(|| name.strip_suffix("_sum"))
        .or_else(|| name.strip_suffix("_count"))
        .unwrap_or(name)
}
