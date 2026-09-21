# Implementation plan: reach the Mac's terminals from the mobile app

Handoff document for the implementing agent. Companion to
[remote-terminals.md](remote-terminals.md), which is the user-facing guide for the same
feature. Read that first for the product shape; this file is the build order, the exact
seams, the test strategy, and the gates.

Every path is relative to the repository root. Every command runs from the root.

---

## 1. Goal and non-goals

**Goal.** A terminal the user opens in Terminal.app, Zed, or Windows Terminal appears in
the mobile app's session list and can be watched, and with the writer lease, typed into.

**Why this is mostly plumbing.** The Controller already transports, authenticates, lists,
attaches, replays, and enforces read-only versus write. What is missing is a *source*: no
code discovers terminals the app did not create. There is no `tmux list-sessions`
anywhere in the tree, and every existing tmux call uses tmux for its exit status only
(`crates/termirust-desktop/src/local.rs:429-438` captures `tmux -V` as an opaque string).

**Non-goals for this work.**

- Adopting terminals already running outside a multiplexer. A process cannot take over a
  PTY it does not own; `reptyr` is not macOS-compatible and SIP blocks that class of
  trick. Out of scope permanently.
- Per-app adapters (iTerm2 Python API, Terminal.app AppleScript). Possible later, not now.
- Answering approval prompts from the phone. `ControllerCommand::Approval` returns
  `approval_unavailable` today and stays that way.
- Any change to the wire protocol. Milestone 1 needs none: `runtime` is a `String` and
  `origin: Terminal` already exists (`crates/termirust-controller-listener/src/protocol.rs:275-306`).

---

## 2. Mechanics already verified

Run on macOS with tmux 3.6b. Reproduce these before designing anything differently;
they are the load-bearing facts.

**Discovery output is machine-parseable.**

```bash
tmux list-sessions -F '#{session_id}|#{session_name}|#{session_attached}|#{session_created}|#{session_windows}|#{session_activity}'
# $0|demo|0|1789221774|1|1789221774
tmux list-panes -a -F '#{session_id}|#{pane_current_command}|#{pane_current_path}'
# $0|bash|/private/tmp
```

With no server running, `tmux list-sessions` exits 1 and prints
`no server running on /private/tmp/tmux-501/default` to stderr. Treat that as "zero
sessions", never as an error.

**A read-only client does not resize the user's window.** tmux 3.x defaults to
`window-size latest`, and a read-only client never becomes the latest *used* client. A
200×50 window stayed 200×50 while an 80×24 read-only client was attached.

**Grouped sessions do not isolate size.** `tmux new-session -t <name>` shares the window
objects, so both sessions report the same dimensions. Do not use grouping to give the
phone its own geometry; it does not work.

**Resolved by Spike 1 (tmux 3.7c), and it changed the design.**

- A client attached under a real Session Host streams correctly: output and typed input
  both arrived through `HostClient`.
- `send-keys` is not usable while a read-only client is attached. tmux 3.7c answers
  `client is read-only`, because the command resolves the read-only client as its current
  client. An earlier check on 3.6b injected raw bytes with `send-keys -H` only because no client was attached.
- `=name` is not a valid *pane* target on 3.7c (`can't find pane: =name`), and 3.7c
  accepts `:` and `.` inside session names. Target sessions by id (`$3`, and `$3:` for its
  active pane) instead.
- `attach-session -f ignore-size` keeps a normal client out of window sizing: with a
  120×40 desktop client attached, an 80×24 `ignore-size` client typed input and the
  window stayed 120×40. With no other client attached, tmux sizes the window to the
  `ignore-size` client, which is the useful behavior when nobody is at the desktop.

So the phone attaches with `tmux -u attach-session -f ignore-size -t $ID` (tmux ≥ 3.2)
and types through the Session Host PTY under the normal writer lease. §3.3 reflects this.

---

## 3. Design

### 3.1 Shape

A third **session source** in the Controller host backend, beside durable sessions and
live desktop panes:

| Source | Enumerated from | Attach mechanism |
| ------ | --------------- | ---------------- |
| Durable sessions | `SessionRepository` records | Unix socket to a running `termirust-session-host` |
| Live desktop panes | `DesktopPaneRegistry` over a Unix socket bridge | In-process transport callbacks |
| **tmux sessions (new)** | `tmux list-sessions` | A shared in-process session host running `tmux attach -f ignore-size` |

### 3.2 Attach without a persisted record

Listing is ephemeral, like live panes: nothing requires a `HostedSession` record
(`crates/termirust-controller-listener/src/host_backend.rs:470-500`).

Attaching reuses the existing durable branch, because that branch needs only two things
that are derived from the session id: the socket path
`<runtime_parent>/<session-uuid>/<22-char-opaque-name>`
(`crates/termirust-client/src/ipc.rs:18-25`, `crates/termirust-host-protocol/src/model.rs:234-249`)
and `host.json` under `sessions.session_data_path(session_id)`, read for the occupant
generation (`host_backend.rs:618-626`). So:

1. Derive a **stable** `HostedSessionId` from the tmux session identity. `HostedSessionId::new()`
   is random and `from_uuid` is the only constructor (`crates/termirust-domain/src/id.rs:307-321`),
   so build a deterministic UUID yourself: hash a namespace constant with the tmux
   `#{session_id}` **and** `#{session_created}` (the id `$0` is reused after a server
   restart; the creation timestamp disambiguates), then `HostedSessionId::from_uuid`.
2. On `Attach`, if no host is running for that id, spawn one with a `LaunchDescriptor`
   whose `executable` is the canonicalized tmux binary and whose arguments are
   `["-u", "attach-session", "-f", "ignore-size", "-t", "$<id>"]`, re-resolving the id from
   a fresh listing immediately before attaching.
3. Let the existing attach code take over: replay, snapshot chunking, and streaming all
   work unchanged.

The session host runs arbitrary programs — no allowlist — and the golden test already
spawns `/usr/bin/ssh -tt` this way
(`crates/termirust-controller-listener/tests/desktop_host_golden.rs:148-178`).

### 3.3 Input and resize

- **Input:** write into the attached `ignore-size` client through the Session Host PTY,
  exactly like a durable session. (`send-keys` was the original plan; Spike 1 showed tmux
  3.7c refuses it while a read-only client is attached.)
- **Hosts run in the listener process** on a dedicated two-thread runtime
  (`termirust_session_host::start`), are shared by every connection attached to the same
  tmux session, and stop when the last one detaches or disconnects. This avoids locating a
  host binary per route and leaves nothing running after the listener exits. Journals
  live in a random user-only directory under the runtime parent and are deleted with the
  host.
- **Occupant generation** advances each time a host for a session goes away, so a phone
  holding a watermark from an earlier host is refused as stale and relists. A newly
  started host replays from zero: a new tmux client redraws the whole screen.
- **Resize:** strip `Resize` from the advertised capabilities, exactly as live panes do
  (`host_backend.rs:487-493`). The phone gets a clipped view of the desktop geometry.
- Keep the writer lease semantics unchanged: the lease still gates `Input`, so a
  device without `SendInput` cannot type
  (`crates/termirust-controller-listener/src/authorization.rs:33-39`).

### 3.4 Where the code lives

The listener crate owns this, because all three routes construct their backend there. The
tmux helpers in `crates/termirust-desktop/src/local.rs` are **private** to the desktop
crate (`local_tmux_probe`, `persistent_tmux_arguments`, `kill_local_tmux_session`,
`probe_tmux_readiness`), and the listener cannot depend on the desktop crate.

Create `crates/termirust-tmux` — a tiny, GPUI-free crate holding binary discovery, the
version probe, the `list-sessions` parser, and the attach arguments. Then have both
`termirust-controller-listener` and `crates/termirust-desktop/src/local.rs` use it, so
there is one tmux integration rather than two. Keep it free of `gpui` so
`scripts/verify/gpui-boundaries.py` stays satisfied, and add it to the workspace members
in `Cargo.toml` (alphabetical) with `rust-version.workspace = true`.

---

## 4. Milestones

Ship each milestone green — all gates in §6 passing — before starting the next.

### M1 — tmux discovery and attach (the unlock)

Nothing in the mobile apps changes. New tmux rows appear in the existing session list and
open in the existing read-only terminal view.

1. **Spike 1 (do this first, before any product code).** Prove a read-only tmux client
   streams under a session host. Write a throwaway integration test that starts a detached
   tmux session, spawns a real session host via `LaunchDescriptor` with
   `tmux attach-session -r -t "=<name>"`, connects a `HostClient`, and asserts the
   session's output arrives. Copy the harness from
   `crates/termirust-desktop/tests/durable_host_mode.rs:16`. **If the stream is empty,
   stop and report** — the fallback is a `tmux pipe-pane`-based reader, which is a
   different design and needs a decision, not a workaround.
   - Gotcha: `/opt/homebrew/bin/tmux` is a symlink and `LaunchDescriptor::validate`
     rejects symlinked executables (`crates/termirust-session-host/src/descriptor.rs:239-249`).
     Canonicalize the probe result.
   - Gotcha: the host calls `env_clear()` then forces `TERM`
     (`crates/termirust-session-host/src/host.rs:389-411`). Pass `PATH`, `HOME`, and
     `SHELL` explicitly, as `hosted_session.rs:242-258` does.
2. **New crate `termirust-tmux`:** binary discovery (honor `MULTIPLEX_TMUX_PATH` first, as
   `local.rs:391-410` does), `tmux -V` probe, `list-sessions` parser, and attach arguments. Parser requirements: tolerate the no-server case, session names containing
   `|` and spaces (use a separator that cannot appear in a name, or parse field-by-field),
   and cap the session count.
3. **Discovery source in the listener:** a `TmuxSessionSource` producing
   `ControllerSessionSummary` rows with `origin: Terminal`, `runtime: Some("tmux")`,
   `lifecycle: "live"`, capabilities from the peer minus `Resize`, and bounded strings
   (256 chars for titles, per `desktop_pane_bridge.rs:362-368`).
4. **Merge into `list_sessions`** (`host_backend.rs:462-546`): extend the shadowing set so
   a tmux row never duplicates a durable or live-pane row, and extend the revision mix —
   it is currently `durable_revision * 31 + live_revision` (`:501-504`). Get this wrong and
   `expected_revision` invalidation silently misbehaves.
5. **Attach path:** ensure-host-then-attach as in §3.2, plus teardown on `Detach` (kill the
   spawned host; leave the user's tmux session alive).
6. **Input path:** unchanged from durable sessions; the host PTY feeds the `ignore-size` client.
7. **A settings flag,** default off, gating discovery entirely. See M2 for the wiring rules;
   M1 may ship with the flag defaulting off and no UI if that keeps the diff small.

### M2 — Opt-in setup flow (desktop UI)

The panel described in `remote-terminals.md`: level picker, diff preview, idempotent
apply, verification, revert. Writes `~/.config/termirust/shell-init.zsh` plus one marked
block in `~/.zshrc`, and never edits anything without showing it first.

This milestone is where the repo's UI contracts bite. Read §6.3 before writing UI code.

### M3 — Windows

`termirust shell` subcommand (a session-host-owned ConPTY the console attaches to) plus a
Windows Terminal profile writer. Follow the CLI-mode conventions in
`crates/termirust-desktop/src/main.rs:34-40, 196-257`: a `const &str` mode name matched
positionally, returning before `Application::new()`. Keep the Windows CI executable
assertions truthful (`.github/workflows/ci.yml:119-132`).

### M4 — Route parity

Live desktop panes and tmux sessions are reachable over the LAN route only, because
`serve_repository_stdio_bridge` builds the backend without the pane bridge
(`crates/termirust-controller-listener/src/launch.rs:802` versus `:883-886`). Give
Controller-over-SSH and the relay the same sources.

### M5 — Background service

LaunchAgent (macOS) and a per-user logon task (Windows) so sessions stay reachable with
the desktop app closed.

---

## 5. Test strategy, and the iteration loop

**You are expected to run tests many times.** Write the test, watch it fail, implement,
watch it pass, then run it repeatedly to prove it is not flaky. Terminal and PTY work is
timing-sensitive; a test that passes once has told you almost nothing.

### 5.1 Layers

1. **Unit, no tmux.** Parser and encoder tests in `termirust-tmux`: real `list-sessions`
   output, the no-server stderr case, names with punctuation and spaces, truncation
   limits, and hex encoding of control bytes. Table-driven, fast, no processes.
2. **Unit with a fake tmux.** `MULTIPLEX_TMUX_PATH` is already the injection seam
   (`local.rs:391-410`). Point it at a fixture script that prints canned output, including
   a failure case, and test discovery without a real server. Copy the injected-probe
   pattern at `local.rs:576-604`, which passes `is_file` and `version_probe` closures.
3. **Integration with real tmux.** Use the skip idiom so the suite stays green where tmux
   is absent (`local.rs:767-771`):
   ```rust
   let Ok((tmux, _)) = tmux_probe() else {
       eprintln!("skipping tmux integration test: tmux is unavailable");
       return;
   };
   ```
   Always isolate the server and clean up. Set `TMUX_TMPDIR` to a temp dir per test and
   copy the `TmuxSessionGuard` drop-kill pattern (`local.rs:507-525`) plus the unique-name
   builder (`local.rs:772-781`). Never touch the developer's own tmux server.
4. **Listener end to end.** One authenticated round trip proving a tmux session lists,
   attaches, replays, and accepts input under a writer lease. Copy the harness in
   `crates/termirust-controller-listener/tests/desktop_host_golden.rs` — it has
   `HostProcess::spawn`, `MutableAuthority`, `connect_controller`, `attach_from`, and
   `input_and_collect` ready to reuse.
5. **Desktop UI (M2).** Follow the `ui::app::tests::e2e_*` convention: drive the real
   rendered control and assert persisted state, as
   `e2e_settings_controls_persist_and_reset_preferences` does.
6. **Manual, once per milestone.** Open a Terminal.app tab (wrapped per the guide),
   confirm it appears on a paired phone, watch output, take the writer lease, type, then
   release. Record what you did and what you saw.

### 5.2 The loop to run while implementing

Fast inner loop, run after every meaningful edit:

```bash
cargo check -p termirust-tmux -p termirust-controller-listener --all-targets
cargo test -p termirust-tmux
cargo test -p termirust-controller-listener
```

Before declaring a milestone done:

```bash
./scripts/verify/rust.sh focused
cargo test --workspace --all-targets --locked
```

Prove the timing-sensitive tests are stable — this is the "multiple tests" requirement,
and 25 iterations is the repo's own standard for tmux regressions
(`.github/workflows/ci.yml:61-65`):

```bash
./scripts/test/repeat.sh 25 <your::exact::test_name>
```

If a run fails intermittently, fix the race. Do not add sleeps until it passes; use the
deadline-and-poll helpers the repo already uses (`wait_for_event`, `wait_for_file`,
`wait_for_tmux_readiness`). Treat a flaky test as a product bug: the phone will hit the
same race.

### 5.3 When you are stuck

Report it, with the failing command and its output. Do not disable a test, widen a
timeout past a few seconds, mark something `#[ignore]`, or weaken an assertion to get
green. The two places where stopping is the correct move are Spike 1 failing (§4 M1) and
any gate that appears to require editing an immutable baseline (§6).

---

## 6. Gates — all must pass

### 6.1 Rust

```bash
./scripts/verify/rust.sh focused      # fmt, gpui boundaries, mcp + browser gates, desktop check, clippy-on-changed, 2 tmux regressions
./scripts/verify/rust.sh workspace    # what CI runs
./scripts/verify/rust.sh policy       # toolchain pins + cargo-deny
```

- Toolchain is pinned: rustc exactly `1.97.1`, cargo-deny exactly `0.19.8`, and every
  workspace package must declare `rust-version = "1.88"` (`scripts/verify/rust.sh:21,25,60`).
- `scripts/dev/clippy-changed.py` fails on any clippy warning whose span overlaps a line
  you touched, and on all lines of new files. Passing `cargo clippy` is not enough.
- `scripts/verify/gpui-boundaries.py` forbids `gpui` in domain, store, and `*-protocol`
  crates. The new `termirust-tmux` crate must not pull GPUI in.
- **Known repository state, not your fault:** the MSRV job (`cargo +1.88.0 check`) fails
  because `crates/termirust-slate` depends on gpui-pre, which needs Rust 1.93. That
  decision is pending with the repository owner. Do not try to fix it, and do not let it
  block you; confirm your own crates declare 1.88.

### 6.2 Contracts, when touching tokens or copy

```bash
cargo run -p termirust-ui-contract --bin generate-messages
cargo run -p termirust-ui-contract --bin generate-tokens
./scripts/verify/design-tokens.sh --all-ui --no-new-baseline
./scripts/verify/localization.sh --locales en-US,en-XA,ar-XB --no-new-baseline
./scripts/verify/contrast.sh design/tokens.toml --all-states
```

Never hand-edit `crates/termirust-ui-contract/src/generated*.rs`, `locales/en-XA.ftl`,
`locales/ar-XB.ftl`, or `design/generated/tokens-contract.json`. Both lint baselines
(`design/legacy-visual-literals.toml`, `design/legacy-user-copy.toml`) are immutable and
must gain zero new entries.

### 6.3 UI rules for M2

- **Every user-visible string** goes in `locales/schema.toml` (with mandatory `context`
  and `description`) plus `locales/en-US.ftl`, then regenerate. A raw string literal in
  `crates/termirust-desktop/src/ui/**` near `.label(`, `.child(`, `.tooltip(` and friends
  fails the copy lint.
- **No raw colors or sizes.** Use `theme::` accessors; `px(theme::SPACE_MICRO)` passes,
  `px(12.)` does not. `crates/termirust-desktop/src/ui/theme.rs` is the only exempt file.
- **A new `SettingId`** means updating `SettingsSectionId`/`SettingId`, the `ALL` arrays
  **and their length literals**, `section()`, `label()`, `description()`, `kind()`, plus
  the two tests that assert the inventory count
  (`crates/termirust-ui-contract/src/settings_surface.rs:111,885,964`), plus
  `setting_presentation` in `crates/termirust-desktop/src/ui/app/settings_surface.rs:66`.
- **A new UI file** must be added to `files_for_surface("settings")`
  (`crates/termirust-ui-contract/src/surface_scope.rs:36-65`), or the scoped gate skips it
  silently. Code inside `library.rs`/`mod.rs` must sit within the
  `// termirust-ui-surface:settings:start|end` markers.
- **A new `AppSettings` field** needs `#[serde(default)]` or a `default_…` helper
  (`crates/termirust-desktop/src/models.rs:739-809`), or existing `state.json` files stop
  loading.
- Run the surface gate:
  ```bash
  ./scripts/verify/ui-surface.sh --surface settings --states all --locales en-US,en-XA,ar-XB --themes all
  ```

### 6.4 Environment facts

- The Docker-backed SSH and SFTP tests cannot run here: `DOCKER_HOST` points at a remote
  machine, so the fixture bind mounts resolve on that host and fail. They self-skip.
  Report them as skipped, never as passed.
- `termirust-client`'s `real_host` test is a known flake under full-suite load; it passes
  in isolation. Confirm rather than chase.

---

## 7. Pitfalls, collected

1. `/opt/homebrew/bin/tmux` is a symlink; `LaunchDescriptor` rejects symlinked
   executables. Canonicalize.
2. The session host clears the environment and forces `TERM`; pass `PATH`, `HOME`, `SHELL`.
3. `ControllerSessionSummary` is `#[serde(deny_unknown_fields)]` with a hand-written
   redacting `Debug`. New fields need `#[serde(default)]` and a `Debug` entry. Prefer
   adding none.
4. `HostConnectionBackend` is built as a struct literal in two in-crate tests
   (`host_backend.rs:757-769,827-839`); a new field breaks them.
5. The list revision mix and the shadowing retain both need extending for a third source.
6. tmux session ids (`$0`) are reused across server restarts. Include `#{session_created}`
   in the stable-id hash.
7. `MAX_LIVE_HOSTS` is 32 in the session host; spawning one host per attached tmux session
   consumes that budget. Tear hosts down on `Detach`.
8. A `Detach` must never kill the user's tmux session — only the spawned host and its
   read-only client. Assert this in a test.
9. Session names are user data. They already flow through the redacting `Debug`; keep it
   that way and do not log them.

---

## 8. Definition of done

Per milestone:

- Every gate in §6 passes, with the environment caveats reported honestly.
- New behavior has tests at the layers in §5.1, and the timing-sensitive ones survive
  `./scripts/test/repeat.sh 25`.
- One manual run recorded: what you did, what appeared on the phone, what did not work.
- `CLAUDE.md` updated if the product shape or architecture changed (it is the contributor
  contract, and its feature lists are maintained per feature).
- `docs/remote-terminals.md` updated so "What works today" and "What has to be built"
  stay true.
- A completion-evidence note under `docs/completion-evidence/` recording the exact commands
  run and their PASS/FAIL/SKIPPED results. Skips are reported as skips.
- No ADR is required for M1 or M2. Add one under `docs/decisions/` if you change a trust
  boundary, a protocol, a persistence model, or add a network route — M4 (route parity)
  likely qualifies.

Commits: `type(scope): imperative summary`, authored by `terminoid`, which is already set
in this repository's local git config. Do not touch the global git config, and do not add
attribution or session links to commit messages. Work on a branch off `test`.
