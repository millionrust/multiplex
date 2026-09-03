# Isolated Browser Capability

TermiRust can capture visible page text, viewport screenshots, and bounded downloads for an
existing Session through its local MCP server. The feature is disabled by default. It does not
appear in `all`, because `all` deliberately means all read-only MCP capabilities.

## Security Model

- Every browser operation needs an explicit MCP startup capability, a short-lived local approval,
  the exact Session ID, a fresh command UUID, and an exact HTTP(S) origin.
- Page text and screenshots run in a separate headless Chrome/Chromium process group with a new
  owner-only profile. TermiRust clears the process environment and never opens the user's normal
  browser profile, cookies, password store, extensions, or credentials.
- An owned loopback proxy resolves and pins approved hostnames, rejects private, loopback,
  link-local, multicast, documentation, benchmark, and metadata addresses, and caps traffic.
  CDP interception independently denies unapproved URLs and non-read HTTP methods.
- Chrome downloads are disabled during page capture. The separate reviewed download action uses
  GET only, rechecks every redirect, streams at most 25 MiB, and never executes response bytes.
- Results are ingested through the existing Session artifact repository. MCP returns artifact
  metadata, never page or download payload bytes. URLs and payloads are excluded from action audit
  and receipt files.
- Cancellation, approval expiry, replacement, or revocation terminates the owned process group,
  stops the proxy, and removes the ephemeral profile.

This is defense in depth around a complex external browser, not a claim that browser content is
safe. Approve only origins needed for the current task.

## Enable

A supported user-installed Chrome or Chromium executable is required. TermiRust does not bundle a
browser. Set `TERMIRUST_BROWSER_EXECUTABLE` in the MCP server environment only when automatic
detection cannot find it.

Add only the required capabilities to the MCP configuration:

```text
browser.text,browser.screenshot,browser.download
```

Then approve an exact Session and the minimum origins for a bounded time:

```bash
termirust-mcp-authorize grant \
  --actions browser_text,browser_screenshot,browser_download \
  --sessions 11111111-1111-1111-1111-111111111111 \
  --browser-origins https://example.com,https://static.example.com \
  --minutes 15
```

Available MCP tools are `termirust_capture_page_text`,
`termirust_capture_page_screenshot`, and `termirust_download_browser_artifact`. Each call requires
`command_id`, `session_id`, `display_name`, and `url`. Reuse a command ID only to retry the exact
same uncertain operation.

Revoke immediately when the task is finished:

```bash
termirust-mcp-authorize revoke
```

## Current Platform Claim

The live reference gate currently proves Google Chrome on macOS arm64. Linux and Windows compile
paths exist, but native process-tree, packaging, and runtime parity remain N14 qualification work.
No browser executable redistribution or automatic browser download is part of this feature.

## Verify

```bash
./scripts/verify-browser-capability.sh
```

The script always runs unit, policy, MCP, strict Clippy, and static security checks. It runs the
live hostile-page/cancellation probes when Chrome or Chromium is available and otherwise prints an
explicit `SKIPPED(browser)` result.
