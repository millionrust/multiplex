# Testing

Use the automated smoke script before committing or before checking a new build manually:

```bash
./scripts/auto-test.sh
```

It runs:

- `cargo fmt --check`
- `cargo check`
- `cargo test`
- `cargo clippy --all-targets --all-features`
- `git diff --check`

## Docker SSH E2E

`cargo test` now includes two Docker-backed end-to-end checks when Docker is available:

- `ssh::tests::docker_ssh_session_connects_and_streams_output`
- `ui::app::tests::e2e_ssh_workspace_connects_renders_output_and_closes`
- `ui::app::tests::e2e_ssh_split_and_broadcast_reaches_all_panes`
- `ui::app::tests::e2e_ssh_auto_reconnect_recovers_after_server_restart`
- `ui::app::tests::e2e_local_shell_paste_confirmation_and_search`
- `sftp::tests::docker_sftp_round_trips_directory_upload_download_and_delete`
- `ui::app::tests::e2e_sftp_files_view_navigates_and_deletes_remote_files`
- `ui::app::tests::e2e_quick_connect_password_flow_opens_workspace`
- `ui::app::tests::e2e_host_editor_saves_and_removes_user_profile`

Those tests build `tests/fixtures/ssh-server/`, start a disposable OpenSSH container, connect through the real `russh` session path, and verify both:

- the raw SSH runtime can authenticate, stream output, and disconnect cleanly
- the GPUI app can open a workspace, render terminal output, accept typed terminal input, split panes, broadcast commands, and auto-reconnect after a non-user disconnect
- the local terminal path can confirm/cancel multi-line paste and drive workspace search against real terminal output
- the SFTP runtime can list directories, upload files, download files, and delete remote files against the same Docker SSH target
- the GPUI app can open the remote Files view, navigate folders, delete remote files, save/remove user hosts, and quick-connect with password auth

If Docker is unavailable, the rest of the suite still runs and the SSH E2E tests self-skip.

## Optional Live SSH Smoke

The default suite does not touch any real server. To verify that your local machine or a test VM accepts SSH, set these environment variables:

```bash
TERMIRUST_TEST_SSH_HOST=localhost \
TERMIRUST_TEST_SSH_USER="$(whoami)" \
TERMIRUST_TEST_SSH_PORT=22 \
TERMIRUST_TEST_SSH_KEY="$HOME/.ssh/termirust_test_key" \
./scripts/auto-test.sh
```

If you use your normal SSH agent or default key, omit `TERMIRUST_TEST_SSH_KEY`.

This smoke check proves the target is reachable and authenticated before you test the app UI against the same host.

## What Still Needs Manual UI Testing

Some desktop behaviors still benefit from manual checks because the app is a native GPUI desktop app and the automated coverage is currently focused on SSH session flows:

- Creating/editing/deleting hosts from the UI.
- SFTP upload/download/delete flows.
- Window resizing, tab dragging, split-pane layout, and copy/paste.
- Visual polish across macOS, Windows, and Linux.

For manual checks, use a disposable local SSH server or VM first, not production infrastructure.
