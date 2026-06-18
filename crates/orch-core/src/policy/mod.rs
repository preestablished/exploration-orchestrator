//! Shared selection policy traits and deterministic candidate priority terms.

pub mod softmax;
pub mod staged;
pub mod ucb;

use core::fmt;

use crate::frontier::{Frontier, FrontierError};
use crate::mirror::CellMirror;
use crate::plateau::PlateauKnobs;
use crate::rng::DeterministicRng;
use crate::tree::Tree;
use crate::types::{CellKey, NodeId, Stage};

pub use crate::types::{PolicyKind, SelectionConfig};

pub type PolicyResult<T> = Result<T, PolicyError>;

#[derive(Clone, Debug, PartialEq)]
pub enum PolicyError {
    Frontier(FrontierError),
    EmptyCandidateSet,
    InvalidConfig { field: &'static str },
    InvalidWeight { field: &'static str, value: f64 },
    NonFiniteValue { field: &'static str, value: f64 },
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frontier(error) => write!(formatter, "{error}"),
            Self::EmptyCandidateSet => formatter.write_str("policy candidate set is empty"),
            Self::InvalidConfig { field } => write!(formatter, "invalid policy config {field}"),
            Self::InvalidWeight { field, value } => {
                write!(formatter, "invalid policy weight {field}={value}")
            }
            Self::NonFiniteValue { field, value } => {
                write!(formatter, "non-finite policy value {field}={value}")
            }
        }
    }
}

impl std::error::Error for PolicyError {}

impl From<FrontierError> for PolicyError {
    fn from(error: FrontierError) -> Self {
        Self::Frontier(error)
    }
}

pub trait SelectionPolicy {
    fn kind(&self) -> PolicyKind;

    fn select(
        &mut self,
        context: &PolicyContext<'_>,
        rng: &mut DeterministicRng,
    ) -> PolicyResult<SelectionChoice>;
}

#[derive(Clone, Copy, Debug)]
pub struct PolicyContext<'a> {
    pub tree: &'a Tree,
    pub frontier: &'a Frontier,
    pub mirror: &'a CellMirror,
    pub plateau: &'a PlateauKnobs,
    pub selection: &'a SelectionConfig,
}

impl<'a> PolicyContext<'a> {
    #[must_use]
    pub const fn new(
        tree: &'a Tree,
        frontier: &'a Frontier,
        mirror: &'a CellMirror,
        plateau: &'a PlateauKnobs,
        selection: &'a SelectionConfig,
    ) -> Self {
        Self {
            tree,
            frontier,
            mirror,
            plateau,
            selection,
        }
    }

    pub fn candidate_snapshots(&self) -> PolicyResult<Vec<CandidateSnapshot>> {
        candidate_snapshots(self)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectionChoice {
    pub selected: NodeId,
    pub candidate_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PriorityTerms {
    pub normalized_score: f64,
    pub novelty: f64,
    pub visit_penalty: f64,
    pub depth_penalty: f64,
}

impl PriorityTerms {
    #[must_use]
    pub const fn new(
        normalized_score: f64,
        novelty: f64,
        visit_penalty: f64,
        depth_penalty: f64,
    ) -> Self {
        Self {
            normalized_score,
            novelty,
            visit_penalty,
            depth_penalty,
        }
    }

    pub fn weighted_priority(&self, selection: &SelectionConfig) -> PolicyResult<f64> {
        validate_selection_weights(selection)?;
        ensure_finite("priority.normalized_score", self.normalized_score)?;
        ensure_finite("priority.novelty", self.novelty)?;
        ensure_finite("priority.visit_penalty", self.visit_penalty)?;
        ensure_finite("priority.depth_penalty", self.depth_penalty)?;

        let priority = selection.alpha * self.normalized_score + selection.beta * self.novelty
            - selection.gamma * self.visit_penalty
            - selection.delta * self.depth_penalty;
        ensure_finite("priority.weighted", priority)?;
        Ok(priority)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CandidateSnapshot {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub depth: u32,
    pub visits: u32,
    pub children: u32,
    pub cell: CellKey,
    pub stage: Stage,
    pub raw_score: f64,
    pub priority_terms: PriorityTerms,
    pub priority: f64,
}

pub fn candidate_snapshots(context: &PolicyContext<'_>) -> PolicyResult<Vec<CandidateSnapshot>> {
    validate_selection_weights(context.selection)?;

    let ids = context.frontier.deterministic_candidates(context.tree)?;
    if ids.is_empty() {
        return Err(PolicyError::EmptyCandidateSet);
    }
    let raw_scores: Vec<f64> = ids
        .iter()
        .map(|id| {
            context
                .tree
                .get(*id)
                .expect("frontier candidates exist in tree")
                .score
                .get()
        })
        .collect();
    let normalized_scores = normalize_scores(&raw_scores)?;

    let mut snapshots = Vec::with_capacity(ids.len());
    for (index, id) in ids.into_iter().enumerate() {
        let record = context
            .tree
            .get(id)
            .expect("frontier candidates exist in tree");
        let novelty = context.mirror.novelty(record.cell);
        ensure_finite("candidate.novelty", novelty)?;

        let terms = PriorityTerms::new(
            normalized_scores[index],
            novelty,
            visit_penalty(record.visits)?,
            depth_penalty(record.depth)?,
        );
        snapshots.push(CandidateSnapshot {
            id,
            parent: record.parent,
            depth: record.depth,
            visits: record.visits,
            children: record.children,
            cell: record.cell,
            stage: record.stage,
            raw_score: raw_scores[index],
            priority: terms.weighted_priority(context.selection)?,
            priority_terms: terms,
        });
    }

    Ok(snapshots)
}

pub fn normalize_scores(scores: &[f64]) -> PolicyResult<Vec<f64>> {
    if scores.is_empty() {
        return Ok(Vec::new());
    }

    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for score in scores {
        ensure_finite("score", *score)?;
        min = min.min(*score);
        max = max.max(*score);
    }

    let span = max - min;
    if span == 0.0 {
        return Ok(vec![0.0; scores.len()]);
    }
    if !span.is_finite() {
        return normalize_scores_with_scale(scores, min, max);
    }

    scores
        .iter()
        .map(|score| {
            let normalized = (*score - min) / span;
            ensure_finite("score.normalized", normalized)?;
            Ok(normalized)
        })
        .collect()
}

fn normalize_scores_with_scale(scores: &[f64], min: f64, max: f64) -> PolicyResult<Vec<f64>> {
    let scale = min.abs().max(max.abs());
    ensure_finite("score.scale", scale)?;
    if scale == 0.0 {
        return Ok(vec![0.0; scores.len()]);
    }

    let scaled_min = min / scale;
    let scaled_max = max / scale;
    let span = scaled_max - scaled_min;
    ensure_finite("score.span", span)?;
    if span == 0.0 {
        return Ok(vec![0.0; scores.len()]);
    }

    scores
        .iter()
        .map(|score| {
            let normalized = ((*score / scale) - scaled_min) / span;
            ensure_finite("score.normalized", normalized)?;
            Ok(normalized)
        })
        .collect()
}

pub fn visit_penalty(visits: u32) -> PolicyResult<f64> {
    let penalty = libm::log(1.0 + f64::from(visits));
    ensure_finite("priority.visit_penalty", penalty)?;
    Ok(penalty)
}

pub fn depth_penalty(depth: u32) -> PolicyResult<f64> {
    let penalty = libm::log(1.0 + f64::from(depth));
    ensure_finite("priority.depth_penalty", penalty)?;
    Ok(penalty)
}

pub fn validate_selection_weights(selection: &SelectionConfig) -> PolicyResult<()> {
    for (field, value) in [
        ("selection.alpha", selection.alpha),
        ("selection.beta", selection.beta),
        ("selection.gamma", selection.gamma),
        ("selection.delta", selection.delta),
        ("selection.ucb_c", selection.ucb_c),
    ] {
        ensure_non_negative_weight(field, value)?;
    }

    if !selection.temperature.is_finite() || selection.temperature <= 0.0 {
        return Err(PolicyError::InvalidWeight {
            field: "selection.temperature",
            value: selection.temperature,
        });
    }
    if !selection.staged.epsilon_regress.is_finite()
        || !(0.0..=1.0).contains(&selection.staged.epsilon_regress)
    {
        return Err(PolicyError::InvalidWeight {
            field: "selection.staged.epsilon_regress",
            value: selection.staged.epsilon_regress,
        });
    }
    if selection.max_visits_per_node == 0 {
        return Err(PolicyError::InvalidConfig {
            field: "selection.max_visits_per_node",
        });
    }
    if selection.exhaust_after_dup_expansions == 0 {
        return Err(PolicyError::InvalidConfig {
            field: "selection.exhaust_after_dup_expansions",
        });
    }
    if selection.policy == PolicyKind::Staged && selection.staged.inner == PolicyKind::Staged {
        return Err(PolicyError::InvalidConfig {
            field: "selection.staged.inner",
        });
    }

    Ok(())
}

fn ensure_non_negative_weight(field: &'static str, value: f64) -> PolicyResult<()> {
    if !value.is_finite() || value < 0.0 {
        Err(PolicyError::InvalidWeight { field, value })
    } else {
        Ok(())
    }
}

fn ensure_finite(field: &'static str, value: f64) -> PolicyResult<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(PolicyError::NonFiniteValue { field, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontier::Frontier;
    use crate::mirror::CellMirror;
    use crate::plateau::{EscalationKnobs, EscalationLevel};
    use crate::tree::{NodePayload, Tree};
    use crate::types::{FrameCount, Novelty, SnapshotRef, Stage, StateHash};

    #[test]
    fn policy_common_normalizes_scores_and_builds_priority_terms() {
        let (mut tree, ids) = sample_tree();
        tree.increment_visits(ids.low).unwrap();
        tree.increment_visits(ids.low).unwrap();

        let mut frontier = Frontier::new();
        for id in [ids.high, ids.low, ids.mid] {
            frontier.insert(id).unwrap();
        }
        let mirror = sample_mirror();
        let selection = SelectionConfig::default();
        let plateau = plateau_knobs();
        let context = PolicyContext::new(&tree, &frontier, &mirror, &plateau, &selection);

        let snapshots = candidate_snapshots(&context).unwrap();
        assert_eq!(
            snapshots
                .iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            vec![ids.low, ids.mid, ids.high]
        );
        assert_eq!(
            snapshots
                .iter()
                .map(|candidate| candidate.priority_terms.normalized_score)
                .collect::<Vec<_>>(),
            vec![0.0, 0.5, 1.0]
        );

        let low = &snapshots[0];
        assert_eq!(low.raw_score, 10.0);
        assert_eq!(low.stage, Stage::new(1));
        assert_eq!(low.priority_terms.visit_penalty, libm::log(3.0));
        assert_eq!(low.priority_terms.depth_penalty, libm::log(2.0));
        assert_eq!(low.priority_terms.novelty, 1.0 / f64::sqrt(5.0));
        assert_eq!(
            low.priority,
            selection.alpha * low.priority_terms.normalized_score
                + selection.beta * low.priority_terms.novelty
                - selection.gamma * low.priority_terms.visit_penalty
                - selection.delta * low.priority_terms.depth_penalty
        );
    }

    #[test]
    fn policy_common_scores_equal_values_to_zero_normalized_range() {
        assert_eq!(
            normalize_scores(&[7.0, 7.0, 7.0]).unwrap(),
            vec![0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn policy_common_normalizes_extreme_finite_score_ranges() {
        assert_eq!(
            normalize_scores(&[-f64::MAX, 0.0, f64::MAX]).unwrap(),
            vec![0.0, 0.5, 1.0]
        );
    }

    #[test]
    fn policy_common_rejects_empty_candidate_sets() {
        let (mut tree, ids) = sample_tree();
        let frontier = Frontier::new();
        let mirror = sample_mirror();
        let selection = SelectionConfig::default();
        let plateau = plateau_knobs();
        let context = PolicyContext::new(&tree, &frontier, &mirror, &plateau, &selection);

        assert_eq!(
            candidate_snapshots(&context),
            Err(PolicyError::EmptyCandidateSet)
        );

        tree.mark_exhausted(ids.low).unwrap();
        tree.mark_goal(ids.mid).unwrap();
        let mut frontier = Frontier::new();
        frontier.insert(ids.low).unwrap();
        frontier.insert(ids.mid).unwrap();
        let context = PolicyContext::new(&tree, &frontier, &mirror, &plateau, &selection);

        assert_eq!(
            candidate_snapshots(&context),
            Err(PolicyError::EmptyCandidateSet)
        );
    }

    #[test]
    fn policy_common_rejects_non_finite_inputs_and_weights() {
        assert!(matches!(
            normalize_scores(&[1.0, f64::NAN]),
            Err(PolicyError::NonFiniteValue {
                field: "score",
                value,
            }) if value.is_nan()
        ));

        let mut selection = SelectionConfig::default();
        selection.alpha = f64::INFINITY;
        assert_eq!(
            validate_selection_weights(&selection),
            Err(PolicyError::InvalidWeight {
                field: "selection.alpha",
                value: f64::INFINITY,
            })
        );

        let terms = PriorityTerms::new(0.0, f64::NAN, 0.0, 0.0);
        assert!(matches!(
            terms.weighted_priority(&SelectionConfig::default()),
            Err(PolicyError::NonFiniteValue {
                field: "priority.novelty",
                value,
            }) if value.is_nan()
        ));

        let mut selection = SelectionConfig::default();
        selection.policy = PolicyKind::Staged;
        selection.staged.inner = PolicyKind::Staged;
        assert_eq!(
            validate_selection_weights(&selection),
            Err(PolicyError::InvalidConfig {
                field: "selection.staged.inner",
            })
        );

        let mut selection = SelectionConfig::default();
        selection.max_visits_per_node = 0;
        assert_eq!(
            validate_selection_weights(&selection),
            Err(PolicyError::InvalidConfig {
                field: "selection.max_visits_per_node",
            })
        );
    }

    #[test]
    fn policy_common_orders_candidates_stably_without_hash_iteration_dependence() {
        let (tree, ids) = sample_tree();
        let selection = SelectionConfig::default();
        let plateau = plateau_knobs();

        let mut frontier_a = Frontier::new();
        for id in [ids.high, ids.grandchild, ids.low, ids.mid] {
            frontier_a.insert(id).unwrap();
        }
        let mut frontier_b = Frontier::new();
        for id in [ids.mid, ids.low, ids.high, ids.grandchild] {
            frontier_b.insert(id).unwrap();
        }

        let mut mirror_a = CellMirror::new();
        mirror_a.bump_by(cell_for(ids.low), 4);
        mirror_a.bump_by(cell_for(ids.mid), 1);
        mirror_a.bump_by(cell_for(ids.high), 9);
        mirror_a.bump_by(cell_for(ids.grandchild), 16);

        let mut mirror_b = CellMirror::new();
        mirror_b.bump_by(cell_for(ids.grandchild), 16);
        mirror_b.bump_by(cell_for(ids.high), 9);
        mirror_b.bump_by(cell_for(ids.mid), 1);
        mirror_b.bump_by(cell_for(ids.low), 4);

        let context_a = PolicyContext::new(&tree, &frontier_a, &mirror_a, &plateau, &selection);
        let context_b = PolicyContext::new(&tree, &frontier_b, &mirror_b, &plateau, &selection);

        let snapshots_a = candidate_snapshots(&context_a).unwrap();
        let snapshots_b = candidate_snapshots(&context_b).unwrap();
        assert_eq!(snapshots_a, snapshots_b);
        assert_eq!(
            snapshots_a
                .iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            vec![ids.low, ids.mid, ids.high, ids.grandchild]
        );
    }

    #[test]
    fn policy_common_models_novelty_visit_and_depth_penalties() {
        let (mut tree, ids) = sample_tree();
        tree.increment_visits(ids.grandchild).unwrap();
        tree.increment_visits(ids.grandchild).unwrap();
        tree.increment_visits(ids.grandchild).unwrap();

        let mut frontier = Frontier::new();
        frontier.insert(ids.grandchild).unwrap();

        let mut mirror = CellMirror::new();
        mirror.bump_by(cell_for(ids.grandchild), 8);

        let selection = SelectionConfig::default();
        let plateau = plateau_knobs();
        let context = PolicyContext::new(&tree, &frontier, &mirror, &plateau, &selection);
        let snapshots = candidate_snapshots(&context).unwrap();

        assert_eq!(snapshots.len(), 1);
        let terms = snapshots[0].priority_terms;
        assert_eq!(terms.normalized_score, 0.0);
        assert_eq!(terms.novelty, 1.0 / 3.0);
        assert_eq!(terms.visit_penalty, libm::log(4.0));
        assert_eq!(terms.depth_penalty, libm::log(3.0));
    }

    #[derive(Clone, Copy)]
    struct SampleIds {
        low: NodeId,
        mid: NodeId,
        high: NodeId,
        grandchild: NodeId,
    }

    fn sample_tree() -> (Tree, SampleIds) {
        let mut tree = Tree::from_root(payload(0, 0.0));
        let low = tree.insert_child(NodeId::ROOT, payload(1, 10.0)).unwrap();
        let mid = tree.insert_child(NodeId::ROOT, payload(2, 20.0)).unwrap();
        let high = tree.insert_child(NodeId::ROOT, payload(3, 30.0)).unwrap();
        let grandchild = tree.insert_child(low, payload(4, 40.0)).unwrap();
        (
            tree,
            SampleIds {
                low,
                mid,
                high,
                grandchild,
            },
        )
    }

    fn sample_mirror() -> CellMirror {
        let mut mirror = CellMirror::new();
        mirror.bump_by(CellKey::new(1), 4);
        mirror.bump_by(CellKey::new(2), 1);
        mirror.bump_by(CellKey::new(3), 9);
        mirror.bump_by(CellKey::new(4), 16);
        mirror
    }

    fn cell_for(id: NodeId) -> CellKey {
        CellKey::new(id.get())
    }

    fn plateau_knobs() -> PlateauKnobs {
        PlateauKnobs::new(
            10,
            0.01,
            EscalationKnobs::new(1.5, 1.75, 0.5, 1.0, 0.5, 2.0, EscalationLevel::L4),
        )
    }

    fn payload(seed: u8, score: f64) -> NodePayload {
        NodePayload::new(
            SnapshotRef::new([seed; 32]),
            crate::types::Score::new(score).unwrap(),
            Novelty::new(1.0).unwrap(),
            CellKey::new(u64::from(seed)),
            StateHash::new([seed.wrapping_add(1); 32]),
            Stage::new(u16::from(seed)),
            FrameCount::new(u32::from(seed) * 10),
        )
    }
}
