pub mod artifacts;
mod atomic;
pub mod continuity;
pub mod controller_devices;
pub mod controller_network;
mod fleet;
pub mod health;
pub mod journal;
pub mod lease;
pub mod notifications;
pub mod presets;
pub mod projects;
pub mod recovery;
pub mod sessions;
pub mod transcript;

pub use artifacts::{
    ArtifactIngestProgress, ArtifactIngestRequest, ArtifactPayload, ArtifactRepository,
    ArtifactSnapshot, ArtifactStoreError, ArtifactSweepResult,
};
pub use atomic::{AtomicWriter, Durability, SystemAtomicWriter};
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
pub use sessions::{
    QuarantinedSession, SessionRemovalManifest, SessionRemovalPlan, SessionRepository,
    SessionSnapshot,
};
pub use transcript::{
    TranscriptExportError, TranscriptExportLabels, TranscriptExportResult,
    TranscriptExportSourceSummary, TranscriptExportSpec, TranscriptPageStream, export_transcript,
};
