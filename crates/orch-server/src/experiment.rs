//! `ExperimentRunner`: the main loop of ARCHITECTURE.md §3 — bring-up,
//! bootstrap, capacity-gated select+synthesize, pipeline execution, score +
//! commit with store writes and event emission, plateau ladder, budgets,
//! pause/stop, WAL journaling, and the §8 checkpoint lockstep. Resume (the
//! §8.2 binding sequence) lives here too, shared by the served surface and
//! the chaos harnesses.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use orch_checkpoint::{
    decode_checkpoint, decode_intent, encode_checkpoint, encode_intent, BudgetsUsed, CheckpointV1,
    ExpansionIntent, ExperimentState, FrontierEntry, IntentKnobs, PlateauCheckpoint,
    PlateauKnobsInEffect, RngCheckpoint,
};
use orch_clients::{
    hypervisor::DeterminismClass,
    input_synth::{ConfigFingerprint, ModelKind, ProposeBurstsRequest, ProposeBurstsResponse},
    observatory::EventSink,
    scorer::{
        ArchiveUpdateMode, CheckpointArchiveRequest, CommittedState, DecodedValue,
        ReplayCommitsRequest, RestoreArchiveRequest, ScoreBatchRequest, ScoreResult, StateInput,
    },
    snapshot_store::{
        CreateNodeRequest, DeleteMetadataRequest, GetMetadataRequest, MetadataExpectation,
        MetadataGeneration, MetadataKey, NodeMeta, NodeUpdate, OrderBy, PutMetadataRequest,
        QueryNodesRequest, UpdateNodesRequest,
    },
    ClientError, ClientErrorKind, ClientResult,
};
use orch_core::{
    commit::{commit_batch, CommitRules, CommitState, ScoredChild},
    frontier::Frontier,
    mirror::{CellMirror, SeenMap},
    plateau::{EscalationLadder, EscalationLevel, PlateauKnobs, StallDetector},
    policy::{
        softmax::SoftmaxPolicy, staged::StagedPolicy, ucb::UcbPolicy, PolicyContext, PolicyError,
        SelectionPolicy,
    },
    rng::{derive_synth_request_seed, DeterministicRng},
    tree::{NodePayload, Tree},
    types::{
        CommitDisposition, DiscardReason, ExperimentConfig, FiniteF64, FrameCount,
        GuestInstructions, NodeId, NodeStatus, Novelty, OnGoal, Score, SnapshotRef,
    },
};
use orch_driver::{
    input_synth::{
        validate_propose_bursts_response, FingerprintCheck, FingerprintRegistry,
        ProposeBurstsBuildSpec, SynthProfile,
    },
    node_attrs::{
        decode_node_attrs, encode_node_attrs, NodeContextLimits, OrchNodeAttrsV1, RootNodeAttrs,
        SynthContextAttrs,
    },
};
use orch_sched::{
    driver::{BootstrapSpec, DriverConfig, JobResult, JobVerdict, WorkerDriver},
    metrics::Gauges,
    pipeline::{Batch, BatchResult, JobOutcome, Pipeline, PipelineConfig},
    ports::{AsyncHypervisor, AsyncScorer, AsyncStore, AsyncSynth},
    retry::{is_retryable, retry_rpc, RetryPolicy},
    slots::{SlotView, SlotViewConfig},
};
use tokio::sync::{mpsc, watch};

use crate::{
    bringup::{
        bring_up, materialize_decoded_features, BringupOutcome, ExperimentSources, SynthBringupPort,
    },
    config::config_validation_failed_detail,
    events::{self, EventEmitter, PruneReason},
    metrics::{BatchLatencyStage, MetricsRegistry, MetricsStatus},
    SyncStoreAccess,
};

// ── fixed grep-able terminal reasons ────────────────────────────────────────

use orch_core::runtime_reasons::cataloged_failed_reason;
pub use orch_core::runtime_reasons::{
    REASON_ARCHIVE_SEQ_MISMATCH, REASON_CAS_OWNERSHIP_LOST, REASON_FINGERPRINT_MISMATCH,
    REASON_FRONTIER_EXHAUSTED, REASON_RUNTIME_ERROR,
};

/// In-process crash lattice (plan D5 Tier 1): every point in the loop where
/// state visibility changes. The chaos harness's policy returns `true` to
/// abort the runner at that point, exactly as a SIGKILL would land there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CrashPoint {
    AfterWalWrite,
    AfterDispatch,
    BeforeCommitOwnerCheck,
    MidBatchCommit,
    AfterCreateNode,
    BeforeWalDelete,
    AfterWalDelete,
    AfterCommitBeforeCheckpoint,
    BeforeCheckpointArchive,
    AfterCheckpointArchive,
    BeforeCasPut,
    AfterCasPut,
}

impl CrashPoint {
    pub const ALL: [CrashPoint; 12] = [
        CrashPoint::AfterWalWrite,
        CrashPoint::AfterDispatch,
        CrashPoint::BeforeCommitOwnerCheck,
        CrashPoint::MidBatchCommit,
        CrashPoint::AfterCreateNode,
        CrashPoint::BeforeWalDelete,
        CrashPoint::AfterWalDelete,
        CrashPoint::AfterCommitBeforeCheckpoint,
        CrashPoint::BeforeCheckpointArchive,
        CrashPoint::AfterCheckpointArchive,
        CrashPoint::BeforeCasPut,
        CrashPoint::AfterCasPut,
    ];

    /// The variant name, as parsed by [`CrashPoint::from_str`] (the Tier-2
    /// harness's `ORCH_CHAOS_HANG_AT` hook names points this way).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AfterWalWrite => "AfterWalWrite",
            Self::AfterDispatch => "AfterDispatch",
            Self::BeforeCommitOwnerCheck => "BeforeCommitOwnerCheck",
            Self::MidBatchCommit => "MidBatchCommit",
            Self::AfterCreateNode => "AfterCreateNode",
            Self::BeforeWalDelete => "BeforeWalDelete",
            Self::AfterWalDelete => "AfterWalDelete",
            Self::AfterCommitBeforeCheckpoint => "AfterCommitBeforeCheckpoint",
            Self::BeforeCheckpointArchive => "BeforeCheckpointArchive",
            Self::AfterCheckpointArchive => "AfterCheckpointArchive",
            Self::BeforeCasPut => "BeforeCasPut",
            Self::AfterCasPut => "AfterCasPut",
        }
    }
}

impl std::str::FromStr for CrashPoint {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|point| point.as_str() == value)
            .ok_or_else(|| format!("unknown crash point '{value}'"))
    }
}

pub trait CrashPolicy: Send {
    /// `true` aborts the runner at this point (simulated SIGKILL).
    fn should_crash(&mut self, point: CrashPoint) -> bool;
}

/// Error message marker for simulated crashes.
pub const CRASHED_MARKER: &str = "simulated-crash";

/// Control messages for the served Pause/Resume/Stop surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Pause,
    Resume,
    Stop,
}

/// Live status broadcast to StreamProgress / GetExperimentStatus.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusSnapshot {
    pub state: ExperimentState,
    pub expansions: u64,
    pub nodes: u64,
    pub best_score: Option<f64>,
    pub best_node: Option<NodeId>,
    pub batch_seq: u64,
    pub checkpointed_batch_seq: u64,
    pub escalation_level: u32,
    pub expansions_since_improvement: u64,
    pub frontier_size: u64,
    pub children_discarded_dup: u64,
    pub children_discarded_regression: u64,
    pub guest_instructions_used: u64,
    pub goal_nodes: Vec<NodeId>,
    pub failure_reason: Option<String>,
}

impl StatusSnapshot {
    fn initial() -> Self {
        Self {
            state: ExperimentState::Pending,
            expansions: 0,
            nodes: 0,
            best_score: None,
            best_node: None,
            batch_seq: 0,
            checkpointed_batch_seq: 0,
            escalation_level: 0,
            expansions_since_improvement: 0,
            frontier_size: 0,
            children_discarded_dup: 0,
            children_discarded_regression: 0,
            guest_instructions_used: 0,
            goal_nodes: Vec::new(),
            failure_reason: None,
        }
    }
}

/// Handle for controlling a running experiment and observing its status.
#[derive(Clone)]
pub struct RunnerHandle {
    control: mpsc::UnboundedSender<Control>,
    status: watch::Receiver<StatusSnapshot>,
}

impl RunnerHandle {
    pub fn send(&self, control: Control) -> ClientResult<()> {
        self.control.send(control).map_err(|_| {
            ClientError::new(ClientErrorKind::Unavailable, "experiment runner stopped")
        })
    }

    #[must_use]
    pub fn status(&self) -> StatusSnapshot {
        self.status.borrow().clone()
    }

    #[must_use]
    pub fn watch(&self) -> watch::Receiver<StatusSnapshot> {
        self.status.clone()
    }
}

/// Final outcome of a run.
#[derive(Clone, Debug)]
pub struct RunOutcome {
    pub state: ExperimentState,
    pub expansions: u64,
    pub nodes: u64,
    pub best_score: Option<f64>,
    pub goal_nodes: Vec<NodeId>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RunnerConfig {
    pub experiment_id: String,
    pub run_id: String,
    /// Deterministically injectable in tests; wall-clock-derived in the
    /// binary (`orchestratord-<startup_unix>`).
    pub producer_id: String,
    pub config: ExperimentConfig,
    /// blake3 over the canonical effective config (API.md §7).
    pub config_hash: [u8; 32],
}

struct NodeRuntime {
    snapshot: SnapshotRef,
    class: DeterminismClass,
}

impl<H, S, St, Sy, E> Drop for ExperimentRunner<H, S, St, Sy, E>
where
    H: AsyncHypervisor + Clone + 'static,
    S: AsyncScorer + Clone + 'static,
    St: AsyncStore + SyncStoreAccess + Clone + 'static,
    Sy: AsyncSynth + SynthBringupPort + Clone + 'static,
    E: EventSink,
{
    fn drop(&mut self) {
        // The SlotView drain task must not outlive its runner (a runner
        // that is started but never run() would otherwise leak it).
        self.slots_drain.abort();
    }
}

/// How the runner came up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartMode {
    Fresh,
    Resumed { checkpoint_batch_seq: u64 },
}

/// The experiment runner. Generic over the four service ports plus the
/// event sink; on fakes every port is a `SyncAdapter` around an
/// `orch-fakes` service.
pub struct ExperimentRunner<H, S, St, Sy, E>
where
    H: AsyncHypervisor + Clone + 'static,
    S: AsyncScorer + Clone + 'static,
    St: AsyncStore + SyncStoreAccess + Clone + 'static,
    Sy: AsyncSynth + SynthBringupPort + Clone + 'static,
    E: EventSink,
{
    cfg: RunnerConfig,
    sources: ExperimentSources,
    scorer: S,
    store: St,
    synth: Sy,
    emitter: EventEmitter<E>,
    driver: WorkerDriver<H>,
    slots_drain: tokio::task::JoinHandle<()>,
    retry: RetryPolicy,
    bringup: BringupOutcome,
    metrics: Arc<MetricsRegistry>,
    pipeline_gauges: Option<Arc<Gauges>>,

    control_rx: mpsc::UnboundedReceiver<Control>,
    status_tx: watch::Sender<StatusSnapshot>,
    crash: Option<Box<dyn CrashPolicy>>,

    // search state
    commit_state: CommitState,
    runtimes: BTreeMap<NodeId, NodeRuntime>,
    /// Committed child index by (parent, producing burst id): the replay
    /// adoption check's identity (unique even when sibling rows share a
    /// state hash, e.g. duplicate pruned-exhausted commits).
    node_bursts: BTreeMap<(NodeId, orch_clients::input_synth::BurstId), NodeId>,
    policy: PolicyBox,
    stall: StallDetector,
    ladder: EscalationLadder,
    fingerprints: FingerprintRegistry,
    fingerprint: Option<ConfigFingerprint>,
    batch_seq: u64,
    expansions: u64,
    best_score: Option<f64>,
    goal_nodes: Vec<NodeId>,
    best_node: Option<NodeId>,
    discarded_dup: u64,
    discarded_regression: u64,
    guest_instructions_used: u64,
    scorer_archive_seq: u64,
    ckpt_generation: Option<MetadataGeneration>,
    checkpointed_batch_seq: u64,
    /// Lowest WAL seq that may still exist (truncation cursor).
    wal_floor: u64,
    commits_since_checkpoint: u32,
    last_checkpoint_at: tokio::time::Instant,
    started_at: tokio::time::Instant,
    status: ExperimentState,
    failure_reason: Option<String>,
    /// WAL intents queued for (re-)dispatch before fresh selection resumes.
    replay_queue: VecDeque<ExpansionIntent>,
}

impl<H, S, St, Sy, E> ExperimentRunner<H, S, St, Sy, E>
where
    H: AsyncHypervisor + Clone + 'static,
    S: AsyncScorer + Clone + 'static,
    St: AsyncStore + SyncStoreAccess + Clone + 'static,
    Sy: AsyncSynth + SynthBringupPort + Clone + 'static,
    E: EventSink,
{
    /// Brings the experiment up (fresh bootstrap or §8.2 resume, decided by
    /// the presence of a checkpoint) and returns the runner plus its control
    /// handle.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        cfg: RunnerConfig,
        sources: ExperimentSources,
        hypervisor: H,
        scorer: S,
        store: St,
        synth: Sy,
        sink: E,
        crash: Option<Box<dyn CrashPolicy>>,
    ) -> ClientResult<(Self, RunnerHandle, StartMode)> {
        Self::start_with_metrics(
            cfg,
            sources,
            hypervisor,
            scorer,
            store,
            synth,
            sink,
            crash,
            Arc::new(MetricsRegistry::default()),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_with_metrics(
        cfg: RunnerConfig,
        sources: ExperimentSources,
        hypervisor: H,
        scorer: S,
        store: St,
        synth: Sy,
        sink: E,
        crash: Option<Box<dyn CrashPolicy>>,
        metrics: Arc<MetricsRegistry>,
    ) -> ClientResult<(Self, RunnerHandle, StartMode)> {
        let mut cfg = cfg;
        let retry = RetryPolicy::from_scheduling(&cfg.config.scheduling);
        let emitter = EventEmitter::new(sink, cfg.run_id.clone(), cfg.producer_id.clone())
            .with_metrics(Arc::clone(&metrics));
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = watch::channel(StatusSnapshot::initial());
        let handle = RunnerHandle {
            control: control_tx,
            status: status_rx,
        };

        // Existing checkpoint decides fresh vs resume (step 1: decode +
        // verify version and config_hash).
        let existing = store
            .get_metadata(GetMetadataRequest {
                key: MetadataKey::checkpoint(&cfg.experiment_id),
            })
            .await;
        let checkpoint = match existing {
            Ok(response) => Some((
                decode_checkpoint(&response.value, &cfg.experiment_id, &cfg.config_hash).map_err(
                    |error| {
                        ClientError::new(ClientErrorKind::FailedPrecondition, error.to_string())
                    },
                )?,
                response.generation,
            )),
            Err(error) if error.kind() == ClientErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };

        // Bring-up runs at the checkpointed feature-map version (L4 re-bins
        // recompile the coarsened document).
        let feature_map_version = checkpoint
            .as_ref()
            .map_or(0, |(checkpoint, _)| checkpoint.feature_map_version);
        let (feature_map, rebin) = if feature_map_version == 0 {
            (sources.feature_map.clone(), false)
        } else {
            (
                coarsened_map(&sources, &cfg.config, feature_map_version)?,
                true,
            )
        };
        let bringup = bring_up(
            &cfg.config,
            &cfg.experiment_id,
            &sources,
            &feature_map,
            rebin,
            &scorer,
            &synth,
        )
        .await?;
        cfg.config.decoded_features = materialize_decoded_features(&cfg.config, &bringup.compiled)
            .map_err(|error| {
                ClientError::new(
                    ClientErrorKind::InvalidRequest,
                    config_validation_failed_detail(&[error]),
                )
            })?;
        if let Some((checkpoint, _)) = &checkpoint {
            if checkpoint.feature_map_hash != bringup.feature_map_hash {
                return Err(ClientError::new(
                    ClientErrorKind::FailedPrecondition,
                    "feature map hash does not match the checkpoint",
                ));
            }
            if checkpoint.scoring_program_hash != bringup.scoring_program_hash {
                return Err(ClientError::new(
                    ClientErrorKind::FailedPrecondition,
                    "scoring program hash does not match the checkpoint",
                ));
            }
        }

        let (slots, slots_drain) = SlotView::start(
            hypervisor.clone(),
            SlotViewConfig {
                drain_interval: Duration::from_millis(5),
                allow_class_mismatch: cfg.config.scheduling.allow_class_mismatch,
            },
        )
        .await?;
        let driver = WorkerDriver::new(
            hypervisor,
            slots,
            DriverConfig {
                experiment_seed: cfg.config.seed,
                capture: bringup.capture.clone(),
                hard_icount_cap: cap_from(cfg.config.burst.max_guest_instructions_per_job),
            },
        );

        let plateau_knobs = PlateauKnobs::from_plateau_config(&cfg.config.plateau);
        let policy = PolicyBox::from_config(&cfg.config);
        let now = tokio::time::Instant::now();
        let placeholder_payload = NodePayload::new(
            SnapshotRef::new([0; 32]),
            Score::new(0.0).expect("zero score"),
            Novelty::new(0.0).expect("zero novelty"),
            orch_core::types::CellKey::new(0),
            orch_core::types::StateHash::new([0; 32]),
            orch_core::types::Stage::NONE,
            FrameCount::new(0),
        );
        let mut runner = Self {
            retry,
            emitter,
            driver,
            slots_drain,
            scorer,
            store,
            synth,
            bringup,
            control_rx,
            status_tx,
            crash,
            metrics,
            pipeline_gauges: None,
            commit_state: CommitState::from_root(placeholder_payload),
            runtimes: BTreeMap::new(),
            node_bursts: BTreeMap::new(),
            policy,
            stall: StallDetector::from_knobs(&plateau_knobs),
            ladder: EscalationLadder::from_knobs(&plateau_knobs),
            fingerprints: FingerprintRegistry::new(),
            fingerprint: None,
            batch_seq: 0,
            expansions: 0,
            best_score: None,
            goal_nodes: Vec::new(),
            best_node: None,
            discarded_dup: 0,
            discarded_regression: 0,
            guest_instructions_used: 0,
            scorer_archive_seq: 0,
            ckpt_generation: None,
            checkpointed_batch_seq: 0,
            wal_floor: 0,
            commits_since_checkpoint: 0,
            last_checkpoint_at: now,
            started_at: now,
            status: ExperimentState::Running,
            failure_reason: None,
            replay_queue: VecDeque::new(),
            cfg,
            sources,
        };

        let mode = match checkpoint {
            None => {
                runner.bootstrap_fresh().await?;
                StartMode::Fresh
            }
            Some((checkpoint, generation)) => {
                let seq = checkpoint.batch_seq;
                runner.resume(checkpoint, generation).await?;
                StartMode::Resumed {
                    checkpoint_batch_seq: seq,
                }
            }
        };
        runner.publish_status();
        Ok((runner, handle, mode))
    }

    fn crash_check(&mut self, point: CrashPoint) -> ClientResult<()> {
        if let Some(policy) = self.crash.as_mut() {
            if policy.should_crash(point) {
                return Err(ClientError::new(
                    ClientErrorKind::Internal,
                    format!("{CRASHED_MARKER}: {point:?}"),
                ));
            }
        }
        Ok(())
    }

    async fn ensure_checkpoint_owner(&mut self, window: &'static str) -> ClientResult<()> {
        let Some(expected_generation) = self.ckpt_generation else {
            return Ok(());
        };
        let store = self.store.clone();
        let key = MetadataKey::checkpoint(&self.cfg.experiment_id);
        match retry_rpc(&self.retry, || {
            store.get_metadata(GetMetadataRequest { key: key.clone() })
        })
        .await
        {
            Ok(response) if response.generation == expected_generation => Ok(()),
            Ok(response) => Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                format!(
                    "{REASON_CAS_OWNERSHIP_LOST}: {window}: checkpoint generation {} != owned {}",
                    response.generation.get(),
                    expected_generation.get()
                ),
            )),
            Err(error)
                if matches!(
                    error.kind(),
                    ClientErrorKind::NotFound
                        | ClientErrorKind::FailedPrecondition
                        | ClientErrorKind::AlreadyExists
                ) =>
            {
                Err(ClientError::new(
                    ClientErrorKind::FailedPrecondition,
                    format!("{REASON_CAS_OWNERSHIP_LOST}: {window}: {}", error.message()),
                ))
            }
            Err(error) => Err(error),
        }
    }

    fn publish_status(&self) {
        let _ = self.status_tx.send(StatusSnapshot {
            state: self.status,
            expansions: self.expansions,
            nodes: self.commit_state.tree.len() as u64,
            best_score: self.best_score,
            best_node: self.best_node,
            batch_seq: self.batch_seq,
            checkpointed_batch_seq: self.checkpointed_batch_seq,
            escalation_level: self.ladder.level().get(),
            expansions_since_improvement: self.stall.observations_since_improvement(),
            frontier_size: self.commit_state.frontier.len() as u64,
            children_discarded_dup: self.discarded_dup,
            children_discarded_regression: self.discarded_regression,
            guest_instructions_used: self.guest_instructions_used,
            goal_nodes: self.goal_nodes.clone(),
            failure_reason: self.failure_reason.clone(),
        });
        self.metrics.update_status(MetricsStatus {
            expansions_total: self.expansions,
            nodes_kept: self.commit_state.tree.len() as u64,
            nodes_dup: self.discarded_dup,
            nodes_regression: self.discarded_regression,
            best_score: self.best_score.unwrap_or_default(),
            frontier_size: self.commit_state.frontier.len() as u64,
            archive_cells: 0,
            escalation_level: self.ladder.level().get(),
            slot_utilization: 0.0,
        });
        if let Some(gauges) = &self.pipeline_gauges {
            self.metrics.update_pipeline_gauges(gauges);
        }
        self.metrics
            .set_observatory_dropped_total(self.emitter.dropped_total());
    }

    // ── fresh bootstrap (API.md §2.1) ───────────────────────────────────────

    async fn bootstrap_fresh(&mut self) -> ClientResult<()> {
        let spec = BootstrapSpec {
            machine_config: self.sources.workload.machine_config.clone(),
            bootstrap_icount_cap: self.sources.workload.bootstrap_icount_cap,
        };
        let root = retry_rpc(&self.retry, || self.driver.bootstrap(&spec)).await?;

        let feature_bytes = root.feature_bytes.clone().ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::DataLoss,
                "bootstrap capture had no feature bytes",
            )
        })?;
        let results = self
            .score_batch(
                "root".to_owned(),
                vec![StateInput {
                    node_ref: "0".to_owned(),
                    feature_bytes,
                    framebuffer: None,
                    fb_meta: None,
                }],
            )
            .await?;
        let result = results.into_iter().next().ok_or_else(|| {
            ClientError::new(ClientErrorKind::DataLoss, "scorer returned no root result")
        })?;

        let payload = NodePayload::new(
            root.snapshot,
            result.progress_score,
            result.novelty_score,
            result.novelty_detail.cell_key,
            result.state_hash,
            result.stage,
            root.frame_counter,
        );
        self.commit_state = CommitState::from_root(payload);
        self.runtimes.insert(
            NodeId::ROOT,
            NodeRuntime {
                snapshot: root.snapshot,
                class: root.determinism_class.clone(),
            },
        );

        let attrs = OrchNodeAttrsV1::new(
            root.machine_config_hash,
            root.determinism_class.clone(),
            SynthContextAttrs {
                created_by_burst: None,
                config_fingerprint: None,
                decoded_features: decoded_map(&self.cfg.config, &self.bringup, &result),
                frame_counter: root.frame_counter,
                state_hash: result.state_hash,
                cell_key: result.novelty_detail.cell_key,
                stage: result.stage,
                score: result.progress_score,
                novelty: result.novelty_score,
                recent_inputs: None,
            },
        )
        .with_root(RootNodeAttrs {
            framebuffer: root.fb_info,
            fps: self.sources.workload.fps,
            pad_layout: self.sources.workload.pad_layout.clone(),
        });
        let store = self.store.clone();
        let request = CreateNodeRequest {
            experiment_id: self.cfg.experiment_id.clone(),
            node_id: NodeId::ROOT,
            parent_node_id: None,
            snapshot_ref: root.snapshot,
            input_log_id: None,
            status: NodeStatus::Frontier,
            progress_score: result.progress_score,
            novelty_score: result.novelty_score,
            attrs: encode_node_attrs(&attrs)?,
            input_log_container: None,
        };
        retry_rpc(&self.retry, || store.create_node(request.clone())).await?;

        self.replay_commits(vec![CommittedState {
            state_hash: result.state_hash,
            cell_key: result.novelty_detail.cell_key,
        }])
        .await?;
        self.observe_score(NodeId::ROOT, result.progress_score.get());
        // Initial checkpoint: from here on every crash resumes through
        // §8.2 instead of falling back to a fresh bootstrap over a
        // non-empty store.
        self.checkpoint().await?;
        Ok(())
    }

    // ── resume (§8.2, the one binding sequence) ─────────────────────────────

    async fn resume(
        &mut self,
        checkpoint: CheckpointV1,
        generation: MetadataGeneration,
    ) -> ClientResult<()> {
        self.ckpt_generation = Some(generation);
        self.checkpointed_batch_seq = checkpoint.batch_seq;

        // Step 2: replay the WAL.
        let intents = self.load_wal_entries(&checkpoint).await?;

        // Step 3: rebuild the tree from QueryNodes (adopt ALL committed
        // nodes; ids are dense, CREATED_AT order == id order) and recompute
        // the id counter from the store.
        let store = self.store.clone();
        let query = QueryNodesRequest {
            experiment_id: self.cfg.experiment_id.clone(),
            statuses: Vec::new(),
            min_progress: None,
            max_progress: None,
            min_novelty: None,
            min_depth: None,
            max_depth: None,
            created_after: None,
            updated_after: None,
            order_by: OrderBy::CreatedAt,
            limit: None,
        };
        let mut nodes = retry_rpc(&self.retry, || store.query_nodes(query.clone()))
            .await?
            .nodes;
        nodes.sort_by_key(|node| node.node_id);
        if nodes.is_empty() || nodes[0].node_id != NodeId::ROOT {
            return Err(ClientError::new(
                ClientErrorKind::DataLoss,
                "checkpoint exists but the store has no root node",
            ));
        }

        let mut decoded: Vec<(NodeMeta, OrchNodeAttrsV1)> = Vec::with_capacity(nodes.len());
        for node in nodes {
            let attrs = decode_node_attrs(&node.attrs)?;
            decoded.push((node, attrs));
        }

        let weights: BTreeMap<NodeId, FrontierEntry> = checkpoint
            .frontier
            .iter()
            .map(|entry| (entry.node, *entry))
            .collect();

        // Tree rebuild with dense id verification.
        let root_payload = payload_from_store(&decoded[0].0, &decoded[0].1);
        let mut tree = Tree::from_root(root_payload);
        for (meta, attrs) in decoded.iter().skip(1) {
            let parent = meta.parent_node_id.ok_or_else(|| {
                ClientError::new(ClientErrorKind::DataLoss, "non-root node without parent")
            })?;
            let assigned = tree
                .insert_child(parent, payload_from_store(meta, attrs))
                .map_err(|error| {
                    ClientError::new(
                        ClientErrorKind::DataLoss,
                        format!("tree rebuild failed: {error:?}"),
                    )
                })?;
            if assigned != meta.node_id {
                return Err(ClientError::new(
                    ClientErrorKind::DataLoss,
                    format!(
                        "store node ids are not dense: expected {} got {}",
                        meta.node_id.get(),
                        assigned.get()
                    ),
                ));
            }
        }

        // Statuses, weights, mirrors, frontier membership.
        let mut frontier = Frontier::new();
        let mut mirror = CellMirror::from_sorted_counts(checkpoint.cell_mirror.clone());
        let mut seen = SeenMap::new();
        for (hash, node) in &checkpoint.seen {
            seen.insert(*hash, *node);
        }
        let checkpoint_seen: BTreeSet<_> = checkpoint.seen.iter().map(|(hash, _)| *hash).collect();
        let mut post_checkpoint = Vec::new();
        let mut streaks = Vec::new();

        for (meta, attrs) in &decoded {
            let id = meta.node_id;
            // Post-checkpoint nodes: catch the mirrors up from their attrs.
            if !checkpoint_seen.contains(&attrs.synth.state_hash)
                && seen.get(attrs.synth.state_hash).is_none()
            {
                seen.insert(attrs.synth.state_hash, id);
                mirror.bump(attrs.synth.cell_key);
                if id != NodeId::ROOT {
                    post_checkpoint.push(CommittedState {
                        state_hash: attrs.synth.state_hash,
                        cell_key: attrs.synth.cell_key,
                    });
                }
            }
            match meta.status {
                NodeStatus::Frontier => {
                    frontier.insert(id).ok();
                    if let Some(entry) = weights.get(&id) {
                        for _ in 0..entry.visits {
                            tree.increment_visits(id).ok();
                        }
                        streaks.push((id, u32::from(entry.consecutive_all_dup)));
                    }
                    // Post-checkpoint FRONTIER rows keep default weights.
                }
                NodeStatus::Goal => {
                    tree.mark_goal(id).ok();
                    if !self.goal_nodes.contains(&id) {
                        self.goal_nodes.push(id);
                    }
                }
                NodeStatus::Expanded => {
                    tree.mark_expanded(id).ok();
                }
                NodeStatus::Pruned => {
                    tree.mark_pruned(id).ok();
                }
            }
            self.runtimes.insert(
                id,
                NodeRuntime {
                    snapshot: meta.snapshot_ref,
                    class: attrs.determinism_class.clone(),
                },
            );
            if let (Some(parent), Some(burst)) =
                (meta.parent_node_id, attrs.synth.created_by_burst.as_ref())
            {
                self.node_bursts.insert((parent, burst.burst.burst_id), id);
            }
        }
        self.commit_state = CommitState::from_parts(tree, frontier, mirror, seen, &streaks);
        self.goal_nodes.sort_unstable();
        for goal in &checkpoint.goal_nodes {
            if !self.goal_nodes.contains(goal) {
                self.goal_nodes.push(*goal);
            }
        }
        self.goal_nodes.sort_unstable();

        // Step 5: RestoreArchive + seq assert + ReplayCommits over the
        // post-checkpoint committed nodes.
        let scorer = self.scorer.clone();
        let restore_request = RestoreArchiveRequest {
            experiment_id: self.cfg.experiment_id.clone(),
            checkpoint_id: checkpoint.scorer_checkpoint_id.clone(),
            archive_ref: checkpoint.scorer_archive_ref.clone(),
        };
        let restored = retry_rpc(&self.retry, || {
            scorer.restore_archive(restore_request.clone())
        })
        .await?;
        if restored.archive_seq != checkpoint.scorer_archive_seq {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                format!(
                    "{REASON_ARCHIVE_SEQ_MISMATCH}: restore returned {} expected {}",
                    restored.archive_seq, checkpoint.scorer_archive_seq
                ),
            ));
        }
        self.scorer_archive_seq = restored.archive_seq;
        // Post-checkpoint states may be replayed again when their WAL batch
        // re-commits below; ReplayCommits is idempotent (seen-set inserts),
        // which the scorer contract guarantees and the fakes implement.
        self.replay_commits(post_checkpoint).await?;

        // Step 6: synth fingerprint assert (bring-up already ran).
        if checkpoint.synth_config_fingerprint != [0u8; 32] {
            self.fingerprint = Some(ConfigFingerprint::new(checkpoint.synth_config_fingerprint));
        }

        // Step 7: queue WAL replays; batch_seq advances past the checkpoint
        // and every replayed seq (fresh batches never reuse a replayed
        // client_batch_id).
        // load_wal_entries already finished any interrupted truncation, so
        // every loaded intent is at or above checkpoint.batch_seq and
        // replayable.
        debug_assert!(intents
            .iter()
            .all(|intent| intent.seq >= checkpoint.batch_seq));
        let mut next_seq = checkpoint.batch_seq;
        for intent in &intents {
            next_seq = next_seq.max(intent.seq + 1);
        }
        self.batch_seq = next_seq;
        self.wal_floor = intents
            .first()
            .map_or(checkpoint.batch_seq, |intent| intent.seq);
        self.replay_queue = intents.into();

        self.expansions = checkpoint.expansions;
        self.guest_instructions_used = checkpoint.budgets_used.guest_instructions;
        self.best_score = checkpoint.plateau.best_score;
        let knobs = PlateauKnobs::from_plateau_config(&self.cfg.config.plateau);
        self.stall = StallDetector::restore(
            &knobs,
            checkpoint.plateau.observations,
            checkpoint
                .plateau
                .best_score
                .and_then(|value| Score::new(value).ok()),
            checkpoint.plateau.observations_since_improvement,
            checkpoint.plateau.completed_stall_windows,
        );
        self.ladder = EscalationLadder::restore(
            knobs.ladder,
            EscalationLevel::from_capped_u32(checkpoint.plateau.level),
            checkpoint.feature_map_version,
        );
        // Durable PAUSED survives the restart: the loop parks until Resume.
        self.status = match checkpoint.status {
            ExperimentState::Paused => ExperimentState::Paused,
            _ => ExperimentState::Running,
        };
        Ok(())
    }

    /// Loads surviving WAL entries.
    ///
    /// Entries at or above `checkpoint.batch_seq` are the replayable window
    /// (contiguous: dispatch writes them in seq order and only checkpoints
    /// truncate). Entries *below* it are covered by the checkpoint; a crash
    /// inside the ascending truncation loop leaves them as one contiguous
    /// segment ending at `batch_seq - 1`, so scanning downward from there
    /// until the first miss finds and deletes every stale survivor — no
    /// matter where in the (up to `every_commits`-wide) window the crash
    /// landed.
    async fn load_wal_entries(
        &mut self,
        checkpoint: &CheckpointV1,
    ) -> ClientResult<Vec<ExpansionIntent>> {
        let store = self.store.clone();

        // Finish any interrupted truncation: downward from batch_seq - 1.
        let mut seq = checkpoint.batch_seq;
        while seq > 0 {
            seq -= 1;
            let key = MetadataKey::wal(&self.cfg.experiment_id, seq);
            match store.get_metadata(GetMetadataRequest { key }).await {
                Ok(_) => {
                    let delete = DeleteMetadataRequest {
                        key: MetadataKey::wal(&self.cfg.experiment_id, seq),
                        expected_generation: MetadataExpectation::unconditional(),
                    };
                    match retry_rpc(&self.retry, || store.delete_metadata(delete.clone())).await {
                        Ok(_) => {}
                        Err(error) if error.kind() == ClientErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                Err(error) if error.kind() == ClientErrorKind::NotFound => break,
                Err(error) if is_retryable(&error) => {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    seq += 1;
                    continue;
                }
                Err(error) => return Err(error),
            }
        }

        // Collect the replayable window: upward from batch_seq.
        let inflight = u64::from(self.cfg.config.scheduling.max_inflight_batches.max(1));
        let mut intents = Vec::new();
        let mut seq = checkpoint.batch_seq;
        let mut consecutive_misses = 0u64;
        while consecutive_misses <= inflight + 1 {
            let key = MetadataKey::wal(&self.cfg.experiment_id, seq);
            match store.get_metadata(GetMetadataRequest { key }).await {
                Ok(response) => {
                    consecutive_misses = 0;
                    let intent = decode_intent(&response.value).map_err(|error| {
                        ClientError::new(ClientErrorKind::DataLoss, error.to_string())
                    })?;
                    intents.push(intent);
                }
                Err(error) if error.kind() == ClientErrorKind::NotFound => {
                    consecutive_misses += 1;
                }
                Err(error) if is_retryable(&error) => {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    continue;
                }
                Err(error) => return Err(error),
            }
            seq += 1;
        }
        intents.sort_by_key(|intent| intent.seq);
        Ok(intents)
    }

    // ── the loop ────────────────────────────────────────────────────────────

    pub async fn run(mut self) -> ClientResult<RunOutcome> {
        let first_seq = self
            .replay_queue
            .front()
            .map_or(self.batch_seq, |intent| intent.seq);
        let mut pipeline = Pipeline::spawn(
            self.driver.clone(),
            PipelineConfig {
                mode: self.cfg.config.scheduling.mode,
                max_inflight_batches: self.cfg.config.scheduling.max_inflight_batches,
                retry: self.retry,
            },
            first_seq,
        );
        self.pipeline_gauges = Some(pipeline.gauges());
        let mut inflight: BTreeMap<u64, ExpansionIntent> = BTreeMap::new();

        let result = self.run_phases(&mut pipeline, &mut inflight).await;
        self.slots_drain.abort();
        match result {
            Ok(()) => {}
            Err(error) if error.message().starts_with(CRASHED_MARKER) => {
                // Simulated SIGKILL: no final checkpoint, no status change —
                // the process is gone.
                return Err(error);
            }
            Err(error) => {
                self.status = ExperimentState::Failed;
                self.failure_reason = Some(cataloged_failed_reason(error.message()));
            }
        }

        // Final checkpoint (stop / budgets / goal / failure all land here).
        let ownership_lost = self
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with(REASON_CAS_OWNERSHIP_LOST));
        if !ownership_lost {
            if let Err(error) = self.checkpoint().await {
                if self.failure_reason.is_none() {
                    self.status = ExperimentState::Failed;
                    self.failure_reason = Some(cataloged_failed_reason(error.message()));
                }
            }
        }
        self.publish_status();
        Ok(RunOutcome {
            state: self.status,
            expansions: self.expansions,
            nodes: self.commit_state.tree.len() as u64,
            best_score: self.best_score,
            goal_nodes: self.goal_nodes.clone(),
            failure_reason: self.failure_reason.clone(),
        })
    }

    async fn run_phases(
        &mut self,
        pipeline: &mut Pipeline,
        inflight: &mut BTreeMap<u64, ExpansionIntent>,
    ) -> ClientResult<()> {
        // A runner resumed from a durable PAUSED checkpoint parks until the
        // served surface sends Resume (or Stop).
        if self.status == ExperimentState::Paused {
            self.publish_status();
            loop {
                match self.control_rx.recv().await {
                    Some(Control::Resume) => {
                        self.status = ExperimentState::Running;
                        self.publish_status();
                        break;
                    }
                    Some(Control::Stop) | None => {
                        self.status = ExperimentState::Stopped;
                        return Ok(());
                    }
                    Some(Control::Pause) => {}
                }
            }
        }
        // WAL replay phase: strictly sequential and identical in fast and
        // deterministic mode (§8.2 step 2). Each surviving intent re-runs
        // its pure jobs and re-commits with adoption of already-committed
        // children, reproducing the original run's effects exactly.
        while let Some(intent) = self.replay_queue.pop_front() {
            if !self.handle_control(pipeline, inflight).await? {
                return Ok(());
            }
            self.dispatch_replay(pipeline, &intent).await?;
            let done = pipeline.next_completed().await?.ok_or_else(|| {
                ClientError::new(ClientErrorKind::Internal, "pipeline ended during replay")
            })?;
            let commit_started = tokio::time::Instant::now();
            let goal = self.commit_replayed(done, &intent).await?;
            self.metrics
                .observe_batch_latency(BatchLatencyStage::Commit, commit_started.elapsed());
            self.publish_status();
            self.crash_check(CrashPoint::AfterCommitBeforeCheckpoint)?;
            if let Some(goal_node) = goal {
                self.checkpoint().await?;
                if self.cfg.config.on_goal == OnGoal::Stop {
                    self.status = ExperimentState::GoalReached;
                    let _ = goal_node;
                    return Ok(());
                }
            } else {
                self.maybe_checkpoint().await?;
            }
        }

        self.run_loop(pipeline, inflight).await
    }

    async fn run_loop(
        &mut self,
        pipeline: &mut Pipeline,
        inflight: &mut BTreeMap<u64, ExpansionIntent>,
    ) -> ClientResult<()> {
        loop {
            if !self.handle_control(pipeline, inflight).await? {
                return Ok(());
            }
            if let Some(reason) = self.budget_exhausted() {
                self.drain_inflight(pipeline, inflight).await?;
                self.status = ExperimentState::BudgetExhausted;
                self.failure_reason = Some(reason);
                return Ok(());
            }

            // A. SELECT + SYNTHESIZE while capacity remains.
            let max_inflight = self.cfg.config.scheduling.max_inflight_batches.max(1) as usize;
            while inflight.len() < max_inflight {
                if self.commit_state.frontier.is_empty() {
                    break;
                }
                let Some(intent) = self.build_intent()? else {
                    break;
                };
                let select_started = tokio::time::Instant::now();
                self.dispatch_intent(pipeline, inflight, intent).await?;
                self.metrics
                    .observe_batch_latency(BatchLatencyStage::Select, select_started.elapsed());
            }

            if inflight.is_empty() {
                // Frontier drained with nothing running: the search is out
                // of work without reaching the goal.
                self.status = ExperimentState::BudgetExhausted;
                self.failure_reason = Some(REASON_FRONTIER_EXHAUSTED.to_owned());
                return Ok(());
            }

            // C. SCORE + COMMIT the next completed batch.
            let Some(done) = pipeline.next_completed().await? else {
                return Ok(());
            };
            let intent = inflight.remove(&done.seq).ok_or_else(|| {
                ClientError::new(ClientErrorKind::Internal, "completed unknown batch")
            })?;
            let commit_started = tokio::time::Instant::now();
            let goal = self.commit_completed(done, &intent).await?;
            self.metrics
                .observe_batch_latency(BatchLatencyStage::Commit, commit_started.elapsed());
            self.publish_status();

            self.crash_check(CrashPoint::AfterCommitBeforeCheckpoint)?;
            if let Some(goal_node) = goal {
                // Checkpoint on goal (ARCHITECTURE.md §8 cadence list).
                self.checkpoint().await?;
                if self.cfg.config.on_goal == OnGoal::Stop {
                    self.drain_inflight(pipeline, inflight).await?;
                    self.status = ExperimentState::GoalReached;
                    let _ = goal_node;
                    return Ok(());
                }
            } else {
                self.maybe_checkpoint().await?;
            }
        }
    }

    /// Applies queued control messages. Returns false when the run must end
    /// (Stop).
    async fn handle_control(
        &mut self,
        pipeline: &mut Pipeline,
        inflight: &mut BTreeMap<u64, ExpansionIntent>,
    ) -> ClientResult<bool> {
        loop {
            match self.control_rx.try_recv() {
                Ok(Control::Pause) => {
                    // Drain in-flight to commit, checkpoint (incl. lockstep),
                    // park durably PAUSED.
                    self.drain_inflight(pipeline, inflight).await?;
                    self.status = ExperimentState::Paused;
                    self.checkpoint().await?;
                    self.publish_status();
                    loop {
                        match self.control_rx.recv().await {
                            Some(Control::Resume) => {
                                self.status = ExperimentState::Running;
                                self.publish_status();
                                break;
                            }
                            Some(Control::Stop) | None => {
                                self.status = ExperimentState::Stopped;
                                return Ok(false);
                            }
                            Some(Control::Pause) => {}
                        }
                    }
                }
                Ok(Control::Stop) => {
                    self.drain_inflight(pipeline, inflight).await?;
                    self.status = ExperimentState::Stopped;
                    return Ok(false);
                }
                Ok(Control::Resume) => {}
                Err(mpsc::error::TryRecvError::Empty)
                | Err(mpsc::error::TryRecvError::Disconnected) => return Ok(true),
            }
        }
    }

    async fn drain_inflight(
        &mut self,
        pipeline: &mut Pipeline,
        inflight: &mut BTreeMap<u64, ExpansionIntent>,
    ) -> ClientResult<()> {
        while !inflight.is_empty() {
            let Some(done) = pipeline.next_completed().await? else {
                break;
            };
            let intent = inflight.remove(&done.seq).ok_or_else(|| {
                ClientError::new(ClientErrorKind::Internal, "completed unknown batch")
            })?;
            let commit_started = tokio::time::Instant::now();
            self.commit_completed(done, &intent).await?;
            self.metrics
                .observe_batch_latency(BatchLatencyStage::Commit, commit_started.elapsed());
        }
        Ok(())
    }

    fn budget_exhausted(&self) -> Option<String> {
        let budgets = &self.cfg.config.budgets;
        if budgets.max_nodes > 0 && self.commit_state.tree.len() as u64 >= budgets.max_nodes {
            return Some("budget-exhausted: max_nodes".to_owned());
        }
        if budgets.max_expansions > 0 && self.expansions >= budgets.max_expansions {
            return Some("budget-exhausted: max_expansions".to_owned());
        }
        if budgets.max_guest_instructions > 0
            && self.guest_instructions_used >= budgets.max_guest_instructions
        {
            return Some("budget-exhausted: max_guest_instructions".to_owned());
        }
        if budgets.max_wall_clock_s > 0
            && self.started_at.elapsed() >= Duration::from_secs(budgets.max_wall_clock_s)
        {
            return Some("budget-exhausted: max_wall_clock_s".to_owned());
        }
        None
    }

    // ── S stage ─────────────────────────────────────────────────────────────

    fn build_intent(&mut self) -> ClientResult<Option<ExpansionIntent>> {
        let snapshot = self.ladder.snapshot();
        let knobs = PlateauKnobs::from_plateau_config(&self.cfg.config.plateau);
        let mut selection = self.cfg.config.selection.clone();
        selection.temperature *= snapshot.temp_factor;
        let backtrack = snapshot.l3_override.map(|l3| l3.backtrack);

        let mut rng = DeterministicRng::selection(self.cfg.config.seed, self.batch_seq);
        self.policy.set_total_expansions(self.expansions);
        let choice = {
            let context = PolicyContext::new(
                &self.commit_state.tree,
                &self.commit_state.frontier,
                &self.commit_state.cell_mirror,
                &knobs,
                &selection,
            )
            .with_backtrack(backtrack);
            match self.policy.select(&context, &mut rng) {
                Ok(choice) => choice,
                Err(PolicyError::EmptyCandidateSet) => return Ok(None),
                Err(error) => {
                    return Err(ClientError::new(
                        ClientErrorKind::Internal,
                        format!("selection failed: {error}"),
                    ))
                }
            }
        };

        let base_len = f64::from(self.cfg.config.burst.base_burst_len_frames);
        let length_hint = (base_len * snapshot.burst_len_factor)
            .min(f64::from(self.cfg.config.burst.max_burst_len_frames.max(1)))
            .max(1.0) as u32;
        let overrides = snapshot
            .l3_override
            .map(|l3| format!("generator_mix:\n  macro: {}\n", l3.macro_weight_hot).into_bytes())
            .unwrap_or_default();

        Ok(Some(ExpansionIntent {
            seq: self.batch_seq,
            node: choice.selected,
            burst_seed: derive_synth_request_seed(self.cfg.config.seed, self.batch_seq),
            knobs: IntentKnobs {
                k: self.cfg.config.burst.k_per_expansion,
                length_hint_frames: length_hint,
                escalation_level: snapshot.level.get(),
                config_overrides_yaml: overrides,
            },
            client_batch_id: format!("b{}", self.batch_seq),
        }))
    }

    async fn dispatch_intent(
        &mut self,
        pipeline: &Pipeline,
        inflight: &mut BTreeMap<u64, ExpansionIntent>,
        intent: ExpansionIntent,
    ) -> ClientResult<()> {
        // Synthesize from committed store state through the real builder.
        let request = self.store.build_propose_bursts(ProposeBurstsBuildSpec {
            experiment_id: self.cfg.experiment_id.clone(),
            node_id: intent.node,
            k: intent.knobs.k,
            length_hint: FrameCount::new(intent.knobs.length_hint_frames),
            experiment_seed: self.cfg.config.seed,
            batch_seq: intent.seq,
            model: ModelKind::Pad,
            config_overrides_yaml: intent.knobs.config_overrides_yaml.clone(),
            context_limits: NodeContextLimits::default(),
        })?;
        let response = self.guarded_propose(request).await?;

        // WAL before dispatch.
        let wal_bytes = encode_intent(&intent)
            .map_err(|error| ClientError::new(ClientErrorKind::Internal, error.to_string()))?;
        let store = self.store.clone();
        // Unconditional: a replayed intent's WAL entry already exists (same
        // seq => bitwise-identical intent), and fresh seqs never collide
        // because resume advances batch_seq past every surviving entry.
        let put = PutMetadataRequest {
            key: MetadataKey::wal(&self.cfg.experiment_id, intent.seq),
            value: wal_bytes,
            expected_generation: MetadataExpectation::unconditional(),
        };
        retry_rpc(&self.retry, || store.put_metadata(put.clone())).await?;
        self.crash_check(CrashPoint::AfterWalWrite)?;

        let runtime = self.runtimes.get(&intent.node).ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::Internal,
                format!("no runtime for node {}", intent.node.get()),
            )
        })?;
        pipeline
            .submit(Batch {
                seq: intent.seq,
                parent: intent.node,
                parent_snapshot: runtime.snapshot,
                required_class: Some(runtime.class.clone()),
                bursts: response.bursts,
            })
            .await?;
        self.crash_check(CrashPoint::AfterDispatch)?;
        self.batch_seq = self.batch_seq.max(intent.seq + 1);
        inflight.insert(intent.seq, intent);
        Ok(())
    }

    async fn guarded_propose(
        &mut self,
        request: ProposeBurstsRequest,
    ) -> ClientResult<ProposeBurstsResponse> {
        let profile = SynthProfile::from_request(&request);
        let synth = self.synth.clone();
        let response = retry_rpc(&self.retry, || synth.propose_bursts(request.clone())).await?;
        validate_propose_bursts_response(&request, &response)?;

        // The fingerprint is a function of the effective synth config, so it
        // legitimately differs per request profile (e.g. L3 overrides). The
        // registry guards per profile; the checkpointed fingerprint pins the
        // base (no-overrides) profile across restarts.
        let base_profile = request.config_overrides_yaml.is_empty();
        if base_profile {
            if let Some(expected) = self.fingerprint {
                if response.config_fingerprint != expected {
                    return Err(ClientError::new(
                        ClientErrorKind::FailedPrecondition,
                        format!(
                            "{REASON_FINGERPRINT_MISMATCH}: base-profile fingerprint diverged from the checkpointed bring-up fingerprint"
                        ),
                    ));
                }
            }
        }
        match self
            .fingerprints
            .check_or_insert(profile, response.config_fingerprint)
        {
            Ok(FingerprintCheck::Inserted | FingerprintCheck::Matched) => {
                if base_profile {
                    self.fingerprint = Some(response.config_fingerprint);
                }
                Ok(response)
            }
            Err(mismatch) => Err(mismatch.into_client_error()),
        }
    }

    // ── C stage ─────────────────────────────────────────────────────────────

    async fn commit_completed(
        &mut self,
        done: BatchResult,
        intent: &ExpansionIntent,
    ) -> ClientResult<Option<NodeId>> {
        let jobs: Vec<&JobResult> = done
            .jobs
            .iter()
            .filter_map(|job| match job {
                JobOutcome::Completed(result) => Some(result.as_ref()),
                // Fast mode: the batch continues without this child; the
                // gap is journaled in the batch result itself.
                JobOutcome::Abandoned { .. } => None,
            })
            .collect();
        for job in &jobs {
            if let Some(capture) = &job.capture {
                self.guest_instructions_used = self
                    .guest_instructions_used
                    .saturating_add(capture.icount.get());
            }
        }
        let scorable: Vec<&JobResult> = jobs
            .iter()
            .copied()
            .filter(|job| job.capture.is_some())
            .collect();

        let results = if scorable.is_empty() {
            Vec::new()
        } else {
            self.score_batch(
                intent.client_batch_id.clone(),
                scorable
                    .iter()
                    .map(|job| {
                        let capture = job.capture.as_ref().expect("scorable");
                        StateInput {
                            node_ref: format!("{}-j{}", intent.client_batch_id, job.job_idx),
                            feature_bytes: capture.feature_bytes.clone().unwrap_or_default(),
                            framebuffer: None,
                            fb_meta: None,
                        }
                    })
                    .collect(),
            )
            .await?
        };

        // Scored children; a guest_hang child routes through the prune rule
        // so it commits frontier-ineligible.
        let mut sibling_hashes = BTreeSet::new();
        let mut children = Vec::with_capacity(scorable.len());
        for (job, result) in scorable.iter().zip(&results) {
            let capture = job.capture.as_ref().expect("scorable");
            let novelty = Novelty::new(
                self.commit_state
                    .cell_mirror
                    .novelty(result.novelty_detail.cell_key),
            )
            .map_err(|error| {
                ClientError::new(ClientErrorKind::Internal, format!("novelty: {error}"))
            })?;
            let payload = NodePayload::new(
                capture.snapshot,
                result.progress_score,
                novelty,
                result.novelty_detail.cell_key,
                result.state_hash,
                result.stage,
                capture.frame_counter,
            );
            let sibling_duplicate = !sibling_hashes.insert(result.state_hash);
            let mut child = ScoredChild::new(payload);
            if result.duplicate || sibling_duplicate {
                child = child.duplicate();
            }
            if result.prune || job.verdict == JobVerdict::GuestHang {
                child = child.prune();
            }
            if result.goal_hit {
                child = child.goal();
            }
            children.push(child);
        }

        self.crash_check(CrashPoint::BeforeCommitOwnerCheck)?;
        self.ensure_checkpoint_owner("node-commit").await?;

        let rules = CommitRules::from_config(&self.cfg.config);
        let outcome = commit_batch(&mut self.commit_state, done.parent, &children, &rules)
            .map_err(|error| {
                ClientError::new(
                    ClientErrorKind::Internal,
                    format!("commit failed: {error:?}"),
                )
            })?;

        // Store writes + events per the commit rules.
        let ts = self.expansions;
        let mut committed_states = Vec::new();
        let mut committed_count = 0u64;
        let mut discarded_count = 0u64;
        let mut best_committed: Option<(NodeId, f64)> = None;
        for (index, (job, child_commit)) in scorable.iter().zip(&outcome.child_commits).enumerate()
        {
            if index > 0 && index == scorable.len() / 2 {
                self.crash_check(CrashPoint::MidBatchCommit)?;
            }
            let result = &results[index];
            match child_commit.node_id {
                Some(node_id) => {
                    let (payload, status) = {
                        let record = self
                            .commit_state
                            .tree
                            .get(node_id)
                            .expect("committed child in tree");
                        (record.payload(), record.status)
                    };
                    self.create_child_node(node_id, done.parent, job, result, status)
                        .await?;
                    self.crash_check(CrashPoint::AfterCreateNode)?;
                    committed_states.push(CommittedState {
                        state_hash: payload.state_hash,
                        cell_key: payload.cell,
                    });
                    committed_count += 1;
                    let capture = job.capture.as_ref().expect("scorable");
                    self.runtimes.insert(
                        node_id,
                        NodeRuntime {
                            snapshot: capture.snapshot,
                            class: capture.determinism_class.clone(),
                        },
                    );
                    self.node_bursts
                        .insert((done.parent, job.burst.burst.burst_id), node_id);
                    if status == NodeStatus::Pruned {
                        self.emitter.emit(
                            ts,
                            "node-pruned",
                            events::node_pruned_payload(
                                done.parent,
                                PruneReason::Exhausted,
                                Some(node_id),
                            ),
                        );
                    } else {
                        let features = decoded_pairs(&self.cfg.config, &self.bringup, result);
                        self.emitter.emit(
                            ts,
                            "node-added",
                            events::node_added_payload(
                                node_id,
                                done.parent,
                                payload.score.get(),
                                payload.novelty_at_commit.get(),
                                payload.cell.get(),
                                u32::from(payload.stage.get()),
                                &features,
                            ),
                        );
                    }
                    if best_committed.is_none_or(|(_, best)| payload.score.get() > best) {
                        best_committed = Some((node_id, payload.score.get()));
                    }
                    // Relay guest-sdk events post-commit with node context.
                    for sdk_event in &job.sdk_events {
                        let event_type = if sdk_event.payload.starts_with(b"assertion") {
                            Some("assertion-violated")
                        } else if sdk_event.payload.starts_with(b"reachability") {
                            Some("reachability-hit")
                        } else {
                            None
                        };
                        if let Some(event_type) = event_type {
                            self.emitter.emit(
                                ts,
                                event_type,
                                events::sdk_event_payload(
                                    node_id,
                                    sdk_event.stream,
                                    &sdk_event.payload,
                                ),
                            );
                        }
                    }
                }
                None => {
                    discarded_count += 1;
                    let reason = match child_commit.disposition {
                        CommitDisposition::Discard(DiscardReason::Regression) => {
                            self.discarded_regression += 1;
                            PruneReason::Regression
                        }
                        _ => {
                            self.discarded_dup += 1;
                            PruneReason::Duplicate
                        }
                    };
                    if let Some(existing) = child_commit.duplicate_route {
                        self.update_node(NodeUpdate {
                            node_id: existing,
                            status: None,
                            progress_score: None,
                            novelty_score: None,
                            visit_count_delta: 1,
                            expand_count_delta: 0,
                            touch_visited: true,
                            attrs: None,
                        })
                        .await?;
                    }
                    self.emitter.emit(
                        ts,
                        "node-pruned",
                        events::node_pruned_payload(done.parent, reason, None),
                    );
                }
            }
        }

        // Parent bookkeeping (rules 4/5).
        let parent_status = self
            .commit_state
            .tree
            .get(done.parent)
            .expect("parent in tree")
            .status;
        self.update_node(NodeUpdate {
            node_id: done.parent,
            status: Some(parent_status),
            progress_score: None,
            novelty_score: None,
            visit_count_delta: 1,
            expand_count_delta: 1,
            touch_visited: true,
            attrs: None,
        })
        .await?;
        if outcome.parent_evicted.is_some() {
            self.emitter.emit(
                ts,
                "node-pruned",
                events::node_pruned_payload(done.parent, PruneReason::FrontierEvict, None),
            );
        }

        self.replay_commits(committed_states).await?;

        self.expansions += 1;
        self.commits_since_checkpoint += 1;
        self.emitter.emit(
            ts,
            "batch-completed",
            events::batch_completed_payload(
                done.seq,
                done.parent,
                committed_count,
                discarded_count,
            ),
        );

        // Plateau observation on the batch's best committed score (or the
        // parent's when nothing committed — still an observation).
        let observed = best_committed
            .map(|(_, score)| score)
            .unwrap_or_else(|| parent_score(&self.commit_state, done.parent));
        let observed_node = best_committed.map_or(done.parent, |(node, _)| node);
        self.observe_score(observed_node, observed);
        self.maybe_rebin().await?;

        if let Some(goal) = outcome.goal_node {
            if !self.goal_nodes.contains(&goal) {
                self.goal_nodes.push(goal);
            }
            let score = self
                .commit_state
                .tree
                .get(goal)
                .map(|record| record.score.get())
                .unwrap_or_default();
            self.emitter.emit(
                ts,
                "goal-reached",
                events::goal_reached_payload(goal, score),
            );
            return Ok(Some(goal));
        }
        Ok(None)
    }

    /// Re-dispatches a surviving WAL intent. The WAL entry already exists;
    /// jobs are pure, so the batch re-executes bit-identically.
    async fn dispatch_replay(
        &mut self,
        pipeline: &Pipeline,
        intent: &ExpansionIntent,
    ) -> ClientResult<()> {
        let request = self.store.build_propose_bursts(ProposeBurstsBuildSpec {
            experiment_id: self.cfg.experiment_id.clone(),
            node_id: intent.node,
            k: intent.knobs.k,
            length_hint: FrameCount::new(intent.knobs.length_hint_frames),
            experiment_seed: self.cfg.config.seed,
            batch_seq: intent.seq,
            model: ModelKind::Pad,
            config_overrides_yaml: intent.knobs.config_overrides_yaml.clone(),
            context_limits: NodeContextLimits::default(),
        })?;
        let response = self.guarded_propose(request).await?;
        self.crash_check(CrashPoint::AfterWalWrite)?;

        let runtime = self.runtimes.get(&intent.node).ok_or_else(|| {
            ClientError::new(
                ClientErrorKind::Internal,
                format!("no runtime for replayed node {}", intent.node.get()),
            )
        })?;
        pipeline
            .submit(Batch {
                seq: intent.seq,
                parent: intent.node,
                parent_snapshot: runtime.snapshot,
                required_class: Some(runtime.class.clone()),
                bursts: response.bursts,
            })
            .await?;
        self.crash_check(CrashPoint::AfterDispatch)?;
        Ok(())
    }

    /// Replay-exact commit (§8.2 step 2): children the original run already
    /// committed are adopted (their tree/seen/mirror effects landed during
    /// resume adoption); everything else recomputes through the commit
    /// rules with duplicate verdicts derived from the orchestrator's own
    /// SeenMap (bitwise equal to the scorer's verdict on fakes, whose dedup
    /// is the same seen-set — disclosed).
    async fn commit_replayed(
        &mut self,
        done: BatchResult,
        intent: &ExpansionIntent,
    ) -> ClientResult<Option<NodeId>> {
        use orch_core::commit::update_parent_exhaustion;

        let jobs: Vec<&JobResult> = done
            .jobs
            .iter()
            .filter_map(|job| match job {
                JobOutcome::Completed(result) => Some(result.as_ref()),
                JobOutcome::Abandoned { .. } => None,
            })
            .collect();
        for job in &jobs {
            if let Some(capture) = &job.capture {
                self.guest_instructions_used = self
                    .guest_instructions_used
                    .saturating_add(capture.icount.get());
            }
        }
        let scorable: Vec<&JobResult> = jobs
            .iter()
            .copied()
            .filter(|job| job.capture.is_some())
            .collect();
        let results = if scorable.is_empty() {
            Vec::new()
        } else {
            self.score_batch(
                intent.client_batch_id.clone(),
                scorable
                    .iter()
                    .map(|job| {
                        let capture = job.capture.as_ref().expect("scorable");
                        StateInput {
                            node_ref: format!("{}-j{}", intent.client_batch_id, job.job_idx),
                            feature_bytes: capture.feature_bytes.clone().unwrap_or_default(),
                            framebuffer: None,
                            fb_meta: None,
                        }
                    })
                    .collect(),
            )
            .await?
        };

        self.crash_check(CrashPoint::BeforeCommitOwnerCheck)?;
        self.ensure_checkpoint_owner("node-commit").await?;

        let rules = CommitRules::from_config(&self.cfg.config);
        let parent = done.parent;
        let parent_score_value = parent_score(&self.commit_state, parent);
        let ts = self.expansions;
        let mut committed_states = Vec::new();
        let mut committed_count = 0u64;
        let mut discarded_count = 0u64;
        let mut best_committed: Option<(NodeId, f64)> = None;
        let mut goal_node: Option<NodeId> = None;
        let mut all_duplicate = !scorable.is_empty();

        for (index, (job, result)) in scorable.iter().zip(&results).enumerate() {
            if index > 0 && index == scorable.len() / 2 {
                self.crash_check(CrashPoint::MidBatchCommit)?;
            }
            let capture = job.capture.as_ref().expect("scorable");
            let hash = result.state_hash;
            let cell = result.novelty_detail.cell_key;

            // Adoption: this exact child row already exists (same parent,
            // same producing burst, same state hash).
            if let Some(&existing) = self.node_bursts.get(&(parent, job.burst.burst.burst_id)) {
                let adopted = self.commit_state.tree.get(existing).is_some_and(|record| {
                    record.parent == Some(parent) && record.state_hash == hash
                });
                if adopted {
                    all_duplicate = false;
                    committed_count += 1;
                    committed_states.push(CommittedState {
                        state_hash: hash,
                        cell_key: cell,
                    });
                    let record = self
                        .commit_state
                        .tree
                        .get(existing)
                        .expect("adopted node in tree");
                    if best_committed.is_none_or(|(_, best)| record.score.get() > best) {
                        best_committed = Some((existing, record.score.get()));
                    }
                    if record.goal && goal_node.is_none() {
                        goal_node = Some(existing);
                    }
                    continue;
                }
            }

            let prune = result.prune || job.verdict == JobVerdict::GuestHang;
            let duplicate = self.commit_state.seen.get(hash).is_some();
            let novelty =
                Novelty::new(self.commit_state.cell_mirror.novelty(cell)).map_err(|error| {
                    ClientError::new(ClientErrorKind::Internal, format!("novelty: {error}"))
                })?;
            let payload = NodePayload::new(
                capture.snapshot,
                result.progress_score,
                novelty,
                cell,
                hash,
                result.stage,
                capture.frame_counter,
            );

            if prune {
                all_duplicate = false;
                match rules.prune_action {
                    orch_core::types::PruneAction::Drop => {
                        discarded_count += 1;
                    }
                    orch_core::types::PruneAction::Exhausted => {
                        let node_id = self
                            .commit_state
                            .tree
                            .insert_child(parent, payload)
                            .map_err(|error| {
                                ClientError::new(
                                    ClientErrorKind::Internal,
                                    format!("replay insert failed: {error:?}"),
                                )
                            })?;
                        self.commit_state.ensure_tracking(node_id);
                        self.commit_state.tree.mark_pruned(node_id).ok();
                        self.commit_state.seen.insert(hash, node_id);
                        self.commit_state.cell_mirror.bump(cell);
                        self.create_child_node(node_id, parent, job, result, NodeStatus::Pruned)
                            .await?;
                        self.crash_check(CrashPoint::AfterCreateNode)?;
                        self.register_committed_child(node_id, job, hash, cell);
                        committed_count += 1;
                        committed_states.push(CommittedState {
                            state_hash: hash,
                            cell_key: cell,
                        });
                        self.emitter.emit(
                            ts,
                            "node-pruned",
                            events::node_pruned_payload(
                                parent,
                                PruneReason::Exhausted,
                                Some(node_id),
                            ),
                        );
                    }
                }
                continue;
            }

            if duplicate {
                discarded_count += 1;
                self.discarded_dup += 1;
                self.commit_state.cell_mirror.bump(cell);
                let route = self.commit_state.seen.get(hash);
                if let Some(existing) = route {
                    if existing != parent {
                        self.commit_state.tree.increment_visits(existing).ok();
                    }
                    self.update_node(NodeUpdate {
                        node_id: existing,
                        status: None,
                        progress_score: None,
                        novelty_score: None,
                        visit_count_delta: 1,
                        expand_count_delta: 0,
                        touch_visited: true,
                        attrs: None,
                    })
                    .await?;
                }
                self.emitter.emit(
                    ts,
                    "node-pruned",
                    events::node_pruned_payload(parent, PruneReason::Duplicate, None),
                );
                continue;
            }

            all_duplicate = false;
            let worse = payload.score.get() + rules.epsilon_keep < parent_score_value;
            let known_cell = self.commit_state.cell_mirror.count(cell) > 0;
            if worse && known_cell {
                discarded_count += 1;
                self.discarded_regression += 1;
                self.emitter.emit(
                    ts,
                    "node-pruned",
                    events::node_pruned_payload(parent, PruneReason::Regression, None),
                );
                continue;
            }

            let node_id = self
                .commit_state
                .tree
                .insert_child(parent, payload)
                .map_err(|error| {
                    ClientError::new(
                        ClientErrorKind::Internal,
                        format!("replay insert failed: {error:?}"),
                    )
                })?;
            self.commit_state.ensure_tracking(node_id);
            self.commit_state.seen.insert(hash, node_id);
            self.commit_state.cell_mirror.bump(cell);
            let status = if result.goal_hit {
                self.commit_state.tree.mark_goal(node_id).ok();
                if goal_node.is_none() {
                    goal_node = Some(node_id);
                }
                NodeStatus::Goal
            } else {
                self.commit_state.frontier.insert(node_id).ok();
                NodeStatus::Frontier
            };
            self.create_child_node(node_id, parent, job, result, status)
                .await?;
            self.crash_check(CrashPoint::AfterCreateNode)?;
            self.register_committed_child(node_id, job, hash, cell);
            committed_count += 1;
            committed_states.push(CommittedState {
                state_hash: hash,
                cell_key: cell,
            });
            if best_committed.is_none_or(|(_, best)| payload.score.get() > best) {
                best_committed = Some((node_id, payload.score.get()));
            }
            let features = decoded_pairs(&self.cfg.config, &self.bringup, result);
            self.emitter.emit(
                ts,
                "node-added",
                events::node_added_payload(
                    node_id,
                    parent,
                    payload.score.get(),
                    payload.novelty_at_commit.get(),
                    payload.cell.get(),
                    u32::from(payload.stage.get()),
                    &features,
                ),
            );
        }

        // Rules 4/5 exactly as commit_batch applies them.
        let parent_visits = self
            .commit_state
            .tree
            .increment_visits(parent)
            .map_err(|error| {
                ClientError::new(
                    ClientErrorKind::Internal,
                    format!("replay visit failed: {error:?}"),
                )
            })?;
        let evicted = update_parent_exhaustion(
            &mut self.commit_state,
            parent,
            parent_visits,
            all_duplicate,
            &rules,
        )
        .map_err(|error| {
            ClientError::new(
                ClientErrorKind::Internal,
                format!("replay exhaustion failed: {error:?}"),
            )
        })?;
        let parent_status = self
            .commit_state
            .tree
            .get(parent)
            .expect("parent in tree")
            .status;
        self.update_node(NodeUpdate {
            node_id: parent,
            status: Some(parent_status),
            progress_score: None,
            novelty_score: None,
            visit_count_delta: 1,
            expand_count_delta: 1,
            touch_visited: true,
            attrs: None,
        })
        .await?;
        if evicted.is_some() {
            self.emitter.emit(
                ts,
                "node-pruned",
                events::node_pruned_payload(parent, PruneReason::FrontierEvict, None),
            );
        }

        self.replay_commits(committed_states).await?;
        self.expansions += 1;
        self.commits_since_checkpoint += 1;
        self.emitter.emit(
            ts,
            "batch-completed",
            events::batch_completed_payload(done.seq, parent, committed_count, discarded_count),
        );
        let observed = best_committed
            .map(|(_, score)| score)
            .unwrap_or_else(|| parent_score(&self.commit_state, parent));
        let observed_node = best_committed.map_or(parent, |(node, _)| node);
        self.observe_score(observed_node, observed);
        self.maybe_rebin().await?;

        if let Some(goal) = goal_node {
            if !self.goal_nodes.contains(&goal) {
                self.goal_nodes.push(goal);
            }
            let score = self
                .commit_state
                .tree
                .get(goal)
                .map(|record| record.score.get())
                .unwrap_or_default();
            self.emitter.emit(
                ts,
                "goal-reached",
                events::goal_reached_payload(goal, score),
            );
            return Ok(Some(goal));
        }
        Ok(None)
    }

    fn register_committed_child(
        &mut self,
        node_id: NodeId,
        job: &JobResult,
        _hash: orch_core::types::StateHash,
        _cell: orch_core::types::CellKey,
    ) {
        let capture = job.capture.as_ref().expect("scorable");
        self.runtimes.insert(
            node_id,
            NodeRuntime {
                snapshot: capture.snapshot,
                class: capture.determinism_class.clone(),
            },
        );
        // register_committed_child parent context: the tree row itself.
        if let Some(record) = self.commit_state.tree.get(node_id) {
            if let Some(parent) = record.parent {
                self.node_bursts
                    .insert((parent, job.burst.burst.burst_id), node_id);
            }
        }
    }

    fn observe_score(&mut self, node: NodeId, score_value: f64) {
        let ts = self.expansions;
        let previous_best = self.best_score;
        let previous_level = self.ladder.level();
        let Ok(score) = Score::new(score_value) else {
            return;
        };
        let observation = self.stall.observe(score);
        if observation.improved
            && previous_best.is_none_or(|best| observation.best_score.get() > best)
        {
            self.best_node = Some(node);
            self.emitter.emit(
                ts,
                "best-score-improved",
                events::best_score_improved_payload(
                    node,
                    observation.best_score.get(),
                    previous_best.unwrap_or(0.0),
                ),
            );
        }
        self.best_score = Some(observation.best_score.get());
        if observation.completed_new_window {
            self.emitter.emit(
                ts,
                "stall-detected",
                events::stall_detected_payload(
                    observation.observations_since_improvement,
                    self.cfg.config.plateau.window_n,
                ),
            );
        }
        let snapshot = self.ladder.apply_stall_observation(observation);
        if snapshot.level != previous_level {
            self.emitter.emit(
                ts,
                "escalation-changed",
                events::escalation_changed_payload(snapshot.level.get(), previous_level.get()),
            );
        }
    }

    /// L4 re-bin: recompile the coarsened map, reload it on the scorer with
    /// `rebin = true`, and reset the selection-side cell mirror. One-way
    /// within an experiment; SeenMap persists; the version rides the
    /// checkpoint.
    async fn maybe_rebin(&mut self) -> ClientResult<()> {
        let target_version = self.ladder.feature_map_version();
        if target_version == self.bringup_feature_map_version() {
            return Ok(());
        }
        let map = coarsened_map(&self.sources, &self.cfg.config, target_version)?;
        let bringup = bring_up(
            &self.cfg.config,
            &self.cfg.experiment_id,
            &self.sources,
            &map,
            true,
            &self.scorer,
            &self.synth,
        )
        .await?;
        self.bringup = bringup;
        self.commit_state.cell_mirror.reset_for_rebin();
        Ok(())
    }

    fn bringup_feature_map_version(&self) -> u32 {
        // The compiled map's meta version starts at the document's version
        // and increments per coarsening pass; the ladder's counter is the
        // number of passes applied.
        self.bringup
            .compiled
            .meta
            .version
            .saturating_sub(self.sources.feature_map.meta.version)
    }

    async fn create_child_node(
        &mut self,
        node_id: NodeId,
        parent: NodeId,
        job: &JobResult,
        result: &ScoreResult,
        status: NodeStatus,
    ) -> ClientResult<()> {
        let capture = job.capture.as_ref().expect("scorable job has a capture");
        let mut attrs = OrchNodeAttrsV1::new(
            capture.machine_config_hash,
            capture.determinism_class.clone(),
            SynthContextAttrs {
                created_by_burst: Some(job.burst.clone()),
                config_fingerprint: self.fingerprint,
                decoded_features: decoded_map(&self.cfg.config, &self.bringup, result),
                frame_counter: capture.frame_counter,
                state_hash: result.state_hash,
                cell_key: result.novelty_detail.cell_key,
                stage: result.stage,
                score: result.progress_score,
                novelty: result.novelty_score,
                recent_inputs: None,
            },
        );
        if result.goal_hit {
            attrs = attrs.with_goal(true);
        }
        if status == NodeStatus::Pruned {
            attrs = attrs.with_prune_reason("exhausted");
        }
        let store = self.store.clone();
        let request = CreateNodeRequest {
            experiment_id: self.cfg.experiment_id.clone(),
            node_id,
            parent_node_id: Some(parent),
            snapshot_ref: capture.snapshot,
            input_log_id: capture.input_log_id,
            status,
            progress_score: result.progress_score,
            novelty_score: result.novelty_score,
            attrs: encode_node_attrs(&attrs)?,
            input_log_container: None,
        };
        // CreateNode is idempotent on (experiment_id, node_id): blind retry.
        match retry_rpc(&self.retry, || store.create_node(request.clone())).await {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == ClientErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn update_node(&mut self, update: NodeUpdate) -> ClientResult<()> {
        let store = self.store.clone();
        let request = UpdateNodesRequest {
            experiment_id: self.cfg.experiment_id.clone(),
            updates: vec![update],
        };
        retry_rpc(&self.retry, || store.update_nodes(request.clone()))
            .await
            .map(|_| ())
    }

    async fn score_batch(
        &mut self,
        client_batch_id: String,
        states: Vec<StateInput>,
    ) -> ClientResult<Vec<ScoreResult>> {
        let scorer = self.scorer.clone();
        let request = ScoreBatchRequest {
            experiment_id: self.cfg.experiment_id.clone(),
            states,
            archive_update: ArchiveUpdateMode::ScoreOnly,
            client_batch_id,
            return_decoded: !self.cfg.config.decoded_features.is_empty(),
        };
        let response = retry_rpc(&self.retry, || scorer.score_batch(request.clone())).await?;
        self.scorer_archive_seq = response.archive_seq;
        Ok(response.results)
    }

    async fn replay_commits(&mut self, states: Vec<CommittedState>) -> ClientResult<()> {
        if states.is_empty() {
            return Ok(());
        }
        let scorer = self.scorer.clone();
        let request = ReplayCommitsRequest {
            experiment_id: self.cfg.experiment_id.clone(),
            states,
        };
        retry_rpc(&self.retry, || scorer.replay_commits(request.clone()))
            .await
            .map(|_| ())
    }

    // ── checkpoint (§8 lockstep) ────────────────────────────────────────────

    /// The seq a checkpoint may claim as fully covered: everything below
    /// the first still-queued replay intent (or batch_seq once the replay
    /// queue has drained).
    fn effective_checkpoint_seq(&self) -> u64 {
        self.replay_queue
            .front()
            .map_or(self.batch_seq, |intent| intent.seq)
    }

    async fn maybe_checkpoint(&mut self) -> ClientResult<()> {
        let cadence = &self.cfg.config.checkpoint;
        let due_commits =
            cadence.every_commits > 0 && self.commits_since_checkpoint >= cadence.every_commits;
        let due_time = cadence.every_seconds > 0
            && self.last_checkpoint_at.elapsed()
                >= Duration::from_secs(u64::from(cadence.every_seconds));
        if due_commits || due_time {
            self.checkpoint().await?;
        }
        Ok(())
    }

    async fn checkpoint(&mut self) -> ClientResult<()> {
        // Lockstep: the C stage scores serially in this task, so there are
        // no in-flight ScoreBatch calls here by construction (drain happened
        // upstream for pause/stop paths).
        let checkpoint_id = format!("ckpt-{}", self.expansions);
        self.crash_check(CrashPoint::BeforeCheckpointArchive)?;
        let scorer = self.scorer.clone();
        let archive_request = CheckpointArchiveRequest {
            experiment_id: self.cfg.experiment_id.clone(),
            checkpoint_id: checkpoint_id.clone(),
        };
        let mut archive = retry_rpc(&self.retry, || {
            scorer.checkpoint_archive(archive_request.clone())
        })
        .await?;
        if archive.archive_seq != self.scorer_archive_seq {
            // A straggler landed: re-issue with the same id (idempotent by
            // overwrite).
            archive = retry_rpc(&self.retry, || {
                scorer.checkpoint_archive(archive_request.clone())
            })
            .await?;
            if archive.archive_seq != self.scorer_archive_seq {
                return Err(ClientError::new(
                    ClientErrorKind::FailedPrecondition,
                    format!(
                        "{REASON_ARCHIVE_SEQ_MISMATCH}: archive {} vs applied {}",
                        archive.archive_seq, self.scorer_archive_seq
                    ),
                ));
            }
        }
        self.crash_check(CrashPoint::AfterCheckpointArchive)?;

        let checkpoint =
            self.build_checkpoint(&checkpoint_id, &archive.archive_ref, archive.archive_seq)?;
        let bytes = encode_checkpoint(&checkpoint)
            .map_err(|error| ClientError::new(ClientErrorKind::Internal, error.to_string()))?;

        self.crash_check(CrashPoint::BeforeCasPut)?;
        let store = self.store.clone();
        let expected = match self.ckpt_generation {
            Some(generation) => MetadataExpectation::generation(generation),
            None => MetadataExpectation::create_only(),
        };
        let put = PutMetadataRequest {
            key: MetadataKey::checkpoint(&self.cfg.experiment_id),
            value: bytes,
            expected_generation: expected,
        };
        let response = match retry_rpc(&self.retry, || store.put_metadata(put.clone())).await {
            Ok(response) => response,
            Err(error)
                if matches!(
                    error.kind(),
                    ClientErrorKind::FailedPrecondition | ClientErrorKind::AlreadyExists
                ) =>
            {
                // Single-writer CAS: another orchestrator owns the
                // experiment; stand down.
                return Err(ClientError::new(
                    ClientErrorKind::FailedPrecondition,
                    format!("{REASON_CAS_OWNERSHIP_LOST}: {}", error.message()),
                ));
            }
            Err(error) => return Err(error),
        };
        self.ckpt_generation = Some(response.generation);
        self.crash_check(CrashPoint::AfterCasPut)?;

        // WAL truncation: the checkpoint covers every batch below the
        // effective seq. While WAL replay is still outstanding (a Pause or
        // goal checkpoint can fire mid-replay) the effective seq is capped
        // at the first un-replayed intent, so queued replays are never
        // truncated and a restart re-loads a batch_seq that still points at
        // them (review finding).
        let effective_seq = self.effective_checkpoint_seq();
        self.crash_check(CrashPoint::BeforeWalDelete)?;
        let store = self.store.clone();
        for seq in self.wal_floor..effective_seq {
            let delete = DeleteMetadataRequest {
                key: MetadataKey::wal(&self.cfg.experiment_id, seq),
                expected_generation: MetadataExpectation::unconditional(),
            };
            match retry_rpc(&self.retry, || store.delete_metadata(delete.clone())).await {
                Ok(_) => {}
                Err(error) if error.kind() == ClientErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        self.wal_floor = effective_seq;
        self.crash_check(CrashPoint::AfterWalDelete)?;

        self.checkpointed_batch_seq = effective_seq;
        self.commits_since_checkpoint = 0;
        self.last_checkpoint_at = tokio::time::Instant::now();
        self.emitter.emit(
            self.expansions,
            "checkpoint",
            events::checkpoint_payload(self.batch_seq, self.expansions, archive.archive_seq),
        );
        Ok(())
    }

    fn build_checkpoint(
        &self,
        checkpoint_id: &str,
        archive_ref: &str,
        archive_seq: u64,
    ) -> ClientResult<CheckpointV1> {
        let mut frontier_entries = Vec::new();
        for id in self.commit_state.frontier.deterministic_entries() {
            let record = self.commit_state.tree.get(id).ok_or_else(|| {
                ClientError::new(ClientErrorKind::Internal, "frontier node missing from tree")
            })?;
            frontier_entries.push(FrontierEntry {
                node: id,
                visits: record.visits,
                exhausted: record.exhausted,
                consecutive_all_dup: u16::try_from(
                    self.commit_state.all_duplicate_streak(id).unwrap_or(0),
                )
                .unwrap_or(u16::MAX),
            });
        }
        let seen = self.commit_state.seen.sorted_entries();

        let knobs = PlateauKnobs::from_plateau_config(&self.cfg.config.plateau);
        Ok(CheckpointV1 {
            version: orch_checkpoint::CHECKPOINT_VERSION,
            experiment_id: self.cfg.experiment_id.clone(),
            config_hash: self.cfg.config_hash,
            last_committed_node: NodeId::new(
                self.commit_state.tree.next_id().get().saturating_sub(1),
            ),
            batch_seq: self.effective_checkpoint_seq(),
            expansions: self.expansions,
            budgets_used: BudgetsUsed {
                nodes: self.commit_state.tree.len() as u64,
                wall_clock_s: self.started_at.elapsed().as_secs(),
                guest_instructions: self.guest_instructions_used,
                expansions: self.expansions,
            },
            rng: RngCheckpoint {
                seed: self.cfg.config.seed,
            },
            frontier: frontier_entries,
            scorer_archive_ref: archive_ref.to_owned(),
            scorer_archive_seq: archive_seq,
            scorer_checkpoint_id: checkpoint_id.to_owned(),
            feature_map_version: self.ladder.feature_map_version(),
            feature_map_hash: self.bringup.feature_map_hash,
            scoring_program_hash: self.bringup.scoring_program_hash,
            synth_config_fingerprint: self
                .fingerprint
                .map_or([0u8; 32], ConfigFingerprint::into_bytes),
            cell_mirror: self.commit_state.cell_mirror.sorted_counts(),
            seen,
            // knobs_in_effect is deliberately write-only: resume re-derives
            // knobs from the config (pinned by config_hash), and the copy in
            // the checkpoint exists for forensics — a checkpoint must be
            // interpretable without the config document (review suggestion,
            // documented rather than dropped to keep the golden format).
            plateau: PlateauCheckpoint {
                best_score: self.best_score,
                observations: self.stall.observations(),
                observations_since_improvement: self.stall.observations_since_improvement(),
                completed_stall_windows: self.stall.completed_stall_windows(),
                level: self.ladder.level().get(),
                knobs_in_effect: PlateauKnobsInEffect {
                    window_n: knobs.window_n,
                    epsilon_s: knobs.epsilon_s,
                    burst_len_factor: knobs.ladder.burst_len_factor,
                    temp_factor: knobs.ladder.temp_factor,
                    macro_weight_hot: knobs.ladder.macro_weight_hot,
                    backtrack_kappa: knobs.ladder.backtrack_kappa,
                    backtrack_depth_quantile: knobs.ladder.backtrack_depth_quantile,
                    radius_factor: knobs.ladder.radius_factor,
                    max_level: knobs.ladder.max_level.get(),
                },
            },
            goal_nodes: self.goal_nodes.clone(),
            status: self.status,
        })
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn cap_from(value: u64) -> Option<GuestInstructions> {
    (value > 0).then(|| GuestInstructions::new(value))
}

fn parent_score(state: &CommitState, parent: NodeId) -> f64 {
    state
        .tree
        .get(parent)
        .map(|record| record.score.get())
        .unwrap_or_default()
}

fn payload_from_store(meta: &NodeMeta, attrs: &OrchNodeAttrsV1) -> NodePayload {
    NodePayload::new(
        meta.snapshot_ref,
        meta.progress_score,
        meta.novelty_score,
        attrs.synth.cell_key,
        attrs.synth.state_hash,
        attrs.synth.stage,
        attrs.synth.frame_counter,
    )
}

fn coarsened_map(
    sources: &ExperimentSources,
    config: &ExperimentConfig,
    version: u32,
) -> ClientResult<orch_core::compile::FeatureMap> {
    use orch_core::compile::{coarsen_l4_preserving_layout, RadiusFactor};
    let ladder = &config.plateau.ladder;
    let radius = RadiusFactor {
        numerator: (ladder.radius_factor.max(0.0) * 1000.0) as u64,
        denominator: std::num::NonZeroU64::new(1000).expect("nonzero"),
    };
    let mut map = sources.feature_map.clone();
    for _ in 0..version {
        map = coarsen_l4_preserving_layout(&map, &sources.region_layouts, radius).map_err(
            |error| {
                ClientError::new(
                    ClientErrorKind::Internal,
                    format!("L4 coarsening failed: {error}"),
                )
            },
        )?;
    }
    Ok(map)
}

fn decoded_pairs(
    config: &ExperimentConfig,
    bringup: &BringupOutcome,
    result: &ScoreResult,
) -> Vec<(String, f64)> {
    let mut pairs = Vec::new();
    for (field, decoded) in bringup.compiled.fields.iter().zip(&result.decoded) {
        if !config
            .decoded_features
            .iter()
            .any(|name| name == &field.name)
        {
            continue;
        }
        if let DecodedValue::Number(value) = decoded {
            pairs.push((field.name.clone(), value.get()));
        }
    }
    pairs
}

fn decoded_map(
    config: &ExperimentConfig,
    bringup: &BringupOutcome,
    result: &ScoreResult,
) -> BTreeMap<String, FiniteF64> {
    decoded_pairs(config, bringup, result)
        .into_iter()
        .filter_map(|(name, value)| FiniteF64::new(value).ok().map(|value| (name, value)))
        .collect()
}

/// Config-driven policy dispatch (ARCHITECTURE.md §5.1's build_policy).
enum PolicyBox {
    Softmax(SoftmaxPolicy),
    Ucb(UcbPolicy),
    Staged(StagedPolicy),
}

impl PolicyBox {
    fn from_config(config: &ExperimentConfig) -> Self {
        match config.selection.policy {
            orch_core::types::PolicyKind::Softmax => Self::Softmax(SoftmaxPolicy::new()),
            orch_core::types::PolicyKind::Ucb => Self::Ucb(UcbPolicy::new()),
            orch_core::types::PolicyKind::Staged => Self::Staged(StagedPolicy::new()),
        }
    }

    fn set_total_expansions(&mut self, total: u64) {
        if let Self::Staged(policy) = self {
            policy.set_total_expansions(total);
        }
    }

    fn select(
        &mut self,
        context: &PolicyContext<'_>,
        rng: &mut DeterministicRng,
    ) -> Result<orch_core::policy::SelectionChoice, PolicyError> {
        match self {
            Self::Softmax(policy) => policy.select(context, rng),
            Self::Ucb(policy) => policy.select(context, rng),
            Self::Staged(policy) => policy.select(context, rng),
        }
    }
}
