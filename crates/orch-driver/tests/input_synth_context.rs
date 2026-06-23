use std::collections::BTreeMap;

use orch_clients::{
    input_synth::{
        Burst, BurstBody, BurstId, ConfigFingerprint, GeneratorKind, HealthRequest, HealthResponse,
        InputSynthClient, LoadMacroPackRequest, LoadMacroPackResponse, LoadMacroPackSource,
        MacroProvenance, MineMacrosRequest, MineMacrosResponse, ModelKind, PadBurst, PadSegment,
        ProposeBurstsRequest, ProposeBurstsResponse, Provenance, ProvenancedBurst,
    },
    snapshot_store::{CreateNodeRequest, GetChildrenRequest, InputLogId, SnapshotStoreClient},
    ClientResult,
};
use orch_core::{
    rng::derive_synth_request_seed,
    types::{
        CellKey, FiniteF64, FrameCount, NodeId, NodeStatus, Novelty, Score, SnapshotRef, Stage,
        StateHash,
    },
};
use orch_driver::{
    input_synth::{
        build_propose_bursts_request, propose_bursts_with_fingerprint_guard, FingerprintRegistry,
        ProposeBurstsBuildSpec, SynthBringup,
    },
    node_attrs::{encode_node_attrs, NodeContextLimits, OrchNodeAttrsV1, SynthContextAttrs},
};
use orch_fakes::{
    fault::{FaultPlan, FaultRate},
    snapshot_store::InMemorySnapshotStore,
    synth::FakeSynth,
};

const EXPERIMENT_ID: &str = "phase4-smoke";
const EXPERIMENT_SEED: u64 = 0x0123_4567_89ab_cdef;
const SNAPSHOT_ROOT: SnapshotRef = SnapshotRef::new([0x10; 32]);
const SNAPSHOT_A: SnapshotRef = SnapshotRef::new([0xA1; 32]);
const SNAPSHOT_B: SnapshotRef = SnapshotRef::new([0xB2; 32]);
const SNAPSHOT_C: SnapshotRef = SnapshotRef::new([0xC3; 32]);

#[test]
fn synth_request_context_carries_parent_and_sibling_bursts() {
    let mut store = populated_store();
    let bringup = synth_bringup();
    let mut synth = RecordingSynth::new(FakeSynth::new());
    bringup.run(&mut synth).expect("bring up synth");
    let mut registry = FingerprintRegistry::new();

    let request = request_for(NodeId::new(1), 7, &store);
    let sibling_request = request_for(NodeId::new(2), 7, &store);

    assert_eq!(request.seed, derive_synth_request_seed(EXPERIMENT_SEED, 7));
    assert_eq!(
        request.seed, sibling_request.seed,
        "node_id must not be mixed into the synth request seed"
    );

    let response = propose_bursts_with_fingerprint_guard(
        &mut synth,
        &bringup,
        &mut registry,
        request.clone(),
        1,
    )
    .expect("guarded propose");
    let captured = synth.last_request.as_ref().expect("recorded request");
    let burst_a = sample_provenanced_burst(1, [0xA1; 32]);
    let burst_b = sample_provenanced_burst(2, [0xB2; 32]);

    assert_eq!(captured.node_context.parent_burst, Some(burst_a));
    assert_eq!(captured.node_context.sibling_bursts.len(), 1);
    assert_eq!(captured.node_context.sibling_bursts[0].burst, burst_b);
    assert_eq!(
        captured.node_context.sibling_bursts[0].score_delta,
        finite(13.0)
    );
    assert!(
        !response
            .degraded
            .iter()
            .any(|degraded| degraded.reason == "no_parent_burst"),
        "mutation-only config should see parent/sibling context"
    );

    commit_synth_child(
        &mut store,
        NodeId::new(3),
        NodeId::new(1),
        SNAPSHOT_C,
        response.bursts[0].clone(),
        response.config_fingerprint,
        11.0,
    )
    .expect("commit returned burst");

    let children = store
        .get_children(GetChildrenRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id: NodeId::new(1),
        })
        .expect("children of A");
    assert_eq!(children.children.len(), 1);
}

#[test]
fn fingerprint_mismatch_commits_no_children() {
    let mut store = populated_store();
    let bringup = synth_bringup();
    let request = request_for(NodeId::new(1), 9, &store);
    let mut registry = FingerprintRegistry::new();

    let mut clean = RecordingSynth::new(FakeSynth::new());
    bringup.run(&mut clean).expect("bring up clean synth");
    propose_bursts_with_fingerprint_guard(&mut clean, &bringup, &mut registry, request.clone(), 0)
        .expect("establish expected fingerprint");

    let mut faulty = RecordingSynth::new(FakeSynth::with_fault_plan(
        FaultPlan::disabled(0xBEEF).with_synth_fingerprint_flip(FaultRate::always()),
    ));
    bringup.run(&mut faulty).expect("bring up faulty synth");
    let before = child_count(&store, NodeId::new(1));

    let result = guarded_propose_then_commit_first_child(
        &mut store,
        &mut faulty,
        &bringup,
        &mut registry,
        request,
        NodeId::new(3),
        1,
    );

    let error = result.expect_err("fingerprint mismatch should halt expansion");
    assert_eq!(
        error.kind(),
        orch_clients::ClientErrorKind::FailedPrecondition
    );
    assert_eq!(child_count(&store, NodeId::new(1)), before);
}

#[test]
fn bringup_checks_required_pack_ids_from_config_health() {
    let config = LoadMacroPackSource::DocumentYaml(
        b"version: 1\nkind: experiment_config\nexperiment_id: phase4-smoke\nmodel: pad\ngenerator_mix:\n  weighted_random: 1\nmacro:\n  packs: [required-pack]\n"
            .to_vec(),
    );
    let missing = SynthBringup::from_sources(EXPERIMENT_ID.to_owned(), config.clone(), Vec::new())
        .expect("bringup spec");
    let mut synth = RecordingSynth::new(FakeSynth::new());

    let error = missing
        .run(&mut synth)
        .expect_err("health missing required-pack");

    assert_eq!(
        error.kind(),
        orch_clients::ClientErrorKind::FailedPrecondition
    );

    let with_pack = SynthBringup::from_sources(
        EXPERIMENT_ID.to_owned(),
        config,
        vec![LoadMacroPackSource::DocumentYaml(
            b"version: 1\nkind: macro_pack\nname: required-pack\nmodel: pad\nmacros:\n  - name: dash\n"
                .to_vec(),
        )],
    )
    .expect("bringup spec with pack");
    let report = with_pack.run(&mut synth).expect("bringup with pack");

    assert_eq!(
        report.required_pack_ids,
        ["required-pack".to_owned()].into()
    );
    assert!(report
        .health
        .loaded_packs
        .iter()
        .any(|pack| pack == "required-pack"));
}

fn request_for(
    node_id: NodeId,
    batch_seq: u64,
    store: &InMemorySnapshotStore,
) -> ProposeBurstsRequest {
    build_propose_bursts_request(
        store,
        ProposeBurstsBuildSpec {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id,
            k: 2,
            length_hint: FrameCount::new(8),
            experiment_seed: EXPERIMENT_SEED,
            batch_seq,
            model: ModelKind::Pad,
            config_overrides_yaml: Vec::new(),
            context_limits: NodeContextLimits::default(),
        },
    )
    .expect("build synth request")
}

fn populated_store() -> InMemorySnapshotStore {
    let mut store = InMemorySnapshotStore::new();
    create_node(
        &mut store,
        NodeId::ROOT,
        None,
        SNAPSHOT_ROOT,
        None,
        NodeStatus::Expanded,
        None,
        2.0,
    );
    create_node(
        &mut store,
        NodeId::new(1),
        Some(NodeId::ROOT),
        SNAPSHOT_A,
        Some(sample_provenanced_burst(1, [0xA1; 32])),
        NodeStatus::Frontier,
        Some(InputLogId::new([0xA1; 32])),
        10.0,
    );
    create_node(
        &mut store,
        NodeId::new(2),
        Some(NodeId::ROOT),
        SNAPSHOT_B,
        Some(sample_provenanced_burst(2, [0xB2; 32])),
        NodeStatus::Frontier,
        Some(InputLogId::new([0xB2; 32])),
        15.0,
    );
    store
}

fn create_node(
    store: &mut InMemorySnapshotStore,
    node_id: NodeId,
    parent_node_id: Option<NodeId>,
    snapshot_ref: SnapshotRef,
    created_by_burst: Option<ProvenancedBurst>,
    status: NodeStatus,
    input_log_id: Option<InputLogId>,
    score_value: f64,
) {
    let score = score(score_value);
    let attrs = attrs_for(node_id, created_by_burst, score, None);
    store
        .create_node(CreateNodeRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id,
            parent_node_id,
            snapshot_ref,
            input_log_id,
            status,
            progress_score: score,
            novelty_score: novelty(0.5),
            attrs: encode_node_attrs(&attrs).expect("encode attrs"),
            input_log_container: None,
        })
        .expect("create store node");
}

fn guarded_propose_then_commit_first_child(
    store: &mut InMemorySnapshotStore,
    synth: &mut RecordingSynth,
    bringup: &SynthBringup,
    registry: &mut FingerprintRegistry,
    request: ProposeBurstsRequest,
    child_id: NodeId,
    fingerprint_retry_budget: u32,
) -> ClientResult<()> {
    let parent_id = request.node_context.node_id;
    let response = propose_bursts_with_fingerprint_guard(
        synth,
        bringup,
        registry,
        request,
        fingerprint_retry_budget,
    )?;
    commit_synth_child(
        store,
        child_id,
        parent_id,
        SNAPSHOT_C,
        response.bursts[0].clone(),
        response.config_fingerprint,
        12.0,
    )
}

fn commit_synth_child(
    store: &mut InMemorySnapshotStore,
    node_id: NodeId,
    parent_node_id: NodeId,
    snapshot_ref: SnapshotRef,
    created_by_burst: ProvenancedBurst,
    config_fingerprint: ConfigFingerprint,
    score_value: f64,
) -> ClientResult<()> {
    let score = score(score_value);
    let attrs = attrs_for(
        node_id,
        Some(created_by_burst),
        score,
        Some(config_fingerprint),
    );
    store.create_node(CreateNodeRequest {
        experiment_id: EXPERIMENT_ID.to_owned(),
        node_id,
        parent_node_id: Some(parent_node_id),
        snapshot_ref,
        input_log_id: Some(InputLogId::new([node_id.get() as u8; 32])),
        status: NodeStatus::Frontier,
        progress_score: score,
        novelty_score: novelty(0.5),
        attrs: encode_node_attrs(&attrs)?,
        input_log_container: None,
    })?;
    Ok(())
}

fn attrs_for(
    node_id: NodeId,
    created_by_burst: Option<ProvenancedBurst>,
    score: Score,
    config_fingerprint: Option<ConfigFingerprint>,
) -> OrchNodeAttrsV1 {
    OrchNodeAttrsV1::new(
        orch_clients::hypervisor::Digest32::new([0xD1; 32]),
        orch_clients::hypervisor::DeterminismClass {
            cpu_model: "test-cpu".to_owned(),
            microcode: "test-ucode".to_owned(),
            host_kernel: "test-kernel".to_owned(),
            vmm_version: "test-vmm".to_owned(),
        },
        SynthContextAttrs {
            created_by_burst,
            config_fingerprint,
            decoded_features: BTreeMap::from([(
                "player_x".to_owned(),
                finite(node_id.get() as f64),
            )]),
            frame_counter: FrameCount::new(100 + node_id.get() as u32),
            state_hash: StateHash::new([node_id.get() as u8; 32]),
            cell_key: CellKey::new(40 + node_id.get()),
            stage: Stage::new(1),
            score,
            novelty: novelty(0.5),
            recent_inputs: None,
        },
    )
}

fn synth_bringup() -> SynthBringup {
    SynthBringup::from_sources(
        EXPERIMENT_ID.to_owned(),
        LoadMacroPackSource::DocumentYaml(
            b"version: 1\nkind: experiment_config\nexperiment_id: phase4-smoke\nmodel: pad\nbutton_alphabet: console16-12btn-v1\ngenerator_mix:\n  weighted_random: 0\n  macro: 0\n  mutation: 1\n  policy: 0\n"
                .to_vec(),
        ),
        Vec::new(),
    )
    .expect("bringup spec")
}

fn child_count(store: &InMemorySnapshotStore, node_id: NodeId) -> usize {
    store
        .get_children(GetChildrenRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id,
        })
        .expect("children")
        .children
        .len()
}

fn sample_provenanced_burst(slot: u32, bytes: [u8; 32]) -> ProvenancedBurst {
    let fingerprint = ConfigFingerprint::new([0x44; 32]);
    ProvenancedBurst {
        burst: Burst {
            format_version: 1,
            burst_id: BurstId::new(bytes),
            body: BurstBody::Pad(PadBurst {
                segments: vec![PadSegment {
                    buttons: slot,
                    hold_frames: FrameCount::new(4 + slot),
                }],
                button_alphabet: "console16-12btn-v1".to_owned(),
            }),
        },
        provenance: Provenance {
            generator: GeneratorKind::Macro,
            slot,
            rng_stream: format!("slot/{slot}/macro"),
            config_fingerprint: fingerprint,
            fallback_from: None,
            macro_provenance: Some(MacroProvenance {
                pack_id: "test-pack".to_owned(),
                macro_name: format!("macro-{slot}"),
                param_bindings: BTreeMap::new(),
                macro_frames: FrameCount::new(4),
                tail_frames: FrameCount::new(slot),
                chain_index: 0,
            }),
            mutation_provenance: None,
            policy_provenance: None,
        },
    }
}

fn finite(value: f64) -> FiniteF64 {
    FiniteF64::new(value).expect("finite value")
}

fn score(value: f64) -> Score {
    Score::new(value).expect("finite score")
}

fn novelty(value: f64) -> Novelty {
    Novelty::new(value).expect("finite novelty")
}

#[derive(Debug)]
struct RecordingSynth {
    inner: FakeSynth,
    last_request: Option<ProposeBurstsRequest>,
}

impl RecordingSynth {
    fn new(inner: FakeSynth) -> Self {
        Self {
            inner,
            last_request: None,
        }
    }
}

impl InputSynthClient for RecordingSynth {
    fn load_macro_pack(
        &mut self,
        request: LoadMacroPackRequest,
    ) -> ClientResult<LoadMacroPackResponse> {
        self.inner.load_macro_pack(request)
    }

    fn health(&self, request: HealthRequest) -> ClientResult<HealthResponse> {
        self.inner.health(request)
    }

    fn propose_bursts(
        &mut self,
        request: ProposeBurstsRequest,
    ) -> ClientResult<ProposeBurstsResponse> {
        self.last_request = Some(request.clone());
        self.inner.propose_bursts(request)
    }

    fn mine_macros(&mut self, request: MineMacrosRequest) -> ClientResult<MineMacrosResponse> {
        self.inner.mine_macros(request)
    }
}
