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

Some desktop behaviors still need manual checks because the app is a native GPUI desktop app and does not currently expose a headless UI automation harness:

- Creating/editing/deleting hosts from the UI.
- Connecting a saved host and interacting with the terminal.
- SFTP upload/download/delete flows.
- Window resizing, tab dragging, split-pane layout, and copy/paste.
- Visual polish across macOS, Windows, and Linux.

For manual checks, use a disposable local SSH server or VM first, not production infrastructure.
