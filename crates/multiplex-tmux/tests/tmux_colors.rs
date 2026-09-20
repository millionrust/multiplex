//! What a wrapped tab looks like: a program inside tmux should reach the terminal with the
//! colors it wrote. tmux re-renders every cell, so it can quietly drop 24-bit color, replace
//! non-ASCII text, or lose attributes, and the tab then looks different from an unwrapped one.
//!
//! These tests attach a real tmux client to a pseudo-terminal and read what tmux writes to it.
#![cfg(unix)]

use std::io::Read as _;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use multiplex_tmux::Tmux;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// The blue Claude Code and other programs write as 24-bit color.
const FOREGROUND: &str = "38;2;88;166;255";
const BACKGROUND: &str = "48;2;30;30;30";
const INDEXED: &str = "38;5;75";
/// Printed last, so a reader knows the pane has drawn everything.
const DONE: &str = "COLORS-DONE";

struct WrappedPane {
    output: String,
}

impl WrappedPane {
    /// Runs `printf` inside a tmux session on a private server, with the client attached to a
    /// pseudo-terminal, and returns everything tmux wrote to that terminal.
    fn draw(environment: &[(&str, &str)], truecolor: bool) -> Option<Self> {
        let Ok(tmux) = Tmux::discover() else {
            eprintln!("skipping tmux color test: tmux is unavailable");
            return None;
        };
        // tmux socket paths are limited to about 100 bytes, so stay out of long temp dirs.
        let sockets = tempfile::Builder::new()
            .prefix("tr-color-")
            .tempdir_in("/tmp")
            .unwrap();

        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pseudo-terminal stands in for the terminal app");

        // The same client flags a wrapped tab and the phone's attach start tmux with.
        let features: &[&str] = if truecolor {
            &[multiplex_tmux::appearance::TRUECOLOR_FEATURE]
        } else {
            &[]
        };
        let mut arguments = multiplex_tmux::appearance::client_flags(features);
        arguments.extend(["-f".to_owned(), "/dev/null".to_owned()]);

        let mut command = CommandBuilder::new(tmux.executable());
        command.args(arguments);
        command.args([
            "new-session",
            &format!(
                "printf '\\033[{FOREGROUND}mRGB\\033[0m \\033[1mBOLD\\033[0m \
                 \\033[2mDIM\\033[0m \\033[{INDEXED}mIDX\\033[0m \
                 \\033[{BACKGROUND}mBG\\033[0m \\303\\251 {DONE}'; sleep 2"
            ),
        ]);
        command.env("TMUX_TMPDIR", sockets.path());
        command.env_remove("TMUX");
        command.env_remove("LC_ALL");
        command.env_remove("LANG");
        for (name, value) in environment {
            command.env(name, value);
        }

        let mut child = pty
            .slave
            .spawn_command(command)
            .expect("tmux should start on the pseudo-terminal");
        let mut reader = pty.master.try_clone_reader().expect("reader");
        drop(pty.slave);

        // Read on a thread: the master stays open while tmux runs, so a plain read blocks.
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                    return;
                }
            }
        });

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut output = Vec::new();
        while Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => output.extend_from_slice(&chunk),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if String::from_utf8_lossy(&output).contains(DONE) {
                break;
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::process::Command::new(tmux.executable())
            .args(["-f", "/dev/null", "kill-server"])
            .env("TMUX_TMPDIR", sockets.path())
            .env_remove("TMUX")
            .output();

        let output = String::from_utf8_lossy(&output).into_owned();
        assert!(
            output.contains(DONE),
            "the pane never finished drawing: {output:?}"
        );
        Some(Self { output })
    }

    /// 24-bit color survives only when the client told tmux the terminal can show it.
    fn assert_carries_24_bit_color(&self, context: &str) {
        for expected in [FOREGROUND, BACKGROUND] {
            assert!(
                self.output.contains(expected),
                "{context}: tmux changed {expected}: {:?}",
                self.output
            );
        }
    }

    fn assert_carries_text_and_attributes(&self, context: &str) {
        assert!(
            self.output.contains(INDEXED),
            "{context}: tmux changed {INDEXED}: {:?}",
            self.output
        );
        assert!(
            self.output.contains("\u{1b}[1m") || self.output.contains(";1m"),
            "{context}: bold did not survive: {:?}",
            self.output
        );
        assert!(
            self.output.contains("\u{1b}[2m") || self.output.contains(";2m"),
            "{context}: dim did not survive: {:?}",
            self.output
        );
        assert!(
            self.output.contains('é'),
            "{context}: non-ASCII text did not survive: {:?}",
            self.output
        );
    }
}

/// tmux converts 24-bit color to the nearest of 256 unless the client says the terminal can
/// show it, which tmux 3.2 and 3.3 never work out for themselves.
#[test]
fn a_wrapped_pane_reaches_the_terminal_with_the_colors_it_wrote() {
    let Some(pane) = WrappedPane::draw(
        &[("TERM", "xterm-256color"), ("COLORTERM", "truecolor")],
        true,
    ) else {
        return;
    };
    pane.assert_carries_text_and_attributes("a terminal that reports truecolor");
    pane.assert_carries_24_bit_color("a terminal that reports truecolor");
}

/// A tab started from a LaunchAgent, a cron job, or a bare CI shell has no locale and no
/// COLORTERM. tmux then has to be told to write UTF-8, which is why every client passes -u.
/// Such a terminal has not claimed 24-bit color, so only the text and attributes are pinned.
#[test]
fn a_wrapped_pane_keeps_its_text_and_attributes_without_a_locale() {
    let Some(pane) = WrappedPane::draw(&[("TERM", "xterm-256color")], false) else {
        return;
    };
    pane.assert_carries_text_and_attributes("a terminal with no locale");
}
