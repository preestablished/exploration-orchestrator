use orch_core::commit::{commit_batch, CommitRules, CommitState, ScoredChild};
use orch_core::policy::{validate_selection_weights, PolicyError, PriorityTerms, SelectionConfig};
use orch_core::tree::NodePayload;
use orch_core::types::{
    CellKey, FrameCount, NodeId, Novelty, PolicyKind, PruneAction, Score, SnapshotRef, Stage,
    StagedConfig, StateHash,
};
use proptest::prelude::*;

proptest! {
    #[test]
    fn core_validation_commit_sequences_never_leave_exhausted_nodes_in_frontier(
        ops in prop::collection::vec((0u8..5, 0u8..8, -5i16..8), 1..128)
    ) {
        let mut state = CommitState::from_root(payload(0, 10.0, 0, 0));
        let rules = CommitRules::new(PruneAction::Exhausted, 0.01, 3, 2);
        let mut next_seed = 1u8;

        for (kind, parent_pick, score_delta) in ops {
            let candidates = state.frontier.deterministic_candidates(&state.tree).unwrap();
            if candidates.is_empty() {
                break;
            }
            let parent = candidates[usize::from(parent_pick) % candidates.len()];
            let parent_score = state.tree.get(parent).unwrap().score.get();
            let child_score = parent_score + f64::from(score_delta) * 0.1;
            let child = match kind {
                0 => ScoredChild::new(payload(next_seed, child_score, u64::from(next_seed), next_seed)),
                1 => ScoredChild::new(payload(0, child_score, 0, next_seed)).duplicate(),
                2 => ScoredChild::new(payload(next_seed, child_score, u64::from(next_seed), next_seed)).prune(),
                3 => ScoredChild::new(payload(next_seed, child_score, u64::from(next_seed), next_seed)).goal(),
                _ => ScoredChild::new(payload(next_seed, parent_score - 1.0, 0, next_seed)),
            };
            next_seed = next_seed.wrapping_add(1);

            commit_batch(&mut state, parent, &[child], &rules).unwrap();
            assert_frontier_invariants(&state)?;
        }
    }

    #[test]
    fn core_validation_weighted_priorities_stay_finite_for_bounded_finite_inputs(
        normalized_score in -1.0f64..2.0,
        novelty in 0.0f64..2.0,
        visit_penalty in 0.0f64..32.0,
        depth_penalty in 0.0f64..32.0,
        alpha in 0.0f64..10.0,
        beta in 0.0f64..10.0,
        gamma in 0.0f64..10.0,
        delta in 0.0f64..10.0,
    ) {
        let selection = SelectionConfig {
            alpha,
            beta,
            gamma,
            delta,
            ..Default::default()
        };
        validate_selection_weights(&selection).unwrap();

        let priority = PriorityTerms::new(
            normalized_score,
            novelty,
            visit_penalty,
            depth_penalty,
        )
        .weighted_priority(&selection)
        .unwrap();

        prop_assert!(priority.is_finite());
    }
}

#[test]
fn core_validation_selection_rejects_non_finite_weights() {
    for (field, mutate) in [
        (
            "selection.alpha",
            set_alpha as fn(&mut SelectionConfig, f64),
        ),
        ("selection.beta", set_beta),
        ("selection.gamma", set_gamma),
        ("selection.delta", set_delta),
        ("selection.ucb_c", set_ucb_c),
    ] {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut selection = SelectionConfig::default();
            mutate(&mut selection, value);
            assert!(matches!(
                validate_selection_weights(&selection),
                Err(PolicyError::InvalidWeight { field: actual, value: rejected })
                    if actual == field && same_float_bits_or_nan(rejected, value)
            ));
        }
    }

    let selection = SelectionConfig {
        temperature: f64::NAN,
        ..Default::default()
    };
    assert!(matches!(
        validate_selection_weights(&selection),
        Err(PolicyError::InvalidWeight { field: "selection.temperature", value }) if value.is_nan()
    ));

    let mut selection = SelectionConfig::default();
    selection.staged.epsilon_regress = f64::INFINITY;
    assert_eq!(
        validate_selection_weights(&selection),
        Err(PolicyError::InvalidWeight {
            field: "selection.staged.epsilon_regress",
            value: f64::INFINITY,
        })
    );

    let selection = SelectionConfig {
        policy: PolicyKind::Staged,
        staged: StagedConfig {
            inner: PolicyKind::Staged,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        validate_selection_weights(&selection),
        Err(PolicyError::InvalidConfig {
            field: "selection.staged.inner",
        })
    );
}

#[test]
fn core_validation_libm_ln_exp_byte_vectors_are_pinned() {
    // These vectors pin pure-Rust libm output locally. The follow-up cross-arch
    // CI bead should run this same test on x86_64 and aarch64 runners.
    const LN_VECTORS: &[(f64, u64)] = &[
        (0.25, 0xbff6_2e42_fefa_39ef),
        (0.5, 0xbfe6_2e42_fefa_39ef),
        (1.0, 0x0000_0000_0000_0000),
        (2.0, 0x3fe6_2e42_fefa_39ef),
        (10.0, 0x4002_6bb1_bbb5_5516),
    ];
    const EXP_VECTORS: &[(f64, u64)] = &[
        (-2.0, 0x3fc1_52aa_a3bf_81cc),
        (-1.0, 0x3fd7_8b56_362c_ef38),
        (0.0, 0x3ff0_0000_0000_0000),
        (1.0, 0x4005_bf0a_8b14_576a),
        (2.0, 0x401d_8e64_b8d4_ddae),
    ];

    for (input, expected_bits) in LN_VECTORS {
        assert_eq!(libm::log(*input).to_bits(), *expected_bits, "ln({input})");
    }
    for (input, expected_bits) in EXP_VECTORS {
        assert_eq!(libm::exp(*input).to_bits(), *expected_bits, "exp({input})");
    }
}

fn assert_frontier_invariants(state: &CommitState) -> Result<(), TestCaseError> {
    for raw_id in 0..state.tree.next_id().get() {
        let id = NodeId::new(raw_id);
        let record = state.tree.get(id).unwrap();
        if record.exhausted {
            prop_assert!(!state.frontier.contains(id));
        }
    }

    for id in state.frontier.entries() {
        let record = state.tree.get(*id).unwrap();
        prop_assert!(!record.exhausted);
    }

    for id in state
        .frontier
        .deterministic_candidates(&state.tree)
        .unwrap()
    {
        let record = state.tree.get(id).unwrap();
        prop_assert!(state.frontier.contains(id));
        prop_assert!(record.is_frontier_candidate());
    }

    Ok(())
}

fn payload(seed: u8, score: f64, cell: u64, stage: u8) -> NodePayload {
    NodePayload::new(
        SnapshotRef::new([seed; 32]),
        Score::new(score).unwrap(),
        Novelty::new(1.0).unwrap(),
        CellKey::new(cell),
        StateHash::new([seed.wrapping_add(1); 32]),
        Stage::new(u16::from(stage)),
        FrameCount::new(u32::from(seed) * 10),
    )
}

fn same_float_bits_or_nan(actual: f64, expected: f64) -> bool {
    actual.to_bits() == expected.to_bits() || actual.is_nan() && expected.is_nan()
}

fn set_alpha(selection: &mut SelectionConfig, value: f64) {
    selection.alpha = value;
}

fn set_beta(selection: &mut SelectionConfig, value: f64) {
    selection.beta = value;
}

fn set_gamma(selection: &mut SelectionConfig, value: f64) {
    selection.gamma = value;
}

fn set_delta(selection: &mut SelectionConfig, value: f64) {
    selection.delta = value;
}

fn set_ucb_c(selection: &mut SelectionConfig, value: f64) {
    selection.ucb_c = value;
}
