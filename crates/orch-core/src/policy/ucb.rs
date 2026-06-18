//! Deterministic UCB selection policy.

use super::{
    CandidateSnapshot, PolicyContext, PolicyError, PolicyKind, PolicyResult, SelectionChoice,
    SelectionPolicy,
};
use crate::rng::DeterministicRng;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UcbPolicy {
    total_expansions: u64,
}

impl UcbPolicy {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            total_expansions: 0,
        }
    }

    #[must_use]
    pub const fn with_total_expansions(total_expansions: u64) -> Self {
        Self { total_expansions }
    }

    #[must_use]
    pub const fn total_expansions(&self) -> u64 {
        self.total_expansions
    }

    pub fn set_total_expansions(&mut self, total_expansions: u64) {
        self.total_expansions = total_expansions;
    }
}

impl SelectionPolicy for UcbPolicy {
    fn kind(&self) -> PolicyKind {
        PolicyKind::Ucb
    }

    fn select(
        &mut self,
        context: &PolicyContext<'_>,
        _rng: &mut DeterministicRng,
    ) -> PolicyResult<SelectionChoice> {
        let candidates = context.candidate_snapshots()?;
        select_from_candidates(&candidates, self.total_expansions, context.selection.ucb_c)
    }
}

pub(crate) fn select_from_candidates(
    candidates: &[CandidateSnapshot],
    total_expansions: u64,
    ucb_c: f64,
) -> PolicyResult<SelectionChoice> {
    let first = candidates.first().ok_or(PolicyError::EmptyCandidateSet)?;
    let mut best = (0usize, first, ucb_value(first, total_expansions, ucb_c)?);

    for (index, candidate) in candidates.iter().enumerate().skip(1) {
        let value = ucb_value(candidate, total_expansions, ucb_c)?;
        if value > best.2 || (value == best.2 && candidate.id < best.1.id) {
            best = (index, candidate, value);
        }
    }

    Ok(SelectionChoice {
        selected: best.1.id,
        candidate_index: best.0,
    })
}

pub(crate) fn ucb_value(
    candidate: &CandidateSnapshot,
    total_expansions: u64,
    ucb_c: f64,
) -> PolicyResult<f64> {
    validate_ucb_c(ucb_c)?;
    ensure_finite("candidate.priority", candidate.priority)?;

    let effective_total = total_expansions.max(2) as f64;
    let visit_denominator = u64::from(candidate.visits).saturating_add(1) as f64;
    let exploration = ucb_c * libm::sqrt(libm::log(effective_total) / visit_denominator);
    ensure_finite("ucb.exploration", exploration)?;

    let value = candidate.priority + exploration;
    ensure_finite("ucb.value", value)?;
    Ok(value)
}

fn validate_ucb_c(ucb_c: f64) -> PolicyResult<()> {
    if !ucb_c.is_finite() || ucb_c < 0.0 {
        Err(PolicyError::InvalidWeight {
            field: "selection.ucb_c",
            value: ucb_c,
        })
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
    use crate::plateau::{EscalationKnobs, EscalationLevel, PlateauKnobs};
    use crate::tree::{NodePayload, Tree};
    use crate::types::{
        CellKey, FrameCount, NodeId, Novelty, Score, SelectionConfig, SnapshotRef, Stage, StateHash,
    };

    #[test]
    fn ucb_hand_computed_fixture_matches() {
        let candidate = candidate(7, 0.25, 3);
        let total_expansions = 10;
        let ucb_c = 1.4;

        let actual = ucb_value(&candidate, total_expansions, ucb_c).unwrap();
        let expected = 0.25 + 1.4 * libm::sqrt(libm::log(10.0) / 4.0);

        assert_eq!(actual, expected);
    }

    #[test]
    fn ucb_ties_choose_smallest_node_id() {
        let candidates = [candidate(2, 2.0, 4), candidate(1, 2.0, 4)];

        let choice = select_from_candidates(&candidates, 64, 1.25).unwrap();

        assert_eq!(choice.selected, NodeId::new(1));
        assert_eq!(choice.candidate_index, 1);
    }

    #[test]
    fn ucb_unvisited_nodes_stay_finite_and_favored() {
        let visited = candidate(1, 1.0, 32);
        let unvisited = candidate(2, 1.0, 0);

        let visited_value = ucb_value(&visited, 100, 1.0).unwrap();
        let unvisited_value = ucb_value(&unvisited, 100, 1.0).unwrap();
        let choice = select_from_candidates(&[visited, unvisited], 100, 1.0).unwrap();

        assert!(visited_value.is_finite());
        assert!(unvisited_value.is_finite());
        assert!(unvisited_value > visited_value);
        assert_eq!(choice.selected, NodeId::new(2));
    }

    #[test]
    fn ucb_policy_consumes_zero_rng_draws() {
        let (tree, ids) = sample_tree();
        let mut frontier = Frontier::new();
        for id in [ids[2], ids[0], ids[1]] {
            frontier.insert(id).unwrap();
        }
        let mirror = CellMirror::new();
        let plateau = plateau_knobs();
        let selection = SelectionConfig::default();
        let context = PolicyContext::new(&tree, &frontier, &mirror, &plateau, &selection);
        let mut rng = DeterministicRng::selection(123, 0);
        let mut policy = UcbPolicy::with_total_expansions(128);

        let before = rng.draw_count();
        let choice = policy.select(&context, &mut rng).unwrap();

        assert_eq!(policy.kind(), PolicyKind::Ucb);
        assert_eq!(choice.selected, ids[2]);
        assert_eq!(rng.draw_count(), before);
        assert_eq!(policy.total_expansions(), 128);
    }

    #[test]
    fn ucb_rejects_invalid_weights_and_non_finite_priority() {
        let valid_candidate = candidate(1, 1.0, 0);
        assert_eq!(
            ucb_value(&valid_candidate, 10, f64::INFINITY),
            Err(PolicyError::InvalidWeight {
                field: "selection.ucb_c",
                value: f64::INFINITY,
            })
        );

        let candidate = candidate(1, f64::NAN, 0);
        assert!(matches!(
            ucb_value(&candidate, 10, 1.0),
            Err(PolicyError::NonFiniteValue {
                field: "candidate.priority",
                value,
            }) if value.is_nan()
        ));
    }

    fn candidate(id: u64, priority: f64, visits: u32) -> CandidateSnapshot {
        CandidateSnapshot {
            id: NodeId::new(id),
            parent: None,
            depth: 0,
            visits,
            children: 0,
            cell: CellKey::new(id),
            stage: Stage::new(0),
            raw_score: priority,
            priority_terms: super::super::PriorityTerms::new(priority, 0.0, 0.0, 0.0),
            priority,
        }
    }

    fn sample_tree() -> (Tree, [NodeId; 3]) {
        let mut tree = Tree::from_root(payload(0, 0.0));
        let first = tree.insert_child(NodeId::ROOT, payload(1, 10.0)).unwrap();
        let second = tree.insert_child(NodeId::ROOT, payload(2, 20.0)).unwrap();
        let third = tree.insert_child(NodeId::ROOT, payload(3, 30.0)).unwrap();
        (tree, [first, second, third])
    }

    fn payload(seed: u8, score: f64) -> NodePayload {
        NodePayload::new(
            SnapshotRef::new([seed; 32]),
            Score::new(score).unwrap(),
            Novelty::new(1.0).unwrap(),
            CellKey::new(u64::from(seed)),
            StateHash::new([seed.wrapping_add(1); 32]),
            Stage::new(u16::from(seed)),
            FrameCount::new(u32::from(seed) * 10),
        )
    }

    fn plateau_knobs() -> PlateauKnobs {
        PlateauKnobs::new(
            10,
            0.01,
            EscalationKnobs::new(1.5, 1.75, 0.5, 1.0, 0.5, 2.0, EscalationLevel::L4),
        )
    }
}
