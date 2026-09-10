# C05 iOS Enrollment Request UI

Status: request workflow implemented and simulator-verified on 2026-09-08.
Native-picker keyboard and physical accessibility qualification remain open below.
C06 is the next queued outcome, not active in this work.

Scope: Devices action menu opens native SwiftUI request preparation, pending reload,
copy/share/JSON export, and reviewed cancellation through the C03 Rust facade and
real Keychain. No startup replication identity creation, imported-data execution,
provider URI conversion, enrollment acceptance, or sync implementation.

Exit: production target build; real-Keychain request reopening and failed-cancellation
recovery tests; SwiftUI interaction evidence for preparation, app reopening, sharing,
export and cancellation; phone/tablet layout inspection. Report actual native picker,
keyboard and accessibility coverage separately from model proof.

## Architecture and Safety

- `TermiRustMobile/Enrollment` contains the repository actor, view model, and SwiftUI
  screen/document representation; XcodeGen adds these to the production app target.
- The repository uses the existing generated `MobileReplicationProduct` and
  `ReplicationKeychainStore`, with real local Application Support staging excluded
  from backup. No document-provider URL is converted to a Rust path.
- Actor-isolated operations execute synchronous native calls away from MainActor.
  A stable nonblocking OS lock also serializes separate repository instances.
- A bounded public-request journal is atomically saved and synchronized before
  reviewed cancellation. Failed Keychain deletion retains the journal; fresh repository
  instances can retry that exact request. Invalid or inaccessible data is never empty.
- Preparation is explicit. Reload is the only action on screen entry. Confirmations
  capture request bytes, and uncertain preparation errors require reload before retry.
- Copy is local-only with a two-minute pasteboard expiry. Share uses the native
  activity sheet. Export uses a bounded JSON `FileDocument` and native file exporter.
  Sharing or exporting never executes the request or claims enrollment acceptance.
  Native API references: [ShareLink](https://developer.apple.com/documentation/swiftui/sharelink)
  and [file exporter cancellation](https://developer.apple.com/documentation/swiftui/view/fileexporter(ispresented:document:contenttypes:defaultfilename:oncompletion:oncancellation:)-34bd6).
- Native semantic styles, scroll layout, SF Symbols, localization-catalog entries,
  accessibility labels, Command-R reload and Escape/Cancel dismissal are provided.
  No full keyboard/VoiceOver qualification is inferred from the presence of shortcuts.

## Test Work

`python3 scripts/test/ios-enrollment.py` creates and removes only an owned simulator,
uses the real app target and Keychain, and exports xcresult summaries/screenshots
under ignored `dist/mobile/c05-evidence`. It refuses startup below 18 GiB free.
While running, it monitors space every two seconds and cancels its owned process
group below a 16 GiB safety margin. It verifies both xcodebuild exit status and
exact test counts, and removes only its own disposable simulator in finally cleanup.

The first run found a real repeat-preparation failure: Foundation rejected an existing
`local-exchange` directory after cancellation. The repository now reuses a directory
only after checking that it is a directory and not a symlink. The regression test
prepares a second request after cancellation. That failed run is not passing evidence.
The first UI test also assumed a system share-sheet Close button; it was changed to
exercise the native Copy collection cell. The native exporter on this runtime exposes
an unrelated non-button element labelled Cancel. Rather than tapping that misleading
element, the test now waits for the filename field, dismisses the picker with its
native sheet gesture, and requires request controls to become enabled again. It also
reopens the picker, saves the JSON file locally, and requires export-success feedback.
The iPad floating tab bar exposes nested Devices matches; navigation uses the first
matching tab. Failed selector runs do not count as successful workflow evidence.

The iPhone 17 Pro / iOS 26.5 run at
`dist/mobile/c05-evidence/iPhone-17-Pro/654374DF-D885-434D-9E32-964748BE75EB`
passed all ten tests, zero skipped: six custody regressions, three new repository/model
tests, and one complete UI workflow. UI relaunch retained pending state; real repository
reopening asserted identical request bytes. Phone portrait, landscape and empty-state
screenshots were inspected for readable controls and clipping. The subsequent cosmetic
change clears stale export feedback when the request changes.

The subsequent iPad Pro 11-inch M5 / iOS 26.5 run at
`dist/mobile/c05-evidence/iPad-Pro-11-inch-M5-12GB/7963AC63-A8F9-435D-933E-959D5802746D`
passed ten tests with zero skipped, including the cleared feedback and enrollment
screen Escape shortcut. Tablet native-picker dismissal uses its visible close glyph
(a runtime-specific coordinate because the native accessibility button label was not
reliable), not the phone sheet gesture. A following test change reloads after rotating
before capturing landscape; earlier tablet landscape captures caught rotation in flight
and are not settled-landscape evidence.

Final iPhone rerun with reload after rotation:
`dist/mobile/c05-evidence/iPhone-17-Pro/049C9B9A-81AE-4C4F-8350-0E386D40A036`
passed ten tests, zero skipped. Landscape screenshot
`A9748FD5-FC37-437C-9104-8FAF1B3BA753.png` was inspected: all request controls fit
and remain readable. Empty-state screenshot `72513D3A-60D9-4700-86E0-9BB643092E42.png`
confirms the stale export notice is cleared after cancellation. Both native local JSON
save and cancelled export retained the correct request state; final Escape dismissed
the enrollment screen after returning from the picker.

Final iPad rerun with reload after rotation:
`dist/mobile/c05-evidence/iPad-Pro-11-inch-M5-12GB/A9147621-6146-4864-84CE-278AD59EAB95`
passed ten tests, zero skipped. The settled landscape screenshot
`1DD3010F-7373-414D-BD3D-385602D3C209.png` and portrait screenshot
`8B04766B-3D15-41F9-B651-637D8E5D9B8C.png` were visually inspected: the native
form sheet is centered and request controls fit without clipping or overlap.
These phone/tablet results are 20 test invocations of the same ten tests, not 20
distinct test cases. Both owned simulators were removed successfully.

Final regression command:

```sh
python3 scripts/test/ios-replication-custody.py --simulator 7F76A1D5-5CC3-44DD-8883-DA554B851C99
```

PASS: production generic iOS device build (`CODE_SIGNING_ALLOWED=NO`), then 21
tests with zero skips: six real-framework custody/enrollment and fifteen Controller
binding/lifecycle/route tests. The selected existing simulator was returned to shutdown;
its data was not erased. This is a build plus simulator proof, not physical deployment.
`git diff --check` and localization catalog JSON parsing passed. Existing NIO duplicate
Objective-C class warnings remain outside this request UI change.

During verification, free space was observed at about 14 GiB. With no cargo/rustc
process running, approximately 6 GiB of root target/debug/deps `.rlib` and `.rmeta`
rebuildable cache was removed, restoring about 20 GiB. No executable, source,
credential, or pre-existing simulator data was removed. The 15 GiB floor was therefore
not maintained throughout this work; the added monitor reduces recurrence risk but
cannot reserve disk space against other processes.

## Limits

Incoming enrollment-bundle import belongs to C08, not request preparation. C06 still
owns file-provider synchronization semantics. Physical Keychain lock/unlock,
third-party provider publication, physical keyboard/VoiceOver, and translated locales
must be qualified separately. The test-only deletion-denied adapter simulates a
Keychain error; it does not prove physical device locking.
Escape while the native iPad file exporter was open did not reliably dismiss only
the picker in exploratory runs. Disabling the parent Done action while exporting did
not establish a fix. The passing path uses touch dismissal; full native-picker keyboard
qualification remains open. Escape on the enrollment screen itself is tested separately.

Existing C01-C04 changes are preserved. No commit, push, or deployment is authorized
by the current brief. The pre-existing canonical Controller artifact lockfile-checksum
failure remains documented in C01/C03, not silently repaired in this UI goal.
