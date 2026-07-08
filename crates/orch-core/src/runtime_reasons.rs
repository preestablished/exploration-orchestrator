//! Stable runtime terminal reason prefixes.
//!
//! These are separate from config validation rejection strings. A runtime
//! reason is the prefix of a terminal experiment outcome's `failure_reason`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalStatus {
    Failed,
    BudgetExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeReason {
    pub prefix: &'static str,
    pub status: TerminalStatus,
}

pub const REASON_CAS_OWNERSHIP_LOST: &str = "checkpoint-cas-ownership-lost";
pub const REASON_ARCHIVE_SEQ_MISMATCH: &str = "scorer-archive-seq-mismatch";
pub const REASON_FINGERPRINT_MISMATCH: &str = "synth-fingerprint-mismatch";
pub const REASON_FRONTIER_EXHAUSTED: &str = "frontier-exhausted";
pub const REASON_JOB_RETRIES_EXHAUSTED: &str = "job-retries-exhausted";
pub const REASON_CLASS_MISMATCH: &str = "determinism-class-mismatch";

pub const CATALOG: &[RuntimeReason] = &[
    RuntimeReason {
        prefix: REASON_CAS_OWNERSHIP_LOST,
        status: TerminalStatus::Failed,
    },
    RuntimeReason {
        prefix: REASON_ARCHIVE_SEQ_MISMATCH,
        status: TerminalStatus::Failed,
    },
    RuntimeReason {
        prefix: REASON_FINGERPRINT_MISMATCH,
        status: TerminalStatus::Failed,
    },
    RuntimeReason {
        prefix: REASON_JOB_RETRIES_EXHAUSTED,
        status: TerminalStatus::Failed,
    },
    RuntimeReason {
        prefix: REASON_CLASS_MISMATCH,
        status: TerminalStatus::Failed,
    },
    RuntimeReason {
        prefix: REASON_FRONTIER_EXHAUSTED,
        status: TerminalStatus::BudgetExhausted,
    },
];

pub const FAILED_REASON_PREFIXES: &[&str] = &[
    REASON_CAS_OWNERSHIP_LOST,
    REASON_ARCHIVE_SEQ_MISMATCH,
    REASON_FINGERPRINT_MISMATCH,
    REASON_JOB_RETRIES_EXHAUSTED,
    REASON_CLASS_MISMATCH,
];
