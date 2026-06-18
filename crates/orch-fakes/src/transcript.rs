//! Transcript hashing helpers for deterministic fake-service tests.

use orch_core::types::DIGEST_LEN;
use serde::{Deserialize, Serialize};

use crate::grid::{GridAction, GridState, StepOutcome};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TranscriptHash(pub [u8; DIGEST_LEN]);

impl TranscriptHash {
    #[must_use]
    pub const fn new(bytes: [u8; DIGEST_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; DIGEST_LEN] {
        &self.0
    }

    #[must_use]
    pub const fn into_bytes(self) -> [u8; DIGEST_LEN] {
        self.0
    }
}

#[must_use]
pub fn hash_transcript(seed: u64, bytes: &[u8]) -> TranscriptHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"orch-fakes/transcript/v1");
    hasher.update(&seed.to_le_bytes());
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    TranscriptHash::new(*hasher.finalize().as_bytes())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TranscriptBuilder {
    seed: u64,
    bytes: Vec<u8>,
}

impl TranscriptBuilder {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            bytes: Vec::new(),
        }
    }

    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn append_action(&mut self, action: GridAction) {
        self.bytes.push(0xA0);
        self.bytes.push(action.tag());
    }

    pub fn append_state(&mut self, state: GridState) {
        self.bytes.push(0xB0);
        self.bytes.push(state.room.id());
        self.bytes.push(state.x);
        self.bytes.push(state.y);
        self.bytes.push(state.keys);
        self.bytes.push(state.boss_hp);
        self.bytes.extend_from_slice(state.state_hash().as_bytes());
    }

    pub fn append_step(
        &mut self,
        before: GridState,
        action: GridAction,
        after: GridState,
        outcome: StepOutcome,
    ) {
        self.bytes.push(0xC0);
        self.append_state(before);
        self.append_action(action);
        self.append_state(after);
        self.bytes.push(outcome_tag(outcome));
    }

    #[must_use]
    pub fn finish(&self) -> TranscriptHash {
        hash_transcript(self.seed, &self.bytes)
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

const fn outcome_tag(outcome: StepOutcome) -> u8 {
    match outcome {
        StepOutcome::Moved => 0,
        StepOutcome::BlockedByWall => 1,
        StepOutcome::BlockedByDoor => 2,
        StepOutcome::PickedKey => 3,
        StepOutcome::HitBoss => 4,
        StepOutcome::BossDefeated => 5,
        StepOutcome::GoalReached => 6,
        StepOutcome::Noop => 7,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_same_bytes_same_hash() {
        let bytes = b"fixed transcript bytes";

        assert_eq!(hash_transcript(7, bytes), hash_transcript(7, bytes));
    }

    #[test]
    fn transcript_changed_seed_or_action_path_changes_hash() {
        let baseline = transcript_for_actions(
            99,
            &[
                GridAction::Right,
                GridAction::Right,
                GridAction::Right,
                GridAction::Right,
            ],
        );
        let changed_seed = transcript_for_actions(
            100,
            &[
                GridAction::Right,
                GridAction::Right,
                GridAction::Right,
                GridAction::Right,
            ],
        );
        let changed_path = transcript_for_actions(
            99,
            &[
                GridAction::Right,
                GridAction::Right,
                GridAction::Wait,
                GridAction::Right,
            ],
        );

        assert_ne!(baseline, changed_seed);
        assert_ne!(baseline, changed_path);
    }

    #[test]
    fn transcript_builder_hashes_grid_steps_deterministically() {
        let actions = [
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
        ];

        assert_eq!(
            transcript_for_actions(1234, &actions),
            transcript_for_actions(1234, &actions)
        );
    }

    fn transcript_for_actions(seed: u64, actions: &[GridAction]) -> TranscriptHash {
        let mut builder = TranscriptBuilder::new(seed);
        let mut state = GridState::new();
        builder.append_state(state);
        for action in actions {
            let before = state;
            let (after, outcome) = state.step(*action);
            builder.append_step(before, *action, after, outcome);
            state = after;
        }
        builder.finish()
    }
}
