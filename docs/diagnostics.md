# Local Diagnostics

Multiplex keeps a bounded local record of allowlisted operational metadata. The
diagnostics path is intentionally separate from terminal scrollback, session
history, and application state.

## Desktop process logs

The desktop also saves its stdout, stderr, application log messages (Info, Warning,
and Error), and panic reports to a per-launch `.log` file under the app data
directory's `logs/` folder. On macOS this is
`~/Library/Application Support/multiplex/logs/multiplex-<timestamp>-<pid>.log`.
`MULTIPLEX_CONFIG_DIR` moves this folder along with the rest of the app data.
Unix log directories are mode `0700` and files are mode `0600`.

Terminal termination records identify the session and stop mode and report the
stage of any failure: metadata, connection, identity verification, request, or
timeout. A successful stop is logged too. Terminal contents stay on their PTY;
worker protocols retain their stdout and stderr pipes.

These local process logs are separate from the bounded, allowlisted diagnostics
below. They are not included in diagnostics previews, exports, or Clear. Process
logs may contain paths and raw errors or panic text and remain local until removed
or shared by the user.

## Privacy boundary

Each record must match the closed schema in `termirust-diagnostics`. A record can
contain only:

- a closed event code, severity, timestamp, and localization message ID
- an opaque random correlation ID
- closed component, operation, and state values
- coarse counts, duration buckets, booleans, and recovery actions

Unknown fields, unknown enum values, mismatched field/value types, and oversized
records are rejected. The schema has no string value capable of carrying user or
terminal content.

Diagnostics exclude terminal input and output, prompts, transcripts, clipboard
content, credentials, environment variables, command arguments, paths,
hostnames, usernames, device names, and artifacts. Panic records contain a
closed failure code only. Raw panic messages and backtraces are not persisted.

## Storage policy

Diagnostics are enabled by default and can be disabled in **Settings > Storage,
Privacy & Diagnostics**. The conservative default and maximum policy is:

- 10 MiB per file
- 5 rotated files
- 14 days retention
- 50 MiB maximum export bundle

Users may lower the per-file limit or retention period. The producer uses a
bounded nonblocking queue, so a full queue drops records instead of blocking a
terminal or PTY. A later safe record reports only the aggregate dropped count.

On Unix platforms the diagnostics directory is mode `0700` and files are mode
`0600`. Files use fixed non-identifying names. The ownership marker prevents
Clear from deleting paths that are not managed by Multiplex.

## Preview and export

Export is a two-step local operation:

1. **Preview export** flushes queued records, reads only fixed diagnostic files,
   strictly decodes and reserializes the closed schema, verifies a consistent
   source snapshot, applies a second privacy scan, and creates a private staged
   bundle.
2. **Export previewed bundle** copies that exact staged bundle to a new local
   destination and atomically renames it. Existing files are never overwritten.

The preview shows exact entry and byte totals, the redaction count, and included
and excluded data classes. Preparation and publication run off the UI thread and
can be cancelled. Cancellation and failure remove the exact temporary file and
leave any prior export unchanged.

There is no upload or network transport in the diagnostics crate. A bundle
leaves the device only when the user chooses a local destination and later
shares that file independently.

## Recovery

- **Dropping events**: terminal work remains unaffected. Lower activity or leave
  diagnostics enabled until the queue returns to Healthy.
- **Disk or permission error**: verify free space and access to the app data
  directory, then restart Multiplex if initialization failed.
- **Source changed**: run Preview export again.
- **Privacy scan failed**: no bundle was published. Keep the source files for
  engineering investigation; do not bypass the scanner.
- **Destination exists**: choose a new filename. Multiplex will not overwrite it.

Clear is best effort. Removed bytes may remain in filesystem snapshots, backups,
or storage media and are not described as securely erased.

## Verification

```bash
cargo test -p termirust-diagnostics --all-targets
cargo test -p termirust -- ui::settings::diagnostics
./scripts/verify/diagnostic-bundle.sh tests/fixtures/diagnostics/export-policy.json --no-network
```

The bundle verifier checks the frozen manifest contract, private file mode,
secret canaries, strict schema behavior, and the absence of a production network
or child-process route in the diagnostics crate.
