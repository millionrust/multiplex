pub mod artifacts;
mod atomic;
pub mod controller_devices;
pub mod controller_network;
pub mod journal;
pub mod lease;
pub mod notifications;
pub mod presets;
pub mod projects;
pub mod sessions;
pub mod transcript;

pub use artifacts::{
    ArtifactIngestProgress, ArtifactIngestRequest, ArtifactPayload, ArtifactRepository,
    ArtifactSnapshot, ArtifactStoreError, ArtifactSweepResult,
};
pub use atomic::{AtomicWriter, Durability, SystemAtomicWriter};
pub use controller_devices::{
    ControllerDeviceRepository, ControllerDeviceSnapshot, ControllerDeviceStoreError,
};
pub use controller_network::{
    ControllerNetworkRepository, ControllerNetworkSnapshot, ControllerNetworkStoreError,
};
pub use journal::{
    AppendOutcome, JournalError, JournalErrorCode, JournalFrame, JournalKind, JournalLimits,
    JournalRead, JournalScan, JournalStore, ScanIssue, TerminalSnapshot, decode_snapshot,
    encode_snapshot, load_snapshot, scan_journal_bytes,
};
pub use lease::{
    HostLease, HostMetadata, LeaseError, LeaseErrorCode, ReconciliationResult, read_host_metadata,
    reconcile_host,
};
pub use notifications::{NotificationRepository, NotificationSnapshot, NotificationStoreError};
pub use presets::{PresetRepository, PresetSnapshot};
pub use projects::{
    CURRENT_FORMAT_VERSION, ProjectRepository, ProjectSnapshot, RemovedProject, StoreError,
    StoreHealth,
};
pub use sessions::{
    QuarantinedSession, SessionRemovalManifest, SessionRemovalPlan, SessionRepository,
    SessionSnapshot,
};
pub use transcript::{
    TranscriptExportError, TranscriptExportLabels, TranscriptExportResult,
    TranscriptExportSourceSummary, TranscriptExportSpec, TranscriptPageStream, export_transcript,
};
