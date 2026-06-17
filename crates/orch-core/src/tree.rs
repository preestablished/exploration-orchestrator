//! Node records and in-memory search tree index.

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::types::{
    CellKey, FrameCount, NodeId, NodeStatus, Novelty, Score, SnapshotRef, Stage, StateHash,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodePayload {
    pub snapshot: SnapshotRef,
    pub score: Score,
    pub novelty_at_commit: Novelty,
    pub cell: CellKey,
    pub state_hash: StateHash,
    pub stage: Stage,
    pub frame_counter: FrameCount,
}

impl NodePayload {
    pub const fn new(
        snapshot: SnapshotRef,
        score: Score,
        novelty_at_commit: Novelty,
        cell: CellKey,
        state_hash: StateHash,
        stage: Stage,
        frame_counter: FrameCount,
    ) -> Self {
        Self {
            snapshot,
            score,
            novelty_at_commit,
            cell,
            state_hash,
            stage,
            frame_counter,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeRecord {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub snapshot: SnapshotRef,
    pub depth: u32,
    pub score: Score,
    pub novelty_at_commit: Novelty,
    pub cell: CellKey,
    pub state_hash: StateHash,
    pub stage: Stage,
    pub frame_counter: FrameCount,
    pub visits: u32,
    pub children: u32,
    pub exhausted: bool,
    pub goal: bool,
    pub status: NodeStatus,
}

impl NodeRecord {
    pub fn root(payload: NodePayload) -> Self {
        Self::new(NodeId::ROOT, None, 0, payload)
    }

    pub fn child(id: NodeId, parent: NodeId, depth: u32, payload: NodePayload) -> Self {
        Self::new(id, Some(parent), depth, payload)
    }

    pub fn new(id: NodeId, parent: Option<NodeId>, depth: u32, payload: NodePayload) -> Self {
        Self {
            id,
            parent,
            snapshot: payload.snapshot,
            depth,
            score: payload.score,
            novelty_at_commit: payload.novelty_at_commit,
            cell: payload.cell,
            state_hash: payload.state_hash,
            stage: payload.stage,
            frame_counter: payload.frame_counter,
            visits: 0,
            children: 0,
            exhausted: false,
            goal: false,
            status: NodeStatus::Frontier,
        }
    }

    pub const fn payload(&self) -> NodePayload {
        NodePayload {
            snapshot: self.snapshot,
            score: self.score,
            novelty_at_commit: self.novelty_at_commit,
            cell: self.cell,
            state_hash: self.state_hash,
            stage: self.stage,
            frame_counter: self.frame_counter,
        }
    }

    pub const fn is_root(&self) -> bool {
        self.id.is_root() && self.parent.is_none()
    }

    pub const fn is_frontier_candidate(&self) -> bool {
        matches!(self.status, NodeStatus::Frontier) && !self.exhausted
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeError {
    InvalidRootId { actual: NodeId },
    RootHasParent { parent: NodeId },
    RootDepthNotZero { depth: u32 },
    MissingParent { parent: NodeId },
    NodeNotFound { id: NodeId },
    RootHasChildren { children: u32 },
    RootHasVisits { visits: u32 },
    RootIsExhausted,
    RootIsGoal,
    RootStatusNotFrontier { status: NodeStatus },
    DepthOverflow { parent: NodeId },
    ChildCountOverflow { parent: NodeId },
    VisitOverflow { id: NodeId },
    TerminalPrunedNode { id: NodeId },
}

impl fmt::Display for TreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRootId { actual } => write!(formatter, "invalid root id {:?}", actual),
            Self::RootHasParent { parent } => {
                write!(formatter, "root cannot have parent {:?}", parent)
            }
            Self::RootDepthNotZero { depth } => {
                write!(formatter, "root depth must be 0, got {depth}")
            }
            Self::MissingParent { parent } => write!(formatter, "missing parent {:?}", parent),
            Self::NodeNotFound { id } => write!(formatter, "node {:?} not found", id),
            Self::RootHasChildren { children } => {
                write!(formatter, "root child count must be 0, got {children}")
            }
            Self::RootHasVisits { visits } => {
                write!(formatter, "root visit count must be 0, got {visits}")
            }
            Self::RootIsExhausted => write!(formatter, "root cannot start exhausted"),
            Self::RootIsGoal => write!(formatter, "root cannot start as goal"),
            Self::RootStatusNotFrontier { status } => {
                write!(formatter, "root status must be Frontier, got {:?}", status)
            }
            Self::DepthOverflow { parent } => {
                write!(formatter, "depth overflow under {:?}", parent)
            }
            Self::ChildCountOverflow { parent } => {
                write!(formatter, "child count overflow under {:?}", parent)
            }
            Self::VisitOverflow { id } => write!(formatter, "visit count overflow for {:?}", id),
            Self::TerminalPrunedNode { id } => {
                write!(formatter, "pruned node {:?} cannot transition", id)
            }
        }
    }
}

impl std::error::Error for TreeError {}

#[derive(Clone, Copy, Debug)]
pub struct NodeContext<'a> {
    pub record: &'a NodeRecord,
    pub parent: Option<&'a NodeRecord>,
    pub children: &'a [NodeId],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Tree {
    records: Vec<NodeRecord>,
    children_by_node: Vec<Vec<NodeId>>,
}

impl Tree {
    pub fn from_root(payload: NodePayload) -> Self {
        Self::from_root_record(NodeRecord::root(payload)).expect("fresh root record is valid")
    }

    pub fn from_root_record(root: NodeRecord) -> Result<Self, TreeError> {
        validate_root(&root)?;
        Ok(Self {
            records: vec![root],
            children_by_node: vec![Vec::new()],
        })
    }

    pub fn insert_child(
        &mut self,
        parent: NodeId,
        payload: NodePayload,
    ) -> Result<NodeId, TreeError> {
        let parent_index = self
            .index_of(parent)
            .ok_or(TreeError::MissingParent { parent })?;
        let parent_depth = self.records[parent_index].depth;
        let child_depth = parent_depth
            .checked_add(1)
            .ok_or(TreeError::DepthOverflow { parent })?;
        let child_count = self.records[parent_index]
            .children
            .checked_add(1)
            .ok_or(TreeError::ChildCountOverflow { parent })?;
        let child_id = self.next_id();

        self.records[parent_index].children = child_count;
        self.children_by_node[parent_index].push(child_id);
        self.records
            .push(NodeRecord::child(child_id, parent, child_depth, payload));
        self.children_by_node.push(Vec::new());

        Ok(child_id)
    }

    pub fn root(&self) -> &NodeRecord {
        &self.records[0]
    }

    pub fn get(&self, id: NodeId) -> Option<&NodeRecord> {
        self.index_of(id).map(|index| &self.records[index])
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.index_of(id).is_some()
    }

    pub fn children_of(&self, id: NodeId) -> Result<&[NodeId], TreeError> {
        let index = self.index_or_not_found(id)?;
        Ok(&self.children_by_node[index])
    }

    pub fn context_of(&self, id: NodeId) -> Result<NodeContext<'_>, TreeError> {
        let index = self.index_or_not_found(id)?;
        let record = &self.records[index];
        let parent = record
            .parent
            .map(|parent_id| self.get(parent_id).expect("tree parent index is valid"));

        Ok(NodeContext {
            record,
            parent,
            children: &self.children_by_node[index],
        })
    }

    pub fn path_from_root(&self, id: NodeId) -> Result<Vec<NodeId>, TreeError> {
        let mut path = Vec::new();
        let mut current = id;

        loop {
            let record = self
                .get(current)
                .ok_or(TreeError::NodeNotFound { id: current })?;
            path.push(record.id);

            if let Some(parent) = record.parent {
                current = parent;
            } else {
                break;
            }
        }

        path.reverse();
        Ok(path)
    }

    pub fn increment_visits(&mut self, id: NodeId) -> Result<u32, TreeError> {
        let record = self.record_mut(id)?;
        record.visits = record
            .visits
            .checked_add(1)
            .ok_or(TreeError::VisitOverflow { id })?;
        Ok(record.visits)
    }

    pub fn mark_expanded(&mut self, id: NodeId) -> Result<(), TreeError> {
        let record = self.record_mut(id)?;
        if !record.goal && record.status != NodeStatus::Pruned {
            record.status = NodeStatus::Expanded;
        }
        Ok(())
    }

    pub fn mark_exhausted(&mut self, id: NodeId) -> Result<(), TreeError> {
        let record = self.record_mut(id)?;
        record.exhausted = true;
        if !record.goal && record.status != NodeStatus::Pruned {
            record.status = NodeStatus::Expanded;
        }
        Ok(())
    }

    pub fn mark_goal(&mut self, id: NodeId) -> Result<(), TreeError> {
        let record = self.record_mut(id)?;
        if record.status == NodeStatus::Pruned {
            return Err(TreeError::TerminalPrunedNode { id });
        }
        record.goal = true;
        record.exhausted = false;
        record.status = NodeStatus::Goal;
        Ok(())
    }

    pub fn mark_pruned(&mut self, id: NodeId) -> Result<(), TreeError> {
        let record = self.record_mut(id)?;
        record.exhausted = true;
        record.goal = false;
        record.status = NodeStatus::Pruned;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn next_id(&self) -> NodeId {
        NodeId::new(self.records.len() as u64)
    }

    fn record_mut(&mut self, id: NodeId) -> Result<&mut NodeRecord, TreeError> {
        let index = self.index_or_not_found(id)?;
        Ok(&mut self.records[index])
    }

    fn index_or_not_found(&self, id: NodeId) -> Result<usize, TreeError> {
        self.index_of(id).ok_or(TreeError::NodeNotFound { id })
    }

    fn index_of(&self, id: NodeId) -> Option<usize> {
        let index = usize::try_from(id.get()).ok()?;
        (index < self.records.len()).then_some(index)
    }
}

fn validate_root(root: &NodeRecord) -> Result<(), TreeError> {
    if !root.id.is_root() {
        return Err(TreeError::InvalidRootId { actual: root.id });
    }
    if let Some(parent) = root.parent {
        return Err(TreeError::RootHasParent { parent });
    }
    if root.depth != 0 {
        return Err(TreeError::RootDepthNotZero { depth: root.depth });
    }
    if root.children != 0 {
        return Err(TreeError::RootHasChildren {
            children: root.children,
        });
    }
    if root.visits != 0 {
        return Err(TreeError::RootHasVisits {
            visits: root.visits,
        });
    }
    if root.exhausted {
        return Err(TreeError::RootIsExhausted);
    }
    if root.goal {
        return Err(TreeError::RootIsGoal);
    }
    if root.status != NodeStatus::Frontier {
        return Err(TreeError::RootStatusNotFrontier {
            status: root.status,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(seed: u8, score: f64, novelty: f64, stage: u16, frame: u32) -> NodePayload {
        NodePayload::new(
            SnapshotRef::new([seed; 32]),
            Score::new(score).unwrap(),
            Novelty::new(novelty).unwrap(),
            CellKey::new(u64::from(seed)),
            StateHash::new([seed.wrapping_add(1); 32]),
            Stage::new(stage),
            FrameCount::new(frame),
        )
    }

    fn sample_tree() -> Tree {
        Tree::from_root(payload(0, 0.0, 1.0, 0, 10))
    }

    #[test]
    fn tree_root_invariants_and_context_lookup() {
        let tree = sample_tree();
        let root = tree.root();
        let context = tree.context_of(NodeId::ROOT).unwrap();

        assert_eq!(tree.len(), 1);
        assert!(!tree.is_empty());
        assert_eq!(tree.next_id(), NodeId::new(1));
        assert_eq!(root.id, NodeId::ROOT);
        assert_eq!(root.parent, None);
        assert_eq!(root.depth, 0);
        assert_eq!(root.visits, 0);
        assert_eq!(root.children, 0);
        assert_eq!(root.status, NodeStatus::Frontier);
        assert!(!root.exhausted);
        assert!(!root.goal);
        assert!(root.is_root());
        assert!(root.is_frontier_candidate());
        assert_eq!(context.record.id, NodeId::ROOT);
        assert!(context.parent.is_none());
        assert!(context.children.is_empty());
        assert_eq!(
            tree.path_from_root(NodeId::ROOT).unwrap(),
            vec![NodeId::ROOT]
        );
    }

    #[test]
    fn tree_rejects_invalid_root_records() {
        let mut invalid_id = NodeRecord::root(payload(0, 0.0, 1.0, 0, 10));
        invalid_id.id = NodeId::new(1);
        assert_eq!(
            Tree::from_root_record(invalid_id),
            Err(TreeError::InvalidRootId {
                actual: NodeId::new(1),
            })
        );

        let mut invalid_parent = NodeRecord::root(payload(0, 0.0, 1.0, 0, 10));
        invalid_parent.parent = Some(NodeId::new(9));
        assert_eq!(
            Tree::from_root_record(invalid_parent),
            Err(TreeError::RootHasParent {
                parent: NodeId::new(9),
            })
        );

        let mut invalid_depth = NodeRecord::root(payload(0, 0.0, 1.0, 0, 10));
        invalid_depth.depth = 2;
        assert_eq!(
            Tree::from_root_record(invalid_depth),
            Err(TreeError::RootDepthNotZero { depth: 2 })
        );

        let mut stale_children = NodeRecord::root(payload(0, 0.0, 1.0, 0, 10));
        stale_children.children = 1;
        assert_eq!(
            Tree::from_root_record(stale_children),
            Err(TreeError::RootHasChildren { children: 1 })
        );

        let mut stale_visits = NodeRecord::root(payload(0, 0.0, 1.0, 0, 10));
        stale_visits.visits = 1;
        assert_eq!(
            Tree::from_root_record(stale_visits),
            Err(TreeError::RootHasVisits { visits: 1 })
        );

        let mut exhausted = NodeRecord::root(payload(0, 0.0, 1.0, 0, 10));
        exhausted.exhausted = true;
        assert_eq!(
            Tree::from_root_record(exhausted),
            Err(TreeError::RootIsExhausted)
        );

        let mut goal = NodeRecord::root(payload(0, 0.0, 1.0, 0, 10));
        goal.goal = true;
        assert_eq!(Tree::from_root_record(goal), Err(TreeError::RootIsGoal));

        let mut expanded = NodeRecord::root(payload(0, 0.0, 1.0, 0, 10));
        expanded.status = NodeStatus::Expanded;
        assert_eq!(
            Tree::from_root_record(expanded),
            Err(TreeError::RootStatusNotFrontier {
                status: NodeStatus::Expanded,
            })
        );
    }

    #[test]
    fn tree_inserts_dense_children_and_counts_links() {
        let mut tree = sample_tree();

        let first = tree
            .insert_child(NodeId::ROOT, payload(1, 1.0, 0.5, 1, 20))
            .unwrap();
        let second = tree
            .insert_child(NodeId::ROOT, payload(2, 2.0, 0.25, 2, 30))
            .unwrap();
        let grandchild = tree
            .insert_child(first, payload(3, 3.0, 0.125, 3, 40))
            .unwrap();

        assert_eq!(first, NodeId::new(1));
        assert_eq!(second, NodeId::new(2));
        assert_eq!(grandchild, NodeId::new(3));
        assert_eq!(tree.next_id(), NodeId::new(4));
        assert_eq!(tree.root().children, 2);
        assert_eq!(tree.get(first).unwrap().children, 1);
        assert_eq!(tree.get(second).unwrap().children, 0);
        assert_eq!(tree.get(grandchild).unwrap().parent, Some(first));
        assert_eq!(tree.get(grandchild).unwrap().depth, 2);
        assert_eq!(tree.children_of(NodeId::ROOT).unwrap(), &[first, second]);
        assert_eq!(tree.children_of(first).unwrap(), &[grandchild]);
    }

    #[test]
    fn tree_rejects_missing_parent_and_unknown_lookup() {
        let mut tree = sample_tree();

        assert_eq!(
            tree.insert_child(NodeId::new(99), payload(1, 1.0, 0.5, 1, 20)),
            Err(TreeError::MissingParent {
                parent: NodeId::new(99),
            })
        );
        assert_eq!(
            tree.children_of(NodeId::new(99)),
            Err(TreeError::NodeNotFound {
                id: NodeId::new(99),
            })
        );
        assert!(!tree.contains(NodeId::new(99)));
    }

    #[test]
    fn tree_path_and_context_follow_parent_links() {
        let mut tree = sample_tree();
        let first = tree
            .insert_child(NodeId::ROOT, payload(1, 1.0, 0.5, 1, 20))
            .unwrap();
        let second = tree
            .insert_child(NodeId::ROOT, payload(2, 2.0, 0.25, 2, 30))
            .unwrap();
        let grandchild = tree
            .insert_child(first, payload(3, 3.0, 0.125, 3, 40))
            .unwrap();
        let context = tree.context_of(first).unwrap();

        assert_eq!(
            tree.path_from_root(grandchild).unwrap(),
            vec![NodeId::ROOT, first, grandchild]
        );
        assert_eq!(context.record.id, first);
        assert_eq!(context.parent.unwrap().id, NodeId::ROOT);
        assert_eq!(context.children, &[grandchild]);
        assert_eq!(
            tree.path_from_root(second).unwrap(),
            vec![NodeId::ROOT, second]
        );
    }

    #[test]
    fn tree_visit_and_status_transitions_are_explicit() {
        let mut tree = sample_tree();
        let expanded = tree
            .insert_child(NodeId::ROOT, payload(1, 1.0, 0.5, 1, 20))
            .unwrap();
        let goal = tree
            .insert_child(NodeId::ROOT, payload(2, 2.0, 0.25, 2, 30))
            .unwrap();
        let pruned = tree
            .insert_child(NodeId::ROOT, payload(3, 3.0, 0.125, 3, 40))
            .unwrap();

        assert_eq!(tree.increment_visits(expanded).unwrap(), 1);
        assert_eq!(tree.increment_visits(expanded).unwrap(), 2);
        tree.mark_exhausted(expanded).unwrap();
        assert_eq!(tree.get(expanded).unwrap().visits, 2);
        assert!(tree.get(expanded).unwrap().exhausted);
        assert_eq!(tree.get(expanded).unwrap().status, NodeStatus::Expanded);
        assert!(!tree.get(expanded).unwrap().is_frontier_candidate());

        tree.mark_goal(goal).unwrap();
        assert!(tree.get(goal).unwrap().goal);
        assert_eq!(tree.get(goal).unwrap().status, NodeStatus::Goal);

        tree.mark_pruned(pruned).unwrap();
        assert!(tree.get(pruned).unwrap().exhausted);
        assert!(!tree.get(pruned).unwrap().goal);
        assert_eq!(tree.get(pruned).unwrap().status, NodeStatus::Pruned);
        assert_eq!(
            tree.mark_goal(pruned),
            Err(TreeError::TerminalPrunedNode { id: pruned })
        );
        assert!(tree.get(pruned).unwrap().exhausted);
        assert!(!tree.get(pruned).unwrap().goal);
        assert_eq!(tree.get(pruned).unwrap().status, NodeStatus::Pruned);
    }
}
