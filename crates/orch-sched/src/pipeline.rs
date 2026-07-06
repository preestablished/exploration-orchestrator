//! The S -> E -> C pipeline (ARCHITECTURE.md §6.2) with bounded queues.
//!
//! The S stage (select + synth) is the caller's: the pipeline receives
//! ready batches through [`Pipeline::submit`], executes them (K jobs fanned
//! out per batch, bounded by the `SlotView`), and hands results back
//! through [`Pipeline::next_completed`]. Every channel is a bounded tokio
//! mpsc; a full channel suspends the upstream stage (backpressure). No
//! `select!` anywhere; task spawn order is fixed (D2 determinism
//! footguns).
//!
//! Modes: FAST hands batches back in completion order and abandons jobs
//! whose retries exhaust (the batch continues, the gap is journaled in the
//! result); DETERMINISTIC hands back strictly in `seq` order with job
//! results sorted by job index, and a retry-exhausted job aborts the
//! experiment with a fixed grep-able reason (config validation already
//! coerces `max_inflight_batches` to 1 in that mode).

use std::{collections::BTreeMap, sync::Arc};

use orch_clients::{
    hypervisor::DeterminismClass, input_synth::ProvenancedBurst, ClientError, ClientErrorKind,
    ClientResult,
};
use orch_core::types::{NodeId, SchedMode, SnapshotRef};
use tokio::sync::{mpsc, Semaphore};

use crate::{
    driver::{JobResult, JobSpec, WorkerDriver},
    metrics::Gauges,
    ports::AsyncHypervisor,
    retry::{run_job_with_retry, RetryPolicy, DETERMINISTIC_RETRIES_EXHAUSTED_REASON},
};

/// One ready-to-execute expansion batch (the S stage's output).
#[derive(Clone, Debug)]
pub struct Batch {
    pub seq: u64,
    pub parent: NodeId,
    pub parent_snapshot: SnapshotRef,
    /// Parent node's determinism class requirement, if any.
    pub required_class: Option<DeterminismClass>,
    pub bursts: Vec<ProvenancedBurst>,
}

/// Outcome of one job within a batch.
#[derive(Clone, Debug)]
pub enum JobOutcome {
    Completed(Box<JobResult>),
    /// Fast mode only: retries exhausted; the batch continues without this
    /// child and the gap is journaled here.
    Abandoned {
        job_idx: u32,
        reason: String,
    },
}

#[derive(Clone, Debug)]
pub struct BatchResult {
    pub seq: u64,
    pub parent: NodeId,
    /// Sorted by job index in both modes.
    pub jobs: Vec<JobOutcome>,
}

#[derive(Clone, Debug)]
pub struct PipelineConfig {
    pub mode: SchedMode,
    pub max_inflight_batches: u32,
    pub retry: RetryPolicy,
}

/// Handle to a running pipeline. Dropping it closes the submit queue; the
/// executor drains in-flight batches and exits.
pub struct Pipeline {
    submit_tx: mpsc::Sender<Batch>,
    completed_rx: mpsc::Receiver<ClientResult<BatchResult>>,
    mode: SchedMode,
    /// DETERMINISTIC-mode hold-back buffer for strict seq order.
    reorder: BTreeMap<u64, ClientResult<BatchResult>>,
    next_seq: u64,
    gauges: Arc<Gauges>,
}

impl Pipeline {
    /// Spawns the executor over the given driver. `first_seq` seeds the
    /// deterministic-mode ordering cursor (resume starts past the
    /// checkpoint).
    pub fn spawn<H>(driver: WorkerDriver<H>, config: PipelineConfig, first_seq: u64) -> Self
    where
        H: AsyncHypervisor + Clone + 'static,
    {
        let max_inflight = config.max_inflight_batches.max(1) as usize;
        // Submit queue cap: max_inflight_batches - 1 (a queued batch beyond
        // the ones executing), floor 1 so submit->recv still flows.
        let submit_cap = max_inflight.saturating_sub(1).max(1);
        let (submit_tx, mut submit_rx) = mpsc::channel::<Batch>(submit_cap);
        let (completed_tx, completed_rx) = mpsc::channel::<ClientResult<BatchResult>>(1);
        let gauges = Arc::new(Gauges::default());

        let executor_gauges = Arc::clone(&gauges);
        let mode = config.mode;
        let retry = config.retry;
        tokio::spawn(async move {
            let inflight = Arc::new(Semaphore::new(max_inflight));
            while let Some(batch) = submit_rx.recv().await {
                Gauges::dequeue(&executor_gauges.queue_depth_submit);
                let permit = Arc::clone(&inflight)
                    .acquire_owned()
                    .await
                    .expect("in-flight semaphore closed");
                let driver = driver.clone();
                let completed_tx = completed_tx.clone();
                let gauges = Arc::clone(&executor_gauges);
                tokio::spawn(async move {
                    let started = tokio::time::Instant::now();
                    let result = execute_batch(&driver, &retry, mode, batch, &gauges).await;
                    let elapsed = started.elapsed();
                    gauges.batch_latency_ms_sum.fetch_add(
                        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                    gauges
                        .batch_latency_count
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Gauges::enqueue(
                        &gauges.queue_depth_complete,
                        &gauges.queue_depth_complete_peak,
                    );
                    // Receiver dropped => pipeline shut down; nothing to do.
                    let _ = completed_tx.send(result).await;
                    drop(permit);
                });
            }
        });

        Self {
            submit_tx,
            completed_rx,
            mode,
            reorder: BTreeMap::new(),
            next_seq: first_seq,
            gauges,
        }
    }

    #[must_use]
    pub fn gauges(&self) -> Arc<Gauges> {
        Arc::clone(&self.gauges)
    }

    /// Feeds one batch into the execute stage; suspends while the bounded
    /// submit queue is full (backpressure into the S stage).
    pub async fn submit(&self, batch: Batch) -> ClientResult<()> {
        Gauges::enqueue(
            &self.gauges.queue_depth_submit,
            &self.gauges.queue_depth_submit_peak,
        );
        self.submit_tx.send(batch).await.map_err(|_| {
            Gauges::dequeue(&self.gauges.queue_depth_submit);
            ClientError::new(ClientErrorKind::Internal, "pipeline executor stopped")
        })
    }

    /// Next completed batch: completion order in FAST mode, strict `seq`
    /// order in DETERMINISTIC mode. `Ok(None)` means the pipeline drained
    /// (submit side closed and all batches handed back).
    pub async fn next_completed(&mut self) -> ClientResult<Option<BatchResult>> {
        loop {
            if self.mode == SchedMode::Deterministic {
                if let Some(result) = self.reorder.remove(&self.next_seq) {
                    self.next_seq += 1;
                    return result.map(Some);
                }
            }
            let Some(result) = self.completed_rx.recv().await else {
                return Ok(None);
            };
            Gauges::dequeue(&self.gauges.queue_depth_complete);
            match self.mode {
                SchedMode::Fast => return result.map(Some),
                SchedMode::Deterministic => {
                    let seq = match &result {
                        Ok(batch) => batch.seq,
                        // A failed batch aborts the experiment; order no
                        // longer matters.
                        Err(error) => return Err(error.clone()),
                    };
                    self.reorder.insert(seq, result);
                }
            }
        }
    }

    /// Closes the submit side; in-flight batches still drain through
    /// [`Self::next_completed`].
    pub fn close(&mut self) {
        let (closed_tx, _) = mpsc::channel(1);
        self.submit_tx = closed_tx;
    }
}

async fn execute_batch<H>(
    driver: &WorkerDriver<H>,
    retry: &RetryPolicy,
    mode: SchedMode,
    batch: Batch,
    gauges: &Gauges,
) -> ClientResult<BatchResult>
where
    H: AsyncHypervisor + Clone + 'static,
{
    let first_pass = driver
        .run_batch(
            batch.seq,
            batch.parent_snapshot,
            batch.required_class.clone(),
            batch.bursts.clone(),
        )
        .await;

    let mut jobs = Vec::with_capacity(first_pass.len());
    for (job_idx, outcome) in first_pass {
        match outcome {
            Ok(result) => jobs.push(JobOutcome::Completed(Box::new(result))),
            Err(first_error) => {
                // Re-run on the Restore path with the full retry ladder.
                let spec = JobSpec {
                    batch_seq: batch.seq,
                    job_idx,
                    parent_snapshot: batch.parent_snapshot,
                    required_class: batch.required_class.clone(),
                    burst: batch.bursts[job_idx as usize].clone(),
                };
                match run_job_with_retry(driver, retry, &spec).await {
                    Ok(result) => jobs.push(JobOutcome::Completed(Box::new(result))),
                    Err(error) => match mode {
                        SchedMode::Fast => {
                            gauges
                                .jobs_failed_total
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            jobs.push(JobOutcome::Abandoned {
                                job_idx,
                                reason: format!("{first_error}; final: {error}"),
                            });
                        }
                        SchedMode::Deterministic => {
                            return Err(ClientError::new(
                                ClientErrorKind::Internal,
                                format!(
                                    "{DETERMINISTIC_RETRIES_EXHAUSTED_REASON}: batch {} job {} — {error}",
                                    batch.seq, job_idx
                                ),
                            ));
                        }
                    },
                }
            }
        }
    }

    // run_batch returns in job-index order already; keep the invariant
    // explicit for both modes.
    jobs.sort_by_key(|job| match job {
        JobOutcome::Completed(result) => result.job_idx,
        JobOutcome::Abandoned { job_idx, .. } => *job_idx,
    });

    Ok(BatchResult {
        seq: batch.seq,
        parent: batch.parent,
        jobs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        driver::{DriverConfig, WorkerDriver},
        ports::SyncAdapter,
        slots::{SlotView, SlotViewConfig},
    };
    use orch_clients::{
        hypervisor::{
            BootSpec, CaptureSpec, CreateVmRequest, DestroyVmRequest, Digest32, ElfBoot,
            EntropySeed, ExtractRange, HashEpochs, HypervisorWorkerClient, MachineConfig,
            TakeSnapshotRequest,
        },
        input_synth::{
            Burst, BurstBody, BurstId, ConfigFingerprint, GeneratorKind, PadBurst, PadSegment,
            Provenance,
        },
    };
    use orch_core::types::{FrameCount, GuestInstructions, StateHash};
    use orch_fakes::{hypervisor::FakeHypervisor, scorer::GRID_FEATURE_BYTES_LEN};

    const BUTTON_RIGHT: u32 = 0b10_0000_0000;
    const BUTTON_DOWN: u32 = 0b1000_0000;

    fn machine_config() -> MachineConfig {
        MachineConfig {
            version: 1,
            mem_bytes: 128 * 1024 * 1024,
            vcpus: 1,
            clock_num: 1,
            clock_den: 1,
            base_image_hash: Digest32::new([0xAA; 32]),
            boot: BootSpec::Elf(ElfBoot {
                kernel_hash: Digest32::new([0xBB; 32]),
                cmdline: b"console=ttyS0".to_vec(),
            }),
            epoch_len: GuestInstructions::new(50_000_000),
            hash_epochs: HashEpochs::EpochsOn,
            skid_margin: 8192,
        }
    }

    fn grid_capture() -> CaptureSpec {
        CaptureSpec {
            ranges: vec![ExtractRange {
                region: "grid".to_owned(),
                layout_version: 1,
                offset: 0,
                len: GRID_FEATURE_BYTES_LEN,
            }],
            framebuffer: false,
        }
    }

    fn burst(slot: u32, buttons: u32, hold: u32) -> ProvenancedBurst {
        ProvenancedBurst {
            burst: Burst {
                format_version: 1,
                burst_id: BurstId::new([slot as u8; 32]),
                body: BurstBody::Pad(PadBurst {
                    segments: vec![PadSegment {
                        buttons,
                        hold_frames: FrameCount::new(hold),
                    }],
                    button_alphabet: "console16-12btn-v1".to_owned(),
                }),
            },
            provenance: Provenance {
                generator: GeneratorKind::WeightedRandom,
                slot,
                rng_stream: format!("slot/{slot}/test"),
                config_fingerprint: ConfigFingerprint::new([0xA5; 32]),
                fallback_from: None,
                macro_provenance: None,
                mutation_provenance: None,
                policy_provenance: None,
            },
        }
    }

    async fn pipeline_over_fakes(mode: SchedMode, max_inflight: u32) -> (Pipeline, SnapshotRef) {
        let adapter = SyncAdapter::new(FakeHypervisor::with_slots(8));
        let (root, _) = {
            let fake = adapter.service();
            let mut fake = fake.lock().await;
            let created = fake
                .create_vm(CreateVmRequest {
                    config: machine_config(),
                    entropy_seed: EntropySeed::new([0x77; 32]),
                })
                .expect("create root");
            let snapshot = fake
                .take_snapshot(TakeSnapshotRequest {
                    lease: created.lease,
                    seal_input_log: false,
                    capture: None,
                })
                .expect("root snapshot");
            fake.destroy_vm(DestroyVmRequest {
                lease: created.lease,
            })
            .expect("destroy root");
            (snapshot.snapshot, snapshot.frame_counter)
        };
        let (view, _drain) = SlotView::start(adapter.clone(), SlotViewConfig::default())
            .await
            .expect("slot view");
        let driver = WorkerDriver::new(
            adapter,
            view,
            DriverConfig {
                experiment_seed: 0x5EED,
                capture: grid_capture(),
                hard_icount_cap: None,
            },
        );
        let pipeline = Pipeline::spawn(
            driver,
            PipelineConfig {
                mode,
                max_inflight_batches: max_inflight,
                retry: RetryPolicy {
                    job_timeout: std::time::Duration::from_secs(120),
                    retry_max: 2,
                    backoff_base: std::time::Duration::from_millis(50),
                },
            },
            0,
        );
        (pipeline, root)
    }

    fn sample_batch(seq: u64, root: SnapshotRef) -> Batch {
        Batch {
            seq,
            parent: NodeId::ROOT,
            parent_snapshot: root,
            required_class: None,
            bursts: vec![
                burst(0, BUTTON_RIGHT, 1),
                burst(1, BUTTON_DOWN, 2),
                burst(2, BUTTON_RIGHT, 3),
            ],
        }
    }

    fn transcript_of(results: &[BatchResult]) -> Vec<(u64, Vec<(u32, StateHash)>)> {
        results
            .iter()
            .map(|batch| {
                (
                    batch.seq,
                    batch
                        .jobs
                        .iter()
                        .map(|job| match job {
                            JobOutcome::Completed(result) => (
                                result.job_idx,
                                result.capture.as_ref().expect("capture").state_hash,
                            ),
                            JobOutcome::Abandoned { .. } => panic!("no abandoned jobs expected"),
                        })
                        .collect(),
                )
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn deterministic_mode_hands_batches_back_in_seq_order() {
        let (mut pipeline, root) = pipeline_over_fakes(SchedMode::Deterministic, 1).await;

        for seq in 0..3 {
            pipeline
                .submit(sample_batch(seq, root))
                .await
                .expect("submit");
        }
        pipeline.close();

        let mut seqs = Vec::new();
        while let Some(result) = pipeline.next_completed().await.expect("batch") {
            assert!(result
                .jobs
                .iter()
                .all(|job| matches!(job, JobOutcome::Completed(_))));
            seqs.push(result.seq);
        }
        assert_eq!(seqs, vec![0, 1, 2]);
    }

    #[tokio::test(start_paused = true)]
    async fn fast_mode_overlaps_batches_and_completes_all() {
        let (mut pipeline, root) = pipeline_over_fakes(SchedMode::Fast, 2).await;

        for seq in 0..4 {
            pipeline
                .submit(sample_batch(seq, root))
                .await
                .expect("submit");
        }
        pipeline.close();

        let mut seqs = Vec::new();
        while let Some(result) = pipeline.next_completed().await.expect("batch") {
            seqs.push(result.seq);
        }
        seqs.sort_unstable();
        assert_eq!(seqs, vec![0, 1, 2, 3]);
    }

    #[tokio::test(start_paused = true)]
    async fn fast_mode_abandons_a_poisoned_job_and_counts_it() {
        let (mut pipeline, root) = pipeline_over_fakes(SchedMode::Fast, 1).await;

        let mut batch = sample_batch(0, root);
        // Zero hold frames: InvalidRequest, non-retryable => abandoned.
        batch.bursts[1] = burst(1, BUTTON_DOWN, 0);
        pipeline.submit(batch).await.expect("submit");
        pipeline.close();

        let result = pipeline
            .next_completed()
            .await
            .expect("batch")
            .expect("one batch");
        let abandoned: Vec<_> = result
            .jobs
            .iter()
            .filter_map(|job| match job {
                JobOutcome::Abandoned { job_idx, .. } => Some(*job_idx),
                JobOutcome::Completed(_) => None,
            })
            .collect();
        assert_eq!(abandoned, vec![1]);
        assert_eq!(
            pipeline
                .gauges()
                .jobs_failed_total
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert!(pipeline.next_completed().await.expect("drained").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn deterministic_mode_fails_the_experiment_on_a_poisoned_job() {
        let (mut pipeline, root) = pipeline_over_fakes(SchedMode::Deterministic, 1).await;

        let mut batch = sample_batch(0, root);
        batch.bursts[0] = burst(0, BUTTON_RIGHT, 0);
        pipeline.submit(batch).await.expect("submit");
        pipeline.close();

        let error = pipeline
            .next_completed()
            .await
            .expect_err("deterministic mode aborts");
        assert!(error
            .message()
            .contains(DETERMINISTIC_RETRIES_EXHAUSTED_REASON));
    }

    // W3.5 determinism smoke: same seed, det mode, two runs of a fixed
    // batch script over fakes => identical commit-order transcripts.
    #[tokio::test(start_paused = true)]
    async fn deterministic_mode_smoke_two_runs_produce_identical_transcripts() {
        async fn run_script() -> Vec<(u64, Vec<(u32, StateHash)>)> {
            let (mut pipeline, root) = pipeline_over_fakes(SchedMode::Deterministic, 1).await;
            for seq in 0..4 {
                pipeline
                    .submit(sample_batch(seq, root))
                    .await
                    .expect("submit");
            }
            pipeline.close();
            let mut results = Vec::new();
            while let Some(result) = pipeline.next_completed().await.expect("batch") {
                results.push(result);
            }
            transcript_of(&results)
        }

        let first = run_script().await;
        let second = run_script().await;

        assert_eq!(first.len(), 4);
        assert_eq!(first, second);
    }
}
