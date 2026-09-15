# C04 Android Enrollment Request UI

Date: 2026-09-08
Status: C04 implementation and automated exit checks complete, with qualification
limits below. C05 and C06 are not active.

## Scope

Devices header menu > Enrollment. Native Compose uses the C03 product facade for
prepare/reload/request-bound cancellation with real Keystore storage. No replication
identity is created at app startup. This is request preparation only, not enrollment
acceptance or synchronization. The service's exchange directory remains a real
private local directory; no provider URI is converted into a Rust path.

Exit: real Compose workflow and storage tests, process-restart request reload,
confirmed cancellation and recovery, clipboard/file export, empty/busy/error states,
and visual inspection on phone/tablet/landscape. Native provider picker cancellation
and physical accessibility behavior must be reported separately if not exercised.

## Implementation

- `EnrollmentRepository` delegates to the packaged C03 Rust product facade on an
  IO dispatcher. An app-private stable file lock coordinates repository instances.
- Cancellation requires review of the exact request. A bounded, atomic public-request
  journal is saved before mutation; failed deletion survives repository recreation
  and can be retried explicitly. Storage failures do not create replacement identities.
- `EnrollmentViewModel` serializes actions, captures reviewed bytes before launching
  work, and exposes typed empty/loading/pending/error/recovery states. An uncertain
  prepare requires reload, not an automatic mutation retry.
- `EnrollmentScreen` uses native Compose, Material theme values, localized string
  resources, a scrollable constrained layout, a confirmation dialog, and an explicit
  device-credential action. Export blocks duplicate actions while choosing/writing.
- `EnrollmentTransfer` copies only the public canonical request and exports inert JSON
  through `ContentResolver`. A document URI is never passed to Rust as a filesystem path.
- Devices > menu > Enrollment opens the screen. Existing Controller pairing and
  terminal routes are preserved. No new permissions, provider transport, or wire format.

## Verification

`bash scripts/test/android-replication-custody.sh --avd Pixel_9`:

- 26 instrumentation test invocations passed, zero skipped, on API 37 arm64-v8a
  with 16 KiB pages. The APK's four replication ABI payloads were checksum-verified;
  only arm64-v8a was executed on this emulator.
- 11 real JNI/Keystore custody tests; four enrollment tests repeated at phone,
  tablet portrait, and wide landscape dimensions; three process-restart stages.
- Compose exercised the actual Devices navigation entry and the production enrollment
  content: prepare, pending reload, copy, export callback, keep request, and confirmed
  cancellation. Clipboard bytes and a `ContentResolver` file export matched exactly.
- A real request survived force-stop and reopened unchanged in a different app PID.
  Cancellation did not remove the other fixture identities. A simulated Keystore
  deletion denial retained the cancellation journal and a fresh repository retried it.
- Owned read-only emulator stopped; fixture directories and aliases removed. No
  personal device was locked, no application data cleared, and no app uninstalled.

`ANDROID_HOME="$HOME/Library/Android/sdk" ./mobile/android/gradlew -p apps/android testDebugUnitTest assembleDebug assembleDebugAndroidTest lintDebug --no-daemon`:

- Build succeeded. JVM XML reports: 81 tests, 77 passed, four skipped, zero failures
  or errors. All five new enrollment ViewModel tests passed, including duplicate-action
  suppression, uncertain preparation, captured cancellation bytes, and recovery errors.
- Skips are the existing one `DirectSshIntegrationTest` and three
  `AndroidSSHControllerTransportLiveTest` cases requiring external SSH fixture variables;
  they are not enrollment proof and are not counted as passed.
- Lint succeeds with warnings in existing surfaces. Gradle deprecation warnings remain.

The first Compose run failed inside Espresso's reflective InputManager lookup on
API 37, before application interaction. Test-only dependencies were updated to
runner 1.7.0, ext-junit 1.3.0 and Espresso 3.7.0; production Compose remains unchanged.
The replacement uses the supported service lookup described in the
[AndroidX release notes](https://developer.android.com/jetpack/androidx/releases/test#espresso-3.7.0).
The failed run is not counted as successful evidence.

## Visual Evidence and Limits

The runner produces `dist/mobile/c04-evidence/results.json` and nine screenshots:
empty/pending/locked at 411x923, 1067x1707 and 1067x600 dp. Phone pending/locked and
tablet/landscape pending captures were visually inspected: readable controls and
wrapped status/error text, no content overlap or clipping. These are Compose test
activity captures, not proof of production system-bar contrast or all device sizes.
Artifacts are generated and ignored by Git; the runner recreates them.

- Actual system document-picker selection/cancellation and third-party provider
  behavior are not qualified by the file-URI export test. C06 still owns provider semantics.
- Physical credential lock/unlock, TalkBack, external-keyboard focus, large-font modes,
  and OEM/API-26 runtime behavior remain manual/device qualification gaps.
- Strings are localization-ready English resources, not completed translations.
- No iOS enrollment UI, enrollment acceptance, or encrypted record sync is claimed.
- The canonical whole-product check's pre-existing Controller artifact lockfile-checksum
  mismatch remains recorded in C01/C03. This goal does not claim aggregate product green.
- Existing C01-C03 changes were preserved. No commit, push, publication, or deployment.
- Final unit/lint rerun passed after parameter-order cleanup; `git diff --check`
  passed. Free space remained above the 15 GiB floor (19-20 GiB observed).

Next queued outcome: C05, the corresponding native iOS enrollment-request workflow.
