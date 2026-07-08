//! W2.2 accept: a scripted op sequence against `PersistentServices::create`
//! reloads into an identical, still-live world; a perturbed `Applied`
//! digest panics loudly on replay.

use std::collections::BTreeMap;

use orch_clients::hypervisor::{
    BootSpec, CaptureSpec, CreateVmRequest, DestroyVmRequest, Digest32, ElfBoot, EntropySeed,
    ExtractRange, HashEpochs, HypervisorWorkerClient, InjectInputsRequest, InputEvent,
    ListSlotsRequest, MachineConfig, PadSet, RestoreSnapshotRequest, RunRequest, RunUntil,
    ScheduleAt, ScheduledEvent, TakeSnapshotRequest,
};
use orch_clients::scorer::{
    ArchiveUpdateMode, ArtifactSource, CommittedState, CompiledLayout,
    ExtractRange as ScorerExtractRange, LoadFeatureMapRequest, LoadScoringProgramRequest,
    ReplayCommitsRequest, ScoreBatchRequest, ScoreResult, StateInput, StateScorerClient,
};
use orch_clients::snapshot_store::{
    CreateNodeRequest, GetMetadataRequest, MetadataExpectation, MetadataKey, NodeUpdate,
    PutMetadataRequest, SnapshotStoreClient, UpdateNodesRequest,
};
use orch_core::types::{FrameCount, GuestInstructions, NodeId, NodeStatus, SnapshotRef};
use orch_driver::node_attrs::{encode_node_attrs, OrchNodeAttrsV1, SynthContextAttrs};
use orch_fakes::grid::{GridAction, GridState};
use orch_fakes::scorer::GRID_FEATURE_BYTES_LEN;
use orch_simstate::compare::{scorer_archive_fingerprint, store_tree_hash};
use orch_simstate::journal::{Journal, RecordKind};
use orch_simstate::records::JournalRecord;
use orch_simstate::world::PersistentServices;

const EXPERIMENT_ID: &str = "tier2-world-reload";
const BUTTON_RIGHT: u32 = 0b10_0000_0000;

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

fn configure_scorer(services: &mut PersistentServices) {
    services
        .scorer
        .load_feature_map(LoadFeatureMapRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            source: ArtifactSource::InlineYaml(b"feature-map: tier2\n".to_vec()),
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
    services
        .scorer
        .load_scoring_program(LoadScoringProgramRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            source: ArtifactSource::InlineYaml(b"score: tier2\n".to_vec()),
        })
        .expect("scoring program");
}

fn score_one(
    services: &mut PersistentServices,
    batch_id: &str,
    feature_bytes: Vec<u8>,
) -> ScoreResult {
    services
        .scorer
        .score_batch(ScoreBatchRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            states: vec![StateInput {
                node_ref: batch_id.to_owned(),
                feature_bytes,
                framebuffer: None,
                fb_meta: None,
            }],
            archive_update: ArchiveUpdateMode::ScoreOnly,
            client_batch_id: batch_id.to_owned(),
            return_decoded: false,
        })
        .expect("score batch")
        .results
        .into_iter()
        .next()
        .expect("one result")
}

struct Committed {
    snapshot: SnapshotRef,
}

/// Root + one child expansion, all through the journaling wrappers —
/// touching every service (mirrors `search_loop`'s shape, shortened).
fn drive_scripted_ops(services: &mut PersistentServices) -> Committed {
    configure_scorer(services);

    let created = services
        .hypervisor
        .create_vm(CreateVmRequest {
            config: machine_config(),
            entropy_seed: EntropySeed::new([0x11; 32]),
        })
        .expect("create vm");
    let root_snapshot = services
        .hypervisor
        .take_snapshot(TakeSnapshotRequest {
            lease: created.lease,
            seal_input_log: false,
            capture: Some(grid_capture()),
        })
        .expect("root snapshot");
    services
        .hypervisor
        .destroy_vm(DestroyVmRequest {
            lease: created.lease,
        })
        .expect("destroy root vm");

    let root_result = score_one(
        services,
        "root",
        root_snapshot.feature_bytes.clone().expect("root features"),
    );
    services
        .scorer
        .replay_commits(ReplayCommitsRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            states: vec![CommittedState {
                state_hash: root_result.state_hash,
                cell_key: root_result.novelty_detail.cell_key,
            }],
        })
        .expect("replay root commit");

    let node_attrs = |snapshot: &orch_clients::hypervisor::TakeSnapshotResponse,
                      result: &ScoreResult| {
        encode_node_attrs(&OrchNodeAttrsV1::new(
            snapshot.machine_config_hash,
            snapshot.determinism_class.clone(),
            SynthContextAttrs {
                created_by_burst: None,
                config_fingerprint: None,
                decoded_features: BTreeMap::new(),
                frame_counter: snapshot.frame_counter,
                state_hash: result.state_hash,
                cell_key: result.novelty_detail.cell_key,
                stage: result.stage,
                score: result.progress_score,
                novelty: result.novelty_score,
                recent_inputs: None,
            },
        ))
        .expect("encode attrs")
    };

    services
        .store
        .create_node(CreateNodeRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id: NodeId::ROOT,
            parent_node_id: None,
            snapshot_ref: root_snapshot.snapshot,
            input_log_id: None,
            status: NodeStatus::Frontier,
            progress_score: root_result.progress_score,
            novelty_score: root_result.novelty_score,
            attrs: node_attrs(&root_snapshot, &root_result),
            input_log_container: None,
        })
        .expect("create root node");

    // One expansion: restore root, step Right, commit the child.
    let restored = services
        .hypervisor
        .restore_snapshot(RestoreSnapshotRequest {
            snapshot: root_snapshot.snapshot,
            entropy_seed: Some(EntropySeed::new([0x22; 32])),
        })
        .expect("restore root");
    services
        .hypervisor
        .inject_inputs(InjectInputsRequest {
            lease: restored.lease,
            events: vec![ScheduledEvent {
                at: ScheduleAt::Frame(FrameCount::new(restored.frame_counter.get() + 1)),
                event: InputEvent::PadSet(PadSet {
                    port: 0,
                    buttons: BUTTON_RIGHT,
                }),
            }],
        })
        .expect("inject right");
    let run = services
        .hypervisor
        .run(RunRequest {
            lease: restored.lease,
            until: RunUntil::FrameBudget(FrameCount::new(1)),
            hard_icount_cap: None,
            capture: Some(grid_capture()),
        })
        .expect("run right");
    let (after, _outcome) = GridState::new().step(GridAction::Right);
    assert_eq!(run.state_hash, after.state_hash());
    let child_snapshot = services
        .hypervisor
        .take_snapshot(TakeSnapshotRequest {
            lease: restored.lease,
            seal_input_log: true,
            capture: Some(grid_capture()),
        })
        .expect("child snapshot");
    services
        .hypervisor
        .destroy_vm(DestroyVmRequest {
            lease: restored.lease,
        })
        .expect("destroy child vm");

    let child_result = score_one(
        services,
        "child",
        child_snapshot
            .feature_bytes
            .clone()
            .expect("child features"),
    );
    services
        .scorer
        .replay_commits(ReplayCommitsRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            states: vec![CommittedState {
                state_hash: child_result.state_hash,
                cell_key: child_result.novelty_detail.cell_key,
            }],
        })
        .expect("replay child commit");
    services
        .store
        .create_node(CreateNodeRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id: NodeId::new(1),
            parent_node_id: Some(NodeId::ROOT),
            snapshot_ref: child_snapshot.snapshot,
            input_log_id: child_snapshot.input_log_id,
            status: NodeStatus::Frontier,
            progress_score: child_result.progress_score,
            novelty_score: child_result.novelty_score,
            attrs: node_attrs(&child_snapshot, &child_result),
            input_log_container: None,
        })
        .expect("create child node");
    services
        .store
        .update_nodes(UpdateNodesRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            updates: vec![NodeUpdate {
                node_id: NodeId::ROOT,
                status: Some(NodeStatus::Expanded),
                progress_score: None,
                novelty_score: None,
                visit_count_delta: 1,
                expand_count_delta: 1,
                touch_visited: true,
                attrs: None,
            }],
        })
        .expect("update root");

    // Metadata: a checkpoint blob and a WAL entry, then delete the WAL entry
    // (the checkpoint/WAL lifecycle the runner drives).
    services
        .store
        .put_metadata(PutMetadataRequest {
            key: MetadataKey::checkpoint(EXPERIMENT_ID),
            value: b"checkpoint-bytes".to_vec(),
            expected_generation: MetadataExpectation::create_only(),
        })
        .expect("put checkpoint");
    let wal_put = services
        .store
        .put_metadata(PutMetadataRequest {
            key: MetadataKey::wal(EXPERIMENT_ID, 1),
            value: b"wal-entry-1".to_vec(),
            expected_generation: MetadataExpectation::create_only(),
        })
        .expect("put wal");
    services
        .store
        .delete_metadata(orch_clients::snapshot_store::DeleteMetadataRequest {
            key: MetadataKey::wal(EXPERIMENT_ID, 1),
            expected_generation: MetadataExpectation::generation(wal_put.generation),
        })
        .expect("delete wal");

    Committed {
        snapshot: child_snapshot.snapshot,
    }
}

#[test]
fn persistent_world_reload_reproduces_state_and_stays_live() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut services = PersistentServices::create(dir.path()).expect("create");
    let committed = drive_scripted_ops(&mut services);

    let tree_hash = store_tree_hash(services.store.inner(), EXPERIMENT_ID);
    let archive = scorer_archive_fingerprint(services.scorer.inner(), EXPERIMENT_ID);
    let slots = services
        .hypervisor
        .list_slots(ListSlotsRequest)
        .expect("list slots");
    let checkpoint = services
        .store
        .get_metadata(GetMetadataRequest {
            key: MetadataKey::checkpoint(EXPERIMENT_ID),
        })
        .expect("get checkpoint");
    drop(services);

    let (reloaded, stats) = PersistentServices::reload(dir.path()).expect("reload");
    assert_eq!(stats.truncated_bytes, 0, "clean shutdown has no torn tail");

    // Committed state is bit-identical.
    assert_eq!(
        store_tree_hash(reloaded.store.inner(), EXPERIMENT_ID),
        tree_hash
    );
    assert_eq!(
        scorer_archive_fingerprint(reloaded.scorer.inner(), EXPERIMENT_ID),
        archive
    );
    assert_eq!(
        reloaded
            .store
            .get_metadata(GetMetadataRequest {
                key: MetadataKey::checkpoint(EXPERIMENT_ID),
            })
            .expect("get checkpoint after reload"),
        checkpoint
    );
    assert_eq!(
        reloaded
            .hypervisor
            .list_slots(ListSlotsRequest)
            .expect("list slots after reload"),
        slots
    );
    // The deleted WAL entry stays deleted.
    assert!(reloaded
        .store
        .get_metadata(GetMetadataRequest {
            key: MetadataKey::wal(EXPERIMENT_ID, 1),
        })
        .is_err());

    // Live, not just readable: re-drive a tail against the reloaded world.
    let mut reloaded = reloaded;
    let restored = reloaded
        .hypervisor
        .restore_snapshot(RestoreSnapshotRequest {
            snapshot: committed.snapshot,
            entropy_seed: Some(EntropySeed::new([0x33; 32])),
        })
        .expect("restore after reload");
    let run = reloaded
        .hypervisor
        .run(RunRequest {
            lease: restored.lease,
            until: RunUntil::FrameBudget(FrameCount::new(1)),
            hard_icount_cap: None,
            capture: Some(grid_capture()),
        })
        .expect("run after reload");
    // No input injected: the world steps Wait deterministically from the
    // committed child state (root -> Right).
    let (child_state, _) = GridState::new().step(GridAction::Right);
    let (waited, _) = child_state.step(GridAction::Wait);
    assert_eq!(run.state_hash, waited.state_hash());

    // ...and the tail itself journals: a second reload replays it too.
    drop(reloaded);
    let (again, _) = PersistentServices::reload(dir.path()).expect("second reload");
    assert_eq!(
        store_tree_hash(again.store.inner(), EXPERIMENT_ID),
        tree_hash
    );
}

#[test]
#[should_panic(expected = "replay digest mismatch")]
fn replay_digest_mismatch_panics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut services = PersistentServices::create(dir.path()).expect("create");
    services
        .hypervisor
        .create_vm(CreateVmRequest {
            config: machine_config(),
            entropy_seed: EntropySeed::new([0x44; 32]),
        })
        .expect("create vm");
    drop(services);

    // Forge an op whose Applied digest cannot match its replayed response.
    let mut journal = Journal::open_existing(dir.path(), 1_000).expect("open journal");
    let op_id = journal.append_op(
        |op_id| JournalRecord::HvCreateVm {
            op_id,
            request: CreateVmRequest {
                config: machine_config(),
                entropy_seed: EntropySeed::new([0x55; 32]),
            },
        },
        RecordKind::Other,
    );
    journal.append_advisory(&JournalRecord::Applied { op_id, digest: 0 });
    drop(journal);

    let _ = PersistentServices::reload(dir.path());
}
