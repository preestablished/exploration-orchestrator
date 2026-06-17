//! Frontier membership and deterministic eligibility helpers.

use core::fmt;
use core::hash::{BuildHasherDefault, Hasher};

use hashbrown::HashMap;

use crate::tree::Tree;
use crate::types::NodeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontierError {
    Duplicate(NodeId),
    Missing(NodeId),
    UnknownNode(NodeId),
}

impl fmt::Display for FrontierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(id) => write!(formatter, "node {:?} already in frontier", id),
            Self::Missing(id) => write!(formatter, "node {:?} not in frontier", id),
            Self::UnknownNode(id) => write!(formatter, "node {:?} not in tree", id),
        }
    }
}

impl std::error::Error for FrontierError {}

type FrontierIndex = HashMap<NodeId, usize, BuildHasherDefault<FrontierHasher>>;

#[derive(Clone, Copy, Debug, Default)]
struct FrontierHasher(u64);

impl Hasher for FrontierHasher {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.write(&value.to_le_bytes());
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frontier {
    entries: Vec<NodeId>,
    index: FrontierIndex,
}

impl Frontier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_root() -> Self {
        let mut frontier = Self::new();
        frontier
            .insert(NodeId::ROOT)
            .expect("fresh frontier is empty");
        frontier
    }

    pub fn insert(&mut self, id: NodeId) -> Result<(), FrontierError> {
        if self.index.contains_key(&id) {
            return Err(FrontierError::Duplicate(id));
        }

        let entry_index = self.entries.len();
        self.entries.push(id);
        self.index.insert(id, entry_index);
        Ok(())
    }

    pub fn remove(&mut self, id: NodeId) -> Result<(), FrontierError> {
        let entry_index = self.index.remove(&id).ok_or(FrontierError::Missing(id))?;
        let removed = self.entries.swap_remove(entry_index);
        debug_assert_eq!(removed, id);

        if let Some(swapped) = self.entries.get(entry_index).copied() {
            self.index.insert(swapped, entry_index);
        }

        Ok(())
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.index.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[NodeId] {
        &self.entries
    }

    pub fn deterministic_entries(&self) -> Vec<NodeId> {
        let mut ids = self.entries.clone();
        ids.sort_unstable();
        ids
    }

    pub fn deterministic_candidates(&self, tree: &Tree) -> Result<Vec<NodeId>, FrontierError> {
        let mut candidates = Vec::new();

        for id in &self.entries {
            let record = tree.get(*id).ok_or(FrontierError::UnknownNode(*id))?;
            if record.is_frontier_candidate() {
                candidates.push(*id);
            }
        }

        candidates.sort_unstable();
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{NodePayload, Tree};
    use crate::types::{CellKey, FrameCount, Novelty, Score, SnapshotRef, Stage, StateHash};

    fn payload(seed: u8) -> NodePayload {
        NodePayload::new(
            SnapshotRef::new([seed; 32]),
            Score::new(f64::from(seed)).unwrap(),
            Novelty::new(1.0).unwrap(),
            CellKey::new(u64::from(seed)),
            StateHash::new([seed.wrapping_add(1); 32]),
            Stage::new(u16::from(seed)),
            FrameCount::new(u32::from(seed) * 10),
        )
    }

    fn sample_tree() -> (Tree, [NodeId; 4]) {
        let mut tree = Tree::from_root(payload(0));
        let first = tree.insert_child(NodeId::ROOT, payload(1)).unwrap();
        let second = tree.insert_child(NodeId::ROOT, payload(2)).unwrap();
        let third = tree.insert_child(first, payload(3)).unwrap();
        (tree, [NodeId::ROOT, first, second, third])
    }

    #[test]
    fn frontier_insert_rejects_duplicates_and_tracks_membership() {
        let mut frontier = Frontier::new();

        assert!(frontier.is_empty());
        frontier.insert(NodeId::new(2)).unwrap();

        assert_eq!(frontier.len(), 1);
        assert!(frontier.contains(NodeId::new(2)));
        assert_eq!(
            frontier.insert(NodeId::new(2)),
            Err(FrontierError::Duplicate(NodeId::new(2)))
        );
        assert_eq!(frontier.entries(), &[NodeId::new(2)]);
    }

    #[test]
    fn frontier_remove_by_swap_repairs_indices() {
        let mut frontier = Frontier::new();
        for id in [
            NodeId::new(1),
            NodeId::new(2),
            NodeId::new(3),
            NodeId::new(4),
        ] {
            frontier.insert(id).unwrap();
        }

        frontier.remove(NodeId::new(2)).unwrap();

        assert!(!frontier.contains(NodeId::new(2)));
        assert!(frontier.contains(NodeId::new(1)));
        assert!(frontier.contains(NodeId::new(3)));
        assert!(frontier.contains(NodeId::new(4)));
        assert_eq!(frontier.len(), 3);

        frontier.remove(NodeId::new(4)).unwrap();
        frontier.remove(NodeId::new(1)).unwrap();
        frontier.remove(NodeId::new(3)).unwrap();

        assert!(frontier.is_empty());
        assert_eq!(
            frontier.remove(NodeId::new(3)),
            Err(FrontierError::Missing(NodeId::new(3)))
        );
    }

    #[test]
    fn frontier_deterministic_entries_are_sorted_by_node_id() {
        let mut frontier = Frontier::new();
        for id in [NodeId::new(9), NodeId::new(1), NodeId::new(5), NodeId::ROOT] {
            frontier.insert(id).unwrap();
        }

        assert_ne!(
            frontier.entries(),
            frontier.deterministic_entries().as_slice()
        );
        assert_eq!(
            frontier.deterministic_entries(),
            vec![NodeId::ROOT, NodeId::new(1), NodeId::new(5), NodeId::new(9)]
        );
    }

    #[test]
    fn frontier_candidates_exclude_exhausted_and_non_frontier_nodes() {
        let (mut tree, [root, first, second, third]) = sample_tree();
        let mut frontier = Frontier::new();
        for id in [third, second, first, root] {
            frontier.insert(id).unwrap();
        }

        tree.mark_exhausted(first).unwrap();
        tree.mark_goal(second).unwrap();
        tree.mark_pruned(third).unwrap();

        assert_eq!(
            frontier.deterministic_candidates(&tree).unwrap(),
            vec![root]
        );
        assert!(frontier.contains(first));
        assert!(frontier.contains(second));
        assert!(frontier.contains(third));
    }

    #[test]
    fn frontier_candidates_report_unknown_tree_nodes() {
        let (tree, [_root, _first, _second, _third]) = sample_tree();
        let mut frontier = Frontier::new();
        frontier.insert(NodeId::new(99)).unwrap();

        assert_eq!(
            frontier.deterministic_candidates(&tree),
            Err(FrontierError::UnknownNode(NodeId::new(99)))
        );
    }
}
