# C06 Picker Access Fixture

Test-only XcodeGen project; not a dependency of TermiRustMobile or its release scheme.

- C06Documents exposes only a synthetic JSON document through Apple's local Files
  provider using UIFileSharingEnabled and LSSupportsOpeningDocumentsInPlace.
- ProviderReader opens Apple's picker in open-in-place mode. It checks that the URL
  is external to its own container and has an active security scope, releases that
  probe scope, then calls the production ReplicationDocumentReader source directly.
- ProviderPickerTests selects the external document, asserts exact-byte success,
  restarts the reader app and reselects. No bookmarks, user credentials, Keychain
  fixtures, enrollment acceptance or sync changes are involved.

Run from the repository root:

```sh
python3 scripts/test-ios-enrollment.py --provider-picker
```

The runner creates/deletes only its disposable simulator, installs both fixture apps
there, checks the exact XCTest count and saves screenshots/results under ignored
dist/mobile/c06-picker. It requires 18 GiB free to start and interrupts owned work
below a 16 GiB safety margin. Do not run these apps on a personal device for evidence.

This is local file-provider integration, not third-party cloud hydration, persisted
bookmark access, grant revocation on a physical device or encrypted sync proof.
