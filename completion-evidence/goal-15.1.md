# Goal 15.1 Read-only Rust TUI fleet navigation

- Status: complete
- Completed: 2026-08-30
- Branch: `test`
- Implementation commit: `4a4f9107402d345d4ead1dca10780fa537688b38`
- Lock review commit: `7aa9e3d9a89d32ddabb5c3d627978ca4479baa08`
- Focused verifier: `scripts/verify-tui-readonly.sh`

## Delivered

1. Added the separate `termirust-tui` Rust binary with deterministic Project, group, and Session
   navigation, bounded filtering, inspection, refresh cancellation, stable-ID selection, help, and
   truthful empty, partial, unavailable, stale, and recovery-required states.
2. Added `termirust-store::load_fleet_read_only`, which validates and reads only existing typed
   metadata. Tests prove it neither initializes a missing store nor changes an existing store.
3. Added responsive 80-column and wide layouts, a clear below-80x20 diagnostic, no-color,
   recording-friendly, inline, English, expanded pseudo-locale, and bidirectional pseudo-locale
   rendering.
4. Added one idempotent terminal lifecycle for normal exit, terminal loss, panic, SIGINT, SIGTERM,
   and SIGHUP. Fullscreen and inline pseudo-terminal tests verify termios, cursor, and alternate
   screen restoration.

## Authority and dependency review

The normal TUI dependency graph contains typed domain/store reads, Ratatui/Crossterm presentation,
and signal delivery only. The focused verifier rejects `termirust-client`, Host protocol/runtime,
SSH, and PTY runtime dependencies. The binary has no attach, input, process, lifecycle, artifact,
transcript, or store-mutation command.

Exact Ratatui 0.30.2, Crossterm 0.29.0, and signal-hook 0.3.18 versions, licenses, Rust floor,
platform scope, unsafe boundary, and terminal cleanup behavior are recorded in
`docs/decisions/read-only-tui.md`. `cargo deny` passed advisories, bans, licenses, and sources. The
workspace lock change was also reviewed under the controller-security ADR; its security dependency
closure and all protocol vectors remain unchanged, and the dedicated checksum/vector verifier
passes.

## Bounds and measurements

- Project projection cap: 1,000.
- visible Session projection cap: 10,000.
- filter cap: 128 Unicode scalars; projected user labels: 256 scalars.
- event queue: fixed capacity 64; no idle timer, animation, polling, or automatic refresh.
- 10,000-Session in-memory viewport render at 140x40: 514 microseconds on the reference
  Apple-silicon Mac.
- live idle sample after two seconds: 2,960 KiB RSS and 0.0% CPU.
- the pseudo-terminal resource test enforces at most 128 MiB RSS and 5% sampled CPU.

These are reference-machine measurements, not cross-platform performance claims.

## Verification

- `./scripts/verify-tui-readonly.sh`: passed. Store read-only tests: 2; TUI unit/integration tests:
  23; strict TUI Clippy, dependency boundary, formatting, and diff checks: passed.
- `cargo test --workspace --all-targets --locked -- --test-threads=1`: passed uninterrupted after
  the required lock checksum review. The main desktop binary passed 633 tests with 3 intentional
  live-provider ignores; all subsequent workspace crate and integration suites passed.
- `cargo doc --workspace --no-deps --locked`: passed; generated documentation was removed afterward
  to preserve disk headroom.
- `./scripts/verify-rust.sh policy`: passed with only the repository's accepted duplicate-dependency
  warnings.
- `./scripts/verify-controller-security-vectors.sh --check`: passed.
- An initial parallel workspace run hit the pre-existing timing-sensitive SFTP cancellation test;
  its exact rerun passed, and both later serialized workspace runs passed that test.

## Honest platform and accessibility scope

Model and TestBackend coverage is platform-neutral, but live pseudo-terminal, signal, resource, and
restoration evidence was collected only on macOS. Crossterm supports macOS, Linux, and Windows;
native Linux/Windows and screen-reader claims remain release-platform work. Raw terminal content is
not claimed to provide a GUI accessibility tree. Goal 15.1 deliberately provides navigation only;
durable terminal attachment, input, resizing, and lifecycle management remain Goals 15.2 and 15.3.
