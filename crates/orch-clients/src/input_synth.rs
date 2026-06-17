//! Input synthesizer client boundary.
//!
//! Owner docs: `/home/infra-admin/.agents/projects/determinism/docs/input-synthesizer/API.md`
//! section 2 and
//! `/home/infra-admin/.agents/projects/determinism/docs/exploration-orchestrator/API.md`
//! section 4.
//!
//! This module mirrors macro-pack/config/grammar loading, health, burst proposal,
//! macro mining, provenance, and degraded-generator shapes without choosing a
//! transport implementation.

use std::collections::BTreeMap;

use orch_core::types::{
    CellKey, FiniteF64, FrameCount, NodeId, Novelty, Score, SnapshotRef, Stage, StateHash,
};
use serde::{Deserialize, Serialize};

use crate::ClientResult;

pub trait InputSynthClient {
    fn load_macro_pack(
        &mut self,
        request: LoadMacroPackRequest,
    ) -> ClientResult<LoadMacroPackResponse>;

    fn health(&self, request: HealthRequest) -> ClientResult<HealthResponse>;

    fn propose_bursts(
        &mut self,
        request: ProposeBurstsRequest,
    ) -> ClientResult<ProposeBurstsResponse>;

    fn mine_macros(&mut self, request: MineMacrosRequest) -> ClientResult<MineMacrosResponse>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadMacroPackRequest {
    pub source: LoadMacroPackSource,
    pub kind: DocumentKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadMacroPackSource {
    DocumentYaml(Vec<u8>),
    ArtifactRef(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadMacroPackResponse {
    pub document_id: String,
    pub items_loaded: u32,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthRequest;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: HealthStatus,
    pub synth_version: String,
    pub loaded_packs: Vec<String>,
    pub loaded_experiments: Vec<String>,
    pub policy_endpoint_up: bool,
    pub policy_deterministic: bool,
    pub mining_in_progress: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposeBurstsRequest {
    pub experiment_id: String,
    pub node_context: NodeContext,
    pub k: u32,
    pub length_hint: FrameCount,
    pub seed: u64,
    pub model: ModelKind,
    pub config_overrides_yaml: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposeBurstsResponse {
    pub bursts: Vec<ProvenancedBurst>,
    pub config_fingerprint: ConfigFingerprint,
    pub synth_version: String,
    pub seed: u64,
    pub degraded: Vec<DegradedGenerator>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MineMacrosRequest {
    pub experiment_id: String,
    pub paths: Vec<PathSample>,
    pub params: MiningParams,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MineMacrosResponse {
    pub macro_pack_yaml: Vec<u8>,
    pub pack_id: String,
    pub stats: Vec<MinedMacroStats>,
    pub paths_used: u32,
    pub tokens_scanned: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeContext {
    pub node_id: NodeId,
    pub parent_node_id: Option<NodeId>,
    pub snapshot_ref: SnapshotRef,
    pub state_hash: StateHash,
    pub cell_key: CellKey,
    pub stage: Stage,
    pub depth: u32,
    pub frame_counter: FrameCount,
    pub node_score: Score,
    pub novelty: Novelty,
    pub ram_features: BTreeMap<String, FiniteF64>,
    pub frame_embedding: Vec<FiniteF64>,
    pub recent_inputs: Option<Burst>,
    pub parent_burst: Option<ProvenancedBurst>,
    pub sibling_bursts: Vec<ScoredBurst>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoredBurst {
    pub burst: ProvenancedBurst,
    pub score_delta: FiniteF64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProvenancedBurst {
    pub burst: Burst,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Burst {
    pub format_version: u32,
    pub pad_segments: Vec<PadSegment>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadSegment {
    pub start_frame: FrameCount,
    pub frames: FrameCount,
    pub buttons: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub generator: GeneratorKind,
    pub slot: u32,
    pub rng_stream: String,
    pub config_fingerprint: ConfigFingerprint,
    pub fallback_from: Option<GeneratorKind>,
    pub macro_provenance: Option<MacroProvenance>,
    pub mutation_provenance: Option<MutationProvenance>,
    pub policy_provenance: Option<PolicyProvenance>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroProvenance {
    pub pack_id: String,
    pub macro_name: String,
    pub param_bindings: BTreeMap<String, String>,
    pub macro_frames: FrameCount,
    pub tail_frames: FrameCount,
    pub chain_index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationProvenance {
    pub base_burst_id: Vec<u8>,
    pub donor_burst_id: Vec<u8>,
    pub base_was_sibling: bool,
    pub ops: Vec<MutationOp>,
    pub post_clamp: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationOp {
    pub op: String,
    pub args: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyProvenance {
    pub model_id: String,
    pub model_version: String,
    pub temperature: FiniteF64,
    pub server_attested_deterministic: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathSample {
    pub expansions: Vec<ScoredBurst>,
    pub terminal_score: Score,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MiningParams {
    pub min_support: Option<u32>,
    pub min_paths: Option<u32>,
    pub max_len_tokens: Option<u32>,
    pub max_macros: Option<u32>,
    pub containment_alpha: Option<FiniteF64>,
    pub dedup_edit_dist: Option<FiniteF64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MinedMacroStats {
    pub name: String,
    pub support: u32,
    pub paths: u32,
    pub lift: FiniteF64,
    pub score: Score,
    pub len_tokens: u32,
}

pub const CONFIG_FINGERPRINT_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConfigFingerprint(pub [u8; CONFIG_FINGERPRINT_LEN]);

impl ConfigFingerprint {
    #[must_use]
    pub const fn new(bytes: [u8; CONFIG_FINGERPRINT_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; CONFIG_FINGERPRINT_LEN] {
        &self.0
    }

    #[must_use]
    pub const fn into_bytes(self) -> [u8; CONFIG_FINGERPRINT_LEN] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocumentKind {
    MacroPack,
    ExperimentConfig,
    EventGrammar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Serving,
    Degraded,
    NotServing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelKind {
    Pad,
    EventGrammar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeneratorKind {
    WeightedRandom,
    Macro,
    Mutation,
    Policy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DegradedGenerator {
    pub generator: GeneratorKind,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP_A: ConfigFingerprint = ConfigFingerprint::new([0xA5; CONFIG_FINGERPRINT_LEN]);
    const FP_B: ConfigFingerprint = ConfigFingerprint::new([0x5A; CONFIG_FINGERPRINT_LEN]);
    const SNAPSHOT: SnapshotRef = SnapshotRef::new([1; 32]);
    const STATE: StateHash = StateHash::new([2; 32]);

    #[test]
    fn input_synth_load_macro_pack_models_inline_and_artifact_sources() {
        let inline = LoadMacroPackRequest {
            source: LoadMacroPackSource::DocumentYaml(b"macros:\n  - dash\n".to_vec()),
            kind: DocumentKind::MacroPack,
        };
        let artifact = LoadMacroPackRequest {
            source: LoadMacroPackSource::ArtifactRef("artifact://synth/config/v1".to_owned()),
            kind: DocumentKind::ExperimentConfig,
        };
        let response = LoadMacroPackResponse {
            document_id: "pack-a5".to_owned(),
            items_loaded: 4,
            warnings: vec!["macro 'dash' shadows pack base".to_owned()],
        };

        assert!(matches!(
            inline.source,
            LoadMacroPackSource::DocumentYaml(_)
        ));
        assert_eq!(artifact.kind, DocumentKind::ExperimentConfig);
        assert_eq!(response.document_id, "pack-a5");
        assert_eq!(response.items_loaded, 4);
        assert_eq!(response.warnings, ["macro 'dash' shadows pack base"]);
    }

    #[test]
    fn input_synth_health_reports_bringup_state() {
        let health = HealthResponse {
            status: HealthStatus::Degraded,
            synth_version: "fake-synth/0.1".to_owned(),
            loaded_packs: vec!["pack-a5".to_owned()],
            loaded_experiments: vec!["exp-a".to_owned()],
            policy_endpoint_up: false,
            policy_deterministic: false,
            mining_in_progress: true,
        };

        assert_eq!(health.status, HealthStatus::Degraded);
        assert_eq!(health.loaded_packs, ["pack-a5"]);
        assert_eq!(health.loaded_experiments, ["exp-a"]);
        assert!(!health.policy_endpoint_up);
        assert!(!health.policy_deterministic);
        assert!(health.mining_in_progress);
    }

    #[test]
    fn input_synth_node_context_carries_generation_context() {
        let context = sample_context();

        assert_eq!(context.node_id, NodeId::new(7));
        assert_eq!(context.parent_node_id, Some(NodeId::ROOT));
        assert_eq!(context.snapshot_ref, SNAPSHOT);
        assert_eq!(context.state_hash, STATE);
        assert_eq!(context.cell_key, CellKey::new(42));
        assert_eq!(context.stage, Stage::new(3));
        assert_eq!(context.depth, 4);
        assert_eq!(context.frame_counter, FrameCount::new(900));
        assert_eq!(context.node_score, score(12.0));
        assert_eq!(context.novelty, novelty(0.5));
        assert_eq!(
            context.ram_features.get("player_x").copied(),
            Some(finite(12.0))
        );
        assert_eq!(context.frame_embedding, [finite(0.125), finite(0.25)]);
        assert!(context.recent_inputs.is_some());
        assert!(context.parent_burst.is_some());
        assert_eq!(context.sibling_bursts[0].score_delta, finite(1.5));
    }

    #[test]
    fn input_synth_propose_request_uses_seed_model_and_yaml_overrides() {
        let request = ProposeBurstsRequest {
            experiment_id: "exp-a".to_owned(),
            node_context: sample_context(),
            k: 16,
            length_hint: FrameCount::new(120),
            seed: 99,
            model: ModelKind::Pad,
            config_overrides_yaml: b"generator_mix:\n  macro: 0.75\n".to_vec(),
        };

        assert_eq!(request.experiment_id, "exp-a");
        assert_eq!(request.k, 16);
        assert_eq!(request.length_hint, FrameCount::new(120));
        assert_eq!(request.seed, 99);
        assert_eq!(request.model, ModelKind::Pad);
        assert_eq!(
            request.config_overrides_yaml,
            b"generator_mix:\n  macro: 0.75\n"
        );
    }

    #[test]
    fn input_synth_response_echoes_seed_fingerprint_and_degraded_generators() {
        let burst = sample_provenanced_burst(3, FP_A);
        let response = ProposeBurstsResponse {
            bursts: vec![burst],
            config_fingerprint: FP_A,
            synth_version: "fake-synth/0.1".to_owned(),
            seed: 99,
            degraded: vec![DegradedGenerator {
                generator: GeneratorKind::Policy,
                reason: "policy_endpoint_down".to_owned(),
            }],
        };

        assert_eq!(response.seed, 99);
        assert_eq!(response.config_fingerprint, FP_A);
        assert_eq!(response.bursts[0].provenance.slot, 3);
        assert_eq!(response.bursts[0].provenance.rng_stream, "slot/3/macro");
        assert_eq!(response.bursts[0].provenance.config_fingerprint, FP_A);
        assert_eq!(
            response.bursts[0].provenance.fallback_from,
            Some(GeneratorKind::Policy)
        );
        assert_eq!(response.degraded[0].generator, GeneratorKind::Policy);
    }

    #[test]
    fn input_synth_provenance_carries_macro_mutation_and_policy_details() {
        let macro_burst = sample_provenanced_burst(1, FP_A);
        let macro_provenance = macro_burst
            .provenance
            .macro_provenance
            .as_ref()
            .expect("macro provenance");
        assert_eq!(macro_provenance.pack_id, "pack-a5");
        assert_eq!(
            macro_provenance
                .param_bindings
                .get("direction")
                .map(String::as_str),
            Some("right")
        );

        let mutation = MutationProvenance {
            base_burst_id: vec![1, 2, 3],
            donor_burst_id: vec![4, 5, 6],
            base_was_sibling: true,
            ops: vec![MutationOp {
                op: "splice".to_owned(),
                args: BTreeMap::from([("cut".to_owned(), "3".to_owned())]),
            }],
            post_clamp: true,
        };
        let policy = PolicyProvenance {
            model_id: "policy-a".to_owned(),
            model_version: "2026-06-17".to_owned(),
            temperature: finite(0.8),
            server_attested_deterministic: true,
        };

        assert_eq!(mutation.ops[0].op, "splice");
        assert_eq!(policy.temperature, finite(0.8));
        assert!(policy.server_attested_deterministic);
    }

    #[test]
    fn input_synth_mine_macro_shapes_cover_scored_corpus_and_output_yaml() {
        let request = MineMacrosRequest {
            experiment_id: "exp-a".to_owned(),
            paths: vec![PathSample {
                expansions: vec![ScoredBurst {
                    burst: sample_provenanced_burst(0, FP_A),
                    score_delta: finite(2.25),
                }],
                terminal_score: score(14.0),
            }],
            params: MiningParams {
                min_support: Some(2),
                min_paths: Some(1),
                max_len_tokens: Some(24),
                max_macros: Some(8),
                containment_alpha: Some(finite(0.8)),
                dedup_edit_dist: Some(finite(0.2)),
            },
        };
        let response = MineMacrosResponse {
            macro_pack_yaml: b"macros:\n  - mined-a\n".to_vec(),
            pack_id: "pack-mined-a".to_owned(),
            stats: vec![MinedMacroStats {
                name: "mined-a".to_owned(),
                support: 3,
                paths: 2,
                lift: finite(1.25),
                score: score(7.0),
                len_tokens: 9,
            }],
            paths_used: 2,
            tokens_scanned: 144,
        };

        assert_eq!(request.paths[0].expansions[0].score_delta, finite(2.25));
        assert_eq!(request.paths[0].terminal_score, score(14.0));
        assert_eq!(request.params.max_len_tokens, Some(24));
        assert_eq!(response.macro_pack_yaml, b"macros:\n  - mined-a\n");
        assert_eq!(response.pack_id, "pack-mined-a");
        assert_eq!(response.stats[0].name, "mined-a");
        assert_eq!(response.paths_used, 2);
        assert_eq!(response.tokens_scanned, 144);
    }

    #[test]
    fn input_synth_provenanced_burst_serializes_deterministically() {
        let burst = sample_provenanced_burst(2, FP_B);
        let encoded = postcard::to_allocvec(&burst).expect("serialize provenanced burst");
        let decoded: ProvenancedBurst =
            postcard::from_bytes(&encoded).expect("deserialize provenanced burst");
        let encoded_again = postcard::to_allocvec(&decoded).expect("reserialize");

        assert_eq!(decoded, burst);
        assert_eq!(encoded_again, encoded);
    }

    fn sample_context() -> NodeContext {
        NodeContext {
            node_id: NodeId::new(7),
            parent_node_id: Some(NodeId::ROOT),
            snapshot_ref: SNAPSHOT,
            state_hash: STATE,
            cell_key: CellKey::new(42),
            stage: Stage::new(3),
            depth: 4,
            frame_counter: FrameCount::new(900),
            node_score: score(12.0),
            novelty: novelty(0.5),
            ram_features: BTreeMap::from([
                ("boss_hp".to_owned(), finite(80.0)),
                ("player_x".to_owned(), finite(12.0)),
            ]),
            frame_embedding: vec![finite(0.125), finite(0.25)],
            recent_inputs: Some(sample_burst()),
            parent_burst: Some(sample_provenanced_burst(0, FP_A)),
            sibling_bursts: vec![ScoredBurst {
                burst: sample_provenanced_burst(1, FP_A),
                score_delta: finite(1.5),
            }],
        }
    }

    fn sample_provenanced_burst(slot: u32, fingerprint: ConfigFingerprint) -> ProvenancedBurst {
        ProvenancedBurst {
            burst: sample_burst(),
            provenance: Provenance {
                generator: GeneratorKind::Macro,
                slot,
                rng_stream: format!("slot/{slot}/macro"),
                config_fingerprint: fingerprint,
                fallback_from: Some(GeneratorKind::Policy),
                macro_provenance: Some(MacroProvenance {
                    pack_id: "pack-a5".to_owned(),
                    macro_name: "dash-right".to_owned(),
                    param_bindings: BTreeMap::from([("direction".to_owned(), "right".to_owned())]),
                    macro_frames: FrameCount::new(12),
                    tail_frames: FrameCount::new(3),
                    chain_index: 0,
                }),
                mutation_provenance: None,
                policy_provenance: None,
            },
        }
    }

    fn sample_burst() -> Burst {
        Burst {
            format_version: 1,
            pad_segments: vec![PadSegment {
                start_frame: FrameCount::new(0),
                frames: FrameCount::new(12),
                buttons: 0b0011,
            }],
        }
    }

    fn finite(value: f64) -> FiniteF64 {
        FiniteF64::new(value).expect("test value is finite")
    }

    fn score(value: f64) -> Score {
        Score::new(value).expect("test score is finite")
    }

    fn novelty(value: f64) -> Novelty {
        Novelty::new(value).expect("test novelty is finite")
    }
}
