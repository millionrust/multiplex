use std::fmt;
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use termirust_domain::{HostInstanceId, HostLifecycle, HostedSessionId};
use termirust_store::{
    AtomicWriter, HostLease, HostLeaseState, HostMetadata, LeaseError, LeaseErrorCode,
    RecoveryKind, RecoveryResult, RecoveryState, RecoveryStep, SystemAtomicWriter,
    probe_host_lease, read_host_metadata_snapshot,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{ConnectOptions, HostClient, LocalEndpoint};

const RECOVERY_DIR: &str = "host-recovery";
const RECOVERY_MARKER: &str = ".termirust-host-recovery-v1";
const ACTIVE_JOURNAL: &str = "active-v1.json";
const BACKUP_FILE: &str = "host.current.json";
const MARKER_BYTES: &[u8] = b"termirust-host-recovery-v1\n";
const MAX_HOST_BYTES: u64 = 16 * 1024;
const MAX_JOURNAL_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostProbeRequest {
    pub session_id: HostedSessionId,
    pub expected_host_instance_id: HostInstanceId,
    pub endpoint: LocalEndpoint,
    pub heartbeat_monotonic_nanos: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthenticatedHostPeer {
    pub session_id: HostedSessionId,
    pub host_instance_id: HostInstanceId,
}

pub trait HostPeerProbe: Send + Sync + 'static {
    fn probe<'a>(
        &'a self,
        request: &'a HostProbeRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<AuthenticatedHostPeer>, HostReconciliationError>>
                + Send
                + 'a,
        >,
    >;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AuthenticatedIpcProbe;

impl HostPeerProbe for AuthenticatedIpcProbe {
    fn probe<'a>(
        &'a self,
        request: &'a HostProbeRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<AuthenticatedHostPeer>, HostReconciliationError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let first = Uuid::new_v4();
            let second = Uuid::new_v4();
            let mut nonce = [0_u8; termirust_host_protocol::HANDSHAKE_NONCE_BYTES];
            nonce[..16].copy_from_slice(first.as_bytes());
            nonce[16..32].copy_from_slice(second.as_bytes());
            let cancel = CancellationToken::new();
            let client = HostClient::connect(
                request.endpoint.clone(),
                ConnectOptions::local_read_only(request.session_id, nonce),
                &cancel,
            )
            .await
            .map_err(|_| error(HostReconciliationErrorCode::PeerUnavailable))?;
            let host_instance_id = client
                .host_instance_id()
                .ok_or_else(|| error(HostReconciliationErrorCode::PeerUnavailable))?;
            Ok(vec![AuthenticatedHostPeer {
                session_id: request.session_id,
                host_instance_id,
            }])
        })
    }
}

pub struct HostReconciliationPlan {
    pub id: Uuid,
    pub kind: RecoveryKind,
    pub session_id: HostedSessionId,
    pub host_instance_id: HostInstanceId,
    pub lifecycle: HostLifecycle,
    pub heartbeat_monotonic_nanos: u64,
    pub lease_state: HostLeaseState,
    pub authenticated_peers: Vec<AuthenticatedHostPeer>,
    pub expected_metadata_sha256: String,
    pub current_backup_path: PathBuf,
    pub current_bytes: u64,
    pub preview_result: RecoveryResult,
    pub steps: Vec<RecoveryStep>,
    pub cancellation_boundary: RecoveryStep,
    pub state: RecoveryState,
    session_dir: PathBuf,
    metadata: HostMetadata,
    metadata_bytes: Vec<u8>,
}

impl fmt::Debug for HostReconciliationPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostReconciliationPlan")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("session_id", &self.session_id)
            .field("host_instance_id", &self.host_instance_id)
            .field("lifecycle", &self.lifecycle)
            .field("heartbeat_monotonic_nanos", &self.heartbeat_monotonic_nanos)
            .field("lease_state", &self.lease_state)
            .field("authenticated_peers", &self.authenticated_peers)
            .field("expected_metadata_sha256", &self.expected_metadata_sha256)
            .field("backup", &self.current_backup_path.file_name())
            .field("current_bytes", &self.current_bytes)
            .field("preview_result", &self.preview_result)
            .field("steps", &self.steps)
            .field("cancellation_boundary", &self.cancellation_boundary)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostReconciliationReceipt {
    pub id: Uuid,
    pub kind: RecoveryKind,
    pub result: RecoveryResult,
    pub state: RecoveryState,
    pub host_instance_id: HostInstanceId,
    pub backup_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostReconciliationErrorCode {
    Cancelled,
    PeerUnavailable,
    StaleEvidence,
    PermissionDenied,
    UnsafeEntry,
    StorageUnavailable,
    VerificationFailed,
    InjectedCrash,
    RecoveryRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostReconciliationError {
    pub code: HostReconciliationErrorCode,
}

impl fmt::Display for HostReconciliationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Host reconciliation failed: {:?}", self.code)
    }
}

impl std::error::Error for HostReconciliationError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[doc(hidden)]
pub enum HostRecoveryFaultPoint {
    AfterBackup,
    AfterJournal,
    AfterApply,
    AfterVerification,
}

#[derive(Clone)]
pub struct HostReconciliationService<P = AuthenticatedIpcProbe> {
    runtime_root: PathBuf,
    probe: Arc<P>,
    writer: Arc<dyn AtomicWriter>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HostRecoveryJournal {
    version: u16,
    plan_id: Uuid,
    host_instance_id: HostInstanceId,
    original_sha256: String,
    applied: bool,
}

impl HostReconciliationService<AuthenticatedIpcProbe> {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self::with_probe(runtime_root, Arc::new(AuthenticatedIpcProbe))
    }
}

impl<P: HostPeerProbe> HostReconciliationService<P> {
    pub fn with_probe(runtime_root: impl Into<PathBuf>, probe: Arc<P>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            probe,
            writer: Arc::new(SystemAtomicWriter),
        }
    }

    #[doc(hidden)]
    pub fn with_probe_and_writer(
        runtime_root: impl Into<PathBuf>,
        probe: Arc<P>,
        writer: Arc<dyn AtomicWriter>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            probe,
            writer,
        }
    }

    pub async fn plan(
        &self,
        session_dir: impl Into<PathBuf>,
    ) -> Result<HostReconciliationPlan, HostReconciliationError> {
        let session_dir = session_dir.into();
        let (metadata_bytes, metadata) =
            read_host_metadata_snapshot(&session_dir).map_err(map_lease)?;
        let lease_state = probe_host_lease(&session_dir).map_err(map_lease)?;
        let endpoint = LocalEndpoint::new(&self.runtime_root, metadata.session_id);
        let expected_endpoint = endpoint
            .socket_path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let authenticated_peers =
            if lease_state == HostLeaseState::Held && metadata.endpoint_name == expected_endpoint {
                self.probe
                    .probe(&HostProbeRequest {
                        session_id: metadata.session_id,
                        expected_host_instance_id: metadata.host_instance_id,
                        endpoint,
                        heartbeat_monotonic_nanos: metadata.heartbeat_monotonic_nanos,
                    })
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        let unique_match = authenticated_peers.as_slice()
            == [AuthenticatedHostPeer {
                session_id: metadata.session_id,
                host_instance_id: metadata.host_instance_id,
            }];
        let preview_result = match lease_state {
            HostLeaseState::Held if unique_match => RecoveryResult::NoChange,
            HostLeaseState::Held => RecoveryResult::Ambiguous,
            HostLeaseState::Available if !authenticated_peers.is_empty() => {
                RecoveryResult::Ambiguous
            }
            HostLeaseState::Available
                if matches!(
                    metadata.lifecycle,
                    HostLifecycle::Exited | HostLifecycle::Orphaned
                ) =>
            {
                RecoveryResult::NoChange
            }
            HostLeaseState::Available => RecoveryResult::Reconciled,
        };
        let id = Uuid::new_v4();
        Ok(HostReconciliationPlan {
            id,
            kind: RecoveryKind::ReconcileHostLeases,
            session_id: metadata.session_id,
            host_instance_id: metadata.host_instance_id,
            lifecycle: metadata.lifecycle,
            heartbeat_monotonic_nanos: metadata.heartbeat_monotonic_nanos,
            lease_state,
            authenticated_peers,
            expected_metadata_sha256: sha256_hex(&metadata_bytes),
            current_backup_path: session_dir
                .join(RECOVERY_DIR)
                .join(id.to_string())
                .join(BACKUP_FILE),
            current_bytes: metadata_bytes.len() as u64,
            preview_result,
            steps: vec![
                RecoveryStep::LockAndRecheck,
                RecoveryStep::BackupCurrent,
                RecoveryStep::VerifyBackup,
                RecoveryStep::PublishLastGood,
                RecoveryStep::ReopenAndVerify,
                RecoveryStep::Rollback,
            ],
            cancellation_boundary: RecoveryStep::PublishLastGood,
            state: RecoveryState::Planned,
            session_dir,
            metadata,
            metadata_bytes,
        })
    }

    pub fn reconcile(
        &self,
        plan: HostReconciliationPlan,
        cancellation: &CancellationToken,
    ) -> Result<HostReconciliationReceipt, HostReconciliationError> {
        self.reconcile_with_fault(plan, cancellation, None)
    }

    #[doc(hidden)]
    pub fn reconcile_with_fault(
        &self,
        plan: HostReconciliationPlan,
        cancellation: &CancellationToken,
        fault: Option<HostRecoveryFaultPoint>,
    ) -> Result<HostReconciliationReceipt, HostReconciliationError> {
        if cancellation.is_cancelled() {
            return Err(error(HostReconciliationErrorCode::Cancelled));
        }
        if plan.preview_result != RecoveryResult::Reconciled {
            return Ok(receipt(&plan, plan.preview_result, 0));
        }
        let lease =
            HostLease::acquire(&plan.session_dir, plan.host_instance_id).map_err(|value| {
                if value.code == LeaseErrorCode::Busy {
                    error(HostReconciliationErrorCode::StaleEvidence)
                } else {
                    map_lease(value)
                }
            })?;
        let (current_bytes, current) =
            read_host_metadata_snapshot(&plan.session_dir).map_err(map_lease)?;
        if sha256_hex(&current_bytes) != plan.expected_metadata_sha256
            || current != plan.metadata
            || current_bytes != plan.metadata_bytes
        {
            return Err(error(HostReconciliationErrorCode::StaleEvidence));
        }
        if cancellation.is_cancelled() {
            return Err(error(HostReconciliationErrorCode::Cancelled));
        }
        self.ensure_recovery_root(&plan.session_dir)?;
        let plan_root = plan
            .current_backup_path
            .parent()
            .ok_or_else(|| error(HostReconciliationErrorCode::UnsafeEntry))?;
        create_private_directory(plan_root)?;
        self.writer
            .write(&plan.current_backup_path, &current_bytes)
            .map_err(map_io)?;
        if read_bounded(&plan.current_backup_path, MAX_HOST_BYTES)? != current_bytes {
            return Err(error(HostReconciliationErrorCode::VerificationFailed));
        }
        inject(fault, HostRecoveryFaultPoint::AfterBackup)?;
        if cancellation.is_cancelled() {
            return Err(error(HostReconciliationErrorCode::Cancelled));
        }
        let mut journal = HostRecoveryJournal {
            version: 1,
            plan_id: plan.id,
            host_instance_id: plan.host_instance_id,
            original_sha256: plan.expected_metadata_sha256.clone(),
            applied: false,
        };
        self.write_journal(&plan.session_dir, &journal)?;
        inject(fault, HostRecoveryFaultPoint::AfterJournal)?;
        let mut updated = current;
        updated.lifecycle = HostLifecycle::Orphaned;
        if let Err(value) = lease.write_metadata(&updated) {
            return self.rollback_or_required(
                &plan.session_dir,
                &lease,
                &journal,
                map_lease(value),
            );
        }
        journal.applied = true;
        if let Err(value) = self.write_journal(&plan.session_dir, &journal) {
            return self.rollback_or_required(&plan.session_dir, &lease, &journal, value);
        }
        inject(fault, HostRecoveryFaultPoint::AfterApply)?;
        let (_, verified) = match read_host_metadata_snapshot(&plan.session_dir) {
            Ok(value) => value,
            Err(value) => {
                return self.rollback_or_required(
                    &plan.session_dir,
                    &lease,
                    &journal,
                    map_lease(value),
                );
            }
        };
        if verified.host_instance_id != plan.host_instance_id
            || verified.session_id != plan.session_id
            || verified.lifecycle != HostLifecycle::Orphaned
        {
            return self.rollback_or_required(
                &plan.session_dir,
                &lease,
                &journal,
                error(HostReconciliationErrorCode::VerificationFailed),
            );
        }
        inject(fault, HostRecoveryFaultPoint::AfterVerification)?;
        self.remove_journal(&plan.session_dir)?;
        Ok(receipt(
            &plan,
            RecoveryResult::Reconciled,
            current_bytes.len() as u64,
        ))
    }

    pub fn recover_interrupted_reconciliation(
        &self,
        session_dir: &Path,
    ) -> Result<Option<HostReconciliationReceipt>, HostReconciliationError> {
        let Some(journal) = self.read_journal(session_dir)? else {
            return Ok(None);
        };
        let lease = HostLease::acquire(session_dir, journal.host_instance_id).map_err(map_lease)?;
        self.rollback(session_dir, &lease, &journal)?;
        self.remove_journal(session_dir)?;
        let (bytes, metadata) = read_host_metadata_snapshot(session_dir).map_err(map_lease)?;
        Ok(Some(HostReconciliationReceipt {
            id: journal.plan_id,
            kind: RecoveryKind::ReconcileHostLeases,
            result: RecoveryResult::RolledBack,
            state: RecoveryState::RolledBack,
            host_instance_id: metadata.host_instance_id,
            backup_bytes: bytes.len() as u64,
        }))
    }

    fn rollback_or_required<T>(
        &self,
        session_dir: &Path,
        lease: &HostLease,
        journal: &HostRecoveryJournal,
        original: HostReconciliationError,
    ) -> Result<T, HostReconciliationError> {
        match self.rollback(session_dir, lease, journal) {
            Ok(()) => {
                let _ = self.remove_journal(session_dir);
                Err(original)
            }
            Err(_) => Err(error(HostReconciliationErrorCode::RecoveryRequired)),
        }
    }

    fn rollback(
        &self,
        session_dir: &Path,
        lease: &HostLease,
        journal: &HostRecoveryJournal,
    ) -> Result<(), HostReconciliationError> {
        validate_journal(journal)?;
        let backup = session_dir
            .join(RECOVERY_DIR)
            .join(journal.plan_id.to_string())
            .join(BACKUP_FILE);
        let bytes = read_bounded(&backup, MAX_HOST_BYTES)?;
        if sha256_hex(&bytes) != journal.original_sha256 {
            return Err(error(HostReconciliationErrorCode::VerificationFailed));
        }
        let original: HostMetadata = serde_json::from_slice(&bytes)
            .map_err(|_| error(HostReconciliationErrorCode::VerificationFailed))?;
        if original.host_instance_id != journal.host_instance_id {
            return Err(error(HostReconciliationErrorCode::VerificationFailed));
        }
        lease.write_metadata(&original).map_err(map_lease)?;
        let (restored, _) = read_host_metadata_snapshot(session_dir).map_err(map_lease)?;
        if restored != bytes {
            return Err(error(HostReconciliationErrorCode::VerificationFailed));
        }
        Ok(())
    }

    fn ensure_recovery_root(&self, session_dir: &Path) -> Result<(), HostReconciliationError> {
        let root = session_dir.join(RECOVERY_DIR);
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(error(HostReconciliationErrorCode::UnsafeEntry));
            }
            Ok(_) => {}
            Err(value) if value.kind() == io::ErrorKind::NotFound => {
                create_private_directory(&root)?;
            }
            Err(value) => return Err(map_io(value)),
        }
        let marker = root.join(RECOVERY_MARKER);
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(error(HostReconciliationErrorCode::UnsafeEntry));
            }
            Ok(_) if fs::read(&marker).map_err(map_io)? != MARKER_BYTES => {
                return Err(error(HostReconciliationErrorCode::UnsafeEntry));
            }
            Ok(_) => {}
            Err(value) if value.kind() == io::ErrorKind::NotFound => {
                create_private_file(&marker, MARKER_BYTES)?;
            }
            Err(value) => return Err(map_io(value)),
        }
        Ok(())
    }

    fn write_journal(
        &self,
        session_dir: &Path,
        journal: &HostRecoveryJournal,
    ) -> Result<(), HostReconciliationError> {
        let bytes = serde_json::to_vec(journal)
            .map_err(|_| error(HostReconciliationErrorCode::VerificationFailed))?;
        self.writer
            .write(&session_dir.join(RECOVERY_DIR).join(ACTIVE_JOURNAL), &bytes)
            .map_err(map_io)?;
        Ok(())
    }

    fn read_journal(
        &self,
        session_dir: &Path,
    ) -> Result<Option<HostRecoveryJournal>, HostReconciliationError> {
        let root = session_dir.join(RECOVERY_DIR);
        match fs::symlink_metadata(&root) {
            Err(value) if value.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(value) => return Err(map_io(value)),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(error(HostReconciliationErrorCode::UnsafeEntry));
            }
            Ok(_) => {}
        }
        if read_bounded(&root.join(RECOVERY_MARKER), 1024)? != MARKER_BYTES {
            return Err(error(HostReconciliationErrorCode::UnsafeEntry));
        }
        let journal_path = root.join(ACTIVE_JOURNAL);
        match fs::symlink_metadata(&journal_path) {
            Err(value) if value.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(value) => return Err(map_io(value)),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(error(HostReconciliationErrorCode::UnsafeEntry));
            }
            Ok(_) => {}
        }
        let bytes = read_bounded(&journal_path, MAX_JOURNAL_BYTES)?;
        let journal: HostRecoveryJournal = serde_json::from_slice(&bytes)
            .map_err(|_| error(HostReconciliationErrorCode::VerificationFailed))?;
        validate_journal(&journal)?;
        Ok(Some(journal))
    }

    fn remove_journal(&self, session_dir: &Path) -> Result<(), HostReconciliationError> {
        match fs::remove_file(session_dir.join(RECOVERY_DIR).join(ACTIVE_JOURNAL)) {
            Ok(()) => Ok(()),
            Err(value) if value.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(value) => Err(map_io(value)),
        }
    }
}

fn receipt(
    plan: &HostReconciliationPlan,
    result: RecoveryResult,
    backup_bytes: u64,
) -> HostReconciliationReceipt {
    HostReconciliationReceipt {
        id: plan.id,
        kind: RecoveryKind::ReconcileHostLeases,
        result,
        state: RecoveryState::Complete,
        host_instance_id: plan.host_instance_id,
        backup_bytes,
    }
}

fn validate_journal(journal: &HostRecoveryJournal) -> Result<(), HostReconciliationError> {
    if journal.version != 1 || journal.original_sha256.len() != 64 {
        Err(error(HostReconciliationErrorCode::UnsafeEntry))
    } else {
        Ok(())
    }
}

fn create_private_directory(path: &Path) -> Result<(), HostReconciliationError> {
    fs::create_dir(path).map_err(map_io)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(map_io)?;
    Ok(())
}

fn create_private_file(path: &Path, bytes: &[u8]) -> Result<(), HostReconciliationError> {
    use std::io::Write as _;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(map_io)?;
    file.write_all(bytes).map_err(map_io)?;
    file.sync_all().map_err(map_io)
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, HostReconciliationError> {
    let metadata = fs::symlink_metadata(path).map_err(map_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > maximum {
        return Err(error(HostReconciliationErrorCode::UnsafeEntry));
    }
    fs::read(path).map_err(map_io)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn inject(
    selected: Option<HostRecoveryFaultPoint>,
    current: HostRecoveryFaultPoint,
) -> Result<(), HostReconciliationError> {
    if selected == Some(current) {
        Err(error(HostReconciliationErrorCode::InjectedCrash))
    } else {
        Ok(())
    }
}

fn map_lease(value: LeaseError) -> HostReconciliationError {
    error(match value.code {
        LeaseErrorCode::PermissionDenied => HostReconciliationErrorCode::PermissionDenied,
        LeaseErrorCode::UnsafeEntry => HostReconciliationErrorCode::UnsafeEntry,
        LeaseErrorCode::Busy => HostReconciliationErrorCode::StaleEvidence,
        LeaseErrorCode::InvalidMetadata => HostReconciliationErrorCode::VerificationFailed,
        LeaseErrorCode::Io => HostReconciliationErrorCode::StorageUnavailable,
    })
}

fn map_io(value: io::Error) -> HostReconciliationError {
    error(match value.kind() {
        io::ErrorKind::PermissionDenied => HostReconciliationErrorCode::PermissionDenied,
        io::ErrorKind::InvalidInput => HostReconciliationErrorCode::UnsafeEntry,
        _ => HostReconciliationErrorCode::StorageUnavailable,
    })
}

const fn error(code: HostReconciliationErrorCode) -> HostReconciliationError {
    HostReconciliationError { code }
}
