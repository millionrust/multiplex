# N01 Deterministic Cross-Repository Baseline

**Latest verification:** 2026-09-07
**Status:** Local baseline and individual live checks passed

## Monorepo Reverification

The original 2026-09-01 evidence below is historical. Swift and Kotlin now live in
`mobile/ios` and `mobile/android` within this repository. The current verification
started at `013c9a8` and resumed after an interruption.

Before the interruption, these commands passed:

- `cargo fmt --check` and `cargo check --workspace --all-targets`
- the exact stalled-handshake cancellation test, 50 consecutive runs
- `./scripts/auto-test.sh`: 665 desktop tests passed, 4 ignored, plus 9 integration
  tests; Clippy and diff hygiene completed
- `./scripts/verify-product-model.sh --local`: workspace tests/docs/policy,
  synchronized fixtures, route contracts, strict Swift 6 verification and generic
  device build, Android unit tests/debug APK, and diff hygiene all passed

The first local verifier attempt failed on an obsolete Android string-resource
assertion before Gradle ran. Commit `183a91f` checks the current Sessions resource
and gives every structural assertion a named failure message. The corrected full
local verifier passed. Runtime-only steps were explicitly skipped in local mode.

The interrupted `--live` invocation has no aggregate completion result. On
2026-09-07, its remaining scripts were invoked individually. Docker initially was
unavailable; Docker Desktop was started and the desktop fixture was rerun.

| Live script | Result on retry |
|---|---|
| `verify-desktop-host-golden-run.sh` | PASS: bundled desktop local/SSH restore, Host replay/control/revocation, owned-resource cleanup |
| `test-mobile-ios-direct-ssh.sh` | PASS: iOS simulator SSH/tmux reconnect |
| `test-mobile-android-direct-ssh.sh` | PASS after test correction described below |
| `test-mobile-android-controller-host.sh` | PASS: Android emulator pairing, terminal lifecycle, reconnect, revocation |
| `test-mobile-controller-ssh-transports.sh` | PASS: native Android and iOS SSH Controller transports |
| `test-mobile-controller-relay-transport.sh` | PASS: native iOS relay connection and reconnect |
| `test-mobile-android-relay-transport.sh` | PASS: Android emulator relay connection and reconnect |
| `verify-controller-lan.sh` | PASS |
| `test-controller-ssh.sh` | PASS |

The Android SSH smoke initially disconnected after observing a session-name
substring that could arrive before its marker write completed. Commit `01e7fb5`
requires an acknowledgement emitted only after the expected tmux session is
confirmed and the marker is written. Reconnect must preserve both the marker and
the shell environment. A concurrent Gradle invocation invalidated an intermediate
run's output directory; the isolated rerun passed. No timeout was increased.

Final cleanup found no running Docker containers or test-owned verifier, relay,
or Controller fixture processes. Temporary live SSH credential properties were
removed. The Gradle daemon started by the checks was stopped. Free disk space was
17 GiB; stale generated Xcode caches were cleared to maintain the requested
15 GiB floor. No application source or user data was removed for disk cleanup.

These results do not claim physical-device UX approval, Windows/Linux execution,
store publication, or completion of the entire engineering roadmap.

## Historical 2026-09-01 Evidence

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
