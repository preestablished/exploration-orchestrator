#![forbid(unsafe_code)]

//! Scheduler for the exploration orchestrator (plan M3).
//!
//! Drives worker jobs against the four service boundaries through async
//! ports ([`ports`]), composing the per-job lease lifecycle, the bounded
//! select->expand->commit pipeline, and the retry policy. Everything is
//! exercised against `orch-fakes` on a paused-clock tokio runtime; real
//! transport adapters arrive at M6 behind the same ports.

pub mod ports;
pub mod slots;
