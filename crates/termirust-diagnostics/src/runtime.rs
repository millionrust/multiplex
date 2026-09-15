use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::Diagnostic;
use crate::export::{ExportCancellation, ExportError, PreparedExport, prepare_export};
use crate::storage::{DiagnosticPolicy, DiagnosticUsage, Writer, current_unix_ms};

const MANAGEMENT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DiagnosticStatus {
    Disabled = 0,
    Healthy = 1,
    Dropping = 2,
    DiskError = 3,
}

impl DiagnosticStatus {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Healthy,
            2 => Self::Dropping,
            3 => Self::DiskError,
            _ => Self::Disabled,
        }
    }
}

enum Command {
    Record {
        diagnostic: Diagnostic,
        dropped_before: u64,
    },
    Flush(mpsc::Sender<Result<(), ()>>),
    Clear(mpsc::Sender<Result<(), ()>>),
    SetPolicy(DiagnosticPolicy, mpsc::Sender<Result<(), ()>>),
    Shutdown,
}

#[derive(Clone)]
pub struct DiagnosticHandle {
    sender: SyncSender<Command>,
    root: Arc<PathBuf>,
    status: Arc<AtomicU8>,
    dropped: Arc<AtomicU64>,
}

impl DiagnosticHandle {
    pub fn record(&self, diagnostic: Diagnostic) -> bool {
        if diagnostic.validate().is_err() || self.status() == DiagnosticStatus::Disabled {
            return false;
        }
        let dropped_before = self.dropped.swap(0, Ordering::AcqRel);
        match self.sender.try_send(Command::Record {
            diagnostic,
            dropped_before,
        }) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped
                    .fetch_add(dropped_before.saturating_add(1), Ordering::Relaxed);
                self.status
                    .store(DiagnosticStatus::Dropping as u8, Ordering::Release);
                false
            }
        }
    }

    #[must_use]
    pub fn status(&self) -> DiagnosticStatus {
        DiagnosticStatus::from_u8(self.status.load(Ordering::Acquire))
    }

    #[must_use]
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Acquire)
    }

    pub fn flush(&self) -> Result<(), ExportError> {
        self.management(Command::Flush)
    }

    pub fn clear(&self) -> Result<(), ExportError> {
        self.management(Command::Clear)
    }

    pub fn set_policy(&self, policy: DiagnosticPolicy) -> Result<(), ExportError> {
        self.management(|reply| Command::SetPolicy(policy, reply))
    }

    pub fn usage(&self) -> Result<DiagnosticUsage, ExportError> {
        crate::storage::usage(&self.root).map_err(ExportError::from_io)
    }

    pub fn prepare_export(&self) -> Result<PreparedExport, ExportError> {
        self.prepare_export_with_cancellation(&ExportCancellation::default())
    }

    pub fn prepare_export_with_cancellation(
        &self,
        cancellation: &ExportCancellation,
    ) -> Result<PreparedExport, ExportError> {
        self.flush()?;
        prepare_export(&self.root, cancellation)
    }

    fn management(
        &self,
        command: impl FnOnce(mpsc::Sender<Result<(), ()>>) -> Command,
    ) -> Result<(), ExportError> {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .try_send(command(sender))
            .map_err(|_| ExportError::runtime_unavailable())?;
        receiver
            .recv_timeout(MANAGEMENT_TIMEOUT)
            .map_err(|_| ExportError::runtime_unavailable())?
            .map_err(|()| ExportError::storage_unavailable())
    }
}

pub struct DiagnosticRuntime {
    handle: DiagnosticHandle,
    thread: Option<JoinHandle<()>>,
}

impl DiagnosticRuntime {
    pub fn start(root: impl Into<PathBuf>, policy: DiagnosticPolicy) -> Result<Self, ExportError> {
        let root = root.into();
        let policy = policy.normalized();
        let writer = Writer::open(root.clone(), policy).map_err(ExportError::from_io)?;
        let (sender, receiver) = mpsc::sync_channel(policy.channel_capacity);
        let status = Arc::new(AtomicU8::new(if policy.enabled {
            DiagnosticStatus::Healthy as u8
        } else {
            DiagnosticStatus::Disabled as u8
        }));
        let dropped = Arc::new(AtomicU64::new(0));
        let handle = DiagnosticHandle {
            sender,
            root: Arc::new(root),
            status: Arc::clone(&status),
            dropped,
        };
        let thread = thread::Builder::new()
            .name("termirust-diagnostics".into())
            .spawn(move || writer_loop(writer, receiver, status))
            .map_err(ExportError::from_io)?;
        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }

    #[must_use]
    pub fn handle(&self) -> DiagnosticHandle {
        self.handle.clone()
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.handle.root
    }
}

impl Drop for DiagnosticRuntime {
    fn drop(&mut self) {
        let _ = self.handle.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn writer_loop(mut writer: Writer, receiver: Receiver<Command>, status: Arc<AtomicU8>) {
    while let Ok(command) = receiver.recv() {
        match command {
            Command::Record {
                diagnostic,
                dropped_before,
            } => {
                let result = writer
                    .append_dropped(dropped_before, current_unix_ms())
                    .and_then(|()| writer.append(&diagnostic));
                set_result_status(&status, writer.policy(), result);
            }
            Command::Flush(reply) => {
                let _ = reply.send(Ok(()));
            }
            Command::Clear(reply) => {
                let result = writer.clear().map_err(|_| ());
                if result.is_err() {
                    status.store(DiagnosticStatus::DiskError as u8, Ordering::Release);
                }
                let _ = reply.send(result);
            }
            Command::SetPolicy(policy, reply) => {
                let result = writer.set_policy(policy).map_err(|_| ());
                let next = if result.is_err() {
                    DiagnosticStatus::DiskError
                } else if policy.enabled {
                    DiagnosticStatus::Healthy
                } else {
                    DiagnosticStatus::Disabled
                };
                status.store(next as u8, Ordering::Release);
                let _ = reply.send(result);
            }
            Command::Shutdown => break,
        }
    }
}

fn set_result_status(status: &AtomicU8, policy: DiagnosticPolicy, result: std::io::Result<()>) {
    let next = if result.is_err() {
        DiagnosticStatus::DiskError
    } else if policy.enabled {
        DiagnosticStatus::Healthy
    } else {
        DiagnosticStatus::Disabled
    };
    status.store(next as u8, Ordering::Release);
}
