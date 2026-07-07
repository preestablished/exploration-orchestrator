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

/// Position in a world: room + cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridPos {
    pub room: Room,
    pub x: u8,
    pub y: u8,
}

impl GridPos {
    #[must_use]
    pub const fn new(room: Room, x: u8, y: u8) -> Self {
        Self { room, x, y }
    }

    #[must_use]
    pub const fn matches(self, state: GridState) -> bool {
        state.room as u8 == self.room as u8 && state.x == self.x && state.y == self.y
    }
}

/// A door edge: standing at `at` and moving `action` crosses to `to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridDoor {
    pub at: GridPos,
    pub action: GridAction,
    pub to: GridPos,
    /// Locked doors block (`BlockedByDoor`) until the key bit is held.
    pub requires_key: bool,
}

/// Data-driven deterministic grid world: room graph, wall/door/key
/// placement, boss and goal (credits) cells, plus the score plan the fake
/// scorer derives progress from. The default fixture reproduces the
/// original hardcoded three-room world bit-for-bit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridWorld {
    pub name: String,
    pub start: GridPos,
    /// Cells that block movement, per room.
    pub walls: Vec<GridPos>,
    pub doors: Vec<GridDoor>,
    /// Standing on this cell picks up the boss-door key.
    pub key: Option<GridPos>,
    /// Boss cell; attacking here decrements hp from `BOSS_MAX_HP`.
    pub boss: Option<GridPos>,
    /// Goal (credits) cell; requires the boss defeated when one exists.
    pub goal: GridPos,
    /// Scoring the fake scorer applies: per-room base score.
    pub room_base_score: [f64; 3],
    /// Per-room score weight along +x and -y (progress direction pull).
    pub room_x_weight: [f64; 3],
    pub room_y_weight: [f64; 3],
    /// Cell the fake scorer marks prune-on-sight, if any.
    pub prune_cell: Option<GridPos>,
}

impl GridWorld {
    /// The original hardcoded three-room world (start -> key vault ->
    /// boss + credits): the boss+credits autonomy fixture and the default
    /// everywhere.
    #[must_use]
    pub fn three_room() -> Self {
        let mut walls = Vec::new();
        for y in 0..GRID_HEIGHT {
            if y != 2 {
                walls.push(GridPos::new(Room::Start, 1, y));
            }
        }
        walls.push(GridPos::new(Room::KeyVault, 3, 1));
        for x in [1, 3] {
            walls.push(GridPos::new(Room::Boss, x, 3));
        }
        Self {
            name: "three-room".to_owned(),
            start: GridPos::new(Room::Start, 0, 2),
            walls,
            doors: vec![
                GridDoor {
                    at: GridPos::new(Room::Start, 4, 2),
                    action: GridAction::Right,
                    to: GridPos::new(Room::KeyVault, 0, 2),
                    requires_key: false,
                },
                GridDoor {
                    at: GridPos::new(Room::KeyVault, 0, 2),
                    action: GridAction::Left,
                    to: GridPos::new(Room::Start, 4, 2),
                    requires_key: false,
                },
                GridDoor {
                    at: GridPos::new(Room::Start, 2, 0),
                    action: GridAction::Up,
                    to: GridPos::new(Room::Boss, 2, 4),
                    requires_key: true,
                },
            ],
            key: Some(GridPos::new(Room::KeyVault, 2, 2)),
            boss: Some(GridPos::new(Room::Boss, 2, 2)),
            goal: GridPos::new(Room::Boss, 4, 0),
            room_base_score: [0.0, 100.0, 200.0],
            room_x_weight: [10.0, 10.0, 10.0],
            room_y_weight: [1.0, 1.0, 1.0],
            prune_cell: Some(GridPos::new(Room::Start, 0, 0)),
        }
    }

    /// Plateau-ladder fixture: a rightward corridor whose score gradient
    /// pulls toward a locked stage gate, with the key hidden at the end of
    /// a zero-gradient detour reached by climbing at the corridor's start.
    /// Short greedy bursts saturate at the gate; unsticking needs longer
    /// bursts / hotter selection / backtracking. (Solvable with Up, Right,
    /// and Left only — the fake synthesizer's pad alphabet has no Down.)
    #[must_use]
    pub fn corridor_hidden_key() -> Self {
        let mut walls = Vec::new();
        // Start room: a corridor along y=2 walled below, and walled above
        // except a single climb shaft at x=0.
        for x in 0..GRID_WIDTH {
            walls.push(GridPos::new(Room::Start, x, 3));
            walls.push(GridPos::new(Room::Start, x, 4));
            if x != 0 {
                walls.push(GridPos::new(Room::Start, x, 1));
                walls.push(GridPos::new(Room::Start, x, 0));
            }
        }
        Self {
            name: "corridor-hidden-key".to_owned(),
            start: GridPos::new(Room::Start, 0, 2),
            walls,
            doors: vec![
                GridDoor {
                    // Top of the climb shaft: into the key annex.
                    at: GridPos::new(Room::Start, 0, 0),
                    action: GridAction::Up,
                    to: GridPos::new(Room::KeyVault, 0, 0),
                    requires_key: false,
                },
                GridDoor {
                    // The key cell doubles as the way back to the corridor.
                    at: GridPos::new(Room::KeyVault, 4, 0),
                    action: GridAction::Up,
                    to: GridPos::new(Room::Start, 0, 2),
                    requires_key: false,
                },
                GridDoor {
                    // The stage gate at the far end of the corridor.
                    at: GridPos::new(Room::Start, 4, 2),
                    action: GridAction::Right,
                    to: GridPos::new(Room::Boss, 0, 2),
                    requires_key: true,
                },
            ],
            key: Some(GridPos::new(Room::KeyVault, 4, 0)),
            boss: None,
            goal: GridPos::new(Room::Boss, 4, 2),
            room_base_score: [0.0, 0.0, 200.0],
            // The key annex is score-flat: crossing it never pays until the
            // key itself, so only novelty/backtracking (the ladder) sustains
            // the detour.
            room_x_weight: [10.0, 0.0, 10.0],
            room_y_weight: [0.0, 0.0, 0.0],
            prune_cell: None,
        }
    }

    #[must_use]
    pub fn initial_state(&self) -> GridState {
        GridState {
            x: self.start.x,
            y: self.start.y,
            room: self.start.room,
            keys: 0,
            boss_hp: self.boss.map_or(0, |_| BOSS_MAX_HP),
        }
    }

    #[must_use]
    pub fn goal_reached(&self, state: GridState) -> bool {
        self.goal.matches(state) && state.boss_hp == 0
    }

    fn is_wall(&self, room: Room, x: u8, y: u8) -> bool {
        self.walls
            .iter()
            .any(|wall| wall.room == room && wall.x == x && wall.y == y)
    }

    #[must_use]
    pub fn step(&self, mut state: GridState, action: GridAction) -> (GridState, StepOutcome) {
        let outcome = match action {
            GridAction::Wait => StepOutcome::Noop,
            GridAction::Attack => self.attack(&mut state),
            GridAction::Up | GridAction::Down | GridAction::Left | GridAction::Right => {
                self.move_dir(&mut state, action)
            }
        };

        if let Some(key) = self.key {
            if key.matches(state) && !state.has_key() {
                state.keys |= KEY_BOSS_DOOR;
                return (state, StepOutcome::PickedKey);
            }
        }
        if self.goal_reached(state) {
            return (state, StepOutcome::GoalReached);
        }

        (state, outcome)
    }

    #[must_use]
    pub fn apply_actions(&self, mut state: GridState, actions: &[GridAction]) -> GridState {
        for action in actions {
            state = self.step(state, *action).0;
        }
        state
    }

    fn attack(&self, state: &mut GridState) -> StepOutcome {
        let Some(boss) = self.boss else {
            return StepOutcome::Noop;
        };
        if !boss.matches(*state) || state.boss_hp == 0 {
            return StepOutcome::Noop;
        }
        state.boss_hp -= 1;
        if state.boss_hp == 0 {
            StepOutcome::BossDefeated
        } else {
            StepOutcome::HitBoss
        }
    }

    fn move_dir(&self, state: &mut GridState, action: GridAction) -> StepOutcome {
        for door in &self.doors {
            if door.at.matches(*state) && door.action == action {
                if door.requires_key && !state.has_key() {
                    return StepOutcome::BlockedByDoor;
                }
                state.room = door.to.room;
                state.x = door.to.x;
                state.y = door.to.y;
                return StepOutcome::Moved;
            }
        }

        let (dx, dy) = match action {
            GridAction::Up => (0i8, -1i8),
            GridAction::Down => (0, 1),
            GridAction::Left => (-1, 0),
            GridAction::Right => (1, 0),
            GridAction::Wait | GridAction::Attack => (0, 0),
        };
        let Some(next_x) = add_delta(state.x, dx) else {
            return StepOutcome::BlockedByWall;
        };
        let Some(next_y) = add_delta(state.y, dy) else {
            return StepOutcome::BlockedByWall;
        };
        if next_x >= GRID_WIDTH || next_y >= GRID_HEIGHT || self.is_wall(state.room, next_x, next_y)
        {
            return StepOutcome::BlockedByWall;
        }

        state.x = next_x;
        state.y = next_y;
        StepOutcome::Moved
    }

    /// Fake-scorer progress score for a state in this world.
    #[must_use]
    pub fn progress_score_value(&self, state: GridState) -> f64 {
        let room = state.room.id() as usize;
        self.room_base_score[room]
            + f64::from(state.x) * self.room_x_weight[room]
            + f64::from((GRID_HEIGHT - 1).saturating_sub(state.y)) * self.room_y_weight[room]
            + if state.has_key() { 25.0 } else { 0.0 }
            + f64::from(self.boss_hp_max().saturating_sub(state.boss_hp)) * 40.0
            + if self.goal_reached(state) {
                1_000.0
            } else {
                0.0
            }
    }

    #[must_use]
    pub fn boss_hp_max(&self) -> u8 {
        self.boss.map_or(0, |_| BOSS_MAX_HP)
    }

    /// Fake-scorer stage for a state in this world.
    #[must_use]
    pub fn stage_value(&self, state: GridState) -> u32 {
        if self.goal_reached(state) {
            4
        } else if state.room == Room::Boss {
            3
        } else if state.has_key() {
            2
        } else if state.room == Room::Start {
            1
        } else {
            0
        }
    }

    #[must_use]
    pub fn prune(&self, state: GridState) -> bool {
        self.prune_cell.is_some_and(|cell| cell.matches(state))
    }
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

    /// Goal check in the default three-room world (back-compat shortcut).
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

    /// Go-Explore cell: position plus the progress dimensions (key bits,
    /// boss hp), so states that differ in progress occupy distinct cells
    /// and rule-2 regressions toward unexplored progress stay commit-able.
    #[must_use]
    pub const fn cell_key(self) -> CellKey {
        let room = self.room.id() as u64;
        let x = self.x as u64;
        let y = self.y as u64;
        let keys = self.keys as u64;
        let boss_hp = self.boss_hp as u64;
        CellKey::new((boss_hp << 48) | (keys << 40) | (room << 32) | (x << 16) | y)
    }

    /// Steps in the default three-room world (back-compat shortcut for
    /// existing tests; world-aware callers use [`GridWorld::step`]).
    #[must_use]
    pub fn step(self, action: GridAction) -> (Self, StepOutcome) {
        GridWorld::three_room().step(self, action)
    }

    /// Applies actions in the default three-room world.
    #[must_use]
    pub fn apply_actions(self, actions: &[GridAction]) -> Self {
        GridWorld::three_room().apply_actions(self, actions)
    }
}

fn add_delta(value: u8, delta: i8) -> Option<u8> {
    if delta.is_negative() {
        value.checked_sub(delta.unsigned_abs())
    } else {
        value.checked_add(delta as u8)
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

    #[test]
    fn world_default_fixture_matches_golden_literals() {
        // GridState::step delegates to GridWorld::three_room(), so comparing
        // the two is tautological (review finding). Pin the default world's
        // observable behavior to golden literals instead: a scripted
        // full solve, its endpoint identity values, and the score curve.
        let world = GridWorld::three_room();
        assert_eq!(world.initial_state(), GridState::new());

        let solve = [
            // Start corridor -> key vault
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right, // door -> KeyVault(0,2)
            GridAction::Right,
            GridAction::Right, // (2,2) key
            GridAction::Left,
            GridAction::Left,
            GridAction::Left, // door -> Start(4,2)
            GridAction::Left,
            GridAction::Left,
            GridAction::Up,
            GridAction::Up, // (2,0) boss door
            GridAction::Up, // -> Boss(2,4)
            GridAction::Up,
            GridAction::Up, // (2,2) boss cell
            GridAction::Attack,
            GridAction::Attack,
            GridAction::Attack, // boss down
            GridAction::Right,
            GridAction::Right,
            GridAction::Up,
            GridAction::Up, // (4,0) credits
        ];
        let solved = world.apply_actions(GridState::new(), &solve);
        assert!(world.goal_reached(solved));
        assert_eq!(
            (solved.room, solved.x, solved.y, solved.keys, solved.boss_hp),
            (Room::Boss, 4, 0, KEY_BOSS_DOOR, 0)
        );

        // Golden identity values (stable across refactors; a change here is
        // a wire-visible break, not a cleanup).
        assert_eq!(
            hex(GridState::new().state_hash().as_bytes()),
            GOLDEN_ROOT_HASH
        );
        assert_eq!(hex(solved.state_hash().as_bytes()), GOLDEN_GOAL_HASH);
        assert_eq!(GridState::new().cell_key().get(), GOLDEN_ROOT_CELL);
        assert_eq!(solved.cell_key().get(), GOLDEN_GOAL_CELL);
        assert_eq!(world.progress_score_value(GridState::new()), 2.0);
        assert_eq!(world.progress_score_value(solved), 1389.0);
    }

    const GOLDEN_ROOT_HASH: &str =
        "a58ed52e5e471cabc5fc2bf4c8296911b751673728415242820e881aa23f87a4";
    const GOLDEN_GOAL_HASH: &str =
        "39c81415f549e99a4d46781ab7cf807bea304cec7cafa8ad420fbfcab9c2d97f";
    const GOLDEN_ROOT_CELL: u64 = 844_424_930_131_970;
    const GOLDEN_GOAL_CELL: u64 = 1_108_101_824_512;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn corridor_hidden_key_world_is_solvable_and_gates_on_the_key() {
        let world = GridWorld::corridor_hidden_key();
        let start = world.initial_state();

        // Greedy rightward run saturates at the locked stage gate.
        let at_gate = world.apply_actions(
            start,
            &[
                GridAction::Right,
                GridAction::Right,
                GridAction::Right,
                GridAction::Right,
            ],
        );
        assert_eq!((at_gate.x, at_gate.y, at_gate.room), (4, 2, Room::Start));
        let (blocked, blocked_outcome) = world.step(at_gate, GridAction::Right);
        assert_eq!(blocked_outcome, StepOutcome::BlockedByDoor);
        assert_eq!(blocked, at_gate);

        // The detour: climb at the corridor start, cross the annex to the
        // hidden key (whose cell doubles as the way back), then through the
        // gate. Up/Right only — the fake synth's alphabet has no Down.
        let scripted = [
            GridAction::Up, // (0,1)
            GridAction::Up, // (0,0)
            GridAction::Up, // door -> KeyVault(0,0)
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right, // (4,0) key
        ];
        let with_key = world.apply_actions(start, &scripted);
        assert!(
            with_key.has_key(),
            "detour must reach the key: {with_key:?}"
        );

        let back_and_through = [
            GridAction::Up, // key-cell door -> Start(0,2)
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right, // gate cell (4,2)
            GridAction::Right, // through the gate -> Boss room (0,2)
            GridAction::Right,
            GridAction::Right,
            GridAction::Right,
            GridAction::Right, // (4,2) goal
        ];
        let solved = world.apply_actions(with_key, &back_and_through);
        assert!(
            world.goal_reached(solved),
            "scripted solve must reach credits: {solved:?}"
        );

        // The score gradient pulls toward the gate, not into the climb
        // shaft: the shaft reads far below sitting at the gate.
        let mid_detour = world.apply_actions(start, &[GridAction::Up, GridAction::Up]);
        assert_eq!(mid_detour.room, Room::Start);
        assert!(
            world.progress_score_value(at_gate) > world.progress_score_value(mid_detour) + 30.0,
            "the detour must be a score trap: gate {} vs shaft {}",
            world.progress_score_value(at_gate),
            world.progress_score_value(mid_detour)
        );
        assert_eq!(world.stage_value(at_gate), 1);
        assert_eq!(world.stage_value(with_key), 2);
        assert_eq!(world.stage_value(solved), 4);
    }
}
