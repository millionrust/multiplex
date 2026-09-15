# TUI exact Codex Session resume

Goal 15.6 adds a reviewed TUI route for continuing one exited, unarchived, exact Codex 0.150.1
Session. It does not add a second resume planner or launch authority. The TUI delegates preview and
commit to `LocalCommandService::SessionResume`, which remains the shared typed route over the
existing store, resume planner, Session Host launcher, and continuity repository.

## Review and commit boundary

The `c` command is available only while fleet Sessions have focus. Preview is read-only and returns
the selected source revision, verified provider and version, fixed `read_only` policy, and next
occupant generation. The TUI then loads the same authoritative source Session and rejects a changed
identity, revision, lifecycle, or archive state. Provider handles, paths, commands, terminal content,
and provider metadata do not cross the review boundary.

Enter dispatches exactly one command with the reviewed Session ID and revision. The existing CLI
route repeats all eligibility and recognition checks, launches one replacement Host, records
continuity only after that Host is ready, and returns the durable successor. The TUI accepts success
only when provider `codex`, version `0.150.1`, policy `read_only`, generation, source revision, live
lifecycle, and committed continuity all match the review.

## Cancellation and races

Escape is the safe default while previewing or reviewing. Generation fencing discards stale preview
and completion events. After Enter, resume is irreversible from the TUI: Escape, `q`, Ctrl-C, and
repeated Enter do not cancel, quit, or redispatch an ambiguous launch. Conflict results expose only
the current revision and require an authoritative refresh before another review.

On verified success, fleet state is refreshed and the successor is selected only if it appears in
the refreshed visible scope. The exited source remains unchanged and no terminal is attached
implicitly.

## Scope boundary

This decision adds no generic provider resume, writable resume, remote or mobile authority, protocol
field, provider-content import, automatic retry, bulk action, attach side effect, dependency, or new
store format.
