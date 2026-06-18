//! Synthetic grid-world state model used by fake hypervisor and scorer tests.

use orch_core::types::{CellKey, StateHash};
use serde::{Deserialize, Serialize};

pub const GRID_WIDTH: u8 = 5;
pub const GRID_HEIGHT: u8 = 5;
pub const BOSS_MAX_HP: u8 = 3;
pub const KEY_BOSS_DOOR: u8 = 0b0000_0001;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum Room {
    #[default]
    Start,
    KeyVault,
    Boss,
}

impl Room {
    pub const fn id(self) -> u8 {
        match self {
            Self::Start => 0,
            Self::KeyVault => 1,
            Self::Boss => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GridAction {
    #[default]
    Wait,
    Up,
    Down,
    Left,
    Right,
    Attack,
}

impl GridAction {
    pub const fn tag(self) -> u8 {
        match self {
            Self::Wait => 0,
            Self::Up => 1,
            Self::Down => 2,
            Self::Left => 3,
            Self::Right => 4,
            Self::Attack => 5,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StepOutcome {
    #[default]
    Moved,
    BlockedByWall,
    BlockedByDoor,
    PickedKey,
    HitBoss,
    BossDefeated,
    GoalReached,
    Noop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GridState {
    pub x: u8,
    pub y: u8,
    pub room: Room,
    pub keys: u8,
    pub boss_hp: u8,
}

impl Default for GridState {
    fn default() -> Self {
        Self::new()
    }
}

impl GridState {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            x: 0,
            y: 2,
            room: Room::Start,
            keys: 0,
            boss_hp: BOSS_MAX_HP,
        }
    }

    #[must_use]
    pub const fn has_key(self) -> bool {
        self.keys & KEY_BOSS_DOOR != 0
    }

    #[must_use]
    pub const fn goal_reached(self) -> bool {
        matches!(self.room, Room::Boss) && self.boss_hp == 0 && self.x == 4 && self.y == 0
    }

    #[must_use]
    pub fn state_hash(self) -> StateHash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"orch-fakes/grid-state/v1");
        hasher.update(&[self.room.id(), self.x, self.y, self.keys, self.boss_hp]);
        StateHash::new(*hasher.finalize().as_bytes())
    }

    #[must_use]
    pub const fn cell_key(self) -> CellKey {
        let room = self.room.id() as u64;
        let x = self.x as u64;
        let y = self.y as u64;
        CellKey::new((room << 32) | (x << 16) | y)
    }

    #[must_use]
    pub fn step(mut self, action: GridAction) -> (Self, StepOutcome) {
        let outcome = match action {
            GridAction::Wait => StepOutcome::Noop,
            GridAction::Attack => self.attack(),
            GridAction::Up | GridAction::Down | GridAction::Left | GridAction::Right => {
                self.move_dir(action)
            }
        };

        if self.room == Room::KeyVault && self.x == 2 && self.y == 2 && !self.has_key() {
            self.keys |= KEY_BOSS_DOOR;
            return (self, StepOutcome::PickedKey);
        }
        if self.goal_reached() {
            return (self, StepOutcome::GoalReached);
        }

        (self, outcome)
    }

    #[must_use]
    pub fn apply_actions(mut self, actions: &[GridAction]) -> Self {
        for action in actions {
            self = self.step(*action).0;
        }
        self
    }

    fn attack(&mut self) -> StepOutcome {
        if self.room != Room::Boss || self.x != 2 || self.y != 2 || self.boss_hp == 0 {
            return StepOutcome::Noop;
        }

        self.boss_hp -= 1;
        if self.boss_hp == 0 {
            StepOutcome::BossDefeated
        } else {
            StepOutcome::HitBoss
        }
    }

    fn move_dir(&mut self, action: GridAction) -> StepOutcome {
        if let Some(outcome) = self.try_door(action) {
            return outcome;
        }

        let (dx, dy) = match action {
            GridAction::Up => (0i8, -1i8),
            GridAction::Down => (0, 1),
            GridAction::Left => (-1, 0),
            GridAction::Right => (1, 0),
            GridAction::Wait | GridAction::Attack => (0, 0),
        };
        let Some(next_x) = add_delta(self.x, dx) else {
            return StepOutcome::BlockedByWall;
        };
        let Some(next_y) = add_delta(self.y, dy) else {
            return StepOutcome::BlockedByWall;
        };
        if next_x >= GRID_WIDTH || next_y >= GRID_HEIGHT || is_wall(self.room, next_x, next_y) {
            return StepOutcome::BlockedByWall;
        }

        self.x = next_x;
        self.y = next_y;
        StepOutcome::Moved
    }

    fn try_door(&mut self, action: GridAction) -> Option<StepOutcome> {
        match (self.room, self.x, self.y, action) {
            (Room::Start, 4, 2, GridAction::Right) => {
                self.room = Room::KeyVault;
                self.x = 0;
                self.y = 2;
                Some(StepOutcome::Moved)
            }
            (Room::KeyVault, 0, 2, GridAction::Left) => {
                self.room = Room::Start;
                self.x = 4;
                self.y = 2;
                Some(StepOutcome::Moved)
            }
            (Room::Start, 2, 0, GridAction::Up) if self.has_key() => {
                self.room = Room::Boss;
                self.x = 2;
                self.y = 4;
                Some(StepOutcome::Moved)
            }
            (Room::Start, 2, 0, GridAction::Up) => Some(StepOutcome::BlockedByDoor),
            _ => None,
        }
    }
}

fn add_delta(value: u8, delta: i8) -> Option<u8> {
    if delta.is_negative() {
        value.checked_sub(delta.unsigned_abs())
    } else {
        value.checked_add(delta as u8)
    }
}

fn is_wall(room: Room, x: u8, y: u8) -> bool {
    match room {
        Room::Start => x == 1 && y != 2,
        Room::KeyVault => x == 3 && y == 1,
        Room::Boss => (x == 1 || x == 3) && y == 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_legal_moves_walls_and_doors_are_deterministic() {
        let start = GridState::new();
        let (wall, wall_outcome) = start.step(GridAction::Left);
        let internal_wall_start = GridState {
            x: 0,
            y: 0,
            ..GridState::new()
        };
        let (internal_wall, internal_wall_outcome) = internal_wall_start.step(GridAction::Right);
        let at_start_door = start.apply_actions(&[
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
        ]);
        let (key_room, door_outcome) = at_start_door.step(GridAction::Right);
        let at_boss_door = GridState {
            x: 2,
            y: 0,
            room: Room::Start,
            keys: 0,
            boss_hp: BOSS_MAX_HP,
        };
        let (blocked, blocked_outcome) = at_boss_door.step(GridAction::Up);

        assert_eq!(wall, start);
        assert_eq!(wall_outcome, StepOutcome::BlockedByWall);
        assert_eq!(internal_wall, internal_wall_start);
        assert_eq!(internal_wall_outcome, StepOutcome::BlockedByWall);
        assert_eq!(key_room.room, Room::KeyVault);
        assert_eq!((key_room.x, key_room.y), (0, 2));
        assert_eq!(door_outcome, StepOutcome::Moved);
        assert_eq!(blocked, at_boss_door);
        assert_eq!(blocked_outcome, StepOutcome::BlockedByDoor);
    }

    #[test]
    fn grid_keys_unlock_boss_room() {
        let with_key = GridState::new().apply_actions(&[
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
        ]);
        let at_boss_door = GridState {
            x: 2,
            y: 0,
            room: Room::Start,
            keys: with_key.keys,
            boss_hp: BOSS_MAX_HP,
        };
        let (boss_entry, outcome) = at_boss_door.step(GridAction::Up);

        assert!(with_key.has_key());
        assert_eq!(with_key.room, Room::KeyVault);
        assert_eq!((with_key.x, with_key.y), (2, 2));
        assert_eq!(outcome, StepOutcome::Moved);
        assert_eq!(boss_entry.room, Room::Boss);
        assert_eq!((boss_entry.x, boss_entry.y), (2, 4));
    }

    #[test]
    fn grid_boss_sequence_and_goal_cell() {
        let mut state = GridState {
            x: 2,
            y: 2,
            room: Room::Boss,
            keys: KEY_BOSS_DOOR,
            boss_hp: BOSS_MAX_HP,
        };

        let (after_hit, hit) = state.step(GridAction::Attack);
        state = after_hit;
        let (after_second, second) = state.step(GridAction::Attack);
        state = after_second;
        let (after_defeat, defeat) = state.step(GridAction::Attack);
        let goal = after_defeat.apply_actions(&[
            GridAction::Right,
            GridAction::Right,
            GridAction::Up,
            GridAction::Up,
        ]);

        assert_eq!(hit, StepOutcome::HitBoss);
        assert_eq!(second, StepOutcome::HitBoss);
        assert_eq!(defeat, StepOutcome::BossDefeated);
        assert_eq!(after_defeat.boss_hp, 0);
        assert!(goal.goal_reached());
    }

    #[test]
    fn grid_state_hash_and_cell_key_are_stable_and_room_sensitive() {
        let a = GridState::new();
        let same = GridState::new();
        let other_room_same_xy = GridState {
            room: Room::KeyVault,
            ..a
        };

        assert_eq!(a.state_hash(), same.state_hash());
        assert_eq!(a.cell_key(), same.cell_key());
        assert_ne!(a.state_hash(), other_room_same_xy.state_hash());
        assert_ne!(a.cell_key(), other_room_same_xy.cell_key());
    }
}
