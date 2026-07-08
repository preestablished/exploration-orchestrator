//! Recording observatory fake: an ordered envelope log with an ack counter
//! and optional deterministic fault injection.

use std::cell::Cell;

use orch_clients::{
    observatory::{EventEnvelope, EventSink},
    ClientResult,
};

use crate::fault::{
    FaultDecision, FaultInjector, FaultPlan, FaultRequest, FaultStats, FaultTarget,
};

/// In-memory observatory sink. Emits append to an ordered log; the ack
/// counter tracks the highest contiguous sequence accepted.
#[derive(Clone, Debug)]
pub struct FakeObservatory {
    events: Vec<EventEnvelope>,
    acked_seq: u64,
    fault_injector: FaultInjector,
    last_fault: Cell<Option<FaultDecision>>,
}

impl Default for FakeObservatory {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeObservatory {
    #[must_use]
    pub fn new() -> Self {
        Self::with_fault_plan(FaultPlan::disabled(0))
    }

    #[must_use]
    pub fn with_fault_plan(fault_plan: FaultPlan) -> Self {
        Self {
            events: Vec::new(),
            acked_seq: 0,
            fault_injector: FaultInjector::new(fault_plan),
            last_fault: Cell::new(None),
        }
    }

    #[must_use]
    pub fn last_fault(&self) -> Option<FaultDecision> {
        self.last_fault.get()
    }

    #[must_use]
    pub fn fault_stats(&self) -> FaultStats {
        self.fault_injector.stats()
    }

    /// Ordered log of every accepted envelope.
    #[must_use]
    pub fn events(&self) -> &[EventEnvelope] {
        &self.events
    }

    /// Accepted envelopes of one event type, in order.
    #[must_use]
    pub fn events_of_type(&self, event_type: &str) -> Vec<&EventEnvelope> {
        self.events
            .iter()
            .filter(|event| event.event_type == event_type)
            .collect()
    }

    /// Clears the accepted-envelope inspection log while preserving the ack
    /// counter. Long fake soaks use this to avoid retaining telemetry that
    /// has already been acknowledged.
    pub fn clear_events(&mut self) -> usize {
        let count = self.events.len();
        self.events.clear();
        count
    }
}

impl EventSink for FakeObservatory {
    fn emit(&mut self, envelope: EventEnvelope) -> ClientResult<()> {
        let decision = self.fault_injector.decide(
            FaultRequest::new(
                FaultTarget::Observatory,
                "emit",
                envelope.event_type.as_bytes(),
            ),
            0,
        );
        self.last_fault.set(Some(decision));
        if let Some(error) = decision.client_error() {
            return Err(error);
        }
        self.acked_seq = self.acked_seq.max(envelope.seq);
        self.events.push(envelope);
        Ok(())
    }

    fn acked_seq(&self) -> ClientResult<u64> {
        let decision = self.fault_injector.decide(
            FaultRequest::new(FaultTarget::Observatory, "acked_seq", b""),
            0,
        );
        self.last_fault.set(Some(decision));
        if let Some(error) = decision.client_error() {
            return Err(error);
        }
        Ok(self.acked_seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fault::FaultRate;
    use orch_clients::{
        observatory::{Payload, PayloadValue},
        ClientErrorKind,
    };

    fn envelope(seq: u64, event_type: &str) -> EventEnvelope {
        let mut payload = Payload::new();
        payload.insert("seq".to_owned(), PayloadValue::U64(seq));
        EventEnvelope {
            run_id: "exp-a".to_owned(),
            source_service: "orchestratord".to_owned(),
            producer_id: "orchestratord-test".to_owned(),
            seq,
            ts_logical: seq,
            event_type: event_type.to_owned(),
            payload,
        }
    }

    #[test]
    fn observatory_records_ordered_events_and_acks() {
        let mut sink = FakeObservatory::new();

        sink.emit(envelope(1, "node-added")).expect("emit");
        sink.emit(envelope(2, "batch-completed")).expect("emit");
        sink.emit(envelope(3, "node-added")).expect("emit");

        assert_eq!(sink.events().len(), 3);
        assert_eq!(sink.events_of_type("node-added").len(), 2);
        assert_eq!(sink.acked_seq().expect("acked"), 3);
    }

    #[test]
    fn observatory_faults_reject_emits_without_recording() {
        let mut sink = FakeObservatory::with_fault_plan(
            FaultPlan::disabled(9).with_error(FaultRate::always(), ClientErrorKind::Unavailable),
        );

        let error = sink.emit(envelope(1, "node-added")).expect_err("fault");

        assert_eq!(error.kind(), ClientErrorKind::Unavailable);
        assert!(sink.events().is_empty());
        assert_eq!(
            sink.acked_seq().expect_err("fault").kind(),
            ClientErrorKind::Unavailable
        );
    }
}
