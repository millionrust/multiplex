use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::localization::{TextId, TuiLocale, text};
use crate::model::{FleetHealth, LoadState, PaneFocus, ProjectAvailability, ScopeId, TuiModel};

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 20;
const WIDE_WIDTH: u16 = 120;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderOptions {
    pub no_color: bool,
    pub recording_friendly: bool,
    pub locale: TuiLocale,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            no_color: false,
            recording_friendly: false,
            locale: TuiLocale::English,
        }
    }
}

pub fn render(frame: &mut Frame<'_>, model: &TuiModel, options: RenderOptions) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_small(frame, area, options);
        return;
    }

    let [header, filter, content, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(12),
        Constraint::Length(2),
    ])
    .areas(area);
    render_header(frame, header, model, options);
    render_filter(frame, filter, model, options);
    render_content(frame, content, model, options);
    render_status(frame, status, model, options);
    if model.help_visible() {
        render_help(frame, centered(area, 72, 9), options);
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, model: &TuiModel, options: RenderOptions) {
    let state = state_label(model.load_state(), options.locale);
    let title = Line::from(vec![
        Span::styled(
            text(options.locale, TextId::AppTitle),
            emphasis(options, true),
        ),
        Span::raw("  "),
        Span::styled(text(options.locale, TextId::ReadOnly), muted(options)),
        Span::raw("  "),
        Span::styled(state, state_style(model.load_state(), options)),
    ]);
    frame.render_widget(Paragraph::new(title), area);
}

fn render_filter(frame: &mut Frame<'_>, area: Rect, model: &TuiModel, options: RenderOptions) {
    let marker = if model.filter_editing() { ">" } else { "/" };
    let value = if model.filter().is_empty() {
        "-"
    } else {
        model.filter()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} {marker}: ", text(options.locale, TextId::Filter)),
                muted(options),
            ),
            Span::raw(value),
        ])),
        area,
    );
}

fn render_content(frame: &mut Frame<'_>, area: Rect, model: &TuiModel, options: RenderOptions) {
    if area.width >= WIDE_WIDTH && model.inspector_visible() {
        let [projects, sessions, inspector] = Layout::horizontal([
            Constraint::Percentage(28),
            Constraint::Percentage(44),
            Constraint::Percentage(28),
        ])
        .areas(area);
        render_projects(frame, projects, model, options);
        render_sessions(frame, sessions, model, options);
        render_inspector(frame, inspector, model, options);
    } else {
        let [projects, primary] =
            Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(66)])
                .areas(area);
        render_projects(frame, projects, model, options);
        if model.inspector_visible() && model.focus() == PaneFocus::Inspector {
            render_inspector(frame, primary, model, options);
        } else {
            render_sessions(frame, primary, model, options);
        }
    }
}

fn render_projects(frame: &mut Frame<'_>, area: Rect, model: &TuiModel, options: RenderOptions) {
    let selected = model.selected_scope_index();
    let visible = usize::from(area.height.saturating_sub(2));
    let start = scroll_start(selected, visible, model.scope_rows().len());
    let items = model
        .scope_rows()
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, scope)| {
            let label = scope_label(model, scope, options);
            ListItem::new(label).style(row_style(
                index == selected,
                model.focus() == PaneFocus::Projects,
                options,
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(panel(
            text(options.locale, TextId::Projects),
            model.focus() == PaneFocus::Projects,
            options,
        )),
        area,
    );
}

fn render_sessions(frame: &mut Frame<'_>, area: Rect, model: &TuiModel, options: RenderOptions) {
    let selected = model.selected_session_index();
    let visible = usize::from(area.height.saturating_sub(2));
    let total = model.visible_sessions().count();
    let start = scroll_start(selected, visible, total);
    let items = model
        .visible_sessions()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, session)| {
            let marker = if session.unread { "*" } else { " " };
            let archived = if session.archived { " archived" } else { "" };
            let title = display_user(&session.title, "session", options);
            ListItem::new(Line::from(vec![
                Span::raw(format!("{marker} {title}")),
                Span::styled(
                    format!("  {} / {}{archived}", session.state, session.activity),
                    muted(options),
                ),
            ]))
            .style(row_style(
                index == selected,
                model.focus() == PaneFocus::Sessions,
                options,
            ))
        })
        .collect::<Vec<_>>();
    let widget = if items.is_empty() {
        List::new(vec![ListItem::new(text(options.locale, TextId::Empty))])
    } else {
        List::new(items)
    };
    frame.render_widget(
        widget.block(panel(
            text(options.locale, TextId::Sessions),
            model.focus() == PaneFocus::Sessions,
            options,
        )),
        area,
    );
}

fn render_inspector(frame: &mut Frame<'_>, area: Rect, model: &TuiModel, options: RenderOptions) {
    let lines = model.selected_session().map_or_else(
        || vec![Line::from(text(options.locale, TextId::Empty))],
        |session| {
            vec![
                Line::from(display_user(&session.title, "session", options)),
                Line::from(format!("State: {}", session.state)),
                Line::from(format!("Activity: {}", session.activity)),
                Line::from(format!("Unread: {}", yes_no(session.unread))),
                Line::from(format!("Archived: {}", yes_no(session.archived))),
                Line::from(format!("Revision: {}", session.revision)),
                Line::from(""),
                Line::from(Span::styled(
                    "Terminal output, paths, commands and credentials are not exposed.",
                    muted(options),
                )),
            ]
        },
    );
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel(
            text(options.locale, TextId::Inspector),
            model.focus() == PaneFocus::Inspector,
            options,
        )),
        area,
    );
}

fn render_status(frame: &mut Frame<'_>, area: Rect, model: &TuiModel, options: RenderOptions) {
    let summary = if let Some(diagnostic) = model.diagnostic() {
        format!(
            "{}: {}  {}",
            diagnostic.code, diagnostic.summary, diagnostic.recovery
        )
    } else if let Some(snapshot) = model.snapshot() {
        let health = match snapshot.health {
            FleetHealth::Healthy => "healthy".to_string(),
            FleetHealth::RecoveredLastGood => "recovered last-good; review required".to_string(),
            FleetHealth::Partial => format!("partial; {} skipped", snapshot.skipped_records),
        };
        format!(
            "{} projects  {} sessions  {health}  revisions {}/{}",
            snapshot.projects.len(),
            snapshot.sessions.len(),
            snapshot.revision.projects,
            snapshot.revision.sessions
        )
    } else {
        state_label(model.load_state(), options.locale)
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(summary),
            Line::from(Span::styled(
                "? help  r refresh  Esc cancel/clear  q quit",
                muted(options),
            )),
        ])
        .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_help(frame: &mut Frame<'_>, area: Rect, options: RenderOptions) {
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(text(options.locale, TextId::HelpKeys)),
            Line::from("Left/Right collapse or expand Projects. Enter only changes selection."),
            Line::from("This interface cannot attach, send input, launch, stop, archive or write."),
            Line::from("Use --inline, --no-color or --recording-friendly when needed."),
        ])
        .wrap(Wrap { trim: true })
        .block(panel(
            text(options.locale, TextId::HelpTitle),
            true,
            options,
        )),
        area,
    );
}

fn render_small(frame: &mut Frame<'_>, area: Rect, options: RenderOptions) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                text(options.locale, TextId::AppTitle),
                emphasis(options, true),
            )),
            Line::from(""),
            Line::from(text(options.locale, TextId::SmallTerminal)),
            Line::from(format!("Current size: {}x{}", area.width, area.height)),
            Line::from("Press q to quit."),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        area,
    );
}

fn scope_label(model: &TuiModel, scope: &ScopeId, options: RenderOptions) -> String {
    match scope {
        ScopeId::All => text(options.locale, TextId::AllSessions),
        ScopeId::Project(id) => model
            .snapshot()
            .and_then(|snapshot| snapshot.projects.iter().find(|project| &project.id == id))
            .map(|project| {
                let marker = if model.is_project_expanded(id) {
                    "v"
                } else {
                    ">"
                };
                let state = match project.availability {
                    ProjectAvailability::Available => "",
                    ProjectAvailability::Unavailable => " [unavailable]",
                    ProjectAvailability::PermissionDenied => " [permission denied]",
                };
                format!(
                    "{marker} {}{state}",
                    display_user(&project.name, "project", options)
                )
            })
            .unwrap_or_else(|| "[missing project]".into()),
        ScopeId::Group(id) => model
            .snapshot()
            .and_then(|snapshot| {
                snapshot
                    .projects
                    .iter()
                    .flat_map(|project| project.groups.iter())
                    .find(|group| &group.id == id)
            })
            .map(|group| format!("  - {}", display_user(&group.name, "group", options)))
            .unwrap_or_else(|| "  - [missing group]".into()),
    }
}

fn panel(title: String, focused: bool, options: RenderOptions) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(if focused {
            emphasis(options, false)
        } else {
            muted(options)
        })
}

fn display_user(value: &str, kind: &str, options: RenderOptions) -> String {
    if options.recording_friendly {
        return format!("[{kind} hidden]");
    }
    format!("\u{2068}{value}\u{2069}")
}

fn row_style(selected: bool, focused: bool, options: RenderOptions) -> Style {
    if !selected {
        return Style::default();
    }
    let style = Style::default().add_modifier(Modifier::REVERSED);
    if options.no_color || !focused {
        style
    } else {
        style.fg(Color::Black).bg(Color::Cyan)
    }
}

fn emphasis(options: RenderOptions, strong: bool) -> Style {
    let style = if strong {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    if options.no_color {
        style
    } else {
        style.fg(Color::Cyan)
    }
}

fn muted(options: RenderOptions) -> Style {
    if options.no_color {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn state_style(state: LoadState, options: RenderOptions) -> Style {
    if options.no_color {
        return Style::default().add_modifier(Modifier::BOLD);
    }
    let color = match state {
        LoadState::Ready | LoadState::Empty => Color::Green,
        LoadState::Starting | LoadState::Loading => Color::Yellow,
        LoadState::Partial | LoadState::RecoveryRequired => Color::Magenta,
        LoadState::Unavailable => Color::Red,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn state_label(state: LoadState, locale: TuiLocale) -> String {
    match state {
        LoadState::Starting | LoadState::Loading => text(locale, TextId::Loading),
        LoadState::Ready => "Ready".into(),
        LoadState::Empty => text(locale, TextId::Empty),
        LoadState::Partial => text(locale, TextId::Partial),
        LoadState::Unavailable => text(locale, TextId::Unavailable),
        LoadState::RecoveryRequired => text(locale, TextId::RecoveryRequired),
    }
}

fn scroll_start(selected: usize, visible: usize, total: usize) -> usize {
    if visible == 0 || total <= visible {
        0
    } else {
        selected
            .saturating_sub(visible.saturating_sub(1))
            .min(total.saturating_sub(visible))
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::model::{
        FleetProject, FleetRevision, FleetSession, FleetSnapshot, ModelAction, ProjectAvailability,
    };

    fn ready_model() -> TuiModel {
        let mut model = TuiModel::default();
        model.reduce(ModelAction::BeginRefresh);
        model.reduce(ModelAction::RefreshSucceeded {
            generation: 1,
            snapshot: FleetSnapshot {
                revision: FleetRevision {
                    projects: 1,
                    sessions: 2,
                },
                projects: vec![FleetProject {
                    id: "p1".into(),
                    name: "Alpha".into(),
                    availability: ProjectAvailability::Available,
                    groups: Vec::new(),
                }],
                sessions: vec![FleetSession {
                    id: "s1".into(),
                    project_id: "p1".into(),
                    group_id: None,
                    title: "Build".into(),
                    state: "live".into(),
                    activity: "busy".into(),
                    unread: true,
                    archived: false,
                    revision: 2,
                }],
                health: FleetHealth::Healthy,
                skipped_records: 0,
            },
        });
        model
    }

    fn rendered(width: u16, height: u16, model: &TuiModel, options: RenderOptions) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, model, options))
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
    fn wide_compact_small_and_recording_friendly_layouts_are_readable() {
        let model = ready_model();
        let wide = rendered(140, 32, &model, RenderOptions::default());
        assert!(wide.contains("Projects"));
        assert!(wide.contains("Sessions"));
        assert!(wide.contains("Inspector"));
        assert!(wide.contains("Build"));

        let compact = rendered(90, 24, &model, RenderOptions::default());
        assert!(compact.contains("Projects"));
        assert!(compact.contains("Sessions"));

        let small = rendered(60, 10, &model, RenderOptions::default());
        assert!(small.contains("at least 80 columns"));

        let recording = rendered(
            120,
            24,
            &model,
            RenderOptions {
                recording_friendly: true,
                no_color: true,
                ..RenderOptions::default()
            },
        );
        assert!(recording.contains("[project hidden]"));
        assert!(recording.contains("[session hidden]"));
        assert!(!recording.contains("Alpha"));
        assert!(!recording.contains("Build"));
    }

    #[test]
    fn pseudo_locale_and_help_reflow_without_panicking() {
        let mut model = ready_model();
        model.reduce(ModelAction::ToggleHelp);
        let output = rendered(
            80,
            20,
            &model,
            RenderOptions {
                locale: TuiLocale::PseudoExpanded,
                no_color: true,
                recording_friendly: false,
            },
        );
        assert!(output.contains("Keyboard help"));
        assert!(output.contains("cannot attach"));
    }
}
