//! Explicit read-only-provider transfer. Never publishes or replaces a transport file.
use super::*;
use crate::replication::{
    ReplicationRepositorySnapshot, ReplicationRepositorySource, canonical_replication,
};
use termirust_domain::merge_replication_documents;
use zeroize::Zeroizing;

pub struct ReplicationTransferReview {
    token: [u8; 32],
    key: ReplicationRecordKey,
    value: Zeroizing<Vec<u8>>,
    local: ReplicationRepositorySnapshot,
    merged: ReplicationDocument,
}

impl ReplicationTransferReview {
    pub fn token(&self) -> &[u8; 32] {
        &self.token
    }
    pub fn key(&self) -> &ReplicationRecordKey {
        &self.key
    }
    pub fn value(&self) -> &[u8] {
        &self.value
    }
    pub fn changes_local(&self) -> bool {
        self.local.document != self.merged
    }
}

impl<B: ReplicationSecretBackend> ReplicationProductService<B> {
    /// Authenticated discovery only, not permission to apply. Ignores other collections.
    pub fn inspect_transfer_collection(
        &self,
        bytes: &[u8],
        collection: &termirust_domain::ReplicationCollectionId,
    ) -> Result<Vec<ReplicationProductRecord>, ReplicationProductError> {
        let _lock = ProductAdvisoryLock::acquire(&self.root.join(PRODUCT_LOCK_FILE))?;
        let (local, _) = self.transfer_snapshot()?;
        let incoming = self.transfer_document(bytes)?;
        self.inspect_collection(&local, &incoming, collection)
    }

    /// Read the authoritative local collection without accepting recovery fallbacks.
    pub fn records_in_collection(
        &self,
        collection: &termirust_domain::ReplicationCollectionId,
    ) -> Result<Vec<ReplicationProductRecord>, ReplicationProductError> {
        let _lock = ProductAdvisoryLock::acquire(&self.root.join(PRODUCT_LOCK_FILE))?;
        let (local, _) = self.transfer_snapshot()?;
        self.inspect_collection(&local, &local.document, collection)
    }

    fn inspect_collection(
        &self,
        local: &ReplicationRepositorySnapshot,
        document: &ReplicationDocument,
        collection: &termirust_domain::ReplicationCollectionId,
    ) -> Result<Vec<ReplicationProductRecord>, ReplicationProductError> {
        document
            .entries
            .iter()
            .filter(|entry| &entry.key.collection == collection)
            .map(|entry| {
                if entry.candidates.len() != 1 {
                    return Err(ReplicationProductError::RecordConflict);
                }
                Ok(ReplicationProductRecord {
                    key: entry.key.clone(),
                    value: self.open_candidate(local, &entry.key, &entry.candidates[0])?,
                })
            })
            .collect()
    }

    /// Returns authenticated plaintext for one selected put, not executable settings.
    /// Other records in the provider document are never imported by this operation.
    pub fn review_record_transfer(
        &self,
        bytes: &[u8],
        key: &ReplicationRecordKey,
    ) -> Result<ReplicationTransferReview, ReplicationProductError> {
        let _lock = ProductAdvisoryLock::acquire(&self.root.join(PRODUCT_LOCK_FILE))?;
        self.record_transfer_review(bytes, key)
    }

    /// Reauthenticates supplied bytes and rejects stale review before local-only CAS.
    pub fn apply_record_transfer(
        &self,
        bytes: &[u8],
        key: &ReplicationRecordKey,
        expected_token: &[u8; 32],
    ) -> Result<ReplicationRepositoryRevision, ReplicationProductError> {
        let _lock = ProductAdvisoryLock::acquire(&self.root.join(PRODUCT_LOCK_FILE))?;
        let fresh = self.record_transfer_review(bytes, key)?;
        if &fresh.token != expected_token {
            return Err(ReplicationStoreError::StaleSyncPlan.into());
        }
        if !fresh.changes_local() {
            return Ok(fresh.local.revision);
        }
        let policy = self.authority.replication_policy()?;
        Ok(self
            .repository
            .commit(
                fresh.local.revision,
                fresh.merged,
                fresh.local.custody,
                &[],
                &policy,
            )?
            .revision)
    }

    fn transfer_snapshot(
        &self,
    ) -> Result<(ReplicationRepositorySnapshot, Vec<u8>), ReplicationProductError> {
        for marker in [
            PRODUCT_TRANSACTION_FILE,
            DELETION_TRANSACTION_FILE,
            ENROLLMENT_TRANSACTION_FILE,
        ] {
            if reject_unsafe_file_if_present(&self.root.join(marker))? {
                return Err(ReplicationStoreError::RecoveryRequired.into());
            }
        }
        let profile = read_profile(&self.root.join(PRODUCT_PROFILE_FILE))?;
        let authority = self.authority.to_canonical_bytes()?;
        if profile.workspace_id != self.workspace_id.as_str()
            || profile.local_replica_id != self.local_replica_id.as_str()
            || decode_hex(&profile.authority_state_hex)? != authority
        {
            return Err(ReplicationStoreError::StaleSyncPlan.into());
        }
        let policy = self.authority.replication_policy()?;
        let local = self.repository.load(&self.workspace_id, &policy)?;
        if self
            .authority
            .device(&self.local_replica_id)
            .is_none_or(|device| device.status() != ReplicationAuthorityDeviceStatus::Active)
        {
            return Err(ReplicationProductError::EnrollmentMismatch);
        }
        if local.source != ReplicationRepositorySource::Primary {
            return Err(ReplicationStoreError::RecoveryRequired.into());
        }
        if local.retirement_pending {
            return Err(ReplicationStoreError::PendingRetirement.into());
        }
        Ok((local, authority))
    }

    fn transfer_document(
        &self,
        bytes: &[u8],
    ) -> Result<ReplicationDocument, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        let incoming = ReplicationDocument::decode_json(bytes, &policy)
            .map_err(ReplicationStoreError::from)?;
        if incoming.workspace_id != self.workspace_id {
            return Err(ReplicationStoreError::WorkspaceMismatch.into());
        }
        let (_, canonical) = canonical_replication(&incoming, &policy)?;
        if canonical != bytes {
            return Err(ReplicationStoreError::Corrupt.into());
        }
        Ok(incoming)
    }

    fn record_transfer_review(
        &self,
        bytes: &[u8],
        key: &ReplicationRecordKey,
    ) -> Result<ReplicationTransferReview, ReplicationProductError> {
        let (local, authority) = self.transfer_snapshot()?;
        let incoming = self.transfer_document(bytes)?;
        let policy = self.authority.replication_policy()?;
        let entry = incoming
            .entries
            .iter()
            .find(|entry| &entry.key == key)
            .ok_or(ReplicationStoreError::Missing)?;
        if entry.candidates.len() != 1 {
            return Err(ReplicationProductError::RecordConflict);
        }
        let value = Zeroizing::new(
            self.open_candidate(&local, key, &entry.candidates[0])?
                .ok_or(ReplicationStoreError::InvalidConflictResolution)?,
        );
        let selected =
            ReplicationDocument::new(self.workspace_id.clone(), vec![entry.clone()], &policy)
                .map_err(ReplicationStoreError::from)?;
        let merged = merge_replication_documents(&local.document, &selected, &policy)
            .map_err(ReplicationStoreError::from)?
            .document;
        let result = merged
            .entries
            .iter()
            .find(|entry| &entry.key == key)
            .ok_or(ReplicationStoreError::Missing)?;
        if result.candidates.len() != 1 {
            return Err(ReplicationProductError::RecordConflict);
        }
        // A dominated incoming value must not be presented as the value being imported.
        if result != entry {
            return Err(ReplicationStoreError::StaleSyncPlan.into());
        }
        let mut digest = Sha256::new();
        digest.update(b"termirust-reviewed-record-transfer-v1\0");
        digest.update(Sha256::digest(bytes));
        digest.update(Sha256::digest(&authority));
        digest.update(Sha256::digest(self.local_replica_id.as_str().as_bytes()));
        digest.update(Sha256::digest(
            canonical_replication(&local.document, &policy)?.1,
        ));
        digest.update(local.revision.get().to_be_bytes());
        digest.update(serde_json::to_vec(key).map_err(|_| ReplicationStoreError::Corrupt)?);
        Ok(ReplicationTransferReview {
            token: digest.finalize().into(),
            key: key.clone(),
            value,
            local,
            merged,
        })
    }
}
