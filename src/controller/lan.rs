use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use termirust_controller_listener::{
    ListenerControlCommand, ListenerError, ListenerLaunchDescriptor, ListenerProcessEvent,
    ProcessPairingDecision,
};
use termirust_domain::PairingOfferId;

const CONTROLLER_LISTENER_MODE: &str = "--controller-listener";
const LISTENER_READY_TIMEOUT: Duration = Duration::from_secs(5);
const LISTENER_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_EVENTS_PER_POLL: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListenerProcessError {
    Executable,
    Spawn,
    Descriptor,
    ReadinessTimeout,
    InvalidReadiness,
    Control,
    Event,
    Exited,
}

pub struct ControllerListenerProcess {
    child: Child,
    control: Option<ChildStdin>,
    events: mpsc::Receiver<Result<ListenerProcessEvent, ListenerProcessError>>,
    pub ready_port: u16,
}

impl std::fmt::Debug for ControllerListenerProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControllerListenerProcess")
            .field("child_id", &self.child.id())
            .field("control", &"[OPAQUE]")
            .field("events", &"[OPAQUE]")
            .field("ready_port", &self.ready_port)
            .finish()
    }
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
        let (event_tx, event_rx) = mpsc::sync_channel(MAX_EVENTS_PER_POLL);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let event = ListenerProcessEvent::read(&mut reader)
                    .map_err(|_| ListenerProcessError::Event);
                match event {
                    Ok(Some(event)) => {
                        if event_tx.send(Ok(event)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => return,
                    Err(error) => {
                        let _ = event_tx.send(Err(error));
                        return;
                    }
                }
            }
        });
        let ready_port = match event_rx.recv_timeout(LISTENER_READY_TIMEOUT) {
            Ok(Ok(ListenerProcessEvent::Ready { port, .. })) if port >= 1_024 => port,
            Ok(_) => {
                terminate(&mut child);
                return Err(ListenerProcessError::InvalidReadiness);
            }
            Err(_) => {
                terminate(&mut child);
                return Err(ListenerProcessError::ReadinessTimeout);
            }
        };
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
            events: event_rx,
            ready_port,
        })
    }

    pub fn begin_pairing(&mut self) -> Result<(), ListenerProcessError> {
        self.write_command(&ListenerControlCommand::begin_pairing())
    }

    pub fn decide_pairing(
        &mut self,
        offer_id: PairingOfferId,
        decision: ProcessPairingDecision,
    ) -> Result<(), ListenerProcessError> {
        self.write_command(&ListenerControlCommand::decide_pairing(offer_id, decision))
    }

    pub fn drain_events(&mut self) -> Result<Vec<ListenerProcessEvent>, ListenerProcessError> {
        let mut events = Vec::new();
        for _ in 0..MAX_EVENTS_PER_POLL {
            match self.events.try_recv() {
                Ok(Ok(event)) => events.push(event),
                Ok(Err(error)) => return Err(error),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.is_running() {
                        return Err(ListenerProcessError::Event);
                    }
                    break;
                }
            }
        }
        Ok(events)
    }

    fn write_command(
        &mut self,
        command: &ListenerControlCommand,
    ) -> Result<(), ListenerProcessError> {
        let control = self.control.as_mut().ok_or(ListenerProcessError::Exited)?;
        command
            .write(control)
            .map_err(|_| ListenerProcessError::Control)
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
    fn process_event_stream_accepts_only_typed_bounded_readiness() {
        let event = ListenerProcessEvent::ready(50_000);
        let mut bytes = Vec::new();
        event.write(&mut bytes).unwrap();
        assert_eq!(
            ListenerProcessEvent::read(&mut bytes.as_slice()).unwrap(),
            Some(event)
        );

        for mut line in [
            b"{\"kind\":\"ready\",\"schema_version\":2,\"port\":50000}\n".as_slice(),
            b"{\"kind\":\"ready\",\"schema_version\":1,\"port\":50000,\"extra\":true}\n".as_slice(),
        ] {
            assert!(ListenerProcessEvent::read(&mut line).is_err());
        }
    }
}
