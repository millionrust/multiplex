# Remote Terminals — Completion Evidence

Branch `feat/remote-terminals`, based on `test` at `ae1d6af`. Plan:
[docs/remote-terminals-implementation-plan.md](../remote-terminals-implementation-plan.md).
Recorded 2026-09-15 on macOS (Darwin 27.0), tmux 3.7c, rustc 1.97.1.

## Result by milestone

| Milestone | State | Commit |
| --------- | ----- | ------ |
| M1 tmux discovery and attach | Done | `ab6d7fc` |
| M2 opt-in setup flow (macOS, Linux) | Done | `c5f70af` |
| M3 Windows `termirust shell` and profile writer | **Not done — blocked** | — |
| M4 route parity (SSH, relay) | Done, ADR added | `16bf937` |
| M5 background listener | Done for macOS; Windows not done | `ee69913` |

### Deviations from the plan, and why

- **Input does not use `tmux send-keys`.** Spike 1 showed tmux 3.7c rejects `send-keys` while a
  read-only client is attached (`client is read-only`), and rejects `=name` as a pane target.
  The phone's client attaches with `attach-session -f ignore-size -t $ID` and types through the
  Session Host PTY under the existing writer lease. With a 120×40 desktop client attached, an
  80×24 phone client typed input and the window stayed 120×40.
- **Attach hosts run in the listener process**, shared per tmux session, instead of spawned
  host binaries. No host-binary discovery per route, and nothing survives the listener.
- **Occupant generation advances per host instance**, so a phone holding a watermark from an
  earlier host is refused as stale; a new host replays from zero.

### M3 blocker

`termirust shell` needs a Session-Host-owned ConPTY. `termirust-session-host` does not compile
for Windows today (`tokio::net::UnixListener`, `libc::kill`, `O_NOFOLLOW`; reproduced with
`cargo check --target x86_64-pc-windows-msvc -p termirust-controller-listener`), and the CI job
"windows runtime and sidecars" already fails on `origin/test` (run 34681192937) with those
errors. There is also no Windows machine here to run anything. Porting the Session Host to
Windows is a prerequisite project, not part of this plan.

## Commands and results

`DEVELOPER_DIR=/Library/Developer/CommandLineTools` was exported for every command: after an OS
update mid-session, Xcode's license was not accepted, which blocks linking and `/usr/bin/git`.

### Tests written for this work

| Command | Result |
| ------- | ------ |
| `cargo test -p termirust-tmux` | PASS — 26 unit, 11 process (real isolated tmux server, fake binaries, real zsh and bash landing in tmux) |
| `cargo test -p termirust-controller-listener --test tmux_sessions` | PASS — 7 authenticated end-to-end tests, including the SSH/relay bridge route |
| `cargo test -p termirust-controller-listener --lib` | PASS — 43, including pointer file and listener-ownership tests |
| `cargo test -p termirust --bin termirust controller::` | PASS — background service supervisor (6) and bridge sources (1) |
| `cargo test -p termirust --bin termirust e2e_remote_terminal_setup` | PASS |
| `cargo test -p termirust --bin termirust e2e_background_listener_row` | PASS |
| `cargo test -p termirust --bin termirust local::tests` | PASS — 9, including the real tmux reattach test after moving discovery to `termirust-tmux` |

### Repetition (timing-sensitive tests)

| Command | Result |
| ------- | ------ |
| 25× `cargo test -p termirust-controller-listener --test tmux_sessions` plus `-p termirust-tmux` | 25/25 |
| 6 parallel copies of the `tmux_sessions` test binary | 6/6 |
| 25× after adding the bridge-route test | 25/25 |
| `./scripts/test/repeat.sh 25 ui::app::tests::e2e_remote_terminal_setup_previews_applies_and_removes_shell_changes` | 25/25 |
| `./scripts/test/repeat.sh 25 ui::app::tests::e2e_background_listener_row_installs_and_removes_through_the_service` | 25/25 |
| 25× `shell_integration_starts_new_terminal_app_shells_inside_tmux` | 25/25 |
| 25× `controller::background_service` tests | 25/25 |

A mutation check confirmed the shell test is real: renaming the wrapper's session prefix made
it fail; restoring it passed.

### Gates

| Command | Result |
| ------- | ------ |
| `./scripts/verify/rust.sh focused` | PASS at the final commit (needs `/private/tmp/termirust-502`, see below) |
| `./scripts/verify/rust.sh workspace` | FAIL at `cargo test`, pre-existing failures only (below); later steps run separately |
| `cargo test --workspace --all-targets --locked --no-fail-fast` | 1,579 passed, 73 failed across 136 binaries; all 73 failures in the desktop binary and pre-existing |
| `cargo doc --workspace --no-deps` | PASS |
| `./scripts/verify/rust.sh policy` | FAIL, pre-existing: CDLA-Permissive-2.0 license (`webpki-root-certs`) and RUSTSEC-2026-0285 (`rustls`, published in today's advisory database). This work adds no external crate |
| `TERMIRUST_CLIPPY_BASE=ae1d6af python3 scripts/dev/clippy-changed.py` | PASS — changed Rust lines are Clippy-clean |
| `cargo fmt --all -- --check` | PASS |
| `python3 scripts/verify/gpui-boundaries.py` | PASS — `termirust-tmux` added to the governed set |
| `git diff --check ae1d6af HEAD` | PASS |
| `./scripts/verify/localization.sh --locales en-US,en-XA,ar-XB --no-new-baseline` | PASS |
| `./scripts/verify/design-tokens.sh --all-ui --no-new-baseline` | FAIL, pre-existing: the same 5 fingerprints fail at `ae1d6af` (QR-code literals in `remote_devices.rs`, two test durations in `mod.rs`); the new UI file adds none |
| `./scripts/verify/ui-surface.sh --surface settings ...` | FAIL at the same 3 pre-existing `remote_devices.rs` literals; its localization and `settings_surface` contract tests pass when run individually |
| `./scripts/verify/controller-lan.sh` | PASS (run after the listener lock was added) |
| `./scripts/verify/local-controller-conformance.sh` | PASS |
| `./scripts/verify/controller-security-vectors.sh --check` | PASS — `cargo_lock_sha256` updated for the new workspace member |
| `./scripts/verify/remote-route-acceptance.sh` | Rust part PASS (24 lifecycles, 6 switch pairs); FAIL in Gradle — `GRADLE_USER_HOME=/Volumes/Footages/.gradle` is on an unmounted volume |

### Pre-existing failures, identified

- 72 desktop tests: the Docker SSH/SFTP fixtures cannot bind-mount `tests/fixtures/` because
  `DOCKER_HOST` points at a remote daemon. Reported as environmental, not as passed.
- `ui::app::tests::e2e_keychain_rendered_empty_add_and_use_button_clicks`: failed at `ae1d6af`
  before this work.
- `termirust-cli --test session_input` (3 tests) failed until `/private/tmp/termirust-502`
  existed; the machine had just rebooted and the desktop app normally creates that directory.
  Created with mode 0700; then 7/7 passed.
- `scripts/verify/*` `--paths` mode resolves `src/ui/...` from the repository root and cannot
  find desktop files since the repository restructure.
- The MSRV job (`cargo +1.88.0`) fails because of `termirust-slate`; untouched, pending decision.

## Manual checks

- `termirust controller-service status|run` with an isolated `TERMIRUST_CONFIG_DIR` and a
  disabled route: runs and waits, a second instance is refused (`service.already_running`),
  and it restarts cleanly after SIGTERM with a stale socket.
- **Not performed:** the plan's manual phone run (open a wrapped Terminal.app tab, see it on a
  paired phone, take the writer lease, type). No paired phone is available in this session.
  **Not performed:** installing the LaunchAgent on this Mac or launching the desktop app with a
  fresh configuration, both of which would change the user's launchd registrations or Keychain.
