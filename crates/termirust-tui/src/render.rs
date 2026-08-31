use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::localization::{TextId, TuiLocale, localize, text};
use crate::management::{
    CommandProgress, ConfirmationKind, ManagementDraft, ManagementIntent, ManagementModel,
};
use crate::model::{FleetHealth, LoadState, PaneFocus, ProjectAvailability, ScopeId, TuiModel};
use crate::{
    AttachedTerminal, DeviceProgress, DevicesModel, InteractiveLease, TuiAttachState, TuiDevice,
    TuiFocus,
};

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

pub fn render_devices(frame: &mut Frame<'_>, model: &DevicesModel, options: RenderOptions) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_small(frame, area, options);
        return;
    }
    let [header, content, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(12),
        Constraint::Length(4),
    ])
    .areas(area);
    render_devices_header(frame, header, model, options);
    let [list, inspector] = Layout::horizontal([
        Constraint::Percentage(if area.width >= WIDE_WIDTH { 38 } else { 44 }),
        Constraint::Percentage(if area.width >= WIDE_WIDTH { 62 } else { 56 }),
    ])
    .areas(content);
    render_device_list(frame, list, model, options);
    render_device_inspector(frame, inspector, model, options);
    render_devices_status(frame, status, model, options);
    if model.help_visible() {
        render_devices_help(frame, centered(area, 70, 9), options);
    }
    if matches!(
        model.progress(),
        DeviceProgress::LoadingReview | DeviceProgress::Reviewing | DeviceProgress::Revoking
    ) {
        render_device_review(frame, centered(area, 72, 13), model, options);
    }
}

fn render_devices_header(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DevicesModel,
    options: RenderOptions,
) {
    let revision = model.snapshot().map_or_else(
        || "-".into(),
        |snapshot| snapshot.repository_revision.to_string(),
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    localize(options.locale, "TermiRust Devices"),
                    emphasis(options, true),
                ),
                Span::raw("  "),
                Span::styled(
                    localize(options.locale, "Local Controller authority"),
                    muted(options),
                ),
            ]),
            Line::from(Span::styled(
                format!(
                    "{} {revision}  {} {}",
                    localize(options.locale, "repository revision"),
                    localize(options.locale, "state"),
                    device_progress_label(model.progress()),
                ),
                muted(options),
            )),
        ]),
        area,
    );
}

fn render_device_list(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DevicesModel,
    options: RenderOptions,
) {
    let selected = model.selected_index();
    let devices = model
        .snapshot()
        .map_or(&[][..], |snapshot| snapshot.devices.as_slice());
    let visible = usize::from(area.height.saturating_sub(2));
    let start = scroll_start(selected, visible, devices.len());
    let items = devices
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, device)| {
            let name = display_user(&device.name, "device", options);
            ListItem::new(Line::from(vec![
                Span::raw(format!("{name}  ")),
                Span::styled(device.status.to_uppercase(), muted(options)),
            ]))
            .style(row_style(index == selected, true, options))
        })
        .collect::<Vec<_>>();
    let widget = if items.is_empty() {
        List::new(vec![ListItem::new(localize(
            options.locale,
            if matches!(model.progress(), DeviceProgress::Loading) {
                "Loading paired devices..."
            } else {
                "No paired devices"
            },
        ))])
    } else {
        List::new(items)
    };
    frame.render_widget(
        widget.block(panel(
            localize(options.locale, "Paired devices"),
            true,
            options,
        )),
        area,
    );
}

fn render_device_inspector(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DevicesModel,
    options: RenderOptions,
) {
    let lines = model.selected_device().map_or_else(
        || {
            vec![Line::from(localize(
                options.locale,
                "Select an active paired device to inspect or revoke access.",
            ))]
        },
        |device| device_detail_lines(device, options),
    );
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel(
            localize(options.locale, "Device details"),
            false,
            options,
        )),
        area,
    );
}

fn device_detail_lines(device: &TuiDevice, options: RenderOptions) -> Vec<Line<'static>> {
    let name = display_user(&device.name, "device", options);
    let id = display_user(&device.id, "device ID", options);
    let fingerprint = display_user(&device.fingerprint_suffix, "fingerprint suffix", options);
    vec![
        Line::from(name),
        Line::from(format!("{} {id}", localize(options.locale, "ID:"))),
        Line::from(format!(
            "{} {}",
            localize(options.locale, "Status:"),
            device.status
        )),
        Line::from(format!(
            "{} {}",
            localize(options.locale, "Capabilities:"),
            if device.capabilities.is_empty() {
                localize(options.locale, "none")
            } else {
                device.capabilities.join(", ")
            }
        )),
        Line::from(format!(
            "{} {} to {}",
            localize(options.locale, "Protocol:"),
            device.protocol_minimum,
            device.protocol_maximum
        )),
        Line::from(format!(
            "{} {}",
            localize(options.locale, "Created:"),
            device.created_at_unix_seconds
        )),
        Line::from(format!(
            "{} {}",
            localize(options.locale, "Last seen:"),
            device.last_seen_at_unix_seconds.map_or_else(
                || localize(options.locale, "never"),
                |value| value.to_string()
            )
        )),
        Line::from(format!(
            "{} {fingerprint}",
            localize(options.locale, "Fingerprint suffix:")
        )),
        Line::from(format!(
            "{} {}",
            localize(options.locale, "Identity generation:"),
            device.identity_generation
        )),
        Line::from(""),
        Line::from(Span::styled(
            localize(
                options.locale,
                "Keys, pairing routes, credentials and terminal content are not exposed.",
            ),
            muted(options),
        )),
    ]
}

fn render_devices_status(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DevicesModel,
    options: RenderOptions,
) {
    let mut lines = match model.progress() {
        DeviceProgress::Failed(error) => {
            let mut lines = vec![Line::from(format!(
                "[{}] {}  {}",
                error.code, error.summary, error.recovery
            ))];
            if let Some(revision) = error.conflict_revision {
                lines.push(Line::from(format!(
                    "{} {revision}",
                    localize(options.locale, "Current repository revision:")
                )));
            }
            lines
        }
        DeviceProgress::Succeeded { summary } => {
            vec![Line::from(localize(options.locale, summary))]
        }
        DeviceProgress::Loading => vec![Line::from(localize(
            options.locale,
            "Reading the bounded local paired-device authority...",
        ))],
        _ => {
            let count = model
                .snapshot()
                .map_or(0, |snapshot| snapshot.devices.len());
            vec![Line::from(format!(
                "{count} {}",
                localize(options.locale, "paired devices")
            ))]
        }
    };
    lines.push(Line::from(Span::styled(
        localize(
            options.locale,
            "j/k or arrows select  x review revoke  r refresh  f/Esc fleet  ? help  q quit",
        ),
        muted(options),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn render_device_review(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DevicesModel,
    options: RenderOptions,
) {
    frame.render_widget(Clear, area);
    let lines = match model.progress() {
        DeviceProgress::LoadingReview => vec![
            Line::from(localize(
                options.locale,
                "Preparing a fresh authoritative revocation review...",
            )),
            Line::from(localize(
                options.locale,
                "Esc cancels this preview without changing device access.",
            )),
        ],
        DeviceProgress::Reviewing => model.review().map_or_else(
            || vec![Line::from(localize(options.locale, "Review unavailable."))],
            |review| {
                vec![
                    Line::from(format!(
                        "{} {}",
                        localize(options.locale, "Target:"),
                        display_user(&review.device.name, "device", options)
                    )),
                    Line::from(format!(
                        "{} {}",
                        localize(options.locale, "Device ID:"),
                        display_user(&review.device.id, "device ID", options)
                    )),
                    Line::from(format!(
                        "{} {}",
                        localize(options.locale, "Repository revision:"),
                        review.repository_revision
                    )),
                    Line::from(localize(
                        options.locale,
                        "Active access for this exact device will be revoked immediately.",
                    )),
                    Line::from(localize(
                        options.locale,
                        if review.other_devices_reconnect {
                            "Other connected devices must reconnect after the authority epoch changes."
                        } else {
                            "No other device is currently marked online."
                        },
                    )),
                    Line::from(""),
                    Line::from(localize(
                        options.locale,
                        "Enter revoke once | Esc cancel (safe default)",
                    )),
                ]
            },
        ),
        DeviceProgress::Revoking => vec![
            Line::from(localize(
                options.locale,
                "Committing one exact revision-checked device revocation...",
            )),
            Line::from(localize(
                options.locale,
                "This command cannot be cancelled or retried silently. Wait for the result.",
            )),
        ],
        _ => Vec::new(),
    };
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel(
            localize(options.locale, "Device revocation"),
            true,
            options,
        )),
        area,
    );
}

fn render_devices_help(frame: &mut Frame<'_>, area: Rect, options: RenderOptions) {
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(localize(
                options.locale,
                "The Devices screen reads the same local authority as desktop and CLI.",
            )),
            Line::from(localize(
                options.locale,
                "j/k or arrows select. r refreshes without creating missing state.",
            )),
            Line::from(localize(
                options.locale,
                "x prepares a fresh review; Enter then revokes once at that exact revision.",
            )),
            Line::from(localize(
                options.locale,
                "Esc is always the safe cancel before commit. f returns to the fleet.",
            )),
            Line::from(localize(
                options.locale,
                "Device keys, pairing routes, credentials and terminal content stay hidden.",
            )),
        ])
        .wrap(Wrap { trim: true })
        .block(panel(
            localize(options.locale, "Devices keyboard help"),
            true,
            options,
        )),
        area,
    );
}

fn device_progress_label(progress: &DeviceProgress) -> &'static str {
    match progress {
        DeviceProgress::Idle => "idle",
        DeviceProgress::Loading => "loading",
        DeviceProgress::Ready => "ready",
        DeviceProgress::LoadingReview => "preparing review",
        DeviceProgress::Reviewing => "reviewing",
        DeviceProgress::Revoking => "revoking",
        DeviceProgress::Succeeded { .. } => "updated",
        DeviceProgress::Failed(_) => "attention required",
    }
}

pub fn render_attached(
    frame: &mut Frame<'_>,
    attached: &AttachedTerminal,
    notice: Option<&str>,
    options: RenderOptions,
) {
    let area = frame.area();
    if area.width < 20 || area.height < 8 {
        render_attached_small(frame, area, options);
        return;
    }
    let [header, terminal, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .areas(area);
    let lease = match attached.input().lease() {
        InteractiveLease::Interactive => "Interactive",
        InteractiveLease::ViewOnly => "View only",
    };
    let durability = if attached.recording_paused() {
        "recording paused"
    } else if attached.durable_sequence() < attached.watermark() {
        "durability pending"
    } else {
        "durable"
    };
    let title = if options.recording_friendly {
        "[session hidden]".to_string()
    } else {
        display_user(attached.title(), "session", options)
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(title, emphasis(options, true)),
                Span::raw(format!(
                    "  {}  {}  local Host  {lease}",
                    attach_state_label(attached.state()),
                    attached.lifecycle().label(),
                )),
            ]),
            Line::from(Span::styled(
                format!(
                    "sequence {} / latest {} / durable {}  {durability}",
                    attached.watermark(),
                    attached.latest_sequence(),
                    attached.durable_sequence(),
                ),
                muted(options),
            )),
        ]),
        header,
    );
    if options.recording_friendly {
        frame.render_widget(
            Paragraph::new("Terminal output hidden in recording-friendly mode")
                .alignment(Alignment::Center),
            terminal,
        );
    } else {
        attached
            .terminal()
            .render(frame, terminal, options.no_color);
    }

    let recovery = notice
        .or(attached.diagnostic())
        .unwrap_or(match attached.state() {
            TuiAttachState::Attaching => "Connecting to the durable Host...",
            TuiAttachState::Replaying => "Replaying retained output...",
            TuiAttachState::LiveInteractive => "Input is sent directly to the current Host lease.",
            TuiAttachState::LiveReadOnly => "Another client may own input. Press i to request it.",
            TuiAttachState::Gap | TuiAttachState::Unavailable => {
                "Press r to retry from the Host journal."
            }
            TuiAttachState::Exited => "Process exited. Retained output remains read-only.",
            TuiAttachState::Detached => "Detached. The Host continues in the background.",
        });
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(recovery),
            Line::from(Span::styled(
                "Ctrl+Space then Esc: detach  Ctrl+Space then Space: send NUL",
                muted(options),
            )),
            Line::from(Span::styled(
                format!(
                    "replay: {} records / {} bytes  focus: {}",
                    attached.replay_records(),
                    attached.replay_bytes(),
                    focus_label(attached.input().focus()),
                ),
                muted(options),
            )),
        ]),
        status,
    );

    if attached.input().focus() == TuiFocus::Leader {
        let overlay = centered(area, 52, 5);
        frame.render_widget(Clear, overlay);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("Leader active"),
                Line::from("Esc detach | Space send NUL | other key sends NUL + key"),
            ])
            .alignment(Alignment::Center)
            .block(panel("Terminal command".into(), true, options)),
            overlay,
        );
    } else if let Some(bytes) = attached.input().pending_paste() {
        let overlay = centered(area, 58, 6);
        frame.render_widget(Clear, overlay);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("Confirm terminal paste"),
                Line::from(format!(
                    "{} bytes will be sent to the active session.",
                    bytes.len()
                )),
                Line::from("The paste contents are intentionally not previewed or logged."),
                Line::from("Enter confirm | Esc cancel"),
            ])
            .alignment(Alignment::Center)
            .block(panel("Paste guard".into(), true, options)),
            overlay,
        );
    }
}

pub fn render_management(
    frame: &mut Frame<'_>,
    management: &ManagementModel,
    options: RenderOptions,
) {
    if !management.active() {
        return;
    }
    let area = centered(frame.area(), 74, 16);
    frame.render_widget(Clear, area);
    let mut lines = Vec::new();
    match management.progress() {
        CommandProgress::Idle => lines.push(Line::from(localize(
            options.locale,
            "Preparing Session management...",
        ))),
        CommandProgress::Running => {
            lines.push(Line::from(localize(
                options.locale,
                "Command in progress. The target remains fixed.",
            )));
            lines.push(Line::from(localize(
                options.locale,
                if management.intent() == Some(ManagementIntent::Remove)
                    && management.cancellation_available()
                {
                    "Esc cancels the bounded preview before any removal command starts."
                } else if management.cancellation_available() {
                    "Esc requests cancellation before the Host becomes ready."
                } else if management.intent() == Some(ManagementIntent::Stop) {
                    "Stop cannot be cancelled after the first Host signal."
                } else if management.intent() == Some(ManagementIntent::Remove) {
                    "Removal commit cannot be cancelled or undone. Wait for the exact result."
                } else {
                    "Wait for the revision-checked command result."
                },
            )));
        }
        CommandProgress::Succeeded {
            summary,
            undo_deadline,
        } => {
            lines.push(Line::from(localize(options.locale, summary)));
            lines.push(Line::from(localize(
                options.locale,
                if undo_deadline.is_some() {
                    "u undo within 10 seconds | Esc close"
                } else {
                    "Esc close"
                },
            )));
        }
        CommandProgress::Failed(error) => {
            lines.push(Line::from(format!("[{}] {}", error.code, error.summary)));
            lines.push(Line::from(error.recovery.clone()));
            if let Some(revision) = error.conflict_revision {
                lines.push(Line::from(format!(
                    "{} {revision}",
                    localize(options.locale, "Current revision:")
                )));
            }
            lines.push(Line::from(localize(
                options.locale,
                "Esc closes this result. Refresh before another command.",
            )));
        }
        CommandProgress::Reviewing => {
            render_management_draft(&mut lines, management.draft(), options)
        }
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel(
            localize(options.locale, "Session management"),
            true,
            options,
        )),
        area,
    );
}

fn render_management_draft(
    lines: &mut Vec<Line<'static>>,
    draft: Option<&ManagementDraft>,
    options: RenderOptions,
) {
    match draft {
        Some(ManagementDraft::LoadingLaunch { .. }) => lines.push(Line::from(localize(
            options.locale,
            "Loading bounded preset choices...",
        ))),
        Some(ManagementDraft::LoadingRemoval { target }) => {
            lines.push(management_target_line(target, options));
            lines.push(Line::from(localize(
                options.locale,
                "Scanning a bounded aggregate removal manifest...",
            )));
            lines.push(Line::from(localize(
                options.locale,
                "Esc cancels this preview without changing the Session.",
            )));
        }
        Some(ManagementDraft::Launch {
            project_name,
            choices,
            selected,
            ..
        }) => {
            let project = display_user(project_name, "project", options);
            lines.push(Line::from(format!(
                "{} {project}",
                localize(options.locale, "Project:")
            )));
            lines.push(Line::from(localize(
                options.locale,
                "Choose one enabled safe preset:",
            )));
            let start = selected.saturating_sub(3);
            for (index, choice) in choices.iter().enumerate().skip(start).take(7) {
                let marker = if index == *selected { ">" } else { " " };
                lines.push(Line::from(format!(
                    "{marker} {}",
                    display_user(&choice.label, "preset", options)
                )));
            }
            lines.push(Line::from(localize(
                options.locale,
                "Up/Down select | Enter launch once | Esc cancel",
            )));
        }
        Some(ManagementDraft::Rename { target, value }) => {
            lines.push(management_target_line(target, options));
            lines.push(Line::from(localize(
                options.locale,
                "New title (bounded text only):",
            )));
            lines.push(Line::from(format!(
                "> {}_",
                display_user(value, "title", options)
            )));
            lines.push(Line::from(localize(
                options.locale,
                "Enter save once | Esc cancel",
            )));
        }
        Some(ManagementDraft::Confirm { kind, target }) => {
            lines.push(management_target_line(target, options));
            lines.push(Line::from(localize(
                options.locale,
                confirmation_consequence(*kind),
            )));
            lines.push(Line::from(localize(
                options.locale,
                "Enter confirm once | Esc cancel (safe default)",
            )));
        }
        Some(ManagementDraft::Remove {
            target,
            preview,
            confirmation,
            confirmation_invalid,
        }) => {
            lines.push(management_target_line(target, options));
            lines.push(Line::from(localize(
                options.locale,
                "Remove metadata and quarantine exact owned data. No process is stopped and no undo is available.",
            )));
            lines.push(Line::from(format!(
                "{} {} B  {} {} B  {} {} B",
                localize(options.locale, "Metadata:"),
                preview.manifest.metadata_bytes,
                localize(options.locale, "Journal:"),
                preview.manifest.journal_bytes,
                localize(options.locale, "Transcript:"),
                preview.manifest.transcript_bytes,
            )));
            lines.push(Line::from(format!(
                "{} {} B  {} {} B  {} {}",
                localize(options.locale, "Artifacts:"),
                preview.manifest.artifact_bytes,
                localize(options.locale, "Total:"),
                preview.manifest.total_bytes(),
                localize(options.locale, "Files:"),
                preview.manifest.file_count,
            )));
            let instruction = if preview.manifest.requires_title_confirmation() {
                "Type the exact Session title to confirm:"
            } else {
                "Type REMOVE to confirm:"
            };
            lines.push(Line::from(localize(options.locale, instruction)));
            lines.push(Line::from(format!(
                "> {}_",
                display_user(confirmation, "confirmation", options)
            )));
            if *confirmation_invalid {
                lines.push(Line::from(localize(
                    options.locale,
                    "Confirmation does not match. Nothing was removed.",
                )));
            }
            lines.push(Line::from(localize(
                options.locale,
                "Enter remove once | Esc cancel (safe default)",
            )));
        }
        None => lines.push(Line::from(localize(
            options.locale,
            "No management draft is available.",
        ))),
    }
}

fn management_target_line(
    target: &crate::management::ManagementTarget,
    options: RenderOptions,
) -> Line<'static> {
    Line::from(format!(
        "{} {}  {} {}  {} {}",
        localize(options.locale, "Target:"),
        display_user(&target.title, "session", options),
        localize(options.locale, "state"),
        target.state,
        localize(options.locale, "revision"),
        target.revision,
    ))
}

fn confirmation_consequence(kind: ConfirmationKind) -> &'static str {
    match kind {
        ConfirmationKind::Pin => "Pin this Session in fleet ordering.",
        ConfirmationKind::Unpin => "Remove this Session from pinned ordering.",
        ConfirmationKind::MarkRead => {
            "Mark activity visible through the captured sequence as read."
        }
        ConfirmationKind::Stop => "Stop the exact owned Host process. This cannot be undone.",
        ConfirmationKind::StopAndArchive => {
            "Stop the exact owned Host, confirm exit, then archive its metadata. This cannot be undone as one action."
        }
        ConfirmationKind::Archive => "Archive exited Session metadata. No process will start.",
        ConfirmationKind::Restore => "Restore Session metadata only. No process will start.",
    }
}

fn attach_state_label(state: TuiAttachState) -> &'static str {
    match state {
        TuiAttachState::Detached => "DETACHED",
        TuiAttachState::Attaching => "ATTACHING",
        TuiAttachState::Replaying => "REPLAYING",
        TuiAttachState::LiveReadOnly => "LIVE READ-ONLY",
        TuiAttachState::LiveInteractive => "LIVE",
        TuiAttachState::Gap => "GAP",
        TuiAttachState::Exited => "EXITED",
        TuiAttachState::Unavailable => "UNAVAILABLE",
    }
}

fn focus_label(focus: TuiFocus) -> &'static str {
    match focus {
        TuiFocus::Fleet => "confirmation",
        TuiFocus::Terminal => "terminal",
        TuiFocus::Leader => "leader",
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
            let marker = match (session.unread, session.pinned) {
                (true, true) => "*!",
                (true, false) => "* ",
                (false, true) => " !",
                (false, false) => "  ",
            };
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
                "n launch  e rename  p pin  m read  s stop  a archive/restore  x remove  d devices  ? help",
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
            Line::from(
                "Left/Right collapse or expand Projects. Enter attaches the selected Session.",
            ),
            Line::from("Terminal: Ctrl+Space then Esc detaches without stopping the Host."),
            Line::from(
                "Fleet: n launch, e rename, p pin, m mark read, s stop, a archive/restore, x remove, d devices.",
            ),
            Line::from("Management keys are never interpreted while terminal input has focus."),
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

fn render_attached_small(frame: &mut Frame<'_>, area: Rect, options: RenderOptions) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                text(options.locale, TextId::AppTitle),
                emphasis(options, true),
            )),
            Line::from(""),
            Line::from("Terminal view needs at least 20 columns by 8 rows."),
            Line::from(format!("Current size: {}x{}", area.width, area.height)),
            Line::from("Ctrl+Space then Esc detaches without stopping the Host."),
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
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use termirust_domain::{HostInstanceId, HostedSessionId, OutputSequence};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::model::{
        FleetProject, FleetRevision, FleetSession, FleetSnapshot, ModelAction, ProjectAvailability,
    };
    use crate::{
        AttachBatch, AttachEvent, AttachedTerminal, HostAttachState, HostLifecycle, Viewport,
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
                    pinned: false,
                    archived: false,
                    revision: 2,
                }],
                health: FleetHealth::Healthy,
                skipped_records: 0,
            },
        });
        model
    }

    #[test]
    fn attached_terminal_reflows_at_eighty_columns_without_exposing_control_bytes() {
        let session_id = HostedSessionId::new();
        let mut attached =
            AttachedTerminal::new(1, session_id, "Build shell".into(), Viewport::new(80, 15));
        attached.apply(AttachEvent::Batch {
            generation: 1,
            batch: AttachBatch {
                host_instance_id: HostInstanceId::new(),
                snapshot: None,
                outputs: vec![termirust_client::SequencedOutput {
                    sequence: OutputSequence::new(1),
                    bytes: b"visible\x1b]0;hidden title\x07".to_vec(),
                }],
                state: HostAttachState {
                    lifecycle: HostLifecycle::Ready,
                    earliest_sequence: OutputSequence::new(1),
                    latest_sequence: OutputSequence::new(1),
                    durable_sequence: OutputSequence::new(1),
                    has_writer_lease: true,
                    recording_paused: false,
                },
            },
        });
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_attached(frame, &attached, None, RenderOptions::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Build shell"));
        assert!(rendered.contains("Interactive"));
        assert!(rendered.contains("visible"));
        assert!(!rendered.contains("hidden title"));
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
        assert!(output.contains("Fleet: n launch"));
        assert!(output.contains("[!!"));
    }

    #[test]
    fn management_confirmation_reflows_in_no_color_and_localized_modes() {
        let model = ready_model();
        let mut management = ManagementModel::default();
        management.begin_session(ManagementIntent::Stop, model.selected_session().unwrap());

        for locale in [
            TuiLocale::English,
            TuiLocale::PseudoExpanded,
            TuiLocale::PseudoRtl,
        ] {
            let backend = TestBackend::new(80, 20);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    render(
                        frame,
                        &model,
                        RenderOptions {
                            locale,
                            no_color: true,
                            recording_friendly: true,
                        },
                    );
                    render_management(
                        frame,
                        &management,
                        RenderOptions {
                            locale,
                            no_color: true,
                            recording_friendly: true,
                        },
                    );
                })
                .unwrap();
            let output = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(output.contains("Session management"));
            assert!(output.contains("Enter confirm once"));
            assert!(output.contains("[session hidden]"));
            assert!(!output.contains("Build"));
        }
    }

    #[test]
    fn removal_preview_is_aggregate_recording_safe_and_localized() {
        let model = ready_model();
        let mut session = model.selected_session().unwrap().clone();
        session.state = "exited".into();
        session.archived = true;
        session.title = "PRIVATE-SESSION-TITLE".into();
        let mut management = ManagementModel::default();
        management.begin_session(ManagementIntent::Remove, &session);
        management.removal_preview_loaded(
            management.generation(),
            Ok(crate::RemovalPreview {
                expected_revision: 12,
                manifest: termirust_cli::ManagementRemovalManifest {
                    metadata_bytes: 10,
                    journal_bytes: 20,
                    transcript_bytes: 30,
                    artifact_bytes: 40,
                    file_count: 4,
                },
            }),
        );
        management.append_paste("PRIVATE-CONFIRMATION");

        for locale in [
            TuiLocale::English,
            TuiLocale::PseudoExpanded,
            TuiLocale::PseudoRtl,
        ] {
            let backend = TestBackend::new(80, 20);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    render_management(
                        frame,
                        &management,
                        RenderOptions {
                            locale,
                            no_color: true,
                            recording_friendly: true,
                        },
                    );
                })
                .unwrap();
            let output = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(output.contains("Session management"));
            assert!(output.contains("100 B"));
            assert!(output.contains("4"));
            assert!(output.contains("[session hidden]"));
            assert!(output.contains("[confirmation hidden]"));
            assert!(!output.contains("PRIVATE-SESSION-TITLE"));
            assert!(!output.contains("PRIVATE-CONFIRMATION"));
        }
    }

    fn devices_model() -> DevicesModel {
        let mut model = DevicesModel::default();
        model.open();
        let generation = model.generation();
        model.loaded(
            generation,
            Ok(crate::DeviceSnapshot {
                repository_revision: 9,
                devices: vec![TuiDevice {
                    id: "00000000-0000-0000-0000-000000000001".into(),
                    name: "PRIVATE PHONE".into(),
                    status: "online".into(),
                    capabilities: vec!["observe_sessions".into(), "send_input".into()],
                    protocol_minimum: 1,
                    protocol_maximum: 1,
                    created_at_unix_seconds: 10,
                    last_seen_at_unix_seconds: Some(20),
                    fingerprint_suffix: "123456789abc".into(),
                    identity_generation: 2,
                }],
            }),
        );
        model
    }

    fn rendered_devices(
        width: u16,
        height: u16,
        model: &DevicesModel,
        options: RenderOptions,
    ) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_devices(frame, model, options))
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
    fn devices_screen_reflows_and_masks_user_identity_for_recording() {
        let model = devices_model();
        let wide = rendered_devices(140, 30, &model, RenderOptions::default());
        assert!(wide.contains("TermiRust Devices"));
        assert!(wide.contains("Paired devices"));
        assert!(wide.contains("Device details"));
        assert!(wide.contains("PRIVATE PHONE"));
        assert!(wide.contains("repository revision 9"));

        let compact = rendered_devices(
            80,
            20,
            &model,
            RenderOptions {
                no_color: true,
                ..RenderOptions::default()
            },
        );
        assert!(compact.contains("Paired devices"));
        assert!(compact.contains("x review revoke"));

        let recording = rendered_devices(
            100,
            24,
            &model,
            RenderOptions {
                no_color: true,
                recording_friendly: true,
                locale: TuiLocale::PseudoRtl,
            },
        );
        assert!(recording.contains("[device hidden]"));
        assert!(recording.contains("[device ID hidden]"));
        assert!(!recording.contains("PRIVATE PHONE"));
        assert!(!recording.contains("00000000-0000"));
        assert!(!recording.contains("123456789abc"));
    }

    #[test]
    fn device_revocation_review_is_exact_textual_and_cancel_first() {
        let mut model = devices_model();
        assert!(matches!(
            model.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
            crate::DeviceEffect::Review { .. }
        ));
        let generation = model.generation();
        model.reviewed(
            generation,
            Ok(crate::DeviceRevocationReview {
                repository_revision: 11,
                device: model.snapshot().unwrap().devices[0].clone(),
                active_access_will_be_revoked: true,
                other_devices_reconnect: true,
            }),
        );
        for locale in [
            TuiLocale::English,
            TuiLocale::PseudoExpanded,
            TuiLocale::PseudoRtl,
        ] {
            let output = rendered_devices(
                80,
                20,
                &model,
                RenderOptions {
                    no_color: true,
                    locale,
                    ..RenderOptions::default()
                },
            );
            assert!(output.contains("Device revocation"));
            assert!(output.contains("Repository revision:"));
            assert!(output.contains("11"));
            assert!(output.contains("Enter revoke once"));
            assert!(output.contains("Esc cancel (safe default)"));
            assert!(output.contains("must reconnect"));
        }
    }
}
