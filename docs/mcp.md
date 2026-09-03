# Read-only MCP

`termirust-mcp` gives a local MCP host bounded access to TermiRust's typed Projects, connection
presets, Sessions, runtime state, semantic transcripts, artifact metadata, and explicitly approved
actions. Its default surface is inspect-only. It uses the MCP `2025-11-25` stdio transport and does
not listen on a network socket.

## Build

```bash
cargo build --release -p termirust-mcp --locked
```

The executable is `target/release/termirust-mcp` (`termirust-mcp.exe` on Windows). Distribution
packages must install it beside the TermiRust CLI and Session Host. An MCP client configuration
uses the executable's installed absolute path:

```json
{
  "mcpServers": {
    "termirust": {
      "command": "/absolute/path/to/termirust-mcp",
      "env": {
        "TERMIRUST_MCP_CAPABILITIES": "status.read,projects.read,connections.read,sessions.read,runtime.read"
      }
    }
  }
}
```

Set `TERMIRUST_CONFIG_DIR` in the same `env` object only when TermiRust itself was started with a
non-default configuration directory. Do not point it at a copied or untrusted data directory.

## Capabilities

The default capability set is metadata-only:

- `status.read`
- `projects.read`
- `connections.read`
- `sessions.read`
- `runtime.read`

Artifact names and transcript bodies require explicit opt-in:

- `artifacts.list` returns metadata only; it never returns artifact payload bytes.
- `transcripts.read` returns only normalized User and Assistant semantic records. It filters
  reasoning, tool calls, tool results, diffs, and raw terminal output, and applies the shared
  secret redactor.

Use a comma-separated exact allowlist. `all` enables every current read-only capability and
deliberately never enables action capabilities; `none` exposes no tools. Unknown values fail
startup instead of silently widening access.

## Approved Actions

Action tools are disabled by default and need both controls below:

1. Add each exact action capability to `TERMIRUST_MCP_CAPABILITIES` in the MCP client config.
2. Create a short-lived local approval for exact Project and Session IDs.

Available action capabilities are `sessions.launch`, `sessions.wait`, `sessions.attach`,
`sessions.cancel`, `sessions.input`, `sessions.resume.review`, `sessions.resume`, and
`artifacts.create`. Isolated browser artifact actions are separately available as `browser.text`,
`browser.screenshot`, and `browser.download`; see [browser.md](browser.md).

Grant only the actions and scopes needed for the current task:

```bash
termirust-mcp-authorize grant \
  --actions wait,attach,input \
  --sessions 11111111-1111-1111-1111-111111111111 \
  --minutes 30

termirust-mcp-authorize grant \
  --actions launch \
  --projects 22222222-2222-2222-2222-222222222222 \
  --minutes 10
```

Only one approval document is active at a time. Replace it with another `grant`, or revoke it:

```bash
termirust-mcp-authorize revoke
```

Every mutating call requires a new UUID `command_id`. Reuse that exact ID only when retrying the
same uncertain call; changing the operation or arguments under an existing ID fails closed.
TermiRust never automatically retries mutations. Launch and resume derive their successor Session
identity from the command ID, Host input/cancel preserve it through the Host idempotency contract,
and completed results are retained in a bounded local receipt store.

`termirust_attach_session` returns replay counts and lifecycle metadata, never terminal bytes, and
does not request control. `termirust_send_input` requires the current Host writer lease. Resume is
split into review and commit: pass the exact source revision from `termirust_review_resume` to
`termirust_resume_session`. Created artifacts are inert UTF-8 data capped at 64 KiB.

Browser actions additionally require exact approved origins and a user-installed Chrome/Chromium
browser. They run with an ephemeral profile and return only inert artifact metadata. They never
inherit browser credentials or return page/download bytes through MCP.

The approval is rechecked during operations every 25 ms. Expiry, replacement, or revocation cancels
active work. A bounded owner-only audit ledger records action, scope kind, command ID, grant ID,
timestamp, and outcome; it never records terminal input, artifact content, paths, titles, or
terminal output. The server refuses new actions if it cannot record their start safely.

## Bounds And Behavior

- Requests are newline-delimited UTF-8 JSON-RPC and limited to 256 KiB.
- Tool pages default to 50 records and permit at most 100.
- Cursors are random, opaque, scoped to one tool and query, and held in a bounded in-memory store.
- Results are limited to 512 KiB before stdio framing.
- At most eight tool calls may be active and at most 120 calls are accepted per minute.
- In-flight reads are cancellable with `notifications/cancelled`.
- IDs must be UUIDs before any filesystem path is derived.
- Transcript files are accepted only at the fixed Session-owned
  `transcripts/records.jsonl` path and symlinks fail closed.

There are no arbitrary shell, resize, archive, restore, remove, filesystem-path, artifact-payload,
or network tools. Action tools cannot spawn commands directly; they delegate only to typed existing
CLI/Host and artifact-store contracts.

## Verify

```bash
./scripts/verify-mcp-readonly.sh
./scripts/verify-mcp-actions.sh
./scripts/verify-browser-capability.sh
```

The gate covers lifecycle negotiation, capability filtering, JSON schemas, pagination, cursor
scope, cancellation, rate/concurrency bounds, oversized input recovery, invalid IDs, mutation
rejection, real store projections, artifact payload exclusion, semantic category filtering, and
secret redaction.

The action gate additionally covers policy permissions, exact scopes, expiry/revocation,
idempotent receipts, conflicting command IDs, redacted audit records, strict action schemas,
destructive annotations, a real artifact-store mutation, and real durable-Host writer-lease input.
