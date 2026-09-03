use base64::Engine as _;
use rustls::pki_types::CertificateDer;
use serde::Deserialize;
use std::env;
use std::fs;
use std::sync::Arc;
use termirust_relay_client::{
    RelayClientRole, RelayCredentialRef, RelayEndpointConfig, RelayEndpointId,
    RelayRevocationEpoch, RelayRouteId, RelaySocket, RelaySpkiPin, RelayTlsClientConfig,
    RelayWssUrl,
};
use termirust_relay_protocol::RelayAdmissionCredential;
use zeroize::{Zeroize, Zeroizing};

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

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("relay echo host failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), &'static str> {
    let values: Vec<String> = env::args().skip(1).collect();
    if values.len() != 3 {
        return Err("expected host-package path, CA DER path, and connection-count");
    }
    let mut encoded = fs::read(&values[0]).map_err(|_| "package unavailable")?;
    let package: Result<HostRoutePackage, _> = serde_json::from_slice(&encoded);
    encoded.zeroize();
    let mut package = package.map_err(|_| "package invalid")?;
    if package.schema != "termirust-relay-route"
        || package.schema_version != 1
        || package.role != "host"
    {
        package.admission_credential.zeroize();
        return Err("package invalid");
    }
    let credential = Zeroizing::new(std::mem::take(&mut package.admission_credential));
    let pin = decode_fixed::<32>(
        package
            .spki_pin
            .strip_prefix("sha256/")
            .ok_or("invalid pin")?,
    )?;
    let route = decode_fixed::<32>(&package.relay_route_id)?;
    let secret = decode_fixed::<32>(&credential)?;
    let expected_bytes = values[2]
        .parse::<usize>()
        .map_err(|_| "invalid expected byte count")?;
    let tls = test_tls(&values[1])?;
    let credential = RelayAdmissionCredential::from_secret_bytes(secret);
    let endpoint = RelayEndpointConfig::new_host(
        RelayEndpointId::new("relay-echo-host").map_err(|_| "invalid config")?,
        RelayWssUrl::parse(&package.endpoint).map_err(|_| "invalid endpoint")?,
        RelayRouteId(route),
        RelayCredentialRef::new("relay-echo-host").map_err(|_| "invalid config")?,
        RelaySpkiPin(pin),
        RelayRevocationEpoch(package.relay_revocation_epoch),
    )
    .map_err(|_| "invalid config")?;

    let mut observed = 0;
    while observed < expected_bytes {
        let mut socket = connect_with_retry(&endpoint, &credential, tls.clone()).await?;
        loop {
            let payload = match socket.receive().await {
                Ok(payload) => payload,
                Err(_) => break,
            };
            observed = observed
                .checked_add(payload.len())
                .ok_or("byte count overflow")?;
            eprintln!("relay echo host observed {observed}/{expected_bytes} bytes");
            socket.send(payload).await.map_err(|_| "write failed")?;
        }
        socket.close().await;
    }
    Ok(())
}

async fn connect_with_retry(
    endpoint: &RelayEndpointConfig,
    credential: &RelayAdmissionCredential,
    tls: RelayTlsClientConfig,
) -> Result<RelaySocket, &'static str> {
    for attempt in 0..50 {
        match RelaySocket::connect_with_tls(
            endpoint,
            RelayClientRole::Host,
            credential,
            tls.clone(),
        )
        .await
        {
            Ok(relay) => return Ok(relay),
            Err(_) if attempt < 49 => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await
            }
            Err(_) => return Err("connect failed"),
        }
    }
    Err("connect failed")
}

fn test_tls(path: &str) -> Result<RelayTlsClientConfig, &'static str> {
    let certificate = fs::read(path).map_err(|_| "test CA unavailable")?;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(certificate))
        .map_err(|_| "test CA invalid")?;
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| "TLS setup failed")?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(RelayTlsClientConfig::from_rustls(config))
}

fn decode_fixed<const N: usize>(value: &str) -> Result<[u8; N], &'static str> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| "invalid Base64")?
        .try_into()
        .map_err(|_| "wrong decoded length")
}
