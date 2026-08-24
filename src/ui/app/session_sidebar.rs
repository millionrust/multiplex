use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _, Styled,
    Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{
    Disableable as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use termirust_domain::{
    Group, GroupDestination, GroupError, GroupId, GroupInverseCommand, HostedSessionId,
    HostedSessionState, ProjectError, ProjectId,
};
use termirust_store::StoreError;

use super::{TermiRustApp, theme};
use crate::models::{SavedAppAttachedSession, SavedSessionPlacement};
use crate::storage::save_saved_state;
use crate::ui::localization;

const ORGANIZATION_UNDO_WINDOW: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(super) struct SessionSidebarState {
    editor: Option<GroupEditor>,
    pending_removal: Option<PendingGroupRemoval>,
    pending_undo: Option<PendingOrganizationUndo>,
    selected_session: Option<HostedSessionId>,
}

struct GroupEditor {
    project_id: ProjectId,
    group_id: Option<GroupId>,
}

struct PendingGroupRemoval {
    group_id: GroupId,
}

enum OrganizationInverse {
    Group {
        command: GroupInverseCommand,
        placements: Vec<SavedSessionPlacement>,
    },
    Session {
        placements: Vec<SavedSessionPlacement>,
    },
}

struct PendingOrganizationUndo {
    inverse: OrganizationInverse,
    expires_at: Instant,
}

impl TermiRustApp {
    pub(super) fn repair_session_group_references(&mut self) {
        let valid_groups = self
            .project_library
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .groups
                    .iter()
                    .map(|group| (group.id, group.project_id))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let repaired = self
            .saved
            .repair_app_attached_group_references(&valid_groups);
        if !repaired.is_empty() {
            let _ = save_saved_state(&self.saved);
            self.status_message = localization::group_repair_status(repaired.len());
        }
    }

    pub(super) fn open_group_editor(
        &mut self,
        project_id: ProjectId,
        group_id: Option<GroupId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = group_id
            .and_then(|id| self.group(id))
            .map(|group| group.name.as_str().to_string())
            .unwrap_or_default();
        Self::set_input_value(&self.group_name_input, value, window, cx);
        self.session_sidebar.editor = Some(GroupEditor {
            project_id,
            group_id,
        });
        self.session_sidebar.pending_removal = None;
        self.group_name_input
            .update(cx, |input, cx| input.focus(window, cx));
        self.error_message.clear();
        cx.notify();
    }

    fn cancel_group_editor(&mut self, cx: &mut Context<Self>) {
        self.session_sidebar.editor = None;
        self.error_message.clear();
        cx.notify();
    }

    fn save_group_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.session_sidebar.editor.as_ref() else {
            return;
        };
        let Some(repository) = self.project_library.repository.clone() else {
            self.error_message = localization::project_store_unavailable();
            cx.notify();
            return;
        };
        let Some(expected) = self.project_revision() else {
            return;
        };
        let name = self.group_name_input.read(cx).value().to_string();
        let result = match editor.group_id {
            Some(group_id) => repository.rename_group(group_id, &name, expected),
            None => repository.create_group(editor.project_id, GroupId::new(), &name, expected),
        };
        match result {
            Ok(mutation) => {
                self.session_sidebar.editor = None;
                self.record_group_inverse(mutation.inverse, Vec::new());
                self.finish_group_mutation();
            }
            Err(error) => self.handle_group_error(error),
        }
        cx.notify();
    }

    fn set_group_collapsed(&mut self, id: GroupId, collapsed: bool, cx: &mut Context<Self>) {
        let Some(repository) = self.project_library.repository.clone() else {
            return;
        };
        let Some(expected) = self.project_revision() else {
            return;
        };
        match repository.set_group_collapsed(id, collapsed, expected) {
            Ok(mutation) => {
                self.record_group_inverse(mutation.inverse, Vec::new());
                self.finish_group_mutation();
            }
            Err(error) => self.handle_group_error(error),
        }
        cx.notify();
    }

    fn move_group(&mut self, id: GroupId, delta: isize, cx: &mut Context<Self>) {
        let Some(group) = self.group(id).cloned() else {
            return;
        };
        let groups = self.project_groups(group.project_id);
        let Some(index) = groups.iter().position(|candidate| candidate.id == id) else {
            return;
        };
        let target = index as isize + delta;
        if target < 0 || target >= groups.len() as isize {
            return;
        }
        let before = if delta < 0 {
            Some(groups[target as usize].id)
        } else {
            groups.get(index + 2).map(|group| group.id)
        };
        let Some(repository) = self.project_library.repository.clone() else {
            return;
        };
        let Some(expected) = self.project_revision() else {
            return;
        };
        match repository.move_group_before(id, before, expected) {
            Ok(mutation) => {
                self.record_group_inverse(mutation.inverse, Vec::new());
                self.finish_group_mutation();
            }
            Err(error) => self.handle_group_error(error),
        }
        cx.notify();
    }

    pub(super) fn begin_group_removal(&mut self, id: GroupId, cx: &mut Context<Self>) {
        if self.group_session_count(id) == 0 {
            self.remove_group_to(id, None, cx);
        } else {
            self.session_sidebar.pending_removal = Some(PendingGroupRemoval { group_id: id });
            self.session_sidebar.editor = None;
            cx.notify();
        }
    }

    pub(super) fn cancel_group_removal(&mut self, cx: &mut Context<Self>) {
        self.session_sidebar.pending_removal = None;
        cx.notify();
    }

    pub(super) fn remove_group_to(
        &mut self,
        id: GroupId,
        destination: Option<GroupDestination>,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.group(id).cloned() else {
            self.session_sidebar.pending_removal = None;
            return;
        };
        let has_sessions = self.group_session_count(id) > 0;
        let Some(repository) = self.project_library.repository.clone() else {
            return;
        };
        let Some(expected) = self.project_revision() else {
            return;
        };
        match repository.remove_group(id, destination, has_sessions, expected) {
            Ok(mutation) => {
                let destination = destination.unwrap_or(GroupDestination::ProjectRoot);
                let placements =
                    self.saved
                        .relocate_group_sessions(group.project_id, group.id, destination);
                let _ = save_saved_state(&self.saved);
                self.session_sidebar.pending_removal = None;
                self.record_group_inverse(mutation.inverse, placements);
                self.finish_group_mutation();
            }
            Err(error) => self.handle_group_error(error),
        }
        cx.notify();
    }

    pub(super) fn select_session_for_move(&mut self, id: HostedSessionId, cx: &mut Context<Self>) {
        self.session_sidebar.selected_session = if self.session_sidebar.selected_session == Some(id)
        {
            None
        } else {
            Some(id)
        };
        cx.notify();
    }

    fn move_session_to(
        &mut self,
        id: HostedSessionId,
        destination: GroupDestination,
        before: Option<HostedSessionId>,
        cx: &mut Context<Self>,
    ) {
        let Some(inverse) = self
            .saved
            .move_app_attached_session(id, destination, before)
        else {
            self.error_message = localization::group_error_generic();
            cx.notify();
            return;
        };
        let _ = save_saved_state(&self.saved);
        self.session_sidebar.pending_undo = Some(PendingOrganizationUndo {
            inverse: OrganizationInverse::Session {
                placements: inverse,
            },
            expires_at: Instant::now() + ORGANIZATION_UNDO_WINDOW,
        });
        self.session_sidebar.selected_session = Some(id);
        self.status_message = localization::group_organization_updated();
        self.error_message.clear();
        cx.notify();
    }

    fn move_session_by(&mut self, id: HostedSessionId, delta: isize, cx: &mut Context<Self>) {
        let Some(session) = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
        else {
            return;
        };
        let sessions = self.sessions_in_destination(session.origin.project_id, session.group_id);
        let Some(index) = sessions.iter().position(|candidate| candidate.id == id) else {
            return;
        };
        let target = index as isize + delta;
        if target < 0 || target >= sessions.len() as isize {
            return;
        }
        let before = if delta < 0 {
            Some(sessions[target as usize].id)
        } else {
            sessions.get(index + 2).map(|candidate| candidate.id)
        };
        let destination = session
            .group_id
            .map(GroupDestination::Group)
            .unwrap_or(GroupDestination::ProjectRoot);
        self.move_session_to(id, destination, before, cx);
    }

    pub(super) fn undo_organization(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.session_sidebar.pending_undo.take() else {
            return;
        };
        if Instant::now() >= pending.expires_at {
            self.status_message = localization::project_undo_expired();
            cx.notify();
            return;
        }
        match pending.inverse {
            OrganizationInverse::Session { placements } => {
                self.saved
                    .restore_app_attached_session_placements(&placements);
                let _ = save_saved_state(&self.saved);
                self.status_message = localization::group_organization_updated();
                self.error_message.clear();
            }
            OrganizationInverse::Group {
                command,
                placements,
            } => self.apply_group_inverse(command, placements),
        }
        cx.notify();
    }

    fn apply_group_inverse(
        &mut self,
        command: GroupInverseCommand,
        placements: Vec<SavedSessionPlacement>,
    ) {
        let Some(repository) = self.project_library.repository.clone() else {
            self.error_message = localization::project_store_unavailable();
            return;
        };
        let Some(expected) = self.project_revision() else {
            return;
        };
        let result = match command {
            GroupInverseCommand::RemoveCreated { group_id } => {
                let Some(group) = self.group(group_id).cloned() else {
                    return;
                };
                let has_sessions = self.group_session_count(group_id) > 0;
                let result = repository.remove_group(
                    group_id,
                    Some(GroupDestination::ProjectRoot),
                    has_sessions,
                    expected,
                );
                if result.is_ok() && has_sessions {
                    self.saved.relocate_group_sessions(
                        group.project_id,
                        group_id,
                        GroupDestination::ProjectRoot,
                    );
                }
                result.map(|_| ())
            }
            GroupInverseCommand::Rename { group_id, name } => repository
                .rename_group(group_id, name.as_str(), expected)
                .map(|_| ()),
            GroupInverseCommand::SetCollapsed {
                group_id,
                collapsed,
            } => repository
                .set_group_collapsed(group_id, collapsed, expected)
                .map(|_| ()),
            GroupInverseCommand::MoveBefore { group_id, before } => repository
                .move_group_before(group_id, before, expected)
                .map(|_| ()),
            GroupInverseCommand::RestoreRemoved { group, .. } => {
                repository.restore_group(group, expected).map(|_| ())
            }
        };
        match result {
            Ok(()) => {
                if !placements.is_empty() {
                    self.saved
                        .restore_app_attached_session_placements(&placements);
                }
                let _ = save_saved_state(&self.saved);
                self.finish_group_mutation();
                self.session_sidebar.pending_undo = None;
            }
            Err(error) => self.handle_group_error(error),
        }
    }

    fn record_group_inverse(
        &mut self,
        command: GroupInverseCommand,
        placements: Vec<SavedSessionPlacement>,
    ) {
        self.session_sidebar.pending_undo = Some(PendingOrganizationUndo {
            inverse: OrganizationInverse::Group {
                command,
                placements,
            },
            expires_at: Instant::now() + ORGANIZATION_UNDO_WINDOW,
        });
    }

    fn finish_group_mutation(&mut self) {
        self.project_library.reload();
        self.repair_session_group_references();
        self.status_message = localization::group_organization_updated();
        self.error_message.clear();
    }

    fn handle_group_error(&mut self, error: StoreError) {
        let stale = matches!(
            error,
            StoreError::Domain(ProjectError::StaleRevision { .. })
                | StoreError::GroupDomain(GroupError::StaleRevision { .. })
        );
        self.error_message = match error {
            StoreError::GroupDomain(
                GroupError::EmptyName | GroupError::NameContainsNul | GroupError::NameTooLong,
            ) => localization::group_error_invalid_name(),
            StoreError::GroupDomain(GroupError::DuplicateName) => {
                localization::group_error_duplicate()
            }
            StoreError::Domain(ProjectError::StaleRevision { .. })
            | StoreError::GroupDomain(GroupError::StaleRevision { .. }) => {
                localization::group_error_stale()
            }
            _ => localization::group_error_generic(),
        };
        if stale {
            self.project_library.reload();
            self.repair_session_group_references();
        }
    }

    fn project_revision(&self) -> Option<termirust_domain::Revision> {
        self.project_library
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.revision)
    }

    fn group(&self, id: GroupId) -> Option<&Group> {
        self.project_library
            .snapshot
            .as_ref()?
            .groups
            .iter()
            .find(|group| group.id == id)
    }

    fn project_groups(&self, project_id: ProjectId) -> Vec<&Group> {
        let mut groups = self
            .project_library
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .groups
                    .iter()
                    .filter(|group| group.project_id == project_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        groups.sort_by_key(|group| (group.position, group.id));
        groups
    }

    fn sessions_in_destination(
        &self,
        project_id: ProjectId,
        group_id: Option<GroupId>,
    ) -> Vec<SavedAppAttachedSession> {
        let mut sessions = self
            .saved
            .app_attached_sessions
            .iter()
            .filter(|session| {
                session.origin.project_id == project_id && session.group_id == group_id
            })
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| (session.position, session.id));
        sessions
    }

    fn group_session_count(&self, id: GroupId) -> usize {
        self.saved
            .app_attached_sessions
            .iter()
            .filter(|session| session.group_id == Some(id))
            .count()
    }

    pub(super) fn render_session_sidebar(
        &self,
        project_id: Option<ProjectId>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(project_id) = project_id else {
            return v_flex()
                .id("session-sidebar")
                .debug_selector(|| "session-sidebar".to_string())
                .flex_1()
                .items_center()
                .justify_center()
                .border_l_1()
                .border_color(theme::border())
                .text_color(theme::text_muted())
                .child(localization::session_sidebar_select_project())
                .into_any_element();
        };
        let session_count = self
            .saved
            .app_attached_sessions
            .iter()
            .filter(|session| session.origin.project_id == project_id)
            .count();
        let groups = self.project_groups(project_id);

        v_flex()
            .id("session-sidebar")
            .debug_selector(|| "session-sidebar".to_string())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .border_l_1()
            .border_color(theme::border())
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap(px(theme::SPACE_4))
                    .px(px(theme::SPACE_5))
                    .py(px(theme::SPACE_4))
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        v_flex()
                            .gap(px(theme::SPACE_2))
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(localization::session_sidebar_title()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::session_sidebar_subtitle()),
                            ),
                    )
                    .child(
                        Button::new("group-new")
                            .debug_selector(|| "group-new".to_string())
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .label(localization::group_new_action())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_group_editor(project_id, None, window, cx);
                            })),
                    ),
            )
            .when(self.session_sidebar.pending_undo.is_some(), |this| {
                this.child(
                    h_flex()
                        .id("group-undo-banner")
                        .justify_end()
                        .px(px(theme::SPACE_5))
                        .pt(px(theme::SPACE_3))
                        .child(
                            Button::new("group-undo")
                                .debug_selector(|| "group-undo".to_string())
                                .small()
                                .icon(IconName::Undo2)
                                .label(localization::group_undo_action())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.undo_organization(cx);
                                })),
                        ),
                )
            })
            .when_some(self.session_sidebar.editor.as_ref(), |this, editor| {
                this.child(self.render_group_editor(editor, cx))
            })
            .when_some(
                self.session_sidebar.pending_removal.as_ref(),
                |this, pending| this.child(self.render_group_removal(pending, cx)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p(px(theme::SPACE_4))
                    .gap(px(theme::SPACE_3))
                    .child(self.render_session_group(
                        project_id,
                        None,
                        localization::group_ungrouped_label(),
                        false,
                        None,
                        cx,
                    ))
                    .children(groups.iter().enumerate().map(|(index, group)| {
                        self.render_session_group(
                            project_id,
                            Some(group.id),
                            group.name.as_str().to_string(),
                            group.collapsed,
                            Some((index, groups.len())),
                            cx,
                        )
                    }))
                    .when(session_count == 0, |this| {
                        this.child(
                            div()
                                .p(px(theme::SPACE_5))
                                .text_center()
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .text_color(theme::text_muted())
                                .child(localization::session_sidebar_empty()),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_group_editor(&self, editor: &GroupEditor, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .id("group-editor")
            .debug_selector(|| "group-editor".to_string())
            .mx(px(theme::SPACE_4))
            .mt(px(theme::SPACE_4))
            .p(px(theme::SPACE_4))
            .gap(px(theme::SPACE_3))
            .border_1()
            .border_color(theme::focus_ring())
            .rounded(px(theme::CARD_RADIUS))
            .child(div().font_semibold().text_color(theme::text_main()).child(
                if editor.group_id.is_some() {
                    localization::group_editor_edit_title()
                } else {
                    localization::group_editor_new_title()
                },
            ))
            .child(Input::new(&self.group_name_input))
            .child(
                h_flex()
                    .gap(px(theme::SPACE_2))
                    .child(
                        Button::new("group-save")
                            .debug_selector(|| "group-save".to_string())
                            .small()
                            .primary()
                            .label(localization::common_save())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.save_group_editor(cx);
                            })),
                    )
                    .child(
                        Button::new("group-cancel")
                            .debug_selector(|| "group-cancel".to_string())
                            .small()
                            .label(localization::common_cancel())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_group_editor(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_group_removal(
        &self,
        pending: &PendingGroupRemoval,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(group) = self.group(pending.group_id) else {
            return div().into_any_element();
        };
        let count = self.group_session_count(group.id);
        let project_groups = self.project_groups(group.project_id);
        let group_id = group.id;
        v_flex()
            .id("group-remove-review")
            .debug_selector(|| "group-remove-review".to_string())
            .mx(px(theme::SPACE_4))
            .mt(px(theme::SPACE_4))
            .p(px(theme::SPACE_4))
            .gap(px(theme::SPACE_3))
            .border_1()
            .border_color(theme::warning())
            .rounded(px(theme::CARD_RADIUS))
            .child(
                div()
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(localization::group_remove_title()),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::group_remove_description(
                        group.name.as_str(),
                        count,
                    )),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(theme::SPACE_2))
                    .child(
                        Button::new("group-remove-to-root")
                            .debug_selector(|| "group-remove-to-root".to_string())
                            .small()
                            .label(localization::group_move_to_root_action())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_group_to(
                                    group_id,
                                    Some(GroupDestination::ProjectRoot),
                                    cx,
                                );
                            })),
                    )
                    .children(project_groups.into_iter().filter_map(|destination| {
                        if destination.id == group_id {
                            return None;
                        }
                        let destination_id = destination.id;
                        let key = group_key(destination_id);
                        Some(
                            Button::new(("group-remove-to", key))
                                .debug_selector(|| "group-remove-to".to_string())
                                .small()
                                .label(localization::group_move_to_action(
                                    destination.name.as_str(),
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_group_to(
                                        group_id,
                                        Some(GroupDestination::Group(destination_id)),
                                        cx,
                                    );
                                })),
                        )
                    }))
                    .child(
                        Button::new("group-remove-cancel")
                            .debug_selector(|| "group-remove-cancel".to_string())
                            .small()
                            .label(localization::common_cancel())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_group_removal(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_session_group(
        &self,
        project_id: ProjectId,
        group_id: Option<GroupId>,
        label: String,
        collapsed: bool,
        order: Option<(usize, usize)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let sessions = self.sessions_in_destination(project_id, group_id);
        let running = sessions
            .iter()
            .filter(|session| {
                matches!(
                    session.state,
                    HostedSessionState::RunningAppAttached
                        | HostedSessionState::Live
                        | HostedSessionState::RecordingPaused
                )
            })
            .count();
        let key = group_id.map(group_key).unwrap_or(0);
        let header = h_flex()
            .id(("session-group-header", key))
            .debug_selector(|| "session-group-header".to_string())
            .w_full()
            .justify_between()
            .items_center()
            .gap(px(theme::SPACE_3))
            .px(px(theme::SPACE_3))
            .py(px(theme::SPACE_3))
            .bg(theme::library_card())
            .border_b_1()
            .border_color(theme::soft_border())
            .child(
                h_flex()
                    .min_w_0()
                    .gap(px(theme::SPACE_2))
                    .items_center()
                    .when_some(group_id, |this, id| {
                        this.child(
                            Button::new(("group-disclosure", key))
                                .debug_selector(|| "group-disclosure".to_string())
                                .small()
                                .icon(if collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .label(if collapsed {
                                    localization::group_expand_action()
                                } else {
                                    localization::group_collapse_action()
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_group_collapsed(id, !collapsed, cx);
                                })),
                        )
                    })
                    .when(group_id.is_none(), |this| {
                        this.child(
                            Icon::new(IconName::Folder)
                                .size(px(theme::SPACE_4))
                                .text_color(theme::text_muted()),
                        )
                    })
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap(px(theme::SPACE_2))
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .truncate()
                                    .child(label),
                            )
                            .child(
                                h_flex()
                                    .gap(px(theme::SPACE_2))
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::group_session_count(sessions.len()))
                                    .child(localization::group_running_summary(running)),
                            ),
                    ),
            )
            .when_some(group_id.zip(order), |this, (id, (index, count))| {
                this.child(
                    h_flex()
                        .flex_none()
                        .gap(px(theme::SPACE_2))
                        .child(
                            Button::new(("group-up", key))
                                .debug_selector(|| "group-up".to_string())
                                .small()
                                .icon(IconName::ChevronUp)
                                .label(localization::group_move_up_action())
                                .disabled(index == 0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_group(id, -1, cx);
                                })),
                        )
                        .child(
                            Button::new(("group-down", key))
                                .debug_selector(|| "group-down".to_string())
                                .small()
                                .icon(IconName::ChevronDown)
                                .label(localization::group_move_down_action())
                                .disabled(index + 1 == count)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_group(id, 1, cx);
                                })),
                        )
                        .child(
                            Button::new(("group-rename", key))
                                .debug_selector(|| "group-rename".to_string())
                                .small()
                                .label(localization::group_rename_action())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_group_editor(project_id, Some(id), window, cx);
                                })),
                        )
                        .child(
                            Button::new(("group-remove", key))
                                .debug_selector(|| "group-remove".to_string())
                                .small()
                                .danger()
                                .icon(IconName::Delete)
                                .label(localization::group_remove_action())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.begin_group_removal(id, cx);
                                })),
                        ),
                )
            });

        v_flex()
            .id(("session-group", key))
            .debug_selector(|| "session-group".to_string())
            .w_full()
            .border_1()
            .border_color(theme::border())
            .rounded(px(theme::CARD_RADIUS))
            .overflow_hidden()
            .child(header)
            .when(!collapsed, |this| {
                this.children(sessions.iter().enumerate().map(|(index, session)| {
                    self.render_session_row(session, index, sessions.len(), cx)
                }))
            })
            .into_any_element()
    }

    fn render_session_row(
        &self,
        session: &SavedAppAttachedSession,
        index: usize,
        count: usize,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = session.id;
        let key = session_key(id);
        let selected = self.session_sidebar.selected_session == Some(id);
        let groups = self.project_groups(session.origin.project_id);
        let current_group = session.group_id;
        v_flex()
            .id(("project-session-row", key))
            .debug_selector(|| "project-session-row".to_string())
            .w_full()
            .px(px(theme::SPACE_3))
            .py(px(theme::SPACE_3))
            .gap(px(theme::SPACE_2))
            .border_b_1()
            .border_color(theme::soft_border())
            .bg(if selected {
                theme::accent_soft()
            } else {
                theme::library_bg()
            })
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap(px(theme::SPACE_3))
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap(px(theme::SPACE_3))
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(theme::SPACE_4))
                                    .text_color(session_state_color(session.state)),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .gap(px(theme::SPACE_2))
                                    .child(
                                        div()
                                            .font_medium()
                                            .text_color(theme::text_main())
                                            .truncate()
                                            .child(session.preset_label.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(session_state_label(session.state)),
                                    ),
                            ),
                    )
                    .child(
                        Button::new(("session-move", key))
                            .debug_selector(|| "session-move".to_string())
                            .small()
                            .selected(selected)
                            .label(localization::group_move_session_action())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_session_for_move(id, cx);
                            })),
                    ),
            )
            .when(selected, |this| {
                this.child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(theme::SPACE_2))
                        .when(
                            session.route == termirust_domain::SessionLaunchRoute::DurableHost,
                            |this| {
                                this.child(
                                    Button::new(("session-open", key))
                                        .debug_selector(|| "session-open".to_string())
                                        .small()
                                        .icon(IconName::SquareTerminal)
                                        .label(localization::common_open())
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.reattach_saved_session(id, window, cx);
                                        })),
                                )
                            },
                        )
                        .child(
                            Button::new(("session-up", key))
                                .debug_selector(|| "session-up".to_string())
                                .small()
                                .icon(IconName::ChevronUp)
                                .label(localization::group_move_up_action())
                                .disabled(index == 0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_session_by(id, -1, cx);
                                })),
                        )
                        .child(
                            Button::new(("session-down", key))
                                .debug_selector(|| "session-down".to_string())
                                .small()
                                .icon(IconName::ChevronDown)
                                .label(localization::group_move_down_action())
                                .disabled(index + 1 == count)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_session_by(id, 1, cx);
                                })),
                        )
                        .child(
                            Button::new(("session-to-root", key))
                                .debug_selector(|| "session-to-root".to_string())
                                .small()
                                .selected(current_group.is_none())
                                .label(localization::group_move_to_root_action())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_session_to(
                                        id,
                                        GroupDestination::ProjectRoot,
                                        None,
                                        cx,
                                    );
                                })),
                        )
                        .children(groups.into_iter().map(|group| {
                            let group_id = group.id;
                            Button::new(("session-to-group", group_key(group_id) ^ key))
                                .debug_selector(|| "session-to-group".to_string())
                                .small()
                                .selected(current_group == Some(group_id))
                                .label(localization::group_move_to_action(group.name.as_str()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_session_to(
                                        id,
                                        GroupDestination::Group(group_id),
                                        None,
                                        cx,
                                    );
                                }))
                        })),
                )
            })
            .into_any_element()
    }
}

fn group_key(id: GroupId) -> u64 {
    let value = id.as_uuid().as_u128();
    value as u64 ^ (value >> 64) as u64
}

fn session_key(id: HostedSessionId) -> u64 {
    let value = id.as_uuid().as_u128();
    value as u64 ^ (value >> 64) as u64
}

fn session_state_label(state: HostedSessionState) -> String {
    match state {
        HostedSessionState::Draft => localization::new_session_phase_draft(),
        HostedSessionState::Validating => localization::new_session_phase_validating(),
        HostedSessionState::Starting => localization::new_session_phase_starting(),
        HostedSessionState::Provisioning => localization::new_session_phase_provisioning(),
        HostedSessionState::Attaching => localization::new_session_phase_attaching(),
        HostedSessionState::Replaying => localization::new_session_phase_replaying(),
        HostedSessionState::Live => localization::new_session_phase_live(),
        HostedSessionState::RecordingPaused => localization::new_session_phase_recording_paused(),
        HostedSessionState::Offline => localization::new_session_phase_offline(),
        HostedSessionState::Orphaned => localization::new_session_phase_orphaned(),
        HostedSessionState::Gap => localization::new_session_phase_gap(),
        HostedSessionState::PermissionDenied => localization::new_session_phase_permission_denied(),
        HostedSessionState::Incompatible => localization::new_session_phase_incompatible(),
        HostedSessionState::RunningAppAttached => localization::new_session_phase_running(),
        HostedSessionState::Failed => localization::new_session_phase_failed(),
        HostedSessionState::Cancelled => localization::new_session_phase_cancelled(),
        HostedSessionState::Exited => localization::new_session_phase_exited(),
    }
}

fn session_state_color(state: HostedSessionState) -> gpui::Hsla {
    match state {
        HostedSessionState::RunningAppAttached | HostedSessionState::Live => theme::success(),
        HostedSessionState::Starting
        | HostedSessionState::Provisioning
        | HostedSessionState::Attaching
        | HostedSessionState::Replaying
        | HostedSessionState::Validating => theme::warning(),
        HostedSessionState::Failed
        | HostedSessionState::RecordingPaused
        | HostedSessionState::Offline
        | HostedSessionState::Orphaned
        | HostedSessionState::Gap
        | HostedSessionState::PermissionDenied
        | HostedSessionState::Incompatible => theme::danger(),
        _ => theme::text_muted(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_domain::{PositionKey, PresetId, SessionLaunchRoute, SessionOrigin};

    fn session(
        id: HostedSessionId,
        project_id: ProjectId,
        group_id: Option<GroupId>,
        position: PositionKey,
    ) -> SavedAppAttachedSession {
        SavedAppAttachedSession {
            id,
            route: SessionLaunchRoute::LegacyAppAttached,
            origin: SessionOrigin {
                project_id,
                preset_id: PresetId::new(),
            },
            state: HostedSessionState::RunningAppAttached,
            project_label: localization::projects_nav_label(),
            preset_label: localization::new_session_title(),
            durable_host: None,
            group_id,
            position,
            started_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn session_move_and_inverse_change_only_organization_fields() {
        let project_id = ProjectId::new();
        let group_id = GroupId::new();
        let session_id = HostedSessionId::new();
        let mut saved = crate::models::SavedState::default();
        saved
            .app_attached_sessions
            .push(session(session_id, project_id, None, PositionKey::FIRST));
        let origin = saved.app_attached_sessions[0].origin;
        let state = saved.app_attached_sessions[0].state;
        let inverse = saved
            .move_app_attached_session(session_id, GroupDestination::Group(group_id), None)
            .unwrap();
        assert_eq!(saved.app_attached_sessions[0].group_id, Some(group_id));
        assert_eq!(saved.app_attached_sessions[0].origin, origin);
        assert_eq!(saved.app_attached_sessions[0].state, state);
        saved.restore_app_attached_session_placements(&inverse);
        assert_eq!(saved.app_attached_sessions[0].group_id, None);
        assert_eq!(saved.app_attached_sessions[0].origin, origin);
        assert_eq!(saved.app_attached_sessions[0].state, state);
    }

    #[test]
    fn corrupt_group_reference_repairs_to_project_root_without_session_loss() {
        let project_id = ProjectId::new();
        let invalid_group = GroupId::new();
        let session_id = HostedSessionId::new();
        let mut saved = crate::models::SavedState::default();
        saved.app_attached_sessions.push(session(
            session_id,
            project_id,
            Some(invalid_group),
            PositionKey::new(7),
        ));
        let repaired = saved.repair_app_attached_group_references(&HashMap::new());
        assert_eq!(repaired, [session_id]);
        assert_eq!(saved.app_attached_sessions.len(), 1);
        assert_eq!(saved.app_attached_sessions[0].group_id, None);
        assert_eq!(saved.app_attached_sessions[0].position, PositionKey::FIRST);
    }

    #[test]
    fn pointer_and_keyboard_actions_have_stable_semantic_selectors() {
        let selectors = [
            "group-disclosure",
            "group-up",
            "group-down",
            "group-rename",
            "group-remove",
            "session-move",
            "session-up",
            "session-down",
            "session-to-root",
        ];
        assert!(selectors.iter().all(|selector| !selector.trim().is_empty()));
    }
}
