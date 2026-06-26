use aes_gcm_siv::aead::{Aead, Payload};
use aes_gcm_siv::{Aes256GcmSiv, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub const MOBILE_VAULT_SCHEMA_VERSION: u16 = 1;
pub const ENCRYPTED_MOBILE_VAULT_VERSION: u16 = 1;

const MOBILE_VAULT_AAD: &[u8] = b"termirust.mobile-vault.v1";
const MOBILE_VAULT_CIPHER: &str = "AES-256-GCM-SIV";
const MOBILE_VAULT_KDF: &str = "Argon2id(m=19456,t=3,p=1)";
const MOBILE_VAULT_KEY_LEN: usize = 32;
const MOBILE_VAULT_SALT_LEN: usize = 16;
const MOBILE_VAULT_NONCE_LEN: usize = 12;
const MOBILE_VAULT_ARGON2_MEMORY_KIB: u32 = 19_456;
const MOBILE_VAULT_ARGON2_ITERATIONS: u32 = 3;
const MOBILE_VAULT_ARGON2_PARALLELISM: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedMobileVaultEnvelope {
    pub version: u16,
    pub schema_version: u16,
    pub cipher: String,
    pub kdf: String,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MobileVaultCryptoError {
    EmptyPassphrase,
    UnsupportedEnvelopeVersion(u16),
    UnsupportedSchemaVersion(u16),
    UnsupportedCipher(String),
    UnsupportedKdf(String),
    InvalidSalt,
    InvalidNonce,
    InvalidCiphertext,
    InvalidKdfParams,
    InvalidKey,
    EncryptFailed,
    DecryptFailed,
    EncodeFailed(String),
    DecodeFailed(String),
}

impl fmt::Display for MobileVaultCryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPassphrase => write!(f, "Mobile vault passphrase cannot be empty."),
            Self::UnsupportedEnvelopeVersion(version) => {
                write!(f, "Unsupported encrypted mobile vault version {version}.")
            }
            Self::UnsupportedSchemaVersion(version) => {
                write!(f, "Unsupported mobile vault schema version {version}.")
            }
            Self::UnsupportedCipher(cipher) => {
                write!(f, "Unsupported encrypted mobile vault cipher {cipher}.")
            }
            Self::UnsupportedKdf(kdf) => {
                write!(f, "Unsupported encrypted mobile vault KDF {kdf}.")
            }
            Self::InvalidSalt => write!(f, "Encrypted mobile vault salt is invalid."),
            Self::InvalidNonce => write!(f, "Encrypted mobile vault nonce is invalid."),
            Self::InvalidCiphertext => write!(f, "Encrypted mobile vault ciphertext is invalid."),
            Self::InvalidKdfParams => {
                write!(f, "Encrypted mobile vault KDF parameters are invalid.")
            }
            Self::InvalidKey => write!(f, "Unable to initialize mobile vault cipher."),
            Self::EncryptFailed => write!(f, "Unable to encrypt mobile vault."),
            Self::DecryptFailed => write!(
                f,
                "Unable to decrypt encrypted mobile vault. Check the passphrase and file integrity."
            ),
            Self::EncodeFailed(error) => write!(f, "Unable to encode mobile vault: {error}"),
            Self::DecodeFailed(error) => write!(f, "Unable to decode mobile vault: {error}"),
        }
    }
}

impl std::error::Error for MobileVaultCryptoError {}

pub fn encrypt_mobile_vault_export(
    export: &MobileVaultExport,
    passphrase: &str,
) -> Result<EncryptedMobileVaultEnvelope, MobileVaultCryptoError> {
    if passphrase.trim().is_empty() {
        return Err(MobileVaultCryptoError::EmptyPassphrase);
    }
    let plaintext = serde_json::to_vec_pretty(export)
        .map_err(|error| MobileVaultCryptoError::EncodeFailed(error.to_string()))?;
    encrypt_mobile_vault_bytes(&plaintext, passphrase)
}

pub fn encrypt_mobile_vault_export_json(
    export: &MobileVaultExport,
    passphrase: &str,
) -> Result<String, MobileVaultCryptoError> {
    let envelope = encrypt_mobile_vault_export(export, passphrase)?;
    serde_json::to_string_pretty(&envelope)
        .map_err(|error| MobileVaultCryptoError::EncodeFailed(error.to_string()))
}

pub fn decrypt_mobile_vault_export(
    envelope: &EncryptedMobileVaultEnvelope,
    passphrase: &str,
) -> Result<MobileVaultExport, MobileVaultCryptoError> {
    let plaintext = decrypt_mobile_vault_bytes(envelope, passphrase)?;
    serde_json::from_slice(&plaintext)
        .map_err(|error| MobileVaultCryptoError::DecodeFailed(error.to_string()))
}

pub fn decrypt_mobile_vault_export_json(
    envelope_json: &str,
    passphrase: &str,
) -> Result<MobileVaultExport, MobileVaultCryptoError> {
    let envelope: EncryptedMobileVaultEnvelope = serde_json::from_str(envelope_json)
        .map_err(|error| MobileVaultCryptoError::DecodeFailed(error.to_string()))?;
    decrypt_mobile_vault_export(&envelope, passphrase)
}

pub fn decrypt_mobile_vault_export_to_json(
    envelope_json: &str,
    passphrase: &str,
) -> Result<String, MobileVaultCryptoError> {
    let export = decrypt_mobile_vault_export_json(envelope_json, passphrase)?;
    serde_json::to_string_pretty(&export)
        .map_err(|error| MobileVaultCryptoError::EncodeFailed(error.to_string()))
}

pub fn encrypt_mobile_vault_bytes(
    plaintext: &[u8],
    passphrase: &str,
) -> Result<EncryptedMobileVaultEnvelope, MobileVaultCryptoError> {
    if passphrase.trim().is_empty() {
        return Err(MobileVaultCryptoError::EmptyPassphrase);
    }

    let mut salt = [0u8; MOBILE_VAULT_SALT_LEN];
    let mut nonce = [0u8; MOBILE_VAULT_NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);

    let key = derive_mobile_vault_key(passphrase, &salt)?;
    let cipher =
        Aes256GcmSiv::new_from_slice(&key).map_err(|_| MobileVaultCryptoError::InvalidKey)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: MOBILE_VAULT_AAD,
            },
        )
        .map_err(|_| MobileVaultCryptoError::EncryptFailed)?;

    Ok(EncryptedMobileVaultEnvelope {
        version: ENCRYPTED_MOBILE_VAULT_VERSION,
        schema_version: MOBILE_VAULT_SCHEMA_VERSION,
        cipher: MOBILE_VAULT_CIPHER.to_string(),
        kdf: MOBILE_VAULT_KDF.to_string(),
        salt: STANDARD_NO_PAD.encode(salt),
        nonce: STANDARD_NO_PAD.encode(nonce),
        ciphertext: STANDARD_NO_PAD.encode(ciphertext),
    })
}

pub fn decrypt_mobile_vault_bytes(
    envelope: &EncryptedMobileVaultEnvelope,
    passphrase: &str,
) -> Result<Vec<u8>, MobileVaultCryptoError> {
    if passphrase.trim().is_empty() {
        return Err(MobileVaultCryptoError::EmptyPassphrase);
    }
    if envelope.version != ENCRYPTED_MOBILE_VAULT_VERSION {
        return Err(MobileVaultCryptoError::UnsupportedEnvelopeVersion(
            envelope.version,
        ));
    }
    if envelope.schema_version != MOBILE_VAULT_SCHEMA_VERSION {
        return Err(MobileVaultCryptoError::UnsupportedSchemaVersion(
            envelope.schema_version,
        ));
    }
    if envelope.cipher != MOBILE_VAULT_CIPHER {
        return Err(MobileVaultCryptoError::UnsupportedCipher(
            envelope.cipher.clone(),
        ));
    }
    if envelope.kdf != MOBILE_VAULT_KDF {
        return Err(MobileVaultCryptoError::UnsupportedKdf(envelope.kdf.clone()));
    }

    let salt = STANDARD_NO_PAD
        .decode(&envelope.salt)
        .map_err(|_| MobileVaultCryptoError::InvalidSalt)?;
    let nonce = STANDARD_NO_PAD
        .decode(&envelope.nonce)
        .map_err(|_| MobileVaultCryptoError::InvalidNonce)?;
    let ciphertext = STANDARD_NO_PAD
        .decode(&envelope.ciphertext)
        .map_err(|_| MobileVaultCryptoError::InvalidCiphertext)?;

    if salt.len() != MOBILE_VAULT_SALT_LEN {
        return Err(MobileVaultCryptoError::InvalidSalt);
    }
    if nonce.len() != MOBILE_VAULT_NONCE_LEN {
        return Err(MobileVaultCryptoError::InvalidNonce);
    }

    let key = derive_mobile_vault_key(passphrase, &salt)?;
    let cipher =
        Aes256GcmSiv::new_from_slice(&key).map_err(|_| MobileVaultCryptoError::InvalidKey)?;
    cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: MOBILE_VAULT_AAD,
            },
        )
        .map_err(|_| MobileVaultCryptoError::DecryptFailed)
}

fn derive_mobile_vault_key(
    passphrase: &str,
    salt: &[u8],
) -> Result<[u8; MOBILE_VAULT_KEY_LEN], MobileVaultCryptoError> {
    let params = Params::new(
        MOBILE_VAULT_ARGON2_MEMORY_KIB,
        MOBILE_VAULT_ARGON2_ITERATIONS,
        MOBILE_VAULT_ARGON2_PARALLELISM,
        Some(MOBILE_VAULT_KEY_LEN),
    )
    .map_err(|_| MobileVaultCryptoError::InvalidKdfParams)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; MOBILE_VAULT_KEY_LEN];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| MobileVaultCryptoError::InvalidKdfParams)?;
    Ok(key)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileVaultExport {
    pub schema_version: u16,
    pub export_id: String,
    pub created_at_millis: u128,
    pub updated_at_millis: u128,
    pub source_device_id: String,
    #[serde(default)]
    pub vaults: Vec<MobileVault>,
    #[serde(default)]
    pub hosts: Vec<MobileHost>,
    #[serde(default)]
    pub groups: Vec<MobileGroup>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub identities: Vec<MobileIdentityMetadata>,
    #[serde(default)]
    pub known_hosts: Vec<MobileKnownHost>,
    #[serde(default)]
    pub sync: MobileSyncMetadata,
    #[serde(default)]
    pub devices: Vec<MobileDeviceRecord>,
    #[serde(default)]
    pub device_keys: Vec<MobileDeviceVaultKey>,
}

impl MobileVaultExport {
    pub fn empty(export_id: impl Into<String>, source_device_id: impl Into<String>) -> Self {
        Self {
            schema_version: MOBILE_VAULT_SCHEMA_VERSION,
            export_id: export_id.into(),
            created_at_millis: 0,
            updated_at_millis: 0,
            source_device_id: source_device_id.into(),
            vaults: Vec::new(),
            hosts: Vec::new(),
            groups: Vec::new(),
            tags: Vec::new(),
            identities: Vec::new(),
            known_hosts: Vec::new(),
            sync: MobileSyncMetadata::default(),
            devices: Vec::new(),
            device_keys: Vec::new(),
        }
    }

    pub fn is_device_revoked(&self, device_id: &str) -> bool {
        let device_id = device_id.trim();
        !device_id.is_empty()
            && self
                .devices
                .iter()
                .any(|device| device.device_id == device_id && device.revoked_at_millis.is_some())
    }

    pub fn source_device_record(&self) -> Option<&MobileDeviceRecord> {
        self.devices
            .iter()
            .find(|device| device.device_id == self.source_device_id)
    }

    pub fn active_devices(&self) -> impl Iterator<Item = &MobileDeviceRecord> {
        self.devices.iter().filter(|device| !device.is_revoked())
    }

    pub fn active_device_key(&self, device_id: &str) -> Option<&MobileDeviceVaultKey> {
        self.device_keys.iter().find(|key| {
            key.device_id == device_id
                && key.revoked_at_millis.is_none()
                && !self.is_device_revoked(device_id)
        })
    }

    pub fn from_desktop_portable_json(
        input: &str,
        export_id: impl Into<String>,
        source_device_id: impl Into<String>,
    ) -> Result<Self, serde_json::Error> {
        let desktop: DesktopPortableBundle = serde_json::from_str(input)?;
        Ok(Self::from_desktop_portable_bundle(
            desktop,
            export_id,
            source_device_id,
        ))
    }

    fn from_desktop_portable_bundle(
        desktop: DesktopPortableBundle,
        export_id: impl Into<String>,
        source_device_id: impl Into<String>,
    ) -> Self {
        let mut groups = Vec::new();
        let mut tag_set = BTreeMap::<String, ()>::new();
        let hosts = desktop
            .profiles
            .into_iter()
            .map(|profile| {
                if !profile.group.trim().is_empty()
                    && !groups
                        .iter()
                        .any(|group: &MobileGroup| group.name == profile.group)
                {
                    groups.push(MobileGroup {
                        id: profile.group.clone(),
                        name: profile.group.clone(),
                    });
                }
                for tag in &profile.tags {
                    tag_set.insert(tag.clone(), ());
                }
                profile.into_mobile_host()
            })
            .collect();

        Self {
            schema_version: MOBILE_VAULT_SCHEMA_VERSION,
            export_id: export_id.into(),
            created_at_millis: desktop.exported_at_millis,
            updated_at_millis: desktop.exported_at_millis,
            source_device_id: source_device_id.into(),
            vaults: desktop
                .vaults
                .into_iter()
                .map(DesktopVault::into_mobile_vault)
                .collect(),
            hosts,
            groups,
            tags: tag_set.into_keys().collect(),
            identities: desktop
                .identities
                .into_iter()
                .map(DesktopIdentity::into_mobile_identity)
                .collect(),
            known_hosts: desktop
                .known_hosts
                .into_iter()
                .map(|(endpoint, public_key)| MobileKnownHost {
                    endpoint,
                    public_key,
                    algorithm: None,
                    fingerprint: None,
                })
                .collect(),
            sync: MobileSyncMetadata::default(),
            devices: Vec::new(),
            device_keys: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileVault {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileHost {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vault_id: Option<String>,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub auth: MobileAuthMetadata,
    #[serde(default)]
    pub jump_host_id: Option<String>,
    #[serde(default)]
    pub startup_directory: Option<String>,
    #[serde(default)]
    pub startup_command: Option<String>,
    #[serde(default)]
    pub start_in_files: bool,
    #[serde(default)]
    pub persistent_session: MobilePersistentSession,
    #[serde(default)]
    pub terminal_scrollback_rows: Option<u32>,
    #[serde(default)]
    pub color_tag: Option<String>,
    #[serde(default)]
    pub environment: Vec<MobileEnvironmentVariable>,
    #[serde(default)]
    pub known_host_endpoint: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobilePersistentSession {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default)]
    pub detach_others: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileAuthMetadata {
    #[serde(default)]
    pub kind: MobileAuthKind,
    #[serde(default)]
    pub identity_id: Option<String>,
    #[serde(default)]
    pub secret_ref: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileAuthKind {
    #[default]
    Password,
    PrivateKey,
}

pub fn mobile_secret_ref_for_host(
    host_id: &str,
    auth_kind: MobileAuthKind,
    identity_id: Option<&str>,
) -> String {
    match auth_kind {
        MobileAuthKind::Password => {
            format!(
                "termirust-mobile://host/{}/password",
                mobile_ref_slug(host_id)
            )
        }
        MobileAuthKind::PrivateKey => {
            if let Some(identity_id) = identity_id.and_then(non_empty_trimmed) {
                format!(
                    "termirust-mobile://identity/{}/private-key",
                    mobile_ref_slug(identity_id)
                )
            } else {
                format!(
                    "termirust-mobile://host/{}/private-key",
                    mobile_ref_slug(host_id)
                )
            }
        }
    }
}

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() { None } else { Some(value) }
}

fn mobile_ref_slug(value: &str) -> String {
    let slug: String = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if slug.is_empty() {
        "unknown".to_string()
    } else {
        slug
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileEnvironmentVariable {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileIdentityMetadata {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub vault_id: Option<String>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub public_key: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub secret_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileKnownHost {
    pub endpoint: String,
    pub public_key: String,
    #[serde(default)]
    pub algorithm: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileGroup {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileSyncMetadata {
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub previous_revision_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileDeviceRecord {
    pub device_id: String,
    pub label: String,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub public_key: Option<String>,
    #[serde(default)]
    pub paired_at_millis: Option<u128>,
    #[serde(default)]
    pub last_seen_at_millis: Option<u128>,
    #[serde(default)]
    pub revoked_at_millis: Option<u128>,
}

impl MobileDeviceRecord {
    pub fn active_desktop(device_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            label: label.into(),
            platform: Some("desktop".to_string()),
            public_key: None,
            paired_at_millis: None,
            last_seen_at_millis: None,
            revoked_at_millis: None,
        }
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked_at_millis.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileDeviceVaultKey {
    pub key_id: String,
    pub device_id: String,
    pub wrapping_algorithm: String,
    pub encrypted_vault_key: String,
    #[serde(default)]
    pub created_at_millis: Option<u128>,
    #[serde(default)]
    pub revoked_at_millis: Option<u128>,
}

fn default_ssh_port() -> u16 {
    22
}

#[derive(Clone, Debug, Deserialize)]
struct DesktopPortableBundle {
    #[allow(dead_code)]
    version: u16,
    exported_at_millis: u128,
    #[serde(default)]
    vaults: Vec<DesktopVault>,
    #[serde(default)]
    profiles: Vec<DesktopHostProfile>,
    #[serde(default)]
    identities: Vec<DesktopIdentity>,
    #[serde(default)]
    known_hosts: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
struct DesktopVault {
    id: String,
    label: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    kind: String,
}

impl DesktopVault {
    fn into_mobile_vault(self) -> MobileVault {
        MobileVault {
            id: self.id,
            label: self.label,
            description: self.description,
            kind: self.kind,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct DesktopHostProfile {
    id: String,
    label: String,
    #[serde(default)]
    vault_id: Option<String>,
    #[serde(default)]
    group: String,
    #[serde(default)]
    tags: Vec<String>,
    host: String,
    #[serde(default = "default_ssh_port")]
    port: u16,
    username: String,
    #[serde(default)]
    auth_mode: MobileAuthKind,
    #[serde(default)]
    identity_id: Option<String>,
    #[serde(default)]
    jump_host_id: Option<String>,
    #[serde(default)]
    startup_directory: Option<String>,
    #[serde(default)]
    startup_command: Option<String>,
    #[serde(default)]
    start_in_files: bool,
    #[serde(default)]
    persistent_session: bool,
    #[serde(default)]
    persistent_session_name: Option<String>,
    #[serde(default)]
    persistent_session_detach_others: bool,
    #[serde(default)]
    terminal_scrollback_rows: Option<u32>,
    #[serde(default)]
    color_tag: Option<String>,
    #[serde(default)]
    environment: Vec<(String, String)>,
}

impl DesktopHostProfile {
    fn into_mobile_host(self) -> MobileHost {
        let known_host_endpoint = Some(format!("{}:{}", self.host, self.port));
        let auth_kind = self.auth_mode;
        let secret_ref =
            mobile_secret_ref_for_host(&self.id, auth_kind, self.identity_id.as_deref());
        MobileHost {
            id: self.id,
            label: self.label,
            vault_id: self.vault_id,
            group: self.group,
            tags: self.tags,
            host: self.host,
            port: self.port,
            username: self.username,
            auth: MobileAuthMetadata {
                kind: auth_kind,
                identity_id: self.identity_id,
                secret_ref: Some(secret_ref),
            },
            jump_host_id: self.jump_host_id,
            startup_directory: self.startup_directory,
            startup_command: self.startup_command,
            start_in_files: self.start_in_files,
            persistent_session: MobilePersistentSession {
                enabled: self.persistent_session,
                session_name: self.persistent_session_name,
                detach_others: self.persistent_session_detach_others,
            },
            terminal_scrollback_rows: self.terminal_scrollback_rows,
            color_tag: self.color_tag,
            environment: self
                .environment
                .into_iter()
                .map(|(name, value)| MobileEnvironmentVariable { name, value })
                .collect(),
            known_host_endpoint,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct DesktopIdentity {
    id: String,
    label: String,
    #[serde(default)]
    vault_id: Option<String>,
    #[serde(default)]
    kind: String,
}

impl DesktopIdentity {
    fn into_mobile_identity(self) -> MobileIdentityMetadata {
        MobileIdentityMetadata {
            id: self.id,
            label: self.label,
            vault_id: self.vault_id,
            kind: self.kind,
            public_key: None,
            fingerprint: None,
            secret_ref: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_vault_schema_round_trips() {
        let export = MobileVaultExport {
            schema_version: MOBILE_VAULT_SCHEMA_VERSION,
            export_id: "export-1".to_string(),
            created_at_millis: 100,
            updated_at_millis: 101,
            source_device_id: "desktop-1".to_string(),
            vaults: vec![MobileVault {
                id: "vault-1".to_string(),
                label: "Ops".to_string(),
                description: String::new(),
                kind: "shared".to_string(),
            }],
            hosts: vec![MobileHost {
                id: "profile-1".to_string(),
                label: "Production".to_string(),
                vault_id: Some("vault-1".to_string()),
                group: "Ops".to_string(),
                tags: vec!["prod".to_string()],
                host: "prod.example.com".to_string(),
                port: 22,
                username: "deploy".to_string(),
                auth: MobileAuthMetadata {
                    kind: MobileAuthKind::PrivateKey,
                    identity_id: Some("identity-1".to_string()),
                    secret_ref: Some("keychain://identity-1".to_string()),
                },
                jump_host_id: None,
                startup_directory: Some("/srv/app".to_string()),
                startup_command: Some("uptime".to_string()),
                start_in_files: false,
                persistent_session: MobilePersistentSession {
                    enabled: true,
                    session_name: Some("tr-prod".to_string()),
                    detach_others: false,
                },
                terminal_scrollback_rows: Some(10_000),
                color_tag: Some("green".to_string()),
                environment: vec![MobileEnvironmentVariable {
                    name: "APP_ENV".to_string(),
                    value: "prod".to_string(),
                }],
                known_host_endpoint: Some("prod.example.com:22".to_string()),
            }],
            groups: vec![MobileGroup {
                id: "Ops".to_string(),
                name: "Ops".to_string(),
            }],
            tags: vec!["prod".to_string()],
            identities: vec![MobileIdentityMetadata {
                id: "identity-1".to_string(),
                label: "Deploy key".to_string(),
                vault_id: Some("vault-1".to_string()),
                kind: "ed25519".to_string(),
                public_key: None,
                fingerprint: None,
                secret_ref: None,
            }],
            known_hosts: vec![MobileKnownHost {
                endpoint: "prod.example.com:22".to_string(),
                public_key: "ssh-ed25519 AAAA".to_string(),
                algorithm: Some("ssh-ed25519".to_string()),
                fingerprint: None,
            }],
            sync: MobileSyncMetadata {
                revision: 1,
                previous_revision_id: None,
            },
            devices: vec![MobileDeviceRecord {
                device_id: "ios-1".to_string(),
                label: "Jacob iPhone".to_string(),
                platform: Some("ios".to_string()),
                public_key: None,
                paired_at_millis: Some(100),
                last_seen_at_millis: Some(101),
                revoked_at_millis: None,
            }],
            device_keys: vec![MobileDeviceVaultKey {
                key_id: "vault-key-ios-1".to_string(),
                device_id: "ios-1".to_string(),
                wrapping_algorithm: "x25519-xsalsa20poly1305".to_string(),
                encrypted_vault_key: "base64-wrapped-key".to_string(),
                created_at_millis: Some(100),
                revoked_at_millis: None,
            }],
        };

        let json = serde_json::to_string_pretty(&export).expect("serialize mobile vault");
        let parsed: MobileVaultExport =
            serde_json::from_str(&json).expect("deserialize mobile vault");
        assert_eq!(parsed, export);
        assert_eq!(
            parsed
                .source_device_record()
                .map(|device| device.label.as_str()),
            None
        );
        assert!(!parsed.is_device_revoked("ios-1"));
        assert_eq!(parsed.active_devices().count(), 1);
        assert_eq!(
            parsed
                .active_device_key("ios-1")
                .map(|key| key.key_id.as_str()),
            Some("vault-key-ios-1")
        );
    }

    #[test]
    fn mobile_vault_detects_revoked_devices() {
        let mut export = MobileVaultExport::empty("export-1", "desktop-1");
        export.devices = vec![
            MobileDeviceRecord::active_desktop("desktop-1", "Desktop"),
            MobileDeviceRecord {
                device_id: "ios-1".to_string(),
                label: "Jacob iPhone".to_string(),
                platform: Some("ios".to_string()),
                public_key: None,
                paired_at_millis: None,
                last_seen_at_millis: None,
                revoked_at_millis: Some(1719356789123),
            },
        ];

        assert_eq!(
            export
                .source_device_record()
                .map(|device| device.label.as_str()),
            Some("Desktop")
        );
        assert!(export.is_device_revoked("ios-1"));
        assert!(!export.is_device_revoked("desktop-1"));
        assert!(!export.is_device_revoked(""));
        assert_eq!(export.active_devices().count(), 1);
    }

    #[test]
    fn mobile_schema_defaults_missing_collections() {
        let parsed: MobileVaultExport = serde_json::from_str(
            r#"{
              "schema_version": 1,
              "export_id": "export-1",
              "created_at_millis": 1,
              "updated_at_millis": 1,
              "source_device_id": "desktop-1"
            }"#,
        )
        .expect("deserialize minimal mobile vault");

        assert!(parsed.hosts.is_empty());
        assert!(parsed.known_hosts.is_empty());
        assert_eq!(parsed.sync, MobileSyncMetadata::default());
        assert!(parsed.devices.is_empty());
        assert!(parsed.device_keys.is_empty());
    }

    #[test]
    fn desktop_portable_bundle_converts_to_mobile_schema() {
        let mobile = MobileVaultExport::from_desktop_portable_json(
            r#"{
              "version": 1,
              "exported_at_millis": 1719356789123,
              "vaults": [{
                "id": "vault-ops",
                "label": "Ops",
                "description": "Operations vault",
                "kind": "shared"
              }],
              "profiles": [{
                "id": "profile-1",
                "label": "Production",
                "vault_id": "vault-ops",
                "group": "Ops",
                "tags": ["prod", "ssh"],
                "host": "prod.example.com",
                "port": 2222,
                "username": "deploy",
                "auth_mode": "private_key",
                "identity_id": "identity-1",
                "startup_directory": "/srv/app",
                "startup_command": "uptime",
                "persistent_session": true,
                "persistent_session_name": "tr-prod",
                "persistent_session_detach_others": true,
                "environment": [["APP_ENV", "prod"]]
              }],
              "identities": [{
                "id": "identity-1",
                "label": "Deploy key",
                "vault_id": "vault-ops",
                "key_path": "/Users/jacob/.ssh/id_ed25519",
                "kind": "ed25519"
              }],
              "known_hosts": {
                "prod.example.com:2222": "ssh-ed25519 AAAA"
              }
            }"#,
            "export-1",
            "desktop-1",
        )
        .expect("convert desktop bundle");

        assert_eq!(mobile.schema_version, MOBILE_VAULT_SCHEMA_VERSION);
        assert_eq!(mobile.created_at_millis, 1719356789123);
        assert_eq!(mobile.vaults[0].id, "vault-ops");
        assert_eq!(mobile.vaults[0].kind, "shared");
        assert_eq!(mobile.groups[0].name, "Ops");
        assert_eq!(mobile.tags, vec!["prod".to_string(), "ssh".to_string()]);
        assert_eq!(mobile.known_hosts[0].endpoint, "prod.example.com:2222");

        let host = &mobile.hosts[0];
        assert_eq!(host.auth.kind, MobileAuthKind::PrivateKey);
        assert_eq!(host.auth.identity_id.as_deref(), Some("identity-1"));
        assert_eq!(
            host.auth.secret_ref.as_deref(),
            Some("termirust-mobile://identity/identity-1/private-key")
        );
        assert!(host.persistent_session.enabled);
        assert_eq!(
            host.persistent_session.session_name.as_deref(),
            Some("tr-prod")
        );
        assert!(host.persistent_session.detach_others);
        assert_eq!(host.environment[0].name, "APP_ENV");

        let identity = &mobile.identities[0];
        assert_eq!(identity.kind, "ed25519");
        assert_eq!(identity.secret_ref, None);
    }

    #[test]
    fn mobile_secret_ref_uses_stable_non_secret_accounts() {
        assert_eq!(
            mobile_secret_ref_for_host("profile 1", MobileAuthKind::Password, None),
            "termirust-mobile://host/profile-1/password"
        );
        assert_eq!(
            mobile_secret_ref_for_host(
                "profile-1",
                MobileAuthKind::PrivateKey,
                Some("identity/ops key")
            ),
            "termirust-mobile://identity/identity-ops-key/private-key"
        );
        assert_eq!(
            mobile_secret_ref_for_host("", MobileAuthKind::PrivateKey, None),
            "termirust-mobile://host/unknown/private-key"
        );
    }

    #[test]
    fn encrypted_mobile_vault_round_trips_without_plaintext() {
        let export = MobileVaultExport {
            schema_version: MOBILE_VAULT_SCHEMA_VERSION,
            export_id: "export-1".to_string(),
            created_at_millis: 10,
            updated_at_millis: 11,
            source_device_id: "desktop-1".to_string(),
            vaults: Vec::new(),
            hosts: vec![MobileHost {
                id: "profile-1".to_string(),
                label: "Production".to_string(),
                vault_id: None,
                group: "Ops".to_string(),
                tags: Vec::new(),
                host: "prod.example.com".to_string(),
                port: 22,
                username: "deploy".to_string(),
                auth: MobileAuthMetadata {
                    kind: MobileAuthKind::PrivateKey,
                    identity_id: Some("identity-1".to_string()),
                    secret_ref: None,
                },
                jump_host_id: None,
                startup_directory: Some("/srv/app".to_string()),
                startup_command: Some("uptime".to_string()),
                start_in_files: false,
                persistent_session: MobilePersistentSession {
                    enabled: true,
                    session_name: Some("tr-prod".to_string()),
                    detach_others: true,
                },
                terminal_scrollback_rows: None,
                color_tag: None,
                environment: Vec::new(),
                known_host_endpoint: Some("prod.example.com:22".to_string()),
            }],
            groups: Vec::new(),
            tags: Vec::new(),
            identities: Vec::new(),
            known_hosts: Vec::new(),
            sync: MobileSyncMetadata::default(),
            devices: Vec::new(),
            device_keys: Vec::new(),
        };

        let envelope =
            encrypt_mobile_vault_export(&export, "hunter2").expect("encrypt mobile vault");
        let envelope_json = serde_json::to_string(&envelope).expect("serialize envelope");

        assert_eq!(envelope.version, ENCRYPTED_MOBILE_VAULT_VERSION);
        assert_eq!(envelope.schema_version, MOBILE_VAULT_SCHEMA_VERSION);
        assert_eq!(envelope.cipher, MOBILE_VAULT_CIPHER);
        assert_eq!(envelope.kdf, MOBILE_VAULT_KDF);
        assert!(!envelope_json.contains("prod.example.com"));
        assert!(!envelope_json.contains("uptime"));

        let decrypted =
            decrypt_mobile_vault_export(&envelope, "hunter2").expect("decrypt mobile vault");
        assert_eq!(decrypted, export);
    }

    #[test]
    fn encrypted_mobile_vault_json_bridge_round_trips() {
        let export = MobileVaultExport::empty("export-1", "desktop-1");
        let envelope_json =
            encrypt_mobile_vault_export_json(&export, "hunter2").expect("encrypt mobile vault");

        assert!(envelope_json.contains("\"cipher\": \"AES-256-GCM-SIV\""));
        assert!(!envelope_json.contains("\"export_id\": \"export-1\""));

        let decrypted =
            decrypt_mobile_vault_export_json(&envelope_json, "hunter2").expect("decrypt export");
        let decrypted_json =
            decrypt_mobile_vault_export_to_json(&envelope_json, "hunter2").expect("decrypt json");

        assert_eq!(decrypted, export);
        assert!(decrypted_json.contains("\"export_id\": \"export-1\""));
        assert!(decrypted_json.contains("\"source_device_id\": \"desktop-1\""));
    }

    #[test]
    fn encrypted_mobile_vault_rejects_wrong_passphrase() {
        let export = MobileVaultExport::empty("export-1", "desktop-1");
        let envelope =
            encrypt_mobile_vault_export(&export, "correct").expect("encrypt mobile vault");

        let error = decrypt_mobile_vault_export(&envelope, "wrong")
            .expect_err("wrong passphrase should fail");

        assert_eq!(error, MobileVaultCryptoError::DecryptFailed);
    }

    #[test]
    fn encrypted_mobile_vault_rejects_unsupported_schema_version() {
        let export = MobileVaultExport::empty("export-1", "desktop-1");
        let mut envelope =
            encrypt_mobile_vault_export(&export, "hunter2").expect("encrypt mobile vault");
        envelope.schema_version = MOBILE_VAULT_SCHEMA_VERSION + 1;

        let error = decrypt_mobile_vault_export(&envelope, "hunter2")
            .expect_err("unsupported schema should fail");

        assert_eq!(
            error,
            MobileVaultCryptoError::UnsupportedSchemaVersion(MOBILE_VAULT_SCHEMA_VERSION + 1)
        );
    }

    #[test]
    fn encrypted_mobile_vault_rejects_unsupported_cipher_and_kdf() {
        let export = MobileVaultExport::empty("export-1", "desktop-1");
        let mut envelope =
            encrypt_mobile_vault_export(&export, "hunter2").expect("encrypt mobile vault");
        envelope.cipher = "AES-128-GCM".to_string();

        let error = decrypt_mobile_vault_export(&envelope, "hunter2")
            .expect_err("unsupported cipher should fail");

        assert_eq!(
            error,
            MobileVaultCryptoError::UnsupportedCipher("AES-128-GCM".to_string())
        );

        let mut envelope =
            encrypt_mobile_vault_export(&export, "hunter2").expect("encrypt mobile vault");
        envelope.kdf = "PBKDF2".to_string();

        let error = decrypt_mobile_vault_export(&envelope, "hunter2").expect_err("unsupported kdf");

        assert_eq!(
            error,
            MobileVaultCryptoError::UnsupportedKdf("PBKDF2".to_string())
        );
    }

    #[test]
    fn encrypted_mobile_vault_rejects_empty_passphrase() {
        let export = MobileVaultExport::empty("export-1", "desktop-1");

        let error =
            encrypt_mobile_vault_export(&export, " ").expect_err("empty passphrase should fail");

        assert_eq!(error, MobileVaultCryptoError::EmptyPassphrase);
    }
}
