#![forbid(unsafe_code)]

#[derive(Clone, Debug, PartialEq)]
pub struct FrontierNode {
    pub node_id: u64,
    pub score: f64,
    pub visits: u32,
}

pub fn select_best(nodes: &[FrontierNode]) -> Option<&FrontierNode> {
    nodes
        .iter()
        .max_by(|left, right| left.score.total_cmp(&right.score))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_highest_score() {
        let nodes = [
            FrontierNode {
                node_id: 1,
                score: 1.0,
                visits: 0,
            },
            FrontierNode {
                node_id: 2,
                score: 2.0,
                visits: 0,
            },
        ];
        assert_eq!(select_best(&nodes).unwrap().node_id, 2);
    }
}
