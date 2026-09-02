# N01 Deterministic Cross-Repository Baseline

**Evidence date:** 2026-09-01
**Status:** Complete

## Scope

N01 establishes one deterministic local baseline across the Rust, Swift, and
Kotlin repositories. It also makes live-test omissions explicit. This evidence
contains command outcomes and bounded environment metadata only; it contains no
credentials, keys, application state, terminal content, or private file content.

## Environment

- Host: macOS Darwin 25.5.0, arm64
- Rust: `rustc 1.97.1`, `cargo 1.97.1`
- Apple tools: Xcode 26.6, Swift 6.3.3
- Android JVM: OpenJDK 17.0.16
- Docker CLI: 29.5.3; Docker daemon unavailable
- Eligible iOS simulator/device destinations: none reported by `simctl`
- Free repository volume space after verification: 53 GiB

## Cancellation Result

SFTP transfers now share one atomic terminal-state arbiter. A cancellation that
commits while a transfer is open wins over a later transport error and emits one
`TransferCancelled` event. Completion, failure, conflict, skip, and cancellation
cannot independently emit competing terminal events. Cancellation after a
committed terminal event is a no-op.

Deterministic tests cover cancellation while queued (before connect), during a
barrier-controlled stalled SSH handshake, at the active cancellation/failure
commit boundary, and after completion. They also check duplicate terminal events,
manager worker shutdown, and closure of the fixture-owned socket.

The required stalled-handshake test passed 50 consecutive executions without
changing its two-second cancellation deadline.

The first full baseline run also exposed an unrelated load-sensitive discovery
test: a cache-cancellation fixture used the two-second production probe timeout.
That test now uses the existing ten-second test-only discovery limit; production
limits and timeout-specific coverage are unchanged. The corrected test passed 20
consecutive executions before the full suite was rerun.

## Command Results

All listed commands exited `0` on their final run:

| Command | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo check --workspace --all-targets` | PASS |
| stalled-handshake cancellation test, 50 consecutive exact runs | PASS, 50/50 |
| SFTP module tests | PASS, 23/23; Docker cases explicitly self-skipped |
| discovery cache-cancellation test, 20 consecutive exact runs | PASS, 20/20 |
| `./scripts/auto-test.sh` | PASS; tests and Clippy reached |
| `./scripts/verify-product-model.sh --local` | PASS |
| Swift `./scripts/verify-ios-unified-routes.sh` | PASS source/lifecycle type-check; runtime SKIPPED |
| Kotlin `./scripts/verify-android-unified-routes.sh` | PASS unit tests and debug APK |
| `git diff --check` in Rust, Swift, and Kotlin repositories | PASS |

The final Rust test phase reported 653 passed, 3 ignored, and 0 failed in the main
binary, plus 9 passed across four integration-test binaries. `auto-test.sh` then
completed Clippy and diff hygiene.

The local product verifier reported these executed steps as `PASS`:

- Rust workspace formatting, compile, Clippy, tests, docs, and policy
- both shared-fixture synchronization checks
- remote-route, mobile-route, and cross-route contracts
- strict Swift 6 source and lifecycle verification
- Android unit tests and debug APK verification
- diff hygiene in all three repositories

It reported these runtime-only steps as `SKIPPED`, not passed:

- iOS runtime execution: no eligible iOS destination is installed
- live Docker SSH fixtures: the Docker daemon is unavailable

`--live` was therefore not run. N01 does not claim live desktop SSH, mobile SSH,
Controller, simulator, emulator, or physical-device execution.

## Warning Inventory

Warnings remain visible and are not suppressed by N01:

- `cargo check`: desktop binary 50 warnings; test binary 32 warnings, 31 duplicates
- Clippy: desktop binary 93 warnings; test binary 78 warnings, 74 duplicates
- categories: Objective-C macro `unexpected_cfgs`, existing dead code, collapsible
  control flow, argument-count/large-enum style lints, and test-only
  `field_reassign_with_default`
- future-incompatibility notices: `block 0.1.6` and `proc-macro-error2 2.0.1`

No warning introduced on the changed Rust lines was reported by
`scripts/clippy-changed.py`.

## Repository And Cleanup State

- Rust: branch `test`, base `f420291`; only N01 source, verifier, testing docs,
  evidence, and the narrow discovery-test correction are modified/untracked
- Swift: branch `main`, `f15f13a`; the sole pre-existing
  `TermiRustMobile.xcodeproj/project.pbxproj` modification remains
- Swift project-file SHA-256 before and after verification:
  `9cbdb068887ddab65f15080e1932f36658c369f8d7c344371d2f369abb5ea25e`
- Kotlin: branch `main`, `a50b726`; clean
- no test-owned Cargo, Clippy, Xcode, SFTP worker, verifier, or Gradle daemon remains
- no mobile live-SSH properties/credential file remains
- no container audit was possible while the Docker daemon was unavailable; no
  live Docker fixture was started

The repositories were not committed or pushed during N01 verification.
