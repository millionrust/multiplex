use crate::models::CanvasNodeId;

pub(super) struct CanvasSelectionRequest<'a> {
    pub node_ids: &'a [CanvasNodeId],
    pub selected_node_id: Option<&'a CanvasNodeId>,
    pub delta: isize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum CanvasSelectionDecision {
    Clear,
    Select(CanvasNodeId),
}

#[derive(Clone, Debug, Default)]
pub(super) struct CanvasCoordinator;

impl CanvasCoordinator {
    pub fn select_adjacent(&self, request: CanvasSelectionRequest<'_>) -> CanvasSelectionDecision {
        let node_count = request.node_ids.len();
        if node_count == 0 {
            return CanvasSelectionDecision::Clear;
        }

        let current = request
            .selected_node_id
            .and_then(|selected| {
                request
                    .node_ids
                    .iter()
                    .position(|node_id| node_id == selected)
            })
            .unwrap_or_else(|| if request.delta < 0 { 0 } else { node_count - 1 });
        let next = wrapped_index(current, request.delta, node_count);
        CanvasSelectionDecision::Select(request.node_ids[next].clone())
    }
}

fn wrapped_index(current: usize, delta: isize, node_count: usize) -> usize {
    if delta >= 0 {
        let offset = delta.unsigned_abs() % node_count;
        let remaining = node_count - current;
        if offset >= remaining {
            offset - remaining
        } else {
            current + offset
        }
    } else {
        let offset = delta.unsigned_abs() % node_count;
        if offset > current {
            node_count - (offset - current)
        } else {
            current - offset
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Vec<CanvasNodeId> {
        ["a", "b", "c"].into_iter().map(CanvasNodeId::new).collect()
    }

    #[test]
    fn canvas_coordinator_clears_selection_when_the_canvas_is_empty() {
        let coordinator = CanvasCoordinator;

        assert_eq!(
            coordinator.select_adjacent(CanvasSelectionRequest {
                node_ids: &[],
                selected_node_id: Some(&CanvasNodeId::new("stale")),
                delta: 1,
            }),
            CanvasSelectionDecision::Clear
        );
    }

    #[test]
    fn canvas_coordinator_starts_at_the_directional_edge_without_a_selection() {
        let coordinator = CanvasCoordinator;
        let node_ids = ids();

        for (delta, expected) in [(1, "a"), (-1, "c")] {
            assert_eq!(
                coordinator.select_adjacent(CanvasSelectionRequest {
                    node_ids: &node_ids,
                    selected_node_id: None,
                    delta,
                }),
                CanvasSelectionDecision::Select(CanvasNodeId::new(expected))
            );
        }
    }

    #[test]
    fn canvas_coordinator_preserves_existing_stale_and_wrapped_selection_behavior() {
        let coordinator = CanvasCoordinator;
        let node_ids = ids();
        let b = CanvasNodeId::new("b");
        let stale = CanvasNodeId::new("stale");

        for (selected, delta, expected) in [
            (Some(&b), 1, "c"),
            (Some(&b), -1, "a"),
            (Some(&stale), 1, "a"),
            (Some(&stale), -1, "c"),
            (Some(&node_ids[2]), 1, "a"),
            (Some(&node_ids[0]), -1, "c"),
        ] {
            assert_eq!(
                coordinator.select_adjacent(CanvasSelectionRequest {
                    node_ids: &node_ids,
                    selected_node_id: selected,
                    delta,
                }),
                CanvasSelectionDecision::Select(CanvasNodeId::new(expected))
            );
        }
    }

    #[test]
    fn canvas_coordinator_handles_full_range_signed_deltas_without_overflow() {
        let coordinator = CanvasCoordinator;
        let node_ids = ids();

        assert_eq!(
            coordinator.select_adjacent(CanvasSelectionRequest {
                node_ids: &node_ids,
                selected_node_id: Some(&node_ids[0]),
                delta: isize::MAX,
            }),
            CanvasSelectionDecision::Select(CanvasNodeId::new("b"))
        );
        assert_eq!(
            coordinator.select_adjacent(CanvasSelectionRequest {
                node_ids: &node_ids,
                selected_node_id: Some(&node_ids[2]),
                delta: isize::MIN,
            }),
            CanvasSelectionDecision::Select(CanvasNodeId::new("a"))
        );
    }

    #[test]
    fn adjacent_selection_policy_has_one_non_ui_owner() {
        let canvas_source = include_str!("canvas.rs");
        assert!(canvas_source.contains("canvas_coordinator.select_adjacent"));
        assert!(!canvas_source.contains("rem_euclid(self.nodes.len()"));
        assert!(!canvas_source.contains("unwrap_or_else(|| if delta < 0"));

        let forbidden_crate = ["gp", "ui"].concat();
        assert!(!include_str!("canvas_coordinator.rs").contains(&forbidden_crate));
    }
}
