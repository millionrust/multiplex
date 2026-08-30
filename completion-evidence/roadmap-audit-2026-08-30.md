# Engineering Roadmap Audit - 2026-08-30

## Result

The active E07 infrastructure-access milestone is complete. Its nine finite children have
implementation evidence, the aggregate verifier passes, the full Rust suite is green, and branch
`test` is synchronized with `origin/test`.

The external dispatcher now has no active finite leaf. This is intentional: its activation rules
forbid silently starting a queued leaf, and the remaining work is gated by an unresolved client
decision, a blocked prerequisite, unavailable platform acceptance, or a future milestone that has
not been decomposed into an authorized finite leaf.

## Completed Engineering Sequence

- E01 foundations through E01.2: durable Host/session survival and shared Rust/Swift/Kotlin
  conformance contracts. The E01 parent remains open for the live native-to-Rust-Host run described
  below.
- E02: desktop coordinator decomposition.
- E03: coherent desktop information architecture.
- E04: native mobile terminal correctness contract and acceptance.
- E05: unified direct-SSH and Controller mobile product model.
- E06: explicit free remote-route contracts and fault acceptance.
- E07: infrastructure-access depth, including SSH policy/certificates/agent access, key lifecycle,
  proxy/forwarding, resilient SFTP, transport decisions, and connection diagnostics.
- Existing numbered goals 01 through 14 and 16 through 17 cover the baseline, project/session
  model, Host/client/CLI, runtime integration, worktrees, notifications, artifacts, and local URLs.

## Remaining Work

| Area | Detailed state | Why it is not marked complete |
| --- | --- | --- |
| E01 live native golden path | Parent remains a container | No live iOS and Android pair/list/attach/write/resize/background/reconnect/revoke run against one Rust Host has been recorded; current evidence is contract, compile, JVM, and route-fixture evidence. |
| Native mobile Controller program | 5 queued leaves, 1 blocked leaf | D01/D06 platform and native product-flow authority remain scoped; iOS pairing/fleet is explicitly blocked and downstream leaves depend on it. |
| TUI | 3 queued leaves | D03 approves direction but the detailed dispatcher has not activated a finite TUI leaf and the release matrix remains pending. |
| MCP | 3 queued leaves | D07 approves direction but exact cross-session and agent authority policy remains pending. |
| Browser automation | 2 queued leaves after a completed No-Go spike | D08 authority remains pending and the feature must stay off until isolation/security prerequisites pass. |
| Accessibility/UI migration | 11 queued leaves, 1 blocked leaf | The GPUI/macOS semantic bridge and deterministic focus harness are not proven; all screen migration and WCAG audit leaves depend on that bridge. |
| Desktop platform adapters | 3 queued leaves | macOS packaging depends on the semantic bridge; Linux and Windows candidates additionally require D01. |
| Updaters | 3 queued leaves | Trust primitives are complete, but platform packaging prerequisites and D01/release gates are unresolved. |
| Relay productization | 3 queued leaves | Local core and desktop route are complete; packaging and native routes require D01/D06. Public operation remains prohibited by D05. |
| E08-E13 expansion | Not active as finite leaves in the detailed dispatcher | Agent depth is substantially represented by completed runtime/session/canvas goals; TUI maps to Goal 15, MCP to Goal 18, browser to Goal 19, and platform quality to Goal 21. Sync/collaboration needs a separately approved bounded roadmap before implementation. |

The detailed goal directory currently contains 33 real queued leaves, two blocked leaves, and no
active leaf. The `_template.md` placeholder is excluded from that count. Containers are grouping
records, not executable tasks.

## Verification Snapshot

- `./scripts/verify-infrastructure-access.sh` - passed every E07.1-E07.9 gate.
- `cargo test -q -- --test-threads=1` - 633 passed, 0 failed, 3 ignored in the main binary; both
  integration binaries passed.
- `cargo fmt --all -- --check` - passed.
- `python3 scripts/clippy-changed.py` - changed Rust lines are Clippy-clean.
- `git diff --check` - passed.
- Rust repository - clean and synchronized with `origin/test` before this audit-only commit.
- Kotlin repository - clean.
- Swift repository - one pre-existing uncommitted `TermiRustMobile.xcodeproj/project.pbxproj`
  change was observed and deliberately preserved.
- Disk - 18 GiB available, above the required 15 GiB floor.

## Next Authorization Point

The owner must select one finite leaf and resolve its named gate or blocker. The first product-level
gap is the live native E01 Controller-to-Rust-Host conformance run. Starting TUI, MCP, browser,
platform packaging, updater, or relay productization without that explicit selection would violate
the roadmap's own activation and security rules.
