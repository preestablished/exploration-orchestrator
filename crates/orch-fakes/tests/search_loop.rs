use std::collections::{BTreeMap, BTreeSet};

use orch_clients::{
    hypervisor::{
        BootSpec, CaptureSpec, CreateVmRequest, DestroyVmRequest, Digest32 as HypervisorDigest32,
        ElfBoot, EntropySeed, ExtractRange as HypervisorExtractRange, HashEpochs,
        HypervisorWorkerClient, InjectInputsRequest, InputEvent, MachineConfig, PadSet,
        RestoreSnapshotRequest, RunRequest, RunUntil, ScheduleAt, ScheduledEvent,
        TakeSnapshotRequest,
    },
    scorer::{
        ArchiveUpdateMode, ArtifactSource, CommittedState, CompiledLayout,
        ExtractRange as ScorerExtractRange, LoadFeatureMapRequest, LoadScoringProgramRequest,
        ReplayCommitsRequest, ScoreBatchRequest, ScoreResult, StateInput, StateScorerClient,
    },
    snapshot_store::{
        CreateNodeRequest, GetPathRequest, InputLogId, NodeAttrs, NodeUpdate, SnapshotStoreClient,
        UpdateNodesRequest,
    },
};
use orch_core::{
    commit::{commit_batch, CommitRules, CommitState, ScoredChild},
    plateau::PlateauKnobs,
    policy::{staged::StagedPolicy, PolicyContext, SelectionPolicy},
    rng::DeterministicRng,
    tree::NodePayload,
    types::{
        CellKey, FrameCount, GuestInstructions, NodeId, NodeStatus, Novelty, PolicyKind,
        PruneAction, SelectionConfig, SnapshotRef, StateHash,
    },
};
use orch_fakes::{
    grid::{GridAction, GridState, StepOutcome},
    hypervisor::FakeHypervisor,
    scorer::{encode_grid_features, FakeScorer, GRID_FEATURE_BYTES_LEN},
    snapshot_store::InMemorySnapshotStore,
    transcript::{TranscriptBuilder, TranscriptHash},
};

const EXPERIMENT_ID: &str = "search-loop-exp";
const MAX_EXPANSIONS: u64 = 10_000;
const SEARCH_EXPANSION_BOUND: u64 = 512;
const BUTTON_ATTACK_A: u32 = 0b0000_0001;
const BUTTON_UP: u32 = 0b0100_0000;
const BUTTON_DOWN: u32 = 0b1000_0000;
const BUTTON_LEFT: u32 = 0b1_0000_0000;
const BUTTON_RIGHT: u32 = 0b10_0000_0000;
const ACTIONS: [GridAction; 6] = [
    GridAction::Up,
    GridAction::Right,
    GridAction::Down,
    GridAction::Left,
    GridAction::Attack,
    GridAction::Wait,
];

#[test]
fn search_loop_solves_three_room_fake_world_deterministically() {
    let first = run_search(0x5EED);
    let second = run_search(0x5EED);
    let different_seed = run_search(0x5EED + 1);

    assert_eq!(first.transcript_bytes, second.transcript_bytes);
    assert_eq!(
        first.transcript_hash.as_bytes(),
        second.transcript_hash.as_bytes()
    );
    assert_ne!(first.transcript_bytes, different_seed.transcript_bytes);
    assert_ne!(
        first.transcript_hash.as_bytes(),
        different_seed.transcript_hash.as_bytes()
    );
    assert!(first.expansions < MAX_EXPANSIONS);
    assert_eq!(first.expansions, second.expansions);
    assert!(
        first.expansions <= SEARCH_EXPANSION_BOUND,
        "same-seed run took {} expansions",
        first.expansions
    );
    assert!(
        different_seed.expansions <= SEARCH_EXPANSION_BOUND,
        "different-seed run took {} expansions",
        different_seed.expansions
    );
    assert!(first.goal_state.goal_reached());
    assert_eq!(first.goal_path_len, second.goal_path_len);
}

#[derive(Clone, Debug)]
struct NodeRuntime {
    snapshot: SnapshotRef,
    state: GridState,
    frame_counter: FrameCount,
}

#[derive(Clone, Debug)]
struct CandidateExecution {
    action: GridAction,
    before: GridState,
    after: GridState,
    outcome: StepOutcome,
    snapshot: SnapshotRef,
    input_log_id: InputLogId,
    frame_counter: FrameCount,
    feature_bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct StoreNodeSpec {
    node_id: NodeId,
    parent_node_id: Option<NodeId>,
    snapshot_ref: SnapshotRef,
    input_log_id: Option<InputLogId>,
    status: NodeStatus,
    attrs: Vec<u8>,
}

#[derive(Clone, Debug)]
struct SearchRun {
    expansions: u64,
    transcript_hash: TranscriptHash,
    transcript_bytes: Vec<u8>,
    goal_state: GridState,
    goal_path_len: usize,
}

#[derive(Clone, Debug, Default)]
struct ScorerArchiveMirror {
    seen: BTreeSet<StateHash>,
    cell_counts: BTreeMap<CellKey, u32>,
}

impl ScorerArchiveMirror {
    fn replay(&mut self, states: &[CommittedState]) {
        for state in states {
            if self.seen.insert(state.state_hash) {
                *self.cell_counts.entry(state.cell_key).or_default() += 1;
            }
        }
    }

    fn contains(&self, state_hash: StateHash) -> bool {
        self.seen.contains(&state_hash)
    }

    fn cell_count(&self, cell_key: CellKey) -> u32 {
        self.cell_counts.get(&cell_key).copied().unwrap_or(0)
    }
}

fn run_search(seed: u64) -> SearchRun {
    let mut hypervisor = FakeHypervisor::with_slots(16);
    let mut scorer = configured_scorer();
    let mut store = InMemorySnapshotStore::new();
    let mut scorer_archive = ScorerArchiveMirror::default();
    let mut transcript = TranscriptBuilder::new(seed);

    let (root_runtime, root_result) = create_root(&mut hypervisor, &mut scorer, seed);
    transcript.append_state(root_runtime.state);
    let root_commit = CommittedState {
        state_hash: root_result.state_hash,
        cell_key: root_result.novelty_detail.cell_key,
    };
    replay_commits(&mut scorer, vec![root_commit]);
    scorer_archive.replay(&[root_commit]);

    let root_payload = payload_from_score(
        root_runtime.snapshot,
        root_runtime.frame_counter,
        &root_result,
        root_result.novelty_score,
    );
    let mut commit_state = CommitState::from_root(root_payload);
    create_store_node(
        &mut store,
        StoreNodeSpec {
            node_id: NodeId::ROOT,
            parent_node_id: None,
            snapshot_ref: root_runtime.snapshot,
            input_log_id: None,
            status: NodeStatus::Frontier,
            attrs: Vec::new(),
        },
        &root_payload,
    );

    let mut runtimes = BTreeMap::from([(NodeId::ROOT, root_runtime)]);
    let mut rng = DeterministicRng::selection(seed, 0);
    let mut policy = StagedPolicy::new();
    let plateau = PlateauKnobs::from_plateau_config(&Default::default());
    let selection = selection_config();
    let rules = CommitRules::new(PruneAction::Drop, 2_000.0, 1, 1);

    for expansions in 0..MAX_EXPANSIONS {
        policy.set_total_expansions(expansions);
        let context = PolicyContext::new(
            &commit_state.tree,
            &commit_state.frontier,
            &commit_state.cell_mirror,
            &plateau,
            &selection,
        );
        let choice = policy.select(&context, &mut rng).expect("select parent");
        let parent_runtime = runtimes
            .get(&choice.selected)
            .cloned()
            .expect("runtime for selected parent");
        let executions = expand_parent(
            &mut hypervisor,
            seed,
            expansions,
            choice.selected,
            &parent_runtime,
            &mut transcript,
        );
        let score_response = scorer
            .score_batch(ScoreBatchRequest {
                experiment_id: EXPERIMENT_ID.to_owned(),
                states: executions
                    .iter()
                    .enumerate()
                    .map(|(index, execution)| StateInput {
                        node_ref: format!(
                            "expand-{expansions}-node-{}-candidate-{index}",
                            choice.selected.get()
                        ),
                        feature_bytes: execution.feature_bytes.clone(),
                        framebuffer: None,
                        fb_meta: None,
                    })
                    .collect(),
                archive_update: ArchiveUpdateMode::ScoreOnly,
                client_batch_id: format!("expand-{expansions}-node-{}", choice.selected.get()),
                return_decoded: false,
            })
            .expect("score expansion");
        assert_eq!(score_response.results.len(), executions.len());

        let mut sibling_hashes = BTreeSet::new();
        let children = executions
            .iter()
            .zip(&score_response.results)
            .map(|(execution, result)| {
                scored_child(
                    &commit_state,
                    &scorer_archive,
                    &mut sibling_hashes,
                    execution,
                    result,
                )
            })
            .collect::<Vec<_>>();
        let outcome = commit_batch(&mut commit_state, choice.selected, &children, &rules)
            .expect("commit expansion");
        update_expanded_parent(&mut store, choice.selected, &commit_state);

        let mut committed = Vec::new();
        for (execution, child_commit) in executions.iter().zip(&outcome.child_commits) {
            let Some(node_id) = child_commit.node_id else {
                continue;
            };
            let record = commit_state.tree.get(node_id).expect("committed child");
            let payload = record.payload();
            create_store_node(
                &mut store,
                StoreNodeSpec {
                    node_id,
                    parent_node_id: Some(choice.selected),
                    snapshot_ref: execution.snapshot,
                    input_log_id: Some(execution.input_log_id),
                    status: record.status,
                    attrs: node_attrs(execution.action, execution.outcome),
                },
                &payload,
            );
            runtimes.insert(
                node_id,
                NodeRuntime {
                    snapshot: execution.snapshot,
                    state: execution.after,
                    frame_counter: execution.frame_counter,
                },
            );
            committed.push(CommittedState {
                state_hash: payload.state_hash,
                cell_key: payload.cell,
            });
        }
        scorer_archive.replay(&committed);
        replay_commits(&mut scorer, committed);

        if let Some(goal) = outcome.goal_node {
            let goal_runtime = runtimes.get(&goal).expect("goal runtime");
            let tree_path = commit_state.tree.path_from_root(goal).expect("goal path");
            let store_path = store
                .get_path(GetPathRequest {
                    experiment_id: EXPERIMENT_ID.to_owned(),
                    node_id: goal,
                    include_input_logs: false,
                })
                .expect("store goal path");
            let store_node_ids = store_path
                .nodes
                .iter()
                .map(|node| node.node_id)
                .collect::<Vec<_>>();
            assert_eq!(&tree_path, &store_node_ids);
            assert_eq!(store_node_ids.last().copied(), Some(goal));
            assert_eq!(
                store_path.nodes.last().expect("goal node").status,
                NodeStatus::Goal
            );

            let transcript_hash = transcript.finish();
            let transcript_bytes = transcript.into_bytes();
            return SearchRun {
                expansions: expansions + 1,
                transcript_hash,
                transcript_bytes,
                goal_state: goal_runtime.state,
                goal_path_len: tree_path.len(),
            };
        }
    }

    panic!("search failed to find goal within {MAX_EXPANSIONS} expansions");
}

fn create_root(
    hypervisor: &mut FakeHypervisor,
    scorer: &mut FakeScorer,
    seed: u64,
) -> (NodeRuntime, ScoreResult) {
    let created = hypervisor
        .create_vm(CreateVmRequest {
            config: machine_config(),
            entropy_seed: entropy_seed(seed, NodeId::ROOT, 0, GridAction::Wait),
        })
        .expect("create root vm");
    let snapshot = hypervisor
        .take_snapshot(TakeSnapshotRequest {
            lease: created.lease,
            seal_input_log: false,
            capture: Some(grid_capture()),
        })
        .expect("snapshot root");
    hypervisor
        .destroy_vm(DestroyVmRequest {
            lease: created.lease,
        })
        .expect("destroy root vm");

    let root_state = GridState::new();
    let feature_bytes = snapshot.feature_bytes.expect("root features");
    assert_eq!(feature_bytes, encode_grid_features(root_state));
    let result = score_one(scorer, "root", "root", feature_bytes);
    assert_eq!(result.error, None);
    assert_eq!(result.state_hash, root_state.state_hash());
    assert_eq!(result.novelty_detail.cell_key, root_state.cell_key());

    (
        NodeRuntime {
            snapshot: snapshot.snapshot,
            state: root_state,
            frame_counter: snapshot.frame_counter,
        },
        result,
    )
}

fn expand_parent(
    hypervisor: &mut FakeHypervisor,
    seed: u64,
    expansion: u64,
    parent: NodeId,
    runtime: &NodeRuntime,
    transcript: &mut TranscriptBuilder,
) -> Vec<CandidateExecution> {
    ACTIONS
        .iter()
        .map(|action| {
            let restored = hypervisor
                .restore_snapshot(RestoreSnapshotRequest {
                    snapshot: runtime.snapshot,
                    entropy_seed: Some(entropy_seed(seed, parent, expansion + 1, *action)),
                })
                .expect("restore parent snapshot");
            let scheduled_frame = FrameCount::new(restored.frame_counter.get() + 1);
            hypervisor
                .inject_inputs(InjectInputsRequest {
                    lease: restored.lease,
                    events: vec![ScheduledEvent {
                        at: ScheduleAt::Frame(scheduled_frame),
                        event: InputEvent::PadSet(PadSet {
                            port: 0,
                            buttons: buttons_for_action(*action),
                        }),
                    }],
                })
                .expect("inject action");
            let (after, outcome) = runtime.state.step(*action);
            let run = hypervisor
                .run(RunRequest {
                    lease: restored.lease,
                    until: RunUntil::FrameBudget(FrameCount::new(1)),
                    hard_icount_cap: None,
                    capture: Some(grid_capture()),
                })
                .expect("run action");
            assert_eq!(run.frames_elapsed, 1);
            assert_eq!(run.state_hash, after.state_hash());
            assert_eq!(run.feature_bytes, Some(encode_grid_features(after)));

            let snapshot = hypervisor
                .take_snapshot(TakeSnapshotRequest {
                    lease: restored.lease,
                    seal_input_log: true,
                    capture: Some(grid_capture()),
                })
                .expect("snapshot child");
            assert_eq!(snapshot.state_hash, after.state_hash());
            assert_eq!(snapshot.feature_bytes, Some(encode_grid_features(after)));
            hypervisor
                .destroy_vm(DestroyVmRequest {
                    lease: restored.lease,
                })
                .expect("destroy child vm");

            transcript.append_step(runtime.state, *action, after, outcome);
            CandidateExecution {
                action: *action,
                before: runtime.state,
                after,
                outcome,
                snapshot: snapshot.snapshot,
                input_log_id: snapshot.input_log_id.expect("sealed input log"),
                frame_counter: snapshot.frame_counter,
                feature_bytes: snapshot.feature_bytes.expect("child features"),
            }
        })
        .collect()
}

fn scored_child(
    commit_state: &CommitState,
    scorer_archive: &ScorerArchiveMirror,
    sibling_hashes: &mut BTreeSet<StateHash>,
    execution: &CandidateExecution,
    result: &ScoreResult,
) -> ScoredChild {
    assert_eq!(result.error, None);
    assert_eq!(result.state_hash, execution.after.state_hash());
    assert_eq!(result.novelty_detail.cell_key, execution.after.cell_key());
    assert_eq!(execution.after, execution.before.step(execution.action).0);

    let archive_duplicate = scorer_archive.contains(result.state_hash);
    assert_eq!(result.duplicate, archive_duplicate);
    assert_eq!(
        commit_state.seen.contains(result.state_hash),
        archive_duplicate
    );

    let archive_cell_count = scorer_archive.cell_count(result.novelty_detail.cell_key);
    let scorer_novelty =
        Novelty::new(1.0 / f64::from(archive_cell_count + 1).sqrt()).expect("finite novelty");
    assert_eq!(result.novelty_detail.cell_count, archive_cell_count);
    assert_eq!(result.novelty_detail.count_novelty, scorer_novelty);
    assert_eq!(result.novelty_score, scorer_novelty);

    let sibling_duplicate = !sibling_hashes.insert(result.state_hash);
    let duplicate = result.duplicate || sibling_duplicate;
    let novelty = Novelty::new(
        commit_state
            .cell_mirror
            .novelty(result.novelty_detail.cell_key),
    )
    .expect("finite novelty");
    let mut child = ScoredChild::new(payload_from_score(
        execution.snapshot,
        execution.frame_counter,
        result,
        novelty,
    ));
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

fn payload_from_score(
    snapshot: SnapshotRef,
    frame_counter: FrameCount,
    result: &ScoreResult,
    novelty: Novelty,
) -> NodePayload {
    NodePayload::new(
        snapshot,
        result.progress_score,
        novelty,
        result.novelty_detail.cell_key,
        result.state_hash,
        result.stage,
        frame_counter,
    )
}

fn create_store_node(
    store: &mut InMemorySnapshotStore,
    spec: StoreNodeSpec,
    payload: &NodePayload,
) {
    store
        .create_node(CreateNodeRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id: spec.node_id,
            parent_node_id: spec.parent_node_id,
            snapshot_ref: spec.snapshot_ref,
            input_log_id: spec.input_log_id,
            status: spec.status,
            progress_score: payload.score,
            novelty_score: payload.novelty_at_commit,
            attrs: NodeAttrs::new(spec.attrs).expect("node attrs"),
            input_log_container: None,
        })
        .expect("create snapshot-store node");
}

fn update_expanded_parent(
    store: &mut InMemorySnapshotStore,
    parent: NodeId,
    commit_state: &CommitState,
) {
    let record = commit_state.tree.get(parent).expect("expanded parent");
    store
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
        .expect("update expanded parent");
}

fn replay_commits(scorer: &mut FakeScorer, states: Vec<CommittedState>) {
    if states.is_empty() {
        return;
    }
    scorer
        .replay_commits(ReplayCommitsRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            states,
        })
        .expect("replay committed states");
}

fn score_one(
    scorer: &mut FakeScorer,
    batch_id: impl Into<String>,
    node_ref: impl Into<String>,
    feature_bytes: Vec<u8>,
) -> ScoreResult {
    scorer
        .score_batch(ScoreBatchRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            states: vec![StateInput {
                node_ref: node_ref.into(),
                feature_bytes,
                framebuffer: None,
                fb_meta: None,
            }],
            archive_update: ArchiveUpdateMode::ScoreOnly,
            client_batch_id: batch_id.into(),
            return_decoded: false,
        })
        .expect("score one")
        .results
        .into_iter()
        .next()
        .expect("one score result")
}

fn configured_scorer() -> FakeScorer {
    let mut scorer = FakeScorer::new();
    scorer
        .load_feature_map(LoadFeatureMapRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            source: ArtifactSource::InlineYaml(b"feature-map: search-loop\n".to_vec()),
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
        })
        .expect("feature map");
    scorer
        .load_scoring_program(LoadScoringProgramRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            source: ArtifactSource::InlineYaml(b"score: search-loop\n".to_vec()),
        })
        .expect("scoring program");
    scorer
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

fn grid_capture() -> CaptureSpec {
    CaptureSpec {
        ranges: vec![HypervisorExtractRange {
            region: "grid".to_owned(),
            layout_version: 1,
            offset: 0,
            len: GRID_FEATURE_BYTES_LEN,
        }],
        framebuffer: false,
    }
}

fn machine_config() -> MachineConfig {
    MachineConfig {
        version: 1,
        mem_bytes: 128 * 1024 * 1024,
        vcpus: 1,
        clock_num: 1,
        clock_den: 1,
        base_image_hash: HypervisorDigest32::new([0xAA; 32]),
        boot: BootSpec::Elf(ElfBoot {
            kernel_hash: HypervisorDigest32::new([0xBB; 32]),
            cmdline: b"console=ttyS0".to_vec(),
        }),
        epoch_len: GuestInstructions::new(50_000_000),
        hash_epochs: HashEpochs::EpochsOn,
        skid_margin: 8192,
    }
}

fn entropy_seed(seed: u64, parent: NodeId, expansion: u64, action: GridAction) -> EntropySeed {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"orch-fakes/search-loop/entropy/v1");
    hasher.update(&seed.to_le_bytes());
    hasher.update(&parent.get().to_le_bytes());
    hasher.update(&expansion.to_le_bytes());
    hasher.update(&[action.tag()]);
    EntropySeed::new(*hasher.finalize().as_bytes())
}

fn buttons_for_action(action: GridAction) -> u32 {
    match action {
        GridAction::Wait => 0,
        GridAction::Up => BUTTON_UP,
        GridAction::Down => BUTTON_DOWN,
        GridAction::Left => BUTTON_LEFT,
        GridAction::Right => BUTTON_RIGHT,
        GridAction::Attack => BUTTON_ATTACK_A,
    }
}

fn node_attrs(action: GridAction, outcome: StepOutcome) -> Vec<u8> {
    vec![action.tag(), outcome_tag(outcome)]
}

const fn outcome_tag(outcome: StepOutcome) -> u8 {
    match outcome {
        StepOutcome::Moved => 0,
        StepOutcome::BlockedByWall => 1,
        StepOutcome::BlockedByDoor => 2,
        StepOutcome::PickedKey => 3,
        StepOutcome::HitBoss => 4,
        StepOutcome::BossDefeated => 5,
        StepOutcome::GoalReached => 6,
        StepOutcome::Noop => 7,
    }
}
