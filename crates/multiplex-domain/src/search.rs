use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use unicode_casefold::UnicodeCaseFold as _;
use unicode_normalization::UnicodeNormalization as _;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::project::MAX_PROJECTS;
use crate::{
    GroupId, HostedSessionId, MAX_GROUPS_PER_PROJECT, MAX_PRESETS, MAX_SESSIONS_PER_PROJECT,
    PositionKey, PresetId, ProjectId,
};

pub const MAX_SEARCH_DOCUMENT_BYTES: usize = 4 * 1024;
pub const MAX_SEARCH_QUERY_SCALARS: usize = 256;
pub const MAX_SEARCH_QUERY_TOKENS: usize = 16;
pub const MAX_SEARCH_RESULTS: usize = 100;
const CANCELLATION_BLOCK: usize = 128;
const MAX_SEARCH_ACTIONS: usize = 64;
const MAX_SEARCH_GROUPS: usize = MAX_GROUPS_PER_PROJECT * MAX_PROJECTS;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SearchActionId {
    AddProject,
    NewSession,
    ShowArchive,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SearchDocumentId {
    Session(HostedSessionId),
    Project(ProjectId),
    Group(GroupId),
    Preset(PresetId),
    Action(SearchActionId),
}

impl SearchDocumentId {
    fn search_text(self) -> String {
        match self {
            Self::Session(id) => id.to_string(),
            Self::Project(id) => id.to_string(),
            Self::Group(id) => id.to_string(),
            Self::Preset(id) => id.to_string(),
            Self::Action(SearchActionId::AddProject) => "add-project".to_string(),
            Self::Action(SearchActionId::NewSession) => "new-session".to_string(),
            Self::Action(SearchActionId::ShowArchive) => "show-archive".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SearchCategory {
    Attention,
    Session,
    Project,
    Group,
    Preset,
    Action,
    Archive,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SearchStatus {
    Attention,
    Busy,
    Done,
    Running,
    Idle,
    Unavailable,
    #[default]
    Unknown,
}

impl SearchStatus {
    fn search_text(self) -> &'static str {
        match self {
            Self::Attention => "needs input attention",
            Self::Busy => "busy",
            Self::Done => "done exited",
            Self::Running => "running live",
            Self::Idle => "idle",
            Self::Unavailable => "unavailable offline stale",
            Self::Unknown => "unknown",
        }
    }

    fn actionable_weight(self) -> u8 {
        match self {
            Self::Attention => 3,
            Self::Busy => 2,
            Self::Done => 1,
            Self::Running | Self::Idle | Self::Unavailable | Self::Unknown => 0,
        }
    }

    fn is_running(self) -> bool {
        matches!(
            self,
            Self::Attention | Self::Busy | Self::Running | Self::Idle
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchAction {
    OpenSession(HostedSessionId),
    OpenProject(ProjectId),
    OpenGroup {
        project_id: ProjectId,
        group_id: GroupId,
    },
    StartPreset(PresetId),
    AddProject,
    NewSession,
    ShowArchive,
}

#[derive(Clone)]
pub struct SearchDocumentInput {
    pub id: SearchDocumentId,
    pub title: String,
    pub project_id: Option<ProjectId>,
    pub project_label: Option<String>,
    pub group_label: Option<String>,
    pub preset_label: Option<String>,
    pub runtime_label: Option<String>,
    pub status: SearchStatus,
    pub pinned: bool,
    pub archived: bool,
    pub position: PositionKey,
    pub meaningful_activity_at: u64,
    pub action: SearchAction,
}

#[derive(Clone)]
pub struct SearchDocument {
    id: SearchDocumentId,
    title: String,
    project_id: Option<ProjectId>,
    project_label: Option<String>,
    group_label: Option<String>,
    preset_label: Option<String>,
    runtime_label: Option<String>,
    status: SearchStatus,
    pinned: bool,
    archived: bool,
    position: PositionKey,
    meaningful_activity_at: u64,
    action: SearchAction,
    fields: Vec<NormalizedField>,
}

impl fmt::Debug for SearchDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchDocument")
            .field("id", &self.id)
            .field("title", &"<redacted>")
            .field("project_id", &self.project_id)
            .field("status", &self.status)
            .field("pinned", &self.pinned)
            .field("archived", &self.archived)
            .finish_non_exhaustive()
    }
}

impl SearchDocument {
    pub fn new(input: SearchDocumentInput) -> Result<Self, SearchError> {
        if input.title.trim().is_empty() || input.title.contains('\0') {
            return Err(SearchError::InvalidDocument);
        }
        let mut fields = Vec::with_capacity(7);
        fields.push(NormalizedField::new(
            HighlightField::Title,
            input.title.clone(),
        ));
        push_optional_field(
            &mut fields,
            HighlightField::Project,
            input.project_label.as_ref(),
        );
        push_optional_field(
            &mut fields,
            HighlightField::Group,
            input.group_label.as_ref(),
        );
        push_optional_field(
            &mut fields,
            HighlightField::Preset,
            input.preset_label.as_ref(),
        );
        push_optional_field(
            &mut fields,
            HighlightField::Runtime,
            input.runtime_label.as_ref(),
        );
        fields.push(NormalizedField::new(
            HighlightField::Status,
            input.status.search_text().to_string(),
        ));
        fields.push(NormalizedField::new(
            HighlightField::Id,
            input.id.search_text(),
        ));
        let normalized_bytes = fields
            .iter()
            .map(NormalizedField::normalized_bytes)
            .sum::<usize>()
            .saturating_add(fields.len().saturating_sub(1));
        if normalized_bytes > MAX_SEARCH_DOCUMENT_BYTES {
            return Err(SearchError::DocumentTooLarge);
        }
        Ok(Self {
            id: input.id,
            title: input.title,
            project_id: input.project_id,
            project_label: input.project_label,
            group_label: input.group_label,
            preset_label: input.preset_label,
            runtime_label: input.runtime_label,
            status: input.status,
            pinned: input.pinned,
            archived: input.archived,
            position: input.position,
            meaningful_activity_at: input.meaningful_activity_at,
            action: input.action,
            fields,
        })
    }

    pub fn id(&self) -> SearchDocumentId {
        self.id
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum Filter {
    Archived,
    Running,
    Attention,
    Project(String),
    Runtime(String),
}

impl fmt::Debug for Filter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archived => formatter.write_str("Archived"),
            Self::Running => formatter.write_str("Running"),
            Self::Attention => formatter.write_str("Attention"),
            Self::Project(_) => formatter.write_str("Project(<redacted>)"),
            Self::Runtime(_) => formatter.write_str("Runtime(<redacted>)"),
        }
    }
}

#[derive(Clone)]
pub struct SearchQuery {
    terms: Vec<Vec<char>>,
    phrase: Vec<char>,
    filters: Vec<Filter>,
}

impl fmt::Debug for SearchQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchQuery")
            .field("terms", &self.terms.len())
            .field("filters", &self.filters)
            .finish()
    }
}

impl SearchQuery {
    pub fn parse(value: &str) -> Result<Self, SearchError> {
        if value.chars().count() > MAX_SEARCH_QUERY_SCALARS {
            return Err(SearchError::QueryTooLong);
        }
        let raw_tokens = value.split_whitespace().collect::<Vec<_>>();
        if raw_tokens.len() > MAX_SEARCH_QUERY_TOKENS {
            return Err(SearchError::TooManyQueryTokens);
        }
        let mut filters = Vec::new();
        let mut plain = Vec::new();
        for token in raw_tokens {
            let normalized = normalize(token);
            match normalized.as_str() {
                "is:archived" => filters.push(Filter::Archived),
                "is:running" => filters.push(Filter::Running),
                "is:attention" => filters.push(Filter::Attention),
                _ if normalized.starts_with("project:") && normalized.len() > 8 => {
                    filters.push(Filter::Project(normalized[8..].to_string()));
                }
                _ if normalized.starts_with("runtime:") && normalized.len() > 8 => {
                    filters.push(Filter::Runtime(normalized[8..].to_string()));
                }
                _ => plain.push(normalized),
            }
        }
        let phrase = plain.join(" ").chars().collect();
        let terms = plain
            .into_iter()
            .filter(|term| !term.is_empty())
            .map(|term| term.chars().collect())
            .collect();
        Ok(Self {
            terms,
            phrase,
            filters,
        })
    }

    pub fn filters(&self) -> &[Filter] {
        &self.filters
    }

    pub fn explicitly_archived(&self) -> bool {
        self.filters.contains(&Filter::Archived)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HighlightField {
    Title,
    Project,
    Group,
    Preset,
    Runtime,
    Status,
    Id,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextHighlight {
    pub field: HighlightField,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScoreTuple {
    pub match_quality: u8,
    pub current_project: u8,
    pub actionable_status: u8,
    pub pinned: u8,
    pub position: PositionKey,
    pub meaningful_activity_at: u64,
    pub id: SearchDocumentId,
}

#[derive(Clone)]
pub struct SearchResult {
    pub id: SearchDocumentId,
    pub category: SearchCategory,
    pub title: String,
    pub project_label: Option<String>,
    pub group_label: Option<String>,
    pub preset_label: Option<String>,
    pub runtime_label: Option<String>,
    pub status: SearchStatus,
    pub pinned: bool,
    pub archived: bool,
    pub highlights: Vec<TextHighlight>,
    pub action: SearchAction,
    pub score: ScoreTuple,
}

impl fmt::Debug for SearchResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchResult")
            .field("id", &self.id)
            .field("category", &self.category)
            .field("title", &"<redacted>")
            .field("status", &self.status)
            .field("archived", &self.archived)
            .field("score", &self.score)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default)]
pub struct SearchPage {
    pub results: Vec<SearchResult>,
    pub archived_fallback: bool,
}

#[derive(Clone, Default)]
pub struct SearchCancellation(Arc<AtomicBool>);

impl SearchCancellation {
    pub fn cancel(&self) {
        self.0.store(true, AtomicOrdering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(AtomicOrdering::Acquire)
    }
}

impl fmt::Debug for SearchCancellation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchCancellation")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchError {
    InvalidDocument,
    DocumentTooLarge,
    QueryTooLong,
    TooManyQueryTokens,
    DuplicateDocument,
    ResourceLimit { kind: SearchCategory, limit: usize },
    Cancelled,
}

impl fmt::Display for SearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDocument => formatter.write_str("search document is invalid"),
            Self::DocumentTooLarge => formatter.write_str("search document is too large"),
            Self::QueryTooLong => formatter.write_str("search query is too long"),
            Self::TooManyQueryTokens => formatter.write_str("search query has too many tokens"),
            Self::DuplicateDocument => formatter.write_str("search document already exists"),
            Self::ResourceLimit { kind, limit } => {
                write!(formatter, "search {kind:?} limit of {limit} reached")
            }
            Self::Cancelled => formatter.write_str("search was cancelled"),
        }
    }
}

impl std::error::Error for SearchError {}

#[derive(Clone, Default)]
pub struct SearchIndex {
    documents: BTreeMap<SearchDocumentId, SearchDocument>,
    sessions: usize,
    projects: usize,
    groups: usize,
    presets: usize,
    actions: usize,
}

impl fmt::Debug for SearchIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SearchIndex")
            .field("documents", &self.documents.len())
            .field("sessions", &self.sessions)
            .field("projects", &self.projects)
            .field("groups", &self.groups)
            .field("presets", &self.presets)
            .field("actions", &self.actions)
            .finish()
    }
}

impl SearchIndex {
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    pub fn insert(&mut self, document: SearchDocument) -> Result<(), SearchError> {
        if self.documents.contains_key(&document.id) {
            return Err(SearchError::DuplicateDocument);
        }
        let (count, limit, kind) = match document.id {
            SearchDocumentId::Session(_) => (
                &mut self.sessions,
                MAX_SESSIONS_PER_PROJECT,
                SearchCategory::Session,
            ),
            SearchDocumentId::Project(_) => {
                (&mut self.projects, MAX_PROJECTS, SearchCategory::Project)
            }
            SearchDocumentId::Group(_) => {
                (&mut self.groups, MAX_SEARCH_GROUPS, SearchCategory::Group)
            }
            SearchDocumentId::Preset(_) => (&mut self.presets, MAX_PRESETS, SearchCategory::Preset),
            SearchDocumentId::Action(_) => (
                &mut self.actions,
                MAX_SEARCH_ACTIONS,
                SearchCategory::Action,
            ),
        };
        if *count >= limit {
            return Err(SearchError::ResourceLimit { kind, limit });
        }
        *count += 1;
        self.documents.insert(document.id, document);
        Ok(())
    }

    pub fn remove(&mut self, id: SearchDocumentId) -> Option<SearchDocument> {
        let removed = self.documents.remove(&id)?;
        match id {
            SearchDocumentId::Session(_) => self.sessions = self.sessions.saturating_sub(1),
            SearchDocumentId::Project(_) => self.projects = self.projects.saturating_sub(1),
            SearchDocumentId::Group(_) => self.groups = self.groups.saturating_sub(1),
            SearchDocumentId::Preset(_) => self.presets = self.presets.saturating_sub(1),
            SearchDocumentId::Action(_) => self.actions = self.actions.saturating_sub(1),
        }
        Some(removed)
    }

    pub fn search(
        &self,
        query: &SearchQuery,
        current_project: Option<ProjectId>,
        cancellation: &SearchCancellation,
    ) -> Result<SearchPage, SearchError> {
        if cancellation.is_cancelled() {
            return Err(SearchError::Cancelled);
        }
        if query.explicitly_archived() {
            let results = self.search_partition(query, current_project, true, cancellation)?;
            return Ok(SearchPage {
                results,
                archived_fallback: false,
            });
        }
        let active = self.search_partition(query, current_project, false, cancellation)?;
        if !active.is_empty() {
            return Ok(SearchPage {
                results: active,
                archived_fallback: false,
            });
        }
        let archived = self.search_partition(query, current_project, true, cancellation)?;
        Ok(SearchPage {
            archived_fallback: !archived.is_empty(),
            results: archived,
        })
    }

    fn search_partition(
        &self,
        query: &SearchQuery,
        current_project: Option<ProjectId>,
        archived: bool,
        cancellation: &SearchCancellation,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let mut heap = BinaryHeap::with_capacity(MAX_SEARCH_RESULTS + 1);
        for (index, document) in self.documents.values().enumerate() {
            if index % CANCELLATION_BLOCK == 0 && cancellation.is_cancelled() {
                return Err(SearchError::Cancelled);
            }
            if document.archived != archived || !matches_filters(document, query) {
                continue;
            }
            let Some((match_quality, highlights)) = match_document(document, query) else {
                continue;
            };
            let score = ScoreTuple {
                match_quality,
                current_project: u8::from(
                    current_project.is_some() && document.project_id == current_project,
                ),
                actionable_status: document.status.actionable_weight(),
                pinned: u8::from(document.pinned),
                position: document.position,
                meaningful_activity_at: document.meaningful_activity_at,
                id: document.id,
            };
            heap.push(HeapEntry(result_from(document, score, highlights)));
            if heap.len() > MAX_SEARCH_RESULTS {
                let _ = heap.pop();
            }
        }
        let mut results = heap.into_iter().map(|entry| entry.0).collect::<Vec<_>>();
        results.sort_by(compare_results);
        Ok(results)
    }
}

#[derive(Clone)]
struct HeapEntry(SearchResult);

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        compare_results(&self.0, &other.0) == Ordering::Equal
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_results(&self.0, &other.0)
    }
}

fn compare_results(left: &SearchResult, right: &SearchResult) -> Ordering {
    right
        .score
        .match_quality
        .cmp(&left.score.match_quality)
        .then_with(|| right.score.current_project.cmp(&left.score.current_project))
        .then_with(|| {
            right
                .score
                .actionable_status
                .cmp(&left.score.actionable_status)
        })
        .then_with(|| right.score.pinned.cmp(&left.score.pinned))
        .then_with(|| left.score.position.cmp(&right.score.position))
        .then_with(|| {
            right
                .score
                .meaningful_activity_at
                .cmp(&left.score.meaningful_activity_at)
        })
        .then_with(|| left.score.id.cmp(&right.score.id))
}

fn result_from(
    document: &SearchDocument,
    score: ScoreTuple,
    highlights: Vec<TextHighlight>,
) -> SearchResult {
    let category = if document.archived {
        SearchCategory::Archive
    } else if document.status == SearchStatus::Attention {
        SearchCategory::Attention
    } else {
        match document.id {
            SearchDocumentId::Session(_) => SearchCategory::Session,
            SearchDocumentId::Project(_) => SearchCategory::Project,
            SearchDocumentId::Group(_) => SearchCategory::Group,
            SearchDocumentId::Preset(_) => SearchCategory::Preset,
            SearchDocumentId::Action(_) => SearchCategory::Action,
        }
    };
    SearchResult {
        id: document.id,
        category,
        title: document.title.clone(),
        project_label: document.project_label.clone(),
        group_label: document.group_label.clone(),
        preset_label: document.preset_label.clone(),
        runtime_label: document.runtime_label.clone(),
        status: document.status,
        pinned: document.pinned,
        archived: document.archived,
        highlights,
        action: document.action,
        score,
    }
}

fn matches_filters(document: &SearchDocument, query: &SearchQuery) -> bool {
    query.filters.iter().all(|filter| match filter {
        Filter::Archived => document.archived,
        Filter::Running => document.status.is_running(),
        Filter::Attention => document.status == SearchStatus::Attention,
        Filter::Project(value) => document
            .project_label
            .as_deref()
            .is_some_and(|label| contains_normalized(label, value)),
        Filter::Runtime(value) => document
            .runtime_label
            .as_deref()
            .is_some_and(|label| contains_normalized(label, value)),
    })
}

fn match_document(
    document: &SearchDocument,
    query: &SearchQuery,
) -> Option<(u8, Vec<TextHighlight>)> {
    if query.terms.is_empty() {
        return Some((0, Vec::new()));
    }
    if !query.phrase.is_empty()
        && let Some(field) = document
            .fields
            .iter()
            .find(|field| field.chars.starts_with(&query.phrase))
    {
        return Some((3, field.highlight_range(0, query.phrase.len())));
    }

    let mut token_highlights = Vec::new();
    let token_prefix = query.terms.iter().all(|term| {
        document.fields.iter().any(|field| {
            let Some((start, end)) = field.token_prefix(term) else {
                return false;
            };
            token_highlights.extend(field.highlight_range(start, end));
            true
        })
    });
    if token_prefix {
        merge_highlights(&mut token_highlights);
        return Some((2, token_highlights));
    }

    let mut subsequence_highlights = Vec::new();
    let subsequence = query.terms.iter().all(|term| {
        document.fields.iter().any(|field| {
            let Some((start, end)) = field.subsequence(term) else {
                return false;
            };
            subsequence_highlights.extend(field.highlight_range(start, end));
            true
        })
    });
    subsequence.then(|| {
        merge_highlights(&mut subsequence_highlights);
        (1, subsequence_highlights)
    })
}

fn contains_normalized(value: &str, needle: &str) -> bool {
    normalize(value).contains(needle)
}

#[derive(Clone)]
struct NormalizedField {
    kind: HighlightField,
    chars: Vec<char>,
    spans: Vec<(usize, usize)>,
}

impl NormalizedField {
    fn new(kind: HighlightField, original: String) -> Self {
        let mut chars = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for (byte_start, grapheme) in original.grapheme_indices(true) {
            let byte_end = byte_start + grapheme.len();
            let folded = grapheme.nfkc().case_fold().collect::<String>();
            for normalized in folded.chars() {
                let normalized = if normalized.is_whitespace() {
                    ' '
                } else {
                    normalized
                };
                if normalized == ' ' && chars.last() == Some(&' ') {
                    if let Some(last) = spans.last_mut() {
                        last.1 = byte_end;
                    }
                    continue;
                }
                chars.push(normalized);
                spans.push((byte_start, byte_end));
            }
        }
        while chars.first() == Some(&' ') {
            chars.remove(0);
            spans.remove(0);
        }
        while chars.last() == Some(&' ') {
            chars.pop();
            spans.pop();
        }
        Self { kind, chars, spans }
    }

    fn normalized_bytes(&self) -> usize {
        self.chars
            .iter()
            .map(|character| character.len_utf8())
            .sum()
    }

    fn token_prefix(&self, term: &[char]) -> Option<(usize, usize)> {
        if term.is_empty() {
            return None;
        }
        let mut start = 0;
        while start < self.chars.len() {
            while start < self.chars.len() && self.chars[start].is_whitespace() {
                start += 1;
            }
            let mut end = start;
            while end < self.chars.len() && !self.chars[end].is_whitespace() {
                end += 1;
            }
            if self.chars[start..end].starts_with(term) {
                return Some((start, start + term.len()));
            }
            start = end.saturating_add(1);
        }
        None
    }

    fn subsequence(&self, term: &[char]) -> Option<(usize, usize)> {
        let mut needle = term.iter();
        let mut next = needle.next()?;
        let mut start = None;
        for (index, candidate) in self.chars.iter().enumerate() {
            if candidate != next {
                continue;
            }
            start.get_or_insert(index);
            match needle.next() {
                Some(value) => next = value,
                None => return Some((start.unwrap_or(index), index + 1)),
            }
        }
        None
    }

    fn highlight_range(&self, start: usize, end: usize) -> Vec<TextHighlight> {
        let Some((byte_start, _)) = self.spans.get(start).copied() else {
            return Vec::new();
        };
        let Some((_, byte_end)) = self.spans.get(end.saturating_sub(1)).copied() else {
            return Vec::new();
        };
        vec![TextHighlight {
            field: self.kind,
            start: byte_start,
            end: byte_end,
        }]
    }
}

fn push_optional_field(
    fields: &mut Vec<NormalizedField>,
    kind: HighlightField,
    value: Option<&String>,
) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        fields.push(NormalizedField::new(kind, value.clone()));
    }
}

fn merge_highlights(highlights: &mut Vec<TextHighlight>) {
    highlights.sort_by_key(|highlight| (highlight.field as u8, highlight.start, highlight.end));
    let mut merged: Vec<TextHighlight> = Vec::with_capacity(highlights.len());
    for highlight in highlights.drain(..) {
        if let Some(last) = merged.last_mut()
            && last.field == highlight.field
            && highlight.start <= last.end
        {
            last.end = last.end.max(highlight.end);
            continue;
        }
        merged.push(highlight);
    }
    *highlights = merged;
}

fn normalize(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.nfkc().case_fold() {
        if character.is_whitespace() {
            pending_space = !normalized.is_empty();
        } else {
            if pending_space {
                normalized.push(' ');
                pending_space = false;
            }
            normalized.push(character);
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn project_id(value: u128) -> ProjectId {
        ProjectId::from_uuid(Uuid::from_u128(value))
    }

    fn session_id(value: u128) -> HostedSessionId {
        HostedSessionId::from_uuid(Uuid::from_u128(value))
    }

    fn session_document(
        id: u128,
        title: &str,
        project: ProjectId,
        status: SearchStatus,
        pinned: bool,
        archived: bool,
        position: u64,
    ) -> SearchDocument {
        let id = session_id(id);
        SearchDocument::new(SearchDocumentInput {
            id: SearchDocumentId::Session(id),
            title: title.to_string(),
            project_id: Some(project),
            project_label: Some("Console".to_string()),
            group_label: Some("Auth".to_string()),
            preset_label: Some("Codex".to_string()),
            runtime_label: Some("codex".to_string()),
            status,
            pinned,
            archived,
            position: PositionKey::new(position),
            meaningful_activity_at: id.as_uuid().as_u128() as u64,
            action: SearchAction::OpenSession(id),
        })
        .unwrap()
    }

    #[test]
    fn search_nfkc_casefold_ranking_and_highlights_are_deterministic() {
        let project = project_id(1);
        let documents = vec![
            session_document(
                3,
                "Straße console",
                project,
                SearchStatus::Running,
                false,
                false,
                3,
            ),
            session_document(
                2,
                "STRASSE worker",
                project,
                SearchStatus::Attention,
                true,
                false,
                2,
            ),
            session_document(
                1,
                "Straße cafe\u{301} archive",
                project,
                SearchStatus::Done,
                true,
                true,
                1,
            ),
        ];
        let query = SearchQuery::parse("strasse").unwrap();
        let cancellation = SearchCancellation::default();

        let mut forward = SearchIndex::default();
        for document in documents.clone() {
            forward.insert(document).unwrap();
        }
        let mut reverse = SearchIndex::default();
        for document in documents.into_iter().rev() {
            reverse.insert(document).unwrap();
        }
        let first = forward
            .search(&query, Some(project), &cancellation)
            .unwrap();
        let second = reverse
            .search(&query, Some(project), &cancellation)
            .unwrap();
        assert_eq!(
            first
                .results
                .iter()
                .map(|result| result.id)
                .collect::<Vec<_>>(),
            second
                .results
                .iter()
                .map(|result| result.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            first.results[0].id,
            SearchDocumentId::Session(session_id(2))
        );
        assert!(!first.results[0].highlights.is_empty());
        assert!(first.results.iter().all(|result| !result.archived));

        let canonical = forward
            .search(
                &SearchQuery::parse("café").unwrap(),
                Some(project),
                &cancellation,
            )
            .unwrap();
        assert!(canonical.archived_fallback);
        assert_eq!(canonical.results.len(), 1);
        assert_eq!(canonical.results[0].highlights[0].start, "Straße ".len());
    }

    #[test]
    fn search_archive_fallback_and_filters_are_exact() {
        let project = project_id(1);
        let mut index = SearchIndex::default();
        index
            .insert(session_document(
                1,
                "Active parser",
                project,
                SearchStatus::Running,
                false,
                false,
                1,
            ))
            .unwrap();
        index
            .insert(session_document(
                2,
                "Archived parser",
                project,
                SearchStatus::Done,
                false,
                true,
                2,
            ))
            .unwrap();
        let cancellation = SearchCancellation::default();
        let fallback = index
            .search(
                &SearchQuery::parse("archived").unwrap(),
                None,
                &cancellation,
            )
            .unwrap();
        assert!(fallback.archived_fallback);
        assert_eq!(fallback.results.len(), 1);
        let explicit = index
            .search(
                &SearchQuery::parse("is:archived parser").unwrap(),
                None,
                &cancellation,
            )
            .unwrap();
        assert!(!explicit.archived_fallback);
        assert_eq!(explicit.results.len(), 1);
        assert!(explicit.results[0].archived);
        let running = index
            .search(
                &SearchQuery::parse("is:running project:console runtime:codex parser").unwrap(),
                None,
                &cancellation,
            )
            .unwrap();
        assert_eq!(running.results.len(), 1);
        assert_eq!(
            running.results[0].id,
            SearchDocumentId::Session(session_id(1))
        );
        let attention_query = SearchQuery::parse("is:attention parser").unwrap();
        assert!(attention_query.filters.contains(&Filter::Attention));
        assert!(
            index
                .search(&attention_query, None, &cancellation)
                .unwrap()
                .results
                .is_empty()
        );
        let unsupported = SearchQuery::parse("owner:alice").unwrap();
        assert!(unsupported.filters.is_empty());
    }

    #[test]
    fn search_bounds_results_queries_documents_and_cancellation() {
        assert!(matches!(
            SearchQuery::parse(&"x".repeat(MAX_SEARCH_QUERY_SCALARS + 1)),
            Err(SearchError::QueryTooLong)
        ));
        assert!(matches!(
            SearchQuery::parse(&vec!["x"; MAX_SEARCH_QUERY_TOKENS + 1].join(" ")),
            Err(SearchError::TooManyQueryTokens)
        ));
        let oversized = SearchDocument::new(SearchDocumentInput {
            id: SearchDocumentId::Project(project_id(1)),
            title: "x".repeat(MAX_SEARCH_DOCUMENT_BYTES + 1),
            project_id: Some(project_id(1)),
            project_label: None,
            group_label: None,
            preset_label: None,
            runtime_label: None,
            status: SearchStatus::Unknown,
            pinned: false,
            archived: false,
            position: PositionKey::FIRST,
            meaningful_activity_at: 0,
            action: SearchAction::OpenProject(project_id(1)),
        });
        assert!(matches!(oversized, Err(SearchError::DocumentTooLarge)));

        let mut index = SearchIndex::default();
        for id in 1..=150 {
            index
                .insert(session_document(
                    id,
                    "matching session",
                    project_id(1),
                    SearchStatus::Unknown,
                    false,
                    false,
                    id as u64,
                ))
                .unwrap();
        }
        let query = SearchQuery::parse("matching").unwrap();
        let cancellation = SearchCancellation::default();
        assert_eq!(
            index
                .search(&query, None, &cancellation)
                .unwrap()
                .results
                .len(),
            MAX_SEARCH_RESULTS
        );
        cancellation.cancel();
        assert!(matches!(
            index.search(&query, None, &cancellation),
            Err(SearchError::Cancelled)
        ));

        let mut bounded = SearchIndex::default();
        for id in 1..=MAX_SESSIONS_PER_PROJECT as u128 {
            bounded
                .insert(session_document(
                    id,
                    "bounded",
                    project_id(2),
                    SearchStatus::Unknown,
                    false,
                    false,
                    id as u64,
                ))
                .unwrap();
        }
        assert!(matches!(
            bounded.insert(session_document(
                MAX_SESSIONS_PER_PROJECT as u128 + 1,
                "overflow",
                project_id(2),
                SearchStatus::Unknown,
                false,
                false,
                MAX_SESSIONS_PER_PROJECT as u64 + 1,
            )),
            Err(SearchError::ResourceLimit {
                kind: SearchCategory::Session,
                limit: MAX_SESSIONS_PER_PROJECT,
            })
        ));
    }

    #[test]
    fn search_viewing_is_stable_and_diagnostics_redact_user_text() {
        let project = project_id(1);
        let document = session_document(
            1,
            "private customer title",
            project,
            SearchStatus::Idle,
            true,
            false,
            1,
        );
        assert!(!format!("{document:?}").contains("private customer title"));
        let query = SearchQuery::parse("project:private-customer runtime:secret").unwrap();
        let query_debug = format!("{query:?}");
        assert!(!query_debug.contains("private-customer"));
        assert!(!query_debug.contains("secret"));
        let query = SearchQuery::parse("private customer title").unwrap();
        assert!(!format!("{query:?}").contains("private"));
        let mut index = SearchIndex::default();
        index.insert(document).unwrap();
        let cancellation = SearchCancellation::default();
        let first = index.search(&query, None, &cancellation).unwrap();
        let second = index.search(&query, None, &cancellation).unwrap();
        assert_eq!(first.results[0].score, second.results[0].score);
        assert!(!format!("{:?}", first.results[0]).contains("private customer title"));
    }
}
