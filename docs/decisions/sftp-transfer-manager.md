# Bounded SFTP Transfer Manager

Status: accepted for the desktop v1 transfer surface

## Decision

TermiRust routes file uploads and downloads through one process-wide `SftpTransferManager`.
The manager admits at most three active jobs and 32 queued jobs. Every job has a typed direction,
operation ID, conflict policy, cancellation token, byte progress, resume offset, SHA-256 result,
and terminal event. Worker creation is fallible and queue saturation is reported before work starts.

Transfers stream in 256 KiB chunks and reject files larger than 8 GiB. Connection establishment has
a 45-second bound and SFTP operations use a 30-second channel timeout, with cancellation racing
connect and every remote chunk operation. Transfer bytes, credentials, and paths are not logged.

## Conflict And Publish Contract

The default policy is Ask. Existing destinations are unchanged until the user explicitly chooses
Replace, Skip, or an available Resume action.

- Upload writes to `<destination>.termirust-<operation>.part` and publishes by remote rename.
- Download writes to `.<name>.termirust-<operation>.part` in the destination directory, flushes and
  syncs it, then publishes by local rename.
- Replacement first revalidates the destination identity. An operation-scoped backup protects the
  existing file while staging is published; failed publish attempts restore that backup where the
  underlying filesystem or SFTP server permits it.
- Cleanup failure after successful publish is a warning, not a false transfer failure.
- Symlinks, special local sources, unsafe remote types, destination races, changed sources, and
  unverifiable final sizes fail closed with staging retained as evidence.

SFTP rename behavior is server-dependent and is not described as a transaction. The implementation
claims only the checks and best-effort rollback it actually performs.

## Resume Contract

Resume is limited to the staging path owned by the same operation ID. It never treats a completed
destination as staging. Before seeking, TermiRust streams and compares every staged prefix byte with
the current source while rebuilding the SHA-256 state. A mismatch refuses resume without modifying
the destination. Source size and modification metadata are captured before transfer and revalidated
before publish; remote source identity includes size, mtime, and permissions where the server
provides them.

Cancellation before start removes a queued job immediately. Cancellation during connect or stream
ends with an exact transferred-byte count and states whether resumable staging may remain. It never
claims remote effects were rolled back.

## User Surface

The workspace Files view exposes Up, Refresh, Upload, Download, Delete, and Terminal actions. One
visible transfer panel per workspace shows queue/running/conflict/cancelled/failed/completed state,
exact bytes and percentage, resume offset, conflict choices, retry/cancel actions, cleanup warnings,
and the final SHA-256. Directory refresh and transfer events use independent operation correlation,
so browsing cannot discard valid transfer progress or let an unrelated error overwrite it.

## Non-Goals

Recursive transfer, synchronization, delta transfer, compression, transfer persistence across app
restart, arbitrary concurrency, shell checksum commands, and automatic destructive retries remain
out of scope.

## Verification

Run `./scripts/verify-sftp-transfer-manager.sh`. Live acceptance requires Docker and fails rather
than silently claiming Docker coverage when it is unavailable.
