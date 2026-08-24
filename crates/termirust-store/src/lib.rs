mod atomic;
pub mod journal;
pub mod lease;
pub mod presets;
pub mod projects;

pub use atomic::{AtomicWriter, Durability, SystemAtomicWriter};
pub use journal::{
    AppendOutcome, JournalError, JournalErrorCode, JournalFrame, JournalKind, JournalLimits,
    JournalRead, JournalScan, JournalStore, ScanIssue, TerminalSnapshot, decode_snapshot,
    encode_snapshot, load_snapshot, scan_journal_bytes,
};
pub use lease::{
    HostLease, HostMetadata, LeaseError, LeaseErrorCode, ReconciliationResult, read_host_metadata,
    reconcile_host,
};
pub use presets::{PresetRepository, PresetSnapshot};
pub use projects::{
    CURRENT_FORMAT_VERSION, ProjectRepository, ProjectSnapshot, RemovedProject, StoreError,
    StoreHealth,
};
