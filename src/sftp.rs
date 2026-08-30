use anyhow::{Context, Result, anyhow};
use russh::keys::PublicKey;
use russh::{ChannelMsg, client};
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpClientError;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Builder;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize;

use crate::models::{AuthConfig, ConnectRequest, JumpHostConnection, OutboundProxy};
use crate::ssh_keys::{
    AuthorizedKeyMutation, PublicKeyMaterial, add_authorized_key, remove_authorized_key,
};
use crate::storage::{HostKeyDecision, KnownHostStore};

const AUTHORIZED_KEYS_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);
const AUTHORIZED_KEYS_STEP_TIMEOUT: Duration = Duration::from_secs(8);
const AUTHORIZED_KEYS_STALE_LOCK_SECS: u64 = 120;
const AUTHORIZED_KEYS_MAX_REMOTE_OUTPUT: usize = 4096;

#[derive(Clone, Debug)]
pub struct RemoteFileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: Option<u64>,
}

#[derive(Debug)]
pub enum SftpEvent {
    DirectoryLoaded {
        workspace_id: u64,
        operation_id: u64,
        path: String,
        entries: Vec<RemoteFileEntry>,
    },
    UploadComplete {
        workspace_id: u64,
        operation_id: u64,
        remote_path: String,
    },
    DownloadComplete {
        workspace_id: u64,
        operation_id: u64,
        remote_path: String,
        local_path: String,
    },
    DeleteComplete {
        workspace_id: u64,
        operation_id: u64,
        remote_path: String,
    },
    Error {
        workspace_id: u64,
        operation_id: u64,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizedKeyAction {
    Add,
    Remove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizedKeyOutcome {
    InstalledAndVerified,
    AlreadyPresentAndVerified,
    InstalledVerificationFailed,
    AlreadyPresentVerificationFailed,
    Removed,
    NotPresent,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorizedKeyEvent {
    Complete {
        operation_id: u64,
        fingerprint: String,
        outcome: AuthorizedKeyOutcome,
    },
    Error {
        operation_id: u64,
        fingerprint: String,
        message: String,
    },
}

#[derive(Clone)]
pub struct AuthorizedKeyControl {
    cancellation: CancellationToken,
}

impl AuthorizedKeyControl {
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
}

impl std::fmt::Debug for AuthorizedKeyControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorizedKeyControl")
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct GeneratedKeyVerification {
    private_key_path: PathBuf,
    passphrase: Option<String>,
}

impl GeneratedKeyVerification {
    pub fn new(private_key_path: PathBuf, passphrase: Option<String>) -> Self {
        Self {
            private_key_path,
            passphrase,
        }
    }
}

impl std::fmt::Debug for GeneratedKeyVerification {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GeneratedKeyVerification")
            .field("private_key_path", &"[REDACTED]")
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl Drop for GeneratedKeyVerification {
    fn drop(&mut self) {
        if let Some(passphrase) = self.passphrase.as_mut() {
            passphrase.zeroize();
        }
    }
}

pub fn spawn_authorized_key_operation(
    operation_id: u64,
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    public_key: PublicKeyMaterial,
    action: AuthorizedKeyAction,
    verification: Option<GeneratedKeyVerification>,
) -> Result<(AuthorizedKeyControl, Receiver<AuthorizedKeyEvent>)> {
    if action == AuthorizedKeyAction::Add && verification.is_none() {
        anyhow::bail!("Adding an SSH public key requires fresh-key verification");
    }
    let cancellation = CancellationToken::new();
    let control = AuthorizedKeyControl {
        cancellation: cancellation.clone(),
    };
    let (event_tx, event_rx) = channel();
    thread::Builder::new()
        .name(format!("authorized-key-{operation_id}"))
        .spawn(move || {
            let runtime = match Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => {
                    let _ = event_tx.send(AuthorizedKeyEvent::Error {
                        operation_id,
                        fingerprint: public_key.fingerprint.clone(),
                        message: "Unable to initialize the SSH key deployment runtime".to_string(),
                    });
                    return;
                }
            };
            let fingerprint = public_key.fingerprint.clone();
            let result = runtime.block_on(run_authorized_key_operation(
                request,
                known_hosts,
                public_key,
                action,
                verification,
                cancellation,
            ));
            let event = match result {
                Ok(outcome) => AuthorizedKeyEvent::Complete {
                    operation_id,
                    fingerprint,
                    outcome,
                },
                Err(error) => AuthorizedKeyEvent::Error {
                    operation_id,
                    fingerprint,
                    message: format!("{error:#}"),
                },
            };
            let _ = event_tx.send(event);
        })
        .context("Unable to start the SSH public-key operation")?;
    Ok((control, event_rx))
}

pub fn spawn_list_directory(
    workspace_id: u64,
    operation_id: u64,
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    path: String,
    event_tx: Sender<SftpEvent>,
) {
    spawn_operation(workspace_id, operation_id, event_tx, async move {
        let (path, entries) = with_sftp(request, known_hosts, move |sftp| async move {
            let canonical_path = canonical_remote_path(&sftp, &path).await?;
            let mut entries = sftp
                .read_dir(canonical_path.clone())
                .await
                .with_context(|| format!("Unable to read remote directory {canonical_path}"))?
                .filter_map(|entry| {
                    let name = entry.file_name();
                    if name == "." || name == ".." {
                        return None;
                    }

                    let file_type = entry.file_type();
                    let metadata = entry.metadata();
                    Some(RemoteFileEntry {
                        path: join_remote_path(&canonical_path, &name),
                        name,
                        is_dir: file_type.is_dir(),
                        is_symlink: file_type.is_symlink(),
                        size: Some(metadata.len()),
                    })
                })
                .collect::<Vec<_>>();

            entries.sort_by(|left, right| {
                right.is_dir.cmp(&left.is_dir).then_with(|| {
                    left.name
                        .to_ascii_lowercase()
                        .cmp(&right.name.to_ascii_lowercase())
                })
            });

            Ok((canonical_path, entries))
        })
        .await?;

        Ok(SftpEvent::DirectoryLoaded {
            workspace_id,
            operation_id,
            path,
            entries,
        })
    });
}

pub fn spawn_upload_file(
    workspace_id: u64,
    operation_id: u64,
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    remote_dir: String,
    local_path: PathBuf,
    event_tx: Sender<SftpEvent>,
) {
    spawn_operation(workspace_id, operation_id, event_tx, async move {
        let file_name = local_path
            .file_name()
            .map(|name| name.to_string_lossy().trim().to_string())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow!("Unable to determine a file name for upload"))?;
        let remote_path = join_remote_path(&remote_dir, &file_name);
        let bytes = fs::read(&local_path)
            .with_context(|| format!("Unable to read {}", local_path.display()))?;

        with_sftp(request, known_hosts, move |sftp| async move {
            let mut file = sftp
                .create(remote_path.clone())
                .await
                .with_context(|| format!("Unable to create remote file {remote_path}"))?;
            tokio::io::AsyncWriteExt::write_all(&mut file, &bytes)
                .await
                .with_context(|| format!("Unable to upload to {remote_path}"))?;
            Ok(remote_path)
        })
        .await
        .map(|remote_path| SftpEvent::UploadComplete {
            workspace_id,
            operation_id,
            remote_path,
        })
    });
}

pub fn spawn_download_file(
    workspace_id: u64,
    operation_id: u64,
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    remote_path: String,
    local_path: PathBuf,
    event_tx: Sender<SftpEvent>,
) {
    spawn_operation(workspace_id, operation_id, event_tx, async move {
        let display_local_path = local_path.display().to_string();
        let remote_path_for_read = remote_path.clone();
        let bytes = with_sftp(request, known_hosts, move |sftp| async move {
            sftp.read(remote_path_for_read.clone())
                .await
                .with_context(|| format!("Unable to read remote file {remote_path_for_read}"))
        })
        .await?;

        fs::write(&local_path, bytes)
            .with_context(|| format!("Unable to write {}", local_path.display()))?;

        Ok(SftpEvent::DownloadComplete {
            workspace_id,
            operation_id,
            remote_path,
            local_path: display_local_path,
        })
    });
}

pub fn spawn_delete_path(
    workspace_id: u64,
    operation_id: u64,
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    remote_path: String,
    is_dir: bool,
    event_tx: Sender<SftpEvent>,
) {
    spawn_operation(workspace_id, operation_id, event_tx, async move {
        with_sftp(request, known_hosts, move |sftp| async move {
            if is_dir {
                sftp.remove_dir(remote_path.clone())
                    .await
                    .with_context(|| format!("Unable to delete remote folder {remote_path}"))?;
            } else {
                sftp.remove_file(remote_path.clone())
                    .await
                    .with_context(|| format!("Unable to delete remote file {remote_path}"))?;
            }

            Ok(remote_path)
        })
        .await
        .map(|remote_path| SftpEvent::DeleteComplete {
            workspace_id,
            operation_id,
            remote_path,
        })
    });
}

fn spawn_operation<F>(workspace_id: u64, operation_id: u64, event_tx: Sender<SftpEvent>, future: F)
where
    F: std::future::Future<Output = Result<SftpEvent>> + Send + 'static,
{
    let thread_name = format!("sftp-{workspace_id}-{operation_id}");
    thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let runtime = match Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = event_tx.send(SftpEvent::Error {
                        workspace_id,
                        operation_id,
                        message: format!("Failed to build async runtime: {error}"),
                    });
                    return;
                }
            };

            let result = runtime.block_on(future);
            match result {
                Ok(event) => {
                    let _ = event_tx.send(event);
                }
                Err(error) => {
                    let _ = event_tx.send(SftpEvent::Error {
                        workspace_id,
                        operation_id,
                        message: format!("{error:#}"),
                    });
                }
            }
        })
        .expect("unable to spawn SFTP thread");
}

async fn run_authorized_key_operation(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    public_key: PublicKeyMaterial,
    action: AuthorizedKeyAction,
    verification: Option<GeneratedKeyVerification>,
    cancellation: CancellationToken,
) -> Result<AuthorizedKeyOutcome> {
    if cancellation.is_cancelled() {
        return Ok(AuthorizedKeyOutcome::Cancelled);
    }
    let (sftp, handles) = timeout(
        AUTHORIZED_KEYS_CONNECT_TIMEOUT,
        open_sftp(request.clone(), known_hosts.clone()),
    )
    .await
    .map_err(|_| anyhow!("The SSH key deployment connection timed out"))??;
    sftp.set_timeout(AUTHORIZED_KEYS_STEP_TIMEOUT.as_secs())
        .await;
    if cancellation.is_cancelled() {
        let _ = sftp.close().await;
        return Ok(AuthorizedKeyOutcome::Cancelled);
    }
    let mutation = mutate_authorized_keys(
        &sftp,
        &handles.target_handle,
        &public_key,
        action,
        &cancellation,
    )
    .await?;
    sftp.close()
        .await
        .map_err(|_| anyhow!("Unable to close the SSH key deployment channel"))?;
    drop(handles);

    match (action, mutation) {
        (_, AuthorizedKeyMutation::Cancelled) => Ok(AuthorizedKeyOutcome::Cancelled),
        (AuthorizedKeyAction::Remove, AuthorizedKeyMutation::Changed(_)) => {
            Ok(AuthorizedKeyOutcome::Removed)
        }
        (AuthorizedKeyAction::Remove, AuthorizedKeyMutation::NotPresent) => {
            Ok(AuthorizedKeyOutcome::NotPresent)
        }
        (AuthorizedKeyAction::Add, AuthorizedKeyMutation::Changed(_)) => {
            let verified = verify_generated_key(request, known_hosts, verification.unwrap()).await;
            Ok(if verified {
                AuthorizedKeyOutcome::InstalledAndVerified
            } else {
                AuthorizedKeyOutcome::InstalledVerificationFailed
            })
        }
        (AuthorizedKeyAction::Add, AuthorizedKeyMutation::AlreadyPresent) => {
            let verified = verify_generated_key(request, known_hosts, verification.unwrap()).await;
            Ok(if verified {
                AuthorizedKeyOutcome::AlreadyPresentAndVerified
            } else {
                AuthorizedKeyOutcome::AlreadyPresentVerificationFailed
            })
        }
        _ => bail_invalid_authorized_key_outcome(),
    }
}

fn bail_invalid_authorized_key_outcome<T>() -> Result<T> {
    anyhow::bail!("The SSH key operation produced an invalid state")
}

async fn verify_generated_key(
    mut request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    verification: GeneratedKeyVerification,
) -> bool {
    request.session_id = request.session_id.saturating_add(1);
    request.auth = Some(AuthConfig::PrivateKey {
        key_path: verification.private_key_path.display().to_string(),
        passphrase: verification.passphrase.clone(),
    });
    let result = timeout(AUTHORIZED_KEYS_STEP_TIMEOUT, async move {
        let (sftp, _handles) = open_sftp(request, known_hosts).await?;
        sftp.set_timeout(AUTHORIZED_KEYS_STEP_TIMEOUT.as_secs())
            .await;
        sftp.canonicalize(".".to_string())
            .await
            .map_err(|_| anyhow!("Fresh generated-key verification failed"))?;
        sftp.close()
            .await
            .map_err(|_| anyhow!("Fresh generated-key verification failed"))?;
        Result::<()>::Ok(())
    })
    .await;
    matches!(result, Ok(Ok(())))
}

async fn mutate_authorized_keys(
    sftp: &SftpSession,
    handle: &client::Handle<SftpHandler>,
    public_key: &PublicKeyMaterial,
    action: AuthorizedKeyAction,
    cancellation: &CancellationToken,
) -> Result<AuthorizedKeyMutation> {
    let home = sftp
        .canonicalize(".".to_string())
        .await
        .map_err(|_| anyhow!("Unable to resolve the remote home directory"))?;
    validate_remote_path(&home)?;
    let home_metadata = sftp
        .symlink_metadata(home.clone())
        .await
        .map_err(|_| anyhow!("Unable to inspect the remote home directory"))?;
    if !home_metadata.is_dir() || home_metadata.is_symlink() {
        anyhow::bail!("The remote home path is not a direct directory");
    }
    let owner = home_metadata
        .uid
        .ok_or_else(|| anyhow!("The remote server did not report home ownership"))?;
    let ssh_dir = join_remote_path(&home, ".ssh");
    ensure_remote_ssh_directory(sftp, &ssh_dir, owner).await?;
    let lock_path = join_remote_path(&ssh_dir, ".termirust-authorized-keys.lock");
    acquire_authorized_keys_lock(sftp, &lock_path, owner).await?;

    let result = mutate_authorized_keys_under_lock(
        sftp,
        handle,
        &ssh_dir,
        owner,
        public_key,
        action,
        cancellation,
    )
    .await;
    let cleanup = sftp.remove_dir(lock_path).await;
    match (result, cleanup) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Ok(_), Err(_)) => anyhow::bail!("Unable to release the remote authorized_keys lock"),
        (Err(error), _) => Err(error),
    }
}

async fn mutate_authorized_keys_under_lock(
    sftp: &SftpSession,
    handle: &client::Handle<SftpHandler>,
    ssh_dir: &str,
    owner: u32,
    public_key: &PublicKeyMaterial,
    action: AuthorizedKeyAction,
    cancellation: &CancellationToken,
) -> Result<AuthorizedKeyMutation> {
    if cancellation.is_cancelled() {
        return Ok(AuthorizedKeyMutation::Cancelled);
    }
    let authorized_keys = join_remote_path(ssh_dir, "authorized_keys");
    let existing = match remote_symlink_metadata(sftp, &authorized_keys).await? {
        Some(metadata) => {
            validate_remote_authorized_keys_metadata(&metadata, owner)?;
            let bytes = sftp
                .read(authorized_keys.clone())
                .await
                .map_err(|_| anyhow!("Unable to read remote authorized_keys"))?;
            if bytes.len() > 1024 * 1024 {
                anyhow::bail!("The remote authorized_keys file exceeds the 1 MiB safety limit");
            }
            bytes
        }
        None => Vec::new(),
    };
    let mutation = match action {
        AuthorizedKeyAction::Add => add_authorized_key(&existing, public_key)?,
        AuthorizedKeyAction::Remove => remove_authorized_key(&existing, public_key)?,
    };
    let AuthorizedKeyMutation::Changed(updated) = &mutation else {
        return Ok(mutation);
    };
    if cancellation.is_cancelled() {
        return Ok(AuthorizedKeyMutation::Cancelled);
    }

    let temporary = join_remote_path(
        ssh_dir,
        &format!(".authorized_keys.termirust-{}.tmp", uuid::Uuid::new_v4()),
    );
    let attributes = FileAttributes {
        permissions: Some(0o600),
        ..FileAttributes::empty()
    };
    let mut file = sftp
        .open_with_flags_and_attributes(
            temporary.clone(),
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
            attributes,
        )
        .await
        .map_err(|_| anyhow!("Unable to create a remote authorized_keys staging file"))?;
    use tokio::io::AsyncWriteExt as _;
    if file.write_all(updated).await.is_err() {
        let _ = sftp.remove_file(temporary).await;
        anyhow::bail!("Unable to write the remote authorized_keys staging file");
    }
    if file.sync_all().await.is_err() || file.shutdown().await.is_err() {
        let _ = sftp.remove_file(temporary).await;
        anyhow::bail!("Unable to sync the remote authorized_keys staging file");
    }
    if cancellation.is_cancelled() {
        let _ = sftp.remove_file(temporary).await;
        return Ok(AuthorizedKeyMutation::Cancelled);
    }
    if let Err(error) = atomic_remote_replace(handle, &temporary, &authorized_keys).await {
        let _ = sftp.remove_file(temporary).await;
        return Err(error);
    }
    let metadata = sftp
        .symlink_metadata(authorized_keys.clone())
        .await
        .map_err(|_| anyhow!("Unable to verify the updated remote authorized_keys"))?;
    validate_remote_authorized_keys_metadata(&metadata, owner)?;
    sftp.set_metadata(
        authorized_keys,
        FileAttributes {
            permissions: Some(0o600),
            ..FileAttributes::empty()
        },
    )
    .await
    .map_err(|_| anyhow!("Unable to secure the remote authorized_keys permissions"))?;
    Ok(mutation)
}

async fn ensure_remote_ssh_directory(sftp: &SftpSession, ssh_dir: &str, owner: u32) -> Result<()> {
    let metadata = match remote_symlink_metadata(sftp, ssh_dir).await? {
        Some(metadata) => metadata,
        None => {
            sftp.create_dir(ssh_dir.to_string())
                .await
                .map_err(|_| anyhow!("Unable to create the remote SSH directory"))?;
            sftp.symlink_metadata(ssh_dir.to_string())
                .await
                .map_err(|_| anyhow!("Unable to inspect the remote SSH directory"))?
        }
    };
    if metadata.is_symlink() || !metadata.is_dir() || metadata.uid != Some(owner) {
        anyhow::bail!("The remote SSH directory has an unsafe type or owner");
    }
    sftp.set_metadata(
        ssh_dir.to_string(),
        FileAttributes {
            permissions: Some(0o700),
            ..FileAttributes::empty()
        },
    )
    .await
    .map_err(|_| anyhow!("Unable to secure the remote SSH directory permissions"))
}

async fn acquire_authorized_keys_lock(
    sftp: &SftpSession,
    lock_path: &str,
    owner: u32,
) -> Result<()> {
    match sftp.create_dir(lock_path.to_string()).await {
        Ok(()) => {}
        Err(_) => {
            let metadata = remote_symlink_metadata(sftp, lock_path)
                .await?
                .ok_or_else(|| anyhow!("The remote authorized_keys lock is busy"))?;
            if metadata.is_symlink() || !metadata.is_dir() || metadata.uid != Some(owner) {
                anyhow::bail!("The remote authorized_keys lock has an unsafe type or owner");
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let modified = u64::from(metadata.mtime.unwrap_or_default());
            if now.saturating_sub(modified) < AUTHORIZED_KEYS_STALE_LOCK_SECS {
                anyhow::bail!("Another authorized_keys operation is already in progress");
            }
            sftp.remove_dir(lock_path.to_string())
                .await
                .map_err(|_| anyhow!("Unable to recover a stale authorized_keys lock"))?;
            sftp.create_dir(lock_path.to_string())
                .await
                .map_err(|_| anyhow!("Another authorized_keys operation is already in progress"))?;
        }
    }
    sftp.set_metadata(
        lock_path.to_string(),
        FileAttributes {
            permissions: Some(0o700),
            ..FileAttributes::empty()
        },
    )
    .await
    .map_err(|_| anyhow!("Unable to secure the remote authorized_keys lock"))
}

async fn remote_symlink_metadata(sftp: &SftpSession, path: &str) -> Result<Option<FileAttributes>> {
    match sftp.symlink_metadata(path.to_string()).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(SftpClientError::Status(status)) if status.status_code == StatusCode::NoSuchFile => {
            Ok(None)
        }
        Err(_) => Err(anyhow!("Unable to inspect a remote SSH key path")),
    }
}

fn validate_remote_authorized_keys_metadata(metadata: &FileAttributes, owner: u32) -> Result<()> {
    if metadata.is_symlink() || !metadata.is_regular() || metadata.uid != Some(owner) {
        anyhow::bail!("The remote authorized_keys file has an unsafe type or owner");
    }
    if metadata.len() > 1024 * 1024 {
        anyhow::bail!("The remote authorized_keys file exceeds the 1 MiB safety limit");
    }
    Ok(())
}

fn validate_remote_path(path: &str) -> Result<()> {
    if path.len() > 4096 || path.chars().any(char::is_control) || !path.starts_with('/') {
        anyhow::bail!("The remote home path is outside the supported safety policy");
    }
    Ok(())
}

async fn atomic_remote_replace(
    handle: &client::Handle<SftpHandler>,
    temporary: &str,
    destination: &str,
) -> Result<()> {
    let command = format!(
        "command mv -f {} {}",
        shell_single_quote(temporary),
        shell_single_quote(destination)
    );
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|_| anyhow!("Unable to open the atomic authorized_keys replacement channel"))?;
    channel
        .exec(true, command)
        .await
        .map_err(|_| anyhow!("Unable to start the atomic authorized_keys replacement"))?;
    let mut output_bytes = 0usize;
    let mut exit_status = None;
    loop {
        let message = timeout(AUTHORIZED_KEYS_STEP_TIMEOUT, channel.wait())
            .await
            .map_err(|_| anyhow!("The atomic authorized_keys replacement timed out"))?;
        match message {
            Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                output_bytes = output_bytes.saturating_add(data.len());
                if output_bytes > AUTHORIZED_KEYS_MAX_REMOTE_OUTPUT {
                    anyhow::bail!(
                        "The atomic authorized_keys replacement exceeded its output limit"
                    );
                }
            }
            Some(ChannelMsg::ExitStatus {
                exit_status: status,
            }) => exit_status = Some(status),
            Some(ChannelMsg::Close) | None => break,
            Some(_) => {}
        }
    }
    if exit_status != Some(0) {
        anyhow::bail!("The atomic authorized_keys replacement was rejected");
    }
    Ok(())
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn with_sftp<T, F, Fut>(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    action: F,
) -> Result<T>
where
    F: FnOnce(SftpSession) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let (sftp, _handles) = open_sftp(request, known_hosts).await?;
    action(sftp).await
}

async fn open_sftp(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
) -> Result<(SftpSession, EstablishedHandles)> {
    let config = Arc::new(client::Config::default());
    let handles = establish_handles(config, request, known_hosts).await?;
    let channel = handles
        .target_handle
        .channel_open_session()
        .await
        .context("Unable to open an SSH session channel for SFTP")?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .context("Unable to start the SFTP subsystem")?;

    let sftp = SftpSession::new(channel.into_stream())
        .await
        .context("Unable to initialize the SFTP session")?;
    Ok((sftp, handles))
}

async fn canonical_remote_path(sftp: &SftpSession, path: &str) -> Result<String> {
    let requested = if path.trim().is_empty() {
        "."
    } else {
        path.trim()
    };
    sftp.canonicalize(requested.to_string())
        .await
        .with_context(|| format!("Unable to resolve remote path {requested}"))
}

fn join_remote_path(base: &str, name: &str) -> String {
    if base == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", base.trim_end_matches('/'), name)
    }
}

struct EstablishedHandles {
    target_handle: client::Handle<SftpHandler>,
    _jump_handles: Vec<client::Handle<SftpHandler>>,
}

async fn establish_handles(
    config: Arc<client::Config>,
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
) -> Result<EstablishedHandles> {
    let auth = request
        .auth
        .clone()
        .context("SFTP session is missing authentication settings")?;
    let (target_handle, jump_handles) = if let Some(jump_host) = request.jump_host.clone() {
        connect_via_jump_chain(
            config,
            request.host.clone(),
            request.port,
            request.known_host_key(),
            request.username.clone(),
            auth,
            jump_host,
            known_hosts,
        )
        .await?
    } else {
        let handle = connect_and_authenticate(
            config,
            request.host.clone(),
            request.port,
            request.outbound_proxy.clone(),
            request.known_host_key(),
            request.username.clone(),
            auth,
            known_hosts,
        )
        .await?;
        (handle, Vec::new())
    };

    Ok(EstablishedHandles {
        target_handle,
        _jump_handles: jump_handles,
    })
}

async fn connect_via_jump_chain(
    config: Arc<client::Config>,
    target_host: String,
    target_port: u16,
    target_endpoint: String,
    target_username: String,
    target_auth: AuthConfig,
    jump_host: JumpHostConnection,
    known_hosts: Arc<KnownHostStore>,
) -> Result<(
    client::Handle<SftpHandler>,
    Vec<client::Handle<SftpHandler>>,
)> {
    let chain = flatten_jump_chain(jump_host);
    let first_hop = chain
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("Jump host chain is empty"))?;
    let mut current_handle = connect_and_authenticate(
        config.clone(),
        first_hop.host.clone(),
        first_hop.port,
        first_hop.outbound_proxy.clone(),
        first_hop.known_host_key(),
        first_hop.username.clone(),
        first_hop.auth.clone(),
        known_hosts.clone(),
    )
    .await?;
    let mut jump_handles = Vec::new();

    for hop in chain.into_iter().skip(1) {
        let channel = current_handle
            .channel_open_direct_tcpip(hop.host.clone(), hop.port.into(), "127.0.0.1", 0)
            .await
            .with_context(|| format!("Unable to open jump-host tunnel to {}", hop.address()))?;
        let next_handle = connect_and_authenticate_stream(
            config.clone(),
            channel.into_stream(),
            hop.known_host_key(),
            hop.username.clone(),
            hop.auth.clone(),
            known_hosts.clone(),
        )
        .await?;
        jump_handles.push(current_handle);
        current_handle = next_handle;
    }

    let jump_channel = current_handle
        .channel_open_direct_tcpip(target_host, target_port.into(), "127.0.0.1", 0)
        .await
        .context("Unable to open jump-host tunnel to the target")?;

    let target_handle = connect_and_authenticate_stream(
        config,
        jump_channel.into_stream(),
        target_endpoint,
        target_username,
        target_auth,
        known_hosts,
    )
    .await?;

    jump_handles.push(current_handle);
    Ok((target_handle, jump_handles))
}

fn flatten_jump_chain(jump_host: JumpHostConnection) -> Vec<JumpHostConnection> {
    let mut chain = Vec::new();
    let mut current = Some(jump_host);

    while let Some(jump) = current {
        current = jump.jump_host.as_deref().cloned();
        chain.push(JumpHostConnection {
            jump_host: None,
            ..jump
        });
    }

    chain.reverse();
    chain
}

async fn connect_and_authenticate(
    config: Arc<client::Config>,
    host: String,
    port: u16,
    outbound_proxy: Option<OutboundProxy>,
    endpoint: String,
    username: String,
    auth: AuthConfig,
    known_hosts: Arc<KnownHostStore>,
) -> Result<client::Handle<SftpHandler>> {
    let handler = SftpHandler {
        endpoint,
        known_hosts,
        trusted_new_host: Arc::new(AtomicBool::new(false)),
    };

    let stream = crate::proxy::connect_first_hop(&host, port, outbound_proxy.as_ref()).await?;
    let mut handle = client::connect_stream(config, stream, handler)
        .await
        .context("Unable to open the SSH transport after establishing the network route")?;
    crate::ssh_auth::authenticate(&mut handle, &username, &auth).await?;
    Ok(handle)
}

async fn connect_and_authenticate_stream<R>(
    config: Arc<client::Config>,
    stream: R,
    endpoint: String,
    username: String,
    auth: AuthConfig,
    known_hosts: Arc<KnownHostStore>,
) -> Result<client::Handle<SftpHandler>>
where
    R: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let handler = SftpHandler {
        endpoint,
        known_hosts,
        trusted_new_host: Arc::new(AtomicBool::new(false)),
    };

    let mut handle = client::connect_stream(config, stream, handler)
        .await
        .context("Unable to open the SSH transport through the jump host")?;
    crate::ssh_auth::authenticate(&mut handle, &username, &auth).await?;
    Ok(handle)
}

#[derive(Clone)]
struct SftpHandler {
    endpoint: String,
    known_hosts: Arc<KnownHostStore>,
    trusted_new_host: Arc<AtomicBool>,
}

impl client::Handler for SftpHandler {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, server_public_key: &PublicKey) -> Result<bool> {
        let key = server_public_key
            .to_openssh()
            .context("Unable to serialize the server public key")?;

        match self.known_hosts.verify_or_trust(&self.endpoint, &key)? {
            HostKeyDecision::Existing => Ok(true),
            HostKeyDecision::Added => {
                self.trusted_new_host.store(true, Ordering::SeqCst);
                Ok(true)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorizedKeyAction, AuthorizedKeyEvent, AuthorizedKeyOutcome, GeneratedKeyVerification,
        SftpEvent, spawn_authorized_key_operation, spawn_delete_path, spawn_download_file,
        spawn_list_directory, spawn_upload_file,
    };
    use crate::models::{AuthConfig, ConnectRequest, ConnectionKind, OutboundProxy};
    use crate::ssh::{SessionCommand, SshEvent, spawn_session};
    use crate::ssh_keys::{PublicKeyMaterial, generate_ed25519_key_pair};
    use crate::storage::KnownHostStore;
    #[cfg(unix)]
    use crate::test_support::TestSshAgent;
    use crate::test_support::{
        DockerSshServer, TestIsolation, TestProxyProtocol, TestTcpProxy,
        create_test_user_certificate,
    };
    use std::fs;
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use std::time::Duration;

    fn docker_sftp_request(server: &DockerSshServer) -> ConnectRequest {
        ConnectRequest {
            session_id: 7,
            title: "Docker SFTP".to_string(),
            kind: ConnectionKind::Ssh,
            host: server.host().to_string(),
            port: server.port,
            username: server.username().to_string(),
            auth: Some(AuthConfig::Password {
                password: server.password().to_string(),
            }),
            jump_host: None,
            outbound_proxy: None,
            startup_directory: None,
            startup_command: None,
            start_in_files: false,
            persistent_session: false,
            persistent_session_name: None,
            persistent_session_detach_others: false,
            terminal_scrollback_rows: 10_000,
            port_forward_rules: Vec::new(),
            local_shell: None,
            environment: Vec::new(),
        }
    }

    fn recv_sftp_event(rx: &Receiver<SftpEvent>) -> SftpEvent {
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for SFTP event"),
            Err(RecvTimeoutError::Disconnected) => panic!("SFTP event channel disconnected"),
        }
    }

    fn recv_authorized_key_event(rx: &Receiver<AuthorizedKeyEvent>) -> AuthorizedKeyEvent {
        match rx.recv_timeout(Duration::from_secs(75)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                panic!("timed out waiting for authorized_keys operation")
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("authorized_keys operation channel disconnected")
            }
        }
    }

    fn run_authorized_key_operation(
        operation_id: u64,
        request: ConnectRequest,
        known_hosts: Arc<KnownHostStore>,
        key: PublicKeyMaterial,
        action: AuthorizedKeyAction,
        verification: Option<GeneratedKeyVerification>,
    ) -> AuthorizedKeyEvent {
        let (_control, events) = spawn_authorized_key_operation(
            operation_id,
            request,
            known_hosts,
            key,
            action,
            verification,
        )
        .expect("unable to start authorized_keys operation");
        recv_authorized_key_event(&events)
    }

    #[test]
    fn docker_sftp_round_trips_directory_upload_download_and_delete() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker sftp e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        server
            .exec(
                "mkdir -p /home/termirust/e2e-sftp && printf 'seed-file\\n' > /home/termirust/e2e-sftp/seed.txt && chown -R termirust:termirust /home/termirust/e2e-sftp",
            )
            .expect("unable to seed remote sftp directory");

        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let request = docker_sftp_request(&server);

        spawn_list_directory(
            1,
            1,
            request.clone(),
            known_hosts.clone(),
            "/home/termirust/e2e-sftp".to_string(),
            event_tx.clone(),
        );
        match recv_sftp_event(&event_rx) {
            SftpEvent::DirectoryLoaded { path, entries, .. } => {
                assert_eq!(path, "/home/termirust/e2e-sftp");
                assert!(entries.iter().any(|entry| entry.name == "seed.txt"));
            }
            event => panic!("unexpected list event: {event:?}"),
        }

        let local_dir =
            std::env::temp_dir().join(format!("termirust-sftp-test-{}", std::process::id()));
        fs::create_dir_all(&local_dir).expect("unable to create local sftp temp dir");
        let upload_path = local_dir.join("upload.txt");
        fs::write(&upload_path, "uploaded from test\n").expect("unable to write upload fixture");

        spawn_upload_file(
            1,
            2,
            request.clone(),
            known_hosts.clone(),
            "/home/termirust/e2e-sftp".to_string(),
            upload_path,
            event_tx.clone(),
        );
        let uploaded_remote_path = match recv_sftp_event(&event_rx) {
            SftpEvent::UploadComplete { remote_path, .. } => remote_path,
            event => panic!("unexpected upload event: {event:?}"),
        };
        assert_eq!(uploaded_remote_path, "/home/termirust/e2e-sftp/upload.txt");

        let download_path = local_dir.join("downloaded.txt");
        spawn_download_file(
            1,
            3,
            request.clone(),
            known_hosts.clone(),
            uploaded_remote_path.clone(),
            download_path.clone(),
            event_tx.clone(),
        );
        match recv_sftp_event(&event_rx) {
            SftpEvent::DownloadComplete { remote_path, .. } => {
                assert_eq!(remote_path, uploaded_remote_path);
            }
            event => panic!("unexpected download event: {event:?}"),
        }
        assert_eq!(
            fs::read_to_string(&download_path).expect("unable to read downloaded file"),
            "uploaded from test\n"
        );

        spawn_delete_path(
            1,
            4,
            request.clone(),
            known_hosts.clone(),
            uploaded_remote_path.clone(),
            false,
            event_tx.clone(),
        );
        match recv_sftp_event(&event_rx) {
            SftpEvent::DeleteComplete { remote_path, .. } => {
                assert_eq!(remote_path, uploaded_remote_path);
            }
            event => panic!("unexpected delete event: {event:?}"),
        }

        spawn_list_directory(
            1,
            5,
            request,
            known_hosts,
            "/home/termirust/e2e-sftp".to_string(),
            event_tx,
        );
        match recv_sftp_event(&event_rx) {
            SftpEvent::DirectoryLoaded { entries, .. } => {
                assert!(entries.iter().all(|entry| entry.name != "upload.txt"));
                assert!(entries.iter().any(|entry| entry.name == "seed.txt"));
            }
            event => panic!("unexpected final list event: {event:?}"),
        }

        let _ = fs::remove_dir_all(local_dir);
    }

    #[test]
    fn docker_sftp_lists_through_http_connect_proxy() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping Docker proxied SFTP e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        let target = format!("{}:{}", server.host(), server.port)
            .parse()
            .expect("invalid Docker SSH endpoint");
        let proxy = TestTcpProxy::start(TestProxyProtocol::HttpConnect, target)
            .expect("unable to start HTTP CONNECT proxy");
        let mut request = docker_sftp_request(&server);
        request.outbound_proxy = Some(OutboundProxy::HttpConnect {
            host: "127.0.0.1".to_string(),
            port: proxy.port,
        });
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        spawn_list_directory(
            1,
            90,
            request,
            known_hosts,
            "/home/termirust".to_string(),
            event_tx,
        );
        match recv_sftp_event(&event_rx) {
            SftpEvent::DirectoryLoaded { path, .. } => assert_eq!(path, "/home/termirust"),
            event => panic!("unexpected proxied SFTP event: {event:?}"),
        }
        assert_eq!(proxy.accepted_connections(), 1);
    }

    #[test]
    fn docker_sftp_lists_directory_with_openssh_certificate() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping docker certificate sftp e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start docker ssh server");
        server
            .exec(
                "mkdir -p /home/termirust/certificate-sftp && touch /home/termirust/certificate-sftp/visible.txt && chown -R termirust:termirust /home/termirust/certificate-sftp",
            )
            .expect("unable to seed certificate sftp directory");
        let certificate_dir = tempfile::TempDir::new().expect("unable to create certificate dir");
        let certificate =
            create_test_user_certificate(certificate_dir.path(), server.username(), true);
        let key_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ssh-server/id_ed25519");
        let mut request = docker_sftp_request(&server);
        request.auth = Some(AuthConfig::OpenSshCertificate {
            key_path: key_path.display().to_string(),
            certificate_path: certificate.display().to_string(),
            passphrase: None,
        });
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();

        spawn_list_directory(
            8,
            1,
            request,
            known_hosts,
            "/home/termirust/certificate-sftp".to_string(),
            event_tx,
        );
        match recv_sftp_event(&event_rx) {
            SftpEvent::DirectoryLoaded { entries, .. } => {
                assert!(entries.iter().any(|entry| entry.name == "visible.txt"));
            }
            event => panic!("unexpected certificate SFTP event: {event:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn docker_sftp_lists_directory_with_ssh_agent_authentication() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping Docker SSH-agent SFTP e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        server
            .exec(
                "mkdir -p /home/termirust/agent-sftp && touch /home/termirust/agent-sftp/visible.txt && chown -R termirust:termirust /home/termirust/agent-sftp",
            )
            .expect("unable to seed agent SFTP directory");
        let agent = TestSshAgent::start_with_fixture_key().expect("unable to start test SSH agent");
        let mut request = docker_sftp_request(&server);
        request.auth = Some(AuthConfig::LocalAgent {
            socket_path: Some(agent.socket_path().display().to_string()),
            forward_agent: false,
        });
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();

        spawn_list_directory(
            9,
            1,
            request,
            known_hosts,
            "/home/termirust/agent-sftp".to_string(),
            event_tx,
        );
        match recv_sftp_event(&event_rx) {
            SftpEvent::DirectoryLoaded { entries, .. } => {
                assert!(entries.iter().any(|entry| entry.name == "visible.txt"));
            }
            event => panic!("unexpected SSH-agent SFTP event: {event:?}"),
        }
    }

    #[test]
    fn docker_generated_key_install_is_verified_idempotent_and_exactly_removable() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping generated-key lifecycle e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        server
            .exec(
                "printf '# preserve this comment with spaces\\nnot-a-key preserve-me exactly\\n' >> /home/termirust/.ssh/authorized_keys",
            )
            .expect("unable to seed unrelated authorized_keys lines");
        let original_authorized_keys = server
            .exec("cat /home/termirust/.ssh/authorized_keys")
            .expect("unable to capture original authorized_keys content");
        let key_dir = tempfile::TempDir::new().expect("unable to create generated-key directory");
        let private_path = key_dir.path().join("id_termirust_generated");
        let passphrase = "generated-key-test-passphrase";
        let generated = generate_ed25519_key_pair(
            &private_path,
            "termirust-generated-key-test",
            Some(passphrase),
        )
        .expect("unable to generate test SSH identity");
        let key = PublicKeyMaterial::parse(&generated.public_key)
            .expect("unable to parse generated public key");
        let key_blob = key
            .openssh
            .split_whitespace()
            .nth(1)
            .expect("generated public key should contain a key blob")
            .to_string();
        let request = docker_sftp_request(&server);
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let verification = || {
            Some(GeneratedKeyVerification::new(
                private_path.clone(),
                Some(passphrase.to_string()),
            ))
        };

        assert!(matches!(
            run_authorized_key_operation(
                20,
                request.clone(),
                known_hosts.clone(),
                key.clone(),
                AuthorizedKeyAction::Add,
                verification(),
            ),
            AuthorizedKeyEvent::Complete {
                outcome: AuthorizedKeyOutcome::InstalledAndVerified,
                ..
            }
        ));

        let mut generated_request = request.clone();
        generated_request.session_id = 25;
        generated_request.auth = Some(AuthConfig::PrivateKey {
            key_path: private_path.display().to_string(),
            passphrase: Some(passphrase.to_string()),
        });
        let (ssh_tx, ssh_rx) = mpsc::channel();
        let runtime = spawn_session(generated_request, known_hosts.clone(), ssh_tx, 0);
        let mut connected = false;
        let mut marker = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline && !marker {
            match ssh_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(SshEvent::Connected { .. }) => {
                    connected = true;
                    runtime
                        .command_tx
                        .send(SessionCommand::Input(
                            b"printf 'generated-key-terminal-ok\\n'\n".to_vec(),
                        ))
                        .expect("unable to send generated-key terminal marker");
                }
                Ok(SshEvent::Output { data, .. }) => {
                    marker |= String::from_utf8_lossy(&data).contains("generated-key-terminal-ok");
                }
                Ok(SshEvent::Error { message, .. }) => {
                    panic!("generated-key terminal login failed: {message}")
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = runtime.command_tx.send(SessionCommand::Disconnect);
        assert!(connected, "generated key should open a fresh terminal");
        assert!(
            marker,
            "generated-key terminal should stream command output"
        );

        assert!(matches!(
            run_authorized_key_operation(
                21,
                request.clone(),
                known_hosts.clone(),
                key.clone(),
                AuthorizedKeyAction::Add,
                verification(),
            ),
            AuthorizedKeyEvent::Complete {
                outcome: AuthorizedKeyOutcome::AlreadyPresentAndVerified,
                ..
            }
        ));

        let installed = server
            .exec("cat /home/termirust/.ssh/authorized_keys")
            .expect("unable to read remote authorized_keys");
        assert_eq!(installed.matches(&key_blob).count(), 1);
        assert!(installed.contains("# preserve this comment with spaces"));
        assert!(installed.contains("not-a-key preserve-me exactly"));
        assert!(
            installed.lines().count() >= 2,
            "fixture key must be preserved"
        );
        assert_eq!(
            server
                .exec("stat -c '%a %U' /home/termirust/.ssh /home/termirust/.ssh/authorized_keys")
                .expect("unable to inspect remote SSH permissions"),
            "700 termirust\n600 termirust"
        );

        assert!(matches!(
            run_authorized_key_operation(
                22,
                request.clone(),
                known_hosts.clone(),
                key.clone(),
                AuthorizedKeyAction::Remove,
                None,
            ),
            AuthorizedKeyEvent::Complete {
                outcome: AuthorizedKeyOutcome::Removed,
                ..
            }
        ));
        let removed = server
            .exec("cat /home/termirust/.ssh/authorized_keys")
            .expect("unable to read remote authorized_keys after removal");
        assert!(!removed.contains(&key_blob));
        assert_eq!(removed, original_authorized_keys);
        assert!(
            !removed.trim().is_empty(),
            "fixture key must remain after removal"
        );
        assert!(matches!(
            run_authorized_key_operation(
                23,
                request,
                known_hosts,
                key,
                AuthorizedKeyAction::Remove,
                None,
            ),
            AuthorizedKeyEvent::Complete {
                outcome: AuthorizedKeyOutcome::NotPresent,
                ..
            }
        ));
    }

    #[test]
    fn docker_generated_key_deployment_rejects_authorized_keys_symlink_without_leaking_secrets() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping generated-key hostile-path e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        server
            .exec(
                "mv /home/termirust/.ssh/authorized_keys /home/termirust/.ssh/authorized_keys.real && ln -s authorized_keys.real /home/termirust/.ssh/authorized_keys",
            )
            .expect("unable to prepare hostile authorized_keys symlink");
        let before = server
            .exec("cat /home/termirust/.ssh/authorized_keys.real")
            .expect("unable to read protected authorized_keys fixture");
        let key_dir = tempfile::TempDir::new().expect("unable to create generated-key directory");
        let private_path = key_dir.path().join("hostile-secret-path");
        let passphrase = "hostile-secret-passphrase";
        let generated = generate_ed25519_key_pair(&private_path, "hostile-test", Some(passphrase))
            .expect("unable to generate hostile-path test identity");
        let key = PublicKeyMaterial::parse(&generated.public_key)
            .expect("unable to parse generated public key");
        let event = run_authorized_key_operation(
            24,
            docker_sftp_request(&server),
            Arc::new(KnownHostStore::load().expect("unable to load known hosts")),
            key,
            AuthorizedKeyAction::Add,
            Some(GeneratedKeyVerification::new(
                private_path.clone(),
                Some(passphrase.to_string()),
            )),
        );
        let AuthorizedKeyEvent::Error { message, .. } = event else {
            panic!("hostile authorized_keys symlink was not rejected: {event:?}");
        };
        assert!(message.contains("unsafe type or owner"));
        assert!(!message.contains(passphrase));
        assert!(!message.contains(private_path.to_string_lossy().as_ref()));
        assert!(!message.contains(&generated.public_key));
        assert_eq!(
            server
                .exec("cat /home/termirust/.ssh/authorized_keys.real")
                .expect("unable to re-read protected authorized_keys fixture"),
            before
        );
    }

    #[test]
    fn docker_generated_key_verification_failure_reports_installed_state_honestly() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping generated-key verification-failure e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        let key_dir = tempfile::TempDir::new().expect("unable to create generated-key directory");
        let installed_path = key_dir.path().join("installed-key");
        let rejected_path = key_dir.path().join("different-verification-key");
        let installed = generate_ed25519_key_pair(&installed_path, "installed", None).unwrap();
        generate_ed25519_key_pair(&rejected_path, "different", None).unwrap();
        let key = PublicKeyMaterial::parse(&installed.public_key).unwrap();
        let key_blob = key.openssh.split_whitespace().nth(1).unwrap().to_string();
        let request = docker_sftp_request(&server);
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));

        assert!(matches!(
            run_authorized_key_operation(
                26,
                request.clone(),
                known_hosts.clone(),
                key.clone(),
                AuthorizedKeyAction::Add,
                Some(GeneratedKeyVerification::new(rejected_path, None)),
            ),
            AuthorizedKeyEvent::Complete {
                outcome: AuthorizedKeyOutcome::InstalledVerificationFailed,
                ..
            }
        ));
        assert_eq!(
            server
                .exec("cat /home/termirust/.ssh/authorized_keys")
                .expect("unable to inspect failed-verification deployment")
                .matches(&key_blob)
                .count(),
            1,
            "a verification failure must not misreport the completed remote write"
        );
        assert!(matches!(
            run_authorized_key_operation(
                27,
                request,
                known_hosts,
                key,
                AuthorizedKeyAction::Remove,
                None,
            ),
            AuthorizedKeyEvent::Complete {
                outcome: AuthorizedKeyOutcome::Removed,
                ..
            }
        ));
    }

    #[test]
    fn docker_concurrent_generated_key_deployments_preserve_both_keys() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping concurrent generated-key e2e: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        let key_dir = tempfile::TempDir::new().expect("unable to create generated-key directory");
        let first_path = key_dir.path().join("concurrent-first");
        let second_path = key_dir.path().join("concurrent-second");
        let first = generate_ed25519_key_pair(&first_path, "concurrent-first", None)
            .expect("unable to generate first concurrent key");
        let second = generate_ed25519_key_pair(&second_path, "concurrent-second", None)
            .expect("unable to generate second concurrent key");
        let first_key = PublicKeyMaterial::parse(&first.public_key).unwrap();
        let second_key = PublicKeyMaterial::parse(&second.public_key).unwrap();
        let first_blob = first_key
            .openssh
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_string();
        let second_blob = second_key
            .openssh
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_string();
        let request = docker_sftp_request(&server);
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));

        let (_first_control, first_events) = spawn_authorized_key_operation(
            31,
            request.clone(),
            known_hosts.clone(),
            first_key.clone(),
            AuthorizedKeyAction::Add,
            Some(GeneratedKeyVerification::new(first_path.clone(), None)),
        )
        .unwrap();
        let (_second_control, second_events) = spawn_authorized_key_operation(
            32,
            request.clone(),
            known_hosts.clone(),
            second_key.clone(),
            AuthorizedKeyAction::Add,
            Some(GeneratedKeyVerification::new(second_path.clone(), None)),
        )
        .unwrap();
        let first_event = recv_authorized_key_event(&first_events);
        let second_event = recv_authorized_key_event(&second_events);

        for (operation_id, event, key, path) in [
            (33, first_event, first_key, first_path),
            (34, second_event, second_key, second_path),
        ] {
            match event {
                AuthorizedKeyEvent::Complete {
                    outcome:
                        AuthorizedKeyOutcome::InstalledAndVerified
                        | AuthorizedKeyOutcome::AlreadyPresentAndVerified,
                    ..
                } => {}
                AuthorizedKeyEvent::Error { message, .. }
                    if message.contains("already in progress") =>
                {
                    let retry = run_authorized_key_operation(
                        operation_id,
                        request.clone(),
                        known_hosts.clone(),
                        key,
                        AuthorizedKeyAction::Add,
                        Some(GeneratedKeyVerification::new(path, None)),
                    );
                    assert!(matches!(
                        retry,
                        AuthorizedKeyEvent::Complete {
                            outcome: AuthorizedKeyOutcome::InstalledAndVerified
                                | AuthorizedKeyOutcome::AlreadyPresentAndVerified,
                            ..
                        }
                    ));
                }
                event => panic!("unexpected concurrent deployment result: {event:?}"),
            }
        }

        let installed = server
            .exec("cat /home/termirust/.ssh/authorized_keys")
            .expect("unable to read concurrent authorized_keys result");
        assert_eq!(installed.matches(&first_blob).count(), 1);
        assert_eq!(installed.matches(&second_blob).count(), 1);
        assert!(
            installed.lines().count() >= 3,
            "fixture key must be preserved"
        );
    }

    #[test]
    fn pre_cancelled_generated_key_operation_is_bounded_and_does_not_connect() {
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();
        let key_dir = tempfile::TempDir::new().unwrap();
        let generated =
            generate_ed25519_key_pair(&key_dir.path().join("cancelled"), "", None).unwrap();
        let key = PublicKeyMaterial::parse(&generated.public_key).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = std::time::Instant::now();
        let outcome = runtime
            .block_on(super::run_authorized_key_operation(
                ConnectRequest {
                    session_id: 40,
                    title: "cancelled".to_string(),
                    kind: ConnectionKind::Ssh,
                    host: "192.0.2.1".to_string(),
                    port: 22,
                    username: "nobody".to_string(),
                    auth: Some(AuthConfig::Password {
                        password: "not-used".to_string(),
                    }),
                    jump_host: None,
                    outbound_proxy: None,
                    startup_directory: None,
                    startup_command: None,
                    start_in_files: false,
                    persistent_session: false,
                    persistent_session_name: None,
                    persistent_session_detach_others: false,
                    terminal_scrollback_rows: 10_000,
                    port_forward_rules: Vec::new(),
                    local_shell: None,
                    environment: Vec::new(),
                },
                Arc::new(KnownHostStore::load().unwrap()),
                key,
                AuthorizedKeyAction::Add,
                Some(GeneratedKeyVerification::new(
                    generated.private_key_path,
                    None,
                )),
                cancellation,
            ))
            .unwrap();
        assert_eq!(outcome, AuthorizedKeyOutcome::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
