use anyhow::{Context, Result, anyhow, bail};
use russh::client;
use russh::keys::PublicKey;
use russh::keys::key::PrivateKeyWithHashAlg;
use russh_sftp::client::SftpSession;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use tokio::runtime::Builder;

use crate::credentials;
use crate::models::{AuthConfig, ConnectRequest, JumpHostConnection};
use crate::storage::{HostKeyDecision, KnownHostStore};

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
            sftp.write(remote_path.clone(), &bytes)
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

async fn with_sftp<T, F, Fut>(
    request: ConnectRequest,
    known_hosts: Arc<KnownHostStore>,
    action: F,
) -> Result<T>
where
    F: FnOnce(SftpSession) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let config = Arc::new(client::Config::default());
    let _handles = establish_handles(config, request.clone(), known_hosts).await?;
    let channel = _handles
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

    action(sftp).await
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
            request.address(),
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

async fn authenticate(
    handle: &mut client::Handle<SftpHandler>,
    username: &str,
    auth: &AuthConfig,
) -> Result<()> {
    let auth_result = match auth {
        AuthConfig::Password { password } => handle
            .authenticate_password(username.to_string(), password.clone())
            .await
            .context("Password authentication failed")?,
        AuthConfig::PasswordRef { credential_id } => {
            let password = credentials::load_password(credential_id).with_context(|| {
                format!(
                    "Unable to load password '{}' from the system credential store",
                    credential_id
                )
            })?;

            handle
                .authenticate_password(username.to_string(), password)
                .await
                .context("Password authentication failed")?
        }
        AuthConfig::PrivateKey {
            key_path,
            passphrase,
        } => {
            let key = match russh::keys::load_secret_key(key_path, passphrase.as_deref()) {
                Ok(key) => key,
                Err(error) => {
                    let message = error.to_string();
                    if message.to_ascii_lowercase().contains("encrypt") && passphrase.is_none() {
                        bail!(
                            "Private key '{}' is passphrase-protected. Enter the passphrase in the host editor and try again.",
                            key_path
                        );
                    }

                    return Err(error)
                        .with_context(|| format!("Unable to load private key from {}", key_path));
                }
            };
            let rsa_hash = handle
                .best_supported_rsa_hash()
                .await
                .context("Unable to negotiate an RSA signature algorithm")?
                .flatten();
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash);

            handle
                .authenticate_publickey(username.to_string(), key)
                .await
                .context("Public key authentication failed")?
        }
    };

    if !auth_result.success() {
        bail!("Authentication was rejected by the server");
    }

    Ok(())
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
        first_hop.address(),
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
    address: String,
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

    let mut handle = client::connect(config, address, handler)
        .await
        .context("Unable to open the SSH transport")?;
    authenticate(&mut handle, &username, &auth).await?;
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
    authenticate(&mut handle, &username, &auth).await?;
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
