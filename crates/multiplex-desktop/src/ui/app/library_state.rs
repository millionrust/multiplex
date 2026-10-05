//! The library of groups and worktrees, and starting a session in a folder.
//!
//! Sessions are organised by group. They used to sit under a Project first, derived from the
//! folder they started in; the folder was the part that did the work, so a session now holds its
//! folder directly and nothing sits above the groups.

use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement, Styled, Window,
    px,
};
use gpui_component::button::Button;
use gpui_component::{Icon, IconName, h_flex, v_flex};
use multiplex_domain::{CanonicalPath, PresetId};
use multiplex_store::{LibraryRepository, LibrarySnapshot, StoreError, StoreHealth};

use super::{MultiplexApp, theme};
use crate::storage::library_store_dir;
use crate::ui::localization;

pub(super) enum LibraryLoadState {
    Loading,
    Ready,
    Failed(LibraryStoreFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LibraryStoreFailure {
    Corrupt,
    Newer,
    Unavailable,
}

pub(super) struct LibraryState {
    pub repository: Option<LibraryRepository>,
    pub load_state: LibraryLoadState,
    pub snapshot: Option<LibrarySnapshot>,
}

impl LibraryState {
    pub fn open_default() -> Self {
        let mut state = Self {
            repository: None,
            load_state: LibraryLoadState::Loading,
            snapshot: None,
        };
        let repository = library_store_dir()
            .map_err(|_| LibraryStoreFailure::Unavailable)
            .and_then(|root| LibraryRepository::open(root).map_err(classify_store_failure));
        match repository {
            Ok(repository) => {
                state.repository = Some(repository);
                state.reload();
            }
            Err(failure) => state.load_state = LibraryLoadState::Failed(failure),
        }
        state
    }

    pub fn reload(&mut self) {
        let Some(repository) = &self.repository else {
            self.load_state = LibraryLoadState::Failed(LibraryStoreFailure::Unavailable);
            self.snapshot = None;
            return;
        };
        match repository.load() {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                self.load_state = LibraryLoadState::Ready;
            }
            Err(error) => {
                self.snapshot = None;
                self.load_state = LibraryLoadState::Failed(classify_store_failure(error));
            }
        }
    }

    pub(super) fn error_message(&self) -> Option<String> {
        match &self.load_state {
            LibraryLoadState::Loading | LibraryLoadState::Ready => self
                .snapshot
                .as_ref()
                .filter(|snapshot| snapshot.health == StoreHealth::RecoveredLastGood)
                .map(|_| localization::library_store_recovered()),
            LibraryLoadState::Failed(LibraryStoreFailure::Corrupt) => {
                Some(localization::library_store_corrupt())
            }
            LibraryLoadState::Failed(LibraryStoreFailure::Newer) => {
                Some(localization::library_store_newer())
            }
            LibraryLoadState::Failed(LibraryStoreFailure::Unavailable) => {
                Some(localization::library_store_unavailable())
            }
        }
    }
}

impl MultiplexApp {
    pub(super) fn render_sessions_view(&self, cx: &Context<Self>) -> AnyElement {
        let content = match &self.library.load_state {
            LibraryLoadState::Loading => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(localization::library_loading())
                .into_any_element(),
            LibraryLoadState::Failed(_) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(theme::SPACE_4))
                .child(
                    self.library
                        .error_message()
                        .unwrap_or_else(localization::library_store_unavailable),
                )
                .child(
                    Button::new("sessions-store-retry")
                        .debug_selector(|| "sessions-store-retry".to_string())
                        .icon(IconName::Redo2)
                        .label(localization::common_retry())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.retry_library(window, cx);
                        })),
                )
                .into_any_element(),
            LibraryLoadState::Ready => self.render_session_sidebar(cx),
        };

        v_flex()
            .id("sessions-view")
            .debug_selector(|| "sessions-view".to_string())
            .track_focus(&self.session_list_focus)
            .flex_1()
            .min_h_0()
            .bg(theme::library_bg())
            .child(self.render_sessions_header(cx))
            // A worktree that was being made when the app stopped is kept as an intent; this is the
            // one place that offers to finish or forget it.
            .when_some(
                self.library
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

    /// Cuts a Git worktree from a repository the person picks.
    pub(super) fn start_worktree_in_a_folder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(test)]
        if let Some(selection) = crate::test_support::take_dialog_selection() {
            if let Some(path) = selection {
                self.open_worktree_launch_in_folder(path, window, cx);
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
                    app.open_worktree_launch_in_folder(path, window, cx);
                });
            });
        })
        .detach();
    }

    fn open_worktree_launch_in_folder(
        &mut self,
        folder: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match CanonicalPath::resolve(&folder) {
            Ok(folder) => self.open_worktree_launch(folder, window, cx),
            Err(_) => {
                self.error_message = localization::folder_unavailable();
                cx.notify();
            }
        }
    }

    /// Starts a session in a folder the person picks.
    pub(super) fn start_session_in_a_folder(
        &mut self,
        preset_id: Option<PresetId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(test)]
        if let Some(selection) = crate::test_support::take_dialog_selection() {
            if let Some(path) = selection {
                self.open_new_session_in_folder(path, preset_id, window, cx);
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
                    app.open_new_session_in_folder(path, preset_id, window, cx);
                });
            });
        })
        .detach();
    }

    pub(super) fn open_new_session_in_folder(
        &mut self,
        folder: PathBuf,
        preset_id: Option<PresetId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(folder) = CanonicalPath::resolve(&folder) else {
            self.error_message = localization::folder_unavailable();
            cx.notify();
            return;
        };
        match preset_id {
            Some(preset_id) => self.open_new_session_with_preset(folder, preset_id, window, cx),
            None => self.open_new_session(folder, window, cx),
        }
    }

    pub(super) fn retry_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.library.reload();
        self.repair_session_group_references();
        if matches!(self.library.load_state, LibraryLoadState::Ready) {
            self.error_message.clear();
            self.session_list_focus.focus(window);
        } else {
            self.error_message = self
                .library
                .error_message()
                .unwrap_or_else(localization::library_store_unavailable);
        }
        cx.notify();
    }
}

pub(super) fn classify_store_failure(error: StoreError) -> LibraryStoreFailure {
    match error {
        StoreError::StoreNewer { .. } => LibraryStoreFailure::Newer,
        StoreError::Corrupt { .. }
        | StoreError::UnsafeEntry { .. }
        | StoreError::TooLarge { .. } => LibraryStoreFailure::Corrupt,
        StoreError::Io { .. }
        | StoreError::InvalidInstanceId
        | StoreError::StaleRevision { .. }
        | StoreError::RevisionOverflow
        | StoreError::Domain(_)
        | StoreError::GroupDomain(_)
        | StoreError::PresetDomain(_)
        | StoreError::SessionDomain(_)
        | StoreError::WorktreeDomain(_) => LibraryStoreFailure::Unavailable,
    }
}
