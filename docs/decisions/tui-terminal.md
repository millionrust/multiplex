# Durable terminal attachment in the TUI

Date: 2026-08-30

## Decision

`termirust-tui` may attach to one existing local durable Session through
`termirust-client`. The session Host remains the sole process owner and protocol
authority. The TUI never adopts a PID, launches a replacement process, or stops a
Session when the user detaches or quits.

Run the TUI against the normal TermiRust store:

```sh
cargo run -p termirust-tui --
```

Use `TERMIRUST_CONFIG_DIR=/path` to select an isolated configuration root. Enter
attaches the selected Session. `Ctrl+Space`, then `Esc`, detaches and returns to
the exact fleet row. `i` requests the writer lease while the attached view is
read-only. `r` retries a gap or unavailable attachment immediately.

## Ordering and ownership

The attach worker uses the typed local Host endpoint and performs the existing
A07 snapshot, replay, and live handoff. It accepts only the selected Session ID,
current Host identity, current client generation, and strictly consecutive output
sequences. Duplicates are discarded and gaps remain visible. Terminal bytes are
kept only in the bounded client parser; authoritative replay stays in the Host
journal.

One client may hold the writer lease. Ordinary terminal keys, including `Ctrl+C`
and Tab, are encoded as terminal input only while that lease is current. No input
is retained while detached, unavailable, stale, or view-only. A second client can
acquire the lease only after the first client releases it.

## Focus, paste, and resize

`Ctrl+Space` is a 750 ms leader. Leader plus `Esc` detaches, leader plus Space
sends one NUL, and timeout or another key sends the literal NUL followed by that
key. This keeps application commands out of ordinary terminal input.

Pastes larger than one line or 4 KiB require confirmation outside terminal focus;
every paste is capped at 64 KiB. Paste contents are not previewed or logged.
Resize bursts are coalesced for 50 ms and only the final bounded viewport is sent.

## Failure and retry

Only an unavailable attach/read operation retries automatically. It uses A16
full-jitter exponential backoff with a 250 ms base, 30 second cap, eight attempts,
and 90 second elapsed limit. The UI exposes the next attempt, immediate retry, and
the leader detach sequence. Authentication, permission, protocol, resource,
validation, and sequence-gap failures never retry automatically. Process launch,
input, resize, lease mutation, and detach are never replayed.

## Terminal confinement

The parser is capped at 320 columns, 120 rows, and 1,000 scrollback rows. Rendering
projects parsed cells and styles into the Ratatui viewport. OSC title, clipboard,
and URL sequences cannot reach the outer terminal title, clipboard, browser, or
application chrome. Output, input, screen contents, paths, and clipboard data are
never diagnostics fields. Recording-friendly mode hides user labels and the
terminal viewport.

Fullscreen, inline, normal exit, panic, `SIGINT`, `SIGTERM`, and `SIGHUP` share one
idempotent restoration path for raw mode, cursor visibility, and alternate-screen
ownership.

## Verification

Run:

```sh
./scripts/verify-tui-terminal.sh
```

The suite covers ordering, stale identity, gaps, exit retention, leader and paste
behavior, real PTY replay/input/resize/reconnect/detach, writer-lease contention,
terminal control confinement, bounded replay throughput, and process-level terminal
restoration.
