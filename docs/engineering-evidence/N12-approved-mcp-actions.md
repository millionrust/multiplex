# N12 Approved MCP Actions Evidence

Date: 2026-09-02

## Outcome

The local MCP server now supports narrowly scoped launch, wait, read-only attach summary, cancel,
input, semantic resume review/commit, and inert artifact creation. Every action is default-off and
requires an explicit startup capability plus a current Project- or Session-scoped local approval.

`termirust-mcp-authorize` creates short-lived owner-only approvals and revokes them immediately.
The server continuously rechecks approval while work is active. Mutations preserve stable command
IDs through the CLI/Host authority, use writer leases and exact revisions, never auto-retry, retain
bounded redacted receipts, and produce content-free audit metadata.

## Automated Evidence

```text
./scripts/verify-mcp-readonly.sh
./scripts/verify-mcp-actions.sh
```

The focused suites cover default denial, exact capability advertisement, strict JSON schemas,
destructive/read-only annotations, invalid IDs and unknown fields, Project/Session scope denial,
owner-only and symlink policy checks, command replay, command fingerprint conflicts, payload-free
receipts and audit, revocation cancellation, real artifact ingestion, and real durable-Host input
under writer-lease contention.

Final verification:

```text
./scripts/verify-mcp-readonly.sh
PASS: 21 MCP tests passed, 0 failed; strict package Clippy passed

./scripts/verify-mcp-actions.sh
PASS: 21 MCP tests and 7 CLI/real-Host input tests passed; strict Clippy passed

./scripts/verify-controller-security-vectors.sh --check
PASS: 3 golden-vector tests passed, 0 failed

cargo test --workspace --all-targets --locked
PASS: exit 0; 1,511 tests passed, 0 failed, 10 explicitly ignored

python3 scripts/clippy-changed.py
PASS: changed Rust lines are Clippy-clean

cargo fmt --all -- --check
git diff --check
python3 scripts/verify-gpui-boundaries.py
PASS: formatting, diff hygiene, and GPUI dependency boundaries passed
```

The workspace dependency change adds only already-locked `fs2` and `sha2` packages to the MCP
crate. The Controller security dependency closure and protocol vectors are unchanged; its ADR and
lock checksums were reviewed and repinned through the dedicated vector gate.

## User-path Smoke

A disposable `TERMIRUST_CONFIG_DIR` was used to grant one Session-scoped input permission for one
minute. The policy was created with Unix mode `0600`; `tools/list` advertised only
`termirust_send_input`; a typed call reached the unavailable disposable Session without exposing its
input; and `termirust-mcp-authorize revoke` removed the policy. No network listener was opened.

## Remaining Qualification

N15 owns independent MCP-host compatibility, packaged installation, crash injection between Host
completion and receipt persistence, and Windows ACL runtime proof. These limitations do not widen
the default surface or permit duplicate dispatch. The final run retained 20 GiB of free space on
the development volume, above the project's 15 GiB minimum-workspace requirement.
