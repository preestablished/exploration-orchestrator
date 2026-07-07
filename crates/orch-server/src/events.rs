//! Observatory event emitter: the producer-side rules from API.md §6.
//!
//! Events flow through a bounded local ring in front of the [`EventSink`]:
//! emit never blocks the loop; on sink outage the ring drops oldest and
//! counts (`orch_observatory_dropped_total`). `ts_logical` is the expansion
//! index; `seq` restarts per process session; `producer_id` is
//! wall-clock-derived in production and injected deterministically by test
//! harnesses.

use std::collections::VecDeque;

use orch_clients::observatory::{EventEnvelope, EventSink, Payload, PayloadValue};
use orch_core::types::{FiniteF64, NodeId};

pub const SOURCE_SERVICE: &str = "EXPLORATION_ORCHESTRATOR";
pub const DEFAULT_RING_CAPACITY: usize = 1024;

/// Prune reasons in the v1 vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PruneReason {
    Duplicate,
    Regression,
    Exhausted,
    FrontierEvict,
    StageGate,
}

impl PruneReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Duplicate => "duplicate",
            Self::Regression => "regression",
            Self::Exhausted => "exhausted",
            Self::FrontierEvict => "frontier-evict",
            Self::StageGate => "stage-gate",
        }
    }
}

/// Bounded drop-oldest emitter in front of an [`EventSink`].
pub struct EventEmitter<S> {
    sink: S,
    run_id: String,
    producer_id: String,
    next_seq: u64,
    ring: VecDeque<EventEnvelope>,
    capacity: usize,
    dropped_total: u64,
}

impl<S: EventSink> EventEmitter<S> {
    pub fn new(sink: S, run_id: impl Into<String>, producer_id: impl Into<String>) -> Self {
        Self {
            sink,
            run_id: run_id.into(),
            producer_id: producer_id.into(),
            next_seq: 1,
            ring: VecDeque::new(),
            capacity: DEFAULT_RING_CAPACITY,
            dropped_total: 0,
        }
    }

    #[must_use]
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity.max(1);
        self
    }

    #[must_use]
    pub fn sink(&self) -> &S {
        &self.sink
    }

    #[cfg(test)]
    fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    #[must_use]
    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
    }

    #[must_use]
    pub fn pending(&self) -> usize {
        self.ring.len()
    }

    /// Enqueues one event and opportunistically flushes the ring. Never
    /// blocks and never fails: on sink outage events wait in the ring and
    /// the oldest are dropped once it is full.
    pub fn emit(&mut self, ts_logical: u64, event_type: &str, payload: Payload) {
        let envelope = EventEnvelope {
            run_id: self.run_id.clone(),
            source_service: SOURCE_SERVICE.to_owned(),
            producer_id: self.producer_id.clone(),
            seq: self.next_seq,
            ts_logical,
            event_type: event_type.to_owned(),
            payload,
        };
        self.next_seq += 1;
        // Give a recovered sink the chance to drain before dropping.
        self.flush();
        if self.ring.len() == self.capacity {
            self.ring.pop_front();
            self.dropped_total += 1;
        }
        self.ring.push_back(envelope);
        self.flush();
    }

    /// Drains as much of the ring as the sink will take right now.
    pub fn flush(&mut self) {
        while let Some(envelope) = self.ring.front() {
            match self.sink.emit(envelope.clone()) {
                Ok(()) => {
                    self.ring.pop_front();
                }
                Err(_) => break,
            }
        }
    }
}

/// Clonable, thread-safe wrapper for a sink shared between the service and
/// an inspector (tests) or across service clones (the binary).
pub struct SharedSink<S>(pub std::sync::Arc<std::sync::Mutex<S>>);

impl<S> SharedSink<S> {
    pub fn new(sink: S) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(sink)))
    }
}

impl<S> Clone for SharedSink<S> {
    fn clone(&self) -> Self {
        Self(std::sync::Arc::clone(&self.0))
    }
}

impl<S: EventSink> EventSink for SharedSink<S> {
    fn emit(&mut self, envelope: EventEnvelope) -> orch_clients::ClientResult<()> {
        // Tolerate poisoning: a panicked emitter elsewhere must not take
        // the telemetry path down with it (review suggestion).
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .emit(envelope)
    }

    fn acked_seq(&self) -> orch_clients::ClientResult<u64> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .acked_seq()
    }
}

// ── payload builders (API.md §6 catalog field shapes) ──────────────────────

fn node_id_value(node: NodeId) -> PayloadValue {
    // Node ids render as decimal strings in payloads.
    PayloadValue::Text(node.get().to_string())
}

fn finite(value: f64) -> PayloadValue {
    match FiniteF64::new(value) {
        Ok(finite) => PayloadValue::F64(finite),
        Err(_) => PayloadValue::Null,
    }
}

#[must_use]
pub fn node_added_payload(
    node: NodeId,
    parent: NodeId,
    score: f64,
    novelty: f64,
    cell_key: u64,
    stage: u32,
    features: &[(String, f64)],
) -> Payload {
    let mut payload = Payload::new();
    payload.insert("node_id".to_owned(), node_id_value(node));
    payload.insert("parent_node_id".to_owned(), node_id_value(parent));
    payload.insert("score".to_owned(), finite(score));
    payload.insert("novelty".to_owned(), finite(novelty));
    payload.insert("cell_key".to_owned(), PayloadValue::U64(cell_key));
    payload.insert("stage".to_owned(), PayloadValue::U64(u64::from(stage)));
    let mut map = std::collections::BTreeMap::new();
    for (name, value) in features {
        if let Ok(finite_value) = FiniteF64::new(*value) {
            map.insert(name.clone(), PayloadValue::F64(finite_value));
        }
    }
    payload.insert("features".to_owned(), PayloadValue::Map(map));
    payload
}

#[must_use]
pub fn node_pruned_payload(parent: NodeId, reason: PruneReason, node: Option<NodeId>) -> Payload {
    let mut payload = Payload::new();
    payload.insert("parent_node_id".to_owned(), node_id_value(parent));
    payload.insert(
        "reason".to_owned(),
        PayloadValue::Text(reason.as_str().to_owned()),
    );
    if let Some(node) = node {
        payload.insert("node_id".to_owned(), node_id_value(node));
    }
    payload
}

#[must_use]
pub fn best_score_improved_payload(node: NodeId, best_score: f64, previous: f64) -> Payload {
    let mut payload = Payload::new();
    payload.insert("node_id".to_owned(), node_id_value(node));
    payload.insert("best_score".to_owned(), finite(best_score));
    payload.insert("previous_best_score".to_owned(), finite(previous));
    payload
}

#[must_use]
pub fn stall_detected_payload(expansions_since_improvement: u64, window: u32) -> Payload {
    let mut payload = Payload::new();
    payload.insert(
        "expansions_since_improvement".to_owned(),
        PayloadValue::U64(expansions_since_improvement),
    );
    payload.insert("window".to_owned(), PayloadValue::U64(u64::from(window)));
    payload
}

#[must_use]
pub fn escalation_changed_payload(level: u32, previous_level: u32) -> Payload {
    let mut payload = Payload::new();
    payload.insert("level".to_owned(), PayloadValue::U64(u64::from(level)));
    payload.insert(
        "previous_level".to_owned(),
        PayloadValue::U64(u64::from(previous_level)),
    );
    payload
}

#[must_use]
pub fn goal_reached_payload(node: NodeId, score: f64) -> Payload {
    let mut payload = Payload::new();
    payload.insert("node_id".to_owned(), node_id_value(node));
    payload.insert("score".to_owned(), finite(score));
    payload
}

#[must_use]
pub fn batch_completed_payload(
    seq: u64,
    parent: NodeId,
    committed: u64,
    discarded: u64,
) -> Payload {
    let mut payload = Payload::new();
    payload.insert("batch_seq".to_owned(), PayloadValue::U64(seq));
    payload.insert("parent_node_id".to_owned(), node_id_value(parent));
    payload.insert("committed".to_owned(), PayloadValue::U64(committed));
    payload.insert("discarded".to_owned(), PayloadValue::U64(discarded));
    payload
}

#[must_use]
pub fn checkpoint_payload(batch_seq: u64, expansions: u64, archive_seq: u64) -> Payload {
    let mut payload = Payload::new();
    payload.insert("batch_seq".to_owned(), PayloadValue::U64(batch_seq));
    payload.insert("expansions".to_owned(), PayloadValue::U64(expansions));
    payload.insert("archive_seq".to_owned(), PayloadValue::U64(archive_seq));
    payload
}

/// Guest-sdk relay payloads (`assertion-violated` / `reachability-hit`),
/// relayed post-commit with node context when the worker surfaces them.
#[must_use]
pub fn sdk_event_payload(node: NodeId, stream: u32, payload_bytes: &[u8]) -> Payload {
    let mut payload = Payload::new();
    payload.insert("node_id".to_owned(), node_id_value(node));
    payload.insert("stream".to_owned(), PayloadValue::U64(u64::from(stream)));
    payload.insert(
        "payload".to_owned(),
        PayloadValue::Bytes(payload_bytes.to_vec()),
    );
    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_clients::{ClientError, ClientErrorKind, ClientResult};

    #[derive(Default)]
    struct FlakySink {
        accept: bool,
        accepted: Vec<EventEnvelope>,
    }

    impl EventSink for FlakySink {
        fn emit(&mut self, envelope: EventEnvelope) -> ClientResult<()> {
            if self.accept {
                self.accepted.push(envelope);
                Ok(())
            } else {
                Err(ClientError::new(ClientErrorKind::Unavailable, "down"))
            }
        }

        fn acked_seq(&self) -> ClientResult<u64> {
            Ok(self.accepted.last().map_or(0, |event| event.seq))
        }
    }

    #[test]
    fn emitter_never_blocks_and_drops_oldest_on_outage() {
        let mut emitter =
            EventEmitter::new(FlakySink::default(), "exp-a", "orchestratord-test").with_capacity(3);

        for expansion in 0..5 {
            emitter.emit(expansion, "batch-completed", Payload::new());
        }
        assert_eq!(emitter.pending(), 3);
        assert_eq!(emitter.dropped_total(), 2);

        // Outage clears: the ring drains in order, seq gaps mark the drops.
        // (Direct field poke: the sink is owned by the emitter.)
        emitter.sink_mut().accept = true;
        emitter.emit(5, "batch-completed", Payload::new());
        assert_eq!(emitter.pending(), 0);
        let seqs: Vec<u64> = emitter
            .sink()
            .accepted
            .iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(seqs, vec![3, 4, 5, 6]);
        assert_eq!(emitter.sink().acked_seq().expect("acked"), 6);
    }
}
