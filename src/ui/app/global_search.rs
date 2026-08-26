use std::collections::HashMap;
use std::sync::Arc;

use gpui::Context;
use termirust_domain::{
    ActivityState, HostedSessionState, PositionKey, ProjectId, ProjectStatus, Revision,
    SearchAction, SearchActionId, SearchCancellation, SearchCategory, SearchDocument,
    SearchDocumentId, SearchDocumentInput, SearchError, SearchIndex, SearchPage, SearchQuery,
    SearchResult, SearchStatus,
};

use super::TermiRustApp;
use super::palette::{CommandPaletteCandidate, PaletteAction, PaletteCategory};
use crate::ui::autocomplete::AutocompleteSource;
use crate::ui::localization;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SearchSourceRevisions {
    projects: Option<Revision>,
    presets: Option<Revision>,
    sessions: Option<Revision>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GlobalSearchFailure {
    QueryTooLong,
    TooManyTokens,
    Partial,
}

pub(super) struct GlobalSearchState {
    index: Arc<SearchIndex>,
    revisions: SearchSourceRevisions,
    project_documents: Vec<SearchDocumentId>,
    preset_documents: Vec<SearchDocumentId>,
    session_documents: Vec<SearchDocumentId>,
    pub results: Vec<SearchResult>,
    pub archived_fallback: bool,
    pub searching: bool,
    pub skipped_documents: usize,
    project_skipped: usize,
    preset_skipped: usize,
    session_skipped: usize,
    pub failure: Option<GlobalSearchFailure>,
    generation: u64,
    cancellation: Option<SearchCancellation>,
}

impl GlobalSearchState {
    pub fn new() -> Self {
        let mut index = SearchIndex::default();
        for (id, title, action) in [
            (
                SearchActionId::AddProject,
                localization::global_palette_add_project_action(),
                SearchAction::AddProject,
            ),
            (
                SearchActionId::NewSession,
                localization::global_palette_new_session_action(),
                SearchAction::NewSession,
            ),
            (
                SearchActionId::ShowArchive,
                localization::global_palette_show_archive_action(),
                SearchAction::ShowArchive,
            ),
        ] {
            let document = SearchDocument::new(SearchDocumentInput {
                id: SearchDocumentId::Action(id),
                title,
                project_id: None,
                project_label: None,
                group_label: None,
                preset_label: None,
                runtime_label: None,
                status: SearchStatus::Unknown,
                pinned: false,
                archived: false,
                position: PositionKey::new((id as u64 + 1) * 1024),
                meaningful_activity_at: 0,
                action,
            })
            .expect("localized global palette actions must remain bounded");
            index
                .insert(document)
                .expect("global palette action IDs must be unique");
        }
        Self {
            index: Arc::new(index),
            revisions: SearchSourceRevisions::default(),
            project_documents: Vec::new(),
            preset_documents: Vec::new(),
            session_documents: Vec::new(),
            results: Vec::new(),
            archived_fallback: false,
            searching: false,
            skipped_documents: 0,
            project_skipped: 0,
            preset_skipped: 0,
            session_skipped: 0,
            failure: None,
            generation: 0,
            cancellation: None,
        }
    }

    fn replace_documents(
        &mut self,
        old_ids: Vec<SearchDocumentId>,
        documents: Vec<Result<SearchDocument, SearchError>>,
    ) -> Vec<SearchDocumentId> {
        let index = Arc::make_mut(&mut self.index);
        for id in old_ids {
            let _ = index.remove(id);
        }
        let mut ids = Vec::with_capacity(documents.len());
        for document in documents {
            let Ok(document) = document else {
                self.skipped_documents = self.skipped_documents.saturating_add(1);
                continue;
            };
            let id = document.id();
            if index.insert(document).is_ok() {
                ids.push(id);
            } else {
                self.skipped_documents = self.skipped_documents.saturating_add(1);
            }
        }
        ids
    }

    fn begin_query(&mut self) -> (u64, SearchCancellation) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        self.generation = self.generation.wrapping_add(1).max(1);
        let cancellation = SearchCancellation::default();
        self.cancellation = Some(cancellation.clone());
        self.searching = true;
        self.failure = (self.skipped_documents > 0).then_some(GlobalSearchFailure::Partial);
        (self.generation, cancellation)
    }

    pub fn cancel(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        self.generation = self.generation.wrapping_add(1).max(1);
        self.searching = false;
    }
}

impl TermiRustApp {
    pub(super) fn refresh_global_search_index(&mut self) {
        let next_revisions = SearchSourceRevisions {
            projects: self
                .project_library
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.revision),
            presets: self
                .preset_library
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.revision),
            sessions: self
                .session_library
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.revision),
        };
        if next_revisions.projects != self.global_search.revisions.projects {
            self.global_search.project_skipped = 0;
            let mut documents = Vec::new();
            if let Some(snapshot) = self.project_library.snapshot.as_ref() {
                let project_labels = snapshot
                    .projects
                    .iter()
                    .map(|summary| {
                        (
                            summary.project.id,
                            summary.project.display_name.as_str().to_string(),
                        )
                    })
                    .collect::<HashMap<_, _>>();
                for summary in &snapshot.projects {
                    let project = &summary.project;
                    documents.push(SearchDocument::new(SearchDocumentInput {
                        id: SearchDocumentId::Project(project.id),
                        title: project.display_name.as_str().to_string(),
                        project_id: Some(project.id),
                        project_label: Some(project.display_name.as_str().to_string()),
                        group_label: None,
                        preset_label: None,
                        runtime_label: None,
                        status: match summary.status {
                            ProjectStatus::Available => SearchStatus::Unknown,
                            ProjectStatus::Unavailable | ProjectStatus::PermissionDenied => {
                                SearchStatus::Unavailable
                            }
                        },
                        pinned: false,
                        archived: false,
                        position: project.position,
                        meaningful_activity_at: 0,
                        action: SearchAction::OpenProject(project.id),
                    }));
                }
                for group in &snapshot.groups {
                    let Some(project_label) = project_labels.get(&group.project_id) else {
                        self.global_search.project_skipped =
                            self.global_search.project_skipped.saturating_add(1);
                        continue;
                    };
                    documents.push(SearchDocument::new(SearchDocumentInput {
                        id: SearchDocumentId::Group(group.id),
                        title: group.name.as_str().to_string(),
                        project_id: Some(group.project_id),
                        project_label: Some(project_label.clone()),
                        group_label: Some(group.name.as_str().to_string()),
                        preset_label: None,
                        runtime_label: None,
                        status: SearchStatus::Unknown,
                        pinned: false,
                        archived: false,
                        position: group.position,
                        meaningful_activity_at: 0,
                        action: SearchAction::OpenGroup {
                            project_id: group.project_id,
                            group_id: group.id,
                        },
                    }));
                }
            }
            let old = std::mem::take(&mut self.global_search.project_documents);
            self.global_search.project_documents =
                self.global_search.replace_documents(old, documents);
            self.global_search.project_skipped = self
                .global_search
                .project_skipped
                .saturating_add(self.global_search.skipped_documents);
            self.global_search.skipped_documents = 0;
        }

        if next_revisions.presets != self.global_search.revisions.presets {
            self.global_search.preset_skipped = 0;
            let mut documents = Vec::new();
            if let Some(snapshot) = self.preset_library.snapshot.as_ref() {
                for preset in &snapshot.presets {
                    documents.push(SearchDocument::new(SearchDocumentInput {
                        id: SearchDocumentId::Preset(preset.id),
                        title: preset.label.as_str().to_string(),
                        project_id: None,
                        project_label: None,
                        group_label: None,
                        preset_label: Some(preset.label.as_str().to_string()),
                        runtime_label: preset
                            .runtime
                            .as_ref()
                            .map(|runtime| runtime.as_str().to_string()),
                        status: if preset.enabled {
                            SearchStatus::Unknown
                        } else {
                            SearchStatus::Unavailable
                        },
                        pinned: preset.favorite,
                        archived: false,
                        position: preset.position,
                        meaningful_activity_at: 0,
                        action: SearchAction::StartPreset(preset.id),
                    }));
                }
            }
            let old = std::mem::take(&mut self.global_search.preset_documents);
            self.global_search.preset_documents =
                self.global_search.replace_documents(old, documents);
            self.global_search.preset_skipped = self.global_search.skipped_documents;
            self.global_search.skipped_documents = 0;
        }

        if next_revisions.sessions != self.global_search.revisions.sessions
            || next_revisions.projects != self.global_search.revisions.projects
            || next_revisions.presets != self.global_search.revisions.presets
        {
            self.global_search.session_skipped = 0;
            let mut documents = Vec::new();
            if let Some(snapshot) = self.session_library.snapshot.as_ref() {
                let projects = self
                    .project_library
                    .snapshot
                    .as_ref()
                    .map(|snapshot| {
                        snapshot
                            .projects
                            .iter()
                            .map(|summary| {
                                (
                                    summary.project.id,
                                    summary.project.display_name.as_str().to_string(),
                                )
                            })
                            .collect::<HashMap<_, _>>()
                    })
                    .unwrap_or_default();
                let groups = self
                    .project_library
                    .snapshot
                    .as_ref()
                    .map(|snapshot| {
                        snapshot
                            .groups
                            .iter()
                            .map(|group| (group.id, group.name.as_str().to_string()))
                            .collect::<HashMap<_, _>>()
                    })
                    .unwrap_or_default();
                let presets = self
                    .preset_library
                    .snapshot
                    .as_ref()
                    .map(|snapshot| {
                        snapshot
                            .presets
                            .iter()
                            .map(|preset| {
                                (
                                    preset.id,
                                    (
                                        preset.label.as_str().to_string(),
                                        preset
                                            .runtime
                                            .as_ref()
                                            .map(|runtime| runtime.as_str().to_string()),
                                    ),
                                )
                            })
                            .collect::<HashMap<_, _>>()
                    })
                    .unwrap_or_default();
                for session in &snapshot.sessions {
                    let Some(project_label) = projects.get(&session.project_id) else {
                        self.global_search.session_skipped =
                            self.global_search.session_skipped.saturating_add(1);
                        continue;
                    };
                    let (preset_label, runtime_label) = session
                        .preset_id
                        .and_then(|id| presets.get(&id).cloned())
                        .map(|(label, runtime)| (Some(label), runtime))
                        .unwrap_or_default();
                    documents.push(SearchDocument::new(SearchDocumentInput {
                        id: SearchDocumentId::Session(session.id),
                        title: session.title.as_str().to_string(),
                        project_id: Some(session.project_id),
                        project_label: Some(project_label.clone()),
                        group_label: session.group_id.and_then(|id| groups.get(&id).cloned()),
                        preset_label,
                        runtime_label,
                        status: search_status_for_session(
                            session.lifecycle,
                            session.activity.state,
                        ),
                        pinned: session.pinned,
                        archived: session.archived_at.is_some(),
                        position: session.position,
                        meaningful_activity_at: session.updated_at,
                        action: SearchAction::OpenSession(session.id),
                    }));
                }
            }
            let old = std::mem::take(&mut self.global_search.session_documents);
            self.global_search.session_documents =
                self.global_search.replace_documents(old, documents);
            self.global_search.session_skipped = self
                .global_search
                .session_skipped
                .saturating_add(self.global_search.skipped_documents);
            self.global_search.skipped_documents = 0;
        }

        self.global_search.revisions = next_revisions;
        self.global_search.skipped_documents = self
            .global_search
            .project_skipped
            .saturating_add(self.global_search.preset_skipped)
            .saturating_add(self.global_search.session_skipped);
        if self.global_search.skipped_documents > 0 {
            self.global_search.failure = Some(GlobalSearchFailure::Partial);
        }
    }

    pub(super) fn queue_global_palette_search(&mut self, cx: &mut Context<Self>) {
        if !self.show_command_palette {
            return;
        }
        self.refresh_global_search_index();
        let query = match SearchQuery::parse(&self.command_palette_query(cx)) {
            Ok(query) => query,
            Err(SearchError::QueryTooLong) => {
                self.global_search.cancel();
                self.global_search.failure = Some(GlobalSearchFailure::QueryTooLong);
                cx.notify();
                return;
            }
            Err(SearchError::TooManyQueryTokens) => {
                self.global_search.cancel();
                self.global_search.failure = Some(GlobalSearchFailure::TooManyTokens);
                cx.notify();
                return;
            }
            Err(_) => {
                self.global_search.cancel();
                self.global_search.failure = Some(GlobalSearchFailure::Partial);
                cx.notify();
                return;
            }
        };
        let current_project = self.current_palette_project();
        let index = Arc::clone(&self.global_search.index);
        let (generation, cancellation) = self.global_search.begin_query();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let cancellation = cancellation.clone();
                    async move { index.search(&query, current_project, &cancellation) }
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.global_search.generation != generation || !app.show_command_palette {
                    return;
                }
                app.global_search.searching = false;
                match result {
                    Ok(SearchPage {
                        results,
                        archived_fallback,
                    }) => {
                        app.global_search.results = results;
                        app.global_search.archived_fallback = archived_fallback;
                        app.global_search.failure = (app.global_search.skipped_documents > 0)
                            .then_some(GlobalSearchFailure::Partial);
                    }
                    Err(SearchError::Cancelled) => return,
                    Err(_) => app.global_search.failure = Some(GlobalSearchFailure::Partial),
                }
                let candidate_count = app.command_palette_candidates(cx).len();
                app.selected_command_palette_index = app
                    .selected_command_palette_index
                    .min(candidate_count.saturating_sub(1));
                cx.notify();
            });
        })
        .detach();
    }

    fn current_palette_project(&self) -> Option<ProjectId> {
        self.active_pane()
            .and_then(|pane| pane.app_attached.as_ref())
            .map(|attached| attached.origin.project_id)
            .or_else(|| self.new_session.as_ref().map(|state| state.project_id))
            .or(self.project_library.selected_id)
    }

    pub(super) fn global_palette_candidates(
        &self,
        command_candidates: Vec<CommandPaletteCandidate>,
    ) -> Vec<CommandPaletteCandidate> {
        let mut candidates = self
            .global_search
            .results
            .iter()
            .map(search_result_candidate)
            .collect::<Vec<_>>();
        candidates.extend(command_candidates);
        candidates.sort_by_key(|candidate| candidate.category.rank());
        candidates
    }
}

pub(super) fn search_result_candidate(result: &SearchResult) -> CommandPaletteCandidate {
    let location = [
        result.project_label.as_deref(),
        result.group_label.as_deref(),
        result.preset_label.as_deref(),
        result.runtime_label.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.trim().is_empty() && *value != result.title)
    .collect::<Vec<_>>()
    .join(" / ");
    CommandPaletteCandidate {
        command: String::new(),
        title: result.title.clone(),
        detail: location,
        source: AutocompleteSource::Builtin,
        pinned: result.pinned,
        category: match result.category {
            SearchCategory::Attention => PaletteCategory::Attention,
            SearchCategory::Session => PaletteCategory::Sessions,
            SearchCategory::Project => PaletteCategory::Projects,
            SearchCategory::Group => PaletteCategory::Groups,
            SearchCategory::Preset => PaletteCategory::Presets,
            SearchCategory::Action => PaletteCategory::Actions,
            SearchCategory::Archive => PaletteCategory::Archive,
        },
        action: PaletteAction::Search(result.action),
        status: Some(result.status),
        highlights: result.highlights.clone(),
    }
}

fn search_status_for_session(
    lifecycle: HostedSessionState,
    activity: ActivityState,
) -> SearchStatus {
    match lifecycle {
        HostedSessionState::Exited | HostedSessionState::Cancelled => SearchStatus::Done,
        HostedSessionState::Live | HostedSessionState::RecordingPaused
            if activity == ActivityState::Idle =>
        {
            SearchStatus::Idle
        }
        HostedSessionState::Provisioning
        | HostedSessionState::Starting
        | HostedSessionState::Attaching
        | HostedSessionState::Replaying
        | HostedSessionState::Live
        | HostedSessionState::RecordingPaused
        | HostedSessionState::Stopping
        | HostedSessionState::RunningAppAttached => SearchStatus::Running,
        HostedSessionState::Offline
        | HostedSessionState::Orphaned
        | HostedSessionState::Gap
        | HostedSessionState::PermissionDenied
        | HostedSessionState::Incompatible
        | HostedSessionState::Failed => SearchStatus::Unavailable,
        HostedSessionState::Draft | HostedSessionState::Validating => SearchStatus::Unknown,
    }
}

pub(super) fn global_search_failure_message(
    failure: GlobalSearchFailure,
    skipped_documents: usize,
) -> String {
    match failure {
        GlobalSearchFailure::QueryTooLong => localization::global_palette_query_too_long(),
        GlobalSearchFailure::TooManyTokens => localization::global_palette_too_many_tokens(),
        GlobalSearchFailure::Partial => {
            localization::global_palette_partial(skipped_documents.max(1))
        }
    }
}

pub(super) fn search_status_label(status: SearchStatus) -> String {
    match status {
        SearchStatus::Attention => localization::global_palette_status_attention(),
        SearchStatus::Busy => localization::global_palette_status_busy(),
        SearchStatus::Done => localization::global_palette_status_done(),
        SearchStatus::Running => localization::global_palette_status_running(),
        SearchStatus::Idle => localization::global_palette_status_idle(),
        SearchStatus::Unavailable => localization::global_palette_status_unavailable(),
        SearchStatus::Unknown => localization::global_palette_status_unknown(),
    }
}

pub(super) fn category_label(category: PaletteCategory) -> String {
    match category {
        PaletteCategory::Attention => localization::global_palette_category_attention(),
        PaletteCategory::Sessions => localization::global_palette_category_sessions(),
        PaletteCategory::Projects => localization::global_palette_category_projects(),
        PaletteCategory::Groups => localization::global_palette_category_groups(),
        PaletteCategory::Presets => localization::global_palette_category_presets(),
        PaletteCategory::Actions => localization::global_palette_category_actions(),
        PaletteCategory::Archive => localization::global_palette_category_archive(),
        PaletteCategory::Commands => localization::global_palette_category_commands(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_domain::{HostedSessionId, SearchDocumentId};

    #[test]
    fn palette_tests_category_mapping_and_status_copy_are_total() {
        let id = HostedSessionId::new();
        let result = SearchResult {
            id: SearchDocumentId::Session(id),
            category: SearchCategory::Archive,
            title: "Retained".to_string(),
            project_label: Some("Console".to_string()),
            group_label: Some("Auth".to_string()),
            preset_label: None,
            runtime_label: Some("codex".to_string()),
            status: SearchStatus::Done,
            pinned: true,
            archived: true,
            highlights: Vec::new(),
            action: SearchAction::OpenSession(id),
            score: termirust_domain::ScoreTuple {
                match_quality: 3,
                current_project: 1,
                actionable_status: 1,
                pinned: 1,
                position: PositionKey::FIRST,
                meaningful_activity_at: 1,
                id: SearchDocumentId::Session(id),
            },
        };
        let candidate = search_result_candidate(&result);
        assert_eq!(candidate.category, PaletteCategory::Archive);
        assert_eq!(candidate.detail, "Console / Auth / codex");
        assert!(candidate.pinned);
        assert_eq!(candidate.status, Some(SearchStatus::Done));
        assert_eq!(search_status_label(SearchStatus::Done), "Done");
    }
}
