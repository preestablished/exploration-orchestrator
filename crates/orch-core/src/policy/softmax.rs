//! Deterministic softmax selection policy.

use super::{
    CandidateSnapshot, PolicyContext, PolicyError, PolicyKind, PolicyResult, SelectionChoice,
    SelectionPolicy,
};
use crate::rng::DeterministicRng;

pub const ARGMAX_TEMPERATURE: f64 = 1.0e-12;

#[derive(Clone, Copy, Debug, Default)]
pub struct SoftmaxPolicy;

impl SoftmaxPolicy {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SelectionPolicy for SoftmaxPolicy {
    fn kind(&self) -> PolicyKind {
        PolicyKind::Softmax
    }

    fn select(
        &mut self,
        context: &PolicyContext<'_>,
        rng: &mut DeterministicRng,
    ) -> PolicyResult<SelectionChoice> {
        let candidates = context.candidate_snapshots()?;
        select_from_candidates(&candidates, context.selection.temperature, rng)
    }
}

pub fn select_from_candidates(
    candidates: &[CandidateSnapshot],
    temperature: f64,
    rng: &mut DeterministicRng,
) -> PolicyResult<SelectionChoice> {
    validate_temperature(temperature)?;
    validate_priorities(candidates)?;

    if temperature <= ARGMAX_TEMPERATURE {
        return argmax_choice(candidates);
    }

    let max_priority = candidates
        .iter()
        .map(|candidate| candidate.priority)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut weights = Vec::with_capacity(candidates.len());
    let mut total_weight = 0.0;

    for candidate in candidates {
        let exponent = (candidate.priority - max_priority) / temperature;
        let weight = libm::exp(exponent);
        ensure_finite("softmax.weight", weight)?;
        weights.push(weight);
        total_weight += weight;
        ensure_finite("softmax.total_weight", total_weight)?;
    }

    if total_weight <= 0.0 {
        return Err(PolicyError::NonFiniteValue {
            field: "softmax.total_weight",
            value: total_weight,
        });
    }

    let target = rng.next_unit_f64() * total_weight;
    let mut prefix = 0.0;
    for (index, weight) in weights.iter().enumerate() {
        prefix += *weight;
        if target < prefix || index + 1 == candidates.len() {
            return Ok(SelectionChoice {
                selected: candidates[index].id,
                candidate_index: index,
            });
        }
    }

    unreachable!("non-empty candidates return during prefix walk")
}

fn argmax_choice(candidates: &[CandidateSnapshot]) -> PolicyResult<SelectionChoice> {
    let mut best = candidates
        .first()
        .ok_or(PolicyError::EmptyCandidateSet)
        .map(|candidate| (0usize, candidate))?;
    for (index, candidate) in candidates.iter().enumerate().skip(1) {
        if candidate.priority > best.1.priority {
            best = (index, candidate);
        }
    }

    Ok(SelectionChoice {
        selected: best.1.id,
        candidate_index: best.0,
    })
}

fn validate_temperature(temperature: f64) -> PolicyResult<()> {
    if !temperature.is_finite() || temperature <= 0.0 {
        Err(PolicyError::InvalidWeight {
            field: "selection.temperature",
            value: temperature,
        })
    } else {
        Ok(())
    }
}

fn validate_priorities(candidates: &[CandidateSnapshot]) -> PolicyResult<()> {
    if candidates.is_empty() {
        return Err(PolicyError::EmptyCandidateSet);
    }

    for candidate in candidates {
        ensure_finite("candidate.priority", candidate.priority)?;
    }

    Ok(())
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
    fn softmax_near_zero_temperature_selects_argmax_without_rng_draw() {
        let candidates = [
            candidate(1, 1.0),
            candidate(2, 4.0),
            candidate(3, 4.0),
            candidate(4, 0.5),
        ];
        let mut rng = DeterministicRng::selection(123, 0);

        let choice = select_from_candidates(&candidates, ARGMAX_TEMPERATURE, &mut rng).unwrap();

        assert_eq!(choice.selected, NodeId::new(2));
        assert_eq!(choice.candidate_index, 1);
        assert_eq!(rng.draw_count(), 0);
    }

    #[test]
    fn softmax_stochastic_selection_uses_exactly_one_rng_draw() {
        let candidates = [candidate(1, 0.0), candidate(2, 1.0), candidate(3, 2.0)];
        let mut rng = DeterministicRng::selection(123, 0);

        let before = rng.draw_count();
        let choice = select_from_candidates(&candidates, 1.0, &mut rng).unwrap();

        assert!(candidates
            .iter()
            .any(|candidate| candidate.id == choice.selected));
        assert_eq!(rng.draw_count() - before, 1);
    }

    #[test]
    fn softmax_sampling_matches_expected_distribution_with_chi_squared_tolerance() {
        let candidates = [
            candidate(1, libm::log(1.0)),
            candidate(2, libm::log(2.0)),
            candidate(3, libm::log(3.0)),
        ];
        let expected = [1.0 / 6.0, 2.0 / 6.0, 3.0 / 6.0];
        let draws = 100_000usize;
        let mut counts = [0usize; 3];
        let mut rng = DeterministicRng::selection(0xfeed_cafe, 19);

        for _ in 0..draws {
            let choice = select_from_candidates(&candidates, 1.0, &mut rng).unwrap();
            counts[choice.candidate_index] += 1;
        }

        assert_eq!(rng.draw_count(), draws as u64);
        let chi_squared = counts
            .iter()
            .zip(expected)
            .map(|(actual, probability)| {
                let expected_count = probability * draws as f64;
                let delta = *actual as f64 - expected_count;
                delta * delta / expected_count
            })
            .sum::<f64>();
        assert!(
            chi_squared < 20.0,
            "counts={counts:?} chi_squared={chi_squared}"
        );
    }

    #[test]
    fn softmax_policy_uses_stable_node_id_candidate_ordering() {
        let (tree, ids) = sample_tree();
        let mut frontier = Frontier::new();
        for id in [ids[2], ids[0], ids[1]] {
            frontier.insert(id).unwrap();
        }
        let mirror = CellMirror::new();
        let plateau = plateau_knobs();
        let mut selection = SelectionConfig::default();
        selection.temperature = ARGMAX_TEMPERATURE;
        let context = PolicyContext::new(&tree, &frontier, &mirror, &plateau, &selection);
        let mut rng = DeterministicRng::selection(123, 0);
        let mut policy = SoftmaxPolicy::new();

        let choice = policy.select(&context, &mut rng).unwrap();

        assert_eq!(policy.kind(), PolicyKind::Softmax);
        assert_eq!(choice.selected, ids[0]);
        assert_eq!(choice.candidate_index, 0);
        assert_eq!(rng.draw_count(), 0);
    }

    #[test]
    fn softmax_rejects_invalid_temperature_and_non_finite_priority() {
        let candidates = [candidate(1, 0.0), candidate(2, 1.0)];
        let mut rng = DeterministicRng::selection(123, 0);

        assert_eq!(
            select_from_candidates(&candidates, 0.0, &mut rng),
            Err(PolicyError::InvalidWeight {
                field: "selection.temperature",
                value: 0.0,
            })
        );

        let candidates = [candidate(1, 0.0), candidate(2, f64::INFINITY)];
        assert_eq!(
            select_from_candidates(&candidates, 1.0, &mut rng),
            Err(PolicyError::NonFiniteValue {
                field: "candidate.priority",
                value: f64::INFINITY,
            })
        );
        assert_eq!(rng.draw_count(), 0);
    }

    fn candidate(id: u64, priority: f64) -> CandidateSnapshot {
        CandidateSnapshot {
            id: NodeId::new(id),
            parent: None,
            depth: 0,
            visits: 0,
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
        let second = tree.insert_child(NodeId::ROOT, payload(2, 10.0)).unwrap();
        let third = tree.insert_child(NodeId::ROOT, payload(3, 10.0)).unwrap();
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
