//! Snapshot-store client boundary.
//!
//! Owner docs: `/home/infra-admin/.agents/projects/determinism/docs/snapshot-store/API.md`
//! sections 1.1, 1.4, 1.5, and 1.6, plus
//! `/home/infra-admin/.agents/projects/determinism/docs/exploration-orchestrator/API.md`
//! section 3.
//!
//! This module mirrors experiment-scoped tree node operations, subtree pruning,
//! cursorable queries, and metadata CAS shapes without choosing a transport
//! implementation.

use orch_core::types::{NodeId, NodeStatus, Novelty, Score, SnapshotRef};
use serde::{Deserialize, Serialize};

use crate::ClientResult;

pub trait SnapshotStoreClient {
    fn create_node(&mut self, request: CreateNodeRequest) -> ClientResult<CreateNodeResponse>;

    fn update_nodes(&mut self, request: UpdateNodesRequest) -> ClientResult<UpdateNodesResponse>;

    fn get_node(&self, request: GetNodeRequest) -> ClientResult<GetNodeResponse>;

    fn get_children(&self, request: GetChildrenRequest) -> ClientResult<GetChildrenResponse>;

    fn get_path(&self, request: GetPathRequest) -> ClientResult<GetPathResponse>;

    fn query_nodes(&self, request: QueryNodesRequest) -> ClientResult<QueryNodesResponse>;

    fn put_metadata(&mut self, request: PutMetadataRequest) -> ClientResult<PutMetadataResponse>;

    fn get_metadata(&self, request: GetMetadataRequest) -> ClientResult<GetMetadataResponse>;

    fn delete_metadata(
        &mut self,
        request: DeleteMetadataRequest,
    ) -> ClientResult<DeleteMetadataResponse>;

    fn prune_subtree(&mut self, request: PruneSubtreeRequest)
        -> ClientResult<PruneSubtreeResponse>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeMeta {
    pub experiment_id: String,
    pub node_id: NodeId,
    pub parent_node_id: Option<NodeId>,
    pub depth: u64,
    pub snapshot_ref: SnapshotRef,
    pub input_log_id: Option<InputLogId>,
    pub status: NodeStatus,
    pub progress_score: Score,
    pub novelty_score: Novelty,
    pub visit_count: u64,
    pub expand_count: u64,
    pub last_visited_at: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub attrs: NodeAttrs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateNodeRequest {
    pub experiment_id: String,
    pub node_id: NodeId,
    pub parent_node_id: Option<NodeId>,
    pub snapshot_ref: SnapshotRef,
    pub input_log_id: Option<InputLogId>,
    pub status: NodeStatus,
    pub progress_score: Score,
    pub novelty_score: Novelty,
    pub attrs: NodeAttrs,
    pub input_log_container: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateNodeResponse {
    pub node: NodeMeta,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateNodesRequest {
    pub experiment_id: String,
    pub updates: Vec<NodeUpdate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeUpdate {
    pub node_id: NodeId,
    pub status: Option<NodeStatus>,
    pub progress_score: Option<Score>,
    pub novelty_score: Option<Novelty>,
    pub visit_count_delta: u64,
    pub expand_count_delta: u64,
    pub touch_visited: bool,
    pub attrs: Option<NodeAttrs>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateNodesResponse {
    pub updated_at: u64,
    pub applied: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetNodeRequest {
    pub experiment_id: String,
    pub node_id: NodeId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GetNodeResponse {
    pub node: NodeMeta,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetChildrenRequest {
    pub experiment_id: String,
    pub node_id: NodeId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GetChildrenResponse {
    pub children: Vec<NodeMeta>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetPathRequest {
    pub experiment_id: String,
    pub node_id: NodeId,
    pub include_input_logs: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GetPathResponse {
    pub nodes: Vec<NodeMeta>,
    pub input_log_containers: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryNodesRequest {
    pub experiment_id: String,
    pub statuses: Vec<NodeStatus>,
    pub min_progress: Option<Score>,
    pub max_progress: Option<Score>,
    pub min_novelty: Option<Novelty>,
    pub min_depth: Option<u64>,
    pub max_depth: Option<u64>,
    pub created_after: Option<u64>,
    pub updated_after: Option<u64>,
    pub order_by: OrderBy,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryNodesResponse {
    pub nodes: Vec<NodeMeta>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutMetadataRequest {
    pub key: MetadataKey,
    pub value: Vec<u8>,
    pub expected_generation: Option<MetadataGeneration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutMetadataResponse {
    pub generation: MetadataGeneration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetMetadataRequest {
    pub key: MetadataKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetMetadataResponse {
    pub value: Vec<u8>,
    pub generation: MetadataGeneration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteMetadataRequest {
    pub key: MetadataKey,
    pub expected_generation: Option<MetadataGeneration>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteMetadataResponse;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneSubtreeRequest {
    pub experiment_id: String,
    pub node_id: NodeId,
    pub allow_root: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneSubtreeResponse {
    pub nodes_pruned: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAttrs(pub Vec<u8>);

impl NodeAttrs {
    pub const MAX_BYTES: usize = 16 * 1024 * 1024;

    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InputLogId(pub [u8; INPUT_LOG_ID_LEN]);

pub const INPUT_LOG_ID_LEN: usize = 32;

impl InputLogId {
    #[must_use]
    pub const fn new(bytes: [u8; INPUT_LOG_ID_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; INPUT_LOG_ID_LEN] {
        &self.0
    }

    #[must_use]
    pub const fn into_bytes(self) -> [u8; INPUT_LOG_ID_LEN] {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetadataKey(String);

impl MetadataKey {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn checkpoint(experiment_id: &str) -> Self {
        Self(format!("orch/ckpt/{experiment_id}"))
    }

    #[must_use]
    pub fn wal(experiment_id: &str, seq: u64) -> Self {
        Self(format!("orch/wal/{experiment_id}/{seq:020}"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MetadataGeneration(pub u64);

impl MetadataGeneration {
    pub const CREATE_ONLY: Self = Self(0);

    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderBy {
    CreatedAt,
    ProgressDesc,
    NoveltyDesc,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT_A: SnapshotRef = SnapshotRef::new([0xA5; 32]);
    const SNAPSHOT_B: SnapshotRef = SnapshotRef::new([0x5A; 32]);
    const LOG_A: InputLogId = InputLogId::new([0x11; 32]);

    #[test]
    fn snapshot_store_node_meta_carries_status_and_attrs_boundaries() {
        let attrs = NodeAttrs::new(vec![1, 2, 3, 4]);
        let node = sample_node(
            NodeId::new(7),
            Some(NodeId::ROOT),
            NodeStatus::Frontier,
            attrs,
        );

        assert_eq!(node.experiment_id, "exp-a");
        assert_eq!(node.node_id, NodeId::new(7));
        assert_eq!(node.parent_node_id, Some(NodeId::ROOT));
        assert_eq!(node.snapshot_ref, SNAPSHOT_A);
        assert_eq!(node.input_log_id, Some(LOG_A));
        assert_eq!(node.status, NodeStatus::Frontier);
        assert_eq!(node.progress_score, score(10.0));
        assert_eq!(node.novelty_score, novelty(0.5));
        assert_eq!(node.attrs.as_bytes(), [1, 2, 3, 4]);
        assert_eq!(NodeAttrs::MAX_BYTES, 16 * 1024 * 1024);
    }

    #[test]
    fn snapshot_store_create_node_shape_supports_blind_retry_idempotency() {
        let request = CreateNodeRequest {
            experiment_id: "exp-a".to_owned(),
            node_id: NodeId::new(8),
            parent_node_id: Some(NodeId::new(7)),
            snapshot_ref: SNAPSHOT_B,
            input_log_id: Some(LOG_A),
            status: NodeStatus::Frontier,
            progress_score: score(11.0),
            novelty_score: novelty(0.25),
            attrs: NodeAttrs::new(vec![9, 9]),
            input_log_container: None,
        };
        let response = CreateNodeResponse {
            node: sample_node(
                request.node_id,
                request.parent_node_id,
                NodeStatus::Frontier,
                request.attrs.clone(),
            ),
        };

        assert_eq!(request.node_id, response.node.node_id);
        assert_eq!(request.parent_node_id, response.node.parent_node_id);
        assert_eq!(request.snapshot_ref, SNAPSHOT_B);
        assert_eq!(request.input_log_id, Some(LOG_A));
        assert_eq!(request.attrs, NodeAttrs::new(vec![9, 9]));
    }

    #[test]
    fn snapshot_store_update_nodes_cover_status_visit_and_attrs_replacement() {
        let request = UpdateNodesRequest {
            experiment_id: "exp-a".to_owned(),
            updates: vec![NodeUpdate {
                node_id: NodeId::new(7),
                status: Some(NodeStatus::Expanded),
                progress_score: Some(score(12.0)),
                novelty_score: Some(novelty(0.125)),
                visit_count_delta: 1,
                expand_count_delta: 1,
                touch_visited: true,
                attrs: Some(NodeAttrs::new(vec![4, 3, 2, 1])),
            }],
        };
        let response = UpdateNodesResponse {
            updated_at: 99,
            applied: 1,
        };

        assert_eq!(request.updates[0].status, Some(NodeStatus::Expanded));
        assert_eq!(request.updates[0].visit_count_delta, 1);
        assert!(request.updates[0].touch_visited);
        assert_eq!(
            request.updates[0].attrs.as_ref().unwrap().as_bytes(),
            [4, 3, 2, 1]
        );
        assert_eq!(response.updated_at, 99);
        assert_eq!(response.applied, 1);
    }

    #[test]
    fn snapshot_store_query_filters_and_cursors_are_explicit() {
        let request = QueryNodesRequest {
            experiment_id: "exp-a".to_owned(),
            statuses: vec![NodeStatus::Frontier, NodeStatus::Goal],
            min_progress: Some(score(4.0)),
            max_progress: Some(score(20.0)),
            min_novelty: Some(novelty(0.1)),
            min_depth: Some(2),
            max_depth: Some(10),
            created_after: Some(100),
            updated_after: Some(120),
            order_by: OrderBy::ProgressDesc,
            limit: Some(512),
        };
        let response = QueryNodesResponse {
            nodes: vec![sample_node(
                NodeId::new(7),
                Some(NodeId::ROOT),
                NodeStatus::Goal,
                NodeAttrs::new(vec![1]),
            )],
        };

        assert_eq!(request.statuses, [NodeStatus::Frontier, NodeStatus::Goal]);
        assert_eq!(request.created_after, Some(100));
        assert_eq!(request.updated_after, Some(120));
        assert_eq!(request.order_by, OrderBy::ProgressDesc);
        assert_eq!(request.limit, Some(512));
        assert_eq!(response.nodes[0].status, NodeStatus::Goal);
    }

    #[test]
    fn snapshot_store_path_children_and_prune_shapes_match_tree_reads() {
        let root = sample_node(
            NodeId::ROOT,
            None,
            NodeStatus::Expanded,
            NodeAttrs::new(vec![]),
        );
        let child = sample_node(
            NodeId::new(7),
            Some(NodeId::ROOT),
            NodeStatus::Pruned,
            NodeAttrs::new(vec![7]),
        );
        let path = GetPathResponse {
            nodes: vec![root.clone(), child.clone()],
            input_log_containers: vec![b"dhi-log".to_vec()],
        };
        let children = GetChildrenResponse {
            children: vec![child],
        };
        let prune = PruneSubtreeRequest {
            experiment_id: "exp-a".to_owned(),
            node_id: NodeId::new(7),
            allow_root: false,
        };
        let pruned = PruneSubtreeResponse { nodes_pruned: 3 };

        assert_eq!(path.nodes[0].node_id, NodeId::ROOT);
        assert_eq!(path.input_log_containers, [b"dhi-log".to_vec()]);
        assert_eq!(children.children[0].parent_node_id, Some(NodeId::ROOT));
        assert!(!prune.allow_root);
        assert_eq!(pruned.nodes_pruned, 3);
    }

    #[test]
    fn snapshot_store_metadata_keys_and_generations_cover_checkpoint_and_wal() {
        let ckpt_key = MetadataKey::checkpoint("exp-a");
        let wal_key = MetadataKey::wal("exp-a", 42);
        let put = PutMetadataRequest {
            key: ckpt_key.clone(),
            value: b"checkpoint-v1".to_vec(),
            expected_generation: Some(MetadataGeneration::CREATE_ONLY),
        };
        let put_response = PutMetadataResponse {
            generation: MetadataGeneration::new(1),
        };
        let get_response = GetMetadataResponse {
            value: put.value.clone(),
            generation: put_response.generation,
        };
        let delete = DeleteMetadataRequest {
            key: wal_key.clone(),
            expected_generation: Some(MetadataGeneration::new(7)),
        };

        assert_eq!(ckpt_key.as_str(), "orch/ckpt/exp-a");
        assert_eq!(wal_key.as_str(), "orch/wal/exp-a/00000000000000000042");
        assert_eq!(
            put.expected_generation,
            Some(MetadataGeneration::CREATE_ONLY)
        );
        assert_eq!(put_response.generation.get(), 1);
        assert_eq!(get_response.value, b"checkpoint-v1");
        assert_eq!(delete.expected_generation.unwrap().get(), 7);
    }

    #[test]
    fn snapshot_store_dtos_round_trip_with_postcard() {
        let response = QueryNodesResponse {
            nodes: vec![sample_node(
                NodeId::new(7),
                Some(NodeId::ROOT),
                NodeStatus::Frontier,
                NodeAttrs::new(vec![1, 2, 3]),
            )],
        };
        let encoded = postcard::to_allocvec(&response).expect("serialize query response");
        let decoded: QueryNodesResponse =
            postcard::from_bytes(&encoded).expect("deserialize query response");

        assert_eq!(decoded, response);
    }

    fn sample_node(
        node_id: NodeId,
        parent_node_id: Option<NodeId>,
        status: NodeStatus,
        attrs: NodeAttrs,
    ) -> NodeMeta {
        NodeMeta {
            experiment_id: "exp-a".to_owned(),
            node_id,
            parent_node_id,
            depth: if node_id.is_root() { 0 } else { 1 },
            snapshot_ref: if node_id == NodeId::new(8) {
                SNAPSHOT_B
            } else {
                SNAPSHOT_A
            },
            input_log_id: if node_id.is_root() { None } else { Some(LOG_A) },
            status,
            progress_score: score(10.0),
            novelty_score: novelty(0.5),
            visit_count: 2,
            expand_count: 1,
            last_visited_at: 90,
            created_at: 80,
            updated_at: 95,
            attrs,
        }
    }

    fn score(value: f64) -> Score {
        Score::new(value).expect("test score is finite")
    }

    fn novelty(value: f64) -> Novelty {
        Novelty::new(value).expect("test novelty is finite")
    }
}
