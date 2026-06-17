//! Input synthesizer client boundary.
//!
//! Owner docs: service-local API doc is pending; current traceable anchors are
//! `../determinism-hypervisor/.agents/docs/phases/phase-4-scoring-and-inputs.md`
//! for input-synthesizer scope and
//! `../control-plane/proto/determinism/inputsynth/v1/synthesizer.proto` for the
//! skeletal v1 proto surface.
//!
//! This module mirrors macro-pack loading, health, burst proposal, macro mining,
//! provenance, and degraded-mode shapes from the owner API without choosing a
//! transport implementation.

use orch_core::types::{CellKey, FrameCount, NodeId, SnapshotRef, Stage, StateHash};

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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoadMacroPackRequest {
    pub experiment_id: String,
    pub pack_ref: String,
    pub kind: MacroPackKind,
    pub document_kind: DocumentKind,
    pub document: String,
    pub expected_fingerprint: Option<ConfigFingerprint>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoadMacroPackResponse {
    pub loaded_pack: LoadedMacroPack,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HealthRequest {
    pub experiment_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HealthResponse {
    pub synth_version: String,
    pub loaded_packs: Vec<LoadedMacroPack>,
    pub degraded: bool,
    pub degraded_reasons: Vec<DegradedReason>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProposeBurstsRequest {
    pub experiment_id: String,
    pub node_context: NodeContext,
    pub k: u32,
    pub length_hint: FrameCount,
    pub seed: u64,
    pub model: ModelKind,
    pub config_overrides_yaml: Option<String>,
    pub macro_overrides: MacroOverrides,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProposeBurstsResponse {
    pub bursts: Vec<ProvenancedBurst>,
    pub config_fingerprint: ConfigFingerprint,
    pub synth_version: String,
    pub degraded: bool,
    pub degraded_reasons: Vec<DegradedReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MineMacrosRequest {
    pub experiment_id: String,
    pub path_samples: Vec<PathSample>,
    pub params: MiningParams,
    pub seed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MineMacrosResponse {
    pub loaded_pack: LoadedMacroPack,
    pub stats: MinedMacroStats,
    pub config_fingerprint: ConfigFingerprint,
    pub degraded: bool,
    pub degraded_reasons: Vec<DegradedReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NodeContext {
    pub node_id: NodeId,
    pub parent_node_id: Option<NodeId>,
    pub snapshot_ref: SnapshotRef,
    pub state_hash: StateHash,
    pub cell_key: CellKey,
    pub stage: Stage,
    pub depth: u32,
    pub frame_counter: FrameCount,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MacroOverrides {
    pub force_macro_refs: Vec<String>,
    pub banned_macro_refs: Vec<String>,
    pub macro_weight_bps: Option<u16>,
}

impl MacroOverrides {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            force_macro_refs: Vec::new(),
            banned_macro_refs: Vec::new(),
            macro_weight_bps: None,
        }
    }
}

impl Default for MacroOverrides {
    fn default() -> Self {
        Self::none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoadedMacroPack {
    pub pack_ref: String,
    pub kind: MacroPackKind,
    pub document_kind: DocumentKind,
    pub macro_count: u32,
    pub config_fingerprint: ConfigFingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProvenancedBurst {
    pub burst: Burst,
    pub provenance: BurstProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Burst {
    pub format_version: u32,
    pub pad: PadBurst,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PadBurst {
    pub segments: Vec<PadSegment>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PadSegment {
    pub start_frame: FrameCount,
    pub frames: FrameCount,
    pub buttons: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BurstProvenance {
    Generator {
        model: ModelKind,
        seed: u64,
        draw_start: u64,
        draw_count: u64,
    },
    Macro {
        pack_ref: String,
        macro_name: String,
        seed: u64,
    },
    Mutation {
        parent_burst_ref: String,
        operator: MutationOperator,
        seed: u64,
    },
    Policy {
        policy_ref: String,
        seed: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PathSample {
    pub node_ids: Vec<NodeId>,
    pub bursts: Vec<ProvenancedBurst>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MiningParams {
    pub min_support: u32,
    pub max_macros: u32,
    pub max_len_frames: FrameCount,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MinedMacroStats {
    pub sampled_paths: u32,
    pub candidate_macros: u32,
    pub accepted_macros: u32,
}

pub const CONFIG_FINGERPRINT_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MacroPackKind {
    BuiltIn,
    Experiment,
    Learned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DocumentKind {
    Yaml,
    Json,
    Binary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelKind {
    WeightedRandom,
    MacroWeighted,
    Mutation,
    PolicyGuided,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MutationOperator {
    Splice,
    Insert,
    Delete,
    Retiming,
    ButtonFlip,
    HoldStretch,
    HoldShrink,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DegradedReason {
    NoMacroPacksLoaded,
    MacroPackUnavailable {
        pack_ref: String,
    },
    ModelUnavailable {
        model: ModelKind,
    },
    FingerprintMismatch {
        expected: ConfigFingerprint,
        actual: ConfigFingerprint,
    },
    OverridesIgnored {
        reason: String,
    },
    InternalFallback {
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP_A: ConfigFingerprint = ConfigFingerprint::new([0xA5; CONFIG_FINGERPRINT_LEN]);
    const FP_B: ConfigFingerprint = ConfigFingerprint::new([0x5A; CONFIG_FINGERPRINT_LEN]);
    const SNAPSHOT: SnapshotRef = SnapshotRef::new([1; 32]);
    const STATE: StateHash = StateHash::new([2; 32]);

    #[test]
    fn input_synth_node_context_carries_core_fields() {
        let context = sample_context();

        assert_eq!(context.node_id, NodeId::new(7));
        assert_eq!(context.parent_node_id, Some(NodeId::ROOT));
        assert_eq!(context.snapshot_ref, SNAPSHOT);
        assert_eq!(context.state_hash, STATE);
        assert_eq!(context.cell_key, CellKey::new(42));
        assert_eq!(context.stage, Stage::new(3));
        assert_eq!(context.depth, 4);
        assert_eq!(context.frame_counter, FrameCount::new(900));
    }

    #[test]
    fn input_synth_propose_request_carries_seed_model_and_overrides() {
        let request = ProposeBurstsRequest {
            experiment_id: "exp-a".to_owned(),
            node_context: sample_context(),
            k: 16,
            length_hint: FrameCount::new(120),
            seed: 99,
            model: ModelKind::MacroWeighted,
            config_overrides_yaml: Some("macro_weight_hot: 0.75\n".to_owned()),
            macro_overrides: MacroOverrides {
                force_macro_refs: vec!["pack://movement/dash".to_owned()],
                banned_macro_refs: vec!["pack://debug/noop".to_owned()],
                macro_weight_bps: Some(7_500),
            },
        };

        assert_eq!(request.experiment_id, "exp-a");
        assert_eq!(request.k, 16);
        assert_eq!(request.length_hint, FrameCount::new(120));
        assert_eq!(request.seed, 99);
        assert_eq!(request.model, ModelKind::MacroWeighted);
        assert_eq!(
            request.config_overrides_yaml.as_deref(),
            Some("macro_weight_hot: 0.75\n")
        );
        assert_eq!(
            request.macro_overrides.force_macro_refs,
            ["pack://movement/dash"]
        );
        assert_eq!(request.macro_overrides.macro_weight_bps, Some(7_500));
    }

    #[test]
    fn input_synth_health_reports_loaded_packs_and_degraded_mode() {
        let loaded_pack = loaded_pack("pack://movement", FP_A);
        let health = HealthResponse {
            synth_version: "fake-synth/0.1".to_owned(),
            loaded_packs: vec![loaded_pack.clone()],
            degraded: true,
            degraded_reasons: vec![
                DegradedReason::NoMacroPacksLoaded,
                DegradedReason::FingerprintMismatch {
                    expected: FP_A,
                    actual: FP_B,
                },
            ],
            warnings: vec!["macro mode disabled".to_owned()],
        };

        assert_eq!(health.loaded_packs, [loaded_pack]);
        assert!(health.degraded);
        assert!(matches!(
            &health.degraded_reasons[0],
            DegradedReason::NoMacroPacksLoaded
        ));
        assert!(matches!(
            &health.degraded_reasons[1],
            DegradedReason::FingerprintMismatch { expected, actual }
                if *expected == FP_A && *actual == FP_B
        ));
        assert_eq!(health.warnings, ["macro mode disabled"]);
    }

    #[test]
    fn input_synth_provenance_and_fingerprint_echo_are_explicit() {
        let burst = ProvenancedBurst {
            burst: Burst {
                format_version: 1,
                pad: PadBurst {
                    segments: vec![PadSegment {
                        start_frame: FrameCount::new(0),
                        frames: FrameCount::new(12),
                        buttons: 0b0011,
                    }],
                },
            },
            provenance: BurstProvenance::Macro {
                pack_ref: "pack://movement".to_owned(),
                macro_name: "dash-right".to_owned(),
                seed: 123,
            },
        };
        let response = ProposeBurstsResponse {
            bursts: vec![burst],
            config_fingerprint: FP_A,
            synth_version: "fake-synth/0.1".to_owned(),
            degraded: false,
            degraded_reasons: Vec::new(),
        };

        assert_eq!(response.config_fingerprint, FP_A);
        assert!(!response.degraded);
        assert_eq!(response.bursts[0].burst.pad.segments[0].buttons, 0b0011);
        assert!(matches!(
            &response.bursts[0].provenance,
            BurstProvenance::Macro {
                pack_ref,
                macro_name,
                seed: 123
            } if pack_ref == "pack://movement" && macro_name == "dash-right"
        ));
    }

    #[test]
    fn input_synth_mine_macro_shapes_cover_path_samples_and_stats() {
        let request = MineMacrosRequest {
            experiment_id: "exp-a".to_owned(),
            path_samples: vec![PathSample {
                node_ids: vec![NodeId::ROOT, NodeId::new(7)],
                bursts: Vec::new(),
            }],
            params: MiningParams {
                min_support: 2,
                max_macros: 8,
                max_len_frames: FrameCount::new(240),
            },
            seed: 55,
        };
        let response = MineMacrosResponse {
            loaded_pack: loaded_pack("pack://mined/exp-a", FP_A),
            stats: MinedMacroStats {
                sampled_paths: 1,
                candidate_macros: 3,
                accepted_macros: 2,
            },
            config_fingerprint: FP_A,
            degraded: false,
            degraded_reasons: Vec::new(),
        };

        assert_eq!(
            request.path_samples[0].node_ids,
            [NodeId::ROOT, NodeId::new(7)]
        );
        assert_eq!(request.params.max_len_frames, FrameCount::new(240));
        assert_eq!(request.seed, 55);
        assert_eq!(response.stats.accepted_macros, 2);
        assert_eq!(response.loaded_pack.config_fingerprint, FP_A);
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
        }
    }

    fn loaded_pack(pack_ref: &str, config_fingerprint: ConfigFingerprint) -> LoadedMacroPack {
        LoadedMacroPack {
            pack_ref: pack_ref.to_owned(),
            kind: MacroPackKind::Experiment,
            document_kind: DocumentKind::Yaml,
            macro_count: 5,
            config_fingerprint,
        }
    }
}
