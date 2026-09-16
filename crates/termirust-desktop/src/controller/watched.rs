//! Computers this Mac may connect to, as their device.
//!
//! Everything else under `controller/` is this machine as a host: it shows a code, a phone pairs,
//! and the phone becomes a device of this Mac. This is the other half. It enters the code another
//! computer shows, keeps the device identity that pairing produced, and lists what this Mac may
//! watch. Without it the desktop viewer has nothing to connect to.
//!
//! The record and the key are kept apart, the way the CLI's SSH controller profiles are: the
//! record is a small JSON file in the app's data directory, and the device's private key lives in
//! the system credential store, never on disk beside it.

use std::fs;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use termirust_controller_listener::{
    ControllerConnectionPurpose, HandshakeEntropy as _, SystemHandshakeEntropy,
    pair_controller_with_code_client,
};
use termirust_controller_security::{
    CapabilitySet, HostStaticPublicKey, PairingCode, StaticPrivateKey,
};
use termirust_domain::ControllerDeviceId;
use zeroize::Zeroize as _;

const SCHEMA_VERSION: u16 = 1;
const SECRET_SERVICE: &str = "com.termirust.controller.client";
const DIRECTORY: &str = "watched-computers";
const MAX_RECORD_BYTES: u64 = 16 * 1024;
const PAIRING_TIMEOUT: Duration = Duration::from_secs(30);

/// A computer this Mac has paired with, as its device.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WatchedComputer {
    pub schema_version: u16,
    /// The address pairing succeeded on, and where a connection is tried first.
    pub address: String,
    /// What to call it in the interface. The person types this; the computer does not send it.
    pub display_name: String,
    pub host_public_key: [u8; 32],
    pub identity_generation: u64,
    pub revocation_epoch: u64,
    pub session_generation: u64,
    pub capability_bits: u16,
    /// Where this computer's device private key lives in the credential store.
    pub secret_ref: String,
    pub paired_at_unix_seconds: u64,
}

impl WatchedComputer {
    /// Whether this computer granted the capability to watch its screen.
    pub fn may_watch_screen(&self) -> bool {
        self.capability_bits & (1 << 5) != 0
    }

    pub fn capabilities(&self) -> Option<CapabilitySet> {
        CapabilitySet::from_bits(self.capability_bits).ok()
    }

    pub fn host_key(&self) -> HostStaticPublicKey {
        HostStaticPublicKey(self.host_public_key)
    }

    /// A stable file name, so one record per computer address.
    fn key(address: &str) -> String {
        address
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() {
                    char::from(byte)
                } else {
                    '-'
                }
            })
            .collect()
    }

    fn validate(&self) -> bool {
        self.schema_version == SCHEMA_VERSION
            && self.identity_generation != 0
            && self.session_generation != 0
            && self.host_public_key != [0; 32]
            && self.secret_ref == format!("controller.client.{}", Self::key(&self.address))
            && self.capabilities().is_some()
    }
}

#[derive(Debug)]
pub enum WatchedComputerError {
    /// The address is not one this Mac will connect to.
    Address,
    /// The code is not six digits, or the computer refused it.
    Code,
    /// The computer could not be reached.
    Unreachable,
    /// The record or its key could not be kept, so the pairing was thrown away.
    Storage,
    /// This Mac is already paired with that computer.
    AlreadyPaired,
}

impl std::fmt::Display for WatchedComputerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Address => "Enter the address the other computer shows, as host:port.",
            Self::Code => "That code was not accepted. Ask for a new one and try again.",
            Self::Unreachable => "That computer did not answer on this network.",
            Self::Storage => "The pairing could not be saved, so it was discarded.",
            Self::AlreadyPaired => "This Mac is already paired with that computer.",
        })
    }
}

impl std::error::Error for WatchedComputerError {}

/// The computers this Mac may connect to, kept as one small file each.
#[derive(Clone, Debug)]
pub struct WatchedComputers {
    directory: PathBuf,
}

impl WatchedComputers {
    pub fn new(controller_root: &Path) -> Self {
        Self {
            directory: controller_root.join(DIRECTORY),
        }
    }

    /// Every computer this Mac may connect to, newest pairing last.
    pub fn load(&self) -> Vec<WatchedComputer> {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return Vec::new();
        };
        let mut computers: Vec<WatchedComputer> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let metadata = fs::metadata(&path).ok()?;
                if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_RECORD_BYTES {
                    return None;
                }
                let bytes = fs::read(&path).ok()?;
                let computer: WatchedComputer = serde_json::from_slice(&bytes).ok()?;
                computer.validate().then_some(computer)
            })
            .collect();
        computers.sort_by_key(|computer| computer.paired_at_unix_seconds);
        computers
    }

    /// Reads the private key this Mac uses with `computer`.
    pub fn private_key(&self, computer: &WatchedComputer) -> Option<StaticPrivateKey> {
        let entry = keyring::Entry::new(SECRET_SERVICE, &computer.secret_ref).ok()?;
        let encoded = entry.get_password().ok()?;
        let mut decoded = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(encoded)
            .ok()?;
        if decoded.len() != 32 || decoded.iter().all(|byte| *byte == 0) {
            decoded.zeroize();
            return None;
        }
        let mut bytes = [0; 32];
        bytes.copy_from_slice(&decoded);
        decoded.zeroize();
        let private = StaticPrivateKey::from_bytes(bytes);
        bytes.zeroize();
        Some(private)
    }

    /// Forgets a computer: the record and the key both go.
    pub fn forget(&self, address: &str) -> bool {
        let key = WatchedComputer::key(address);
        if let Ok(entry) = keyring::Entry::new(SECRET_SERVICE, &format!("controller.client.{key}"))
        {
            let _ = entry.delete_credential();
        }
        fs::remove_file(self.directory.join(format!("{key}.json"))).is_ok()
    }

    /// Pairs with the computer at `address` using the six-digit code it is showing.
    ///
    /// This blocks: the caller runs it off the interface thread. The record and the key are
    /// written before the pairing is acknowledged, so a pairing this Mac cannot keep is refused
    /// rather than silently lost.
    pub fn pair(
        &self,
        address: &str,
        code: &str,
        display_name: &str,
        device_name: &str,
    ) -> Result<WatchedComputer, WatchedComputerError> {
        let endpoint: SocketAddr = address
            .trim()
            .parse()
            .map_err(|_| WatchedComputerError::Address)?;
        let code = PairingCode::parse(code.trim()).map_err(|_| WatchedComputerError::Code)?;
        let key = WatchedComputer::key(address.trim());
        if self.directory.join(format!("{key}.json")).exists() {
            return Err(WatchedComputerError::AlreadyPaired);
        }

        // The listener's entropy is the same source the handshake itself uses.
        let mut entropy = SystemHandshakeEntropy;
        let mut seed = entropy.nonce().map_err(|_| WatchedComputerError::Storage)?;
        let device_private = StaticPrivateKey::from_bytes(seed);
        let mut ephemeral = entropy.nonce().map_err(|_| WatchedComputerError::Storage)?;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| WatchedComputerError::Storage)?;
        let directory = self.directory.clone();
        let address = address.trim().to_owned();
        let display_name = display_name.trim().to_owned();
        let device_name = device_name.trim().to_owned();

        let owned_key = key.clone();
        let result = runtime.block_on(async move {
            let mut stream =
                tokio::time::timeout(PAIRING_TIMEOUT, tokio::net::TcpStream::connect(endpoint))
                    .await
                    .map_err(|_| WatchedComputerError::Unreachable)?
                    .map_err(|_| WatchedComputerError::Unreachable)?;
            ControllerConnectionPurpose::PairCode
                .write_to(&mut stream)
                .await
                .map_err(|_| WatchedComputerError::Unreachable)?;
            pair_controller_with_code_client(
                &mut stream,
                &code,
                device_private,
                StaticPrivateKey::from_bytes(ephemeral),
                &mut entropy,
                ControllerDeviceId::new(),
                device_name,
                |result| {
                    // Commit before the acknowledgement: a pairing this Mac cannot keep is
                    // refused on the spot rather than becoming a device the other computer
                    // trusts and this one has forgotten.
                    let computer = WatchedComputer {
                        schema_version: SCHEMA_VERSION,
                        address: address.clone(),
                        display_name: if display_name.is_empty() {
                            address.clone()
                        } else {
                            display_name.clone()
                        },
                        host_public_key: result.host_public_key.0,
                        identity_generation: result.identity_generation,
                        revocation_epoch: result.revocation_epoch,
                        session_generation: result.session_generation,
                        capability_bits: result.capability_bits,
                        secret_ref: format!("controller.client.{owned_key}"),
                        paired_at_unix_seconds: unix_seconds(),
                    };
                    save(&directory, &owned_key, &computer, &seed).map_err(|_| {
                        termirust_controller_listener::ListenerError::new(
                            termirust_controller_listener::ListenerErrorCode::Io,
                        )
                    })
                },
            )
            .await
            .map_err(|_| WatchedComputerError::Code)
        });
        seed.zeroize();
        ephemeral.zeroize();
        result?;

        self.load()
            .into_iter()
            .find(|computer| computer.secret_ref.ends_with(&key))
            .ok_or(WatchedComputerError::Storage)
    }
}

/// Writes the record, then the key. A record without its key is useless, so the key goes in
/// first and the record is what makes the pairing visible.
fn save(
    directory: &Path,
    key: &str,
    computer: &WatchedComputer,
    seed: &[u8; 32],
) -> Result<(), ()> {
    fs::create_dir_all(directory).map_err(|_| ())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = fs::set_permissions(directory, fs::Permissions::from_mode(0o700));
    }
    let entry = keyring::Entry::new(SECRET_SERVICE, &computer.secret_ref).map_err(|_| ())?;
    let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(seed);
    entry.set_password(&encoded).map_err(|_| ())?;

    let bytes = serde_json::to_vec_pretty(computer).map_err(|_| ())?;
    let path = directory.join(format!("{key}.json"));
    let temporary = directory.join(format!("{key}.json.tmp"));
    let mut file = fs::File::create(&temporary).map_err(|_| ())?;
    file.write_all(&bytes).map_err(|_| ())?;
    file.sync_all().map_err(|_| ())?;
    drop(file);
    fs::rename(&temporary, &path).map_err(|_| ())?;
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(address: &str) -> WatchedComputer {
        WatchedComputer {
            schema_version: SCHEMA_VERSION,
            address: address.to_owned(),
            display_name: "Studio".to_owned(),
            host_public_key: [7; 32],
            identity_generation: 1,
            revocation_epoch: 1,
            session_generation: 1,
            capability_bits: 0x00e3,
            secret_ref: format!("controller.client.{}", WatchedComputer::key(address)),
            paired_at_unix_seconds: 10,
        }
    }

    #[test]
    fn a_record_names_its_own_key_or_it_is_not_this_mac_s() {
        let mut computer = record("192.168.1.10:63322");
        assert!(computer.validate());
        computer.secret_ref = "controller.client.someone-else".to_owned();
        assert!(
            !computer.validate(),
            "a record may not point at another key"
        );
    }

    #[test]
    fn a_record_without_a_generation_is_refused() {
        let mut computer = record("192.168.1.10:63322");
        computer.session_generation = 0;
        assert!(!computer.validate());
        let mut computer = record("192.168.1.10:63322");
        computer.identity_generation = 0;
        assert!(!computer.validate());
    }

    #[test]
    fn a_record_carrying_a_capability_this_build_does_not_know_is_refused() {
        let mut computer = record("192.168.1.10:63322");
        computer.capability_bits = 0xf000;
        assert!(!computer.validate(), "unknown bits fail closed");
    }

    #[test]
    fn watching_needs_the_capability_the_other_computer_granted() {
        let mut computer = record("192.168.1.10:63322");
        assert!(computer.may_watch_screen());
        computer.capability_bits = 0x0003;
        assert!(!computer.may_watch_screen());
    }

    #[test]
    fn one_file_per_computer_whatever_its_address_looks_like() {
        assert_eq!(
            WatchedComputer::key("192.168.1.10:63322"),
            "192-168-1-10-63322"
        );
        assert_eq!(
            WatchedComputer::key("[fd7a::1]:63322"),
            "-fd7a--1--63322",
            "an address is never a path"
        );
    }

    #[test]
    fn records_that_are_not_this_mac_s_are_skipped_rather_than_failing_the_list() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let computers = WatchedComputers::new(directory.path());
        let root = directory.path().join(DIRECTORY);
        fs::create_dir_all(&root).expect("the directory");
        fs::write(root.join("broken.json"), b"not json").expect("a broken record");
        let good = record("192.168.1.10:63322");
        fs::write(
            root.join(format!("{}.json", WatchedComputer::key(&good.address))),
            serde_json::to_vec(&good).expect("json"),
        )
        .expect("a good record");
        let loaded = computers.load();
        assert_eq!(loaded.len(), 1, "the broken one was skipped");
        assert_eq!(loaded[0].address, good.address);
    }
}
