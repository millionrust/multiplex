# Safe durable Session management in the TUI

Date: 2026-08-30

## Decision

`termirust-tui` may launch an existing safe preset and apply bounded Session rename, pin,
mark-read, stop, archive, and restore commands through `termirust-cli`'s typed local command
facade. The TUI does not edit store files, construct executable text, adopt a PID, or bypass the
durable Session Host.

Management shortcuts exist only in fleet focus: `n` launches, `e` renames, `p` toggles pin,
`m` marks read, `s` stops, and `a` archives or restores. While a terminal is attached, every one
of those keys is sent unchanged to the PTY. Management forms trap focus until cancelled or a
single reviewed command is submitted.

## Identity and replay

Every submitted command has a new typed `CommandId`, typed Project/preset/Session identifiers,
and the Session revision captured when its form opened. A management launch derives its stable
Session identity from the command identity, so replay after a lost response returns the committed
Session without launching a second Host. Reversible metadata replay succeeds only when the desired
state is already present; otherwise a stale revision remains a conflict. Undo creates a new command
against the revision returned by the original mutation and expires after ten seconds.

Stop reads the selected Session's persisted `HostInstanceId`, connects only to that Session's local
endpoint, verifies the connected Host identity, and asks that Host to stop its internally owned
process. It never signals a raw PID or process name. Archive of a live Session is one reviewed
stop-and-archive workflow: metadata is archived only after confirmed Host exit. Restore changes
metadata only and never launches a process. Launch and stop never claim undo.

## Failure and privacy

Selection changes cannot retarget in-flight work. Stale generations are discarded, stop cannot be
cancelled after dispatch, and a failed or uncertain stop leaves archive uncommitted. Errors expose
bounded stable classes and recovery text. User titles are isolated for terminal rendering and
redacted from command debug output; paths, argv, terminal output, transcripts, and credentials do
not enter management state or diagnostics.

## Verification

Run:

```sh
./scripts/verify-tui-management.sh
```

The suite covers typed command replay and revision conflicts, bounded forms and cancellation,
fleet/terminal focus separation, no-color and pseudo/RTL rendering, exact Host ownership,
stop-and-archive failure ordering, and survival of an unrelated sentinel process.
