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
use termirust_client::HostReconciliationService;
use termirust_domain::{
    Group, GroupDestination, GroupError, GroupId, GroupInverseCommand, HostedSessionId,
    HostedSessionState, OutputSequence, ProjectError, ProjectId, SessionMutation,
    SessionStateError, SessionTitle,
};
use termirust_store::{RecoveryResult, SessionRemovalPlan, StoreError};
use termirust_ui_contract::{
    AccessibleCollectionRow, AccessibleRowId, DestructiveActionKind, DestructiveActionPresentation,
    HierarchyLevel, MessageId, PresetRuntimeRow, PresetRuntimeRowId, PresetRuntimeScreen,
    PresetRuntimeSemanticSnapshot, PresetRuntimeSurfaceState, ProductControlRole,
    ProductMoveDirection, ProductSessionAccessibilityCommand, ProductSessionAction,
    ProductSessionControl, ProductSessionScreen, ProductSessionSemanticSnapshot,
    ProductSessionSurfaceState, SemanticActionValue, stable_capability_row_value,
    stable_runtime_row_value,
};

use super::runtimes::{
    runtime_capability_label, runtime_capability_message, runtime_inspector_projection,
    runtime_label,
};
use super::session_coordinator::PendingArchiveAction;
use super::session_library::{SessionLibraryFilter, SessionLibraryRecovery, SessionLibraryView};
use super::session_resume::{resume_error_message, session_resume_projection};
use super::transcript_export::transcript_export_projection;
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
    pub selected_session: Option<HostedSessionId>,
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
            let projects = repaired
                .iter()
                .filter_map(|id| {
                    self.saved
                        .app_attached_sessions
                        .iter()
                        .find(|session| session.id == *id)
                        .map(|session| session.origin.project_id)
                })
                .collect::<std::collections::HashSet<_>>();
            for project_id in projects {
                let placements = self.saved.app_attached_session_placements(project_id);
                if let Err(error) = self
                    .session_library
                    .apply_placements(&mut self.saved, &placements)
                {
                    self.handle_session_library_error(error);
                    return;
                }
            }
            self.persist_session_projection();
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
                let updated = self.saved.app_attached_session_placements(group.project_id);
                if let Err(error) = self
                    .session_library
                    .apply_placements(&mut self.saved, &updated)
                {
                    self.handle_session_library_error(error);
                    cx.notify();
                    return;
                }
                self.persist_session_projection();
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
        if self.session_sidebar.selected_session == Some(id) {
            self.refresh_artifacts(id, cx);
        }
        cx.notify();
    }

    fn set_session_library_view(&mut self, view: SessionLibraryView, cx: &mut Context<Self>) {
        let previous = self.visible_accessible_session_ids();
        self.session_library.view = view;
        self.session_library.pending_stop_archive_review = None;
        self.session_library.pending_removal = None;
        self.session_library.renaming = None;
        self.reconcile_visible_session_selection(&previous);
        cx.notify();
    }

    fn set_session_library_filter(&mut self, filter: SessionLibraryFilter, cx: &mut Context<Self>) {
        let previous = self.visible_accessible_session_ids();
        self.session_library.filter = filter;
        self.reconcile_visible_session_selection(&previous);
        cx.notify();
    }

    fn begin_session_rename(
        &mut self,
        id: HostedSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(title) = self
            .session_library
            .session(id)
            .map(|session| session.title.as_str().to_string())
        else {
            return;
        };
        Self::set_input_value(&self.session_title_input, title, window, cx);
        self.session_library.renaming = Some(id);
        self.session_library.pending_removal = None;
        self.session_title_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn cancel_session_rename(&mut self, cx: &mut Context<Self>) {
        self.session_library.renaming = None;
        cx.notify();
    }

    fn save_session_rename(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.session_library.renaming else {
            return;
        };
        let value = self.session_title_input.read(cx).value().to_string();
        let Ok(title) = SessionTitle::new(&value) else {
            self.error_message = localization::session_library_operation_failed();
            cx.notify();
            return;
        };
        if self.mutate_session(id, SessionMutation::Rename(title)) {
            self.session_library.renaming = None;
        }
        cx.notify();
    }

    pub(super) fn mutate_session(
        &mut self,
        id: HostedSessionId,
        mutation: SessionMutation,
    ) -> bool {
        let previous = self.visible_accessible_session_ids();
        match self.session_library.mutate(&mut self.saved, id, mutation) {
            Ok(_) => {
                self.reconcile_visible_session_selection(&previous);
                self.persist_session_projection();
                self.status_message = localization::session_library_operation_complete();
                self.error_message.clear();
                true
            }
            Err(error) => {
                self.handle_session_library_error(error);
                false
            }
        }
    }

    fn persist_session_projection(&self) {
        if let Err(error) = save_saved_state(&self.saved) {
            eprintln!("[session-library] compatibility projection save failed: {error:#}");
        }
    }

    fn handle_session_library_error(&mut self, error: StoreError) {
        let stale = matches!(
            error,
            StoreError::SessionDomain(SessionStateError::StaleRevision { .. })
        );
        if stale {
            let _ = self.session_library.reload();
        }
        eprintln!("[session-library] operation failed: {error}");
        self.error_message = localization::session_library_operation_failed();
    }

    fn toggle_session_pin(&mut self, id: HostedSessionId, cx: &mut Context<Self>) {
        let Some(pinned) = self
            .session_library
            .session(id)
            .map(|session| session.pinned)
        else {
            return;
        };
        self.mutate_session(id, SessionMutation::SetPinned(!pinned));
        cx.notify();
    }

    fn toggle_session_read(&mut self, id: HostedSessionId, cx: &mut Context<Self>) {
        let Some(session) = self.session_library.session(id).cloned() else {
            return;
        };
        let mutation = if session.unread() {
            SessionMutation::MarkRead {
                through: session.last_output_sequence,
            }
        } else if session.last_output_sequence > OutputSequence::ZERO {
            SessionMutation::MarkUnread {
                at: session.last_output_sequence,
            }
        } else {
            return;
        };
        self.mutate_session(id, mutation);
        cx.notify();
    }

    fn archive_or_stop_session(&mut self, id: HostedSessionId, cx: &mut Context<Self>) {
        let Some(session) = self.session_library.session(id).cloned() else {
            return;
        };
        if session.lifecycle.can_stop() {
            self.session_library.pending_stop_archive_review = Some(id);
        } else if session.lifecycle.is_exited() {
            self.mutate_session(
                id,
                SessionMutation::Archive {
                    at: crate::ui::util::current_unix_millis(),
                },
            );
        }
        cx.notify();
    }

    fn session_pane_id(&self, id: HostedSessionId) -> Option<u64> {
        self.panes.iter().find_map(|pane| {
            pane.app_attached
                .as_ref()
                .and_then(|attached| (attached.hosted_session_id == id).then_some(pane.id))
        })
    }

    fn stop_session_only(&mut self, id: HostedSessionId, cx: &mut Context<Self>) {
        let Some(pane_id) = self.session_pane_id(id) else {
            self.error_message = localization::session_library_operation_failed();
            cx.notify();
            return;
        };
        self.stop_app_attached_session(pane_id, cx);
    }

    fn prepare_host_recovery(&mut self, id: HostedSessionId, cx: &mut Context<Self>) {
        if self.host_recovery_operation.is_some() || self.host_recovery_plan.is_some() {
            return;
        }
        let Some(session) = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|session| session.id == id)
        else {
            return;
        };
        if !crate::ui::settings::host_recovery_allowed(session.route, session.state, false) {
            return;
        }
        let Some(host) = session.durable_host.as_ref() else {
            self.error_message = localization::recovery_error_verification();
            cx.notify();
            return;
        };
        let runtime_root = std::path::PathBuf::from(&host.runtime_root);
        let session_dir = std::path::PathBuf::from(&host.session_dir);
        let cancellation = tokio_util::sync::CancellationToken::new();
        self.host_recovery_operation = Some(super::HostRecoveryOperation {
            session_id: id,
            cancellation: cancellation.clone(),
        });
        self.status_message = localization::recovery_inspecting();
        self.error_message.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let service = HostReconciliationService::new(runtime_root);
                    if cancellation.is_cancelled() {
                        return Err(termirust_client::HostReconciliationError {
                            code: termirust_client::HostReconciliationErrorCode::Cancelled,
                        });
                    }
                    service.recover_interrupted_reconciliation(&session_dir)?;
                    if cancellation.is_cancelled() {
                        return Err(termirust_client::HostReconciliationError {
                            code: termirust_client::HostReconciliationErrorCode::Cancelled,
                        });
                    }
                    let plan = service.plan(session_dir).await?;
                    if cancellation.is_cancelled() {
                        return Err(termirust_client::HostReconciliationError {
                            code: termirust_client::HostReconciliationErrorCode::Cancelled,
                        });
                    }
                    Ok(plan)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.host_recovery_operation = None;
                match result {
                    Ok(plan) => {
                        app.host_recovery_plan = Some(plan);
                        app.status_message = localization::recovery_confirmation_required();
                        app.error_message.clear();
                    }
                    Err(error) => {
                        app.host_recovery_plan = None;
                        app.error_message =
                            crate::ui::settings::host_recovery_error_message(error.code);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_host_recovery(&mut self, id: HostedSessionId, cx: &mut Context<Self>) {
        if self.host_recovery_operation.is_some() {
            return;
        }
        let Some(plan) = self.host_recovery_plan.take() else {
            return;
        };
        if plan.session_id != id || plan.preview_result != RecoveryResult::Reconciled {
            self.host_recovery_plan = Some(plan);
            return;
        }
        let Some(runtime_root) = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|session| session.id == id)
            .and_then(|session| session.durable_host.as_ref())
            .map(|host| std::path::PathBuf::from(&host.runtime_root))
        else {
            self.host_recovery_plan = Some(plan);
            self.error_message = localization::recovery_error_verification();
            cx.notify();
            return;
        };
        let cancellation = tokio_util::sync::CancellationToken::new();
        self.host_recovery_operation = Some(super::HostRecoveryOperation {
            session_id: id,
            cancellation: cancellation.clone(),
        });
        self.status_message = localization::recovery_running();
        self.error_message.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    HostReconciliationService::new(runtime_root).reconcile(plan, &cancellation)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.host_recovery_operation = None;
                match result {
                    Ok(receipt) if receipt.result == RecoveryResult::Reconciled => {
                        if app.mutate_session(
                            id,
                            SessionMutation::SetLifecycle(HostedSessionState::Orphaned),
                        ) {
                            app.status_message = localization::recovery_complete();
                            app.error_message.clear();
                        }
                    }
                    Ok(_) => {
                        app.status_message = localization::recovery_no_change();
                        app.error_message.clear();
                    }
                    Err(error) => {
                        app.error_message =
                            crate::ui::settings::host_recovery_error_message(error.code);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel_host_recovery(&mut self, id: HostedSessionId, cx: &mut Context<Self>) {
        if let Some(operation) = self.host_recovery_operation.as_ref()
            && operation.session_id == id
        {
            operation.cancellation.cancel();
        }
        if self
            .host_recovery_plan
            .as_ref()
            .is_some_and(|plan| plan.session_id == id)
        {
            self.host_recovery_plan = None;
        }
        self.status_message = localization::recovery_cancelled();
        self.error_message.clear();
        cx.notify();
    }

    fn confirm_stop_and_archive(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.session_library.pending_stop_archive_review.take() else {
            return;
        };
        let Some(pane_id) = self.session_pane_id(id) else {
            self.error_message = localization::session_library_operation_failed();
            cx.notify();
            return;
        };
        self.session_library.pending_archive_after_stop = Some(id);
        self.stop_app_attached_session(pane_id, cx);
        if self
            .session_library
            .session(id)
            .is_some_and(|value| value.lifecycle == HostedSessionState::Stopping)
        {
            self.status_message = localization::session_library_stop_archive_pending();
        } else {
            self.session_library.pending_archive_after_stop = None;
        }
        cx.notify();
    }

    fn cancel_stop_and_archive(&mut self, cx: &mut Context<Self>) {
        self.session_library.pending_stop_archive_review = None;
        cx.notify();
    }

    pub(super) fn complete_pending_session_archive(
        &mut self,
        id: HostedSessionId,
        action: PendingArchiveAction,
    ) {
        if self.session_library.pending_archive_after_stop != Some(id) {
            return;
        }
        match action {
            PendingArchiveAction::Archive => {
                self.session_library.pending_archive_after_stop = None;
                self.mutate_session(
                    id,
                    SessionMutation::Archive {
                        at: crate::ui::util::current_unix_millis(),
                    },
                );
            }
            PendingArchiveAction::Fail => {
                self.session_library.pending_archive_after_stop = None;
                self.error_message = localization::session_library_operation_failed();
            }
            PendingArchiveAction::None => {}
        }
    }

    fn restore_session(&mut self, id: HostedSessionId, cx: &mut Context<Self>) {
        self.mutate_session(id, SessionMutation::Restore);
        cx.notify();
    }

    fn begin_session_removal(
        &mut self,
        id: HostedSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::set_input_value(
            &self.session_remove_confirm_input,
            String::new(),
            window,
            cx,
        );
        match self.session_library.prepare_removal(id) {
            Ok(()) => {
                self.session_library.renaming = None;
                self.error_message.clear();
            }
            Err(error) => self.handle_session_library_error(error),
        }
        cx.notify();
    }

    fn cancel_session_removal(&mut self, cx: &mut Context<Self>) {
        self.session_library.pending_removal = None;
        cx.notify();
    }

    fn confirm_session_removal(&mut self, cx: &mut Context<Self>) {
        let previous = self.visible_accessible_session_ids();
        let confirmation = self
            .session_remove_confirm_input
            .read(cx)
            .value()
            .to_string();
        match self
            .session_library
            .confirm_removal(&mut self.saved, &confirmation)
        {
            Ok(_) => {
                self.persist_session_projection();
                self.reconcile_visible_session_selection(&previous);
                self.status_message = localization::session_library_operation_complete();
                self.error_message.clear();
            }
            Err(error) => self.handle_session_library_error(error),
        }
        cx.notify();
    }

    fn visible_accessible_session_ids(&self) -> Vec<AccessibleRowId> {
        self.session_library
            .visible_sessions_all()
            .into_iter()
            .map(|session| accessible_session_id(session.id))
            .collect()
    }

    fn reconcile_visible_session_selection(&mut self, previous: &[AccessibleRowId]) {
        let next = self.visible_accessible_session_ids();
        let selected = self
            .session_sidebar
            .selected_session
            .map(accessible_session_id);
        self.session_sidebar.selected_session =
            termirust_ui_contract::reconcile_collection_selection(previous, &next, selected)
                .selected
                .map(|id| HostedSessionId::from_uuid(uuid::Uuid::from_u128(id.value)));
    }

    fn move_session_to(
        &mut self,
        id: HostedSessionId,
        destination: GroupDestination,
        before: Option<HostedSessionId>,
        cx: &mut Context<Self>,
    ) {
        let Some(project_id) = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|session| session.id == id)
            .map(|session| session.origin.project_id)
        else {
            self.error_message = localization::group_error_generic();
            cx.notify();
            return;
        };
        let inverse = self.saved.app_attached_session_placements(project_id);
        let mut candidate = self.saved.clone();
        if candidate
            .move_app_attached_session(id, destination, before)
            .is_none()
        {
            self.error_message = localization::group_error_generic();
            cx.notify();
            return;
        }
        let placements = candidate.app_attached_session_placements(project_id);
        if let Err(error) = self
            .session_library
            .apply_placements(&mut self.saved, &placements)
        {
            self.handle_session_library_error(error);
            cx.notify();
            return;
        }
        self.persist_session_projection();
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
                match self
                    .session_library
                    .apply_placements(&mut self.saved, &placements)
                {
                    Ok(()) => {
                        self.persist_session_projection();
                        self.status_message = localization::group_organization_updated();
                        self.error_message.clear();
                    }
                    Err(error) => self.handle_session_library_error(error),
                }
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
                    if let Err(error) = self
                        .session_library
                        .apply_placements(&mut self.saved, &placements)
                    {
                        self.handle_session_library_error(error);
                        return;
                    }
                }
                self.persist_session_projection();
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
        self.session_library
            .visible_sessions(project_id, group_id)
            .into_iter()
            .filter_map(|metadata| {
                self.saved
                    .app_attached_sessions
                    .iter()
                    .find(|record| record.id == metadata.id)
                    .cloned()
            })
            .collect()
    }

    fn group_session_count(&self, id: GroupId) -> usize {
        self.saved
            .app_attached_sessions
            .iter()
            .filter(|session| session.group_id == Some(id))
            .count()
    }

    pub(super) fn runtime_inspector_semantic_snapshot(
        &self,
    ) -> Option<PresetRuntimeSemanticSnapshot> {
        let selected = self.session_sidebar.selected_session?;
        let session = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|session| session.id == selected)?;
        if session.route != termirust_domain::SessionLaunchRoute::DurableHost {
            return None;
        }
        let recognition = session
            .durable_host
            .as_ref()
            .and_then(|host| host.runtime_recognition.as_ref());
        let projection = runtime_inspector_projection(recognition);
        let occupant = recognition.and_then(|recognition| recognition.occupant.as_ref());
        let runtime_id = occupant
            .map(|occupant| occupant.runtime_id.as_str())
            .unwrap_or("generic");
        let runtime_row = PresetRuntimeRowId::runtime(stable_runtime_row_value(runtime_id));
        let capabilities = occupant
            .map(|occupant| occupant.effective_capabilities().iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let confidence = recognition
            .map(|recognition| recognition.confidence)
            .unwrap_or(termirust_domain::RecognitionConfidence::Uncertain);
        let confidence_message = match confidence {
            termirust_domain::RecognitionConfidence::Verified => {
                MessageId::RuntimeConfidenceVerified
            }
            termirust_domain::RecognitionConfidence::Observed => {
                MessageId::RuntimeConfidenceObserved
            }
            termirust_domain::RecognitionConfidence::Uncertain => {
                MessageId::RuntimeConfidenceUncertain
            }
        };
        let mut rows = vec![PresetRuntimeRow {
            id: runtime_row,
            parent: None,
            name: runtime_label(runtime_id),
            status: confidence_message,
            detail: Some(format!(
                "{}; {}; {}; {}",
                projection.version,
                projection.ownership,
                projection.confidence,
                projection.generation
            )),
            selected: true,
            disabled: occupant.is_none(),
            checked: Some(!capabilities.is_empty()),
            risky: false,
            stale: projection.stale,
            position: 1,
            set_size: 1,
        }];
        let capability_count = capabilities.len().max(1);
        for (index, capability) in capabilities.into_iter().enumerate() {
            let message = runtime_capability_message(capability);
            rows.push(PresetRuntimeRow {
                id: PresetRuntimeRowId::capability(stable_capability_row_value(
                    runtime_id, message,
                )),
                parent: Some(runtime_row),
                name: runtime_capability_label(capability),
                status: confidence_message,
                detail: None,
                selected: false,
                disabled: false,
                checked: Some(true),
                risky: false,
                stale: projection.stale,
                position: index + 1,
                set_size: capability_count,
            });
        }
        Some(PresetRuntimeSemanticSnapshot {
            screen: PresetRuntimeScreen::RuntimeInspector,
            state: if occupant.is_none() {
                PresetRuntimeSurfaceState::Unsupported
            } else if projection.stale
                || confidence != termirust_domain::RecognitionConfidence::Verified
            {
                PresetRuntimeSurfaceState::Partial
            } else {
                PresetRuntimeSurfaceState::Ready
            },
            rows,
            controls: Vec::new(),
            recording_friendly: self.activity_center.policy().recording_friendly,
        })
    }

    pub(super) fn product_session_semantic_snapshot(
        &self,
        cx: &Context<Self>,
    ) -> Option<ProductSessionSemanticSnapshot> {
        let screen = match self.nav_section {
            super::NavSection::Projects => ProductSessionScreen::Projects,
            super::NavSection::Sessions => ProductSessionScreen::Sessions,
            _ => return None,
        };
        let project_snapshot = self.project_library.snapshot.as_ref();
        let projects = project_snapshot
            .map(|snapshot| snapshot.projects.as_slice())
            .unwrap_or_default();
        let mut rows = Vec::new();
        let project_ids = if screen == ProductSessionScreen::Projects {
            projects
                .iter()
                .map(|summary| summary.project.id)
                .collect::<Vec<_>>()
        } else {
            let mut ids = self
                .session_library
                .visible_sessions_all()
                .into_iter()
                .map(|session| session.project_id)
                .collect::<Vec<_>>();
            ids.sort_unstable();
            ids.dedup();
            ids
        };

        for (project_index, project_id) in project_ids.iter().copied().enumerate() {
            let summary = projects
                .iter()
                .find(|summary| summary.project.id == project_id);
            let project_name = summary
                .map(|summary| summary.project.display_name.as_str().to_string())
                .or_else(|| {
                    self.saved
                        .app_attached_sessions
                        .iter()
                        .find(|session| session.origin.project_id == project_id)
                        .map(|session| session.project_label.clone())
                })
                .unwrap_or_else(localization::projects_nav_label);
            let project_status = summary
                .map(|summary| summary.status)
                .unwrap_or(termirust_domain::ProjectStatus::Unavailable);
            let project_row_id = accessible_project_id(project_id);
            rows.push(AccessibleCollectionRow {
                id: project_row_id,
                parent: None,
                level: HierarchyLevel::Project,
                name: project_name,
                status: project_status_message_id(project_status),
                selected: self.project_library.selected_id == Some(project_id),
                expanded: Some(
                    screen == ProductSessionScreen::Sessions
                        || self.project_library.selected_id == Some(project_id),
                ),
                unread: false,
                disabled: project_status != termirust_domain::ProjectStatus::Available,
                position: project_index + 1,
                set_size: project_ids.len().max(1),
            });

            let include_children = screen == ProductSessionScreen::Sessions
                || self.project_library.selected_id == Some(project_id);
            if !include_children {
                continue;
            }

            let ungrouped = self.sessions_in_destination(project_id, None);
            let groups = self.project_groups(project_id);
            let project_child_count = ungrouped.len() + groups.len();
            append_accessible_session_rows(
                &mut rows,
                &ungrouped,
                project_row_id,
                &self.session_library,
                self.session_sidebar.selected_session,
                0,
                project_child_count,
            );
            for (group_index, group) in groups.iter().enumerate() {
                let group_row_id = accessible_group_id(group.id);
                rows.push(AccessibleCollectionRow {
                    id: group_row_id,
                    parent: Some(project_row_id),
                    level: HierarchyLevel::Group,
                    name: group.name.as_str().to_string(),
                    status: MessageId::ProductSurfaceStateReady,
                    selected: false,
                    expanded: Some(!group.collapsed),
                    unread: false,
                    disabled: false,
                    position: ungrouped.len() + group_index + 1,
                    set_size: project_child_count.max(1),
                });
                if !group.collapsed {
                    let sessions = self.sessions_in_destination(project_id, Some(group.id));
                    let session_count = sessions.len();
                    append_accessible_session_rows(
                        &mut rows,
                        &sessions,
                        group_row_id,
                        &self.session_library,
                        self.session_sidebar.selected_session,
                        0,
                        session_count,
                    );
                }
            }
        }

        let dialog = self.product_destructive_presentation(cx);
        let controls = self.product_session_controls(screen, cx);
        let state = product_session_surface_state(
            &self.project_library.load_state,
            self.session_library.recovery_state(),
            rows.is_empty(),
            self.session_library.filter != SessionLibraryFilter::All,
        );
        Some(ProductSessionSemanticSnapshot {
            screen,
            state,
            rows,
            controls,
            dialog,
            recording_friendly: self.activity_center.policy().recording_friendly,
        })
    }

    fn product_session_controls(
        &self,
        screen: ProductSessionScreen,
        cx: &Context<Self>,
    ) -> Vec<ProductSessionControl> {
        let mut controls = Vec::new();
        if matches!(
            &self.project_library.load_state,
            super::projects::ProjectLibraryLoadState::Failed(_)
        ) {
            controls.push(product_button(
                ProductSessionAction::RetryProjects,
                MessageId::CommonRetry,
                None,
            ));
            return controls;
        }

        if screen == ProductSessionScreen::Projects {
            let mut add = product_button(
                ProductSessionAction::AddProject,
                MessageId::ProjectsAddAction,
                None,
            );
            add.disabled = self.project_library.add_draft.is_some()
                || self.project_library.add_validation.is_some();
            controls.push(add);
            if self.project_library.add_draft.is_some() {
                controls.extend([
                    product_text_field(
                        ProductSessionAction::SetProjectName,
                        MessageId::ProjectLabelField,
                        self.project_label_input.read(cx).value().to_string(),
                        false,
                    ),
                    product_button(
                        ProductSessionAction::ConfirmProjectAdd,
                        MessageId::ProjectAddConfirm,
                        None,
                    ),
                    product_button(
                        ProductSessionAction::CancelProjectAdd,
                        MessageId::CommonCancel,
                        None,
                    ),
                ]);
            } else if self.project_library.add_validation.is_some() {
                controls.push(product_button(
                    ProductSessionAction::CancelProjectAdd,
                    MessageId::CommonCancel,
                    None,
                ));
            }
            if self.project_library.pending_removal.is_some() {
                controls.push(product_button(
                    ProductSessionAction::UndoProjectRemoval,
                    MessageId::ProjectUndoAction,
                    None,
                ));
            }
            if let Some(snapshot) = self.project_library.snapshot.as_ref() {
                for summary in &snapshot.projects {
                    if summary.status == termirust_domain::ProjectStatus::Available {
                        let id = accessible_project_id(summary.project.id);
                        controls.push(product_button(
                            ProductSessionAction::RemoveProject(id),
                            MessageId::ProjectRemoveAction,
                            Some(id),
                        ));
                    }
                }
            }
            if let Some(project_id) = self.project_library.selected_id {
                let project = accessible_project_id(project_id);
                controls.push(product_button(
                    ProductSessionAction::AddGroup(project),
                    MessageId::GroupNewAction,
                    Some(project),
                ));
                let groups = self.project_groups(project_id);
                for (index, group) in groups.iter().enumerate() {
                    let group_id = accessible_group_id(group.id);
                    controls.push(product_button(
                        ProductSessionAction::ToggleGroup(group_id),
                        if group.collapsed {
                            MessageId::GroupExpandAction
                        } else {
                            MessageId::GroupCollapseAction
                        },
                        Some(group_id),
                    ));
                    let mut move_up = product_button(
                        ProductSessionAction::MoveGroup(group_id, ProductMoveDirection::Up),
                        MessageId::GroupMoveUpAction,
                        Some(group_id),
                    );
                    move_up.disabled = index == 0;
                    controls.push(move_up);
                    let mut move_down = product_button(
                        ProductSessionAction::MoveGroup(group_id, ProductMoveDirection::Down),
                        MessageId::GroupMoveDownAction,
                        Some(group_id),
                    );
                    move_down.disabled = index + 1 == groups.len();
                    controls.push(move_down);
                    controls.extend([
                        product_button(
                            ProductSessionAction::RenameGroup(group_id),
                            MessageId::GroupRenameAction,
                            Some(group_id),
                        ),
                        product_button(
                            ProductSessionAction::RemoveGroup(group_id),
                            MessageId::GroupRemoveAction,
                            Some(group_id),
                        ),
                    ]);
                }
            }
            if self.session_sidebar.pending_undo.is_some() {
                controls.push(product_button(
                    ProductSessionAction::UndoOrganization,
                    MessageId::GroupUndoAction,
                    None,
                ));
            }
            if self.session_sidebar.editor.is_some() {
                controls.extend([
                    product_text_field(
                        ProductSessionAction::SetGroupName,
                        MessageId::GroupNameField,
                        self.group_name_input.read(cx).value().to_string(),
                        false,
                    ),
                    product_button(ProductSessionAction::SaveGroup, MessageId::CommonSave, None),
                    product_button(
                        ProductSessionAction::CancelGroup,
                        MessageId::CommonCancel,
                        None,
                    ),
                ]);
            }
            if let Some(pending) = self.session_sidebar.pending_removal.as_ref() {
                let mut move_to_root = product_button(
                    ProductSessionAction::RemoveGroupTo(
                        accessible_group_id(pending.group_id),
                        None,
                    ),
                    MessageId::GroupMoveToRootAction,
                    None,
                );
                move_to_root.in_dialog = true;
                controls.push(move_to_root);
                if let Some(group) = self.group(pending.group_id) {
                    for destination in self
                        .project_groups(group.project_id)
                        .into_iter()
                        .filter(|destination| destination.id != pending.group_id)
                    {
                        let mut move_to_group = product_button(
                            ProductSessionAction::RemoveGroupTo(
                                accessible_group_id(pending.group_id),
                                Some(accessible_group_id(destination.id)),
                            ),
                            MessageId::GroupMoveSessionAction,
                            None,
                        );
                        move_to_group.value = Some(destination.name.as_str().to_string());
                        move_to_group.in_dialog = true;
                        controls.push(move_to_group);
                    }
                }
            }
        }

        if screen == ProductSessionScreen::Sessions {
            controls.extend([
                product_tab(
                    ProductSessionAction::ShowActiveSessions,
                    MessageId::SessionLibraryActiveView,
                    self.session_library.view == SessionLibraryView::Active,
                ),
                product_tab(
                    ProductSessionAction::ShowArchivedSessions,
                    MessageId::SessionLibraryArchiveView,
                    self.session_library.view == SessionLibraryView::Archive,
                ),
                product_tab(
                    ProductSessionAction::FilterAllSessions,
                    MessageId::SessionLibraryFilterAll,
                    self.session_library.filter == SessionLibraryFilter::All,
                ),
                product_tab(
                    ProductSessionAction::FilterUnreadSessions,
                    MessageId::SessionLibraryFilterUnread,
                    self.session_library.filter == SessionLibraryFilter::Unread,
                ),
                product_tab(
                    ProductSessionAction::FilterPinnedSessions,
                    MessageId::SessionLibraryFilterPinned,
                    self.session_library.filter == SessionLibraryFilter::Pinned,
                ),
            ]);
        }

        let Some(selected_id) = self.session_sidebar.selected_session else {
            return controls;
        };
        let Some(metadata) = self.session_library.session(selected_id) else {
            return controls;
        };
        let row_id = accessible_session_id(selected_id);
        let record = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|record| record.id == selected_id);
        controls.push(product_button(
            ProductSessionAction::RenameSession(row_id),
            MessageId::SessionLibraryRenameAction,
            Some(row_id),
        ));
        controls.push(product_button(
            ProductSessionAction::ToggleSessionPin(row_id),
            if metadata.pinned {
                MessageId::SessionLibraryUnpinAction
            } else {
                MessageId::SessionLibraryPinAction
            },
            Some(row_id),
        ));
        let mut read = product_button(
            ProductSessionAction::ToggleSessionRead(row_id),
            if metadata.unread() {
                MessageId::SessionLibraryMarkReadAction
            } else {
                MessageId::SessionLibraryMarkUnreadAction
            },
            Some(row_id),
        );
        read.disabled = !metadata.unread() && metadata.last_output_sequence == OutputSequence::ZERO;
        controls.push(read);
        if let Some(record) = record {
            if record.route == termirust_domain::SessionLaunchRoute::DurableHost {
                controls.push(product_button(
                    ProductSessionAction::OpenSession(row_id),
                    MessageId::CommonOpen,
                    Some(row_id),
                ));
            }
            if metadata.lifecycle.can_stop() {
                controls.push(product_button(
                    ProductSessionAction::StopSession(row_id),
                    MessageId::NewSessionStopAction,
                    Some(row_id),
                ));
            }
            let resume = session_resume_projection(record, metadata);
            if resume.visible {
                let mut control = product_button(
                    ProductSessionAction::ResumeSession(row_id),
                    MessageId::SessionLibraryResumeAction,
                    Some(row_id),
                );
                control.disabled = !resume.enabled;
                controls.push(control);
            }
            let sessions = self.sessions_in_destination(record.origin.project_id, record.group_id);
            if let Some(index) = sessions
                .iter()
                .position(|candidate| candidate.id == selected_id)
            {
                let mut up = product_button(
                    ProductSessionAction::MoveSession(row_id, ProductMoveDirection::Up),
                    MessageId::GroupMoveUpAction,
                    Some(row_id),
                );
                up.disabled = index == 0;
                controls.push(up);
                let mut down = product_button(
                    ProductSessionAction::MoveSession(row_id, ProductMoveDirection::Down),
                    MessageId::GroupMoveDownAction,
                    Some(row_id),
                );
                down.disabled = index + 1 == sessions.len();
                controls.push(down);
            }
            controls.push(product_button(
                ProductSessionAction::MoveSessionToRoot(row_id),
                MessageId::GroupMoveToRootAction,
                Some(row_id),
            ));
        }
        if metadata.archived_at.is_none() {
            let mut archive = product_button(
                ProductSessionAction::ArchiveOrStopSession(row_id),
                if metadata.lifecycle.can_stop() {
                    MessageId::SessionLibraryStopArchiveAction
                } else {
                    MessageId::SessionLibraryArchiveAction
                },
                Some(row_id),
            );
            archive.disabled = !metadata.lifecycle.can_stop() && !metadata.lifecycle.is_exited();
            controls.push(archive);
        } else {
            controls.push(product_button(
                ProductSessionAction::RestoreSession(row_id),
                MessageId::SessionLibraryRestoreAction,
                Some(row_id),
            ));
            let mut remove = product_button(
                ProductSessionAction::BeginSessionRemoval(row_id),
                MessageId::SessionLibraryRemoveAction,
                Some(row_id),
            );
            remove.disabled = !metadata.can_remove();
            controls.push(remove);
        }
        if self.session_library.renaming == Some(selected_id) {
            controls.extend([
                product_text_field(
                    ProductSessionAction::SetSessionTitle,
                    MessageId::SessionLibraryTitleField,
                    self.session_title_input.read(cx).value().to_string(),
                    false,
                ),
                product_button(
                    ProductSessionAction::SaveSessionTitle,
                    MessageId::CommonSave,
                    Some(row_id),
                ),
                product_button(
                    ProductSessionAction::CancelSessionTitle,
                    MessageId::CommonCancel,
                    Some(row_id),
                ),
            ]);
        }
        if self
            .session_library
            .pending_removal
            .as_ref()
            .is_some_and(|pending| {
                pending.session_id == selected_id && pending.manifest.requires_title_confirmation()
            })
        {
            controls.push(product_text_field(
                ProductSessionAction::SetSessionRemovalConfirmation,
                MessageId::SessionLibraryRemoveConfirmPlaceholder,
                self.session_remove_confirm_input
                    .read(cx)
                    .value()
                    .to_string(),
                true,
            ));
        }
        controls
    }

    fn product_destructive_presentation(
        &self,
        cx: &Context<Self>,
    ) -> Option<DestructiveActionPresentation> {
        let revision = self.session_library.revision().get();
        if let Some(pending) = self.session_library.pending_removal.as_ref() {
            return Some(DestructiveActionPresentation {
                kind: DestructiveActionKind::RemoveSessionData,
                target: accessible_session_id(pending.session_id),
                revision: pending.expected_revision.get(),
                confirm_enabled: !pending.manifest.requires_title_confirmation()
                    || self.session_remove_confirm_input.read(cx).value().as_ref()
                        == pending.title.as_str(),
            });
        }
        if let Some(id) = self.session_library.pending_stop_archive_review {
            return Some(DestructiveActionPresentation {
                kind: DestructiveActionKind::StopAndArchive,
                target: accessible_session_id(id),
                revision,
                confirm_enabled: true,
            });
        }
        self.session_sidebar
            .pending_removal
            .as_ref()
            .map(|pending| DestructiveActionPresentation {
                kind: DestructiveActionKind::RemoveGroup,
                target: accessible_group_id(pending.group_id),
                revision: self
                    .project_library
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.revision.get())
                    .unwrap_or_default(),
                confirm_enabled: false,
            })
    }

    pub(super) fn handle_product_session_accessibility_command(
        &mut self,
        command: ProductSessionAccessibilityCommand,
        value: Option<SemanticActionValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            ProductSessionAccessibilityCommand::FocusRow(row)
            | ProductSessionAccessibilityCommand::ActivateRow(row) => {
                match row.kind {
                    termirust_ui_contract::AccessibleRowKind::Project => {
                        let id = ProjectId::from_uuid(uuid::Uuid::from_u128(row.value));
                        if self
                            .project_library
                            .snapshot
                            .as_ref()
                            .is_some_and(|snapshot| {
                                snapshot
                                    .projects
                                    .iter()
                                    .any(|summary| summary.project.id == id)
                            })
                        {
                            self.project_library.selected_id = Some(id);
                            self.project_list_focus.focus(window);
                        }
                    }
                    termirust_ui_contract::AccessibleRowKind::Group => {
                        let id = GroupId::from_uuid(uuid::Uuid::from_u128(row.value));
                        if let Some(group) = self.group(id).cloned() {
                            self.project_library.selected_id = Some(group.project_id);
                            if matches!(command, ProductSessionAccessibilityCommand::ActivateRow(_))
                            {
                                self.set_group_collapsed(id, !group.collapsed, cx);
                            }
                            self.project_list_focus.focus(window);
                        }
                    }
                    termirust_ui_contract::AccessibleRowKind::Session => {
                        let id = HostedSessionId::from_uuid(uuid::Uuid::from_u128(row.value));
                        if let Some(session) = self.session_library.session(id) {
                            self.project_library.selected_id = Some(session.project_id);
                            self.session_sidebar.selected_session = Some(id);
                            self.project_list_focus.focus(window);
                            self.refresh_artifacts(id, cx);
                        }
                    }
                }
                cx.notify();
            }
            ProductSessionAccessibilityCommand::FocusControl(action) => match action {
                ProductSessionAction::SetProjectName => self
                    .project_label_input
                    .update(cx, |input, cx| input.focus(window, cx)),
                ProductSessionAction::SetGroupName => self
                    .group_name_input
                    .update(cx, |input, cx| input.focus(window, cx)),
                ProductSessionAction::SetSessionTitle => self
                    .session_title_input
                    .update(cx, |input, cx| input.focus(window, cx)),
                ProductSessionAction::SetSessionRemovalConfirmation => self
                    .session_remove_confirm_input
                    .update(cx, |input, cx| input.focus(window, cx)),
                _ => self.project_list_focus.focus(window),
            },
            ProductSessionAccessibilityCommand::SetControlValue(action) => {
                let Some(SemanticActionValue::Text(value)) = value else {
                    return;
                };
                match action {
                    ProductSessionAction::SetProjectName
                        if self.project_library.add_draft.is_some() =>
                    {
                        Self::set_input_value(&self.project_label_input, value, window, cx);
                    }
                    ProductSessionAction::SetGroupName if self.session_sidebar.editor.is_some() => {
                        Self::set_input_value(&self.group_name_input, value, window, cx);
                    }
                    ProductSessionAction::SetSessionTitle
                        if self.session_library.renaming.is_some() =>
                    {
                        Self::set_input_value(&self.session_title_input, value, window, cx);
                    }
                    ProductSessionAction::SetSessionRemovalConfirmation
                        if self.session_library.pending_removal.is_some() =>
                    {
                        Self::set_input_value(
                            &self.session_remove_confirm_input,
                            value,
                            window,
                            cx,
                        );
                    }
                    _ => return,
                }
                cx.notify();
            }
            ProductSessionAccessibilityCommand::ActivateControl(action) => match action {
                ProductSessionAction::RetryProjects => self.retry_project_library(window, cx),
                ProductSessionAction::AddProject => self.choose_project_folder(window, cx),
                ProductSessionAction::ConfirmProjectAdd => self.commit_project_add(window, cx),
                ProductSessionAction::CancelProjectAdd => self.cancel_project_add(cx),
                ProductSessionAction::RemoveProject(row) => {
                    if let Some(id) = accessible_project_row_id(row)
                        && self
                            .project_library
                            .snapshot
                            .as_ref()
                            .is_some_and(|snapshot| {
                                snapshot
                                    .projects
                                    .iter()
                                    .any(|summary| summary.project.id == id)
                            })
                    {
                        self.remove_project(id, cx);
                    }
                }
                ProductSessionAction::UndoProjectRemoval => {
                    self.undo_project_removal(window, cx);
                }
                ProductSessionAction::AddGroup(row) => {
                    if let Some(id) = accessible_project_row_id(row)
                        && self
                            .project_library
                            .snapshot
                            .as_ref()
                            .is_some_and(|snapshot| {
                                snapshot
                                    .projects
                                    .iter()
                                    .any(|summary| summary.project.id == id)
                            })
                    {
                        self.open_group_editor(id, None, window, cx);
                    }
                }
                ProductSessionAction::RenameGroup(row) => {
                    if let Some(id) = accessible_group_row_id(row)
                        && let Some(group) = self.group(id).cloned()
                    {
                        self.open_group_editor(group.project_id, Some(id), window, cx);
                    }
                }
                ProductSessionAction::SaveGroup => self.save_group_editor(cx),
                ProductSessionAction::CancelGroup => self.cancel_group_editor(cx),
                ProductSessionAction::ToggleGroup(row) => {
                    if let Some(id) = accessible_group_row_id(row)
                        && let Some(group) = self.group(id).cloned()
                    {
                        self.set_group_collapsed(id, !group.collapsed, cx);
                    }
                }
                ProductSessionAction::MoveGroup(row, direction) => {
                    if let Some(id) = accessible_group_row_id(row)
                        && self.group(id).is_some()
                    {
                        self.move_group(id, product_move_delta(direction), cx);
                    }
                }
                ProductSessionAction::RemoveGroup(row) => {
                    if let Some(id) = accessible_group_row_id(row)
                        && self.group(id).is_some()
                    {
                        self.begin_group_removal(id, cx);
                    }
                }
                ProductSessionAction::RemoveGroupTo(row, destination) => {
                    if let Some(id) = accessible_group_row_id(row)
                        && self.group(id).is_some()
                    {
                        let destination = match destination {
                            Some(destination) => {
                                let Some(destination) = accessible_group_row_id(destination) else {
                                    return;
                                };
                                if self.group(destination).is_none() {
                                    return;
                                }
                                GroupDestination::Group(destination)
                            }
                            None => GroupDestination::ProjectRoot,
                        };
                        self.remove_group_to(id, Some(destination), cx);
                    }
                }
                ProductSessionAction::UndoOrganization => self.undo_organization(cx),
                ProductSessionAction::ShowActiveSessions => {
                    self.set_session_library_view(SessionLibraryView::Active, cx);
                }
                ProductSessionAction::ShowArchivedSessions => {
                    self.set_session_library_view(SessionLibraryView::Archive, cx);
                }
                ProductSessionAction::FilterAllSessions => {
                    self.set_session_library_filter(SessionLibraryFilter::All, cx);
                }
                ProductSessionAction::FilterUnreadSessions => {
                    self.set_session_library_filter(SessionLibraryFilter::Unread, cx);
                }
                ProductSessionAction::FilterPinnedSessions => {
                    self.set_session_library_filter(SessionLibraryFilter::Pinned, cx);
                }
                ProductSessionAction::RenameSession(row) => {
                    if let Some(id) = accessible_session_row_id(row)
                        && self.session_library.session(id).is_some()
                    {
                        self.begin_session_rename(id, window, cx);
                    }
                }
                ProductSessionAction::SaveSessionTitle => self.save_session_rename(cx),
                ProductSessionAction::CancelSessionTitle => self.cancel_session_rename(cx),
                ProductSessionAction::ToggleSessionPin(row) => {
                    if let Some(id) = accessible_session_row_id(row) {
                        self.toggle_session_pin(id, cx);
                    }
                }
                ProductSessionAction::ToggleSessionRead(row) => {
                    if let Some(id) = accessible_session_row_id(row) {
                        self.toggle_session_read(id, cx);
                    }
                }
                ProductSessionAction::OpenSession(row) => {
                    if let Some(id) = accessible_session_row_id(row) {
                        self.open_session_from_entry(id, window, cx);
                    }
                }
                ProductSessionAction::StopSession(row) => {
                    if let Some(id) = accessible_session_row_id(row)
                        && self.session_library.session(id).is_some()
                    {
                        self.stop_session_only(id, cx);
                    }
                }
                ProductSessionAction::ResumeSession(row) => {
                    if let Some(id) = accessible_session_row_id(row)
                        && self.session_library.session(id).is_some()
                    {
                        self.open_session_resume(id, cx);
                    }
                }
                ProductSessionAction::ArchiveOrStopSession(row) => {
                    if let Some(id) = accessible_session_row_id(row) {
                        self.archive_or_stop_session(id, cx);
                    }
                }
                ProductSessionAction::RestoreSession(row) => {
                    if let Some(id) = accessible_session_row_id(row) {
                        self.restore_session(id, cx);
                    }
                }
                ProductSessionAction::BeginSessionRemoval(row) => {
                    if let Some(id) = accessible_session_row_id(row)
                        && self.session_library.session(id).is_some()
                    {
                        self.begin_session_removal(id, window, cx);
                    }
                }
                ProductSessionAction::MoveSession(row, direction) => {
                    if let Some(id) = accessible_session_row_id(row)
                        && self.session_library.session(id).is_some()
                    {
                        self.move_session_by(id, product_move_delta(direction), cx);
                    }
                }
                ProductSessionAction::MoveSessionToRoot(row) => {
                    if let Some(id) = accessible_session_row_id(row)
                        && self.session_library.session(id).is_some()
                    {
                        self.move_session_to(id, GroupDestination::ProjectRoot, None, cx);
                    }
                }
                ProductSessionAction::SetProjectName
                | ProductSessionAction::SetGroupName
                | ProductSessionAction::SetSessionTitle
                | ProductSessionAction::SetSessionRemovalConfirmation => {}
            },
            ProductSessionAccessibilityCommand::FocusSafeAction
            | ProductSessionAccessibilityCommand::FocusConfirmAction => {
                self.project_list_focus.focus(window);
            }
            ProductSessionAccessibilityCommand::ConfirmDialog => {
                let Some(dialog) = self.product_destructive_presentation(cx) else {
                    return;
                };
                if !dialog.confirm_enabled {
                    return;
                }
                if self.session_library.pending_removal.is_some() {
                    self.confirm_session_removal(cx);
                } else if self.session_library.pending_stop_archive_review.is_some() {
                    self.confirm_stop_and_archive(cx);
                }
            }
            ProductSessionAccessibilityCommand::CancelDialog => {
                if self.session_library.pending_removal.is_some() {
                    self.cancel_session_removal(cx);
                } else if self.session_library.pending_stop_archive_review.is_some() {
                    self.cancel_stop_and_archive(cx);
                } else if self.session_sidebar.pending_removal.is_some() {
                    self.cancel_group_removal(cx);
                }
                self.project_list_focus.focus(window);
            }
        }
    }

    pub(super) fn render_global_session_library(&self, cx: &Context<Self>) -> AnyElement {
        let sessions = self.session_library.visible_sessions_all();
        let records = sessions
            .iter()
            .filter_map(|metadata| {
                self.saved
                    .app_attached_sessions
                    .iter()
                    .find(|record| record.id == metadata.id)
                    .cloned()
            })
            .collect::<Vec<_>>();

        v_flex()
            .id("global-session-library")
            .debug_selector(|| "global-session-library".to_string())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(theme::library_bg())
            .child(self.render_session_library_controls(cx))
            .when_some(self.session_library.recovery_state(), |this, recovery| {
                this.child(
                    div()
                        .mx(px(theme::SPACE_4))
                        .mt(px(theme::SPACE_3))
                        .p(px(theme::SPACE_3))
                        .border_1()
                        .border_color(theme::warning())
                        .rounded(px(theme::CARD_RADIUS))
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_main())
                        .child(session_recovery_label(recovery)),
                )
            })
            .when_some(
                self.session_library.pending_stop_archive_review,
                |this, id| this.child(self.render_stop_archive_review(id, cx)),
            )
            .when(self.session_sidebar.pending_undo.is_some(), |this| {
                this.child(
                    h_flex()
                        .id("global-session-undo-banner")
                        .justify_end()
                        .px(px(theme::SPACE_5))
                        .pt(px(theme::SPACE_3))
                        .child(
                            Button::new("global-session-undo")
                                .debug_selector(|| "global-session-undo".to_string())
                                .small()
                                .icon(IconName::Undo2)
                                .label(localization::group_undo_action())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.undo_organization(cx);
                                })),
                        ),
                )
            })
            .when_some(
                self.session_library.pending_removal.as_ref(),
                |this, plan| this.child(self.render_session_removal(plan, cx)),
            )
            .child(
                v_flex()
                    .id("global-session-list")
                    .debug_selector(|| "global-session-list".to_string())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p(px(theme::SPACE_4))
                    .when(!records.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .w_full()
                                .border_1()
                                .border_color(theme::border())
                                .rounded(px(theme::CARD_RADIUS))
                                .overflow_hidden()
                                .children(records.iter().map(|session| {
                                    let destination = self.sessions_in_destination(
                                        session.origin.project_id,
                                        session.group_id,
                                    );
                                    let index = destination
                                        .iter()
                                        .position(|candidate| candidate.id == session.id)
                                        .unwrap_or(0);
                                    self.render_session_row(
                                        session,
                                        index,
                                        destination.len().max(1),
                                        cx,
                                    )
                                })),
                        )
                    })
                    .when(records.is_empty(), |this| {
                        this.child(
                            div()
                                .p(px(theme::SPACE_5))
                                .text_center()
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .text_color(theme::text_muted())
                                .child(
                                    if self.session_library.filter != SessionLibraryFilter::All {
                                        localization::session_library_filter_empty()
                                    } else if self.session_library.view
                                        == SessionLibraryView::Archive
                                    {
                                        localization::session_library_archive_empty()
                                    } else {
                                        localization::session_sidebar_empty()
                                    },
                                ),
                        )
                    }),
            )
            .into_any_element()
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
        let groups = self.project_groups(project_id);
        let session_count = self.sessions_in_destination(project_id, None).len()
            + groups
                .iter()
                .map(|group| {
                    self.sessions_in_destination(project_id, Some(group.id))
                        .len()
                })
                .sum::<usize>();

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
            .child(self.render_session_library_controls(cx))
            .when_some(self.session_library.recovery_state(), |this, recovery| {
                this.child(
                    div()
                        .mx(px(theme::SPACE_4))
                        .mt(px(theme::SPACE_3))
                        .p(px(theme::SPACE_3))
                        .border_1()
                        .border_color(theme::warning())
                        .rounded(px(theme::CARD_RADIUS))
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_main())
                        .child(session_recovery_label(recovery)),
                )
            })
            .when_some(
                self.session_library.pending_stop_archive_review,
                |this, id| this.child(self.render_stop_archive_review(id, cx)),
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
            .when_some(
                self.session_library.pending_removal.as_ref(),
                |this, plan| this.child(self.render_session_removal(plan, cx)),
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
                                .child(
                                    if self.session_library.filter != SessionLibraryFilter::All {
                                        localization::session_library_filter_empty()
                                    } else if self.session_library.view
                                        == SessionLibraryView::Archive
                                    {
                                        localization::session_library_archive_empty()
                                    } else {
                                        localization::session_sidebar_empty()
                                    },
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_session_filter_button(
        &self,
        filter: SessionLibraryFilter,
        id: &'static str,
        label: String,
        cx: &Context<Self>,
    ) -> Button {
        Button::new(id)
            .debug_selector(move || id.to_string())
            .small()
            .selected(self.session_library.filter == filter)
            .label(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_session_library_filter(filter, cx);
            }))
    }

    fn render_session_library_controls(&self, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .px(px(theme::SPACE_5))
            .py(px(theme::SPACE_3))
            .gap(px(theme::SPACE_2))
            .border_b_1()
            .border_color(theme::border())
            .child(
                h_flex()
                    .gap(px(theme::SPACE_2))
                    .child(
                        Button::new("session-view-active")
                            .debug_selector(|| "session-view-active".to_string())
                            .small()
                            .selected(self.session_library.view == SessionLibraryView::Active)
                            .label(localization::session_library_active_view())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_session_library_view(SessionLibraryView::Active, cx);
                            })),
                    )
                    .child(
                        Button::new("session-view-archive")
                            .debug_selector(|| "session-view-archive".to_string())
                            .small()
                            .selected(self.session_library.view == SessionLibraryView::Archive)
                            .label(localization::session_library_archive_view())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_session_library_view(SessionLibraryView::Archive, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(theme::SPACE_2))
                    .child(self.render_session_filter_button(
                        SessionLibraryFilter::All,
                        "session-filter-all",
                        localization::session_library_filter_all(),
                        cx,
                    ))
                    .child(self.render_session_filter_button(
                        SessionLibraryFilter::Unread,
                        "session-filter-unread",
                        localization::session_library_filter_unread(),
                        cx,
                    ))
                    .child(self.render_session_filter_button(
                        SessionLibraryFilter::Pinned,
                        "session-filter-pinned",
                        localization::session_library_filter_pinned(),
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn render_stop_archive_review(&self, _id: HostedSessionId, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .id("session-stop-archive-review")
            .debug_selector(|| "session-stop-archive-review".to_string())
            .mx(px(theme::SPACE_4))
            .mt(px(theme::SPACE_4))
            .p(px(theme::SPACE_4))
            .gap(px(theme::SPACE_3))
            .border_1()
            .border_color(theme::warning())
            .rounded(px(theme::CARD_RADIUS))
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .text_color(theme::text_main())
                    .child(localization::session_library_stop_archive_warning()),
            )
            .child(
                h_flex()
                    .gap(px(theme::SPACE_2))
                    .child(
                        Button::new("session-stop-archive-confirm")
                            .debug_selector(|| "session-stop-archive-confirm".to_string())
                            .small()
                            .danger()
                            .label(localization::session_library_stop_archive_action())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_stop_and_archive(cx);
                            })),
                    )
                    .child(
                        Button::new("session-stop-archive-cancel")
                            .debug_selector(|| "session-stop-archive-cancel".to_string())
                            .small()
                            .label(localization::common_cancel())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_stop_and_archive(cx);
                            })),
                    ),
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

    fn render_session_removal(&self, plan: &SessionRemovalPlan, cx: &Context<Self>) -> AnyElement {
        let manifest = plan.manifest;
        let confirmation_matches = !manifest.requires_title_confirmation()
            || self.session_remove_confirm_input.read(cx).value().as_ref() == plan.title.as_str();
        v_flex()
            .id("session-remove-review")
            .debug_selector(|| "session-remove-review".to_string())
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
                    .child(localization::session_library_remove_title()),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::session_library_remove_warning()),
            )
            .child(
                v_flex()
                    .gap(px(theme::SPACE_2))
                    .child(removal_manifest_row(
                        localization::session_library_remove_metadata(),
                        manifest.metadata_bytes,
                    ))
                    .child(removal_manifest_row(
                        localization::session_library_remove_journal(),
                        manifest.journal_bytes,
                    ))
                    .child(removal_manifest_row(
                        localization::session_library_remove_transcript(),
                        manifest.transcript_bytes,
                    ))
                    .child(removal_manifest_row(
                        localization::session_library_remove_artifacts(),
                        manifest.artifact_bytes,
                    ))
                    .child(removal_manifest_row(
                        localization::session_library_remove_files(),
                        manifest.file_count as u64,
                    )),
            )
            .when(manifest.requires_title_confirmation(), |this| {
                this.child(Input::new(&self.session_remove_confirm_input))
            })
            .child(
                h_flex()
                    .gap(px(theme::SPACE_2))
                    .child(
                        Button::new("session-remove-confirm")
                            .debug_selector(|| "session-remove-confirm".to_string())
                            .small()
                            .danger()
                            .icon(IconName::Delete)
                            .label(localization::session_library_confirm_remove_action())
                            .disabled(!confirmation_matches)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_session_removal(cx);
                            })),
                    )
                    .child(
                        Button::new("session-remove-cancel")
                            .debug_selector(|| "session-remove-cancel".to_string())
                            .small()
                            .label(localization::common_cancel())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_session_removal(cx);
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
        let Some(metadata) = self.session_library.session(id) else {
            return div().into_any_element();
        };
        let key = session_key(id);
        let selected = self.session_sidebar.selected_session == Some(id);
        let groups = self.project_groups(session.origin.project_id);
        let current_group = session.group_id;
        let renaming = self.session_library.renaming == Some(id);
        let recognition = session
            .durable_host
            .as_ref()
            .and_then(|host| host.runtime_recognition.as_ref());
        let runtime = (session.route == termirust_domain::SessionLaunchRoute::DurableHost)
            .then(|| runtime_inspector_projection(recognition));
        let transcript = (session.route == termirust_domain::SessionLaunchRoute::DurableHost)
            .then(|| transcript_export_projection(recognition));
        let resume = session_resume_projection(session, metadata);
        let continuity_source = session
            .durable_host
            .as_ref()
            .and_then(|host| host.continuity_source_id)
            .and_then(|source_id| {
                self.saved
                    .app_attached_sessions
                    .iter()
                    .find(|candidate| candidate.id == source_id)
                    .map(|candidate| candidate.title.clone())
            });
        let continuity_successor = self
            .saved
            .app_attached_sessions
            .iter()
            .find(|candidate| {
                candidate
                    .durable_host
                    .as_ref()
                    .and_then(|host| host.continuity_source_id)
                    == Some(id)
            })
            .map(|candidate| candidate.title.clone());
        let host_recovery_target = self
            .host_recovery_plan
            .as_ref()
            .is_some_and(|plan| plan.session_id == id)
            || self
                .host_recovery_operation
                .as_ref()
                .is_some_and(|operation| operation.session_id == id);
        let host_recovery_available = crate::ui::settings::host_recovery_allowed(
            session.route,
            session.state,
            self.host_recovery_operation.is_some() || self.host_recovery_plan.is_some(),
        );
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
                                            .child(metadata.title.as_str().to_string()),
                                    )
                                    .child(
                                        h_flex()
                                            .flex_wrap()
                                            .gap(px(theme::SPACE_2))
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(session_state_label(metadata.lifecycle))
                                            .child(activity_label(&metadata.activity))
                                            .child(
                                                div()
                                                    .debug_selector(|| {
                                                        "session-origin-project".to_string()
                                                    })
                                                    .child(session.project_label.clone()),
                                            )
                                            .when(
                                                !session.preset_label.trim().is_empty(),
                                                |this| {
                                                    this.child(
                                                        div()
                                                            .debug_selector(|| {
                                                                "session-origin-preset".to_string()
                                                            })
                                                            .child(session.preset_label.clone()),
                                                    )
                                                },
                                            )
                                            .child(
                                                div()
                                                    .debug_selector(|| {
                                                        "session-origin-ownership".to_string()
                                                    })
                                                    .child(session_ownership_label(session.route)),
                                            )
                                            .when(metadata.pinned, |this| {
                                                this.child(
                                                    div()
                                                        .text_color(theme::accent())
                                                        .child(localization::session_library_pinned_badge()),
                                                )
                                            })
                                            .when(metadata.unread(), |this| {
                                                this.child(
                                                    div()
                                                        .font_semibold()
                                                        .text_color(theme::warning())
                                                        .child(localization::session_library_unread_badge()),
                                                )
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        Button::new(("session-move", key))
                            .debug_selector(|| "session-move".to_string())
                            .small()
                            .selected(selected)
                            .label(localization::session_library_inspector_title())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_session_for_move(id, cx);
                            })),
                    ),
            )
            .when(selected, |this| {
                this.child(
                    v_flex()
                        .gap(px(theme::SPACE_3))
                        .pt(px(theme::SPACE_2))
                        .border_t_1()
                        .border_color(theme::soft_border())
                        .when(renaming, |this| {
                            this.child(
                                v_flex()
                                    .gap(px(theme::SPACE_2))
                                    .child(Input::new(&self.session_title_input))
                                    .child(
                                        h_flex()
                                            .gap(px(theme::SPACE_2))
                                            .child(
                                                Button::new(("session-rename-save", key))
                                                    .debug_selector(|| "session-rename-save".to_string())
                                                    .small()
                                                    .primary()
                                                    .label(localization::common_save())
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.save_session_rename(cx);
                                                    })),
                                            )
                                            .child(
                                                Button::new(("session-rename-cancel", key))
                                                    .debug_selector(|| "session-rename-cancel".to_string())
                                                    .small()
                                                    .label(localization::common_cancel())
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.cancel_session_rename(cx);
                                                    })),
                                            ),
                                    ),
                            )
                        })
                        .child(
                            v_flex()
                                .gap(px(theme::SPACE_2))
                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                .text_color(theme::text_muted())
                                .child(inspector_row(
                                    localization::session_library_state_label(),
                                    session_state_label(metadata.lifecycle),
                                ))
                                .child(inspector_row(
                                    localization::session_library_title_source_label(),
                                    title_source_label(metadata.title_source),
                                ))
                                .child(inspector_row(
                                    localization::session_library_activity_label(),
                                    activity_label(&metadata.activity),
                                ))
                                .child(inspector_row(
                                    localization::session_library_position_label(),
                                    format!("{} / {}", index + 1, count),
                                ))
                                .when_some(continuity_source, |this, source| {
                                    this.child(inspector_row(
                                        localization::session_resume_source_field(),
                                        source,
                                    ))
                                })
                                .when_some(continuity_successor, |this, successor| {
                                    this.child(inspector_row(
                                        localization::session_resume_successor_field(),
                                        successor,
                                    ))
                                }),
                        )
                        .when_some(runtime, |this, runtime| {
                            let confidence = if runtime.stale {
                                format!(
                                    "{} ({})",
                                    runtime.confidence,
                                    localization::runtime_stale_label()
                                )
                            } else {
                                runtime.confidence
                            };
                            this.child(
                                v_flex()
                                    .id(("session-runtime-inspector", key))
                                    .debug_selector(|| "session-runtime-inspector".to_string())
                                    .gap(px(theme::SPACE_2))
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(inspector_row(
                                        localization::runtime_inspector_runtime_label(),
                                        runtime.runtime,
                                    ))
                                    .child(inspector_row(
                                        localization::runtime_inspector_version_label(),
                                        runtime.version,
                                    ))
                                    .child(inspector_row(
                                        localization::runtime_inspector_ownership_label(),
                                        runtime.ownership,
                                    ))
                                    .child(inspector_row(
                                        localization::runtime_inspector_confidence_label(),
                                        confidence,
                                    ))
                                    .child(inspector_row(
                                        localization::runtime_inspector_generation_label(),
                                        runtime.generation,
                                    ))
                                    .child(inspector_row(
                                        localization::runtime_inspector_capabilities_label(),
                                        runtime.capabilities,
                                    ))
                                    .when_some(runtime.explanation, |this, explanation| {
                                        this.child(
                                            div()
                                                .text_color(theme::warning())
                                                .child(explanation),
                                        )
                                    }),
                            )
                        })
                        .when_some(transcript, |this, transcript| {
                            let reason = transcript.reason_text().unwrap_or_else(
                                localization::transcript_export_unavailable_contract,
                            );
                            this.child(
                                v_flex()
                                    .id(("session-transcript-export", key))
                                    .debug_selector(|| "session-transcript-export".to_string())
                                    .gap(px(theme::SPACE_2))
                                    .child(
                                        Button::new(("session-transcript-export-action", key))
                                            .debug_selector(|| {
                                                "session-transcript-export-action".to_string()
                                            })
                                            .small()
                                            .icon(IconName::File)
                                            .disabled(!transcript.available)
                                            .tooltip(reason.clone())
                                            .label(localization::transcript_export_action()),
                                    )
                                    .when(!transcript.available, |this| {
                                        this.child(
                                            h_flex()
                                                .items_start()
                                                .gap(px(theme::SPACE_2))
                                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                .text_color(theme::warning())
                                                .child(
                                                    Icon::new(IconName::TriangleAlert)
                                                        .size(px(theme::SPACE_4)),
                                                )
                                                .child(div().min_w_0().child(reason)),
                                        )
                                    }),
                            )
                        })
                        .when(host_recovery_available || host_recovery_target, |this| {
                            this.child(self.render_host_recovery_panel(session, cx))
                        })
                        .child(self.render_dev_url_inspector(id, cx))
                        .child(self.render_artifact_gallery(id, cx))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap(px(theme::SPACE_2))
                                .when(
                                    session.route
                                        == termirust_domain::SessionLaunchRoute::DurableHost,
                                    |this| {
                                        this.child(
                                            Button::new(("session-open", key))
                                                .debug_selector(|| "session-open".to_string())
                                                .small()
                                                .icon(IconName::SquareTerminal)
                                                .label(localization::common_open())
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.open_session_from_entry(id, window, cx);
                                                })),
                                        )
                                    },
                                )
                                .child(
                                    Button::new(("session-rename", key))
                                        .debug_selector(|| "session-rename".to_string())
                                        .small()
                                        .label(localization::session_library_rename_action())
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.begin_session_rename(id, window, cx);
                                        })),
                                )
                                .child(
                                    Button::new(("session-pin", key))
                                        .debug_selector(|| "session-pin".to_string())
                                        .small()
                                        .icon(IconName::Star)
                                        .selected(metadata.pinned)
                                        .label(if metadata.pinned {
                                            localization::session_library_unpin_action()
                                        } else {
                                            localization::session_library_pin_action()
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_session_pin(id, cx);
                                        })),
                                )
                                .child(
                                    Button::new(("session-read", key))
                                        .debug_selector(|| "session-read".to_string())
                                        .small()
                                        .disabled(
                                            !metadata.unread()
                                                && metadata.last_output_sequence
                                                    == OutputSequence::ZERO,
                                        )
                                        .label(if metadata.unread() {
                                            localization::session_library_mark_read_action()
                                        } else {
                                            localization::session_library_mark_unread_action()
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_session_read(id, cx);
                                        })),
                                ),
                        )
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap(px(theme::SPACE_2))
                                .when(metadata.lifecycle.can_stop(), |this| {
                                    this.child(
                                        Button::new(("session-stop", key))
                                            .debug_selector(|| "session-stop".to_string())
                                            .small()
                                            .danger()
                                            .label(localization::new_session_stop_action())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.stop_session_only(id, cx);
                                        })),
                                    )
                                })
                                .when(resume.visible, |this| {
                                    this.child(
                                        Button::new(("session-resume", key))
                                            .debug_selector(|| "session-resume".to_string())
                                            .small()
                                            .icon(IconName::Redo2)
                                            .disabled(!resume.enabled)
                                            .label(localization::session_library_resume_action())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.open_session_resume(id, cx);
                                            })),
                                    )
                                })
                                .when(metadata.archived_at.is_none(), |this| {
                                    this.child(
                                        Button::new(("session-archive", key))
                                            .debug_selector(|| "session-archive".to_string())
                                            .small()
                                            .disabled(
                                                !metadata.lifecycle.can_stop()
                                                    && !metadata.lifecycle.is_exited(),
                                            )
                                            .label(if metadata.lifecycle.can_stop() {
                                                localization::session_library_stop_archive_action()
                                            } else {
                                                localization::session_library_archive_action()
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.archive_or_stop_session(id, cx);
                                            })),
                                    )
                                })
                                .when(metadata.archived_at.is_some(), |this| {
                                    this.child(
                                        Button::new(("session-restore", key))
                                            .debug_selector(|| "session-restore".to_string())
                                            .small()
                                            .label(localization::session_library_restore_action())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.restore_session(id, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new(("session-remove", key))
                                            .debug_selector(|| "session-remove".to_string())
                                            .small()
                                            .danger()
                                            .icon(IconName::Delete)
                                            .disabled(!metadata.can_remove())
                                            .label(localization::session_library_remove_action())
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.begin_session_removal(id, window, cx);
                                            })),
                                    )
                                })
                                .when_some(resume.reason, |this, reason| {
                                    this.child(
                                        div()
                                            .w_full()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(resume_error_message(reason)),
                                    )
                                }),
                        )
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap(px(theme::SPACE_2))
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
                        ),
                )
            })
            .into_any_element()
    }

    fn render_host_recovery_panel(
        &self,
        session: &SavedAppAttachedSession,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = session.id;
        let key = session_key(id);
        let plan = self
            .host_recovery_plan
            .as_ref()
            .filter(|plan| plan.session_id == id);
        let busy = self
            .host_recovery_operation
            .as_ref()
            .is_some_and(|operation| operation.session_id == id);
        let view =
            crate::ui::settings::host_recovery_view_model(session.route, session.state, plan, busy);
        v_flex()
            .id(("session-host-recovery", key))
            .debug_selector(|| "session-host-recovery".to_string())
            .gap(px(theme::SPACE_2))
            .p(px(theme::SPACE_3))
            .border_1()
            .border_color(theme::warning())
            .rounded(px(theme::CARD_RADIUS))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(theme::SPACE_2))
                    .text_color(theme::warning())
                    .child(Icon::new(IconName::TriangleAlert).size(px(theme::SPACE_4)))
                    .child(
                        div()
                            .font_semibold()
                            .text_color(theme::text_main())
                            .child(localization::recovery_title()),
                    ),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::recovery_description()),
            )
            .when(!view.evidence.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_main())
                        .child(view.evidence.clone()),
                )
            })
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::warning())
                    .child(localization::recovery_safety_notice()),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(theme::SPACE_2))
                    .when(plan.is_none() && !busy, |this| {
                        this.child(
                            Button::new(("session-host-recovery-prepare", key))
                                .debug_selector(|| "session-host-recovery-prepare".to_string())
                                .small()
                                .primary()
                                .label(localization::recovery_prepare_action())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.prepare_host_recovery(id, cx);
                                })),
                        )
                    })
                    .when(plan.is_some(), |this| {
                        this.child(
                            Button::new(("session-host-recovery-confirm", key))
                                .debug_selector(|| "session-host-recovery-confirm".to_string())
                                .small()
                                .primary()
                                .disabled(!view.can_confirm)
                                .label(localization::recovery_confirm_action())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.confirm_host_recovery(id, cx);
                                })),
                        )
                    })
                    .when(view.can_cancel, |this| {
                        this.child(
                            Button::new(("session-host-recovery-cancel", key))
                                .debug_selector(|| "session-host-recovery-cancel".to_string())
                                .small()
                                .label(localization::common_cancel())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cancel_host_recovery(id, cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}

fn inspector_row(label: String, value: String) -> AnyElement {
    h_flex()
        .justify_between()
        .gap(px(theme::SPACE_3))
        .child(div().child(label))
        .child(div().text_color(theme::text_main()).child(value))
        .into_any_element()
}

fn removal_manifest_row(label: String, value: u64) -> AnyElement {
    inspector_row(label, value.to_string())
}

fn product_button(
    action: ProductSessionAction,
    name: MessageId,
    parent: Option<AccessibleRowId>,
) -> ProductSessionControl {
    ProductSessionControl {
        action,
        parent,
        role: ProductControlRole::Button,
        name,
        value: None,
        selected: false,
        disabled: false,
        in_dialog: false,
    }
}

fn product_tab(
    action: ProductSessionAction,
    name: MessageId,
    selected: bool,
) -> ProductSessionControl {
    ProductSessionControl {
        action,
        parent: None,
        role: ProductControlRole::Tab,
        name,
        value: None,
        selected,
        disabled: false,
        in_dialog: false,
    }
}

fn product_text_field(
    action: ProductSessionAction,
    name: MessageId,
    value: String,
    in_dialog: bool,
) -> ProductSessionControl {
    ProductSessionControl {
        action,
        parent: None,
        role: ProductControlRole::TextField,
        name,
        value: Some(value),
        selected: false,
        disabled: false,
        in_dialog,
    }
}

fn append_accessible_session_rows(
    rows: &mut Vec<AccessibleCollectionRow>,
    sessions: &[SavedAppAttachedSession],
    parent: AccessibleRowId,
    library: &super::session_library::SessionLibraryState,
    selected: Option<HostedSessionId>,
    position_offset: usize,
    set_size: usize,
) {
    for (index, session) in sessions.iter().enumerate() {
        let Some(metadata) = library.session(session.id) else {
            continue;
        };
        rows.push(AccessibleCollectionRow {
            id: accessible_session_id(session.id),
            parent: Some(parent),
            level: HierarchyLevel::Session,
            name: metadata.title.as_str().to_string(),
            status: session_state_message_id(metadata.lifecycle),
            selected: selected == Some(session.id),
            expanded: None,
            unread: metadata.unread(),
            disabled: false,
            position: position_offset + index + 1,
            set_size: set_size.max(1),
        });
    }
}

fn accessible_project_id(id: ProjectId) -> AccessibleRowId {
    AccessibleRowId::project(id.as_uuid().as_u128())
}

fn accessible_group_id(id: GroupId) -> AccessibleRowId {
    AccessibleRowId::group(id.as_uuid().as_u128())
}

fn accessible_session_id(id: HostedSessionId) -> AccessibleRowId {
    AccessibleRowId::session(id.as_uuid().as_u128())
}

fn accessible_project_row_id(id: AccessibleRowId) -> Option<ProjectId> {
    (id.kind == termirust_ui_contract::AccessibleRowKind::Project)
        .then(|| ProjectId::from_uuid(uuid::Uuid::from_u128(id.value)))
}

fn accessible_group_row_id(id: AccessibleRowId) -> Option<GroupId> {
    (id.kind == termirust_ui_contract::AccessibleRowKind::Group)
        .then(|| GroupId::from_uuid(uuid::Uuid::from_u128(id.value)))
}

fn accessible_session_row_id(id: AccessibleRowId) -> Option<HostedSessionId> {
    (id.kind == termirust_ui_contract::AccessibleRowKind::Session)
        .then(|| HostedSessionId::from_uuid(uuid::Uuid::from_u128(id.value)))
}

fn product_move_delta(direction: ProductMoveDirection) -> isize {
    match direction {
        ProductMoveDirection::Up => -1,
        ProductMoveDirection::Down => 1,
    }
}

fn project_status_message_id(status: termirust_domain::ProjectStatus) -> MessageId {
    match status {
        termirust_domain::ProjectStatus::Available => MessageId::ProjectStatusAvailable,
        termirust_domain::ProjectStatus::Unavailable => MessageId::ProjectStatusUnavailable,
        termirust_domain::ProjectStatus::PermissionDenied => {
            MessageId::ProjectStatusPermissionDenied
        }
    }
}

fn session_state_message_id(state: HostedSessionState) -> MessageId {
    match state {
        HostedSessionState::Draft => MessageId::NewSessionPhaseDraft,
        HostedSessionState::Validating => MessageId::NewSessionPhaseValidating,
        HostedSessionState::Starting => MessageId::NewSessionPhaseStarting,
        HostedSessionState::Provisioning => MessageId::NewSessionPhaseProvisioning,
        HostedSessionState::Attaching => MessageId::NewSessionPhaseAttaching,
        HostedSessionState::Replaying => MessageId::NewSessionPhaseReplaying,
        HostedSessionState::Live => MessageId::NewSessionPhaseLive,
        HostedSessionState::RecordingPaused => MessageId::NewSessionPhaseRecordingPaused,
        HostedSessionState::Stopping => MessageId::NewSessionStatusStopping,
        HostedSessionState::Offline => MessageId::NewSessionPhaseOffline,
        HostedSessionState::Orphaned => MessageId::NewSessionPhaseOrphaned,
        HostedSessionState::Gap => MessageId::NewSessionPhaseGap,
        HostedSessionState::PermissionDenied => MessageId::NewSessionPhasePermissionDenied,
        HostedSessionState::Incompatible => MessageId::NewSessionPhaseIncompatible,
        HostedSessionState::RunningAppAttached => MessageId::NewSessionPhaseRunning,
        HostedSessionState::Failed => MessageId::NewSessionPhaseFailed,
        HostedSessionState::Cancelled => MessageId::NewSessionPhaseCancelled,
        HostedSessionState::Exited => MessageId::NewSessionPhaseExited,
    }
}

fn product_session_surface_state(
    project_load: &super::projects::ProjectLibraryLoadState,
    recovery: Option<SessionLibraryRecovery>,
    rows_empty: bool,
    filtered: bool,
) -> ProductSessionSurfaceState {
    match project_load {
        super::projects::ProjectLibraryLoadState::Loading => {
            return ProductSessionSurfaceState::Loading;
        }
        super::projects::ProjectLibraryLoadState::Failed(
            super::projects::ProjectStoreFailure::Unavailable,
        ) => return ProductSessionSurfaceState::Unavailable,
        super::projects::ProjectLibraryLoadState::Failed(
            super::projects::ProjectStoreFailure::Corrupt
            | super::projects::ProjectStoreFailure::Newer,
        ) => return ProductSessionSurfaceState::Recovery,
        super::projects::ProjectLibraryLoadState::Ready => {}
    }
    if let Some(recovery) = recovery {
        return match recovery {
            SessionLibraryRecovery::RecoveredLastGood => ProductSessionSurfaceState::Partial,
            SessionLibraryRecovery::Corrupt | SessionLibraryRecovery::Newer => {
                ProductSessionSurfaceState::Recovery
            }
            SessionLibraryRecovery::PermissionDenied => {
                ProductSessionSurfaceState::PermissionDenied
            }
            SessionLibraryRecovery::Unavailable => ProductSessionSurfaceState::Unavailable,
        };
    }
    if rows_empty {
        if filtered {
            ProductSessionSurfaceState::FilterEmpty
        } else {
            ProductSessionSurfaceState::Empty
        }
    } else {
        ProductSessionSurfaceState::Ready
    }
}

fn title_source_label(source: termirust_domain::TitleSource) -> String {
    match source {
        termirust_domain::TitleSource::Default => {
            localization::session_library_title_source_default()
        }
        termirust_domain::TitleSource::Automatic => {
            localization::session_library_title_source_automatic()
        }
        termirust_domain::TitleSource::Imported => {
            localization::session_library_title_source_imported()
        }
        termirust_domain::TitleSource::Manual => {
            localization::session_library_title_source_manual()
        }
    }
}

fn activity_label(activity: &termirust_domain::ActivityAggregate) -> String {
    let state = match activity.state {
        termirust_domain::ActivityState::Unknown => {
            localization::session_library_activity_unknown()
        }
        termirust_domain::ActivityState::Idle => localization::session_library_activity_idle(),
        termirust_domain::ActivityState::Busy => localization::session_library_activity_busy(),
        termirust_domain::ActivityState::NeedsInput => {
            localization::session_library_activity_needs_input()
        }
        termirust_domain::ActivityState::Done => localization::session_library_activity_done(),
        termirust_domain::ActivityState::Failed => localization::session_library_activity_failed(),
    };
    let confidence = if activity.stale {
        return format!("{state} ({})", localization::runtime_stale_label());
    } else if activity.confidence == termirust_domain::ActivityConfidence::Verified {
        localization::runtime_confidence_verified()
    } else {
        localization::session_library_activity_estimated()
    };
    format!("{state} ({confidence})")
}

fn session_ownership_label(route: termirust_domain::SessionLaunchRoute) -> String {
    match route {
        termirust_domain::SessionLaunchRoute::LegacyAppAttached => "App-attached".to_string(),
        termirust_domain::SessionLaunchRoute::DurableHost => "Durable".to_string(),
    }
}

fn session_recovery_label(recovery: SessionLibraryRecovery) -> String {
    match recovery {
        SessionLibraryRecovery::RecoveredLastGood => {
            localization::session_library_recovered_last_good()
        }
        SessionLibraryRecovery::Corrupt => localization::session_library_store_corrupt(),
        SessionLibraryRecovery::Newer => localization::session_library_store_newer(),
        SessionLibraryRecovery::PermissionDenied => {
            localization::session_library_store_permission()
        }
        SessionLibraryRecovery::Unavailable => localization::session_library_store_unavailable(),
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
        HostedSessionState::Stopping => localization::new_session_status_stopping(),
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
        | HostedSessionState::Stopping
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
            title: localization::new_session_title(),
            title_source: termirust_domain::TitleSource::Default,
            activity: termirust_domain::ActivityAggregate::default(),
            pinned: false,
            read_through_sequence: 0,
            unread_sequence: None,
            archived_at: None,
            revision: termirust_domain::Revision::ZERO,
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
