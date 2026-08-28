use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use keyring::{Entry, Error as KeyringError};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use termirust_client::{
    AsyncSshControllerProcess, SshControllerErrorCode, SshControllerTarget, resolve_system_ssh,
};
use termirust_controller_listener::{
    ApprovalDecision as WireApprovalDecision, ControllerClientChannel, ControllerCommand,
    ControllerResponse, ControllerSessionSummary, ListenerError, ListenerErrorCode,
    SystemHandshakeEntropy,
};
use termirust_controller_security::{
    CapabilitySet, ControllerCapability, HostStaticPublicKey, StaticPrivateKey,
};
use termirust_domain::{HostFingerprint, HostPublicKey, HostedSessionId, OccupantGeneration};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite};
use zeroize::Zeroize as _;

use crate::{
    ApprovalDecision, Cancellation, CliData, CliError, ControllerRemoteSessionView,
    ControllerSshAction, ControllerSshCommand, ControllerSshData, ErrorCode,
    SshControllerCommandExecutor,
};

const PROFILE_SCHEMA_VERSION: u16 = 1;
const SECRET_SERVICE: &str = "com.termirust.controller.client";
const PROFILE_DIR: &str = "controller-ssh";
const PROFILE_MAX_BYTES: u64 = 16 * 1024;
const INPUT_MAX_BYTES: u64 = 16 * 1024;
const STDERR_MAX_BYTES: u64 = 16 * 1024;
const ROUTE_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_TIMEOUT_MILLIS: u64 = 10_000;
const RESPONSE_LIMIT: usize = 4_096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredControllerProfile {
    schema_version: u16,
    host_public_key: [u8; 32],
    identity_generation: u64,
    revocation_epoch: u64,
    session_generation: u64,
    capability_bits: u16,
    secret_ref: String,
}

impl StoredControllerProfile {
    fn load(config_root: &Path, target: &SshControllerTarget) -> Result<Self, CliError> {
        let route_key = route_key(target);
        let path = config_root
            .join(PROFILE_DIR)
            .join(format!("{route_key}.json"));
        let metadata = fs::metadata(&path).map_err(|_| pairing_required())?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > PROFILE_MAX_BYTES {
            return Err(invalid_profile());
        }
        let bytes = fs::read(path).map_err(|_| invalid_profile())?;
        let profile: Self = serde_json::from_slice(&bytes).map_err(|_| invalid_profile())?;
        if profile.schema_version != PROFILE_SCHEMA_VERSION
            || profile.identity_generation == 0
            || profile.session_generation == 0
            || profile.host_public_key == [0; 32]
            || profile.secret_ref != format!("controller.client.{route_key}")
            || CapabilitySet::from_bits(profile.capability_bits).is_err()
        {
            return Err(invalid_profile());
        }
        Ok(profile)
    }

    fn private_key(&self) -> Result<StaticPrivateKey, CliError> {
        let entry =
            Entry::new(SECRET_SERVICE, &self.secret_ref).map_err(|_| secret_unavailable())?;
        let encoded = entry.get_password().map_err(map_keyring)?;
        let mut decoded = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(encoded)
            .map_err(|_| secret_invalid())?;
        if decoded.len() != 32 || decoded.iter().all(|byte| *byte == 0) {
            decoded.zeroize();
            return Err(secret_invalid());
        }
        let mut bytes = [0; 32];
        bytes.copy_from_slice(&decoded);
        decoded.zeroize();
        let private = StaticPrivateKey::from_bytes(bytes);
        bytes.zeroize();
        Ok(private)
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::from_bits(self.capability_bits)
            .expect("stored profile capability bits were validated")
    }
}

pub struct SystemSshControllerExecutor {
    config_root: PathBuf,
    stdin_lock: Mutex<()>,
}

impl SystemSshControllerExecutor {
    pub fn new(config_root: impl Into<PathBuf>) -> Self {
        Self {
            config_root: config_root.into(),
            stdin_lock: Mutex::new(()),
        }
    }

    fn read_input(&self) -> Result<Vec<u8>, CliError> {
        let _guard = self.stdin_lock.lock().map_err(|_| {
            CliError::new(
                ErrorCode::OperationFailed,
                "standard input is unavailable",
                "Retry the input command with at most 16 KiB on stdin.",
            )
        })?;
        let mut bytes = Vec::new();
        std::io::stdin()
            .lock()
            .take(INPUT_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| {
                CliError::new(
                    ErrorCode::OperationFailed,
                    "unable to read terminal input from stdin",
                    "Pipe one non-empty input payload of at most 16 KiB into this command.",
                )
            })?;
        if bytes.is_empty() || bytes.len() as u64 > INPUT_MAX_BYTES {
            bytes.zeroize();
            return Err(CliError::new(
                ErrorCode::Validation,
                "terminal input must contain between 1 byte and 16 KiB",
                "Pipe one bounded input payload through stdin; input is never accepted in argv.",
            ));
        }
        Ok(bytes)
    }
}

impl SshControllerCommandExecutor for SystemSshControllerExecutor {
    fn execute(
        &self,
        command: ControllerSshCommand,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        if matches!(command.action, ControllerSshAction::Pair) {
            return Err(CliError::new(
                ErrorCode::InteractionRequired,
                "SSH Controller pairing requires confirmation on the Host",
                "Open Remote Devices on the Host and start an SSH pairing request there. JSON mode never prompts or auto-confirms.",
            ));
        }
        let input = if matches!(command.action, ControllerSshAction::Input { .. }) {
            Some(self.read_input()?)
        } else {
            None
        };
        let profile = StoredControllerProfile::load(&self.config_root, &command.target)?;
        let private_key = profile.private_key()?;
        let executable = resolve_system_ssh().map_err(map_spawn)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| route_unavailable())?;
        runtime.block_on(execute_route(
            executable,
            command,
            profile,
            private_key,
            input,
            cancellation,
        ))
    }
}

struct ChildDuplex {
    reader: tokio::process::ChildStdout,
    writer: tokio::process::ChildStdin,
}

impl AsyncRead for ChildDuplex {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().reader).poll_read(context, buffer)
    }
}

impl AsyncWrite for ChildDuplex {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().writer).poll_write(context, bytes)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().writer).poll_shutdown(context)
    }
}

async fn execute_route(
    executable: PathBuf,
    command: ControllerSshCommand,
    profile: StoredControllerProfile,
    private_key: StaticPrivateKey,
    input: Option<Vec<u8>>,
    cancellation: &Cancellation,
) -> Result<CliData, CliError> {
    let mut process =
        AsyncSshControllerProcess::spawn(&executable, &command.target).map_err(map_spawn)?;
    let reader = process.take_stdout().ok_or_else(route_unavailable)?;
    let writer = process.take_stdin().ok_or_else(route_unavailable)?;
    let stderr = process.take_stderr().ok_or_else(route_unavailable)?;
    let stderr_task = tokio::spawn(read_bounded_stderr(stderr));
    let operation = action_name(&command.action).to_owned();
    let route = run_authenticated_action(
        ChildDuplex { reader, writer },
        &command.action,
        &profile,
        private_key,
        input,
    );
    let result = tokio::select! {
        result = tokio::time::timeout(ROUTE_TIMEOUT, route) => {
            result.map_err(|_| CliError::new(
                ErrorCode::Timeout,
                "remote Controller route timed out",
                "Check SSH connectivity and the remote TermiRust bridge, then retry read-only commands.",
            ))?
        }
        () = wait_for_cancellation(cancellation) => Err(CliError::new(
            ErrorCode::Cancelled,
            "remote Controller command was cancelled",
            "The remote Host and durable session continue running.",
        )),
    };
    process.terminate().await;
    let stderr = stderr_task.await.unwrap_or_default();
    let mut data = match result {
        Ok(data) => data,
        Err(error) => return Err(classify_route_error(error, &stderr)),
    };
    data.operation = operation;
    Ok(CliData::ControllerSsh(data))
}

async fn run_authenticated_action(
    stream: ChildDuplex,
    action: &ControllerSshAction,
    profile: &StoredControllerProfile,
    private_key: StaticPrivateKey,
    input: Option<Vec<u8>>,
) -> Result<ControllerSshData, CliError> {
    let mut channel = ControllerClientChannel::connect(
        stream,
        profile.identity_generation,
        profile.revocation_epoch,
        profile.session_generation,
        HostStaticPublicKey(profile.host_public_key),
        private_key,
        profile.capabilities(),
        &mut SystemHandshakeEntropy,
    )
    .await
    .map_err(map_listener)?;
    let granted = channel.granted_capabilities();
    let mut writer_lease = None;
    let sessions = match action {
        ControllerSshAction::Sessions => list_sessions(&mut channel).await?,
        ControllerSshAction::Attach {
            session_id,
            occupant_generation,
            from_sequence,
            columns,
            rows,
            request_control,
        } => {
            attach(
                &mut channel,
                *session_id,
                *occupant_generation,
                *from_sequence,
                *columns,
                *rows,
            )
            .await?;
            if *request_control {
                complete_mutation(
                    &mut channel,
                    ControllerCommand::AcquireWriter {
                        session_id: *session_id,
                        occupant_generation: *occupant_generation,
                    },
                )
                .await?;
                writer_lease = Some("held_until_detach".into());
            } else {
                writer_lease = Some("observer".into());
            }
            Vec::new()
        }
        ControllerSshAction::Input {
            session_id,
            occupant_generation,
        } => {
            attach_default(&mut channel, *session_id, *occupant_generation).await?;
            acquire_writer(&mut channel, *session_id, *occupant_generation).await?;
            complete_mutation(
                &mut channel,
                ControllerCommand::Input {
                    session_id: *session_id,
                    occupant_generation: *occupant_generation,
                    bytes: input.ok_or_else(|| {
                        CliError::new(
                            ErrorCode::Validation,
                            "terminal input is missing",
                            "Pipe one bounded payload through stdin.",
                        )
                    })?,
                },
            )
            .await?;
            writer_lease = Some("released_on_exit".into());
            Vec::new()
        }
        ControllerSshAction::Resize {
            session_id,
            occupant_generation,
            columns,
            rows,
        } => {
            attach_default(&mut channel, *session_id, *occupant_generation).await?;
            acquire_writer(&mut channel, *session_id, *occupant_generation).await?;
            complete_mutation(
                &mut channel,
                ControllerCommand::Resize {
                    session_id: *session_id,
                    occupant_generation: *occupant_generation,
                    columns: u32::from(*columns),
                    rows: u32::from(*rows),
                },
            )
            .await?;
            writer_lease = Some("released_on_exit".into());
            Vec::new()
        }
        ControllerSshAction::Approval {
            session_id,
            occupant_generation,
            approval_id,
            decision,
        } => {
            attach_default(&mut channel, *session_id, *occupant_generation).await?;
            acquire_writer(&mut channel, *session_id, *occupant_generation).await?;
            complete_mutation(
                &mut channel,
                ControllerCommand::Approval {
                    session_id: *session_id,
                    occupant_generation: *occupant_generation,
                    approval_id: *approval_id,
                    decision: match decision {
                        ApprovalDecision::Allow => WireApprovalDecision::Approve,
                        ApprovalDecision::Deny => WireApprovalDecision::Deny,
                    },
                },
            )
            .await?;
            writer_lease = Some("released_on_exit".into());
            Vec::new()
        }
        ControllerSshAction::Detach {
            session_id,
            occupant_generation,
        } => {
            attach_default(&mut channel, *session_id, *occupant_generation).await?;
            let command_id = channel
                .send(
                    ControllerCommand::Detach {
                        session_id: *session_id,
                        occupant_generation: *occupant_generation,
                    },
                    deadline(),
                )
                .await
                .map_err(map_listener)?;
            wait_for(&mut channel, command_id).await?;
            writer_lease = Some("released".into());
            Vec::new()
        }
        ControllerSshAction::Pair => unreachable!("pairing is handled before route startup"),
    };
    Ok(ControllerSshData {
        operation: String::new(),
        route_state: "ready".into(),
        target_label: "SSH target".into(),
        ssh_host_key: "matched".into(),
        host_fingerprint_suffix: Some(
            HostFingerprint::derive(HostPublicKey(profile.host_public_key)).row_suffix(),
        ),
        capabilities: capability_names(granted),
        session_generation: Some(profile.session_generation),
        writer_lease,
        reconnect_attempt: None,
        reconnect_deadline_millis: None,
        sessions,
    })
}

async fn list_sessions(
    channel: &mut ControllerClientChannel<ChildDuplex>,
) -> Result<Vec<ControllerRemoteSessionView>, CliError> {
    let command_id = channel
        .send(
            ControllerCommand::ListSessions {
                offset: 0,
                limit: 1_000,
                expected_revision: None,
            },
            deadline(),
        )
        .await
        .map_err(map_listener)?;
    match wait_for(channel, command_id).await? {
        ControllerResponse::Sessions {
            sessions,
            next_offset,
            ..
        } if next_offset.is_none() => Ok(sessions.into_iter().map(session_view).collect()),
        ControllerResponse::Sessions { .. } => Err(CliError::new(
            ErrorCode::ResourceLimit,
            "remote session list exceeds the 1000 record limit",
            "Use the desktop application to narrow or archive remote sessions.",
        )),
        _ => Err(protocol_failure()),
    }
}

async fn attach_default(
    channel: &mut ControllerClientChannel<ChildDuplex>,
    session_id: HostedSessionId,
    occupant_generation: OccupantGeneration,
) -> Result<(), CliError> {
    attach(
        channel,
        session_id,
        occupant_generation,
        termirust_domain::OutputSequence::ZERO,
        80,
        24,
    )
    .await
}

async fn attach(
    channel: &mut ControllerClientChannel<ChildDuplex>,
    session_id: HostedSessionId,
    occupant_generation: OccupantGeneration,
    from_sequence: termirust_domain::OutputSequence,
    columns: u16,
    rows: u16,
) -> Result<(), CliError> {
    let command_id = channel
        .send(
            ControllerCommand::Attach {
                session_id,
                occupant_generation,
                from_sequence,
                columns: u32::from(columns),
                rows: u32::from(rows),
            },
            deadline(),
        )
        .await
        .map_err(map_listener)?;
    match wait_for(channel, command_id).await? {
        ControllerResponse::Attached { .. } => Ok(()),
        _ => Err(protocol_failure()),
    }
}

async fn acquire_writer(
    channel: &mut ControllerClientChannel<ChildDuplex>,
    session_id: HostedSessionId,
    occupant_generation: OccupantGeneration,
) -> Result<(), CliError> {
    complete_mutation(
        channel,
        ControllerCommand::AcquireWriter {
            session_id,
            occupant_generation,
        },
    )
    .await
}

async fn complete_mutation(
    channel: &mut ControllerClientChannel<ChildDuplex>,
    command: ControllerCommand,
) -> Result<(), CliError> {
    let command_id = channel
        .send(command, deadline())
        .await
        .map_err(map_listener)?;
    match wait_for(channel, command_id).await {
        Ok(ControllerResponse::Completed { applied: true, .. }) => Ok(()),
        Ok(ControllerResponse::Completed { applied: false, .. }) => Err(CliError::new(
            ErrorCode::Conflict,
            "remote command was not applied",
            "Refresh the session generation and writer state before retrying.",
        )),
        Ok(ControllerResponse::Error {
            code,
            completion_unknown,
            ..
        }) => Err(remote_command_error(&code, completion_unknown)),
        Ok(_) => Err(protocol_failure()),
        Err(error) if error.code == ErrorCode::Cancelled => Err(error),
        Err(_) => Err(CliError::new(
            ErrorCode::UnknownCompletion,
            "remote mutation completion is unknown",
            "Do not replay automatically. Reconnect and inspect the authoritative session state.",
        )),
    }
}

async fn wait_for(
    channel: &mut ControllerClientChannel<ChildDuplex>,
    command_id: termirust_domain::CommandId,
) -> Result<ControllerResponse, CliError> {
    for _ in 0..RESPONSE_LIMIT {
        let response = channel.read_response().await.map_err(map_listener)?;
        let matches = match &response {
            ControllerResponse::Sessions { command_id: id, .. }
            | ControllerResponse::Attached { command_id: id, .. }
            | ControllerResponse::Snapshot { command_id: id, .. }
            | ControllerResponse::Completed { command_id: id, .. }
            | ControllerResponse::Detached { command_id: id }
            | ControllerResponse::Error { command_id: id, .. } => *id == command_id,
            ControllerResponse::Output { .. } => false,
        };
        if matches && !matches!(response, ControllerResponse::Snapshot { .. }) {
            return Ok(response);
        }
    }
    Err(CliError::new(
        ErrorCode::ResourceLimit,
        "remote Controller response limit was reached",
        "Detach and retry after reducing terminal output pressure.",
    ))
}

fn session_view(value: ControllerSessionSummary) -> ControllerRemoteSessionView {
    ControllerRemoteSessionView {
        id: value.session_id.to_string(),
        title: value.title,
        lifecycle: value.lifecycle,
        activity: value.activity,
        occupant_generation: value.occupant_generation.map(OccupantGeneration::get),
        last_output_sequence: value.last_output_sequence.get(),
        has_writer: value.has_writer,
        unread: value.unread,
    }
}

async fn read_bounded_stderr(stderr: tokio::process::ChildStderr) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = stderr
        .take(STDERR_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .await;
    bytes.truncate(STDERR_MAX_BYTES as usize);
    bytes
}

async fn wait_for_cancellation(cancellation: &Cancellation) {
    while !cancellation.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn deadline() -> u64 {
    unix_millis().saturating_add(COMMAND_TIMEOUT_MILLIS)
}

fn unix_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

fn route_key(target: &SshControllerTarget) -> String {
    let mut digest = Sha256::new();
    digest.update(b"termirust-ssh-controller-target-v1\0");
    digest.update(target.host.as_str().as_bytes());
    digest.update([0]);
    if let Some(user) = &target.user {
        digest.update(user.as_str().as_bytes());
    }
    digest.update([0]);
    digest.update(target.port.to_be_bytes());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn capability_names(capabilities: CapabilitySet) -> Vec<String> {
    [
        (ControllerCapability::ObserveSessions, "observe_sessions"),
        (ControllerCapability::AttachOutput, "attach_output"),
        (ControllerCapability::SendInput, "send_input"),
        (ControllerCapability::Resize, "resize"),
        (
            ControllerCapability::RespondToApproval,
            "respond_to_approval",
        ),
    ]
    .into_iter()
    .filter_map(|(capability, name)| capabilities.contains(capability).then(|| name.into()))
    .collect()
}

fn action_name(action: &ControllerSshAction) -> &'static str {
    match action {
        ControllerSshAction::Pair => "pair",
        ControllerSshAction::Sessions => "sessions",
        ControllerSshAction::Attach { .. } => "attach",
        ControllerSshAction::Input { .. } => "input",
        ControllerSshAction::Resize { .. } => "resize",
        ControllerSshAction::Approval { .. } => "approval",
        ControllerSshAction::Detach { .. } => "detach",
    }
}

fn classify_route_error(error: CliError, stderr: &[u8]) -> CliError {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if stderr.contains("remote host identification has changed")
        || stderr.contains("host key verification failed") && stderr.contains("offending")
    {
        CliError::new(
            ErrorCode::HostKeyChanged,
            "SSH Host key changed",
            "Stop and verify the server identity independently. Update known_hosts manually only after verification.",
        )
    } else if stderr.contains("no host key is known")
        || stderr.contains("host key verification failed")
    {
        CliError::new(
            ErrorCode::HostKeyUnknown,
            "SSH Host key is not trusted yet",
            "Verify and add the Host key with the system ssh client, then retry. TermiRust never auto-accepts it.",
        )
    } else if stderr.contains("permission denied") {
        CliError::new(
            ErrorCode::AuthenticationDenied,
            "SSH authentication was denied",
            "Verify the system SSH agent/default key and remote OS account outside TermiRust.",
        )
    } else if stderr.contains("command not found") || stderr.contains("not found") {
        CliError::new(
            ErrorCode::BridgeUnavailable,
            "remote TermiRust Controller bridge is unavailable",
            "Install the compatible TermiRust binary for the remote OS user and retry.",
        )
    } else {
        error
    }
}

fn map_spawn(error: termirust_client::SshControllerError) -> CliError {
    match error.code {
        SshControllerErrorCode::InvalidTarget => CliError::new(
            ErrorCode::Validation,
            "SSH Controller target is invalid",
            "Use only a DNS name or IP, optional user, and port.",
        ),
        SshControllerErrorCode::MissingExecutable => CliError::new(
            ErrorCode::Unavailable,
            "system OpenSSH client is unavailable",
            "Install the operating system OpenSSH client in its standard path.",
        ),
        SshControllerErrorCode::Cancelled => CliError::new(
            ErrorCode::Cancelled,
            "SSH Controller route was cancelled",
            "The remote Host and session continue running.",
        ),
        _ => route_unavailable(),
    }
}

fn map_listener(error: ListenerError) -> CliError {
    match error.code {
        ListenerErrorCode::AuthenticationFailed | ListenerErrorCode::Unauthorized => CliError::new(
            ErrorCode::PermissionDenied,
            "Controller authentication or authorization failed",
            "Pair again if the device was revoked or the Host identity changed.",
        ),
        ListenerErrorCode::StaleGeneration => CliError::new(
            ErrorCode::Conflict,
            "Controller session generation is stale",
            "Run sessions again and retry with the current occupant generation.",
        ),
        ListenerErrorCode::WriterLeaseRequired => CliError::new(
            ErrorCode::Conflict,
            "the remote session writer is busy",
            "Detach the current writer or retry as an observer.",
        ),
        ListenerErrorCode::FrameTooLarge
        | ListenerErrorCode::QueueFull
        | ListenerErrorCode::ConnectionLimit => CliError::new(
            ErrorCode::ResourceLimit,
            "remote Controller resource limit was reached",
            "Reduce output/input pressure and retry without automatic mutation replay.",
        ),
        ListenerErrorCode::HandshakeTimeout => CliError::new(
            ErrorCode::Timeout,
            "Controller authentication timed out",
            "Check the remote bridge and retry.",
        ),
        ListenerErrorCode::Cancelled => CliError::new(
            ErrorCode::Cancelled,
            "Controller route was cancelled",
            "The remote Host and session continue running.",
        ),
        ListenerErrorCode::HostUnavailable => CliError::new(
            ErrorCode::Unavailable,
            "authoritative remote Host is unavailable",
            "Keep the remote TermiRust Host running and retry.",
        ),
        _ => protocol_failure(),
    }
}

fn remote_command_error(code: &str, completion_unknown: bool) -> CliError {
    if completion_unknown {
        return CliError::new(
            ErrorCode::UnknownCompletion,
            "remote mutation completion is unknown",
            "Do not replay automatically. Reconnect and inspect the authoritative state.",
        );
    }
    match code {
        "approval_unavailable" => CliError::new(
            ErrorCode::Unavailable,
            "approval response is unavailable for this Host session",
            "Respond in the authoritative Host UI or terminal.",
        ),
        "writer_lease_required" => CliError::new(
            ErrorCode::Conflict,
            "the remote session writer is busy",
            "Detach the current writer before retrying.",
        ),
        _ => CliError::new(
            ErrorCode::OperationFailed,
            "remote Controller command failed",
            "Refresh the session list and inspect the authoritative Host state.",
        ),
    }
}

fn map_keyring(error: KeyringError) -> CliError {
    match error {
        KeyringError::NoEntry => pairing_required(),
        KeyringError::NoStorageAccess(_) => CliError::new(
            ErrorCode::PermissionDenied,
            "Controller device key access was denied",
            "Unlock the system credential store and allow TermiRust access.",
        ),
        KeyringError::BadEncoding(_) => secret_invalid(),
        _ => secret_unavailable(),
    }
}

fn pairing_required() -> CliError {
    CliError::new(
        ErrorCode::InteractionRequired,
        "this SSH target is not paired with the remote Host",
        "Open Remote Devices on the Host and complete SSH Controller pairing first.",
    )
}

fn invalid_profile() -> CliError {
    CliError::new(
        ErrorCode::Incompatible,
        "stored SSH Controller profile is invalid or incompatible",
        "Remove the profile through Remote Devices and pair this target again.",
    )
}

fn secret_invalid() -> CliError {
    CliError::new(
        ErrorCode::Incompatible,
        "stored Controller device key is invalid",
        "Revoke this device on the Host and pair again.",
    )
}

fn secret_unavailable() -> CliError {
    CliError::new(
        ErrorCode::Unavailable,
        "system credential store is unavailable",
        "Unlock the credential store and retry without exporting the device key.",
    )
}

fn route_unavailable() -> CliError {
    CliError::new(
        ErrorCode::Unavailable,
        "unable to start the strict SSH Controller route",
        "Verify system OpenSSH and the remote TermiRust installation.",
    )
}

fn protocol_failure() -> CliError {
    CliError::new(
        ErrorCode::Incompatible,
        "remote Controller protocol response was invalid",
        "Update TermiRust on both systems and pair again if the Host identity changed.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_client::{SshControllerTargetId, ValidatedDnsOrIp, ValidatedUser};

    fn target() -> SshControllerTarget {
        SshControllerTarget::new(
            SshControllerTargetId::new("test").unwrap(),
            ValidatedDnsOrIp::parse("Example.COM").unwrap(),
            Some(ValidatedUser::parse("operator").unwrap()),
            Some(2202),
        )
        .unwrap()
    }

    #[test]
    fn route_key_is_stable_and_does_not_disclose_target_values() {
        let key = route_key(&target());
        assert_eq!(key.len(), 64);
        assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(!key.contains("example"));
        assert_eq!(key, route_key(&target()));
    }

    #[test]
    fn stderr_is_bounded_to_stable_redacted_error_classes() {
        let fallback = protocol_failure();
        for (stderr, code) in [
            (
                "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! private.example",
                ErrorCode::HostKeyChanged,
            ),
            (
                "No host key is known for private.example",
                ErrorCode::HostKeyUnknown,
            ),
            (
                "operator@private.example: Permission denied (publickey)",
                ErrorCode::AuthenticationDenied,
            ),
            ("termirust: command not found", ErrorCode::BridgeUnavailable),
        ] {
            let error = classify_route_error(fallback.clone(), stderr.as_bytes());
            assert_eq!(error.code, code);
            assert!(!error.message.contains("private.example"));
            assert!(!error.hint.contains("private.example"));
        }
    }
}
