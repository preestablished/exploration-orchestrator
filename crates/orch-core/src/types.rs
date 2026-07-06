//! Shared deterministic value types for the search core.

use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const DIGEST_LEN: usize = 32;
pub const EXPERIMENT_CONFIG_VERSION: u32 = 1;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct NodeId(pub u64);

impl NodeId {
    pub const ROOT: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn is_root(self) -> bool {
        self.0 == Self::ROOT.0
    }
}

impl From<u64> for NodeId {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapshotRef(pub [u8; DIGEST_LEN]);

impl SnapshotRef {
    pub const fn new(bytes: [u8; DIGEST_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; DIGEST_LEN] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; DIGEST_LEN] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StateHash(pub [u8; DIGEST_LEN]);

impl StateHash {
    pub const fn new(bytes: [u8; DIGEST_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; DIGEST_LEN] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; DIGEST_LEN] {
        self.0
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CellKey(pub u64);

impl CellKey {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for CellKey {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NonFiniteFloatError {
    value: f64,
}

impl NonFiniteFloatError {
    pub const fn value(self) -> f64 {
        self.value
    }
}

impl fmt::Display for NonFiniteFloatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "non-finite float value {}", self.value)
    }
}

impl std::error::Error for NonFiniteFloatError {}

#[derive(Clone, Copy, Debug)]
pub struct FiniteF64(f64);

impl FiniteF64 {
    pub fn new(value: f64) -> Result<Self, NonFiniteFloatError> {
        if !value.is_finite() {
            return Err(NonFiniteFloatError { value });
        }

        let normalized = if value == 0.0 { 0.0 } else { value };
        Ok(Self(normalized))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for FiniteF64 {
    type Error = NonFiniteFloatError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl PartialEq for FiniteF64 {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for FiniteF64 {}

impl PartialOrd for FiniteF64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FiniteF64 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Hash for FiniteF64 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl Serialize for FiniteF64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for FiniteF64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Score(FiniteF64);

impl Score {
    pub fn new(value: f64) -> Result<Self, NonFiniteFloatError> {
        FiniteF64::new(value).map(Self)
    }

    pub const fn get(self) -> f64 {
        self.0.get()
    }
}

impl TryFrom<f64> for Score {
    type Error = NonFiniteFloatError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Novelty(FiniteF64);

impl Novelty {
    pub fn new(value: f64) -> Result<Self, NonFiniteFloatError> {
        FiniteF64::new(value).map(Self)
    }

    pub const fn get(self) -> f64 {
        self.0.get()
    }
}

impl TryFrom<f64> for Novelty {
    type Error = NonFiniteFloatError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Stage(pub u16);

impl Stage {
    pub const NONE: Self = Self(0);

    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FrameCount(pub u32);

impl FrameCount {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GuestInstructions(pub u64);

impl GuestInstructions {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum NodeStatus {
    #[default]
    Frontier,
    Expanded,
    Pruned,
    Goal,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum PruneAction {
    #[default]
    Exhausted,
    Drop,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum OnGoal {
    #[default]
    Stop,
    Continue,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum PolicyKind {
    #[default]
    Softmax,
    Ucb,
    Staged,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum SchedMode {
    #[default]
    Fast,
    Deterministic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CommitDisposition {
    Keep,
    Discard(DiscardReason),
    PrunedExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DiscardReason {
    Duplicate,
    Regression,
    PruneDrop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FrontierEvictReason {
    MaxVisits,
    AllDuplicateExpansions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    pub max_nodes: u64,
    pub max_wall_clock_s: u64,
    pub max_guest_instructions: u64,
    pub max_expansions: u64,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            max_nodes: 1_000_000,
            max_wall_clock_s: 86_400,
            max_guest_instructions: 0,
            max_expansions: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectionConfig {
    pub policy: PolicyKind,
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
    pub delta: f64,
    pub temperature: f64,
    pub ucb_c: f64,
    pub staged: StagedConfig,
    pub max_visits_per_node: u32,
    pub exhaust_after_dup_expansions: u32,
}

impl Default for SelectionConfig {
    fn default() -> Self {
        Self {
            policy: PolicyKind::Softmax,
            alpha: 1.0,
            beta: 0.5,
            gamma: 0.3,
            delta: 0.1,
            temperature: 1.0,
            ucb_c: 1.4,
            staged: StagedConfig::default(),
            max_visits_per_node: 64,
            exhaust_after_dup_expansions: 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StagedConfig {
    pub inner: PolicyKind,
    pub epsilon_regress: f64,
}

impl Default for StagedConfig {
    fn default() -> Self {
        Self {
            inner: PolicyKind::Softmax,
            epsilon_regress: 0.05,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BurstConfig {
    pub k_per_expansion: u32,
    pub base_burst_len_frames: u32,
    pub max_burst_len_frames: u32,
    pub max_guest_instructions_per_job: u64,
}

impl Default for BurstConfig {
    fn default() -> Self {
        Self {
            k_per_expansion: 16,
            base_burst_len_frames: 120,
            max_burst_len_frames: 600,
            max_guest_instructions_per_job: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlateauConfig {
    pub window_n: u32,
    pub epsilon_s: f64,
    pub ladder: LadderConfig,
}

impl Default for PlateauConfig {
    fn default() -> Self {
        Self {
            window_n: 200,
            epsilon_s: 0.001,
            ladder: LadderConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LadderConfig {
    pub burst_len_factor: f64,
    pub temp_factor: f64,
    pub macro_weight_hot: f64,
    pub backtrack_kappa: f64,
    pub backtrack_depth_quantile: f64,
    pub radius_factor: f64,
    pub max_level: u32,
}

impl Default for LadderConfig {
    fn default() -> Self {
        Self {
            burst_len_factor: 1.5,
            temp_factor: 1.75,
            macro_weight_hot: 0.5,
            backtrack_kappa: 1.0,
            backtrack_depth_quantile: 0.5,
            radius_factor: 2.0,
            max_level: 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulingConfig {
    pub mode: SchedMode,
    pub max_inflight_batches: u32,
    pub job_timeout_s: u32,
    pub retry_max: u32,
    pub retry_backoff_ms: u32,
    pub hypervisor_endpoints: Vec<String>,
    pub allow_class_mismatch: bool,
}

impl Default for SchedulingConfig {
    fn default() -> Self {
        Self {
            mode: SchedMode::Fast,
            max_inflight_batches: 2,
            job_timeout_s: 120,
            retry_max: 3,
            retry_backoff_ms: 250,
            hypervisor_endpoints: Vec::new(),
            allow_class_mismatch: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointConfig {
    pub every_commits: u32,
    pub every_seconds: u32,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            every_commits: 64,
            every_seconds: 30,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExperimentConfig {
    pub version: u32,
    pub seed: u64,
    pub workload_image_ref: String,
    pub feature_map_ref: String,
    pub scoring_program_ref: String,
    pub synth_config_ref: String,
    pub macro_pack_refs: Vec<String>,
    pub budgets: Budgets,
    pub selection: SelectionConfig,
    pub burst: BurstConfig,
    pub plateau: PlateauConfig,
    pub scheduling: SchedulingConfig,
    pub checkpoint: CheckpointConfig,
    pub prune_action: PruneAction,
    pub on_goal: OnGoal,
    pub decoded_features: Vec<String>,
}

impl ExperimentConfig {
    pub fn new(
        seed: u64,
        workload_image_ref: impl Into<String>,
        feature_map_ref: impl Into<String>,
        scoring_program_ref: impl Into<String>,
        synth_config_ref: impl Into<String>,
    ) -> Self {
        Self {
            version: EXPERIMENT_CONFIG_VERSION,
            seed,
            workload_image_ref: workload_image_ref.into(),
            feature_map_ref: feature_map_ref.into(),
            scoring_program_ref: scoring_program_ref.into(),
            synth_config_ref: synth_config_ref.into(),
            macro_pack_refs: Vec::new(),
            budgets: Budgets::default(),
            selection: SelectionConfig::default(),
            burst: BurstConfig::default(),
            plateau: PlateauConfig::default(),
            scheduling: SchedulingConfig::default(),
            checkpoint: CheckpointConfig::default(),
            prune_action: PruneAction::default(),
            on_goal: OnGoal::default(),
            decoded_features: Vec::new(),
        }
    }

    /// First-violation validation (kept for existing callers).
    pub fn validate(&self) -> Result<(), ConfigError> {
        match self.validate_all().into_iter().next() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Accumulating validation: returns every violation so the served
    /// surface's INVALID_ARGUMENT can list every bad field (API.md §1).
    pub fn validate_all(&self) -> Vec<ConfigError> {
        let mut errors = Vec::new();
        if self.version != EXPERIMENT_CONFIG_VERSION {
            errors.push(ConfigError::InvalidVersion(self.version));
        }

        if let Err(error) = require_non_empty("workload_image_ref", &self.workload_image_ref) {
            errors.push(error);
        }
        if let Err(error) = require_non_empty("feature_map_ref", &self.feature_map_ref) {
            errors.push(error);
        }
        if let Err(error) = require_non_empty("scoring_program_ref", &self.scoring_program_ref) {
            errors.push(error);
        }
        if let Err(error) = require_non_empty("synth_config_ref", &self.synth_config_ref) {
            errors.push(error);
        }

        if !(1..=256).contains(&self.burst.k_per_expansion) {
            errors.push(ConfigError::OutOfRange("burst.k_per_expansion"));
        }
        if self.burst.base_burst_len_frames > self.burst.max_burst_len_frames {
            errors.push(ConfigError::OutOfRange("burst.base_burst_len_frames"));
        }
        if !finite_positive(self.selection.temperature) {
            errors.push(ConfigError::OutOfRange("selection.temperature"));
        }
        let non_negative_selection_weights = [
            ("selection.alpha", self.selection.alpha),
            ("selection.beta", self.selection.beta),
            ("selection.gamma", self.selection.gamma),
            ("selection.delta", self.selection.delta),
            ("selection.ucb_c", self.selection.ucb_c),
        ];
        for (field, value) in non_negative_selection_weights {
            if !finite_non_negative(value) {
                errors.push(ConfigError::OutOfRange(field));
            }
        }
        if !finite_unit_interval(self.selection.staged.epsilon_regress) {
            errors.push(ConfigError::OutOfRange("selection.staged.epsilon_regress"));
        }
        if self.selection.max_visits_per_node == 0 {
            errors.push(ConfigError::OutOfRange("selection.max_visits_per_node"));
        }
        if self.selection.exhaust_after_dup_expansions == 0 {
            errors.push(ConfigError::OutOfRange(
                "selection.exhaust_after_dup_expansions",
            ));
        }
        if self.selection.policy == PolicyKind::Staged
            && self.selection.staged.inner == PolicyKind::Staged
        {
            errors.push(ConfigError::InvalidStagedInnerPolicy);
        }
        if self.burst.base_burst_len_frames == 0 {
            errors.push(ConfigError::OutOfRange("burst.base_burst_len_frames"));
        }
        if self.burst.max_burst_len_frames == 0 {
            errors.push(ConfigError::OutOfRange("burst.max_burst_len_frames"));
        }
        if self.plateau.window_n < 10 {
            errors.push(ConfigError::OutOfRange("plateau.window_n"));
        }
        if !finite_positive(self.plateau.epsilon_s) {
            errors.push(ConfigError::OutOfRange("plateau.epsilon_s"));
        }
        if self.plateau.ladder.max_level > 4 {
            errors.push(ConfigError::OutOfRange("plateau.ladder.max_level"));
        }
        if self.scheduling.mode == SchedMode::Deterministic
            && self.scheduling.max_inflight_batches != 1
        {
            errors.push(ConfigError::OutOfRange("scheduling.max_inflight_batches"));
        }
        if self.scheduling.mode == SchedMode::Fast && self.scheduling.max_inflight_batches == 0 {
            errors.push(ConfigError::OutOfRange("scheduling.max_inflight_batches"));
        }
        if self.scheduling.job_timeout_s == 0 {
            errors.push(ConfigError::OutOfRange("scheduling.job_timeout_s"));
        }
        if self.checkpoint.every_commits == 0 {
            errors.push(ConfigError::OutOfRange("checkpoint.every_commits"));
        }
        if self.checkpoint.every_seconds == 0 {
            errors.push(ConfigError::OutOfRange("checkpoint.every_seconds"));
        }

        let ladder_factors_at_least_one = [
            (
                "plateau.ladder.burst_len_factor",
                self.plateau.ladder.burst_len_factor,
            ),
            (
                "plateau.ladder.temp_factor",
                self.plateau.ladder.temp_factor,
            ),
            (
                "plateau.ladder.radius_factor",
                self.plateau.ladder.radius_factor,
            ),
        ];
        for (field, value) in ladder_factors_at_least_one {
            if !value.is_finite() || value < 1.0 {
                errors.push(ConfigError::OutOfRange(field));
            }
        }
        if !finite_non_negative(self.plateau.ladder.macro_weight_hot) {
            errors.push(ConfigError::OutOfRange("plateau.ladder.macro_weight_hot"));
        }
        if !finite_non_negative(self.plateau.ladder.backtrack_kappa) {
            errors.push(ConfigError::OutOfRange("plateau.ladder.backtrack_kappa"));
        }
        if !finite_unit_interval(self.plateau.ladder.backtrack_depth_quantile) {
            errors.push(ConfigError::OutOfRange(
                "plateau.ladder.backtrack_depth_quantile",
            ));
        }

        errors
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    InvalidVersion(u32),
    MissingRequiredField(&'static str),
    OutOfRange(&'static str),
    InvalidStagedInnerPolicy,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidVersion(version) => write!(formatter, "invalid config version {version}"),
            Self::MissingRequiredField(field) => {
                write!(formatter, "missing required field {field}")
            }
            Self::OutOfRange(field) => write!(formatter, "field out of range {field}"),
            Self::InvalidStagedInnerPolicy => {
                write!(formatter, "staged inner policy cannot be staged")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), ConfigError> {
    if value.is_empty() {
        Err(ConfigError::MissingRequiredField(field))
    } else {
        Ok(())
    }
}

fn finite_positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn finite_non_negative(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

fn finite_unit_interval(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_node_id_orders_by_dense_value_and_names_root() {
        assert_eq!(NodeId::ROOT, NodeId::new(0));
        assert!(NodeId::ROOT.is_root());
        assert!(!NodeId::new(1).is_root());
        assert!(NodeId::new(2) > NodeId::new(1));
    }

    #[test]
    fn types_digest_wrappers_round_trip_bytes() {
        let snapshot = SnapshotRef::new([7; DIGEST_LEN]);
        let hash = StateHash::new([9; DIGEST_LEN]);

        assert_eq!(snapshot.as_bytes(), &[7; DIGEST_LEN]);
        assert_eq!(snapshot.into_bytes(), [7; DIGEST_LEN]);
        assert_eq!(hash.as_bytes(), &[9; DIGEST_LEN]);
        assert_eq!(hash.into_bytes(), [9; DIGEST_LEN]);
    }

    #[test]
    fn types_finite_scores_reject_non_finite_inputs() {
        assert_eq!(Score::new(1.25).unwrap().get(), 1.25);
        assert_eq!(Novelty::new(0.5).unwrap().get(), 0.5);
        assert!(Score::new(f64::NAN).is_err());
        assert!(Score::new(f64::INFINITY).is_err());
        assert!(Novelty::new(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn types_finite_scores_normalize_signed_zero_for_ordering() {
        let negative_zero = Score::new(-0.0).unwrap();
        let positive_zero = Score::new(0.0).unwrap();

        assert_eq!(negative_zero, positive_zero);
        assert_eq!(negative_zero.cmp(&positive_zero), Ordering::Equal);
    }

    #[test]
    fn types_value_types_postcard_round_trip() {
        let encoded = postcard::to_allocvec(&(
            NodeId::new(42),
            SnapshotRef::new([1; DIGEST_LEN]),
            StateHash::new([2; DIGEST_LEN]),
            CellKey::new(99),
            Score::new(12.5).unwrap(),
            Novelty::new(0.25).unwrap(),
            Stage::new(3),
            NodeStatus::Goal,
            CommitDisposition::Discard(DiscardReason::Duplicate),
        ))
        .unwrap();
        let decoded: (
            NodeId,
            SnapshotRef,
            StateHash,
            CellKey,
            Score,
            Novelty,
            Stage,
            NodeStatus,
            CommitDisposition,
        ) = postcard::from_bytes(&encoded).unwrap();

        assert_eq!(decoded.0, NodeId::new(42));
        assert_eq!(decoded.1, SnapshotRef::new([1; DIGEST_LEN]));
        assert_eq!(decoded.2, StateHash::new([2; DIGEST_LEN]));
        assert_eq!(decoded.3, CellKey::new(99));
        assert_eq!(decoded.4, Score::new(12.5).unwrap());
        assert_eq!(decoded.5, Novelty::new(0.25).unwrap());
        assert_eq!(decoded.6, Stage::new(3));
        assert_eq!(decoded.7, NodeStatus::Goal);
        assert_eq!(
            decoded.8,
            CommitDisposition::Discard(DiscardReason::Duplicate)
        );
    }

    #[test]
    fn types_score_and_novelty_reject_non_finite_deserialization() {
        let nan = postcard::to_allocvec(&f64::NAN).unwrap();
        let infinity = postcard::to_allocvec(&f64::INFINITY).unwrap();

        assert!(postcard::from_bytes::<Score>(&nan).is_err());
        assert!(postcard::from_bytes::<Score>(&infinity).is_err());
        assert!(postcard::from_bytes::<Novelty>(&nan).is_err());
        assert!(postcard::from_bytes::<Novelty>(&infinity).is_err());
    }

    #[test]
    fn types_enums_have_stable_ordering() {
        assert!(NodeStatus::Frontier < NodeStatus::Expanded);
        assert!(PruneAction::Exhausted < PruneAction::Drop);
        assert!(OnGoal::Stop < OnGoal::Continue);
        assert!(PolicyKind::Softmax < PolicyKind::Ucb);
        assert!(SchedMode::Fast < SchedMode::Deterministic);
        assert!(
            CommitDisposition::Discard(DiscardReason::Duplicate)
                < CommitDisposition::PrunedExhausted
        );
        assert!(DiscardReason::Duplicate < DiscardReason::Regression);
        assert!(FrontierEvictReason::MaxVisits < FrontierEvictReason::AllDuplicateExpansions);
    }

    #[test]
    fn types_config_defaults_match_api_schema() {
        let config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");

        assert_eq!(config.version, EXPERIMENT_CONFIG_VERSION);
        assert_eq!(config.seed, 123);
        assert_eq!(config.budgets.max_nodes, 1_000_000);
        assert_eq!(config.budgets.max_wall_clock_s, 86_400);
        assert_eq!(config.selection.policy, PolicyKind::Softmax);
        assert_eq!(config.selection.temperature, 1.0);
        assert_eq!(config.selection.ucb_c, 1.4);
        assert_eq!(config.selection.staged.inner, PolicyKind::Softmax);
        assert_eq!(config.selection.staged.epsilon_regress, 0.05);
        assert_eq!(config.selection.max_visits_per_node, 64);
        assert_eq!(config.selection.exhaust_after_dup_expansions, 8);
        assert_eq!(config.burst.k_per_expansion, 16);
        assert_eq!(config.burst.base_burst_len_frames, 120);
        assert_eq!(config.burst.max_burst_len_frames, 600);
        assert_eq!(config.plateau.window_n, 200);
        assert_eq!(config.plateau.epsilon_s, 0.001);
        assert_eq!(config.plateau.ladder.max_level, 4);
        assert_eq!(config.scheduling.mode, SchedMode::Fast);
        assert_eq!(config.scheduling.max_inflight_batches, 2);
        assert_eq!(config.checkpoint.every_commits, 64);
        assert_eq!(config.checkpoint.every_seconds, 30);
        assert_eq!(config.prune_action, PruneAction::Exhausted);
        assert_eq!(config.on_goal, OnGoal::Stop);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn types_config_validation_rejects_invalid_boundaries() {
        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.burst.k_per_expansion = 0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("burst.k_per_expansion"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.selection.temperature = 0.0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("selection.temperature"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.selection.alpha = f64::NAN;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("selection.alpha"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.selection.beta = f64::INFINITY;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("selection.beta"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.selection.gamma = f64::NEG_INFINITY;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("selection.gamma"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.selection.delta = f64::NAN;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("selection.delta"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.selection.ucb_c = f64::NAN;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("selection.ucb_c"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.selection.staged.epsilon_regress = 2.0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("selection.staged.epsilon_regress"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.selection.max_visits_per_node = 0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("selection.max_visits_per_node"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.selection.exhaust_after_dup_expansions = 0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange(
                "selection.exhaust_after_dup_expansions"
            ))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.selection.policy = PolicyKind::Staged;
        config.selection.staged.inner = PolicyKind::Staged;
        assert_eq!(
            config.validate(),
            Err(ConfigError::InvalidStagedInnerPolicy)
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.plateau.ladder.radius_factor = 0.5;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("plateau.ladder.radius_factor"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.scheduling.mode = SchedMode::Deterministic;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("scheduling.max_inflight_batches"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.scheduling.max_inflight_batches = 0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("scheduling.max_inflight_batches"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.scheduling.job_timeout_s = 0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("scheduling.job_timeout_s"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.checkpoint.every_commits = 0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("checkpoint.every_commits"))
        );

        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.checkpoint.every_seconds = 0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::OutOfRange("checkpoint.every_seconds"))
        );
    }

    #[test]
    fn types_config_postcard_round_trip() {
        let mut config = ExperimentConfig::new(123, "image", "features", "scoring", "synth");
        config.macro_pack_refs.push("macro-pack".to_owned());
        config.decoded_features.push("position_x".to_owned());

        let encoded = postcard::to_allocvec(&config).unwrap();
        let decoded: ExperimentConfig = postcard::from_bytes(&encoded).unwrap();

        assert_eq!(decoded, config);
        assert!(decoded.validate().is_ok());
    }
}
#[cfg(test)]
mod validate_all_tests {
    use super::*;

    #[test]
    fn validate_all_accumulates_every_violation() {
        let mut config = ExperimentConfig::new(1, "w", "f", "s", "y");
        config.workload_image_ref = String::new();
        config.selection.temperature = 0.0;
        config.plateau.window_n = 1;
        config.checkpoint.every_commits = 0;

        let errors = config.validate_all();

        assert!(errors.len() >= 4, "all violations listed: {errors:?}");
        assert!(errors.iter().any(|error| matches!(
            error,
            ConfigError::MissingRequiredField("workload_image_ref")
        )));
        assert!(errors
            .iter()
            .any(|error| matches!(error, ConfigError::OutOfRange("selection.temperature"))));
        assert!(errors
            .iter()
            .any(|error| matches!(error, ConfigError::OutOfRange("plateau.window_n"))));
        assert!(errors
            .iter()
            .any(|error| matches!(error, ConfigError::OutOfRange("checkpoint.every_commits"))));
        assert_eq!(config.validate().expect_err("first violation"), errors[0]);
    }
}
