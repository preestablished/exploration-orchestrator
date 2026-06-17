//! Selection-side cell mirror and state-hash duplicate routing.

use core::hash::{BuildHasherDefault, Hasher};

use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::types::{CellKey, NodeId, StateHash};

type CellCounts = HashMap<CellKey, u32, BuildHasherDefault<MirrorHasher>>;
type SeenEntries = HashMap<StateHash, NodeId, BuildHasherDefault<MirrorHasher>>;

#[derive(Clone, Copy, Debug, Default)]
struct MirrorHasher(u64);

impl Hasher for MirrorHasher {
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
pub struct CellMirror {
    counts: CellCounts,
}

impl CellMirror {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bump(&mut self, cell: CellKey) -> u32 {
        self.bump_by(cell, 1)
    }

    pub fn bump_by(&mut self, cell: CellKey, amount: u32) -> u32 {
        if amount == 0 {
            return self.count(cell);
        }

        let entry = self.counts.entry(cell).or_insert(0);
        *entry = entry
            .checked_add(amount)
            .expect("cell mirror count overflowed");
        *entry
    }

    pub fn count(&self, cell: CellKey) -> u32 {
        self.counts.get(&cell).copied().unwrap_or(0)
    }

    pub fn novelty(&self, cell: CellKey) -> f64 {
        1.0 / f64::sqrt(1.0 + f64::from(self.count(cell)))
    }

    pub fn reset_for_rebin(&mut self) {
        self.counts.clear();
    }

    pub fn len(&self) -> usize {
        self.counts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    pub fn sorted_counts(&self) -> Vec<(CellKey, u32)> {
        let mut counts: Vec<_> = self
            .counts
            .iter()
            .map(|(cell, count)| (*cell, *count))
            .collect();
        counts.sort_unstable_by_key(|(cell, _count)| *cell);
        counts
    }

    pub fn from_sorted_counts(counts: impl IntoIterator<Item = (CellKey, u32)>) -> Self {
        let mut mirror = Self::new();
        for (cell, count) in counts {
            if count != 0 {
                mirror.counts.insert(cell, count);
            }
        }
        mirror
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeenMap {
    map: SeenEntries,
}

impl SeenMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, state_hash: StateHash, node: NodeId) -> Option<NodeId> {
        if let Some(existing) = self.map.get(&state_hash).copied() {
            Some(existing)
        } else {
            self.map.insert(state_hash, node);
            None
        }
    }

    pub fn get(&self, state_hash: StateHash) -> Option<NodeId> {
        self.map.get(&state_hash).copied()
    }

    pub fn contains(&self, state_hash: StateHash) -> bool {
        self.map.contains_key(&state_hash)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn sorted_entries(&self) -> Vec<(StateHash, NodeId)> {
        let mut entries: Vec<_> = self.map.iter().map(|(hash, node)| (*hash, *node)).collect();
        entries.sort_unstable_by_key(|(hash, _node)| *hash);
        entries
    }

    pub fn from_sorted_entries(entries: impl IntoIterator<Item = (StateHash, NodeId)>) -> Self {
        let mut seen = Self::new();
        for (hash, node) in entries {
            seen.insert(hash, node);
        }
        seen
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorSnapshot {
    pub cell_counts: Vec<(CellKey, u32)>,
    pub seen: Vec<(StateHash, NodeId)>,
}

impl MirrorSnapshot {
    pub fn from_parts(cells: &CellMirror, seen: &SeenMap) -> Self {
        Self {
            cell_counts: cells.sorted_counts(),
            seen: seen.sorted_entries(),
        }
    }

    pub fn into_parts(self) -> (CellMirror, SeenMap) {
        (
            CellMirror::from_sorted_counts(self.cell_counts),
            SeenMap::from_sorted_entries(self.seen),
        )
    }

    pub fn reset_cell_counts_for_rebin(&mut self) {
        self.cell_counts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(seed: u8) -> StateHash {
        StateHash::new([seed; 32])
    }

    #[test]
    fn mirror_count_bumps_and_novelty_math_match_formula() {
        let mut mirror = CellMirror::new();
        let cell = CellKey::new(7);

        assert_eq!(mirror.count(cell), 0);
        assert_eq!(mirror.novelty(cell), 1.0);
        assert_eq!(mirror.bump(cell), 1);
        assert_eq!(mirror.bump_by(cell, 3), 4);
        assert_eq!(mirror.count(cell), 4);
        assert_eq!(mirror.novelty(cell), 1.0 / f64::sqrt(5.0));
    }

    #[test]
    fn mirror_sorted_counts_are_stable_for_serialization() {
        let mut mirror = CellMirror::new();
        mirror.bump_by(CellKey::new(30), 3);
        mirror.bump_by(CellKey::new(10), 1);
        mirror.bump_by(CellKey::new(20), 2);

        assert_eq!(
            mirror.sorted_counts(),
            vec![
                (CellKey::new(10), 1),
                (CellKey::new(20), 2),
                (CellKey::new(30), 3),
            ]
        );
        assert_eq!(
            CellMirror::from_sorted_counts(mirror.sorted_counts()).sorted_counts(),
            mirror.sorted_counts()
        );
    }

    #[test]
    fn seen_map_routes_duplicates_and_keeps_stable_entries() {
        let mut seen = SeenMap::new();
        let first = hash(1);
        let second = hash(2);

        assert_eq!(seen.get(first), None);
        assert_eq!(seen.insert(first, NodeId::new(11)), None);
        assert_eq!(seen.insert(second, NodeId::new(22)), None);
        assert_eq!(seen.get(first), Some(NodeId::new(11)));
        assert!(seen.contains(second));
        assert_eq!(seen.insert(first, NodeId::new(33)), Some(NodeId::new(11)));
        assert_eq!(seen.get(first), Some(NodeId::new(11)));
        assert_eq!(
            seen.sorted_entries(),
            vec![(first, NodeId::new(11)), (second, NodeId::new(22))]
        );
    }

    #[test]
    fn mirror_rebin_resets_only_cell_counts() {
        let mut cells = CellMirror::new();
        let mut seen = SeenMap::new();
        cells.bump_by(CellKey::new(4), 9);
        cells.bump(CellKey::new(8));
        seen.insert(hash(9), NodeId::new(99));

        cells.reset_for_rebin();

        assert!(cells.is_empty());
        assert_eq!(cells.novelty(CellKey::new(4)), 1.0);
        assert_eq!(seen.get(hash(9)), Some(NodeId::new(99)));
    }

    #[test]
    fn mirror_snapshot_sorts_and_preserves_seen_across_rebin() {
        let mut cells = CellMirror::new();
        let mut seen = SeenMap::new();
        cells.bump(CellKey::new(9));
        cells.bump_by(CellKey::new(3), 2);
        seen.insert(hash(7), NodeId::new(7));
        seen.insert(hash(1), NodeId::new(1));

        let mut snapshot = MirrorSnapshot::from_parts(&cells, &seen);

        assert_eq!(
            snapshot.cell_counts,
            vec![(CellKey::new(3), 2), (CellKey::new(9), 1)]
        );
        assert_eq!(
            snapshot.seen,
            vec![(hash(1), NodeId::new(1)), (hash(7), NodeId::new(7))]
        );

        snapshot.reset_cell_counts_for_rebin();
        let (rebinned_cells, persistent_seen) = snapshot.into_parts();

        assert!(rebinned_cells.is_empty());
        assert_eq!(persistent_seen.get(hash(1)), Some(NodeId::new(1)));
        assert_eq!(persistent_seen.get(hash(7)), Some(NodeId::new(7)));
    }

    #[test]
    fn mirror_snapshot_reconstruction_preserves_first_seen_route() {
        let snapshot = MirrorSnapshot {
            cell_counts: Vec::new(),
            seen: vec![
                (hash(5), NodeId::new(5)),
                (hash(5), NodeId::new(55)),
                (hash(9), NodeId::new(9)),
            ],
        };

        let (_cells, seen) = snapshot.into_parts();

        assert_eq!(seen.get(hash(5)), Some(NodeId::new(5)));
        assert_eq!(seen.get(hash(9)), Some(NodeId::new(9)));
        assert_eq!(
            seen.sorted_entries(),
            vec![(hash(5), NodeId::new(5)), (hash(9), NodeId::new(9))]
        );
    }
}
