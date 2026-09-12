# C06 Mobile Provider Transport

Status: bounded read-only contract and native adapter fixtures verified on 2026-09-08,
including iOS local Files picker access. Cloud-provider qualification remains open;
this completes the C06 explicit-transfer foundation, not mobile encrypted sync.

## Decision

Use explicit bounded document transfer, not automatic shared-file publication.
The existing desktop filesystem lock/revision protocol is unchanged. Mobile provider
publication is unsupported because neither SAF nor file coordination establishes
cross-device CAS. See [the frozen initial contract](../mobile/replication-provider-contract.md).

Android `ReplicationDocumentReader` reads through ContentResolver; Swift's actor
reads within NSFileCoordinator and balances security-scope access. Both return only
untrusted bytes, reject empty/oversized documents, and offer no publication path.
No provider URL is cast to a Rust path. There is no incoming UI, native crypto,
provider polling, credential migration, or automatic mutation of existing replicas.

## Fixtures and Commands

```sh
python3 scripts/test/android-replication-custody.py --avd Pixel_9 --provider-only
python3 scripts/test/ios-replication-custody.py --simulator 7F76A1D5-5CC3-44DD-8883-DA554B851C99 --provider-only
```

The Android runner keeps its existing packaged-library checksum checks and owned
read-only emulator cleanup. Provider-only mode adds no fixture PIN and does not run
custody mutations. Its debug-only DocumentsProvider has no advertised roots, requires
the system MANAGE_DOCUMENTS permission, and serves only synthetic fixture bytes.
Every temporary file is unlinked after obtaining the read descriptor. Release builds
exclude the fixture provider and manifest entry.

The iOS runner builds the production device target, then executes seven XCTest cases
using only UUID-named temporary directories. It shuts down the selected simulator
only if it booted it; it never erases existing simulator data.

The first Android run built both APKs but crashed before tests: DocumentsProvider
requires `exported=true` and MANAGE_DOCUMENTS. The debug manifest was corrected;
that failed run is not passing evidence. The initial iOS run passed five tests with
zero skips, before extending the tests to the full 8 MiB limit and cancellation.

Initial verified Android run: PASS, five instrumentation tests and zero skips on owned Pixel 9,
API 37, arm64-v8a, 16 KiB pages. Both APK builds and native artifact checks passed.
Coverage includes exact and overflowing 192 KiB / 8 MiB reads, empty/missing data,
provider-injected denial, pre-cancelled reads, filesystem URI rejection, reader reopen,
and publication rejection. Only the owned read-only emulator was stopped afterward.

Initial verified iOS runner: PASS, production generic iOS device build with signing disabled,
then five XCTest cases and zero skips on iPhone 17 Pro / iOS 26.5. Tests include both
byte bounds, empty/missing/directory/symlink rejection, non-file URLs, pre-cancelled
work, repeated coordinated reads, and unsupported publication. UUID fixture files
were removed and the existing simulator returned to shutdown without erasing data.

Additional regression command:

```sh
ANDROID_HOME="$HOME/Library/Android/sdk" ./mobile/android/gradlew -p apps/android testDebugUnitTest lintDebug processReleaseMainManifest --no-daemon --console=plain
```

PASS: lint and manifest generation; JVM XML reports 81 cases, 77 passed, zero
failures/errors and four existing external live-SSH skips (one DirectSshIntegrationTest,
three AndroidSSHControllerTransportLiveTest). These skips are not provider proof.
The initial invocation without ANDROID_HOME failed SDK discovery; the explicit path
above succeeded. Structured inspection of the generated release manifest confirmed
the fixture provider is absent. Python runner syntax and `git diff --check` passed.
Final device listings showed no booted simulator or Android emulator. About 20 GiB
remained free; no cache/source cleanup was needed during this C06 slice.

## Follow-up Access Fixtures

The Android suite now includes a reliable pipe that sends bytes and closes with an
error, and a separate-UID permission probe in the instrumentation APK. The first
seven-test run passed the partial-transfer rejection and the original five tests,
but the probe failed to bind within ten seconds. That run is not a passing access
qualification. The probe was rewritten using Android/Java APIs only so it does not
depend on the target app's Kotlin classloader; its manifest also explicitly declares
target-package visibility. The exact original startup cause was not captured.

The probe only accepts the instrumented app UID and the synthetic provider authority.
It tests fresh opens before a grant, after an exact temporary read grant, and after
exact revocation. It does not exercise persisted picker grants or invalidate existing
descriptors. A new iOS test removes read permission from a UUID-owned local file,
then restores permission and verifies unchanged bytes. This is local permission
recovery, not a document-picker security-scope or cloud-provider test.

Android follow-up PASS: seven tests, zero skips on the same API 37 arm64/16 KiB
owned emulator configuration. The external probe asserts its UID differs from the
instrumented app and checks denied -> exact-byte read -> denied around a temporary
URI grant and revocation. Partial reliable-pipe output is discarded on provider error.
Fixture grants are revoked in finally cleanup, the service unbound, the reply thread
stopped, and the owned emulator removed. Both APK builds/native checksum checks passed.

The first iOS permission-recovery run built successfully but failed one of six tests:
the inaccessible file produced `unavailable` instead of `denied`. The adapter's error
classification was extended to recognize POSIX EACCES/EPERM, including Foundation
underlying-error wrappers, with an eight-level traversal bound. A separate structured
mapping test checks that missing-file and unknown-domain codes are not mistaken for
permission denial. Provider messages and file paths are never used for classification.

iOS follow-up PASS: seven XCTest cases, zero skips, and the production iOS device
build on the same iPhone 17 Pro / iOS 26.5 simulator configuration. The original
permission-recovery assertion passed without relaxing its expected error; restoring
file permissions recovered identical bytes. This includes one synthetic error-mapping
case, distinct from the actual filesystem denial test. The owned fixture directory
was removed and the simulator returned to shutdown.

Follow-up Android lint and release manifest generation passed. Structured XML
inspection confirmed both ReplicationTransferTestProvider and the instrumentation-only
TransferPermissionProbeService are absent from the generated release manifests.
No JVM suite rerun is claimed for this follow-up; the earlier 77-pass/four-live-SSH-skip
result remains the last JVM result. `git diff --check` passed. About 20 GiB stayed free;
no source, credential, user app data or build cache was deleted in this follow-up.

## iOS Local Picker Integration

`apps/ios/ProviderAccessFixture` is a separate test-only XcodeGen project. A donor
app exposes synthetic JSON through Apple's local Files provider; a reader app uses
the real open-in-place picker and the production reader source, without any custody
or enrollment facade. The UI test requires a positive security-scope probe and a URL
outside the reader container, releases the probe, reads via the production actor,
then cancels a subsequent picker and reselects after an app restart.

Run with `python3 scripts/test/ios-enrollment.py --provider-picker`. It uses an owned
disposable simulator and retains counts/screenshots under ignored dist/mobile/c06-picker.
The first run failed before selection because the built donor plist omitted
UIFileSharingEnabled despite the generated build setting. An explicit fixture plist
now supplies both sharing/open-in-place booleans; the runner checks their built values
before installation. That failed run is not successful picker evidence.

A second run reached the donor file but failed to select it: tapping the center of
the accessibility cell hit its metadata area. Screen recording and the accessibility
hierarchy confirmed the picker stayed open. The test now taps the visible thumbnail
within that cell; the positive scope, external-container and exact-byte assertions
were not relaxed.

Final PASS: one UI workflow test, zero failures/skips, on an owned iPhone 17 Pro /
iOS 26.5 simulator. It selected the donor's 17-byte synthetic JSON via the real Files
picker, required successful security-scope access outside the reader's canonical
container, released the probe, and verified identical bytes through the production
reader. Cancellation and app restart/reselection passed in the same test. The
success screenshot visibly reads `Verified external scoped read`.

Artifacts: `dist/mobile/c06-picker/iPhone-17-Pro/137FC271-71D9-4E8A-8FF2-D12D96F03F34/`
contains the result JSON, attachment manifest and picker/success screenshots. Both
fixture apps are confined to the separate test project; production has no dependency
on them. The runner removed only its owned simulator.

The shared runner's original path was then rerun with
`python3 scripts/test/ios-enrollment.py`: PASS, ten tests and zero failures/skips.
Results: `dist/mobile/c05-evidence/iPhone-17-Pro/C7909712-359F-4754-BD6B-16FDC130BA7E/results.json`.
This is phone enrollment regression evidence, not a fresh iPad qualification.

## Remaining Qualification

Android now has real temporary URI grant/revocation proof in addition to injected
provider denial; this does not prove a human revoking a persisted picker grant.
iOS now has real local Files picker security-scope proof, but not a third-party
File Provider extension, cloud hydration, bookmark persistence or physical-device
grant revocation. Neither suite proves provider-wide snapshots or distributed
atomicity; no adapter claims these capabilities.

Third-party provider picker and persisted-grant/hydration qualification remain open.
C07/C08 must integrate Rust authenticated parsing, exact-byte review and service
acceptance before a user can import records. C09 owns bidirectional convergence.
The C05 native iPad exporter keyboard limitation remains separately documented.

The earlier Controller artifact lockfile-checksum baseline remains in C01/C03; no
whole-product green status is inferred here. No commit, push or deployment performed.
