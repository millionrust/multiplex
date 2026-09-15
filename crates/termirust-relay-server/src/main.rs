use base64::Engine as _;
use rand_core::{OsRng, RngCore};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::Serialize;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use termirust_relay_protocol::{
    RELAY_LOOPBACK_ORIGIN, RelayAdmissionCredential, RelayRevocationEpoch, RelayRouteId,
    RelayRouteRegistration,
};
use termirust_relay_server::{
    RelayMetadataStore, RelayServer, RelayServerConfig, RelayServerLimits, RelayTlsServerConfig,
};
use zeroize::Zeroize;

#[derive(Serialize)]
struct RoutePackage<'a> {
    schema: &'static str,
    schema_version: u32,
    role: &'a str,
    endpoint: &'a str,
    spki_pin: &'a str,
    relay_route_id: String,
    relay_revocation_epoch: u64,
    admission_credential: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("provision") => provision(parse_options(args)?),
        Some("revoke") => revoke(parse_options(args)?),
        Some("remove") => remove(parse_options(args)?),
        Some("run") => run_server(parse_options(args)?).await,
        Some("help") | Some("--help") | Some("-h") | None => {
            print_help();
            Ok(())
        }
        Some(_) => Err("unknown command; run `termirust-relay help`".to_owned()),
    }
}

fn provision(options: Vec<(String, String)>) -> Result<(), String> {
    ensure_allowed(&options, &["state", "endpoint", "spki-pin", "output-dir"])?;
    let state = required(&options, "state")?;
    let endpoint = required(&options, "endpoint")?;
    let spki_pin = required(&options, "spki-pin")?;
    let output = PathBuf::from(required(&options, "output-dir")?);
    if !endpoint.starts_with("wss://") || !endpoint.ends_with("/relay/v1") {
        return Err("--endpoint must be a wss:// URL ending in /relay/v1".to_owned());
    }
    validate_base64_pin(spki_pin)?;
    fs::create_dir_all(&output).map_err(redacted_io)?;
    set_private_directory(&output)?;

    let mut route = [0_u8; 32];
    let mut host_secret = [0_u8; 32];
    let mut controller_secret = [0_u8; 32];
    OsRng.fill_bytes(&mut route);
    OsRng.fill_bytes(&mut host_secret);
    OsRng.fill_bytes(&mut controller_secret);
    let host = RelayAdmissionCredential::from_secret_bytes(host_secret);
    let controller = RelayAdmissionCredential::from_secret_bytes(controller_secret);
    let registration = RelayRouteRegistration::new(RelayRouteId(route), &host, &controller);

    let route_id = base64::engine::general_purpose::STANDARD.encode(route);
    let mut host_package = RoutePackage {
        schema: "termirust-relay-route",
        schema_version: 1,
        role: "host",
        endpoint,
        spki_pin,
        relay_route_id: route_id.clone(),
        relay_revocation_epoch: 0,
        admission_credential: base64::engine::general_purpose::STANDARD.encode(host_secret),
    };
    let mut controller_package = RoutePackage {
        schema: "termirust-relay-route",
        schema_version: 1,
        role: "controller",
        endpoint,
        spki_pin,
        relay_route_id: route_id,
        relay_revocation_epoch: 0,
        admission_credential: base64::engine::general_purpose::STANDARD.encode(controller_secret),
    };
    host_secret.zeroize();
    controller_secret.zeroize();

    let host_path = output.join("host-route.json");
    let controller_path = output.join("controller-route.json");
    let result = (|| {
        write_private_json(&host_path, &host_package)?;
        if let Err(error) = write_private_json(&controller_path, &controller_package) {
            remove_if_exists(&host_path);
            return Err(error);
        }
        if let Err(error) = sync_directory(&output) {
            remove_if_exists(&host_path);
            remove_if_exists(&controller_path);
            return Err(error);
        }

        let state_result = (|| {
            let store = RelayMetadataStore::acquire(state).map_err(relay_error)?;
            let mut registrations = store.load().map_err(relay_error)?;
            if registrations
                .iter()
                .any(|existing| existing.route_id == registration.route_id)
            {
                return Err(
                    "generated route ID is already registered; retry provisioning".to_owned(),
                );
            }
            registrations.push(registration);
            store.commit(&registrations).map_err(relay_error)
        })();
        if let Err(error) = state_result {
            remove_if_exists(&host_path);
            remove_if_exists(&controller_path);
            return Err(error);
        }
        Ok(())
    })();
    host_package.admission_credential.zeroize();
    controller_package.admission_credential.zeroize();
    result?;
    println!("Provisioned one relay route.");
    println!("Host package: {}", host_path.display());
    println!("Controller package: {}", controller_path.display());
    println!("Treat both files as secrets and delete each after importing it.");
    Ok(())
}

fn revoke(options: Vec<(String, String)>) -> Result<(), String> {
    ensure_allowed(&options, &["state", "route-id"])?;
    let state = required(&options, "state")?;
    let encoded_route = required(&options, "route-id")?;
    let route = decode_route(encoded_route)?;
    let store = RelayMetadataStore::acquire(state).map_err(relay_error)?;
    let mut registrations = store.load().map_err(relay_error)?;
    let registration = registrations
        .iter_mut()
        .find(|registration| registration.route_id == route)
        .ok_or_else(|| "route ID is not registered".to_owned())?;
    registration.revoked = true;
    registration.revocation_epoch = RelayRevocationEpoch(
        registration
            .revocation_epoch
            .0
            .checked_add(1)
            .ok_or_else(|| "route revocation epoch is exhausted".to_owned())?,
    );
    let epoch = registration.revocation_epoch.0;
    store.commit(&registrations).map_err(relay_error)?;
    println!("Revoked relay route at epoch {epoch}.");
    Ok(())
}

fn remove(options: Vec<(String, String)>) -> Result<(), String> {
    ensure_allowed(&options, &["state", "route-id"])?;
    let state = required(&options, "state")?;
    let route = decode_route(required(&options, "route-id")?)?;
    let store = RelayMetadataStore::acquire(state).map_err(relay_error)?;
    let mut registrations = store.load().map_err(relay_error)?;
    let original_len = registrations.len();
    registrations.retain(|registration| registration.route_id != route);
    if registrations.len() == original_len {
        return Err("route ID is not registered".to_owned());
    }
    store.commit(&registrations).map_err(relay_error)?;
    println!("Removed relay route metadata.");
    Ok(())
}

async fn run_server(options: Vec<(String, String)>) -> Result<(), String> {
    ensure_allowed(&options, &["state", "bind", "cert", "key"])?;
    let state = PathBuf::from(required(&options, "state")?);
    let bind: SocketAddr = required(&options, "bind")?
        .parse()
        .map_err(|_| "--bind must be a valid loopback IP and port".to_owned())?;
    if !bind.ip().is_loopback() {
        return Err("--bind must use a loopback address".to_owned());
    }
    let config = RelayServerConfig {
        bind,
        state_path: state,
        allowed_origin: RELAY_LOOPBACK_ORIGIN.to_owned(),
        limits: RelayServerLimits::default(),
    };
    let cert = optional(&options, "cert");
    let key = optional(&options, "key");
    let handle = match (cert, key) {
        (None, None) => RelayServer::start(config).await,
        (Some(cert), Some(key)) => {
            let tls = load_tls(Path::new(cert), Path::new(key))?;
            RelayServer::start_tls(config, tls).await
        }
        _ => return Err("--cert and --key must be provided together".to_owned()),
    }
    .map_err(relay_error)?;
    println!("TermiRust relay listening on {}", handle.websocket_url());
    println!("The relay stores route verifiers only and never stores forwarded frames.");
    tokio::signal::ctrl_c()
        .await
        .map_err(|_| "signal handling failed".to_owned())?;
    handle.shutdown().await.map_err(relay_error)
}

fn load_tls(cert_path: &Path, key_path: &Path) -> Result<RelayTlsServerConfig, String> {
    let mut cert_reader = BufReader::new(File::open(cert_path).map_err(redacted_io)?);
    let certificates: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<_, _>>()
        .map_err(|_| "certificate PEM is invalid".to_owned())?;
    if certificates.is_empty() {
        return Err("certificate PEM is empty".to_owned());
    }
    let mut key_reader = BufReader::new(File::open(key_path).map_err(redacted_io)?);
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|_| "private-key PEM is invalid".to_owned())?
        .ok_or_else(|| "private-key PEM is empty".to_owned())?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| "TLS protocol configuration failed".to_owned())?
    .with_no_client_auth()
    .with_single_cert(certificates, key)
    .map_err(|_| "certificate and private key do not match".to_owned())?;
    Ok(RelayTlsServerConfig::new(config))
}

fn parse_options(args: impl Iterator<Item = String>) -> Result<Vec<(String, String)>, String> {
    let mut args = args.peekable();
    let mut result = Vec::new();
    while let Some(name) = args.next() {
        if !name.starts_with("--") {
            return Err("options must start with --".to_owned());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{name} requires a value"))?;
        if value.starts_with("--") {
            return Err(format!("{name} requires a value"));
        }
        let key = name.trim_start_matches("--").to_owned();
        if result.iter().any(|(existing, _)| existing == &key) {
            return Err(format!("duplicate option --{key}"));
        }
        result.push((key, value));
    }
    Ok(result)
}

fn required<'a>(options: &'a [(String, String)], name: &str) -> Result<&'a str, String> {
    optional(options, name).ok_or_else(|| format!("--{name} is required"))
}

fn optional<'a>(options: &'a [(String, String)], name: &str) -> Option<&'a str> {
    options
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn ensure_allowed(options: &[(String, String)], allowed: &[&str]) -> Result<(), String> {
    if let Some((name, _)) = options
        .iter()
        .find(|(name, _)| !allowed.contains(&name.as_str()))
    {
        return Err(format!("unknown option --{name}"));
    }
    Ok(())
}

fn decode_route(value: &str) -> Result<RelayRouteId, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| "route ID must be Base64".to_owned())?;
    let route: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "route ID must contain 32 bytes".to_owned())?;
    Ok(RelayRouteId(route))
}

fn validate_base64_pin(pin: &str) -> Result<(), String> {
    let encoded = pin
        .strip_prefix("sha256/")
        .ok_or_else(|| "--spki-pin must start with sha256/".to_owned())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "--spki-pin must contain Base64".to_owned())?;
    if bytes.len() != 32 {
        return Err("--spki-pin must contain a 32-byte digest".to_owned());
    }
    Ok(())
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| "route package encoding failed".to_owned())?;
    let mut file = private_create_new(path)?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(redacted_io)
}

fn remove_if_exists(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(redacted_io)
}

#[cfg(unix)]
fn private_create_new(path: &Path) -> Result<File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(redacted_io)
}

#[cfg(not(unix))]
fn private_create_new(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(redacted_io)
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(redacted_io)
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn relay_error(error: termirust_relay_server::RelayServerError) -> String {
    format!("relay operation failed ({})", error.code().as_str())
}

fn redacted_io(error: std::io::Error) -> String {
    format!("filesystem operation failed ({:?})", error.kind())
}

fn print_help() {
    println!("TermiRust ciphertext relay operator\n");
    println!("Provision:");
    println!(
        "  termirust-relay provision --state PATH --endpoint wss://HOST/relay/v1 --spki-pin sha256/BASE64 --output-dir DIR"
    );
    println!("Run behind a TLS reverse proxy:");
    println!("  termirust-relay run --state PATH --bind 127.0.0.1:7878");
    println!("Run with TLS directly:");
    println!(
        "  termirust-relay run --state PATH --bind 127.0.0.1:7878 --cert CERT.pem --key KEY.pem"
    );
    println!("Revoke:");
    println!("  termirust-relay revoke --state PATH --route-id BASE64");
    println!("Remove route metadata:");
    println!("  termirust-relay remove --state PATH --route-id BASE64");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tempfile::TempDir;

    const TEST_PIN: &str = "sha256/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    fn options(values: &[(&str, &str)]) -> Vec<(String, String)> {
        values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn provision_in(temp: &TempDir) -> (PathBuf, PathBuf) {
        let state = temp.path().join("state/relay.json");
        let output = temp.path().join("packages");
        provision(options(&[
            ("state", state.to_str().unwrap()),
            ("endpoint", "wss://relay.example.test/relay/v1"),
            ("spki-pin", TEST_PIN),
            ("output-dir", output.to_str().unwrap()),
        ]))
        .unwrap();
        (state, output)
    }

    #[test]
    fn provision_persists_verifiers_but_not_credentials() {
        let temp = TempDir::new().unwrap();
        let (state, output) = provision_in(&temp);
        let host: Value =
            serde_json::from_slice(&fs::read(output.join("host-route.json")).unwrap()).unwrap();
        let controller: Value =
            serde_json::from_slice(&fs::read(output.join("controller-route.json")).unwrap())
                .unwrap();
        assert_eq!(host["role"], "host");
        assert_eq!(controller["role"], "controller");
        assert_eq!(host["relay_route_id"], controller["relay_route_id"]);

        let state_text = fs::read_to_string(&state).unwrap();
        assert!(!state_text.contains(host["admission_credential"].as_str().unwrap()));
        assert!(!state_text.contains(controller["admission_credential"].as_str().unwrap()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(output.join("host-route.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn provision_refuses_to_replace_packages_without_registering_a_route() {
        let temp = TempDir::new().unwrap();
        let state = temp.path().join("state/relay.json");
        let output = temp.path().join("packages");
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join("host-route.json"), b"keep-me").unwrap();

        let error = provision(options(&[
            ("state", state.to_str().unwrap()),
            ("endpoint", "wss://relay.example.test/relay/v1"),
            ("spki-pin", TEST_PIN),
            ("output-dir", output.to_str().unwrap()),
        ]))
        .unwrap_err();
        assert!(error.contains("AlreadyExists"));
        assert_eq!(
            fs::read(output.join("host-route.json")).unwrap(),
            b"keep-me"
        );
        assert!(!state.exists());
    }

    #[test]
    fn revoke_then_remove_updates_only_the_selected_route() {
        let temp = TempDir::new().unwrap();
        let (state, output) = provision_in(&temp);
        let package: Value =
            serde_json::from_slice(&fs::read(output.join("host-route.json")).unwrap()).unwrap();
        let route = package["relay_route_id"].as_str().unwrap();

        revoke(options(&[
            ("state", state.to_str().unwrap()),
            ("route-id", route),
        ]))
        .unwrap();
        let store = RelayMetadataStore::acquire(&state).unwrap();
        let registrations = store.load().unwrap();
        assert_eq!(registrations.len(), 1);
        assert!(registrations[0].revoked);
        assert_eq!(registrations[0].revocation_epoch.0, 1);
        drop(store);

        remove(options(&[
            ("state", state.to_str().unwrap()),
            ("route-id", route),
        ]))
        .unwrap();
        let store = RelayMetadataStore::acquire(&state).unwrap();
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn command_options_reject_typos() {
        let error = ensure_allowed(&options(&[("stat", "x")]), &["state"]).unwrap_err();
        assert_eq!(error, "unknown option --stat");
    }
}
