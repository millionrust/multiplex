use anyhow::{Context as _, Result, bail};
use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

const MAX_PUBLIC_KEY_BYTES: u64 = 16 * 1024;
const MAX_AUTHORIZED_KEYS_BYTES: usize = 1024 * 1024;
const MAX_AUTHORIZED_KEYS_LINES: usize = 10_000;
const MAX_AUTHORIZED_KEY_LINE_BYTES: usize = 16 * 1024;
const MAX_KEY_COMMENT_CHARS: usize = 256;
const MAX_KEY_AUDIT_ENTRIES: usize = 200;
const MAX_KEY_AUDIT_FILE_BYTES: u64 = 256 * 1024;
const KEY_AUDIT_FILE: &str = "ssh-key-operations.json";

#[derive(Clone, PartialEq, Eq)]
pub struct GeneratedSshIdentity {
    pub private_key_path: PathBuf,
    pub public_key_path: PathBuf,
    pub public_key: String,
    pub fingerprint: String,
}

impl std::fmt::Debug for GeneratedSshIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GeneratedSshIdentity")
            .field("private_key_path", &"[REDACTED]")
            .field("public_key_path", &"[REDACTED]")
            .field("public_key", &"[PUBLIC KEY OMITTED]")
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PublicKeyMaterial {
    pub openssh: String,
    pub fingerprint: String,
    key: PublicKey,
}

impl std::fmt::Debug for PublicKeyMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicKeyMaterial")
            .field("openssh", &"[PUBLIC KEY OMITTED]")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl PublicKeyMaterial {
    pub fn parse(openssh: &str) -> Result<Self> {
        if openssh.len() as u64 > MAX_PUBLIC_KEY_BYTES {
            bail!("The SSH public key exceeds the safety limit");
        }
        let key = PublicKey::from_openssh(openssh.trim())
            .map_err(|_| anyhow::anyhow!("The SSH public key is malformed"))?;
        let openssh = key
            .to_openssh()
            .map_err(|_| anyhow::anyhow!("Unable to normalize the SSH public key"))?;
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        Ok(Self {
            openssh,
            fingerprint,
            key,
        })
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| anyhow::anyhow!("The SSH public key file is unavailable"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!("The SSH public key path is not a direct regular file");
        }
        if metadata.len() > MAX_PUBLIC_KEY_BYTES {
            bail!("The SSH public key exceeds the safety limit");
        }
        let bytes = fs::read(path)
            .map_err(|_| anyhow::anyhow!("Unable to read the SSH public key file"))?;
        let encoded = std::str::from_utf8(&bytes)
            .map_err(|_| anyhow::anyhow!("The SSH public key is not valid UTF-8"))?;
        Self::parse(encoded)
    }

    fn same_key(&self, other: &PublicKey) -> bool {
        self.key.key_data() == other.key_data()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorizedKeyMutation {
    AlreadyPresent,
    NotPresent,
    Changed(Vec<u8>),
    Cancelled,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SshKeyAuditEntry {
    pub recorded_at: u64,
    pub operation: String,
    pub result: String,
    pub fingerprint: String,
    pub host_id: String,
    pub endpoint: String,
    pub username: String,
}

pub fn record_ssh_key_audit(
    operation: &str,
    result: &str,
    fingerprint: &str,
    host_id: &str,
    endpoint: &str,
    username: &str,
) -> Result<()> {
    let entry = SshKeyAuditEntry {
        recorded_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        operation: bounded_audit_field(operation, 16)?,
        result: bounded_audit_field(result, 64)?,
        fingerprint: bounded_audit_field(fingerprint, 128)?,
        host_id: bounded_audit_field(host_id, 256)?,
        endpoint: bounded_audit_field(endpoint, 512)?,
        username: bounded_audit_field(username, 256)?,
    };
    let directory = crate::storage::app_dir()?;
    let path = directory.join(KEY_AUDIT_FILE);
    let mut entries = read_key_audit(&path)?;
    entries.push(entry);
    if entries.len() > MAX_KEY_AUDIT_ENTRIES {
        entries.drain(..entries.len() - MAX_KEY_AUDIT_ENTRIES);
    }
    let encoded = serde_json::to_vec_pretty(&entries)?;
    if encoded.len() as u64 > MAX_KEY_AUDIT_FILE_BYTES {
        bail!("The SSH key audit exceeds its safety limit");
    }
    let temporary = directory.join(format!(".{KEY_AUDIT_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    write_exclusive_file(&temporary, &encoded, 0o600)?;
    if fs::rename(&temporary, &path).is_err() {
        let _ = fs::remove_file(&temporary);
        bail!("Unable to save the SSH key audit");
    }
    set_file_mode(&path, 0o600)?;
    sync_directory(&directory)
}

fn bounded_audit_field(value: &str, max_chars: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max_chars || value.chars().any(char::is_control)
    {
        bail!("The SSH key audit metadata is invalid");
    }
    Ok(value.to_string())
}

fn read_key_audit(path: &Path) -> Result<Vec<SshKeyAuditEntry>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.len() > MAX_KEY_AUDIT_FILE_BYTES
            {
                bail!("The SSH key audit file has an unsafe type or size");
            }
            let bytes =
                fs::read(path).map_err(|_| anyhow::anyhow!("Unable to read the SSH key audit"))?;
            serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("The SSH key audit file is malformed"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => bail!("Unable to inspect the SSH key audit file"),
    }
}

pub fn generate_ed25519_key_pair(
    destination: &Path,
    comment: &str,
    passphrase: Option<&str>,
) -> Result<GeneratedSshIdentity> {
    validate_key_comment(comment)?;
    let parent = validate_key_destination(destination)?;
    let public_destination = public_key_path(destination)?;
    ensure_destination_absent(destination)?;
    ensure_destination_absent(&public_destination)?;

    let mut rng = rand::rngs::OsRng;
    let generated = PrivateKey::random(&mut rng, Algorithm::Ed25519)
        .map_err(|_| anyhow::anyhow!("Unable to generate an Ed25519 key"))?;
    let generated = PrivateKey::new(generated.key_data().clone(), comment)
        .map_err(|_| anyhow::anyhow!("Unable to prepare the generated SSH key"))?;
    let generated = match passphrase.filter(|value| !value.is_empty()) {
        Some(passphrase) => generated
            .encrypt(&mut rng, passphrase)
            .map_err(|_| anyhow::anyhow!("Unable to encrypt the generated SSH key"))?,
        None => generated,
    };
    let private_bytes = generated
        .to_openssh(LineEnding::LF)
        .map_err(|_| anyhow::anyhow!("Unable to encode the generated SSH key"))?;
    let mut public_key = generated.public_key().clone();
    public_key.set_comment(comment);
    let public_line = public_key
        .to_openssh()
        .map_err(|_| anyhow::anyhow!("Unable to encode the generated SSH public key"))?;
    let fingerprint = public_key.fingerprint(HashAlg::Sha256).to_string();

    let private_temp = temporary_peer_path(&parent, "private");
    let public_temp = temporary_peer_path(&parent, "public");
    let mut cleanup = LocalKeyCleanup::new(private_temp.clone(), public_temp.clone());

    write_exclusive_file(&private_temp, private_bytes.as_bytes(), 0o600)
        .context("Unable to stage the generated private key")?;
    write_exclusive_file(&public_temp, format!("{public_line}\n").as_bytes(), 0o644)
        .context("Unable to stage the generated public key")?;

    fs::hard_link(&private_temp, destination)
        .map_err(|_| anyhow::anyhow!("The private-key destination is no longer available"))?;
    cleanup.private_destination = Some(LinkedDestination::capture(destination)?);
    fs::hard_link(&public_temp, &public_destination)
        .map_err(|_| anyhow::anyhow!("The public-key destination is no longer available"))?;
    cleanup.public_destination = Some(LinkedDestination::capture(&public_destination)?);

    set_file_mode(destination, 0o600)?;
    set_file_mode(&public_destination, 0o644)?;
    cleanup.remove_temps();
    sync_directory(&parent)?;
    cleanup.committed = true;

    Ok(GeneratedSshIdentity {
        private_key_path: destination.to_path_buf(),
        public_key_path: public_destination,
        public_key: public_line,
        fingerprint,
    })
}

pub fn add_authorized_key(
    existing: &[u8],
    target: &PublicKeyMaterial,
) -> Result<AuthorizedKeyMutation> {
    validate_authorized_keys(existing)?;
    if authorized_keys_contains(existing, target) {
        return Ok(AuthorizedKeyMutation::AlreadyPresent);
    }

    let mut updated = Vec::with_capacity(existing.len() + target.openssh.len() + 2);
    updated.extend_from_slice(existing);
    if !updated.is_empty() && !updated.ends_with(b"\n") {
        updated.push(b'\n');
    }
    updated.extend_from_slice(target.openssh.as_bytes());
    updated.push(b'\n');
    Ok(AuthorizedKeyMutation::Changed(updated))
}

pub fn remove_authorized_key(
    existing: &[u8],
    target: &PublicKeyMaterial,
) -> Result<AuthorizedKeyMutation> {
    validate_authorized_keys(existing)?;
    let mut changed = false;
    let mut updated = Vec::with_capacity(existing.len());
    for raw_line in existing.split_inclusive(|byte| *byte == b'\n') {
        let line = raw_line.strip_suffix(b"\n").unwrap_or(raw_line);
        if parse_key_from_authorized_line(line).is_some_and(|key| target.same_key(&key)) {
            changed = true;
            continue;
        }
        updated.extend_from_slice(raw_line);
    }
    if changed {
        Ok(AuthorizedKeyMutation::Changed(updated))
    } else {
        Ok(AuthorizedKeyMutation::NotPresent)
    }
}

fn validate_key_comment(comment: &str) -> Result<()> {
    if comment.chars().count() > MAX_KEY_COMMENT_CHARS {
        bail!("The SSH key comment exceeds the 256-character safety limit");
    }
    if comment
        .chars()
        .any(|character| character.is_control() || matches!(character, '\n' | '\r'))
    {
        bail!("The SSH key comment contains unsupported control characters");
    }
    Ok(())
}

fn validate_key_destination(destination: &Path) -> Result<PathBuf> {
    if !destination.is_absolute() || destination.file_name().is_none() {
        bail!("Choose an absolute private-key file destination");
    }
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("The private-key destination has no parent folder"))?;
    let metadata = fs::symlink_metadata(parent)
        .map_err(|_| anyhow::anyhow!("The private-key destination folder is unavailable"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("The private-key destination folder must be a direct directory");
    }
    validate_local_owner(&metadata)?;
    Ok(parent.to_path_buf())
}

fn public_key_path(destination: &Path) -> Result<PathBuf> {
    let file_name = destination
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("The private-key destination has no file name"))?;
    let mut public_name = OsString::from(file_name);
    public_name.push(".pub");
    Ok(destination.with_file_name(public_name))
}

fn ensure_destination_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!("The SSH key destination already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => bail!("Unable to inspect the SSH key destination"),
    }
}

fn temporary_peer_path(parent: &Path, kind: &str) -> PathBuf {
    parent.join(format!(
        ".termirust-key-{kind}-{}.tmp",
        uuid::Uuid::new_v4()
    ))
}

fn write_exclusive_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(mode);
    }
    let mut file = options
        .open(path)
        .map_err(|_| anyhow::anyhow!("Unable to create a temporary SSH key file"))?;
    file.write_all(bytes)
        .map_err(|_| anyhow::anyhow!("Unable to write a temporary SSH key file"))?;
    file.sync_all()
        .map_err(|_| anyhow::anyhow!("Unable to sync a temporary SSH key file"))?;
    set_file_mode(path, mode)
}

#[cfg(unix)]
fn set_file_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|_| anyhow::anyhow!("Unable to secure the generated SSH key files"))
}

#[cfg(not(unix))]
fn set_file_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn validate_local_owner(metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    if metadata.uid() != unsafe { libc::geteuid() } {
        bail!("The private-key destination folder is owned by another user");
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_local_owner(_metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

fn sync_directory(parent: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| anyhow::anyhow!("Unable to sync the SSH key destination folder"))?;
    Ok(())
}

fn validate_authorized_keys(existing: &[u8]) -> Result<()> {
    if existing.len() > MAX_AUTHORIZED_KEYS_BYTES {
        bail!("The remote authorized_keys file exceeds the 1 MiB safety limit");
    }
    let mut lines = 0usize;
    for line in existing.split(|byte| *byte == b'\n') {
        lines += 1;
        if lines > MAX_AUTHORIZED_KEYS_LINES {
            bail!("The remote authorized_keys file exceeds the 10,000-line safety limit");
        }
        if line.len() > MAX_AUTHORIZED_KEY_LINE_BYTES {
            bail!("A remote authorized_keys line exceeds the 16 KiB safety limit");
        }
    }
    Ok(())
}

fn authorized_keys_contains(existing: &[u8], target: &PublicKeyMaterial) -> bool {
    existing
        .split(|byte| *byte == b'\n')
        .filter_map(parse_key_from_authorized_line)
        .any(|key| target.same_key(&key))
}

fn parse_key_from_authorized_line(line: &[u8]) -> Option<PublicKey> {
    let mut tokens = line
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|token| !token.is_empty())
        .peekable();
    while let Some(token) = tokens.next() {
        let algorithm = std::str::from_utf8(token).ok()?;
        if !(algorithm.starts_with("ssh-")
            || algorithm.starts_with("ecdsa-")
            || algorithm.starts_with("sk-"))
        {
            continue;
        }
        let encoded = std::str::from_utf8(tokens.peek().copied()?).ok()?;
        if let Ok(key) = PublicKey::from_openssh(&format!("{algorithm} {encoded}")) {
            return Some(key);
        }
    }
    None
}

struct LocalKeyCleanup {
    private_temp: PathBuf,
    public_temp: PathBuf,
    private_destination: Option<LinkedDestination>,
    public_destination: Option<LinkedDestination>,
    committed: bool,
}

struct LinkedDestination {
    path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl LinkedDestination {
    fn capture(path: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| anyhow::anyhow!("Unable to verify the generated SSH key file"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            Ok(Self {
                path: path.to_path_buf(),
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                path: path.to_path_buf(),
            })
        }
    }

    fn remove_if_unchanged(&self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let unchanged = fs::symlink_metadata(&self.path)
                .map(|metadata| metadata.dev() == self.device && metadata.ino() == self.inode)
                .unwrap_or(false);
            if unchanged {
                let _ = fs::remove_file(&self.path);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl LocalKeyCleanup {
    fn new(private_temp: PathBuf, public_temp: PathBuf) -> Self {
        Self {
            private_temp,
            public_temp,
            private_destination: None,
            public_destination: None,
            committed: false,
        }
    }

    fn remove_temps(&self) {
        let _ = fs::remove_file(&self.private_temp);
        let _ = fs::remove_file(&self.public_temp);
    }
}

impl Drop for LocalKeyCleanup {
    fn drop(&mut self) {
        self.remove_temps();
        if self.committed {
            return;
        }
        if let Some(destination) = self.public_destination.as_ref() {
            destination.remove_if_unchanged();
        }
        if let Some(destination) = self.private_destination.as_ref() {
            destination.remove_if_unchanged();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorizedKeyMutation, PublicKeyMaterial, add_authorized_key, generate_ed25519_key_pair,
        record_ssh_key_audit, remove_authorized_key,
    };
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn generates_encrypted_openssh_ed25519_pair_with_strict_permissions() {
        let directory = TempDir::new().unwrap();
        let private_path = directory.path().join("generated_ed25519");
        let generated =
            generate_ed25519_key_pair(&private_path, "deploy@example.com", Some("test-passphrase"))
                .unwrap();

        let private = russh::keys::load_secret_key(&private_path, Some("test-passphrase")).unwrap();
        let public = PublicKeyMaterial::from_file(&generated.public_key_path).unwrap();
        assert_eq!(private.algorithm(), russh::keys::Algorithm::Ed25519);
        assert_eq!(public.fingerprint, generated.fingerprint);
        assert_eq!(public.openssh, generated.public_key);
        assert!(format!("{generated:?}").contains("[REDACTED]"));
        assert!(!format!("{generated:?}").contains(private_path.to_string_lossy().as_ref()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&private_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&generated.public_key_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o644
            );
        }
    }

    #[test]
    fn generated_unencrypted_key_is_accepted_by_openssh_tooling() {
        if Command::new("ssh-keygen").arg("-V").output().is_err() {
            eprintln!("skipping OpenSSH compatibility check: ssh-keygen is unavailable");
            return;
        }
        let directory = TempDir::new().unwrap();
        let private_path = directory.path().join("openssh-compatible");
        let generated = generate_ed25519_key_pair(&private_path, "openssh-check", None).unwrap();
        let public_path = generated.public_key_path.clone();
        let expected_fingerprint = generated.fingerprint.clone();

        let derived = Command::new("ssh-keygen")
            .args(["-y", "-f"])
            .arg(&private_path)
            .output()
            .expect("ssh-keygen should inspect the generated private key");
        assert!(derived.status.success());
        let derived =
            PublicKeyMaterial::parse(&String::from_utf8(derived.stdout).unwrap()).unwrap();
        let generated_material = PublicKeyMaterial::parse(&generated.public_key).unwrap();
        assert!(generated_material.same_key(&derived.key));

        let fingerprint = Command::new("ssh-keygen")
            .args(["-l", "-f"])
            .arg(public_path)
            .output()
            .expect("ssh-keygen should inspect the generated public key");
        assert!(fingerprint.status.success());
        assert!(String::from_utf8_lossy(&fingerprint.stdout).contains(&expected_fingerprint));
    }

    #[test]
    fn generation_never_overwrites_private_or_public_collisions() {
        let directory = TempDir::new().unwrap();
        let private_path = directory.path().join("collision");
        fs::write(&private_path, "existing-private").unwrap();
        assert!(generate_ed25519_key_pair(&private_path, "", None).is_err());
        assert_eq!(
            fs::read_to_string(&private_path).unwrap(),
            "existing-private"
        );

        fs::remove_file(&private_path).unwrap();
        fs::write(
            private_path.with_file_name("collision.pub"),
            "existing-public",
        )
        .unwrap();
        assert!(generate_ed25519_key_pair(&private_path, "", None).is_err());
        assert!(!private_path.exists());
        assert_eq!(
            fs::read_to_string(private_path.with_file_name("collision.pub")).unwrap(),
            "existing-public"
        );
    }

    #[cfg(unix)]
    #[test]
    fn generation_rejects_symlink_destination_folder() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let real = directory.path().join("real");
        let linked = directory.path().join("linked");
        fs::create_dir(&real).unwrap();
        symlink(&real, &linked).unwrap();
        let error = generate_ed25519_key_pair(&linked.join("id_ed25519"), "", None).unwrap_err();
        assert!(error.to_string().contains("direct directory"));
        assert!(!real.join("id_ed25519").exists());
    }

    #[test]
    fn authorized_key_mutation_is_exact_idempotent_and_preserves_unrelated_bytes() {
        let directory = TempDir::new().unwrap();
        let first_path = directory.path().join("first");
        let second_path = directory.path().join("second");
        let first = generate_ed25519_key_pair(&first_path, "first", None).unwrap();
        let second = generate_ed25519_key_pair(&second_path, "second", None).unwrap();
        let first = PublicKeyMaterial::parse(&first.public_key).unwrap();
        let second = PublicKeyMaterial::parse(&second.public_key).unwrap();
        let hostile_prefix = b"# preserve exactly\ncommand=\"printf spaced value\" ssh-invalid not-base64 comment\xff\n";

        let AuthorizedKeyMutation::Changed(once) =
            add_authorized_key(hostile_prefix, &first).unwrap()
        else {
            panic!("first add should change content");
        };
        assert!(once.starts_with(hostile_prefix));
        assert_eq!(
            add_authorized_key(&once, &first).unwrap(),
            AuthorizedKeyMutation::AlreadyPresent
        );

        let AuthorizedKeyMutation::Changed(twice) = add_authorized_key(&once, &second).unwrap()
        else {
            panic!("second add should change content");
        };
        let AuthorizedKeyMutation::Changed(removed) =
            remove_authorized_key(&twice, &first).unwrap()
        else {
            panic!("exact removal should change content");
        };
        assert!(removed.starts_with(hostile_prefix));
        assert_eq!(
            remove_authorized_key(&removed, &first).unwrap(),
            AuthorizedKeyMutation::NotPresent
        );
        assert_eq!(
            add_authorized_key(&removed, &second).unwrap(),
            AuthorizedKeyMutation::AlreadyPresent
        );
    }

    #[test]
    fn key_comment_and_authorized_keys_limits_fail_closed() {
        let directory = TempDir::new().unwrap();
        let private_path = directory.path().join("bad-comment");
        assert!(generate_ed25519_key_pair(&private_path, "bad\ncomment", None).is_err());
        assert!(!private_path.exists());

        let key_path = directory.path().join("valid");
        let key = generate_ed25519_key_pair(&key_path, "valid", None).unwrap();
        let key = PublicKeyMaterial::parse(&key.public_key).unwrap();
        let oversized = vec![b'x'; 1024 * 1024 + 1];
        assert!(add_authorized_key(&oversized, &key).is_err());
    }

    #[test]
    fn key_audit_is_bounded_and_contains_only_redacted_metadata() {
        let _isolation = crate::test_support::TestIsolation::acquire();
        record_ssh_key_audit(
            "install",
            "installed_and_verified",
            "SHA256:test-fingerprint",
            "profile-test",
            "example.test:22",
            "deploy",
        )
        .unwrap();
        let audit = fs::read_to_string(
            crate::storage::app_dir()
                .unwrap()
                .join("ssh-key-operations.json"),
        )
        .unwrap();
        assert!(audit.contains("SHA256:test-fingerprint"));
        assert!(audit.contains("example.test:22"));
        assert!(!audit.contains("PRIVATE KEY"));
        assert!(!audit.contains("passphrase"));
        assert!(!audit.contains("authorized_keys"));
    }
}
