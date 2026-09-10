# Mobile Replication Provider Contract (C06)

Status: initial read-only adapter and qualification fixtures. Not encrypted sync.

## Boundary

The existing Rust shared-folder transport uses an advisory filesystem lock, a
content revision comparison, and atomic local publication. Those are cooperating
local-writer guarantees, not cloud-wide compare-and-swap (CAS). Preserve that route.
Neither SAF write flags nor NSFileCoordinator establishes distributed CAS.

The mobile route is explicit document transfer. The native adapter reads one
user-selected document into bounded memory. It returns untrusted bytes, never a
Rust filesystem path, authenticated document, durable revision, or successful sync
receipt. Rust must parse canonical bytes, verify authority/workspace/recipient and
decrypt using existing custody before presenting a review or mutating local state.
The same exact bytes reviewed must be applied; do not reopen a provider at apply.

Kinds and bounds match existing Rust contracts:

| Kind | Maximum bytes |
|---|---:|
| Enrollment bundle | 196608 |
| Encrypted replication document | 8388608 |

Zero bytes and limit+1 are errors. Read to EOF with at most limit+1 bytes; provider
size, MIME type, display name, mtime and URI are not trusted content validation.
Read errors discard partial data. A changing provider can yield inconsistent bytes:
only Rust validation may accept them. No native signature or encryption implementation.
Do not log bytes, paths, URIs, bookmarks, credentials or provider exception messages.

## Platform Rules

- Android accepts only `content:` document URIs through ContentResolver. Never use
  `Uri.path`, `_data`, or File(uri). Reads run off the UI thread. The caller owns a
  CancellationSignal and cancels it on abandonment; check it between chunks too.
- C06 holds no persistent provider grant. C07 must use ACTION_OPEN_DOCUMENT for a
  single reviewed transfer. A later persistent route must take only actually offered
  read grants, handle revocation by requiring reselection, and release only owned grants.
- iOS accepts a picker URL, balances successful security-scope access, and reads
  inside NSFileCoordinator's accessor using its supplied URL. A false scope result
  does not prove denial for an already-accessible sandbox file; actual access decides.
  Reject directories and symlinks. Never forward that URL to Rust.
- C06 stores no bookmark. Future reopened access must resolve a bookmark, reject
  stale access until reselection, and balance scope access again. A bookmark is not
  authority to import data.
- Missing/offline files and denied access are failures, not an absent repository or
  permission to create one. No automatic retry after uncertain provider operations.
- Revoking a URI grant prevents future opens; do not assume it invalidates a descriptor
  already handed out. Close each transfer handle promptly. Android reliable-pipe errors
  must discard even already-read bytes, never produce a partial successful transfer.
- Coordination/provider calls may block despite task cancellation. C06 does not
  promise a hard deadline for third-party provider code. UI integration must remain
  off MainActor and ignore results for abandoned review generations.

## Publication

`publish` always returns `unsupportedPublication` before opening or modifying a
provider. No rename/read-back/hash loop is called CAS. No overwrite, deletion,
background polling, provider directory scanning, network transport or last-writer-wins.

An eventual outbound workflow may offer a new explicitly reviewed export file via
the native picker, clearly labelled transfer-only. It must not replace the canonical
shared replica or mark sync complete. Automatic bidirectional publication requires a
separately qualified revision/atomicity contract and conflict recovery fixtures.

## Integration Sequence

C07/C08: select -> bounded native read -> Rust canonical/authenticated decode ->
verification-code review -> exact-byte service acceptance -> recoverable local commit.
The enrollment service's exchange directory remains app-private staging, never the
provider URL. Importing a host additionally requires a narrow reviewed-input service
API; copying provider data over `replica.json` is forbidden. No incoming UI is wired
in C06. C09 must separately qualify conflicts, revocation and convergence.

## Verification Scope

Android disposable DocumentsProvider fixtures exercise real ContentResolver access.
An instrumentation-APK service with a separate UID probes denied/granted/revoked
temporary URI access. Each probe closes its stream; this is not persisted-picker-grant
or already-open-descriptor revocation proof. Its exported test-only endpoint accepts
only the instrumented app UID and the synthetic fixture provider authority.
iOS disposable local files exercise real NSFileCoordinator and URL scope calls.
A separate two-app fixture also selects synthetic JSON through the actual local Files
picker, requires a scoped URL outside the reader container, and verifies exact bytes
through the production reader after releasing its initial scope probe. Cancellation
and restart/reselection pass. This does not establish third-party File Provider
hydration, bookmark persistence or grant revocation. Track those separately rather
than call local fixtures cloud-provider proof.

References: [Android document access](https://developer.android.com/training/data-storage/shared/documents-files),
[DocumentsProvider contract](https://developer.android.com/guide/topics/providers/document-provider),
[Apple coordinated reads](https://developer.apple.com/documentation/foundation/nsfilecoordinator/coordinate(readingitemat:options:error:byaccessor:)),
[security-scope lifetime](https://developer.apple.com/documentation/foundation/url/startaccessingsecurityscopedresource()).
Additional platform contracts: [Android URI grants](https://developer.android.com/reference/android/content/Context#grantUriPermission(java.lang.String,android.net.Uri,int))
and [reliable pipe errors](https://developer.android.com/reference/android/os/ParcelFileDescriptor#createReliablePipe()).
