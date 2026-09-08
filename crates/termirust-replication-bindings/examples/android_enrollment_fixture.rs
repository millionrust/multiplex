//! Disposable desktop side of the Android acceptance test. Never uses user custody.
use std::{
    collections::{HashMap, hash_map::Entry},
    fs,
    io::Read,
    sync::{Arc, Mutex},
};
use termirust_domain::{ReplicationCollectionId, ReplicationRecordId, ReplicationRecordKey};
use termirust_replication_bindings::{
    NativeReplicationSecretBackend, ReplicationSecureStore, ReplicationStorageError,
};
use termirust_store::{ReplicationEnrollmentRequest, ReplicationProductService};
use zeroize::Zeroizing;

#[derive(Default)]
struct FixtureStore(Mutex<HashMap<String, Zeroizing<Vec<u8>>>>);
impl ReplicationSecureStore for FixtureStore {
    fn create(&self, account: String, value: Vec<u8>) -> Result<(), ReplicationStorageError> {
        match self.0.lock().unwrap().entry(account) {
            Entry::Occupied(_) => Err(ReplicationStorageError::Collision),
            Entry::Vacant(entry) => {
                entry.insert(Zeroizing::new(value));
                Ok(())
            }
        }
    }
    fn load(&self, account: String) -> Result<Vec<u8>, ReplicationStorageError> {
        self.0
            .lock()
            .unwrap()
            .get(&account)
            .map(|v| v.to_vec())
            .ok_or(ReplicationStorageError::Missing)
    }
    fn delete(&self, account: String) -> Result<bool, ReplicationStorageError> {
        Ok(self.0.lock().unwrap().remove(&account).is_some())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("expected request file and new output directory".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(&args[0])?
        .take(4097)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err("fixture request exceeds limit".into());
    }
    let request = ReplicationEnrollmentRequest::from_canonical_bytes(&bytes)?;
    let output = std::path::PathBuf::from(&args[1]);
    fs::create_dir(&output)?;
    let directory = tempfile::tempdir()?;
    let exchange = tempfile::tempdir()?;
    let mut desktop = ReplicationProductService::bootstrap(
        directory.path().join("owner"),
        exchange.path(),
        NativeReplicationSecretBackend::new(Arc::new(FixtureStore::default())),
    )?;
    let bundle = desktop.enroll_request(&request)?;
    let id = "79ac23a7e0e3d16216f125ae099fad0f3edc0dca33540c12c25c9d44766aaf16";
    let host = serde_json::json!({"schema_version":1,"stable_id":"profile-1","value":{
        "id":"profile-1","label":"Desktop fixture host","host":"example.test","port":2222,"username":"demo",
        "startup_command":"never execute","password_credential_id":"never reuse"}});
    desktop.put_record(
        ReplicationRecordKey::new(
            ReplicationCollectionId::new("desktop-profiles")?,
            ReplicationRecordId::new(id)?,
        ),
        &serde_json::to_vec(&host)?,
    )?;
    let published = desktop.apply_sync(&desktop.review_sync()?)?;
    fs::write(output.join("bundle.json"), bundle.to_canonical_bytes()?)?;
    fs::write(
        output.join("replica.json"),
        serde_json::to_vec(&published.transport.document)?,
    )?;
    fs::write(
        output.join("review.json"),
        serde_json::to_vec(&serde_json::json!({
            "code": bundle.verification_code(&request)?, "record_id": id
        }))?,
    )?;
    println!("PASS: disposable desktop bundle and encrypted host fixture created");
    Ok(())
}
