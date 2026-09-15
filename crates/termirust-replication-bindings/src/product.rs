//! Local-filesystem enrollment facade. Provider URI transport belongs to C06.
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{
    NativeReplicationSecretBackend, ReplicationSecureStore, ReplicationStorageError, binding_error,
};
use termirust_store::replication::{
    ReplicationProductError, ReplicationProductService, ReplicationStoreError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Error)]
pub enum MobileReplicationError {
    Busy,
    Invalid,
    AlreadyConfigured,
    RecoveryRequired,
    StaleRequest,
    Locked,
    MissingSecret,
    Collision,
    Unavailable,
}

impl std::fmt::Display for MobileReplicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "replication operation is busy",
            Self::Invalid => "replication data or path is invalid",
            Self::AlreadyConfigured => "replication is already configured",
            Self::RecoveryRequired => "replication recovery is required",
            Self::StaleRequest => "replication enrollment request changed",
            Self::Locked => "replication storage access is denied or locked",
            Self::MissingSecret => "replication secret is missing",
            Self::Collision => "replication secret already exists",
            Self::Unavailable => "replication operation is unavailable",
        })
    }
}
impl std::error::Error for MobileReplicationError {}

#[derive(uniffi::Record)]
pub struct MobileEnrollmentRequest {
    /// Inert canonical public request bytes, never private key or custody reference.
    pub canonical_request: Vec<u8>,
}

#[derive(uniffi::Record)]
pub struct MobileEnrollmentReview {
    /// Display-only claims until the service authenticates acceptance.
    pub workspace_id: String,
    pub recipient: String,
    pub verification_code: String,
}

#[derive(uniffi::Record)]
pub struct MobileRecordTransferReview {
    /// Authenticated but inert data. Native callers must validate the product schema.
    pub plaintext: Vec<u8>,
    pub review_token: Vec<u8>,
    pub changes_local: bool,
}

#[derive(uniffi::Record)]
pub struct MobileHostTransferReview {
    pub stable_id: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub review_token: Vec<u8>,
    pub changes_local: bool,
}

/// Display-only authenticated host metadata, never an executable connection profile.
#[derive(uniffi::Record)]
pub struct MobileReplicatedHost {
    pub record_id: String,
    pub stable_id: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub username: String,
}

#[derive(uniffi::Object)]
pub struct MobileReplicationProduct {
    container: PathBuf,
    store: Arc<dyn ReplicationSecureStore>,
}

type Service = ReplicationProductService<NativeReplicationSecretBackend>;

#[uniffi::export]
impl MobileReplicationProduct {
    /// An existing, app-private, no-backup directory dedicated to replication.
    /// Run all methods on a worker thread; these are synchronous filesystem calls.
    #[uniffi::constructor]
    pub fn new(
        private_directory: String,
        store: Arc<dyn ReplicationSecureStore>,
    ) -> Result<Arc<Self>, MobileReplicationError> {
        Ok(Arc::new(Self {
            container: directory(&private_directory)?,
            store,
        }))
    }

    /// Requires a real existing local exchange directory, not a provider URI.
    /// Preparation does not enroll, accept remote data, or synchronize anything.
    pub fn prepare_enrollment(
        &self,
        local_exchange_directory: String,
    ) -> Result<MobileEnrollmentRequest, MobileReplicationError> {
        let exchange = directory(&local_exchange_directory)?;
        let _lock = self.lock()?;
        let request = Service::prepare_enrollment(self.root(), exchange, self.backend())
            .map_err(map_error)?;
        Ok(MobileEnrollmentRequest {
            canonical_request: request.to_canonical_bytes().map_err(map_error)?,
        })
    }

    /// None means no enrollment root, not a corrupt profile or unavailable secret.
    pub fn pending_enrollment(
        &self,
    ) -> Result<Option<MobileEnrollmentRequest>, MobileReplicationError> {
        let _lock = self.lock()?;
        let root = self.root();
        match fs::symlink_metadata(&root) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error(e)),
            Ok(m) if !m.is_dir() || m.file_type().is_symlink() => {
                return Err(MobileReplicationError::Invalid);
            }
            Ok(_) => {}
        }
        for transaction in ["deletion.transaction.json", "enrollment.transaction.json"] {
            if present(&root.join(transaction))? {
                return Err(MobileReplicationError::RecoveryRequired);
            }
        }
        if present(&root.join("profile.json"))? {
            return Err(MobileReplicationError::AlreadyConfigured);
        }
        let request = Service::pending_enrollment_request(root).map_err(map_error)?;
        Ok(Some(MobileEnrollmentRequest {
            canonical_request: request.to_canonical_bytes().map_err(map_error)?,
        }))
    }

    /// Explicit journal recovery only. Call pending_enrollment again to inspect
    /// the result; never automatically retry acceptance after this operation.
    pub fn recover_pending_enrollment(&self) -> Result<(), MobileReplicationError> {
        let _lock = self.lock()?;
        Service::recover_pending_enrollment(self.root(), self.backend()).map_err(map_error)
    }

    pub fn cancel_pending_enrollment(
        &self,
        expected_request: Vec<u8>,
    ) -> Result<bool, MobileReplicationError> {
        let _lock = self.lock()?;
        let expected =
            termirust_store::replication::ReplicationEnrollmentRequest::from_canonical_bytes(
                &expected_request,
            )
            .map_err(map_error)?;
        if !present(&self.root())? {
            return Ok(false);
        }
        if present(&self.root().join("profile.json"))? {
            return Err(MobileReplicationError::AlreadyConfigured);
        }
        let actual = Service::pending_enrollment_request(self.root()).map_err(map_error)?;
        if actual != expected {
            return Err(MobileReplicationError::StaleRequest);
        }
        Service::cancel_pending_enrollment(self.root(), self.backend()).map_err(map_error)
    }

    /// Canonical decoding and recipient matching only; this does not establish trust.
    /// Keep the exact input bytes for acceptance and compare the code with the desktop.
    pub fn review_enrollment(
        &self,
        expected_request: Vec<u8>,
        bundle_bytes: Vec<u8>,
    ) -> Result<MobileEnrollmentReview, MobileReplicationError> {
        let _lock = self.lock()?;
        self.enrollment_review(&expected_request, &bundle_bytes)
    }

    /// Receives reviewed bytes, never a document-provider path. The service performs
    /// authenticated key unwrap and recoverable activation before returning success.
    pub fn accept_enrollment(
        &self,
        expected_request: Vec<u8>,
        bundle_bytes: Vec<u8>,
        expected_workspace: String,
        verified_code: String,
    ) -> Result<(), MobileReplicationError> {
        let _lock = self.lock()?;
        let review = self.enrollment_review(&expected_request, &bundle_bytes)?;
        if review.workspace_id != expected_workspace || review.verification_code != verified_code {
            return Err(MobileReplicationError::Invalid);
        }
        let bundle =
            termirust_store::replication::ReplicationEnrollmentBundle::from_canonical_bytes(
                &bundle_bytes,
            )
            .map_err(map_error)?;
        Service::accept_enrollment(self.root(), self.backend(), &bundle, &verified_code)
            .map_err(map_error)?;
        Ok(())
    }

    /// Runs on a worker thread. Provider URLs never enter this API.
    pub fn review_record_transfer(
        &self,
        document_bytes: Vec<u8>,
        collection: String,
        record_id: String,
    ) -> Result<MobileRecordTransferReview, MobileReplicationError> {
        let _lock = self.lock()?;
        let key = record_key(collection, record_id)?;
        let service = Service::open(self.root(), self.backend()).map_err(map_error)?;
        let review = service
            .review_record_transfer(&document_bytes, &key)
            .map_err(map_error)?;
        Ok(MobileRecordTransferReview {
            plaintext: review.value().to_vec(),
            review_token: review.token().to_vec(),
            changes_local: review.changes_local(),
        })
    }

    /// Local encrypted replica commit only. Does not connect, run commands or publish.
    pub fn apply_record_transfer(
        &self,
        document_bytes: Vec<u8>,
        collection: String,
        record_id: String,
        expected_token: Vec<u8>,
    ) -> Result<(), MobileReplicationError> {
        let _lock = self.lock()?;
        let token: [u8; 32] = expected_token
            .try_into()
            .map_err(|_| MobileReplicationError::Invalid)?;
        let key = record_key(collection, record_id)?;
        let service = Service::open(self.root(), self.backend()).map_err(map_error)?;
        service
            .apply_record_transfer(&document_bytes, &key, &token)
            .map_err(map_error)?;
        Ok(())
    }

    /// Authenticated discovery, not an import review. Select one ID and call
    /// review_host_transfer before offering to apply it. No provider path is accepted.
    pub fn preview_host_transfers(
        &self,
        document_bytes: Vec<u8>,
    ) -> Result<Vec<MobileReplicatedHost>, MobileReplicationError> {
        let _lock = self.lock()?;
        let service = Service::open(self.root(), self.backend()).map_err(map_error)?;
        let records = service
            .inspect_transfer_collection(&document_bytes, &host_collection()?)
            .map_err(map_error)?;
        project_hosts(records)
    }

    /// Authoritative locally imported host metadata. Does not expose credentials or
    /// install hosts into the connection library. Missing keys remain errors.
    pub fn imported_hosts(&self) -> Result<Vec<MobileReplicatedHost>, MobileReplicationError> {
        let _lock = self.lock()?;
        let service = Service::open(self.root(), self.backend()).map_err(map_error)?;
        let records = service
            .records_in_collection(&host_collection()?)
            .map_err(map_error)?;
        project_hosts(records)
    }

    /// Inert host details only: no credentials, routes, startup or environment fields.
    pub fn review_host_transfer(
        &self,
        document_bytes: Vec<u8>,
        record_id: String,
    ) -> Result<MobileHostTransferReview, MobileReplicationError> {
        let _lock = self.lock()?;
        let key = record_key("desktop-profiles".into(), record_id.clone())?;
        let service = Service::open(self.root(), self.backend()).map_err(map_error)?;
        let review = service
            .review_record_transfer(&document_bytes, &key)
            .map_err(map_error)?;
        let host = termirust_protocol::review_replicated_host(
            review.value(),
            "desktop-profiles",
            &record_id,
        )
        .map_err(|_| MobileReplicationError::Invalid)?;
        Ok(MobileHostTransferReview {
            stable_id: host.stable_id,
            label: host.label,
            host: host.host,
            port: host.port,
            username: host.username,
            review_token: review.token().to_vec(),
            changes_local: review.changes_local(),
        })
    }

    /// Revalidates the host schema before local encrypted-record commit. This does
    /// not enable an SSH connection or install any imported credential reference.
    pub fn apply_host_transfer(
        &self,
        document_bytes: Vec<u8>,
        record_id: String,
        expected_token: Vec<u8>,
    ) -> Result<(), MobileReplicationError> {
        let _lock = self.lock()?;
        let token: [u8; 32] = expected_token
            .try_into()
            .map_err(|_| MobileReplicationError::Invalid)?;
        let key = record_key("desktop-profiles".into(), record_id.clone())?;
        let service = Service::open(self.root(), self.backend()).map_err(map_error)?;
        let review = service
            .review_record_transfer(&document_bytes, &key)
            .map_err(map_error)?;
        termirust_protocol::review_replicated_host(review.value(), "desktop-profiles", &record_id)
            .map_err(|_| MobileReplicationError::Invalid)?;
        if review.token() != &token {
            return Err(MobileReplicationError::StaleRequest);
        }
        service
            .apply_record_transfer(&document_bytes, &key, &token)
            .map_err(map_error)?;
        Ok(())
    }
}

fn host_collection() -> Result<termirust_domain::ReplicationCollectionId, MobileReplicationError> {
    termirust_domain::ReplicationCollectionId::new("desktop-profiles")
        .map_err(|_| MobileReplicationError::Invalid)
}

fn project_hosts(
    records: Vec<termirust_store::replication::ReplicationProductRecord>,
) -> Result<Vec<MobileReplicatedHost>, MobileReplicationError> {
    let mut hosts = Vec::new();
    for record in records {
        // Authenticated tombstones are absent hosts, not an instruction to delete locally.
        let Some(value) = record.value() else {
            continue;
        };
        let id = record.key().record_id.as_str();
        let host = termirust_protocol::review_replicated_host(value, "desktop-profiles", id)
            .map_err(|_| MobileReplicationError::Invalid)?;
        hosts.push(MobileReplicatedHost {
            record_id: id.to_owned(),
            stable_id: host.stable_id,
            label: host.label,
            host: host.host,
            port: host.port,
            username: host.username,
        });
    }
    Ok(hosts)
}

fn record_key(
    collection: String,
    record_id: String,
) -> Result<termirust_domain::ReplicationRecordKey, MobileReplicationError> {
    Ok(termirust_domain::ReplicationRecordKey::new(
        termirust_domain::ReplicationCollectionId::new(collection)
            .map_err(|_| MobileReplicationError::Invalid)?,
        termirust_domain::ReplicationRecordId::new(record_id)
            .map_err(|_| MobileReplicationError::Invalid)?,
    ))
}

impl MobileReplicationProduct {
    fn enrollment_review(
        &self,
        expected_request: &[u8],
        bundle_bytes: &[u8],
    ) -> Result<MobileEnrollmentReview, MobileReplicationError> {
        if present(&self.root().join("profile.json"))? {
            return Err(MobileReplicationError::AlreadyConfigured);
        }
        for transaction in ["deletion.transaction.json", "enrollment.transaction.json"] {
            if present(&self.root().join(transaction))? {
                return Err(MobileReplicationError::RecoveryRequired);
            }
        }
        let request = Service::pending_enrollment_request(self.root()).map_err(map_error)?;
        if request.to_canonical_bytes().map_err(map_error)? != expected_request {
            return Err(MobileReplicationError::StaleRequest);
        }
        let bundle =
            termirust_store::replication::ReplicationEnrollmentBundle::from_canonical_bytes(
                bundle_bytes,
            )
            .map_err(map_error)?;
        Ok(MobileEnrollmentReview {
            workspace_id: bundle.workspace_id().as_str().to_owned(),
            recipient: bundle.recipient().as_str().to_owned(),
            verification_code: bundle.verification_code(&request).map_err(map_error)?,
        })
    }
    fn root(&self) -> PathBuf {
        self.container.join("enrollment")
    }
    fn backend(&self) -> NativeReplicationSecretBackend {
        NativeReplicationSecretBackend::new(self.store.clone())
    }

    fn lock(&self) -> Result<File, MobileReplicationError> {
        if directory(
            self.container
                .to_str()
                .ok_or(MobileReplicationError::Invalid)?,
        )? != self.container
        {
            return Err(MobileReplicationError::Invalid);
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(not(unix))]
        return Err(MobileReplicationError::Unavailable);
        let file = options
            .open(self.container.join("enrollment.lock"))
            .map_err(io_error)?;
        if !file.metadata().map_err(io_error)?.is_file() {
            return Err(MobileReplicationError::Invalid);
        }
        fs2::FileExt::try_lock_exclusive(&file).map_err(|e| {
            if e.kind() == io::ErrorKind::WouldBlock {
                MobileReplicationError::Busy
            } else {
                io_error(e)
            }
        })?;
        // The stable lock lives outside the root removed by exact cancellation.
        Ok(file)
    }
}

fn directory(value: &str) -> Result<PathBuf, MobileReplicationError> {
    let path = Path::new(value);
    if !path.is_absolute() || value.len() > 4096 || value.contains('\0') || value.contains("://") {
        return Err(MobileReplicationError::Invalid);
    }
    for component in path.ancestors() {
        let meta = fs::symlink_metadata(component).map_err(io_error)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(MobileReplicationError::Invalid);
        }
    }
    fs::canonicalize(path).map_err(io_error)
}

fn present(path: &Path) -> Result<bool, MobileReplicationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_error(e)),
    }
}

fn io_error(error: io::Error) -> MobileReplicationError {
    if error.kind() == io::ErrorKind::PermissionDenied {
        MobileReplicationError::Locked
    } else {
        MobileReplicationError::Unavailable
    }
}

fn map_error(error: ReplicationProductError) -> MobileReplicationError {
    match error {
        ReplicationProductError::Custody(error)
        | ReplicationProductError::Store(ReplicationStoreError::Custody(error)) => {
            match binding_error(error) {
                ReplicationStorageError::Missing => MobileReplicationError::MissingSecret,
                ReplicationStorageError::Locked => MobileReplicationError::Locked,
                ReplicationStorageError::Invalid => MobileReplicationError::Invalid,
                ReplicationStorageError::Collision => MobileReplicationError::Collision,
                ReplicationStorageError::Unavailable => MobileReplicationError::Unavailable,
            }
        }
        ReplicationProductError::AlreadyConfigured => MobileReplicationError::AlreadyConfigured,
        ReplicationProductError::Store(
            ReplicationStoreError::StaleSyncPlan
            | ReplicationStoreError::StaleRepositoryRevision { .. },
        ) => MobileReplicationError::StaleRequest,
        ReplicationProductError::PendingAuthorityTransition
        | ReplicationProductError::Store(ReplicationStoreError::RecoveryRequired) => {
            MobileReplicationError::RecoveryRequired
        }
        ReplicationProductError::Store(ReplicationStoreError::Io {
            kind: io::ErrorKind::PermissionDenied,
            ..
        }) => MobileReplicationError::Locked,
        ReplicationProductError::Store(ReplicationStoreError::Io { .. }) => {
            MobileReplicationError::Unavailable
        }
        ReplicationProductError::InvalidProfile
        | ReplicationProductError::EnrollmentMismatch
        | ReplicationProductError::VerificationCodeMismatch
        | ReplicationProductError::InvalidPath
        | ReplicationProductError::NotConfigured
        | ReplicationProductError::NewerProfile { .. }
        | ReplicationProductError::Store(
            ReplicationStoreError::UnsafeEntry
            | ReplicationStoreError::Corrupt
            | ReplicationStoreError::TooLarge
            | ReplicationStoreError::WorkspaceMismatch
            | ReplicationStoreError::Domain(
                termirust_domain::ReplicationError::MalformedDocument
                | termirust_domain::ReplicationError::DocumentTooLarge
                | termirust_domain::ReplicationError::UnsupportedSchema
                | termirust_domain::ReplicationError::WorkspaceMismatch,
            )
            | ReplicationStoreError::Newer { .. },
        ) => MobileReplicationError::Invalid,
        _ => MobileReplicationError::Unavailable,
    }
}
