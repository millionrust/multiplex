# Session Host runtime review — 2026-10-12

Scope: the desktop → raw CLI → durable Session Host path, live/historical replay,
terminal input and resize, bounded queues and state, writer leases, compaction,
reconnect, native process ownership, and shutdown. This is not an audit of the
entire application or third-party terminal emulators.

## Findings and changes

1. **High: idle replay and state polling scaled with journal history.** Raw CLI
   viewers poll every 10 ms. `serve_attach` decoded the complete retained journal
   to return only its tail; `send_state` decoded it again to find its boundaries.
   Every attached desktop viewer could therefore scan history up to 200 times a
   second, allocating decoded payloads on each pass. Live replay now uses a cache
   capped at 4 MiB and 4,096 records. Idle polls return an empty tail without disk
   reads; state uses sequence metadata maintained by append/open/compaction.
   Historical replay still validates journal records on disk.

2. **High: logs above the replay limit could not reconnect.** Replay permits
   8 MiB; normal disk compaction starts around a 64 MiB segment. A reconnect from
   sequence zero could exceed the protocol limit before compaction provided a
   usable snapshot. Oversized replay now returns the current screen and journal
   boundary, followed by normal live output. Snapshot/tail recovery holds the
   journal lock across both reads, excluding concurrent compaction. Output
   updates take journal → parser locks in that order so a generated live snapshot
   represents the boundary it advertises.

3. **Medium: typing repeatedly resized an unchanged PTY.** The raw bridge sent
   Resize before each input chunk. It now compares the authoritative Host viewport
   with the frontend size; the Host also ignores unchanged native dimensions.
   This reduces RPC work and avoids disturbing ConPTY input during typing.

4. **High: resize lacked startup's allocation limits.** Startup dimensions were
   limited to 1,000 rows/columns, but Resize accepted dimensions up to `u16::MAX`.
   Resize now enforces the same limit before PTY or screen-model allocation. A
   regression verifies rejection while another viewer remains connected.

5. **Windows: nested console mouse negotiation was lost.** ConPTY consumes VT
   mouse requests. The raw bridge now preserves VT input/output, mirrors the
   Host's requested mouse mode through the native console API, disables Quick
   Edit during raw attachment, and restores prior modes on exit. Native Windows
   end-to-end validation is tracked separately before release publication.

6. **Medium: snapshots omitted TUI input modes.** Snapshot rendering now restores
   alternate-screen state, mouse mode/encoding, bracketed paste and application
   keys along with the visible cells. The regression also verifies that returning
   to the shell clears stale mouse modes.

## Evidence

- 137 Store/CLI/Host library checks passed after the bounded-cache/resize changes.
- 40 Host checks passed, including shutdown/owned-grandchild cleanup, recovery,
  replay, client disconnect handling, and 1,000 GUI disconnect/reconnect cycles.
- 193 Store/Host checks passed after reconnect changes, including a real shell
  producing 9 MiB of logs, reconnecting twice, then accepting input and returning
  fresh output. The new retained-sequence open/compact/reopen check also passed.
- All 52 desktop terminal interaction checks passed with freshly built helpers;
  the additional TUI snapshot-mode round-trip regression passed.
- An isolated 8 MiB journal experiment measured approximately 1.45 ms per idle
  disk scan versus 8 ns per cached empty-tail lookup on this machine. This is a
  journal microbenchmark, not a measurement of complete Host CPU or network cost.
- The observed process snapshot contained 177 Session Hosts, mostly old debug
  test processes. Aggregate CPU was approximately 51.5%, RSS approximately
  909 MiB; no individual Host was above 5% at that moment. One sampled test Host
  still held files in a prior isolated test directory. Production sessions were
  not terminated as part of inspection. This count alone does not establish a
  per-Host heap leak.

## Limits and follow-up

- The reported individual 100% CPU episode was not live during inspection. The
  repeated history scans are a concrete scaling defect; final steady-state CPU
  and memory should also be measured under the user's ongoing desktop log load.
- Host PTY and activity queues are bounded (64 output frames, 128 activity events);
  mutation/nonce caches and simultaneous connections are bounded. Scrollback
  retains up to 10,000 lines, so continued output can legitimately increase
  memory until that bound; a leak has not been demonstrated.
- Historical replay can still decode disk history once when outside the live
  cache. Its temporary cost is distinct from the eliminated per-poll scans.
- Old test-process accumulation requires ownership-verified cleanup. The current
  desktop test helper sends Force only to matching Host identities before deleting
  its isolated store; existing production shells must remain untouched.
- GitHub Windows repeated mouse validation and the final release builds must pass
  before publishing these changes.
- Existing detached Hosts keep their old executable code across app updates.
  The Host fixes apply to newly started sessions; the review does not restart or
  terminate the user's running shells.
