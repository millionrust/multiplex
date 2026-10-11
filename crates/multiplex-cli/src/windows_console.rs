//! Console modes for the byte-for-byte Windows shell bridge.
//!
//! Crossterm's event reader uses Win32 events. Raw shell attachment uses ReadFile
//! instead, so it must ask ConPTY for VT input and enable VT output before replay.

use std::ffi::c_void;
use std::io::{self, Write};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
    fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
}

/// Saved native handles are integer values so this guard can cross async awaits.
/// Console modes belong to the process, and handles remain owned by the standard streams.
pub(crate) struct RawVtModeGuard {
    saved: [(isize, u32); 2],
}

impl RawVtModeGuard {
    pub(crate) fn enable() -> io::Result<Self> {
        let mut saved = [(0, 0); 2];
        for (index, which) in [(-10_i32) as u32, (-11_i32) as u32].into_iter().enumerate() {
            // SAFETY: GetStdHandle returns a process-owned standard console handle.
            let handle = unsafe { GetStdHandle(which) };
            let mut mode = 0;
            // SAFETY: mode points to a writable u32; the handle is checked by the API.
            if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
                return Err(io::Error::last_os_error());
            }
            saved[index] = (handle as isize, mode);
        }
        let guard = Self { saved };
        // ENABLE_VIRTUAL_TERMINAL_INPUT; processed output with VT processing.
        for (index, flag) in [0x0200, 0x0001 | 0x0004].into_iter().enumerate() {
            let (handle, mode) = guard.saved[index];
            // SAFETY: these handles were validated above and remain process-owned.
            let next_mode = if index == 0 {
                // Extended flags are required to clear Quick Edit. Start without
                // mouse capture; the Host's output enables it only for a TUI.
                (mode | flag | 0x0080) & !(0x0040 | 0x0010)
            } else {
                mode | flag
            };
            if unsafe { SetConsoleMode(handle as *mut c_void, next_mode) } == 0 {
                // Dropping the guard also rolls back a partially enabled mode.
                return Err(io::Error::last_os_error());
            }
        }
        Ok(guard)
    }
}

/// ConPTY consumes VT mouse requests instead of forwarding them through a
/// nested console. Mirror the Host's requested mode into this console's native
/// input mode, which makes ConPTY negotiate mouse reporting with the frontend.
pub(crate) struct MouseModeWriter<W> {
    writer: W,
    parser: vt100::Parser,
    mouse_enabled: bool,
}

impl<W: Write> MouseModeWriter<W> {
    pub(crate) fn new(writer: W) -> Self {
        Self {
            writer,
            parser: vt100::Parser::new(2, 2, 0),
            mouse_enabled: false,
        }
    }
}

impl<W: Write> Write for MouseModeWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.writer.write(bytes)?;
        self.parser.process(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()?;
        let enabled = self.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None;
        if enabled != self.mouse_enabled {
            // SAFETY: the process-owned input handle and writable mode are
            // validated by GetConsoleMode, then used only by SetConsoleMode.
            unsafe {
                let handle = GetStdHandle((-10_i32) as u32);
                let mut mode = 0;
                if GetConsoleMode(handle, &mut mode) == 0 {
                    return Err(io::Error::last_os_error());
                }
                mode = (mode | 0x0200 | 0x0080) & !(0x0040 | 0x0010);
                if enabled {
                    mode |= 0x0010;
                }
                if SetConsoleMode(handle, mode) == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            self.mouse_enabled = enabled;
        }
        Ok(())
    }
}

impl Drop for RawVtModeGuard {
    fn drop(&mut self) {
        for (handle, mode) in self.saved {
            // SAFETY: restore only the process-owned handles captured by this guard.
            let _ = unsafe { SetConsoleMode(handle as *mut c_void, mode) };
        }
    }
}
