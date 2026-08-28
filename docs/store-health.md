# Store Health and Derived-Index Repair

TermiRust's **Settings > Health** workflow separates diagnosis from repair. A scan is
explicit and read-only. It checks the store format, authoritative record readability and
hashes, and the two supported derived indexes.

## Authoritative Data

The health service treats these metadata files as authoritative:

- `format.json`
- `projects.json`
- `sessions.json`
- `presets.json`

A derived-index repair never writes these files and never reads project contents,
provider output, terminal journals, or session output. A future-format or malformed
authoritative source disables every repair action.

## Repairable Indexes

Only these marker-owned files under `derived-indexes/` can be rebuilt:

- `project-session-v1.json`
- `palette-v1.json`

The builders use stable ordering and canonical JSON. The same source revisions and bytes
therefore produce identical index bytes and SHA-256 digests. The project/session builder
rejects more than 10,000 sessions in one project.

## Atomic Repair

A named repair follows this state machine:

1. `Planned`: capture exact source revisions and hashes.
2. `BuildingTemp`: write a private marker-owned temporary file.
3. `Verifying`: reopen it and verify its bytes and digest.
4. `Publishing`: take the metadata lock, re-read all source revisions and hashes, journal,
   and atomically rename the temporary file.
5. `Complete`: reopen the published index and verify it before removing the journal.

Cancellation is supported before publishing and removes the temporary file. A source
change makes the plan stale and publishes nothing. On restart, TermiRust only cleans up
temporary files covered by its exact ownership marker and completes verification of a
journaled publish.

On Unix, the derived-index directory is mode `0700`; markers, temporary files, journals,
and published indexes are mode `0600`. Symlinked or unexpected entries fail closed.

## Operator Verification

Run the bounded repair suite with:

```sh
./scripts/test-index-repair.sh --fixtures tests/fixtures/health-index --crash-matrix
```

The suite checks deterministic output, authoritative-byte preservation, cancellation,
stale revisions, malformed and symlinked indexes, and every supported crash injection
point. The health workflow intentionally has no reset, delete, Host-control, network, or
arbitrary repair action.
