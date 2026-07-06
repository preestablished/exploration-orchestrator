//! Pure commit-rule decisions for scored child candidates.

use core::fmt;

use crate::frontier::{Frontier, FrontierError};
use crate::mirror::{CellMirror, SeenMap};
use crate::tree::{NodePayload, Tree, TreeError};
use crate::types::{
    CommitDisposition, DiscardReason, ExperimentConfig, FrontierEvictReason, NodeId, PruneAction,
};

#[derive(Clone, Debug, PartialEq)]
pub struct CommitRules {
    pub prune_action: PruneAction,
    pub epsilon_keep: f64,
    pub max_visits_per_node: u32,
    pub exhaust_after_dup_expansions: u32,
}

impl CommitRules {
    pub const fn new(
        prune_action: PruneAction,
        epsilon_keep: f64,
        max_visits_per_node: u32,
        exhaust_after_dup_expansions: u32,
    ) -> Self {
        Self {
            prune_action,
            epsilon_keep,
            max_visits_per_node,
            exhaust_after_dup_expansions,
        }
    }

    pub fn from_config(config: &ExperimentConfig) -> Self {
        Self {
            prune_action: config.prune_action,
            epsilon_keep: config.plateau.epsilon_s,
            max_visits_per_node: config.selection.max_visits_per_node,
            exhaust_after_dup_expansions: config.selection.exhaust_after_dup_expansions,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoredChild {
    pub payload: NodePayload,
    pub duplicate: bool,
    pub prune: bool,
    pub goal: bool,
}

impl ScoredChild {
    pub const fn new(payload: NodePayload) -> Self {
        Self {
            payload,
            duplicate: false,
            prune: false,
            goal: false,
        }
    }

    pub const fn duplicate(mut self) -> Self {
        self.duplicate = true;
        self
    }

    pub const fn prune(mut self) -> Self {
        self.prune = true;
        self
    }

    pub const fn goal(mut self) -> Self {
        self.goal = true;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildCommit {
    pub disposition: CommitDisposition,
    pub node_id: Option<NodeId>,
    pub duplicate_route: Option<NodeId>,
}

impl ChildCommit {
    const fn new(
        disposition: CommitDisposition,
        node_id: Option<NodeId>,
        duplicate_route: Option<NodeId>,
    ) -> Self {
        Self {
            disposition,
            node_id,
            duplicate_route,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchCommitOutcome {
    pub parent: NodeId,
    pub parent_visits: u32,
    pub child_commits: Vec<ChildCommit>,
    pub parent_evicted: Option<FrontierEvictReason>,
    pub goal_node: Option<NodeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitState {
    pub tree: Tree,
    pub frontier: Frontier,
    pub cell_mirror: CellMirror,
    pub seen: SeenMap,
    all_duplicate_streaks: Vec<u32>,
}

impl CommitState {
    pub fn from_root(payload: NodePayload) -> Self {
        let mut seen = SeenMap::new();
        let mut cell_mirror = CellMirror::new();
        seen.insert(payload.state_hash, NodeId::ROOT);
        cell_mirror.bump(payload.cell);
        Self {
            tree: Tree::from_root(payload),
            frontier: Frontier::with_root(),
            cell_mirror,
            seen,
            all_duplicate_streaks: vec![0],
        }
    }

    /// Rebuilds commit state from resume-recovered parts (§8.2): the tree
    /// adopted from the store, frontier membership from FRONTIER rows,
    /// mirrors from checkpoint vectors plus post-checkpoint attrs, and
    /// all-duplicate streaks from the checkpoint's frontier weights.
    #[must_use]
    pub fn from_parts(
        tree: Tree,
        frontier: Frontier,
        cell_mirror: CellMirror,
        seen: SeenMap,
        streaks: &[(NodeId, u32)],
    ) -> Self {
        let mut all_duplicate_streaks = vec![0; tree.next_id().get() as usize];
        for (id, streak) in streaks {
            let index = id.get() as usize;
            if index < all_duplicate_streaks.len() {
                all_duplicate_streaks[index] = *streak;
            }
        }
        Self {
            tree,
            frontier,
            cell_mirror,
            seen,
            all_duplicate_streaks,
        }
    }

    pub fn all_duplicate_streak(&self, id: NodeId) -> Option<u32> {
        self.all_duplicate_streaks
            .get(usize::try_from(id.get()).ok()?)
            .copied()
    }

    fn ensure_tracking(&mut self, id: NodeId) {
        let index = usize::try_from(id.get()).expect("node id fits in usize");
        if self.all_duplicate_streaks.len() <= index {
            self.all_duplicate_streaks.resize(index + 1, 0);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitError {
    ParentNotFound(NodeId),
    InvalidRule(&'static str),
    Tree(TreeError),
    Frontier(FrontierError),
}

impl fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParentNotFound(id) => write!(formatter, "parent {:?} not found", id),
            Self::InvalidRule(field) => write!(formatter, "invalid commit rule {field}"),
            Self::Tree(error) => write!(formatter, "{error}"),
            Self::Frontier(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CommitError {}

impl From<TreeError> for CommitError {
    fn from(error: TreeError) -> Self {
        Self::Tree(error)
    }
}

impl From<FrontierError> for CommitError {
    fn from(error: FrontierError) -> Self {
        Self::Frontier(error)
    }
}

pub fn commit_batch(
    state: &mut CommitState,
    parent: NodeId,
    children: &[ScoredChild],
    rules: &CommitRules,
) -> Result<BatchCommitOutcome, CommitError> {
    validate_rules(rules)?;
    let parent_score = state
        .tree
        .get(parent)
        .ok_or(CommitError::ParentNotFound(parent))?
        .score;
    let mut child_commits = Vec::with_capacity(children.len());
    let mut goal_node = None;

    for child in children {
        let commit = commit_child(state, parent, parent_score.get(), *child, rules)?;
        if child.goal
            && commit.disposition == CommitDisposition::Keep
            && commit.node_id.is_some()
            && goal_node.is_none()
        {
            goal_node = commit.node_id;
        }
        child_commits.push(commit);
    }

    let parent_visits = state.tree.increment_visits(parent)?;
    let all_duplicate = !child_commits.is_empty()
        && child_commits.iter().all(|commit| {
            commit.disposition == CommitDisposition::Discard(DiscardReason::Duplicate)
        });
    let parent_evicted =
        update_parent_exhaustion(state, parent, parent_visits, all_duplicate, rules)?;

    Ok(BatchCommitOutcome {
        parent,
        parent_visits,
        child_commits,
        parent_evicted,
        goal_node,
    })
}

fn commit_child(
    state: &mut CommitState,
    parent: NodeId,
    parent_score: f64,
    child: ScoredChild,
    rules: &CommitRules,
) -> Result<ChildCommit, CommitError> {
    if child.prune {
        return match rules.prune_action {
            PruneAction::Drop => Ok(ChildCommit::new(
                CommitDisposition::Discard(DiscardReason::PruneDrop),
                None,
                None,
            )),
            PruneAction::Exhausted => {
                let child_id = state.tree.insert_child(parent, child.payload)?;
                state.ensure_tracking(child_id);
                state.tree.mark_pruned(child_id)?;
                state.seen.insert(child.payload.state_hash, child_id);
                state.cell_mirror.bump(child.payload.cell);
                Ok(ChildCommit::new(
                    CommitDisposition::PrunedExhausted,
                    Some(child_id),
                    None,
                ))
            }
        };
    }

    if child.duplicate {
        state.cell_mirror.bump(child.payload.cell);
        let route = state.seen.get(child.payload.state_hash);
        if let Some(existing) = route {
            if existing != parent {
                state.tree.increment_visits(existing)?;
            }
        }
        return Ok(ChildCommit::new(
            CommitDisposition::Discard(DiscardReason::Duplicate),
            None,
            route,
        ));
    }

    let worse_than_parent = child.payload.score.get() + rules.epsilon_keep < parent_score;
    let known_cell = state.cell_mirror.count(child.payload.cell) > 0;
    if worse_than_parent && known_cell {
        return Ok(ChildCommit::new(
            CommitDisposition::Discard(DiscardReason::Regression),
            None,
            None,
        ));
    }

    let child_id = state.tree.insert_child(parent, child.payload)?;
    state.ensure_tracking(child_id);
    state.seen.insert(child.payload.state_hash, child_id);
    state.cell_mirror.bump(child.payload.cell);
    state.frontier.insert(child_id)?;

    if child.goal {
        state.tree.mark_goal(child_id)?;
    }

    Ok(ChildCommit::new(
        CommitDisposition::Keep,
        Some(child_id),
        None,
    ))
}

fn update_parent_exhaustion(
    state: &mut CommitState,
    parent: NodeId,
    parent_visits: u32,
    all_duplicate: bool,
    rules: &CommitRules,
) -> Result<Option<FrontierEvictReason>, CommitError> {
    state.ensure_tracking(parent);
    let parent_index = usize::try_from(parent.get()).expect("node id fits in usize");
    if all_duplicate {
        state.all_duplicate_streaks[parent_index] = state.all_duplicate_streaks[parent_index]
            .checked_add(1)
            .expect("all-duplicate streak overflowed");
    } else {
        state.all_duplicate_streaks[parent_index] = 0;
    }

    let reason = if parent_visits >= rules.max_visits_per_node {
        Some(FrontierEvictReason::MaxVisits)
    } else if state.all_duplicate_streaks[parent_index] >= rules.exhaust_after_dup_expansions {
        Some(FrontierEvictReason::AllDuplicateExpansions)
    } else {
        None
    };

    if reason.is_some() {
        state.tree.mark_exhausted(parent)?;
        if state.frontier.contains(parent) {
            state.frontier.remove(parent)?;
        }
    }

    Ok(reason)
}

fn validate_rules(rules: &CommitRules) -> Result<(), CommitError> {
    if !rules.epsilon_keep.is_finite() || rules.epsilon_keep < 0.0 {
        return Err(CommitError::InvalidRule("epsilon_keep"));
    }
    if rules.max_visits_per_node == 0 {
        return Err(CommitError::InvalidRule("max_visits_per_node"));
    }
    if rules.exhaust_after_dup_expansions == 0 {
        return Err(CommitError::InvalidRule("exhaust_after_dup_expansions"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CellKey, FrameCount, Novelty, Score, SnapshotRef, Stage, StateHash};

    fn payload(seed: u8, score: f64, cell: u64) -> NodePayload {
        NodePayload::new(
            SnapshotRef::new([seed; 32]),
            Score::new(score).unwrap(),
            Novelty::new(1.0).unwrap(),
            CellKey::new(cell),
            StateHash::new([seed.wrapping_add(1); 32]),
            Stage::new(u16::from(seed)),
            FrameCount::new(u32::from(seed) * 10),
        )
    }

    fn state() -> CommitState {
        CommitState::from_root(payload(0, 10.0, 0))
    }

    fn rules() -> CommitRules {
        CommitRules::new(PruneAction::Exhausted, 0.001, 64, 8)
    }

    #[test]
    fn commit_prune_exhausted_commits_pruned_node_outside_frontier() {
        let mut state = state();
        let outcome = commit_batch(
            &mut state,
            NodeId::ROOT,
            &[ScoredChild::new(payload(1, 0.0, 1)).prune()],
            &rules(),
        )
        .unwrap();
        let child = NodeId::new(1);

        assert_eq!(
            outcome.child_commits,
            vec![ChildCommit::new(
                CommitDisposition::PrunedExhausted,
                Some(child),
                None,
            )]
        );
        assert_eq!(
            state.tree.get(child).unwrap().status,
            crate::types::NodeStatus::Pruned
        );
        assert!(!state.frontier.contains(child));
        assert_eq!(state.tree.root().visits, 1);
    }

    #[test]
    fn commit_prune_drop_discards_without_tree_or_frontier_insert() {
        let mut state = state();
        let mut drop_rules = rules();
        drop_rules.prune_action = PruneAction::Drop;

        let outcome = commit_batch(
            &mut state,
            NodeId::ROOT,
            &[ScoredChild::new(payload(1, 0.0, 1)).prune()],
            &drop_rules,
        )
        .unwrap();

        assert_eq!(
            outcome.child_commits,
            vec![ChildCommit::new(
                CommitDisposition::Discard(DiscardReason::PruneDrop),
                None,
                None,
            )]
        );
        assert_eq!(state.tree.len(), 1);
        assert_eq!(state.frontier.deterministic_entries(), vec![NodeId::ROOT]);
        assert_eq!(state.tree.root().visits, 1);
    }

    #[test]
    fn commit_duplicate_routes_to_seen_node_and_bumps_cell() {
        let mut state = state();
        let kept = commit_batch(
            &mut state,
            NodeId::ROOT,
            &[ScoredChild::new(payload(1, 11.0, 9))],
            &rules(),
        )
        .unwrap()
        .child_commits[0]
            .node_id
            .unwrap();
        let duplicate = ScoredChild::new(payload(1, 11.0, 9)).duplicate();

        let outcome = commit_batch(&mut state, NodeId::ROOT, &[duplicate], &rules()).unwrap();

        assert_eq!(
            outcome.child_commits,
            vec![ChildCommit::new(
                CommitDisposition::Discard(DiscardReason::Duplicate),
                None,
                Some(kept),
            )]
        );
        assert_eq!(state.tree.get(kept).unwrap().visits, 1);
        assert_eq!(state.cell_mirror.count(CellKey::new(9)), 2);
        assert_eq!(state.tree.root().visits, 2);
    }

    #[test]
    fn commit_regression_discards_only_when_cell_is_known() {
        let mut state = state();

        let regression = commit_batch(
            &mut state,
            NodeId::ROOT,
            &[ScoredChild::new(payload(1, 9.0, 0))],
            &rules(),
        )
        .unwrap();
        let novel_cell = commit_batch(
            &mut state,
            NodeId::ROOT,
            &[ScoredChild::new(payload(2, 9.0, 6))],
            &rules(),
        )
        .unwrap();

        assert_eq!(
            regression.child_commits,
            vec![ChildCommit::new(
                CommitDisposition::Discard(DiscardReason::Regression),
                None,
                None,
            )]
        );
        assert_eq!(
            novel_cell.child_commits[0].disposition,
            CommitDisposition::Keep
        );
        assert_eq!(novel_cell.child_commits[0].node_id, Some(NodeId::new(1)));
        assert!(state.frontier.contains(NodeId::new(1)));
        assert_eq!(state.cell_mirror.count(CellKey::new(6)), 1);
    }

    #[test]
    fn commit_goal_keeps_goal_node_but_excludes_it_from_candidates() {
        let mut state = state();

        let outcome = commit_batch(
            &mut state,
            NodeId::ROOT,
            &[ScoredChild::new(payload(1, 12.0, 1)).goal()],
            &rules(),
        )
        .unwrap();
        let goal = outcome.goal_node.unwrap();

        assert_eq!(
            outcome.child_commits[0].disposition,
            CommitDisposition::Keep
        );
        assert_eq!(
            state.tree.get(goal).unwrap().status,
            crate::types::NodeStatus::Goal
        );
        assert!(state.frontier.contains(goal));
        assert_eq!(
            state
                .frontier
                .deterministic_candidates(&state.tree)
                .unwrap(),
            vec![NodeId::ROOT]
        );
    }

    #[test]
    fn commit_pruned_exhausted_updates_mirrors_and_goal_does_not_win() {
        let mut state = state();
        let prune_payload = payload(1, 12.0, 12);

        let outcome = commit_batch(
            &mut state,
            NodeId::ROOT,
            &[ScoredChild::new(prune_payload).prune().goal()],
            &rules(),
        )
        .unwrap();
        let pruned = outcome.child_commits[0].node_id.unwrap();

        assert_eq!(outcome.goal_node, None);
        assert_eq!(state.seen.get(prune_payload.state_hash), Some(pruned));
        assert_eq!(state.cell_mirror.count(prune_payload.cell), 1);

        let duplicate = ScoredChild::new(prune_payload).duplicate();
        let duplicate_outcome =
            commit_batch(&mut state, NodeId::ROOT, &[duplicate], &rules()).unwrap();

        assert_eq!(
            duplicate_outcome.child_commits[0].duplicate_route,
            Some(pruned)
        );
        assert_eq!(state.tree.get(pruned).unwrap().visits, 1);
        assert_eq!(state.cell_mirror.count(prune_payload.cell), 2);
    }

    #[test]
    fn commit_later_non_kept_goal_does_not_clear_earlier_kept_goal() {
        let mut state = state();
        let outcome = commit_batch(
            &mut state,
            NodeId::ROOT,
            &[
                ScoredChild::new(payload(1, 12.0, 1)).goal(),
                ScoredChild::new(payload(2, 0.0, 2)).prune().goal(),
            ],
            &rules(),
        )
        .unwrap();

        assert_eq!(outcome.goal_node, Some(NodeId::new(1)));
        assert_eq!(
            outcome.child_commits[0].disposition,
            CommitDisposition::Keep
        );
        assert_eq!(
            outcome.child_commits[1].disposition,
            CommitDisposition::PrunedExhausted
        );
    }

    #[test]
    fn commit_duplicate_route_to_parent_does_not_double_count_parent_visit() {
        let mut state = state();
        let duplicate_root = ScoredChild::new(payload(0, 10.0, 0)).duplicate();

        let outcome = commit_batch(&mut state, NodeId::ROOT, &[duplicate_root], &rules()).unwrap();

        assert_eq!(outcome.child_commits[0].duplicate_route, Some(NodeId::ROOT));
        assert_eq!(outcome.parent_visits, 1);
        assert_eq!(state.tree.root().visits, 1);
    }

    #[test]
    fn commit_parent_visit_increments_once_per_expansion_and_max_visits_evicts() {
        let mut state = state();
        let max_one = CommitRules::new(PruneAction::Exhausted, 0.001, 1, 8);

        let outcome = commit_batch(
            &mut state,
            NodeId::ROOT,
            &[
                ScoredChild::new(payload(1, 11.0, 1)),
                ScoredChild::new(payload(2, 12.0, 2)),
            ],
            &max_one,
        )
        .unwrap();

        assert_eq!(outcome.parent_visits, 1);
        assert_eq!(outcome.parent_evicted, Some(FrontierEvictReason::MaxVisits));
        assert!(state.tree.root().exhausted);
        assert!(!state.frontier.contains(NodeId::ROOT));
        assert_eq!(state.tree.root().children, 2);
    }

    #[test]
    fn commit_all_duplicate_streak_exhausts_and_removes_parent() {
        let mut state = state();
        let first = commit_batch(
            &mut state,
            NodeId::ROOT,
            &[ScoredChild::new(payload(1, 11.0, 1))],
            &rules(),
        )
        .unwrap()
        .child_commits[0]
            .node_id
            .unwrap();
        let mut dup_rules = rules();
        dup_rules.exhaust_after_dup_expansions = 2;
        let duplicate = ScoredChild::new(payload(1, 11.0, 1)).duplicate();

        let first_dup = commit_batch(&mut state, NodeId::ROOT, &[duplicate], &dup_rules).unwrap();
        let second_dup = commit_batch(&mut state, NodeId::ROOT, &[duplicate], &dup_rules).unwrap();

        assert_eq!(first_dup.parent_evicted, None);
        assert_eq!(state.all_duplicate_streak(NodeId::ROOT), Some(2));
        assert_eq!(
            second_dup.parent_evicted,
            Some(FrontierEvictReason::AllDuplicateExpansions)
        );
        assert_eq!(state.tree.get(first).unwrap().visits, 2);
        assert!(state.tree.root().exhausted);
        assert!(!state.frontier.contains(NodeId::ROOT));
    }
}
