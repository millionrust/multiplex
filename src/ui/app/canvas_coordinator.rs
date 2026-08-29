use std::collections::{HashMap, HashSet};

use crate::models::{CanvasEdgeId, CanvasEdgeKind, CanvasNodeId, SavedContextPolicy};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CanvasLinkNodeSummary {
    pub id: CanvasNodeId,
    pub can_source_context: bool,
    pub executable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CanvasLinkEdgeSummary {
    pub id: CanvasEdgeId,
    pub source: CanvasNodeId,
    pub target: CanvasNodeId,
    pub kind: CanvasEdgeKind,
    pub enabled: bool,
}

pub(super) struct CanvasLinkCreationRequest<'a> {
    pub nodes: &'a [CanvasLinkNodeSummary],
    pub edges: &'a [CanvasLinkEdgeSummary],
    pub source: CanvasNodeId,
    pub target: CanvasNodeId,
    pub kind: CanvasEdgeKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CanvasLinkMutation {
    SetEnabled(bool),
    Remove,
}

pub(super) struct CanvasLinkMutationRequest<'a> {
    pub edges: &'a [CanvasLinkEdgeSummary],
    pub edge_id: CanvasEdgeId,
    pub operation: CanvasLinkMutation,
    pub reviewed_edge_id: Option<&'a CanvasEdgeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum CanvasLinkMutationDecision {
    Missing,
    Apply {
        edge_id: CanvasEdgeId,
        operation: CanvasLinkMutation,
        dependency_changed: bool,
        clear_context_review: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CanvasLinkPlan {
    pub id: CanvasEdgeId,
    pub source: CanvasNodeId,
    pub target: CanvasNodeId,
    pub kind: CanvasEdgeKind,
    pub enabled: bool,
    pub context_policy: Option<SavedContextPolicy>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CanvasLinkCreationError {
    SameContextNode,
    SameDependencyNode,
    MissingContextNode,
    MissingDependencyNode,
    InvalidContextSource,
    InvalidContextTarget,
    InvalidDependencyEndpoint,
    DuplicateContext,
    DuplicateDependency,
    DependencyCycle,
}

impl std::fmt::Display for CanvasLinkCreationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SameContextNode => "Choose a different target node",
            Self::SameDependencyNode => "Choose a different dependency target",
            Self::MissingContextNode => "Both context-link nodes must exist",
            Self::MissingDependencyNode => "Both dependency nodes must exist",
            Self::InvalidContextSource => "Group frames cannot be used as context sources",
            Self::InvalidContextTarget => "Choose a terminal or agent as the context target",
            Self::InvalidDependencyEndpoint => "Dependencies can only connect terminals and agents",
            Self::DuplicateContext => "That context link already exists",
            Self::DuplicateDependency => "That dependency already exists",
            Self::DependencyCycle => "That dependency would create a cycle",
        })
    }
}

impl std::error::Error for CanvasLinkCreationError {}

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
    pub fn mutate_link(
        &self,
        request: CanvasLinkMutationRequest<'_>,
    ) -> CanvasLinkMutationDecision {
        let Some(edge) = request.edges.iter().find(|edge| edge.id == request.edge_id) else {
            return CanvasLinkMutationDecision::Missing;
        };
        CanvasLinkMutationDecision::Apply {
            edge_id: request.edge_id,
            operation: request.operation,
            dependency_changed: edge.kind == CanvasEdgeKind::Dependency,
            clear_context_review: request.reviewed_edge_id == Some(&edge.id)
                && matches!(
                    request.operation,
                    CanvasLinkMutation::SetEnabled(false) | CanvasLinkMutation::Remove
                ),
        }
    }

    pub fn create_link(
        &self,
        request: CanvasLinkCreationRequest<'_>,
    ) -> Result<CanvasLinkPlan, CanvasLinkCreationError> {
        if request.source == request.target {
            return Err(match request.kind {
                CanvasEdgeKind::Context => CanvasLinkCreationError::SameContextNode,
                CanvasEdgeKind::Dependency => CanvasLinkCreationError::SameDependencyNode,
            });
        }

        let source = request.nodes.iter().find(|node| node.id == request.source);
        let target = request.nodes.iter().find(|node| node.id == request.target);
        let (Some(source), Some(target)) = (source, target) else {
            return Err(match request.kind {
                CanvasEdgeKind::Context => CanvasLinkCreationError::MissingContextNode,
                CanvasEdgeKind::Dependency => CanvasLinkCreationError::MissingDependencyNode,
            });
        };

        match request.kind {
            CanvasEdgeKind::Context => {
                if !source.can_source_context {
                    return Err(CanvasLinkCreationError::InvalidContextSource);
                }
                if !target.executable {
                    return Err(CanvasLinkCreationError::InvalidContextTarget);
                }
            }
            CanvasEdgeKind::Dependency => {
                if !source.executable || !target.executable {
                    return Err(CanvasLinkCreationError::InvalidDependencyEndpoint);
                }
            }
        }

        if request.edges.iter().any(|edge| {
            edge.source == request.source
                && edge.target == request.target
                && edge.kind == request.kind
        }) {
            return Err(match request.kind {
                CanvasEdgeKind::Context => CanvasLinkCreationError::DuplicateContext,
                CanvasEdgeKind::Dependency => CanvasLinkCreationError::DuplicateDependency,
            });
        }

        if request.kind == CanvasEdgeKind::Dependency
            && dependency_path_exists(request.edges, &request.target, &request.source)
        {
            return Err(CanvasLinkCreationError::DependencyCycle);
        }

        let prefix = match request.kind {
            CanvasEdgeKind::Context => "context-edge",
            CanvasEdgeKind::Dependency => "dependency-edge",
        };
        let mut ordinal = request.edges.len() + 1;
        let id = loop {
            let candidate = CanvasEdgeId::new(format!("{prefix}-{ordinal}"));
            if !request.edges.iter().any(|edge| edge.id == candidate) {
                break candidate;
            }
            ordinal += 1;
        };
        Ok(CanvasLinkPlan {
            id,
            source: request.source,
            target: request.target,
            kind: request.kind,
            enabled: true,
            context_policy: (request.kind == CanvasEdgeKind::Context)
                .then(SavedContextPolicy::default),
        })
    }

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

fn dependency_path_exists(
    edges: &[CanvasLinkEdgeSummary],
    start: &CanvasNodeId,
    destination: &CanvasNodeId,
) -> bool {
    let mut adjacency: HashMap<&CanvasNodeId, Vec<&CanvasNodeId>> = HashMap::new();
    for edge in edges
        .iter()
        .filter(|edge| edge.enabled && edge.kind == CanvasEdgeKind::Dependency)
    {
        adjacency
            .entry(&edge.source)
            .or_default()
            .push(&edge.target);
    }
    let mut pending = vec![start];
    let mut visited = HashSet::new();
    while let Some(node) = pending.pop() {
        if node == destination {
            return true;
        }
        if visited.insert(node) {
            pending.extend(adjacency.get(node).into_iter().flatten().copied());
        }
    }
    false
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

    fn link_node(id: &str, can_source_context: bool, executable: bool) -> CanvasLinkNodeSummary {
        CanvasLinkNodeSummary {
            id: CanvasNodeId::new(id),
            can_source_context,
            executable,
        }
    }

    fn link_edge(
        id: &str,
        source: &str,
        target: &str,
        kind: CanvasEdgeKind,
        enabled: bool,
    ) -> CanvasLinkEdgeSummary {
        CanvasLinkEdgeSummary {
            id: CanvasEdgeId::new(id),
            source: CanvasNodeId::new(source),
            target: CanvasNodeId::new(target),
            kind,
            enabled,
        }
    }

    fn create_link(
        nodes: &[CanvasLinkNodeSummary],
        edges: &[CanvasLinkEdgeSummary],
        source: &str,
        target: &str,
        kind: CanvasEdgeKind,
    ) -> Result<CanvasLinkPlan, CanvasLinkCreationError> {
        CanvasCoordinator.create_link(CanvasLinkCreationRequest {
            nodes,
            edges,
            source: CanvasNodeId::new(source),
            target: CanvasNodeId::new(target),
            kind,
        })
    }

    #[test]
    fn canvas_coordinator_creates_complete_context_and_dependency_plans() {
        let nodes = [
            link_node("note", true, false),
            link_node("a", true, true),
            link_node("b", true, true),
        ];
        assert_eq!(
            create_link(&nodes, &[], "note", "a", CanvasEdgeKind::Context).unwrap(),
            CanvasLinkPlan {
                id: CanvasEdgeId::new("context-edge-1"),
                source: CanvasNodeId::new("note"),
                target: CanvasNodeId::new("a"),
                kind: CanvasEdgeKind::Context,
                enabled: true,
                context_policy: Some(SavedContextPolicy::default()),
            }
        );
        assert_eq!(
            create_link(&nodes, &[], "a", "b", CanvasEdgeKind::Dependency).unwrap(),
            CanvasLinkPlan {
                id: CanvasEdgeId::new("dependency-edge-1"),
                source: CanvasNodeId::new("a"),
                target: CanvasNodeId::new("b"),
                kind: CanvasEdgeKind::Dependency,
                enabled: true,
                context_policy: None,
            }
        );
    }

    #[test]
    fn canvas_coordinator_classifies_link_mutations_and_review_invalidation() {
        let context = link_edge("context-edge-1", "a", "b", CanvasEdgeKind::Context, true);
        let dependency = link_edge(
            "dependency-edge-2",
            "b",
            "c",
            CanvasEdgeKind::Dependency,
            true,
        );
        let edges = [context.clone(), dependency.clone()];

        assert_eq!(
            CanvasCoordinator.mutate_link(CanvasLinkMutationRequest {
                edges: &edges,
                edge_id: context.id.clone(),
                operation: CanvasLinkMutation::SetEnabled(true),
                reviewed_edge_id: Some(&context.id),
            }),
            CanvasLinkMutationDecision::Apply {
                edge_id: context.id.clone(),
                operation: CanvasLinkMutation::SetEnabled(true),
                dependency_changed: false,
                clear_context_review: false,
            }
        );
        assert_eq!(
            CanvasCoordinator.mutate_link(CanvasLinkMutationRequest {
                edges: &edges,
                edge_id: context.id.clone(),
                operation: CanvasLinkMutation::SetEnabled(false),
                reviewed_edge_id: Some(&context.id),
            }),
            CanvasLinkMutationDecision::Apply {
                edge_id: context.id.clone(),
                operation: CanvasLinkMutation::SetEnabled(false),
                dependency_changed: false,
                clear_context_review: true,
            }
        );
        assert_eq!(
            CanvasCoordinator.mutate_link(CanvasLinkMutationRequest {
                edges: &edges,
                edge_id: dependency.id.clone(),
                operation: CanvasLinkMutation::Remove,
                reviewed_edge_id: Some(&dependency.id),
            }),
            CanvasLinkMutationDecision::Apply {
                edge_id: dependency.id,
                operation: CanvasLinkMutation::Remove,
                dependency_changed: true,
                clear_context_review: true,
            }
        );
    }

    #[test]
    fn canvas_coordinator_rejects_missing_link_mutations_without_side_effects() {
        assert_eq!(
            CanvasCoordinator.mutate_link(CanvasLinkMutationRequest {
                edges: &[],
                edge_id: CanvasEdgeId::new("missing"),
                operation: CanvasLinkMutation::Remove,
                reviewed_edge_id: Some(&CanvasEdgeId::new("missing")),
            }),
            CanvasLinkMutationDecision::Missing
        );
    }

    #[test]
    fn canvas_coordinator_rejects_invalid_endpoints_and_roles_with_exact_errors() {
        let nodes = [
            link_node("group", false, false),
            link_node("note", true, false),
            link_node("agent", true, true),
        ];
        let cases = [
            (
                "agent",
                "agent",
                CanvasEdgeKind::Context,
                CanvasLinkCreationError::SameContextNode,
                "Choose a different target node",
            ),
            (
                "agent",
                "agent",
                CanvasEdgeKind::Dependency,
                CanvasLinkCreationError::SameDependencyNode,
                "Choose a different dependency target",
            ),
            (
                "missing",
                "agent",
                CanvasEdgeKind::Context,
                CanvasLinkCreationError::MissingContextNode,
                "Both context-link nodes must exist",
            ),
            (
                "agent",
                "missing",
                CanvasEdgeKind::Dependency,
                CanvasLinkCreationError::MissingDependencyNode,
                "Both dependency nodes must exist",
            ),
            (
                "group",
                "agent",
                CanvasEdgeKind::Context,
                CanvasLinkCreationError::InvalidContextSource,
                "Group frames cannot be used as context sources",
            ),
            (
                "agent",
                "note",
                CanvasEdgeKind::Context,
                CanvasLinkCreationError::InvalidContextTarget,
                "Choose a terminal or agent as the context target",
            ),
            (
                "note",
                "agent",
                CanvasEdgeKind::Dependency,
                CanvasLinkCreationError::InvalidDependencyEndpoint,
                "Dependencies can only connect terminals and agents",
            ),
        ];
        for (source, target, kind, expected, message) in cases {
            let error = create_link(&nodes, &[], source, target, kind).unwrap_err();
            assert_eq!(error, expected);
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn canvas_coordinator_preserves_directed_same_kind_duplicate_rules_and_ids() {
        let nodes = [
            link_node("a", true, true),
            link_node("b", true, true),
            link_node("c", true, true),
        ];
        let context = link_edge("context-edge-1", "a", "b", CanvasEdgeKind::Context, true);
        assert_eq!(
            create_link(
                &nodes,
                std::slice::from_ref(&context),
                "a",
                "b",
                CanvasEdgeKind::Context,
            ),
            Err(CanvasLinkCreationError::DuplicateContext)
        );
        assert!(
            create_link(
                &nodes,
                std::slice::from_ref(&context),
                "b",
                "a",
                CanvasEdgeKind::Context,
            )
            .is_ok()
        );
        assert!(
            create_link(
                &nodes,
                std::slice::from_ref(&context),
                "a",
                "b",
                CanvasEdgeKind::Dependency,
            )
            .is_ok()
        );

        let dependency = link_edge(
            "dependency-edge-1",
            "a",
            "b",
            CanvasEdgeKind::Dependency,
            true,
        );
        assert_eq!(
            create_link(&nodes, &[dependency], "a", "b", CanvasEdgeKind::Dependency,),
            Err(CanvasLinkCreationError::DuplicateDependency)
        );

        let collision = link_edge("dependency-edge-2", "c", "a", CanvasEdgeKind::Context, true);
        assert_eq!(
            create_link(&nodes, &[collision], "a", "b", CanvasEdgeKind::Dependency,)
                .unwrap()
                .id,
            CanvasEdgeId::new("dependency-edge-3")
        );
    }

    #[test]
    fn canvas_coordinator_detects_cycles_through_enabled_dependencies_only() {
        let nodes = [
            link_node("a", true, true),
            link_node("b", true, true),
            link_node("c", true, true),
        ];
        let mut edges = [
            link_edge(
                "dependency-edge-1",
                "a",
                "b",
                CanvasEdgeKind::Dependency,
                true,
            ),
            link_edge(
                "dependency-edge-2",
                "b",
                "c",
                CanvasEdgeKind::Dependency,
                true,
            ),
        ];
        assert_eq!(
            create_link(&nodes, &edges, "c", "a", CanvasEdgeKind::Dependency),
            Err(CanvasLinkCreationError::DependencyCycle)
        );
        edges[0].enabled = false;
        assert!(create_link(&nodes, &edges, "c", "a", CanvasEdgeKind::Dependency).is_ok());
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
        assert!(canvas_source.contains("canvas_coordinator.create_link"));
        assert!(canvas_source.contains("canvas_coordinator.mutate_link"));
        assert!(!canvas_source.contains("rem_euclid(self.nodes.len()"));
        assert!(!canvas_source.contains("unwrap_or_else(|| if delta < 0"));
        assert!(!canvas_source.contains("Both context-link nodes must exist"));
        assert!(!canvas_source.contains("That dependency would create a cycle"));
        assert!(!canvas_source.contains("format!(\"context-edge-{ordinal}\")"));

        let forbidden_crate = ["gp", "ui"].concat();
        assert!(!include_str!("canvas_coordinator.rs").contains(&forbidden_crate));
    }
}
