# C02 iOS Replication Custody

Status: complete. C01 is complete; C03 is queued, not active.

## Scope And Exit Gate

Package the existing generated Swift custody binding and Keychain adapter in the
real iOS application target. Reuse the static XCFramework packaging convention,
stage a complete device/Apple Silicon/Intel simulator artifact set, and verify
checksums and exported symbols. Preserve Android and Controller outputs.

Exit requires a production device build and simulator XCTest using the app's
shipped framework and real Keychain: identity creation, fresh-instance reopening,
exact/idempotent deletion preserving other identity and Controller data, duplicate
create, malformed data, invalid inputs, and structured errors. Record counts,
cleanup, and limitations. No mobile enrollment UI or product facade in C02.

No commits or pushes. Retain at least 15 GiB free; serialize native builds.

## Implementation

- `apps/ios/Replication` owns generated Swift, the static XCFramework, provenance,
  and a SHA-256 inventory as one staged artifact set.
- `project.yml` and its regenerated Xcode project include the existing
  `ReplicationKeychainStore` and generated binding in the production app target.
  XCTest imports that app with `@testable import`, not separate conformance sources.
- Rust 1.97.1 and UniFFI 0.32.0 are pinned. Targets: arm64 device, arm64 simulator,
  x86_64 simulator; iOS minimum 17.0. The two simulator archives are combined.
- Existing `create-ios-static-xcframework.sh` is reused. The C01 directory-promotion
  helper gained an optional validator, preserving its Android default and outputs.
- Apple `nm` rejected newer Rust LLVM bitcode during initial validation. The final
  builder uses Rust's matching `llvm-tools-preview` symbol reader. Replication
  dependency LTO is disabled for Apple machine-code archives; Rust standard archives
  may still contain LLVM bitcode. Actual Xcode linking is a separate required gate.
- Build failures did not publish partial artifacts. Temporary slices were removed
  before building the next architecture. No Controller libraries were regenerated.

## Commands And Observations

```sh
python3 scripts/build/ios-replication-artifacts.py build
python3 scripts/build/ios-replication-artifacts.py sync --write
python3 scripts/build/ios-replication-artifacts.py sync
python3 scripts/test/ios-replication-custody.py --simulator 7F76A1D5-5CC3-44DD-8883-DA554B851C99
python3 scripts/test/ios-replication-artifacts.py
python3 scripts/test/mobile-replication-artifacts.py
bash scripts/sync/mobile-replication-bindings.sh --android --check
cargo test --locked -p termirust-replication-bindings
cargo clippy --locked -p termirust-replication-bindings --all-targets -- -D warnings
git diff --check
```

| Gate | Observed result |
|---|---|
| Apple archive construction, architecture, exported custody symbols, checksums | Passed |
| Packaged Swift/framework matches built inventory | Passed |
| Production generic iOS device Debug build, signing disabled | Passed |
| Real-framework Keychain XCTest | 5 passed, zero skipped |
| Controller binding and route lifecycle regression | 15 passed; combined run 20 passed, zero failed/skipped |
| iOS artifact metadata/publication tests | 3 passed; mocked binary inspection, not runtime proof |
| Shared publication/child cleanup tests | 4 passed |
| Android packaged artifact check | Passed, unchanged |
| Rust custody contract | 5 passed |
| Rust custody Clippy | Passed |
| Explicit unavailable simulator | Failed before build/install as required |

Runtime: Xcode 26.6 (17F113), iOS 26.5 simulator, arm64, TermiRust iPhone 17 Pro.
The first test-build attempt had a String/Data mismatch in the new invalid-reference
fixture. It was corrected to the generated API's Data type; no assertions or native
requirements were weakened. That failed build is not counted as runtime proof.

## Proven Storage Behavior

- Real Rust creates two identities. New Keychain store and Rust custody instances
  load the same public keys from their opaque references.
- Exact deletion returns true then false, leaves the other identity usable, and
  preserves a real Controller Keychain sentinel in its separate fixture service.
- Two concurrent tasks with independent stores produce one create and one Collision;
  winning bytes remain unchanged.
- Empty, truncated, and oversized Keychain records fail Invalid, remain present,
  and cannot be replaced through create.
- Invalid account and secret sizes fail; malformed references fail through Rust.
  Exact account deletion cannot affect the same account under another service.
- Actual Keychain attributes are non-synchronizable and WhenUnlockedThisDeviceOnly.
  Locked/unavailable status mappings are simulated, not physical lock testing.

Fixtures use UUID-scoped test services and delete only those services with cleanup
status assertions. The runner shuts down a simulator only if it booted it, never
erases/uninstalls/clears the app, and bounds subprocess trees and result-bundle
lifetimes. No physical iPhone was used. Approximately 18 GiB free remained.

## Limits And Next Goal

This proves fresh-instance reopening, not runner-driven app-process restart or
physical iPhone background/lock/unlock/backup restoration. The Intel simulator slice
was built and inspected but not executed. Simulator Keychain attributes do not prove
all physical-device protection or backup semantics. No blanket FFI buffer-zeroization
claim is made. No new identity is created automatically at launch.

The pre-existing canonical Controller Cargo.lock checksum failure documented in
[C01](C01-android-replication-custody.md#canonical-baseline-failure) remains untouched.
The whole-product verifier was not rerun in C02 and is not claimed green.

C03 is queued: expose recoverable enrollment preparation, pending-request reload,
and exact cancellation through a mobile facade over `ReplicationProductService`.
Mobile enrollment/sync UI and transport remain open. C02 does not launch mobile sync.
