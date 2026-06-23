//! Orchestrator-private node attrs and input-synth context reconstruction.
//!
//! The attrs envelope stores a `recent_inputs` tail when commit code supplies one.
//! These helpers deliberately do not synthesize a tail from root-to-node history.

use std::collections::BTreeMap;

use orch_clients::{
    hypervisor::{DeterminismClass, Digest32, FbInfo},
    input_synth::{Burst, ConfigFingerprint, NodeContext, ProvenancedBurst, ScoredBurst},
    snapshot_store::{
        GetChildrenRequest, GetNodeRequest, NodeAttrs, NodeMeta, SnapshotStoreClient,
    },
    ClientError, ClientErrorKind, ClientResult,
};
use orch_core::types::{
    CellKey, FiniteF64, FrameCount, NodeId, NodeStatus, Novelty, Score, Stage, StateHash,
};
use serde::{Deserialize, Serialize};

pub const ORCH_NODE_ATTRS_MAGIC: [u8; 8] = *b"ORCHNA1\0";
pub const ORCH_NODE_ATTRS_VERSION: u16 = 1;
pub const DEFAULT_MAX_SIBLING_BURSTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeContextLimits {
    pub max_sibling_bursts: usize,
}

impl Default for NodeContextLimits {
    fn default() -> Self {
        Self {
            max_sibling_bursts: DEFAULT_MAX_SIBLING_BURSTS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrchNodeAttrsV1 {
    pub magic: [u8; 8],
    pub version: u16,
    pub machine_config_hash: Digest32,
    pub determinism_class: DeterminismClass,
    pub goal: bool,
    pub prune_reason: Option<String>,
    pub root: Option<RootNodeAttrs>,
    pub synth: SynthContextAttrs,
}

impl OrchNodeAttrsV1 {
    #[must_use]
    pub fn new(
        machine_config_hash: Digest32,
        determinism_class: DeterminismClass,
        synth: SynthContextAttrs,
    ) -> Self {
        Self {
            magic: ORCH_NODE_ATTRS_MAGIC,
            version: ORCH_NODE_ATTRS_VERSION,
            machine_config_hash,
            determinism_class,
            goal: false,
            prune_reason: None,
            root: None,
            synth,
        }
    }

    #[must_use]
    pub fn with_root(mut self, root: RootNodeAttrs) -> Self {
        self.root = Some(root);
        self
    }

    #[must_use]
    pub fn with_goal(mut self, goal: bool) -> Self {
        self.goal = goal;
        self
    }

    #[must_use]
    pub fn with_prune_reason(mut self, prune_reason: impl Into<String>) -> Self {
        self.prune_reason = Some(prune_reason.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootNodeAttrs {
    pub framebuffer: Option<FbInfo>,
    pub fps: Option<FpsRational>,
    pub pad_layout: Option<PadLayoutAttrs>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FpsRational {
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PadLayoutAttrs {
    pub alphabet: String,
    pub button_bits: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SynthContextAttrs {
    pub created_by_burst: Option<ProvenancedBurst>,
    pub config_fingerprint: Option<ConfigFingerprint>,
    pub decoded_features: BTreeMap<String, FiniteF64>,
    pub frame_counter: FrameCount,
    pub state_hash: StateHash,
    pub cell_key: CellKey,
    pub stage: Stage,
    pub score: Score,
    pub novelty: Novelty,
    pub recent_inputs: Option<Burst>,
}

pub fn encode_node_attrs(attrs: &OrchNodeAttrsV1) -> ClientResult<NodeAttrs> {
    validate_envelope(attrs, ClientErrorKind::InvalidRequest)?;
    let bytes = postcard::to_allocvec(attrs).map_err(|error| {
        ClientError::new(
            ClientErrorKind::Internal,
            format!("failed to encode orchestrator node attrs: {error}"),
        )
    })?;
    NodeAttrs::new(bytes)
}

pub fn decode_node_attrs(attrs: &NodeAttrs) -> ClientResult<OrchNodeAttrsV1> {
    let decoded = postcard::from_bytes::<OrchNodeAttrsV1>(attrs.as_bytes()).map_err(|error| {
        ClientError::new(
            ClientErrorKind::DataLoss,
            format!("failed to decode orchestrator node attrs: {error}"),
        )
    })?;
    validate_envelope(&decoded, ClientErrorKind::DataLoss)?;
    Ok(decoded)
}

pub fn build_input_synth_node_context<S: SnapshotStoreClient>(
    store: &S,
    experiment_id: &str,
    node_id: NodeId,
    limits: NodeContextLimits,
) -> ClientResult<NodeContext> {
    let selected = store
        .get_node(GetNodeRequest {
            experiment_id: experiment_id.to_owned(),
            node_id,
        })?
        .node;
    let selected_attrs = decode_node_attrs(&selected.attrs)?;
    let depth = u32::try_from(selected.depth).map_err(|_| {
        ClientError::new(
            ClientErrorKind::DataLoss,
            format!(
                "node {} depth {} exceeds u32",
                node_id.get(),
                selected.depth
            ),
        )
    })?;

    let sibling_bursts = if let Some(parent_id) = selected.parent_node_id {
        let parent = store
            .get_node(GetNodeRequest {
                experiment_id: experiment_id.to_owned(),
                node_id: parent_id,
            })?
            .node;
        if parent.node_id != parent_id || parent.experiment_id != selected.experiment_id {
            return Err(ClientError::new(
                ClientErrorKind::DataLoss,
                "loaded parent metadata does not match selected node parent",
            ));
        }
        build_sibling_bursts(store, experiment_id, &selected, &parent, limits)?
    } else {
        Vec::new()
    };

    Ok(NodeContext {
        node_id: selected.node_id,
        parent_node_id: selected.parent_node_id,
        snapshot_ref: selected.snapshot_ref,
        state_hash: selected_attrs.synth.state_hash,
        cell_key: selected_attrs.synth.cell_key,
        stage: selected_attrs.synth.stage,
        depth,
        frame_counter: selected_attrs.synth.frame_counter,
        node_score: selected.progress_score,
        novelty: selected.novelty_score,
        ram_features: selected_attrs.synth.decoded_features,
        frame_embedding: Vec::new(),
        recent_inputs: selected_attrs.synth.recent_inputs,
        parent_burst: selected_attrs.synth.created_by_burst,
        sibling_bursts,
    })
}

fn build_sibling_bursts<S: SnapshotStoreClient>(
    store: &S,
    experiment_id: &str,
    selected: &NodeMeta,
    parent: &NodeMeta,
    limits: NodeContextLimits,
) -> ClientResult<Vec<ScoredBurst>> {
    let mut children = store
        .get_children(GetChildrenRequest {
            experiment_id: experiment_id.to_owned(),
            node_id: parent.node_id,
        })?
        .children;
    children.sort_by_key(|child| child.node_id);

    let mut out = Vec::new();
    for child in children {
        if child.node_id == selected.node_id || !is_context_sibling_status(child.status) {
            continue;
        }
        let attrs = decode_node_attrs(&child.attrs)?;
        let Some(burst) = attrs.synth.created_by_burst else {
            continue;
        };
        let score_delta = FiniteF64::new(child.progress_score.get() - parent.progress_score.get())
            .map_err(|error| {
                ClientError::new(
                    ClientErrorKind::DataLoss,
                    format!("non-finite sibling score delta: {error}"),
                )
            })?;
        out.push(ScoredBurst { burst, score_delta });
        if out.len() >= limits.max_sibling_bursts {
            break;
        }
    }

    Ok(out)
}

fn is_context_sibling_status(status: NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Frontier | NodeStatus::Expanded | NodeStatus::Goal
    )
}

fn validate_envelope(attrs: &OrchNodeAttrsV1, kind: ClientErrorKind) -> ClientResult<()> {
    if attrs.magic != ORCH_NODE_ATTRS_MAGIC {
        return Err(ClientError::new(
            kind,
            "unknown orchestrator node attrs magic",
        ));
    }
    if attrs.version != ORCH_NODE_ATTRS_VERSION {
        return Err(ClientError::new(
            kind,
            format!("unknown orchestrator node attrs version {}", attrs.version),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_clients::input_synth::{
        BurstBody, BurstId, GeneratorKind, PadBurst, PadSegment, Provenance,
    };

    #[test]
    fn attrs_round_trip_root_and_child_records() {
        let root = sample_attrs(NodeId::ROOT, None).with_root(RootNodeAttrs {
            framebuffer: Some(FbInfo {
                width: 320,
                height: 240,
                stride: 1280,
                format: orch_clients::hypervisor::PixelFormat::Xrgb8888,
                frame_counter: FrameCount::new(0),
            }),
            fps: Some(FpsRational {
                numerator: 60,
                denominator: 1,
            }),
            pad_layout: Some(PadLayoutAttrs {
                alphabet: "console16-12btn-v1".to_owned(),
                button_bits: BTreeMap::from([("A".to_owned(), 0), ("RIGHT".to_owned(), 9)]),
            }),
        });
        let child_burst = sample_burst(1);
        let child = sample_attrs(NodeId::new(1), Some(child_burst.clone()));

        let decoded_root = decode_node_attrs(&encode_node_attrs(&root).expect("encode root"))
            .expect("decode root");
        let decoded_child = decode_node_attrs(&encode_node_attrs(&child).expect("encode child"))
            .expect("decode child");

        assert_eq!(decoded_root, root);
        assert_eq!(
            decoded_child.synth.created_by_burst.as_ref(),
            Some(&child_burst)
        );
        assert_eq!(decoded_child.synth.recent_inputs, None);
    }

    #[test]
    fn attrs_decode_rejects_unknown_version_and_malformed_bytes() {
        let mut attrs = sample_attrs(NodeId::ROOT, None);
        attrs.version = 99;
        let encoded = postcard::to_allocvec(&attrs).expect("manual encode");

        let version_error = decode_node_attrs(&NodeAttrs::from_trusted_bytes(encoded))
            .expect_err("unknown version");
        let malformed_error = decode_node_attrs(&NodeAttrs::from_trusted_bytes(vec![1, 2, 3]))
            .expect_err("malformed");

        assert_eq!(version_error.kind(), ClientErrorKind::DataLoss);
        assert_eq!(malformed_error.kind(), ClientErrorKind::DataLoss);
    }

    fn sample_attrs(
        node_id: NodeId,
        created_by_burst: Option<ProvenancedBurst>,
    ) -> OrchNodeAttrsV1 {
        OrchNodeAttrsV1::new(
            Digest32::new([0x33; 32]),
            DeterminismClass {
                cpu_model: "test-cpu".to_owned(),
                microcode: "test-ucode".to_owned(),
                host_kernel: "test-kernel".to_owned(),
                vmm_version: "test-vmm".to_owned(),
            },
            SynthContextAttrs {
                created_by_burst,
                config_fingerprint: Some(ConfigFingerprint::new([0x44; 32])),
                decoded_features: BTreeMap::from([(
                    "feat/player_x".to_owned(),
                    FiniteF64::new(node_id.get() as f64).expect("finite"),
                )]),
                frame_counter: FrameCount::new(10 + node_id.get() as u32),
                state_hash: StateHash::new([node_id.get() as u8; 32]),
                cell_key: CellKey::new(99 + node_id.get()),
                stage: Stage::new(1),
                score: Score::new(node_id.get() as f64).expect("finite"),
                novelty: Novelty::new(0.5).expect("finite"),
                recent_inputs: None,
            },
        )
    }

    fn sample_burst(slot: u32) -> ProvenancedBurst {
        let fingerprint = ConfigFingerprint::new([0x44; 32]);
        ProvenancedBurst {
            burst: Burst {
                format_version: 1,
                burst_id: BurstId::new([slot as u8; 32]),
                body: BurstBody::Pad(PadBurst {
                    segments: vec![PadSegment {
                        buttons: slot,
                        hold_frames: FrameCount::new(3),
                    }],
                    button_alphabet: "console16-12btn-v1".to_owned(),
                }),
            },
            provenance: Provenance {
                generator: GeneratorKind::Mutation,
                slot,
                rng_stream: format!("slot/{slot}"),
                config_fingerprint: fingerprint,
                fallback_from: None,
                macro_provenance: None,
                mutation_provenance: None,
                policy_provenance: None,
            },
        }
    }
}
