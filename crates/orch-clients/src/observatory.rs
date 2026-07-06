//! Transport-free observatory event boundary.
//!
//! Owner docs: observatory's event-envelope contract (API.md §6 lists the
//! orchestrator-side v1 event vocabulary and producer rules). The trait
//! models the wire: one envelope per emit, at-least-once, ordered per
//! producer. The *bounded ring, drop-oldest, never-blocks* semantics are
//! producer rules and live in the orchestrator's emitter (orch-server),
//! not here.
//!
//! Payload schemas are observatory's contract; on fakes the orchestrator
//! emits the v1 vocabulary with the documented field shapes as
//! serde-serialized maps ([`Payload`]), which also gives the deterministic
//! canonical bytes the seed-gate event hash is computed over.

use std::collections::BTreeMap;

use orch_core::types::FiniteF64;
use serde::{Deserialize, Serialize};

use crate::ClientResult;

/// One structured payload value. Floats are finite by construction so the
/// canonical encoding (and therefore the event-sequence hash) is total.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayloadValue {
    Null,
    Bool(bool),
    U64(u64),
    I64(i64),
    F64(FiniteF64),
    Text(String),
    Bytes(Vec<u8>),
    List(Vec<PayloadValue>),
    Map(BTreeMap<String, PayloadValue>),
}

/// Deterministically ordered event payload.
pub type Payload = BTreeMap<String, PayloadValue>;

/// Observatory event envelope (observatory's envelope contract).
///
/// `producer_id` is wall-clock-derived in production
/// (`orchestratord-<startup_unix>`) and `seq` restarts per session — both
/// nondeterministic run-to-run by design. The seed-gate determinism hash is
/// therefore defined over `(ts_logical, event_type, payload)` only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub run_id: String,
    pub source_service: String,
    pub producer_id: String,
    pub seq: u64,
    /// Logical timestamp: the orchestrator's commit counter, not wall time.
    pub ts_logical: u64,
    pub event_type: String,
    pub payload: Payload,
}

/// Sync observatory sink boundary, matching the style of the other four
/// service boundaries. Implementations acknowledge the highest contiguous
/// sequence they have durably accepted.
pub trait EventSink {
    fn emit(&mut self, envelope: EventEnvelope) -> ClientResult<()>;

    /// Highest acknowledged producer sequence (0 = nothing acked). Resume
    /// re-sends events from `acked_seq + 1`.
    fn acked_seq(&self) -> ClientResult<u64>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(seq: u64) -> EventEnvelope {
        let mut payload = Payload::new();
        payload.insert("node_id".to_owned(), PayloadValue::U64(7));
        payload.insert(
            "score".to_owned(),
            PayloadValue::F64(FiniteF64::new(12.5).expect("finite")),
        );
        EventEnvelope {
            run_id: "exp-a".to_owned(),
            source_service: "orchestratord".to_owned(),
            producer_id: "orchestratord-test".to_owned(),
            seq,
            ts_logical: seq,
            event_type: "node-added".to_owned(),
            payload,
        }
    }

    #[test]
    fn envelope_payload_has_canonical_deterministic_bytes() {
        let first = postcard::to_allocvec(&envelope(3)).expect("encode");
        let second = postcard::to_allocvec(&envelope(3)).expect("encode");

        assert_eq!(first, second);
        let decoded: EventEnvelope = postcard::from_bytes(&first).expect("decode");
        assert_eq!(decoded, envelope(3));
    }

    #[test]
    fn payload_map_ordering_is_stable_regardless_of_insert_order() {
        let mut forward = Payload::new();
        forward.insert("a".to_owned(), PayloadValue::U64(1));
        forward.insert("b".to_owned(), PayloadValue::U64(2));
        let mut reverse = Payload::new();
        reverse.insert("b".to_owned(), PayloadValue::U64(2));
        reverse.insert("a".to_owned(), PayloadValue::U64(1));

        assert_eq!(
            postcard::to_allocvec(&forward).expect("encode"),
            postcard::to_allocvec(&reverse).expect("encode"),
        );
    }
}
