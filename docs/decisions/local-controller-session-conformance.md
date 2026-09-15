# Local Controller Session mutation conformance

Goal 15.7 freezes a synthetic Session metadata lifecycle shared by the desktop Session library,
local CLI, and Rust TUI. It adds no production command or mutation authority. Each path continues to
use its existing adapter and the same domain reducer and atomic Session repository remain
authoritative.

## Frozen projection

`tests/fixtures/controller/local-session-mutation-v1.json` defines one exited Session with bounded
synthetic IDs and no path, terminal, provider, credential, route, environment, or executable data.
The fixed sequence is rename, pin, mark read, archive, and restore, followed by an intervening write
and stale rename attempt.

The normalized projection includes repository and Session revisions, typed identities, title and
source, lifecycle, activity, pin, output/read/unread sequences, unread status, and archive status.
Wall-clock creation/update/archive values are excluded because each interface owns a different
clock boundary and their exact value does not alter Session semantics.

## Independent consumers

The desktop test enters through `SessionLibraryState`, including its compatibility `SavedState`
projection. The CLI test enters through `LocalCommandService`; the TUI test enters through
`LocalManagementExecutor`. CLI and TUI use separate equivalent temporary stores so one path cannot
make the other pass by reusing its result.

Every command captures the current Session revision. The stale case first commits an independent
pin mutation, then submits the prior revision. Desktop reports the store stale-revision class, CLI
reports typed conflict, and TUI reports its bounded conflict projection. All three reload the same
final state and leave exact authoritative bytes unchanged during the rejected attempt.

## Scope boundary

This contract does not claim conformance for Host process control, launch, stop, resume, remove,
terminal attachment/input/resize, remote routes, mobile, transcripts, artifacts, providers, MCP,
browser, relay, release platforms, or wall-clock values. Those retain their existing focused
acceptance suites and authority gates.
