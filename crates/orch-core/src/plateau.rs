//! Stall detection and escalation ladder state.

use crate::types::{ExperimentConfig, LadderConfig, PlateauConfig, Score};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlateauKnobs {
    pub window_n: u32,
    pub epsilon_s: f64,
    pub ladder: EscalationKnobs,
}

impl PlateauKnobs {
    #[must_use]
    pub const fn new(window_n: u32, epsilon_s: f64, ladder: EscalationKnobs) -> Self {
        Self {
            window_n,
            epsilon_s,
            ladder,
        }
    }

    #[must_use]
    pub fn from_config(config: &ExperimentConfig) -> Self {
        Self::from_plateau_config(&config.plateau)
    }

    #[must_use]
    pub fn from_plateau_config(config: &PlateauConfig) -> Self {
        Self {
            window_n: config.window_n,
            epsilon_s: config.epsilon_s,
            ladder: EscalationKnobs::from_ladder_config(&config.ladder),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EscalationKnobs {
    pub burst_len_factor: f64,
    pub temp_factor: f64,
    pub macro_weight_hot: f64,
    pub backtrack_kappa: f64,
    pub backtrack_depth_quantile: f64,
    pub radius_factor: f64,
    pub max_level: EscalationLevel,
}

impl EscalationKnobs {
    #[must_use]
    pub const fn new(
        burst_len_factor: f64,
        temp_factor: f64,
        macro_weight_hot: f64,
        backtrack_kappa: f64,
        backtrack_depth_quantile: f64,
        radius_factor: f64,
        max_level: EscalationLevel,
    ) -> Self {
        Self {
            burst_len_factor,
            temp_factor,
            macro_weight_hot,
            backtrack_kappa,
            backtrack_depth_quantile,
            radius_factor,
            max_level,
        }
    }

    #[must_use]
    pub fn from_ladder_config(config: &LadderConfig) -> Self {
        Self {
            burst_len_factor: config.burst_len_factor,
            temp_factor: config.temp_factor,
            macro_weight_hot: config.macro_weight_hot,
            backtrack_kappa: config.backtrack_kappa,
            backtrack_depth_quantile: config.backtrack_depth_quantile,
            radius_factor: config.radius_factor,
            max_level: EscalationLevel::from_capped_u32(config.max_level),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum EscalationLevel {
    #[default]
    L0,
    L1,
    L2,
    L3,
    L4,
}

impl EscalationLevel {
    #[must_use]
    pub const fn get(self) -> u32 {
        match self {
            Self::L0 => 0,
            Self::L1 => 1,
            Self::L2 => 2,
            Self::L3 => 3,
            Self::L4 => 4,
        }
    }

    #[must_use]
    pub const fn from_capped_u32(value: u32) -> Self {
        match value {
            0 => Self::L0,
            1 => Self::L1,
            2 => Self::L2,
            3 => Self::L3,
            _ => Self::L4,
        }
    }

    #[must_use]
    pub const fn includes(self, level: Self) -> bool {
        self.get() >= level.get()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StallObservation {
    pub observation_index: u64,
    pub score: Score,
    pub best_score: Score,
    pub improved: bool,
    pub observations_since_improvement: u64,
    pub completed_stall_windows: u64,
    pub stalled: bool,
    pub completed_new_window: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StallDetector {
    window_n: u32,
    epsilon_s: f64,
    observations: u64,
    best_score: Option<Score>,
    observations_since_improvement: u64,
    completed_stall_windows: u64,
}

impl StallDetector {
    #[must_use]
    pub const fn new(window_n: u32, epsilon_s: f64) -> Self {
        Self {
            window_n,
            epsilon_s,
            observations: 0,
            best_score: None,
            observations_since_improvement: 0,
            completed_stall_windows: 0,
        }
    }

    #[must_use]
    pub const fn from_knobs(knobs: &PlateauKnobs) -> Self {
        Self::new(knobs.window_n, knobs.epsilon_s)
    }

    #[must_use]
    pub const fn observations(&self) -> u64 {
        self.observations
    }

    #[must_use]
    pub const fn best_score(&self) -> Option<Score> {
        self.best_score
    }

    #[must_use]
    pub const fn observations_since_improvement(&self) -> u64 {
        self.observations_since_improvement
    }

    #[must_use]
    pub const fn completed_stall_windows(&self) -> u64 {
        self.completed_stall_windows
    }

    pub fn reset(&mut self) {
        self.observations = 0;
        self.best_score = None;
        self.observations_since_improvement = 0;
        self.completed_stall_windows = 0;
    }

    pub fn observe(&mut self, score: Score) -> StallObservation {
        self.observations = self.observations.saturating_add(1);
        let observation_index = self.observations - 1;
        let mut improved = false;

        match self.best_score {
            Some(best_score) if score.get() > best_score.get() + self.epsilon_s => {
                self.best_score = Some(score);
                self.observations_since_improvement = 0;
                self.completed_stall_windows = 0;
                improved = true;
            }
            Some(_) => {
                self.observations_since_improvement =
                    self.observations_since_improvement.saturating_add(1);
                self.completed_stall_windows = if self.window_n == 0 {
                    0
                } else {
                    self.observations_since_improvement / u64::from(self.window_n)
                };
            }
            None => {
                self.best_score = Some(score);
                improved = true;
            }
        }

        let completed_new_window = !improved
            && self.window_n > 0
            && self.observations_since_improvement > 0
            && self
                .observations_since_improvement
                .is_multiple_of(u64::from(self.window_n));
        let best_score = self.best_score.expect("best score is set after observe");

        StallObservation {
            observation_index,
            score,
            best_score,
            improved,
            observations_since_improvement: self.observations_since_improvement,
            completed_stall_windows: self.completed_stall_windows,
            stalled: self.completed_stall_windows > 0,
            completed_new_window,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct L3Override {
    pub macro_weight_hot: f64,
    pub backtrack: BacktrackKnobs,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BacktrackKnobs {
    pub kappa: f64,
    pub depth_quantile: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct L4Rebin {
    pub feature_map_version: u32,
    pub radius_factor: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EscalationSnapshot {
    pub level: EscalationLevel,
    pub burst_len_factor: f64,
    pub temp_factor: f64,
    pub l3_override: Option<L3Override>,
    pub l4_rebin: Option<L4Rebin>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EscalationLadder {
    knobs: EscalationKnobs,
    level: EscalationLevel,
    feature_map_version: u32,
}

impl EscalationLadder {
    #[must_use]
    pub const fn new(knobs: EscalationKnobs) -> Self {
        Self {
            knobs,
            level: EscalationLevel::L0,
            feature_map_version: 0,
        }
    }

    #[must_use]
    pub const fn from_knobs(knobs: &PlateauKnobs) -> Self {
        Self::new(knobs.ladder)
    }

    #[must_use]
    pub const fn level(&self) -> EscalationLevel {
        self.level
    }

    #[must_use]
    pub const fn feature_map_version(&self) -> u32 {
        self.feature_map_version
    }

    #[must_use]
    pub fn snapshot(&self) -> EscalationSnapshot {
        EscalationSnapshot {
            level: self.level,
            burst_len_factor: if self.level.includes(EscalationLevel::L1) {
                self.knobs.burst_len_factor
            } else {
                1.0
            },
            temp_factor: if self.level.includes(EscalationLevel::L2) {
                self.knobs.temp_factor
            } else {
                1.0
            },
            l3_override: self
                .level
                .includes(EscalationLevel::L3)
                .then_some(L3Override {
                    macro_weight_hot: self.knobs.macro_weight_hot,
                    backtrack: BacktrackKnobs {
                        kappa: self.knobs.backtrack_kappa,
                        depth_quantile: self.knobs.backtrack_depth_quantile,
                    },
                }),
            l4_rebin: self.level.includes(EscalationLevel::L4).then_some(L4Rebin {
                feature_map_version: self.feature_map_version,
                radius_factor: self.knobs.radius_factor,
            }),
        }
    }

    pub fn reset_for_improvement(&mut self) {
        self.level = EscalationLevel::L0;
    }

    pub fn apply_stall_observation(&mut self, observation: StallObservation) -> EscalationSnapshot {
        if observation.improved {
            self.reset_for_improvement();
        } else if observation.completed_new_window {
            self.set_completed_stall_windows(observation.completed_stall_windows);
        }
        self.snapshot()
    }

    pub fn advance_one_stall_window(&mut self) -> EscalationSnapshot {
        let next = self.level.get().saturating_add(1);
        self.set_level(EscalationLevel::from_capped_u32(next));
        self.snapshot()
    }

    pub fn set_completed_stall_windows(
        &mut self,
        completed_stall_windows: u64,
    ) -> EscalationSnapshot {
        let capped = completed_stall_windows.min(u64::from(self.knobs.max_level.get()));
        let level = EscalationLevel::from_capped_u32(u32::try_from(capped).unwrap_or(u32::MAX));
        self.set_level(level);
        self.snapshot()
    }

    fn set_level(&mut self, requested: EscalationLevel) {
        let next = self.level.max(requested.min(self.knobs.max_level));
        if self.level < EscalationLevel::L4 && next >= EscalationLevel::L4 {
            self.feature_map_version = self.feature_map_version.saturating_add(1);
        }
        self.level = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plateau_stall_detector_window_edges_and_reset_on_improvement() {
        let mut detector = StallDetector::new(3, 0.01);

        let first = detector.observe(score(10.0));
        assert_eq!(first.observation_index, 0);
        assert!(first.improved);
        assert!(!first.stalled);
        assert_eq!(first.completed_stall_windows, 0);

        let second = detector.observe(score(10.005));
        assert!(!second.improved);
        assert!(!second.stalled);
        assert_eq!(second.observations_since_improvement, 1);

        let third = detector.observe(score(10.005));
        assert!(!third.stalled);
        assert_eq!(third.observations_since_improvement, 2);
        assert_eq!(third.completed_stall_windows, 0);

        let fourth = detector.observe(score(10.005));
        assert!(fourth.stalled);
        assert!(fourth.completed_new_window);
        assert_eq!(fourth.completed_stall_windows, 1);

        let fifth = detector.observe(score(10.005));
        assert!(fifth.stalled);
        assert!(!fifth.completed_new_window);
        assert_eq!(fifth.completed_stall_windows, 1);

        let improved = detector.observe(score(10.011));
        assert!(improved.improved);
        assert_eq!(improved.best_score, score(10.011));
        assert_eq!(improved.observations_since_improvement, 0);
        assert_eq!(improved.completed_stall_windows, 0);
        assert!(!improved.stalled);
    }

    #[test]
    fn plateau_improvement_boundary_is_strictly_greater_than_epsilon() {
        let mut detector = StallDetector::new(10, 1.0);

        assert!(detector.observe(score(10.0)).improved);
        let boundary = detector.observe(score(11.0));
        assert!(!boundary.improved);
        assert_eq!(boundary.best_score, score(10.0));

        let improved = detector.observe(score(11.000_001));
        assert!(improved.improved);
        assert_eq!(improved.best_score, score(11.000_001));
    }

    #[test]
    fn plateau_ladder_applies_cumulative_l1_l4_knobs_and_max_level() {
        let knobs = EscalationKnobs::new(1.5, 1.75, 0.5, 2.0, 0.6, 2.5, EscalationLevel::L4);
        let mut ladder = EscalationLadder::new(knobs);

        let level_1 = ladder.advance_one_stall_window();
        assert_eq!(level_1.level, EscalationLevel::L1);
        assert_eq!(level_1.burst_len_factor, 1.5);
        assert_eq!(level_1.temp_factor, 1.0);
        assert_eq!(level_1.l3_override, None);
        assert_eq!(level_1.l4_rebin, None);

        let level_2 = ladder.advance_one_stall_window();
        assert_eq!(level_2.level, EscalationLevel::L2);
        assert_eq!(level_2.burst_len_factor, 1.5);
        assert_eq!(level_2.temp_factor, 1.75);
        assert_eq!(level_2.l3_override, None);

        let level_3 = ladder.advance_one_stall_window();
        assert_eq!(level_3.level, EscalationLevel::L3);
        assert!(level_3.l3_override.is_some());
        assert_eq!(level_3.l4_rebin, None);

        let level_4 = ladder.advance_one_stall_window();
        assert_eq!(level_4.level, EscalationLevel::L4);
        assert_eq!(
            level_4.l4_rebin,
            Some(L4Rebin {
                feature_map_version: 1,
                radius_factor: 2.5,
            })
        );

        let still_4 = ladder.advance_one_stall_window();
        assert_eq!(still_4.level, EscalationLevel::L4);
        assert_eq!(still_4.l4_rebin.expect("l4 active").feature_map_version, 1);

        let capped_knobs = EscalationKnobs::new(1.5, 1.75, 0.5, 2.0, 0.6, 2.5, EscalationLevel::L2);
        let mut capped = EscalationLadder::new(capped_knobs);
        let capped_snapshot = capped.set_completed_stall_windows(4);
        assert_eq!(capped_snapshot.level, EscalationLevel::L2);
        assert_eq!(capped_snapshot.l3_override, None);
        assert_eq!(capped_snapshot.l4_rebin, None);
    }

    #[test]
    fn plateau_ladder_exposes_l3_override_and_backtrack_quantile_inputs() {
        let mut ladder = EscalationLadder::new(EscalationKnobs::new(
            1.5,
            1.75,
            0.8,
            3.25,
            0.9,
            2.0,
            EscalationLevel::L4,
        ));

        let snapshot = ladder.set_completed_stall_windows(3);
        assert_eq!(
            snapshot.l3_override,
            Some(L3Override {
                macro_weight_hot: 0.8,
                backtrack: BacktrackKnobs {
                    kappa: 3.25,
                    depth_quantile: 0.9,
                },
            })
        );
    }

    #[test]
    fn plateau_l4_map_version_progresses_one_way_across_resets() {
        let mut ladder = EscalationLadder::new(EscalationKnobs::new(
            1.5,
            1.75,
            0.5,
            1.0,
            0.5,
            2.0,
            EscalationLevel::L4,
        ));

        assert_eq!(
            ladder.set_completed_stall_windows(4).level,
            EscalationLevel::L4
        );
        assert_eq!(ladder.feature_map_version(), 1);

        let stale_lower_count = ladder.set_completed_stall_windows(2);
        assert_eq!(stale_lower_count.level, EscalationLevel::L4);
        assert_eq!(
            stale_lower_count
                .l4_rebin
                .expect("l4 remains active")
                .feature_map_version,
            1
        );
        assert_eq!(ladder.feature_map_version(), 1);

        let repeated_l4 = ladder.set_completed_stall_windows(4);
        assert_eq!(repeated_l4.level, EscalationLevel::L4);
        assert_eq!(
            repeated_l4
                .l4_rebin
                .expect("l4 remains active")
                .feature_map_version,
            1
        );
        assert_eq!(ladder.feature_map_version(), 1);

        ladder.reset_for_improvement();
        assert_eq!(ladder.level(), EscalationLevel::L0);
        assert_eq!(ladder.feature_map_version(), 1);
        assert_eq!(ladder.snapshot().l4_rebin, None);

        ladder.set_completed_stall_windows(2);
        assert_eq!(ladder.level(), EscalationLevel::L2);
        assert_eq!(ladder.feature_map_version(), 1);

        let l4_again = ladder.set_completed_stall_windows(4);
        assert_eq!(l4_again.level, EscalationLevel::L4);
        assert_eq!(l4_again.l4_rebin.expect("l4 active").feature_map_version, 2);
        assert_eq!(ladder.feature_map_version(), 2);
    }

    #[test]
    fn plateau_detector_and_ladder_integrate_consecutive_stall_windows() {
        let mut detector = StallDetector::new(2, 0.1);
        let mut ladder = EscalationLadder::new(EscalationKnobs::new(
            2.0,
            3.0,
            0.75,
            1.5,
            0.25,
            4.0,
            EscalationLevel::L4,
        ));

        for value in [5.0, 5.0, 5.0, 5.0, 5.0] {
            let observation = detector.observe(score(value));
            ladder.apply_stall_observation(observation);
        }
        assert_eq!(detector.completed_stall_windows(), 2);
        assert_eq!(ladder.level(), EscalationLevel::L2);

        let improved = detector.observe(score(5.2));
        let snapshot = ladder.apply_stall_observation(improved);
        assert_eq!(snapshot.level, EscalationLevel::L0);
        assert_eq!(detector.completed_stall_windows(), 0);
    }

    #[test]
    fn plateau_knobs_project_existing_experiment_config() {
        let config = ExperimentConfig::new(7, "image", "features", "scoring", "synth");
        let knobs = PlateauKnobs::from_config(&config);

        assert_eq!(knobs.window_n, 200);
        assert_eq!(knobs.epsilon_s, 0.001);
        assert_eq!(knobs.ladder.max_level, EscalationLevel::L4);
        assert_eq!(knobs.ladder.backtrack_depth_quantile, 0.5);
    }

    fn score(value: f64) -> Score {
        Score::new(value).expect("finite score")
    }
}
