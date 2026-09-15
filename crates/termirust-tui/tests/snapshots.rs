use ratatui::Terminal;
use ratatui::backend::TestBackend;
use termirust_tui::localization::TuiLocale;
use termirust_tui::render::{RenderOptions, render};
use termirust_tui::{
    FleetHealth, FleetProject, FleetRevision, FleetSession, FleetSnapshot, ModelAction,
    ProjectAvailability, TuiModel,
};

fn model() -> TuiModel {
    let mut model = TuiModel::default();
    model.reduce(ModelAction::BeginRefresh);
    model.reduce(ModelAction::RefreshSucceeded {
        generation: 1,
        snapshot: FleetSnapshot {
            revision: FleetRevision {
                projects: 4,
                sessions: 9,
            },
            projects: vec![FleetProject {
                id: "p".into(),
                name: "Deployment".into(),
                availability: ProjectAvailability::PermissionDenied,
                groups: Vec::new(),
            }],
            sessions: vec![FleetSession {
                id: "s".into(),
                project_id: "p".into(),
                group_id: None,
                title: "Production review".into(),
                state: "permission_denied".into(),
                activity: "needs_input".into(),
                unread: true,
                pinned: false,
                archived: false,
                revision: 9,
            }],
            health: FleetHealth::Partial,
            skipped_records: 2,
        },
    });
    model
}

fn draw(width: u16, height: u16, options: RenderOptions) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| render(frame, &model(), options))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn snapshots_keep_status_textual_and_hide_sensitive_labels_when_requested() {
    let normal = draw(140, 30, RenderOptions::default());
    assert!(normal.contains("permission denied"));
    assert!(normal.contains("permission_denied / needs_input"));
    assert!(normal.contains("partial; 2 skipped"));

    let hidden = draw(
        80,
        20,
        RenderOptions {
            no_color: true,
            recording_friendly: true,
            locale: TuiLocale::PseudoRtl,
        },
    );
    assert!(hidden.contains("[project hidden]"));
    assert!(hidden.contains("[session hidden]"));
    assert!(!hidden.contains("Deployment"));
    assert!(!hidden.contains("Production review"));
}
