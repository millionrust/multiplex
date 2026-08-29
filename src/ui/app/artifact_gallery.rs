use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ObjectFit, ParentElement as _,
    RenderImage, StatefulInteractiveElement as _, Styled, StyledImage as _, Window, div, img, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{
    Disableable as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use image::{Frame, RgbaImage};
use smallvec::SmallVec;
use termirust_domain::{
    ArtifactCancellation, ArtifactError, ArtifactId, ArtifactMediaType, ArtifactMetadata,
    ArtifactOrigin, ArtifactScope, ArtifactState, HostedSessionId,
};
use termirust_store::{
    ArtifactIngestProgress, ArtifactIngestRequest, ArtifactRepository, ArtifactSnapshot,
    ArtifactStoreError,
};

use super::{TermiRustApp, theme};
use crate::artifact_preview::{ArtifactPreview, build_preview};
use crate::models::SavedAppAttachedSession;
use crate::storage::app_dir;
use crate::ui::localization;
use crate::ui::util::{current_unix_millis, format_relative_time, format_size};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum GalleryLayout {
    #[default]
    List,
    Grid,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum FilesLibraryTab {
    #[default]
    Artifacts,
    Sftp,
}

#[derive(Clone)]
struct GlobalArtifactRow {
    session_id: HostedSessionId,
    project_label: String,
    session_title: String,
    preset_label: String,
    artifact: ArtifactMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArtifactOperationKind {
    Import,
    Preview,
    Export,
    Quarantine,
    Restore,
    Purge,
}

struct ArtifactOperation {
    kind: ArtifactOperationKind,
    session_id: HostedSessionId,
    artifact_id: ArtifactId,
    cancellation: ArtifactCancellation,
    progress: Option<ArtifactIngestProgress>,
    progress_rx: Option<Receiver<ArtifactIngestProgress>>,
}

impl fmt::Debug for ArtifactOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactOperation")
            .field("kind", &self.kind)
            .field("session_id", &self.session_id)
            .field("artifact_id", &self.artifact_id)
            .field("cancellation", &self.cancellation)
            .field("progress", &self.progress)
            .finish_non_exhaustive()
    }
}

struct ArtifactImportReview {
    session_id: HostedSessionId,
    source: PathBuf,
    display_name: String,
    byte_len: u64,
    session_used: u64,
    session_limit: u64,
}

impl fmt::Debug for ArtifactImportReview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactImportReview")
            .field("session_id", &self.session_id)
            .field("source", &"<redacted>")
            .field("display_name", &"<redacted>")
            .field("byte_len", &self.byte_len)
            .field("session_used", &self.session_used)
            .field("session_limit", &self.session_limit)
            .finish()
    }
}

enum ArtifactUiPreview {
    Text {
        artifact_id: ArtifactId,
        value: String,
        truncated: bool,
    },
    Raster {
        artifact_id: ArtifactId,
        image: Arc<RenderImage>,
        width: u32,
        height: u32,
    },
    MetadataOnly {
        artifact_id: ArtifactId,
    },
}

impl ArtifactUiPreview {
    fn artifact_id(&self) -> ArtifactId {
        match self {
            Self::Text { artifact_id, .. }
            | Self::Raster { artifact_id, .. }
            | Self::MetadataOnly { artifact_id } => *artifact_id,
        }
    }
}

impl fmt::Debug for ArtifactUiPreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text {
                artifact_id,
                value,
                truncated,
            } => formatter
                .debug_struct("ArtifactUiPreview::Text")
                .field("artifact_id", artifact_id)
                .field("value", &format_args!("<redacted:{} bytes>", value.len()))
                .field("truncated", truncated)
                .finish(),
            Self::Raster {
                artifact_id,
                width,
                height,
                ..
            } => formatter
                .debug_struct("ArtifactUiPreview::Raster")
                .field("artifact_id", artifact_id)
                .field("width", width)
                .field("height", height)
                .finish_non_exhaustive(),
            Self::MetadataOnly { artifact_id } => formatter
                .debug_struct("ArtifactUiPreview::MetadataOnly")
                .field("artifact_id", artifact_id)
                .finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArtifactNotice {
    Imported,
    Exported,
    Quarantined,
    Restored,
    Purged,
}

pub(super) struct ArtifactGalleryState {
    repository: Option<ArtifactRepository>,
    snapshots: HashMap<HostedSessionId, ArtifactSnapshot>,
    loading: Option<HostedSessionId>,
    generation: u64,
    layout: GalleryLayout,
    pending_import: Option<ArtifactImportReview>,
    operation: Option<ArtifactOperation>,
    preview: Option<ArtifactUiPreview>,
    metadata_expanded: Option<ArtifactId>,
    pending_purge: Option<(HostedSessionId, ArtifactId)>,
    notice: Option<ArtifactNotice>,
    error: Option<ArtifactError>,
    files_tab: FilesLibraryTab,
    global_loading: bool,
    global_generation: u64,
    global_selected_session: Option<HostedSessionId>,
}

impl ArtifactGalleryState {
    pub fn open_default() -> Self {
        let repository = app_dir()
            .map(|root| root.join("durable-sessions"))
            .ok()
            .and_then(|root| ArtifactRepository::open(root).ok());
        Self {
            repository,
            snapshots: HashMap::new(),
            loading: None,
            generation: 1,
            layout: GalleryLayout::List,
            pending_import: None,
            operation: None,
            preview: None,
            metadata_expanded: None,
            pending_purge: None,
            notice: None,
            error: None,
            files_tab: FilesLibraryTab::Artifacts,
            global_loading: false,
            global_generation: 1,
            global_selected_session: None,
        }
    }

    #[cfg(test)]
    fn with_repository(repository: ArtifactRepository) -> Self {
        let mut state = Self::open_default();
        state.repository = Some(repository);
        state
    }

    #[cfg(test)]
    pub(super) fn install_test_snapshots(&mut self, snapshots: Vec<ArtifactSnapshot>) {
        self.snapshots = snapshots
            .into_iter()
            .map(|snapshot| (snapshot.scope.session_id, snapshot))
            .collect();
        self.files_tab = FilesLibraryTab::Artifacts;
        self.global_loading = false;
        self.global_selected_session = self.snapshots.keys().copied().next();
    }
}

impl TermiRustApp {
    pub(super) fn open_files_library(
        &mut self,
        tab: FilesLibraryTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.artifact_gallery.files_tab = tab;
        self.activate_library_section(super::NavSection::Sftp, window, cx);
        if tab == FilesLibraryTab::Artifacts {
            self.refresh_global_artifacts(cx);
        }
    }

    fn set_files_library_tab(&mut self, tab: FilesLibraryTab, cx: &mut Context<Self>) {
        self.artifact_gallery.files_tab = tab;
        if tab == FilesLibraryTab::Artifacts {
            self.refresh_global_artifacts(cx);
        } else {
            cx.notify();
        }
    }

    fn refresh_global_artifacts(&mut self, cx: &mut Context<Self>) {
        let Some(repository) = self.artifact_gallery.repository.clone() else {
            self.artifact_gallery.error = Some(ArtifactError::Unavailable);
            cx.notify();
            return;
        };
        let session_ids = self
            .saved
            .app_attached_sessions
            .iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        self.artifact_gallery.global_generation = self
            .artifact_gallery
            .global_generation
            .wrapping_add(1)
            .max(1);
        let generation = self.artifact_gallery.global_generation;
        self.artifact_gallery.global_loading = true;
        self.artifact_gallery.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    session_ids
                        .into_iter()
                        .map(|session_id| {
                            let result = repository
                                .list(ArtifactScope { session_id })
                                .map_err(domain_error);
                            (session_id, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.artifact_gallery.global_generation != generation {
                    return;
                }
                app.artifact_gallery.global_loading = false;
                let authoritative = app
                    .saved
                    .app_attached_sessions
                    .iter()
                    .map(|session| session.id)
                    .collect::<std::collections::HashSet<_>>();
                app.artifact_gallery
                    .snapshots
                    .retain(|session_id, _| authoritative.contains(session_id));
                for (session_id, result) in results {
                    match result {
                        Ok(snapshot) => {
                            app.artifact_gallery.snapshots.insert(session_id, snapshot);
                        }
                        Err(error) if app.artifact_gallery.error.is_none() => {
                            app.artifact_gallery.error = Some(error);
                        }
                        Err(_) => {}
                    }
                }
                let rows = global_artifact_rows(
                    &app.saved.app_attached_sessions,
                    &app.artifact_gallery.snapshots,
                );
                let selected_exists = app
                    .artifact_gallery
                    .global_selected_session
                    .is_some_and(|selected| rows.iter().any(|row| row.session_id == selected));
                if !selected_exists {
                    app.artifact_gallery.global_selected_session =
                        rows.first().map(|row| row.session_id);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_global_artifact_session(
        &mut self,
        session_id: HostedSessionId,
        cx: &mut Context<Self>,
    ) {
        self.artifact_gallery.global_selected_session = Some(session_id);
        self.refresh_artifacts(session_id, cx);
    }

    pub(super) fn render_files_artifacts_view(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .id("files-artifacts-view")
            .debug_selector(|| "files-artifacts-view".to_string())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SPACE_3))
                    .px(px(theme::SPACE_5))
                    .py(px(theme::SPACE_4))
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap(px(theme::SPACE_2))
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_HEADING_SIZE))
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child("Files / Artifacts"),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(
                                        "Browse live files separately from durable Session artifacts.",
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_2))
                            .child(
                                Button::new("files-tab-artifacts")
                                    .debug_selector(|| "files-tab-artifacts".to_string())
                                    .small()
                                    .icon(IconName::File)
                                    .selected(
                                        self.artifact_gallery.files_tab
                                            == FilesLibraryTab::Artifacts,
                                    )
                                    .label("Session artifacts")
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.set_files_library_tab(
                                            FilesLibraryTab::Artifacts,
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new("files-tab-sftp")
                                    .debug_selector(|| "files-tab-sftp".to_string())
                                    .small()
                                    .icon(IconName::Folder)
                                    .selected(
                                        self.artifact_gallery.files_tab == FilesLibraryTab::Sftp,
                                    )
                                    .label("SFTP files")
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.set_files_library_tab(FilesLibraryTab::Sftp, cx);
                                    })),
                            ),
                    ),
            )
            .child(match self.artifact_gallery.files_tab {
                FilesLibraryTab::Artifacts => self.render_global_artifact_index(cx),
                FilesLibraryTab::Sftp => self.render_sftp_view(cx).into_any_element(),
            })
            .into_any_element()
    }

    fn render_global_artifact_index(&self, cx: &Context<Self>) -> AnyElement {
        let rows = global_artifact_rows(
            &self.saved.app_attached_sessions,
            &self.artifact_gallery.snapshots,
        );
        let selected_session = self
            .artifact_gallery
            .global_selected_session
            .filter(|selected| rows.iter().any(|row| row.session_id == *selected))
            .or_else(|| rows.first().map(|row| row.session_id));

        h_flex()
            .id("global-artifact-index")
            .debug_selector(|| "global-artifact-index".to_string())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_start()
            .child(
                v_flex()
                    .w(px(360.))
                    .max_w(px(420.))
                    .min_w(px(280.))
                    .h_full()
                    .min_h_0()
                    .border_r_1()
                    .border_color(theme::border())
                    .child(
                        h_flex()
                            .justify_between()
                            .px(px(theme::SPACE_4))
                            .py(px(theme::SPACE_3))
                            .border_b_1()
                            .border_color(theme::soft_border())
                            .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                            .text_color(theme::text_muted())
                            .child("All authoritative Sessions")
                            .child(format!("{} artifacts", rows.len())),
                    )
                    .when(self.artifact_gallery.global_loading, |this| {
                        this.child(
                            div()
                                .debug_selector(|| "global-artifacts-loading".to_string())
                                .p(px(theme::SPACE_4))
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .text_color(theme::text_muted())
                            .child(localization::artifact_gallery_loading()),
                        )
                    })
                    .when_some(
                        self.artifact_gallery
                            .error
                            .filter(|_| selected_session.is_none()),
                        |this, error| {
                            this.child(
                                h_flex()
                                    .debug_selector(|| "global-artifacts-error".to_string())
                                    .items_start()
                                    .gap(px(theme::SPACE_2))
                                    .m(px(theme::SPACE_4))
                                    .p(px(theme::SPACE_3))
                                    .border_1()
                                    .border_color(theme::danger())
                                    .rounded(px(theme::CARD_RADIUS))
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .text_color(theme::danger())
                                    .child(
                                        Icon::new(IconName::TriangleAlert)
                                            .size(px(theme::SPACE_4)),
                                    )
                                    .child(div().min_w_0().child(artifact_error_label(error))),
                            )
                        },
                    )
                    .when(
                        !self.artifact_gallery.global_loading
                            && self.artifact_gallery.error.is_none()
                            && rows.is_empty(),
                        |this| {
                            this.child(
                                div()
                                    .debug_selector(|| "global-artifacts-empty".to_string())
                                    .p(px(theme::SPACE_5))
                                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(
                                        "No Session artifacts yet. Select a Session and import a file to keep it here.",
                                    ),
                            )
                        },
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .children(rows.iter().enumerate().map(|(index, row)| {
                                let session_id = row.session_id;
                                let selected = selected_session == Some(session_id);
                                v_flex()
                                    .id(("global-artifact-row", index))
                                    .debug_selector(|| "global-artifact-row".to_string())
                                    .gap(px(theme::SPACE_2))
                                    .px(px(theme::SPACE_4))
                                    .py(px(theme::SPACE_3))
                                    .border_b_1()
                                    .border_color(theme::soft_border())
                                    .bg(if selected {
                                        theme::accent_soft()
                                    } else {
                                        gpui::transparent_black()
                                    })
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::hover()))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.select_global_artifact_session(session_id, cx);
                                    }))
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .gap(px(theme::SPACE_2))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .font_medium()
                                                    .text_color(theme::text_main())
                                                    .child(
                                                        row.artifact
                                                            .display_name
                                                            .as_str()
                                                            .to_string(),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                    .text_color(artifact_state_color(
                                                        row.artifact.state,
                                                    ))
                                                    .child(artifact_state_label(
                                                        row.artifact.state,
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .debug_selector(|| {
                                                "global-artifact-origin".to_string()
                                            })
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .child(format!(
                                                "{} · {} · {}",
                                                artifact_type_label(row.artifact.media_type),
                                                format_size(row.artifact.byte_len),
                                                artifact_origin_label(row.artifact.origin),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .debug_selector(|| {
                                                "global-artifact-project".to_string()
                                            })
                                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                                            .text_color(theme::text_muted())
                                            .truncate()
                                            .child(format!(
                                                "{} / {}",
                                                row.project_label, row.session_title
                                            )),
                                    )
                                    .when(!row.preset_label.is_empty(), |this| {
                                        this.child(
                                            div()
                                                .debug_selector(|| {
                                                    "global-artifact-preset".to_string()
                                                })
                                                .text_size(px(theme::TYPE_CAPTION_SIZE))
                                                .text_color(theme::text_muted())
                                                .truncate()
                                                .child(row.preset_label.clone()),
                                        )
                                    })
                                    .into_any_element()
                            })),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p(px(theme::SPACE_5))
                    .when_some(selected_session, |this, session_id| {
                        this.child(self.render_artifact_gallery(session_id, cx))
                    })
                    .when(selected_session.is_none(), |this| {
                        this.child(
                            div()
                                .p(px(theme::SPACE_5))
                                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                                .text_color(theme::text_muted())
                                .child(
                                    "Artifact actions appear here after an artifact is available.",
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    pub(super) fn refresh_artifacts(
        &mut self,
        session_id: HostedSessionId,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self.artifact_gallery.repository.clone() else {
            self.artifact_gallery.error = Some(ArtifactError::Unavailable);
            cx.notify();
            return;
        };
        self.artifact_gallery.generation = self.artifact_gallery.generation.wrapping_add(1).max(1);
        let generation = self.artifact_gallery.generation;
        self.artifact_gallery.loading = Some(session_id);
        self.artifact_gallery.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { repository.list(ArtifactScope { session_id }) })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.artifact_gallery.generation != generation {
                    return;
                }
                app.artifact_gallery.loading = None;
                match result {
                    Ok(snapshot) => {
                        app.artifact_gallery.snapshots.insert(session_id, snapshot);
                        app.artifact_gallery.error = None;
                    }
                    Err(error) => app.artifact_gallery.error = Some(domain_error(error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn process_artifact_progress(&mut self, cx: &mut Context<Self>) {
        let Some(operation) = self.artifact_gallery.operation.as_mut() else {
            return;
        };
        let Some(receiver) = operation.progress_rx.as_ref() else {
            return;
        };
        let mut changed = false;
        while let Ok(progress) = receiver.try_recv() {
            operation.progress = Some(progress);
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    pub(super) fn choose_artifact_import(
        &mut self,
        session_id: HostedSessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(test)]
        if let Some(selection) = crate::test_support::take_dialog_selection() {
            if let Some(path) = selection {
                self.review_artifact_import(session_id, path, cx);
            }
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = rfd::AsyncFileDialog::new()
                .set_title(localization::artifact_import_picker_title())
                .pick_file()
                .await
                .map(|file| file.path().to_path_buf())
            else {
                return;
            };
            let _ = cx.update(|_, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.review_artifact_import(session_id, path, cx);
                });
            });
        })
        .detach();
    }

    fn review_artifact_import(
        &mut self,
        session_id: HostedSessionId,
        source: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self.artifact_gallery.repository.clone() else {
            self.artifact_gallery.error = Some(ArtifactError::Unavailable);
            cx.notify();
            return;
        };
        self.artifact_gallery.error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let metadata = fs::symlink_metadata(&source)
                        .map_err(|error| map_review_io(error.kind()))?;
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        return Err(ArtifactError::UnsupportedSource);
                    }
                    let display_name = source
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("artifact")
                        .to_string();
                    let snapshot = repository
                        .list(ArtifactScope { session_id })
                        .map_err(domain_error)?;
                    let limits = repository.limits();
                    if metadata.len() > limits.item_bytes
                        || snapshot.session_bytes.saturating_add(metadata.len())
                            > limits.session_bytes
                        || snapshot.global_bytes.saturating_add(metadata.len())
                            > limits.global_bytes
                    {
                        return Err(ArtifactError::ItemQuotaExceeded);
                    }
                    Ok(ArtifactImportReview {
                        session_id,
                        source,
                        display_name,
                        byte_len: metadata.len(),
                        session_used: snapshot.session_bytes,
                        session_limit: snapshot.session_limit,
                    })
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                match result {
                    Ok(review) => app.artifact_gallery.pending_import = Some(review),
                    Err(error) => app.artifact_gallery.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel_artifact_import_review(&mut self, cx: &mut Context<Self>) {
        self.artifact_gallery.pending_import = None;
        cx.notify();
    }

    fn confirm_artifact_import(&mut self, cx: &mut Context<Self>) {
        let Some(review) = self.artifact_gallery.pending_import.take() else {
            return;
        };
        let Some(repository) = self.artifact_gallery.repository.clone() else {
            self.artifact_gallery.error = Some(ArtifactError::Unavailable);
            cx.notify();
            return;
        };
        let artifact_id = ArtifactId::new();
        let session_id = review.session_id;
        let cancellation = ArtifactCancellation::default();
        let worker_cancellation = cancellation.clone();
        let (progress_tx, progress_rx) = mpsc::channel();
        self.artifact_gallery.operation = Some(ArtifactOperation {
            kind: ArtifactOperationKind::Import,
            session_id,
            artifact_id,
            cancellation,
            progress: None,
            progress_rx: Some(progress_rx),
        });
        self.artifact_gallery.error = None;
        self.artifact_gallery.notice = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository.ingest(
                        ArtifactIngestRequest {
                            id: artifact_id,
                            scope: ArtifactScope { session_id },
                            source: review.source,
                            display_name: Some(review.display_name),
                            created_at: current_unix_millis(),
                        },
                        &worker_cancellation,
                        move |progress| {
                            let _ = progress_tx.send(progress);
                        },
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.finish_artifact_operation(
                    session_id,
                    result.map(|_| ArtifactNotice::Imported),
                    cx,
                );
            });
        })
        .detach();
    }

    fn cancel_artifact_operation(&mut self, cx: &mut Context<Self>) {
        if let Some(operation) = self.artifact_gallery.operation.as_ref() {
            operation.cancellation.cancel();
        }
        cx.notify();
    }

    fn request_artifact_preview(
        &mut self,
        session_id: HostedSessionId,
        artifact_id: ArtifactId,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self.artifact_gallery.repository.clone() else {
            self.artifact_gallery.error = Some(ArtifactError::Unavailable);
            cx.notify();
            return;
        };
        let cancellation = ArtifactCancellation::default();
        let worker_cancellation = cancellation.clone();
        self.artifact_gallery.operation = Some(ArtifactOperation {
            kind: ArtifactOperationKind::Preview,
            session_id,
            artifact_id,
            cancellation,
            progress: None,
            progress_rx: None,
        });
        self.artifact_gallery.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let payload = repository
                        .read_payload(
                            ArtifactScope { session_id },
                            artifact_id,
                            &worker_cancellation,
                        )
                        .map_err(domain_error)?;
                    build_preview(&payload, repository.limits(), &worker_cancellation)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.artifact_gallery.operation = None;
                match result.and_then(|preview| ui_preview(artifact_id, preview)) {
                    Ok(preview) => {
                        app.artifact_gallery.preview = Some(preview);
                        app.artifact_gallery.error = None;
                    }
                    Err(error) => app.artifact_gallery.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_artifact_export(
        &mut self,
        session_id: HostedSessionId,
        artifact_id: ArtifactId,
        display_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let Some(destination) = rfd::AsyncFileDialog::new()
                .set_title(localization::artifact_export_picker_title())
                .set_file_name(display_name)
                .save_file()
                .await
                .map(|file| file.path().to_path_buf())
            else {
                return;
            };
            let _ = cx.update(|_, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.start_artifact_export(session_id, artifact_id, destination, cx);
                });
            });
        })
        .detach();
    }

    fn start_artifact_export(
        &mut self,
        session_id: HostedSessionId,
        artifact_id: ArtifactId,
        destination: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self.artifact_gallery.repository.clone() else {
            self.artifact_gallery.error = Some(ArtifactError::Unavailable);
            cx.notify();
            return;
        };
        let cancellation = ArtifactCancellation::default();
        let worker_cancellation = cancellation.clone();
        self.artifact_gallery.operation = Some(ArtifactOperation {
            kind: ArtifactOperationKind::Export,
            session_id,
            artifact_id,
            cancellation,
            progress: None,
            progress_rx: None,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository.export_copy(
                        ArtifactScope { session_id },
                        artifact_id,
                        &destination,
                        &worker_cancellation,
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.finish_artifact_operation(
                    session_id,
                    result.map(|_| ArtifactNotice::Exported),
                    cx,
                );
            });
        })
        .detach();
    }

    fn start_artifact_mutation(
        &mut self,
        session_id: HostedSessionId,
        artifact_id: ArtifactId,
        kind: ArtifactOperationKind,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self.artifact_gallery.repository.clone() else {
            self.artifact_gallery.error = Some(ArtifactError::Unavailable);
            cx.notify();
            return;
        };
        let cancellation = ArtifactCancellation::default();
        let worker_cancellation = cancellation.clone();
        self.artifact_gallery.operation = Some(ArtifactOperation {
            kind,
            session_id,
            artifact_id,
            cancellation,
            progress: None,
            progress_rx: None,
        });
        self.artifact_gallery.pending_purge = None;
        self.artifact_gallery.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let scope = ArtifactScope { session_id };
                    match kind {
                        ArtifactOperationKind::Quarantine => repository
                            .quarantine(scope, artifact_id)
                            .map(|_| ArtifactNotice::Quarantined),
                        ArtifactOperationKind::Restore => repository
                            .restore(scope, artifact_id, &worker_cancellation)
                            .map(|_| ArtifactNotice::Restored),
                        ArtifactOperationKind::Purge => repository
                            .purge(scope, artifact_id, &worker_cancellation)
                            .map(|_| ArtifactNotice::Purged),
                        ArtifactOperationKind::Import
                        | ArtifactOperationKind::Preview
                        | ArtifactOperationKind::Export => Err(ArtifactError::InvalidState.into()),
                    }
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.finish_artifact_operation(session_id, result, cx);
            });
        })
        .detach();
    }

    fn finish_artifact_operation(
        &mut self,
        session_id: HostedSessionId,
        result: Result<ArtifactNotice, ArtifactStoreError>,
        cx: &mut Context<Self>,
    ) {
        self.artifact_gallery.operation = None;
        match result {
            Ok(notice) => {
                self.artifact_gallery.notice = Some(notice);
                self.artifact_gallery.error = None;
                if matches!(notice, ArtifactNotice::Quarantined | ArtifactNotice::Purged) {
                    self.artifact_gallery.preview = None;
                }
            }
            Err(error) => {
                self.artifact_gallery.error = Some(domain_error(error));
                self.artifact_gallery.notice = None;
            }
        }
        self.refresh_artifacts(session_id, cx);
    }

    fn toggle_artifact_metadata(&mut self, artifact_id: ArtifactId, cx: &mut Context<Self>) {
        self.artifact_gallery.metadata_expanded =
            (self.artifact_gallery.metadata_expanded != Some(artifact_id)).then_some(artifact_id);
        cx.notify();
    }

    fn request_artifact_purge(
        &mut self,
        session_id: HostedSessionId,
        artifact_id: ArtifactId,
        cx: &mut Context<Self>,
    ) {
        self.artifact_gallery.pending_purge = Some((session_id, artifact_id));
        cx.notify();
    }

    fn cancel_artifact_purge(&mut self, cx: &mut Context<Self>) {
        self.artifact_gallery.pending_purge = None;
        cx.notify();
    }

    pub(super) fn render_artifact_gallery(
        &self,
        session_id: HostedSessionId,
        cx: &Context<Self>,
    ) -> AnyElement {
        let snapshot = self.artifact_gallery.snapshots.get(&session_id);
        let loading = self.artifact_gallery.loading == Some(session_id);
        let busy = self
            .artifact_gallery
            .operation
            .as_ref()
            .is_some_and(|operation| operation.session_id == session_id);
        let artifacts = snapshot
            .map(|snapshot| snapshot.artifacts.as_slice())
            .unwrap_or_default();
        v_flex()
            .id("artifact-gallery")
            .debug_selector(|| "artifact-gallery".to_string())
            .gap(px(theme::SPACE_3))
            .pt(px(theme::SPACE_3))
            .border_t_1()
            .border_color(theme::soft_border())
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .items_center()
                    .gap(px(theme::SPACE_3))
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap(px(theme::SPACE_2))
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(theme::text_main())
                                    .child(localization::artifact_gallery_title()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::artifact_gallery_description()),
                            ),
                    )
                    .child(
                        Button::new("artifact-import")
                            .debug_selector(|| "artifact-import".to_string())
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .disabled(busy || self.artifact_gallery.repository.is_none())
                            .label(localization::artifact_import_action())
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.choose_artifact_import(session_id, window, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SPACE_2))
                    .child(
                        div()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(snapshot.map_or_else(
                                localization::artifact_quota_unavailable,
                                |snapshot| {
                                    localization::artifact_quota_summary(
                                        format_size(snapshot.session_bytes),
                                        format_size(snapshot.session_limit),
                                    )
                                },
                            )),
                    )
                    .child(
                        h_flex()
                            .gap(px(theme::SPACE_2))
                            .child(
                                Button::new("artifact-layout-list")
                                    .debug_selector(|| "artifact-layout-list".to_string())
                                    .small()
                                    .icon(IconName::Menu)
                                    .selected(self.artifact_gallery.layout == GalleryLayout::List)
                                    .tooltip(localization::artifact_layout_list())
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.artifact_gallery.layout = GalleryLayout::List;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("artifact-layout-grid")
                                    .debug_selector(|| "artifact-layout-grid".to_string())
                                    .small()
                                    .icon(IconName::LayoutDashboard)
                                    .selected(self.artifact_gallery.layout == GalleryLayout::Grid)
                                    .tooltip(localization::artifact_layout_grid())
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.artifact_gallery.layout = GalleryLayout::Grid;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .when_some(
                self.artifact_gallery.pending_import.as_ref(),
                |this, review| {
                    if review.session_id == session_id {
                        this.child(self.render_artifact_import_review(review, cx))
                    } else {
                        this
                    }
                },
            )
            .when_some(
                self.artifact_gallery.operation.as_ref(),
                |this, operation| {
                    if operation.session_id == session_id {
                        this.child(self.render_artifact_operation(operation, cx))
                    } else {
                        this
                    }
                },
            )
            .when_some(self.artifact_gallery.error, |this, error| {
                this.child(
                    h_flex()
                        .id("artifact-error")
                        .debug_selector(|| "artifact-error".to_string())
                        .items_start()
                        .gap(px(theme::SPACE_2))
                        .p(px(theme::SPACE_3))
                        .border_1()
                        .border_color(theme::danger())
                        .rounded(px(theme::CARD_RADIUS))
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::danger())
                        .child(Icon::new(IconName::TriangleAlert).size(px(theme::SPACE_4)))
                        .child(div().min_w_0().child(artifact_error_label(error))),
                )
            })
            .when_some(self.artifact_gallery.notice, |this, notice| {
                this.child(
                    div()
                        .id("artifact-notice")
                        .debug_selector(|| "artifact-notice".to_string())
                        .p(px(theme::SPACE_3))
                        .border_1()
                        .border_color(theme::success())
                        .rounded(px(theme::CARD_RADIUS))
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::success())
                        .child(artifact_notice_label(notice)),
                )
            })
            .when(loading, |this| {
                this.child(
                    div()
                        .id("artifact-loading")
                        .debug_selector(|| "artifact-loading".to_string())
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::artifact_gallery_loading()),
                )
            })
            .when(!loading && artifacts.is_empty(), |this| {
                this.child(
                    div()
                        .id("artifact-empty")
                        .debug_selector(|| "artifact-empty".to_string())
                        .p(px(theme::SPACE_4))
                        .border_1()
                        .border_color(theme::soft_border())
                        .rounded(px(theme::CARD_RADIUS))
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::artifact_gallery_empty()),
                )
            })
            .child(
                h_flex()
                    .items_start()
                    .flex_wrap()
                    .gap(px(theme::SPACE_3))
                    .children(artifacts.iter().enumerate().map(|(index, artifact)| {
                        self.render_artifact_card(
                            session_id,
                            artifact,
                            index,
                            artifacts.len(),
                            busy,
                            cx,
                        )
                    })),
            )
            .into_any_element()
    }

    fn render_artifact_import_review(
        &self,
        review: &ArtifactImportReview,
        cx: &Context<Self>,
    ) -> AnyElement {
        v_flex()
            .id("artifact-import-review")
            .debug_selector(|| "artifact-import-review".to_string())
            .gap(px(theme::SPACE_2))
            .p(px(theme::SPACE_3))
            .border_1()
            .border_color(theme::accent())
            .rounded(px(theme::CARD_RADIUS))
            .child(
                div()
                    .font_semibold()
                    .text_color(theme::text_main())
                    .child(localization::artifact_import_review_title()),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .text_color(theme::text_main())
                    .child(localization::artifact_import_review_file(
                        review.display_name.clone(),
                        format_size(review.byte_len),
                    )),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::artifact_import_review_quota(
                        format_size(review.session_used),
                        format_size(review.session_limit),
                    )),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(theme::text_muted())
                    .child(localization::artifact_import_source_preserved()),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(theme::SPACE_2))
                    .child(
                        Button::new("artifact-import-confirm")
                            .debug_selector(|| "artifact-import-confirm".to_string())
                            .small()
                            .primary()
                            .label(localization::artifact_import_confirm())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.confirm_artifact_import(cx);
                            })),
                    )
                    .child(
                        Button::new("artifact-import-cancel")
                            .debug_selector(|| "artifact-import-cancel".to_string())
                            .small()
                            .label(localization::common_cancel())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.cancel_artifact_import_review(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_artifact_operation(
        &self,
        operation: &ArtifactOperation,
        cx: &Context<Self>,
    ) -> AnyElement {
        let label = artifact_operation_label(operation);
        h_flex()
            .flex_wrap()
            .id("artifact-operation")
            .debug_selector(|| "artifact-operation".to_string())
            .items_center()
            .justify_between()
            .gap(px(theme::SPACE_3))
            .p(px(theme::SPACE_3))
            .border_1()
            .border_color(theme::accent())
            .rounded(px(theme::CARD_RADIUS))
            .child(
                div()
                    .min_w_0()
                    .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                    .text_color(theme::text_main())
                    .child(label),
            )
            .child(
                Button::new("artifact-operation-cancel")
                    .debug_selector(|| "artifact-operation-cancel".to_string())
                    .small()
                    .label(localization::common_cancel())
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.cancel_artifact_operation(cx);
                    })),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_artifact_card(
        &self,
        session_id: HostedSessionId,
        artifact: &ArtifactMetadata,
        index: usize,
        count: usize,
        busy: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let artifact_id = artifact.id;
        let key = index;
        let show_metadata = self.artifact_gallery.metadata_expanded == Some(artifact_id);
        let pending_purge = self.artifact_gallery.pending_purge == Some((session_id, artifact_id));
        let preview = self
            .artifact_gallery
            .preview
            .as_ref()
            .filter(|preview| preview.artifact_id() == artifact_id);
        v_flex()
            .id(("artifact-card", key))
            .debug_selector(|| "artifact-card".to_string())
            .min_w_0()
            .max_w_full()
            .when(
                self.artifact_gallery.layout == GalleryLayout::List,
                |this| this.w_full(),
            )
            .when(
                self.artifact_gallery.layout == GalleryLayout::Grid,
                |this| this.w(px(260.0)).flex_grow(),
            )
            .gap(px(theme::SPACE_3))
            .p(px(theme::SPACE_3))
            .border_1()
            .border_color(artifact_state_color(artifact.state))
            .rounded(px(theme::CARD_RADIUS))
            .bg(theme::library_bg())
            .child(
                h_flex()
                    .items_start()
                    .justify_between()
                    .gap(px(theme::SPACE_3))
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap(px(theme::SPACE_2))
                            .child(
                                div()
                                    .font_medium()
                                    .text_color(theme::text_main())
                                    .truncate()
                                    .child(artifact.display_name.as_str().to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                                    .text_color(theme::text_muted())
                                    .child(localization::artifact_card_summary(
                                        artifact_type_label(artifact.media_type),
                                        format_size(artifact.byte_len),
                                        artifact_state_label(artifact.state),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TYPE_CAPTION_SIZE))
                            .text_color(theme::text_muted())
                            .child(localization::artifact_position(index + 1, count)),
                    ),
            )
            .child(
                div()
                    .text_size(px(theme::TYPE_CAPTION_SIZE))
                    .text_color(artifact_state_color(artifact.state))
                    .child(artifact_preview_label(artifact)),
            )
            .when_some(preview, |this, preview| {
                this.child(self.render_artifact_preview(preview))
            })
            .when(show_metadata, |this| {
                this.child(
                    v_flex()
                        .id(("artifact-metadata", key))
                        .debug_selector(|| "artifact-metadata".to_string())
                        .gap(px(theme::SPACE_2))
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(artifact_detail_row(
                            localization::artifact_origin_label(),
                            artifact_origin_label(artifact.origin),
                        ))
                        .child(artifact_detail_row(
                            localization::artifact_created_label(),
                            format_relative_time(artifact.created_at),
                        ))
                        .child(artifact_detail_row(
                            localization::artifact_hash_label(),
                            artifact.sha256.short_label(),
                        )),
                )
            })
            .when(pending_purge, |this| {
                this.child(
                    v_flex()
                        .id(("artifact-purge-review", key))
                        .debug_selector(|| "artifact-purge-review".to_string())
                        .gap(px(theme::SPACE_2))
                        .p(px(theme::SPACE_3))
                        .border_1()
                        .border_color(theme::danger())
                        .rounded(px(theme::CARD_RADIUS))
                        .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                        .text_color(theme::danger())
                        .child(localization::artifact_purge_warning())
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap(px(theme::SPACE_2))
                                .child(
                                    Button::new(("artifact-purge-confirm", key))
                                        .debug_selector(|| "artifact-purge-confirm".to_string())
                                        .small()
                                        .danger()
                                        .label(localization::artifact_purge_confirm())
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.start_artifact_mutation(
                                                session_id,
                                                artifact_id,
                                                ArtifactOperationKind::Purge,
                                                cx,
                                            );
                                        })),
                                )
                                .child(
                                    Button::new(("artifact-purge-cancel", key))
                                        .debug_selector(|| "artifact-purge-cancel".to_string())
                                        .small()
                                        .label(localization::common_cancel())
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.cancel_artifact_purge(cx);
                                        })),
                                ),
                        ),
                )
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(theme::SPACE_2))
                    .child(
                        Button::new(("artifact-preview", key))
                            .debug_selector(|| "artifact-preview".to_string())
                            .small()
                            .icon(IconName::Eye)
                            .disabled(
                                busy || artifact.state != ArtifactState::Ready
                                    || artifact.media_type == ArtifactMediaType::MetadataOnly,
                            )
                            .label(localization::artifact_preview_action())
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.request_artifact_preview(session_id, artifact_id, cx);
                            })),
                    )
                    .child(
                        Button::new(("artifact-export", key))
                            .debug_selector(|| "artifact-export".to_string())
                            .small()
                            .icon(IconName::ArrowDown)
                            .disabled(busy || artifact.state == ArtifactState::Corrupt)
                            .label(localization::artifact_export_action())
                            .on_click({
                                let display_name = artifact.display_name.as_str().to_string();
                                cx.listener(move |app, _, window, cx| {
                                    app.choose_artifact_export(
                                        session_id,
                                        artifact_id,
                                        display_name.clone(),
                                        window,
                                        cx,
                                    );
                                })
                            }),
                    )
                    .child(
                        Button::new(("artifact-metadata-toggle", key))
                            .debug_selector(|| "artifact-metadata-toggle".to_string())
                            .small()
                            .label(if show_metadata {
                                localization::artifact_hide_metadata_action()
                            } else {
                                localization::artifact_show_metadata_action()
                            })
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.toggle_artifact_metadata(artifact_id, cx);
                            })),
                    )
                    .when(artifact.state == ArtifactState::Ready, |this| {
                        this.child(
                            Button::new(("artifact-quarantine", key))
                                .debug_selector(|| "artifact-quarantine".to_string())
                                .small()
                                .danger()
                                .disabled(busy)
                                .label(localization::artifact_quarantine_action())
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.start_artifact_mutation(
                                        session_id,
                                        artifact_id,
                                        ArtifactOperationKind::Quarantine,
                                        cx,
                                    );
                                })),
                        )
                    })
                    .when(artifact.state == ArtifactState::Quarantined, |this| {
                        this.child(
                            Button::new(("artifact-restore", key))
                                .debug_selector(|| "artifact-restore".to_string())
                                .small()
                                .disabled(busy)
                                .label(localization::artifact_restore_action())
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.start_artifact_mutation(
                                        session_id,
                                        artifact_id,
                                        ArtifactOperationKind::Restore,
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            Button::new(("artifact-purge", key))
                                .debug_selector(|| "artifact-purge".to_string())
                                .small()
                                .danger()
                                .disabled(busy)
                                .label(localization::artifact_purge_action())
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.request_artifact_purge(session_id, artifact_id, cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_artifact_preview(&self, preview: &ArtifactUiPreview) -> AnyElement {
        match preview {
            ArtifactUiPreview::Text {
                value, truncated, ..
            } => v_flex()
                .id("artifact-text-preview")
                .debug_selector(|| "artifact-text-preview".to_string())
                .max_h(px(240.0))
                .overflow_y_scroll()
                .gap(px(theme::SPACE_2))
                .p(px(theme::SPACE_3))
                .border_1()
                .border_color(theme::soft_border())
                .rounded(px(theme::CARD_RADIUS))
                .bg(theme::terminal_bg())
                .font_family("monospace")
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .text_color(theme::text_main())
                .child(value.clone())
                .when(*truncated, |this| {
                    this.child(
                        div()
                            .text_color(theme::warning())
                            .child(localization::artifact_preview_truncated()),
                    )
                })
                .into_any_element(),
            ArtifactUiPreview::Raster {
                image,
                width,
                height,
                ..
            } => v_flex()
                .id("artifact-raster-preview")
                .debug_selector(|| "artifact-raster-preview".to_string())
                .gap(px(theme::SPACE_2))
                .child(
                    img(image.clone())
                        .w_full()
                        .h(px(180.0))
                        .object_fit(ObjectFit::Contain),
                )
                .child(
                    div()
                        .text_size(px(theme::TYPE_CAPTION_SIZE))
                        .text_color(theme::text_muted())
                        .child(localization::artifact_preview_dimensions(
                            usize::try_from(*width).unwrap_or(usize::MAX),
                            usize::try_from(*height).unwrap_or(usize::MAX),
                        )),
                )
                .into_any_element(),
            ArtifactUiPreview::MetadataOnly { .. } => div()
                .text_size(px(theme::TYPE_BODY_SMALL_SIZE))
                .text_color(theme::text_muted())
                .child(localization::artifact_preview_metadata_only())
                .into_any_element(),
        }
    }
}

fn ui_preview(
    artifact_id: ArtifactId,
    preview: ArtifactPreview,
) -> Result<ArtifactUiPreview, ArtifactError> {
    match preview {
        ArtifactPreview::Text { value, truncated } => Ok(ArtifactUiPreview::Text {
            artifact_id,
            value,
            truncated,
        }),
        ArtifactPreview::Raster {
            width,
            height,
            rgba,
        } => {
            let buffer =
                RgbaImage::from_raw(width, height, rgba).ok_or(ArtifactError::DecodeFailed)?;
            let image = Arc::new(RenderImage::new(SmallVec::from_vec(vec![Frame::new(
                buffer,
            )])));
            Ok(ArtifactUiPreview::Raster {
                artifact_id,
                image,
                width,
                height,
            })
        }
        ArtifactPreview::MetadataOnly => Ok(ArtifactUiPreview::MetadataOnly { artifact_id }),
    }
}

fn domain_error(error: ArtifactStoreError) -> ArtifactError {
    match error {
        ArtifactStoreError::Domain(error) => error,
        ArtifactStoreError::Io { kind, .. } => map_review_io(kind),
        ArtifactStoreError::Corrupt { .. } | ArtifactStoreError::TooLarge { .. } => {
            ArtifactError::Corrupt
        }
        ArtifactStoreError::UnsafeEntry { .. } => ArtifactError::UnsafeEntry,
    }
}

fn map_review_io(kind: std::io::ErrorKind) -> ArtifactError {
    match kind {
        std::io::ErrorKind::PermissionDenied => ArtifactError::PermissionDenied,
        std::io::ErrorKind::StorageFull => ArtifactError::StorageFull,
        std::io::ErrorKind::NotFound => ArtifactError::Unavailable,
        _ => ArtifactError::Unavailable,
    }
}

fn artifact_operation_label(operation: &ArtifactOperation) -> String {
    match (operation.kind, operation.progress) {
        (ArtifactOperationKind::Import, Some(progress)) => localization::artifact_import_progress(
            format_size(progress.bytes),
            format_size(progress.item_limit),
        ),
        (ArtifactOperationKind::Import, None) => localization::artifact_operation_importing(),
        (ArtifactOperationKind::Preview, _) => localization::artifact_operation_previewing(),
        (ArtifactOperationKind::Export, _) => localization::artifact_operation_exporting(),
        (ArtifactOperationKind::Quarantine, _) => localization::artifact_operation_quarantining(),
        (ArtifactOperationKind::Restore, _) => localization::artifact_operation_restoring(),
        (ArtifactOperationKind::Purge, _) => localization::artifact_operation_purging(),
    }
}

fn artifact_error_label(error: ArtifactError) -> String {
    match error {
        ArtifactError::ItemQuotaExceeded
        | ArtifactError::SessionQuotaExceeded
        | ArtifactError::GlobalQuotaExceeded
        | ArtifactError::CountQuotaExceeded => localization::artifact_error_quota(),
        ArtifactError::SourceChanged => localization::artifact_error_source_changed(),
        ArtifactError::UnsupportedSource | ArtifactError::UnsafeEntry => {
            localization::artifact_error_unsafe_source()
        }
        ArtifactError::Conflict => localization::artifact_error_export_conflict(),
        ArtifactError::Corrupt
        | ArtifactError::InvalidDigest
        | ArtifactError::InvalidMetadata
        | ArtifactError::InvalidState => localization::artifact_error_corrupt(),
        ArtifactError::PermissionDenied => localization::artifact_error_permission(),
        ArtifactError::StorageFull => localization::artifact_error_storage_full(),
        ArtifactError::Cancelled => localization::artifact_error_cancelled(),
        ArtifactError::Timeout => localization::artifact_error_timeout(),
        ArtifactError::DecodeFailed => localization::artifact_error_decode(),
        ArtifactError::InvalidDisplayName
        | ArtifactError::InvalidLimits
        | ArtifactError::Unavailable => localization::artifact_error_unavailable(),
    }
}

fn artifact_notice_label(notice: ArtifactNotice) -> String {
    match notice {
        ArtifactNotice::Imported => localization::artifact_notice_imported(),
        ArtifactNotice::Exported => localization::artifact_notice_exported(),
        ArtifactNotice::Quarantined => localization::artifact_notice_quarantined(),
        ArtifactNotice::Restored => localization::artifact_notice_restored(),
        ArtifactNotice::Purged => localization::artifact_notice_purged(),
    }
}

fn artifact_type_label(media_type: ArtifactMediaType) -> String {
    match media_type {
        ArtifactMediaType::TextPlainUtf8 => localization::artifact_type_text(),
        ArtifactMediaType::ImagePng => localization::artifact_type_png(),
        ArtifactMediaType::ImageJpeg => localization::artifact_type_jpeg(),
        ArtifactMediaType::MetadataOnly => localization::artifact_type_file(),
    }
}

fn artifact_state_label(state: ArtifactState) -> String {
    match state {
        ArtifactState::Staging => localization::artifact_state_staging(),
        ArtifactState::Ready => localization::artifact_state_ready(),
        ArtifactState::Quarantined => localization::artifact_state_quarantined(),
        ArtifactState::Corrupt => localization::artifact_state_corrupt(),
    }
}

fn artifact_state_color(state: ArtifactState) -> gpui::Hsla {
    match state {
        ArtifactState::Ready => theme::success(),
        ArtifactState::Staging => theme::accent(),
        ArtifactState::Quarantined => theme::warning(),
        ArtifactState::Corrupt => theme::danger(),
    }
}

fn artifact_preview_label(artifact: &ArtifactMetadata) -> String {
    match artifact.state {
        ArtifactState::Quarantined => localization::artifact_preview_quarantined(),
        ArtifactState::Corrupt => localization::artifact_preview_corrupt(),
        ArtifactState::Staging => localization::artifact_state_staging(),
        ArtifactState::Ready => match artifact.media_type {
            ArtifactMediaType::TextPlainUtf8 => localization::artifact_preview_text(),
            ArtifactMediaType::ImagePng | ArtifactMediaType::ImageJpeg => {
                localization::artifact_preview_raster()
            }
            ArtifactMediaType::MetadataOnly => localization::artifact_preview_metadata_only(),
        },
    }
}

fn artifact_origin_label(origin: ArtifactOrigin) -> String {
    match origin {
        ArtifactOrigin::ExplicitImport => localization::artifact_origin_import(),
    }
}

fn global_artifact_rows(
    sessions: &[SavedAppAttachedSession],
    snapshots: &HashMap<HostedSessionId, ArtifactSnapshot>,
) -> Vec<GlobalArtifactRow> {
    let mut rows = sessions
        .iter()
        .flat_map(|session| {
            snapshots
                .get(&session.id)
                .into_iter()
                .flat_map(|snapshot| snapshot.artifacts.iter())
                .map(|artifact| GlobalArtifactRow {
                    session_id: session.id,
                    project_label: session.project_label.clone(),
                    session_title: session.title.clone(),
                    preset_label: session.preset_label.clone(),
                    artifact: artifact.clone(),
                })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        left.project_label
            .cmp(&right.project_label)
            .then_with(|| left.session_title.cmp(&right.session_title))
            .then_with(|| right.artifact.created_at.cmp(&left.artifact.created_at))
            .then_with(|| left.artifact.id.cmp(&right.artifact.id))
    });
    rows
}

fn artifact_detail_row(label: String, value: String) -> AnyElement {
    h_flex()
        .flex_wrap()
        .justify_between()
        .gap(px(theme::SPACE_3))
        .child(div().child(label))
        .child(div().min_w_0().truncate().child(value))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved_session(label: &str) -> SavedAppAttachedSession {
        SavedAppAttachedSession {
            id: HostedSessionId::new(),
            route: termirust_domain::SessionLaunchRoute::LegacyAppAttached,
            origin: termirust_domain::SessionOrigin {
                project_id: termirust_domain::ProjectId::new(),
                preset_id: termirust_domain::PresetId::new(),
            },
            state: termirust_domain::HostedSessionState::Exited,
            project_label: label.to_string(),
            preset_label: "Codex".to_string(),
            title: format!("{label} task"),
            title_source: termirust_domain::TitleSource::Manual,
            activity: termirust_domain::ActivityAggregate::default(),
            pinned: false,
            read_through_sequence: 0,
            unread_sequence: None,
            archived_at: None,
            revision: termirust_domain::Revision::ZERO,
            durable_host: None,
            group_id: None,
            position: termirust_domain::PositionKey::FIRST,
            started_at: 1,
            updated_at: 1,
        }
    }

    fn snapshot(session_id: HostedSessionId, name: &str, created_at: u64) -> ArtifactSnapshot {
        ArtifactSnapshot {
            scope: ArtifactScope { session_id },
            artifacts: vec![ArtifactMetadata {
                id: ArtifactId::new(),
                scope: ArtifactScope { session_id },
                display_name: termirust_domain::ArtifactDisplayName::new(name).unwrap(),
                origin: ArtifactOrigin::ExplicitImport,
                media_type: ArtifactMediaType::TextPlainUtf8,
                byte_len: 4,
                sha256: termirust_domain::ArtifactSha256::new([created_at as u8; 32]),
                created_at,
                preview_kind: termirust_domain::ArtifactPreviewKind::Text,
                state: ArtifactState::Ready,
            }],
            session_bytes: 4,
            session_limit: 1024,
            global_bytes: 8,
            global_limit: 2048,
            durability: termirust_store::Durability::Full,
        }
    }

    #[test]
    fn artifact_gallery_error_mapping_is_stable_and_content_free() {
        assert_eq!(
            artifact_error_label(ArtifactError::SourceChanged),
            localization::artifact_error_source_changed()
        );
        assert!(
            !format!(
                "{:?}",
                ArtifactGalleryState::with_repository(
                    ArtifactRepository::open(tempfile::tempdir().unwrap().path()).unwrap()
                )
                .operation
            )
            .contains("canary-secret")
        );
    }

    #[test]
    fn artifact_gallery_preview_conversion_rejects_invalid_rgba_shape() {
        assert!(matches!(
            ui_preview(
                ArtifactId::new(),
                ArtifactPreview::Raster {
                    width: 2,
                    height: 2,
                    rgba: vec![0; 3],
                },
            ),
            Err(ArtifactError::DecodeFailed)
        ));
    }

    #[test]
    fn files_artifacts_index_joins_only_authoritative_sessions_with_origin_context() {
        let beta = saved_session("Beta");
        let alpha = saved_session("Alpha");
        let unknown_id = HostedSessionId::new();
        let snapshots = HashMap::from([
            (beta.id, snapshot(beta.id, "beta.txt", 2)),
            (alpha.id, snapshot(alpha.id, "alpha.txt", 1)),
            (unknown_id, snapshot(unknown_id, "unknown.txt", 3)),
        ]);

        let rows = global_artifact_rows(&[beta, alpha], &snapshots);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].project_label, "Alpha");
        assert_eq!(rows[1].project_label, "Beta");
        assert!(
            rows.iter()
                .all(|row| row.artifact.origin == ArtifactOrigin::ExplicitImport)
        );
        assert!(rows.iter().all(|row| row.session_id != unknown_id));
    }
}
