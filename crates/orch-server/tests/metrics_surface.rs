use std::sync::Arc;

use orch_clients::{
    observatory::{EventEnvelope, EventSink},
    ClientError, ClientErrorKind, ClientResult,
};
use orch_server::{
    events::EventEmitter,
    metrics::{
        render_prometheus, rendered_families, rendered_label_values, MetricsRegistry,
        MetricsStatus, BATCH_LATENCY_STAGES, NODE_VERDICTS, ORCH_BATCH_LATENCY_SECONDS,
        ORCH_NODES_TOTAL, ORCH_OBSERVATORY_DROPPED_TOTAL, ORCH_PIPELINE_QUEUE_DEPTH, QUEUE_STAGES,
        REQUIRED_METRIC_FAMILIES,
    },
};

#[derive(Default)]
struct RejectingSink;

impl EventSink for RejectingSink {
    fn emit(&mut self, _envelope: EventEnvelope) -> ClientResult<()> {
        Err(ClientError::new(ClientErrorKind::Unavailable, "sink down"))
    }

    fn acked_seq(&self) -> ClientResult<u64> {
        Ok(0)
    }
}

#[test]
fn metrics_catalog_is_complete() {
    let registry = MetricsRegistry::default();
    let text = render_prometheus(&registry.snapshot());
    let families = rendered_families(&text);

    for family in REQUIRED_METRIC_FAMILIES {
        assert!(
            families.contains(*family),
            "missing metric family {family}; got {families:?}"
        );
    }
    assert_eq!(
        rendered_label_values(&text, ORCH_NODES_TOTAL, "verdict"),
        NODE_VERDICTS
            .iter()
            .map(|value| value.to_string())
            .collect()
    );
    assert_eq!(
        rendered_label_values(&text, ORCH_PIPELINE_QUEUE_DEPTH, "stage"),
        QUEUE_STAGES.iter().map(|value| value.to_string()).collect()
    );
    assert_eq!(
        rendered_label_values(&text, ORCH_BATCH_LATENCY_SECONDS, "stage"),
        BATCH_LATENCY_STAGES
            .iter()
            .map(|stage| stage.as_str().to_owned())
            .collect()
    );

    for stage in BATCH_LATENCY_STAGES {
        assert!(
            text.contains(&format!(
                "orch_batch_latency_seconds_bucket{{stage=\"{}\",le=\"+Inf\"}}",
                stage.as_str()
            )),
            "histogram bucket missing for stage {}",
            stage.as_str()
        );
        assert!(
            text.contains(&format!(
                "orch_batch_latency_seconds_sum{{stage=\"{}\"}}",
                stage.as_str()
            )),
            "histogram sum missing for stage {}",
            stage.as_str()
        );
        assert!(
            text.contains(&format!(
                "orch_batch_latency_seconds_count{{stage=\"{}\"}}",
                stage.as_str()
            )),
            "histogram count missing for stage {}",
            stage.as_str()
        );
    }
}

#[test]
fn live_status_values_are_exported() {
    let registry = MetricsRegistry::default();
    registry.update_status(MetricsStatus {
        expansions_total: 7,
        nodes_kept: 11,
        nodes_dup: 3,
        nodes_regression: 2,
        best_score: 42.5,
        frontier_size: 5,
        archive_cells: 4,
        escalation_level: 1,
        slot_utilization: 0.25,
    });

    let text = render_prometheus(&registry.snapshot());
    assert!(text.contains("orch_expansions_total 7"));
    assert!(text.contains("orch_nodes_total{verdict=\"kept\"} 11"));
    assert!(text.contains("orch_nodes_total{verdict=\"dup\"} 3"));
    assert!(text.contains("orch_nodes_total{verdict=\"regression\"} 2"));
    assert!(text.contains("orch_best_score 42.500000000"));
}

#[test]
fn observatory_drops_are_exported() {
    let registry = Arc::new(MetricsRegistry::default());
    let mut emitter = EventEmitter::new(RejectingSink, "run", "producer")
        .with_capacity(1)
        .with_metrics(Arc::clone(&registry));

    emitter.emit(0, "batch-completed", Default::default());
    emitter.emit(1, "batch-completed", Default::default());
    emitter.emit(2, "batch-completed", Default::default());

    let text = render_prometheus(&registry.snapshot());
    assert!(
        text.contains(&format!("{ORCH_OBSERVATORY_DROPPED_TOTAL} 2")),
        "drop counter not exported: {text}"
    );
}
