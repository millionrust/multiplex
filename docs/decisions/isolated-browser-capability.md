# Isolated Browser Capability

- Decision: **Conditional Go**
- Scope: opt-in local MCP artifact capture on proven platforms
- Decision date: 2026-09-03
- Selected controller: `chromiumoxide` 0.9.1 over Chrome DevTools Protocol

## Decision

TermiRust may expose page-text, screenshot, and download artifact actions only through the N12
capability and approval boundary. The browser executable must be user-installed. The product does
not redistribute Chrome, auto-download a browser, reuse a personal browser profile, or expose
general JavaScript evaluation, element handles, clicks, form submission, credentials, cookies,
arbitrary filesystem paths, or raw downloaded bytes through MCP.

This decision narrows and supersedes the production freeze recorded by the earlier feasibility
spike. That spike correctly rejected an unbounded embedded browser and remains historical evidence.
The selected narrower route is conditional because macOS arm64 is the only live browser platform
proved on the reference machine. Linux and Windows cannot be advertised until N14 executes their
native process and package gates.

## Required Boundaries

1. Browser process and profile ownership are per operation and ephemeral.
2. Browser sandbox, certificate verification, and site isolation are never disabled.
3. A filtering proxy with DNS pinning and CDP interception both enforce the exact-origin policy.
4. Page captures permit only read HTTP methods and explicitly disable Chrome downloads.
5. Reviewed downloads are GET-only, redirect-aware, capped at 25 MiB, and become inert artifacts.
6. Browser URLs, page content, screenshots, and downloads never enter audit or receipt logs.
7. Cancellation and policy revocation stop owned resources within 30 seconds.
8. Capability advertisement and local approval are both required; defaults remain read-only.

## Rejected Alternatives

- Reusing the user's browser profile would expose credentials and private state.
- Embedding an in-process web view would weaken process ownership and hostile-page containment.
- Browser auto-download or bundled branded Chrome would create unresolved redistribution and
  update obligations.
- General-purpose browser automation would permit unreviewed mutations and stale element actions.
- CDP interception alone cannot prevent every missed target or DNS-rebinding route; the owned proxy
  is a second enforcement boundary.

## Sources

The implementation follows the official Chrome DevTools Protocol Fetch, Browser download, and
Target contracts; Chrome's sandbox and Site Isolation remain enabled. Primary source links and the
original candidate research are retained in `docs/decisions/browser-engine.md`.
