# C07 Android Enrollment and Reviewed Import

Status: in progress. A desktop-service-to-Android fixture now proves successful
enrollment, one reviewed encrypted host import, duplicate import and process restart
through packaged JNI, Keystore and the document reader. System-picker acceptance and
rollback of a failed, journaled activation now also pass on Android. Finalization of
a reconstructed post-profile cleanup state passes across native process restarts.
Write-ahead custody intents now prevent untracked epoch creation. Safe resolution of
ambiguous creation, cold-start reliability and exact-instruction crash timing remain open.

The latest system-picker runner passed 18 executions (seven provider cases and eleven
acceptance/recovery/lifecycle stages). This is separate from the preceding 41-execution
custody/UI matrix. JVM tests passed 88 cases with four existing live-SSH skips.
See the final runtime subsection for exact scope; earlier sections are chronological.

## Frozen Scope

1. Extend the separate Rust mobile facade to decode a canonical desktop bundle,
   match the exact pending request, and return display-only workspace/recipient/code
   claims. Review must not create secrets or activate enrollment.
2. Accept only the retained reviewed bundle bytes, exact request, reviewed workspace
   and independently compared desktop verification code. Reuse the product service's
   authenticated key unwrap and activation; do not implement Kotlin cryptography.
3. Add a narrow service boundary for reviewing an encrypted replica as supplied bytes
   and applying one reviewed host locally. Do not overwrite `replica.json`, synthesize
   provider filesystem paths, publish automatically or execute imported startup data.
4. Integrate Android ACTION_OPEN_DOCUMENT with the C06 reader, worker-thread facade
   calls, localized review/cancel/error states, and app-private recovery state. Ignore
   abandoned picker/read generations. Preserve Controller storage and pairing.
5. Regenerate/package native bindings and prove the complete workflow using real
   packaged Rust and Android Keystore, including restart, cancellation, duplicate
   import, wrong workspace/recipient, changed review bytes and missing keys.

## Current Slice

The Rust facade now exposes `review_enrollment` and `accept_enrollment`. Review is
explicitly not authentication: it returns canonical bundle claims and a comparison
code without custody mutation. Acceptance rechecks the pending request and reviewed
workspace/code under the existing facade lock, then delegates authentication and
activation to `ReplicationProductService`.

Existing transaction markers fail closed with RecoveryRequired. An Android recovery
workflow for interrupted activation is still required; merely reopening a facade is
not proof of interrupted-transaction recovery. Duplicate acceptance is rejected as
AlreadyConfigured rather than recreating custody.

Android artifacts have now been regenerated for these methods (see the mobile binding
slice below). iOS artifacts still contain the earlier API. Neither platform calls the
new methods from production UI yet; earlier C01-C06 evidence is not import acceptance
proof.

## Verification on 2026-09-08

`cargo test --locked -p termirust-replication-bindings` passed: eleven integration
tests, zero failures/skips; unit/doc targets contain zero tests. The new integration
case uses the real product service to create a desktop bundle and exercises review
without custody mutation, exact pending-request matching, oversized input rejection,
wrong recipient, cancelled request, wrong reviewed workspace/code, missing-key
failure without replacement, facade recreation, authenticated member activation,
duplicate acceptance rejection and reopening through the service.

Custody in this source test is an in-memory callback implementation, not Android
Keystore. No device, UI or interrupted-activation recovery proof is claimed here.
`cargo fmt -p termirust-replication-bindings` and `git diff --check` passed. Disk stayed
above the 15 GiB floor, with about 19 GiB free after testing; no cache/data deletion.

## Local-Only Record Transfer Slice

The product service now has `review_record_transfer` / `apply_record_transfer` in
`crates/termirust-store/src/replication/product/reviewed_transfer.rs`. They accept
bounded canonical document bytes and one selected record key, never a provider path.
Review authenticates the selected encrypted put using existing authority policy and
epoch custody. Other records are not imported. Deletions, conflicts and dominated
incoming values are rejected rather than silently resolved.

The review token binds exact input bytes, authority, local replica, local document,
repository revision and selected key. Apply repeats validation and authentication,
checks the token, and uses the existing repository revision CAS. It never invokes
shared-folder publication. A fresh review of an identical imported record is a no-op;
reusing an obsolete token fails. Transaction/recovery markers and stale in-memory
authority fail closed. Review plaintext is held in a zeroizing buffer; the existing
pinned zeroize dependency moved from dev-only to normal store dependencies.

This is a generic encrypted-record service boundary, not yet host-schema review,
native FFI exposure, Android UI acceptance or complete mobile sync. A full concurrent
authority/store transaction audit remains separate from deterministic stale-review
checks. No device test was run for this source-only slice.

Verification: `cargo test --locked -p termirust-store --test replication_product`
passed twelve tests, zero failures/skips. The new case proves exact selected plaintext,
no mutation on review, persistence across service recreation, unselected-record
exclusion, no files published to the member exchange, duplicate no-op, stale token
after local edit, changed canonical input rejection, wrong workspace, noncanonical
and oversized bytes, envelope context tampering, conflict/deletion rejection and
missing keys without replacement. Test secrets use an in-memory backend.

`cargo test --locked -p termirust-replication-bindings` also passed eleven integration
tests and empty unit/doc targets after the service addition. The first store build
failed because zeroize was dev-only; moving the already-pinned dependency corrected
that compile error. No failing run counts as passing evidence. Disk remained at least
18 GiB free during this slice; no user data or cache was removed.

## Mobile Binding Slice

`MobileReplicationProduct` now exposes record transfer review/apply through UniFFI.
Native callers supply bytes and typed collection/record identifiers; malformed tokens
are rejected before opening the service. Stale review/repository revisions map to
the existing changed-request error so callers must obtain a new review. Plaintext
returned for review remains inert and requires host-envelope/schema validation in the
product integration; it is not directly installed as a connection or executed.

The Rust binding suite passed eleven integration cases after extending the real
enrollment round trip with encrypted record review, invalid/stale token rejection,
local apply and duplicate no-op. This uses real Rust with in-memory custody, not
Android Keystore. The direct domain dependency and test-only JSON dependency are
recorded in Cargo.lock without unrelated version updates.

The Android artifact builder now requires all four new enrollment/transfer exports
on every ABI. Four artifact-publication/owned-process tests passed. Existing iOS
artifacts have not been regenerated for this extension.

Android verification on 2026-09-08:

- `bash scripts/build/mobile-replication-bindings.sh --android`: passed all four
  architectures, pinned UniFFI/NDK checks, required exports and 16 KiB alignment.
- `bash scripts/sync/mobile-replication-bindings.sh --android --write` and `--check`:
  complete set promoted; packaged inventory matches verified output.
- `python3 scripts/test/android-replication-custody.py --avd Pixel_9`: passed 26
  instrumentation executions, zero skipped: eleven custody cases; four existing
  enrollment UI cases at each of phone, tablet and landscape sizes; three separate
  prepare/reopen/cleanup process invocations. API 37, arm64-v8a, 16 KiB pages.
- Both debug APK builds and APK/library checksum checks passed. The existing real
  Keystore enrollment test now invokes all four new JNI methods with invalid input,
  checks their typed failures, and verifies that the pending request and sentinel
  identity survive. This is negative-path JNI proof, not successful mobile import.
- The owned read-only emulator stopped and temporary build slices were removed by
  their runners. No user app data, source or cache was deleted. Space stayed above
  15 GiB (lowest observed about 16.2 GiB); about 16.7 GiB remained after cleanup.

The initial binding-test command used `--offline` to update only the new direct
dependency edges in Cargo.lock. No unrelated package versions changed. The native
artifact builds subsequently used `--locked` successfully. `git diff --check` passed.

## Shared Host Schema Slice

`termirust-protocol/src/replicated_host.rs` reuses the existing typed desktop profile
decoder for an inert `ReplicatedHostReview`. It accepts only `desktop-profiles`,
envelope schema 1, matching envelope/profile stable IDs, and the existing desktop
domain-separated SHA-256 record key. Required text fields are bounded; control and
bidi-override/isolate characters are rejected. Invalid ports, malformed addresses,
duplicate required fields and unknown envelope fields fail. IPv6, default port 22
and ordinary Unicode labels remain supported. Unsupported authentication modes,
including desktop modes not represented by MobileAuthKind, fail closed.

The projection exposes only stable ID, label, host, port and username. It has no
conversion into an executable MobileHost. Credential references, key paths, routes,
startup actions, environment, tmux settings and all other profile fields are absent.
Original encrypted record bytes remain in the replication repository; this projection
does not sanitize or replace the canonical desktop record. It also does not establish
that ignored desktop-only settings are semantically valid or locally supported.

The binding adds `review_host_transfer` / `apply_host_transfer`. Both authenticate
the selected record and validate its host schema; apply checks the retained review
token again before delegating the local-only commit. The existing raw-record API
remains generic; the future host UI must use the host-specific API.

The Rust binding suite passed eleven integration cases after adding a valid encrypted
desktop-host round trip, inert field projection, malformed-host rejection before
commit, wrong-token rejection and duplicate no-op. These are in-memory custody tests.
Android and iOS artifacts have NOT been rebuilt for these two additional host-specific
methods. The preceding 26 Android executions cover the previous generic-transfer API,
not this extension. No production Android UI was wired in this slice.

`cargo test --locked -p termirust-protocol` passed nineteen tests (four new host-schema
cases and fifteen existing protocol cases), zero failures/skips. The binding run used
`cargo test --offline -p termirust-replication-bindings` to record only new dependency
edges; no unrelated versions changed. Formatting and `git diff --check` passed.
About 16.5 GiB remained free; no device, user data or build cache was removed.

## Android Bundle Review UI Slice

The production enrollment screen now opens ACTION_OPEN_DOCUMENT and passes the
selected content URI through the bounded C06 reader on Dispatchers.IO. It does not
persist a provider grant or cast a URI to a path. Leaving the screen cancels the
read signal and discards in-memory review; abandoned model generations cannot restore
an old review. The pending enrollment request is not deleted when review is dismissed.

The review dialog displays workspace, recipient and bundle code, then requires the
exact independently viewed desktop code before enabling acceptance. The repository
captures request/bundle bytes privately and delegates acceptance to packaged Rust.
It rejects pending cancellation, does not retry uncertain activation, and clears the
UI review on failure. A configured metadata state hides preparation/cancellation;
that state is not proof of currently accessible keys or successful host import.

The acceptance flow uses the already-packaged generic enrollment APIs. The newer
host-specific APIs still need artifact regeneration and UI wiring. Interrupted
activation requiring RecoveryRequired still needs its explicit recovery workflow;
repeated reload is not claimed to recover such a transaction.

Verification command:

```sh
ANDROID_HOME="$HOME/Library/Android/sdk" ./mobile/android/gradlew -p apps/android testDebugUnitTest assembleDebug assembleDebugAndroidTest --no-daemon --console=plain
```

PASS: both debug APKs built. JVM XML reports 85 cases, 81 passed, zero failures/errors,
four existing live-SSH skips. Nine enrollment model tests passed, including exact-code
gating, duplicate acceptance suppression, abandoned review, immutable captured bytes,
configured-state preparation suppression and no retry after uncertain acceptance.
These use a fake repository, not real bundle activation on Android.

Two new Compose instrumentation cases compile in the test APK: exact-code acceptance
gating and configured-state action removal. They have NOT run. The device runner now
expects six UI cases per viewport and reports actual accumulated counts (32 on its
owned emulator). Existing cancellation tests scroll to the action when necessary.

Free space dropped to about 15.4 GiB during Gradle. A stop was attempted against only
the owned wrapper/daemon, but both had already exited successfully; no process was
killed. No further emulator/native build was started, no data/cache was deleted, and
the 15 GiB floor was preserved at observed checks. Device/UI acceptance remains open;
do not apply the earlier 26-run evidence to these new screens.

## Verification Disk Guard

On the next continuation, only about 15.3 GiB remained free. No emulator or native
compiler was launched. `run_owned` now optionally checks available disk space before
process creation and every two seconds while waiting. Below the caller's threshold,
it raises a failure and uses its existing owned-process-group termination path.
Output, exit status and timeout behavior remain intact; callers without a threshold
keep their prior behavior. Cleanup commands are not gated on free space.

The Android native artifact builder and the Android runner's Gradle build use a
16 GiB threshold, leaving a buffer above the user's 15 GiB floor. This is a sampled
safeguard, not a guarantee against disk writes by other applications or detached
processes outside the owned group. Existing emulator startup still requires 17 GiB.

`python3 scripts/test/mobile-replication-artifacts.py` passed seven tests: existing
artifact rollback/isolation and timeout cases, plus prelaunch low-space rejection,
a simulated space drop stopping a parent/child group, and output/exit-code retention.
`bash scripts/build/mobile-replication-bindings.sh --android` then correctly failed
the real preflight before launching even its first rustc metadata command. This is
verified refusal, not a successful native rebuild. `git diff --check` passed.
No cache, source, application data or unrelated process was deleted or stopped.

Native regeneration and device verification remain pending until sufficient space is
available. The Android bundle UI, host-import UI and interrupted-activation recovery
are not promoted to a completed C07 acceptance gate by these runner tests.

## Recovery Review Follow-up

Source inspection on the following continuation found a prerequisite for exposing a
mobile recovery action. `recover_enrollment_activation` checks whether profile.json
exists, then removes the pending request and activation journal before `Service::open`
validates the profile. An existing malformed or newer-format profile can therefore
lose recovery evidence even though the subsequent open fails. This is source analysis,
not a reproduced runtime test or a shipped fix.

Before exposing recovery, add deterministic service regressions for malformed and
newer-format profiles alongside an activation journal and pending request. Require
failure without changing either evidence file or custody. Validate the committed
profile and its repository/authority/custody relationship before treating activation
as complete; presence alone is insufficient. Also cover invalid transaction reference
roles, failed epoch deletion, valid committed activation, and rollback before profile
publication. Keep cancellation and enrollment activation recovery distinct.

No recovery implementation was changed in this follow-up. About 15.2 GiB remained
free, below native build and emulator thresholds. The existing incremental Rust cache
directory was empty; no caches, test evidence or user data were removed to bypass the
space guard. Native builds/device tests remain unrun.

## Recovery Validation Implemented

After owner-authorized build-cache cleanup restored about 20.7 GiB free, the
recovery risk above was reproduced and fixed in the shared product service.
The first fixture run incorrectly assumed enrollment retained epoch 1; the fixture
was corrected to obtain the actual enrolled authority epoch. Against the unchanged
service, three regressions then failed on lost pending evidence or destructive
rollback, while valid committed activation passed.

`recover_enrollment_activation` now rejects non-epoch transaction references before
rollback. For a published profile, cleanup requires a canonical supported profile,
matching active member authority, primary repository without pending retirement,
matching current epoch reference, available device/epoch keys, matching device public
key, and matching pending request/custody when the pending request still exists.
Missing repositories are rejected without recreating them. Other outstanding
authority/deletion markers require recovery rather than automatic cleanup.

Seven new service regressions cover malformed/newer profiles, valid activation,
missing epoch custody, wrong-role and unrelated epoch references, invalid pending
data/missing repository, and failed epoch deletion followed by retry. Fixtures use
temporary directories and in-memory custody, not Android Keystore or real power-loss
injection. Secret bytes are compared before/after failure; they are not logged.

Verification:

- `cargo test --locked -p termirust-store --test replication_product`: 19 passed,
  zero failed/skipped.
- `cargo test --locked -p termirust-replication-bindings`: 11 integration tests passed,
  zero failed/skipped; zero unit/doc tests were present.
- `cargo clippy --locked -p termirust-store --test replication_product -- -D warnings`:
  passed. `git diff --check` also passed.
- All Cargo commands ran under `run_owned` with the 16 GiB disk threshold and a bounded
  timeout. Builds were serialized; available space remained above 20 GiB.

This change does not add the native recovery action, qualify every interruption
boundary, regenerate Android/iOS libraries, or prove host-import UI acceptance.
Those C07 requirements remain open. Existing activation error-unwind paths and
concurrent mutation coverage still require separate review before a complete
recovery claim.

## Android Artifact and Keyboard Runtime Follow-up

All four Android ABIs were regenerated and packaged with the shared recovery fix
and host-specific review/apply methods. The builder checked exported symbols,
architecture, 16 KiB segment alignment, and inventories before promotion. Kotlin
and JNI libraries match the verified output. iOS artifacts were not regenerated.

Commands completed:

- `bash scripts/build/mobile-replication-bindings.sh --android`
- `bash scripts/sync/mobile-replication-bindings.sh --android --write`
- `bash scripts/sync/mobile-replication-bindings.sh --android --check`
- `python3 scripts/test/android-replication-custody.py --avd Pixel_9`

The final runner result is 32 executions, zero failures/skips: eleven JNI/real
Keystore tests, six UI cases at each of phone/tablet/landscape sizes, and three
separate prepare/reopen/cleanup process runs. API 37, arm64-v8a, 16384-byte pages.
APK checksums matched all four packaged native libraries; runtime execution was
arm64 only. Added JNI calls reject invalid host IDs and token lengths while
preserving the pending request and existing identity.

The first run failed nine custody tests with Device locked after compilation.
The disposable-emulator branch now enables stay-awake and a bounded 30-minute
screen timeout before setting its fixture PIN. This was a runner correction, not
a relaxation of production Keystore protection. No personal device setting was
changed. Subsequent native custody runs passed.

Screenshot inspection caught a problem despite the first 32-test green run:
the landscape keyboard obscured the enrollment confirmation controls. The dialog
now uses IME padding, scrolls the focused input into view as keyboard insets change,
and places its heading with the scrollable details so the complete input fits.
The confirmation test now checks field/button visibility and uses touch injection,
not a semantics-only click. An intermediate build required the scoped experimental
Compose foundation opt-in; intermediate screenshots exposed field clipping that was
fixed before the final run. No intermediate failure is counted as passing evidence.

Final phone and landscape review screenshots were inspected; landscape shows the
full code field, cursor, and both actions above the keyboard. Tablet pending layout
was also inspected. These fixtures use MaterialTheme and synthetic review claims;
they are not a full production-theme or real desktop-bundle acceptance qualification.
Screenshots and the final count inventory are in `dist/mobile/c04-evidence`.

Final Android `testDebugUnitTest` also completed: 85 cases across 19 suites,
81 passed, four existing live-SSH skips, zero failures/errors. Artifact sync check
and `git diff --check` passed after the UI changes.

Each owned read-only emulator stopped on exit, including failed runs. Fixtures were
cleaned without clearing/uninstalling app data. About 20.0 GiB remained free after
verification. C07 still requires host selection/import UI, explicit activation
recovery controls, and a real desktop-to-Android successful enrollment/import run.

## Authenticated Host Discovery Source Slice

The Android picker needs host names, not user-entered internal record hashes.
The shared service now exposes `inspect_transfer_collection` and
`records_in_collection`. Both use the same fail-closed snapshot checks as reviewed
transfer: fresh profile/authority, active member, primary local repository, and no
outstanding recovery/retirement. Provider discovery decodes and checks the bounded
canonical document once, checks workspace, and authenticates the selected collection.
It neither merges nor commits records. The existing apply-token path is unchanged.

The mobile facade exposes `preview_host_transfers` and `imported_hosts`, returning
typed inert metadata through `MobileReplicatedHost`. No credentials, environment,
startup instructions or executable MobileHost conversions are exposed. Unsupported
host schemas fail the list; unrelated collections are not interpreted as hosts.
Authenticated tombstones are omitted without deleting any local data. Each selected
candidate still needs a fresh single-record review/token before import.

A new binding integration test proves authenticated discovery without mutation,
metadata projection, collection isolation, tampered-author/canonical/workspace and
empty-input rejection, missing-key failure without replacement, selected import and
facade recreation, unsupported provider/local schema rejection, and inert provider
deletion discovery. Fixtures use real Rust service/crypto and memory custody, not JNI
or Keystore.

`cargo test --locked -p termirust-replication-bindings -p termirust-store --test custody
--test replication_product` passed 12 binding and 19 service tests, zero failures or
skips. The source refactor reuses document/snapshot validation so collection discovery
does not reparse an entire provider file once per host. Native artifact regeneration,
host-selection/import UI, and device acceptance of these new list methods remain
pending; the preceding 32-execution device result predates this source change.
Focused Clippy for both test targets passed with warnings denied, and `git diff
--check` passed. The Android builder now requires the two new exported symbols on
its next regeneration; its existing published files were not modified in this slice.

## Android Host Selection and Review Runtime Slice

The four-ABI Android set was rebuilt again with `preview_host_transfers` and
`imported_hosts`. Artifact synchronization and the final `--check` passed. iOS
artifacts remain unchanged; Android runtime execution was arm64 only.

Configured enrollment now opens an Imported hosts screen. Its document picker uses
the bounded C06 reader on a worker, retains the selected encrypted bytes in memory,
and cancels abandoned reads. Authenticated discovery shows inert host metadata;
selecting a candidate obtains its single-record review. Only explicit confirmation
passes the retained bytes and token to the native apply method. The resulting list
comes from the encrypted local replica, not the executable SSH connection library.
No credential, route, startup command or connection is activated by this workflow.

The ViewModel blocks overlapping commands and duplicate confirmation, discards
cancelled/stale review state, distinguishes missing custody keys, and requires
explicit reload after an uncertain failure. Five new JVM cases cover captured-byte
ownership, selection/confirmation, abandoned preview, cancelled review and uncertain
apply without automatic retry. The complete JVM report contains 90 cases across 20
suites: 86 passed, four existing live-SSH skips, zero failures/errors.

`python3 scripts/test/android-replication-custody.py --avd Pixel_9` passed 38
executions: eleven real JNI/Keystore cases, eight UI cases at each of phone, tablet
and landscape sizes, and three separate prepare/reopen/cleanup process runs. API 37,
arm64-v8a, 16384-byte pages. New native discovery calls prove rejection before
enrollment; they do not prove successful imported-record discovery through Keystore.
New UI fixtures prove selection then explicit confirmation and missing-key gating;
their synthetic repository state is not a desktop acceptance fixture.

Phone and landscape host-review screenshots and the tablet missing-key screenshot
were visually inspected. Review content and actions fit without clipping in those
fixtures. Captures and execution inventory are in `dist/mobile/c04-evidence`.
The fixtures use MaterialTheme, not a complete production-theme accessibility audit.
The runner stopped its owned read-only emulator without clearing or uninstalling
app data. About 20 GiB remained free, above the user's 15 GiB floor.

Still required: successful desktop-produced enrollment and encrypted-host transfer
through Android provider/JNI/Keystore, explicit activation-recovery controls, and
native success-path restart/duplicate/wrong-recipient/changed-review/missing-key
coverage. The shared Rust tests and synthetic Compose tests do not substitute for
that acceptance run.

## Explicit Activation Recovery Slice

A deterministic failed-publication test reproduced a journal-loss bug: the old
activation error path attempted key deletion, ignored its failure, and still removed
the enrollment journal. The regression failed on the original path. Activation
errors after journaling now preserve custody and recovery evidence instead of trying
best-effort destructive cleanup. This also avoids treating a profile-publication
error after rename as proof that no profile was committed.

The shared service and mobile facade expose `recover_pending_enrollment`. It resolves
only an existing activation journal, rechecks it after acquiring the product lock,
and refuses concurrent authority/deletion recovery markers. A validated committed
member is finalized; an incomplete activation is rolled back while retaining its
pending request and device identity. It never accepts a bundle or prepares keys.
Callers reload status after success and must not automatically retry acceptance.

Focused Rust tests passed: 21 product-service and 13 binding tests, no skips/failures.
The new cases cover failed publication plus unavailable cleanup, explicit committed
recovery twice, unrelated transaction preservation before/after profile publication,
locked epoch deletion, retained pending request and exact epoch-only rollback through
the facade. Shared tests use in-memory custody; they are not Android Keystore crash
tests. Focused Clippy passed with warnings denied. An intermediate type-inference
compile error was corrected before the passing run.

Android source adds a localized recovery confirmation, a worker-thread repository
call, single-flight ViewModel action and no automatic retry. Reviewed cancellation
cannot be bypassed through recovery. Tests cover confirmation/dismissal, duplicate
taps, locked recovery and cancellation preservation. The runtime results below
supersede preceding counts.

The first artifact attempt stopped at the fourth ABI because a temporary test-only
dependency edit made `--locked` reject the manifest. That edit was removed; the
complete locked source test run passed again. No incomplete ABI set was promoted.

The subsequent build passed all four ABI checks and was synchronized into Android.
The final artifact `--check` passed. Android `testDebugUnitTest`,
`assembleDebugAndroidTest` and `lintDebug` completed successfully: 92 JVM cases across
20 suites, 88 passed, four existing live-SSH skips, no failures/errors. Lint reported
26 warnings, none located in the replication files, and no errors.

The owned Pixel_9 runner passed 41 executions: eleven JNI/Keystore cases, nine UI
cases at each of phone/tablet/landscape sizes, and three separate process lifecycle
cases. Real JNI recovery calls prove no-op recovery preserves a pending request and
malformed-journal rejection retains the journal and a separate identity. A real
repository check rejects recovery while reviewed cancellation is outstanding.
The confirmation UI case uses synthetic recovery status, not a crashed activation.
API 37, arm64-v8a, 16384-byte pages; other packaged ABIs were not runtime-tested.

Phone and landscape recovery-dialog screenshots were inspected; all dialog text
and actions fit. These are MaterialTheme fixture captures, not a production-theme
accessibility audit. The runner stopped its read-only emulator without clearing or
uninstalling app data. About 19.7 GiB remained free. No source or user data was
deleted for space. iOS artifacts were not regenerated.

The recovery action now exists, but successful desktop-to-Android acceptance/import
and real native activation interruption/restart coverage are still required. The
source regression does not establish every crash boundary, including a failure
between epoch-key storage and initial journal publication. Do not claim C07 complete.

## Desktop-Service to Android Acceptance Fixture

Run from the repository root:

```sh
python3 scripts/test/android-replication-custody.py --avd Pixel_9 --enrollment-only
```

The runner now exchanges a real Android-generated pending request with
`android_enrollment_fixture`, a disposable Rust example using the production desktop
`ReplicationProductService`. Desktop authority keys exist only in a fixture memory
store. It emits a signed enrollment bundle and a real encrypted `desktop-profiles`
record; no Android enrollment cryptography is reimplemented in Kotlin.

The Android stages use production `NativeEnrollmentRepository`, ViewModels and
Compose content, real packaged Rust and replication-only Keystore storage. The debug
DocumentsProvider exposes only the UUID-scoped bundle/replica fixtures through
`content://` URIs; `ReplicationDocumentReader` reads their bounded bytes. Provider
paths are never passed to Rust. The reviewed verification code comes from the
desktop-generated sidecar, independently of the code returned by mobile review.

Proven in the final run:

- Prepare a request, restart the process and recover that exact request.
- Reject another recipient's bundle and a wrong reviewed workspace without changing
  the pending request. Dismissing review also preserves it.
- Keep acceptance disabled for the wrong code; accept the correct desktop code
  through the visible confirmation; reject replayed acceptance.
- Reopen the enrolled product in another process with an empty imported-host list.
- Read/authenticate the encrypted host document without importing it. Require host
  selection and explicit import confirmation. Reject changed reviewed bytes first.
- Display the imported label, endpoint and port; duplicate reviewed import leaves
  the list unchanged. Reopen it after another explicit app-process stop.
- Delete only the fixture wrapping key, then fail to read imported records without
  provisioning a replacement key. Cleanup deletes only UUID-owned fixture state.

The first run enrolled successfully but failed on the malformed-document assertion:
domain JSON decode errors were mapped to Unavailable. The facade now maps malformed,
oversized, unsupported-schema and workspace-mismatch document errors to Invalid.
A shared binding regression verifies malformed apply rejection without importing a
host. All four Android ABIs were rebuilt and synchronized after this fix. No ABI
or Controller protocol changes were introduced; iOS artifacts were not regenerated.

Final native result: 12 executions, zero failures/skips, API 37, arm64-v8a,
16384-byte pages. The provider suite accounts for seven executions; prepare, accept,
import, reopen and cleanup account for five. The runner verifies APK native checksums
and stops its owned read-only emulator on success or failure. No user app data was
cleared or uninstalled. About 19.7 GiB remained free.

An intermediate successful run captured the acceptance screen before Compose showed
the configured state. The test now asserts the visible configured heading and waits
for Compose idle before capture. The entire fixture passed again. Final accepted and
imported screenshots were inspected and show the expected states without clipping;
evidence is in `docs/engineering-evidence/C07-android-enrollment-import/c07-evidence`.

Focused Rust custody tests passed all 13 cases. Android JVM tests passed 88 cases,
with four existing live-SSH skips, and lint passed. Fixture generation and the touched
binding tests pass focused Clippy with warnings denied. Artifact sync check passed.

Scope limits: this is a desktop service fixture, not desktop UI automation. Compose
uses the real product ViewModels with already-selected provider bytes, not a system
picker interaction or the entire production navigation/theme. The document provider
is disposable, not a third-party cloud provider. Process stops occur between completed
operations, not at activation crash boundaries. No SSH connection is enabled by this
inert import. Full mobile sync/convergence and C08 iOS acceptance are not established.

## System-Picker Follow-Up (Not Yet Verified)

Added `EnrollmentPickerTest` and the runner's `--enrollment-only --system-picker`
mode. The fixture uses the production screens' ActivityResult launchers and Android
DocumentsUI rather than injecting a selected URI. Internal repository parameters keep
the test's UUID-owned custody separate from production state. The debug-only provider
offers a browseable root containing only its enrollment bundle and encrypted host
document. Verification codes and request/PID sidecars are not browseable documents.

The intended checks cover picker cancellation, explicit enrollment confirmation,
reviewed host import, and the existing separate-process reopen check. Import assertions
wait for the loading control to become enabled after the review dialog closes.

The native run did **not** reach compilation or instrumentation: the disposable API 37
emulator booted, then the 16 GiB build guard stopped verification. No successful picker
result or screenshot is claimed. The runner now also checks disk headroom during boot.
Its owned emulator was stopped and the temporary
`TermiRust_C07_Picker_20260908` AVD deleted. The separately running Pixel_9 emulator
was left untouched. Cargo clean reported 2.2 GiB of generated output removed; the
803 MiB TermiRust Xcode DerivedData cache was also removed. Source, packaged bindings,
test evidence, credentials and user emulator data were preserved.

Rerun with sufficient disk headroom and an isolated available AVD:

```sh
python3 scripts/test/android-replication-custody.py --avd <isolated-avd> --enrollment-only --system-picker
```

### Follow-Up Compilation and Resource-Guard Checks

The production Android APK and instrumentation APK now compile successfully with the
picker additions (`assembleDebug assembleDebugAndroidTest`). The first standalone
invocation lacked `ANDROID_HOME`; rerunning with the installed SDK path succeeded.
`testDebugUnitTest lintDebug` also passed: 92 JVM cases, 88 passed, four existing
live-SSH skips, zero failures/errors; lint has zero errors and 26 warnings.

`python3 scripts/test/mobile-replication-artifacts.py` passed seven cases, and
`sh scripts/sync/mobile-replication-bindings.sh --android --check` confirmed the
packaged bindings match verified output. No Rust implementation or ABI was changed
in this follow-up.

Added `scripts/test/android-replication-runner.py`: three simulated checks pass for
low-space startup refusal, cleanup of only the owned emulator after space drops
during boot, and rejection of picker mode without enrollment mode. These tests never
build, launch an emulator, install an APK or touch a device. They are resource-guard
proof, not native enrollment proof.

Native picker execution remains pending. Approximately 16.7 GiB was available during
these checks, below the isolated-emulator startup requirement of 17 GiB. Earlier
successful C07 results above apply only to the previously executed provider/content
fixture. The epoch-key-storage-to-initial-journal-publication failure window and native
activation crash boundaries also remain open; compile success does not close them.

## System-Picker and Journaled-Activation Runtime Result

The following full run passed on API 37, arm64-v8a, 16384-byte pages:

```sh
CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 python3 scripts/test/android-replication-custody.py --avd Pixel_9 --enrollment-only --system-picker
```

All 14 executions passed with zero skips. Results and accepted/imported screenshots
are in `docs/engineering-evidence/C07-android-enrollment-import/c07-picker-evidence`. The screenshots show the configured heading
and imported fixture label/endpoint without content clipping. These use the test's
MaterialTheme, not a full production-theme/accessibility qualification.

The first picker runs exposed a test-selector limitation: the file labels were present,
but a five-parent accessibility walk did not reach their selectable container. The
selector now considers all exact visible matches, walks a bounded parent chain, and
can tap a file label's measured bounds when no click action is exposed. No URI/result
injection or fixed screen coordinates are used. Broad temporary tree diagnostics were
removed because picker roots can include unrelated account labels.

The production screens now have runtime evidence for cancellation of both system
pickers, independently supplied verification-code confirmation, explicit host review
and import, and process reopening. This closes the C07 fixture system-picker gap; it
does not prove every third-party provider or physical-device configuration.

`EnrollmentActivationTest` adds an actual failed-publication path through packaged
Rust and real Keystore. A fixture-only secure-store delegate obstructs profile
publication after epoch-key storage; Rust publishes its journal and fails activation.
The runner then force-stops the app before the recovery stage. Assertions verify:

- The pending request, journal and stored epoch key survive process restart.
- An unsafe profile path prevents recovery without discarding that evidence.
- A denied epoch-key deletion also preserves the journal, request and epoch key.
- Successful explicit recovery removes the incomplete repository, journal and exact
  abandoned epoch key, preserves the original request, and is idempotent.
- The unchanged request subsequently enrolls and imports successfully through the
  real picker. The final separate-process check rejects loss of the wrapping key.

This is recovery after a real publication error followed by a process stop, not an
injected kill between arbitrary filesystem instructions. All failure injection and
sidecars are confined to UUID-owned fixtures. Cleanup passed and stopped only the
owned read-only emulator; user AVD data and app data were not cleared. About 17.7 GiB
remained free after the run. No Rust ABI changes or packaged-library replacements were
needed for these tests.

Final follow-up checks passed: Android `lintDebug`, three runner resource-guard tests,
Python syntax checks, Android packaged-artifact synchronization, and `git diff --check`.
The owned emulator is no longer running. No commit or push was performed.

## Committed Enrollment Cleanup Across Restart

The system-picker runner now passes 16 executions, zero skips, on API 37/arm64-v8a
with 16384-byte pages. It adds `prepareCommittedRecovery` and `finishCommittedRecovery`
between successful enrollment and reviewed host import. The runner force-stops the
app between these stages and the following import stage.

The preparation stage keeps the actual committed profile/repository/custody, restores
the original fixture pending request, and reconstructs the cleanup journal from the
committed repository's epoch reference. This is a fixture-only reconstruction of the
post-profile state, not a process kill during profile publication. Only opaque references,
metadata and ciphertext digests are persisted in the fixture; no plaintext keys are
saved or logged.

After restart, denied secure-store reads prevent cleanup and preserve the journal and
pending request. Restored read access allows explicit recovery and repeat recovery to
succeed. Assertions require zero secure-store creation/deletion calls, unchanged
profile/repository bytes, unchanged ciphertext digests, and removal of only the pending
request and enrollment journal. Subsequent system-picker host import, reopening and
missing-wrapping-key refusal all pass. Results are in `docs/engineering-evidence/C07-android-enrollment-import/c07-picker-evidence`.

An earlier attempt failed during initial request preparation, before the new recovery
stages; fixture cleanup succeeded. A fresh guarded run passed all 16 stages. This does
not establish the cause of the earlier preparation failure or eliminate cold-start
flakiness. An intervening startup attempt was refused by the disk guard. No unrelated
Xcode process was stopped and no user data was cleared.

Final lint, packaged Android artifact synchronization, three runner guard tests,
Python syntax and whitespace checks passed. Approximately 18.1 GiB remained free.
The test-owned emulator was stopped; a separately started emulator was left untouched.

## Write-Ahead Custody Intent Safety

Enrollment previously stored the epoch key before publishing its journal reference.
The shared vault now allocates an opaque epoch reference without creating custody;
the product service durably writes a prepared intent before creating the key at that
reference. Only successful create is followed by a confirmed journal and activation.
Key copies and encoded envelopes use zeroizing buffers. The create-at-reference API
checks role/epoch and retains the backend's create-only collision behavior.

A prepared intent with no stored key can be removed while preserving a valid pending
request. Malformed pending data and inaccessible storage prevent cleanup. A present
key is ambiguous: creation may have committed before reporting failure or the reference
may have collided. Recovery returns RecoveryRequired without adopting or deleting it.
This closes the untracked-reference window but is deliberately not an automatic
solution for ambiguous creation. Confirmed journals omit the new flag and retain the
legacy canonical encoding; existing journal recovery regression tests pass. Older
readers reject the new prepared state rather than misinterpreting it as confirmed.

All four Android ABI libraries were rebuilt, verified and synchronized. The real
system-picker runner passed 18 executions with zero skips on API 37/arm64-v8a and
16384-byte pages. Added stages check that the intent exists inside the secure-store
create callback, failure before a write leaves the request recoverable, and failure
after a real Keystore write preserves ciphertext and journal across a process stop.
The fixture then deletes only its own injected ambiguous key to allow later tests to
proceed. **That deletion is fixture cleanup, not an implemented user recovery flow.**
The remaining rollback, committed-finalization, picker enrollment/import, reopen and
lost-key checks all pass. Updated evidence is in `docs/engineering-evidence/C07-android-enrollment-import/c07-picker-evidence`.

The initial source test run caught canonical legacy-journal incompatibility; omitting
the confirmed flag fixed it. Eight custody-contract, 22 product-service and 13 binding
tests pass in the focused suites. iOS artifacts were not rebuilt; iOS does not yet
carry this safety change. No public UniFFI method or Controller protocol changed.

Final checks: all 43 focused Rust tests executed again and passed; focused library
Clippy passed with warnings denied. Android `testDebugUnitTest lintDebug` succeeded;
Gradle reused the unchanged JVM test result (88 passed, four existing live-SSH skips),
while lint analyzed the updated instrumentation. Artifact sync, Python syntax, three
runner guards and whitespace checks passed. All owned build/test processes finished;
about 18.1 GiB remained free. No commit or push was performed.

## Remaining Product Gate

C07 remains open for a reviewed, ownership-safe resolution of ambiguous prepared
custody. Intent tracking, system-picker acceptance, confirmed-activation rollback and
reconstructed post-profile finalization now have native evidence. Exact-instruction
interruption and cold-start reliability still need qualification; this is not complete
mobile sync. Never resolve ambiguity by blindly deleting or adopting the referenced key.
The owner subsequently authorized clean commits without co-author trailers. Push,
publication and deployment are not part of this verification step.

## Unused Intent Identity Validation

The next recovery review found that a missing epoch key alone was insufficient to
clear a prepared journal: the surviving pending identity could itself be unusable.
Recovery now validates the pending format version, typed request and device reference,
then loads the original device key and compares its public key before removing an
unused intent. Missing, locked, corrupt or mismatched identity custody preserves both
the journal and pending request. A present unconfirmed epoch remains untouched.

Verification on 2026-09-08:

- Focused Rust suites: 44 passed (23 product, 8 custody, 13 binding tests). The new
  product regression covers missing, locked, corrupt, different-key and unsupported
  pending-version states, asserting unchanged evidence and no custody mutations.
- Focused Rust library Clippy with `-D warnings`: passed.
- Android `lintDebug`: passed, including analysis of the modified instrumentation
  source. No JVM unit-test rerun is claimed for this scoped change.
- All four Android ABI libraries rebuilt, verified and synchronized. The generated
  FFI contract is unchanged; iOS artifacts were not rebuilt for this change.
- Real packaged JNI on an owned read-only Pixel_9, API 37, arm64-v8a, 16 KiB pages:
  18 executions passed, zero skipped. The existing prepared-creation stage now also
  injects locked, missing and invalid identity reads, checks unchanged journal,
  request and ciphertext digests, then verifies recovery with restored access.
- System picker acceptance, import and process reopening passed again. Evidence in
  `dist/mobile/c07-picker-evidence` was refreshed. Fixture cleanup passed and the
  owned emulator stopped without app-data clearing or uninstalling.
- Final free space was approximately 18.6 GiB. Builds and native tests were serial
  and guarded above the user's 15 GiB floor. `git diff --check` passed.

These checks validate safe handling of an unused intent, not ownership-safe recovery
of a present ambiguous key. The remaining product gate above is unchanged.

## Ownership Receipt Prerequisite

Shared Rust custody now has an opt-in versioned epoch record with an independent
per-attempt ownership receipt. It preserves the v1 record/reference contract, rejects
adoption of an untagged legacy key, and can validate its own tagged record after a
create commits but reports failure. This is only a read/create primitive: no product
recovery path has been enabled and no existing ambiguous journal is migrated.

The format, trust boundary and ordered integration gates are recorded in
[`replication-custody-ownership.md`](../mobile/replication-custody-ownership.md).
The native callback and Android/iOS stores still intentionally reject the new record
size until their support and real platform verification are implemented together.
Enrollment continues to emit v1 records. Mobile artifacts are unchanged in this step.

Verification on 2026-09-08: the complete replication-security suite passed 25 tests,
including 11 custody tests; the product and native-facade Rust contracts passed another
36 tests (23 product, 13 binding). Total: 61 passed, zero failed or skipped. New checks
cover receipt mismatch, legacy rejection, collision preservation, exact recovered-key
bytes, malformed sizes/versions/roles, locked/missing/invalid storage and uncertain
write readback. No Android or iOS device rerun is claimed for this shared-only step.
Security all-targets Clippy and store/binding library Clippy passed with `-D warnings`;
`git diff --check` passed. Final free space was approximately 19.6 GiB.

Next: extend and qualify bounded native custody record support before integrating
receipts with request-bound enrollment journals and explicit recovery. Legacy ambiguous
prepared journals remain an open product gate; a new receipt cannot retroactively
prove their ownership.
