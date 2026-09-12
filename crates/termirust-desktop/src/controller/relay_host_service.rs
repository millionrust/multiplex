use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use termirust_relay_client::{
    RelayCredentialRef, RelayCredentialSecret, RelayEndpointConfig, RelayEndpointId,
    RelayRevocationEpoch, RelayRouteErrorCode, RelayRouteId, RelaySecretStore, RelaySpkiPin,
    RelayWssUrl,
};
use termirust_store::{AtomicWriter as _, SystemAtomicWriter};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize;

use super::host_identity::{HostIdentityService, OsIdentityEntropy, OsSecretStore};
use super::relay_host::{OsRelaySecretStore, RelayHostError, RelayHostRouteOwner};

const MAX_PACKAGE_BYTES: u64 = 16 * 1_024;
const INSTALLED_ROUTE_FILE: &str = "relay-host-route.json";

#[derive(Debug)]
pub struct RelayHostServiceError(&'static str);

impl RelayHostServiceError {
    pub const fn code(&self) -> &'static str {
        self.0
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostRoutePackage {
    schema: String,
    schema_version: u32,
    role: String,
    endpoint: String,
    spki_pin: String,
    relay_route_id: String,
    relay_revocation_epoch: u64,
    admission_credential: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstalledHostRoute {
    schema: String,
    schema_version: u32,
    endpoint: String,
    spki_pin: String,
    relay_route_id: String,
    relay_revocation_epoch: u64,
    credential_ref: String,
}

pub fn run_command(args: &[String]) -> Result<(), RelayHostServiceError> {
    let root = crate::storage::controller_store_dir()
        .map_err(|_| RelayHostServiceError("relay.host.storage_unavailable"))?;
    let installed = root.join(INSTALLED_ROUTE_FILE);
    match args.first().map(String::as_str) {
        Some("install") if args.len() == 3 && args[1] == "--package" => {
            install(Path::new(&args[2]), &installed, &OsRelaySecretStore)?;
            println!("Self-hosted relay route installed. Delete the source package securely.");
            Ok(())
        }
        Some("remove") if args.len() == 1 => {
            remove(&installed, &OsRelaySecretStore)?;
            println!("Self-hosted relay route removed. Local Sessions were not changed.");
            Ok(())
        }
        Some("status") if args.len() == 1 => {
            if installed.exists() {
                let _ = load_installed(&installed)?;
                println!("Self-hosted relay route is configured.");
            } else {
                println!("Self-hosted relay route is not configured.");
            }
            Ok(())
        }
        Some("run") if args.len() == 1 => run_foreground(installed),
        _ => Err(RelayHostServiceError("relay.host.usage")),
    }
}

fn install(
    package_path: &Path,
    installed_path: &Path,
    secrets: &dyn RelaySecretStore,
) -> Result<(), RelayHostServiceError> {
    if installed_path.exists() {
        return Err(RelayHostServiceError("relay.host.already_configured"));
    }
    let package = load_package(package_path)?;
    let (installed, endpoint, mut secret) = validate_package(package)?;
    let reference = endpoint.credential_ref.clone();
    secrets
        .put(&reference, &secret)
        .map_err(|_| RelayHostServiceError("relay.host.credential_store_failed"))?;
    secret.zeroize();

    let encoded = serde_json::to_vec_pretty(&installed)
        .map_err(|_| RelayHostServiceError("relay.host.config_invalid"))?;
    if let Some(parent) = installed_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| RelayHostServiceError("relay.host.storage_unavailable"))?;
    }
    if SystemAtomicWriter.write(installed_path, &encoded).is_err() {
        let _ = secrets.delete(&reference);
        return Err(RelayHostServiceError("relay.host.config_write_failed"));
    }
    Ok(())
}

fn remove(
    installed_path: &Path,
    secrets: &dyn RelaySecretStore,
) -> Result<(), RelayHostServiceError> {
    let installed = load_installed(installed_path)?;
    let reference = RelayCredentialRef::new(installed.credential_ref)
        .map_err(|_| RelayHostServiceError("relay.host.config_invalid"))?;
    secrets
        .delete(&reference)
        .map_err(|_| RelayHostServiceError("relay.host.credential_remove_failed"))?;
    fs::remove_file(installed_path)
        .map_err(|_| RelayHostServiceError("relay.host.config_remove_failed"))
}

fn run_foreground(installed_path: PathBuf) -> Result<(), RelayHostServiceError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| RelayHostServiceError("relay.host.runtime_failed"))?;
    runtime.block_on(async move {
        let installed = load_installed(&installed_path)?;
        let endpoint = installed.endpoint()?;
        let app_root = crate::storage::app_dir()
            .map_err(|_| RelayHostServiceError("relay.host.storage_unavailable"))?;
        let controller_root = crate::storage::controller_store_dir()
            .map_err(|_| RelayHostServiceError("relay.host.storage_unavailable"))?;
        let repository = termirust_store::ControllerDeviceRepository::open(controller_root.clone())
            .map_err(|_| RelayHostServiceError("relay.host.identity_unavailable"))?;
        let identity = HostIdentityService::new(repository, OsSecretStore, OsIdentityEntropy)
            .load_or_create()
            .map_err(|_| RelayHostServiceError("relay.host.identity_unavailable"))?;
        let host_private = identity
            .static_private_key()
            .ok_or(RelayHostServiceError("relay.host.identity_unavailable"))?;
        let runtime_parent = crate::controller_runtime_parent(&app_root);
        let project_root = crate::storage::project_store_dir()
            .map_err(|_| RelayHostServiceError("relay.host.storage_unavailable"))?;
        let owner = RelayHostRouteOwner::new(endpoint, Arc::new(OsRelaySecretStore));
        let cancel = CancellationToken::new();
        let signal_cancel = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                signal_cancel.cancel();
            }
        });
        println!("Self-hosted relay route is running. Press Ctrl-C to stop.");
        while !cancel.is_cancelled() {
            let result = owner
                .serve_repository(
                    controller_root.clone(),
                    project_root.clone(),
                    app_root.join("durable-sessions"),
                    runtime_parent.clone(),
                    runtime_parent.join("controller-pairing.sock"),
                    host_private.clone(),
                    cancel.child_token(),
                )
                .await;
            if cancel.is_cancelled() {
                return Ok(());
            }
            if let Err(error) = result {
                if !retryable(&error) {
                    return Err(service_error(&error));
                }
                eprintln!("Self-hosted relay disconnected; retrying.");
            }
            tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                () = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
        Ok(())
    })
}

fn load_package(path: &Path) -> Result<HostRoutePackage, RelayHostServiceError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| RelayHostServiceError("relay.host.package_unavailable"))?;
    if !metadata.is_file() || metadata.len() > MAX_PACKAGE_BYTES {
        return Err(RelayHostServiceError("relay.host.package_invalid"));
    }
    let mut bytes =
        fs::read(path).map_err(|_| RelayHostServiceError("relay.host.package_unavailable"))?;
    let decoded = serde_json::from_slice(&bytes)
        .map_err(|_| RelayHostServiceError("relay.host.package_invalid"));
    bytes.zeroize();
    decoded
}

fn load_installed(path: &Path) -> Result<InstalledHostRoute, RelayHostServiceError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| RelayHostServiceError("relay.host.not_configured"))?;
    if !metadata.is_file() || metadata.len() > MAX_PACKAGE_BYTES {
        return Err(RelayHostServiceError("relay.host.config_invalid"));
    }
    let bytes = fs::read(path).map_err(|_| RelayHostServiceError("relay.host.config_invalid"))?;
    let installed: InstalledHostRoute = serde_json::from_slice(&bytes)
        .map_err(|_| RelayHostServiceError("relay.host.config_invalid"))?;
    let _ = installed.endpoint()?;
    Ok(installed)
}

fn validate_package(
    mut package: HostRoutePackage,
) -> Result<
    (
        InstalledHostRoute,
        RelayEndpointConfig,
        RelayCredentialSecret,
    ),
    RelayHostServiceError,
> {
    if package.schema != "termirust-relay-route"
        || package.schema_version != 1
        || package.role != "host"
    {
        package.admission_credential.zeroize();
        return Err(RelayHostServiceError("relay.host.package_invalid"));
    }
    let credential_ref =
        credential_reference(&package.relay_route_id, package.relay_revocation_epoch);
    let installed = InstalledHostRoute {
        schema: "termirust-relay-host-route".to_owned(),
        schema_version: 1,
        endpoint: package.endpoint,
        spki_pin: package.spki_pin,
        relay_route_id: package.relay_route_id,
        relay_revocation_epoch: package.relay_revocation_epoch,
        credential_ref,
    };
    let decoded = base64::engine::general_purpose::STANDARD.decode(&package.admission_credential);
    package.admission_credential.zeroize();
    let mut bytes = decoded.map_err(|_| RelayHostServiceError("relay.host.package_invalid"))?;
    if bytes.len() != 32 {
        bytes.zeroize();
        return Err(RelayHostServiceError("relay.host.package_invalid"));
    }
    let mut secret = [0_u8; 32];
    secret.copy_from_slice(&bytes);
    bytes.zeroize();
    let credential = RelayCredentialSecret::from_bytes(secret);
    let endpoint = installed.endpoint()?;
    Ok((installed, endpoint, credential))
}

impl InstalledHostRoute {
    fn endpoint(&self) -> Result<RelayEndpointConfig, RelayHostServiceError> {
        if self.schema != "termirust-relay-host-route" || self.schema_version != 1 {
            return Err(RelayHostServiceError("relay.host.config_invalid"));
        }
        let route = decode_fixed::<32>(&self.relay_route_id)?;
        let encoded_pin = self
            .spki_pin
            .strip_prefix("sha256/")
            .ok_or(RelayHostServiceError("relay.host.config_invalid"))?;
        let pin = decode_fixed::<32>(encoded_pin)?;
        RelayEndpointConfig::new_host(
            RelayEndpointId::new("installed-host-route")
                .map_err(|_| RelayHostServiceError("relay.host.config_invalid"))?,
            RelayWssUrl::parse(&self.endpoint)
                .map_err(|_| RelayHostServiceError("relay.host.config_invalid"))?,
            RelayRouteId(route),
            RelayCredentialRef::new(self.credential_ref.clone())
                .map_err(|_| RelayHostServiceError("relay.host.config_invalid"))?,
            RelaySpkiPin(pin),
            RelayRevocationEpoch(self.relay_revocation_epoch),
        )
        .map_err(|_| RelayHostServiceError("relay.host.config_invalid"))
    }
}

fn decode_fixed<const N: usize>(encoded: &str) -> Result<[u8; N], RelayHostServiceError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| RelayHostServiceError("relay.host.config_invalid"))?;
    bytes
        .try_into()
        .map_err(|_| RelayHostServiceError("relay.host.config_invalid"))
}

fn credential_reference(route_id: &str, epoch: u64) -> String {
    let safe: String = route_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(48)
        .collect();
    format!("host-{safe}-{epoch}")
}

fn retryable(error: &RelayHostError) -> bool {
    match error {
        RelayHostError::Controller(_) => true,
        RelayHostError::Route(error) => matches!(
            error.code,
            RelayRouteErrorCode::DnsFailed
                | RelayRouteErrorCode::ConnectFailed
                | RelayRouteErrorCode::TlsFailed
                | RelayRouteErrorCode::PeerDisconnected
                | RelayRouteErrorCode::QueuePressure
                | RelayRouteErrorCode::Cancelled
                | RelayRouteErrorCode::UnknownCompletion
                | RelayRouteErrorCode::Internal
        ),
    }
}

fn service_error(error: &RelayHostError) -> RelayHostServiceError {
    match error {
        RelayHostError::Route(error) => RelayHostServiceError(error.diagnostic_id()),
        RelayHostError::Controller(_) => RelayHostServiceError("relay.host.controller_failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_relay_client::MemoryRelaySecretStore;

    fn package(role: &str) -> String {
        serde_json::json!({
            "schema": "termirust-relay-route",
            "schema_version": 1,
            "role": role,
            "endpoint": "wss://relay.example/relay/v1",
            "spki_pin": format!("sha256/{}", base64::engine::general_purpose::STANDARD.encode([4_u8; 32])),
            "relay_route_id": base64::engine::general_purpose::STANDARD.encode([5_u8; 32]),
            "relay_revocation_epoch": 3,
            "admission_credential": base64::engine::general_purpose::STANDARD.encode([6_u8; 32]),
        })
        .to_string()
    }

    #[test]
    fn install_persists_no_secret_and_remove_keeps_unrelated_data() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("host-route.json");
        let installed = temp.path().join("config/relay-host-route.json");
        let unrelated = temp.path().join("config/session.json");
        fs::write(&source, package("host")).unwrap();
        fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        fs::write(&unrelated, b"session-history").unwrap();
        let secrets = MemoryRelaySecretStore::default();

        install(&source, &installed, &secrets).unwrap();
        let text = fs::read_to_string(&installed).unwrap();
        assert!(!text.contains(&base64::engine::general_purpose::STANDARD.encode([6_u8; 32])));
        assert_eq!(
            load_installed(&installed).unwrap().relay_revocation_epoch,
            3
        );

        remove(&installed, &secrets).unwrap();
        assert!(!installed.exists());
        assert_eq!(fs::read(&unrelated).unwrap(), b"session-history");
    }

    #[test]
    fn install_rejects_controller_package_and_existing_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("route.json");
        let installed = temp.path().join("relay-host-route.json");
        let secrets = MemoryRelaySecretStore::default();
        fs::write(&source, package("controller")).unwrap();
        assert_eq!(
            install(&source, &installed, &secrets).unwrap_err().code(),
            "relay.host.package_invalid"
        );
        fs::write(&source, package("host")).unwrap();
        install(&source, &installed, &secrets).unwrap();
        assert_eq!(
            install(&source, &installed, &secrets).unwrap_err().code(),
            "relay.host.already_configured"
        );
    }
}
