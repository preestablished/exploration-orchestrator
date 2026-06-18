#![forbid(unsafe_code)]

//! Transport-free client trait and DTO boundary for orchestrator-side service use.
//!
//! This crate intentionally owns only platform-independent request, response, and
//! error shapes. It must not contain tonic clients, async transports, or direct
//! dependencies on the real service crates.
//!
//! Owner-doc citation convention: each service module starts with an `Owner docs`
//! line naming the traceable API, integration, proto, or planning artifact that its
//! future trait and DTO shapes mirror. Service-specific DTOs should keep those
//! citations near the type or method they model.

use std::{error::Error, fmt};

/// Result alias shared by all transport-free orchestrator client traits.
pub type ClientResult<T> = Result<T, ClientError>;

/// Broad error classes that fake clients and later transport adapters can map into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClientErrorKind {
    /// Request fields are malformed, missing, or outside the accepted schema.
    InvalidRequest,
    /// Request is well-formed, but the target service state cannot accept it.
    FailedPrecondition,
    /// Referenced experiment, slot, snapshot, node, macro pack, or archive is absent.
    NotFound,
    /// Create-if-absent request collided with an existing resource.
    AlreadyExists,
    /// Service-side capacity, quota, queue, or slot pool is exhausted.
    ResourceExhausted,
    /// Service is temporarily unreachable or not ready to serve the request.
    Unavailable,
    /// Unexpected service or adapter failure that does not fit a stable category.
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
        if self.message.is_empty() {
            f.write_str(self.kind.as_str())
        } else {
            write!(f, "{}: {}", self.kind.as_str(), self.message)
        }
    }
}

impl Error for ClientError {}

pub mod hypervisor;

pub mod input_synth;

pub mod scorer;

pub mod snapshot_store;

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
    fn error_display_omits_empty_separator() {
        let error = ClientError::new(ClientErrorKind::Internal, "");

        assert_eq!(error.to_string(), "internal");
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
