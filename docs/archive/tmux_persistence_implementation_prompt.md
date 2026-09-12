# TermiRust Tmux Persistence Implementation Prompt

> Historical planning document, archived after the September 2026 repository
> restructure. This is not an active implementation brief or completion report.
> Original paths and proposals below are preserved for reference; current desktop
> sources live in `crates/termirust-desktop`. Consult current engineering evidence
> before using these instructions.

## Role

You are a senior Rust desktop engineer working inside the TermiRust codebase. Implement persistent terminal sessions using tmux with clean, minimal, production-quality code. Preserve existing behavior unless a change is explicitly required for this feature.

Do not remove, rewrite, or simplify unrelated code. Do not perform broad refactors. Keep each change scoped, readable, and easy to review.

## Goal

Add a per-host persistent session mode so closing TermiRust, closing a tab, losing the network, or reopening the app does not kill the user's shell or long-running process. The durable session must live on the target machine through tmux.

The core behavior:

```text
TermiRust SSH client -> target host -> tmux named session
```

TermiRust should attach to a named tmux session when connecting. If that session already exists, attach to it. If it does not exist, create it.

## Research Basis

Use tmux as the persistence primitive. `tmux new-session -A -s <session>` attaches to an existing named session or creates it if missing.

References:

- https://github.com/tmux/tmux/wiki/Getting-Started
- https://man7.org/linux/man-pages/man1/tmux.1.html
- https://github.com/tmux/tmux/wiki/Control-Mode

## Non-Negotiable Engineering Rules

- Keep the current architecture intact.
- Do not delete existing features, UI, settings, storage fields, SSH behavior, local terminal behavior, SFTP, snippets, vault sync, reconnect, or logs.
- Do not introduce a new terminal emulator, SSH library, storage system, or UI framework.
- Use the repo's existing style and patterns.
- Keep folder structure clean. Add new files only when they reduce real complexity.
- Avoid giant functions. Prefer small helpers in the closest relevant module.
- Avoid clever shell string construction. Build and escape shell commands deliberately.
- Add tests for parsing, model defaults, persistence, shell bootstrap generation, and migrations where applicable.
- Run `cargo fmt` before every commit.
- Run `cargo check` before every commit.
- If tests exist for the touched area, run them. If no targeted tests exist, add focused tests.
- Make regular, clean commits after each coherent milestone.
- Commit without co-author trailers. Do not include `Co-authored-by`.
- Use clear commit messages such as:
  - `Add tmux persistence settings to host profiles`
  - `Generate tmux bootstrap command for SSH sessions`
  - `Add persistent session controls to host editor`
  - `Persist tmux session restore metadata`

## Current Codebase Map

Read these files before editing:

- `src/models.rs`
- `src/storage.rs`
- `src/local.rs`
- `src/terminal.rs`
- `src/ui/app/mod.rs`
- `src/ui/app/editor.rs`
- `src/ui/app/workspace.rs`
- `src/ui/app/connect.rs`
- `src/ui/app/types.rs`
- `src/ui/shell.rs`

Also read `AGENTS.md` and follow all project instructions.

Important implementation anchor points:

- `HostProfile` is the saved host config.
- `DraftProfile` is the host editor's mutable form state.
- `ConnectRequest` is what flows into connection startup.
- `RestorableConnection` is what persists workspace panes across app restarts.
- `startup_bytes_for_request` in `src/ui/shell.rs` is the startup command injection point.
- `shell_single_quote` in `src/ui/shell.rs` is the existing shell escaping helper.
- `send_startup_actions` in `src/ui/app/mod.rs` sends startup bytes into the already-open SSH shell.

For v1, do not modify `src/ssh.rs` unless direct evidence proves it is necessary. Tmux bootstrapping should happen through startup bytes sent to the remote shell; the SSH runtime should remain transparent to this feature.

## Data Model

Add persistent session fields to the host/profile model using the existing flat-field style. Do not introduce a nested `PersistentSessionSettings` struct unless the codebase has already moved to nested settings elsewhere.

Add to `HostProfile` with `#[serde(default)]` for backward compatibility:

```rust
#[serde(default)]
pub persistent_session: bool,
#[serde(default)]
pub persistent_session_name: Option<String>,
#[serde(default)]
pub persistent_session_detach_others: bool,
```

Do not store `install_hint_shown` on `HostProfile`. That is UI state, not host configuration. If it becomes necessary later, put it in app settings or a separate UI-state map.

Add to `DraftProfile` using the editor's existing text-input convention:

```rust
pub persistent_session: bool,
pub persistent_session_name: String,
pub persistent_session_detach_others: bool,
```

Convert `DraftProfile.persistent_session_name` to `HostProfile.persistent_session_name` with the existing `non_empty()` pattern.

Add matching fields to `ConnectRequest`:

```rust
pub persistent_session: bool,
pub persistent_session_name: Option<String>,
pub persistent_session_detach_others: bool,
```

Add matching fields to `RestorableConnection` with serde defaults so workspace restore preserves the tmux behavior:

```rust
#[serde(default)]
pub persistent_session: bool,
#[serde(default)]
pub persistent_session_name: Option<String>,
#[serde(default)]
pub persistent_session_detach_others: bool,
```

Default behavior:

- Persistent mode is off for existing hosts.
- No migration should break old `state.json` files.
- Missing fields must deserialize safely.
- The default tmux session name should be stable per host profile.

Recommended default session name:

```text
tr-<sanitized-profile-id>
```

Use the existing stable profile id when available. Sanitize it to tmux-safe characters:

```rust
fn default_persistent_session_name(profile: &HostProfile) -> String {
    let slug = profile
        .id
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-");
    format!("tr-{slug}")
}
```

If no stable id exists in a specific flow, derive a deterministic slug from host address, username, and port. Do not derive the default from the display title because users can rename it and accidentally lose the expected session identity.

## Shell Safety

Implement tmux startup generation in `src/ui/shell.rs` near `startup_bytes_for_request`. Create a focused helper for tmux bootstrap command creation. Do not scatter shell snippets across UI or app-state code.

When `request.persistent_session` is false, preserve the current `startup_bytes_for_request` behavior exactly.

When `request.persistent_session` is true, generate a tmux wrapper as the top-level startup payload:

1. Emit environment variable exports first so a newly created tmux session inherits them.
2. Check for `tmux`.
3. If the session exists, attach only.
4. If the session does not exist, create it with startup directory/startup command applied only to that creation path.

This means startup directory and startup command must not be emitted as unconditional shell bytes before the tmux attach/create branch.

The bootstrap must:

- Check whether `tmux` exists on the target host.
- Attach to an existing session if present.
- Create a new session if absent.
- Apply startup directory only when creating a new session.
- Apply startup command only when creating a new session.
- Fall back to the user's shell if tmux is missing, while clearly printing an error.
- Degrade gracefully if an old tmux version rejects a flag or command.
- Avoid command injection through session names, startup directory, environment variables, host titles, notes, or user input.
- Keep terminal compatibility in mind. Do not force unusual `TERM` values in v1; let the remote shell/tmux negotiate normally unless the existing app already sets terminal env.

Base bootstrap:

```bash
if command -v tmux >/dev/null 2>&1; then
  if tmux has-session -t '<session>' 2>/dev/null; then
    exec tmux attach-session -t '<session>'
  else
    exec tmux new-session -s '<session>' -c '<startup-dir>'
  fi
else
  printf 'TermiRust persistent sessions require tmux on this host.\n' >&2
  exec "${SHELL:-/bin/sh}"
fi
```

If `detach_others` is enabled, attach with:

```bash
tmux attach-session -d -t '<session>'
```

or create/attach with equivalent safe tmux behavior.

Do not use tmux control mode for the first implementation. Keep the first version simple: attach the user's terminal to tmux normally.

Environment note:

- Export configured environment variables before the tmux block.
- On attach to an existing tmux session, arbitrary environment variables inside already-running shells may not refresh. Do not add speculative tmux environment-sync commands unless they are tested and covered by a clear requirement.

## UI Requirements

Host editor:

- Add a `Persistent Session` toggle.
- Add optional `Session Name` input.
- Add `Detach other clients when connecting` as an advanced toggle.
- Place these controls near the existing startup directory/startup command/start-in-files settings because they describe what happens on connect.
- When `Detach other clients when connecting` is enabled, show a concise warning that reconnecting can detach another TermiRust window or terminal from the same tmux session.
- Keep layout consistent with the existing editor style.
- Do not add a marketing/explainer section in the app.
- Use concise labels.

Workspace:

- Show a small `tmux` badge or status indicator when a pane is connected through persistent mode.
- Closing a TermiRust tab should detach the SSH client only. It must not kill the tmux session.
- Defer `Kill Persistent Session` and `List Persistent Sessions` for v1 unless they fit naturally without expanding scope.
- If revisiting these actions later, prioritize `List Persistent Sessions` before `Kill Persistent Session` so users can understand existing server-side state before deleting anything.

## Behavioral Requirements

- Existing non-persistent SSH sessions must behave exactly as before.
- Local PTY sessions must not use tmux unless the existing product explicitly supports local persistent mode later.
- Reconnect must reattach to the same tmux session.
- Workspace restore must reattach to persistent sessions when enabled.
- If the host is unreachable during workspace restore, keep the existing disconnected/error-pane behavior with the reconnect action available. Do not silently drop panes or delete workspace state.
- Startup commands must not rerun on every attach.
- Environment variables should be available before tmux starts.
- Host key pinning, credentials, jump hosts, port forwards, snippets, logs, and SFTP must remain unaffected.

## Implementation Plan

### Phase 1: Model And Storage

1. Add flat persistent session fields to `HostProfile`, `DraftProfile`, `ConnectRequest`, and `RestorableConnection`.
2. Add `#[serde(default)]` to persisted fields for backward-compatible deserialization.
3. Use `String` for `DraftProfile.persistent_session_name` and convert it with the existing `non_empty()` helper.
4. Ensure profile-to-draft, draft-to-profile, profile-to-request, restorable-to-request, and request-to-restorable paths preserve the fields.
5. Ensure export/import preserves the new fields.
6. Add tests for old state compatibility, field round-tripping, and default session name generation.
7. Run:

```bash
cargo fmt
cargo check
```

8. Commit:

```bash
git add src/models.rs src/storage.rs
git commit -m "Add tmux persistence settings to host profiles"
```

Do not include co-author trailers.

### Phase 2: Bootstrap Command Generation

1. Modify `startup_bytes_for_request` in `src/ui/shell.rs` to branch on `request.persistent_session`.
2. Add `tmux_bootstrap_script()` or an equivalently focused helper in `src/ui/shell.rs`.
3. Reuse or extend `shell_single_quote`; do not add ad-hoc escaping.
4. Emit environment exports before the tmux if/else block.
5. Nest startup directory and startup command inside the new-session path only.
6. Preserve current startup bytes exactly when persistent mode is off.
7. Add shell escaping tests.
8. Add tests for:
   - default session name
   - custom session name
   - tmux missing fallback
   - startup directory only on new session
   - startup command only on new session
   - detach-others mode
   - non-persistent sessions preserving existing startup output
   - single quotes and spaces in startup directory, session name, and startup command
9. Run:

```bash
cargo fmt
cargo check
cargo test
```

10. Commit:

```bash
git add src
git commit -m "Generate tmux bootstrap command for persistent SSH sessions"
```

Do not include co-author trailers.

### Phase 3: Host Editor UI

1. Add editor controls in `src/ui/app/editor.rs`.
2. Place controls near startup directory/startup command/start-in-files.
3. Add a concise warning for `persistent_session_detach_others`.
4. Keep visual style consistent with existing settings.
5. Ensure draft changes save correctly.
6. Ensure disabling persistent mode does not erase a custom session name unless existing UI conventions do that.
7. Run:

```bash
cargo fmt
cargo check
```

8. Commit:

```bash
git add src/ui src/models.rs
git commit -m "Add persistent session controls to host editor"
```

Do not include co-author trailers.

### Phase 4: Workspace Integration

1. Mark panes/workspaces that are connected using persistent mode.
2. Show a small tmux status badge if it fits the existing UI.
3. Ensure `RestorableConnection` preserves persistent fields and workspace restore reattaches through the same startup-byte mechanism.
4. If a restored host is unreachable, keep the existing errored/disconnected pane and reconnect flow.
5. Add logs that clearly identify persistent attach/create failures without leaking secrets.
6. Do not add kill/list tmux-session actions in this phase unless they are trivial and fit existing pane action patterns.
7. Run:

```bash
cargo fmt
cargo check
cargo test
```

8. Commit:

```bash
git add src
git commit -m "Restore persistent tmux sessions from workspaces"
```

Do not include co-author trailers.

### Phase 5: Manual Verification

Verify with a real SSH host that has tmux installed:

1. Enable persistent session for a host.
2. Connect and run a long command:

```bash
while true; do date; sleep 2; done
```

3. Close the TermiRust tab.
4. Reconnect to the same host.
5. Confirm the command is still running.
6. Quit and reopen TermiRust.
7. Confirm reconnect attaches to the same tmux session.
8. Test a host without tmux and confirm a clear fallback message appears.
9. Test a custom session name.
10. Test detach-others behavior from two TermiRust windows or one TermiRust plus another terminal.
11. Test a session name and startup directory containing spaces and single quotes.
12. Test workspace restore while the host is unreachable, then reconnect after the host is reachable.
13. Test a host with an older tmux if available, or document that only modern tmux was manually verified.

Commit any fixes from manual verification:

```bash
git add src
git commit -m "Polish tmux persistence behavior"
```

Do not include co-author trailers.

## Acceptance Criteria

- Persistent mode can be enabled per host.
- Existing sessions attach instead of creating duplicates.
- Closing TermiRust does not kill the remote tmux session.
- Reopening TermiRust can reattach to the same remote session.
- Startup command is not repeated on reattach.
- Workspace restore preserves persistent session settings and leaves unreachable hosts reconnectable.
- Old saved state files still load.
- Non-persistent SSH behavior is unchanged.
- `src/ssh.rs` remains unchanged unless a clearly documented need was discovered.
- Code is formatted.
- `cargo check` passes.
- Relevant tests pass.
- Commits are small, readable, and do not include `Co-authored-by`.

## Final Report Format

At the end, report:

- Files changed.
- Commits created.
- Tests run and results.
- Manual verification performed.
- Any deferred work.

Keep the report factual and concise.
