//! Curriculum-stage policy dispatch with deterministic fallback sets.

use super::{
    softmax, ucb, CandidateSnapshot, PolicyContext, PolicyError, PolicyKind, PolicyResult,
    SelectionChoice, SelectionPolicy,
};
use crate::rng::DeterministicRng;
use crate::types::Stage;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StagedPolicy {
    total_expansions: u64,
}

impl StagedPolicy {
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

impl SelectionPolicy for StagedPolicy {
    fn kind(&self) -> PolicyKind {
        PolicyKind::Staged
    }

    fn select(
        &mut self,
        context: &PolicyContext<'_>,
        rng: &mut DeterministicRng,
    ) -> PolicyResult<SelectionChoice> {
        let candidates = context.candidate_snapshots()?;
        select_from_candidates(
            &candidates,
            context.selection.staged.inner,
            context.selection.staged.epsilon_regress,
            context.selection.temperature,
            context.selection.ucb_c,
            self.total_expansions,
            rng,
        )
    }
}

pub(crate) fn select_from_candidates(
    candidates: &[CandidateSnapshot],
    inner: PolicyKind,
    epsilon_regress: f64,
    temperature: f64,
    ucb_c: f64,
    total_expansions: u64,
    rng: &mut DeterministicRng,
) -> PolicyResult<SelectionChoice> {
    validate_staged_config(inner, epsilon_regress)?;
    let target_stage = target_stage(candidates)?;
    let partition = partition_candidates(candidates, target_stage);
    let regress_mix = rng.next_unit_f64() < epsilon_regress;
    let selected = match (
        regress_mix,
        partition.regress.is_empty(),
        partition.leading.is_empty(),
    ) {
        (true, false, _) => &partition.regress,
        (true, true, false) => &partition.leading,
        (false, _, false) => &partition.leading,
        (false, false, true) => &partition.regress,
        (_, true, true) => return Err(PolicyError::EmptyCandidateSet),
    };
    let selected_candidates = selected
        .iter()
        .map(|entry| entry.candidate.clone())
        .collect::<Vec<_>>();
    let inner_choice = dispatch_inner(
        &selected_candidates,
        inner,
        temperature,
        ucb_c,
        total_expansions,
        rng,
    )?;
    let original = selected[inner_choice.candidate_index];

    Ok(SelectionChoice {
        selected: inner_choice.selected,
        candidate_index: original.original_index,
    })
}

pub(crate) fn target_stage(candidates: &[CandidateSnapshot]) -> PolicyResult<Stage> {
    candidates
        .iter()
        .map(|candidate| candidate.stage)
        .max()
        .ok_or(PolicyError::EmptyCandidateSet)
}

fn validate_staged_config(inner: PolicyKind, epsilon_regress: f64) -> PolicyResult<()> {
    if inner == PolicyKind::Staged {
        return Err(PolicyError::InvalidConfig {
            field: "selection.staged.inner",
        });
    }
    if !epsilon_regress.is_finite() || !(0.0..=1.0).contains(&epsilon_regress) {
        return Err(PolicyError::InvalidWeight {
            field: "selection.staged.epsilon_regress",
            value: epsilon_regress,
        });
    }
    Ok(())
}

fn dispatch_inner(
    candidates: &[CandidateSnapshot],
    inner: PolicyKind,
    temperature: f64,
    ucb_c: f64,
    total_expansions: u64,
    rng: &mut DeterministicRng,
) -> PolicyResult<SelectionChoice> {
    match inner {
        PolicyKind::Softmax => softmax::select_from_candidates(candidates, temperature, rng),
        PolicyKind::Ucb => ucb::select_from_candidates(candidates, total_expansions, ucb_c),
        PolicyKind::Staged => Err(PolicyError::InvalidConfig {
            field: "selection.staged.inner",
        }),
    }
}

#[derive(Clone, Debug)]
struct PartitionedCandidates<'a> {
    leading: Vec<PartitionEntry<'a>>,
    regress: Vec<PartitionEntry<'a>>,
}

#[derive(Clone, Copy, Debug)]
struct PartitionEntry<'a> {
    original_index: usize,
    candidate: &'a CandidateSnapshot,
}

fn partition_candidates(
    candidates: &[CandidateSnapshot],
    target_stage: Stage,
) -> PartitionedCandidates<'_> {
    let mut leading = Vec::new();
    let mut regress = Vec::new();

    for (original_index, candidate) in candidates.iter().enumerate() {
        let entry = PartitionEntry {
            original_index,
            candidate,
        };
        if candidate.stage == target_stage {
            leading.push(entry);
        } else {
            regress.push(entry);
        }
    }

    PartitionedCandidates { leading, regress }
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
    fn staged_target_stage_gating_prefers_leading_edge() {
        let candidates = [
            candidate(1, 100.0, 1),
            candidate(2, 1.0, 2),
            candidate(3, 2.0, 2),
        ];
        let mut rng = DeterministicRng::selection(123, 0);

        let choice =
            select_from_candidates(&candidates, PolicyKind::Ucb, 0.0, 1.0, 0.0, 64, &mut rng)
                .unwrap();

        assert_eq!(target_stage(&candidates).unwrap(), Stage::new(2));
        assert_eq!(choice.selected, NodeId::new(3));
        assert_eq!(choice.candidate_index, 2);
        assert_eq!(rng.draw_count(), 1);
    }

    #[test]
    fn staged_epsilon_regress_mix_can_choose_lower_stage_candidates() {
        let candidates = [
            candidate(1, 100.0, 1),
            candidate(2, 1.0, 2),
            candidate(3, 2.0, 2),
        ];
        let mut rng = DeterministicRng::selection(123, 0);

        let choice =
            select_from_candidates(&candidates, PolicyKind::Ucb, 1.0, 1.0, 0.0, 64, &mut rng)
                .unwrap();

        assert_eq!(choice.selected, NodeId::new(1));
        assert_eq!(choice.candidate_index, 0);
        assert_eq!(rng.draw_count(), 1);
    }

    #[test]
    fn staged_empty_regress_set_falls_back_to_leading_edge() {
        let candidates = [candidate(1, 1.0, 2), candidate(2, 2.0, 2)];
        let mut rng = DeterministicRng::selection(123, 0);

        let choice =
            select_from_candidates(&candidates, PolicyKind::Ucb, 1.0, 1.0, 0.0, 64, &mut rng)
                .unwrap();

        assert_eq!(choice.selected, NodeId::new(2));
        assert_eq!(choice.candidate_index, 1);
        assert_eq!(rng.draw_count(), 1);
    }

    #[test]
    fn staged_rejects_invalid_staged_inner_policy() {
        let candidates = [candidate(1, 1.0, 1)];
        let mut rng = DeterministicRng::selection(123, 0);

        assert_eq!(
            select_from_candidates(
                &candidates,
                PolicyKind::Staged,
                0.05,
                1.0,
                1.0,
                64,
                &mut rng
            ),
            Err(PolicyError::InvalidConfig {
                field: "selection.staged.inner",
            })
        );
        assert_eq!(rng.draw_count(), 0);
    }

    #[test]
    fn staged_preserves_deterministic_ordering_and_original_index() {
        let (tree, ids) = sample_tree();
        let mut frontier = Frontier::new();
        for id in [ids[2], ids[0], ids[1]] {
            frontier.insert(id).unwrap();
        }
        let mirror = CellMirror::new();
        let plateau = plateau_knobs();
        let mut selection = SelectionConfig::default();
        selection.policy = PolicyKind::Staged;
        selection.staged.inner = PolicyKind::Ucb;
        selection.staged.epsilon_regress = 0.0;
        selection.ucb_c = 0.0;
        let context = PolicyContext::new(&tree, &frontier, &mirror, &plateau, &selection);
        let mut rng = DeterministicRng::selection(123, 0);
        let mut policy = StagedPolicy::with_total_expansions(64);

        let choice = policy.select(&context, &mut rng).unwrap();

        assert_eq!(policy.kind(), PolicyKind::Staged);
        assert_eq!(choice.selected, ids[1]);
        assert_eq!(choice.candidate_index, 1);
        assert_eq!(rng.draw_count(), 1);
        assert_eq!(policy.total_expansions(), 64);
    }

    #[test]
    fn staged_rng_accounting_includes_mix_draw_then_inner_draws() {
        let candidates = [candidate(1, 1.0, 1), candidate(2, 2.0, 2)];
        let mut rng = DeterministicRng::selection(123, 0);

        let _choice = select_from_candidates(
            &candidates,
            PolicyKind::Softmax,
            0.0,
            1.0,
            1.0,
            64,
            &mut rng,
        )
        .unwrap();

        assert_eq!(rng.draw_count(), 2);
    }

    fn candidate(id: u64, priority: f64, stage: u16) -> CandidateSnapshot {
        CandidateSnapshot {
            id: NodeId::new(id),
            parent: None,
            depth: 0,
            visits: 0,
            children: 0,
            cell: CellKey::new(id),
            stage: Stage::new(stage),
            raw_score: priority,
            priority_terms: super::super::PriorityTerms::new(priority, 0.0, 0.0, 0.0),
            priority,
        }
    }

    fn sample_tree() -> (Tree, [NodeId; 3]) {
        let mut tree = Tree::from_root(payload(0, 0.0, 0));
        let first = tree
            .insert_child(NodeId::ROOT, payload(1, 100.0, 1))
            .unwrap();
        let second = tree
            .insert_child(NodeId::ROOT, payload(2, 10.0, 2))
            .unwrap();
        let third = tree.insert_child(NodeId::ROOT, payload(3, 5.0, 2)).unwrap();
        (tree, [first, second, third])
    }

    fn payload(seed: u8, score: f64, stage: u16) -> NodePayload {
        NodePayload::new(
            SnapshotRef::new([seed; 32]),
            Score::new(score).unwrap(),
            Novelty::new(1.0).unwrap(),
            CellKey::new(u64::from(seed)),
            StateHash::new([seed.wrapping_add(1); 32]),
            Stage::new(stage),
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
