//! The shared end-state comparator (plan W2.4): extracted from Tier-1's
//! `chaos_resume.rs` so both tiers assert the same bit-identical bar, plus
//! the Tier-2 scorer-archive fingerprint (the bar says tree **and** archive
//! state).

use orch_clients::snapshot_store::{OrderBy, QueryNodesRequest, SnapshotStoreClient};
use orch_driver::node_attrs::decode_node_attrs;
use orch_fakes::{scorer::FakeScorer, snapshot_store::InMemorySnapshotStore};

fn all_nodes(
    store: &InMemorySnapshotStore,
    experiment_id: &str,
) -> Vec<orch_clients::snapshot_store::NodeMeta> {
    store
        .query_nodes(QueryNodesRequest {
            experiment_id: experiment_id.to_owned(),
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
        .expect("query nodes")
        .nodes
}

/// Blake3 over the sorted committed tree (node id, parent, synth state
/// hash, progress score, cell key), asserting dense ids on the way — the
/// Tier-1 comparator, parameterized by experiment id.
#[must_use]
pub fn store_tree_hash(store: &InMemorySnapshotStore, experiment_id: &str) -> [u8; 32] {
    let mut nodes = all_nodes(store, experiment_id);
    nodes.sort_by_key(|node| node.node_id);

    let mut hasher = blake3::Hasher::new();
    let mut previous_id: Option<u64> = None;
    for node in &nodes {
        // Zero id reuse / dense ids: strictly increasing by exactly one.
        if let Some(previous) = previous_id {
            assert_eq!(
                node.node_id.get(),
                previous + 1,
                "node ids must stay dense (no reuse, no gaps)"
            );
        }
        previous_id = Some(node.node_id.get());
        let attrs = decode_node_attrs(&node.attrs).expect("node attrs decode");
        hasher.update(&node.node_id.get().to_le_bytes());
        hasher.update(
            &node
                .parent_node_id
                .map_or(u64::MAX, |parent| parent.get())
                .to_le_bytes(),
        );
        hasher.update(attrs.synth.state_hash.as_bytes());
        hasher.update(&node.progress_score.get().to_le_bytes());
        hasher.update(&attrs.synth.cell_key.get().to_le_bytes());
    }
    *hasher.finalize().as_bytes()
}

/// Every FRONTIER row must be adoptable: parent chain intact and attrs
/// decodable (what §8.2 step 4 needs). Nothing may reference a missing
/// node.
pub fn assert_no_stranded_frontier(store: &InMemorySnapshotStore, experiment_id: &str) {
    let nodes = all_nodes(store, experiment_id);
    let ids: std::collections::BTreeSet<_> = nodes.iter().map(|node| node.node_id).collect();
    for node in &nodes {
        if let Some(parent) = node.parent_node_id {
            assert!(
                ids.contains(&parent),
                "node {} references missing parent {}",
                node.node_id.get(),
                parent.get()
            );
        }
        decode_node_attrs(&node.attrs).expect("attrs decode");
    }
}

/// Blake3 over the scorer's committed archive state for one experiment:
/// archive_seq, feature-map/program hashes, and the sorted cell counts.
/// Batch caches and fault state are deliberately excluded (idempotency
/// caches, not committed search state). An absent archive hashes a fixed
/// empty marker.
#[must_use]
pub fn scorer_archive_fingerprint(scorer: &FakeScorer, experiment_id: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"orch-simstate/scorer-archive-fingerprint/v1");
    match scorer.archive_parts(experiment_id) {
        None => {
            hasher.update(b"absent");
        }
        Some((archive_seq, cell_counts, feature_map_hash, program_hash)) => {
            hasher.update(&archive_seq.to_le_bytes());
            match feature_map_hash {
                Some(digest) => hasher.update(digest.as_bytes()),
                None => hasher.update(b"no-feature-map"),
            };
            match program_hash {
                Some(digest) => hasher.update(digest.as_bytes()),
                None => hasher.update(b"no-program"),
            };
            for (cell_key, count) in cell_counts {
                hasher.update(&cell_key.get().to_le_bytes());
                hasher.update(&count.to_le_bytes());
            }
        }
    }
    *hasher.finalize().as_bytes()
}
