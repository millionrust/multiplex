use std::fs::{self, OpenOptions};
use std::io::{IsTerminal as _, Read as _, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use crossterm::event::{
    Event as TerminalEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use futures_util::StreamExt as _;
use keyring::{Entry, Error as KeyringError};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use termirust_client::{
    AsyncSshControllerProcess, SshControllerErrorCode, SshControllerTarget, SshOperationClass,
    SshReconnectDecision, SshReconnectPolicy, resolve_system_ssh,
};
use termirust_controller_listener::{
    ApprovalDecision as WireApprovalDecision, ControllerClientChannel, ControllerCommand,
    ControllerConnectionPurpose, ControllerResponse, ControllerSessionSummary, ListenerError,
    ListenerErrorCode, SshControllerPairingOffer, SystemHandshakeEntropy, pair_controller_client,
};
use termirust_controller_security::{
    CapabilitySet, ControllerCapability, HostStaticPublicKey, StaticPrivateKey,
};
use termirust_domain::{
    ControllerDeviceId, HostFingerprint, HostPublicKey, HostedSessionId, OccupantGeneration,
};
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

#[derive(Clone, Deserialize, Serialize)]
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

    fn from_pairing(
        target: &SshControllerTarget,
        result: &termirust_controller_listener::ControllerClientPairingResult,
    ) -> Self {
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            host_public_key: result.host_public_key.0,
            identity_generation: result.identity_generation,
            revocation_epoch: result.revocation_epoch,
            session_generation: result.session_generation,
            capability_bits: result.capability_bits,
            secret_ref: format!("controller.client.{}", route_key(target)),
        }
    }

    fn save_new(
        &self,
        config_root: &Path,
        target: &SshControllerTarget,
        private_key: &StaticPrivateKey,
    ) -> Result<(), CliError> {
        let directory = config_root.join(PROFILE_DIR);
        prepare_profile_directory(&directory)?;
        let route_key = route_key(target);
        let path = directory.join(format!("{route_key}.json"));
        if path.exists() {
            return Err(CliError::new(
                ErrorCode::Conflict,
                "this SSH target already has a Controller pairing",
                "Use the existing pairing, or revoke and remove it before pairing again.",
            ));
        }
        let entry =
            Entry::new(SECRET_SERVICE, &self.secret_ref).map_err(|_| secret_unavailable())?;
        let mut private_bytes = private_key.copy_for_secret_storage();
        let mut encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(private_bytes);
        private_bytes.zeroize();
        let secret_result = entry.set_password(&encoded).map_err(map_keyring);
        encoded.zeroize();
        secret_result?;

        if let Err(error) = write_profile_atomically(&directory, &path, self) {
            let _ = entry.delete_credential();
            return Err(error);
        }
        Ok(())
    }

    fn remove_new(config_root: &Path, target: &SshControllerTarget) {
        let route_key = route_key(target);
        let path = config_root
            .join(PROFILE_DIR)
            .join(format!("{route_key}.json"));
        let _ = fs::remove_file(path);
        if let Ok(entry) = Entry::new(SECRET_SERVICE, &format!("controller.client.{route_key}")) {
            let _ = entry.delete_credential();
        }
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

    fn confirm_pairing(
        &self,
        sas: &termirust_controller_security::SasCode,
    ) -> Result<bool, ListenerError> {
        let _guard = self
            .stdin_lock
            .lock()
            .map_err(|_| ListenerError::new(ListenerErrorCode::Io))?;
        let mut stderr = std::io::stderr().lock();
        writeln!(stderr, "SSH Controller pairing code: {}", sas.as_str())
            .map_err(ListenerError::from)?;
        writeln!(
            stderr,
            "Compare this exact code with Settings > Remote Devices on the remote Host."
        )
        .map_err(ListenerError::from)?;
        write!(stderr, "Codes match? Type yes to confirm [no]: ").map_err(ListenerError::from)?;
        stderr.flush().map_err(ListenerError::from)?;
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(ListenerError::from)?;
        Ok(answer.trim().eq_ignore_ascii_case("yes"))
    }

    fn pair(
        &self,
        command: ControllerSshCommand,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        if !command.allow_interaction
            || !std::io::stdin().is_terminal()
            || !std::io::stderr().is_terminal()
        {
            return Err(CliError::new(
                ErrorCode::InteractionRequired,
                "SSH Controller pairing requires an interactive terminal",
                "Run the pair command without --json in a terminal, keep Remote Devices open on the Host, and confirm only matching codes.",
            ));
        }
        let profile_path = self
            .config_root
            .join(PROFILE_DIR)
            .join(format!("{}.json", route_key(&command.target)));
        if profile_path.exists() {
            return Err(CliError::new(
                ErrorCode::Conflict,
                "this SSH target already has a Controller pairing",
                "Use the existing pairing, or revoke and remove it before pairing again.",
            ));
        }
        let private_key = random_private_key()?;
        let ephemeral_key = random_private_key()?;
        let device_id = ControllerDeviceId::new();
        let executable = resolve_system_ssh().map_err(map_spawn)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| route_unavailable())?;
        let storage_error = std::cell::RefCell::new(None);
        let local_identity_saved = std::cell::Cell::new(false);
        let route_result = runtime.block_on(execute_pair_route(
            executable,
            &command,
            private_key.clone(),
            ephemeral_key,
            device_id,
            |sas| self.confirm_pairing(sas),
            |result| {
                let profile = StoredControllerProfile::from_pairing(&command.target, result);
                profile
                    .save_new(&self.config_root, &command.target, &private_key)
                    .map_err(|error| {
                        *storage_error.borrow_mut() = Some(error);
                        ListenerError::new(ListenerErrorCode::PermissionDenied)
                    })?;
                local_identity_saved.set(true);
                Ok(())
            },
            cancellation,
        ));
        let result = match route_result {
            Ok(result) => result,
            Err(error) => {
                if local_identity_saved.get() {
                    StoredControllerProfile::remove_new(&self.config_root, &command.target);
                }
                return Err(storage_error.into_inner().unwrap_or(error));
            }
        };
        Ok(CliData::ControllerSsh(ControllerSshData {
            operation: "pair".into(),
            route_state: "ready".into(),
            target_label: "SSH target".into(),
            ssh_host_key: "matched".into(),
            host_fingerprint_suffix: Some(
                HostFingerprint::derive(HostPublicKey(result.host_public_key.0)).row_suffix(),
            ),
            capabilities: capability_names(
                CapabilitySet::from_bits(result.capability_bits).map_err(|_| protocol_failure())?,
            ),
            session_generation: Some(result.session_generation),
            writer_lease: None,
            reconnect_attempt: None,
            reconnect_deadline_millis: None,
            sessions: Vec::new(),
        }))
    }

    pub fn execute_interactive_attach(
        &self,
        mut command: ControllerSshCommand,
        cancellation: &Cancellation,
    ) -> Result<(), CliError> {
        if !command.allow_interaction
            || !std::io::stdin().is_terminal()
            || !std::io::stdout().is_terminal()
            || !std::io::stderr().is_terminal()
        {
            return Err(CliError::new(
                ErrorCode::InteractionRequired,
                "interactive SSH Controller attach requires a terminal",
                "Run attach without --json and without redirecting stdin, stdout, or stderr.",
            ));
        }
        if !matches!(command.action, ControllerSshAction::Attach { .. }) {
            return Err(CliError::new(
                ErrorCode::Usage,
                "interactive execution requires the attach action",
                "Run termirust-cli controller ssh ... attach with a session and generation.",
            ));
        }
        if let Ok((terminal_columns, terminal_rows)) = crossterm::terminal::size()
            && let ControllerSshAction::Attach { columns, rows, .. } = &mut command.action
        {
            *columns = terminal_columns.max(1);
            *rows = terminal_rows.max(1);
        }
        let profile = StoredControllerProfile::load(&self.config_root, &command.target)?;
        let private_key = profile.private_key()?;
        let executable = resolve_system_ssh().map_err(map_spawn)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| route_unavailable())?;
        runtime.block_on(execute_interactive_route(
            executable,
            command,
            profile,
            private_key,
            cancellation,
        ))
    }
}

impl SshControllerCommandExecutor for SystemSshControllerExecutor {
    fn execute(
        &self,
        command: ControllerSshCommand,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        if matches!(command.action, ControllerSshAction::Pair) {
            return self.pair(command, cancellation);
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
        runtime.block_on(execute_route_with_reconnect(
            executable,
            command,
            profile,
            private_key,
            input,
            cancellation,
        ))
    }
}

async fn execute_route_with_reconnect(
    executable: PathBuf,
    command: ControllerSshCommand,
    profile: StoredControllerProfile,
    private_key: StaticPrivateKey,
    input: Option<Vec<u8>>,
    cancellation: &Cancellation,
) -> Result<CliData, CliError> {
    let operation_class = ssh_operation_class(&command.action);
    let policy = SshReconnectPolicy::default();
    let started = Instant::now();
    let mut attempts_completed = 0_u8;
    loop {
        let result = execute_route(
            executable.clone(),
            command.clone(),
            profile.clone(),
            private_key.clone(),
            input.clone(),
            cancellation,
        )
        .await;
        match result {
            Ok(mut data) => {
                if attempts_completed > 0
                    && let CliData::ControllerSsh(route) = &mut data
                {
                    route.reconnect_attempt = Some(attempts_completed);
                }
                return Ok(data);
            }
            Err(error) if !retryable_route_error(&error) => return Err(error),
            Err(error) => {
                let mut entropy = [0_u8; 8];
                rand::rngs::OsRng
                    .try_fill_bytes(&mut entropy)
                    .map_err(|_| route_unavailable())?;
                match policy.decide(
                    operation_class,
                    attempts_completed,
                    started.elapsed(),
                    u64::from_be_bytes(entropy),
                ) {
                    SshReconnectDecision::RetryAfter(delay) => {
                        attempts_completed = attempts_completed.saturating_add(1);
                        tokio::select! {
                            () = tokio::time::sleep(delay) => {}
                            () = wait_for_cancellation(cancellation) => {
                                return Err(CliError::new(
                                    ErrorCode::Cancelled,
                                    "remote Controller reconnect was cancelled",
                                    "The remote Host and durable session continue running.",
                                ));
                            }
                        }
                    }
                    SshReconnectDecision::Stop => return Err(error),
                }
            }
        }
    }
}

fn ssh_operation_class(action: &ControllerSshAction) -> SshOperationClass {
    match action {
        ControllerSshAction::Sessions
        | ControllerSshAction::Attach {
            request_control: false,
            ..
        } => SshOperationClass::IdempotentRead,
        _ => SshOperationClass::Mutation,
    }
}

fn retryable_route_error(error: &CliError) -> bool {
    matches!(error.code, ErrorCode::Unavailable | ErrorCode::Timeout)
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

async fn execute_pair_route<F, G>(
    executable: PathBuf,
    command: &ControllerSshCommand,
    private_key: StaticPrivateKey,
    ephemeral_key: StaticPrivateKey,
    device_id: ControllerDeviceId,
    confirm_sas: F,
    prepare_registration: G,
    cancellation: &Cancellation,
) -> Result<termirust_controller_listener::ControllerClientPairingResult, CliError>
where
    F: FnOnce(&termirust_controller_security::SasCode) -> Result<bool, ListenerError>,
    G: FnOnce(
        &termirust_controller_listener::ControllerClientPairingResult,
    ) -> Result<(), ListenerError>,
{
    let mut process =
        AsyncSshControllerProcess::spawn(&executable, &command.target).map_err(map_spawn)?;
    let reader = process.take_stdout().ok_or_else(route_unavailable)?;
    let writer = process.take_stdin().ok_or_else(route_unavailable)?;
    let stderr = process.take_stderr().ok_or_else(route_unavailable)?;
    let stderr_task = tokio::spawn(read_bounded_stderr(stderr));
    let mut stream = ChildDuplex { reader, writer };
    let route = async {
        ControllerConnectionPurpose::Pair
            .write_to(&mut stream)
            .await
            .map_err(map_listener)?;
        let offer = SshControllerPairingOffer::read_from(&mut stream)
            .await
            .map_err(map_listener)?;
        pair_controller_client(
            &mut stream,
            offer,
            private_key,
            ephemeral_key,
            device_id,
            "TermiRust CLI".into(),
            confirm_sas,
            prepare_registration,
        )
        .await
        .map_err(map_listener)
    };
    let result = tokio::select! {
        result = tokio::time::timeout(ROUTE_TIMEOUT, route) => {
            result.map_err(|_| CliError::new(
                ErrorCode::Timeout,
                "SSH Controller pairing timed out",
                "Keep Remote Devices open on the Host and retry the pair command.",
            ))?
        }
        () = wait_for_cancellation(cancellation) => Err(CliError::new(
            ErrorCode::Cancelled,
            "SSH Controller pairing was cancelled",
            "No pairing was saved; the remote Host and its sessions continue running.",
        )),
    };
    process.terminate().await;
    let stderr = stderr_task.await.unwrap_or_default();
    result.map_err(|error| classify_route_error(error, &stderr))
}

async fn execute_interactive_route(
    executable: PathBuf,
    command: ControllerSshCommand,
    profile: StoredControllerProfile,
    private_key: StaticPrivateKey,
    cancellation: &Cancellation,
) -> Result<(), CliError> {
    let mut process =
        AsyncSshControllerProcess::spawn(&executable, &command.target).map_err(map_spawn)?;
    let reader = process.take_stdout().ok_or_else(route_unavailable)?;
    let writer = process.take_stdin().ok_or_else(route_unavailable)?;
    let stderr = process.take_stderr().ok_or_else(route_unavailable)?;
    let stderr_task = tokio::spawn(read_bounded_stderr(stderr));
    let stream = ChildDuplex { reader, writer };
    let connected = tokio::time::timeout(
        ROUTE_TIMEOUT,
        ControllerClientChannel::connect(
            stream,
            profile.identity_generation,
            profile.revocation_epoch,
            profile.session_generation,
            HostStaticPublicKey(profile.host_public_key),
            private_key,
            profile.capabilities(),
            &mut SystemHandshakeEntropy,
        ),
    )
    .await
    .map_err(|_| {
        CliError::new(
            ErrorCode::Timeout,
            "remote Controller authentication timed out",
            "Check SSH connectivity and retry attach.",
        )
    })?
    .map_err(map_listener);
    let result = match connected {
        Ok(mut channel) => {
            run_interactive_attach(&mut channel, &command.action, cancellation).await
        }
        Err(error) => Err(error),
    };
    process.terminate().await;
    let stderr = stderr_task.await.unwrap_or_default();
    result.map_err(|error| classify_route_error(error, &stderr))
}

async fn run_interactive_attach(
    channel: &mut ControllerClientChannel<ChildDuplex>,
    action: &ControllerSshAction,
    cancellation: &Cancellation,
) -> Result<(), CliError> {
    let ControllerSshAction::Attach {
        session_id,
        occupant_generation,
        from_sequence,
        columns,
        rows,
        request_control,
    } = action
    else {
        return Err(protocol_failure());
    };
    let mut stdout = std::io::stdout().lock();
    let attach_id = channel
        .send(
            ControllerCommand::Attach {
                session_id: *session_id,
                occupant_generation: *occupant_generation,
                from_sequence: *from_sequence,
                columns: u32::from(*columns),
                rows: u32::from(*rows),
            },
            deadline(),
        )
        .await
        .map_err(map_listener)?;
    let mut last_sequence = *from_sequence;
    wait_for_interactive_response(
        channel,
        attach_id,
        InteractiveExpected::Attached,
        *session_id,
        &mut last_sequence,
        &mut stdout,
    )
    .await?;
    if *request_control {
        let command_id = channel
            .send(
                ControllerCommand::AcquireWriter {
                    session_id: *session_id,
                    occupant_generation: *occupant_generation,
                },
                deadline(),
            )
            .await
            .map_err(map_listener)?;
        wait_for_interactive_response(
            channel,
            command_id,
            InteractiveExpected::Completed,
            *session_id,
            &mut last_sequence,
            &mut stdout,
        )
        .await?;
    }
    let mode = if *request_control {
        "writer input enabled"
    } else {
        "read-only observer"
    };
    writeln!(
        std::io::stderr(),
        "Attached to the authoritative Host ({mode}). Detach with Ctrl-] then d."
    )
    .map_err(|_| output_unavailable())?;
    enable_raw_mode().map_err(|_| terminal_mode_unavailable())?;
    let _raw_mode = RawModeGuard;
    let mut events = EventStream::new();
    let mut leader = false;
    let mut input = Vec::new();
    let mut resize = None;
    let mut pending_mutation = None;
    let mut detach_requested = false;
    loop {
        if pending_mutation.is_none() && *request_control {
            let command = if !input.is_empty() {
                Some(ControllerCommand::Input {
                    session_id: *session_id,
                    occupant_generation: *occupant_generation,
                    bytes: std::mem::take(&mut input),
                })
            } else {
                resize
                    .take()
                    .map(|(columns, rows)| ControllerCommand::Resize {
                        session_id: *session_id,
                        occupant_generation: *occupant_generation,
                        columns,
                        rows,
                    })
            };
            if let Some(command) = command {
                pending_mutation = Some(
                    channel
                        .send(command, deadline())
                        .await
                        .map_err(map_listener)?,
                );
            }
        }
        if detach_requested && pending_mutation.is_none() {
            break;
        }
        tokio::select! {
            response = channel.read_response() => {
                match response.map_err(map_listener)? {
                    ControllerResponse::Output { session_id: response_session, sequence, bytes }
                        if response_session == *session_id => {
                            write_live_output(&mut stdout, &mut last_sequence, sequence, &bytes)?;
                        }
                    ControllerResponse::Completed { command_id, applied: true }
                        if pending_mutation == Some(command_id) => {
                            pending_mutation = None;
                        }
                    ControllerResponse::Completed { command_id, applied: false }
                        if pending_mutation == Some(command_id) => {
                            return Err(CliError::new(
                                ErrorCode::Conflict,
                                "remote terminal input was not applied",
                                "Detach, refresh the session generation and writer state, then retry.",
                            ));
                        }
                    ControllerResponse::Error { command_id, code, completion_unknown }
                        if pending_mutation == Some(command_id) => {
                            return Err(remote_command_error(&code, completion_unknown));
                        }
                    _ => return Err(protocol_failure()),
                }
            }
            event = events.next() => {
                let event = event
                    .ok_or_else(terminal_mode_unavailable)?
                    .map_err(|_| terminal_mode_unavailable())?;
                match event {
                    TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => {
                        if is_cancel_key(&key) {
                            detach_requested = true;
                            continue;
                        }
                        if leader {
                            leader = false;
                            if matches!(key.code, KeyCode::Char('d' | 'D'))
                                && key.modifiers.is_empty()
                            {
                                detach_requested = true;
                                continue;
                            }
                            if *request_control {
                                input.push(0x1d);
                            }
                        } else if is_leader_key(&key) {
                            leader = true;
                            continue;
                        }
                        if *request_control {
                            append_key_bytes(&mut input, key)?;
                            if input.len() > INPUT_MAX_BYTES as usize {
                                return Err(CliError::new(
                                    ErrorCode::ResourceLimit,
                                    "interactive terminal input queue is full",
                                    "Wait for the remote Host to catch up, then reattach.",
                                ));
                            }
                        }
                    }
                    TerminalEvent::Paste(bytes) if *request_control => {
                        if input.len().saturating_add(bytes.len()) > INPUT_MAX_BYTES as usize {
                            return Err(CliError::new(
                                ErrorCode::ResourceLimit,
                                "interactive paste exceeds the 16 KiB input limit",
                                "Paste a smaller payload or send it in separate chunks.",
                            ));
                        }
                        input.extend_from_slice(bytes.as_bytes());
                    }
                    TerminalEvent::Resize(columns, rows) if *request_control => {
                        resize = Some((u32::from(columns.max(1)), u32::from(rows.max(1))));
                    }
                    _ => {}
                }
            }
            () = wait_for_cancellation(cancellation), if !detach_requested => {
                detach_requested = true;
            }
        }
    }
    if *request_control {
        let command_id = channel
            .send(
                ControllerCommand::ReleaseWriter {
                    session_id: *session_id,
                    occupant_generation: *occupant_generation,
                },
                deadline(),
            )
            .await
            .map_err(map_listener)?;
        wait_for_interactive_response(
            channel,
            command_id,
            InteractiveExpected::Completed,
            *session_id,
            &mut last_sequence,
            &mut stdout,
        )
        .await?;
    }
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
    wait_for_interactive_response(
        channel,
        command_id,
        InteractiveExpected::Detached,
        *session_id,
        &mut last_sequence,
        &mut stdout,
    )
    .await
}

#[derive(Clone, Copy)]
enum InteractiveExpected {
    Attached,
    Completed,
    Detached,
}

async fn wait_for_interactive_response(
    channel: &mut ControllerClientChannel<ChildDuplex>,
    expected_id: termirust_domain::CommandId,
    expected: InteractiveExpected,
    session_id: HostedSessionId,
    last_sequence: &mut termirust_domain::OutputSequence,
    stdout: &mut impl Write,
) -> Result<(), CliError> {
    let mut snapshot_chunk = 0;
    let mut snapshot_chunks = None;
    for _ in 0..RESPONSE_LIMIT {
        match channel.read_response().await.map_err(map_listener)? {
            ControllerResponse::Snapshot {
                command_id,
                session_id: response_session,
                boundary_sequence,
                chunk_index,
                chunk_count,
                bytes,
                ..
            } if command_id == expected_id
                && response_session == session_id
                && matches!(expected, InteractiveExpected::Attached) =>
            {
                if chunk_count == 0
                    || usize::try_from(chunk_count).unwrap_or(usize::MAX) > RESPONSE_LIMIT
                    || snapshot_chunks.is_some_and(|count| count != chunk_count)
                    || chunk_index != snapshot_chunk
                {
                    return Err(protocol_failure());
                }
                snapshot_chunks = Some(chunk_count);
                snapshot_chunk = snapshot_chunk.saturating_add(1);
                stdout.write_all(&bytes).map_err(|_| output_unavailable())?;
                stdout.flush().map_err(|_| output_unavailable())?;
                *last_sequence = boundary_sequence;
            }
            ControllerResponse::Output {
                session_id: response_session,
                sequence,
                bytes,
            } if response_session == session_id => {
                write_live_output(stdout, last_sequence, sequence, &bytes)?;
            }
            ControllerResponse::Attached { command_id, .. }
                if command_id == expected_id
                    && matches!(expected, InteractiveExpected::Attached) =>
            {
                if snapshot_chunks.is_some_and(|count| snapshot_chunk != count) {
                    return Err(protocol_failure());
                }
                return Ok(());
            }
            ControllerResponse::Completed {
                command_id,
                applied: true,
            } if command_id == expected_id
                && matches!(expected, InteractiveExpected::Completed) =>
            {
                return Ok(());
            }
            ControllerResponse::Detached { command_id }
                if command_id == expected_id
                    && matches!(expected, InteractiveExpected::Detached) =>
            {
                return Ok(());
            }
            ControllerResponse::Completed {
                command_id,
                applied: false,
            } if command_id == expected_id => {
                return Err(CliError::new(
                    ErrorCode::Conflict,
                    "remote Controller command was not applied",
                    "Refresh the session generation and writer state before retrying.",
                ));
            }
            ControllerResponse::Error {
                command_id,
                code,
                completion_unknown,
            } if command_id == expected_id => {
                return Err(remote_command_error(&code, completion_unknown));
            }
            _ => return Err(protocol_failure()),
        }
    }
    Err(CliError::new(
        ErrorCode::ResourceLimit,
        "interactive response limit was exceeded",
        "Detach and retry after remote output pressure subsides.",
    ))
}

fn write_live_output(
    stdout: &mut impl Write,
    last_sequence: &mut termirust_domain::OutputSequence,
    sequence: termirust_domain::OutputSequence,
    bytes: &[u8],
) -> Result<(), CliError> {
    if sequence <= *last_sequence {
        return Ok(());
    }
    if last_sequence.checked_next() != Some(sequence) {
        return Err(CliError::new(
            ErrorCode::Conflict,
            "remote terminal output has a sequence gap",
            "Detach and reattach from an earlier output sequence to recover the snapshot.",
        ));
    }
    stdout.write_all(bytes).map_err(|_| output_unavailable())?;
    stdout.flush().map_err(|_| output_unavailable())?;
    *last_sequence = sequence;
    Ok(())
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

fn is_leader_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(']')) && key.modifiers == KeyModifiers::CONTROL
}

fn is_cancel_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c' | 'C')) && key.modifiers == KeyModifiers::CONTROL
}

fn append_key_bytes(bytes: &mut Vec<u8>, key: KeyEvent) -> Result<(), CliError> {
    let mut encoded = Vec::with_capacity(8);
    match key.code {
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if character.is_ascii() {
                encoded.push((character.to_ascii_uppercase() as u8) & 0x1f);
            } else {
                return Ok(());
            }
        }
        KeyCode::Char(character) => {
            let mut buffer = [0; 4];
            encoded.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
        }
        KeyCode::Enter => encoded.push(b'\r'),
        KeyCode::Tab => encoded.push(b'\t'),
        KeyCode::BackTab => encoded.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => encoded.push(0x7f),
        KeyCode::Esc => encoded.push(0x1b),
        KeyCode::Left => encoded.extend_from_slice(b"\x1b[D"),
        KeyCode::Right => encoded.extend_from_slice(b"\x1b[C"),
        KeyCode::Up => encoded.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => encoded.extend_from_slice(b"\x1b[B"),
        KeyCode::Home => encoded.extend_from_slice(b"\x1b[H"),
        KeyCode::End => encoded.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => encoded.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => encoded.extend_from_slice(b"\x1b[6~"),
        KeyCode::Delete => encoded.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => encoded.extend_from_slice(b"\x1b[2~"),
        KeyCode::F(number) if (1..=4).contains(&number) => {
            encoded.extend_from_slice([0x1b, b'O', b'P' + number - 1].as_slice());
        }
        KeyCode::F(number) if (5..=12).contains(&number) => {
            let sequence = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(number - 5)];
            encoded.extend_from_slice(format!("\x1b[{sequence}~").as_bytes());
        }
        _ => return Ok(()),
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(&encoded);
    Ok(())
}

fn terminal_mode_unavailable() -> CliError {
    CliError::new(
        ErrorCode::Unavailable,
        "interactive terminal mode is unavailable",
        "Run attach from a supported local terminal and retry.",
    )
}

fn output_unavailable() -> CliError {
    CliError::new(
        ErrorCode::OperationFailed,
        "interactive terminal output is unavailable",
        "Restore stdout and reattach; the remote Host session continues running.",
    )
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

fn random_private_key() -> Result<StaticPrivateKey, CliError> {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.try_fill_bytes(&mut bytes).map_err(|_| {
        CliError::new(
            ErrorCode::Unavailable,
            "secure random generation is unavailable",
            "Restore operating-system randomness and retry pairing.",
        )
    })?;
    if bytes == [0; 32] {
        return Err(CliError::new(
            ErrorCode::Unavailable,
            "secure random generation returned an invalid key",
            "Restart the operating system before retrying pairing.",
        ));
    }
    let key = StaticPrivateKey::from_bytes(bytes);
    bytes.zeroize();
    Ok(key)
}

#[cfg(unix)]
fn prepare_profile_directory(path: &Path) -> Result<(), CliError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(profile_storage_denied());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| profile_storage_denied())?;
        }
        Err(_) => return Err(profile_storage_denied()),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| profile_storage_denied())?;
    let metadata = fs::symlink_metadata(path).map_err(|_| profile_storage_denied())?;
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(profile_storage_denied());
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_profile_directory(path: &Path) -> Result<(), CliError> {
    fs::create_dir_all(path).map_err(|_| profile_storage_denied())
}

fn write_profile_atomically(
    directory: &Path,
    path: &Path,
    profile: &StoredControllerProfile,
) -> Result<(), CliError> {
    let bytes = serde_json::to_vec_pretty(profile).map_err(|_| invalid_profile())?;
    if bytes.is_empty() || bytes.len() as u64 > PROFILE_MAX_BYTES {
        return Err(invalid_profile());
    }
    let temporary = directory.join(format!(".pairing-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = open_private_profile(&temporary)?;
        file.write_all(&bytes)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|_| profile_storage_denied())?;
        fs::rename(&temporary, path).map_err(|_| profile_storage_denied())?;
        fs::File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| profile_storage_denied())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn open_private_profile(path: &Path) -> Result<fs::File, CliError> {
    use std::os::unix::fs::OpenOptionsExt as _;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| profile_storage_denied())
}

#[cfg(not(unix))]
fn open_private_profile(path: &Path) -> Result<fs::File, CliError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| profile_storage_denied())
}

fn profile_storage_denied() -> CliError {
    CliError::new(
        ErrorCode::PermissionDenied,
        "unable to save the SSH Controller profile securely",
        "Check the TermiRust configuration directory permissions and retry pairing.",
    )
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
        ListenerErrorCode::Io => CliError::new(
            ErrorCode::Unavailable,
            "remote Controller connection was interrupted",
            "Check SSH connectivity; read-only commands retry within the bounded route budget.",
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

    #[test]
    fn public_pairing_profile_is_atomic_bounded_and_contains_no_private_key() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join(PROFILE_DIR);
        prepare_profile_directory(&directory).unwrap();
        let result = termirust_controller_listener::ControllerClientPairingResult {
            device_id: ControllerDeviceId::new(),
            host_public_key: HostStaticPublicKey([7; 32]),
            identity_generation: 3,
            revocation_epoch: 4,
            session_generation: 5,
            capability_bits: CapabilitySet::default()
                .with(ControllerCapability::ObserveSessions)
                .bits(),
        };
        let profile = StoredControllerProfile::from_pairing(&target(), &result);
        let path = directory.join(format!("{}.json", route_key(&target())));
        write_profile_atomically(&directory, &path, &profile).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.len() as u64 <= PROFILE_MAX_BYTES);
        assert!(!bytes.windows(32).any(|window| window == [9; 32]));
        let loaded = StoredControllerProfile::load(temp.path(), &target()).unwrap();
        assert_eq!(loaded.host_public_key, [7; 32]);
        assert_eq!(loaded.secret_ref, profile.secret_ref);
        assert!(
            !directory
                .read_dir()
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn interactive_keys_encode_xterm_bytes_and_reserve_the_detach_leader() {
        let mut bytes = Vec::new();
        append_key_bytes(
            &mut bytes,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        )
        .unwrap();
        append_key_bytes(&mut bytes, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)).unwrap();
        append_key_bytes(
            &mut bytes,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        )
        .unwrap();
        assert_eq!(bytes, b"a\x1b[A\x18");
        assert!(is_leader_key(&KeyEvent::new(
            KeyCode::Char(']'),
            KeyModifiers::CONTROL,
        )));
        assert!(is_cancel_key(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
    }

    #[test]
    fn interactive_output_drops_duplicates_and_fails_on_gaps() {
        let mut output = Vec::new();
        let mut sequence = termirust_domain::OutputSequence::new(4);
        write_live_output(
            &mut output,
            &mut sequence,
            termirust_domain::OutputSequence::new(5),
            b"five",
        )
        .unwrap();
        write_live_output(
            &mut output,
            &mut sequence,
            termirust_domain::OutputSequence::new(5),
            b"duplicate",
        )
        .unwrap();
        assert_eq!(output, b"five");
        assert_eq!(
            write_live_output(
                &mut output,
                &mut sequence,
                termirust_domain::OutputSequence::new(7),
                b"gap",
            )
            .unwrap_err()
            .code,
            ErrorCode::Conflict
        );
    }

    #[test]
    fn production_retry_classification_excludes_every_mutation() {
        assert_eq!(
            ssh_operation_class(&ControllerSshAction::Sessions),
            SshOperationClass::IdempotentRead
        );
        assert_eq!(
            ssh_operation_class(&ControllerSshAction::Pair),
            SshOperationClass::Mutation
        );
        assert!(retryable_route_error(&route_unavailable()));
        assert!(!retryable_route_error(&CliError::new(
            ErrorCode::AuthenticationDenied,
            "denied",
            "verify credentials",
        )));
    }
}
