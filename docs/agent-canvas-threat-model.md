# Agent Canvas Threat Model

Status: v1 release baseline

The Agent Canvas launches developer tools with access to repositories and can
move reviewed text between processes. Its trust boundary includes TermiRust,
local provider children, remote SSH hosts, repository content, and provider
output. Repository files, terminal output, provider JSON, and context are all
untrusted.

## Protected assets

- source code, uncommitted changes, Git branches, and indexes;
- SSH credentials, API tokens, key files, cookies, and environment variables;
- local and remote command execution authority;
- provider approval decisions and process identity;
- saved workspace integrity and terminal-session continuity.

## Threats and controls

### Argument and shell injection

Local agent processes use executable paths and argument arrays. Custom CLIs are
never launched through `sh -c`. The single remote shell boundary quotes the
executable and every argument with the established shell-quote helper. Node
titles are display-only. Tests cover spaces, quotes, and shell metacharacters.

### Malicious paths and destructive Git cleanup

Managed paths are canonicalized. The worktree root must be outside the source
repository. Cleanup verifies that the target is below the app-managed root and
is listed by `git worktree list --porcelain -z`. It refuses dirty worktrees,
untracked files, commits after the recorded base, submodule repositories,
active canvas references, and active terminals. It never uses force removal.

### Prompt injection and unintended command execution

Context links are pull/review operations, not continuous streams. Snapshots are
wrapped in an explicit untrusted-data boundary and are never interpreted by
TermiRust as commands. The user can edit or cancel the preview. Interactive
delivery goes through multiline paste confirmation.

### Secret disclosure

Context snapshots have line and byte limits and redact common token, password,
authorization-header, and private-key patterns before preview. Full transcripts
and previews are runtime-only. Agent definitions do not contain credential maps.
No automatic file collection or environment export occurs.

### Provider event spoofing and memory exhaustion

Only documented machine-readable stdout is parsed with `serde_json`; stderr is
diagnostic-only. Malformed messages produce a diagnostic instead of a panic.
Unknown fields are tolerated where safe. Event channels, diagnostic text,
transcripts, and context are bounded. Structured completion is driven by
provider terminal events, not by matching terminal prompts.

### Approval escalation

Approval cards expose the requesting operation and support deny or one-time
allow. No broad remembered policy is created. Provider danger/bypass/yolo modes
are rejected or omitted by default. Cancellation targets the owned child handle,
not a user-supplied PID.

### Dependency loops and cross-host confusion

Dependency cycles are rejected on creation and checked again before scheduling.
The scheduler has a concurrency cap of two and does not auto-retry or resume
after app restart. Context and dependency execution compares explicit execution
host identities and refuses cross-host use.

### Remote compromise and persistence confusion

Remote interactive agents reuse the existing SSH and TOFU boundaries. TermiRust
does not install a remote helper or provider executable. tmux persistence and
agent orchestration remain separate; canvas state cannot silently kill a tmux
server session.

## Residual risks

Redaction is defense in depth and cannot identify every secret format. A user
must review context. Provider tools can still modify everything permitted by
their native sandbox and the selected worktree policy. Interactive terminal
agents do not expose reliable structured completion. Remote hosts and provider
CLIs remain external trust domains. Live-output capacity requires release-machine
profiling in addition to pure geometry tests.

## Security regression checklist

- Run process argument and remote quoting tests.
- Run context limit and redaction fixtures.
- Run malformed JSON, unknown-field, cancellation, and abrupt-exit fixtures.
- Run dependency cycle, blocking, and concurrency tests.
- Run real temporary-repository worktree create/status/removal tests.
- Confirm state serialization contains no transcript, context preview, or secret.
