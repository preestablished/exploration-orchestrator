use std::collections::BTreeMap;

use orch_clients::{
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
    fault::{
        FaultInjector, FaultPlan, FaultRate, FaultRequest, FaultTarget, LatencyFault,
        PartialResponseFault,
    },
    grid::{GridAction, GridState},
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
fn contracts_fault_knobs_are_deterministic_for_each_fake_surface() {
    let plan = FaultPlan::disabled(0xC0DE)
        .with_latency(LatencyFault::new(5, 7))
        .with_partial_response(PartialResponseFault::new(FaultRate::always(), 1));
    for target in [
        FaultTarget::Hypervisor,
        FaultTarget::SnapshotStore,
        FaultTarget::Scorer,
        FaultTarget::Synth,
    ] {
        let request = FaultRequest::new(target, "contract_operation", b"stable-request");
        let first = FaultInjector::new(plan.clone()).decide(request, 6);
        let second = FaultInjector::new(plan.clone()).decide(request, 6);
        let error = FaultInjector::new(
            FaultPlan::disabled(0xC0DE).with_error(FaultRate::always(), ClientErrorKind::DataLoss),
        )
        .decide(request, 6)
        .client_error()
        .expect("forced error");
        let timeout = FaultInjector::new(
            FaultPlan::disabled(0xC0DE)
                .with_timeout(FaultRate::always())
                .with_partial_response(PartialResponseFault::new(FaultRate::always(), 0)),
        )
        .decide(request, 6);

        assert_eq!(first, second);
        assert!((5..=12).contains(&first.latency_ticks));
        assert!(first.partial.expect("partial").keep_items < 6);
        assert_eq!(error.kind(), ClientErrorKind::DataLoss);
        assert_eq!(
            timeout.client_error().expect("timeout").kind(),
            ClientErrorKind::Unavailable
        );
        assert_eq!(timeout.partial, None);
    }
}

#[test]
fn contracts_snapshot_store_cas_and_partial_faults_are_observable() {
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
            .with_partial_response(PartialResponseFault::new(FaultRate::always(), 0))
            .with_synth_fingerprint_flip(FaultRate::always()),
    );
    let mut erroring = configured_synth(
        FaultPlan::disabled(0xBEEF).with_error(FaultRate::always(), ClientErrorKind::Unavailable),
    );

    let clean_response = clean
        .propose_bursts(sample_burst_request(7))
        .expect("clean bursts");
    let faulty_response = faulty
        .propose_bursts(sample_burst_request(7))
        .expect("faulty bursts");
    let error = erroring
        .propose_bursts(sample_burst_request(7))
        .expect_err("terminal synth fault");
    let health = faulty.health(HealthRequest).expect("health");

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
    assert_eq!(error.kind(), ClientErrorKind::Unavailable);
    assert_eq!(health.status, HealthStatus::Serving);
}

#[test]
fn contracts_scorer_item_errors_and_partial_fault_decisions_are_explicit() {
    let mut scorer = configured_scorer();
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
    let partial = FaultInjector::new(
        FaultPlan::disabled(0x51)
            .with_partial_response(PartialResponseFault::new(FaultRate::always(), 1)),
    )
    .decide(
        FaultRequest::new(FaultTarget::Scorer, "score_batch", b"states=4"),
        4,
    );

    assert_eq!(valid.results[0].error, None);
    assert_eq!(
        invalid.results[0].error.as_ref().map(|error| &error.kind),
        Some(&ItemErrorKind::FeatureLenMismatch)
    );
    assert_eq!(
        partial.truncate_len(4),
        partial.partial.expect("partial").keep_items as usize
    );
    assert!(partial.truncate_len(4) < 4);
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
    let mut scorer = FakeScorer::new();
    scorer
        .load_feature_map(LoadFeatureMapRequest {
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
        })
        .expect("feature map");
    scorer
        .load_scoring_program(LoadScoringProgramRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            source: ArtifactSource::InlineYaml(b"score: contracts\n".to_vec()),
        })
        .expect("scoring program");
    scorer
}

fn grid_transcript_with_disabled_faults(seed: u64) -> orch_fakes::transcript::TranscriptHash {
    let injector = FaultInjector::new(FaultPlan::disabled(seed));
    let request = FaultRequest::new(FaultTarget::Grid, "transcript", b"fixed-path");
    let decision = injector.decide(request, 3);
    assert_eq!(decision.client_error(), None);
    assert_eq!(decision.partial, None);

    let mut transcript = TranscriptBuilder::new(99);
    let mut state = GridState::new();
    transcript.append_state(state);
    for action in [GridAction::Right, GridAction::Right, GridAction::Wait] {
        let (next, outcome) = state.step(action);
        transcript.append_step(state, action, next, outcome);
        state = next;
    }
    transcript.finish()
}

fn score(value: f64) -> Score {
    Score::new(value).expect("finite score")
}

fn novelty(value: f64) -> Novelty {
    Novelty::new(value).expect("finite novelty")
}
