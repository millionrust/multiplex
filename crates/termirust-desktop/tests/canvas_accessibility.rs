use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use termirust_ui_contract::{
    AgentCanvasPresentationMode, AgentCanvasSemanticSnapshot, AgentCanvasSurfaceState,
    CanvasAlternativeEdge, CanvasAlternativeEdgeKind, CanvasAlternativeNodeKind,
    CanvasAlternativeNodeState, CanvasAlternativeRow, CanvasEdgeSemanticId, CanvasNodeAction,
    CanvasNodeSemanticId, SemanticText, ShellRegionId, ShellSemanticSnapshot,
    agent_canvas_presentation_mode, shell_region_semantic_node,
};

fn row(index: usize) -> CanvasAlternativeRow {
    CanvasAlternativeRow {
        id: CanvasNodeSemanticId::from_stable_key(&format!("node-{index}")),
        explicit_order: None,
        kind: if index.is_multiple_of(2) {
            CanvasAlternativeNodeKind::Agent
        } else {
            CanvasAlternativeNodeKind::Terminal
        },
        state: CanvasAlternativeNodeState::Idle,
        title: Some(format!("Synthetic node {index}")),
        parent: None,
        x: ((index % 10) * 640) as i32,
        y: ((index / 10) * 420) as i32,
        width: 620,
        height: 400,
        selected: index == 0,
        collapsed: false,
        actions: [
            CanvasNodeAction::Open,
            CanvasNodeAction::OpenMenu,
            CanvasNodeAction::MoveUp,
            CanvasNodeAction::MoveDown,
            CanvasNodeAction::MoveLeft,
            CanvasNodeAction::MoveRight,
            CanvasNodeAction::Rename,
            CanvasNodeAction::ToggleCollapsed,
            CanvasNodeAction::Remove,
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
    }
}

fn snapshot(count: usize) -> AgentCanvasSemanticSnapshot {
    let rows = (0..count).map(row).collect::<Vec<_>>();
    let edges = rows
        .windows(2)
        .enumerate()
        .map(|(index, pair)| CanvasAlternativeEdge {
            id: CanvasEdgeSemanticId::from_stable_key(&format!("edge-{index}")),
            source: pair[0].id,
            target: pair[1].id,
            kind: CanvasAlternativeEdgeKind::Dependency,
            enabled: true,
        })
        .collect();
    AgentCanvasSemanticSnapshot {
        generation: 1,
        revision: 1,
        workspace_id: 1,
        state: if count == 0 {
            AgentCanvasSurfaceState::Empty
        } else {
            AgentCanvasSurfaceState::Ready
        },
        mode: AgentCanvasPresentationMode::Graph,
        recording_friendly: false,
        focused: rows.first().map(|row| row.id),
        rows,
        edges,
    }
}

#[test]
fn graph_and_list_semantics_are_equivalent_and_shell_routable() {
    let graph = snapshot(100);
    let mut list = graph.clone();
    list.mode = AgentCanvasPresentationMode::ListInspector;
    assert_eq!(
        graph
            .ordered_rows()
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        list.ordered_rows()
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>()
    );

    let shell = ShellSemanticSnapshot {
        agent_canvas: Some(graph),
        ..ShellSemanticSnapshot::default()
    };
    let tree = shell.try_tree().unwrap();
    shell.try_router(&tree).unwrap();
    assert!(
        tree.nodes()
            .values()
            .any(|node| node.parent == Some(shell_region_semantic_node(ShellRegionId::Content)))
    );
}

#[test]
fn recording_projection_excludes_titles_prompts_output_and_paths() {
    let canaries = [
        "prompt-canary-must-not-escape",
        "output-canary-must-not-escape",
        "/private/path/canary",
        "token-canary-must-not-escape",
    ];
    let mut surface = snapshot(canaries.len());
    surface.recording_friendly = true;
    for (row, canary) in surface.rows.iter_mut().zip(canaries) {
        row.title = Some(canary.to_string());
    }
    let nodes = surface
        .try_nodes(shell_region_semantic_node(ShellRegionId::Content))
        .unwrap();
    let rendered = format!("{nodes:?}");
    assert!(canaries.iter().all(|canary| !rendered.contains(canary)));
    assert!(nodes.iter().all(|node| {
        !matches!(node.name, Some(SemanticText::UserText(_)))
            && !matches!(node.description, Some(SemanticText::UserText(_)))
    }));
}

#[test]
fn one_thousand_node_semantic_projection_is_bounded() {
    let surface = snapshot(1_000);
    let started = Instant::now();
    let nodes = surface
        .try_nodes(shell_region_semantic_node(ShellRegionId::Content))
        .unwrap();
    assert!(nodes.len() < termirust_ui_contract::MAX_SEMANTIC_NODES);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        agent_canvas_presentation_mode(400),
        Some(AgentCanvasPresentationMode::ListInspector)
    );
}
