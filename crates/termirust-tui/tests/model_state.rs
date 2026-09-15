use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use termirust_tui::render::{RenderOptions, render};
use termirust_tui::{
    FleetHealth, FleetProject, FleetRevision, FleetSession, FleetSnapshot, LoadState,
    MAX_VISIBLE_SESSIONS, ModelAction, ModelEffect, ProjectAvailability, TuiDiagnostic, TuiModel,
};

fn large_snapshot() -> FleetSnapshot {
    let project = FleetProject {
        id: "project".into(),
        name: "Large project".into(),
        availability: ProjectAvailability::Available,
        groups: Vec::new(),
    };
    let sessions = (0..MAX_VISIBLE_SESSIONS)
        .map(|index| FleetSession {
            id: format!("session-{index:05}"),
            project_id: project.id.clone(),
            group_id: None,
            title: format!("Session {index:05}"),
            state: if index % 2 == 0 { "live" } else { "offline" }.into(),
            activity: if index % 3 == 0 { "busy" } else { "idle" }.into(),
            unread: index % 5 == 0,
            pinned: index % 7 == 0,
            archived: false,
            revision: index as u64,
        })
        .collect();
    FleetSnapshot {
        revision: FleetRevision {
            projects: 1,
            sessions: MAX_VISIBLE_SESSIONS as u64,
        },
        projects: vec![project],
        sessions,
        health: FleetHealth::Healthy,
        skipped_records: 0,
    }
}

#[test]
fn ten_thousand_sessions_refresh_filter_and_navigation_remain_bounded() {
    let started = Instant::now();
    let mut model = TuiModel::default();
    assert_eq!(
        model.reduce(ModelAction::BeginRefresh),
        ModelEffect::StartRefresh(1)
    );
    model.reduce(ModelAction::RefreshSucceeded {
        generation: 1,
        snapshot: large_snapshot(),
    });
    assert_eq!(model.visible_sessions().count(), MAX_VISIBLE_SESSIONS);
    model.reduce(ModelAction::FocusNext);
    for _ in 0..500 {
        model.reduce(ModelAction::Move(1));
    }
    assert_eq!(model.selected_session().unwrap().id, "session-00500");
    model.reduce(ModelAction::StartFilter);
    for character in "09999".chars() {
        model.reduce(ModelAction::FilterCharacter(character));
    }
    assert_eq!(model.visible_sessions().count(), 1);
    assert_eq!(model.selected_session().unwrap().id, "session-09999");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn ten_thousand_sessions_render_only_the_visible_viewport() {
    let mut model = TuiModel::default();
    model.reduce(ModelAction::BeginRefresh);
    model.reduce(ModelAction::RefreshSucceeded {
        generation: 1,
        snapshot: large_snapshot(),
    });
    model.reduce(ModelAction::FocusNext);
    for _ in 0..9_900 {
        model.reduce(ModelAction::Move(1));
    }

    let backend = TestBackend::new(140, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    let started = Instant::now();
    terminal
        .draw(|frame| render(frame, &model, RenderOptions::default()))
        .unwrap();
    let elapsed = started.elapsed();

    eprintln!("10k-session viewport render: {elapsed:?}");
    assert!(
        elapsed < Duration::from_millis(500),
        "render took {elapsed:?}"
    );
}

#[test]
fn failed_refresh_preserves_last_safe_snapshot_and_exposes_recovery() {
    let mut model = TuiModel::default();
    model.reduce(ModelAction::BeginRefresh);
    model.reduce(ModelAction::RefreshSucceeded {
        generation: 1,
        snapshot: large_snapshot(),
    });
    model.reduce(ModelAction::BeginRefresh);
    model.reduce(ModelAction::RefreshFailed {
        generation: 2,
        diagnostic: TuiDiagnostic {
            code: "store-corrupt",
            summary: "Metadata is corrupt",
            recovery: "Use desktop diagnostics.",
        },
        recovery_required: true,
    });
    assert_eq!(model.load_state(), LoadState::RecoveryRequired);
    assert_eq!(model.visible_sessions().count(), MAX_VISIBLE_SESSIONS);
    assert_eq!(model.diagnostic().unwrap().code, "store-corrupt");
}
