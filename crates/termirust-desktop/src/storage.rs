use aes_gcm_siv::aead::{Aead, Payload};
use aes_gcm_siv::{Aes256GcmSiv, KeyInit, Nonce};
use anyhow::{Context, Result, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(test)]
use termirust_protocol::{
    EncryptedMobileVaultEnvelope, decrypt_mobile_vault_export as decrypt_mobile_vault_envelope,
};
use termirust_protocol::{
    MOBILE_VAULT_SCHEMA_VERSION, MobileAuthKind, MobileAuthMetadata, MobileDeviceRecord,
    MobileEnvironmentVariable, MobileGroup, MobileHost, MobileIdentityMetadata, MobileKnownHost,
    MobilePersistentSession, MobileVault, MobileVaultExport,
    encrypt_mobile_vault_export as encrypt_mobile_vault_envelope, mobile_secret_ref_for_host,
};

use crate::models::{
    AuthMode, DEFAULT_VAULT_ID, HostProfile, ImportedIdentity, ProfileSource, SavedIdentity,
    SavedSnippet, SavedState, SavedVault, VaultKind,
};

const APP_DIR_NAME: &str = "termirust";
const STATE_FILE_NAME: &str = "state.json";
const KNOWN_HOSTS_FILE_NAME: &str = "known_hosts.json";
const PORTABLE_BUNDLE_VERSION: u16 = 1;
const ENCRYPTED_PORTABLE_BUNDLE_VERSION: u16 = 1;
const PORTABLE_BUNDLE_AAD: &[u8] = b"termirust.portable-bundle.v1";
const PORTABLE_BUNDLE_KEY_LEN: usize = 32;
const PORTABLE_BUNDLE_SALT_LEN: usize = 16;
const PORTABLE_BUNDLE_NONCE_LEN: usize = 12;
const PORTABLE_BUNDLE_ARGON2_MEMORY_KIB: u32 = 19_456;
const PORTABLE_BUNDLE_ARGON2_ITERATIONS: u32 = 3;
const PORTABLE_BUNDLE_ARGON2_PARALLELISM: u32 = 1;

#[cfg(test)]
thread_local! {
    static TEST_APP_DIR_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableDataBundle {
    version: u16,
    exported_at_millis: u128,
    #[serde(default)]
    vaults: Vec<SavedVault>,
    #[serde(default)]
    profiles: Vec<HostProfile>,
    #[serde(default)]
    identities: Vec<SavedIdentity>,
    #[serde(default)]
    snippets: Vec<SavedSnippet>,
    #[serde(default)]
    known_hosts: HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EncryptedPortableDataBundle {
    version: u16,
    cipher: String,
    kdf: String,
    salt: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortableDataReport {
    pub vaults: usize,
    pub profiles: usize,
    pub identities: usize,
    pub snippets: usize,
    pub known_hosts: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MobileVaultExportReport {
    pub vaults: usize,
    pub hosts: usize,
    pub identities: usize,
    pub known_hosts: usize,
}

pub(crate) fn app_dir() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_APP_DIR_OVERRIDE.with(|override_path| override_path.borrow().clone()) {
        fs::create_dir_all(&path)
            .with_context(|| format!("Unable to create app directory at {}", path.display()))?;
        return Ok(path);
    }

    let path = if let Some(explicit_dir) = std::env::var_os("TERMIRUST_CONFIG_DIR") {
        PathBuf::from(explicit_dir)
    } else {
        let base_dir = dirs::config_dir().unwrap_or(std::env::current_dir()?);
        base_dir.join(APP_DIR_NAME)
    };
    fs::create_dir_all(&path)
        .with_context(|| format!("Unable to create app directory at {}", path.display()))?;
    Ok(path)
}

pub(crate) fn managed_agent_worktree_dir() -> Result<PathBuf> {
    Ok(app_dir()?.join("agent-worktrees"))
}

pub(crate) fn project_store_dir() -> Result<PathBuf> {
    Ok(app_dir()?.join("agent-workspace"))
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn controller_store_dir() -> Result<PathBuf> {
    Ok(app_dir()?.join("controller"))
}

#[cfg(test)]
pub(crate) fn set_test_app_dir_override(path: Option<PathBuf>) -> Option<PathBuf> {
    TEST_APP_DIR_OVERRIDE.with(|override_path| override_path.replace(path))
}

fn state_file() -> Result<PathBuf> {
    Ok(app_dir()?.join(STATE_FILE_NAME))
}

fn known_hosts_file() -> Result<PathBuf> {
    Ok(app_dir()?.join(KNOWN_HOSTS_FILE_NAME))
}

pub fn export_portable_data_bundle(
    path: impl AsRef<Path>,
    state: &SavedState,
    known_hosts: &KnownHostStore,
) -> Result<PortableDataReport> {
    let bundle = build_portable_data_bundle(state, known_hosts)?;
    let report = report_for_portable_data_bundle(&bundle);
    let content = serde_json::to_string_pretty(&bundle)?;
    write_bundle_file(path.as_ref(), content)?;
    Ok(report)
}

pub fn export_encrypted_portable_data_bundle(
    path: impl AsRef<Path>,
    state: &SavedState,
    known_hosts: &KnownHostStore,
    passphrase: &str,
) -> Result<PortableDataReport> {
    ensure_backup_passphrase(passphrase)?;
    let bundle = build_portable_data_bundle(state, known_hosts)?;
    let report = report_for_portable_data_bundle(&bundle);
    let plaintext = serde_json::to_vec_pretty(&bundle)?;
    let encrypted = encrypt_portable_data_bundle(&plaintext, passphrase)?;
    let content = serde_json::to_string_pretty(&encrypted)?;
    write_bundle_file(path.as_ref(), content)?;
    Ok(report)
}

pub fn export_encrypted_mobile_vault(
    path: impl AsRef<Path>,
    state: &SavedState,
    known_hosts: &KnownHostStore,
    passphrase: &str,
    export_id: impl Into<String>,
    source_device_id: impl Into<String>,
) -> Result<MobileVaultExportReport> {
    ensure_backup_passphrase(passphrase)?;
    let export = build_mobile_vault_export(state, known_hosts, export_id, source_device_id)?;
    let report = report_for_mobile_vault_export(&export);
    let encrypted = encrypt_mobile_vault_envelope(&export, passphrase)?;
    let content = serde_json::to_string_pretty(&encrypted)?;
    write_bundle_file(path.as_ref(), content)?;
    Ok(report)
}

#[cfg(test)]
pub fn read_encrypted_mobile_vault_export(
    path: impl AsRef<Path>,
    passphrase: &str,
) -> Result<MobileVaultExport> {
    ensure_backup_passphrase(passphrase)?;
    let content = fs::read_to_string(path.as_ref())
        .with_context(|| format!("Unable to read {}", path.as_ref().display()))?;
    let encrypted: EncryptedMobileVaultEnvelope = serde_json::from_str(&content)
        .with_context(|| format!("Unable to parse {}", path.as_ref().display()))?;
    decrypt_mobile_vault_envelope(&encrypted, passphrase).with_context(|| {
        format!(
            "Unable to decode encrypted mobile vault {}",
            path.as_ref().display()
        )
    })
}

pub fn import_portable_data_bundle(
    path: impl AsRef<Path>,
    state: &mut SavedState,
    known_hosts: &KnownHostStore,
) -> Result<PortableDataReport> {
    let bundle = read_plain_portable_data_bundle(path.as_ref())?;
    apply_portable_data_bundle(bundle, state, known_hosts)
}

pub fn import_encrypted_portable_data_bundle(
    path: impl AsRef<Path>,
    state: &mut SavedState,
    known_hosts: &KnownHostStore,
    passphrase: &str,
) -> Result<PortableDataReport> {
    ensure_backup_passphrase(passphrase)?;
    let bundle = read_encrypted_portable_data_bundle(path.as_ref(), passphrase)?;
    apply_portable_data_bundle(bundle, state, known_hosts)
}

fn build_portable_data_bundle(
    state: &SavedState,
    known_hosts: &KnownHostStore,
) -> Result<PortableDataBundle> {
    let mut exported = state.clone();
    exported.ensure_vaults();
    exported
        .profiles
        .retain(|profile| profile.source == ProfileSource::User);

    Ok(PortableDataBundle {
        version: PORTABLE_BUNDLE_VERSION,
        exported_at_millis: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        vaults: exported.vaults.clone(),
        profiles: exported
            .profiles
            .into_iter()
            .map(|mut profile| {
                profile.password_credential_id = None;
                profile
            })
            .collect(),
        identities: exported.identities.clone(),
        snippets: exported.snippets.clone(),
        known_hosts: known_hosts.entries()?.into_iter().collect(),
    })
}

fn report_for_portable_data_bundle(bundle: &PortableDataBundle) -> PortableDataReport {
    PortableDataReport {
        vaults: bundle.vaults.len(),
        profiles: bundle.profiles.len(),
        identities: bundle.identities.len(),
        snippets: bundle.snippets.len(),
        known_hosts: bundle.known_hosts.len(),
    }
}

fn build_mobile_vault_export(
    state: &SavedState,
    known_hosts: &KnownHostStore,
    export_id: impl Into<String>,
    source_device_id: impl Into<String>,
) -> Result<MobileVaultExport> {
    let mut exported = state.clone();
    exported.ensure_vaults();
    exported
        .profiles
        .retain(|profile| profile.source == ProfileSource::User);
    if exported.profiles.iter().any(|profile| {
        profile.certificate_path.is_some() || profile.auth_mode == AuthMode::LocalAgent
    }) {
        bail!(
            "Mobile vault export does not yet support OpenSSH certificate or SSH-agent hosts; remove those hosts from the export or use the portable desktop bundle"
        );
    }

    let exported_at_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let source_device_id = source_device_id.into();
    let mut groups = Vec::new();
    let mut tags = BTreeMap::<String, ()>::new();
    let hosts = exported
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
                tags.insert(tag.clone(), ());
            }
            mobile_host_from_profile(profile)
        })
        .collect();

    let mut devices = exported.settings.mobile_devices;
    if let Some(device) = devices
        .iter_mut()
        .find(|device| device.device_id == source_device_id)
    {
        device.label = "TermiRust Desktop".to_string();
        device.platform = Some("desktop".to_string());
        device.revoked_at_millis = None;
    } else {
        devices.push(MobileDeviceRecord::active_desktop(
            source_device_id.clone(),
            "TermiRust Desktop",
        ));
    }

    Ok(MobileVaultExport {
        schema_version: MOBILE_VAULT_SCHEMA_VERSION,
        export_id: export_id.into(),
        created_at_millis: exported_at_millis,
        updated_at_millis: exported_at_millis,
        source_device_id: source_device_id.clone(),
        vaults: exported
            .vaults
            .into_iter()
            .map(mobile_vault_from_saved)
            .collect(),
        hosts,
        groups,
        tags: tags.into_keys().collect(),
        identities: exported
            .identities
            .into_iter()
            .map(mobile_identity_from_saved)
            .collect(),
        known_hosts: known_hosts
            .entries()?
            .into_iter()
            .map(|(endpoint, public_key)| MobileKnownHost {
                endpoint,
                public_key,
                algorithm: None,
                fingerprint: None,
            })
            .collect(),
        sync: Default::default(),
        devices,
        device_keys: exported.settings.mobile_device_keys,
    })
}

fn report_for_mobile_vault_export(export: &MobileVaultExport) -> MobileVaultExportReport {
    MobileVaultExportReport {
        vaults: export.vaults.len(),
        hosts: export.hosts.len(),
        identities: export.identities.len(),
        known_hosts: export.known_hosts.len(),
    }
}

fn mobile_vault_from_saved(vault: SavedVault) -> MobileVault {
    MobileVault {
        id: vault.id,
        label: vault.label,
        description: vault.description,
        kind: match vault.kind {
            VaultKind::Personal => "personal",
            VaultKind::Shared => "shared",
        }
        .to_string(),
    }
}

fn mobile_identity_from_saved(identity: SavedIdentity) -> MobileIdentityMetadata {
    MobileIdentityMetadata {
        id: identity.id,
        label: identity.label,
        vault_id: identity.vault_id,
        kind: identity.kind,
        public_key: None,
        fingerprint: None,
        secret_ref: None,
    }
}

fn mobile_host_from_profile(profile: HostProfile) -> MobileHost {
    let auth_kind = match profile.auth_mode {
        AuthMode::Password => MobileAuthKind::Password,
        AuthMode::PrivateKey => MobileAuthKind::PrivateKey,
        AuthMode::LocalAgent => unreachable!("agent profiles are rejected before mobile export"),
    };
    let secret_ref =
        mobile_secret_ref_for_host(&profile.id, auth_kind, profile.identity_id.as_deref());
    MobileHost {
        id: profile.id,
        label: profile.label,
        vault_id: profile.vault_id,
        group: profile.group,
        tags: profile.tags,
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username,
        auth: MobileAuthMetadata {
            kind: auth_kind,
            identity_id: profile.identity_id,
            secret_ref: Some(secret_ref),
        },
        jump_host_id: profile.jump_host_id,
        startup_directory: profile.startup_directory,
        startup_command: profile.startup_command,
        start_in_files: profile.start_in_files,
        persistent_session: MobilePersistentSession {
            enabled: profile.persistent_session,
            session_name: profile.persistent_session_name,
            detach_others: profile.persistent_session_detach_others,
        },
        terminal_scrollback_rows: profile.terminal_scrollback_rows,
        color_tag: profile
            .color_tag
            .map(|tag| tag.label().to_ascii_lowercase()),
        environment: profile
            .environment
            .into_iter()
            .map(|(name, value)| MobileEnvironmentVariable { name, value })
            .collect(),
        known_host_endpoint: Some(format!("{}:{}", profile.host, profile.port)),
    }
}

fn apply_portable_data_bundle(
    bundle: PortableDataBundle,
    state: &mut SavedState,
    known_hosts: &KnownHostStore,
) -> Result<PortableDataReport> {
    let mut report = PortableDataReport::default();

    for vault in bundle.vaults {
        if vault.id == DEFAULT_VAULT_ID {
            continue;
        }
        state.upsert_vault(vault);
        report.vaults += 1;
    }

    for mut profile in bundle.profiles {
        profile.source = ProfileSource::User;
        profile.password_credential_id = None;
        state.upsert_profile(profile);
        report.profiles += 1;
    }

    for mut identity in bundle.identities {
        identity.source = crate::models::IdentitySource::User;
        state.upsert_identity(identity);
        report.identities += 1;
    }

    for snippet in bundle.snippets {
        state.upsert_snippet(snippet);
        report.snippets += 1;
    }

    report.known_hosts = known_hosts.merge_entries(bundle.known_hosts)?;

    state.ensure_vaults();
    Ok(report)
}

fn write_bundle_file(path: &Path, content: String) -> Result<()> {
    fs::write(path, content).with_context(|| format!("Unable to write {}", path.display()))?;
    Ok(())
}

fn read_plain_portable_data_bundle(path: &Path) -> Result<PortableDataBundle> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Unable to read {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("Unable to parse {}", path.display()))
}

fn read_encrypted_portable_data_bundle(
    path: &Path,
    passphrase: &str,
) -> Result<PortableDataBundle> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Unable to read {}", path.display()))?;
    let encrypted: EncryptedPortableDataBundle = serde_json::from_str(&content)
        .with_context(|| format!("Unable to parse {}", path.display()))?;
    let plaintext = decrypt_portable_data_bundle(&encrypted, passphrase)?;
    serde_json::from_slice(&plaintext)
        .with_context(|| format!("Unable to decode encrypted bundle {}", path.display()))
}

fn ensure_backup_passphrase(passphrase: &str) -> Result<()> {
    anyhow::ensure!(
        !passphrase.trim().is_empty(),
        "Backup passphrase cannot be empty."
    );
    Ok(())
}

fn encrypt_portable_data_bundle(
    plaintext: &[u8],
    passphrase: &str,
) -> Result<EncryptedPortableDataBundle> {
    let mut salt = [0u8; PORTABLE_BUNDLE_SALT_LEN];
    let mut nonce = [0u8; PORTABLE_BUNDLE_NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);

    let key = derive_portable_bundle_key(passphrase, &salt)?;
    let cipher =
        Aes256GcmSiv::new_from_slice(&key).context("Unable to initialize backup cipher")?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: PORTABLE_BUNDLE_AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("Unable to encrypt portable bundle"))?;

    Ok(EncryptedPortableDataBundle {
        version: ENCRYPTED_PORTABLE_BUNDLE_VERSION,
        cipher: "AES-256-GCM-SIV".to_string(),
        kdf: "Argon2id(m=19456,t=3,p=1)".to_string(),
        salt: STANDARD_NO_PAD.encode(salt),
        nonce: STANDARD_NO_PAD.encode(nonce),
        ciphertext: STANDARD_NO_PAD.encode(ciphertext),
    })
}

fn decrypt_portable_data_bundle(
    encrypted: &EncryptedPortableDataBundle,
    passphrase: &str,
) -> Result<Vec<u8>> {
    anyhow::ensure!(
        encrypted.version == ENCRYPTED_PORTABLE_BUNDLE_VERSION,
        "Unsupported encrypted bundle version {}.",
        encrypted.version
    );

    let salt = STANDARD_NO_PAD
        .decode(&encrypted.salt)
        .context("Encrypted bundle salt is invalid.")?;
    let nonce = STANDARD_NO_PAD
        .decode(&encrypted.nonce)
        .context("Encrypted bundle nonce is invalid.")?;
    let ciphertext = STANDARD_NO_PAD
        .decode(&encrypted.ciphertext)
        .context("Encrypted bundle ciphertext is invalid.")?;

    anyhow::ensure!(
        salt.len() == PORTABLE_BUNDLE_SALT_LEN,
        "Encrypted bundle salt is invalid."
    );
    anyhow::ensure!(
        nonce.len() == PORTABLE_BUNDLE_NONCE_LEN,
        "Encrypted bundle nonce is invalid."
    );

    let key = derive_portable_bundle_key(passphrase, &salt)?;
    let cipher =
        Aes256GcmSiv::new_from_slice(&key).context("Unable to initialize backup cipher")?;
    cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: PORTABLE_BUNDLE_AAD,
            },
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "Unable to decrypt encrypted bundle. Check the passphrase and file integrity."
            )
        })
}

fn derive_portable_bundle_key(
    passphrase: &str,
    salt: &[u8],
) -> Result<[u8; PORTABLE_BUNDLE_KEY_LEN]> {
    let params = Params::new(
        PORTABLE_BUNDLE_ARGON2_MEMORY_KIB,
        PORTABLE_BUNDLE_ARGON2_ITERATIONS,
        PORTABLE_BUNDLE_ARGON2_PARALLELISM,
        Some(PORTABLE_BUNDLE_KEY_LEN),
    )
    .map_err(|_| anyhow::anyhow!("Unable to configure backup key derivation"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; PORTABLE_BUNDLE_KEY_LEN];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| anyhow::anyhow!("Unable to derive backup key"))?;
    Ok(key)
}

pub fn load_saved_state() -> Result<SavedState> {
    let path = state_file()?;
    if !path.exists() {
        let mut state = SavedState::default();
        state.ensure_vaults();
        return Ok(state);
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("Unable to read {}", path.display()))?;
    let mut state: SavedState = serde_json::from_str(&content)
        .with_context(|| format!("Unable to parse {}", path.display()))?;
    state.mark_app_attached_sessions_exited();
    state.ensure_vaults();
    Ok(state)
}

pub fn save_saved_state(state: &SavedState) -> Result<()> {
    let path = state_file()?;
    let mut persisted = state.clone();
    persisted.mark_app_attached_sessions_exited();
    persisted.ensure_vaults();
    persisted
        .profiles
        .retain(|profile| profile.source == ProfileSource::User);
    persisted
        .restored_workspaces
        .iter_mut()
        .for_each(|workspace| workspace.normalize());
    persisted
        .restored_workspaces
        .retain(|workspace| !workspace.panes.is_empty());
    if persisted
        .selected_profile_id
        .as_ref()
        .is_some_and(|profile_id| !persisted.profiles.iter().any(|item| &item.id == profile_id))
    {
        persisted.selected_profile_id =
            persisted.profiles.first().map(|profile| profile.id.clone());
    }
    if persisted
        .active_workspace_index
        .is_some_and(|index| index >= persisted.restored_workspaces.len())
    {
        persisted.active_workspace_index = None;
    }

    let content = serde_json::to_string_pretty(&persisted)?;
    fs::write(&path, content).with_context(|| format!("Unable to write {}", path.display()))?;
    Ok(())
}

pub fn load_local_ssh_identities() -> Result<Vec<ImportedIdentity>> {
    let Some(home_dir) = dirs::home_dir() else {
        return Ok(Vec::new());
    };

    let ssh_dir = home_dir.join(".ssh");
    if !ssh_dir.exists() {
        return Ok(Vec::new());
    }

    let mut identities = Vec::new();
    for entry in
        fs::read_dir(&ssh_dir).with_context(|| format!("Unable to read {}", ssh_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let label = entry.file_name().to_string_lossy().trim().to_string();
        if should_skip_ssh_entry(&label) {
            continue;
        }

        let Some(identity) = inspect_identity_file(&path)? else {
            continue;
        };

        identities.push(ImportedIdentity {
            label: label.clone(),
            ..identity
        });
    }

    identities.sort_by(|left, right| {
        identity_priority(&left.label)
            .cmp(&identity_priority(&right.label))
            .then_with(|| {
                left.label
                    .to_ascii_lowercase()
                    .cmp(&right.label.to_ascii_lowercase())
            })
    });

    Ok(identities)
}

pub fn inspect_identity_file(path: &std::path::Path) -> Result<Option<ImportedIdentity>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };

    let preview = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let Some(kind) = detect_identity_kind(&preview) else {
        return Ok(None);
    };

    let label = path
        .file_name()
        .map(|name| name.to_string_lossy().trim().to_string())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| path.display().to_string());

    Ok(Some(ImportedIdentity {
        label,
        path: path.display().to_string(),
        kind: kind.to_string(),
    }))
}

fn should_skip_ssh_entry(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();

    lower.starts_with('.')
        || lower.ends_with(".pub")
        || lower == "authorized_keys"
        || lower == "config"
        || lower.starts_with("known_hosts")
}

fn detect_identity_kind(preview: &str) -> Option<&'static str> {
    let preview = preview.trim_start_matches('\u{feff}').trim_start();

    if preview.starts_with("PuTTY-User-Key-File-") {
        Some("PuTTY PPK")
    } else if preview.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") {
        Some("OpenSSH")
    } else if preview.starts_with("-----BEGIN RSA PRIVATE KEY-----") {
        Some("RSA PEM")
    } else if preview.starts_with("-----BEGIN EC PRIVATE KEY-----") {
        Some("EC PEM")
    } else if preview.starts_with("-----BEGIN PRIVATE KEY-----")
        || preview.starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----")
    {
        Some("PKCS#8")
    } else {
        None
    }
}

fn identity_priority(label: &str) -> u8 {
    let lower = label.to_ascii_lowercase();

    match lower.as_str() {
        "id_ed25519" => 0,
        "id_ecdsa" => 1,
        "id_rsa" => 2,
        "identity" => 3,
        _ if lower.starts_with("id_") => 4,
        _ => 5,
    }
}

pub fn load_local_ssh_hosts() -> Result<Vec<HostProfile>> {
    let Some(home_dir) = dirs::home_dir() else {
        return Ok(Vec::new());
    };

    let config_path = home_dir.join(".ssh").join("config");
    if !config_path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("Unable to read {}", config_path.display()))?;
    Ok(parse_ssh_config_hosts(&content))
}

#[derive(Default)]
struct SshConfigBlock {
    host_name: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    certificate_file: Option<String>,
    identity_agent: Option<String>,
    proxy_jump: Option<String>,
    unsupported_proxy_command: bool,
}

fn parse_ssh_config_hosts(content: &str) -> Vec<HostProfile> {
    let mut entries = HashMap::new();
    let mut aliases = Vec::new();
    let mut block = SshConfigBlock::default();

    for raw_line in content.lines() {
        let line = strip_ssh_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        let Some((key, value)) = split_ssh_directive(line) else {
            continue;
        };
        let key = key.to_ascii_lowercase();

        if key == "host" {
            flush_ssh_config_block(&mut entries, &aliases, &block);
            aliases = value
                .split_whitespace()
                .filter(|alias| is_importable_host_alias(alias))
                .map(|alias| alias.to_string())
                .collect();
            block = SshConfigBlock::default();
            continue;
        }

        if key == "match" {
            flush_ssh_config_block(&mut entries, &aliases, &block);
            aliases.clear();
            block = SshConfigBlock::default();
            continue;
        }

        if aliases.is_empty() {
            continue;
        }

        match key.as_str() {
            "hostname" => block.host_name = Some(value.to_string()),
            "user" => block.user = Some(value.to_string()),
            "port" => block.port = value.parse::<u16>().ok(),
            "identityfile" => {
                if block.identity_file.is_none() {
                    block.identity_file = Some(expand_home_path(value));
                }
            }
            "certificatefile" => {
                if block.certificate_file.is_none() {
                    block.certificate_file = Some(expand_home_path(value));
                }
            }
            "identityagent" => {
                if block.identity_agent.is_none() {
                    block.identity_agent = Some(
                        if value.eq_ignore_ascii_case("SSH_AUTH_SOCK")
                            || value.eq_ignore_ascii_case("none")
                        {
                            value.to_string()
                        } else {
                            expand_home_path(value)
                        },
                    );
                }
            }
            "proxyjump" => {
                if block.proxy_jump.is_none() {
                    block.proxy_jump = parse_proxyjump_alias(value);
                }
            }
            "proxycommand" => block.unsupported_proxy_command = true,
            _ => {}
        }
    }

    flush_ssh_config_block(&mut entries, &aliases, &block);

    let mut hosts = entries.into_values().collect::<Vec<_>>();
    hosts.sort_by_key(|profile| profile.display_name().to_ascii_lowercase());
    hosts
}

fn flush_ssh_config_block(
    entries: &mut HashMap<String, HostProfile>,
    aliases: &[String],
    block: &SshConfigBlock,
) {
    if aliases.is_empty() || block.unsupported_proxy_command {
        return;
    }

    for alias in aliases {
        let host = block
            .host_name
            .clone()
            .unwrap_or_else(|| alias.trim().to_string());
        if host.is_empty() {
            continue;
        }

        let username = block
            .user
            .clone()
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_default();
        let key_path = block
            .identity_file
            .clone()
            .filter(|path| !path.to_ascii_lowercase().ends_with(".pub"))
            .unwrap_or_default();

        let agent_configured = block
            .identity_agent
            .as_deref()
            .is_some_and(|value| !value.eq_ignore_ascii_case("none"));
        let auth_mode = if !key_path.is_empty() {
            AuthMode::PrivateKey
        } else if agent_configured {
            AuthMode::LocalAgent
        } else {
            AuthMode::Password
        };

        entries.insert(
            imported_host_id(alias),
            HostProfile {
                id: imported_host_id(alias),
                label: alias.trim().to_string(),
                vault_id: Some(crate::models::DEFAULT_VAULT_ID.to_string()),
                favorite: false,
                group: String::new(),
                tags: Vec::new(),
                host,
                port: block.port.unwrap_or(22),
                username,
                auth_mode,
                certificate_path: (!key_path.is_empty())
                    .then(|| block.certificate_file.clone())
                    .flatten(),
                identity_agent: (auth_mode == AuthMode::LocalAgent)
                    .then(|| block.identity_agent.clone())
                    .flatten()
                    .filter(|value| !value.eq_ignore_ascii_case("SSH_AUTH_SOCK")),
                key_path,
                identity_id: None,
                jump_host_id: block
                    .proxy_jump
                    .as_ref()
                    .map(|alias| imported_host_id(alias)),
                outbound_proxy: None,
                startup_directory: None,
                startup_command: None,
                start_in_files: false,
                persistent_session: false,
                persistent_session_name: None,
                persistent_session_detach_others: false,
                terminal_scrollback_rows: None,
                port_forward_rules: Vec::new(),
                local_forwards: Vec::new(),
                local_forward: None,
                password_credential_id: None,
                source: ProfileSource::SshConfig,
                description: String::new(),
                color_tag: None,
                environment: Vec::new(),
            },
        );
    }
}

fn parse_proxyjump_alias(value: &str) -> Option<String> {
    let first = value.split(',').next()?.trim();
    if first.is_empty() {
        return None;
    }

    let host_part = first.rsplit('@').next().unwrap_or(first);
    let alias = host_part.split(':').next().unwrap_or(host_part).trim();
    if alias.is_empty() || !is_importable_host_alias(alias) {
        return None;
    }

    Some(alias.to_string())
}

fn strip_ssh_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or("")
}

fn split_ssh_directive(line: &str) -> Option<(&str, &str)> {
    let sep = line.find(|c: char| c == '=' || c.is_ascii_whitespace())?;
    let key = line[..sep].trim();
    let rest = line[sep..].trim_start_matches(|c: char| c == '=' || c.is_ascii_whitespace());
    let value = rest.trim();
    if key.is_empty() || value.is_empty() {
        None
    } else {
        Some((key, value))
    }
}

fn is_importable_host_alias(alias: &str) -> bool {
    !alias.is_empty() && !alias.contains('*') && !alias.contains('?') && !alias.starts_with('!')
}

fn expand_home_path(value: &str) -> String {
    if let Some(stripped) = value.strip_prefix("~/") {
        if let Some(home_dir) = dirs::home_dir() {
            return home_dir.join(stripped).display().to_string();
        }
    }

    value.to_string()
}

fn imported_host_id(alias: &str) -> String {
    let mut normalized = String::with_capacity(alias.len());
    for ch in alias.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
        } else {
            normalized.push('-');
        }
    }

    while normalized.contains("--") {
        normalized = normalized.replace("--", "-");
    }

    format!("ssh-config-{}", normalized.trim_matches('-'))
}

#[derive(Default, Serialize, Deserialize)]
struct KnownHostsFile {
    #[serde(default)]
    entries: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyDecision {
    Existing,
    Added,
}

#[derive(Debug)]
pub struct KnownHostStore {
    path: PathBuf,
    entries: Mutex<HashMap<String, String>>,
}

impl KnownHostStore {
    fn persist_entries(&self, entries: &HashMap<String, String>) -> Result<()> {
        let file = KnownHostsFile {
            entries: entries.clone(),
        };
        let content = serde_json::to_string_pretty(&file)?;
        fs::write(&self.path, content)
            .with_context(|| format!("Unable to write {}", self.path.display()))?;
        Ok(())
    }

    pub fn load() -> Result<Self> {
        Self::open(known_hosts_file()?)
    }

    pub(crate) fn open(path: PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                path,
                entries: Mutex::new(HashMap::new()),
            });
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Unable to read {}", path.display()))?;
        let file: KnownHostsFile = serde_json::from_str(&content)
            .with_context(|| format!("Unable to parse {}", path.display()))?;

        Ok(Self {
            path,
            entries: Mutex::new(file.entries),
        })
    }

    pub fn verify_or_trust(&self, endpoint: &str, key: &str) -> Result<HostKeyDecision> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Unable to lock known host store"))?;

        match entries.get(endpoint) {
            Some(existing) if existing == key => Ok(HostKeyDecision::Existing),
            Some(_) => Err(anyhow::anyhow!(
                "Host key mismatch for {endpoint}. Remove the saved entry from {} if the server changed keys intentionally.",
                self.path.display()
            )),
            None => {
                entries.insert(endpoint.to_string(), key.to_string());
                self.persist_entries(&entries)?;
                Ok(HostKeyDecision::Added)
            }
        }
    }

    pub fn verify_existing(&self, endpoint: &str, key: &str) -> Result<()> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Unable to lock known host store"))?;

        match entries.get(endpoint) {
            Some(existing) if existing == key => Ok(()),
            Some(_) => Err(anyhow::anyhow!(
                "Host key mismatch for {endpoint}. Verify the server key before removing the saved entry."
            )),
            None => Err(anyhow::anyhow!(
                "Host key is not trusted for {endpoint}. Connect normally once to review and pin it."
            )),
        }
    }

    pub fn entries(&self) -> Result<Vec<(String, String)>> {
        let entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Unable to lock known host store"))?;
        let mut items = entries
            .iter()
            .map(|(endpoint, key)| (endpoint.clone(), key.clone()))
            .collect::<Vec<_>>();
        items.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(items)
    }

    pub fn remove(&self, endpoint: &str) -> Result<bool> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Unable to lock known host store"))?;
        let removed = entries.remove(endpoint).is_some();
        if removed {
            self.persist_entries(&entries)?;
        }
        Ok(removed)
    }

    pub fn merge_entries(&self, imported: HashMap<String, String>) -> Result<usize> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Unable to lock known host store"))?;
        let mut added = 0usize;
        for (endpoint, key) in imported {
            if entries.contains_key(&endpoint) {
                continue;
            }
            entries.insert(endpoint, key);
            added += 1;
        }
        if added > 0 {
            self.persist_entries(&entries)?;
        }
        Ok(added)
    }

    pub(crate) fn replace_entries(&self, replacement: HashMap<String, String>) -> Result<()> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Unable to lock known host store"))?;
        if *entries == replacement {
            return Ok(());
        }
        self.persist_entries(&replacement)?;
        *entries = replacement;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        KnownHostStore, KnownHostsFile, detect_identity_kind, export_encrypted_mobile_vault,
        export_encrypted_portable_data_bundle, export_portable_data_bundle, identity_priority,
        import_encrypted_portable_data_bundle, import_portable_data_bundle, parse_ssh_config_hosts,
        read_encrypted_mobile_vault_export, should_skip_ssh_entry,
    };
    use crate::models::{
        AppSettings, AuthMode, DEFAULT_VAULT_ID, HostProfile, ProfileSource, SavedIdentity,
        SavedSnippet, SavedState, SavedVault, ThemePreset, VaultKind, VaultMemberRole,
    };
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    use termirust_protocol::{MOBILE_VAULT_SCHEMA_VERSION, MobileAuthKind, MobileDeviceRecord};

    #[test]
    fn detects_supported_identity_formats() {
        assert_eq!(
            detect_identity_kind("-----BEGIN OPENSSH PRIVATE KEY-----\nabc"),
            Some("OpenSSH")
        );
        assert_eq!(
            detect_identity_kind("PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none"),
            Some("PuTTY PPK")
        );
        assert_eq!(
            detect_identity_kind("-----BEGIN RSA PRIVATE KEY-----\nabc"),
            Some("RSA PEM")
        );
        assert_eq!(
            detect_identity_kind("-----BEGIN PRIVATE KEY-----\nabc"),
            Some("PKCS#8")
        );
        assert_eq!(detect_identity_kind("not a key"), None);
    }

    #[test]
    fn skips_non_identity_ssh_files() {
        assert!(should_skip_ssh_entry("id_ed25519.pub"));
        assert!(should_skip_ssh_entry("known_hosts"));
        assert!(should_skip_ssh_entry("config"));
        assert!(!should_skip_ssh_entry("id_ed25519"));
        assert!(!should_skip_ssh_entry("runpod_key"));
    }

    #[test]
    fn prefers_common_default_identity_names() {
        assert!(identity_priority("id_ed25519") < identity_priority("id_rsa"));
        assert!(identity_priority("id_rsa") < identity_priority("vast"));
    }

    #[test]
    fn parses_simple_ssh_config_hosts() {
        let hosts = parse_ssh_config_hosts(
            r#"
Host app-prod
  HostName 203.0.113.10
  User ubuntu
  IdentityFile ~/.ssh/id_ed25519
  Port 2222

Host wildcard-*
  HostName ignored.example.com
  User root

Host tunnel-box
  ProxyCommand ssh jumpbox -W %h:%p
  HostName 10.0.0.7
  User root
"#,
        );

        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].label, "app-prod");
        assert_eq!(hosts[0].host, "203.0.113.10");
        assert_eq!(hosts[0].username, "ubuntu");
        assert_eq!(hosts[0].port, 2222);
        assert_eq!(hosts[0].auth_mode, AuthMode::PrivateKey);
        assert_eq!(hosts[0].source, ProfileSource::SshConfig);
        assert!(hosts[0].key_path.ends_with("/.ssh/id_ed25519"));
        assert_eq!(hosts[0].certificate_path, None);
    }

    #[test]
    fn parses_ssh_config_certificate_file_without_plain_key_fallback() {
        let hosts = parse_ssh_config_hosts(
            r#"
Host cert-prod
  HostName cert.example.com
  User deploy
  IdentityFile ~/.ssh/id_ed25519
  CertificateFile ~/.ssh/id_ed25519-cert.pub
"#,
        );

        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].auth_mode, AuthMode::PrivateKey);
        assert!(hosts[0].key_path.ends_with("/.ssh/id_ed25519"));
        assert!(
            hosts[0]
                .certificate_path
                .as_deref()
                .is_some_and(|path| path.ends_with("/.ssh/id_ed25519-cert.pub"))
        );
        assert!(matches!(
            hosts[0].saved_auth_config().unwrap(),
            crate::models::AuthConfig::OpenSshCertificate { .. }
        ));
    }

    #[test]
    fn ignores_ssh_config_certificate_without_supported_signer_key() {
        let hosts = parse_ssh_config_hosts(
            r#"
Host cert-only
  HostName cert-only.example.com
  User deploy
  CertificateFile ~/.ssh/id_ed25519-cert.pub
"#,
        );

        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].auth_mode, AuthMode::Password);
        assert_eq!(hosts[0].certificate_path, None);
    }

    #[test]
    fn mobile_vault_export_rejects_certificate_hosts_without_downgrade() {
        let mut profile = parse_ssh_config_hosts(
            r#"
Host cert-prod
  HostName cert.example.com
  User deploy
  IdentityFile ~/.ssh/id_ed25519
  CertificateFile ~/.ssh/id_ed25519-cert.pub
"#,
        )
        .remove(0);
        profile.source = ProfileSource::User;
        let mut state = SavedState::default();
        state.upsert_profile(profile);
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let export_path = std::env::temp_dir().join(format!("termirust-cert-mobile-{suffix}.json"));
        let known_hosts = KnownHostStore {
            path: std::env::temp_dir().join(format!("termirust-cert-hosts-{suffix}.json")),
            entries: std::sync::Mutex::new(HashMap::new()),
        };

        let error = export_encrypted_mobile_vault(
            &export_path,
            &state,
            &known_hosts,
            "hunter2",
            "cert-export",
            "desktop-1",
        )
        .expect_err("certificate host must not export as plain private-key auth");
        assert!(
            error
                .to_string()
                .contains("does not yet support OpenSSH certificate or SSH-agent hosts")
        );
        assert!(!export_path.exists());
    }

    #[test]
    fn parses_ssh_config_identity_agent_without_overriding_explicit_keys() {
        let hosts = parse_ssh_config_hosts(
            r#"
Host default-agent
  HostName agent.example.com
  User deploy
  IdentityAgent SSH_AUTH_SOCK

Host explicit-agent
  HostName explicit.example.com
  IdentityAgent ~/.ssh/custom-agent.sock

Host disabled-agent
  HostName disabled.example.com
  IdentityAgent none

Host key-wins
  HostName key.example.com
  IdentityFile ~/.ssh/id_ed25519
  IdentityAgent ~/.ssh/custom-agent.sock
"#,
        );

        let default_agent = hosts
            .iter()
            .find(|host| host.label == "default-agent")
            .unwrap();
        assert_eq!(default_agent.auth_mode, AuthMode::LocalAgent);
        assert_eq!(default_agent.identity_agent, None);
        assert!(matches!(
            default_agent.saved_auth_config().unwrap(),
            crate::models::AuthConfig::LocalAgent {
                socket_path: None,
                forward_agent: false
            }
        ));

        let explicit = hosts
            .iter()
            .find(|host| host.label == "explicit-agent")
            .unwrap();
        assert_eq!(explicit.auth_mode, AuthMode::LocalAgent);
        assert!(
            explicit
                .identity_agent
                .as_deref()
                .is_some_and(|path| path.ends_with("/.ssh/custom-agent.sock"))
        );
        assert_eq!(
            hosts
                .iter()
                .find(|host| host.label == "disabled-agent")
                .unwrap()
                .auth_mode,
            AuthMode::Password
        );
        assert_eq!(
            hosts
                .iter()
                .find(|host| host.label == "key-wins")
                .unwrap()
                .auth_mode,
            AuthMode::PrivateKey
        );
    }

    #[test]
    fn mobile_vault_export_rejects_ssh_agent_hosts_without_downgrade() {
        let profile = HostProfile {
            id: "agent-export".to_string(),
            label: "Agent export".to_string(),
            host: "agent.example.com".to_string(),
            username: "deploy".to_string(),
            auth_mode: AuthMode::LocalAgent,
            ..Default::default()
        };
        let mut state = SavedState::default();
        state.upsert_profile(profile);
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let export_path =
            std::env::temp_dir().join(format!("termirust-agent-mobile-{suffix}.json"));
        let known_hosts = KnownHostStore {
            path: std::env::temp_dir().join(format!("termirust-agent-hosts-{suffix}.json")),
            entries: std::sync::Mutex::new(HashMap::new()),
        };

        let error = export_encrypted_mobile_vault(
            &export_path,
            &state,
            &known_hosts,
            "hunter2",
            "agent-export",
            "desktop-1",
        )
        .expect_err("agent host must not be downgraded during mobile export");
        assert!(error.to_string().contains("SSH-agent hosts"));
        assert!(!export_path.exists());
    }

    #[test]
    fn parses_ssh_config_with_equals_delimiter() {
        let hosts = parse_ssh_config_hosts(
            r#"
Host eq-host
  HostName=10.0.0.1
  User=deploy
  Port=3022

Host eq-spaced
  HostName = 10.0.0.2
  User = admin
"#,
        );

        assert_eq!(hosts.len(), 2);
        let eq_host = hosts.iter().find(|h| h.label == "eq-host").unwrap();
        assert_eq!(eq_host.host, "10.0.0.1");
        assert_eq!(eq_host.username, "deploy");
        assert_eq!(eq_host.port, 3022);

        let eq_spaced = hosts.iter().find(|h| h.label == "eq-spaced").unwrap();
        assert_eq!(eq_spaced.host, "10.0.0.2");
        assert_eq!(eq_spaced.username, "admin");
    }

    #[test]
    fn parses_proxyjump_alias_as_jump_host_reference() {
        let hosts = parse_ssh_config_hosts(
            r#"
Host bastion
  HostName 198.51.100.10
  User ubuntu
  IdentityFile ~/.ssh/id_ed25519

Host app-prod
  HostName 10.0.0.15
  User deploy
  ProxyJump bastion
"#,
        );

        assert_eq!(hosts.len(), 2);
        let app = hosts.iter().find(|host| host.label == "app-prod").unwrap();
        assert_eq!(app.jump_host_id.as_deref(), Some("ssh-config-bastion"));
    }

    #[test]
    fn portable_data_bundle_round_trips_user_data() {
        let mut state = SavedState::default();
        state.settings = AppSettings {
            theme_preset: ThemePreset::Daylight,
            terminal_font_size: 16,
            ..AppSettings::default()
        };
        state.vaults.push(SavedVault {
            id: "vault-shared-ops".to_string(),
            label: "Ops".to_string(),
            description: "Ops vault".to_string(),
            kind: VaultKind::Shared,
            members: vec![crate::models::SavedVaultMember {
                id: "member-1".to_string(),
                name: "Alex".to_string(),
                email: "alex@example.com".to_string(),
                role: VaultMemberRole::Editor,
            }],
        });
        state.upsert_profile(HostProfile {
            id: "profile-prod".to_string(),
            label: "Prod".to_string(),
            vault_id: Some("vault-shared-ops".to_string()),
            favorite: true,
            group: "Production".to_string(),
            tags: vec!["critical".to_string()],
            host: "prod.example.com".to_string(),
            port: 22,
            username: "ubuntu".to_string(),
            auth_mode: AuthMode::Password,
            key_path: String::new(),
            certificate_path: None,
            identity_agent: None,
            identity_id: None,
            jump_host_id: None,
            outbound_proxy: None,
            startup_directory: Some("/srv/prod".to_string()),
            startup_command: Some("sudo systemctl status app".to_string()),
            start_in_files: true,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: Some(4096),
            port_forward_rules: Vec::new(),
            local_forwards: Vec::new(),
            local_forward: None,
            password_credential_id: Some("secret-ref".to_string()),
            source: ProfileSource::User,
            description: "Production app server".to_string(),
            color_tag: None,
            environment: Vec::new(),
        });
        state.upsert_identity(SavedIdentity {
            id: "identity-1".to_string(),
            label: "ops-key".to_string(),
            vault_id: Some("vault-shared-ops".to_string()),
            key_path: "/tmp/id_ed25519".to_string(),
            kind: "OpenSSH".to_string(),
            source: crate::models::IdentitySource::Imported,
        });
        state.upsert_snippet(SavedSnippet {
            id: "snippet-1".to_string(),
            label: "Restart".to_string(),
            vault_id: Some("vault-shared-ops".to_string()),
            group: "Ops".to_string(),
            pinned: true,
            command: "sudo systemctl restart app".to_string(),
        });

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let export_path = std::env::temp_dir().join(format!("termirust-portable-{}.json", suffix));
        let known_hosts_path =
            std::env::temp_dir().join(format!("termirust-known-hosts-{suffix}.json"));
        let known_hosts = KnownHostStore {
            path: known_hosts_path.clone(),
            entries: std::sync::Mutex::new(HashMap::from([(
                "prod.example.com:22".to_string(),
                "ssh-ed25519 AAAAC3Nza".to_string(),
            )])),
        };

        let report = export_portable_data_bundle(&export_path, &state, &known_hosts).unwrap();
        assert_eq!(report.vaults, 2);
        assert_eq!(report.profiles, 1);
        assert_eq!(report.identities, 1);
        assert_eq!(report.snippets, 1);
        assert_eq!(report.known_hosts, 1);

        let mut imported = SavedState::default();
        imported.settings.theme_preset = ThemePreset::Ocean;
        let imported_known_hosts_path =
            std::env::temp_dir().join(format!("termirust-known-hosts-import-{suffix}.json"));
        let imported_known_hosts = KnownHostStore {
            path: imported_known_hosts_path.clone(),
            entries: std::sync::Mutex::new(HashMap::new()),
        };
        let import_report =
            import_portable_data_bundle(&export_path, &mut imported, &imported_known_hosts)
                .unwrap();
        assert_eq!(import_report.profiles, 1);
        assert_eq!(import_report.known_hosts, 1);
        assert_eq!(imported.settings.theme_preset, ThemePreset::Ocean);
        assert!(
            imported
                .vaults
                .iter()
                .any(|vault| vault.id == "vault-shared-ops" && vault.kind == VaultKind::Shared)
        );
        assert_eq!(imported.profiles[0].password_credential_id, None);
        assert_eq!(imported.profiles[0].source, ProfileSource::User);
        assert_eq!(imported.profiles[0].tags, vec!["critical".to_string()]);
        assert_eq!(
            imported.identities[0].source,
            crate::models::IdentitySource::User
        );
        assert_eq!(imported.snippets[0].command, "sudo systemctl restart app");
        let persisted_known_hosts = imported_known_hosts.entries().unwrap();
        assert_eq!(persisted_known_hosts.len(), 1);
        assert_eq!(persisted_known_hosts[0].0, "prod.example.com:22");

        let _ = fs::remove_file(export_path);
        let _ = fs::remove_file(known_hosts_path);
        let _ = fs::remove_file(imported_known_hosts_path);
    }

    #[test]
    fn encrypted_portable_data_bundle_round_trips_user_data() {
        let mut state = SavedState::default();
        state.upsert_profile(HostProfile {
            id: "profile-prod".to_string(),
            label: "Prod".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            favorite: false,
            group: "Production".to_string(),
            tags: vec!["critical".to_string()],
            host: "prod.example.com".to_string(),
            port: 22,
            username: "ubuntu".to_string(),
            auth_mode: AuthMode::Password,
            key_path: String::new(),
            certificate_path: None,
            identity_agent: None,
            identity_id: None,
            jump_host_id: None,
            outbound_proxy: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: None,
            port_forward_rules: Vec::new(),
            local_forwards: Vec::new(),
            local_forward: None,
            password_credential_id: Some("secret-ref".to_string()),
            source: ProfileSource::User,
            description: String::new(),
            color_tag: None,
            environment: Vec::new(),
        });
        state.upsert_snippet(SavedSnippet {
            id: "snippet-1".to_string(),
            label: "Restart".to_string(),
            vault_id: Some(DEFAULT_VAULT_ID.to_string()),
            group: "Ops".to_string(),
            pinned: false,
            command: "sudo systemctl restart app".to_string(),
        });

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let export_path =
            std::env::temp_dir().join(format!("termirust-portable-encrypted-{suffix}.json"));
        let known_hosts_path =
            std::env::temp_dir().join(format!("termirust-known-hosts-encrypted-{suffix}.json"));
        let known_hosts = KnownHostStore {
            path: known_hosts_path.clone(),
            entries: std::sync::Mutex::new(HashMap::from([(
                "prod.example.com:22".to_string(),
                "ssh-ed25519 AAAAC3Nza".to_string(),
            )])),
        };

        let report =
            export_encrypted_portable_data_bundle(&export_path, &state, &known_hosts, "hunter2")
                .unwrap();
        assert_eq!(report.profiles, 1);
        assert_eq!(report.known_hosts, 1);

        let encrypted_file = fs::read_to_string(&export_path).unwrap();
        assert!(!encrypted_file.contains("prod.example.com"));
        assert!(!encrypted_file.contains("sudo systemctl restart app"));

        let imported_known_hosts_path = std::env::temp_dir().join(format!(
            "termirust-known-hosts-encrypted-import-{suffix}.json"
        ));
        let imported_known_hosts = KnownHostStore {
            path: imported_known_hosts_path.clone(),
            entries: std::sync::Mutex::new(HashMap::new()),
        };
        let mut imported = SavedState::default();
        let import_report = import_encrypted_portable_data_bundle(
            &export_path,
            &mut imported,
            &imported_known_hosts,
            "hunter2",
        )
        .unwrap();
        assert_eq!(import_report.profiles, 1);
        assert_eq!(import_report.snippets, 1);
        assert_eq!(imported.profiles[0].password_credential_id, None);
        assert_eq!(imported.snippets[0].command, "sudo systemctl restart app");
        assert_eq!(imported_known_hosts.entries().unwrap().len(), 1);

        let _ = fs::remove_file(export_path);
        let _ = fs::remove_file(known_hosts_path);
        let _ = fs::remove_file(imported_known_hosts_path);
    }

    #[test]
    fn encrypted_mobile_vault_export_contains_mobile_schema_without_plaintext() {
        let mut state = SavedState::default();
        state.settings.mobile_devices.push(MobileDeviceRecord {
            device_id: "ios-1".to_string(),
            label: "Jacob iPhone".to_string(),
            platform: Some("ios".to_string()),
            public_key: Some("x25519-public-key".to_string()),
            paired_at_millis: Some(1719356789000),
            last_seen_at_millis: Some(1719356789000),
            revoked_at_millis: None,
        });
        state.vaults.push(SavedVault {
            id: "vault-shared-ops".to_string(),
            label: "Ops".to_string(),
            description: "Operations vault".to_string(),
            kind: VaultKind::Shared,
            members: vec![crate::models::SavedVaultMember {
                id: "member-1".to_string(),
                name: "Jacob".to_string(),
                email: "jacob@example.com".to_string(),
                role: VaultMemberRole::Owner,
            }],
        });
        state.identities.push(SavedIdentity {
            id: "identity-prod".to_string(),
            label: "Prod key".to_string(),
            vault_id: Some("vault-shared-ops".to_string()),
            key_path: "/Users/jacob/.ssh/prod_ed25519".to_string(),
            kind: "ed25519".to_string(),
            source: crate::models::IdentitySource::User,
        });
        state.upsert_profile(HostProfile {
            id: "profile-prod".to_string(),
            label: "Prod".to_string(),
            vault_id: Some("vault-shared-ops".to_string()),
            favorite: false,
            group: "Production".to_string(),
            tags: vec!["critical".to_string(), "ssh".to_string()],
            host: "prod.example.com".to_string(),
            port: 2222,
            username: "ubuntu".to_string(),
            auth_mode: AuthMode::PrivateKey,
            key_path: "/Users/jacob/.ssh/prod_ed25519".to_string(),
            certificate_path: None,
            identity_agent: None,
            identity_id: Some("identity-prod".to_string()),
            jump_host_id: None,
            outbound_proxy: None,
            startup_directory: Some("/srv/app".to_string()),
            startup_command: Some("uptime".to_string()),
            start_in_files: false,
            persistent_session: true,
            persistent_session_name: Some("tr-prod".to_string()),
            persistent_session_detach_others: true,
            terminal_scrollback_rows: Some(20_000),
            port_forward_rules: Vec::new(),
            local_forwards: Vec::new(),
            local_forward: None,
            password_credential_id: Some("desktop-secret-ref".to_string()),
            source: ProfileSource::User,
            description: String::new(),
            color_tag: None,
            environment: vec![("APP_ENV".to_string(), "prod".to_string())],
        });

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let export_path =
            std::env::temp_dir().join(format!("termirust-mobile-vault-{suffix}.json"));
        let known_hosts_path =
            std::env::temp_dir().join(format!("termirust-mobile-known-hosts-{suffix}.json"));
        let known_hosts = KnownHostStore {
            path: known_hosts_path.clone(),
            entries: std::sync::Mutex::new(HashMap::from([(
                "prod.example.com:2222".to_string(),
                "ssh-ed25519 AAAAC3Nza".to_string(),
            )])),
        };

        let report = export_encrypted_mobile_vault(
            &export_path,
            &state,
            &known_hosts,
            "hunter2",
            "export-1",
            "desktop-1",
        )
        .unwrap();
        assert_eq!(report.vaults, 2);
        assert_eq!(report.hosts, 1);
        assert_eq!(report.identities, 1);
        assert_eq!(report.known_hosts, 1);

        let encrypted_file = fs::read_to_string(&export_path).unwrap();
        assert!(encrypted_file.contains("\"schema_version\": 1"));
        assert!(!encrypted_file.contains("prod.example.com"));
        assert!(!encrypted_file.contains("uptime"));
        assert!(!encrypted_file.contains("desktop-secret-ref"));
        assert!(!encrypted_file.contains("/Users/jacob/.ssh/prod_ed25519"));

        let mobile = read_encrypted_mobile_vault_export(&export_path, "hunter2")
            .expect("decrypt mobile vault");
        assert_eq!(mobile.schema_version, MOBILE_VAULT_SCHEMA_VERSION);
        assert_eq!(mobile.export_id, "export-1");
        assert_eq!(mobile.source_device_id, "desktop-1");
        assert_eq!(mobile.devices.len(), 2);
        let desktop_device = mobile
            .devices
            .iter()
            .find(|device| device.device_id == "desktop-1")
            .expect("desktop device should export");
        assert_eq!(desktop_device.label, "TermiRust Desktop");
        assert_eq!(desktop_device.platform.as_deref(), Some("desktop"));
        assert_eq!(desktop_device.revoked_at_millis, None);
        let ios_device = mobile
            .devices
            .iter()
            .find(|device| device.device_id == "ios-1")
            .expect("approved iOS device should export");
        assert_eq!(ios_device.label, "Jacob iPhone");
        assert_eq!(ios_device.platform.as_deref(), Some("ios"));
        assert_eq!(ios_device.public_key.as_deref(), Some("x25519-public-key"));
        assert_eq!(mobile.vaults[1].kind, "shared");
        assert_eq!(mobile.known_hosts[0].endpoint, "prod.example.com:2222");

        let host = &mobile.hosts[0];
        assert_eq!(host.auth.kind, MobileAuthKind::PrivateKey);
        assert_eq!(host.auth.identity_id.as_deref(), Some("identity-prod"));
        assert_eq!(
            host.auth.secret_ref.as_deref(),
            Some("termirust-mobile://identity/identity-prod/private-key")
        );
        assert!(host.persistent_session.enabled);
        assert_eq!(
            host.persistent_session.session_name.as_deref(),
            Some("tr-prod")
        );
        assert!(host.persistent_session.detach_others);
        assert_eq!(host.startup_directory.as_deref(), Some("/srv/app"));
        assert_eq!(host.startup_command.as_deref(), Some("uptime"));

        let _ = fs::remove_file(export_path);
        let _ = fs::remove_file(known_hosts_path);
    }

    #[test]
    fn encrypted_portable_data_bundle_rejects_wrong_passphrase() {
        let state = SavedState::default();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let export_path =
            std::env::temp_dir().join(format!("termirust-portable-encrypted-reject-{suffix}.json"));
        let known_hosts_path = std::env::temp_dir().join(format!(
            "termirust-known-hosts-encrypted-reject-{suffix}.json"
        ));
        let known_hosts = KnownHostStore {
            path: known_hosts_path.clone(),
            entries: std::sync::Mutex::new(HashMap::new()),
        };

        export_encrypted_portable_data_bundle(&export_path, &state, &known_hosts, "correct")
            .unwrap();

        let imported_known_hosts_path = std::env::temp_dir().join(format!(
            "termirust-known-hosts-encrypted-reject-import-{suffix}.json"
        ));
        let imported_known_hosts = KnownHostStore {
            path: imported_known_hosts_path.clone(),
            entries: std::sync::Mutex::new(HashMap::new()),
        };
        let mut imported = SavedState::default();
        let error = import_encrypted_portable_data_bundle(
            &export_path,
            &mut imported,
            &imported_known_hosts,
            "wrong",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Unable to decrypt encrypted bundle")
        );

        let _ = fs::remove_file(export_path);
        let _ = fs::remove_file(known_hosts_path);
        let _ = fs::remove_file(imported_known_hosts_path);
    }

    #[test]
    fn merge_known_host_entries_preserves_existing_items() {
        let path = std::env::temp_dir().join(format!(
            "termirust-known-host-merge-{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let store = KnownHostStore {
            path: path.clone(),
            entries: std::sync::Mutex::new(HashMap::from([(
                "existing.example.com:22".to_string(),
                "ssh-ed25519 existing".to_string(),
            )])),
        };

        let merged = store
            .merge_entries(HashMap::from([
                (
                    "existing.example.com:22".to_string(),
                    "ssh-ed25519 replacement".to_string(),
                ),
                (
                    "new.example.com:22".to_string(),
                    "ssh-ed25519 new".to_string(),
                ),
            ]))
            .unwrap();
        assert_eq!(merged, 1);

        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|(endpoint, key)| endpoint == "existing.example.com:22"
                    && key == "ssh-ed25519 existing")
        );

        let content = fs::read_to_string(&path).unwrap();
        let persisted: KnownHostsFile = serde_json::from_str(&content).unwrap();
        assert_eq!(persisted.entries.len(), 2);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn strict_known_host_verification_never_mutates_trust() {
        let path = std::env::temp_dir().join(format!(
            "termirust-known-host-strict-{}.json",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let store = KnownHostStore {
            path: path.clone(),
            entries: std::sync::Mutex::new(HashMap::from([(
                "known.example.com:22".to_string(),
                "ssh-ed25519 expected".to_string(),
            )])),
        };

        store
            .verify_existing("known.example.com:22", "ssh-ed25519 expected")
            .unwrap();
        let mismatch = store
            .verify_existing("known.example.com:22", "ssh-ed25519 changed")
            .unwrap_err();
        assert!(mismatch.to_string().contains("Host key mismatch"));
        let unknown = store
            .verify_existing("unknown.example.com:22", "ssh-ed25519 unknown")
            .unwrap_err();
        assert!(unknown.to_string().contains("Host key is not trusted"));
        assert_eq!(store.entries().unwrap().len(), 1);
        assert!(!path.exists());
    }
}
