//! SFTP page renderers (local file browser + connect-host empty state and
//! host picker). All methods are part of the `TermiRustApp` impl.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Div, InteractiveElement as _, IntoElement, ParentElement,
    StatefulInteractiveElement as _, Styled, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::{Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};
use termirust_ui_contract::{
    MessageId, SemanticActionValue, SftpAccessibilityCommand, SftpAction, SftpConflictChoice,
    SftpControl, SftpControlRole, SftpRow, SftpRowId, SftpScreen, SftpSemanticSnapshot,
    SftpSurfaceState, stable_sftp_value,
};

use crate::ui::app::TermiRustApp;
use crate::ui::app::types::WorkspaceViewMode;
use crate::ui::localization;
use crate::ui::sftp_local::{read_local_dir, read_local_dir_result};
use crate::ui::theme;
use crate::ui::util::{format_modified_time, format_size};

impl TermiRustApp {
    pub(super) fn sftp_semantic_snapshot(&self, cx: &App) -> Option<SftpSemanticSnapshot> {
        if let Some(workspace) = self
            .active_workspace()
            .filter(|workspace| workspace.view_mode == WorkspaceViewMode::Files)
        {
            return Some(self.workspace_sftp_semantic_snapshot(workspace.id));
        }
        (self.nav_section == super::NavSection::Sftp && self.sftp_library_tab_active())
            .then(|| self.library_sftp_semantic_snapshot(cx))
    }

    fn library_sftp_semantic_snapshot(&self, cx: &App) -> SftpSemanticSnapshot {
        let recording_friendly = self.activity_center.policy().recording_friendly;
        if self.sftp_show_host_picker {
            let row_count = self.saved.profiles.len().max(1);
            let rows = self
                .saved
                .profiles
                .iter()
                .enumerate()
                .map(|(index, profile)| SftpRow {
                    id: sftp_host_row_id(&profile.id),
                    name: profile.display_name(),
                    detail: Some(format!("{}@{}", profile.username, profile.endpoint())),
                    status: MessageId::SftpRowHost,
                    selected: false,
                    disabled: false,
                    activatable: true,
                    stale: false,
                    position: index + 1,
                    set_size: row_count,
                })
                .collect::<Vec<_>>();
            let mut controls = vec![sftp_control(
                SftpAction::CloseHostPicker,
                None,
                SftpControlRole::Button,
                MessageId::SftpCloseHostPickerAction,
                None,
                false,
            )];
            controls.extend(rows.iter().map(|row| {
                sftp_control(
                    SftpAction::ConnectHost(row.id),
                    Some(row.id),
                    SftpControlRole::Button,
                    MessageId::SftpConnectHostAction,
                    None,
                    false,
                )
            }));
            return SftpSemanticSnapshot {
                screen: SftpScreen::HostPicker,
                state: if rows.is_empty() {
                    SftpSurfaceState::HostRequired
                } else {
                    SftpSurfaceState::Ready
                },
                rows,
                controls,
                recording_friendly,
            };
        }

        let filter = self
            .shell_inputs
            .sftp_local_filter
            .read(cx)
            .value()
            .trim()
            .to_ascii_lowercase();
        let listing = read_local_dir_result(&self.sftp_local_path);
        let state_from_read = listing.as_ref().err().map(|error| match error.kind() {
            std::io::ErrorKind::PermissionDenied => SftpSurfaceState::PermissionDenied,
            std::io::ErrorKind::NotFound => SftpSurfaceState::Stale,
            _ => SftpSurfaceState::Error,
        });
        let entries = listing
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| filter.is_empty() || entry.name.to_ascii_lowercase().contains(&filter))
            .take(termirust_ui_contract::MAX_SFTP_ROWS)
            .collect::<Vec<_>>();
        let row_count = entries.len().max(1);
        let rows = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let path = self.sftp_local_path.join(&entry.name);
                SftpRow {
                    id: SftpRowId::local(stable_sftp_value(&path.to_string_lossy())),
                    name: entry.name.clone(),
                    detail: Some(path.to_string_lossy().to_string()),
                    status: if entry.is_dir {
                        MessageId::SftpRowLocalFolder
                    } else {
                        MessageId::SftpRowLocalFile
                    },
                    selected: false,
                    disabled: false,
                    activatable: entry.is_dir,
                    stale: false,
                    position: index + 1,
                    set_size: row_count,
                }
            })
            .collect::<Vec<_>>();
        let listing_state = if rows.is_empty() && !filter.is_empty() {
            SftpSurfaceState::FilterEmpty
        } else if rows.is_empty() {
            SftpSurfaceState::Empty
        } else {
            SftpSurfaceState::Ready
        };
        let state = state_from_read.unwrap_or(listing_state);
        SftpSemanticSnapshot {
            screen: SftpScreen::Library,
            state,
            rows,
            controls: vec![
                sftp_control(
                    SftpAction::ToggleLocalFilter,
                    None,
                    SftpControlRole::Button,
                    MessageId::SftpFilterAction,
                    None,
                    false,
                ),
                sftp_control(
                    SftpAction::SetLocalFilter,
                    None,
                    SftpControlRole::TextField,
                    MessageId::SftpFilterField,
                    (!filter.is_empty()).then_some(filter),
                    false,
                ),
                sftp_control(
                    SftpAction::OpenLocalFolder,
                    None,
                    SftpControlRole::Button,
                    MessageId::SftpOpenLocalAction,
                    None,
                    false,
                ),
                sftp_control(
                    SftpAction::NavigateLocalParent,
                    None,
                    SftpControlRole::Button,
                    MessageId::SftpParentAction,
                    None,
                    self.sftp_local_path.parent().is_none(),
                ),
                sftp_control(
                    SftpAction::ShowHostPicker,
                    None,
                    SftpControlRole::Button,
                    MessageId::SftpSelectHostAction,
                    None,
                    false,
                ),
            ],
            recording_friendly,
        }
    }

    fn workspace_sftp_semantic_snapshot(&self, workspace_id: u64) -> SftpSemanticSnapshot {
        let recording_friendly = self.activity_center.policy().recording_friendly;
        let Some(workspace) = self.workspace(workspace_id) else {
            return empty_workspace_sftp_snapshot(
                workspace_id,
                SftpSurfaceState::Stale,
                recording_friendly,
            );
        };
        let Some(browser) = workspace.sftp.as_ref() else {
            let local = self
                .active_pane()
                .is_some_and(|pane| pane.request.is_local_shell());
            return SftpSemanticSnapshot {
                screen: SftpScreen::Workspace,
                state: if local {
                    SftpSurfaceState::LocalUnavailable
                } else {
                    SftpSurfaceState::HostRequired
                },
                rows: Vec::new(),
                controls: vec![
                    sftp_control(
                        SftpAction::OpenWorkspaceFiles(workspace_id),
                        None,
                        SftpControlRole::Button,
                        MessageId::SftpOpenFilesAction,
                        None,
                        local,
                    ),
                    sftp_control(
                        SftpAction::BackToTerminal(workspace_id),
                        None,
                        SftpControlRole::Button,
                        MessageId::SftpBackTerminalAction,
                        None,
                        false,
                    ),
                ],
                recording_friendly,
            };
        };

        let mut rows = browser
            .entries
            .iter()
            .take(termirust_ui_contract::MAX_SFTP_ROWS.saturating_sub(1))
            .enumerate()
            .map(|(index, entry)| SftpRow {
                id: remote_sftp_row_id(workspace_id, &entry.path),
                name: non_empty_sftp_name(&entry.name, &entry.path),
                detail: Some(entry.path.clone()),
                status: if entry.is_dir {
                    MessageId::SftpRowRemoteFolder
                } else if entry.is_symlink {
                    MessageId::SftpRowRemoteSymlink
                } else {
                    MessageId::SftpRowRemoteFile
                },
                selected: browser.selected_path.as_deref() == Some(entry.path.as_str()),
                disabled: browser.loading,
                activatable: true,
                stale: false,
                position: index + 1,
                set_size: browser.entries.len().max(1),
            })
            .collect::<Vec<_>>();
        let transfer_state = browser.transfer.as_ref().map(classify_sftp_transfer_state);
        if let Some(transfer) = browser.transfer.as_ref() {
            let row_id = SftpRowId::transfer(workspace_id, transfer.request.operation_id());
            rows.push(SftpRow {
                id: row_id,
                name: transfer.direction.label().to_string(),
                detail: Some(transfer.status.clone()),
                status: transfer_state.unwrap_or(SftpSurfaceState::Error).message(),
                selected: false,
                disabled: false,
                activatable: false,
                stale: false,
                position: rows.len() + 1,
                set_size: rows.len() + 1,
            });
            let total = rows.len();
            for row in &mut rows {
                row.set_size = total;
            }
        }
        let selected = self.selected_workspace_sftp_entry(workspace_id);
        let transfer_active = browser
            .transfer
            .as_ref()
            .is_some_and(|transfer| transfer.active);
        let mut controls = vec![
            sftp_control(
                SftpAction::BackToTerminal(workspace_id),
                None,
                SftpControlRole::Button,
                MessageId::SftpBackTerminalAction,
                None,
                false,
            ),
            sftp_control(
                SftpAction::NavigateRemoteParent(workspace_id),
                None,
                SftpControlRole::Button,
                MessageId::SftpParentAction,
                None,
                super::remote_parent_path(&browser.current_path).is_none(),
            ),
            sftp_control(
                SftpAction::RefreshRemote(workspace_id),
                None,
                SftpControlRole::Button,
                MessageId::SftpRefreshAction,
                None,
                browser.loading,
            ),
            sftp_control(
                SftpAction::Upload(workspace_id),
                None,
                SftpControlRole::Button,
                MessageId::SftpUploadAction,
                None,
                transfer_active,
            ),
            sftp_control(
                SftpAction::Download(workspace_id),
                None,
                SftpControlRole::Button,
                MessageId::SftpDownloadAction,
                None,
                transfer_active || selected.as_ref().is_none_or(|entry| entry.is_dir),
            ),
            sftp_control(
                SftpAction::Delete(workspace_id),
                None,
                SftpControlRole::Button,
                MessageId::SftpDeleteAction,
                None,
                transfer_active || selected.is_none(),
            ),
        ];
        controls.extend(
            rows.iter()
                .filter(|row| row.id.kind == termirust_ui_contract::SftpRowKind::RemoteEntry)
                .map(|row| {
                    sftp_control(
                        SftpAction::SelectEntry(row.id),
                        Some(row.id),
                        SftpControlRole::Button,
                        MessageId::SftpSelectEntryAction,
                        None,
                        row.disabled,
                    )
                }),
        );
        if let Some(transfer) = browser.transfer.as_ref() {
            let operation_id = transfer.request.operation_id();
            let transfer_row = SftpRowId::transfer(workspace_id, operation_id);
            if transfer.active {
                controls.push(sftp_control(
                    SftpAction::CancelTransfer {
                        workspace_id,
                        operation_id,
                    },
                    Some(transfer_row),
                    SftpControlRole::Button,
                    MessageId::SftpCancelTransferAction,
                    None,
                    false,
                ));
            } else if transfer.conflict.is_none() && transfer.sha256.is_none() {
                controls.push(sftp_control(
                    SftpAction::RetryTransfer {
                        workspace_id,
                        operation_id,
                    },
                    Some(transfer_row),
                    SftpControlRole::Button,
                    MessageId::SftpRetryTransferAction,
                    None,
                    false,
                ));
            }
            if let Some(conflict) = transfer.conflict {
                for (choice, name, disabled) in [
                    (
                        SftpConflictChoice::Replace,
                        MessageId::SftpReplaceAction,
                        false,
                    ),
                    (SftpConflictChoice::Skip, MessageId::SftpSkipAction, false),
                    (
                        SftpConflictChoice::Resume,
                        MessageId::SftpResumeTransferAction,
                        !conflict.resume_available,
                    ),
                ] {
                    controls.push(sftp_control(
                        SftpAction::ResolveConflict {
                            workspace_id,
                            operation_id,
                            choice,
                        },
                        Some(transfer_row),
                        SftpControlRole::Button,
                        name,
                        None,
                        disabled,
                    ));
                }
            }
        }
        let directory_state = if browser.loading {
            SftpSurfaceState::Loading
        } else if browser.entries.is_empty() {
            SftpSurfaceState::Empty
        } else {
            SftpSurfaceState::Ready
        };
        SftpSemanticSnapshot {
            screen: SftpScreen::Workspace,
            state: transfer_state.unwrap_or(directory_state),
            rows,
            controls,
            recording_friendly,
        }
    }

    pub(super) fn handle_sftp_accessibility_command(
        &mut self,
        command: SftpAccessibilityCommand,
        value: Option<SemanticActionValue>,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.sftp_semantic_snapshot(cx) else {
            return;
        };
        match command {
            SftpAccessibilityCommand::FocusRow(_) => self.project_list_focus.focus(window),
            SftpAccessibilityCommand::ActivateRow(row_id) => {
                if snapshot
                    .rows
                    .iter()
                    .any(|row| row.id == row_id && row.activatable && !row.disabled)
                {
                    self.activate_sftp_row(row_id, window, cx);
                }
            }
            SftpAccessibilityCommand::FocusControl(action) => {
                if snapshot
                    .controls
                    .iter()
                    .any(|control| control.action == action)
                {
                    if action == SftpAction::SetLocalFilter {
                        self.shell_inputs
                            .sftp_local_filter
                            .update(cx, |state, cx| state.focus(window, cx));
                    } else {
                        self.project_list_focus.focus(window);
                    }
                }
            }
            SftpAccessibilityCommand::SetControlValue(action) => {
                if snapshot.controls.iter().any(|control| {
                    control.action == action
                        && control.role == SftpControlRole::TextField
                        && !control.disabled
                }) && let Some(SemanticActionValue::Text(value)) = value
                {
                    Self::set_input_value(&self.shell_inputs.sftp_local_filter, value, window, cx);
                }
            }
            SftpAccessibilityCommand::ActivateControl(action) => {
                if snapshot.controls.iter().any(|control| {
                    control.action == action
                        && control.role == SftpControlRole::Button
                        && !control.disabled
                }) {
                    self.activate_sftp_control(action, window, cx);
                }
            }
        }
    }

    fn activate_sftp_row(
        &mut self,
        row_id: SftpRowId,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        match row_id.kind {
            termirust_ui_contract::SftpRowKind::LocalEntry => {
                if let Some(entry) =
                    read_local_dir(&self.sftp_local_path)
                        .into_iter()
                        .find(|entry| {
                            let path = self.sftp_local_path.join(&entry.name);
                            SftpRowId::local(stable_sftp_value(&path.to_string_lossy())) == row_id
                        })
                    && entry.is_dir
                {
                    self.sftp_local_path.push(entry.name);
                    cx.notify();
                }
            }
            termirust_ui_contract::SftpRowKind::Host => {
                if let Some(profile_id) = self
                    .saved
                    .profiles
                    .iter()
                    .find(|profile| sftp_host_row_id(&profile.id) == row_id)
                    .map(|profile| profile.id.clone())
                {
                    self.sftp_show_host_picker = false;
                    self.open_connect_dialog_tab(&profile_id, window, cx);
                }
            }
            termirust_ui_contract::SftpRowKind::RemoteEntry => {
                let workspace_id = row_id.owner as u64;
                if self.active_workspace_id != Some(workspace_id) {
                    return;
                }
                let entry =
                    self.workspace(workspace_id)
                        .and_then(|workspace| {
                            workspace.sftp.as_ref()?.entries.iter().find(|entry| {
                                remote_sftp_row_id(workspace_id, &entry.path) == row_id
                            })
                        })
                        .cloned();
                if let Some(entry) = entry {
                    self.select_workspace_file_entry(workspace_id, entry.path, cx);
                    if entry.is_dir {
                        self.open_selected_workspace_file_entry(cx);
                    }
                }
            }
            termirust_ui_contract::SftpRowKind::Transfer => {}
        }
    }

    fn activate_sftp_control(
        &mut self,
        action: SftpAction,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SftpAction::ToggleLocalFilter => {
                self.sftp_local_filter_visible = !self.sftp_local_filter_visible;
                if self.sftp_local_filter_visible {
                    self.shell_inputs
                        .sftp_local_filter
                        .update(cx, |state, cx| state.focus(window, cx));
                }
                cx.notify();
            }
            SftpAction::OpenLocalFolder => {
                let _ = std::process::Command::new("open")
                    .arg(&self.sftp_local_path)
                    .spawn();
            }
            SftpAction::NavigateLocalParent => {
                if let Some(parent) = self.sftp_local_path.parent() {
                    self.sftp_local_path = parent.to_path_buf();
                    cx.notify();
                }
            }
            SftpAction::ShowHostPicker => {
                self.sftp_show_host_picker = true;
                cx.notify();
            }
            SftpAction::CloseHostPicker => {
                self.sftp_show_host_picker = false;
                cx.notify();
            }
            SftpAction::ConnectHost(row) => self.activate_sftp_row(row, window, cx),
            SftpAction::OpenWorkspaceFiles(workspace_id) => {
                if self.active_workspace_id == Some(workspace_id) {
                    self.open_active_workspace_files(cx);
                }
            }
            SftpAction::BackToTerminal(workspace_id) => {
                if self.active_workspace_id == Some(workspace_id) {
                    self.show_active_workspace_terminal(cx);
                }
            }
            SftpAction::NavigateRemoteParent(workspace_id) => {
                if self.active_workspace_id == Some(workspace_id) {
                    self.navigate_workspace_files_up(cx);
                }
            }
            SftpAction::RefreshRemote(workspace_id) => self.refresh_workspace_files(workspace_id),
            SftpAction::Upload(workspace_id) if self.active_workspace_id == Some(workspace_id) => {
                self.upload_workspace_file(window, cx);
            }
            SftpAction::Download(workspace_id)
                if self.active_workspace_id == Some(workspace_id) =>
            {
                self.download_workspace_file(window, cx);
            }
            SftpAction::Delete(workspace_id) if self.active_workspace_id == Some(workspace_id) => {
                self.delete_workspace_file(cx);
            }
            SftpAction::SelectEntry(row) | SftpAction::OpenEntry(row) => {
                self.activate_sftp_row(row, window, cx);
            }
            SftpAction::CancelTransfer { workspace_id, .. } => {
                self.cancel_workspace_transfer(workspace_id, cx);
            }
            SftpAction::RetryTransfer { workspace_id, .. } => {
                self.retry_workspace_transfer(workspace_id, cx);
            }
            SftpAction::ResolveConflict {
                workspace_id,
                choice,
                ..
            } => {
                let policy = match choice {
                    SftpConflictChoice::Replace => crate::sftp::SftpConflictPolicy::Replace,
                    SftpConflictChoice::Skip => crate::sftp::SftpConflictPolicy::Skip,
                    SftpConflictChoice::Resume => crate::sftp::SftpConflictPolicy::Resume,
                };
                self.resolve_workspace_transfer(workspace_id, policy, cx);
            }
            SftpAction::SetLocalFilter
            | SftpAction::Upload(_)
            | SftpAction::Download(_)
            | SftpAction::Delete(_) => {}
        }
    }

    pub(super) fn render_sftp_view(&self, cx: &mut Context<Self>) -> Div {
        gpui::div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .size_full()
            .bg(theme::library_bg())
            .child(self.render_sftp_local_pane(cx))
            .child(
                gpui::div()
                    .w(px(theme::BORDER_HAIRLINE))
                    .h_full()
                    .bg(theme::soft_border()),
            )
            .child(if self.sftp_show_host_picker {
                self.render_sftp_host_picker(cx)
            } else {
                self.render_sftp_remote_empty(cx)
            })
    }

    fn render_sftp_local_pane(&self, cx: &mut Context<Self>) -> Div {
        let listing = read_local_dir_result(&self.sftp_local_path);
        let local_error = listing.as_ref().err().map(|error| match error.kind() {
            std::io::ErrorKind::PermissionDenied => MessageId::SftpStatePermission,
            std::io::ErrorKind::NotFound => MessageId::SftpStateStale,
            _ => MessageId::SftpStateError,
        });
        let mut entries = listing.unwrap_or_default();
        let filter_value = self
            .shell_inputs
            .sftp_local_filter
            .read(cx)
            .value()
            .trim()
            .to_ascii_lowercase();
        if !filter_value.is_empty() {
            entries.retain(|e| e.name.to_ascii_lowercase().contains(&filter_value));
        }
        let local_state_message = local_error.or_else(|| {
            entries.is_empty().then_some(if filter_value.is_empty() {
                MessageId::SftpStateEmpty
            } else {
                MessageId::SftpStateFilterEmpty
            })
        });
        let path_segments: Vec<String> = self
            .sftp_local_path
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => Some(s.to_string_lossy().to_string()),
                _ => None,
            })
            .collect();
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                h_flex()
                    .h(px(theme::WORKSPACE_HEADER_HEIGHT))
                    .px(px(theme::TYPE_HEADING_SMALL_SIZE))
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_3))
                            .items_center()
                            .child(
                                gpui::div()
                                    .size(px(theme::SFTP_ICON_CONTAINER_SMALL))
                                    .rounded(px(theme::SPACE_FINE))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(theme::with_alpha(theme::accent(), 0.2))
                                    .child(
                                        Icon::new(IconName::Folder)
                                            .size(px(theme::TYPE_BODY_SMALL_SIZE))
                                            .text_color(theme::accent()),
                                    ),
                            )
                            .child(
                                gpui::div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(sftp_text(MessageId::SftpLocalPane)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(theme::TYPE_CAPTION_SIZE))
                            .items_center()
                            .child(
                                h_flex()
                                    .id("sftp-filter-toggle")
                                    .gap(px(theme::SPACE_2))
                                    .items_center()
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme::text_main()))
                                    .child(
                                        Icon::new(IconName::Search)
                                            .size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted()),
                                    )
                                    .child(
                                        gpui::div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(sftp_text(MessageId::SftpFilterAction)),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.sftp_local_filter_visible =
                                            !this.sftp_local_filter_visible;
                                        if this.sftp_local_filter_visible {
                                            this.shell_inputs
                                                .sftp_local_filter
                                                .update(cx, |state, cx| state.focus(window, cx));
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                h_flex()
                                    .id("sftp-actions-open")
                                    .gap(px(theme::SPACE_2))
                                    .items_center()
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme::text_main()))
                                    .child(
                                        gpui::div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(sftp_text(MessageId::SftpOpenLocalAction)),
                                    )
                                    .child(
                                        Icon::new(IconName::ChevronDown)
                                            .size(px(theme::TYPE_MICRO_SIZE))
                                            .text_color(theme::text_muted()),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let path = this.sftp_local_path.clone();
                                        let _ =
                                            std::process::Command::new("open").arg(&path).spawn();
                                        this.status_message =
                                            localization::sftp_opened_local_folder(
                                                path.display().to_string(),
                                            );
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(theme::SFTP_PATH_ROW_HEIGHT))
                    .px(px(theme::TYPE_HEADING_SMALL_SIZE))
                    .items_center()
                    .gap(px(theme::SPACE_DENSE))
                    .child(
                        gpui::div()
                            .id("sftp-back")
                            .size(px(theme::SFTP_ICON_CONTAINER_SMALL))
                            .rounded(px(theme::SPACE_FINE))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.7)))
                            .child(
                                Icon::new(IconName::ArrowLeft)
                                    .size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .text_color(theme::text_muted()),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(parent) = this.sftp_local_path.parent() {
                                    this.sftp_local_path = parent.to_path_buf();
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Icon::new(IconName::ArrowRight)
                            .size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .text_color(theme::with_alpha(theme::text_muted(), 0.5)),
                    )
                    .children(path_segments.iter().enumerate().flat_map(|(idx, seg)| {
                        let is_last = idx == path_segments.len() - 1;
                        let mut items: Vec<AnyElement> = Vec::new();
                        items.push(
                            h_flex()
                                .gap(px(theme::SPACE_2))
                                .items_center()
                                .child(
                                    Icon::new(IconName::Folder)
                                        .size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::accent()),
                                )
                                .child(
                                    gpui::div()
                                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                                        .text_color(theme::text_main())
                                        .child(seg.clone()),
                                )
                                .into_any_element(),
                        );
                        if !is_last {
                            items.push(
                                Icon::new(IconName::ChevronRight)
                                    .size(px(theme::TYPE_MICRO_SIZE))
                                    .text_color(theme::text_muted())
                                    .into_any_element(),
                            );
                        }
                        items
                    })),
            )
            .when(self.sftp_local_filter_visible, |this| {
                this.child(
                    gpui::div()
                        .px(px(theme::TYPE_HEADING_SMALL_SIZE))
                        .pb(px(theme::SPACE_3))
                        .child(Input::new(&self.shell_inputs.sftp_local_filter).xsmall()),
                )
            })
            .child(
                h_flex()
                    .h(px(theme::SFTP_COLUMN_HEADER_HEIGHT))
                    .px(px(theme::TYPE_HEADING_SMALL_SIZE))
                    .items_center()
                    .border_t_1()
                    .border_color(theme::soft_border())
                    .border_b_1()
                    .child(
                        gpui::div()
                            .w(px(theme::SFTP_COLUMN_NAME_WIDTH))
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(sftp_text(MessageId::SftpColumnName)),
                    )
                    .child(
                        gpui::div()
                            .w(px(theme::SFTP_COLUMN_MODIFIED_WIDTH))
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(sftp_text(MessageId::SftpColumnModified)),
                    )
                    .child(
                        gpui::div()
                            .w(px(theme::SFTP_COLUMN_SIZE_WIDTH))
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(sftp_text(MessageId::SftpColumnSize)),
                    )
                    .child(
                        gpui::div()
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(sftp_text(MessageId::SftpColumnKind)),
                    ),
            )
            .child(
                v_flex()
                    .id("sftp-local-list")
                    .debug_selector(|| "sftp-local-list".to_string())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when_some(local_state_message, |this, message| {
                        this.child(
                            gpui::div()
                                .p(px(theme::SPACE_5))
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .text_color(theme::text_muted())
                                .child(sftp_text(message)),
                        )
                    })
                    .children(entries.into_iter().enumerate().map(|(idx, entry)| {
                        let path = self.sftp_local_path.join(&entry.name);
                        let is_dir = entry.is_dir;
                        let date_str = entry
                            .modified
                            .map(format_modified_time)
                            .unwrap_or_else(|| "--".to_string());
                        let size_str = if is_dir {
                            "--".to_string()
                        } else {
                            format_size(entry.size)
                        };
                        let kind_str = if is_dir {
                            sftp_text(MessageId::SftpFolderKind)
                        } else {
                            sftp_text(MessageId::SftpFileKind)
                        };
                        let entry_clone = entry.name.clone();
                        h_flex()
                            .id(("sftp-row", idx))
                            .h(px(theme::SFTP_LOCAL_ROW_HEIGHT))
                            .px(px(theme::TYPE_HEADING_SMALL_SIZE))
                            .items_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.4)))
                            .child(
                                h_flex()
                                    .w(px(theme::SFTP_COLUMN_NAME_WIDTH))
                                    .gap(px(theme::SPACE_3))
                                    .items_center()
                                    .child(
                                        Icon::new(if is_dir {
                                            IconName::Folder
                                        } else {
                                            IconName::File
                                        })
                                        .size(px(theme::TYPE_BODY_SIZE))
                                        .text_color(theme::accent()),
                                    )
                                    .child(
                                        gpui::div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_main())
                                            .child(entry_clone),
                                    ),
                            )
                            .child(
                                gpui::div()
                                    .w(px(theme::SFTP_COLUMN_MODIFIED_WIDTH))
                                    .text_size(px(theme::TYPE_MICRO_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(date_str),
                            )
                            .child(
                                gpui::div()
                                    .w(px(theme::SFTP_COLUMN_SIZE_WIDTH))
                                    .text_size(px(theme::TYPE_MICRO_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(size_str),
                            )
                            .child(
                                gpui::div()
                                    .text_size(px(theme::TYPE_MICRO_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(kind_str),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if is_dir {
                                    this.sftp_local_path = path.clone();
                                    cx.notify();
                                }
                            }))
                    })),
            )
    }

    fn render_sftp_remote_empty(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .justify_center()
            .gap(px(theme::TYPE_BODY_SIZE))
            .child(
                gpui::div()
                    .size(px(theme::SFTP_EMPTY_ICON_CONTAINER))
                    .rounded(px(theme::TYPE_CAPTION_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::with_alpha(theme::hover(), 0.7))
                    .child(
                        Icon::new(IconName::Folder)
                            .size(px(theme::SFTP_EMPTY_ICON_SIZE))
                            .text_color(theme::text_main()),
                    ),
            )
            .child(
                gpui::div()
                    .text_size(px(theme::ICON_SIZE_COMPACT))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(sftp_text(MessageId::SftpConnectEmptyTitle)),
            )
            .child(
                gpui::div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .max_w(px(theme::SFTP_EMPTY_COPY_WIDTH))
                    .child(sftp_text(MessageId::SftpConnectEmptyDescription)),
            )
            .child(
                Button::new("sftp-select-host")
                    .small()
                    .custom(Self::action_button_style(theme::ActionTone::Neutral, cx))
                    .label(sftp_text(MessageId::SftpSelectHostAction))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sftp_show_host_picker = true;
                        cx.notify();
                    })),
            )
    }

    fn render_sftp_host_picker(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .h(px(theme::WORKSPACE_HEADER_HEIGHT))
                    .px(px(theme::TYPE_HEADING_SMALL_SIZE))
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_COMPACT))
                            .items_center()
                            .child(
                                gpui::div()
                                    .id("sftp-picker-back")
                                    .size(px(theme::SFTP_ICON_CONTAINER_SMALL))
                                    .rounded(px(theme::SPACE_FINE))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.7)))
                                    .child(
                                        Icon::new(IconName::ArrowLeft)
                                            .size(px(theme::TYPE_BODY_SMALL_SIZE))
                                            .text_color(theme::text_main()),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sftp_show_host_picker = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                v_flex()
                                    .gap(px(theme::SPACE_1))
                                    .child(
                                        gpui::div()
                                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                            .font_semibold()
                                            .text_color(theme::text_main())
                                            .child(sftp_text(MessageId::SftpHostPickerTitle)),
                                    )
                                    .child(
                                        h_flex()
                                            .gap(px(theme::SPACE_2))
                                            .items_center()
                                            .child(
                                                gpui::div()
                                                    .text_size(px(theme::TYPE_MICRO_SIZE))
                                                    .text_color(theme::text_muted())
                                                    .child(sftp_text(MessageId::SftpRemotePane)),
                                            )
                                            .child(
                                                Icon::new(IconName::ChevronDown)
                                                    .size(px(theme::SPACE_COMPACT))
                                                    .text_color(theme::text_muted()),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .h(px(theme::SFTP_PICKER_BADGE_HEIGHT))
                            .px(px(theme::SPACE_3))
                            .gap(px(theme::SPACE_DENSE))
                            .items_center()
                            .rounded(px(theme::SPACE_DENSE))
                            .bg(theme::with_alpha(theme::accent(), 0.15))
                            .border_1()
                            .border_color(theme::accent())
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(theme::TYPE_MICRO_SIZE))
                                    .text_color(theme::accent()),
                            )
                            .child(
                                gpui::div()
                                    .text_size(px(theme::TYPE_MICRO_SIZE))
                                    .font_semibold()
                                    .text_color(theme::accent())
                                    .child(sftp_text(MessageId::SftpLocalPane)),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(theme::SFTP_PATH_ROW_HEIGHT))
                    .mx(px(theme::TYPE_HEADING_SMALL_SIZE))
                    .px(px(theme::SPACE_COMPACT))
                    .gap(px(theme::SPACE_DENSE))
                    .items_center()
                    .rounded(px(theme::SPACE_DENSE))
                    .bg(theme::library_bg())
                    .border_1()
                    .border_color(theme::soft_border())
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .text_color(theme::text_muted()),
                    )
                    .child(
                        gpui::div()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(sftp_text(MessageId::SftpFilterField)),
                    ),
            )
            .child(
                v_flex()
                    .px(px(theme::TYPE_HEADING_SMALL_SIZE))
                    .pt(px(theme::TYPE_BODY_SIZE))
                    .gap(px(theme::SPACE_DENSE))
                    .child(
                        gpui::div()
                            .text_size(px(theme::TYPE_MICRO_SIZE))
                            .font_semibold()
                            .text_color(theme::text_muted())
                            .child(sftp_text(MessageId::SftpHostPickerTitle)),
                    )
                    .children(
                        self.saved
                            .profiles
                            .iter()
                            .enumerate()
                            .map(|(idx, profile)| {
                                let profile_id = profile.id.clone();
                                let display_name = profile.display_name();
                                let proto_summary = localization::sftp_host_summary(
                                    profile.username.clone(),
                                    profile.endpoint(),
                                );
                                h_flex()
                                    .id(("sftp-host", idx))
                                    .h(px(theme::SFTP_HOST_ROW_HEIGHT))
                                    .gap(px(theme::SPACE_COMPACT))
                                    .items_center()
                                    .px(px(theme::SPACE_3))
                                    .rounded(px(theme::SPACE_DENSE))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme::with_alpha(theme::hover(), 0.5)))
                                    .child(
                                        gpui::div()
                                            .size(px(theme::SFTP_ICON_CONTAINER))
                                            .rounded(px(theme::SPACE_DENSE))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(theme::library_card())
                                            .child(
                                                Icon::new(IconName::SquareTerminal)
                                                    .size(px(theme::ICON_SIZE_COMPACT))
                                                    .text_color(theme::accent()),
                                            ),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .gap(px(theme::SPACE_1))
                                            .child(
                                                gpui::div()
                                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                    .font_semibold()
                                                    .text_color(theme::text_main())
                                                    .child(display_name),
                                            )
                                            .child(
                                                gpui::div()
                                                    .text_size(px(theme::SPACE_COMPACT))
                                                    .text_color(theme::text_muted())
                                                    .child(proto_summary),
                                            ),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.sftp_show_host_picker = false;
                                        this.open_connect_dialog_tab(&profile_id, window, cx);
                                    }))
                            }),
                    ),
            )
    }
}

fn sftp_control(
    action: SftpAction,
    parent: Option<SftpRowId>,
    role: SftpControlRole,
    name: MessageId,
    value: Option<String>,
    disabled: bool,
) -> SftpControl {
    SftpControl {
        action,
        parent,
        role,
        name,
        value,
        selected: false,
        disabled,
        invalid: false,
    }
}

fn sftp_host_row_id(profile_id: &str) -> SftpRowId {
    SftpRowId::host(stable_sftp_value(profile_id))
}

fn remote_sftp_row_id(workspace_id: u64, path: &str) -> SftpRowId {
    SftpRowId::remote(workspace_id, stable_sftp_value(path))
}

fn non_empty_sftp_name(name: &str, path: &str) -> String {
    if name.trim().is_empty() {
        path.rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("/")
            .to_string()
    } else {
        name.to_string()
    }
}

fn empty_workspace_sftp_snapshot(
    workspace_id: u64,
    state: SftpSurfaceState,
    recording_friendly: bool,
) -> SftpSemanticSnapshot {
    SftpSemanticSnapshot {
        screen: SftpScreen::Workspace,
        state,
        rows: Vec::new(),
        controls: vec![sftp_control(
            SftpAction::BackToTerminal(workspace_id),
            None,
            SftpControlRole::Button,
            MessageId::SftpBackTerminalAction,
            None,
            false,
        )],
        recording_friendly,
    }
}

pub(super) fn classify_sftp_transfer_state(
    transfer: &super::WorkspaceSftpTransfer,
) -> SftpSurfaceState {
    classify_sftp_transfer_values(
        &transfer.status,
        transfer.active,
        transfer.conflict.is_some(),
        transfer.sha256.is_some(),
        transfer.transferred_bytes,
    )
}

fn classify_sftp_transfer_values(
    status: &str,
    active: bool,
    conflict: bool,
    verified: bool,
    transferred_bytes: u64,
) -> SftpSurfaceState {
    if conflict {
        return SftpSurfaceState::Conflict;
    }
    let status = status.to_ascii_lowercase();
    if active {
        if status.contains("cancellation requested") {
            return SftpSurfaceState::CancelRequested;
        }
        if status.contains("queued") {
            return SftpSurfaceState::Queued;
        }
        return SftpSurfaceState::Transferring;
    }
    if verified {
        return SftpSurfaceState::Completed;
    }
    if status.contains("cancel") {
        return SftpSurfaceState::Cancelled;
    }
    if status.contains("no space") || status.contains("disk full") {
        return SftpSurfaceState::DiskFull;
    }
    if status.contains("limit") || status.contains("too large") || status.contains("queue is full")
    {
        return SftpSurfaceState::ResourceLimit;
    }
    if status.contains("timeout") || status.contains("timed out") {
        return SftpSurfaceState::Timeout;
    }
    if status.contains("permission") || status.contains("denied") {
        return SftpSurfaceState::PermissionDenied;
    }
    if status.contains("offline") || status.contains("unreachable") || status.contains("network") {
        return SftpSurfaceState::Offline;
    }
    if transferred_bytes > 0 {
        SftpSurfaceState::Partial
    } else {
        SftpSurfaceState::Error
    }
}

fn sftp_text(message: MessageId) -> String {
    localization::message_id(message).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_state_projection_preserves_conflict_cancel_and_partial_evidence() {
        assert_eq!(
            classify_sftp_transfer_values("Upload queued", true, false, false, 0),
            SftpSurfaceState::Queued
        );
        assert_eq!(
            classify_sftp_transfer_values("Cancellation requested...", true, false, false, 64,),
            SftpSurfaceState::CancelRequested
        );
        assert_eq!(
            classify_sftp_transfer_values("waiting", false, true, false, 64),
            SftpSurfaceState::Conflict
        );
        assert_eq!(
            classify_sftp_transfer_values("failed", false, false, false, 64),
            SftpSurfaceState::Partial
        );
        assert_eq!(
            classify_sftp_transfer_values("complete", false, false, true, 64),
            SftpSurfaceState::Completed
        );
    }

    #[test]
    fn remote_entry_ids_are_stable_and_workspace_scoped() {
        let first = remote_sftp_row_id(7, "/srv/data.txt");
        assert_eq!(first, remote_sftp_row_id(7, "/srv/data.txt"));
        assert_ne!(first, remote_sftp_row_id(8, "/srv/data.txt"));
        assert_ne!(first, remote_sftp_row_id(7, "/srv/other.txt"));
    }
}
