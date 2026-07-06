//! Worker driver: the per-job lease lifecycle composition (API.md §2.2) and
//! the bootstrap composition (§2.1).
//!
//! Job path: `RestoreSnapshot | Fork -> InjectInputs -> Run(frame budget,
//! hard icount cap) -> TakeSnapshot(seal) -> DestroyVm`. Sibling forks of
//! one parent are batched into a single `ForkRequest` (the fake, like a
//! real worker, freezes the parent per fork and rejects forks of a frozen
//! parent), Restore is used otherwise, and retries always take the Restore
//! path. Frozen fork-parents hold slot permits, so they count as busy in
//! utilization accounting.
//!
//! The composition is pure with respect to the experiment seed: entropy
//! seeds derive from `("entropy", batch_seq, job_idx)` and `("boot")`
//! substreams, so a re-run of the same job spec is bytewise identical —
//! the property W3.4's retry equivalence leans on.

use std::time::Duration;

use orch_clients::{
    hypervisor::{
        CaptureSpec, CreateVmRequest, DestroyVmRequest, DeterminismClass, Digest32, EntropySeed,
        FbInfo, ForkRequest, GuestEvent, InjectInputsRequest, InputEvent, Lease, MachineConfig,
        PadSet, RestoreSnapshotRequest, RunRequest, RunUntil, ScheduleAt, ScheduledEvent,
        StopReason, TakeSnapshotRequest,
    },
    input_synth::{BurstBody, PadBurst, ProvenancedBurst},
    snapshot_store::InputLogId,
    ClientError, ClientErrorKind, ClientResult,
};
use orch_core::{
    rng::{derive_boot_entropy_seed, derive_job_entropy_seed},
    types::{FrameCount, GuestInstructions, SnapshotRef, StateHash},
};

use crate::{ports::AsyncHypervisor, slots::SlotView};

/// Virtual-time pause before retrying a `RESOURCE_EXHAUSTED` allocation
/// (backpressure, not an error).
const EXHAUSTED_BACKOFF: Duration = Duration::from_millis(5);

#[derive(Clone, Debug)]
pub struct DriverConfig {
    pub experiment_seed: u64,
    /// Capture spec attached to `Run` and `TakeSnapshot` (feature ranges +
    /// framebuffer request compiled from the experiment's feature map).
    pub capture: CaptureSpec,
    /// `BurstConfig::max_guest_instructions_per_job`; zero means uncapped.
    pub hard_icount_cap: Option<GuestInstructions>,
}

/// One expansion job: play one burst from a parent snapshot.
#[derive(Clone, Debug)]
pub struct JobSpec {
    pub batch_seq: u64,
    pub job_idx: u32,
    pub parent_snapshot: SnapshotRef,
    /// Parent node's determinism class requirement (from its attrs), gated
    /// against the worker class at acquire time.
    pub required_class: Option<DeterminismClass>,
    pub burst: ProvenancedBurst,
}

/// Verdict mapping per API.md §2.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobVerdict {
    /// BUDGET_REACHED | GOAL_SATISFIED | NEXT_SDK_EVENT.
    Ok,
    /// HARD_CAP: child is committed but marked frontier-ineligible.
    GuestHang,
    /// GUEST_HALTED | FAULTED: no child capture.
    GuestCrash,
}

/// Maps a worker stop reason to the job verdict (API.md §2.2 table).
pub fn verdict_from_stop_reason(reason: StopReason) -> ClientResult<JobVerdict> {
    match reason {
        StopReason::BudgetReached | StopReason::GoalSatisfied | StopReason::NextSdkEvent => {
            Ok(JobVerdict::Ok)
        }
        StopReason::HardCap => Ok(JobVerdict::GuestHang),
        StopReason::GuestHalted | StopReason::Faulted => Ok(JobVerdict::GuestCrash),
        StopReason::Unspecified | StopReason::Paused => Err(ClientError::new(
            ClientErrorKind::Internal,
            format!("unexpected worker stop reason {reason:?}"),
        )),
    }
}

/// Captured child state for a completed job.
#[derive(Clone, Debug, PartialEq)]
pub struct ChildCapture {
    pub snapshot: SnapshotRef,
    pub input_log_id: Option<InputLogId>,
    pub state_hash: StateHash,
    pub machine_config_hash: Digest32,
    pub determinism_class: DeterminismClass,
    pub icount: GuestInstructions,
    pub vns: u64,
    pub frame_counter: FrameCount,
    pub feature_bytes: Option<Vec<u8>>,
    pub fb_lz4: Option<Vec<u8>>,
    pub fb_info: Option<FbInfo>,
}

#[derive(Clone, Debug)]
pub struct JobResult {
    pub job_idx: u32,
    pub verdict: JobVerdict,
    /// The burst that produced this child, echoed for commit and node attrs.
    pub burst: ProvenancedBurst,
    /// `None` exactly when `verdict` is `GuestCrash`.
    pub capture: Option<ChildCapture>,
    /// SDK events observed during the run (assertion/reachability relay).
    pub sdk_events: Vec<GuestEvent>,
}

/// Bootstrap output (API.md §2.1): the root capture; scoring and the root
/// `CreateNode` stay in the M4 runner.
#[derive(Clone, Debug)]
pub struct RootCapture {
    pub snapshot: SnapshotRef,
    pub state_hash: StateHash,
    pub machine_config_hash: Digest32,
    pub determinism_class: DeterminismClass,
    pub icount: GuestInstructions,
    pub vns: u64,
    pub frame_counter: FrameCount,
    pub feature_bytes: Option<Vec<u8>>,
    pub fb_lz4: Option<Vec<u8>>,
    pub fb_info: Option<FbInfo>,
    pub ready_event: GuestEvent,
}

#[derive(Clone, Debug)]
pub struct BootstrapSpec {
    pub machine_config: MachineConfig,
    /// Instruction cap for reaching the Ready SDK event.
    pub bootstrap_icount_cap: Option<GuestInstructions>,
}

/// Translates a pad burst into absolute-frame `PadSet` events.
///
/// The worker (fake and frame-coherent real alike) rejects events at
/// `frame <= current frame_counter`, so the base is the parent's
/// `frame_counter + 1`; each segment lands at the cumulative offset of the
/// segments before it. Returns the events plus the total frame budget
/// (the sum of hold frames).
pub fn burst_events(
    burst: &PadBurst,
    parent_frame: FrameCount,
) -> ClientResult<(Vec<ScheduledEvent>, FrameCount)> {
    if burst.segments.is_empty() {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "pad burst has no segments",
        ));
    }
    let base = parent_frame.get().saturating_add(1);
    let mut offset: u32 = 0;
    let mut events = Vec::with_capacity(burst.segments.len());
    for segment in &burst.segments {
        events.push(ScheduledEvent {
            at: ScheduleAt::Frame(FrameCount::new(base.saturating_add(offset))),
            event: InputEvent::PadSet(PadSet {
                port: 0,
                buttons: segment.buttons,
            }),
        });
        offset = offset.saturating_add(segment.hold_frames.get());
    }
    if offset == 0 {
        return Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "pad burst holds zero frames",
        ));
    }
    Ok((events, FrameCount::new(offset)))
}

fn pad_burst(burst: &ProvenancedBurst) -> ClientResult<&PadBurst> {
    match &burst.burst.body {
        BurstBody::Pad(pad) => Ok(pad),
        BurstBody::Event(_) => Err(ClientError::new(
            ClientErrorKind::InvalidRequest,
            "event bursts are not supported by the M3 driver",
        )),
    }
}

fn is_backpressure(error: &ClientError) -> bool {
    error.kind() == ClientErrorKind::ResourceExhausted
}

pub struct WorkerDriver<H> {
    hypervisor: H,
    slots: SlotView,
    config: DriverConfig,
}

impl<H> Clone for WorkerDriver<H>
where
    H: Clone,
{
    fn clone(&self) -> Self {
        Self {
            hypervisor: self.hypervisor.clone(),
            slots: self.slots.clone(),
            config: self.config.clone(),
        }
    }
}

impl<H> WorkerDriver<H>
where
    H: AsyncHypervisor + Clone + 'static,
{
    pub fn new(hypervisor: H, slots: SlotView, config: DriverConfig) -> Self {
        Self {
            hypervisor,
            slots,
            config,
        }
    }

    #[must_use]
    pub fn slots(&self) -> &SlotView {
        &self.slots
    }

    /// Destroys a lease, absorbing transient injected/transport errors with
    /// a short salted retry ladder. A slot that stays undestroyed is leaked
    /// capacity for the rest of the run, so destroy is never one-shot.
    async fn destroy_with_retry(&self, lease: Lease) -> ClientResult<()> {
        let mut attempts = 0u32;
        loop {
            match self.hypervisor.destroy_vm(DestroyVmRequest { lease }).await {
                Ok(_) => return Ok(()),
                // Already gone (e.g. a prior teardown pass won the race).
                Err(error) if error.kind() == ClientErrorKind::NotFound => return Ok(()),
                Err(error) if attempts >= 8 => return Err(error),
                Err(_) => {
                    attempts += 1;
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            }
        }
    }

    /// Runs one job on the Restore path (also the retry path).
    pub async fn run_job(&self, spec: &JobSpec) -> ClientResult<JobResult> {
        self.run_job_deadline(spec, None).await
    }

    /// Runs one job with an optional per-attempt deadline covering
    /// everything after the slot permit is acquired. On expiry, any lease
    /// the attempt allocated is torn down best-effort so a timed-out job
    /// never leaks its slot (ARCHITECTURE.md §6.4).
    pub async fn run_job_deadline(
        &self,
        spec: &JobSpec,
        deadline: Option<Duration>,
    ) -> ClientResult<JobResult> {
        let permit = self.slots.acquire(spec.required_class.as_ref()).await?;
        let entropy = EntropySeed::new(derive_job_entropy_seed(
            self.config.experiment_seed,
            spec.batch_seq,
            spec.job_idx,
        ));

        let active_lease: std::sync::Mutex<Option<Lease>> = std::sync::Mutex::new(None);
        let work = async {
            // RESOURCE_EXHAUSTED on the actual allocation is backpressure:
            // the view over-admitted; wait a beat and retry the allocation.
            let restored = loop {
                match self
                    .hypervisor
                    .restore_snapshot(RestoreSnapshotRequest {
                        snapshot: spec.parent_snapshot,
                        entropy_seed: Some(entropy),
                    })
                    .await
                {
                    Ok(response) => break response,
                    Err(error) if is_backpressure(&error) => {
                        tokio::time::sleep(EXHAUSTED_BACKOFF).await;
                    }
                    Err(error) => return Err(error),
                }
            };
            *active_lease.lock().expect("lease cell poisoned") = Some(restored.lease);
            let result = self
                .drive_leased_job(spec, restored.lease, restored.frame_counter)
                .await;
            active_lease.lock().expect("lease cell poisoned").take();
            result
        };

        let result = match deadline {
            None => work.await,
            Some(deadline) => match tokio::time::timeout(deadline, work).await {
                Ok(result) => result,
                Err(_elapsed) => Err(ClientError::new(
                    ClientErrorKind::Unavailable,
                    format!("job timed out after {} ms", deadline.as_millis()),
                )),
            },
        };
        // Teardown of a lease stranded by cancellation.
        let stranded = active_lease.lock().expect("lease cell poisoned").take();
        if let Some(lease) = stranded {
            let _ = self.destroy_with_retry(lease).await;
        }
        drop(permit);
        result
    }

    /// Runs a batch of sibling jobs, forking them from one restored parent
    /// when enough slots are free right now (all-or-nothing `try_acquire`,
    /// so concurrent batches cannot deadlock); otherwise falls back to
    /// sequential-restore dispatch through `run_job`.
    ///
    /// Individual job failures surface in the returned vector; the caller's
    /// retry policy re-runs them on the Restore path.
    pub async fn run_batch(
        &self,
        batch_seq: u64,
        parent_snapshot: SnapshotRef,
        required_class: Option<DeterminismClass>,
        bursts: Vec<ProvenancedBurst>,
    ) -> Vec<(u32, ClientResult<JobResult>)> {
        let count = u32::try_from(bursts.len()).unwrap_or(u32::MAX);
        if count >= 2 {
            if let Some(results) = self
                .try_run_forked(batch_seq, parent_snapshot, required_class.as_ref(), &bursts)
                .await
            {
                return results;
            }
        }

        // Restore-path fallback: fan all K jobs out concurrently; each
        // acquires its own slot permit, so the SlotView is the bound.
        let mut jobs = Vec::with_capacity(bursts.len());
        for (job_idx, burst) in bursts.into_iter().enumerate() {
            let job_idx = u32::try_from(job_idx).unwrap_or(u32::MAX);
            let spec = JobSpec {
                batch_seq,
                job_idx,
                parent_snapshot,
                required_class: required_class.clone(),
                burst,
            };
            let driver = self.clone();
            jobs.push(async move { (job_idx, driver.run_job(&spec).await) });
        }
        futures_join_all(jobs).await
    }

    /// Fork-path dispatch; `None` means "not enough free slots right now,
    /// use the restore path" (no work was done).
    async fn try_run_forked(
        &self,
        batch_seq: u64,
        parent_snapshot: SnapshotRef,
        required_class: Option<&DeterminismClass>,
        bursts: &[ProvenancedBurst],
    ) -> Option<Vec<(u32, ClientResult<JobResult>)>> {
        let count = bursts.len();

        // All-or-nothing: parent slot + one per child.
        let mut permits = Vec::with_capacity(count + 1);
        for _ in 0..=count {
            match self.slots.try_acquire() {
                Some(permit) => permits.push(permit),
                None => return None,
            }
        }
        if let Some(required) = required_class {
            // Same gate acquire() applies; permits release on return.
            if let Err(error) = self.slots.acquire_class_check(required) {
                return Some(
                    (0..count)
                        .map(|idx| (idx as u32, Err(error.clone())))
                        .collect(),
                );
            }
        }

        let parent = match self
            .hypervisor
            .restore_snapshot(RestoreSnapshotRequest {
                snapshot: parent_snapshot,
                entropy_seed: None,
            })
            .await
        {
            Ok(response) => response,
            Err(error) if is_backpressure(&error) => {
                drop(permits);
                return None;
            }
            Err(_) => {
                drop(permits);
                return None; // fall back to restore path, which will surface the error per job
            }
        };

        let entropy_seeds = (0..count)
            .map(|job_idx| {
                EntropySeed::new(derive_job_entropy_seed(
                    self.config.experiment_seed,
                    batch_seq,
                    job_idx as u32,
                ))
            })
            .collect::<Vec<_>>();
        let fork_request = match ForkRequest::new(parent.lease, entropy_seeds) {
            Ok(request) => request,
            Err(error) => {
                let _ = self.destroy_with_retry(parent.lease).await;
                drop(permits);
                return Some(
                    (0..count)
                        .map(|idx| (idx as u32, Err(error.clone())))
                        .collect(),
                );
            }
        };
        let children = match self.hypervisor.fork(fork_request).await {
            Ok(response) => response.children,
            Err(error) => {
                let _ = self.destroy_with_retry(parent.lease).await;
                drop(permits);
                if is_backpressure(&error) {
                    return None;
                }
                return Some(
                    (0..count)
                        .map(|idx| (idx as u32, Err(error.clone())))
                        .collect(),
                );
            }
        };

        // Drive all children concurrently. Each child task owns one permit
        // and releases it as soon as its slot is destroyed, so an
        // early-finishing sibling's slot reads as free; the parent's permit
        // is held until the parent is destroyed (frozen parents are busy).
        let parent_permit = permits.pop().expect("permit set contains parent permit");
        let mut jobs = Vec::with_capacity(count);
        for (((job_idx, burst), lease), permit) in
            bursts.iter().enumerate().zip(children).zip(permits)
        {
            let spec = JobSpec {
                batch_seq,
                job_idx: job_idx as u32,
                parent_snapshot,
                required_class: required_class.cloned(),
                burst: burst.clone(),
            };
            let driver = self.clone();
            let parent_frame = parent.frame_counter;
            jobs.push(async move {
                let result = driver.drive_leased_job(&spec, lease, parent_frame).await;
                drop(permit);
                (spec.job_idx, result)
            });
        }
        let results = futures_join_all(jobs).await;

        let _ = self.destroy_with_retry(parent.lease).await;
        drop(parent_permit);
        Some(results)
    }

    /// Inject -> Run -> TakeSnapshot -> DestroyVm on an already-leased slot.
    async fn drive_leased_job(
        &self,
        spec: &JobSpec,
        lease: Lease,
        parent_frame: FrameCount,
    ) -> ClientResult<JobResult> {
        let result = self.drive_leased_job_inner(spec, lease, parent_frame).await;
        if result.is_err() {
            // Teardown so a failed job never leaks its slot.
            let _ = self.destroy_with_retry(lease).await;
        }
        result
    }

    async fn drive_leased_job_inner(
        &self,
        spec: &JobSpec,
        lease: Lease,
        parent_frame: FrameCount,
    ) -> ClientResult<JobResult> {
        let pad = pad_burst(&spec.burst)?;
        let (events, frame_budget) = burst_events(pad, parent_frame)?;

        self.hypervisor
            .inject_inputs(InjectInputsRequest { lease, events })
            .await?;

        let run = self
            .hypervisor
            .run(RunRequest {
                lease,
                until: RunUntil::FrameBudget(frame_budget),
                hard_icount_cap: self.config.hard_icount_cap,
                capture: Some(self.config.capture.clone()),
            })
            .await?;
        let verdict = verdict_from_stop_reason(run.reason)?;
        let sdk_events = run.sdk_event.clone().into_iter().collect();

        if verdict == JobVerdict::GuestCrash {
            self.destroy_with_retry(lease).await?;
            return Ok(JobResult {
                job_idx: spec.job_idx,
                verdict,
                burst: spec.burst.clone(),
                capture: None,
                sdk_events,
            });
        }

        let snapshot = self
            .hypervisor
            .take_snapshot(TakeSnapshotRequest {
                lease,
                seal_input_log: true,
                capture: Some(self.config.capture.clone()),
            })
            .await?;
        self.destroy_with_retry(lease).await?;

        Ok(JobResult {
            job_idx: spec.job_idx,
            verdict,
            burst: spec.burst.clone(),
            capture: Some(ChildCapture {
                snapshot: snapshot.snapshot,
                input_log_id: snapshot.input_log_id,
                state_hash: snapshot.state_hash,
                machine_config_hash: snapshot.machine_config_hash,
                determinism_class: snapshot.determinism_class,
                icount: snapshot.icount,
                vns: snapshot.vns,
                frame_counter: snapshot.frame_counter,
                feature_bytes: snapshot.feature_bytes,
                fb_lz4: snapshot.fb_lz4,
                fb_info: snapshot.fb_info,
            }),
            sdk_events,
        })
    }

    /// Bootstrap per API.md §2.1: `CreateVm -> Run(until Ready SDK event,
    /// bootstrap icount cap) -> TakeSnapshot -> DestroyVm`.
    pub async fn bootstrap(&self, spec: &BootstrapSpec) -> ClientResult<RootCapture> {
        let permit = self.slots.acquire(None).await?;
        let entropy = EntropySeed::new(derive_boot_entropy_seed(self.config.experiment_seed));

        let created = loop {
            match self
                .hypervisor
                .create_vm(CreateVmRequest {
                    config: spec.machine_config.clone(),
                    entropy_seed: entropy,
                })
                .await
            {
                Ok(response) => break response,
                Err(error) if is_backpressure(&error) => {
                    tokio::time::sleep(EXHAUSTED_BACKOFF).await;
                }
                Err(error) => return Err(error),
            }
        };
        let lease = created.lease;

        let result = self.bootstrap_leased(spec, lease).await;
        if result.is_err() {
            let _ = self.destroy_with_retry(lease).await;
        }
        drop(permit);
        result
    }

    async fn bootstrap_leased(
        &self,
        spec: &BootstrapSpec,
        lease: Lease,
    ) -> ClientResult<RootCapture> {
        let run = self
            .hypervisor
            .run(RunRequest {
                lease,
                until: RunUntil::NextSdkEvent(orch_clients::hypervisor::NextSdkEvent {
                    stream: None,
                }),
                hard_icount_cap: spec.bootstrap_icount_cap,
                capture: Some(self.config.capture.clone()),
            })
            .await?;
        if run.reason != StopReason::NextSdkEvent {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                format!("bootstrap did not reach the Ready event: {:?}", run.reason),
            ));
        }
        let ready_event = run.sdk_event.ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::DataLoss,
                "worker reported NEXT_SDK_EVENT without an event",
            )
        })?;

        let snapshot = self
            .hypervisor
            .take_snapshot(TakeSnapshotRequest {
                lease,
                seal_input_log: false,
                capture: Some(self.config.capture.clone()),
            })
            .await?;
        self.destroy_with_retry(lease).await?;

        Ok(RootCapture {
            snapshot: snapshot.snapshot,
            state_hash: snapshot.state_hash,
            machine_config_hash: snapshot.machine_config_hash,
            determinism_class: snapshot.determinism_class,
            icount: snapshot.icount,
            vns: snapshot.vns,
            frame_counter: snapshot.frame_counter,
            feature_bytes: snapshot.feature_bytes,
            fb_lz4: snapshot.fb_lz4,
            fb_info: snapshot.fb_info,
            ready_event,
        })
    }
}

/// Minimal ordered join for a homogeneous future set (avoids a futures-util
/// dependency): results arrive in spawn order.
async fn futures_join_all<F, T>(futures: Vec<F>) -> Vec<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let mut handles = Vec::with_capacity(futures.len());
    for future in futures {
        handles.push(tokio::spawn(future));
    }
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        results.push(handle.await.expect("driver job task panicked"));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ports::SyncAdapter,
        slots::{SlotView, SlotViewConfig},
    };
    use orch_clients::{
        hypervisor::{
            BootSpec, ElfBoot, ExtractRange, HashEpochs, HypervisorWorkerClient, ListSlotsRequest,
        },
        input_synth::{Burst, BurstId, ConfigFingerprint, GeneratorKind, PadSegment, Provenance},
    };
    use orch_fakes::{hypervisor::FakeHypervisor, scorer::GRID_FEATURE_BYTES_LEN};

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

    fn pad_provenanced(slot: u32, segments: Vec<PadSegment>) -> ProvenancedBurst {
        ProvenancedBurst {
            burst: Burst {
                format_version: 1,
                burst_id: BurstId::new([slot as u8; 32]),
                body: BurstBody::Pad(PadBurst {
                    segments,
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

    fn segment(buttons: u32, hold: u32) -> PadSegment {
        PadSegment {
            buttons,
            hold_frames: FrameCount::new(hold),
        }
    }

    const BUTTON_RIGHT: u32 = 0b10_0000_0000;
    const BUTTON_DOWN: u32 = 0b1000_0000;

    async fn driver_on(
        slots: u32,
    ) -> (
        WorkerDriver<SyncAdapter<FakeHypervisor>>,
        SyncAdapter<FakeHypervisor>,
    ) {
        let adapter = SyncAdapter::new(FakeHypervisor::with_slots(slots));
        let (view, _drain) = SlotView::start(adapter.clone(), SlotViewConfig::default())
            .await
            .expect("slot view");
        let driver = WorkerDriver::new(
            adapter.clone(),
            view,
            DriverConfig {
                experiment_seed: 0x5EED,
                capture: grid_capture(),
                hard_icount_cap: None,
            },
        );
        (driver, adapter)
    }

    async fn root_snapshot(adapter: &SyncAdapter<FakeHypervisor>) -> (SnapshotRef, FrameCount) {
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
    }

    #[test]
    fn burst_events_use_strict_future_absolute_frames() {
        let pad = PadBurst {
            segments: vec![segment(BUTTON_RIGHT, 2), segment(BUTTON_DOWN, 3)],
            button_alphabet: "console16-12btn-v1".to_owned(),
        };

        let (events, budget) = burst_events(&pad, FrameCount::new(5)).expect("events");

        assert_eq!(budget, FrameCount::new(5));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].at, ScheduleAt::Frame(FrameCount::new(6)));
        assert_eq!(events[1].at, ScheduleAt::Frame(FrameCount::new(8)));
        assert!(matches!(
            events[0].event,
            InputEvent::PadSet(PadSet {
                port: 0,
                buttons: BUTTON_RIGHT
            })
        ));
    }

    #[test]
    fn burst_events_reject_empty_and_zero_hold_bursts() {
        let empty = PadBurst {
            segments: Vec::new(),
            button_alphabet: String::new(),
        };
        let zero_hold = PadBurst {
            segments: vec![segment(BUTTON_RIGHT, 0)],
            button_alphabet: String::new(),
        };

        assert!(burst_events(&empty, FrameCount::new(0)).is_err());
        assert!(burst_events(&zero_hold, FrameCount::new(0)).is_err());
    }

    #[test]
    fn verdict_mapping_matches_api_table() {
        assert_eq!(
            verdict_from_stop_reason(StopReason::BudgetReached).unwrap(),
            JobVerdict::Ok
        );
        assert_eq!(
            verdict_from_stop_reason(StopReason::GoalSatisfied).unwrap(),
            JobVerdict::Ok
        );
        assert_eq!(
            verdict_from_stop_reason(StopReason::NextSdkEvent).unwrap(),
            JobVerdict::Ok
        );
        assert_eq!(
            verdict_from_stop_reason(StopReason::HardCap).unwrap(),
            JobVerdict::GuestHang
        );
        assert_eq!(
            verdict_from_stop_reason(StopReason::GuestHalted).unwrap(),
            JobVerdict::GuestCrash
        );
        assert_eq!(
            verdict_from_stop_reason(StopReason::Faulted).unwrap(),
            JobVerdict::GuestCrash
        );
        assert!(verdict_from_stop_reason(StopReason::Unspecified).is_err());
        assert!(verdict_from_stop_reason(StopReason::Paused).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn run_job_restores_runs_snapshots_and_frees_the_slot() {
        let (driver, adapter) = driver_on(4).await;
        let (parent, _) = root_snapshot(&adapter).await;

        let result = driver
            .run_job(&JobSpec {
                batch_seq: 0,
                job_idx: 0,
                parent_snapshot: parent,
                required_class: None,
                burst: pad_provenanced(0, vec![segment(BUTTON_RIGHT, 2)]),
            })
            .await
            .expect("job runs");

        assert_eq!(result.verdict, JobVerdict::Ok);
        let capture = result.capture.expect("child capture");
        assert!(capture.input_log_id.is_some());
        assert_eq!(capture.frame_counter, FrameCount::new(2));
        assert!(capture.feature_bytes.is_some());
        assert_eq!(driver.slots().snapshot().reserved, 0);
        let listed = adapter
            .service()
            .lock()
            .await
            .list_slots(ListSlotsRequest)
            .expect("list");
        assert!(listed.slots.is_empty(), "no leaked slots: {listed:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn run_batch_forks_siblings_and_leaves_no_slots_behind() {
        let (driver, adapter) = driver_on(8).await;
        let (parent, _) = root_snapshot(&adapter).await;

        let results = driver
            .run_batch(
                1,
                parent,
                None,
                vec![
                    pad_provenanced(0, vec![segment(BUTTON_RIGHT, 1)]),
                    pad_provenanced(1, vec![segment(BUTTON_DOWN, 1)]),
                    pad_provenanced(2, vec![segment(BUTTON_RIGHT, 2)]),
                ],
            )
            .await;

        assert_eq!(results.len(), 3);
        for (job_idx, result) in &results {
            let result = result.as_ref().expect("forked job ok");
            assert_eq!(result.job_idx, *job_idx);
            assert_eq!(result.verdict, JobVerdict::Ok);
            assert!(result.capture.is_some());
        }
        assert_eq!(driver.slots().snapshot().reserved, 0);
        let listed = adapter
            .service()
            .lock()
            .await
            .list_slots(ListSlotsRequest)
            .expect("list");
        assert!(listed.slots.is_empty(), "no leaked slots: {listed:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn run_batch_falls_back_to_restore_when_pool_is_too_small_for_forks() {
        let (driver, adapter) = driver_on(2).await;
        let (parent, _) = root_snapshot(&adapter).await;

        let results = driver
            .run_batch(
                2,
                parent,
                None,
                vec![
                    pad_provenanced(0, vec![segment(BUTTON_RIGHT, 1)]),
                    pad_provenanced(1, vec![segment(BUTTON_DOWN, 1)]),
                    pad_provenanced(2, vec![segment(BUTTON_RIGHT, 1)]),
                ],
            )
            .await;

        assert_eq!(results.len(), 3);
        for (_, result) in &results {
            assert!(result.is_ok(), "restore fallback job failed: {result:?}");
        }
        assert_eq!(driver.slots().snapshot().reserved, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn hard_icount_cap_maps_to_guest_hang() {
        let (driver, adapter) = driver_on(4).await;
        let (parent, _) = root_snapshot(&adapter).await;
        let mut driver = driver;
        driver.config.hard_icount_cap = Some(GuestInstructions::new(500));

        let result = driver
            .run_job(&JobSpec {
                batch_seq: 3,
                job_idx: 0,
                parent_snapshot: parent,
                required_class: None,
                burst: pad_provenanced(0, vec![segment(BUTTON_RIGHT, 5)]),
            })
            .await
            .expect("job runs to hard cap");

        assert_eq!(result.verdict, JobVerdict::GuestHang);
    }

    #[tokio::test(start_paused = true)]
    async fn bootstrap_reaches_ready_event_and_captures_root() {
        let (driver, adapter) = driver_on(4).await;

        let root = driver
            .bootstrap(&BootstrapSpec {
                machine_config: machine_config(),
                bootstrap_icount_cap: Some(GuestInstructions::new(10_000_000)),
            })
            .await
            .expect("bootstrap");

        assert_eq!(root.ready_event.payload, b"Ready");
        assert!(root.feature_bytes.is_some());
        assert_eq!(driver.slots().snapshot().reserved, 0);
        let listed = adapter
            .service()
            .lock()
            .await
            .list_slots(ListSlotsRequest)
            .expect("list");
        assert!(listed.slots.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn class_mismatch_refuses_job_with_grep_able_reason() {
        let (driver, adapter) = driver_on(4).await;
        let (parent, _) = root_snapshot(&adapter).await;
        let mut wrong = driver.slots().worker_class();
        wrong.cpu_model = "other-cpu".to_owned();

        let error = driver
            .run_job(&JobSpec {
                batch_seq: 0,
                job_idx: 0,
                parent_snapshot: parent,
                required_class: Some(wrong),
                burst: pad_provenanced(0, vec![segment(BUTTON_RIGHT, 1)]),
            })
            .await
            .expect_err("class mismatch");

        assert!(error
            .message()
            .contains(crate::slots::CLASS_MISMATCH_REASON));
    }
}
