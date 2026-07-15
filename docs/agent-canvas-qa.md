# Agent Canvas v1 QA Record

Date: 2026-07-15

Environment: macOS development machine, branch `test`, Rust test profile.

## Automated results

| Check | Result | Notes |
| --- | --- | --- |
| `cargo fmt --all -- --check` | Pass | No formatting differences. |
| `cargo check -q` | Pass | Existing macOS `objc` cfg and dead-code warnings remain. |
| `cargo test -q` | Pass | 250 passed, 0 failed, 0 ignored in 18.78s. |
| Agent adapter focused tests | Pass | Codex fake app-server, Claude/Gemini stream fixtures, literal argument child, cancellation, malformed data. |
| Worktree integration tests | Pass | Real temporary Git repositories cover create, status, dirty refusal, committed refusal, and path boundaries. |
| Canvas capacity test | Pass | 20 nodes, 40 edges, finite fit geometry and supported zoom. |
| `cargo clippy --all-targets` | Pass with warnings | Repository reports existing warnings. |
| `cargo clippy --all-targets -- -D warnings` | Blocked | 121 existing warnings include old `objc` cfg macros and unrelated UI lints; not changed in this feature. |
| `git diff --check` | Pass | No whitespace errors. |

The two `docker_ssh_persistent_tmux` tests are self-skipping in this run because
`docker info` returned exit status 1. Cargo reports their early-return path as
two passing tests, but this record treats them as **not exercised**. They were
previously run successfully by the repository owner on 2026-06-24: two passed,
including reconnect without startup-command re-execution and missing-tmux
fallback.

## Native launch smoke

`cargo run` built and launched `target/debug/termirust`. CoreGraphics reported
one on-screen TermiRust window with bounds `1175x946`; the process remained alive
until it was intentionally interrupted. macOS denied automated `screencapture`,
so no screenshot or automated click-through is claimed.

## Covered behavior

- Legacy state defaults to Split and new canvas state round-trips.
- Invalid geometry, duplicate IDs, dangling edges, and dependency cycles repair
  or reject deterministically.
- Pan/zoom transforms, cursor anchoring, placement, fit, minimum resize, and
  stable restore mapping are unit tested. Keyboard node selection cycles and
  wraps in both directions.
- Local and SSH canvas nodes reuse `SessionPane` requests and persistence.
- Interactive launch arguments remain literal locally and quoted at the one SSH
  shell boundary; unsafe provider bypass flags are rejected.
- Structured Codex handshake, request correlation, streaming, approval,
  cancellation, malformed JSON, unknown fields, and abrupt process behavior use
  a deterministic fake server.
- Claude and Gemini documented stream shapes normalize through deterministic
  fixtures and a real local child fixture without provider/API use.
- Context is bounded, redacted, marked untrusted, reviewable, and not persisted.
- Dependency scheduling covers cycle rejection, topological readiness,
  concurrency two, and blocked dependants.
- Managed worktrees persist independently of nodes and refuse active, dirty,
  committed, unregistered, or out-of-root cleanup.

## Manual results available

The repository owner previously verified the SSH tmux flow against the Docker
fixture: `$TMUX` was populated, session `manual-startup-test` survived client
disconnect, `manual-startup-count` contained exactly one `startup-ran` line after
reattach, and a second SSH client attached to the same session.

## Manual release checks still required

These checks require a human-visible app, provider accounts, hardware, or remote
hosts and were not claimed by automation:

- drag, resize, selection, terminal text input, search, xterm mouse reporting,
  and guarded paste at multiple zoom levels;
- restart and visual restore of a populated canvas;
- eight simultaneously active output-producing panes and Retina/multi-monitor
  responsiveness;
- live authenticated Codex, Claude Code, and Gemini structured calls;
- password SSH, jump-host, forwarding, disconnect/reconnect, and remote missing
  provider guidance in Canvas mode;
- visual approval cards, worktree manager actions, context preview editing, and
  cross-host rejection copy;
- destructive tmux kill choice, which remains outside the Canvas-specific UI.

Use the reviewer smoke test in [agent-canvas.md](agent-canvas.md). Do not use a
production repository or paid provider account for the first pass.
