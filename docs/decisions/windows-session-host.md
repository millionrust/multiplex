# Windows Session Host

Status: accepted; implementation in progress

Reviewed: 2026-09-21

## Why

A durable session is a terminal owned by a Session Host process rather than by a window, so it
outlives the window and every client — the desktop app, the CLI, a paired phone — reaches it the
same way. On macOS and Linux the Host exists; on Windows `multiplex-session-host` compiles
`host_unsupported.rs`, so Windows has no durable sessions and no way to make the terminals a person
opens in PowerShell or Command Prompt reachable from a paired device. Everything in
[remote-terminals.md](../remote-terminals.md) under "Windows" depends on this Host.

The Unix Host's guarantees are the contract; the Windows Host keeps each one with the platform's
own mechanism rather than weakening it.

## What each Unix guarantee becomes

| Guarantee | Unix | Windows |
| --- | --- | --- |
| Only this user reaches a Host | `0700` runtime directory, `0600` socket, `SO_PEERCRED` uid check on every connection | A named pipe whose DACL grants access to this user's SID alone, with remote clients rejected and the first instance required, plus a check of the connecting process's token user against this user on every connection |
| A client finds the Host | The socket file at `runtime_root/<opaque endpoint>` | The same path holds a small endpoint file naming the pipe; the pipe name is `\\.\pipe\multiplex-<opaque endpoint>` and carries no session identifier |
| The endpoint was not planted by someone else | Socket file type, owner, mode, and device/inode compared at bind and at removal | The runtime root is under the user's profile, whose inherited ACL already excludes other standard users; it must not be a reparse point, and the endpoint file is created new (`create_new`) and removed only if its file index is the one written |
| One Host per slot, bounded | `flock` on `host-slot-NN.lock` | `File::try_lock` on the same files; the standard library maps it to `LockFileEx` |
| The terminal | `openpty` via `portable-pty` | ConPTY via `portable-pty` |
| The process tree it stops | The child is a process-group leader; `kill(-pgid, …)` | The child is assigned to a job object at spawn; the job is the tree |
| Interrupt (first stop step) | `SIGINT` to the group | `Ctrl+C` (`0x03`) written to the pseudo-console input, which is what a person pressing it does |
| Terminate (second step) | `SIGTERM` to the group | `TerminateJobObject`: the Host keeps the pseudo-console open for resizes until the process is gone, so there is no gentler second step to send; programs get their graceful chance at the interrupt |
| Kill (last step) | `SIGKILL` to the group | `TerminateJobObject` |
| Process identity in tokens | The process-group id | The child's process id; the job handle is held by the Host for the child's life |

## Consequences

- The wire protocol, journal, activity, resume, and every client-facing behavior are shared code.
  Only transport, directory trust, locking, and process control differ, and they sit behind one
  platform seam in the Host and one `PlatformStream` in the client.
- A Windows Host cannot be reached across users on the same machine, including by an administrator
  impersonating no one: the DACL names one SID. An administrator can still take ownership of the
  pipe or read the process's memory, as root can on Unix; that is outside this boundary on both.
- A process the shell starts in the first instant after spawn, before the child is assigned to the
  job, is not in the job. `portable-pty` cannot create the child suspended; the window is the time
  between `CreateProcess` returning and `AssignProcessToJobObject`, and a shell starts nothing that
  early.
- Behavior that depends on Unix signals — a program that traps `SIGTERM` to save state — sees the
  Windows equivalents instead. Console programs already handle `Ctrl+C` and close events.
- Nothing here is verified by a person on real hardware yet. CI's Windows runners run the Host's
  test suite; the external checks in [N14](../engineering-evidence/N14-linux-windows-parity.md)
  still apply.
