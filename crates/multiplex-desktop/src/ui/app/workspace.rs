//! Workspace shell rendering: search bar, autocomplete bar, files (SFTP)
//! view, terminal pane (cells/rows), workspace body and shell wrapper.
//! All methods are part of `MultiplexApp`.

use std::time::{Duration, Instant};

use gpui::AppContext as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, CursorStyle, Div, DragMoveEvent,
    ExternalPaths, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, ScrollWheelEvent, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, Window, div, px, relative,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Disableable as _, Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};

use crate::models::{ConnectionKind, WorkspaceLayoutMode};
use crate::ui::app::motion::{self, MotionRect};
use crate::ui::app::split_tree::{SplitEdge, SplitPreset, compute_split_layout};
use crate::ui::app::{
    ConnectDialogMode, DividerRect, DropZone, MAX_SPLIT_PANES, MultiplexApp, PaneDrag, SessionPane,
    SplitAxis, TERMINAL_INNER_PADDING_X, TERMINAL_INNER_PADDING_Y, WORKSPACE_PADDING,
    WORKSPACE_SEARCH_ROW_HEIGHT, WorkspaceTabDrag, WorkspaceViewMode,
};
use crate::ui::localization;
use crate::ui::path::{format_file_size, remote_parent_path};
use crate::ui::theme;
use gpui_component::ActiveTheme as _;
use multiplex_domain::{HostedSessionState, SessionLaunchRoute};
use multiplex_ui_contract::{MessageId, TerminalSemanticSnapshot};

/// The chip that follows the pointer while a pane is dragged by its header.
pub(super) struct PaneDragPreview {
    title: String,
}

impl gpui::Render for PaneDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("pane-drag-preview")
            .gap(px(theme::SPACE_2))
            .items_center()
            .px(px(theme::SPACE_3))
            .py(px(theme::SPACE_2))
            .rounded(px(theme::SPACE_2))
            .bg(theme::terminal_panel())
            .border_1()
            .border_color(theme::accent())
            .shadow_lg()
            .child(
                div()
                    .size(px(PANE_STATUS_DOT))
                    .rounded_full()
                    .bg(theme::success()),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_semibold()
                    .text_color(theme::text_on_dark())
                    .child(self.title.clone()),
            )
    }
}

/// Which part of a pane `position` is over. The middle 40% swaps, when offered;
/// otherwise the nearest edge wins.
fn drop_zone_at(
    bounds: gpui::Bounds<gpui::Pixels>,
    position: gpui::Point<gpui::Pixels>,
    center: bool,
) -> DropZone {
    let width = f32::from(bounds.size.width).max(1.0);
    let height = f32::from(bounds.size.height).max(1.0);
    let rx = (f32::from(position.x) - f32::from(bounds.origin.x)) / width;
    let ry = (f32::from(position.y) - f32::from(bounds.origin.y)) / height;
    if center && (0.3..0.7).contains(&rx) && (0.3..0.7).contains(&ry) {
        return DropZone::Center;
    }
    [
        (rx, DropZone::Left),
        (1.0 - rx, DropZone::Right),
        (ry, DropZone::Top),
        (1.0 - ry, DropZone::Bottom),
    ]
    .into_iter()
    .min_by(|a, b| a.0.total_cmp(&b.0))
    .map_or(DropZone::Right, |(_, zone)| zone)
}

/// The part of a pane a drop in `zone` would take, as fractions of the pane.
fn drop_zone_fraction(zone: DropZone) -> MotionRect {
    match zone {
        DropZone::Left => MotionRect::new(0.0, 0.0, 0.5, 1.0),
        DropZone::Right => MotionRect::new(0.5, 0.0, 0.5, 1.0),
        DropZone::Top => MotionRect::new(0.0, 0.0, 1.0, 0.5),
        DropZone::Bottom => MotionRect::new(0.0, 0.5, 1.0, 0.5),
        DropZone::Center => MotionRect::new(0.0, 0.0, 1.0, 1.0),
    }
}

fn center_zone_guide() -> MotionRect {
    MotionRect::new(0.3, 0.3, 0.4, 0.4)
}

const PANE_HEADER_HEIGHT: f32 = 30.0;
/// How far the content of panes without focus fades back in a split.
const INACTIVE_PANE_OPACITY: f32 = 0.66;
const PANE_HEADER_BUTTON: f32 = 24.0;
const PANE_STATUS_DOT: f32 = 8.0;
const PANE_RENAME_WIDTH: f32 = 180.0;
const PANE_BROADCAST_BADGE_SIZE: f32 = 10.0;
const DROP_PREVIEW_INSET: f32 = 3.0;

/// The divider line stops short of the panes' rounded corners.
const DIVIDER_LINE_INSET: f32 = 6.0;
const DIVIDER_LINE_WIDTH: f32 = 2.0;
const DIVIDER_READOUT_WIDTH: f32 = 76.0;
const DIVIDER_READOUT_HEIGHT: f32 = 22.0;
const ZOOM_PILL_HEIGHT: f32 = 30.0;
/// How close to the bottom of the window the pointer brings up the layout bar.
const SPLIT_LAYOUT_BAR_REVEAL: f32 = 72.0;
const SPLIT_LAYOUT_BUTTON_SIZE: f32 = 28.0;
/// Preset glyphs are the preset's own layout, drawn at a tenth of this size.
const PRESET_GLYPH_WIDTH: f32 = 22.0;
const PRESET_GLYPH_HEIGHT: f32 = 16.0;

pub(super) fn preset_message(preset: SplitPreset) -> MessageId {
    match preset {
        SplitPreset::Single => MessageId::SplitPresetSingle,
        SplitPreset::Columns => MessageId::SplitPresetColumns,
        SplitPreset::Rows => MessageId::SplitPresetRows,
        SplitPreset::MainAndStack => MessageId::SplitPresetMainAndStack,
        SplitPreset::ThreeColumns => MessageId::SplitPresetThreeColumns,
        SplitPreset::Grid => MessageId::SplitPresetGrid,
        SplitPreset::GridOfSix => MessageId::SplitPresetGridOfSix,
    }
}

/// A small picture of `preset`: its real layout, scaled down.
fn preset_glyph(preset: SplitPreset, group: SharedString) -> Div {
    const SCALE: f32 = 10.0;
    let ids: Vec<u64> = (0..preset.pane_count() as u64).collect();
    let mut rects = Vec::new();
    if let Some(tree) = preset.build(&ids) {
        compute_split_layout(
            &tree,
            0.0,
            0.0,
            PRESET_GLYPH_WIDTH * SCALE,
            PRESET_GLYPH_HEIGHT * SCALE,
            &mut rects,
            &mut Vec::new(),
        );
    }
    let mut glyph = div()
        .relative()
        .w(px(PRESET_GLYPH_WIDTH))
        .h(px(PRESET_GLYPH_HEIGHT));
    for rect in rects {
        glyph = glyph.child(
            div()
                .absolute()
                .left(px(rect.x / SCALE))
                .top(px(rect.y / SCALE))
                .w(px(rect.width / SCALE))
                .h(px(rect.height / SCALE))
                .rounded(px(2.0))
                .border_1()
                .border_color(theme::text_muted_dark())
                .group_hover(group.clone(), |style| {
                    style.border_color(theme::text_on_dark())
                }),
        );
    }
    glyph
}

impl MultiplexApp {
    pub(super) fn terminal_semantic_snapshot(&self) -> Option<TerminalSemanticSnapshot> {
        let workspace = self.active_workspace()?;
        if workspace.layout_mode != WorkspaceLayoutMode::Split
            || workspace.view_mode != WorkspaceViewMode::Terminal
        {
            return None;
        }
        let pane = self.pane(workspace.active_pane_id)?;
        Some(TerminalSemanticSnapshot {
            terminal: pane.terminal_accessibility.snapshot(),
            focus_mode: pane.terminal_focus_mode,
            input_authorized: pane.input_authorized(),
            recording_friendly: self.activity_center.policy().recording_friendly,
            announcement: pane.terminal_announcement,
        })
    }

    fn render_workspace_search(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        let workspace = self.active_workspace()?;
        if workspace.view_mode != WorkspaceViewMode::Terminal {
            return None;
        }
        if !workspace.search_visible {
            return None;
        }
        let matches = workspace.search_results.len();
        let current_match = workspace
            .active_search_index
            .map(|index| index + 1)
            .unwrap_or(0);

        Some(
            h_flex()
                .h(px(WORKSPACE_SEARCH_ROW_HEIGHT))
                .w_full()
                .px_4()
                .gap_3()
                .items_center()
                .bg(theme::terminal_bg())
                .border_b_1()
                .border_color(theme::border_dark())
                .child(
                    div()
                        .id("workspace-search-input-wrap")
                        .flex_1()
                        .child(Input::new(&self.shell_inputs.terminal_search).flex_1()),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_muted_dark())
                        .child(localization::dynamic_user_data_message(
                            multiplex_ui_contract::MessageId::AgentCanvasDynamicCurrentMatchMatches,
                            vec![(current_match).to_string(), (matches).to_string()],
                        )),
                )
                .child(
                    Button::new("workspace-search-prev")
                        .ghost()
                        .small()
                        .label(localization::static_message(
                            multiplex_ui_contract::MessageId::AgentCanvasCopyPrev,
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.jump_workspace_search(-1, cx);
                        })),
                )
                .child(
                    Button::new("workspace-search-next")
                        .ghost()
                        .small()
                        .label(localization::static_message(
                            multiplex_ui_contract::MessageId::AgentCanvasCopyNext,
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.jump_workspace_search(1, cx);
                        })),
                )
                .child(
                    Button::new("workspace-search-close")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_workspace_search(window, cx);
                        })),
                ),
        )
    }

    fn render_workspace_files_view(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(workspace) = self.active_workspace() else {
            return v_flex();
        };
        let workspace_id = workspace.id;
        let Some(browser) = workspace.sftp.as_ref() else {
            let active_pane_is_local = self
                .active_pane()
                .is_some_and(|pane| pane.request.kind == ConnectionKind::LocalShell);
            let empty_state = if active_pane_is_local {
                self.render_workspace_empty_state(
                    Icon::new(IconName::FolderOpen)
                        .size(px(theme::ICON_SIZE_LARGE))
                        .text_color(theme::accent()),
                    workspace_sftp_text(MessageId::SftpWorkspaceLocalTitle),
                    workspace_sftp_text(MessageId::SftpWorkspaceLocalDescription),
                )
                .child(
                    h_flex().gap_2().justify_center().child(
                        Button::new("workspace-files-local-back")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .label(workspace_sftp_text(MessageId::SftpBackTerminalAction))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_active_workspace_terminal(cx);
                            })),
                    ),
                )
            } else {
                self.render_workspace_empty_state(
                    Icon::new(IconName::FolderOpen)
                        .size(px(theme::ICON_SIZE_LARGE))
                        .text_color(theme::accent()),
                    workspace_sftp_text(MessageId::SftpWorkspaceOpenTitle),
                    workspace_sftp_text(MessageId::SftpWorkspaceOpenDescription),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .justify_center()
                        .child(
                            Button::new("workspace-files-open")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                                .label(workspace_sftp_text(MessageId::SftpOpenFilesAction))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_active_workspace_files(cx);
                                })),
                        )
                        .child(
                            Button::new("workspace-files-back")
                                .small()
                                .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                                .label(workspace_sftp_text(MessageId::SftpBackTerminalAction))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_active_workspace_terminal(cx);
                                })),
                        ),
                )
            };

            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .bg(theme::terminal_bg())
                .p(px(WORKSPACE_PADDING))
                .child(empty_state);
        };
        let selected_entry = self.selected_workspace_sftp_entry(workspace.id);
        let transfer_active = browser
            .transfer
            .as_ref()
            .is_some_and(|transfer| transfer.active);
        let selected_is_file = selected_entry.as_ref().is_some_and(|entry| !entry.is_dir);

        v_flex()
            .flex_1()
            .p(px(WORKSPACE_PADDING))
            .gap_3()
            .bg(theme::terminal_bg())
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::terminal_panel())
                    .border_1()
                    .border_color(theme::border_dark())
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                v_flex()
                                    .gap(px(theme::SPACE_1))
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                            .font_medium()
                                            .text_color(theme::text_muted_dark())
                                            .child(workspace_sftp_text(
                                                MessageId::SftpRemotePathLabel,
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                                            .font_semibold()
                                            .text_color(theme::text_on_dark())
                                            .child(browser.current_path.clone()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(self.status_badge(
                                        browser.request.address(),
                                        theme::terminal_bg(),
                                        theme::accent(),
                                    ))
                                    .when(browser.loading, |this| {
                                        this.child(self.status_badge(
                                            workspace_sftp_text(MessageId::SftpSyncingStatus),
                                            theme::terminal_bg(),
                                            theme::warning(),
                                        ))
                                    })
                                    .when_some(selected_entry.as_ref(), |this, entry| {
                                        this.child(self.status_badge(
                                            if entry.is_dir {
                                                workspace_sftp_text(MessageId::SftpFolderKind)
                                            } else {
                                                workspace_sftp_text(MessageId::SftpFileKind)
                                            },
                                            theme::terminal_bg(),
                                            theme::success(),
                                        ))
                                    }),
                            ),
                    )
                    .when_some(selected_entry.as_ref(), |this, entry| {
                        this.child(
                            div()
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted_dark())
                                .child(if entry.is_dir {
                                    localization::sftp_selected_folder(entry.path.clone())
                                } else {
                                    localization::sftp_selected_file(
                                        entry.path.clone(),
                                        format_file_size(entry.size.unwrap_or(0)),
                                    )
                                }),
                        )
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("workspace-files-up")
                            .small()
                            .ghost()
                            .icon(IconName::ChevronUp)
                            .label(workspace_sftp_text(MessageId::SftpParentAction))
                            .disabled(remote_parent_path(&browser.current_path).is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.navigate_workspace_files_up(cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-files-refresh")
                            .small()
                            .ghost()
                            .icon(IconName::Redo2)
                            .label(workspace_sftp_text(MessageId::SftpRefreshAction))
                            .disabled(browser.loading)
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.refresh_workspace_files(workspace_id);
                            })),
                    )
                    .child(
                        Button::new("workspace-files-upload")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Accent, cx))
                            .icon(IconName::Plus)
                            .label(workspace_sftp_text(MessageId::SftpUploadAction))
                            .disabled(transfer_active)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.upload_workspace_file(window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-files-download")
                            .small()
                            .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                            .icon(IconName::ArrowDown)
                            .label(workspace_sftp_text(MessageId::SftpDownloadAction))
                            .disabled(transfer_active || !selected_is_file)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.download_workspace_file(window, cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-files-delete")
                            .small()
                            .ghost()
                            .icon(IconName::Delete)
                            .label(workspace_sftp_text(MessageId::SftpDeleteAction))
                            .disabled(transfer_active || selected_entry.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.delete_workspace_file(cx);
                            })),
                    )
                    .child(
                        Button::new("workspace-files-terminal")
                            .small()
                            .ghost()
                            .icon(IconName::SquareTerminal)
                            .label(workspace_sftp_text(MessageId::SftpBackTerminalAction))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_active_workspace_terminal(cx);
                            })),
                    ),
            )
            .when_some(browser.transfer.as_ref(), |this, transfer| {
                let progress = if transfer.total_bytes == 0 {
                    0.0
                } else {
                    (transfer.transferred_bytes as f32 / transfer.total_bytes as f32)
                        .clamp(0.0, 1.0)
                };
                let progress_percent = (progress * 100.0).round() as u32;
                let can_retry = !transfer.active
                    && transfer.conflict.is_none()
                    && transfer.sha256.is_none()
                    && (transfer.transferred_bytes > 0
                        || transfer.status.contains("failed")
                        || transfer.status.starts_with("Cancelled"));
                this.child(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .p_3()
                        .rounded(px(theme::CARD_RADIUS))
                        .bg(theme::terminal_panel())
                        .border_1()
                        .border_color(if transfer.conflict.is_some() {
                            theme::warning()
                        } else if transfer.active {
                            theme::with_alpha(theme::accent(), 0.55)
                        } else {
                            theme::border_dark()
                        })
                        .child(
                            h_flex()
                                .w_full()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    v_flex()
                                        .gap(px(theme::SPACE_1))
                                        .child(
                                            div()
                                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                                .font_semibold()
                                                .text_color(theme::text_on_dark())
                                                .child(workspace_sftp_text(
                                                    super::sftp::classify_sftp_transfer_state(
                                                        transfer,
                                                    )
                                                    .message(),
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                .text_color(theme::text_muted_dark())
                                                .child(if transfer.resumed_from > 0 {
                                                    localization::sftp_transfer_progress_resumed(
                                                        format_file_size(
                                                            transfer.transferred_bytes,
                                                        ),
                                                        if transfer.total_bytes > 0 {
                                                            format_file_size(transfer.total_bytes)
                                                        } else {
                                                            workspace_sftp_text(
                                                                MessageId::SftpTransferWaiting,
                                                            )
                                                        },
                                                        progress_percent,
                                                        format_file_size(transfer.resumed_from),
                                                    )
                                                } else {
                                                    localization::sftp_transfer_progress(
                                                        format_file_size(
                                                            transfer.transferred_bytes,
                                                        ),
                                                        if transfer.total_bytes > 0 {
                                                            format_file_size(transfer.total_bytes)
                                                        } else {
                                                            workspace_sftp_text(
                                                                MessageId::SftpTransferWaiting,
                                                            )
                                                        },
                                                        progress_percent,
                                                    )
                                                }),
                                        ),
                                )
                                .when(transfer.active, |this| {
                                    this.child(
                                        Button::new("workspace-transfer-cancel")
                                            .small()
                                            .ghost()
                                            .icon(IconName::Close)
                                            .label(workspace_sftp_text(
                                                MessageId::SftpCancelTransferAction,
                                            ))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.cancel_workspace_transfer(workspace_id, cx);
                                            })),
                                    )
                                })
                                .when(can_retry, |this| {
                                    this.child(
                                        Button::new("workspace-transfer-retry")
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Accent,
                                                cx,
                                            ))
                                            .icon(IconName::Redo2)
                                            .label(if transfer.transferred_bytes > 0 {
                                                workspace_sftp_text(
                                                    MessageId::SftpResumeTransferAction,
                                                )
                                            } else {
                                                workspace_sftp_text(
                                                    MessageId::SftpRetryTransferAction,
                                                )
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.retry_workspace_transfer(workspace_id, cx);
                                            })),
                                    )
                                }),
                        )
                        .when(transfer.total_bytes > 0, |this| {
                            this.child(
                                div()
                                    .w_full()
                                    .h(px(theme::current_design_tokens()
                                        .layout_progress_compact_height()
                                        .0))
                                    .rounded(px(theme::current_design_tokens().radius_progress().0))
                                    .bg(theme::with_alpha(theme::border_dark(), 0.7))
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(progress))
                                            .rounded(px(theme::current_design_tokens()
                                                .radius_progress()
                                                .0))
                                            .bg(theme::accent()),
                                    ),
                            )
                        })
                        .when_some(transfer.conflict, |this, conflict| {
                            this.child(
                                h_flex()
                                    .w_full()
                                    .flex_wrap()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::warning())
                                            .child(localization::sftp_conflict_description(
                                                format_file_size(conflict.existing_bytes),
                                            )),
                                    )
                                    .child(
                                        Button::new("workspace-transfer-replace")
                                            .small()
                                            .custom(Self::action_button_style(
                                                theme::ActionTone::Danger,
                                                cx,
                                            ))
                                            .icon(IconName::Replace)
                                            .label(workspace_sftp_text(
                                                MessageId::SftpReplaceAction,
                                            ))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.resolve_workspace_transfer(
                                                    workspace_id,
                                                    crate::sftp::SftpConflictPolicy::Replace,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new("workspace-transfer-skip")
                                            .small()
                                            .ghost()
                                            .label(workspace_sftp_text(MessageId::SftpSkipAction))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.resolve_workspace_transfer(
                                                    workspace_id,
                                                    crate::sftp::SftpConflictPolicy::Skip,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .when(conflict.resume_available, |this| {
                                        this.child(
                                            Button::new("workspace-transfer-resume")
                                                .small()
                                                .custom(Self::action_button_style(
                                                    theme::ActionTone::Accent,
                                                    cx,
                                                ))
                                                .icon(IconName::Redo2)
                                                .label(workspace_sftp_text(
                                                    MessageId::SftpResumeTransferAction,
                                                ))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.resolve_workspace_transfer(
                                                        workspace_id,
                                                        crate::sftp::SftpConflictPolicy::Resume,
                                                        cx,
                                                    );
                                                })),
                                        )
                                    }),
                            )
                        })
                        .when_some(transfer.sha256.as_ref(), |this, checksum| {
                            this.child(
                                div()
                                    .text_size(px(theme::TYPE_MICRO_SIZE))
                                    .text_color(theme::text_muted_dark())
                                    .child(localization::sftp_checksum(checksum.clone())),
                            )
                        }),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .gap_2()
                    .when(browser.entries.is_empty() && browser.loading, |this| {
                        this.child(
                            v_flex()
                                .w_full()
                                .items_center()
                                .justify_center()
                                .p_8()
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::terminal_panel())
                                .border_1()
                                .border_color(theme::border_dark())
                                .gap_2()
                                .child(
                                    Icon::new(IconName::LoaderCircle)
                                        .size(px(theme::ICON_SIZE_LARGE))
                                        .text_color(theme::accent()),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_BODY_SIZE))
                                        .text_color(theme::text_muted_dark())
                                        .child(workspace_sftp_text(
                                            MessageId::SftpLoadingDirectory,
                                        )),
                                ),
                        )
                    })
                    .when(browser.entries.is_empty() && !browser.loading, |this| {
                        this.child(
                            self.render_workspace_empty_state(
                                Icon::new(IconName::Folder)
                                    .size(px(theme::ICON_SIZE_LARGE))
                                    .text_color(theme::accent()),
                                workspace_sftp_text(MessageId::SftpDirectoryEmptyTitle),
                                workspace_sftp_text(MessageId::SftpDirectoryEmptyDescription),
                            )
                            .w_full(),
                        )
                    })
                    .children(browser.entries.iter().enumerate().map(|(index, entry)| {
                        let click_path = entry.path.clone();
                        let open_path = entry.path.clone();
                        let is_selected =
                            browser.selected_path.as_deref() == Some(entry.path.as_str());
                        let kind = if entry.is_dir {
                            workspace_sftp_text(MessageId::SftpFolderKind)
                        } else if entry.is_symlink {
                            workspace_sftp_text(MessageId::SftpSymlinkKind)
                        } else {
                            format_file_size(entry.size.unwrap_or(0))
                        };

                        h_flex()
                            .id(("workspace-file-entry", index))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .p_3()
                            .rounded(px(theme::SFTP_REMOTE_ROW_RADIUS))
                            .bg(if is_selected {
                                theme::with_alpha(theme::accent(), 0.18)
                            } else {
                                theme::terminal_panel()
                            })
                            .border_1()
                            .border_color(if is_selected {
                                theme::with_alpha(theme::accent(), 0.45)
                            } else {
                                theme::border_dark()
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::with_alpha(theme::accent(), 0.12)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_workspace_file_entry(
                                    workspace_id,
                                    click_path.clone(),
                                    cx,
                                );
                            }))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        Icon::new(if entry.is_dir {
                                            IconName::FolderClosed
                                        } else {
                                            IconName::File
                                        })
                                        .size(px(theme::ICON_SIZE_DEFAULT))
                                        .text_color(
                                            if entry.is_dir {
                                                theme::warning()
                                            } else {
                                                theme::text_muted_dark()
                                            },
                                        ),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(px(theme::SFTP_ROW_LABEL_GAP))
                                            .child(
                                                div()
                                                    .text_size(px(theme::TYPE_BODY_SIZE))
                                                    .font_medium()
                                                    .text_color(theme::text_on_dark())
                                                    .child(entry.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                    .text_color(theme::text_muted_dark())
                                                    .child(entry.path.clone()),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted_dark())
                                            .child(kind),
                                    )
                                    .when(entry.is_dir, |this| {
                                        this.child(
                                            Button::new(("workspace-file-open", index))
                                                .ghost()
                                                .small()
                                                .icon(IconName::ChevronRight)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.select_workspace_file_entry(
                                                        workspace_id,
                                                        open_path.clone(),
                                                        cx,
                                                    );
                                                    this.open_selected_workspace_file_entry(cx);
                                                })),
                                        )
                                    }),
                            )
                            .into_any_element()
                    })),
            )
    }

    pub(super) fn terminal_font_family(&self, cx: &Context<Self>) -> SharedString {
        self.saved
            .settings
            .terminal_font_family
            .as_deref()
            .filter(|family| !family.trim().is_empty())
            .map(|family| SharedString::from(family.to_string()))
            .unwrap_or_else(|| cx.theme().mono_font_family.clone())
    }

    /// Covers an SSH pane until its first connection: the host, a progress line from plug to
    /// terminal, and the connection log on request. A failed first attempt stays on this card
    /// with the error, Retry, and Close.
    fn render_pane_connecting_card(&self, pane: &SessionPane, cx: &mut Context<Self>) -> Div {
        let pane_id = pane.id;
        let failed = pane.last_error.is_some();
        let waiting_to_retry = pane.auto_reconnect_at.is_some();
        let tone = if failed {
            theme::danger()
        } else {
            theme::accent()
        };
        let endpoint = format!("SSH {}", pane.request.address());
        let step = |icon: IconName, color: gpui::Hsla| {
            div()
                .size(px(theme::STATUS_HEIGHT))
                .flex_none()
                .rounded(px(theme::PILL_RADIUS))
                .flex()
                .items_center()
                .justify_center()
                .bg(color)
                .child(
                    Icon::new(icon)
                        .size(px(theme::HOST_ICON_SIZE_BODY))
                        .text_color(theme::library_card()),
                )
        };
        let track = div()
            .flex_1()
            .h(px(theme::SPACE_1))
            .rounded(px(theme::PILL_RADIUS))
            .bg(theme::with_alpha(theme::text_muted(), 0.35))
            .overflow_hidden()
            .child(if failed || waiting_to_retry {
                div()
                    .h_full()
                    .w(relative(if failed { 1.0 } else { 0.5 }))
                    .bg(tone)
                    .into_any_element()
            } else {
                div()
                    .h_full()
                    .bg(tone)
                    .with_animation(
                        ("pane-connecting-progress", pane_id),
                        Animation::new(theme::motion_duration(
                            theme::current_design_tokens().motion_progress(false),
                        ))
                        .repeat(),
                        |bar, delta| bar.w(relative(delta)),
                    )
                    .into_any_element()
            });

        div()
            .absolute()
            .top(px(theme::SPACE_0))
            .left(px(theme::SPACE_0))
            .size_full()
            .bg(theme::terminal_bg())
            .child(
                v_flex()
                    .id(("pane-connecting-card", pane_id))
                    .debug_selector(move || format!("pane-connecting-card-{pane_id}"))
                    .size_full()
                    .items_center()
                    .pt(px(theme::CONNECT_CONTENT_TOP))
                    .px(px(theme::SPACE_4))
                    .gap(px(theme::STATUS_HEIGHT))
                    .child(
                        h_flex()
                            .w(px(theme::CONNECT_PANEL_WIDTH))
                            .max_w_full()
                            .gap(px(theme::SPACE_4))
                            .items_center()
                            .child(
                                div()
                                    .size(px(theme::CHROME_HEIGHT))
                                    .flex_none()
                                    .rounded(px(theme::CARD_RADIUS))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(theme::with_alpha(tone, 0.18))
                                    .child(
                                        Icon::new(IconName::SquareTerminal)
                                            .size(px(theme::ICON_SIZE_MEDIUM))
                                            .text_color(tone),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(theme::SPACE_1))
                                    .child(
                                        div()
                                            .text_size(px(theme::ICON_SIZE_COMPACT))
                                            .font_semibold()
                                            .text_color(theme::text_on_dark())
                                            .child(pane.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_MICRO_SIZE))
                                            .text_color(theme::text_muted_dark())
                                            .child(endpoint),
                                    ),
                            )
                            .child(
                                Button::new(("pane-connect-logs", pane_id))
                                    .debug_selector(move || format!("pane-connect-logs-{pane_id}"))
                                    .small()
                                    .label(if pane.show_connect_log {
                                        localization::terminal_pane_hide_logs()
                                    } else {
                                        localization::terminal_pane_show_logs()
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(pane) = this.pane_mut(pane_id) {
                                            pane.show_connect_log = !pane.show_connect_log;
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .w(px(theme::CONNECT_PANEL_WIDTH))
                            .max_w_full()
                            .items_center()
                            .gap(px(theme::SPACE_3))
                            .child(step(IconName::Globe, tone))
                            .child(track)
                            .child(step(
                                IconName::SquareTerminal,
                                if failed {
                                    theme::with_alpha(theme::text_muted(), 0.5)
                                } else {
                                    theme::accent()
                                },
                            )),
                    )
                    .when_some(pane.last_error.clone(), |this, error| {
                        this.child(
                            v_flex()
                                .w(px(theme::CONNECT_PANEL_WIDTH))
                                .max_w_full()
                                .gap(px(theme::SPACE_2))
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                        .font_semibold()
                                        .text_color(theme::danger())
                                        .child(localization::terminal_pane_connect_failed(
                                            &pane.request.address(),
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::text_on_dark())
                                        .child(error),
                                )
                                .when(waiting_to_retry, |this| {
                                    this.child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted_dark())
                                            .child(pane.status.clone()),
                                    )
                                })
                                .child(
                                    h_flex()
                                        .gap(px(theme::SPACE_2))
                                        .when(pane.closed, |this| {
                                            this.child(
                                                Button::new(("pane-connect-retry", pane_id))
                                                    .debug_selector(move || {
                                                        format!("pane-connect-retry-{pane_id}")
                                                    })
                                                    .small()
                                                    .primary()
                                                    .icon(IconName::Redo)
                                                    .label(localization::common_retry())
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.reconnect_pane(
                                                                pane_id, window, cx,
                                                            );
                                                        },
                                                    )),
                                            )
                                        })
                                        .child(
                                            Button::new(("pane-connect-close", pane_id))
                                                .small()
                                                .ghost()
                                                .label(localization::common_close())
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.close_pane(pane_id, cx);
                                                })),
                                        ),
                                ),
                        )
                    })
                    .when(pane.show_connect_log, |this| {
                        this.child(
                            v_flex()
                                .id(("pane-connect-log", pane_id))
                                .w(px(theme::CONNECT_PANEL_WIDTH))
                                .max_w_full()
                                .p(px(theme::SPACE_3))
                                .gap(px(theme::SPACE_1))
                                .rounded(px(theme::CARD_RADIUS))
                                .bg(theme::terminal_panel())
                                .border_1()
                                .border_color(theme::border_dark())
                                .children(pane.connect_log.iter().map(|line| {
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .font_family(
                                            theme::current_design_tokens().font_mono_family().0,
                                        )
                                        .text_color(theme::text_on_dark())
                                        .child(line.clone())
                                })),
                        )
                    }),
            )
    }

    /// A banner along the bottom of an SSH or local pane that is not connected: why it
    /// failed or closed, when it retries, and a way to reconnect now. Durable sessions show
    /// this in their header instead.
    fn render_pane_connection_notice(
        &self,
        pane: &SessionPane,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if pane.connected || pane.app_attached.is_some() {
            return None;
        }
        if !pane.ever_connected && !pane.request.is_local_shell() {
            return Some(self.render_pane_connecting_card(pane, cx));
        }
        let pane_id = pane.id;
        let endpoint = pane.request.endpoint_label();
        let failed = pane.last_error.is_some();
        if !failed && !pane.closed && pane.request.is_local_shell() {
            // Local shells start in a moment; a banner would only flash.
            return None;
        }
        let heading = if failed {
            localization::terminal_pane_connect_failed(&endpoint)
        } else if pane.closed {
            localization::terminal_pane_session_closed(&endpoint)
        } else {
            localization::terminal_pane_connecting(&endpoint)
        };
        let retrying = pane.auto_reconnect_at.is_some() || !pane.closed;
        Some(
            div()
                .absolute()
                .left(px(theme::SPACE_3))
                .right(px(theme::SPACE_3))
                .bottom(px(theme::SPACE_3))
                .child(
                    h_flex()
                        .id(("pane-connection-notice", pane_id))
                        .debug_selector(move || format!("pane-connection-notice-{pane_id}"))
                        .items_center()
                        .justify_between()
                        .flex_wrap()
                        .gap(px(theme::SPACE_3))
                        .px(px(theme::SPACE_4))
                        .py(px(theme::SPACE_3))
                        .rounded(px(theme::CONTROL_RADIUS))
                        .border_1()
                        .border_color(if failed {
                            theme::with_alpha(theme::danger(), 0.5)
                        } else {
                            theme::border_dark()
                        })
                        .bg(theme::terminal_panel())
                        .child(
                            v_flex()
                                .min_w_0()
                                .flex_1()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                        .font_semibold()
                                        .text_color(if failed {
                                            theme::danger()
                                        } else {
                                            theme::text_on_dark()
                                        })
                                        .child(heading),
                                )
                                .when_some(pane.last_error.clone(), |this, error| {
                                    this.child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_on_dark())
                                            .child(error),
                                    )
                                })
                                .when(failed && retrying, |this| {
                                    this.child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted_dark())
                                            .child(pane.status.clone()),
                                    )
                                }),
                        )
                        .when(pane.closed, |this| {
                            this.child(
                                Button::new(("pane-reconnect", pane_id))
                                    .debug_selector(move || format!("pane-reconnect-{pane_id}"))
                                    .small()
                                    .icon(IconName::Redo)
                                    .label(localization::terminal_pane_reconnect_action())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.reconnect_pane(pane_id, window, cx);
                                    })),
                            )
                        }),
                ),
        )
    }

    pub(super) fn render_terminal_pane(
        &self,
        pane: &SessionPane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pane_id = pane.id;
        let terminal_selector = format!("terminal-surface-{pane_id}");
        let drop_zone = self
            .split_drop_target
            .and_then(|(pid, zone)| (pid == pane.id).then_some(zone));
        let is_active_pane = self
            .active_workspace()
            .map(|workspace| workspace.active_pane_id)
            == Some(pane.id);
        // Only a split with more than one pane on screen needs to show which has focus:
        // the focused pane gets the accent and the others' content dims.
        let marks_active_pane = is_active_pane
            && self
                .active_workspace()
                .is_some_and(|workspace| workspace.pane_ids.len() > 1)
            && self.zoomed_pane().is_none();
        let dims_content = !is_active_pane
            && self.active_workspace().is_some_and(|workspace| {
                workspace.layout_mode == WorkspaceLayoutMode::Split
                    && workspace
                        .layout
                        .as_ref()
                        .is_some_and(|layout| layout.leaf_count() > 1)
            });
        let broadcast_target = !pane.closed
            && self.active_workspace().is_some_and(|workspace| {
                workspace.broadcast_input
                    && workspace.pane_ids.len() > 1
                    && workspace.pane_ids.contains(&pane.id)
            });
        let is_app_attached = pane.app_attached.is_some();
        let durable = pane
            .app_attached
            .as_ref()
            .is_some_and(|attached| attached.route == SessionLaunchRoute::DurableHost);
        let durable_state = pane.app_attached.as_ref().and_then(|attached| {
            self.saved
                .app_attached_sessions
                .iter()
                .find(|session| session.id == attached.hosted_session_id)
                .map(|session| session.state)
        });
        let can_retry = durable
            && pane.closed
            && durable_state.is_some_and(|state| {
                matches!(
                    state,
                    HostedSessionState::Offline
                        | HostedSessionState::Gap
                        | HostedSessionState::PermissionDenied
                        | HostedSessionState::Incompatible
                )
            });
        let can_stop = !pane.closed
            && (!durable
                || durable_state.is_some_and(|state| {
                    !matches!(
                        state,
                        HostedSessionState::Exited
                            | HostedSessionState::Orphaned
                            | HostedSessionState::Gap
                            | HostedSessionState::PermissionDenied
                            | HostedSessionState::Incompatible
                            | HostedSessionState::Offline
                    )
                }));
        let input_authorized = pane.input_authorized();
        // Durable sessions have their own header: it carries their writer state, retry, and stop.
        let show_terminal_chrome = is_app_attached;
        let workspace_id = self.workspace_id_for_pane(pane_id).unwrap_or_default();
        let in_split = self.active_workspace().is_some_and(|workspace| {
            workspace.layout_mode == WorkspaceLayoutMode::Split
                && workspace
                    .layout
                    .as_ref()
                    .is_some_and(|layout| layout.contains(pane_id))
        });
        let pane_drag = PaneDrag {
            workspace_id,
            pane_id,
            title: pane.title.clone(),
        };

        v_flex()
            .id(("terminal-pane", pane.id))
            .relative()
            .size_full()
            .rounded(px(theme::TYPE_NANO_SIZE))
            .border_1()
            .border_color(if marks_active_pane {
                theme::accent()
            } else if broadcast_target {
                theme::with_alpha(theme::warning(), 0.7)
            } else {
                theme::with_alpha(theme::border_dark(), 0.6)
            })
            .bg(theme::terminal_panel())
            .overflow_hidden()
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<WorkspaceTabDrag>, _, cx| {
                    this.update_split_drop_target(pane_id, event, cx);
                },
            ))
            .on_drop(
                cx.listener(move |this, drag: &WorkspaceTabDrag, window, cx| {
                    this.drop_tab_on_pane(drag.workspace_id, pane_id, window, cx);
                }),
            )
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<PaneDrag>, _, cx| {
                    this.update_pane_drop_target(pane_id, event, cx);
                }),
            )
            .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                this.drop_pane_on_pane(drag.pane_id, pane_id, window, cx);
            }))
            .when(in_split && !show_terminal_chrome, |this| {
                this.child(self.render_split_pane_header(pane, pane_drag.clone(), cx))
            })
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                this.drop_paths_on_pane(pane_id, paths.paths(), window, cx);
            }))
            .when(show_terminal_chrome, |this| {
                this.child(
                    h_flex()
                        .id(("terminal-chrome", pane.id))
                        .when(in_split, |row| {
                            row.cursor_grab().on_drag(
                                pane_drag.clone(),
                                |drag: &PaneDrag, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.new(|_| PaneDragPreview {
                                        title: drag.title.clone(),
                                    })
                                },
                            )
                        })
                        .w_full()
                        .min_h(px(theme::WORKSPACE_HEADER_HEIGHT))
                        .track_focus(&pane.terminal_chrome_focus)
                        .focusable()
                        .items_center()
                        .flex_wrap()
                        .justify_between()
                        .gap(px(theme::SPACE_3))
                        .px(px(theme::SPACE_4))
                        .py(px(theme::SPACE_2))
                        .border_b_1()
                        .border_color(theme::border_dark())
                        .child(
                            v_flex()
                                .min_w_0()
                                .flex_1()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                        .font_semibold()
                                        .text_color(theme::text_on_dark())
                                        .child(pane.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::text_muted_dark())
                                        .child(pane.status.clone()),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap(px(theme::SPACE_2))
                                .flex_wrap()
                                .justify_end()
                                .when(is_app_attached, |this| {
                                    this.child(self.status_badge(
                                        localization::static_message(if input_authorized {
                                            MessageId::TerminalWriterHeld
                                        } else {
                                            MessageId::TerminalReadOnly
                                        }),
                                        theme::terminal_bg(),
                                        if input_authorized {
                                            theme::success()
                                        } else {
                                            theme::warning()
                                        },
                                    ))
                                })
                                .when_some(self.render_dev_url_header(pane, cx), |this, chip| {
                                    this.child(chip)
                                })
                                .when(can_retry, |this| {
                                    this.child(
                                        Button::new(("durable-retry", pane_id))
                                            .small()
                                            .icon(IconName::Redo2)
                                            .label(localization::common_retry())
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.retry_durable_pane(pane_id, window, cx);
                                            })),
                                    )
                                })
                                .when(is_app_attached, |this| {
                                    this.child(
                                        Button::new(("app-attached-stop", pane_id))
                                            .small()
                                            .danger()
                                            .icon(IconName::Close)
                                            .label(localization::new_session_stop_action())
                                            .disabled(!can_stop)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.stop_app_attached_session(pane_id, cx);
                                            })),
                                    )
                                }),
                        ),
                )
            })
            .child(
                div()
                    .id(("terminal-surface", pane.id))
                    .debug_selector(move || terminal_selector.clone())
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .track_focus(&pane.terminal_focus)
                    .focusable()
                    .bg(theme::terminal_bg())
                    .when(dims_content, |surface| {
                        surface.opacity(INACTIVE_PANE_OPACITY)
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.open_pane_context_menu(pane_id, event.position, window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_pane(pane_id, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.handle_pane_mouse_down(pane_id, event, window, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            this.handle_pane_mouse_up(pane_id, event, window, cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                            this.handle_pane_mouse_up(pane_id, event, window, cx);
                        }),
                    )
                    .on_mouse_move(
                        cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                            this.handle_pane_mouse_move(pane_id, event, window, cx);
                        }),
                    )
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &ScrollWheelEvent, window, cx| {
                            // On the canvas, a terminal without focus lets the canvas pan.
                            if !this.pane_takes_scroll(pane_id, event) {
                                return;
                            }
                            this.handle_pane_scroll(pane_id, event, window, cx);
                            cx.stop_propagation();
                        },
                    ))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        if this.handle_terminal_key(pane_id, event, window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        div()
                            .size_full()
                            .overflow_hidden()
                            .px(px(TERMINAL_INNER_PADDING_X))
                            .pt(px(TERMINAL_INNER_PADDING_Y))
                            .pb(px(TERMINAL_INNER_PADDING_Y))
                            .child(pane.terminal_grid.clone()),
                    ),
            )
            .when_some(
                self.render_pane_connection_notice(pane, cx),
                |this, notice| this.child(notice),
            )
            .when(pane.request.persistent_session, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(theme::TYPE_NANO_SIZE))
                        .right(px(theme::TYPE_NANO_SIZE))
                        .child(self.status_badge("tmux", theme::terminal_bg(), theme::accent())),
                )
            })
            .when_some(drop_zone, |this, zone| {
                this.child(self.render_split_drop_preview(pane_id, zone))
            })
    }

    fn pane_takes_scroll(&self, pane_id: u64, event: &ScrollWheelEvent) -> bool {
        let Some(workspace) = self.active_workspace() else {
            return true;
        };
        if workspace.layout_mode != WorkspaceLayoutMode::Canvas {
            return true;
        }
        if event.modifiers.secondary() || event.modifiers.control {
            return false;
        }
        workspace
            .canvas
            .nodes
            .iter()
            .find(|node| node.kind.pane_id() == Some(pane_id))
            .is_some_and(|node| self.canvas_node_takes_scroll(workspace, node))
    }

    fn render_workspace_body(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(workspace) = self.active_workspace() else {
            return v_flex()
                .flex_1()
                .bg(theme::terminal_bg())
                .items_center()
                .justify_center()
                .p(px(WORKSPACE_PADDING))
                .child(
                    self.render_workspace_empty_state(
                        Icon::new(IconName::SquareTerminal)
                            .size(px(theme::SPACE_6))
                            .text_color(theme::accent()),
                        "Open a host to start a workspace",
                        "Select a saved host from the library, use quick connect, or open a local terminal to start working.",
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_center()
                            .child(
                                Button::new("workspace-empty-local")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Accent,
                                        cx,
                                    ))
                                    .label(localization::static_message(multiplex_ui_contract::MessageId::AgentCanvasCopyLocalTerminal))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_local_terminal(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("workspace-empty-hosts")
                                    .small()
                                    .custom(Self::action_button_style(
                                        theme::ActionTone::Neutral,
                                        cx,
                                    ))
                                    .label(localization::static_message(multiplex_ui_contract::MessageId::HostsAddAction))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_editor_for_new_host(window, cx);
                                    })),
                            ),
                    ),
                );
        };

        if let Some(failure) = workspace.connect_failure.clone() {
            let wid = workspace.id;
            return v_flex()
                .flex_1()
                .bg(theme::terminal_bg())
                .child(self.render_connect_failure_dialog(wid, &failure, cx));
        }
        if let Some(pending) = workspace.pending_connect.clone() {
            let mode = workspace.pending_connect_mode;
            let protocol = workspace.pending_connect_protocol;
            let wid = workspace.id;
            return v_flex()
                .flex_1()
                .bg(theme::terminal_bg())
                .child(match mode {
                    ConnectDialogMode::Username => self
                        .render_connect_dialog(wid, &pending, cx)
                        .into_any_element(),
                    ConnectDialogMode::ChooseProtocol => self
                        .render_choose_protocol_dialog(wid, &pending, protocol, cx)
                        .into_any_element(),
                });
        }

        if workspace.layout_mode == crate::models::WorkspaceLayoutMode::Canvas {
            return self.render_canvas_workspace(window, cx);
        }

        let workspace_id = workspace.id;
        let (panes, dividers) = self.workspace_split_rects(window);
        // Motion is tracked in window coordinates so Split and Canvas, whose bodies
        // start at different heights, can pick up from each other.
        let body_top = theme::CHROME_HEIGHT
            + if workspace.search_visible {
                WORKSPACE_SEARCH_ROW_HEIGHT
            } else {
                0.0
            };
        let targets: Vec<(u64, MotionRect)> = panes
            .iter()
            .map(|rect| {
                (
                    rect.pane_id,
                    MotionRect::new(rect.x, rect.y + body_top, rect.width, rect.height),
                )
            })
            .collect();
        let now = Instant::now();
        let transition = self
            .layout_transition
            .as_ref()
            .filter(|transition| transition.workspace_id == workspace_id);
        let moving = transition.is_some_and(|transition| !transition.is_finished(now));
        let frames = match transition {
            Some(transition) if moving => transition.frames(now, &targets, |pane_id| {
                workspace.pane_ids.contains(&pane_id) && self.pane(pane_id).is_some()
            }),
            _ => motion::settled_frames(&targets),
        };
        *self.drawn_layout.borrow_mut() = Some((
            workspace_id,
            frames
                .iter()
                .filter(|frame| !frame.leaving)
                .map(|frame| (frame.pane_id, frame.rect))
                .collect(),
        ));
        if moving {
            window.request_animation_frame();
        } else if self.layout_motion_pending() {
            cx.defer_in(window, |this, window, cx| {
                this.settle_layout_transition(window, cx);
            });
        }

        let broadcasting = workspace.broadcast_input && workspace.pane_ids.len() > 1;
        let mut container = div().relative().size_full().bg(theme::terminal_bg());
        for frame in frames {
            let Some(pane) = self.pane(frame.pane_id) else {
                continue;
            };
            pane.grid_bounds.set_scale(1.0);
            container = container.child(
                div()
                    .absolute()
                    .left(px(frame.rect.x))
                    .top(px(frame.rect.y - body_top))
                    .w(px(frame.rect.width))
                    .h(px(frame.rect.height))
                    .opacity(frame.opacity)
                    .child(self.render_terminal_pane(pane, window, cx)),
            );
        }
        // Dividers appear once the panes have settled around them.
        if !moving {
            for divider in dividers {
                container = container.child(self.render_pane_divider(workspace_id, divider, cx));
            }
        }
        if broadcasting {
            // An amber rim around the whole split while typing goes to every pane.
            container = container.child(
                div()
                    .absolute()
                    .inset_0()
                    .border_2()
                    .border_color(theme::with_alpha(theme::warning(), 0.55)),
            );
        }
        if let Some(pill) = self.render_zoom_pill(workspace_id, cx) {
            container = container.child(pill);
        } else if let Some(pill) = self.render_attention_pill(cx) {
            container = container.child(pill);
        }
        if self.split_layout_bar_revealed {
            container = container.child(self.render_split_layout_bar(cx));
        }
        // The layout bar shows while the pointer is near the bottom edge, so it never
        // sits over a prompt that is being typed at.
        container =
            container.on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                let bottom = f32::from(window.viewport_size().height);
                let revealed = f32::from(event.position.y) > bottom - SPLIT_LAYOUT_BAR_REVEAL;
                if this.split_layout_bar_revealed != revealed {
                    this.split_layout_bar_revealed = revealed;
                    cx.notify();
                }
            }));

        v_flex().flex_1().bg(theme::terminal_bg()).child(container)
    }

    pub(super) fn render_workspace_shell(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let content = if self
            .active_workspace()
            .is_some_and(|workspace| workspace.view_mode == WorkspaceViewMode::Files)
        {
            self.render_workspace_files_view(window, cx)
        } else if self
            .active_workspace()
            .is_some_and(|workspace| workspace.view_mode == WorkspaceViewMode::Screen)
        {
            self.render_workspace_screen_view(window, cx)
        } else {
            self.render_workspace_body(window, cx)
        };
        v_flex()
            .flex_1()
            .bg(theme::terminal_bg())
            .when_some(self.render_snippet_prompts_panel(cx), |this, panel| {
                this.child(panel)
            })
            .when_some(self.render_snippet_insert_review(cx), |this, panel| {
                this.child(panel)
            })
            .when_some(self.render_paste_confirmation(cx), |this, banner| {
                this.child(banner)
            })
            .when_some(self.render_pinned_snippet_actions(cx), |this, actions| {
                this.child(actions)
            })
            .when_some(self.render_autocomplete_suggestions(), |this, bar| {
                this.child(bar)
            })
            .when_some(self.render_workspace_search(window, cx), |this, search| {
                this.child(search)
            })
            .child(content)
    }

    fn render_split_layout_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut bar = h_flex()
            .id("split-layout-bar")
            .debug_selector(|| "split-layout-bar".to_string())
            .items_center()
            .gap(px(theme::BORDER_HAIRLINE * 2.0))
            .p(px(theme::SPACE_1))
            .rounded(px(theme::SPACE_3))
            .bg(theme::with_alpha(theme::terminal_panel(), 0.94))
            .border_1()
            .border_color(theme::border_strong())
            .shadow_lg()
            // Clicks on the bar must not reach the pane underneath.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for preset in SplitPreset::ALL {
            let label = localization::static_message(preset_message(preset));
            let group = SharedString::from(format!("split-preset-{}", preset as usize));
            bar = bar.child(
                div()
                    .id(("split-preset", preset as usize))
                    .group(group.clone())
                    .debug_selector(move || format!("split-preset-{preset:?}"))
                    .h(px(SPLIT_LAYOUT_BUTTON_SIZE))
                    .min_w(px(SPLIT_LAYOUT_BUTTON_SIZE))
                    .px(px(theme::SPACE_2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::SPACE_2))
                    .text_color(theme::text_muted_dark())
                    .cursor_pointer()
                    .hover(|style| {
                        style
                            .bg(theme::with_alpha(theme::hover(), 0.5))
                            .text_color(theme::text_on_dark())
                    })
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_split_preset(preset, window, cx);
                    }))
                    .child(preset_glyph(preset, group)),
            );
        }
        bar = bar
            .child(
                div()
                    .w(px(theme::BORDER_HAIRLINE))
                    .h(px(SPLIT_LAYOUT_BUTTON_SIZE * 0.6))
                    .mx(px(theme::SPACE_1))
                    .bg(theme::border_strong()),
            )
            .child(self.split_layout_bar_text_button(
                "split-layout-equalize",
                None,
                localization::static_message(MessageId::SplitEqualizeAction),
                |this, window, cx| {
                    this.equalize_active_split(window, cx);
                },
                cx,
            ))
            .child(self.split_layout_bar_text_button(
                "split-layout-zoom",
                Some(IconName::Maximize),
                localization::static_message(MessageId::SplitZoomAction),
                |this, window, cx| {
                    this.toggle_pane_zoom(window, cx);
                },
                cx,
            ));
        h_flex()
            .absolute()
            .bottom(px(theme::SPACE_4))
            .left_0()
            .right_0()
            .justify_center()
            .child(bar)
            .with_animation(
                "split-layout-bar",
                Animation::new(motion::MotionSpeed::DropPreview.animation_duration()),
                |element, delta| element.opacity(delta),
            )
            .into_any_element()
    }

    fn split_layout_bar_text_button(
        &self,
        id: &'static str,
        icon: Option<IconName>,
        label: String,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(id)
            .debug_selector(move || id.to_string())
            .h(px(SPLIT_LAYOUT_BUTTON_SIZE))
            .px(px(theme::SPACE_3))
            .gap(px(theme::SPACE_2))
            .items_center()
            .rounded(px(theme::SPACE_2))
            .text_size(px(theme::TYPE_CAPTION_SIZE))
            .text_color(theme::text_muted_dark())
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(theme::with_alpha(theme::hover(), 0.5))
                    .text_color(theme::text_on_dark())
            })
            .when_some(icon, |button, icon| {
                button.child(Icon::new(icon).size(px(theme::ICON_SIZE_DEFAULT)))
            })
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
    }

    /// While one pane fills the split: which one, how many are hidden, and the
    /// way back.
    fn render_zoom_pill(&self, workspace_id: u64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pane_id = self.zoomed_pane_in(workspace_id)?;
        let title = self.pane(pane_id)?.title.clone();
        let hidden = self
            .workspace(workspace_id)?
            .layout
            .as_ref()?
            .leaf_count()
            .saturating_sub(1);
        Some(
            h_flex()
                .id("split-zoom-pill")
                .debug_selector(|| "split-zoom-pill".to_string())
                .absolute()
                .top(px(theme::SPACE_3))
                .left_0()
                .right_0()
                .justify_center()
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(theme::SPACE_3))
                        .h(px(ZOOM_PILL_HEIGHT))
                        .pl(px(theme::SPACE_4))
                        .pr(px(theme::SPACE_1))
                        .rounded(px(ZOOM_PILL_HEIGHT / 2.0))
                        .bg(theme::terminal_panel())
                        .border_1()
                        .border_color(theme::border_strong())
                        .shadow_lg()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted_dark())
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .font_semibold()
                                        .text_color(theme::text_on_dark())
                                        .child(title),
                                )
                                .child(localization::split_zoom_hidden_panes(hidden)),
                        )
                        .child(
                            Button::new("split-zoom-restore")
                                .small()
                                .label(localization::static_message(
                                    multiplex_ui_contract::MessageId::SplitZoomRestoreAction,
                                ))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.restore_zoomed_pane(window, cx);
                                })),
                        ),
                )
                .with_animation(
                    ("split-zoom-pill", pane_id),
                    Animation::new(motion::MotionSpeed::Quick.animation_duration()),
                    |element, delta| element.opacity(delta),
                )
                .into_any_element(),
        )
    }

    fn render_pane_divider(
        &self,
        workspace_id: u64,
        divider: DividerRect,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let DividerRect {
            divider_id,
            axis,
            x,
            y,
            width,
            height,
            span,
            ratio,
        } = divider;
        let drag = self
            .divider_drag
            .filter(|drag| drag.divider_id == divider_id && drag.workspace_id == workspace_id);
        let group = SharedString::from(format!("pane-divider-{divider_id}"));
        // An invisible strip the width of the gap; its line shows on hover and while
        // dragging, and turns green when the ratio clicks into a third or a half.
        let line_color = match drag {
            Some(drag) if drag.snapped.is_some() => theme::success(),
            Some(_) => theme::accent(),
            None => gpui::transparent_black(),
        };
        let inset = px(DIVIDER_LINE_INSET);
        let line = div()
            .absolute()
            .rounded(px(theme::BORDER_HAIRLINE))
            .bg(line_color)
            .when(drag.is_none(), |line| {
                line.group_hover(group.clone(), |style| style.bg(theme::accent()))
            })
            .map(|line| match axis {
                SplitAxis::Horizontal => line
                    .top(inset)
                    .bottom(inset)
                    .left(px((width - DIVIDER_LINE_WIDTH) / 2.0))
                    .w(px(DIVIDER_LINE_WIDTH)),
                SplitAxis::Vertical => line
                    .left(inset)
                    .right(inset)
                    .top(px((height - DIVIDER_LINE_WIDTH) / 2.0))
                    .h(px(DIVIDER_LINE_WIDTH)),
            });
        let readout = drag.map(|drag| {
            let text = match drag.snapped {
                Some(mark) => mark.label().to_string(),
                None => {
                    let first = (drag.ratio * 100.0).round() as i32;
                    format!("{first} : {}", 100 - first)
                }
            };
            div()
                .absolute()
                .left(px(width / 2.0 - DIVIDER_READOUT_WIDTH / 2.0))
                .top(px(height / 2.0 - DIVIDER_READOUT_HEIGHT / 2.0))
                .w(px(DIVIDER_READOUT_WIDTH))
                .h(px(DIVIDER_READOUT_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::SPACE_2))
                .bg(theme::terminal_panel())
                .border_1()
                .border_color(if drag.snapped.is_some() {
                    theme::success()
                } else {
                    theme::border_strong()
                })
                .text_size(px(theme::TYPE_CAPTION_SIZE))
                .font_family(self.terminal_font_family(cx))
                .text_color(if drag.snapped.is_some() {
                    theme::success()
                } else {
                    theme::text_on_dark()
                })
                .child(text)
        });
        div()
            .id(("pane-divider", divider_id))
            .debug_selector(move || format!("pane-divider-{}", divider_id))
            .group(group)
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(width))
            .h(px(height))
            .cursor(match axis {
                SplitAxis::Horizontal => CursorStyle::ResizeLeftRight,
                SplitAxis::Vertical => CursorStyle::ResizeUpDown,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if event.click_count == 2 {
                        this.equalize_divider(workspace_id, divider_id, window, cx);
                        return;
                    }
                    this.start_divider_drag(
                        workspace_id,
                        divider_id,
                        axis,
                        span,
                        ratio,
                        event.position,
                        cx,
                    );
                }),
            )
            .child(line)
            .when_some(readout, |divider, readout| divider.child(readout))
    }

    fn update_split_drop_target(
        &mut self,
        pane_id: u64,
        event: &DragMoveEvent<WorkspaceTabDrag>,
        cx: &mut Context<Self>,
    ) {
        let source_workspace_id = event.drag(cx).workspace_id;
        let same_workspace = self.workspace_id_for_pane(pane_id) == Some(source_workspace_id);
        // `on_drag_move` fires for every pane on every move — only react for the
        // pane the cursor is actually over, and never for the drag's own workspace.
        let zone = (!same_workspace && event.bounds.contains(&event.event.position))
            .then(|| drop_zone_at(event.bounds, event.event.position, false));
        // A merge that would go past the cap says so before the drop, not after.
        let full = zone.is_some() && {
            let target = self
                .workspace_id_for_pane(pane_id)
                .and_then(|workspace_id| self.workspace(workspace_id))
                .map_or(0, |workspace| workspace.pane_ids.len());
            let source = self
                .workspace(source_workspace_id)
                .map_or(0, |workspace| workspace.pane_ids.len());
            target + source > MAX_SPLIT_PANES
        };
        self.set_split_drop_target(pane_id, zone, full, cx);
    }

    fn update_pane_drop_target(
        &mut self,
        pane_id: u64,
        event: &DragMoveEvent<PaneDrag>,
        cx: &mut Context<Self>,
    ) {
        let drag = event.drag(cx);
        let same_split = drag.pane_id != pane_id
            && self.workspace_id_for_pane(pane_id) == Some(drag.workspace_id);
        let zone = (same_split && event.bounds.contains(&event.event.position))
            .then(|| drop_zone_at(event.bounds, event.event.position, true));
        self.set_split_drop_target(pane_id, zone, false, cx);
    }

    /// Point the drop preview at `zone` of `pane_id`, or take it away from that pane.
    /// A change of zone on the same pane slides the preview from where it was.
    fn set_split_drop_target(
        &mut self,
        pane_id: u64,
        zone: Option<DropZone>,
        full: bool,
        cx: &mut Context<Self>,
    ) {
        let owns_target = matches!(self.split_drop_target, Some((target, _)) if target == pane_id);
        let Some(zone) = zone else {
            if owns_target {
                self.split_drop_target = None;
                self.split_drop_full = false;
                self.split_drop_preview_from = None;
                cx.notify();
            }
            return;
        };
        let next = Some((pane_id, zone));
        if self.split_drop_target == next && self.split_drop_full == full {
            return;
        }
        self.split_drop_preview_from = match self.split_drop_target {
            Some((target, previous)) if target == pane_id && previous != zone => Some((
                pane_id,
                self.drawn_drop_preview(pane_id, previous),
                Instant::now(),
            )),
            _ => None,
        };
        self.split_drop_target = next;
        self.split_drop_full = full;
        cx.notify();
    }

    /// Where the preview for `zone` is drawn on this frame, as fractions of the pane,
    /// partway along a slide that is still under way.
    fn drawn_drop_preview(&self, pane_id: u64, zone: DropZone) -> MotionRect {
        let to = drop_zone_fraction(zone);
        match self.split_drop_preview_from {
            Some((target, from, started)) if target == pane_id => {
                let tween = motion::Tween::new(from, to, motion::MotionSpeed::DropPreview, started);
                from.lerp(to, tween.progress(Instant::now()))
            }
            _ => to,
        }
    }

    fn render_split_drop_preview(&self, pane_id: u64, zone: DropZone) -> AnyElement {
        let rect = self.drawn_drop_preview(pane_id, zone);
        let full = self.split_drop_full;
        let color = if full {
            theme::danger()
        } else {
            theme::accent()
        };
        let label = if full {
            localization::split_drop_full(MAX_SPLIT_PANES)
        } else {
            localization::static_message(match zone {
                DropZone::Left => MessageId::SplitDropLeft,
                DropZone::Right => MessageId::SplitDropRight,
                DropZone::Top => MessageId::SplitDropUp,
                DropZone::Bottom => MessageId::SplitDropDown,
                DropZone::Center => MessageId::SplitDropSwap,
            })
        };
        let sliding = self
            .split_drop_preview_from
            .is_some_and(|(target, _, started)| {
                target == pane_id && started.elapsed() < motion::MotionSpeed::DropPreview.duration()
            });
        // Faint outlines of every zone the pane offers, under the one that will be used.
        let mut zones = div().absolute().inset_0();
        // Only a pane moving within its split can swap; a dragged tab cannot.
        let offers_center = self.dragging_pane;
        for guide in [
            DropZone::Left,
            DropZone::Right,
            DropZone::Top,
            DropZone::Bottom,
        ]
        .into_iter()
        .map(drop_zone_fraction)
        .chain(offers_center.then(center_zone_guide))
        {
            zones = zones.child(
                div()
                    .absolute()
                    .left(relative(guide.x))
                    .top(relative(guide.y))
                    .w(relative(guide.width))
                    .h(relative(guide.height))
                    .rounded(px(theme::SPACE_2))
                    .border_1()
                    .border_color(theme::with_alpha(theme::accent(), 0.35)),
            );
        }
        let preview = div()
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .p(px(DROP_PREVIEW_INSET))
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::SPACE_3))
                    .border_2()
                    .border_color(color)
                    .bg(theme::with_alpha(color, 0.16))
                    .child(
                        div()
                            .px(px(theme::SPACE_3))
                            .py(px(theme::SPACE_1))
                            .rounded(px(theme::SPACE_2))
                            .bg(color)
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .font_semibold()
                            .text_color(theme::terminal_bg())
                            .child(label),
                    ),
            );
        div()
            .id(("split-drop-zone", pane_id))
            .debug_selector(move || format!("split-drop-zone-{pane_id}"))
            .absolute()
            .inset_0()
            .child(zones)
            .child(preview)
            .map(|overlay| {
                if sliding {
                    overlay.into_any_element()
                } else {
                    overlay
                        .with_animation(
                            ("split-drop-zone-fade", pane_id),
                            Animation::new(motion::MotionSpeed::DropPreview.animation_duration()),
                            |element, delta| element.opacity(delta),
                        )
                        .into_any_element()
                }
            })
    }

    /// Move a pane dragged by its header to an edge of another pane in its split,
    /// or trade places with it when dropped in the middle.
    pub(super) fn drop_pane_on_pane(
        &mut self,
        dragged: u64,
        target: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let zone = self
            .split_drop_target
            .and_then(|(pane_id, zone)| (pane_id == target).then_some(zone));
        self.split_drop_target = None;
        self.split_drop_full = false;
        self.split_drop_preview_from = None;
        self.dragging_pane = false;
        let Some(zone) = zone else {
            cx.notify();
            return;
        };
        let Some(workspace_id) = self
            .workspace_id_for_pane(dragged)
            .filter(|workspace_id| self.workspace_id_for_pane(target) == Some(*workspace_id))
        else {
            cx.notify();
            return;
        };
        if self.active_workspace_id == Some(workspace_id) {
            self.begin_layout_transition(motion::MotionSpeed::Quick);
        }
        self.zoomed_panes.remove(&workspace_id);
        let moved = self
            .workspace_mut(workspace_id)
            .and_then(|workspace| workspace.layout.as_mut())
            .is_some_and(|layout| match zone {
                DropZone::Center => layout.swap_leaves(dragged, target),
                DropZone::Left => layout.move_leaf(dragged, target, SplitEdge::Left),
                DropZone::Right => layout.move_leaf(dragged, target, SplitEdge::Right),
                DropZone::Top => layout.move_leaf(dragged, target, SplitEdge::Top),
                DropZone::Bottom => layout.move_leaf(dragged, target, SplitEdge::Bottom),
            });
        if !moved {
            self.layout_transition = None;
            cx.notify();
            return;
        }
        self.activate_pane(dragged, window, cx);
        self.sync_terminal_layout(window, cx);
        self.persist_runtime_state();
        cx.notify();
    }

    /// The bar across the top of a split pane: its state, name and address, the
    /// broadcast mark, and zoom and close. Dragging it moves the pane.
    fn render_split_pane_header(
        &self,
        pane: &SessionPane,
        drag: PaneDrag,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pane_id = pane.id;
        let active = self
            .active_workspace()
            .is_some_and(|workspace| workspace.active_pane_id == pane_id);
        let broadcast = self.active_workspace().is_some_and(|workspace| {
            workspace.broadcast_input && workspace.pane_ids.len() > 1 && !pane.closed
        });
        let zoomed = self.zoomed_pane() == Some(pane_id);
        let renaming = self.pane_rename_id == Some(pane_id);
        let status_color = if pane.connected {
            theme::success()
        } else if pane.closed {
            theme::text_muted_dark()
        } else {
            theme::accent()
        };
        let connecting = !pane.connected && !pane.closed;
        let dot = div()
            .flex_none()
            .size(px(PANE_STATUS_DOT))
            .rounded_full()
            .bg(status_color);
        let dot = if connecting {
            dot.with_animation(
                ("pane-status-pulse", pane_id),
                Animation::new(Duration::from_millis(1200))
                    .repeat()
                    .with_easing(gpui::pulsating_between(0.35, 1.0)),
                |dot, delta| dot.opacity(delta),
            )
            .into_any_element()
        } else {
            dot.into_any_element()
        };
        let icon_button = |id: (&'static str, u64), icon: IconName| {
            div()
                .id(id)
                .flex_none()
                .size(px(PANE_HEADER_BUTTON))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::SPACE_1))
                .text_color(theme::text_muted_dark())
                .cursor_pointer()
                .hover(|style| {
                    style
                        .bg(theme::with_alpha(theme::hover(), 0.5))
                        .text_color(theme::text_on_dark())
                })
                // A click on a button must not start dragging the pane.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Icon::new(icon).size(px(theme::ICON_SIZE_DEFAULT)))
        };
        h_flex()
            .id(("split-pane-header", pane_id))
            .debug_selector(move || format!("split-pane-header-{pane_id}"))
            .flex_none()
            .w_full()
            .h(px(PANE_HEADER_HEIGHT))
            .items_center()
            .gap(px(theme::SPACE_2))
            .pl(px(theme::SPACE_3))
            .pr(px(theme::SPACE_1))
            .bg(if active {
                theme::chrome_tab_active()
            } else {
                theme::terminal_panel()
            })
            .border_b_1()
            .border_color(theme::border_dark())
            .cursor_grab()
            .on_drag(drag, |drag: &PaneDrag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| PaneDragPreview {
                    title: drag.title.clone(),
                })
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.dragging_pane = true;
                    if event.click_count == 2 {
                        this.start_pane_rename(pane_id, window, cx);
                    } else {
                        this.activate_pane(pane_id, window, cx);
                    }
                }),
            )
            .child(dot)
            .child(if renaming {
                div()
                    .w(px(PANE_RENAME_WIDTH))
                    .child(Input::new(&self.pane_rename_input).small())
                    .into_any_element()
            } else {
                div()
                    .flex_none()
                    .max_w(relative(0.5))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .font_semibold()
                    .text_color(if active {
                        theme::text_on_dark()
                    } else {
                        theme::text_secondary()
                    })
                    .child(pane.title.clone())
                    .into_any_element()
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .font_family(self.terminal_font_family(cx))
                    .text_color(theme::text_muted_dark())
                    .child(pane.request.address()),
            )
            .when(broadcast, |header| {
                header.child(
                    div()
                        .flex_none()
                        .text_size(px(PANE_BROADCAST_BADGE_SIZE))
                        .font_bold()
                        .text_color(theme::warning())
                        .child(localization::static_message(
                            MessageId::SplitPaneBroadcastBadge,
                        )),
                )
            })
            .child(
                icon_button(
                    ("split-pane-zoom", pane_id),
                    if zoomed {
                        IconName::Minimize
                    } else {
                        IconName::Maximize
                    },
                )
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(localization::static_message(if zoomed {
                        MessageId::PaneContextRestoreAction
                    } else {
                        MessageId::PaneContextZoomAction
                    }))
                    .build(window, cx)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.activate_pane(pane_id, window, cx);
                    this.toggle_pane_zoom(window, cx);
                })),
            )
            .child(
                icon_button(("split-pane-close", pane_id), IconName::Close).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.close_pane(pane_id, cx);
                    },
                )),
            )
    }

    fn drop_tab_on_pane(
        &mut self,
        source_workspace_id: u64,
        target_pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let zone = self
            .split_drop_target
            .and_then(|(pid, zone)| (pid == target_pane_id).then_some(zone))
            .unwrap_or(DropZone::Right);
        self.merge_tab_as_split(source_workspace_id, target_pane_id, zone, window, cx);
    }
}

fn workspace_sftp_text(message: MessageId) -> String {
    localization::message_id(message).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, point, size};

    fn at(x: f32, y: f32, center: bool) -> DropZone {
        let bounds = Bounds::new(point(px(100.0), px(50.0)), size(px(400.0), px(200.0)));
        drop_zone_at(
            bounds,
            point(px(100.0 + x * 400.0), px(50.0 + y * 200.0)),
            center,
        )
    }

    #[test]
    fn the_nearest_edge_wins_and_the_middle_swaps_only_when_offered() {
        assert_eq!(at(0.1, 0.5, true), DropZone::Left);
        assert_eq!(at(0.9, 0.5, true), DropZone::Right);
        assert_eq!(at(0.5, 0.05, true), DropZone::Top);
        assert_eq!(at(0.4, 0.95, true), DropZone::Bottom);
        assert_eq!(at(0.5, 0.5, true), DropZone::Center);
        assert_eq!(at(0.35, 0.65, true), DropZone::Center);
        assert_ne!(at(0.5, 0.5, false), DropZone::Center);
        assert_eq!(at(0.45, 0.62, false), DropZone::Bottom);
    }

    #[test]
    fn a_drop_takes_the_half_of_the_pane_on_its_edge() {
        assert_eq!(
            drop_zone_fraction(DropZone::Left),
            MotionRect::new(0.0, 0.0, 0.5, 1.0)
        );
        assert_eq!(
            drop_zone_fraction(DropZone::Bottom),
            MotionRect::new(0.0, 0.5, 1.0, 0.5)
        );
        assert_eq!(
            drop_zone_fraction(DropZone::Center),
            MotionRect::new(0.0, 0.0, 1.0, 1.0)
        );
    }
}
