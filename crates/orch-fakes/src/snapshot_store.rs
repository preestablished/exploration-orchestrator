//! In-memory snapshot-store surface for deterministic search-loop tests.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};

use orch_clients::snapshot_store::{
    CreateNodeRequest, CreateNodeResponse, DeleteMetadataRequest, DeleteMetadataResponse,
    GetChildrenRequest, GetChildrenResponse, GetMetadataRequest, GetMetadataResponse,
    GetNodeRequest, GetNodeResponse, GetPathRequest, GetPathResponse, InputLogId,
    MetadataExpectation, MetadataGeneration, MetadataKey, NodeAttrs, NodeMeta, NodeUpdate,
    PutMetadataRequest, PutMetadataResponse, QueryNodesRequest, QueryNodesResponse,
    SnapshotStoreClient, UpdateNodesRequest, UpdateNodesResponse,
};
use orch_clients::{ClientError, ClientErrorKind, ClientResult};
use orch_core::types::{NodeId, NodeStatus};
use serde::Serialize;

use crate::fault::{FaultDecision, FaultInjector, FaultPlan, FaultRequest, FaultTarget};

#[derive(Clone, Debug)]
pub struct InMemorySnapshotStore {
    experiments: BTreeMap<String, ExperimentStore>,
    metadata: HashMap<MetadataKey, MetadataEntry>,
    fault_injector: FaultInjector,
    last_fault: Cell<Option<FaultDecision>>,
    logical_clock: u64,
}

impl Default for InMemorySnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemorySnapshotStore {
    #[must_use]
    pub fn new() -> Self {
        Self::with_fault_plan(FaultPlan::disabled(0))
    }

    #[must_use]
    pub fn with_fault_plan(fault_plan: FaultPlan) -> Self {
        Self {
            experiments: BTreeMap::new(),
            metadata: HashMap::new(),
            fault_injector: FaultInjector::new(fault_plan),
            last_fault: Cell::new(None),
            logical_clock: 0,
        }
    }

    #[must_use]
    pub const fn last_fault(&self) -> Option<FaultDecision> {
        self.last_fault.get()
    }

    #[must_use]
    pub fn preview_fault(
        &self,
        operation: &'static str,
        request_identity: &[u8],
        response_items: u32,
    ) -> FaultDecision {
        self.fault_injector.decide(
            FaultRequest::new(FaultTarget::SnapshotStore, operation, request_identity),
            response_items,
        )
    }

    fn tick(&mut self) -> ClientResult<u64> {
        self.logical_clock = self.logical_clock.checked_add(1).ok_or_else(|| {
            ClientError::new(ClientErrorKind::Internal, "snapshot-store clock overflow")
        })?;
        Ok(self.logical_clock)
    }

    fn decide_fault(
        &self,
        operation: &'static str,
        request_identity: Vec<u8>,
        response_items: u32,
    ) -> ClientResult<FaultDecision> {
        let decision = self.preview_fault(operation, &request_identity, response_items);
        self.last_fault.set(Some(decision));
        if let Some(error) = decision.client_error() {
            return Err(error);
        }
        Ok(decision)
    }
}

impl SnapshotStoreClient for InMemorySnapshotStore {
    fn create_node(&mut self, request: CreateNodeRequest) -> ClientResult<CreateNodeResponse> {
        self.decide_fault("create_node", request_identity(&request), 1)?;
        request.validate()?;

        if let Some(experiment) = self.experiments.get(&request.experiment_id) {
            if let Some(existing) = experiment.nodes.get(&request.node_id) {
                let create_key = CreateNodeKey::from_request(&request);
                if existing.create_key == create_key {
                    return Ok(CreateNodeResponse {
                        node: existing.meta.clone(),
                    });
                }

                return Err(ClientError::new(
                    ClientErrorKind::AlreadyExists,
                    "node id already exists with different immutable create data",
                ));
            }
        }

        if !request.node_id.is_root() {
            let parent = request
                .parent_node_id
                .expect("request validation requires parent");
            let experiment = self
                .experiments
                .get(&request.experiment_id)
                .ok_or_else(|| {
                    ClientError::new(ClientErrorKind::NotFound, "parent experiment not found")
                })?;
            if !experiment.nodes.contains_key(&parent) {
                return Err(ClientError::new(
                    ClientErrorKind::NotFound,
                    "parent node not found",
                ));
            }
        }

        let created_at = self.tick()?;
        let experiment_id = request.experiment_id.clone();
        let parent_depth = request
            .parent_node_id
            .and_then(|parent| {
                self.experiments
                    .get(&experiment_id)
                    .and_then(|experiment| experiment.nodes.get(&parent))
                    .map(|node| node.meta.depth)
            })
            .unwrap_or(0);
        let input_log_container = request.input_log_container.clone();
        let input_log_id = resolved_input_log_id(&request);
        let create_key = CreateNodeKey::from_request(&request);

        let node = NodeMeta {
            experiment_id: experiment_id.clone(),
            node_id: request.node_id,
            parent_node_id: request.parent_node_id,
            depth: if request.node_id.is_root() {
                0
            } else {
                parent_depth.checked_add(1).ok_or_else(|| {
                    ClientError::new(ClientErrorKind::Internal, "node depth overflow")
                })?
            },
            snapshot_ref: request.snapshot_ref,
            input_log_id,
            status: request.status,
            progress_score: request.progress_score,
            novelty_score: request.novelty_score,
            visit_count: 0,
            expand_count: 0,
            last_visited_at: 0,
            created_at,
            updated_at: created_at,
            attrs: request.attrs,
        };

        let experiment = self
            .experiments
            .entry(experiment_id)
            .or_insert_with(ExperimentStore::default);
        experiment.insert_node(StoredNode {
            meta: node.clone(),
            create_key,
            input_log_container,
        });

        Ok(CreateNodeResponse { node })
    }

    fn update_nodes(&mut self, request: UpdateNodesRequest) -> ClientResult<UpdateNodesResponse> {
        self.decide_fault(
            "update_nodes",
            request_identity(&request),
            request.updates.len() as u32,
        )?;

        if request.updates.is_empty() {
            return Ok(UpdateNodesResponse {
                updated_at: self.logical_clock,
                applied: 0,
            });
        }

        let experiment = self
            .experiments
            .get(&request.experiment_id)
            .ok_or_else(|| ClientError::new(ClientErrorKind::NotFound, "experiment not found"))?;
        for update in &request.updates {
            if !experiment.nodes.contains_key(&update.node_id) {
                return Err(ClientError::new(
                    ClientErrorKind::NotFound,
                    "node not found",
                ));
            }
            if let Some(attrs) = &update.attrs {
                attrs.validate()?;
            }
        }

        let applied = request.updates.len() as u32;
        let updated_at = self.tick()?;
        let experiment = self
            .experiments
            .get_mut(&request.experiment_id)
            .expect("experiment existence was checked");
        for update in request.updates {
            apply_update(
                experiment
                    .nodes
                    .get_mut(&update.node_id)
                    .expect("node existence was checked"),
                update,
                updated_at,
            )?;
        }

        Ok(UpdateNodesResponse {
            updated_at,
            applied,
        })
    }

    fn get_node(&self, request: GetNodeRequest) -> ClientResult<GetNodeResponse> {
        self.decide_fault("get_node", request_identity(&request), 1)?;
        let node = self
            .node(&request.experiment_id, request.node_id)?
            .meta
            .clone();
        Ok(GetNodeResponse { node })
    }

    fn get_children(&self, request: GetChildrenRequest) -> ClientResult<GetChildrenResponse> {
        let experiment = self.experiment(&request.experiment_id)?;
        let child_ids = experiment
            .children
            .get(&request.node_id)
            .ok_or_else(|| ClientError::new(ClientErrorKind::NotFound, "node not found"))?;
        let decision = self.decide_fault(
            "get_children",
            request_identity(&request),
            child_ids.len() as u32,
        )?;
        let mut children = child_ids
            .iter()
            .filter_map(|id| experiment.nodes.get(id))
            .map(|node| node.meta.clone())
            .collect::<Vec<_>>();
        children.truncate(decision.truncate_len(children.len()));
        Ok(GetChildrenResponse { children })
    }

    fn get_path(&self, request: GetPathRequest) -> ClientResult<GetPathResponse> {
        let experiment = self.experiment(&request.experiment_id)?;
        let mut path_ids = Vec::new();
        let mut current = request.node_id;
        loop {
            let node = experiment
                .nodes
                .get(&current)
                .ok_or_else(|| ClientError::new(ClientErrorKind::NotFound, "node not found"))?;
            path_ids.push(current);
            if let Some(parent) = node.meta.parent_node_id {
                current = parent;
            } else {
                break;
            }
        }
        path_ids.reverse();

        let decision = self.decide_fault(
            "get_path",
            request_identity(&request),
            path_ids.len() as u32,
        )?;
        let keep = decision.truncate_len(path_ids.len());
        path_ids.truncate(keep);

        let mut nodes = Vec::with_capacity(path_ids.len());
        let mut input_log_containers = Vec::new();
        for id in path_ids {
            let node = experiment.nodes.get(&id).expect("path id exists");
            if request.include_input_logs {
                if let Some(container) = &node.input_log_container {
                    input_log_containers.push(container.clone());
                }
            }
            nodes.push(node.meta.clone());
        }

        Ok(GetPathResponse {
            nodes,
            input_log_containers,
        })
    }

    fn query_nodes(&self, request: QueryNodesRequest) -> ClientResult<QueryNodesResponse> {
        let experiment = self.experiment(&request.experiment_id)?;
        let mut nodes = experiment
            .nodes
            .values()
            .filter(|node| query_matches(&node.meta, &request))
            .map(|node| node.meta.clone())
            .collect::<Vec<_>>();

        sort_query_nodes(&mut nodes, request.order_by);
        if let Some(limit) = request.limit {
            nodes.truncate(limit as usize);
        }

        let decision = self.decide_fault(
            "query_nodes",
            request_identity(&request),
            nodes.len() as u32,
        )?;
        nodes.truncate(decision.truncate_len(nodes.len()));
        Ok(QueryNodesResponse { nodes })
    }

    fn put_metadata(&mut self, request: PutMetadataRequest) -> ClientResult<PutMetadataResponse> {
        self.decide_fault("put_metadata", request_identity(&request), 1)?;
        let current_generation = self
            .metadata
            .get(&request.key)
            .map(|entry| entry.generation);
        check_metadata_expectation(current_generation, request.expected_generation, true)?;

        let generation = MetadataGeneration::new(
            current_generation.map_or(1, |generation| generation.get().saturating_add(1)),
        );
        self.metadata.insert(
            request.key,
            MetadataEntry {
                value: request.value,
                generation,
            },
        );
        Ok(PutMetadataResponse { generation })
    }

    fn get_metadata(&self, request: GetMetadataRequest) -> ClientResult<GetMetadataResponse> {
        self.decide_fault("get_metadata", request_identity(&request), 1)?;
        let entry = self
            .metadata
            .get(&request.key)
            .ok_or_else(|| ClientError::new(ClientErrorKind::NotFound, "metadata not found"))?;
        Ok(GetMetadataResponse {
            value: entry.value.clone(),
            generation: entry.generation,
        })
    }

    fn delete_metadata(
        &mut self,
        request: DeleteMetadataRequest,
    ) -> ClientResult<DeleteMetadataResponse> {
        self.decide_fault("delete_metadata", request_identity(&request), 1)?;
        let current_generation = self
            .metadata
            .get(&request.key)
            .map(|entry| entry.generation);
        check_metadata_expectation(current_generation, request.expected_generation, false)?;
        self.metadata
            .remove(&request.key)
            .ok_or_else(|| ClientError::new(ClientErrorKind::NotFound, "metadata not found"))?;
        Ok(DeleteMetadataResponse)
    }

    fn prune_subtree(
        &mut self,
        request: orch_clients::snapshot_store::PruneSubtreeRequest,
    ) -> ClientResult<orch_clients::snapshot_store::PruneSubtreeResponse> {
        self.decide_fault("prune_subtree", request_identity(&request), 1)?;
        if request.node_id.is_root() && !request.allow_root {
            return Err(ClientError::new(
                ClientErrorKind::FailedPrecondition,
                "root prune requires allow_root",
            ));
        }

        let experiment = self
            .experiments
            .get(&request.experiment_id)
            .ok_or_else(|| ClientError::new(ClientErrorKind::NotFound, "experiment not found"))?;
        if !experiment.nodes.contains_key(&request.node_id) {
            return Err(ClientError::new(
                ClientErrorKind::NotFound,
                "node not found",
            ));
        }
        let subtree = experiment.subtree_ids(request.node_id);
        let updated_at = self.tick()?;
        let experiment = self
            .experiments
            .get_mut(&request.experiment_id)
            .expect("experiment existence was checked");
        for id in &subtree {
            let node = experiment.nodes.get_mut(id).expect("subtree id exists");
            node.meta.status = NodeStatus::Pruned;
            node.meta.updated_at = updated_at;
        }

        Ok(orch_clients::snapshot_store::PruneSubtreeResponse {
            nodes_pruned: subtree.len() as u64,
        })
    }
}

impl InMemorySnapshotStore {
    fn experiment(&self, experiment_id: &str) -> ClientResult<&ExperimentStore> {
        self.experiments
            .get(experiment_id)
            .ok_or_else(|| ClientError::new(ClientErrorKind::NotFound, "experiment not found"))
    }

    fn node(&self, experiment_id: &str, node_id: NodeId) -> ClientResult<&StoredNode> {
        self.experiment(experiment_id)?
            .nodes
            .get(&node_id)
            .ok_or_else(|| ClientError::new(ClientErrorKind::NotFound, "node not found"))
    }
}

#[derive(Clone, Debug, Default)]
struct ExperimentStore {
    nodes: BTreeMap<NodeId, StoredNode>,
    children: BTreeMap<NodeId, Vec<NodeId>>,
}

impl ExperimentStore {
    fn insert_node(&mut self, node: StoredNode) {
        let node_id = node.meta.node_id;
        self.children.entry(node_id).or_default();
        if let Some(parent) = node.meta.parent_node_id {
            self.children.entry(parent).or_default().push(node_id);
            self.children
                .get_mut(&parent)
                .expect("parent children were inserted")
                .sort_unstable();
        }
        self.nodes.insert(node_id, node);
    }

    fn subtree_ids(&self, root: NodeId) -> Vec<NodeId> {
        let mut stack = vec![root];
        let mut out = Vec::new();
        while let Some(id) = stack.pop() {
            out.push(id);
            if let Some(children) = self.children.get(&id) {
                for child in children.iter().rev() {
                    stack.push(*child);
                }
            }
        }
        out
    }
}

#[derive(Clone, Debug)]
struct StoredNode {
    meta: NodeMeta,
    create_key: CreateNodeKey,
    input_log_container: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
struct CreateNodeKey {
    parent_node_id: Option<NodeId>,
    snapshot_ref: orch_core::types::SnapshotRef,
    input_log_id: Option<InputLogId>,
    status: NodeStatus,
    progress_score: orch_core::types::Score,
    novelty_score: orch_core::types::Novelty,
    attrs: NodeAttrs,
    input_log_container: Option<Vec<u8>>,
}

impl CreateNodeKey {
    fn from_request(request: &CreateNodeRequest) -> Self {
        Self {
            parent_node_id: request.parent_node_id,
            snapshot_ref: request.snapshot_ref,
            input_log_id: request.input_log_id,
            status: request.status,
            progress_score: request.progress_score,
            novelty_score: request.novelty_score,
            attrs: request.attrs.clone(),
            input_log_container: request.input_log_container.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct MetadataEntry {
    value: Vec<u8>,
    generation: MetadataGeneration,
}

fn apply_update(node: &mut StoredNode, update: NodeUpdate, updated_at: u64) -> ClientResult<()> {
    if let Some(status) = update.status {
        node.meta.status = status;
    }
    if let Some(progress_score) = update.progress_score {
        node.meta.progress_score = progress_score;
    }
    if let Some(novelty_score) = update.novelty_score {
        node.meta.novelty_score = novelty_score;
    }
    if update.visit_count_delta != 0 {
        node.meta.visit_count = node
            .meta
            .visit_count
            .checked_add(update.visit_count_delta)
            .ok_or_else(|| ClientError::new(ClientErrorKind::Internal, "visit count overflow"))?;
    }
    if update.expand_count_delta != 0 {
        node.meta.expand_count = node
            .meta
            .expand_count
            .checked_add(update.expand_count_delta)
            .ok_or_else(|| ClientError::new(ClientErrorKind::Internal, "expand count overflow"))?;
    }
    if update.touch_visited {
        node.meta.last_visited_at = updated_at;
    }
    if let Some(attrs) = update.attrs {
        node.meta.attrs = attrs;
    }
    node.meta.updated_at = updated_at;
    Ok(())
}

fn resolved_input_log_id(request: &CreateNodeRequest) -> Option<InputLogId> {
    request.input_log_id.or_else(|| {
        request.input_log_container.as_ref().map(|container| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"orch-fakes/snapshot-store/input-log/v1");
            hasher.update(request.experiment_id.as_bytes());
            hasher.update(&request.node_id.get().to_le_bytes());
            hasher.update(container);
            InputLogId::new(*hasher.finalize().as_bytes())
        })
    })
}

fn check_metadata_expectation(
    current: Option<MetadataGeneration>,
    expected: MetadataExpectation,
    allow_create_only: bool,
) -> ClientResult<()> {
    match expected {
        MetadataExpectation::Unconditional => Ok(()),
        MetadataExpectation::CreateOnly if allow_create_only && current.is_none() => Ok(()),
        MetadataExpectation::CreateOnly if allow_create_only => Err(ClientError::new(
            ClientErrorKind::AlreadyExists,
            "metadata key already exists",
        )),
        MetadataExpectation::CreateOnly => Err(ClientError::new(
            ClientErrorKind::FailedPrecondition,
            "create-only expectation is invalid for delete",
        )),
        MetadataExpectation::Generation(expected) if current == Some(expected) => Ok(()),
        MetadataExpectation::Generation(_) => Err(ClientError::new(
            ClientErrorKind::FailedPrecondition,
            "metadata generation conflict",
        )),
    }
}

fn query_matches(node: &NodeMeta, request: &QueryNodesRequest) -> bool {
    (request.statuses.is_empty() || request.statuses.contains(&node.status))
        && request
            .min_progress
            .map_or(true, |min| node.progress_score >= min)
        && request
            .max_progress
            .map_or(true, |max| node.progress_score <= max)
        && request
            .min_novelty
            .map_or(true, |min| node.novelty_score >= min)
        && request.min_depth.map_or(true, |min| node.depth >= min)
        && request.max_depth.map_or(true, |max| node.depth <= max)
        && request
            .created_after
            .map_or(true, |after| node.created_at > after)
        && request
            .updated_after
            .map_or(true, |after| node.updated_at > after)
}

fn sort_query_nodes(nodes: &mut [NodeMeta], order_by: orch_clients::snapshot_store::OrderBy) {
    match order_by {
        orch_clients::snapshot_store::OrderBy::CreatedAt => {
            nodes.sort_by_key(|node| (node.created_at, node.node_id));
        }
        orch_clients::snapshot_store::OrderBy::ProgressDesc => {
            nodes.sort_by(|left, right| {
                right
                    .progress_score
                    .cmp(&left.progress_score)
                    .then_with(|| left.created_at.cmp(&right.created_at))
                    .then_with(|| left.node_id.cmp(&right.node_id))
            });
        }
        orch_clients::snapshot_store::OrderBy::NoveltyDesc => {
            nodes.sort_by(|left, right| {
                right
                    .novelty_score
                    .cmp(&left.novelty_score)
                    .then_with(|| left.created_at.cmp(&right.created_at))
                    .then_with(|| left.node_id.cmp(&right.node_id))
            });
        }
    }
}

fn request_identity<T: Serialize>(request: &T) -> Vec<u8> {
    postcard::to_allocvec(request).expect("snapshot-store request DTO serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_clients::snapshot_store::{OrderBy, PruneSubtreeRequest};
    use orch_core::types::{Novelty, Score, SnapshotRef};

    const EXP_A: &str = "exp-a";
    const EXP_B: &str = "exp-b";
    const SNAPSHOT_A: SnapshotRef = SnapshotRef::new([0xA5; 32]);
    const SNAPSHOT_B: SnapshotRef = SnapshotRef::new([0x5A; 32]);
    const LOG_A: InputLogId = InputLogId::new([0x11; 32]);

    #[test]
    fn snapshot_store_root_and_dense_caller_ids_are_experiment_scoped() {
        let mut store = InMemorySnapshotStore::new();

        let root = store.create_node(root_request(EXP_A)).unwrap().node;
        let child_1 = store
            .create_node(child_request(
                EXP_A,
                1,
                NodeId::ROOT,
                10.0,
                NodeStatus::Frontier,
            ))
            .unwrap()
            .node;
        let child_2 = store
            .create_node(child_request(
                EXP_A,
                2,
                NodeId::new(1),
                20.0,
                NodeStatus::Goal,
            ))
            .unwrap()
            .node;
        let other_root = store.create_node(root_request(EXP_B)).unwrap().node;

        assert_eq!(root.node_id, NodeId::ROOT);
        assert_eq!(root.parent_node_id, None);
        assert_eq!(root.depth, 0);
        assert_eq!(child_1.node_id, NodeId::new(1));
        assert_eq!(child_1.parent_node_id, Some(NodeId::ROOT));
        assert_eq!(child_1.depth, 1);
        assert_eq!(child_2.node_id, NodeId::new(2));
        assert_eq!(child_2.depth, 2);
        assert_eq!(other_root.experiment_id, EXP_B);
        assert_eq!(other_root.node_id, NodeId::ROOT);
    }

    #[test]
    fn snapshot_store_create_is_idempotent_and_rejects_mismatch() {
        let mut store = InMemorySnapshotStore::new();
        let request = root_request(EXP_A);

        let first = store.create_node(request.clone()).unwrap().node;
        let retry = store.create_node(request).unwrap().node;
        let mut mismatch = root_request(EXP_A);
        mismatch.attrs = NodeAttrs::new(b"different".to_vec()).unwrap();
        let error = store
            .create_node(mismatch)
            .expect_err("same node id with different create data should fail");

        assert_eq!(retry, first);
        assert_eq!(error.kind(), ClientErrorKind::AlreadyExists);
    }

    #[test]
    fn snapshot_store_update_and_status_filtered_query_nodes() {
        let mut store = populated_store();
        let response = store
            .update_nodes(UpdateNodesRequest {
                experiment_id: EXP_A.to_owned(),
                updates: vec![NodeUpdate {
                    node_id: NodeId::new(1),
                    status: Some(NodeStatus::Expanded),
                    progress_score: Some(score(99.0)),
                    novelty_score: None,
                    visit_count_delta: 2,
                    expand_count_delta: 1,
                    touch_visited: true,
                    attrs: Some(NodeAttrs::new(b"updated".to_vec()).unwrap()),
                }],
            })
            .unwrap();

        let updated = store
            .get_node(GetNodeRequest {
                experiment_id: EXP_A.to_owned(),
                node_id: NodeId::new(1),
            })
            .unwrap()
            .node;
        let query = store
            .query_nodes(QueryNodesRequest {
                experiment_id: EXP_A.to_owned(),
                statuses: vec![NodeStatus::Expanded],
                min_progress: Some(score(90.0)),
                max_progress: None,
                min_novelty: None,
                min_depth: Some(1),
                max_depth: Some(1),
                created_after: None,
                updated_after: Some(0),
                order_by: OrderBy::ProgressDesc,
                limit: None,
            })
            .unwrap();

        assert_eq!(response.applied, 1);
        assert_eq!(updated.status, NodeStatus::Expanded);
        assert_eq!(updated.progress_score, score(99.0));
        assert_eq!(updated.visit_count, 2);
        assert_eq!(updated.expand_count, 1);
        assert_eq!(updated.last_visited_at, response.updated_at);
        assert_eq!(updated.attrs, NodeAttrs::new(b"updated".to_vec()).unwrap());
        assert_eq!(
            query
                .nodes
                .iter()
                .map(|node| node.node_id)
                .collect::<Vec<_>>(),
            vec![NodeId::new(1)]
        );
    }

    #[test]
    fn snapshot_store_path_children_and_prune_reads_are_deterministic() {
        let mut store = populated_store();

        let root_children = store
            .get_children(GetChildrenRequest {
                experiment_id: EXP_A.to_owned(),
                node_id: NodeId::ROOT,
            })
            .unwrap();
        let path = store
            .get_path(GetPathRequest {
                experiment_id: EXP_A.to_owned(),
                node_id: NodeId::new(2),
                include_input_logs: true,
            })
            .unwrap();
        let prune = store
            .prune_subtree(PruneSubtreeRequest {
                experiment_id: EXP_A.to_owned(),
                node_id: NodeId::new(1),
                allow_root: false,
            })
            .unwrap();
        let pruned = store
            .query_nodes(QueryNodesRequest {
                experiment_id: EXP_A.to_owned(),
                statuses: vec![NodeStatus::Pruned],
                min_progress: None,
                max_progress: None,
                min_novelty: None,
                min_depth: None,
                max_depth: None,
                created_after: None,
                updated_after: None,
                order_by: OrderBy::CreatedAt,
                limit: None,
            })
            .unwrap();

        assert_eq!(
            root_children
                .children
                .iter()
                .map(|node| node.node_id)
                .collect::<Vec<_>>(),
            vec![NodeId::new(1), NodeId::new(3)]
        );
        assert_eq!(
            path.nodes
                .iter()
                .map(|node| node.node_id)
                .collect::<Vec<_>>(),
            vec![NodeId::ROOT, NodeId::new(1), NodeId::new(2)]
        );
        assert_eq!(
            path.input_log_containers,
            vec![b"log-1".to_vec(), b"log-2".to_vec()]
        );
        assert_eq!(prune.nodes_pruned, 2);
        assert_eq!(
            pruned
                .nodes
                .iter()
                .map(|node| node.node_id)
                .collect::<Vec<_>>(),
            vec![NodeId::new(1), NodeId::new(2)]
        );
    }

    #[test]
    fn snapshot_store_metadata_generations_and_cas_conflicts() {
        let mut store = InMemorySnapshotStore::new();
        let ckpt = MetadataKey::checkpoint(EXP_A);
        let wal = MetadataKey::wal(EXP_A, 42);

        let first = store
            .put_metadata(PutMetadataRequest {
                key: ckpt.clone(),
                value: b"checkpoint-v1".to_vec(),
                expected_generation: MetadataExpectation::create_only(),
            })
            .unwrap();
        let conflict = store
            .put_metadata(PutMetadataRequest {
                key: ckpt.clone(),
                value: b"checkpoint-conflict".to_vec(),
                expected_generation: MetadataExpectation::create_only(),
            })
            .expect_err("create-only should reject existing metadata");
        let second = store
            .put_metadata(PutMetadataRequest {
                key: ckpt.clone(),
                value: b"checkpoint-v2".to_vec(),
                expected_generation: MetadataExpectation::generation(first.generation),
            })
            .unwrap();
        let stale = store
            .put_metadata(PutMetadataRequest {
                key: ckpt.clone(),
                value: b"checkpoint-stale".to_vec(),
                expected_generation: MetadataExpectation::generation(first.generation),
            })
            .expect_err("stale generation should conflict");
        let wal_put = store
            .put_metadata(PutMetadataRequest {
                key: wal.clone(),
                value: b"wal-42".to_vec(),
                expected_generation: MetadataExpectation::create_only(),
            })
            .unwrap();
        let fetched = store
            .get_metadata(GetMetadataRequest { key: ckpt.clone() })
            .unwrap();
        store
            .delete_metadata(DeleteMetadataRequest {
                key: wal,
                expected_generation: MetadataExpectation::generation(wal_put.generation),
            })
            .unwrap();

        assert_eq!(ckpt.as_str(), "orch/ckpt/exp-a");
        assert_eq!(
            MetadataKey::wal(EXP_A, 42).as_str(),
            "orch/wal/exp-a/00000000000000000042"
        );
        assert_eq!(first.generation, MetadataGeneration::new(1));
        assert_eq!(second.generation, MetadataGeneration::new(2));
        assert_eq!(fetched.value, b"checkpoint-v2");
        assert_eq!(fetched.generation, second.generation);
        assert_eq!(conflict.kind(), ClientErrorKind::AlreadyExists);
        assert_eq!(stale.kind(), ClientErrorKind::FailedPrecondition);
    }

    #[test]
    fn snapshot_store_fault_knobs_are_deterministic_and_disable_cleanly() {
        let plan = FaultPlan::disabled(0x5eed)
            .with_latency(crate::fault::LatencyFault::new(3, 19))
            .with_partial_response(crate::fault::PartialResponseFault::new(
                crate::fault::FaultRate::always(),
                0,
            ));
        let same_a = InMemorySnapshotStore::with_fault_plan(plan.clone()).preview_fault(
            "query_nodes",
            b"same-request",
            8,
        );
        let same_b = InMemorySnapshotStore::with_fault_plan(plan.clone()).preview_fault(
            "query_nodes",
            b"same-request",
            8,
        );
        let different_seed = InMemorySnapshotStore::with_fault_plan(
            FaultPlan::disabled(0x5eee)
                .with_latency(crate::fault::LatencyFault::new(3, 19))
                .with_partial_response(crate::fault::PartialResponseFault::new(
                    crate::fault::FaultRate::always(),
                    0,
                )),
        )
        .preview_fault("query_nodes", b"same-request", 8);

        let baseline = baseline_transcript(InMemorySnapshotStore::new());
        let disabled = baseline_transcript(InMemorySnapshotStore::with_fault_plan(
            FaultPlan::disabled(u64::MAX),
        ));
        let mut partial_store = InMemorySnapshotStore::with_fault_plan(plan);
        populate(&mut partial_store);
        let partial = partial_store
            .get_children(GetChildrenRequest {
                experiment_id: EXP_A.to_owned(),
                node_id: NodeId::ROOT,
            })
            .unwrap();

        assert_eq!(same_a, same_b);
        assert_ne!(same_a, different_seed);
        assert_eq!(baseline, disabled);
        assert_eq!(
            partial.children.len(),
            partial_store.last_fault().unwrap().truncate_len(2)
        );
        assert!(partial.children.len() < 2);
    }

    fn populated_store() -> InMemorySnapshotStore {
        let mut store = InMemorySnapshotStore::new();
        populate(&mut store);
        store
    }

    fn populate(store: &mut InMemorySnapshotStore) {
        store.create_node(root_request(EXP_A)).unwrap();
        store
            .create_node(child_request(
                EXP_A,
                1,
                NodeId::ROOT,
                10.0,
                NodeStatus::Frontier,
            ))
            .unwrap();
        store
            .create_node(child_request(
                EXP_A,
                2,
                NodeId::new(1),
                20.0,
                NodeStatus::Goal,
            ))
            .unwrap();
        store
            .create_node(child_request(
                EXP_A,
                3,
                NodeId::ROOT,
                5.0,
                NodeStatus::Frontier,
            ))
            .unwrap();
    }

    fn baseline_transcript(mut store: InMemorySnapshotStore) -> Vec<NodeId> {
        populate(&mut store);
        store
            .query_nodes(QueryNodesRequest {
                experiment_id: EXP_A.to_owned(),
                statuses: Vec::new(),
                min_progress: None,
                max_progress: None,
                min_novelty: None,
                min_depth: None,
                max_depth: None,
                created_after: None,
                updated_after: None,
                order_by: OrderBy::CreatedAt,
                limit: None,
            })
            .unwrap()
            .nodes
            .into_iter()
            .map(|node| node.node_id)
            .collect()
    }

    fn root_request(experiment_id: &str) -> CreateNodeRequest {
        CreateNodeRequest {
            experiment_id: experiment_id.to_owned(),
            node_id: NodeId::ROOT,
            parent_node_id: None,
            snapshot_ref: SNAPSHOT_A,
            input_log_id: None,
            status: NodeStatus::Frontier,
            progress_score: score(0.0),
            novelty_score: novelty(1.0),
            attrs: NodeAttrs::new(b"root".to_vec()).unwrap(),
            input_log_container: None,
        }
    }

    fn child_request(
        experiment_id: &str,
        node_id: u64,
        parent: NodeId,
        progress: f64,
        status: NodeStatus,
    ) -> CreateNodeRequest {
        CreateNodeRequest {
            experiment_id: experiment_id.to_owned(),
            node_id: NodeId::new(node_id),
            parent_node_id: Some(parent),
            snapshot_ref: if node_id % 2 == 0 {
                SNAPSHOT_B
            } else {
                SNAPSHOT_A
            },
            input_log_id: (node_id == 3).then_some(LOG_A),
            status,
            progress_score: score(progress),
            novelty_score: novelty(1.0 / (node_id as f64 + 1.0)),
            attrs: NodeAttrs::new(format!("node-{node_id}").into_bytes()).unwrap(),
            input_log_container: (node_id != 3).then(|| format!("log-{node_id}").into_bytes()),
        }
    }

    fn score(value: f64) -> Score {
        Score::new(value).unwrap()
    }

    fn novelty(value: f64) -> Novelty {
        Novelty::new(value).unwrap()
    }
}
