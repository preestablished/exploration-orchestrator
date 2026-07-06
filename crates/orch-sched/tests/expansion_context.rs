//! Request `02-…` §M3 note: re-exercise the input-synth context contract
//! through the real expansion path. Parent and sibling bursts plus
//! score_delta must reach the ProposeBurstsRequest via
//! `build_propose_bursts_request` over committed store state, with children
//! executed through the real pipeline; and a fingerprint-flip fault means
//! no children are committed.

mod support;

use orch_clients::{
    input_synth::{
        HealthRequest, HealthResponse, InputSynthClient, LoadMacroPackRequest,
        LoadMacroPackResponse, LoadMacroPackSource, MineMacrosRequest, MineMacrosResponse,
        ModelKind, ProposeBurstsRequest, ProposeBurstsResponse,
    },
    snapshot_store::{CreateNodeRequest, GetChildrenRequest, SnapshotStoreClient},
    ClientErrorKind, ClientResult,
};
use orch_core::{
    rng::derive_synth_request_seed,
    types::{FrameCount, NodeId, NodeStatus, SchedMode, Score},
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
use orch_sched::{
    driver::JobResult,
    pipeline::{Batch, JobOutcome, Pipeline, PipelineConfig},
    retry::RetryPolicy,
};
use std::time::Duration;
use support::{
    bootstrap_spec, harness, pad_burst, score_batch_with_retry, Harness, HarnessSpec, BUTTON_DOWN,
    BUTTON_RIGHT, EXPERIMENT_ID,
};

const SEED: u64 = 0x5EED;

fn retry() -> RetryPolicy {
    RetryPolicy {
        job_timeout: Duration::from_secs(120),
        retry_max: 3,
        backoff_base: Duration::from_millis(10),
    }
}

fn synth_bringup() -> SynthBringup {
    SynthBringup::from_sources(
        EXPERIMENT_ID.to_owned(),
        LoadMacroPackSource::DocumentYaml(
            format!(
                "version: 1\nkind: experiment_config\nexperiment_id: {EXPERIMENT_ID}\nmodel: pad\nbutton_alphabet: console16-12btn-v1\ngenerator_mix:\n  weighted_random: 0\n  macro: 0\n  mutation: 1\n  policy: 0\n"
            )
            .into_bytes(),
        ),
        Vec::new(),
    )
    .expect("bringup spec")
}

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

struct Committed {
    node_id: NodeId,
    score: Score,
    snapshot: orch_core::types::SnapshotRef,
}

/// Runs one batch through the real pipeline and commits every completed
/// job to the store under `parent`, mirroring the C stage's store writes.
async fn dispatch_and_commit(
    harness: &Harness,
    seq: u64,
    parent: &Committed,
    bursts: Vec<orch_clients::input_synth::ProvenancedBurst>,
    first_child_id: u64,
) -> Vec<Committed> {
    let mut pipeline = Pipeline::spawn(
        harness.driver.clone(),
        PipelineConfig {
            mode: SchedMode::Deterministic,
            max_inflight_batches: 1,
            retry: retry(),
        },
        seq,
    );
    pipeline
        .submit(Batch {
            seq,
            parent: parent.node_id,
            parent_snapshot: parent.snapshot,
            required_class: None,
            bursts,
        })
        .await
        .expect("submit");
    pipeline.close();
    let result = pipeline
        .next_completed()
        .await
        .expect("batch")
        .expect("one batch");

    let jobs: Vec<&JobResult> = result
        .jobs
        .iter()
        .map(|job| match job {
            JobOutcome::Completed(job) => job.as_ref(),
            JobOutcome::Abandoned { job_idx, reason } => {
                panic!("job {job_idx} abandoned: {reason}")
            }
        })
        .collect();

    let scorer = harness.scorer.service();
    let score_results = score_batch_with_retry(
        &scorer,
        &retry(),
        format!("b{seq}"),
        jobs.iter()
            .map(|job| orch_clients::scorer::StateInput {
                node_ref: format!("b{seq}-j{}", job.job_idx),
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
    .await
    .expect("score batch");

    let mut committed = Vec::new();
    for (offset, (job, score_result)) in jobs.iter().zip(&score_results).enumerate() {
        let capture = job.capture.as_ref().expect("capture");
        let node_id = NodeId::new(first_child_id + offset as u64);
        let attrs = OrchNodeAttrsV1::new(
            capture.machine_config_hash,
            capture.determinism_class.clone(),
            SynthContextAttrs {
                created_by_burst: Some(job.burst.clone()),
                config_fingerprint: Some(job.burst.provenance.config_fingerprint),
                decoded_features: Default::default(),
                frame_counter: capture.frame_counter,
                state_hash: capture.state_hash,
                cell_key: score_result.novelty_detail.cell_key,
                stage: score_result.stage,
                score: score_result.progress_score,
                novelty: score_result.novelty_score,
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
                parent_node_id: Some(parent.node_id),
                snapshot_ref: capture.snapshot,
                input_log_id: capture.input_log_id,
                status: NodeStatus::Frontier,
                progress_score: score_result.progress_score,
                novelty_score: score_result.novelty_score,
                attrs: encode_node_attrs(&attrs).expect("encode attrs"),
                input_log_container: None,
            })
            .expect("create child node");
        committed.push(Committed {
            node_id,
            score: score_result.progress_score,
            snapshot: capture.snapshot,
        });
    }
    committed
}

async fn build_request(harness: &Harness, node_id: NodeId, batch_seq: u64) -> ProposeBurstsRequest {
    let store = harness.store.service();
    let store = store.lock().await;
    build_propose_bursts_request(
        &*store,
        ProposeBurstsBuildSpec {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id,
            k: 2,
            length_hint: FrameCount::new(8),
            experiment_seed: SEED,
            batch_seq,
            model: ModelKind::Pad,
            config_overrides_yaml: Vec::new(),
            context_limits: NodeContextLimits::default(),
        },
    )
    .expect("build synth request")
}

async fn child_count(store: &InMemorySnapshotStore, node_id: NodeId) -> usize {
    store
        .get_children(GetChildrenRequest {
            experiment_id: EXPERIMENT_ID.to_owned(),
            node_id,
        })
        .expect("children")
        .children
        .len()
}

#[tokio::test(start_paused = true)]
async fn synth_context_flows_through_the_real_expansion_path() {
    let harness = harness(HarnessSpec {
        experiment_seed: SEED,
        ..HarnessSpec::default()
    })
    .await;

    // Root: bootstrap, score, commit.
    let root = harness
        .driver
        .bootstrap(&bootstrap_spec())
        .await
        .expect("bootstrap");
    let scorer = harness.scorer.service();
    let root_score = score_batch_with_retry(
        &scorer,
        &retry(),
        "root",
        vec![orch_clients::scorer::StateInput {
            node_ref: "root".to_owned(),
            feature_bytes: root.feature_bytes.clone().expect("root features"),
            framebuffer: None,
            fb_meta: None,
        }],
    )
    .await
    .expect("score root")
    .remove(0);
    let root_attrs = OrchNodeAttrsV1::new(
        root.machine_config_hash,
        root.determinism_class.clone(),
        SynthContextAttrs {
            created_by_burst: None,
            config_fingerprint: None,
            decoded_features: Default::default(),
            frame_counter: root.frame_counter,
            state_hash: root.state_hash,
            cell_key: root_score.novelty_detail.cell_key,
            stage: root_score.stage,
            score: root_score.progress_score,
            novelty: root_score.novelty_score,
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
            node_id: NodeId::ROOT,
            parent_node_id: None,
            snapshot_ref: root.snapshot,
            input_log_id: None,
            status: NodeStatus::Expanded,
            progress_score: root_score.progress_score,
            novelty_score: root_score.novelty_score,
            attrs: encode_node_attrs(&root_attrs).expect("encode root attrs"),
            input_log_container: None,
        })
        .expect("create root node");
    let root_committed = Committed {
        node_id: NodeId::ROOT,
        score: root_score.progress_score,
        snapshot: root.snapshot,
    };

    // First expansion: two fixed bursts through the real pipeline, committed
    // as nodes 1 and 2 (each carrying created_by_burst in its attrs).
    let children = dispatch_and_commit(
        &harness,
        0,
        &root_committed,
        vec![pad_burst(0, BUTTON_RIGHT, 2), pad_burst(1, BUTTON_DOWN, 3)],
        1,
    )
    .await;
    assert_eq!(children.len(), 2);

    // S stage for node 1 over committed store state: the request must carry
    // node 1's own burst as parent_burst and node 2's as a sibling with
    // score_delta = sibling score - shared parent (root) score.
    let request = build_request(&harness, NodeId::new(1), 1).await;
    assert_eq!(request.seed, derive_synth_request_seed(SEED, 1));
    let parent_burst = request
        .node_context
        .parent_burst
        .as_ref()
        .expect("parent burst present");
    assert_eq!(
        parent_burst.burst.burst_id,
        pad_burst(0, BUTTON_RIGHT, 2).burst.burst_id
    );
    assert_eq!(request.node_context.sibling_bursts.len(), 1);
    let sibling = &request.node_context.sibling_bursts[0];
    assert_eq!(
        sibling.burst.burst.burst_id,
        pad_burst(1, BUTTON_DOWN, 3).burst.burst_id
    );
    let expected_delta = children[1].score.get() - root_committed.score.get();
    assert!(
        (sibling.score_delta.get() - expected_delta).abs() < 1e-9,
        "sibling score_delta {} != committed delta {expected_delta}",
        sibling.score_delta.get()
    );

    // Guarded propose against the real synth, then dispatch the returned
    // bursts through the pipeline and commit under node 1.
    let bringup = synth_bringup();
    let mut synth = RecordingSynth::new(FakeSynth::new());
    bringup.run(&mut synth).expect("bring up synth");
    let mut registry = FingerprintRegistry::new();
    let response =
        propose_bursts_with_fingerprint_guard(&mut synth, &bringup, &mut registry, request, 1)
            .expect("guarded propose");
    let captured = synth.last_request.as_ref().expect("recorded request");
    assert!(captured.node_context.parent_burst.is_some());
    assert_eq!(captured.node_context.sibling_bursts.len(), 1);
    assert!(
        !response
            .degraded
            .iter()
            .any(|degraded| degraded.reason == "no_parent_burst"),
        "mutation generator must see the parent burst context"
    );

    let node1 = &children[0];
    let grandchildren = dispatch_and_commit(&harness, 1, node1, response.bursts.clone(), 3).await;
    assert_eq!(grandchildren.len(), response.bursts.len());
    {
        let store = harness.store.service();
        let store = store.lock().await;
        assert_eq!(
            child_count(&store, NodeId::new(1)).await,
            response.bursts.len()
        );
    }

    // Fingerprint-flip fault: the guard refuses the response, nothing is
    // dispatched, and no children are committed under node 2.
    let mut flipping = RecordingSynth::new(FakeSynth::with_fault_plan(
        FaultPlan::disabled(0xBEEF).with_synth_fingerprint_flip(FaultRate::always()),
    ));
    bringup.run(&mut flipping).expect("bring up flipping synth");
    let flip_request = build_request(&harness, NodeId::new(2), 2).await;
    let error = propose_bursts_with_fingerprint_guard(
        &mut flipping,
        &bringup,
        &mut registry,
        flip_request,
        0,
    )
    .expect_err("fingerprint mismatch must halt the expansion");
    assert_eq!(error.kind(), ClientErrorKind::FailedPrecondition);
    {
        let store = harness.store.service();
        let store = store.lock().await;
        assert_eq!(child_count(&store, NodeId::new(2)).await, 0);
    }
    harness.drain.abort();
}
