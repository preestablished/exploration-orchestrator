//! Shared harness for the M3 acceptance suite: fake-backed adapters, the
//! fixed-action grid burst set, a latency probe replaying fake fault plans,
//! and a mini select->expand->score->commit search over the pipeline
//! (`search_loop.rs` semantics, driven through orch-sched).

// Each integration-test binary compiles this module separately and uses a
// different subset of it.
#![allow(dead_code)]

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use orch_clients::{
    hypervisor::{
        BootSpec, CaptureSpec, Digest32, ElfBoot, ExtractRange, HashEpochs, MachineConfig,
    },
    input_synth::{
        Burst, BurstBody, BurstId, ConfigFingerprint, GeneratorKind, PadBurst, PadSegment,
        Provenance, ProvenancedBurst,
    },
    scorer::{
        ArchiveUpdateMode, ArtifactSource, CommittedState, CompiledLayout,
        ExtractRange as ScorerExtractRange, LoadFeatureMapRequest, LoadScoringProgramRequest,
        ReplayCommitsRequest, ScoreBatchRequest, ScoreResult, StateInput,
    },
    snapshot_store::{CreateNodeRequest, NodeUpdate, UpdateNodesRequest},
    ClientResult,
};
use orch_core::{
    commit::{commit_batch, CommitRules, CommitState, ScoredChild},
    plateau::PlateauKnobs,
    policy::{staged::StagedPolicy, PolicyContext, SelectionPolicy},
    rng::DeterministicRng,
    tree::NodePayload,
    types::{
        FrameCount, GuestInstructions, NodeId, NodeStatus, Novelty, PolicyKind, SchedMode,
        SelectionConfig, SnapshotRef, StateHash,
    },
};
use orch_driver::node_attrs::{encode_node_attrs, OrchNodeAttrsV1, SynthContextAttrs};
use orch_fakes::{
    fault::{FaultInjector, FaultPlan, FaultRequest, FaultTarget},
    hypervisor::FakeHypervisor,
    scorer::{FakeScorer, GRID_FEATURE_BYTES_LEN},
    snapshot_store::InMemorySnapshotStore,
};
use orch_sched::{
    driver::{BootstrapSpec, DriverConfig, JobResult, RootCapture, WorkerDriver},
    pipeline::{Batch, JobOutcome, Pipeline, PipelineConfig},
    ports::{LatencyProbe, PendingCall, SyncAdapter},
    retry::{retry_rpc, RetryPolicy},
    slots::{SlotView, SlotViewConfig},
};
use tokio::sync::Mutex;

pub const EXPERIMENT_ID: &str = "m3-acceptance";

pub const BUTTON_ATTACK_A: u32 = 0b0000_0001;
pub const BUTTON_UP: u32 = 0b0100_0000;
pub const BUTTON_DOWN: u32 = 0b1000_0000;
pub const BUTTON_LEFT: u32 = 0b1_0000_0000;
pub const BUTTON_RIGHT: u32 = 0b10_0000_0000;

/// The six grid actions as one-frame pad bursts (search_loop.rs semantics:
/// one event, one frame per candidate).
pub const ACTION_BUTTONS: [u32; 6] = [
    BUTTON_UP,
    BUTTON_RIGHT,
    BUTTON_DOWN,
    BUTTON_LEFT,
    BUTTON_ATTACK_A,
    0, // Wait
];

pub fn machine_config() -> MachineConfig {
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

pub fn bootstrap_spec() -> BootstrapSpec {
    BootstrapSpec {
        machine_config: machine_config(),
        bootstrap_icount_cap: Some(GuestInstructions::new(10_000_000)),
    }
}

pub fn grid_capture() -> CaptureSpec {
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

pub fn pad_burst(slot: u32, buttons: u32, hold_frames: u32) -> ProvenancedBurst {
    let mut id = [0u8; 32];
    id[0] = slot as u8;
    id[1] = (buttons & 0xFF) as u8;
    id[2] = ((buttons >> 8) & 0xFF) as u8;
    id[3] = hold_frames as u8;
    ProvenancedBurst {
        burst: Burst {
            format_version: 1,
            burst_id: BurstId::new(id),
            body: BurstBody::Pad(PadBurst {
                segments: vec![PadSegment {
                    buttons,
                    hold_frames: FrameCount::new(hold_frames),
                }],
                button_alphabet: "console16-12btn-v1".to_owned(),
            }),
        },
        provenance: Provenance {
            generator: GeneratorKind::WeightedRandom,
            slot,
            rng_stream: format!("slot/{slot}/m3"),
            config_fingerprint: ConfigFingerprint::new([0xA5; 32]),
            fallback_from: None,
            macro_provenance: None,
            mutation_provenance: None,
            policy_provenance: None,
        },
    }
}

/// The six fixed action bursts for one expansion.
pub fn action_bursts() -> Vec<ProvenancedBurst> {
    ACTION_BUTTONS
        .iter()
        .enumerate()
        .map(|(slot, &buttons)| pad_burst(slot as u32, buttons, 1))
        .collect()
}

/// Latency probe replaying a fake fault plan's latency channel: a cloned
/// injector draws from the same deterministic distribution the fake uses,
/// scoped to the configured operations.
pub struct PlanProbe {
    injector: FaultInjector,
    target: FaultTarget,
    operations: BTreeSet<&'static str>,
    charge_timeouts: bool,
}

impl PlanProbe {
    pub fn hypervisor(plan: FaultPlan, operations: &[&'static str]) -> Self {
        Self {
            injector: FaultInjector::new(plan),
            target: FaultTarget::Hypervisor,
            operations: operations.iter().copied().collect(),
            charge_timeouts: true,
        }
    }

    pub fn scorer(plan: FaultPlan, operations: &[&'static str]) -> Self {
        Self {
            injector: FaultInjector::new(plan),
            target: FaultTarget::Scorer,
            operations: operations.iter().copied().collect(),
            charge_timeouts: true,
        }
    }
}

impl LatencyProbe for PlanProbe {
    fn pending_call(&mut self, operation: &'static str, request_identity: &[u8]) -> PendingCall {
        if !self.operations.contains(operation) {
            return PendingCall::default();
        }
        let decision = self.injector.decide(
            FaultRequest::new(self.target, operation, request_identity),
            0,
        );
        PendingCall {
            latency_ticks: decision.latency_ticks,
            timeout: self.charge_timeouts
                && matches!(decision.terminal, orch_fakes::fault::FaultTerminal::Timeout),
        }
    }
}

pub struct Harness {
    pub hypervisor: SyncAdapter<FakeHypervisor>,
    pub scorer: SyncAdapter<FakeScorer>,
    pub store: SyncAdapter<InMemorySnapshotStore>,
    pub driver: WorkerDriver<SyncAdapter<FakeHypervisor>>,
    pub slots: SlotView,
    pub drain: tokio::task::JoinHandle<()>,
}

pub struct HarnessSpec {
    pub slots: u32,
    pub experiment_seed: u64,
    pub hypervisor_plan: FaultPlan,
    pub scorer_plan: FaultPlan,
    pub hypervisor_probe: Option<PlanProbe>,
    pub scorer_probe: Option<PlanProbe>,
    pub drain_interval: Duration,
}

impl Default for HarnessSpec {
    fn default() -> Self {
        Self {
            slots: 8,
            experiment_seed: 0x5EED,
            hypervisor_plan: FaultPlan::disabled(0),
            scorer_plan: FaultPlan::disabled(0),
            hypervisor_probe: None,
            scorer_probe: None,
            drain_interval: Duration::from_millis(5),
        }
    }
}

pub async fn harness(spec: HarnessSpec) -> Harness {
    let mut hypervisor = SyncAdapter::new(FakeHypervisor::with_slots_and_fault_plan(
        spec.slots,
        spec.hypervisor_plan,
    ));
    if let Some(probe) = spec.hypervisor_probe {
        hypervisor = hypervisor.with_probe(probe);
    }
    let mut scorer = SyncAdapter::new(FakeScorer::with_fault_plan(spec.scorer_plan));
    {
        // Configure the scorer's grid feature map + program. The scorer's
        // own fault plan may make individual load attempts draw injected
        // errors; the attempt salt clears them on retry.
        use orch_clients::scorer::StateScorerClient;
        let service = scorer.service();
        let mut service = service.lock().await;
        let mut attempts = 0;
        loop {
            let result = service.load_feature_map(LoadFeatureMapRequest {
                experiment_id: EXPERIMENT_ID.to_owned(),
                source: ArtifactSource::InlineYaml(b"feature-map: m3\n".to_vec()),
                layout: CompiledLayout {
                    ranges: vec![ScorerExtractRange {
                        region: "grid".to_owned(),
                        layout_version: 1,
                        offset: 0,
                        len: GRID_FEATURE_BYTES_LEN,
                    }],
                },
                frame: None,
                rebin: false,
            });
            match result {
                Ok(_) => break,
                Err(_) if attempts < 32 => attempts += 1,
                Err(error) => panic!("feature map load kept failing: {error}"),
            }
        }
        let mut attempts = 0;
        loop {
            let result = service.load_scoring_program(LoadScoringProgramRequest {
                experiment_id: EXPERIMENT_ID.to_owned(),
                source: ArtifactSource::InlineYaml(b"score: m3\n".to_vec()),
            });
            match result {
                Ok(_) => break,
                Err(_) if attempts < 32 => attempts += 1,
                Err(error) => panic!("scoring program load kept failing: {error}"),
            }
        }
    }
    if let Some(probe) = spec.scorer_probe {
        scorer = scorer.with_probe(probe);
    }
    let store = SyncAdapter::new(InMemorySnapshotStore::new());

    // Under injected fault plans the seeding worker_info/list_slots calls
    // can draw errors; retry like any transient RPC.
    let policy = RetryPolicy {
        job_timeout: Duration::from_secs(120),
        retry_max: 16,
        backoff_base: Duration::from_millis(1),
    };
    let (slots, drain) = retry_rpc(&policy, || {
        SlotView::start(
            hypervisor.clone(),
            SlotViewConfig {
                drain_interval: spec.drain_interval,
                allow_class_mismatch: false,
            },
        )
    })
    .await
    .expect("slot view starts");

    let driver = WorkerDriver::new(
        hypervisor.clone(),
        slots.clone(),
        DriverConfig {
            experiment_seed: spec.experiment_seed,
            capture: grid_capture(),
            hard_icount_cap: None,
        },
    );

    Harness {
        hypervisor,
        scorer,
        store,
        driver,
        slots,
        drain,
    }
}

fn scorer_from(adapter: &SyncAdapter<FakeScorer>) -> Arc<Mutex<FakeScorer>> {
    adapter.service()
}

/// Full mini search over the pipeline (retry-equivalence + seed-gate
/// vehicle). Deterministic mode, fixed six-action bursts, FakeScorer
/// scoring, orch-core commit, store writes.
pub struct SearchOutcome {
    pub expansions: u64,
    pub goal: NodeId,
    pub tree_hash: [u8; 32],
    /// Commit-order transcript: (batch seq, committed node ids, state hashes).
    pub transcript: Vec<(u64, Vec<(NodeId, StateHash)>)>,
}

struct NodeRuntime {
    snapshot: SnapshotRef,
    frame_counter: FrameCount,
}

pub async fn run_search(
    harness: &Harness,
    seed: u64,
    retry: RetryPolicy,
    max_expansions: u64,
) -> ClientResult<SearchOutcome> {
    let boot = bootstrap_spec();
    let root = retry_rpc(&retry, || harness.driver.bootstrap(&boot)).await?;

    let scorer = scorer_from(&harness.scorer);
    let root_result = score_batch_with_retry(
        &scorer,
        &retry,
        "root",
        vec![StateInput {
            node_ref: "root".to_owned(),
            feature_bytes: root.feature_bytes.clone().expect("root features"),
            framebuffer: None,
            fb_meta: None,
        }],
    )
    .await?
    .remove(0);

    let root_payload = NodePayload::new(
        root.snapshot,
        root_result.progress_score,
        root_result.novelty_score,
        root_result.novelty_detail.cell_key,
        root_result.state_hash,
        root_result.stage,
        root.frame_counter,
    );
    let mut commit_state = CommitState::from_root(root_payload);
    create_store_node(harness, NodeId::ROOT, None, &root, &root_payload).await?;
    replay(
        &scorer,
        vec![CommittedState {
            state_hash: root_result.state_hash,
            cell_key: root_result.novelty_detail.cell_key,
        }],
    )
    .await;

    let mut runtimes = std::collections::BTreeMap::from([(
        NodeId::ROOT,
        NodeRuntime {
            snapshot: root.snapshot,
            frame_counter: root.frame_counter,
        },
    )]);
    let mut archive = ArchiveMirror::default();
    archive.replay(&[CommittedState {
        state_hash: root_result.state_hash,
        cell_key: root_result.novelty_detail.cell_key,
    }]);

    let mut pipeline = Pipeline::spawn(
        harness.driver.clone(),
        PipelineConfig {
            mode: SchedMode::Deterministic,
            max_inflight_batches: 1,
            retry,
        },
        0,
    );

    let mut rng = DeterministicRng::selection(seed, 0);
    let mut policy = StagedPolicy::new();
    let plateau = PlateauKnobs::from_plateau_config(&Default::default());
    let selection = selection_config();
    let rules = CommitRules::new(orch_core::types::PruneAction::Drop, 2_000.0, 1, 1);
    let mut transcript = Vec::new();

    for expansion in 0..max_expansions {
        policy.set_total_expansions(expansion);
        let selected = {
            let context = PolicyContext::new(
                &commit_state.tree,
                &commit_state.frontier,
                &commit_state.cell_mirror,
                &plateau,
                &selection,
            );
            policy
                .select(&context, &mut rng)
                .expect("select parent")
                .selected
        };
        let runtime = runtimes.get(&selected).expect("runtime for parent");

        pipeline
            .submit(Batch {
                seq: expansion,
                parent: selected,
                parent_snapshot: runtime.snapshot,
                required_class: None,
                bursts: action_bursts(),
            })
            .await?;
        let result = pipeline
            .next_completed()
            .await?
            .expect("pipeline yields the batch");

        let jobs: Vec<&JobResult> = result
            .jobs
            .iter()
            .map(|job| match job {
                JobOutcome::Completed(job) => job.as_ref(),
                JobOutcome::Abandoned { job_idx, reason } => {
                    panic!("job {job_idx} abandoned in deterministic search: {reason}")
                }
            })
            .collect();

        let score_results = score_batch_with_retry(
            &scorer,
            &retry,
            format!("b{expansion}"),
            jobs.iter()
                .map(|job| StateInput {
                    node_ref: format!("b{expansion}-j{}", job.job_idx),
                    feature_bytes: job
                        .capture
                        .as_ref()
                        .expect("capture")
                        .feature_bytes
                        .clone()
                        .expect("features"),
                    framebuffer: None,
                    fb_meta: None,
                })
                .collect(),
        )
        .await?;

        let mut sibling_hashes = BTreeSet::new();
        let children: Vec<ScoredChild> = jobs
            .iter()
            .zip(&score_results)
            .map(|(job, result)| {
                scored_child(&commit_state, &archive, &mut sibling_hashes, job, result)
            })
            .collect();
        let outcome =
            commit_batch(&mut commit_state, selected, &children, &rules).expect("commit batch");
        update_expanded_parent(harness, selected, &commit_state).await?;

        let mut committed = Vec::new();
        let mut committed_transcript = Vec::new();
        for (job, child_commit) in jobs.iter().zip(&outcome.child_commits) {
            let Some(node_id) = child_commit.node_id else {
                continue;
            };
            let record = commit_state.tree.get(node_id).expect("committed child");
            let payload = record.payload();
            let capture = job.capture.as_ref().expect("capture");
            create_child_store_node(harness, node_id, selected, job, &payload, record.status)
                .await?;
            runtimes.insert(
                node_id,
                NodeRuntime {
                    snapshot: capture.snapshot,
                    frame_counter: capture.frame_counter,
                },
            );
            committed.push(CommittedState {
                state_hash: payload.state_hash,
                cell_key: payload.cell,
            });
            committed_transcript.push((node_id, payload.state_hash));
        }
        archive.replay(&committed);
        replay(&scorer, committed).await;
        transcript.push((expansion, committed_transcript));

        if let Some(goal) = outcome.goal_node {
            return Ok(SearchOutcome {
                expansions: expansion + 1,
                goal,
                tree_hash: tree_hash(&commit_state),
                transcript,
            });
        }
    }
    panic!("search failed to reach the goal within {max_expansions} expansions");
}

#[derive(Default)]
struct ArchiveMirror {
    seen: BTreeSet<StateHash>,
    cell_counts: std::collections::BTreeMap<orch_core::types::CellKey, u32>,
}

impl ArchiveMirror {
    fn replay(&mut self, states: &[CommittedState]) {
        for state in states {
            if self.seen.insert(state.state_hash) {
                *self.cell_counts.entry(state.cell_key).or_default() += 1;
            }
        }
    }
}

fn scored_child(
    commit_state: &CommitState,
    archive: &ArchiveMirror,
    sibling_hashes: &mut BTreeSet<StateHash>,
    job: &JobResult,
    result: &ScoreResult,
) -> ScoredChild {
    assert_eq!(result.error, None);
    let capture = job.capture.as_ref().expect("capture");
    assert_eq!(result.state_hash, capture.state_hash);

    let sibling_duplicate = !sibling_hashes.insert(result.state_hash);
    assert_eq!(result.duplicate, archive.seen.contains(&result.state_hash));
    let duplicate = result.duplicate || sibling_duplicate;
    let novelty = Novelty::new(
        commit_state
            .cell_mirror
            .novelty(result.novelty_detail.cell_key),
    )
    .expect("finite novelty");
    let payload = NodePayload::new(
        capture.snapshot,
        result.progress_score,
        novelty,
        result.novelty_detail.cell_key,
        result.state_hash,
        result.stage,
        capture.frame_counter,
    );
    let mut child = ScoredChild::new(payload);
    if duplicate {
        child = child.duplicate();
    }
    if result.prune {
        child = child.prune();
    }
    if result.goal_hit {
        child = child.goal();
    }
    child
}

fn selection_config() -> SelectionConfig {
    SelectionConfig {
        policy: PolicyKind::Staged,
        staged: orch_core::types::StagedConfig {
            inner: PolicyKind::Softmax,
            epsilon_regress: 0.15,
        },
        temperature: 8.0,
        ucb_c: 1.0,
        max_visits_per_node: 1,
        exhaust_after_dup_expansions: 1,
        ..SelectionConfig::default()
    }
}

/// Canonical tree hash: blake3 over (node_id, parent, state_hash, score,
/// cell_key) in id order.
pub fn tree_hash(commit_state: &CommitState) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    let next = commit_state.tree.next_id().get();
    for id in 0..next {
        let Some(record) = commit_state.tree.get(NodeId::new(id)) else {
            continue;
        };
        let payload = record.payload();
        hasher.update(&id.to_le_bytes());
        hasher.update(&record.parent.map_or(u64::MAX, NodeId::get).to_le_bytes());
        hasher.update(payload.state_hash.as_bytes());
        hasher.update(&payload.score.get().to_le_bytes());
        hasher.update(&payload.cell.get().to_le_bytes());
    }
    *hasher.finalize().as_bytes()
}

pub async fn score_batch_with_retry(
    scorer: &Arc<Mutex<FakeScorer>>,
    retry: &RetryPolicy,
    client_batch_id: impl Into<String>,
    states: Vec<StateInput>,
) -> ClientResult<Vec<ScoreResult>> {
    use orch_clients::scorer::StateScorerClient;
    let client_batch_id = client_batch_id.into();
    let response = retry_rpc(retry, || {
        let scorer = Arc::clone(scorer);
        let request = ScoreBatchRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            states: states.clone(),
            archive_update: ArchiveUpdateMode::ScoreOnly,
            client_batch_id: client_batch_id.clone(),
            return_decoded: false,
        };
        async move { scorer.lock().await.score_batch(request) }
    })
    .await?;
    Ok(response.results)
}

async fn replay(scorer: &Arc<Mutex<FakeScorer>>, states: Vec<CommittedState>) {
    use orch_clients::scorer::StateScorerClient;
    if states.is_empty() {
        return;
    }
    // Blind retry is safe: replaying committed states is idempotent
    // (seen-set inserts).
    let mut attempts = 0;
    loop {
        let result = scorer.lock().await.replay_commits(ReplayCommitsRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            states: states.clone(),
        });
        match result {
            Ok(_) => return,
            Err(_) if attempts < 32 => attempts += 1,
            Err(error) => panic!("replay commits kept failing: {error}"),
        }
    }
}

async fn create_store_node(
    harness: &Harness,
    node_id: NodeId,
    parent: Option<NodeId>,
    root: &RootCapture,
    payload: &NodePayload,
) -> ClientResult<()> {
    use orch_clients::snapshot_store::SnapshotStoreClient;
    let attrs = OrchNodeAttrsV1::new(
        root.machine_config_hash,
        root.determinism_class.clone(),
        SynthContextAttrs {
            created_by_burst: None,
            config_fingerprint: None,
            decoded_features: Default::default(),
            frame_counter: root.frame_counter,
            state_hash: payload.state_hash,
            cell_key: payload.cell,
            stage: payload.stage,
            score: payload.score,
            novelty: payload.novelty_at_commit,
            recent_inputs: None,
        },
    );
    harness
        .store
        .service()
        .lock()
        .await
        .create_node(CreateNodeRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id,
            parent_node_id: parent,
            snapshot_ref: root.snapshot,
            input_log_id: None,
            status: NodeStatus::Frontier,
            progress_score: payload.score,
            novelty_score: payload.novelty_at_commit,
            attrs: encode_node_attrs(&attrs).expect("encode attrs"),
            input_log_container: None,
        })
        .map(|_| ())
}

async fn create_child_store_node(
    harness: &Harness,
    node_id: NodeId,
    parent: NodeId,
    job: &JobResult,
    payload: &NodePayload,
    status: NodeStatus,
) -> ClientResult<()> {
    use orch_clients::snapshot_store::SnapshotStoreClient;
    let capture = job.capture.as_ref().expect("capture");
    let attrs = OrchNodeAttrsV1::new(
        capture.machine_config_hash,
        capture.determinism_class.clone(),
        SynthContextAttrs {
            created_by_burst: Some(job.burst.clone()),
            config_fingerprint: Some(job.burst.provenance.config_fingerprint),
            decoded_features: Default::default(),
            frame_counter: capture.frame_counter,
            state_hash: payload.state_hash,
            cell_key: payload.cell,
            stage: payload.stage,
            score: payload.score,
            novelty: payload.novelty_at_commit,
            recent_inputs: None,
        },
    );
    harness
        .store
        .service()
        .lock()
        .await
        .create_node(CreateNodeRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id,
            parent_node_id: Some(parent),
            snapshot_ref: capture.snapshot,
            input_log_id: capture.input_log_id,
            status,
            progress_score: payload.score,
            novelty_score: payload.novelty_at_commit,
            attrs: encode_node_attrs(&attrs).expect("encode attrs"),
            input_log_container: None,
        })
        .map(|_| ())
}

async fn update_expanded_parent(
    harness: &Harness,
    parent: NodeId,
    commit_state: &CommitState,
) -> ClientResult<()> {
    use orch_clients::snapshot_store::SnapshotStoreClient;
    let record = commit_state.tree.get(parent).expect("expanded parent");
    harness
        .store
        .service()
        .lock()
        .await
        .update_nodes(UpdateNodesRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            updates: vec![NodeUpdate {
                node_id: parent,
                status: Some(record.status),
                progress_score: None,
                novelty_score: None,
                visit_count_delta: 1,
                expand_count_delta: 1,
                touch_visited: true,
                attrs: None,
            }],
        })
        .map(|_| ())
}
