use std::collections::BTreeSet;

pub const MAX_PROJECTS: usize = 1_000;
pub const MAX_VISIBLE_SESSIONS: usize = 10_000;
pub const MAX_FILTER_SCALARS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FleetRevision {
    pub projects: u64,
    pub sessions: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectAvailability {
    Available,
    Unavailable,
    PermissionDenied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FleetGroup {
    pub id: String,
    pub project_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FleetProject {
    pub id: String,
    pub name: String,
    pub availability: ProjectAvailability,
    pub groups: Vec<FleetGroup>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FleetSession {
    pub id: String,
    pub project_id: String,
    pub group_id: Option<String>,
    pub title: String,
    pub state: String,
    pub activity: String,
    pub unread: bool,
    pub pinned: bool,
    pub archived: bool,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FleetHealth {
    Healthy,
    RecoveredLastGood,
    Partial,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FleetSnapshot {
    pub revision: FleetRevision,
    pub projects: Vec<FleetProject>,
    pub sessions: Vec<FleetSession>,
    pub health: FleetHealth,
    pub skipped_records: usize,
}

impl FleetSnapshot {
    pub fn empty() -> Self {
        Self {
            revision: FleetRevision {
                projects: 0,
                sessions: 0,
            },
            projects: Vec::new(),
            sessions: Vec::new(),
            health: FleetHealth::Healthy,
            skipped_records: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadState {
    Starting,
    Loading,
    Ready,
    Empty,
    Partial,
    Unavailable,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaneFocus {
    Projects,
    Sessions,
    Inspector,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ScopeId {
    All,
    Project(String),
    Group(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuiDiagnostic {
    pub code: &'static str,
    pub summary: &'static str,
    pub recovery: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelAction {
    Move(i8),
    FocusNext,
    FocusPrevious,
    Expand,
    Collapse,
    Activate,
    StartFilter,
    FilterCharacter(char),
    FilterBackspace,
    FinishFilter,
    Escape,
    ToggleInspector,
    ToggleHelp,
    BeginRefresh,
    RefreshSucceeded {
        generation: u64,
        snapshot: FleetSnapshot,
    },
    RefreshFailed {
        generation: u64,
        diagnostic: TuiDiagnostic,
        recovery_required: bool,
    },
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelEffect {
    None,
    StartRefresh(u64),
    CancelRefresh,
    Quit,
}

#[derive(Clone, Debug)]
pub struct TuiModel {
    load_state: LoadState,
    snapshot: Option<FleetSnapshot>,
    focus: PaneFocus,
    selected_scope: ScopeId,
    selected_session_id: Option<String>,
    expanded_projects: BTreeSet<String>,
    scope_rows: Vec<ScopeId>,
    session_rows: Vec<usize>,
    filter: String,
    filter_editing: bool,
    inspector_visible: bool,
    help_visible: bool,
    diagnostic: Option<TuiDiagnostic>,
    refresh_generation: u64,
    refresh_active: bool,
}

impl Default for TuiModel {
    fn default() -> Self {
        Self {
            load_state: LoadState::Starting,
            snapshot: None,
            focus: PaneFocus::Projects,
            selected_scope: ScopeId::All,
            selected_session_id: None,
            expanded_projects: BTreeSet::new(),
            scope_rows: vec![ScopeId::All],
            session_rows: Vec::new(),
            filter: String::new(),
            filter_editing: false,
            inspector_visible: true,
            help_visible: false,
            diagnostic: None,
            refresh_generation: 0,
            refresh_active: false,
        }
    }
}

impl TuiModel {
    pub fn load_state(&self) -> LoadState {
        self.load_state
    }

    pub fn snapshot(&self) -> Option<&FleetSnapshot> {
        self.snapshot.as_ref()
    }

    pub fn focus(&self) -> PaneFocus {
        self.focus
    }

    pub fn selected_scope(&self) -> &ScopeId {
        &self.selected_scope
    }

    pub fn scope_rows(&self) -> &[ScopeId] {
        &self.scope_rows
    }

    pub fn is_project_expanded(&self, id: &str) -> bool {
        self.expanded_projects.contains(id)
    }

    pub fn visible_sessions(&self) -> impl Iterator<Item = &FleetSession> {
        self.snapshot.iter().flat_map(|snapshot| {
            self.session_rows
                .iter()
                .map(|index| &snapshot.sessions[*index])
        })
    }

    pub fn selected_session(&self) -> Option<&FleetSession> {
        let id = self.selected_session_id.as_deref()?;
        self.snapshot
            .as_ref()?
            .sessions
            .iter()
            .find(|session| session.id == id)
    }

    pub fn selected_scope_index(&self) -> usize {
        self.scope_rows
            .iter()
            .position(|scope| scope == &self.selected_scope)
            .unwrap_or(0)
    }

    pub fn selected_session_index(&self) -> usize {
        let selected = self.selected_session_id.as_deref();
        self.session_rows
            .iter()
            .position(|index| {
                self.snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.sessions.get(*index))
                    .is_some_and(|session| Some(session.id.as_str()) == selected)
            })
            .unwrap_or(0)
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn filter_editing(&self) -> bool {
        self.filter_editing
    }

    pub fn inspector_visible(&self) -> bool {
        self.inspector_visible
    }

    pub fn help_visible(&self) -> bool {
        self.help_visible
    }

    pub fn diagnostic(&self) -> Option<&TuiDiagnostic> {
        self.diagnostic.as_ref()
    }

    pub fn refresh_active(&self) -> bool {
        self.refresh_active
    }

    pub fn reduce(&mut self, action: ModelAction) -> ModelEffect {
        match action {
            ModelAction::Move(delta) => self.move_selection(delta),
            ModelAction::FocusNext => self.focus = self.next_focus(false),
            ModelAction::FocusPrevious => self.focus = self.next_focus(true),
            ModelAction::Expand => self.expand_selected(),
            ModelAction::Collapse => self.collapse_selected(),
            ModelAction::Activate => self.activate_selected(),
            ModelAction::StartFilter => self.filter_editing = true,
            ModelAction::FilterCharacter(character) => self.add_filter_character(character),
            ModelAction::FilterBackspace => {
                self.filter.pop();
                self.rebuild_sessions();
            }
            ModelAction::FinishFilter => self.filter_editing = false,
            ModelAction::Escape => {
                if self.refresh_active {
                    self.refresh_active = false;
                    self.load_state = self.state_for_snapshot();
                    return ModelEffect::CancelRefresh;
                }
                if self.help_visible {
                    self.help_visible = false;
                } else if self.filter_editing || !self.filter.is_empty() {
                    self.filter_editing = false;
                    self.filter.clear();
                    self.rebuild_sessions();
                }
            }
            ModelAction::ToggleInspector => {
                if self.focus == PaneFocus::Inspector {
                    self.inspector_visible = false;
                    self.focus = PaneFocus::Sessions;
                } else {
                    self.inspector_visible = true;
                    self.focus = PaneFocus::Inspector;
                }
            }
            ModelAction::ToggleHelp => self.help_visible = !self.help_visible,
            ModelAction::BeginRefresh => {
                if self.refresh_active {
                    return ModelEffect::None;
                }
                self.refresh_generation = self.refresh_generation.saturating_add(1);
                self.refresh_active = true;
                self.load_state = LoadState::Loading;
                return ModelEffect::StartRefresh(self.refresh_generation);
            }
            ModelAction::RefreshSucceeded {
                generation,
                snapshot,
            } => {
                if !self.refresh_active || generation != self.refresh_generation {
                    return ModelEffect::None;
                }
                self.refresh_active = false;
                self.diagnostic = None;
                self.apply_snapshot(snapshot);
            }
            ModelAction::RefreshFailed {
                generation,
                diagnostic,
                recovery_required,
            } => {
                if !self.refresh_active || generation != self.refresh_generation {
                    return ModelEffect::None;
                }
                self.refresh_active = false;
                self.diagnostic = Some(diagnostic);
                self.load_state = if recovery_required {
                    LoadState::RecoveryRequired
                } else {
                    LoadState::Unavailable
                };
            }
            ModelAction::Quit => return ModelEffect::Quit,
        }
        ModelEffect::None
    }

    fn apply_snapshot(&mut self, snapshot: FleetSnapshot) {
        self.snapshot = Some(snapshot);
        self.rebuild_scopes();
        self.rebuild_sessions();
        self.load_state = self.state_for_snapshot();
    }

    fn state_for_snapshot(&self) -> LoadState {
        match self.snapshot.as_ref() {
            None => LoadState::Starting,
            Some(snapshot) if snapshot.projects.is_empty() && snapshot.sessions.is_empty() => {
                LoadState::Empty
            }
            Some(snapshot) if snapshot.health == FleetHealth::RecoveredLastGood => {
                LoadState::RecoveryRequired
            }
            Some(snapshot) if snapshot.health == FleetHealth::Partial => LoadState::Partial,
            Some(_) => LoadState::Ready,
        }
    }

    fn rebuild_scopes(&mut self) {
        self.scope_rows.clear();
        self.scope_rows.push(ScopeId::All);
        if let Some(snapshot) = &self.snapshot {
            for project in &snapshot.projects {
                self.scope_rows.push(ScopeId::Project(project.id.clone()));
                if self.expanded_projects.contains(&project.id) {
                    self.scope_rows.extend(
                        project
                            .groups
                            .iter()
                            .map(|group| ScopeId::Group(group.id.clone())),
                    );
                }
            }
        }
        if !self.scope_rows.contains(&self.selected_scope) {
            self.selected_scope = ScopeId::All;
        }
    }

    fn rebuild_sessions(&mut self) {
        let normalized = self.filter.to_lowercase();
        let selected_scope = self.selected_scope.clone();
        self.session_rows = if let Some(snapshot) = &self.snapshot {
            snapshot
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, session)| match &selected_scope {
                    ScopeId::All => true,
                    ScopeId::Project(id) => &session.project_id == id,
                    ScopeId::Group(id) => session.group_id.as_ref() == Some(id),
                })
                .filter(|(_, session)| {
                    normalized.is_empty()
                        || session.title.to_lowercase().contains(&normalized)
                        || session.state.contains(&normalized)
                        || session.activity.contains(&normalized)
                })
                .map(|(index, _)| index)
                .take(MAX_VISIBLE_SESSIONS)
                .collect()
        } else {
            Vec::new()
        };
        if !self.session_rows.iter().any(|index| {
            self.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.sessions.get(*index))
                .is_some_and(|session| {
                    Some(session.id.as_str()) == self.selected_session_id.as_deref()
                })
        }) {
            self.selected_session_id = self.session_rows.first().and_then(|index| {
                self.snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.sessions.get(*index))
                    .map(|session| session.id.clone())
            });
        }
    }

    fn move_selection(&mut self, delta: i8) {
        match self.focus {
            PaneFocus::Projects => {
                let current = self
                    .scope_rows
                    .iter()
                    .position(|scope| scope == &self.selected_scope)
                    .unwrap_or(0);
                let next = bounded_move(current, self.scope_rows.len(), delta);
                if let Some(scope) = self.scope_rows.get(next).cloned() {
                    self.selected_scope = scope;
                    self.rebuild_sessions();
                }
            }
            PaneFocus::Sessions | PaneFocus::Inspector => {
                let current = self
                    .selected_session_id
                    .as_deref()
                    .and_then(|id| {
                        self.session_rows.iter().position(|index| {
                            self.snapshot
                                .as_ref()
                                .and_then(|snapshot| snapshot.sessions.get(*index))
                                .is_some_and(|session| session.id == id)
                        })
                    })
                    .unwrap_or(0);
                let next = bounded_move(current, self.session_rows.len(), delta);
                self.selected_session_id = self.session_rows.get(next).and_then(|index| {
                    self.snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.sessions.get(*index))
                        .map(|session| session.id.clone())
                });
            }
        }
    }

    fn next_focus(&self, backwards: bool) -> PaneFocus {
        match (self.focus, backwards, self.inspector_visible) {
            (PaneFocus::Projects, false, _) => PaneFocus::Sessions,
            (PaneFocus::Sessions, false, true) => PaneFocus::Inspector,
            (PaneFocus::Sessions, false, false) | (PaneFocus::Inspector, false, _) => {
                PaneFocus::Projects
            }
            (PaneFocus::Projects, true, true) => PaneFocus::Inspector,
            (PaneFocus::Projects, true, false) => PaneFocus::Sessions,
            (PaneFocus::Sessions, true, _) => PaneFocus::Projects,
            (PaneFocus::Inspector, true, _) => PaneFocus::Sessions,
        }
    }

    fn expand_selected(&mut self) {
        if let ScopeId::Project(id) = &self.selected_scope {
            self.expanded_projects.insert(id.clone());
            self.rebuild_scopes();
        }
    }

    fn collapse_selected(&mut self) {
        match &self.selected_scope {
            ScopeId::Project(id) => {
                self.expanded_projects.remove(id);
                self.rebuild_scopes();
            }
            ScopeId::Group(group_id) => {
                if let Some(project) = self.snapshot.as_ref().and_then(|snapshot| {
                    snapshot
                        .projects
                        .iter()
                        .find(|project| project.groups.iter().any(|group| &group.id == group_id))
                }) {
                    self.selected_scope = ScopeId::Project(project.id.clone());
                    self.rebuild_sessions();
                }
            }
            ScopeId::All => {}
        }
    }

    fn activate_selected(&mut self) {
        if let ScopeId::Project(id) = &self.selected_scope {
            let id = id.clone();
            if !self.expanded_projects.remove(&id) {
                self.expanded_projects.insert(id);
            }
            self.rebuild_scopes();
        }
    }

    fn add_filter_character(&mut self, character: char) {
        if self.filter.chars().count() >= MAX_FILTER_SCALARS || character.is_control() {
            return;
        }
        self.filter.push(character);
        self.rebuild_sessions();
    }
}

fn bounded_move(current: usize, count: usize, delta: i8) -> usize {
    if count == 0 {
        return 0;
    }
    if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        current
            .saturating_add(delta as usize)
            .min(count.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> FleetSnapshot {
        FleetSnapshot {
            revision: FleetRevision {
                projects: 3,
                sessions: 7,
            },
            projects: vec![FleetProject {
                id: "project-a".into(),
                name: "Alpha".into(),
                availability: ProjectAvailability::Available,
                groups: vec![FleetGroup {
                    id: "group-a".into(),
                    project_id: "project-a".into(),
                    name: "Review".into(),
                }],
            }],
            sessions: vec![
                FleetSession {
                    id: "session-a".into(),
                    project_id: "project-a".into(),
                    group_id: None,
                    title: "Build".into(),
                    state: "live".into(),
                    activity: "busy".into(),
                    unread: true,
                    pinned: false,
                    archived: false,
                    revision: 4,
                },
                FleetSession {
                    id: "session-b".into(),
                    project_id: "project-a".into(),
                    group_id: Some("group-a".into()),
                    title: "Review tests".into(),
                    state: "offline".into(),
                    activity: "idle".into(),
                    unread: false,
                    pinned: true,
                    archived: false,
                    revision: 5,
                },
            ],
            health: FleetHealth::Healthy,
            skipped_records: 0,
        }
    }

    #[test]
    fn reducer_preserves_stable_selection_and_filters_deterministically() {
        let mut model = TuiModel::default();
        assert_eq!(
            model.reduce(ModelAction::BeginRefresh),
            ModelEffect::StartRefresh(1)
        );
        model.reduce(ModelAction::RefreshSucceeded {
            generation: 1,
            snapshot: snapshot(),
        });
        assert_eq!(model.load_state(), LoadState::Ready);
        assert_eq!(model.selected_session().unwrap().id, "session-a");

        model.reduce(ModelAction::FocusNext);
        model.reduce(ModelAction::Move(1));
        assert_eq!(model.selected_session().unwrap().id, "session-b");
        model.reduce(ModelAction::StartFilter);
        for character in "build".chars() {
            model.reduce(ModelAction::FilterCharacter(character));
        }
        assert_eq!(model.selected_session().unwrap().id, "session-a");
        assert_eq!(model.visible_sessions().count(), 1);
    }

    #[test]
    fn stale_refresh_results_and_cancelled_refresh_cannot_replace_snapshot() {
        let mut model = TuiModel::default();
        model.reduce(ModelAction::BeginRefresh);
        model.reduce(ModelAction::RefreshSucceeded {
            generation: 1,
            snapshot: snapshot(),
        });
        model.reduce(ModelAction::BeginRefresh);
        assert_eq!(
            model.reduce(ModelAction::Escape),
            ModelEffect::CancelRefresh
        );
        model.reduce(ModelAction::RefreshSucceeded {
            generation: 2,
            snapshot: FleetSnapshot::empty(),
        });
        assert_eq!(model.snapshot().unwrap().revision.sessions, 7);
        assert_eq!(model.load_state(), LoadState::Ready);
    }

    #[test]
    fn group_navigation_expands_collapses_and_returns_to_parent() {
        let mut model = TuiModel::default();
        model.reduce(ModelAction::BeginRefresh);
        model.reduce(ModelAction::RefreshSucceeded {
            generation: 1,
            snapshot: snapshot(),
        });
        model.reduce(ModelAction::Move(1));
        model.reduce(ModelAction::Expand);
        assert_eq!(model.scope_rows().len(), 3);
        model.reduce(ModelAction::Move(1));
        assert_eq!(model.selected_scope(), &ScopeId::Group("group-a".into()));
        model.reduce(ModelAction::Collapse);
        assert_eq!(
            model.selected_scope(),
            &ScopeId::Project("project-a".into())
        );
    }

    #[test]
    fn filter_is_bounded_and_rejects_control_characters() {
        let mut model = TuiModel::default();
        for _ in 0..MAX_FILTER_SCALARS + 20 {
            model.reduce(ModelAction::FilterCharacter('x'));
        }
        model.reduce(ModelAction::FilterCharacter('\u{1b}'));
        assert_eq!(model.filter().chars().count(), MAX_FILTER_SCALARS);
        assert!(!model.filter().contains('\u{1b}'));
    }
}
