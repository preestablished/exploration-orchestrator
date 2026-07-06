#![forbid(unsafe_code)]

//! Pure checkpoint and WAL encoding for the exploration orchestrator
//! (ARCHITECTURE.md §8). This crate owns `encode / decode / validate` only;
//! store I/O (the `PutMetadata` generation-CAS single-writer protocol on
//! `orch/ckpt/…` and `orch/wal/…` keys) belongs to `orch-server`.
//!
//! Serialization is postcard (MAP convention) with an explicit version
//! field; a pinned golden vector breaks loudly on accidental field
//! reorders.

use orch_core::types::{CellKey, NodeId, StateHash};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const CHECKPOINT_VERSION: u32 = 1;

/// Experiment lifecycle state (API.md §1 enum).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExperimentState {
    #[default]
    Pending,
    Running,
    Paused,
    Stopped,
    GoalReached,
    BudgetExhausted,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetsUsed {
    pub nodes: u64,
    pub wall_clock_s: u64,
    pub guest_instructions: u64,
    pub expansions: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RngCheckpoint {
    /// Substreams are derived from `(seed, purpose, batch_seq)`; no stream
    /// positions are stored (§8.1).
    pub seed: u64,
}

/// Selection weights for one frontier row the checkpoint covers. Frontier
/// membership truth stays in the store's FRONTIER rows (§8.2 step 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontierEntry {
    pub node: NodeId,
    pub visits: u32,
    pub exhausted: bool,
    pub consecutive_all_dup: u16,
}

/// Escalation-ladder knob set in effect at checkpoint time (mirrors
/// `orch_core::plateau::EscalationKnobs`, serializable).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlateauKnobsInEffect {
    pub window_n: u32,
    pub epsilon_s: f64,
    pub burst_len_factor: f64,
    pub temp_factor: f64,
    pub macro_weight_hot: f64,
    pub backtrack_kappa: f64,
    pub backtrack_depth_quantile: f64,
    pub radius_factor: f64,
    pub max_level: u32,
}

/// Plateau state machine snapshot. The implementation tracks stall
/// counters rather than the score ring the design sketch shows; the
/// counters are the complete resumable state of
/// `StallDetector`/`EscalationLadder` (disclosed reinterpretation).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlateauCheckpoint {
    pub best_score: Option<f64>,
    pub observations: u64,
    pub observations_since_improvement: u64,
    pub completed_stall_windows: u64,
    /// Current escalation level, 0..=4.
    pub level: u32,
    pub knobs_in_effect: PlateauKnobsInEffect,
}

/// The orchestrator checkpoint (ARCHITECTURE.md §8, field-for-field).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckpointV1 {
    pub version: u32,
    pub experiment_id: String,
    /// blake3 of the canonical effective ExperimentConfig; resume refuses
    /// on mismatch.
    pub config_hash: [u8; 32],
    /// Diagnostic only — resume NEVER derives the id counter from it.
    pub last_committed_node: NodeId,
    pub batch_seq: u64,
    pub expansions: u64,
    pub budgets_used: BudgetsUsed,
    pub rng: RngCheckpoint,
    pub frontier: Vec<FrontierEntry>,
    /// Scorer-archive lockstep: written only after `CheckpointArchive`
    /// returned and the seq assertion passed.
    pub scorer_archive_ref: String,
    pub scorer_archive_seq: u64,
    pub scorer_checkpoint_id: String,
    pub feature_map_version: u32,
    pub feature_map_hash: [u8; 32],
    pub scoring_program_hash: [u8; 32],
    pub synth_config_fingerprint: [u8; 32],
    /// Selection-side cell counts, sorted by cell key.
    pub cell_mirror: Vec<(CellKey, u32)>,
    /// SeenMap entries, sorted by state hash.
    pub seen: Vec<(StateHash, NodeId)>,
    pub plateau: PlateauCheckpoint,
    pub goal_nodes: Vec<NodeId>,
    pub status: ExperimentState,
}

/// WAL entry written before dispatching batch `seq`; deleted after the
/// batch commits. Replayed identically in fast and deterministic mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExpansionIntent {
    pub seq: u64,
    pub node: NodeId,
    /// The synth request seed for this batch (pins burst generation).
    pub burst_seed: u64,
    /// Ladder knobs pinned at dispatch time.
    pub knobs: IntentKnobs,
    pub client_batch_id: String,
}

/// The dispatch-time knobs an intent pins (burst length + policy scaling
/// plus the config-overrides payload L3 injects).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IntentKnobs {
    pub k: u32,
    pub length_hint_frames: u32,
    pub escalation_level: u32,
    pub config_overrides_yaml: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckpointError {
    Encode(String),
    Decode(String),
    /// version / config_hash / experiment_id verification failed.
    Mismatch(String),
}

impl fmt::Display for CheckpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(detail) => write!(formatter, "checkpoint encode failed: {detail}"),
            Self::Decode(detail) => write!(formatter, "checkpoint decode failed: {detail}"),
            Self::Mismatch(detail) => write!(formatter, "checkpoint mismatch: {detail}"),
        }
    }
}

impl std::error::Error for CheckpointError {}

pub fn encode_checkpoint(checkpoint: &CheckpointV1) -> Result<Vec<u8>, CheckpointError> {
    if checkpoint.version != CHECKPOINT_VERSION {
        return Err(CheckpointError::Encode(format!(
            "unsupported checkpoint version {}",
            checkpoint.version
        )));
    }
    validate_sorted(checkpoint).map_err(CheckpointError::Encode)?;
    postcard::to_allocvec(checkpoint).map_err(|error| CheckpointError::Encode(error.to_string()))
}

/// Decodes and verifies version + experiment id + config hash.
pub fn decode_checkpoint(
    bytes: &[u8],
    expected_experiment_id: &str,
    expected_config_hash: &[u8; 32],
) -> Result<CheckpointV1, CheckpointError> {
    let checkpoint: CheckpointV1 =
        postcard::from_bytes(bytes).map_err(|error| CheckpointError::Decode(error.to_string()))?;
    if checkpoint.version != CHECKPOINT_VERSION {
        return Err(CheckpointError::Mismatch(format!(
            "checkpoint version {} != supported {CHECKPOINT_VERSION}",
            checkpoint.version
        )));
    }
    if checkpoint.experiment_id != expected_experiment_id {
        return Err(CheckpointError::Mismatch(format!(
            "checkpoint experiment '{}' != requested '{expected_experiment_id}'",
            checkpoint.experiment_id
        )));
    }
    if checkpoint.config_hash != *expected_config_hash {
        return Err(CheckpointError::Mismatch(
            "checkpoint config_hash does not match the supplied config".to_owned(),
        ));
    }
    validate_sorted(&checkpoint).map_err(CheckpointError::Decode)?;
    Ok(checkpoint)
}

pub fn encode_intent(intent: &ExpansionIntent) -> Result<Vec<u8>, CheckpointError> {
    postcard::to_allocvec(intent).map_err(|error| CheckpointError::Encode(error.to_string()))
}

pub fn decode_intent(bytes: &[u8]) -> Result<ExpansionIntent, CheckpointError> {
    postcard::from_bytes(bytes).map_err(|error| CheckpointError::Decode(error.to_string()))
}

fn validate_sorted(checkpoint: &CheckpointV1) -> Result<(), String> {
    if !checkpoint
        .cell_mirror
        .windows(2)
        .all(|pair| pair[0].0 < pair[1].0)
    {
        return Err("cell_mirror must be strictly sorted by cell key".to_owned());
    }
    if !checkpoint
        .seen
        .windows(2)
        .all(|pair| pair[0].0.as_bytes() < pair[1].0.as_bytes())
    {
        return Err("seen must be strictly sorted by state hash".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample_checkpoint() -> CheckpointV1 {
        CheckpointV1 {
            version: CHECKPOINT_VERSION,
            experiment_id: "exp-a".to_owned(),
            config_hash: [0x11; 32],
            last_committed_node: NodeId::new(41),
            batch_seq: 17,
            expansions: 17,
            budgets_used: BudgetsUsed {
                nodes: 42,
                wall_clock_s: 9,
                guest_instructions: 123_456,
                expansions: 17,
            },
            rng: RngCheckpoint { seed: 0x5EED },
            frontier: vec![
                FrontierEntry {
                    node: NodeId::new(3),
                    visits: 2,
                    exhausted: false,
                    consecutive_all_dup: 0,
                },
                FrontierEntry {
                    node: NodeId::new(9),
                    visits: 0,
                    exhausted: true,
                    consecutive_all_dup: 4,
                },
            ],
            scorer_archive_ref: "scar:blake3:00ff".to_owned(),
            scorer_archive_seq: 17,
            scorer_checkpoint_id: "ckpt-17".to_owned(),
            feature_map_version: 1,
            feature_map_hash: [0x22; 32],
            scoring_program_hash: [0x33; 32],
            synth_config_fingerprint: [0x44; 32],
            cell_mirror: vec![(CellKey::new(1), 3), (CellKey::new(7), 1)],
            seen: vec![
                (StateHash::new([0x01; 32]), NodeId::new(1)),
                (StateHash::new([0x02; 32]), NodeId::new(2)),
            ],
            plateau: PlateauCheckpoint {
                best_score: Some(12.5),
                observations: 100,
                observations_since_improvement: 10,
                completed_stall_windows: 0,
                level: 1,
                knobs_in_effect: PlateauKnobsInEffect {
                    window_n: 200,
                    epsilon_s: 0.001,
                    burst_len_factor: 1.5,
                    temp_factor: 1.75,
                    macro_weight_hot: 0.5,
                    backtrack_kappa: 1.0,
                    backtrack_depth_quantile: 0.5,
                    radius_factor: 2.0,
                    max_level: 4,
                },
            },
            goal_nodes: vec![NodeId::new(40)],
            status: ExperimentState::Running,
        }
    }

    /// Pinned golden vector: digest of the canonical encoding of
    /// `sample_checkpoint()`. Breaks loudly on field reorders or type
    /// changes — bump CHECKPOINT_VERSION instead of editing this.
    const GOLDEN_CHECKPOINT_DIGEST: &str = "12b7cb69a38cc09024954a3cb42b4aaf";

    #[test]
    fn checkpoint_round_trips_and_matches_the_golden_vector() {
        let checkpoint = sample_checkpoint();
        let bytes = encode_checkpoint(&checkpoint).expect("encode");
        let decoded =
            decode_checkpoint(&bytes, "exp-a", &[0x11; 32]).expect("decode with matching hash");

        assert_eq!(decoded, checkpoint);
        let digest = blake3_hex(&bytes);
        assert_eq!(
            digest, GOLDEN_CHECKPOINT_DIGEST,
            "checkpoint encoding changed; if intentional, bump CHECKPOINT_VERSION"
        );
    }

    fn blake3_hex(bytes: &[u8]) -> String {
        // FNV-1a-style fold over the canonical bytes: stable across
        // platforms without pulling a hash dependency into this pure crate.
        const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
        const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
        let mut hash = OFFSET;
        for &byte in bytes {
            hash ^= u128::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
        format!("{hash:032x}")
    }

    #[test]
    fn decode_refuses_wrong_experiment_config_hash_and_version() {
        let checkpoint = sample_checkpoint();
        let bytes = encode_checkpoint(&checkpoint).expect("encode");

        assert!(matches!(
            decode_checkpoint(&bytes, "exp-b", &[0x11; 32]),
            Err(CheckpointError::Mismatch(_))
        ));
        assert!(matches!(
            decode_checkpoint(&bytes, "exp-a", &[0x99; 32]),
            Err(CheckpointError::Mismatch(_))
        ));

        let mut wrong_version = checkpoint;
        wrong_version.version = 2;
        assert!(matches!(
            encode_checkpoint(&wrong_version),
            Err(CheckpointError::Encode(_))
        ));
    }

    #[test]
    fn encode_refuses_unsorted_mirrors_and_seen() {
        let mut unsorted_cells = sample_checkpoint();
        unsorted_cells.cell_mirror.reverse();
        assert!(matches!(
            encode_checkpoint(&unsorted_cells),
            Err(CheckpointError::Encode(_))
        ));

        let mut unsorted_seen = sample_checkpoint();
        unsorted_seen.seen.reverse();
        assert!(matches!(
            encode_checkpoint(&unsorted_seen),
            Err(CheckpointError::Encode(_))
        ));
    }

    #[test]
    fn intent_round_trips() {
        let intent = ExpansionIntent {
            seq: 18,
            node: NodeId::new(9),
            burst_seed: 0xABCD,
            knobs: IntentKnobs {
                k: 16,
                length_hint_frames: 120,
                escalation_level: 2,
                config_overrides_yaml: b"generator_mix:\n  macro: 0.5\n".to_vec(),
            },
            client_batch_id: "b18".to_owned(),
        };

        let bytes = encode_intent(&intent).expect("encode");
        assert_eq!(decode_intent(&bytes).expect("decode"), intent);
    }

    proptest! {
        #[test]
        fn checkpoint_round_trips_for_arbitrary_counters(
            batch_seq in 0u64..u64::MAX,
            expansions in 0u64..u64::MAX,
            seed in proptest::num::u64::ANY,
            level in 0u32..5,
            visits in 0u32..u32::MAX,
        ) {
            let mut checkpoint = sample_checkpoint();
            checkpoint.batch_seq = batch_seq;
            checkpoint.expansions = expansions;
            checkpoint.rng.seed = seed;
            checkpoint.plateau.level = level;
            checkpoint.frontier[0].visits = visits;

            let bytes = encode_checkpoint(&checkpoint).expect("encode");
            let decoded = decode_checkpoint(&bytes, "exp-a", &[0x11; 32]).expect("decode");
            prop_assert_eq!(decoded, checkpoint);
        }

        #[test]
        fn intent_round_trips_for_arbitrary_payloads(
            seq in proptest::num::u64::ANY,
            node in 0u64..u64::MAX,
            burst_seed in proptest::num::u64::ANY,
            overrides in proptest::collection::vec(proptest::num::u8::ANY, 0..256),
        ) {
            let intent = ExpansionIntent {
                seq,
                node: NodeId::new(node),
                burst_seed,
                knobs: IntentKnobs {
                    k: 4,
                    length_hint_frames: 60,
                    escalation_level: 0,
                    config_overrides_yaml: overrides,
                },
                client_batch_id: format!("b{seq}"),
            };
            let bytes = encode_intent(&intent).expect("encode");
            prop_assert_eq!(decode_intent(&bytes).expect("decode"), intent);
        }
    }
}
