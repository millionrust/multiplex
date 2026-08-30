use anyhow::{Context, Result, anyhow};
use russh::keys::PublicKey;
use russh::{ChannelMsg, client};
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpClientError;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use sha2::{Digest as _, Sha256};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, Weak};
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
pub const SFTP_TRANSFER_CHUNK_BYTES: usize = 256 * 1024;
pub const SFTP_TRANSFER_MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const SFTP_TRANSFER_MAX_ACTIVE: usize = 3;
pub const SFTP_TRANSFER_MAX_QUEUED: usize = 32;
const SFTP_TRANSFER_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);
const SFTP_TRANSFER_IO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct RemoteFileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SftpTransferDirection {
    Upload,
    Download,
}

impl SftpTransferDirection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Upload => "Upload",
            Self::Download => "Download",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SftpConflictPolicy {
    #[default]
    Ask,
    Replace,
    Skip,
    Resume,
}

#[derive(Clone, Debug)]
pub struct SftpTransferSpec {
    pub workspace_id: u64,
    pub operation_id: u64,
    pub request: ConnectRequest,
    pub direction: SftpTransferDirection,
    pub local_path: PathBuf,
    pub remote_path: String,
    pub conflict_policy: SftpConflictPolicy,
}

#[derive(Clone)]
pub struct SftpTransferControl {
    inner: Weak<SftpTransferManagerInner>,
    workspace_id: u64,
    operation_id: u64,
    cancellation: CancellationToken,
}

impl SftpTransferControl {
    pub fn cancel(&self) {
        self.cancellation.cancel();
        if let Some(inner) = self.inner.upgrade() {
            cancel_queued_transfer(&inner, self.workspace_id, self.operation_id);
        }
    }
}

impl std::fmt::Debug for SftpTransferControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SftpTransferControl")
            .field("workspace_id", &self.workspace_id)
            .field("operation_id", &self.operation_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum SftpEvent {
    DirectoryLoaded {
        workspace_id: u64,
        operation_id: u64,
        path: String,
        entries: Vec<RemoteFileEntry>,
    },
    DeleteComplete {
        workspace_id: u64,
        operation_id: u64,
        remote_path: String,
    },
    TransferQueued {
        workspace_id: u64,
        operation_id: u64,
        direction: SftpTransferDirection,
        queued_ahead: usize,
    },
    TransferStarted {
        workspace_id: u64,
        operation_id: u64,
        direction: SftpTransferDirection,
        total_bytes: u64,
        resumed_from: u64,
    },
    TransferProgress {
        workspace_id: u64,
        operation_id: u64,
        direction: SftpTransferDirection,
        transferred_bytes: u64,
        total_bytes: u64,
    },
    TransferConflict {
        workspace_id: u64,
        operation_id: u64,
        direction: SftpTransferDirection,
        existing_bytes: u64,
        resume_available: bool,
    },
    TransferSkipped {
        workspace_id: u64,
        operation_id: u64,
        direction: SftpTransferDirection,
    },
    TransferCancelled {
        workspace_id: u64,
        operation_id: u64,
        direction: SftpTransferDirection,
        transferred_bytes: u64,
        staging_retained: bool,
    },
    TransferComplete {
        workspace_id: u64,
        operation_id: u64,
        direction: SftpTransferDirection,
        transferred_bytes: u64,
        resumed_from: u64,
        sha256: String,
        cleanup_warning: bool,
    },
    Error {
        workspace_id: u64,
        operation_id: u64,
        message: String,
    },
}

#[derive(Clone)]
pub struct SftpTransferManager {
    inner: Arc<SftpTransferManagerInner>,
}

struct SftpTransferManagerInner {
    state: Mutex<SftpTransferManagerState>,
}

#[derive(Default)]
struct SftpTransferManagerState {
    active: usize,
    queued: VecDeque<SftpTransferJob>,
}

struct SftpTransferJob {
    spec: SftpTransferSpec,
    known_hosts: Arc<KnownHostStore>,
    event_tx: Sender<SftpEvent>,
    cancellation: CancellationToken,
}

impl Default for SftpTransferManager {
    fn default() -> Self {
        Self {
            inner: Arc::new(SftpTransferManagerInner {
                state: Mutex::new(SftpTransferManagerState::default()),
            }),
        }
    }
}

impl SftpTransferManager {
    pub fn enqueue(
        &self,
        spec: SftpTransferSpec,
        known_hosts: Arc<KnownHostStore>,
        event_tx: Sender<SftpEvent>,
    ) -> Result<SftpTransferControl> {
        validate_transfer_spec(&spec)?;
        let cancellation = CancellationToken::new();
        let queued_ahead = {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| anyhow!("SFTP transfer scheduler is unavailable"))?;
            if state.active + state.queued.len()
                >= SFTP_TRANSFER_MAX_ACTIVE + SFTP_TRANSFER_MAX_QUEUED
            {
                anyhow::bail!(
                    "SFTP transfer queue is full ({} active, {} queued)",
                    SFTP_TRANSFER_MAX_ACTIVE,
                    SFTP_TRANSFER_MAX_QUEUED
                );
            }
            let queued_ahead = state.queued.len() + state.active;
            state.queued.push_back(SftpTransferJob {
                spec: spec.clone(),
                known_hosts,
                event_tx: event_tx.clone(),
                cancellation: cancellation.clone(),
            });
            queued_ahead
        };

        let _ = event_tx.send(SftpEvent::TransferQueued {
            workspace_id: spec.workspace_id,
            operation_id: spec.operation_id,
            direction: spec.direction,
            queued_ahead,
        });
        dispatch_transfers(self.inner.clone());

        Ok(SftpTransferControl {
            inner: Arc::downgrade(&self.inner),
            workspace_id: spec.workspace_id,
            operation_id: spec.operation_id,
            cancellation,
        })
    }
}

fn validate_transfer_spec(spec: &SftpTransferSpec) -> Result<()> {
    if spec.remote_path.is_empty() || spec.remote_path.len() > 4096 {
        anyhow::bail!("Remote transfer path must contain between 1 and 4096 bytes");
    }
    validate_remote_path(&spec.remote_path)?;
    if spec.local_path.as_os_str().is_empty() {
        anyhow::bail!("Local transfer path is required");
    }
    Ok(())
}

fn cancel_queued_transfer(
    inner: &Arc<SftpTransferManagerInner>,
    workspace_id: u64,
    operation_id: u64,
) {
    let removed = inner.state.lock().ok().and_then(|mut state| {
        let index = state.queued.iter().position(|job| {
            job.spec.workspace_id == workspace_id && job.spec.operation_id == operation_id
        })?;
        state.queued.remove(index)
    });
    if let Some(job) = removed {
        let _ = job.event_tx.send(SftpEvent::TransferCancelled {
            workspace_id,
            operation_id,
            direction: job.spec.direction,
            transferred_bytes: 0,
            staging_retained: false,
        });
    }
}

fn dispatch_transfers(inner: Arc<SftpTransferManagerInner>) {
    loop {
        let job = {
            let Ok(mut state) = inner.state.lock() else {
                return;
            };
            if state.active >= SFTP_TRANSFER_MAX_ACTIVE {
                return;
            }
            let Some(job) = state.queued.pop_front() else {
                return;
            };
            state.active += 1;
            job
        };

        let worker_inner = inner.clone();
        let workspace_id = job.spec.workspace_id;
        let operation_id = job.spec.operation_id;
        let event_tx = job.event_tx.clone();
        let worker_event_tx = event_tx.clone();
        let spawn_result = thread::Builder::new()
            .name(format!("sftp-transfer-{workspace_id}-{operation_id}"))
            .spawn(move || {
                let result = Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()
                    .context("Unable to initialize the SFTP transfer runtime")
                    .and_then(|runtime| runtime.block_on(run_transfer(job)));
                if let Err(error) = result {
                    let _ = worker_event_tx.send(SftpEvent::Error {
                        workspace_id,
                        operation_id,
                        message: format!("SFTP transfer failed: {error:#}"),
                    });
                }
                finish_transfer(&worker_inner);
            });

        if let Err(error) = spawn_result {
            let _ = event_tx.send(SftpEvent::Error {
                workspace_id,
                operation_id,
                message: format!("Unable to start the SFTP transfer worker: {error}"),
            });
            finish_transfer(&inner);
        }
    }
}

fn finish_transfer(inner: &Arc<SftpTransferManagerInner>) {
    if let Ok(mut state) = inner.state.lock() {
        state.active = state.active.saturating_sub(1);
    }
    dispatch_transfers(inner.clone());
}

async fn run_transfer(job: SftpTransferJob) -> Result<()> {
    if job.cancellation.is_cancelled() {
        send_transfer_cancelled(&job, 0, false);
        return Ok(());
    }
    match job.spec.direction {
        SftpTransferDirection::Upload => run_upload(job).await,
        SftpTransferDirection::Download => run_download(job).await,
    }
}

fn send_transfer_cancelled(job: &SftpTransferJob, transferred: u64, retained: bool) {
    let _ = job.event_tx.send(SftpEvent::TransferCancelled {
        workspace_id: job.spec.workspace_id,
        operation_id: job.spec.operation_id,
        direction: job.spec.direction,
        transferred_bytes: transferred,
        staging_retained: retained,
    });
}

fn send_transfer_progress(job: &SftpTransferJob, transferred: u64, total: u64) {
    let _ = job.event_tx.send(SftpEvent::TransferProgress {
        workspace_id: job.spec.workspace_id,
        operation_id: job.spec.operation_id,
        direction: job.spec.direction,
        transferred_bytes: transferred,
        total_bytes: total,
    });
}

fn send_transfer_started(job: &SftpTransferJob, total: u64, resumed_from: u64) {
    let _ = job.event_tx.send(SftpEvent::TransferStarted {
        workspace_id: job.spec.workspace_id,
        operation_id: job.spec.operation_id,
        direction: job.spec.direction,
        total_bytes: total,
        resumed_from,
    });
}

fn send_transfer_complete(
    job: &SftpTransferJob,
    transferred: u64,
    resumed_from: u64,
    hasher: Sha256,
    cleanup_warning: bool,
) {
    let _ = job.event_tx.send(SftpEvent::TransferComplete {
        workspace_id: job.spec.workspace_id,
        operation_id: job.spec.operation_id,
        direction: job.spec.direction,
        transferred_bytes: transferred,
        resumed_from,
        sha256: encode_sha256(hasher.finalize().as_slice()),
        cleanup_warning,
    });
}

async fn cancellable_transfer_step<T, E, F>(
    cancellation: &CancellationToken,
    timeout_duration: Duration,
    timeout_message: &'static str,
    future: F,
) -> Result<Option<T>>
where
    F: std::future::Future<Output = std::result::Result<T, E>>,
    E: std::fmt::Display,
{
    tokio::select! {
        _ = cancellation.cancelled() => Ok(None),
        result = timeout(timeout_duration, future) => {
            let value = result
                .map_err(|_| anyhow!(timeout_message))?
                .map_err(|error| anyhow!("{error}"))?;
            Ok(Some(value))
        }
    }
}

fn encode_sha256(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn local_file_identity(path: &Path) -> Result<(u64, Option<SystemTime>)> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Unable to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("The local transfer source or destination must be a regular file");
    }
    Ok((metadata.len(), metadata.modified().ok()))
}

fn local_destination_identity(path: &Path) -> Result<Option<(u64, Option<SystemTime>)>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                anyhow::bail!("The local destination exists but is not a regular file");
            }
            Ok(Some((metadata.len(), metadata.modified().ok())))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Unable to inspect {}", path.display())),
    }
}

fn local_staging_path(destination: &Path, operation_id: u64) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("The local destination needs a parent directory"))?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("The local destination needs a valid file name"))?;
    Ok(parent.join(format!(".{name}.termirust-{operation_id}.part")))
}

fn local_backup_path(destination: &Path, operation_id: u64) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("The local destination needs a parent directory"))?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("The local destination needs a valid file name"))?;
    Ok(parent.join(format!(".{name}.termirust-{operation_id}.backup")))
}

fn remote_staging_path(destination: &str, operation_id: u64) -> Result<String> {
    let staging = format!("{destination}.termirust-{operation_id}.part");
    if staging.len() > 4096 {
        anyhow::bail!("Remote staging path exceeds 4096 bytes");
    }
    Ok(staging)
}

fn ensure_transfer_size(size: u64) -> Result<()> {
    if size > SFTP_TRANSFER_MAX_BYTES {
        anyhow::bail!(
            "Transfer size {} exceeds the {} byte per-file limit",
            size,
            SFTP_TRANSFER_MAX_BYTES
        );
    }
    Ok(())
}

fn same_remote_identity(left: &FileAttributes, right: &FileAttributes) -> bool {
    left.size == right.size && left.mtime == right.mtime && left.permissions == right.permissions
}

async fn run_upload(job: SftpTransferJob) -> Result<()> {
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

    let (total, source_modified) = local_file_identity(&job.spec.local_path)?;
    ensure_transfer_size(total)?;
    let Some((sftp, _handles)) = cancellable_transfer_step(
        &job.cancellation,
        SFTP_TRANSFER_CONNECT_TIMEOUT,
        "SFTP transfer connection timed out",
        open_sftp(job.spec.request.clone(), job.known_hosts.clone()),
    )
    .await?
    else {
        send_transfer_cancelled(&job, 0, false);
        return Ok(());
    };
    sftp.set_timeout(SFTP_TRANSFER_IO_TIMEOUT.as_secs()).await;

    let destination_before = remote_symlink_metadata(&sftp, &job.spec.remote_path).await?;
    if let Some(metadata) = destination_before.as_ref()
        && (metadata.is_symlink() || !metadata.is_regular())
    {
        anyhow::bail!("The remote destination exists but is not a regular file");
    }
    if let Some(metadata) = destination_before.as_ref() {
        match job.spec.conflict_policy {
            SftpConflictPolicy::Ask => {
                let staging = remote_staging_path(&job.spec.remote_path, job.spec.operation_id)?;
                let resume_available =
                    remote_symlink_metadata(&sftp, &staging)
                        .await?
                        .is_some_and(|part| {
                            part.is_regular() && !part.is_empty() && part.len() < total
                        });
                let _ = job.event_tx.send(SftpEvent::TransferConflict {
                    workspace_id: job.spec.workspace_id,
                    operation_id: job.spec.operation_id,
                    direction: job.spec.direction,
                    existing_bytes: metadata.len(),
                    resume_available,
                });
                let _ = sftp.close().await;
                return Ok(());
            }
            SftpConflictPolicy::Skip => {
                let _ = job.event_tx.send(SftpEvent::TransferSkipped {
                    workspace_id: job.spec.workspace_id,
                    operation_id: job.spec.operation_id,
                    direction: job.spec.direction,
                });
                let _ = sftp.close().await;
                return Ok(());
            }
            SftpConflictPolicy::Replace | SftpConflictPolicy::Resume => {}
        }
    } else if job.spec.conflict_policy == SftpConflictPolicy::Skip {
        let _ = job.event_tx.send(SftpEvent::TransferSkipped {
            workspace_id: job.spec.workspace_id,
            operation_id: job.spec.operation_id,
            direction: job.spec.direction,
        });
        let _ = sftp.close().await;
        return Ok(());
    }

    let staging = remote_staging_path(&job.spec.remote_path, job.spec.operation_id)?;
    let mut source = File::open(&job.spec.local_path)
        .with_context(|| format!("Unable to open {}", job.spec.local_path.display()))?;
    let mut hasher = Sha256::new();
    let mut resumed_from = 0;

    if job.spec.conflict_policy == SftpConflictPolicy::Resume {
        let staged = remote_symlink_metadata(&sftp, &staging)
            .await?
            .ok_or_else(|| anyhow!("No app-owned remote staging file is available to resume"))?;
        if !staged.is_regular() || staged.is_symlink() || staged.len() > total {
            anyhow::bail!("The remote staging file is not safe to resume");
        }
        resumed_from = staged.len();
        let mut remote_prefix = sftp
            .open(staging.clone())
            .await
            .context("Unable to open the remote staging file for resume validation")?;
        let mut local_chunk = vec![0u8; SFTP_TRANSFER_CHUNK_BYTES];
        let mut remote_chunk = vec![0u8; SFTP_TRANSFER_CHUNK_BYTES];
        let mut checked = 0u64;
        while checked < resumed_from {
            if job.cancellation.is_cancelled() {
                send_transfer_cancelled(&job, checked, true);
                let _ = sftp.close().await;
                return Ok(());
            }
            let want =
                usize::try_from((resumed_from - checked).min(SFTP_TRANSFER_CHUNK_BYTES as u64))
                    .unwrap_or(SFTP_TRANSFER_CHUNK_BYTES);
            source.read_exact(&mut local_chunk[..want])?;
            let Some(_) = cancellable_transfer_step(
                &job.cancellation,
                SFTP_TRANSFER_IO_TIMEOUT,
                "Remote staging validation timed out",
                remote_prefix.read_exact(&mut remote_chunk[..want]),
            )
            .await?
            else {
                send_transfer_cancelled(&job, checked, true);
                let _ = sftp.close().await;
                return Ok(());
            };
            if local_chunk[..want] != remote_chunk[..want] {
                anyhow::bail!("Resume refused because the staged prefix does not match the source");
            }
            hasher.update(&local_chunk[..want]);
            checked += want as u64;
        }
        timeout(SFTP_TRANSFER_IO_TIMEOUT, remote_prefix.shutdown())
            .await
            .map_err(|_| anyhow!("Closing the remote resume reader timed out"))??;
    } else {
        if remote_symlink_metadata(&sftp, &staging).await?.is_some() {
            sftp.remove_file(staging.clone())
                .await
                .context("Unable to clear an old app-owned upload staging file")?;
        }
    }

    let flags = if resumed_from == 0 {
        OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE
    } else {
        OpenFlags::CREATE | OpenFlags::WRITE
    };
    let mut remote = sftp
        .open_with_flags(staging.clone(), flags)
        .await
        .context("Unable to open the remote upload staging file")?;
    if resumed_from > 0 {
        timeout(
            SFTP_TRANSFER_IO_TIMEOUT,
            remote.seek(tokio::io::SeekFrom::Start(resumed_from)),
        )
        .await
        .map_err(|_| anyhow!("Seeking the remote staging file timed out"))??;
        source.seek(SeekFrom::Start(resumed_from))?;
    }
    send_transfer_started(&job, total, resumed_from);
    send_transfer_progress(&job, resumed_from, total);

    let mut transferred = resumed_from;
    let mut chunk = vec![0u8; SFTP_TRANSFER_CHUNK_BYTES];
    while transferred < total {
        if job.cancellation.is_cancelled() {
            let _ = timeout(SFTP_TRANSFER_IO_TIMEOUT, remote.shutdown()).await;
            send_transfer_cancelled(&job, transferred, true);
            let _ = sftp.close().await;
            return Ok(());
        }
        let read = source
            .read(&mut chunk)
            .context("Unable to read the local upload source")?;
        if read == 0 {
            anyhow::bail!("The local upload source ended before its reported size");
        }
        let Some(()) = cancellable_transfer_step(
            &job.cancellation,
            SFTP_TRANSFER_IO_TIMEOUT,
            "Uploading a file chunk timed out",
            remote.write_all(&chunk[..read]),
        )
        .await?
        else {
            let _ = timeout(SFTP_TRANSFER_IO_TIMEOUT, remote.shutdown()).await;
            send_transfer_cancelled(&job, transferred, true);
            let _ = sftp.close().await;
            return Ok(());
        };
        hasher.update(&chunk[..read]);
        transferred = transferred.saturating_add(read as u64);
        send_transfer_progress(&job, transferred, total);
    }
    timeout(SFTP_TRANSFER_IO_TIMEOUT, remote.flush())
        .await
        .map_err(|_| anyhow!("Flushing the remote upload timed out"))??;
    timeout(SFTP_TRANSFER_IO_TIMEOUT, remote.shutdown())
        .await
        .map_err(|_| anyhow!("Closing the remote upload timed out"))??;

    let (source_size_after, source_modified_after) = local_file_identity(&job.spec.local_path)?;
    if source_size_after != total || source_modified_after != source_modified {
        anyhow::bail!("The local upload source changed during transfer; staging was not published");
    }
    let staged = remote_symlink_metadata(&sftp, &staging)
        .await?
        .ok_or_else(|| anyhow!("The remote upload staging file disappeared"))?;
    if staged.len() != total || !staged.is_regular() || staged.is_symlink() {
        anyhow::bail!("The remote upload staging size or type could not be verified");
    }
    let cleanup_warning = publish_remote_staging(
        &sftp,
        &staging,
        &job.spec.remote_path,
        destination_before.as_ref(),
        job.spec.operation_id,
    )
    .await?;
    let _ = sftp.close().await;
    send_transfer_complete(&job, transferred, resumed_from, hasher, cleanup_warning);
    Ok(())
}

async fn publish_remote_staging(
    sftp: &SftpSession,
    staging: &str,
    destination: &str,
    destination_before: Option<&FileAttributes>,
    operation_id: u64,
) -> Result<bool> {
    let mut cleanup_warning = false;
    if let Some(expected) = destination_before {
        let current = remote_symlink_metadata(sftp, destination)
            .await?
            .ok_or_else(|| anyhow!("The remote destination changed before publish"))?;
        if !same_remote_identity(expected, &current) {
            anyhow::bail!("The remote destination changed before publish; replace was refused");
        }
        let backup = format!("{destination}.termirust-{operation_id}.backup");
        if remote_symlink_metadata(sftp, &backup).await?.is_some() {
            anyhow::bail!("An app-owned remote backup already exists; publish was refused");
        }
        sftp.rename(destination.to_string(), backup.clone())
            .await
            .context("Unable to stage the existing remote destination for replacement")?;
        if let Err(error) = sftp
            .rename(staging.to_string(), destination.to_string())
            .await
        {
            let _ = sftp.rename(backup, destination.to_string()).await;
            return Err(error).context("Unable to publish the remote upload staging file");
        }
        if sftp.remove_file(backup).await.is_err() {
            cleanup_warning = true;
        }
    } else {
        if remote_symlink_metadata(sftp, destination).await?.is_some() {
            anyhow::bail!("The remote destination appeared during transfer; publish was refused");
        }
        sftp.rename(staging.to_string(), destination.to_string())
            .await
            .context("Unable to publish the remote upload staging file")?;
    }
    Ok(cleanup_warning)
}

async fn run_download(job: SftpTransferJob) -> Result<()> {
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

    let Some((sftp, _handles)) = cancellable_transfer_step(
        &job.cancellation,
        SFTP_TRANSFER_CONNECT_TIMEOUT,
        "SFTP transfer connection timed out",
        open_sftp(job.spec.request.clone(), job.known_hosts.clone()),
    )
    .await?
    else {
        send_transfer_cancelled(&job, 0, false);
        return Ok(());
    };
    sftp.set_timeout(SFTP_TRANSFER_IO_TIMEOUT.as_secs()).await;
    let source_before = remote_symlink_metadata(&sftp, &job.spec.remote_path)
        .await?
        .ok_or_else(|| anyhow!("The remote download source does not exist"))?;
    if source_before.is_symlink() || !source_before.is_regular() {
        anyhow::bail!("The remote download source must be a regular file");
    }
    let total = source_before.len();
    ensure_transfer_size(total)?;
    let destination_before = local_destination_identity(&job.spec.local_path)?;
    let staging = local_staging_path(&job.spec.local_path, job.spec.operation_id)?;
    let staged_before = local_destination_identity(&staging)?;

    if let Some((existing_bytes, _)) = destination_before.as_ref() {
        match job.spec.conflict_policy {
            SftpConflictPolicy::Ask => {
                let resume_available =
                    staged_before.is_some_and(|(size, _)| size > 0 && size < total);
                let _ = job.event_tx.send(SftpEvent::TransferConflict {
                    workspace_id: job.spec.workspace_id,
                    operation_id: job.spec.operation_id,
                    direction: job.spec.direction,
                    existing_bytes: *existing_bytes,
                    resume_available,
                });
                let _ = sftp.close().await;
                return Ok(());
            }
            SftpConflictPolicy::Skip => {
                let _ = job.event_tx.send(SftpEvent::TransferSkipped {
                    workspace_id: job.spec.workspace_id,
                    operation_id: job.spec.operation_id,
                    direction: job.spec.direction,
                });
                let _ = sftp.close().await;
                return Ok(());
            }
            SftpConflictPolicy::Replace | SftpConflictPolicy::Resume => {}
        }
    } else if job.spec.conflict_policy == SftpConflictPolicy::Skip {
        let _ = job.event_tx.send(SftpEvent::TransferSkipped {
            workspace_id: job.spec.workspace_id,
            operation_id: job.spec.operation_id,
            direction: job.spec.direction,
        });
        let _ = sftp.close().await;
        return Ok(());
    }

    let mut remote = sftp
        .open(job.spec.remote_path.clone())
        .await
        .context("Unable to open the remote download source")?;
    let mut hasher = Sha256::new();
    let mut resumed_from = 0;
    if job.spec.conflict_policy == SftpConflictPolicy::Resume {
        let (staged_size, _) = staged_before
            .ok_or_else(|| anyhow!("No app-owned local staging file is available to resume"))?;
        if staged_size > total {
            anyhow::bail!("The local staging file is larger than the remote source");
        }
        resumed_from = staged_size;
        let mut local_prefix = File::open(&staging)
            .with_context(|| format!("Unable to open {}", staging.display()))?;
        let mut local_chunk = vec![0u8; SFTP_TRANSFER_CHUNK_BYTES];
        let mut remote_chunk = vec![0u8; SFTP_TRANSFER_CHUNK_BYTES];
        let mut checked = 0u64;
        while checked < resumed_from {
            if job.cancellation.is_cancelled() {
                send_transfer_cancelled(&job, checked, true);
                let _ = sftp.close().await;
                return Ok(());
            }
            let want =
                usize::try_from((resumed_from - checked).min(SFTP_TRANSFER_CHUNK_BYTES as u64))
                    .unwrap_or(SFTP_TRANSFER_CHUNK_BYTES);
            local_prefix.read_exact(&mut local_chunk[..want])?;
            let Some(_) = cancellable_transfer_step(
                &job.cancellation,
                SFTP_TRANSFER_IO_TIMEOUT,
                "Remote source validation timed out",
                remote.read_exact(&mut remote_chunk[..want]),
            )
            .await?
            else {
                send_transfer_cancelled(&job, checked, true);
                let _ = sftp.close().await;
                return Ok(());
            };
            if local_chunk[..want] != remote_chunk[..want] {
                anyhow::bail!("Resume refused because the staged prefix does not match the source");
            }
            hasher.update(&remote_chunk[..want]);
            checked += want as u64;
        }
    } else if staged_before.is_some() {
        fs::remove_file(&staging)
            .with_context(|| format!("Unable to clear old staging file {}", staging.display()))?;
    }

    let mut local = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(resumed_from == 0)
        .open(&staging)
        .with_context(|| format!("Unable to open staging file {}", staging.display()))?;
    if resumed_from > 0 {
        local.seek(SeekFrom::Start(resumed_from))?;
        timeout(
            SFTP_TRANSFER_IO_TIMEOUT,
            remote.seek(tokio::io::SeekFrom::Start(resumed_from)),
        )
        .await
        .map_err(|_| anyhow!("Seeking the remote source timed out"))??;
    }
    send_transfer_started(&job, total, resumed_from);
    send_transfer_progress(&job, resumed_from, total);
    let mut transferred = resumed_from;
    let mut chunk = vec![0u8; SFTP_TRANSFER_CHUNK_BYTES];
    while transferred < total {
        if job.cancellation.is_cancelled() {
            local.flush()?;
            send_transfer_cancelled(&job, transferred, true);
            let _ = sftp.close().await;
            return Ok(());
        }
        let Some(read) = cancellable_transfer_step(
            &job.cancellation,
            SFTP_TRANSFER_IO_TIMEOUT,
            "Downloading a file chunk timed out",
            remote.read(&mut chunk),
        )
        .await?
        else {
            local.flush()?;
            send_transfer_cancelled(&job, transferred, true);
            let _ = sftp.close().await;
            return Ok(());
        };
        if read == 0 {
            anyhow::bail!("The remote download source ended before its reported size");
        }
        local
            .write_all(&chunk[..read])
            .context("Unable to write the local download staging file")?;
        hasher.update(&chunk[..read]);
        transferred = transferred.saturating_add(read as u64);
        send_transfer_progress(&job, transferred, total);
    }
    local.flush()?;
    local.sync_all()?;

    let source_after = remote_symlink_metadata(&sftp, &job.spec.remote_path)
        .await?
        .ok_or_else(|| anyhow!("The remote source disappeared during download"))?;
    if !same_remote_identity(&source_before, &source_after) {
        anyhow::bail!("The remote source changed during download; staging was not published");
    }
    let staged_after = local_file_identity(&staging)?;
    if staged_after.0 != total {
        anyhow::bail!("The local download staging size could not be verified");
    }
    let cleanup_warning = publish_local_staging(
        &staging,
        &job.spec.local_path,
        destination_before.as_ref(),
        job.spec.operation_id,
    )?;
    let _ = sftp.close().await;
    send_transfer_complete(&job, transferred, resumed_from, hasher, cleanup_warning);
    Ok(())
}

fn publish_local_staging(
    staging: &Path,
    destination: &Path,
    destination_before: Option<&(u64, Option<SystemTime>)>,
    operation_id: u64,
) -> Result<bool> {
    let mut cleanup_warning = false;
    let current = local_destination_identity(destination)?;
    if current.as_ref() != destination_before {
        anyhow::bail!("The local destination changed during transfer; replace was refused");
    }
    if let Some(parent) = destination.parent() {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .ok();
    }
    if destination_before.is_some() {
        let backup = local_backup_path(destination, operation_id)?;
        match fs::symlink_metadata(&backup) {
            Ok(_) => {
                anyhow::bail!("An app-owned local backup already exists; publish was refused")
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Unable to inspect {}", backup.display()));
            }
        }
        fs::rename(destination, &backup).with_context(|| {
            format!("Unable to stage {} for replacement", destination.display())
        })?;
        if let Err(error) = fs::rename(staging, destination) {
            let _ = fs::rename(&backup, destination);
            return Err(error).context("Unable to publish the local download staging file");
        }
        if fs::remove_file(&backup).is_err() {
            cleanup_warning = true;
        }
    } else {
        fs::hard_link(staging, destination)
            .context("Unable to publish the local download without overwriting another file")?;
        if fs::remove_file(staging).is_err() {
            cleanup_warning = true;
        }
    }
    Ok(cleanup_warning)
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
    let worker_event_tx = event_tx.clone();
    if let Err(error) = thread::Builder::new().name(thread_name).spawn(move || {
        let runtime = match Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = worker_event_tx.send(SftpEvent::Error {
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
                let _ = worker_event_tx.send(event);
            }
            Err(error) => {
                let _ = worker_event_tx.send(SftpEvent::Error {
                    workspace_id,
                    operation_id,
                    message: format!("{error:#}"),
                });
            }
        }
    }) {
        let _ = event_tx.send(SftpEvent::Error {
            workspace_id,
            operation_id,
            message: format!("Unable to start the SFTP operation: {error}"),
        });
    }
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
        SFTP_TRANSFER_CHUNK_BYTES, SFTP_TRANSFER_MAX_ACTIVE, SFTP_TRANSFER_MAX_BYTES,
        SFTP_TRANSFER_MAX_QUEUED, SftpConflictPolicy, SftpEvent, SftpTransferDirection,
        SftpTransferManager, SftpTransferSpec, cancellable_transfer_step,
        local_destination_identity, local_file_identity, local_staging_path, publish_local_staging,
        spawn_authorized_key_operation, spawn_delete_path, spawn_list_directory,
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
    use std::io::Read as _;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use std::time::Duration;

    fn sha256(bytes: &[u8]) -> String {
        use sha2::{Digest as _, Sha256};

        super::encode_sha256(&Sha256::digest(bytes))
    }

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

    fn synthetic_sftp_request(port: u16) -> ConnectRequest {
        ConnectRequest {
            session_id: 99,
            title: "Synthetic SFTP".to_string(),
            kind: ConnectionKind::Ssh,
            host: "127.0.0.1".to_string(),
            port,
            username: "test".to_string(),
            auth: Some(AuthConfig::Password {
                password: "not-a-real-secret".to_string(),
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

    fn recv_transfer_complete(
        rx: &Receiver<SftpEvent>,
        operation_id: u64,
    ) -> (SftpTransferDirection, String) {
        let mut previous_progress = 0;
        loop {
            match recv_sftp_event(rx) {
                SftpEvent::TransferQueued {
                    operation_id: actual,
                    ..
                }
                | SftpEvent::TransferStarted {
                    operation_id: actual,
                    ..
                } if actual == operation_id => {}
                SftpEvent::TransferProgress {
                    operation_id: actual,
                    transferred_bytes,
                    total_bytes,
                    ..
                } if actual == operation_id => {
                    assert!(transferred_bytes >= previous_progress);
                    assert!(transferred_bytes <= total_bytes);
                    previous_progress = transferred_bytes;
                }
                SftpEvent::TransferComplete {
                    operation_id: actual,
                    direction,
                    transferred_bytes,
                    sha256,
                    ..
                } if actual == operation_id => {
                    assert_eq!(transferred_bytes, previous_progress);
                    return (direction, sha256);
                }
                SftpEvent::Error {
                    operation_id: actual,
                    message,
                    ..
                } if actual == operation_id => panic!("transfer failed: {message}"),
                event => panic!("unexpected transfer event: {event:?}"),
            }
        }
    }

    fn recv_transfer_terminal(rx: &Receiver<SftpEvent>, operation_id: u64) -> SftpEvent {
        loop {
            let event = recv_sftp_event(rx);
            let (actual, terminal) = match &event {
                SftpEvent::TransferConflict { operation_id, .. }
                | SftpEvent::TransferSkipped { operation_id, .. }
                | SftpEvent::TransferCancelled { operation_id, .. }
                | SftpEvent::TransferComplete { operation_id, .. }
                | SftpEvent::Error { operation_id, .. } => (*operation_id, true),
                SftpEvent::TransferQueued { operation_id, .. }
                | SftpEvent::TransferStarted { operation_id, .. }
                | SftpEvent::TransferProgress { operation_id, .. } => (*operation_id, false),
                _ => continue,
            };
            if actual == operation_id && terminal {
                return event;
            }
        }
    }

    #[test]
    fn transfer_manager_bounds_queue_and_cancels_queued_jobs_immediately() {
        let _isolation = TestIsolation::acquire();
        let manager = SftpTransferManager::default();
        manager.inner.state.lock().unwrap().active = SFTP_TRANSFER_MAX_ACTIVE;
        let known_hosts = Arc::new(KnownHostStore::load().expect("known-host store should load"));
        let (event_tx, event_rx) = mpsc::channel();
        let mut controls = Vec::new();

        for operation_id in 1..=SFTP_TRANSFER_MAX_QUEUED as u64 {
            controls.push(
                manager
                    .enqueue(
                        SftpTransferSpec {
                            workspace_id: 1,
                            operation_id,
                            request: synthetic_sftp_request(9),
                            direction: SftpTransferDirection::Download,
                            local_path: PathBuf::from(format!("/tmp/queued-{operation_id}")),
                            remote_path: format!("/queued-{operation_id}"),
                            conflict_policy: SftpConflictPolicy::Ask,
                        },
                        known_hosts.clone(),
                        event_tx.clone(),
                    )
                    .expect("queue slot should be available"),
            );
        }
        assert_eq!(manager.inner.state.lock().unwrap().queued.len(), 32);
        let rejected = manager.enqueue(
            SftpTransferSpec {
                workspace_id: 1,
                operation_id: 100,
                request: synthetic_sftp_request(9),
                direction: SftpTransferDirection::Download,
                local_path: PathBuf::from("/tmp/rejected"),
                remote_path: "/rejected".to_string(),
                conflict_policy: SftpConflictPolicy::Ask,
            },
            known_hosts.clone(),
            event_tx.clone(),
        );
        assert!(rejected.unwrap_err().to_string().contains("queue is full"));

        controls[7].cancel();
        assert_eq!(manager.inner.state.lock().unwrap().queued.len(), 31);
        let cancelled = recv_transfer_terminal(&event_rx, 8);
        assert!(matches!(
            cancelled,
            SftpEvent::TransferCancelled {
                transferred_bytes: 0,
                staging_retained: false,
                ..
            }
        ));
        manager
            .enqueue(
                SftpTransferSpec {
                    workspace_id: 1,
                    operation_id: 101,
                    request: synthetic_sftp_request(9),
                    direction: SftpTransferDirection::Download,
                    local_path: PathBuf::from("/tmp/replacement"),
                    remote_path: "/replacement".to_string(),
                    conflict_policy: SftpConflictPolicy::Ask,
                },
                known_hosts,
                event_tx,
            )
            .expect("cancelled queue slot should be reusable");
    }

    #[test]
    fn transfer_step_timeout_and_file_ceiling_fail_deterministically() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let cancellation = tokio_util::sync::CancellationToken::new();
        let error = runtime
            .block_on(cancellable_transfer_step(
                &cancellation,
                Duration::from_millis(20),
                "synthetic transfer step timed out",
                std::future::pending::<std::io::Result<()>>(),
            ))
            .unwrap_err();
        assert_eq!(error.to_string(), "synthetic transfer step timed out");

        let _isolation = TestIsolation::acquire();
        let temp = tempfile::TempDir::new().unwrap();
        let oversized = temp.path().join("oversized.bin");
        fs::File::create(&oversized)
            .unwrap()
            .set_len(SFTP_TRANSFER_MAX_BYTES + 1)
            .unwrap();
        let manager = SftpTransferManager::default();
        let known_hosts = Arc::new(KnownHostStore::load().unwrap());
        let (event_tx, event_rx) = mpsc::channel();
        manager
            .enqueue(
                SftpTransferSpec {
                    workspace_id: 1,
                    operation_id: 150,
                    request: synthetic_sftp_request(9),
                    direction: SftpTransferDirection::Upload,
                    local_path: oversized,
                    remote_path: "/oversized.bin".to_string(),
                    conflict_policy: SftpConflictPolicy::Ask,
                },
                known_hosts,
                event_tx,
            )
            .unwrap();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 150),
            SftpEvent::Error { message, .. }
                if message.contains("exceeds the 8589934592 byte per-file limit")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn upload_source_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let regular = temp.path().join("regular.txt");
        let linked = temp.path().join("linked.txt");
        fs::write(&regular, b"content").unwrap();
        symlink(&regular, &linked).unwrap();
        assert!(
            local_file_identity(&linked)
                .unwrap_err()
                .to_string()
                .contains("regular file")
        );
    }

    #[test]
    fn transfer_implementation_keeps_streaming_and_scheduler_limits_visible() {
        let source = include_str!("sftp.rs");
        let start = source.find("impl SftpTransferManager").unwrap();
        let end = source
            .find("pub fn spawn_authorized_key_operation")
            .unwrap();
        let implementation = &source[start..end];
        assert!(implementation.contains("SFTP_TRANSFER_CHUNK_BYTES"));
        assert!(implementation.contains("SFTP_TRANSFER_MAX_ACTIVE"));
        assert!(implementation.contains("SFTP_TRANSFER_MAX_QUEUED"));
        assert!(!implementation.contains("read_to_end"));
        assert!(!implementation.contains("fs::read("));
        assert!(!source.contains("expect(\"unable to spawn SFTP thread\")"));
        assert_eq!(SFTP_TRANSFER_CHUNK_BYTES, 256 * 1024);
    }

    #[test]
    fn active_transfer_cancellation_interrupts_a_stalled_ssh_handshake() {
        let _isolation = TestIsolation::acquire();
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener should bind");
        let port = listener.local_addr().unwrap().port();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("test connection should arrive");
            accepted_tx.send(()).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut buffer = [0u8; 64];
            let _ = stream.read(&mut buffer);
        });
        let manager = SftpTransferManager::default();
        let known_hosts = Arc::new(KnownHostStore::load().expect("known-host store should load"));
        let (event_tx, event_rx) = mpsc::channel();
        let control = manager
            .enqueue(
                SftpTransferSpec {
                    workspace_id: 1,
                    operation_id: 200,
                    request: synthetic_sftp_request(port),
                    direction: SftpTransferDirection::Download,
                    local_path: PathBuf::from("/tmp/stalled-download"),
                    remote_path: "/stalled".to_string(),
                    conflict_policy: SftpConflictPolicy::Ask,
                },
                known_hosts,
                event_tx,
            )
            .expect("transfer should queue");
        accepted_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker should connect to fixture");
        control.cancel();
        let started = std::time::Instant::now();
        let event = recv_transfer_terminal(&event_rx, 200);
        assert!(matches!(event, SftpEvent::TransferCancelled { .. }));
        assert!(started.elapsed() < Duration::from_secs(2));
        server.join().unwrap();
    }

    #[test]
    fn local_staging_publish_revalidates_destination_and_preserves_evidence_on_race() {
        let temp = tempfile::TempDir::new().expect("temp dir should be created");
        let destination = temp.path().join("download.txt");
        let staging = temp.path().join(".download.txt.termirust-1.part");
        fs::write(&destination, "old").unwrap();
        fs::write(&staging, "new").unwrap();
        let expected = local_destination_identity(&destination).unwrap().unwrap();
        fs::write(&destination, "changed after review").unwrap();

        let error = publish_local_staging(&staging, &destination, Some(&expected), 1).unwrap_err();
        assert!(error.to_string().contains("changed during transfer"));
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "changed after review"
        );
        assert_eq!(fs::read_to_string(&staging).unwrap(), "new");
    }

    #[test]
    fn local_staging_publish_without_destination_is_no_overwrite_and_cleans_staging() {
        let temp = tempfile::TempDir::new().unwrap();
        let destination = temp.path().join("download.txt");
        let staging = temp.path().join(".download.txt.termirust-2.part");
        fs::write(&staging, "verified").unwrap();

        let cleanup_warning = publish_local_staging(&staging, &destination, None, 2).unwrap();
        assert!(!cleanup_warning);
        assert_eq!(fs::read_to_string(&destination).unwrap(), "verified");
        assert!(!staging.exists());
    }

    #[cfg(unix)]
    #[test]
    fn local_staging_publish_rejects_a_dangling_backup_path() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let destination = temp.path().join("download.txt");
        let staging = temp.path().join(".download.txt.termirust-3.part");
        let backup = super::local_backup_path(&destination, 3).unwrap();
        fs::write(&destination, "old").unwrap();
        fs::write(&staging, "new").unwrap();
        symlink(temp.path().join("missing"), &backup).unwrap();
        let expected = local_destination_identity(&destination).unwrap().unwrap();

        let error = publish_local_staging(&staging, &destination, Some(&expected), 3).unwrap_err();
        assert!(error.to_string().contains("backup already exists"));
        assert_eq!(fs::read_to_string(&destination).unwrap(), "old");
        assert_eq!(fs::read_to_string(&staging).unwrap(), "new");
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

        let transfers = SftpTransferManager::default();
        let uploaded_remote_path = "/home/termirust/e2e-sftp/upload.txt".to_string();
        let _upload = transfers
            .enqueue(
                SftpTransferSpec {
                    workspace_id: 1,
                    operation_id: 2,
                    request: request.clone(),
                    direction: SftpTransferDirection::Upload,
                    local_path: upload_path,
                    remote_path: uploaded_remote_path.clone(),
                    conflict_policy: SftpConflictPolicy::Ask,
                },
                known_hosts.clone(),
                event_tx.clone(),
            )
            .expect("unable to queue upload");
        let (direction, upload_sha256) = recv_transfer_complete(&event_rx, 2);
        assert_eq!(direction, SftpTransferDirection::Upload);
        assert_eq!(upload_sha256.len(), 64);
        assert_eq!(uploaded_remote_path, "/home/termirust/e2e-sftp/upload.txt");

        let download_path = local_dir.join("downloaded.txt");
        let _download = transfers
            .enqueue(
                SftpTransferSpec {
                    workspace_id: 1,
                    operation_id: 3,
                    request: request.clone(),
                    direction: SftpTransferDirection::Download,
                    local_path: download_path.clone(),
                    remote_path: uploaded_remote_path.clone(),
                    conflict_policy: SftpConflictPolicy::Ask,
                },
                known_hosts.clone(),
                event_tx.clone(),
            )
            .expect("unable to queue download");
        let (direction, download_sha256) = recv_transfer_complete(&event_rx, 3);
        assert_eq!(direction, SftpTransferDirection::Download);
        assert_eq!(download_sha256, upload_sha256);
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
    fn docker_transfer_manager_enforces_conflicts_resume_and_identity_checks() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping Docker transfer-manager conflicts: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        server
            .exec(
                "mkdir -p /home/termirust/e2e-transfer-manager && printf 'remote-old' > /home/termirust/e2e-transfer-manager/upload.bin && chown -R termirust:termirust /home/termirust/e2e-transfer-manager",
            )
            .expect("unable to seed transfer-manager fixture");

        let temp = tempfile::TempDir::new().expect("unable to create transfer temp dir");
        let source_path = temp.path().join("upload.bin");
        let source = (0..700_000)
            .map(|index| ((index * 31) % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(&source_path, &source).expect("unable to write upload source");
        let expected_sha256 = sha256(&source);
        let destination = "/home/termirust/e2e-transfer-manager/upload.bin";
        let manager = SftpTransferManager::default();
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let request = docker_sftp_request(&server);
        let (event_tx, event_rx) = mpsc::channel();
        let upload = |operation_id, conflict_policy| SftpTransferSpec {
            workspace_id: 10,
            operation_id,
            request: request.clone(),
            direction: SftpTransferDirection::Upload,
            local_path: source_path.clone(),
            remote_path: destination.to_string(),
            conflict_policy,
        };

        manager
            .enqueue(
                upload(10, SftpConflictPolicy::Ask),
                known_hosts.clone(),
                event_tx.clone(),
            )
            .unwrap();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 10),
            SftpEvent::TransferConflict {
                existing_bytes: 10,
                resume_available: false,
                ..
            }
        ));
        assert_eq!(
            server.exec(&format!("cat {destination}")).unwrap(),
            "remote-old"
        );

        manager
            .enqueue(
                upload(11, SftpConflictPolicy::Skip),
                known_hosts.clone(),
                event_tx.clone(),
            )
            .unwrap();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 11),
            SftpEvent::TransferSkipped { .. }
        ));
        assert_eq!(
            server.exec(&format!("cat {destination}")).unwrap(),
            "remote-old"
        );

        manager
            .enqueue(
                upload(12, SftpConflictPolicy::Replace),
                known_hosts.clone(),
                event_tx.clone(),
            )
            .unwrap();
        let (_, replace_sha256) = recv_transfer_complete(&event_rx, 12);
        assert_eq!(replace_sha256, expected_sha256);
        assert_eq!(
            server
                .exec(&format!("sha256sum {destination} | cut -d' ' -f1"))
                .unwrap(),
            expected_sha256
        );

        server
            .exec(&format!(
                "cp {destination} {destination}.termirust-13.part && truncate -s 300000 {destination}.termirust-13.part && chown termirust:termirust {destination}.termirust-13.part"
            ))
            .expect("unable to seed resumable remote staging");
        manager
            .enqueue(
                upload(13, SftpConflictPolicy::Ask),
                known_hosts.clone(),
                event_tx.clone(),
            )
            .unwrap();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 13),
            SftpEvent::TransferConflict {
                resume_available: true,
                ..
            }
        ));
        manager
            .enqueue(
                upload(13, SftpConflictPolicy::Resume),
                known_hosts.clone(),
                event_tx.clone(),
            )
            .unwrap();
        let resumed = recv_transfer_terminal(&event_rx, 13);
        assert!(
            matches!(
                resumed,
            SftpEvent::TransferComplete {
                resumed_from: 300_000,
                ref sha256,
                ..
            } if sha256 == &expected_sha256
            ),
            "unexpected resumed upload outcome: {resumed:?}"
        );

        server
            .exec(&format!(
                "printf 'wrong-prefix' > {destination}.termirust-14.part"
            ))
            .expect("unable to seed mismatched remote staging");
        manager
            .enqueue(
                upload(14, SftpConflictPolicy::Resume),
                known_hosts.clone(),
                event_tx.clone(),
            )
            .unwrap();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 14),
            SftpEvent::Error { message, .. }
                if message.contains("staged prefix does not match")
        ));
        assert_eq!(
            server
                .exec(&format!("sha256sum {destination} | cut -d' ' -f1"))
                .unwrap(),
            expected_sha256
        );

        let download_path = temp.path().join("download.bin");
        fs::write(&download_path, b"local-old").unwrap();
        let download = |operation_id, conflict_policy| SftpTransferSpec {
            workspace_id: 10,
            operation_id,
            request: request.clone(),
            direction: SftpTransferDirection::Download,
            local_path: download_path.clone(),
            remote_path: destination.to_string(),
            conflict_policy,
        };
        manager
            .enqueue(
                download(20, SftpConflictPolicy::Ask),
                known_hosts.clone(),
                event_tx.clone(),
            )
            .unwrap();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 20),
            SftpEvent::TransferConflict {
                existing_bytes: 9,
                resume_available: false,
                ..
            }
        ));

        manager
            .enqueue(
                download(21, SftpConflictPolicy::Skip),
                known_hosts.clone(),
                event_tx.clone(),
            )
            .unwrap();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 21),
            SftpEvent::TransferSkipped { .. }
        ));
        assert_eq!(fs::read(&download_path).unwrap(), b"local-old");

        let download_staging = local_staging_path(&download_path, 22).unwrap();
        fs::write(&download_staging, &source[..300_000]).unwrap();
        manager
            .enqueue(
                download(22, SftpConflictPolicy::Ask),
                known_hosts.clone(),
                event_tx.clone(),
            )
            .unwrap();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 22),
            SftpEvent::TransferConflict {
                resume_available: true,
                ..
            }
        ));
        manager
            .enqueue(
                download(22, SftpConflictPolicy::Resume),
                known_hosts.clone(),
                event_tx.clone(),
            )
            .unwrap();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 22),
            SftpEvent::TransferComplete {
                resumed_from: 300_000,
                ref sha256,
                ..
            } if sha256 == &expected_sha256
        ));
        assert_eq!(fs::read(&download_path).unwrap(), source);

        fs::write(&download_path, b"must-survive").unwrap();
        let mismatched_staging = local_staging_path(&download_path, 23).unwrap();
        fs::write(&mismatched_staging, b"wrong-prefix").unwrap();
        manager
            .enqueue(
                download(23, SftpConflictPolicy::Resume),
                known_hosts,
                event_tx,
            )
            .unwrap();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 23),
            SftpEvent::Error { message, .. }
                if message.contains("staged prefix does not match")
        ));
        assert_eq!(fs::read(&download_path).unwrap(), b"must-survive");
        assert!(mismatched_staging.exists());
    }

    #[test]
    fn docker_active_upload_cancellation_does_not_clobber_destination() {
        let _isolation = TestIsolation::acquire();
        if !DockerSshServer::docker_available() {
            eprintln!("skipping Docker active transfer cancellation: Docker is unavailable");
            return;
        }
        let server = DockerSshServer::start().expect("unable to start Docker SSH server");
        let destination = "/home/termirust/cancel-transfer.bin";
        server
            .exec(&format!(
                "printf 'original-destination' > {destination} && chown termirust:termirust {destination}"
            ))
            .expect("unable to seed cancellation destination");
        let temp = tempfile::TempDir::new().expect("unable to create cancellation fixture");
        let source = temp.path().join("cancel-transfer.bin");
        let file = fs::File::create(&source).expect("unable to create sparse upload fixture");
        file.set_len(64 * 1024 * 1024)
            .expect("unable to size sparse upload fixture");
        let manager = SftpTransferManager::default();
        let known_hosts = Arc::new(KnownHostStore::load().expect("unable to load known hosts"));
        let (event_tx, event_rx) = mpsc::channel();
        let control = manager
            .enqueue(
                SftpTransferSpec {
                    workspace_id: 11,
                    operation_id: 30,
                    request: docker_sftp_request(&server),
                    direction: SftpTransferDirection::Upload,
                    local_path: source,
                    remote_path: destination.to_string(),
                    conflict_policy: SftpConflictPolicy::Replace,
                },
                known_hosts,
                event_tx,
            )
            .expect("unable to queue cancellable upload");

        loop {
            match recv_sftp_event(&event_rx) {
                SftpEvent::TransferStarted {
                    operation_id: 30, ..
                } => break,
                SftpEvent::TransferQueued {
                    operation_id: 30, ..
                } => {}
                SftpEvent::Error {
                    operation_id: 30,
                    message,
                    ..
                } => panic!("upload failed before cancellation: {message}"),
                event => panic!("unexpected cancellation setup event: {event:?}"),
            }
        }
        control.cancel();
        assert!(matches!(
            recv_transfer_terminal(&event_rx, 30),
            SftpEvent::TransferCancelled {
                staging_retained: true,
                ..
            }
        ));
        assert_eq!(
            server.exec(&format!("cat {destination}")).unwrap(),
            "original-destination"
        );
        assert_eq!(
            server
                .exec(&format!("test -e {destination}.termirust-30.part; echo $?"))
                .unwrap(),
            "0"
        );
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
