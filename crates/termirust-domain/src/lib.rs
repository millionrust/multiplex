pub mod activity;
pub mod group;
pub mod host;
pub mod id;
pub mod preset;
pub mod project;
pub mod runtime;
pub mod search;
pub mod session;
pub mod transcript;

pub use activity::{
    ActivityAggregate, ActivityConfidence, ActivityError, ActivityEvidence, ActivityEvidenceKind,
    ActivitySourceKind, ActivityState, AttentionReason, HEURISTIC_IDLE_QUIET_NANOS, HostSequence,
    MAX_ACTIVITY_SOURCE_ID_BYTES, ReadWatermark, reduce_activity, refresh_activity_staleness,
};
pub use group::{
    Group, GroupDestination, GroupError, GroupInverseCommand, GroupMutation, GroupName,
    MAX_GROUP_NAME_SCALARS, MAX_GROUPS_PER_PROJECT, validate_group_set,
};
pub use host::{DurabilityWatermark, HostLifecycle, ProcessToken};
pub use id::{
    CommandId, GroupId, HostInstanceId, HostedSessionId, OutputSequence, PositionError,
    PositionKey, PresetId, ProjectId, Revision,
};
pub use preset::{
    DetectionCandidate, DetectionReport, DetectionStatus, ExecutableSpec, LaunchPreset,
    MAX_ARGUMENT_BYTES, MAX_ARGUMENTS, MAX_DETECTION_CANDIDATES, MAX_EXECUTABLE_BYTES, MAX_PRESETS,
    MAX_RESOLVED_LAUNCH_BYTES, OsStringValue, PermissionPolicy, PresetDraft, PresetError,
    PresetOrigin, PresetRisk, PresetService, RuntimeId, WorkingDirectoryRule,
    classify_argument_strings, classify_arguments,
};
pub use project::{
    AddProject, CanonicalPath, FileIdentity, LocalizedUserText, Project, ProjectError,
    ProjectService, ProjectStatus, ProjectSummary,
};
pub use runtime::{
    ExecutableFingerprint, MAX_RUNTIME_CANDIDATES, MAX_RUNTIME_DESCRIPTORS,
    MAX_RUNTIME_VERSION_BYTES, ObservedProcess, OccupantGeneration, OccupantOwnership,
    ProcessIdentity, ProcessObservation, ProcessObservationStatus, RecognitionConfidence,
    RuntimeCapability, RuntimeCapabilitySet, RuntimeDescriptor, RuntimeDescriptorKind,
    RuntimeDetectionResult, RuntimeDetectionStatus, RuntimeLaunchMode, RuntimeOccupant,
    RuntimeRecognition, RuntimeVersion, RuntimeVersionRule, compiled_runtime_descriptors,
    parse_runtime_version,
};
pub use search::{
    Filter, HighlightField, MAX_SEARCH_DOCUMENT_BYTES, MAX_SEARCH_QUERY_SCALARS,
    MAX_SEARCH_QUERY_TOKENS, MAX_SEARCH_RESULTS, ScoreTuple, SearchAction, SearchActionId,
    SearchCancellation, SearchCategory, SearchDocument, SearchDocumentId, SearchDocumentInput,
    SearchError, SearchIndex, SearchPage, SearchQuery, SearchResult, SearchStatus, TextHighlight,
};
pub use session::{
    HostedSession, HostedSessionState, LaunchResolutionError, MAX_AUTOMATIC_TITLE_GRAPHEMES,
    MAX_PATH_SEARCH_DIRECTORIES, MAX_SESSION_TITLE_SCALARS, MAX_SESSIONS_PER_PROJECT,
    ResolvedLaunch, SessionLaunchRoute, SessionMutation, SessionOrigin, SessionStateError,
    SessionTitle, TitleSource, automatic_title_from_explicit_input, reduce_session, resolve_launch,
};
pub use transcript::{
    ExportManifest, MAX_PROVIDER_CONTRACT_BYTES, MAX_PROVIDER_RECORD_REF_BYTES,
    MAX_TRANSCRIPT_EXPORTED_ENTRIES, MAX_TRANSCRIPT_OUTPUT_BYTES, MAX_TRANSCRIPT_PAGE_ENTRIES,
    MAX_TRANSCRIPT_RECORD_BYTES, MAX_TRANSCRIPT_SCANNED_RECORDS, NormalizedTranscript,
    ProviderRecordRef, TranscriptCancellation, TranscriptCategorySet, TranscriptContent,
    TranscriptEntry, TranscriptError, TranscriptKind, TranscriptLimits, TranscriptPage,
    TranscriptRange, TranscriptRequest, deterministic_content_hash, escape_markdown_text,
    normalize_transcript_content, render_transcript_entry_markdown,
    render_transcript_entry_markdown_with_label,
};
