# N11 Read-only MCP Evidence

Date: 2026-09-02

## Outcome

`termirust-mcp` is a separate local stdio server implementing MCP protocol revision `2025-11-25`.
It exposes only capability-approved, read-only inspection over existing TermiRust CLI, store, and
Host-projected Session contracts.

Default tools inspect status, Projects, connection presets, Sessions, and runtime state. Artifact
metadata and normalized semantic transcript bodies require separate explicit capabilities.
Artifact payloads, raw terminal journals, reasoning, tool records, diffs, and every mutation are
absent from the default advertised surface. N12 later added separately enabled actions behind an
independent local approval policy without widening these defaults.

## Executable Bounds

- 256 KiB request lines with oversize-line discard and parser recovery
- 100 records per page, 256 live opaque cursors, 512 KiB tool results
- eight active tool calls and 120 calls per rolling minute
- fixed UUID-derived Session paths with symlink rejection
- 100,000-record/100 MiB transcript scan ceilings and 256 KiB transcript page content ceiling
- cancellation tokens checked through store and transcript reads

## Automated Evidence

Run:

```text
./scripts/verify-mcp-readonly.sh
```

The package tests exercise initialization ordering, read-only tool annotations, default-denied
sensitive capabilities, unauthorized-tool non-disclosure, cursor opacity and query scoping,
in-flight cancellation, malformed/batch/mutating requests, unknown fields, traversal-like IDs,
stdio framing, oversized request recovery, real Project and Session repositories, artifact
metadata without payload bytes, User/Assistant-only transcript output, and shared secret
redaction.

The verifier also rejects source dependencies on CLI Session mutation variants and artifact
payload reads, runs package Clippy with warnings denied, and checks diff hygiene.

Final verification on 2026-09-02:

```text
./scripts/verify-mcp-readonly.sh
PASS: 16 tests passed, 0 failed; package Clippy passed with warnings denied

./scripts/verify-controller-security-vectors.sh --check
PASS: 3 golden-vector tests passed, 0 failed

cargo test --workspace --all-targets --locked
PASS: exit 0; 1,505 tests passed, 0 failed, 10 explicitly ignored

cargo fmt --all -- --check
PASS

git diff --check
PASS
```

Adding the MCP package and the previously implemented mobile/relay adapters changed only the
workspace dependency graph. The Controller security closure remains pinned to the same package
versions and selected features, so the security ADR records that review and the fixture pins the
new ADR and lockfile checksums without changing any protocol vector bytes.

## Protocol Basis

The implementation follows the official MCP `2025-11-25` lifecycle, stdio transport, tool,
pagination, and cancellation contracts documented at `modelcontextprotocol.io`. HTTP and
experimental task support are intentionally not advertised.

## Remaining Qualification

N15 still owns compatibility runs against multiple independent MCP hosts and release-package
installation on macOS, Windows, and Linux. N12 must define a separate authority and idempotency
contract before any mutation can be added.
