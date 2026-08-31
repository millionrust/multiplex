# Atomic replica repository and shared-folder transport

## Decision

TermiRust persists local-first replication state in a dedicated versioned repository under an
application-selected private root. The authoritative file contains a canonical
`ReplicationDocument` plus only the opaque secure-store references required to open retained key
epochs. It cannot contain raw authority, device, or epoch key material. This repository is not the
legacy portable vault bundle and does not change the current production storage default.

An optional adapter exchanges only the canonical sealed replication document through an existing
folder explicitly selected by the caller. It does not export local custody references, journals,
plaintext records, terminal output, credentials, or paths. The adapter has no account, cloud API,
network client, background watcher, or operated TermiRust service.

## Private repository format

The current private layout is intentionally small and fixed:

```text
<private root>/
  replica.lock
  replica.json
  replica.last-good.json
  replica.transaction.json  # present only while secure-store retirement is pending
```

The root is `0700` and created files are `0600` on supported Unix systems. A symlink or special
file at the root, lock, primary, backup, or journal boundary is rejected. Repository JSON is capped
at 8 MiB plus 64 KiB of envelope overhead; the retirement journal is capped at 16 KiB and 64
references. Encoded opaque references are exactly 47 bytes and their sequence visitors stop before
allocating beyond the declared item and byte bounds.

Every mutation holds a process guard and an advisory filesystem writer lock across current read,
expected-revision comparison, validation, and activation. A same-directory temporary file is
written completely, synchronized with `sync_all`, renamed over the destination, and followed by a
parent-directory sync when supported. Rust documents that `sync_all` attempts to flush file content
and metadata, while `rename` replaces a destination but cannot cross mount points
([`File::sync_all`](https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all),
[`fs::rename`](https://doc.rust-lang.org/std/fs/fn.rename.html)). The API returns `Full` or
`RenameOnly` durability instead of claiming unsupported directory synchronization succeeded.

The repository keeps one previously validated primary as last-good evidence before activating its
successor. Current-format corruption or oversize may expose that copy as a read-only snapshot. A
newer primary, unsafe entry, wrong workspace, invalid authority policy, or stale revision never
falls back and never rewrites evidence. Corruption with no valid backup remains corruption rather
than silently becoming an empty repository.

## Commit and secret-retirement ordering

Epoch retirement uses the following crash-safe order:

1. validate the complete successor document and custody transition;
2. prove the declared retired set exactly equals owned epoch references removed by the successor;
3. persist the validated current primary as last-good;
4. atomically persist a bounded journal naming the successor revision and opaque retired refs;
5. atomically activate the successor repository document;
6. delete journaled secure-store items idempotently;
7. remove and directory-sync the journal.

Authority/device references cannot be retired through this epoch path. An uncommitted journal is
discarded without deleting secrets. A committed journal survives restart and a locked, denied, or
unavailable credential store. Retry treats an already missing item as completed, so a partial
multi-item retirement converges without delete-before-commit. Removing a secure-store item is not
described as cryptographic erasure from SSDs, snapshots, backups, synchronized keychains, or cloud
storage.

## Shared-folder contract

The shared folder must already exist and must not itself be a symlink. A caller supplies a random
64-character lowercase hexadecimal slot, which becomes the only variable path component:

```text
.termirust-replica-<opaque-slot>.json
.termirust-replica-<opaque-slot>.lock
```

Workspace, replica, collection, and record IDs never become filenames. Pull bounds bytes before
parsing, validates schema/workspace/authority policy, requires canonical encoding, and returns the
SHA-256 digest of the exact bytes as an observed content revision. Publish accepts either observed
absence or that exact revision, rechecks under the local lock, and refuses appeared, disappeared,
or changed content before atomic replacement. Two cooperating writers in one process or on one
mounted Unix filesystem therefore cannot both replace the same observed different state.

This is optimistic filesystem compare-and-swap, not a distributed lock. A cloud-folder provider
can propagate another device's write after the local recheck, and its own conflict policy remains
authoritative. Syncthing documents that simultaneous differing edits can create propagated
`.sync-conflict-...` copies and that destination updates use temporary files rather than direct
writes ([Syncthing synchronization](https://docs.syncthing.net/users/syncing)). TermiRust therefore
does not use modification time to choose a winner and does not claim provider-level atomicity.

The adapter recognizes only bounded slot-specific conflict-copy name patterns, validates each as an
inert sealed document, sorts evidence deterministically, and caps evidence at 16 artifacts while
scanning at most 1,024 directory entries. It never executes, decrypts, silently deletes, or
automatically selects a conflict copy. E12.7 owns user-facing review and recovery.

## Platform and product boundary

The current lock implementation fails closed outside Unix rather than pretending a no-op lock is
safe. macOS and Linux use the same `flock`-based path; later platform acceptance must add and test a
Windows lock adapter before a Windows persistence claim. Filesystem tests use disposable local
directories and injected writers. No live Dropbox, iCloud Drive, Google Drive, Syncthing, keychain,
account, or user data is touched.

D04 still owns final retention count/duration, backup/export inclusion, recovery policy, and delete
wording. D05 still prohibits an operated TermiRust cloud service. No production UI, automatic
folder choice, polling worker, provider-specific resolver, or legacy bundle migration is decided
here.
