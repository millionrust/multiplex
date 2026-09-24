pub mod artifacts;
mod atomic;
pub mod console_sessions;
pub mod continuity;
pub mod controller_devices;
pub mod controller_network;
mod file_lock;
mod fleet;
pub mod health;
pub mod journal;
pub mod lease;
pub mod notifications;
pub mod presets;
pub mod projects;
pub mod recovery;
pub mod replication;
pub mod sessions;
pub mod transcript;

pub use artifacts::{
    ArtifactIngestProgress, ArtifactIngestRequest, ArtifactPayload, ArtifactRepository,
    ArtifactSnapshot, ArtifactStoreError, ArtifactSweepResult,
};
pub use atomic::{AtomicWriter, Durability, SystemAtomicWriter};
pub use console_sessions::{
    CONSOLE_SESSION_RECORD, CONSOLE_SESSIONS_DIR, ConsoleSessionRecord, LiveConsoleSession,
    console_session_generation, console_sessions_root, live_console_sessions, read_console_session,
    write_console_session,
};
pub use continuity::{
    ContinuityRepository, ContinuitySnapshot, ContinuityStoreError, MAX_CONTINUITY_LINKS,
};
pub use controller_devices::{
    ControllerDeviceRepository, ControllerDeviceSnapshot, ControllerDeviceStoreError,
};
pub use controller_network::{
    ControllerNetworkRepository, ControllerNetworkSnapshot, ControllerNetworkStoreError,
};
pub use fleet::{FleetStoreSnapshot, load_fleet_read_only};
pub use health::{
    HealthCheckId, HealthCheckKind, HealthError, HealthErrorCode, HealthEvidenceCode,
    HealthFinding, HealthFindingState, HealthReport, HealthRepository, IndexRepairKind,
    IndexRepairPlan, IndexRepairReceipt, IndexRepairState, IndexRepairStep, RepairCancellation,
    RepairFaultPoint, SourceHash,
};
pub use journal::{
    AppendOutcome, JournalError, JournalErrorCode, JournalFrame, JournalKind, JournalLimits,
    JournalRead, JournalScan, JournalStore, ScanIssue, TerminalSnapshot, decode_snapshot,
    encode_snapshot, load_snapshot, scan_journal_bytes,
};
pub use lease::{
    HostLease, HostLeaseState, HostMetadata, LeaseError, LeaseErrorCode, ReconciliationResult,
    probe_host_lease, read_host_metadata, read_host_metadata_snapshot, reconcile_host,
};
#[cfg(feature = "os-keyring")]
pub use multiplex_replication_security::OsReplicationSecretBackend;
/// Only exists with a credential store to talk to; a build without one, such as the mobile
/// bindings, has no keychain to migrate names in.
#[cfg(feature = "os-keyring")]
pub use multiplex_replication_security::keychain;
pub use multiplex_replication_security::{
    ReplicationAuthorityDeviceStatus, ReplicationSecretBackend, ReplicationSecretRef,
    ReplicationSecretStoreError,
};
pub use notifications::{NotificationRepository, NotificationSnapshot, NotificationStoreError};
pub use presets::{PresetRepository, PresetSnapshot};
pub use projects::{
    CURRENT_FORMAT_VERSION, ProjectRepository, ProjectSnapshot, RemovedProject, StoreError,
    StoreHealth,
};
pub use recovery::{
    MetadataFileKind, MetadataRecoveryService, RecoveryCancellation, RecoveryError,
    RecoveryErrorCode, RecoveryFaultPoint, RecoveryFilePlan, RecoveryKind, RecoveryPlan,
    RecoveryReceipt, RecoveryResult, RecoveryState, RecoveryStep,
};
pub use replication::{
    MAX_REPLICATION_CONFLICT_ARTIFACTS, ReplicationAuthorityUpdate, ReplicationConflictCandidate,
    ReplicationConflictChoice, ReplicationConflictOperationMix, ReplicationConflictResolution,
    ReplicationConflictReview, ReplicationContentRevision, ReplicationCustodyMetadata,
    ReplicationDeletionPlan, ReplicationDeviceKeyPackage, ReplicationEnrollmentBundle,
    ReplicationEnrollmentRequest, ReplicationProductError, ReplicationProductRecord,
    ReplicationProductService, ReplicationProductStatus, ReplicationRecoveryOutcome,
    ReplicationRepository, ReplicationRepositoryRevision, ReplicationRepositorySnapshot,
    ReplicationRepositorySource, ReplicationResolutionContext, ReplicationRetirementOutcome,
    ReplicationStoreError, ReplicationSyncCoordinator, ReplicationSyncDisposition,
    ReplicationSyncOutcome, ReplicationSyncPlan, ReplicationSyncReviewToken,
    SharedFolderConflictArtifact, SharedFolderReplicationInputs, SharedFolderReplicationTransport,
    SharedFolderSlot, SharedFolderTransportSnapshot, SharedFolderTransportState,
};
pub use sessions::{
    QuarantinedSession, SessionRemovalManifest, SessionRemovalPlan, SessionRepository,
    SessionSnapshot,
};
pub use transcript::{
    TranscriptExportError, TranscriptExportLabels, TranscriptExportResult,
    TranscriptExportSourceSummary, TranscriptExportSpec, TranscriptPageStream, export_transcript,
};
