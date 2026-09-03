# N13 Isolated Browser Evidence

Date: 2026-09-03

## Outcome

TermiRust now has an opt-in browser boundary for semantic text capture, viewport screenshots, and
bounded downloads. All outputs enter the existing Session artifact repository as inert data.
MCP browser tools are absent by default and require an exact startup capability, Session scope,
command ID, short-lived local approval, and exact-origin allowlist.

The runtime launches user-installed Chrome/Chromium in a separate process group with an empty
environment and ephemeral owner-only profile. An owned proxy resolves and pins approved hosts,
blocks non-public destinations, caps connections and bytes, and rechecks redirects. CDP provides a
second request boundary, denies non-read methods, and disables page-triggered downloads.

## Automated Evidence

```text
cargo test -p termirust-browser -p termirust-mcp --locked
PASS: 31 tests passed, 0 failed, 2 live browser tests explicitly ignored

cargo test -p termirust-browser --locked -- --ignored --nocapture
PASS: 2 live Chrome tests passed, 0 failed

cargo clippy -p termirust-browser -p termirust-mcp --all-targets --locked -- -D warnings
PASS

cargo deny check licenses bans sources
PASS: bans, licenses, and sources

cargo test --workspace --all-targets --locked
PASS: the complete Rust workspace, including Docker-backed SSH/tmux coverage

./scripts/verify-rust.sh focused
PASS: GPUI boundaries, MCP capabilities, browser containment, all-feature compilation,
changed-line Clippy, and tmux regressions
```

Coverage includes exact origins, credential-bearing URL rejection, private/metadata address
denial (including IPv4-embedded IPv6 transition ranges), symlinked profile rejection, bounded
download streaming, pre-allocation content-length rejection, prompt cancellation of a stalled
download, redirect denial before connection, hostile page subresource/iframe/popup/WebSocket
containment, live semantic-text and PNG capture, process termination, profile cleanup, MCP
capability hiding, strict schemas, idempotent command receipts, and URL-free audit/receipt records.

## Remaining Qualification

The reference live run used Google Chrome 152.0.7977.65 on macOS arm64. N14 owns Linux and Windows
runtime/process/package parity. N15 owns sustained hostile-page fuzzing, independent MCP-host
compatibility, long-running resource measurements, forced crash matrices, and browser-version
update/rollback drills. Chrome redistribution remains intentionally out of scope.
