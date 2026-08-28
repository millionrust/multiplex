# Local Diagnostics

TermiRust keeps a bounded local record of allowlisted operational metadata. The
diagnostics path is intentionally separate from terminal scrollback, session
history, and application state.

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
Clear from deleting paths that are not managed by TermiRust.

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
  directory, then restart TermiRust if initialization failed.
- **Source changed**: run Preview export again.
- **Privacy scan failed**: no bundle was published. Keep the source files for
  engineering investigation; do not bypass the scanner.
- **Destination exists**: choose a new filename. TermiRust will not overwrite it.

Clear is best effort. Removed bytes may remain in filesystem snapshots, backups,
or storage media and are not described as securely erased.

## Verification

```bash
cargo test -p termirust-diagnostics --all-targets
cargo test -p termirust -- ui::settings::diagnostics
./scripts/verify-diagnostic-bundle.sh tests/fixtures/diagnostics/export-policy.json --no-network
```

The bundle verifier checks the frozen manifest contract, private file mode,
secret canaries, strict schema behavior, and the absence of a production network
or child-process route in the diagnostics crate.
