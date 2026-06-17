#![forbid(unsafe_code)]

//! Transport-free client trait and DTO boundary for orchestrator-side service use.
//!
//! This crate intentionally owns only platform-independent request, response, and
//! error shapes. It must not contain tonic clients, async transports, or direct
//! dependencies on the real service crates.
//!
//! Owner-doc citation convention: each service module starts with an `Owner docs`
//! line naming the API/INTEGRATION sections that its future trait and DTO shapes
//! mirror. Service-specific DTOs should keep those citations near the type or
//! method they model.

use std::{error::Error, fmt};

/// Result alias shared by all transport-free orchestrator client traits.
pub type ClientResult<T> = Result<T, ClientError>;

/// Broad error classes that fake clients and later transport adapters can map into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientErrorKind {
    InvalidRequest,
    FailedPrecondition,
    NotFound,
    AlreadyExists,
    ResourceExhausted,
    Unavailable,
    Internal,
}

impl ClientErrorKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid request",
            Self::FailedPrecondition => "failed precondition",
            Self::NotFound => "not found",
            Self::AlreadyExists => "already exists",
            Self::ResourceExhausted => "resource exhausted",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        }
    }
}

/// Transport-independent client error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientError {
    kind: ClientErrorKind,
    message: String,
}

impl ClientError {
    #[must_use]
    pub fn new(kind: ClientErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> ClientErrorKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl Error for ClientError {}

pub mod hypervisor {
    //! Hypervisor worker client boundary.
    //!
    //! Owner docs: exploration-orchestrator `API.md` §2 and `INTEGRATION.md` §1.
    //! This module will mirror the slot lease, VM lifecycle, input injection, run,
    //! snapshot, worker-info, and slot-watch shapes from the owner API without
    //! exposing a bespoke orchestrator job API.
}

pub mod input_synth {
    //! Input synthesizer client boundary.
    //!
    //! Owner docs: exploration-orchestrator `API.md` §4 and `INTEGRATION.md` §1.
    //! This module will mirror macro-pack loading, health, burst proposal, macro
    //! mining, provenance, and degraded-mode shapes from the owner API.
}

pub mod scorer {
    //! State scorer client boundary.
    //!
    //! Owner docs: exploration-orchestrator `API.md` §5 and `INTEGRATION.md` §1.
    //! This module will mirror feature-map loading, scoring program loading,
    //! batch scoring, archive checkpoint/restore, replay, novelty, and decoded
    //! component shapes from the owner API.
}

pub mod snapshot_store {
    //! Snapshot-store client boundary.
    //!
    //! Owner docs: exploration-orchestrator `API.md` §3 and `INTEGRATION.md` §1.
    //! This module will mirror tree node, subtree prune, path/query, metadata CAS,
    //! checkpoint, WAL, and private node-attribute shapes from the owner API.
}

#[cfg(test)]
mod tests {
    use super::{ClientError, ClientErrorKind, ClientResult};

    #[test]
    fn error_exposes_kind_and_message() {
        let error = ClientError::new(ClientErrorKind::Unavailable, "hypervisor offline");

        assert_eq!(error.kind(), ClientErrorKind::Unavailable);
        assert_eq!(error.message(), "hypervisor offline");
        assert_eq!(error.to_string(), "unavailable: hypervisor offline");
    }

    #[test]
    fn result_alias_accepts_client_error() {
        fn validate(flag: bool) -> ClientResult<()> {
            if flag {
                Ok(())
            } else {
                Err(ClientError::new(
                    ClientErrorKind::InvalidRequest,
                    "missing experiment id",
                ))
            }
        }

        assert!(validate(true).is_ok());
        let error = validate(false).expect_err("invalid input should fail");
        assert_eq!(error.kind(), ClientErrorKind::InvalidRequest);
    }
}
