# TUI Session removal

Goal 15.4 extends fleet-focused TUI management with reviewed removal of an exited archived
Session. It does not add a deletion implementation. `SessionRepository::remove_session` remains the
only authority: metadata is removed atomically and exact owned Session data is moved to the existing
quarantine after containment and manifest checks.

## Review and commit boundary

`LocalCommandService::prepare_management_removal` accepts a typed Session ID and captured Session
revision. It verifies the selected record, scans the bounded store manifest, and returns only the
repository revision plus aggregate metadata, journal, transcript, artifact, and file counts. Paths,
filenames, content, terminal output, and provider data never cross into the TUI.

The final typed command carries that exact repository revision and aggregate manifest. The service
recomputes the plan immediately before commit and rejects any revision or manifest difference. The
store then scans once more under its lock and rejects a changed plan, unsafe entry, symlink,
non-directory root, quarantine collision, or non-exited/non-archived lifecycle.

## Confirmation and lifecycle

The command is available as `x` only while fleet Sessions have focus. An attached terminal receives
the byte literally. Manifest loading may be cancelled with Escape. Review requires `REMOVE` for a
metadata/journal-only Session or the exact Session title when transcripts/artifacts exist. Input is
bounded to 256 Unicode scalars and excluded from `Debug` and errors.

Once dispatched, removal cannot be cancelled or undone. Quarantine is internal recovery evidence,
not a user-facing undo promise. The TUI does not stop a process, purge quarantine, recursively delete
an arbitrary path, or retry an ambiguous result. Success refreshes the authoritative fleet; failure
keeps the reviewed result visible and requires refresh before another command.

## Scope boundary

This decision adds no bulk removal, live stop-and-remove, secure-delete claim, retention policy,
remote/mobile/MCP authority, protocol field, account, service, telemetry, or dependency.
