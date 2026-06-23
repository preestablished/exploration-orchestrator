use std::collections::BTreeMap;

use orch_clients::{
    input_synth::{
        Burst, BurstBody, BurstId, ConfigFingerprint, DocumentKind, EventBurst, FieldValue,
        GeneratorKind, GrammarEvent, GrammarField, HealthStatus, LoadMacroPackRequest,
        LoadMacroPackSource, MacroProvenance, MineMacrosRequest, MiningParams, ModelKind,
        MutationOp, MutationProvenance, NodeContext, PadBurst, PadSegment, PathSample,
        PolicyProvenance, ProposeBurstsRequest, Provenance, ProvenancedBurst, ScoredBurst,
    },
    ClientErrorKind,
};
use orch_core::types::{
    CellKey, FiniteF64, FrameCount, NodeId, Novelty, Score, SnapshotRef, Stage, StateHash,
};
use orch_driver::input_synth::{
    dto_to_wire_load_macro_pack_request, dto_to_wire_mine_macros_request,
    dto_to_wire_propose_bursts_request, node_id_from_wire, node_id_to_wire, snapshot_ref_from_wire,
    snapshot_ref_to_wire, tonic_code_to_client_error_kind, wire_to_dto_health_response,
    wire_to_dto_mine_macros_response, wire_to_dto_propose_bursts_response,
    wire_to_dto_scored_burst,
};
use orch_proto::inputsynth::v1 as wire;
use tonic::Code;

const FP_A: ConfigFingerprint = ConfigFingerprint::new([0xA5; 32]);
const FP_B: ConfigFingerprint = ConfigFingerprint::new([0x5A; 32]);
const SNAPSHOT: SnapshotRef = SnapshotRef::new([0xAB; 32]);

#[test]
fn node_context_request_conversion_sends_only_owner_wire_fields() {
    let request = sample_request(2);
    let wire = dto_to_wire_propose_bursts_request(&request).expect("wire request");
    let context = wire.node_context.expect("node context");

    assert_eq!(context.node_id, "7");
    assert_eq!(context.snapshot_ref, "ab".repeat(32));
    assert_eq!(context.depth, 4);
    assert_eq!(context.node_score, 12.0);
    assert_eq!(context.novelty, 0.5);
    assert_eq!(context.ram_features.get("boss_hp"), Some(&80.0));
    assert!(context.frame_embedding.is_empty());
    assert!(context.recent_inputs.is_some());
    assert!(context.parent_burst.is_some());
    assert_eq!(context.sibling_bursts.len(), 1);
}

#[test]
fn node_context_rejects_local_frame_embedding_until_phase8() {
    let mut request = sample_request(1);
    request.node_context.frame_embedding = vec![finite(0.125)];

    let error =
        dto_to_wire_propose_bursts_request(&request).expect_err("frame_embedding must be rejected");

    assert_eq!(error.kind(), ClientErrorKind::InvalidRequest);
}

#[test]
fn node_id_and_snapshot_ref_wire_encodings_are_strict() {
    assert_eq!(node_id_to_wire(NodeId::new(42)), "42");
    assert_eq!(
        node_id_from_wire("42", ClientErrorKind::DataLoss).expect("node id"),
        NodeId::new(42)
    );
    for bad in ["", "-1", "abc", "18446744073709551616"] {
        assert_eq!(
            node_id_from_wire(bad, ClientErrorKind::DataLoss)
                .expect_err("bad node id")
                .kind(),
            ClientErrorKind::DataLoss
        );
    }

    let encoded = snapshot_ref_to_wire(SNAPSHOT);
    assert_eq!(encoded, "ab".repeat(32));
    assert_eq!(
        snapshot_ref_from_wire(&encoded, ClientErrorKind::DataLoss).expect("snapshot"),
        SNAPSHOT
    );
    for bad in [
        "ab",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    ] {
        assert_eq!(
            snapshot_ref_from_wire(bad, ClientErrorKind::DataLoss)
                .expect_err("bad snapshot")
                .kind(),
            ClientErrorKind::DataLoss
        );
    }
}

#[test]
fn load_health_and_status_mappings_are_strict() {
    let inline = dto_to_wire_load_macro_pack_request(&LoadMacroPackRequest {
        source: LoadMacroPackSource::DocumentYaml(b"macro: pack\n".to_vec()),
        kind: DocumentKind::MacroPack,
    })
    .expect("inline load");
    assert!(matches!(
        inline.source,
        Some(wire::load_macro_pack_request::Source::DocumentYaml(_))
    ));
    assert_eq!(inline.kind, wire::DocumentKind::MacroPack as i32);

    let artifact = dto_to_wire_load_macro_pack_request(&LoadMacroPackRequest {
        source: LoadMacroPackSource::ArtifactRef("artifact://pack".to_owned()),
        kind: DocumentKind::ExperimentConfig,
    })
    .expect("artifact load");
    assert!(matches!(
        artifact.source,
        Some(wire::load_macro_pack_request::Source::ArtifactRef(_))
    ));
    assert_eq!(artifact.kind, wire::DocumentKind::ExperimentConfig as i32);

    let health = wire_to_dto_health_response(wire::HealthResponse {
        status: wire::health_response::Status::Degraded as i32,
        synth_version: "test".to_owned(),
        loaded_packs: vec!["pack-a".to_owned()],
        loaded_experiments: vec!["exp-a".to_owned()],
        policy_endpoint_up: false,
        policy_deterministic: true,
        mining_in_progress: false,
    })
    .expect("health");
    assert_eq!(health.status, HealthStatus::Degraded);

    for status in [wire::health_response::Status::Unspecified as i32, 99] {
        assert_eq!(
            wire_to_dto_health_response(wire::HealthResponse {
                status,
                synth_version: String::new(),
                loaded_packs: Vec::new(),
                loaded_experiments: Vec::new(),
                policy_endpoint_up: false,
                policy_deterministic: false,
                mining_in_progress: false,
            })
            .expect_err("bad status")
            .kind(),
            ClientErrorKind::DataLoss
        );
    }
}

#[test]
fn propose_response_validates_seed_exact_k_slots_and_fingerprints() {
    let request = sample_request(2);
    let good = sample_wire_response(&request, FP_A);
    let response = wire_to_dto_propose_bursts_response(&request, good).expect("good response");

    assert_eq!(response.seed, request.seed);
    assert_eq!(response.bursts.len(), 2);
    assert_eq!(response.bursts[0].provenance.slot, 0);
    assert_eq!(response.bursts[1].provenance.slot, 1);

    let mut bad_seed = sample_wire_response(&request, FP_A);
    bad_seed.seed += 1;
    assert_data_loss(&request, bad_seed);

    let mut wrong_len = sample_wire_response(&request, FP_A);
    wrong_len.bursts.pop();
    assert_data_loss(&request, wrong_len);

    let mut wrong_slot = sample_wire_response(&request, FP_A);
    wrong_slot.bursts[1]
        .provenance
        .as_mut()
        .expect("provenance")
        .slot = 0;
    assert_data_loss(&request, wrong_slot);

    let mut wrong_fingerprint = sample_wire_response(&request, FP_A);
    wrong_fingerprint.bursts[0]
        .provenance
        .as_mut()
        .expect("provenance")
        .config_fingerprint = FP_B.as_bytes().to_vec();
    assert_data_loss(&request, wrong_fingerprint);
}

#[test]
fn generated_missing_oneofs_and_fixed_byte_lengths_are_data_loss() {
    let request = sample_request(1);

    let mut missing_body = sample_wire_response(&request, FP_A);
    missing_body.bursts[0].burst.as_mut().expect("burst").body = None;
    assert_data_loss(&request, missing_body);

    let mut missing_provenance = sample_wire_response(&request, FP_A);
    missing_provenance.bursts[0].provenance = None;
    assert_data_loss(&request, missing_provenance);

    let mut short_burst_id = sample_wire_response(&request, FP_A);
    short_burst_id.bursts[0]
        .burst
        .as_mut()
        .expect("burst")
        .burst_id = vec![1, 2, 3];
    assert_data_loss(&request, short_burst_id);

    let mut short_fingerprint = sample_wire_response(&request, FP_A);
    short_fingerprint.config_fingerprint = vec![0; 31];
    assert_data_loss(&request, short_fingerprint);
}

#[test]
fn mutation_provenance_ids_are_length_checked_on_request_and_response() {
    let mut request = sample_request(1);
    request
        .node_context
        .parent_burst
        .as_mut()
        .unwrap()
        .provenance = sample_provenance(0, FP_A, Some(vec![1, 2, 3]));

    let error =
        dto_to_wire_propose_bursts_request(&request).expect_err("short mutation base id rejected");
    assert_eq!(error.kind(), ClientErrorKind::InvalidRequest);

    let request = sample_request(1);
    let mut response = sample_wire_response(&request, FP_A);
    response.bursts[0]
        .provenance
        .as_mut()
        .expect("provenance")
        .mutation = Some(wire::MutationProvenance {
        base_burst_id: vec![1, 2, 3],
        donor_burst_id: Vec::new(),
        base_was_sibling: false,
        ops: Vec::new(),
        post_clamp: true,
    });
    assert_data_loss(&request, response);
}

#[test]
fn generated_non_finite_response_numbers_are_data_loss() {
    let request = sample_request(1);
    let mut response = sample_wire_response(&request, FP_A);
    response.bursts[0]
        .provenance
        .as_mut()
        .expect("provenance")
        .policy = Some(wire::PolicyProvenance {
        model_id: "policy".to_owned(),
        model_version: "v1".to_owned(),
        temperature: f64::NAN,
        server_attested_deterministic: true,
    });
    assert_data_loss(&request, response);

    let scored_error = wire_to_dto_scored_burst(wire::ScoredBurst {
        burst: Some(wire::ProvenancedBurst {
            burst: Some(sample_wire_burst(0)),
            provenance: Some(sample_wire_provenance(0, FP_A)),
        }),
        score_delta: f64::INFINITY,
    })
    .expect_err("non-finite score_delta");
    assert_eq!(scored_error.kind(), ClientErrorKind::DataLoss);

    let mined_error = wire_to_dto_mine_macros_response(wire::MineMacrosResponse {
        macro_pack_yaml: Vec::new(),
        pack_id: "pack".to_owned(),
        stats: vec![wire::MinedMacroStats {
            name: "m".to_owned(),
            support: 1,
            paths: 1,
            lift: f64::NEG_INFINITY,
            score: 1.0,
            len_tokens: 4,
        }],
        paths_used: 1,
        tokens_scanned: 4,
    })
    .expect_err("non-finite lift");
    assert_eq!(mined_error.kind(), ClientErrorKind::DataLoss);
}

#[test]
fn mine_macros_request_preserves_optional_params_and_paths() {
    let request = MineMacrosRequest {
        experiment_id: "exp-a".to_owned(),
        paths: vec![PathSample {
            expansions: vec![ScoredBurst {
                burst: sample_provenanced_burst(0, FP_A),
                score_delta: finite(2.5),
            }],
            terminal_score: score(14.0),
        }],
        params: MiningParams {
            min_support: Some(2),
            min_paths: None,
            max_len_tokens: Some(24),
            max_macros: None,
            containment_alpha: Some(finite(0.8)),
            dedup_edit_dist: None,
        },
    };

    let wire = dto_to_wire_mine_macros_request(&request).expect("mine request");
    let params = wire.params.expect("params");

    assert_eq!(wire.paths.len(), 1);
    assert_eq!(params.min_support, Some(2));
    assert_eq!(params.min_paths, None);
    assert_eq!(params.max_len_tokens, Some(24));
    assert_eq!(params.containment_alpha, Some(0.8));
    assert_eq!(params.dedup_edit_dist, None);
}

#[test]
fn tonic_status_mapping_matches_client_error_contract() {
    assert_eq!(
        tonic_code_to_client_error_kind(Code::InvalidArgument),
        ClientErrorKind::InvalidRequest
    );
    assert_eq!(
        tonic_code_to_client_error_kind(Code::FailedPrecondition),
        ClientErrorKind::FailedPrecondition
    );
    assert_eq!(
        tonic_code_to_client_error_kind(Code::NotFound),
        ClientErrorKind::NotFound
    );
    assert_eq!(
        tonic_code_to_client_error_kind(Code::AlreadyExists),
        ClientErrorKind::AlreadyExists
    );
    assert_eq!(
        tonic_code_to_client_error_kind(Code::ResourceExhausted),
        ClientErrorKind::ResourceExhausted
    );
    assert_eq!(
        tonic_code_to_client_error_kind(Code::Unavailable),
        ClientErrorKind::Unavailable
    );
    assert_eq!(
        tonic_code_to_client_error_kind(Code::DeadlineExceeded),
        ClientErrorKind::Unavailable
    );
    assert_eq!(
        tonic_code_to_client_error_kind(Code::Internal),
        ClientErrorKind::Internal
    );
}

fn assert_data_loss(request: &ProposeBurstsRequest, response: wire::ProposeBurstsResponse) {
    let error = wire_to_dto_propose_bursts_response(request, response).expect_err("data loss");
    assert_eq!(error.kind(), ClientErrorKind::DataLoss);
}

fn sample_request(k: u32) -> ProposeBurstsRequest {
    ProposeBurstsRequest {
        experiment_id: "exp-a".to_owned(),
        node_context: NodeContext {
            node_id: NodeId::new(7),
            parent_node_id: Some(NodeId::ROOT),
            snapshot_ref: SNAPSHOT,
            state_hash: StateHash::new([0x11; 32]),
            cell_key: CellKey::new(42),
            stage: Stage::new(3),
            depth: 4,
            frame_counter: FrameCount::new(900),
            node_score: score(12.0),
            novelty: novelty(0.5),
            ram_features: BTreeMap::from([("boss_hp".to_owned(), finite(80.0))]),
            frame_embedding: Vec::new(),
            recent_inputs: Some(sample_burst(9)),
            parent_burst: Some(sample_provenanced_burst(0, FP_A)),
            sibling_bursts: vec![ScoredBurst {
                burst: sample_provenanced_burst(1, FP_A),
                score_delta: finite(1.5),
            }],
        },
        k,
        length_hint: FrameCount::new(8),
        seed: 99,
        model: ModelKind::Pad,
        config_overrides_yaml: Vec::new(),
    }
}

fn sample_wire_response(
    request: &ProposeBurstsRequest,
    fingerprint: ConfigFingerprint,
) -> wire::ProposeBurstsResponse {
    wire::ProposeBurstsResponse {
        bursts: (0..request.k)
            .map(|slot| wire::ProvenancedBurst {
                burst: Some(sample_wire_burst(slot)),
                provenance: Some(sample_wire_provenance(slot, fingerprint)),
            })
            .collect(),
        config_fingerprint: fingerprint.as_bytes().to_vec(),
        synth_version: "wire-test".to_owned(),
        seed: request.seed,
        degraded: Vec::new(),
    }
}

fn sample_wire_burst(slot: u32) -> wire::Burst {
    wire::Burst {
        format_version: wire::BURST_FORMAT_VERSION,
        burst_id: [slot as u8; 32].to_vec(),
        body: Some(wire::burst::Body::Pad(wire::PadBurst {
            segments: vec![wire::PadSegment {
                buttons: slot,
                hold_frames: 4,
            }],
            button_alphabet: "console16-12btn-v1".to_owned(),
        })),
    }
}

fn sample_wire_provenance(slot: u32, fingerprint: ConfigFingerprint) -> wire::Provenance {
    wire::Provenance {
        generator: wire::GeneratorKind::Macro as i32,
        slot,
        rng_stream: format!("slot/{slot}/macro"),
        config_fingerprint: fingerprint.as_bytes().to_vec(),
        fallback_from: wire::GeneratorKind::Unspecified as i32,
        r#macro: Some(wire::MacroProvenance {
            pack_id: "pack-a".to_owned(),
            macro_name: "dash".to_owned(),
            param_bindings: Default::default(),
            macro_frames: 4,
            tail_frames: 0,
            chain_index: 0,
        }),
        mutation: None,
        policy: None,
    }
}

fn sample_provenanced_burst(slot: u32, fingerprint: ConfigFingerprint) -> ProvenancedBurst {
    ProvenancedBurst {
        burst: sample_burst(slot),
        provenance: sample_provenance(slot, fingerprint, None),
    }
}

fn sample_burst(slot: u32) -> Burst {
    Burst {
        format_version: wire::BURST_FORMAT_VERSION,
        burst_id: BurstId::new([slot as u8; 32]),
        body: if slot == 9 {
            BurstBody::Event(EventBurst {
                events: vec![GrammarEvent {
                    event_type: "event".to_owned(),
                    at_offset_ns: 10,
                    fields: vec![GrammarField {
                        name: "value".to_owned(),
                        value: FieldValue::Int(7),
                    }],
                    payload: vec![1, 2, 3],
                }],
                grammar_id: "grammar-a".to_owned(),
            })
        } else {
            BurstBody::Pad(PadBurst {
                segments: vec![PadSegment {
                    buttons: slot,
                    hold_frames: FrameCount::new(4),
                }],
                button_alphabet: "console16-12btn-v1".to_owned(),
            })
        },
    }
}

fn sample_provenance(
    slot: u32,
    fingerprint: ConfigFingerprint,
    mutation_base: Option<Vec<u8>>,
) -> Provenance {
    Provenance {
        generator: if mutation_base.is_some() {
            GeneratorKind::Mutation
        } else {
            GeneratorKind::Macro
        },
        slot,
        rng_stream: format!("slot/{slot}"),
        config_fingerprint: fingerprint,
        fallback_from: Some(GeneratorKind::Policy),
        macro_provenance: mutation_base.is_none().then(|| MacroProvenance {
            pack_id: "pack-a".to_owned(),
            macro_name: "dash".to_owned(),
            param_bindings: BTreeMap::new(),
            macro_frames: FrameCount::new(4),
            tail_frames: FrameCount::new(0),
            chain_index: 0,
        }),
        mutation_provenance: mutation_base.map(|base_burst_id| MutationProvenance {
            base_burst_id,
            donor_burst_id: Vec::new(),
            base_was_sibling: false,
            ops: vec![MutationOp {
                op: "flip_button".to_owned(),
                args: BTreeMap::new(),
            }],
            post_clamp: true,
        }),
        policy_provenance: Some(PolicyProvenance {
            model_id: "policy".to_owned(),
            model_version: "v1".to_owned(),
            temperature: finite(0.8),
            server_attested_deterministic: true,
        }),
    }
}

fn finite(value: f64) -> FiniteF64 {
    FiniteF64::new(value).expect("finite")
}

fn score(value: f64) -> Score {
    Score::new(value).expect("finite score")
}

fn novelty(value: f64) -> Novelty {
    Novelty::new(value).expect("finite novelty")
}
