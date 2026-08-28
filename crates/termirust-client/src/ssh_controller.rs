use std::ffi::OsString;
use std::fmt;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

const DEFAULT_SSH_PORT: u16 = 22;
const CANCEL_GRACE: Duration = Duration::from_secs(2);
const WAIT_POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SshOperationClass {
    IdempotentRead,
    Mutation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SshReconnectDecision {
    RetryAfter(Duration),
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SshReconnectPolicy {
    pub maximum_attempts: u8,
    pub maximum_elapsed: Duration,
    pub base_delay: Duration,
    pub maximum_delay: Duration,
}

impl Default for SshReconnectPolicy {
    fn default() -> Self {
        Self {
            maximum_attempts: 8,
            maximum_elapsed: Duration::from_secs(90),
            base_delay: Duration::from_millis(250),
            maximum_delay: Duration::from_secs(10),
        }
    }
}

impl SshReconnectPolicy {
    pub fn decide(
        self,
        operation: SshOperationClass,
        attempts_completed: u8,
        elapsed: Duration,
        entropy: u64,
    ) -> SshReconnectDecision {
        if operation != SshOperationClass::IdempotentRead
            || attempts_completed >= self.maximum_attempts
            || elapsed >= self.maximum_elapsed
        {
            return SshReconnectDecision::Stop;
        }
        let exponent = u32::from(attempts_completed.min(31));
        let cap_millis = self
            .base_delay
            .as_millis()
            .saturating_mul(1_u128 << exponent)
            .min(self.maximum_delay.as_millis());
        let jitter_millis = u128::from(entropy) % cap_millis.saturating_add(1);
        let delay = Duration::from_millis(u64::try_from(jitter_millis).unwrap_or(u64::MAX));
        if elapsed.saturating_add(delay) > self.maximum_elapsed {
            SshReconnectDecision::Stop
        } else {
            SshReconnectDecision::RetryAfter(delay)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KnownHostPolicy {
    Strict,
}

#[derive(Clone, Eq, PartialEq)]
pub struct SshControllerTargetId(String);

impl SshControllerTargetId {
    pub fn new(value: impl Into<String>) -> Result<Self, SshControllerError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || value.starts_with('-')
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(SshControllerError::invalid_target());
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SshControllerTargetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SshControllerTargetId([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ValidatedDnsOrIp(String);

impl ValidatedDnsOrIp {
    pub fn parse(value: &str) -> Result<Self, SshControllerError> {
        if value.is_empty()
            || value.len() > 253
            || value.starts_with('-')
            || value.chars().any(char::is_control)
        {
            return Err(SshControllerError::invalid_target());
        }
        if let Ok(address) = value.parse::<IpAddr>() {
            return Ok(Self(address.to_string()));
        }
        if !value.is_ascii()
            || value.ends_with('.')
            || value.contains([':', '/', '@', '\\', '[', ']'])
            || value.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        {
            return Err(SshControllerError::invalid_target());
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ValidatedDnsOrIp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedDnsOrIp([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ValidatedUser(String);

impl ValidatedUser {
    pub fn parse(value: &str) -> Result<Self, SshControllerError> {
        let mut bytes = value.bytes();
        let Some(first) = bytes.next() else {
            return Err(SshControllerError::invalid_target());
        };
        if value.len() > 64
            || !(first.is_ascii_alphanumeric() || first == b'_')
            || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(SshControllerError::invalid_target());
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ValidatedUser {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedUser([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ControllerClientIdentityRef(String);

impl ControllerClientIdentityRef {
    pub fn new(value: impl Into<String>) -> Result<Self, SshControllerError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 256
            || value.chars().any(char::is_control)
            || !value.starts_with("controller.client.")
        {
            return Err(SshControllerError::invalid_target());
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ControllerClientIdentityRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ControllerClientIdentityRef([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SshControllerTarget {
    pub id: SshControllerTargetId,
    pub host: ValidatedDnsOrIp,
    pub user: Option<ValidatedUser>,
    pub port: u16,
    pub known_host_policy: KnownHostPolicy,
    explicit_port: bool,
}

impl SshControllerTarget {
    pub fn new(
        id: SshControllerTargetId,
        host: ValidatedDnsOrIp,
        user: Option<ValidatedUser>,
        port: Option<u16>,
    ) -> Result<Self, SshControllerError> {
        if port == Some(0) {
            return Err(SshControllerError::invalid_target());
        }
        Ok(Self {
            id,
            host,
            user,
            port: port.unwrap_or(DEFAULT_SSH_PORT),
            known_host_policy: KnownHostPolicy::Strict,
            explicit_port: port.is_some(),
        })
    }
}

impl fmt::Debug for SshControllerTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshControllerTarget")
            .field("id", &self.id)
            .field("route", &"[REDACTED]")
            .field("known_host_policy", &self.known_host_policy)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SshRouteState {
    Disconnected,
    Connecting,
    Authenticating,
    Pairing,
    Ready,
    Reconnecting,
    HostKeyChanged,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SshControllerErrorCode {
    InvalidTarget,
    MissingExecutable,
    SpawnFailed,
    Cancelled,
    ChildExited,
    Io,
}

#[derive(Clone, Eq, PartialEq)]
pub struct SshControllerError {
    pub code: SshControllerErrorCode,
}

impl SshControllerError {
    fn invalid_target() -> Self {
        Self {
            code: SshControllerErrorCode::InvalidTarget,
        }
    }

    fn new(code: SshControllerErrorCode) -> Self {
        Self { code }
    }
}

impl fmt::Debug for SshControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshControllerError")
            .field("code", &self.code)
            .finish()
    }
}

impl fmt::Display for SshControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SSH Controller route failed: {:?}", self.code)
    }
}

impl std::error::Error for SshControllerError {}

pub fn strict_ssh_command_argv(target: &SshControllerTarget) -> Vec<OsString> {
    let mut argv = [
        "ssh",
        "-F",
        "none",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "LocalCommand=none",
        "-o",
        "ProxyCommand=none",
        "-o",
        "ProxyJump=none",
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
        "-o",
        "RequestTTY=no",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    if let Some(user) = &target.user {
        argv.push("-l".into());
        argv.push(user.as_str().into());
    }
    if target.explicit_port {
        argv.push("-p".into());
        argv.push(target.port.to_string().into());
    }
    argv.push(target.host.as_str().into());
    argv.extend(["termirust", "controller-bridge", "--stdio"].map(OsString::from));
    argv
}

pub fn resolve_system_ssh() -> Result<PathBuf, SshControllerError> {
    #[cfg(unix)]
    const CANDIDATES: &[&str] = &["/usr/bin/ssh", "/bin/ssh"];
    #[cfg(windows)]
    const CANDIDATES: &[&str] = &[r"C:\Windows\System32\OpenSSH\ssh.exe"];
    CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or_else(|| SshControllerError::new(SshControllerErrorCode::MissingExecutable))
}

pub struct SshControllerProcess {
    child: Child,
    state: SshRouteState,
}

impl SshControllerProcess {
    pub fn spawn(
        executable: &Path,
        target: &SshControllerTarget,
    ) -> Result<Self, SshControllerError> {
        if !executable.is_absolute() || !executable.is_file() {
            return Err(SshControllerError::new(
                SshControllerErrorCode::MissingExecutable,
            ));
        }
        let argv = strict_ssh_command_argv(target);
        let mut command = Command::new(executable);
        command
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_owned_process_group(&mut command);
        let child = command
            .spawn()
            .map_err(|_| SshControllerError::new(SshControllerErrorCode::SpawnFailed))?;
        Ok(Self {
            child,
            state: SshRouteState::Connecting,
        })
    }

    pub fn state(&self) -> SshRouteState {
        self.state
    }

    pub fn set_state(&mut self, state: SshRouteState) {
        self.state = state;
    }

    pub fn stdin(&mut self) -> Option<&mut ChildStdin> {
        self.child.stdin.as_mut()
    }

    pub fn stdout(&mut self) -> Option<&mut ChildStdout> {
        self.child.stdout.as_mut()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn wait_with_cancellation(
        &mut self,
        cancellation: &CancellationToken,
    ) -> Result<ExitStatus, SshControllerError> {
        loop {
            if cancellation.is_cancelled() {
                self.terminate();
                self.state = SshRouteState::Disconnected;
                return Err(SshControllerError::new(SshControllerErrorCode::Cancelled));
            }
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.state = if status.success() {
                        SshRouteState::Disconnected
                    } else {
                        SshRouteState::Failed
                    };
                    return if status.success() {
                        Ok(status)
                    } else {
                        Err(SshControllerError::new(SshControllerErrorCode::ChildExited))
                    };
                }
                Ok(None) => thread::sleep(WAIT_POLL),
                Err(_) => return Err(SshControllerError::new(SshControllerErrorCode::Io)),
            }
        }
    }

    pub fn terminate(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        terminate_process_group(&mut self.child);
        let deadline = Instant::now() + CANCEL_GRACE;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(WAIT_POLL);
        }
        kill_process_group(&mut self.child);
        let _ = self.child.wait();
    }
}

impl fmt::Debug for SshControllerProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshControllerProcess")
            .field("state", &self.state)
            .field("process", &"[OWNED]")
            .finish()
    }
}

impl Drop for SshControllerProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub struct RemoteControllerSession {
    pub target: SshControllerTarget,
    pub identity: ControllerClientIdentityRef,
    pub state: SshRouteState,
    process: Option<SshControllerProcess>,
}

impl RemoteControllerSession {
    pub fn new(target: SshControllerTarget, identity: ControllerClientIdentityRef) -> Self {
        Self {
            target,
            identity,
            state: SshRouteState::Disconnected,
            process: None,
        }
    }

    pub fn connect(&mut self, executable: &Path) -> Result<(), SshControllerError> {
        if self.process.is_some() {
            return Err(SshControllerError::new(SshControllerErrorCode::SpawnFailed));
        }
        let process = SshControllerProcess::spawn(executable, &self.target)?;
        self.state = process.state();
        self.process = Some(process);
        Ok(())
    }

    pub fn disconnect(&mut self) {
        if let Some(mut process) = self.process.take() {
            process.terminate();
        }
        self.state = SshRouteState::Disconnected;
    }
}

impl fmt::Debug for RemoteControllerSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteControllerSession")
            .field("target", &self.target)
            .field("identity", &self.identity)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

#[cfg(unix)]
fn configure_owned_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
}

#[cfg(windows)]
fn configure_owned_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(unix)]
fn terminate_process_group(child: &mut Child) {
    let process_group = i32::try_from(child.id()).map_or(0, |id| -id);
    if process_group != 0 {
        unsafe {
            libc::kill(process_group, libc::SIGTERM);
        }
    }
}

#[cfg(windows)]
fn terminate_process_group(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(unix)]
fn kill_process_group(child: &mut Child) {
    let process_group = i32::try_from(child.id()).map_or(0, |id| -id);
    if process_group != 0 {
        unsafe {
            libc::kill(process_group, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
fn kill_process_group(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(user: Option<&str>, port: Option<u16>) -> SshControllerTarget {
        SshControllerTarget::new(
            SshControllerTargetId::new("target-1").unwrap(),
            ValidatedDnsOrIp::parse("Example.COM").unwrap(),
            user.map(ValidatedUser::parse).transpose().unwrap(),
            port,
        )
        .unwrap()
    }

    #[test]
    fn strict_argv_is_exact_and_has_only_constant_remote_command_elements() {
        let actual = strict_ssh_command_argv(&target(Some("deploy_user"), Some(2222)))
            .into_iter()
            .map(|value| value.into_string().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            [
                "ssh",
                "-F",
                "none",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ForwardX11=no",
                "-o",
                "PermitLocalCommand=no",
                "-o",
                "LocalCommand=none",
                "-o",
                "ProxyCommand=none",
                "-o",
                "ProxyJump=none",
                "-o",
                "ControlMaster=no",
                "-o",
                "ControlPath=none",
                "-o",
                "RequestTTY=no",
                "-l",
                "deploy_user",
                "-p",
                "2222",
                "example.com",
                "termirust",
                "controller-bridge",
                "--stdio",
            ]
        );
        let default = strict_ssh_command_argv(&target(None, None));
        assert!(!default.iter().any(|value| value == "-l" || value == "-p"));
    }

    #[test]
    fn target_validation_rejects_option_uri_shell_and_control_injection() {
        for host in [
            "-oProxyCommand=bad",
            "ssh://host",
            "user@host",
            "host;touch-x",
            "host name",
            "host\nname",
            "[::1]",
            "a..b",
            "a-.example",
        ] {
            assert_eq!(
                ValidatedDnsOrIp::parse(host).unwrap_err().code,
                SshControllerErrorCode::InvalidTarget,
                "accepted hostile host {host:?}",
            );
        }
        for user in ["-root", "a@host", "a/b", "a b", "a\n"] {
            assert!(
                ValidatedUser::parse(user).is_err(),
                "accepted user {user:?}"
            );
        }
        assert_eq!(
            ValidatedDnsOrIp::parse("2001:db8::1").unwrap().as_str(),
            "2001:db8::1"
        );
    }

    #[test]
    fn debug_output_redacts_route_identity_and_process_details() {
        let target = target(Some("private-user"), Some(2222));
        let identity = ControllerClientIdentityRef::new("controller.client.private").unwrap();
        let debug = format!("{:?}", RemoteControllerSession::new(target, identity));
        for secret in ["private-user", "example.com", "2222", "private"] {
            assert!(!debug.contains(secret));
        }
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn reconnect_policy_retries_only_reads_with_bounded_full_jitter() {
        let policy = SshReconnectPolicy::default();
        assert_eq!(
            policy.decide(SshOperationClass::Mutation, 0, Duration::ZERO, 0),
            SshReconnectDecision::Stop
        );
        assert_eq!(
            policy.decide(SshOperationClass::IdempotentRead, 0, Duration::ZERO, 0),
            SshReconnectDecision::RetryAfter(Duration::ZERO)
        );
        assert_eq!(
            policy.decide(SshOperationClass::IdempotentRead, 8, Duration::ZERO, 0),
            SshReconnectDecision::Stop
        );
        assert_eq!(
            policy.decide(
                SshOperationClass::IdempotentRead,
                1,
                Duration::from_secs(90),
                0,
            ),
            SshReconnectDecision::Stop
        );
        let SshReconnectDecision::RetryAfter(delay) = policy.decide(
            SshOperationClass::IdempotentRead,
            7,
            Duration::from_secs(1),
            u64::MAX,
        ) else {
            panic!("read retry should remain inside the budget")
        };
        assert!(delay <= Duration::from_secs(10));
    }
}
