#[cfg(test)]
use std::path::Path;

use termirust_domain::{
    ActivityAggregate, GroupId, HostedSession, HostedSessionId, HostedSessionState, OutputSequence,
    ProjectId, Revision, SessionMutation, SessionStateError,
};
use termirust_store::{
    SessionRemovalPlan, SessionRepository, SessionSnapshot, StoreError, StoreHealth,
};

use crate::models::{SavedAppAttachedSession, SavedSessionPlacement, SavedState};
use crate::storage::{app_dir, project_store_dir};
use crate::ui::util::current_unix_millis;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum SessionLibraryView {
    #[default]
    Active,
    Archive,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum SessionLibraryFilter {
    #[default]
    All,
    Unread,
    Pinned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionLibraryFailure {
    Corrupt,
    Newer,
    PermissionDenied,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionLibraryRecovery {
    RecoveredLastGood,
    Corrupt,
    Newer,
    PermissionDenied,
    Unavailable,
}

pub(super) enum SessionLibraryLoadState {
    Ready,
    Failed(SessionLibraryFailure),
}

pub(super) struct SessionLibraryState {
    pub repository: Option<SessionRepository>,
    pub snapshot: Option<SessionSnapshot>,
    pub load_state: SessionLibraryLoadState,
    pub view: SessionLibraryView,
    pub filter: SessionLibraryFilter,
    pub pending_stop_archive_review: Option<HostedSessionId>,
    pub pending_archive_after_stop: Option<HostedSessionId>,
    pub pending_removal: Option<SessionRemovalPlan>,
    pub renaming: Option<HostedSessionId>,
}

impl SessionLibraryState {
    pub fn open_default(saved: &mut SavedState) -> Self {
        let Ok(root) = project_store_dir() else {
            return Self::failed(SessionLibraryFailure::Unavailable);
        };
        let Ok(data_root) = app_dir().map(|root| root.join("durable-sessions")) else {
            return Self::failed(SessionLibraryFailure::Unavailable);
        };
        match SessionRepository::open(root, data_root) {
            Ok(repository) => Self::open_repository(repository, saved),
            Err(error) => Self::failed(classify_store_failure(&error)),
        }
    }

    pub(super) fn open_repository(repository: SessionRepository, saved: &mut SavedState) -> Self {
        let mut state = Self {
            repository: Some(repository),
            snapshot: None,
            load_state: SessionLibraryLoadState::Ready,
            view: SessionLibraryView::Active,
            filter: SessionLibraryFilter::All,
            pending_stop_archive_review: None,
            pending_archive_after_stop: None,
            pending_removal: None,
            renaming: None,
        };
        if let Err(error) = state.reload() {
            state.load_state = SessionLibraryLoadState::Failed(classify_store_failure(&error));
            return state;
        }
        if let Err(error) = state.migrate_saved_records(saved) {
            state.load_state = SessionLibraryLoadState::Failed(classify_store_failure(&error));
            return state;
        }
        state.sync_saved_projection(saved, false);
        state
    }

    fn failed(failure: SessionLibraryFailure) -> Self {
        Self {
            repository: None,
            snapshot: None,
            load_state: SessionLibraryLoadState::Failed(failure),
            view: SessionLibraryView::Active,
            filter: SessionLibraryFilter::All,
            pending_stop_archive_review: None,
            pending_archive_after_stop: None,
            pending_removal: None,
            renaming: None,
        }
    }

    pub fn reload(&mut self) -> Result<(), StoreError> {
        let repository = self
            .repository
            .as_ref()
            .ok_or(StoreError::SessionDomain(SessionStateError::Unavailable))?;
        let snapshot = repository.load()?;
        self.load_state = SessionLibraryLoadState::Ready;
        self.snapshot = Some(snapshot);
        Ok(())
    }

    fn migrate_saved_records(&mut self, saved: &mut SavedState) -> Result<(), StoreError> {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.read_only)
        {
            return Ok(());
        }
        let mut records = saved.app_attached_sessions.clone();
        records.sort_by_key(|session| {
            (
                session.origin.project_id,
                session.group_id,
                session.position,
                session.id,
            )
        });
        for record in records {
            if self.session(record.id).is_some() {
                continue;
            }
            let session = record.to_hosted_session()?;
            let expected = self.revision();
            let repository = self
                .repository
                .as_ref()
                .ok_or(StoreError::SessionDomain(SessionStateError::Unavailable))?;
            repository.create_session(session, expected)?;
            self.reload()?;
        }
        Ok(())
    }

    pub fn create_from_saved(
        &mut self,
        saved: &mut SavedState,
        mut record: SavedAppAttachedSession,
    ) -> Result<HostedSession, StoreError> {
        let session = record.to_hosted_session()?;
        let repository = self
            .repository
            .as_ref()
            .ok_or(StoreError::SessionDomain(SessionStateError::Unavailable))?;
        let created = repository.create_session(session, self.revision())?;
        record.apply_hosted_session(&created);
        saved.upsert_app_attached_session(record);
        self.reload()?;
        Ok(created)
    }

    pub fn mutate(
        &mut self,
        saved: &mut SavedState,
        id: HostedSessionId,
        mutation: SessionMutation,
    ) -> Result<HostedSession, StoreError> {
        let repository = self
            .repository
            .as_ref()
            .ok_or(StoreError::SessionDomain(SessionStateError::Unavailable))?;
        let result =
            repository.mutate_session(id, self.revision(), mutation, current_unix_millis());
        match result {
            Ok(session) => {
                self.reload()?;
                apply_to_saved(saved, &session);
                Ok(session)
            }
            Err(error) => {
                if matches!(
                    error,
                    StoreError::SessionDomain(SessionStateError::StaleRevision { .. })
                ) {
                    let _ = self.reload();
                    self.sync_saved_projection(saved, false);
                }
                Err(error)
            }
        }
    }

    pub fn reconcile(
        &mut self,
        saved: &mut SavedState,
        id: HostedSessionId,
        lifecycle: HostedSessionState,
        activity: ActivityAggregate,
        through: OutputSequence,
    ) -> Result<HostedSession, StoreError> {
        if let Some(current) = self.session(id)
            && current.lifecycle == lifecycle
            && current.activity == activity
            && current.last_output_sequence >= through
        {
            return Ok(current.clone());
        }
        self.mutate(
            saved,
            id,
            SessionMutation::Reconcile {
                lifecycle,
                activity,
                through,
            },
        )
    }

    pub fn prepare_removal(&mut self, id: HostedSessionId) -> Result<(), StoreError> {
        let plan = self
            .repository
            .as_ref()
            .ok_or(StoreError::SessionDomain(SessionStateError::Unavailable))?
            .removal_plan(id)?;
        self.pending_removal = Some(plan);
        Ok(())
    }

    pub fn apply_placements(
        &mut self,
        saved: &mut SavedState,
        placements: &[SavedSessionPlacement],
    ) -> Result<(), StoreError> {
        let values = placements
            .iter()
            .map(|placement| (placement.id, placement.group_id, placement.position))
            .collect::<Vec<_>>();
        self.repository
            .as_ref()
            .ok_or(StoreError::SessionDomain(SessionStateError::Unavailable))?
            .apply_placements(&values, self.revision(), current_unix_millis())?;
        self.reload()?;
        self.sync_saved_projection(saved, false);
        Ok(())
    }

    pub fn confirm_removal(
        &mut self,
        saved: &mut SavedState,
        confirmation: &str,
    ) -> Result<termirust_store::QuarantinedSession, StoreError> {
        let plan = self
            .pending_removal
            .as_ref()
            .ok_or(StoreError::SessionDomain(SessionStateError::Unavailable))?;
        if plan.manifest.requires_title_confirmation() && confirmation != plan.title.as_str() {
            return Err(StoreError::SessionDomain(SessionStateError::Store {
                code: "title-confirmation-mismatch",
            }));
        }
        let removed = self
            .repository
            .as_ref()
            .ok_or(StoreError::SessionDomain(SessionStateError::Unavailable))?
            .remove_session(plan, self.revision())?;
        let id = removed.session.id;
        self.pending_removal = None;
        self.reload()?;
        saved.remove_app_attached_session(id);
        Ok(removed)
    }

    pub fn revision(&self) -> Revision {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.revision)
            .unwrap_or(Revision::ZERO)
    }

    pub fn session(&self, id: HostedSessionId) -> Option<&HostedSession> {
        self.snapshot
            .as_ref()?
            .sessions
            .iter()
            .find(|session| session.id == id)
    }

    pub fn visible_sessions(
        &self,
        project_id: ProjectId,
        group_id: Option<GroupId>,
    ) -> Vec<HostedSession> {
        let mut sessions = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .sessions
                    .iter()
                    .filter(|session| {
                        session.project_id == project_id
                            && session.group_id == group_id
                            && match self.view {
                                SessionLibraryView::Active => session.archived_at.is_none(),
                                SessionLibraryView::Archive => session.archived_at.is_some(),
                            }
                            && match self.filter {
                                SessionLibraryFilter::All => true,
                                SessionLibraryFilter::Unread => session.unread(),
                                SessionLibraryFilter::Pinned => session.pinned,
                            }
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        sessions.sort_by_key(|session| (!session.pinned, session.position, session.id));
        sessions
    }

    pub fn visible_sessions_all(&self) -> Vec<HostedSession> {
        let mut sessions = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .sessions
                    .iter()
                    .filter(|session| {
                        (match self.view {
                            SessionLibraryView::Active => session.archived_at.is_none(),
                            SessionLibraryView::Archive => session.archived_at.is_some(),
                        }) && match self.filter {
                            SessionLibraryFilter::All => true,
                            SessionLibraryFilter::Unread => session.unread(),
                            SessionLibraryFilter::Pinned => session.pinned,
                        }
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        sessions.sort_by_key(|session| {
            (
                session.project_id,
                !session.pinned,
                session.position,
                session.id,
            )
        });
        sessions
    }

    pub fn recovery_state(&self) -> Option<SessionLibraryRecovery> {
        match self.load_state {
            SessionLibraryLoadState::Ready => self
                .snapshot
                .as_ref()
                .filter(|snapshot| snapshot.health == StoreHealth::RecoveredLastGood)
                .map(|_| SessionLibraryRecovery::RecoveredLastGood),
            SessionLibraryLoadState::Failed(SessionLibraryFailure::Corrupt) => {
                Some(SessionLibraryRecovery::Corrupt)
            }
            SessionLibraryLoadState::Failed(SessionLibraryFailure::Newer) => {
                Some(SessionLibraryRecovery::Newer)
            }
            SessionLibraryLoadState::Failed(SessionLibraryFailure::PermissionDenied) => {
                Some(SessionLibraryRecovery::PermissionDenied)
            }
            SessionLibraryLoadState::Failed(SessionLibraryFailure::Unavailable) => {
                Some(SessionLibraryRecovery::Unavailable)
            }
        }
    }

    fn sync_saved_projection(&self, saved: &mut SavedState, prune_removed: bool) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let ids = snapshot
            .sessions
            .iter()
            .map(|session| session.id)
            .collect::<std::collections::HashSet<_>>();
        for metadata in &snapshot.sessions {
            apply_to_saved(saved, metadata);
        }
        if prune_removed {
            saved
                .app_attached_sessions
                .retain(|session| ids.contains(&session.id));
        }
    }
}

fn apply_to_saved(saved: &mut SavedState, metadata: &HostedSession) {
    if let Some(record) = saved
        .app_attached_sessions
        .iter_mut()
        .find(|record| record.id == metadata.id)
    {
        record.apply_hosted_session(metadata);
    }
}

fn classify_store_failure(error: &StoreError) -> SessionLibraryFailure {
    match error {
        StoreError::Corrupt { .. }
        | StoreError::UnsafeEntry { .. }
        | StoreError::TooLarge { .. } => SessionLibraryFailure::Corrupt,
        StoreError::StoreNewer { .. } => SessionLibraryFailure::Newer,
        StoreError::Io {
            kind: std::io::ErrorKind::PermissionDenied,
            ..
        } => SessionLibraryFailure::PermissionDenied,
        _ => SessionLibraryFailure::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SavedDurableHost, SavedState};
    use termirust_domain::{
        HostedSessionState, PositionKey, PresetId, SessionLaunchRoute, SessionOrigin, TitleSource,
    };

    fn record(state: HostedSessionState) -> SavedAppAttachedSession {
        let id = HostedSessionId::new();
        SavedAppAttachedSession {
            id,
            route: SessionLaunchRoute::DurableHost,
            origin: SessionOrigin {
                project_id: ProjectId::new(),
                preset_id: PresetId::new(),
            },
            state,
            project_label: "Project".to_string(),
            preset_label: "Codex".to_string(),
            title: "Investigate parser".to_string(),
            title_source: TitleSource::Manual,
            activity: ActivityAggregate::default(),
            pinned: false,
            read_through_sequence: 0,
            unread_sequence: None,
            archived_at: None,
            revision: Revision::ZERO,
            durable_host: Some(SavedDurableHost::default()),
            group_id: None,
            position: PositionKey::FIRST,
            started_at: 1,
            updated_at: 1,
        }
    }

    fn state_with_repository(saved: &mut SavedState, root: &Path) -> SessionLibraryState {
        let repository = SessionRepository::open(root.join("metadata"), root.join("sessions"))
            .expect("session repository should open");
        SessionLibraryState::open_repository(repository, saved)
    }

    #[test]
    fn migrated_session_library_survives_restart_and_filters_pin_unread_archive() {
        let fixture = tempfile::tempdir().unwrap();
        let mut saved = SavedState::default();
        let record = record(HostedSessionState::Exited);
        let id = record.id;
        let project_id = record.origin.project_id;
        saved.app_attached_sessions.push(record);
        let mut library = state_with_repository(&mut saved, fixture.path());
        library
            .mutate(&mut saved, id, SessionMutation::SetPinned(true))
            .unwrap();
        library
            .mutate(
                &mut saved,
                id,
                SessionMutation::ObserveOutput {
                    through: OutputSequence::new(4),
                },
            )
            .unwrap();
        library
            .mutate(
                &mut saved,
                id,
                SessionMutation::MarkUnread {
                    at: OutputSequence::new(4),
                },
            )
            .unwrap();
        library.filter = SessionLibraryFilter::Unread;
        assert_eq!(library.visible_sessions(project_id, None).len(), 1);
        library
            .mutate(&mut saved, id, SessionMutation::Archive { at: 9 })
            .unwrap();
        assert!(library.visible_sessions(project_id, None).is_empty());
        library.view = SessionLibraryView::Archive;
        assert_eq!(library.visible_sessions(project_id, None).len(), 1);

        let reopened = state_with_repository(&mut saved, fixture.path());
        let metadata = reopened.session(id).unwrap();
        assert!(metadata.pinned);
        assert!(metadata.unread());
        assert_eq!(metadata.archived_at, Some(9));
    }

    #[test]
    fn archived_restore_never_changes_exited_lifecycle() {
        let fixture = tempfile::tempdir().unwrap();
        let mut saved = SavedState::default();
        let record = record(HostedSessionState::Exited);
        let id = record.id;
        saved.app_attached_sessions.push(record);
        let mut library = state_with_repository(&mut saved, fixture.path());
        library
            .mutate(&mut saved, id, SessionMutation::Archive { at: 9 })
            .unwrap();
        let restored = library
            .mutate(&mut saved, id, SessionMutation::Restore)
            .unwrap();
        assert_eq!(restored.archived_at, None);
        assert_eq!(restored.lifecycle, HostedSessionState::Exited);
    }

    #[test]
    fn live_archive_is_rejected_and_pending_stop_is_explicit() {
        let fixture = tempfile::tempdir().unwrap();
        let mut saved = SavedState::default();
        let record = record(HostedSessionState::Live);
        let id = record.id;
        saved.app_attached_sessions.push(record);
        let mut library = state_with_repository(&mut saved, fixture.path());
        assert!(matches!(
            library.mutate(&mut saved, id, SessionMutation::Archive { at: 9 }),
            Err(StoreError::SessionDomain(
                SessionStateError::StopRequiredBeforeArchive
            ))
        ));
        library.pending_archive_after_stop = Some(id);
        assert_eq!(library.pending_archive_after_stop, Some(id));
        assert_eq!(library.session(id).unwrap().archived_at, None);
    }

    #[test]
    fn session_library_filters_preserve_pinned_order_and_archive_is_orthogonal() {
        let fixture = tempfile::tempdir().unwrap();
        let mut saved = SavedState::default();
        let first = record(HostedSessionState::Exited);
        let project_id = first.origin.project_id;
        let first_id = first.id;
        let mut second = record(HostedSessionState::Exited);
        second.origin.project_id = project_id;
        let second_id = second.id;
        saved.app_attached_sessions = vec![first, second];
        let mut library = state_with_repository(&mut saved, fixture.path());
        library
            .mutate(&mut saved, second_id, SessionMutation::SetPinned(true))
            .unwrap();
        library
            .mutate(
                &mut saved,
                first_id,
                SessionMutation::ObserveOutput {
                    through: OutputSequence::new(3),
                },
            )
            .unwrap();
        library
            .mutate(
                &mut saved,
                first_id,
                SessionMutation::MarkUnread {
                    at: OutputSequence::new(3),
                },
            )
            .unwrap();

        let visible = library.visible_sessions(project_id, None);
        assert_eq!(visible[0].id, second_id);
        assert_eq!(visible[1].id, first_id);
        library.filter = SessionLibraryFilter::Unread;
        assert_eq!(library.visible_sessions(project_id, None)[0].id, first_id);

        library.filter = SessionLibraryFilter::All;
        library
            .mutate(&mut saved, first_id, SessionMutation::Archive { at: 9 })
            .unwrap();
        assert_eq!(library.visible_sessions(project_id, None)[0].id, second_id);
        library.view = SessionLibraryView::Archive;
        assert_eq!(library.visible_sessions(project_id, None)[0].id, first_id);
        assert!(library.session(first_id).unwrap().unread());
    }

    #[test]
    fn unified_sessions_query_spans_projects_and_keeps_archive_orthogonal() {
        let fixture = tempfile::tempdir().unwrap();
        let mut saved = SavedState::default();
        let first = record(HostedSessionState::Live);
        let first_id = first.id;
        let first_project = first.origin.project_id;
        let mut second = record(HostedSessionState::RunningAppAttached);
        second.route = SessionLaunchRoute::LegacyAppAttached;
        second.durable_host = None;
        let second_id = second.id;
        let second_project = second.origin.project_id;
        let mut archived = record(HostedSessionState::Exited);
        archived.archived_at = Some(9);
        let archived_id = archived.id;
        saved.app_attached_sessions = vec![first, second, archived];

        let mut library = state_with_repository(&mut saved, fixture.path());
        let active = library.visible_sessions_all();
        assert_eq!(active.len(), 2);
        assert!(active.iter().any(|session| session.id == first_id));
        assert!(active.iter().any(|session| session.id == second_id));
        assert_ne!(first_project, second_project);

        library.view = SessionLibraryView::Archive;
        let archived = library.visible_sessions_all();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, archived_id);
    }
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    use termirust_domain::{
        ActivityConfidence, ActivitySourceKind, ActivityState, AttentionReason, HostSequence,
        PositionKey, PresetId, SessionLaunchRoute, SessionOrigin, TitleSource,
    };

    fn record() -> SavedAppAttachedSession {
        SavedAppAttachedSession {
            id: HostedSessionId::new(),
            route: SessionLaunchRoute::DurableHost,
            origin: SessionOrigin {
                project_id: ProjectId::new(),
                preset_id: PresetId::new(),
            },
            state: HostedSessionState::Live,
            project_label: "Project".to_string(),
            preset_label: "Agent".to_string(),
            title: "Attention fixture".to_string(),
            title_source: TitleSource::Manual,
            activity: ActivityAggregate::default(),
            pinned: false,
            read_through_sequence: 0,
            unread_sequence: None,
            archived_at: None,
            revision: Revision::ZERO,
            durable_host: Some(crate::models::SavedDurableHost::default()),
            group_id: None,
            position: PositionKey::FIRST,
            started_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn activity_attention_is_unread_but_output_and_row_selection_are_not_acknowledgements() {
        let fixture = tempfile::tempdir().unwrap();
        let mut saved = SavedState::default();
        let record = record();
        let id = record.id;
        saved.app_attached_sessions.push(record);
        let mut library = SessionLibraryState::open_repository(
            SessionRepository::open(
                fixture.path().join("metadata"),
                fixture.path().join("sessions"),
            )
            .unwrap(),
            &mut saved,
        );
        let output = library
            .mutate(
                &mut saved,
                id,
                SessionMutation::ObserveOutput {
                    through: OutputSequence::new(4),
                },
            )
            .unwrap();
        assert!(!output.unread());
        let activity = ActivityAggregate {
            state: ActivityState::NeedsInput,
            confidence: ActivityConfidence::Verified,
            effective_sequence: HostSequence::new(1),
            source_kind: ActivitySourceKind::Approval,
            source_id: "ui-test".to_string(),
            stale: false,
            attention_reason: Some(AttentionReason::Approval),
            attention_sequence: Some(OutputSequence::new(4)),
            ..ActivityAggregate::default()
        };
        let attention = library
            .mutate(
                &mut saved,
                id,
                SessionMutation::ApplyActivity {
                    activity,
                    visible_through: None,
                },
            )
            .unwrap();
        assert!(attention.unread());
        assert!(library.session(id).unwrap().unread());
    }

    #[test]
    fn activity_read_watermark_clears_only_the_sequence_actually_displayed() {
        let mut session = record().to_hosted_session().unwrap();
        termirust_domain::reduce_session(
            &mut session,
            SessionMutation::ObserveOutput {
                through: OutputSequence::new(6),
            },
        )
        .unwrap();
        let activity = ActivityAggregate {
            state: ActivityState::Failed,
            confidence: ActivityConfidence::Verified,
            effective_sequence: HostSequence::new(2),
            source_kind: ActivitySourceKind::ProcessExit,
            source_id: "ui-test".to_string(),
            stale: false,
            attention_sequence: Some(OutputSequence::new(6)),
            ..ActivityAggregate::default()
        };
        termirust_domain::reduce_session(
            &mut session,
            SessionMutation::ApplyActivity {
                activity,
                visible_through: Some(termirust_domain::ReadWatermark(OutputSequence::new(5))),
            },
        )
        .unwrap();
        assert_eq!(session.read_through_sequence, OutputSequence::new(5));
        assert_eq!(session.unread_sequence, Some(OutputSequence::new(6)));
    }
}

#[cfg(test)]
mod resume_tests {
    use super::super::session_resume::session_resume_projection;
    use crate::models::{SavedAppAttachedSession, SavedDurableHost};
    use termirust_domain::{
        ActivityAggregate, ExecutableFingerprint, HostInstanceId, HostedSessionId,
        HostedSessionState, OccupantGeneration, OccupantOwnership, PositionKey, PresetId,
        ProcessToken, ProjectId, RecognitionConfidence, ResumeError, Revision, RuntimeCapability,
        RuntimeCapabilitySet, RuntimeId, RuntimeOccupant, RuntimeRecognition, SessionLaunchRoute,
        SessionOrigin, TitleSource,
    };

    fn saved_session(
        lifecycle: HostedSessionState,
        runtime: &str,
        version: &str,
        ownership: OccupantOwnership,
    ) -> SavedAppAttachedSession {
        SavedAppAttachedSession {
            id: HostedSessionId::new(),
            route: SessionLaunchRoute::DurableHost,
            origin: SessionOrigin {
                project_id: ProjectId::new(),
                preset_id: PresetId::new(),
            },
            state: lifecycle,
            project_label: "Project".to_string(),
            preset_label: "Agent".to_string(),
            title: "Resume fixture".to_string(),
            title_source: TitleSource::Default,
            activity: ActivityAggregate::default(),
            pinned: false,
            read_through_sequence: 0,
            unread_sequence: None,
            archived_at: None,
            revision: Revision::new(4),
            durable_host: Some(SavedDurableHost {
                runtime_recognition: Some(RuntimeRecognition {
                    occupant: Some(RuntimeOccupant {
                        runtime_id: RuntimeId::new(runtime).unwrap(),
                        descriptor_version: 1,
                        safe_version: Some(version.to_string()),
                        executable_fingerprint: Some(ExecutableFingerprint {
                            file_identity: 1,
                            size: 1,
                            modified_nanos: 1,
                            bounded_content_hash: 1,
                        }),
                        generation: OccupantGeneration::new(3),
                        ownership,
                        capabilities: RuntimeCapabilitySet::new([
                            RuntimeCapability::InteractivePty,
                            RuntimeCapability::Resume,
                        ]),
                        stale: false,
                    }),
                    confidence: RecognitionConfidence::Verified,
                    observed_at_nanos: 1,
                }),
                ..SavedDurableHost::default()
            }),
            group_id: None,
            position: PositionKey::FIRST,
            started_at: 1,
            updated_at: 1,
        }
    }

    fn managed_ownership() -> OccupantOwnership {
        let host = HostInstanceId::new();
        OccupantOwnership::Managed {
            host_instance: host,
            child_token: ProcessToken::new(host, 7, 1),
        }
    }

    #[test]
    fn session_resume_exact_exited_codex_contract_is_visible_and_can_discover_its_handle() {
        let saved = saved_session(
            HostedSessionState::Exited,
            "codex",
            "0.150.1",
            managed_ownership(),
        );
        let metadata = saved.to_hosted_session().unwrap();

        let projection = session_resume_projection(&saved, &metadata);

        assert!(projection.visible);
        assert!(projection.enabled);
        assert_eq!(projection.reason, None);
    }

    #[test]
    fn session_resume_live_observed_and_unsupported_have_exact_reasons() {
        let live = saved_session(
            HostedSessionState::Live,
            "codex",
            "0.150.1",
            managed_ownership(),
        );
        let observed = saved_session(
            HostedSessionState::Exited,
            "codex",
            "0.150.1",
            OccupantOwnership::Observed {
                executable: ExecutableFingerprint {
                    file_identity: 1,
                    size: 1,
                    modified_nanos: 1,
                    bounded_content_hash: 1,
                },
            },
        );
        let unsupported = saved_session(
            HostedSessionState::Exited,
            "codex",
            "0.151.0",
            managed_ownership(),
        );

        let live_projection = session_resume_projection(&live, &live.to_hosted_session().unwrap());
        let observed_projection =
            session_resume_projection(&observed, &observed.to_hosted_session().unwrap());
        let unsupported_projection =
            session_resume_projection(&unsupported, &unsupported.to_hosted_session().unwrap());

        assert_eq!(live_projection.reason, Some(ResumeError::StillRunning));
        assert_eq!(
            observed_projection.reason,
            Some(ResumeError::OwnershipUnproven)
        );
        assert_eq!(
            unsupported_projection.reason,
            Some(ResumeError::UnsupportedVersion)
        );
        assert!(!live_projection.enabled);
        assert!(!observed_projection.enabled);
        assert!(!unsupported_projection.enabled);
    }

    #[test]
    fn session_resume_unrelated_runtime_does_not_advertise_resume() {
        let saved = saved_session(
            HostedSessionState::Exited,
            "claude",
            "1.0.0",
            managed_ownership(),
        );

        let projection = session_resume_projection(&saved, &saved.to_hosted_session().unwrap());

        assert!(!projection.visible);
        assert!(!projection.enabled);
        assert_eq!(projection.reason, None);
    }
}
