//! Retry policy (ARCHITECTURE.md §6.4), config-driven from
//! `SchedulingConfig`.
//!
//! Driver jobs: per-attempt timeout, best-effort `DestroyVm`, exponential
//! backoff (base `retry_backoff_ms`, x2 per attempt, 10 s cap), full re-run
//! on any free class-compatible slot, up to `retry_max` attempts. After
//! `retry_max`: FAST abandons the job (batch continues, failure counted and
//! journaled); DETERMINISTIC aborts the experiment with a fixed grep-able
//! reason. The retry license is the composition's purity: entropy seeds
//! derive from `(batch_seq, job_idx)`, so a re-run is bytewise identical.
//!
//! Synth/store/scorer RPCs use the same backoff via [`retry_rpc`];
//! `ScoreBatch`'s license is `client_batch_id` dedup replay, `CreateNode`
//! blind retry is safe (idempotent), and `ProposeBursts` retries go through
//! the existing fingerprint guard.

use std::{future::Future, time::Duration};

use orch_clients::{ClientError, ClientErrorKind, ClientResult};
use orch_core::types::SchedulingConfig;

use crate::{
    driver::{JobResult, JobSpec, WorkerDriver},
    ports::AsyncHypervisor,
};

/// Grep-able FAILED reason when a deterministic run exhausts job retries.
pub const DETERMINISTIC_RETRIES_EXHAUSTED_REASON: &str = "job-retries-exhausted";

/// Ceiling on exponential backoff.
pub const BACKOFF_CAP: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    /// Per-attempt job timeout (virtual time under paused-clock tests).
    pub job_timeout: Duration,
    pub retry_max: u32,
    pub backoff_base: Duration,
}

impl RetryPolicy {
    #[must_use]
    pub fn from_scheduling(config: &SchedulingConfig) -> Self {
        Self {
            job_timeout: Duration::from_secs(u64::from(config.job_timeout_s)),
            retry_max: config.retry_max,
            backoff_base: Duration::from_millis(u64::from(config.retry_backoff_ms)),
        }
    }

    /// `backoff_base * 2^attempt`, capped at [`BACKOFF_CAP`].
    #[must_use]
    pub fn backoff(&self, attempt: u32) -> Duration {
        let factor = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
        self.backoff_base
            .checked_mul(factor)
            .unwrap_or(BACKOFF_CAP)
            .min(BACKOFF_CAP)
    }
}

/// Errors that warrant a fresh attempt: transient transport/service states
/// and determinism violations that a clean slot can clear. Invalid requests,
/// missing resources, and precondition refusals (e.g. class mismatch) are
/// terminal.
#[must_use]
pub fn is_retryable(error: &ClientError) -> bool {
    matches!(
        error.kind(),
        ClientErrorKind::Unavailable
            | ClientErrorKind::Internal
            | ClientErrorKind::DataLoss
            | ClientErrorKind::ResourceExhausted
    )
}

/// Runs one job with the full retry ladder. Every attempt (including the
/// first) is bounded by `policy.job_timeout`, applied inside the driver so
/// a timed-out attempt tears its lease down best-effort; retries take the
/// Restore path on any free class-compatible slot.
pub async fn run_job_with_retry<H>(
    driver: &WorkerDriver<H>,
    policy: &RetryPolicy,
    spec: &JobSpec,
) -> ClientResult<JobResult>
where
    H: AsyncHypervisor + Clone + 'static,
{
    let mut attempt: u32 = 0;
    loop {
        let outcome = driver
            .run_job_deadline(spec, Some(policy.job_timeout))
            .await;
        match outcome {
            Ok(result) => return Ok(result),
            Err(error) if !is_retryable(&error) => return Err(error),
            Err(error) if attempt >= policy.retry_max => return Err(error),
            Err(_) => {
                tokio::time::sleep(policy.backoff(attempt)).await;
                attempt += 1;
            }
        }
    }
}

/// Same exponential backoff for non-job RPCs (synth/store/scorer). The
/// closure re-issues the identical request each attempt; idempotency comes
/// from the callee's contract (batch-id dedup, idempotent CreateNode,
/// generation re-read for PutMetadata).
pub async fn retry_rpc<T, F, Fut>(policy: &RetryPolicy, mut call: F) -> ClientResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ClientResult<T>>,
{
    let mut attempt: u32 = 0;
    loop {
        match call().await {
            Ok(value) => return Ok(value),
            Err(error) if !is_retryable(&error) => return Err(error),
            Err(error) if attempt >= policy.retry_max => return Err(error),
            Err(_) => {
                tokio::time::sleep(policy.backoff(attempt)).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };

    #[test]
    fn backoff_doubles_from_base_and_caps_at_ten_seconds() {
        let policy = RetryPolicy {
            job_timeout: Duration::from_secs(120),
            retry_max: 8,
            backoff_base: Duration::from_millis(250),
        };

        assert_eq!(policy.backoff(0), Duration::from_millis(250));
        assert_eq!(policy.backoff(1), Duration::from_millis(500));
        assert_eq!(policy.backoff(2), Duration::from_secs(1));
        assert_eq!(policy.backoff(5), Duration::from_secs(8));
        assert_eq!(policy.backoff(6), BACKOFF_CAP);
        assert_eq!(policy.backoff(31), BACKOFF_CAP);
        assert_eq!(policy.backoff(u32::MAX), BACKOFF_CAP);
    }

    #[test]
    fn retryable_classification_matches_the_policy_table() {
        for kind in [
            ClientErrorKind::Unavailable,
            ClientErrorKind::Internal,
            ClientErrorKind::DataLoss,
            ClientErrorKind::ResourceExhausted,
        ] {
            assert!(is_retryable(&ClientError::new(kind, "")), "{kind:?}");
        }
        for kind in [
            ClientErrorKind::InvalidRequest,
            ClientErrorKind::FailedPrecondition,
            ClientErrorKind::NotFound,
            ClientErrorKind::AlreadyExists,
        ] {
            assert!(!is_retryable(&ClientError::new(kind, "")), "{kind:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retry_rpc_backs_off_then_succeeds_within_budget() {
        let policy = RetryPolicy {
            job_timeout: Duration::from_secs(120),
            retry_max: 3,
            backoff_base: Duration::from_millis(100),
        };
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&attempts);

        let started = tokio::time::Instant::now();
        let value = retry_rpc(&policy, move || {
            let counter = Arc::clone(&counter);
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err(ClientError::new(ClientErrorKind::Unavailable, "flaky"))
                } else {
                    Ok(42)
                }
            }
        })
        .await
        .expect("third attempt succeeds");

        assert_eq!(value, 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        // 100ms + 200ms of backoff elapsed in virtual time.
        assert_eq!(started.elapsed(), Duration::from_millis(300));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_rpc_surfaces_terminal_and_exhausted_errors() {
        let policy = RetryPolicy {
            job_timeout: Duration::from_secs(120),
            retry_max: 2,
            backoff_base: Duration::from_millis(10),
        };

        let terminal = retry_rpc::<u32, _, _>(&policy, || async {
            Err(ClientError::new(ClientErrorKind::InvalidRequest, "bad"))
        })
        .await
        .expect_err("terminal error is not retried");
        assert_eq!(terminal.kind(), ClientErrorKind::InvalidRequest);

        let attempts = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&attempts);
        let exhausted = retry_rpc::<u32, _, _>(&policy, move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err(ClientError::new(ClientErrorKind::Unavailable, "down"))
            }
        })
        .await
        .expect_err("budget exhausts");
        assert_eq!(exhausted.kind(), ClientErrorKind::Unavailable);
        assert_eq!(attempts.load(Ordering::SeqCst), 3); // initial + 2 retries
    }
}
