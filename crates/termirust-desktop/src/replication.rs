use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use termirust_domain::{
    ReplicationCollectionId, ReplicationRecordId, ReplicationRecordKey, ReplicationReplicaId,
};
use termirust_store::{
    ReplicationAuthorityDeviceStatus, ReplicationAuthorityUpdate, ReplicationConflictChoice,
    ReplicationDeletionPlan, ReplicationEnrollmentBundle, ReplicationEnrollmentRequest,
    ReplicationProductRecord, ReplicationProductService, ReplicationProductStatus,
    ReplicationSecretBackend, ReplicationSyncDisposition, ReplicationSyncOutcome,
    ReplicationSyncPlan,
};

use crate::models::{
    DEFAULT_VAULT_ID, HostProfile, IdentitySource, ProfileSource, SavedIdentity, SavedSnippet,
    SavedState, SavedVault,
};
use crate::storage::KnownHostStore;

const DESKTOP_RECORD_VERSION: u16 = 1;
const RECORD_ID_DOMAIN: &[u8] = b"termirust.desktop-replication-record.v1\0";
const VAULTS: &str = "desktop-vaults";
const PROFILES: &str = "desktop-profiles";
const IDENTITIES: &str = "desktop-identities";
const SNIPPETS: &str = "desktop-snippets";
const KNOWN_HOSTS: &str = "desktop-known-hosts";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DesktopReplicationChanges {
    pub puts: usize,
    pub deletes: usize,
}

pub(crate) struct DesktopReplicationReview {
    plan: ReplicationSyncPlan,
    pub changes: DesktopReplicationChanges,
}

impl DesktopReplicationReview {
    pub fn disposition(&self) -> ReplicationSyncDisposition {
        self.plan.disposition()
    }
}

pub(crate) struct DesktopReplicationConflictCandidate {
    pub device_id: String,
    pub summary: String,
    pub deleted: bool,
}

pub(crate) struct DesktopReplicationConflict {
    key: ReplicationRecordKey,
    pub title: String,
    pub candidates: Vec<DesktopReplicationConflictCandidate>,
}

pub(crate) struct DesktopConflictSelection {
    key: ReplicationRecordKey,
    candidate_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DesktopReplicationDevice {
    pub id: String,
    pub local: bool,
    pub active: bool,
}

pub(crate) struct DesktopReplicationDeletionReview {
    plan: ReplicationDeletionPlan,
    pub record_count: usize,
    pub secret_count: usize,
    pub authority_owner: bool,
}

pub(crate) struct DesktopEnrollmentPackage {
    pub bundle: String,
    pub verification_code: String,
    pub authority_revision: u64,
}

pub(crate) struct DesktopAuthorityPackage {
    pub payload: String,
    pub authority_revision: u64,
}

impl DesktopConflictSelection {
    pub fn new(conflict: &DesktopReplicationConflict, candidate_index: usize) -> Self {
        Self {
            key: conflict.key.clone(),
            candidate_index,
        }
    }
}

pub(crate) struct DesktopReplication<B> {
    product: ReplicationProductService<B>,
}

impl<B: ReplicationSecretBackend> DesktopReplication<B> {
    pub fn prepare_enrollment(
        root: impl Into<PathBuf>,
        shared_folder: impl Into<PathBuf>,
        backend: B,
    ) -> Result<ReplicationEnrollmentRequest> {
        Ok(ReplicationProductService::prepare_enrollment(
            root,
            shared_folder,
            backend,
        )?)
    }

    pub fn pending_enrollment_request(root: impl AsRef<Path>) -> Result<String> {
        canonical_text(
            ReplicationProductService::<B>::pending_enrollment_request(root)?
                .to_canonical_bytes()?,
        )
    }

    pub fn cancel_pending_enrollment(root: impl Into<PathBuf>, backend: B) -> Result<bool> {
        Ok(ReplicationProductService::cancel_pending_enrollment(
            root, backend,
        )?)
    }

    pub fn bootstrap(
        root: impl Into<PathBuf>,
        shared_folder: impl Into<PathBuf>,
        backend: B,
    ) -> Result<Self> {
        Ok(Self {
            product: ReplicationProductService::bootstrap(root, shared_folder, backend)?,
        })
    }

    pub fn open(root: impl Into<PathBuf>, backend: B) -> Result<Self> {
        Ok(Self {
            product: ReplicationProductService::open(root, backend)?,
        })
    }

    pub fn accept_enrollment(
        root: impl Into<PathBuf>,
        backend: B,
        bundle: &ReplicationEnrollmentBundle,
        verification_code: &str,
    ) -> Result<Self> {
        Ok(Self {
            product: ReplicationProductService::accept_enrollment(
                root,
                backend,
                bundle,
                verification_code,
            )?,
        })
    }

    pub fn enroll(
        &mut self,
        request: &ReplicationEnrollmentRequest,
    ) -> Result<ReplicationEnrollmentBundle> {
        Ok(self.product.enroll_request(request)?)
    }

    pub fn enroll_text(&mut self, request: &str) -> Result<DesktopEnrollmentPackage> {
        let request = ReplicationEnrollmentRequest::from_canonical_bytes(request.as_bytes())?;
        let bundle = if let Some(bundle) = self.product.pending_enrollment_bundle(&request)? {
            bundle
        } else {
            self.enroll(&request)?
        };
        let verification_code = bundle.verification_code(&request)?;
        let authority_revision = self.product.status()?.authority_revision;
        Ok(DesktopEnrollmentPackage {
            bundle: canonical_text(bundle.to_canonical_bytes()?)?,
            verification_code,
            authority_revision,
        })
    }

    pub fn accept_enrollment_text(
        root: impl Into<PathBuf>,
        backend: B,
        bundle: &str,
        verification_code: &str,
    ) -> Result<Self> {
        let bundle = ReplicationEnrollmentBundle::from_canonical_bytes(bundle.as_bytes())?;
        Self::accept_enrollment(root, backend, &bundle, verification_code)
    }

    pub fn status(&self) -> Result<ReplicationProductStatus> {
        Ok(self.product.status()?)
    }

    pub fn devices(&self) -> Vec<DesktopReplicationDevice> {
        self.product
            .authority()
            .devices()
            .map(|device| DesktopReplicationDevice {
                id: device.replica_id().as_str().to_string(),
                local: device.replica_id() == self.product.local_replica_id(),
                active: device.status() == ReplicationAuthorityDeviceStatus::Active,
            })
            .collect()
    }

    pub fn pending_authority_package(&self) -> Result<Option<DesktopAuthorityPackage>> {
        self.product
            .pending_authority_update()?
            .map(authority_package)
            .transpose()
    }

    pub fn acknowledge_authority_package(&self, authority_revision: u64) -> Result<bool> {
        Ok(self
            .product
            .acknowledge_authority_update(authority_revision)?)
    }

    pub fn rotate_keys(&mut self) -> Result<DesktopAuthorityPackage> {
        authority_package(self.product.rotate_keys()?)
    }

    pub fn revoke_device(&mut self, device_id: &str) -> Result<DesktopAuthorityPackage> {
        let replica_id = ReplicationReplicaId::new(device_id.to_string())?;
        authority_package(self.product.revoke_device(&replica_id)?)
    }

    pub fn apply_authority_package(&mut self, payload: &str) -> Result<()> {
        let update = ReplicationAuthorityUpdate::from_canonical_bytes(payload.as_bytes())?;
        self.product.apply_authority_update(&update)?;
        Ok(())
    }

    pub fn deletion_review(&self) -> Result<DesktopReplicationDeletionReview> {
        let plan = self.product.deletion_plan()?;
        Ok(DesktopReplicationDeletionReview {
            record_count: plan.record_count,
            secret_count: plan.secret_count,
            authority_owner: plan.authority_owner,
            plan,
        })
    }

    pub fn delete_local_replica(
        self,
        review: &DesktopReplicationDeletionReview,
        confirmation: &str,
    ) -> Result<()> {
        self.product
            .delete_local_replica(&review.plan, confirmation)?;
        Ok(())
    }

    pub fn deletion_confirmation_phrase() -> &'static str {
        ReplicationDeletionPlan::confirmation_phrase()
    }

    pub fn review(
        &self,
        state: &SavedState,
        known_hosts: &KnownHostStore,
    ) -> Result<DesktopReplicationReview> {
        let status = self.product.status()?;
        let changes = if status.recovery_required {
            DesktopReplicationChanges::default()
        } else {
            reconcile_local_records(&self.product, state, known_hosts)?
        };
        Ok(DesktopReplicationReview {
            plan: self.product.review_sync()?,
            changes,
        })
    }

    pub fn conflicts(
        &self,
        review: &DesktopReplicationReview,
    ) -> Result<Vec<DesktopReplicationConflict>> {
        review
            .plan
            .conflicts()
            .iter()
            .map(|conflict| {
                let candidates = self
                    .product
                    .conflict_candidates(&review.plan, conflict.key())?;
                let decoded = candidates
                    .iter()
                    .map(|candidate| match candidate.value() {
                        Some(value) => {
                            let record = decode_record_value(conflict.key(), value)?;
                            Ok(DesktopReplicationConflictCandidate {
                                device_id: candidate.author().as_str().to_string(),
                                summary: record_summary(&record),
                                deleted: false,
                            })
                        }
                        None => Ok(DesktopReplicationConflictCandidate {
                            device_id: candidate.author().as_str().to_string(),
                            summary: "Deleted on this device".to_string(),
                            deleted: true,
                        }),
                    })
                    .collect::<Result<Vec<_>>>()?;
                let title = decoded
                    .iter()
                    .find(|candidate| !candidate.deleted)
                    .map(|candidate| candidate.summary.clone())
                    .unwrap_or_else(|| collection_label(conflict.key()).to_string());
                Ok(DesktopReplicationConflict {
                    key: conflict.key().clone(),
                    title,
                    candidates: decoded,
                })
            })
            .collect()
    }

    pub fn apply(
        &self,
        review: DesktopReplicationReview,
        state: &mut SavedState,
        known_hosts: &KnownHostStore,
    ) -> Result<ReplicationSyncOutcome> {
        if matches!(
            review.plan.disposition(),
            ReplicationSyncDisposition::ConflictReviewRequired
                | ReplicationSyncDisposition::RecoveryRequired
        ) {
            bail!("replication review requires an explicit user decision");
        }
        let outcome = self.product.apply_sync(&review.plan)?;
        apply_product_records(&self.product, state, known_hosts)?;
        Ok(outcome)
    }

    pub fn resolve(
        &self,
        review: DesktopReplicationReview,
        selections: &[DesktopConflictSelection],
        state: &mut SavedState,
        known_hosts: &KnownHostStore,
    ) -> Result<ReplicationSyncOutcome> {
        if review.plan.disposition() != ReplicationSyncDisposition::ConflictReviewRequired {
            bail!("replication review has no conflicts");
        }
        if selections.len() != review.plan.conflicts().len() {
            bail!("every replication conflict requires a selection");
        }
        let mut choices = Vec::with_capacity(selections.len());
        for conflict in review.plan.conflicts() {
            let selection = selections
                .iter()
                .find(|selection| selection.key == *conflict.key())
                .context("replication conflict selection is missing")?;
            let candidates = self
                .product
                .conflict_candidates(&review.plan, conflict.key())?;
            let candidate = candidates
                .get(selection.candidate_index)
                .context("replication conflict selection is invalid")?;
            choices.push(match candidate.value() {
                Some(value) => {
                    ReplicationConflictChoice::put(conflict.key().clone(), value.to_vec())?
                }
                None => ReplicationConflictChoice::delete(conflict.key().clone()),
            });
        }
        let outcome = self.product.resolve_sync(&review.plan, choices)?;
        apply_product_records(&self.product, state, known_hosts)?;
        Ok(outcome)
    }

    pub fn recover(&self) -> Result<()> {
        self.product.recover_repository()?;
        Ok(())
    }

    pub fn product(&self) -> &ReplicationProductService<B> {
        &self.product
    }
}

fn canonical_text(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).context("replication package is not canonical UTF-8")
}

fn authority_package(update: ReplicationAuthorityUpdate) -> Result<DesktopAuthorityPackage> {
    Ok(DesktopAuthorityPackage {
        authority_revision: update.authority_revision,
        payload: canonical_text(update.to_canonical_bytes()?)?,
    })
}

pub(crate) fn desktop_replication_root() -> Result<PathBuf> {
    Ok(crate::storage::app_dir()?.join("replication-v1"))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DesktopRecord<T> {
    schema_version: u16,
    stable_id: String,
    value: T,
}

fn reconcile_local_records<B: ReplicationSecretBackend>(
    product: &ReplicationProductService<B>,
    state: &SavedState,
    known_hosts: &KnownHostStore,
) -> Result<DesktopReplicationChanges> {
    let desired = desired_records(state, known_hosts)?;
    let current = product
        .records()?
        .into_iter()
        .filter(|record| is_managed_collection(record.key()))
        .map(|record| (record.key().clone(), record.value().map(ToOwned::to_owned)))
        .collect::<BTreeMap<_, _>>();
    let mut changes = DesktopReplicationChanges::default();

    for (key, value) in &desired {
        if current.get(key).and_then(|value| value.as_deref()) != Some(value.as_slice()) {
            product.put_record(key.clone(), value)?;
            changes.puts += 1;
        }
    }
    for (key, value) in current {
        if value.is_some() && !desired.contains_key(&key) {
            product.delete_record(key)?;
            changes.deletes += 1;
        }
    }
    Ok(changes)
}

fn desired_records(
    state: &SavedState,
    known_hosts: &KnownHostStore,
) -> Result<BTreeMap<ReplicationRecordKey, Vec<u8>>> {
    let mut records = BTreeMap::new();
    for vault in state
        .vaults
        .iter()
        .filter(|vault| vault.id != DEFAULT_VAULT_ID)
    {
        insert_record(&mut records, VAULTS, &vault.id, vault)?;
    }
    for profile in state
        .profiles
        .iter()
        .filter(|profile| profile.source == ProfileSource::User)
    {
        let mut profile = profile.clone();
        profile.password_credential_id = None;
        insert_record(&mut records, PROFILES, &profile.id.clone(), &profile)?;
    }
    for identity in state
        .identities
        .iter()
        .filter(|identity| identity.source == IdentitySource::User)
    {
        insert_record(&mut records, IDENTITIES, &identity.id, identity)?;
    }
    for snippet in &state.snippets {
        insert_record(&mut records, SNIPPETS, &snippet.id, snippet)?;
    }
    for (endpoint, host_key) in known_hosts.entries()? {
        insert_record(&mut records, KNOWN_HOSTS, &endpoint, &host_key)?;
    }
    Ok(records)
}

fn insert_record<T: Serialize>(
    records: &mut BTreeMap<ReplicationRecordKey, Vec<u8>>,
    collection: &str,
    stable_id: &str,
    value: &T,
) -> Result<()> {
    let key = record_key(collection, stable_id)?;
    let bytes = serde_json::to_vec(&DesktopRecord {
        schema_version: DESKTOP_RECORD_VERSION,
        stable_id: stable_id.to_string(),
        value,
    })?;
    if records.insert(key, bytes).is_some() {
        bail!("duplicate desktop replication record");
    }
    Ok(())
}

fn record_key(collection: &str, stable_id: &str) -> Result<ReplicationRecordKey> {
    let mut digest = Sha256::new();
    digest.update(RECORD_ID_DOMAIN);
    digest.update(collection.as_bytes());
    digest.update([0]);
    digest.update(stable_id.as_bytes());
    let record_id = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(ReplicationRecordKey::new(
        ReplicationCollectionId::new(collection)?,
        ReplicationRecordId::new(record_id)?,
    ))
}

fn apply_product_records<B: ReplicationSecretBackend>(
    product: &ReplicationProductService<B>,
    state: &mut SavedState,
    known_hosts: &KnownHostStore,
) -> Result<()> {
    let decoded = decode_product_records(product.records()?)?;

    state.vaults.retain(|vault| vault.id == DEFAULT_VAULT_ID);
    state.vaults.extend(decoded.vaults);
    state
        .profiles
        .retain(|profile| profile.source != ProfileSource::User);
    state.profiles.extend(decoded.profiles);
    state
        .identities
        .retain(|identity| identity.source != IdentitySource::User);
    state.identities.extend(decoded.identities);
    state.snippets = decoded.snippets;
    state.ensure_vaults();
    known_hosts.replace_entries(decoded.known_hosts)?;
    Ok(())
}

#[derive(Default)]
struct DecodedRecords {
    vaults: Vec<SavedVault>,
    profiles: Vec<HostProfile>,
    identities: Vec<SavedIdentity>,
    snippets: Vec<SavedSnippet>,
    known_hosts: HashMap<String, String>,
}

fn decode_product_records(records: Vec<ReplicationProductRecord>) -> Result<DecodedRecords> {
    let mut decoded = DecodedRecords::default();
    for record in records {
        let Some(bytes) = record.value() else {
            continue;
        };
        match record.key().collection.as_str() {
            VAULTS => decoded.vaults.push(decode_record(record.key(), bytes)?),
            PROFILES => {
                let mut profile: HostProfile = decode_record(record.key(), bytes)?;
                profile.source = ProfileSource::User;
                profile.password_credential_id = None;
                decoded.profiles.push(profile);
            }
            IDENTITIES => {
                let mut identity: SavedIdentity = decode_record(record.key(), bytes)?;
                identity.source = IdentitySource::User;
                decoded.identities.push(identity);
            }
            SNIPPETS => decoded.snippets.push(decode_record(record.key(), bytes)?),
            KNOWN_HOSTS => {
                let record: DesktopRecord<String> = decode_record_envelope(record.key(), bytes)?;
                if decoded
                    .known_hosts
                    .insert(record.stable_id, record.value)
                    .is_some()
                {
                    bail!("duplicate replicated known host");
                }
            }
            _ => {}
        }
    }
    Ok(decoded)
}

fn decode_record<T: DeserializeOwned>(key: &ReplicationRecordKey, bytes: &[u8]) -> Result<T> {
    Ok(decode_record_envelope::<T>(key, bytes)?.value)
}

fn decode_record_envelope<T: DeserializeOwned>(
    key: &ReplicationRecordKey,
    bytes: &[u8],
) -> Result<DesktopRecord<T>> {
    let record: DesktopRecord<T> = serde_json::from_slice(bytes)?;
    if record.schema_version != DESKTOP_RECORD_VERSION
        || record_key(key.collection.as_str(), &record.stable_id)? != *key
    {
        bail!("replicated desktop record is invalid");
    }
    Ok(record)
}

fn decode_record_value(key: &ReplicationRecordKey, bytes: &[u8]) -> Result<DesktopRecord<Value>> {
    decode_record_envelope(key, bytes)
}

fn record_summary(record: &DesktopRecord<Value>) -> String {
    record
        .value
        .get("label")
        .and_then(Value::as_str)
        .or_else(|| record.value.as_str())
        .unwrap_or(&record.stable_id)
        .to_string()
}

fn is_managed_collection(key: &ReplicationRecordKey) -> bool {
    matches!(
        key.collection.as_str(),
        VAULTS | PROFILES | IDENTITIES | SNIPPETS | KNOWN_HOSTS
    )
}

fn collection_label(key: &ReplicationRecordKey) -> &'static str {
    match key.collection.as_str() {
        VAULTS => "Vault",
        PROFILES => "Connection",
        IDENTITIES => "Key",
        SNIPPETS => "Snippet",
        KNOWN_HOSTS => "Known host",
        _ => "Item",
    }
}

pub(crate) fn replication_is_configured(root: &Path) -> bool {
    root.join("profile.json").is_file()
}

pub(crate) fn replication_has_pending_enrollment(root: &Path) -> bool {
    root.join("pending-enrollment.json").is_file()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use termirust_store::{ReplicationSecretRef, ReplicationSecretStoreError};
    use zeroize::Zeroizing;

    use super::*;
    use crate::models::{AuthMode, SavedSnippet};
    use crate::storage::{KnownHostStore, set_test_app_dir_override};

    #[derive(Clone, Default)]
    struct MemorySecrets(Arc<Mutex<HashMap<Vec<u8>, Vec<u8>>>>);

    impl ReplicationSecretBackend for MemorySecrets {
        fn put(
            &self,
            reference: &ReplicationSecretRef,
            secret: &[u8],
        ) -> std::result::Result<(), ReplicationSecretStoreError> {
            let mut values = self.0.lock().unwrap();
            if values
                .insert(reference.to_bytes().to_vec(), secret.to_vec())
                .is_some()
            {
                return Err(ReplicationSecretStoreError::Collision);
            }
            Ok(())
        }

        fn get(
            &self,
            reference: &ReplicationSecretRef,
        ) -> std::result::Result<Zeroizing<Vec<u8>>, ReplicationSecretStoreError> {
            self.0
                .lock()
                .unwrap()
                .get(reference.to_bytes().as_slice())
                .cloned()
                .map(Zeroizing::new)
                .ok_or(ReplicationSecretStoreError::Missing)
        }

        fn delete(
            &self,
            reference: &ReplicationSecretRef,
        ) -> std::result::Result<bool, ReplicationSecretStoreError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .remove(reference.to_bytes().as_slice())
                .is_some())
        }
    }

    fn profile(id: &str, label: &str) -> HostProfile {
        HostProfile {
            id: id.to_string(),
            label: label.to_string(),
            host: "example.test".to_string(),
            port: 22,
            username: "alice".to_string(),
            auth_mode: AuthMode::PrivateKey,
            source: ProfileSource::User,
            ..HostProfile::default()
        }
    }

    #[test]
    fn desktop_records_converge_across_enrolled_devices_and_delete_exactly() {
        let app_dir = tempfile::tempdir().unwrap();
        let _previous = set_test_app_dir_override(Some(app_dir.path().to_path_buf()));
        let first_known_hosts = KnownHostStore::load().unwrap();
        first_known_hosts
            .verify_or_trust("example.test:22", "ssh-ed25519 AAAA")
            .unwrap();
        let shared = tempfile::tempdir().unwrap();
        let first_root = app_dir.path().join("first-replica");
        let second_root = app_dir.path().join("second-replica");
        let backend = MemorySecrets::default();
        let mut first =
            DesktopReplication::bootstrap(&first_root, shared.path(), backend.clone()).unwrap();

        let mut first_state = SavedState::default();
        first_state.ensure_vaults();
        first_state.profiles.push(profile("host-one", "Production"));
        first_state.snippets.push(SavedSnippet {
            id: "snippet-one".to_string(),
            label: "Inspect".to_string(),
            command: "uptime".to_string(),
            ..SavedSnippet::default()
        });
        let review = first.review(&first_state, &first_known_hosts).unwrap();
        assert_eq!(review.changes.puts, 3);
        assert_eq!(
            review.disposition(),
            ReplicationSyncDisposition::PublishLocal
        );
        first
            .apply(review, &mut first_state, &first_known_hosts)
            .unwrap();

        let request =
            DesktopReplication::prepare_enrollment(&second_root, shared.path(), backend.clone())
                .unwrap();
        let request_text = String::from_utf8(request.to_canonical_bytes().unwrap()).unwrap();
        assert_eq!(
            DesktopReplication::<MemorySecrets>::pending_enrollment_request(&second_root).unwrap(),
            request_text
        );
        let package = first.enroll_text(&request_text).unwrap();
        assert_eq!(first.status().unwrap().active_devices, 2);
        assert_eq!(
            first
                .devices()
                .iter()
                .filter(|device| device.active)
                .count(),
            2
        );
        assert!(first.pending_authority_package().unwrap().is_some());
        let review = first.review(&first_state, &first_known_hosts).unwrap();
        first
            .apply(review, &mut first_state, &first_known_hosts)
            .unwrap();

        let second = DesktopReplication::accept_enrollment_text(
            &second_root,
            backend,
            &package.bundle,
            &package.verification_code,
        )
        .unwrap();
        let second_known_hosts =
            KnownHostStore::open(app_dir.path().join("second-known-hosts.json")).unwrap();
        let mut second_state = SavedState::default();
        second_state.ensure_vaults();
        let review = second.review(&second_state, &second_known_hosts).unwrap();
        assert_eq!(
            review.disposition(),
            ReplicationSyncDisposition::UpdateLocal
        );
        second
            .apply(review, &mut second_state, &second_known_hosts)
            .unwrap();
        assert_eq!(second_state.profiles[0].label, "Production");
        assert_eq!(second_state.snippets.len(), 1);
        assert_eq!(second_known_hosts.entries().unwrap().len(), 1);

        second_state.profiles[0].label = "Production from phone".to_string();
        let review = second.review(&second_state, &second_known_hosts).unwrap();
        second
            .apply(review, &mut second_state, &second_known_hosts)
            .unwrap();
        let review = first.review(&first_state, &first_known_hosts).unwrap();
        first
            .apply(review, &mut first_state, &first_known_hosts)
            .unwrap();
        assert_eq!(first_state.profiles[0].label, "Production from phone");

        first_state.profiles.clear();
        let review = first.review(&first_state, &first_known_hosts).unwrap();
        assert_eq!(review.changes.deletes, 1);
        first
            .apply(review, &mut first_state, &first_known_hosts)
            .unwrap();
        let review = second.review(&second_state, &second_known_hosts).unwrap();
        second
            .apply(review, &mut second_state, &second_known_hosts)
            .unwrap();
        assert!(second_state.profiles.is_empty());

        set_test_app_dir_override(None);
    }

    #[test]
    fn malformed_or_swapped_record_binding_is_rejected_before_state_changes() {
        let key = record_key(PROFILES, "host-one").unwrap();
        let bytes = serde_json::to_vec(&DesktopRecord {
            schema_version: DESKTOP_RECORD_VERSION,
            stable_id: "host-two".to_string(),
            value: profile("host-one", "Production"),
        })
        .unwrap();
        assert!(decode_record::<HostProfile>(&key, &bytes).is_err());
    }
}
