use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement,
    StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::{
    Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use termirust_domain::{AddProject, CanonicalPath, ProjectError, ProjectId, ProjectStatus};
use termirust_store::{
    ProjectRepository, ProjectSnapshot, RemovedProject, StoreError, StoreHealth,
};

use super::{TermiRustApp, theme};
use crate::storage::project_store_dir;
use crate::ui::localization;

const PROJECT_UNDO_WINDOW: Duration = Duration::from_secs(10);

pub(super) enum ProjectLibraryLoadState {
    Loading,
    Ready,
    Failed(ProjectStoreFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProjectStoreFailure {
    Corrupt,
    Newer,
    Unavailable,
}

pub(super) struct ProjectAddDraft {
    canonical_root: CanonicalPath,
}

pub(super) struct ProjectAddValidation {
    generation: u64,
    selected_root: PathBuf,
}

pub(super) struct PendingProjectRemoval {
    removed: RemovedProject,
    expires_at: Instant,
}

pub(super) struct ProjectLibraryState {
    pub repository: Option<ProjectRepository>,
    pub load_state: ProjectLibraryLoadState,
    pub snapshot: Option<ProjectSnapshot>,
    pub selected_id: Option<ProjectId>,
    pub add_draft: Option<ProjectAddDraft>,
    pub add_validation: Option<ProjectAddValidation>,
    pub pending_removal: Option<PendingProjectRemoval>,
    next_validation_generation: u64,
}

impl ProjectLibraryState {
    pub fn open_default() -> Self {
        let mut state = Self {
            repository: None,
            load_state: ProjectLibraryLoadState::Loading,
            snapshot: None,
            selected_id: None,
            add_draft: None,
            add_validation: None,
            pending_removal: None,
            next_validation_generation: 1,
        };
        let repository = project_store_dir()
            .map_err(|_| ProjectStoreFailure::Unavailable)
            .and_then(|root| ProjectRepository::open(root).map_err(classify_store_failure));
        match repository {
            Ok(repository) => {
                state.repository = Some(repository);
                state.reload();
            }
            Err(failure) => state.load_state = ProjectLibraryLoadState::Failed(failure),
        }
        state
    }

    pub fn reload(&mut self) {
        let Some(repository) = &self.repository else {
            self.load_state = ProjectLibraryLoadState::Failed(ProjectStoreFailure::Unavailable);
            self.snapshot = None;
            return;
        };
        match repository.load() {
            Ok(snapshot) => {
                if self.selected_id.is_some_and(|selected| {
                    !snapshot
                        .projects
                        .iter()
                        .any(|summary| summary.project.id == selected)
                }) {
                    self.selected_id = None;
                }
                self.snapshot = Some(snapshot);
                self.load_state = ProjectLibraryLoadState::Ready;
            }
            Err(error) => {
                self.snapshot = None;
                self.load_state = ProjectLibraryLoadState::Failed(classify_store_failure(error));
            }
        }
    }

    fn error_message(&self) -> Option<String> {
        match &self.load_state {
            ProjectLibraryLoadState::Loading | ProjectLibraryLoadState::Ready => self
                .snapshot
                .as_ref()
                .filter(|snapshot| snapshot.health == StoreHealth::RecoveredLastGood)
                .map(|_| localization::project_store_recovered()),
            ProjectLibraryLoadState::Failed(ProjectStoreFailure::Corrupt) => {
                Some(localization::project_store_corrupt())
            }
            ProjectLibraryLoadState::Failed(ProjectStoreFailure::Newer) => {
                Some(localization::project_store_newer())
            }
            ProjectLibraryLoadState::Failed(ProjectStoreFailure::Unavailable) => {
                Some(localization::project_store_unavailable())
            }
        }
    }
}

impl TermiRustApp {
    pub(super) fn choose_project_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(test)]
        if let Some(selection) = crate::test_support::take_dialog_selection() {
            if let Some(path) = selection {
                self.start_project_validation(path, window, cx);
            }
            return;
        }

        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = rfd::AsyncFileDialog::new()
                .pick_folder()
                .await
                .map(|folder| folder.path().to_path_buf())
            else {
                return;
            };
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.start_project_validation(path, window, cx);
                });
            });
        })
        .detach();
    }

    fn start_project_validation(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let generation = self.project_library.next_validation_generation;
        self.project_library.next_validation_generation = generation.wrapping_add(1).max(1);
        self.project_library.add_draft = None;
        self.project_library.add_validation = Some(ProjectAddValidation {
            generation,
            selected_root: path.clone(),
        });
        self.error_message.clear();
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { CanonicalPath::resolve(&path) })
                .await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.finish_project_validation(generation, result, window, cx);
                });
            });
        })
        .detach();
    }

    fn finish_project_validation(
        &mut self,
        generation: u64,
        result: Result<CanonicalPath, ProjectError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .project_library
            .add_validation
            .as_ref()
            .is_none_or(|validation| validation.generation != generation)
        {
            return;
        }
        self.project_library.add_validation = None;
        match result {
            Ok(canonical_root) => {
                let default_name = canonical_root
                    .display_name()
                    .map(|name| name.as_str().to_string())
                    .unwrap_or_default();
                Self::set_input_value(&self.project_label_input, default_name, window, cx);
                self.project_library.add_draft = Some(ProjectAddDraft { canonical_root });
                self.project_label_input
                    .update(cx, |input, cx| input.focus(window, cx));
                self.error_message.clear();
            }
            Err(error) => self.error_message = project_error_message(&error),
        }
        cx.notify();
    }

    pub(super) fn cancel_project_add(&mut self, cx: &mut Context<Self>) {
        self.project_library.add_draft = None;
        self.project_library.add_validation = None;
        self.project_library.next_validation_generation = self
            .project_library
            .next_validation_generation
            .wrapping_add(1)
            .max(1);
        self.error_message.clear();
        cx.notify();
    }

    pub(super) fn commit_project_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.project_library.add_draft else {
            return;
        };
        let Some(repository) = self.project_library.repository.clone() else {
            self.error_message = localization::project_store_unavailable();
            cx.notify();
            return;
        };
        let expected = self
            .project_library
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.revision)
            .unwrap_or_default();
        let display_name = self.project_label_input.read(cx).value().to_string();
        let request = AddProject {
            id: ProjectId::new(),
            root: PathBuf::from(draft.canonical_root.as_path()),
            display_name: Some(display_name),
            expected,
        };
        match repository.add_project(request) {
            Ok(project) => {
                let name = project.display_name.as_str().to_string();
                self.project_library.add_draft = None;
                self.project_library.reload();
                self.project_library.selected_id = Some(project.id);
                self.project_list_focus.focus(window);
                self.status_message = localization::project_added_status(name);
                self.error_message.clear();
            }
            Err(StoreError::Domain(ProjectError::AlreadyPresent { id })) => {
                self.project_library.add_draft = None;
                self.project_library.reload();
                self.project_library.selected_id = Some(id);
                self.project_list_focus.focus(window);
                let name = self
                    .project_library
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| {
                        snapshot
                            .projects
                            .iter()
                            .find(|summary| summary.project.id == id)
                    })
                    .map(|summary| summary.project.display_name.as_str().to_string())
                    .unwrap_or_default();
                self.status_message = localization::project_duplicate_status(name);
                self.error_message.clear();
            }
            Err(StoreError::Domain(ProjectError::StaleRevision { .. })) => {
                self.project_library.reload();
                self.error_message = localization::project_error_stale();
            }
            Err(StoreError::Domain(error)) => {
                self.error_message = project_error_message(&error);
            }
            Err(error) => {
                self.error_message = project_store_error_message(&error);
                self.project_library.reload();
            }
        }
        cx.notify();
    }

    pub(super) fn remove_project(&mut self, id: ProjectId, cx: &mut Context<Self>) {
        let Some(repository) = self.project_library.repository.clone() else {
            self.error_message = localization::project_store_unavailable();
            cx.notify();
            return;
        };
        let Some(expected) = self
            .project_library
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.revision)
        else {
            return;
        };
        match repository.remove_project(id, expected) {
            Ok(removed) => {
                let name = removed.project.display_name.as_str().to_string();
                self.project_library.pending_removal = Some(PendingProjectRemoval {
                    removed,
                    expires_at: Instant::now() + PROJECT_UNDO_WINDOW,
                });
                self.project_library.reload();
                self.repair_session_group_references();
                self.status_message = localization::project_removed_status(name);
                self.error_message.clear();
            }
            Err(StoreError::Domain(ProjectError::StaleRevision { .. })) => {
                self.project_library.reload();
                self.error_message = localization::project_error_stale();
            }
            Err(error) => self.error_message = project_store_error_message(&error),
        }
        cx.notify();
    }

    pub(super) fn undo_project_removal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.project_library.pending_removal.take() else {
            return;
        };
        if Instant::now() >= pending.expires_at {
            self.status_message = localization::project_undo_expired();
            cx.notify();
            return;
        }
        let Some(repository) = self.project_library.repository.clone() else {
            self.error_message = localization::project_store_unavailable();
            cx.notify();
            return;
        };
        let Some(expected) = self
            .project_library
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.revision)
        else {
            return;
        };
        match repository.restore_project(pending.removed, expected) {
            Ok(restored) => {
                let project = restored.project;
                let name = project.display_name.as_str().to_string();
                self.project_library.reload();
                self.project_library.selected_id = Some(project.id);
                self.project_list_focus.focus(window);
                self.status_message = localization::project_restored_status(name);
                self.error_message.clear();
            }
            Err(error) => {
                self.error_message = project_store_error_message(&error);
                self.project_library.reload();
            }
        }
        cx.notify();
    }

    pub(super) fn process_project_undo_expiry(&mut self) -> bool {
        let expired = self
            .project_library
            .pending_removal
            .as_ref()
            .is_some_and(|pending| Instant::now() >= pending.expires_at);
        if expired {
            self.project_library.pending_removal = None;
            self.status_message = localization::project_undo_expired();
        }
        expired
    }

    pub(super) fn retry_project_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.project_library.reload();
        self.repair_session_group_references();
        if matches!(
            self.project_library.load_state,
            ProjectLibraryLoadState::Ready
        ) {
            self.status_message = localization::projects_ready_status();
            self.error_message.clear();
            self.project_list_focus.focus(window);
        } else {
            self.error_message = self
                .project_library
                .error_message()
                .unwrap_or_else(localization::project_store_unavailable);
        }
        cx.notify();
    }

    pub(super) fn render_projects_view(&self, cx: &Context<Self>) -> AnyElement {
        let add_disabled = self
            .project_library
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.read_only);
        let content = match &self.project_library.load_state {
            ProjectLibraryLoadState::Loading => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(localization::projects_loading())
                .into_any_element(),
            ProjectLibraryLoadState::Failed(_) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(theme::SPACE_4))
                .child(
                    self.project_library
                        .error_message()
                        .unwrap_or_else(localization::project_store_unavailable),
                )
                .child(
                    Button::new("projects-store-retry")
                        .debug_selector(|| "projects-store-retry".to_string())
                        .icon(IconName::Redo2)
                        .label(localization::common_retry())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.retry_project_library(window, cx);
                        })),
                )
                .into_any_element(),
            ProjectLibraryLoadState::Ready => h_flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(self.render_project_list(cx)),
                )
                .child(self.render_session_sidebar(self.project_library.selected_id, cx))
                .into_any_element(),
        };

        v_flex()
            .id("projects-view")
            .debug_selector(|| "projects-view".to_string())
            .track_focus(&self.project_list_focus)
            .flex_1()
            .min_h_0()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .flex_none()
                    .justify_between()
                    .items_center()
                    .flex_wrap()
                    .px(px(theme::SPACE_6))
                    .py(px(theme::SPACE_5))
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        v_flex()
                            .gap(px(theme::SPACE_2))
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_HEADING_SIZE))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(localization::projects_title()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::projects_subtitle()),
                            ),
                    )
                    .child(
                        Button::new("projects-add")
                            .debug_selector(|| "projects-add".to_string())
                            .primary()
                            .icon(IconName::Plus)
                            .label(localization::projects_add_action())
                            .disabled(add_disabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose_project_folder(window, cx);
                            })),
                    ),
            )
            .when_some(self.project_library.error_message(), |this, message| {
                this.child(
                    h_flex()
                        .id("projects-recovery-banner")
                        .mx(px(theme::SPACE_6))
                        .mt(px(theme::SPACE_5))
                        .p(px(theme::SPACE_4))
                        .gap(px(theme::SPACE_3))
                        .rounded(px(theme::CARD_RADIUS))
                        .bg(theme::accent_soft())
                        .text_color(theme::warning())
                        .child(Icon::new(IconName::TriangleAlert).size(px(theme::SPACE_5)))
                        .child(message),
                )
            })
            .when_some(
                self.project_library.pending_removal.as_ref(),
                |this, pending| {
                    let name = pending.removed.project.display_name.as_str().to_string();
                    this.child(
                        h_flex()
                            .id("project-undo-banner")
                            .mx(px(theme::SPACE_6))
                            .mt(px(theme::SPACE_5))
                            .p(px(theme::SPACE_4))
                            .gap(px(theme::SPACE_4))
                            .justify_between()
                            .flex_wrap()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::accent_soft())
                            .child(localization::project_removed_status(name))
                            .child(
                                Button::new("project-undo")
                                    .debug_selector(|| "project-undo".to_string())
                                    .small()
                                    .icon(IconName::Undo2)
                                    .label(localization::project_undo_action())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.undo_project_removal(window, cx);
                                    })),
                            ),
                    )
                },
            )
            .when_some(self.project_library.add_draft.as_ref(), |this, draft| {
                this.child(self.render_project_add_review(draft, cx))
            })
            .when_some(
                self.project_library.add_validation.as_ref(),
                |this, validation| this.child(self.render_project_validation(validation, cx)),
            )
            .when_some(
                self.project_library
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.worktree_intents.first()),
                |this, intent| {
                    let id = intent.plan.id;
                    this.child(
                        h_flex()
                            .id("worktree-recovery-banner")
                            .mx(px(theme::SPACE_6))
                            .mt(px(theme::SPACE_5))
                            .p(px(theme::SPACE_4))
                            .gap(px(theme::SPACE_4))
                            .justify_between()
                            .flex_wrap()
                            .rounded(px(theme::CARD_RADIUS))
                            .bg(theme::accent_soft())
                            .text_color(theme::warning())
                            .child(
                                h_flex()
                                    .gap(px(theme::SPACE_2))
                                    .child(
                                        Icon::new(IconName::TriangleAlert)
                                            .size(px(theme::ICON_SIZE_DEFAULT)),
                                    )
                                    .child(localization::worktree_recovery_banner()),
                            )
                            .child(
                                Button::new("worktree-review-recovery")
                                    .label(localization::worktree_review_recovery_action())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.review_worktree_recovery(id, window, cx);
                                    })),
                            ),
                    )
                },
            )
            .child(content)
            .into_any_element()
    }

    fn render_project_list(&self, cx: &Context<Self>) -> AnyElement {
        let projects = self
            .project_library
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.projects.as_slice())
            .unwrap_or_default();
        if projects.is_empty() {
            return v_flex()
                .id("projects-empty")
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(theme::SPACE_4))
                .p(px(theme::SPACE_7))
                .child(
                    Icon::new(IconName::FolderOpen)
                        .size(px(theme::SPACE_7))
                        .text_color(theme::accent()),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                        .font_semibold()
                        .text_color(theme::text_main())
                        .child(localization::projects_empty_title()),
                )
                .child(
                    div()
                        .text_center()
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::projects_empty_description()),
                )
                .child(
                    Button::new("projects-empty-add")
                        .debug_selector(|| "projects-empty-add".to_string())
                        .primary()
                        .icon(IconName::Plus)
                        .label(localization::projects_add_action())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.choose_project_folder(window, cx);
                        })),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::projects_folder_safety()),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::projects_local_only()),
                )
                .into_any_element();
        }

        v_flex()
            .id("projects-list")
            .debug_selector(|| "projects-list".to_string())
            .flex_1()
            .min_h_0()
            .gap(px(theme::SPACE_3))
            .p(px(theme::SPACE_6))
            .overflow_y_scroll()
            .children(projects.iter().map(|summary| {
                let project_id = summary.project.id;
                let element_key = project_element_key(project_id);
                let selected = self.project_library.selected_id == Some(project_id);
                let (status, status_color, icon) = project_status_presentation(summary.status);
                h_flex()
                    .id(("project-row", element_key))
                    .debug_selector(|| "project-row".to_string())
                    .w_full()
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap(px(theme::SPACE_5))
                    .p(px(theme::SPACE_5))
                    .rounded(px(theme::CARD_RADIUS))
                    .bg(theme::library_card())
                    .border_1()
                    .border_color(if selected {
                        theme::focus_ring()
                    } else {
                        theme::border()
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.project_library.selected_id = Some(project_id);
                        cx.notify();
                    }))
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap(px(theme::SPACE_4))
                            .items_center()
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(theme::SPACE_5))
                                    .text_color(theme::accent()),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .gap(px(theme::SPACE_2))
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_BODY_SIZE))
                                            .font_semibold()
                                            .text_color(theme::text_main())
                                            .child(
                                                summary.project.display_name.as_str().to_string(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .truncate()
                                            .child(
                                                summary
                                                    .project
                                                    .canonical_root
                                                    .as_path()
                                                    .display()
                                                    .to_string(),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .justify_end()
                            .gap(px(theme::SPACE_4))
                            .items_center()
                            .child(
                                h_flex()
                                    .gap(px(theme::SPACE_2))
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(status_color)
                                    .child(Icon::new(icon).size(px(theme::TYPE_BODY_SMALL_SIZE)))
                                    .child(status),
                            )
                            .when(summary.status != ProjectStatus::Available, |this| {
                                this.child(
                                    Button::new(("project-retry", element_key))
                                        .debug_selector(|| "project-retry".to_string())
                                        .small()
                                        .icon(IconName::Redo2)
                                        .label(localization::common_retry())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.retry_project_library(window, cx);
                                        })),
                                )
                            })
                            .child(
                                Button::new(("project-new-session", element_key))
                                    .debug_selector(|| "project-new-session".to_string())
                                    .small()
                                    .primary()
                                    .icon(IconName::SquareTerminal)
                                    .label(localization::new_session_action())
                                    .disabled(summary.status != ProjectStatus::Available)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_new_session(project_id, window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("project-new-worktree", element_key))
                                    .debug_selector(|| "project-new-worktree".to_string())
                                    .small()
                                    .icon(IconName::Plus)
                                    .label(localization::worktree_new_action())
                                    .disabled(summary.status != ProjectStatus::Available)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_worktree_launch(project_id, window, cx);
                                    })),
                            )
                            .child(
                                v_flex()
                                    .items_end()
                                    .gap(px(theme::SPACE_2))
                                    .child(
                                        Button::new(("project-remove", element_key))
                                            .debug_selector(|| "project-remove".to_string())
                                            .small()
                                            .danger()
                                            .icon(IconName::Delete)
                                            .label(localization::project_remove_action())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.remove_project(project_id, cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(localization::project_files_stay()),
                                    ),
                            ),
                    )
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn render_project_add_review(&self, draft: &ProjectAddDraft, cx: &Context<Self>) -> AnyElement {
        v_flex()
            .id("project-add-review")
            .debug_selector(|| "project-add-review".to_string())
            .mx(px(theme::SPACE_6))
            .mt(px(theme::SPACE_5))
            .p(px(theme::SPACE_5))
            .gap(px(theme::SPACE_4))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::focus_ring())
            .child(
                div()
                    .text_size(px(theme::TYPE_HEADING_SMALL_SIZE))
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(localization::project_review_title()),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::project_review_description(
                        draft.canonical_root.as_path().display().to_string(),
                    )),
            )
            .child(
                v_flex()
                    .gap(px(theme::SPACE_2))
                    .child(
                        div()
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .font_medium()
                            .text_color(theme::text_main())
                            .child(localization::project_label_field()),
                    )
                    .child(Input::new(&self.project_label_input)),
            )
            .child(
                h_flex()
                    .gap(px(theme::SPACE_3))
                    .child(
                        Button::new("project-add-confirm")
                            .debug_selector(|| "project-add-confirm".to_string())
                            .primary()
                            .label(localization::project_add_confirm())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.commit_project_add(window, cx);
                            })),
                    )
                    .child(
                        Button::new("project-add-cancel")
                            .debug_selector(|| "project-add-cancel".to_string())
                            .label(localization::common_cancel())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_project_add(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::projects_folder_safety()),
            )
            .into_any_element()
    }

    fn render_project_validation(
        &self,
        validation: &ProjectAddValidation,
        cx: &Context<Self>,
    ) -> AnyElement {
        v_flex()
            .id("project-validation")
            .debug_selector(|| "project-validation".to_string())
            .mx(px(theme::SPACE_6))
            .mt(px(theme::SPACE_5))
            .p(px(theme::SPACE_5))
            .gap(px(theme::SPACE_4))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_card())
            .border_1()
            .border_color(theme::focus_ring())
            .child(
                h_flex()
                    .gap(px(theme::SPACE_3))
                    .child(Icon::new(IconName::LoaderCircle).size(px(theme::SPACE_5)))
                    .child(localization::project_validating()),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::project_review_description(
                        validation.selected_root.display().to_string(),
                    )),
            )
            .child(
                Button::new("project-validation-cancel")
                    .debug_selector(|| "project-validation-cancel".to_string())
                    .label(localization::common_cancel())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cancel_project_add(cx);
                    })),
            )
            .into_any_element()
    }
}

fn classify_store_failure(error: StoreError) -> ProjectStoreFailure {
    match error {
        StoreError::StoreNewer { .. } => ProjectStoreFailure::Newer,
        StoreError::Corrupt { .. }
        | StoreError::UnsafeEntry { .. }
        | StoreError::TooLarge { .. } => ProjectStoreFailure::Corrupt,
        StoreError::Io { .. }
        | StoreError::InvalidInstanceId
        | StoreError::Domain(_)
        | StoreError::GroupDomain(_)
        | StoreError::PresetDomain(_)
        | StoreError::SessionDomain(_)
        | StoreError::WorktreeDomain(_) => ProjectStoreFailure::Unavailable,
    }
}

fn project_store_error_message(error: &StoreError) -> String {
    match classify_store_failure(error.clone()) {
        ProjectStoreFailure::Corrupt => localization::project_store_corrupt(),
        ProjectStoreFailure::Newer => localization::project_store_newer(),
        ProjectStoreFailure::Unavailable => localization::project_store_unavailable(),
    }
}

fn project_error_message(error: &ProjectError) -> String {
    match error {
        ProjectError::EmptyPath | ProjectError::PathContainsNul => {
            localization::project_error_empty_path()
        }
        ProjectError::PermissionDenied => localization::project_error_permission_denied(),
        ProjectError::Unavailable | ProjectError::PathChanged | ProjectError::PathValidation => {
            localization::project_error_unavailable()
        }
        ProjectError::NotDirectory => localization::project_error_not_directory(),
        ProjectError::PathTooLong | ProjectError::NonUnicodePath => {
            localization::project_error_path_too_long()
        }
        ProjectError::EmptyLabel | ProjectError::LabelContainsNul | ProjectError::LabelTooLong => {
            localization::project_error_invalid_label()
        }
        ProjectError::StaleRevision { .. } => localization::project_error_stale(),
        ProjectError::AlreadyPresent { .. }
        | ProjectError::ResourceLimit { .. }
        | ProjectError::RevisionOverflow
        | ProjectError::Store { .. } => localization::project_error_generic(),
    }
}

fn project_status_presentation(status: ProjectStatus) -> (String, gpui::Hsla, IconName) {
    match status {
        ProjectStatus::Available => (
            localization::project_status_available(),
            theme::success(),
            IconName::Check,
        ),
        ProjectStatus::Unavailable => (
            localization::project_status_unavailable(),
            theme::warning(),
            IconName::TriangleAlert,
        ),
        ProjectStatus::PermissionDenied => (
            localization::project_status_permission_denied(),
            theme::danger(),
            IconName::TriangleAlert,
        ),
    }
}

fn project_element_key(id: ProjectId) -> u64 {
    let value = id.as_uuid().as_u128();
    (value as u64) ^ ((value >> 64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_errors_map_to_localized_non_sensitive_copy() {
        assert_eq!(
            project_error_message(&ProjectError::PermissionDenied),
            localization::project_error_permission_denied()
        );
        assert!(!project_error_message(&ProjectError::PathValidation).contains('/'));
    }

    #[test]
    fn project_status_always_has_text_color_and_shape() {
        for status in [
            ProjectStatus::Available,
            ProjectStatus::Unavailable,
            ProjectStatus::PermissionDenied,
        ] {
            let (text, _, _) = project_status_presentation(status);
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn every_store_failure_has_distinct_localized_recovery_copy() {
        let messages = [
            (
                ProjectStoreFailure::Corrupt,
                localization::project_store_corrupt(),
            ),
            (
                ProjectStoreFailure::Newer,
                localization::project_store_newer(),
            ),
            (
                ProjectStoreFailure::Unavailable,
                localization::project_store_unavailable(),
            ),
        ];
        for (failure, expected) in &messages {
            let state = ProjectLibraryState {
                repository: None,
                load_state: ProjectLibraryLoadState::Failed(*failure),
                snapshot: None,
                selected_id: None,
                add_draft: None,
                add_validation: None,
                pending_removal: None,
                next_validation_generation: 1,
            };
            assert_eq!(state.error_message().as_deref(), Some(expected.as_str()));
        }
        assert_ne!(messages[0].1, messages[1].1);
        assert_ne!(messages[1].1, messages[2].1);
    }

    #[test]
    fn undo_expiry_is_exactly_ten_seconds() {
        assert_eq!(PROJECT_UNDO_WINDOW, Duration::from_secs(10));
    }
}
