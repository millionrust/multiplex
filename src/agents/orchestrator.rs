use std::collections::{HashMap, HashSet};

use crate::agents::AgentRunState;
use crate::models::{CanvasEdgeKind, CanvasNodeId, SavedCanvasEdge};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulableAgent {
    pub node_id: CanvasNodeId,
    pub state: AgentRunState,
    pub has_queued_task: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DagSchedule {
    pub ready: Vec<CanvasNodeId>,
    pub blocked: Vec<CanvasNodeId>,
    pub cycle_detected: bool,
}

pub fn schedule_dependency_dag(
    agents: &[SchedulableAgent],
    edges: &[SavedCanvasEdge],
    max_concurrency: usize,
) -> DagSchedule {
    let states: HashMap<_, _> = agents
        .iter()
        .map(|agent| (agent.node_id.clone(), agent.state))
        .collect();
    let node_ids: HashSet<_> = states.keys().cloned().collect();
    let dependencies: Vec<_> = edges
        .iter()
        .filter(|edge| {
            edge.enabled
                && edge.kind == CanvasEdgeKind::Dependency
                && node_ids.contains(&edge.source)
                && node_ids.contains(&edge.target)
        })
        .collect();
    if dependency_cycle(&node_ids, &dependencies) {
        return DagSchedule {
            cycle_detected: true,
            ..DagSchedule::default()
        };
    }
    let running = agents
        .iter()
        .filter(|agent| {
            matches!(
                agent.state,
                AgentRunState::Starting | AgentRunState::Running
            )
        })
        .count();
    let available = max_concurrency.max(1).saturating_sub(running);
    let mut ready = Vec::new();
    let mut blocked = Vec::new();
    for agent in agents.iter().filter(|agent| {
        agent.has_queued_task && matches!(agent.state, AgentRunState::Idle | AgentRunState::Blocked)
    }) {
        let prerequisites: Vec<_> = dependencies
            .iter()
            .filter(|edge| edge.target == agent.node_id)
            .filter_map(|edge| states.get(&edge.source).copied())
            .collect();
        if prerequisites.iter().any(|state| {
            matches!(
                state,
                AgentRunState::Failed
                    | AgentRunState::Cancelled
                    | AgentRunState::Disconnected
                    | AgentRunState::Blocked
            )
        }) {
            blocked.push(agent.node_id.clone());
        } else if prerequisites
            .iter()
            .all(|state| *state == AgentRunState::Succeeded)
            && ready.len() < available
        {
            ready.push(agent.node_id.clone());
        }
    }
    DagSchedule {
        ready,
        blocked,
        cycle_detected: false,
    }
}

fn dependency_cycle(nodes: &HashSet<CanvasNodeId>, edges: &[&SavedCanvasEdge]) -> bool {
    let mut adjacency: HashMap<CanvasNodeId, Vec<CanvasNodeId>> = HashMap::new();
    for edge in edges {
        adjacency
            .entry(edge.source.clone())
            .or_default()
            .push(edge.target.clone());
    }
    fn visit(
        node: &CanvasNodeId,
        adjacency: &HashMap<CanvasNodeId, Vec<CanvasNodeId>>,
        visiting: &mut HashSet<CanvasNodeId>,
        visited: &mut HashSet<CanvasNodeId>,
    ) -> bool {
        if visiting.contains(node) {
            return true;
        }
        if !visited.insert(node.clone()) {
            return false;
        }
        visiting.insert(node.clone());
        if adjacency.get(node).is_some_and(|targets| {
            targets
                .iter()
                .any(|target| visit(target, adjacency, visiting, visited))
        }) {
            return true;
        }
        visiting.remove(node);
        false
    }
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    nodes
        .iter()
        .any(|node| visit(node, &adjacency, &mut visiting, &mut visited))
}

#[cfg(test)]
mod tests {
    use super::{SchedulableAgent, schedule_dependency_dag};
    use crate::agents::AgentRunState;
    use crate::models::{CanvasEdgeId, CanvasEdgeKind, CanvasNodeId, SavedCanvasEdge};

    fn agent(id: &str, state: AgentRunState, queued: bool) -> SchedulableAgent {
        SchedulableAgent {
            node_id: CanvasNodeId::new(id),
            state,
            has_queued_task: queued,
        }
    }

    fn edge(source: &str, target: &str) -> SavedCanvasEdge {
        SavedCanvasEdge {
            id: CanvasEdgeId::new(format!("{source}-{target}")),
            source: CanvasNodeId::new(source),
            target: CanvasNodeId::new(target),
            kind: CanvasEdgeKind::Dependency,
            enabled: true,
            context_policy: None,
        }
    }

    #[test]
    fn schedules_only_ready_nodes_with_concurrency_limit() {
        let agents = vec![
            agent("a", AgentRunState::Idle, true),
            agent("b", AgentRunState::Idle, true),
            agent("c", AgentRunState::Idle, true),
        ];
        let schedule = schedule_dependency_dag(&agents, &[edge("a", "c")], 2);
        assert_eq!(
            schedule.ready,
            vec![CanvasNodeId::new("a"), CanvasNodeId::new("b")]
        );
        assert!(schedule.blocked.is_empty());
    }

    #[test]
    fn unlocks_successors_and_blocks_failed_dependencies() {
        let success = schedule_dependency_dag(
            &[
                agent("a", AgentRunState::Succeeded, false),
                agent("b", AgentRunState::Idle, true),
            ],
            &[edge("a", "b")],
            2,
        );
        assert_eq!(success.ready, vec![CanvasNodeId::new("b")]);

        let failed = schedule_dependency_dag(
            &[
                agent("a", AgentRunState::Failed, false),
                agent("b", AgentRunState::Idle, true),
            ],
            &[edge("a", "b")],
            2,
        );
        assert_eq!(failed.blocked, vec![CanvasNodeId::new("b")]);
    }

    #[test]
    fn refuses_cycles() {
        let schedule = schedule_dependency_dag(
            &[
                agent("a", AgentRunState::Idle, true),
                agent("b", AgentRunState::Idle, true),
            ],
            &[edge("a", "b"), edge("b", "a")],
            2,
        );
        assert!(schedule.cycle_detected);
        assert!(schedule.ready.is_empty());
    }
}
