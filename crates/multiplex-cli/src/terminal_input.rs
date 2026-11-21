//! A terminal window's input, read byte for byte.
//!
//! Every interactive attach reads its window this way rather than as key events. A terminal sends
//! keys in several encodings, mouse and focus reports, pastes, and its answers to the program's
//! own questions; a key-event parser drops what it does not know, and one that waits for the end
//! of a sequence it cannot read swallows every key after it.

#[cfg(unix)]
#[path = "raw_stdin.rs"]
mod raw_stdin;

pub(crate) struct TerminalInput {
    #[cfg(unix)]
    stdin: raw_stdin::RawStdin,
    #[cfg(not(unix))]
    receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
    #[cfg(not(unix))]
    reader: Option<std::thread::JoinHandle<()>>,
}

impl TerminalInput {
    pub(crate) fn stdin() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                stdin: raw_stdin::RawStdin::stdin()?,
            })
        }
        #[cfg(not(unix))]
        {
            let (sender, receiver) = tokio::sync::mpsc::channel(8);
            let reader = std::thread::Builder::new()
                .name("terminal-stdin".into())
                .spawn(move || {
                    let mut console = crate::windows_console::ConsoleInput::default();
                    // A console has no end of input; only a failed read stops this.
                    while let Ok(bytes) = console.read() {
                        if !bytes.is_empty() && sender.blocking_send(bytes).is_err() {
                            break;
                        }
                    }
                })?;
            Ok(Self {
                receiver,
                reader: Some(reader),
            })
        }
    }

    /// The next bytes the window sent, or `None` once its input has ended.
    pub(crate) async fn recv(&mut self) -> Option<Vec<u8>> {
        #[cfg(unix)]
        {
            self.stdin.recv().await
        }
        #[cfg(not(unix))]
        {
            self.receiver.recv().await
        }
    }
}

#[cfg(not(unix))]
impl Drop for TerminalInput {
    /// The reader waits in a console read that only a key would end, and that read would take
    /// whatever is typed next from anything reading the console after this attach. Cancel it.
    /// A read not yet begun cannot be cancelled, so try for a moment; a thread that still has not
    /// ended is left to end with the process rather than holding up the window.
    fn drop(&mut self) {
        self.receiver.close();
        let Some(reader) = self.reader.take() else {
            return;
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        while !reader.is_finished() && std::time::Instant::now() < deadline {
            crate::windows_console::cancel_blocking_read(&reader);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if reader.is_finished() {
            let _ = reader.join();
        }
    }
}

/// The window's size, capped as the Host accepts it, when the terminal reports a real one. Some
/// windows report nothing, or 0x0, until they are drawn; those are left for the next resize
/// rather than shrinking the program to a single cell.
pub(crate) fn window_size() -> Option<(u16, u16)> {
    usable_size(crossterm::terminal::size().ok()?)
}

fn usable_size((columns, rows): (u16, u16)) -> Option<(u16, u16)> {
    (columns >= 10 && rows >= 2).then_some((columns.min(1_000), rows.min(1_000)))
}

/// Tells an attach when its window changes size. On Unix the terminal signals a resize, so the
/// size is read only then; Windows has no such signal, and reads the console's size every 40 ms,
/// which opens and closes a handle to the console each time.
///
/// A resize signalled before the watch began is gone, and starting an attach takes a while (the
/// Host is launched or connected and its output replayed); the app sends its pane's real size in
/// that time. So the first wait compares the window with the starting size at once.
pub(crate) struct WindowResizes {
    size: (u16, u16),
    compared: bool,
    #[cfg(unix)]
    signal: tokio::signal::unix::Signal,
    #[cfg(not(unix))]
    interval: tokio::time::Interval,
}

impl WindowResizes {
    /// Watches from `size`, the size the attach started with.
    pub(crate) fn new(size: (u16, u16)) -> std::io::Result<Self> {
        Ok(Self {
            size,
            compared: false,
            #[cfg(unix)]
            signal: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?,
            #[cfg(not(unix))]
            interval: {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(40));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                interval
            },
        })
    }

    /// The window's next new size. Safe to drop and call again, as `select!` does.
    pub(crate) async fn changed(&mut self) -> (u16, u16) {
        if !self.compared {
            self.compared = true;
            if let Some(size) = window_size()
                && size != self.size
            {
                self.size = size;
                return size;
            }
        }
        loop {
            #[cfg(unix)]
            if self.signal.recv().await.is_none() {
                std::future::pending::<()>().await;
            }
            #[cfg(not(unix))]
            self.interval.tick().await;
            if let Some(size) = window_size()
                && size != self.size
            {
                self.size = size;
                return size;
            }
        }
    }
}

/// What an attach writes its window through, and the console modes reading it byte for byte
/// needs, put back when the returned guard drops. On Windows the console must send VT input and
/// honour VT output, and a program's mouse requests are mirrored into the console's own mode;
/// call this before raw mode snapshots the console's flags.
pub(crate) fn window_output(
    stdout: std::io::StdoutLock<'static>,
) -> std::io::Result<(WindowModes, impl std::io::Write)> {
    #[cfg(windows)]
    {
        let modes = WindowModes {
            _console: crate::windows_console::RawVtModeGuard::enable()?,
        };
        Ok((modes, crate::windows_console::MouseModeWriter::new(stdout)))
    }
    #[cfg(not(windows))]
    {
        Ok((WindowModes {}, stdout))
    }
}

pub(crate) struct WindowModes {
    #[cfg(windows)]
    _console: crate::windows_console::RawVtModeGuard,
}

#[cfg(test)]
mod tests {
    use super::usable_size;

    #[test]
    fn a_window_without_a_real_size_is_left_alone_and_a_huge_one_is_capped() {
        assert_eq!(usable_size((0, 0)), None);
        assert_eq!(usable_size((1, 1)), None);
        assert_eq!(usable_size((9, 24)), None);
        assert_eq!(usable_size((80, 1)), None);
        assert_eq!(usable_size((10, 2)), Some((10, 2)));
        assert_eq!(usable_size((80, 24)), Some((80, 24)));
        assert_eq!(usable_size((1_200, 1_001)), Some((1_000, 1_000)));
    }
}
