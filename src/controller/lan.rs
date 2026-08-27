use std::io::{BufRead as _, BufReader, Read as _};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use termirust_controller_listener::{ListenerError, ListenerLaunchDescriptor};

const CONTROLLER_LISTENER_MODE: &str = "--controller-listener";
const LISTENER_READY_TIMEOUT: Duration = Duration::from_secs(5);
const LISTENER_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_READINESS_LINE_BYTES: u64 = 4 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListenerProcessError {
    Executable,
    Spawn,
    Descriptor,
    ReadinessTimeout,
    InvalidReadiness,
    Exited,
}

pub struct ControllerListenerProcess {
    child: Child,
    control: Option<ChildStdin>,
    pub ready_port: u16,
}

impl std::fmt::Debug for ControllerListenerProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControllerListenerProcess")
            .field("child_id", &self.child.id())
            .field("control", &"[OPAQUE]")
            .field("ready_port", &self.ready_port)
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadyLine {
    schema_version: u16,
    lifecycle: String,
    code: String,
    port: u16,
}

impl ControllerListenerProcess {
    pub fn start(descriptor: &ListenerLaunchDescriptor) -> Result<Self, ListenerProcessError> {
        let executable = listener_executable()?;
        let mut child = Command::new(executable)
            .arg(CONTROLLER_LISTENER_MODE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ListenerProcessError::Spawn)?;
        let mut control = child.stdin.take().ok_or(ListenerProcessError::Spawn)?;
        if descriptor.write(&mut control).is_err() {
            terminate(&mut child);
            return Err(ListenerProcessError::Descriptor);
        }
        let stdout = child.stdout.take().ok_or(ListenerProcessError::Spawn)?;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout)
                .take(MAX_READINESS_LINE_BYTES)
                .read_line(&mut line)
                .map(|_| line);
            let _ = ready_tx.send(result);
        });
        let line = match ready_rx.recv_timeout(LISTENER_READY_TIMEOUT) {
            Ok(Ok(line)) => line,
            Ok(Err(_)) => {
                terminate(&mut child);
                return Err(ListenerProcessError::InvalidReadiness);
            }
            Err(_) => {
                terminate(&mut child);
                return Err(ListenerProcessError::ReadinessTimeout);
            }
        };
        let ready: ReadyLine = serde_json::from_str(&line).map_err(|_| {
            terminate(&mut child);
            ListenerProcessError::InvalidReadiness
        })?;
        if ready.schema_version != 1
            || ready.lifecycle != "ready"
            || ready.code != "controller_listener_ready"
            || ready.port < 1_024
        {
            terminate(&mut child);
            return Err(ListenerProcessError::InvalidReadiness);
        }
        if child
            .try_wait()
            .map_err(|_| ListenerProcessError::Exited)?
            .is_some()
        {
            return Err(ListenerProcessError::Exited);
        }
        Ok(Self {
            child,
            control: Some(control),
            ready_port: ready.port,
        })
    }

    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    pub fn stop(&mut self) {
        self.control.take();
        let deadline = Instant::now() + LISTENER_STOP_TIMEOUT;
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        terminate(&mut self.child);
    }
}

impl Drop for ControllerListenerProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn listener_executable() -> Result<PathBuf, ListenerProcessError> {
    if let Some(path) = std::env::var_os("TERMIRUST_CONTROLLER_LISTENER_BIN") {
        return Ok(PathBuf::from(path));
    }
    std::env::current_exe().map_err(|_| ListenerProcessError::Executable)
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

impl From<ListenerError> for ListenerProcessError {
    fn from(_: ListenerError) -> Self {
        Self::Descriptor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_schema_rejects_wrong_state_code_and_privileged_port() {
        for line in [
            r#"{"schema_version":2,"lifecycle":"ready","code":"controller_listener_ready","port":50000}"#,
            r#"{"schema_version":1,"lifecycle":"failed","code":"controller_listener_ready","port":50000}"#,
            r#"{"schema_version":1,"lifecycle":"ready","code":"wrong","port":50000}"#,
            r#"{"schema_version":1,"lifecycle":"ready","code":"controller_listener_ready","port":80}"#,
        ] {
            let ready: ReadyLine = serde_json::from_str(line).unwrap();
            assert!(
                ready.schema_version != 1
                    || ready.lifecycle != "ready"
                    || ready.code != "controller_listener_ready"
                    || ready.port < 1_024
            );
        }
    }
}
