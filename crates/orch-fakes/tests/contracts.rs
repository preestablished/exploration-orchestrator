use std::collections::BTreeMap;

use orch_clients::{
    hypervisor::{
        BootSpec, CreateVmRequest, Digest32 as HypervisorDigest32, ElfBoot, EntropySeed,
        HashEpochs, HypervisorWorkerClient, InjectInputsRequest, InputEvent, ListSlotsRequest,
        MachineConfig, PadSet, RunRequest, RunUntil, ScheduleAt, ScheduledEvent,
        TakeSnapshotRequest,
    },
    input_synth::{
        DocumentKind, HealthRequest, HealthStatus, InputSynthClient, LoadMacroPackRequest,
        LoadMacroPackSource, ModelKind, NodeContext, ProposeBurstsRequest,
    },
    scorer::{
        ArchiveUpdateMode, ArtifactSource, CompiledLayout, ExtractRange, ItemErrorKind,
        LoadFeatureMapRequest, LoadScoringProgramRequest, ScoreBatchRequest, StateInput,
        StateScorerClient,
    },
    snapshot_store::{
        CreateNodeRequest, GetChildrenRequest, MetadataExpectation, MetadataKey, NodeAttrs,
        PutMetadataRequest, SnapshotStoreClient,
    },
    ClientErrorKind,
};
use orch_core::types::{
    CellKey, FrameCount, NodeId, NodeStatus, Novelty, Score, SnapshotRef, Stage, StateHash,
};
use orch_fakes::{
    fault::{FaultPlan, FaultRate, LatencyFault, PartialResponseFault},
    grid::{GridAction, GridState},
    hypervisor::FakeHypervisor,
    scorer::{encode_grid_features, FakeScorer, GRID_FEATURE_BYTES_LEN},
    snapshot_store::InMemorySnapshotStore,
    synth::FakeSynth,
    transcript::TranscriptBuilder,
};

const EXPERIMENT_ID: &str = "contracts-exp";
const SNAPSHOT_A: SnapshotRef = SnapshotRef::new([0xA5; 32]);
const SNAPSHOT_B: SnapshotRef = SnapshotRef::new([0x5A; 32]);
const STATE_A: StateHash = StateHash::new([0x11; 32]);

#[test]
fn contracts_hypervisor_faults_surface_through_client_calls() {
    let mut hypervisor = FakeHypervisor::with_slots_and_fault_plan(
        4,
        FaultPlan::disabled(0xC0DE)
            .with_latency(LatencyFault::new(5, 7))
            .with_partial_response(PartialResponseFault::new(FaultRate::always(), 1)),
    );
    for seed in [0x10, 0x11, 0x12] {
        hypervisor
            .create_vm(sample_vm_request(seed))
            .expect("create vm");
    }
    let latency = hypervisor.last_fault().expect("create fault decision");
    let slots = hypervisor
        .list_slots(ListSlotsRequest)
        .expect("partial slot list");
    let partial = hypervisor.last_fault().expect("list fault decision");
    let mut erroring = FakeHypervisor::with_fault_plan(
        FaultPlan::disabled(0xC0DE).with_error(FaultRate::always(), ClientErrorKind::DataLoss),
    );
    let error = erroring
        .create_vm(sample_vm_request(0x20))
        .expect_err("forced hypervisor error");
    let mut timing_out = FakeHypervisor::with_fault_plan(
        FaultPlan::disabled(0xC0DE).with_timeout(FaultRate::always()),
    );
    let timeout = timing_out
        .create_vm(sample_vm_request(0x21))
        .expect_err("forced hypervisor timeout");

    assert!((5..=12).contains(&latency.latency_ticks));
    assert_eq!(slots.slots.len(), partial.truncate_len(3));
    assert!(slots.slots.len() < 3);
    assert_eq!(error.kind(), ClientErrorKind::DataLoss);
    assert_eq!(timeout.kind(), ClientErrorKind::Unavailable);
}

#[test]
fn contracts_snapshot_store_cas_and_partial_faults_are_observable() {
    let mut latency_store = InMemorySnapshotStore::with_fault_plan(
        FaultPlan::disabled(0xAA55).with_latency(LatencyFault::new(4, 6)),
    );
    latency_store.create_node(root_request()).expect("root");
    let latency = latency_store.last_fault().expect("latency fault decision");
    let mut error_store = InMemorySnapshotStore::with_fault_plan(
        FaultPlan::disabled(0xAA55).with_error(FaultRate::always(), ClientErrorKind::DataLoss),
    );
    let error = error_store
        .create_node(root_request())
        .expect_err("forced snapshot-store error");
    let mut timeout_store = InMemorySnapshotStore::with_fault_plan(
        FaultPlan::disabled(0xAA55).with_timeout(FaultRate::always()),
    );
    let timeout = timeout_store
        .create_node(root_request())
        .expect_err("forced snapshot-store timeout");
    let mut store = InMemorySnapshotStore::with_fault_plan(
        FaultPlan::disabled(0xAA55)
            .with_partial_response(PartialResponseFault::new(FaultRate::always(), 0)),
    );
    populate_snapshot_store(&mut store);

    let children = store
        .get_children(GetChildrenRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id: NodeId::ROOT,
        })
        .expect("children");
    let partial = store.last_fault().expect("partial fault decision");
    let key = MetadataKey::checkpoint(EXPERIMENT_ID);
    let first_put = store
        .put_metadata(PutMetadataRequest {
            key: key.clone(),
            value: b"checkpoint-a".to_vec(),
            expected_generation: MetadataExpectation::create_only(),
        })
        .expect("first put");
    let create_conflict = store
        .put_metadata(PutMetadataRequest {
            key: key.clone(),
            value: b"checkpoint-b".to_vec(),
            expected_generation: MetadataExpectation::create_only(),
        })
        .expect_err("create-only conflict");
    let second_put = store
        .put_metadata(PutMetadataRequest {
            key: key.clone(),
            value: b"checkpoint-c".to_vec(),
            expected_generation: MetadataExpectation::generation(first_put.generation),
        })
        .expect("generation update");
    let stale_generation = store
        .put_metadata(PutMetadataRequest {
            key,
            value: b"checkpoint-d".to_vec(),
            expected_generation: MetadataExpectation::generation(first_put.generation),
        })
        .expect_err("stale generation");

    assert!((4..=10).contains(&latency.latency_ticks));
    assert_eq!(error.kind(), ClientErrorKind::DataLoss);
    assert_eq!(timeout.kind(), ClientErrorKind::Unavailable);
    assert_eq!(children.children.len(), partial.truncate_len(2));
    assert!(children.children.len() < 2);
    assert_eq!(create_conflict.kind(), ClientErrorKind::FailedPrecondition);
    assert_eq!(second_put.generation.get(), first_put.generation.get() + 1);
    assert_eq!(stale_generation.kind(), ClientErrorKind::FailedPrecondition);
}

#[test]
fn contracts_synth_faults_preserve_shape_and_expose_fingerprint_flip() {
    let mut clean = configured_synth(FaultPlan::disabled(0));
    let mut faulty = configured_synth(
        FaultPlan::disabled(0xBEEF)
            .with_latency(LatencyFault::new(2, 3))
            .with_partial_response(PartialResponseFault::new(FaultRate::always(), 0))
            .with_synth_fingerprint_flip(FaultRate::always()),
    );
    let mut erroring = configured_synth(
        FaultPlan::disabled(0xBEEF).with_error(FaultRate::always(), ClientErrorKind::DataLoss),
    );
    let mut timing_out =
        configured_synth(FaultPlan::disabled(0xBEEF).with_timeout(FaultRate::always()));

    let clean_response = clean
        .propose_bursts(sample_burst_request(7))
        .expect("clean bursts");
    let faulty_response = faulty
        .propose_bursts(sample_burst_request(7))
        .expect("faulty bursts");
    let error = erroring
        .propose_bursts(sample_burst_request(7))
        .expect_err("terminal synth fault");
    let timeout = timing_out
        .propose_bursts(sample_burst_request(7))
        .expect_err("terminal synth timeout");
    let fault = faulty.last_fault().expect("synth fault decision");
    let health = faulty.health(HealthRequest).expect("health");

    assert!((2..=5).contains(&fault.latency_ticks));
    assert!(fault.synth_fingerprint_flip.is_some());
    assert_eq!(clean_response.bursts.len(), 4);
    assert_eq!(faulty_response.bursts.len(), 4);
    assert_ne!(
        clean_response.config_fingerprint,
        faulty_response.config_fingerprint
    );
    assert!(faulty_response
        .bursts
        .iter()
        .all(|burst| burst.provenance.config_fingerprint == faulty_response.config_fingerprint));
    assert_eq!(error.kind(), ClientErrorKind::DataLoss);
    assert_eq!(timeout.kind(), ClientErrorKind::Unavailable);
    assert_eq!(health.status, HealthStatus::Serving);
}

#[test]
fn contracts_scorer_item_errors_and_partial_fault_decisions_are_explicit() {
    let mut scorer = configured_scorer();
    let mut partial_scorer = configured_scorer_with_fault(
        FaultPlan::disabled(0x51)
            .with_latency(LatencyFault::new(3, 4))
            .with_partial_response(PartialResponseFault::new(FaultRate::always(), 1)),
    );
    let mut erroring = FakeScorer::with_fault_plan(
        FaultPlan::disabled(0x51).with_error(FaultRate::always(), ClientErrorKind::DataLoss),
    );
    let mut timing_out =
        FakeScorer::with_fault_plan(FaultPlan::disabled(0x51).with_timeout(FaultRate::always()));
    let valid = scorer
        .score_batch(ScoreBatchRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            states: vec![StateInput {
                node_ref: "valid".to_owned(),
                feature_bytes: encode_grid_features(GridState::new()),
                framebuffer: None,
                fb_meta: None,
            }],
            archive_update: ArchiveUpdateMode::ScoreOnly,
            client_batch_id: "valid-batch".to_owned(),
            return_decoded: true,
        })
        .expect("valid score");
    let invalid = scorer
        .score_batch(ScoreBatchRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            states: vec![StateInput {
                node_ref: "bad-len".to_owned(),
                feature_bytes: vec![0, 1],
                framebuffer: None,
                fb_meta: None,
            }],
            archive_update: ArchiveUpdateMode::ScoreOnly,
            client_batch_id: "invalid-batch".to_owned(),
            return_decoded: false,
        })
        .expect("item error is per-result");
    let partial_response = partial_scorer
        .score_batch(ScoreBatchRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            states: (0..4)
                .map(|index| StateInput {
                    node_ref: format!("partial-{index}"),
                    feature_bytes: encode_grid_features(GridState::new()),
                    framebuffer: None,
                    fb_meta: None,
                })
                .collect(),
            archive_update: ArchiveUpdateMode::ScoreOnly,
            client_batch_id: "partial-batch".to_owned(),
            return_decoded: false,
        })
        .expect("partial score");
    let partial = partial_scorer.last_fault().expect("partial scorer fault");
    let error = erroring
        .load_feature_map(sample_feature_map_request())
        .expect_err("forced scorer error");
    let timeout = timing_out
        .load_feature_map(sample_feature_map_request())
        .expect_err("forced scorer timeout");

    assert_eq!(valid.results[0].error, None);
    assert_eq!(
        invalid.results[0].error.as_ref().map(|error| &error.kind),
        Some(&ItemErrorKind::FeatureLenMismatch)
    );
    assert!((3..=7).contains(&partial.latency_ticks));
    assert_eq!(
        partial_response.results.len(),
        partial.partial.expect("partial").keep_items as usize
    );
    assert!(partial_response.results.len() < 4);
    assert_eq!(error.kind(), ClientErrorKind::DataLoss);
    assert_eq!(timeout.kind(), ClientErrorKind::Unavailable);
}

#[test]
fn contracts_disabled_faults_leave_transcript_hashes_stable() {
    let clean = grid_transcript_with_disabled_faults(0);
    let disabled_with_different_seed = grid_transcript_with_disabled_faults(u64::MAX);

    assert_eq!(clean, disabled_with_different_seed);
}

fn populate_snapshot_store(store: &mut InMemorySnapshotStore) {
    store.create_node(root_request()).expect("root");
    store
        .create_node(child_request(NodeId::new(1), SNAPSHOT_A))
        .expect("child one");
    store
        .create_node(child_request(NodeId::new(2), SNAPSHOT_B))
        .expect("child two");
}

fn root_request() -> CreateNodeRequest {
    CreateNodeRequest {
        experiment_id: EXPERIMENT_ID.to_owned(),
        node_id: NodeId::ROOT,
        parent_node_id: None,
        snapshot_ref: SNAPSHOT_A,
        input_log_id: None,
        status: NodeStatus::Frontier,
        progress_score: score(0.0),
        novelty_score: novelty(1.0),
        attrs: NodeAttrs::new(Vec::new()).expect("attrs"),
        input_log_container: None,
    }
}

fn child_request(node_id: NodeId, snapshot_ref: SnapshotRef) -> CreateNodeRequest {
    CreateNodeRequest {
        experiment_id: EXPERIMENT_ID.to_owned(),
        node_id,
        parent_node_id: Some(NodeId::ROOT),
        snapshot_ref,
        input_log_id: Some(orch_clients::snapshot_store::InputLogId::new(
            [node_id.get() as u8; 32],
        )),
        status: NodeStatus::Frontier,
        progress_score: score(node_id.get() as f64),
        novelty_score: novelty(0.5),
        attrs: NodeAttrs::new(vec![node_id.get() as u8]).expect("attrs"),
        input_log_container: None,
    }
}

fn configured_synth(fault_plan: FaultPlan) -> FakeSynth {
    let mut synth = FakeSynth::with_fault_plan(fault_plan);
    synth
        .load_macro_pack(LoadMacroPackRequest {
            source: LoadMacroPackSource::DocumentYaml(
                b"version: 1\nkind: experiment_config\nexperiment_id: contracts-exp\nmodel: pad\nbutton_alphabet: console16-12btn-v1\ngenerator_mix:\n  weighted_random: 1\n  macro: 0\n"
                    .to_vec(),
            ),
            kind: DocumentKind::ExperimentConfig,
        })
        .expect("experiment config");
    synth
        .load_macro_pack(LoadMacroPackRequest {
            source: LoadMacroPackSource::DocumentYaml(
                b"version: 1\nkind: macro_pack\nname: contracts-pack\nmodel: pad\nmacros:\n  - name: dash\n"
                    .to_vec(),
            ),
            kind: DocumentKind::MacroPack,
        })
        .expect("macro pack");
    synth
}

fn sample_burst_request(seed: u64) -> ProposeBurstsRequest {
    ProposeBurstsRequest {
        experiment_id: EXPERIMENT_ID.to_owned(),
        node_context: NodeContext {
            node_id: NodeId::new(7),
            parent_node_id: Some(NodeId::ROOT),
            snapshot_ref: SNAPSHOT_A,
            state_hash: STATE_A,
            cell_key: CellKey::new(9),
            stage: Stage::new(1),
            depth: 1,
            frame_counter: FrameCount::new(10),
            node_score: score(1.0),
            novelty: novelty(0.25),
            ram_features: BTreeMap::new(),
            frame_embedding: Vec::new(),
            recent_inputs: None,
            parent_burst: None,
            sibling_bursts: Vec::new(),
        },
        k: 4,
        length_hint: FrameCount::new(8),
        seed,
        model: ModelKind::Pad,
        config_overrides_yaml: Vec::new(),
    }
}

fn configured_scorer() -> FakeScorer {
    configured_scorer_with_fault(FaultPlan::disabled(0))
}

fn configured_scorer_with_fault(fault_plan: FaultPlan) -> FakeScorer {
    let mut scorer = FakeScorer::with_fault_plan(fault_plan);
    scorer
        .load_feature_map(sample_feature_map_request())
        .expect("feature map");
    scorer
        .load_scoring_program(sample_scoring_program_request())
        .expect("scoring program");
    scorer
}

fn sample_feature_map_request() -> LoadFeatureMapRequest {
    LoadFeatureMapRequest {
        experiment_id: EXPERIMENT_ID.to_owned(),
        source: ArtifactSource::InlineYaml(b"feature-map: contracts\n".to_vec()),
        layout: CompiledLayout {
            ranges: vec![ExtractRange {
                region: "grid".to_owned(),
                layout_version: 1,
                offset: 0,
                len: GRID_FEATURE_BYTES_LEN,
            }],
        },
        frame: None,
        rebin: false,
    }
}

fn sample_scoring_program_request() -> LoadScoringProgramRequest {
    LoadScoringProgramRequest {
        experiment_id: EXPERIMENT_ID.to_owned(),
        source: ArtifactSource::InlineYaml(b"score: contracts\n".to_vec()),
    }
}

fn sample_vm_request(seed_byte: u8) -> CreateVmRequest {
    CreateVmRequest {
        config: sample_machine_config(),
        entropy_seed: EntropySeed::new([seed_byte; 32]),
    }
}

fn sample_machine_config() -> MachineConfig {
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
        epoch_len: orch_core::types::GuestInstructions::new(50_000_000),
        hash_epochs: HashEpochs::EpochsOn,
        skid_margin: 8192,
    }
}

fn grid_transcript_with_disabled_faults(seed: u64) -> orch_fakes::transcript::TranscriptHash {
    let mut hypervisor = FakeHypervisor::with_fault_plan(FaultPlan::disabled(seed));
    let created = hypervisor
        .create_vm(sample_vm_request(0x70))
        .expect("create vm");
    hypervisor
        .inject_inputs(InjectInputsRequest {
            lease: created.lease,
            events: vec![ScheduledEvent {
                at: ScheduleAt::Frame(FrameCount::new(1)),
                event: InputEvent::PadSet(PadSet {
                    port: 0,
                    buttons: 0b10_0000_0000,
                }),
            }],
        })
        .expect("inject input");
    let run = hypervisor
        .run(RunRequest {
            lease: created.lease,
            until: RunUntil::FrameBudget(FrameCount::new(1)),
            hard_icount_cap: None,
            capture: None,
        })
        .expect("run frame");
    let snapshot = hypervisor
        .take_snapshot(TakeSnapshotRequest {
            lease: created.lease,
            seal_input_log: true,
            capture: None,
        })
        .expect("snapshot");
    let mut transcript = TranscriptBuilder::new(99);
    let state = GridState::new();
    let (next, outcome) = state.step(GridAction::Right);
    assert_eq!(run.state_hash, next.state_hash());
    assert_eq!(snapshot.state_hash, next.state_hash());
    transcript.append_state(state);
    transcript.append_step(state, GridAction::Right, next, outcome);
    transcript.finish()
}

fn score(value: f64) -> Score {
    Score::new(value).expect("finite score")
}

fn novelty(value: f64) -> Novelty {
    Novelty::new(value).expect("finite novelty")
}
