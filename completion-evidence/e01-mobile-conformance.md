# E01 mobile conformance evidence

- Completed: 2026-08-29
- Owner: Codex
- Canonical repository: `/Users/jacob/Projects/terminal`
- Rust branch: `test`
- Fixture schema: `universal-session-v1`
- Fixture SHA-256: `e7e2fa8b84c91be9cbd353ccd7be3644bbb964da5e634c8007d2bfee98180a7e`

## Result

The Rust Controller protocol and both native mobile clients now expose the same
authoritative Session identity, origin, runtime, and closed capability set. A single
checked-in fixture drives deterministic Rust, Swift, and Kotlin scenarios for writer
exclusion, lifecycle loss of writer authority, acknowledged-input reconciliation,
resize bounds, and post-authority mutation denial.

This evidence does not claim that Swift or Kotlin has completed a live transport run
against the same Rust Host process. That remains the next finite E01 child.

## Commits

- Rust:
  - `4385fc5` `feat(controller): prove universal session conformance`
  - `0c3daf9` `test(controller): lock universal command identities`
- Swift:
  - `27ee0f3` `feat(controller): add universal session conformance`
  - `4cf3328` `test(controller): lock universal command identities`
- Kotlin:
  - `29223c9` `feat(controller): add universal session conformance`
  - `648522d` `test(controller): lock universal command identities`

No commit has a `Co-authored-by` trailer. The Rust commits were pushed to
`origin/test`; the native repositories have no configured remotes.

## Delivered

- Added additive, backward-compatible Rust Controller Session fields for exact Host
  instance identity, terminal/managed/observed origin, runtime, and ordered closed
  capabilities.
- Derived mobile-visible capability truth from the authenticated peer grant and the
  authoritative Host runtime occupant instead of titles or client guesses.
- Updated Swift and Kotlin strict wire models, validation, view models, and fleet rows.
  Writer and resize UI now intersects device authorization with per-Session capability
  truth while preserving legacy decoding compatibility.
- Added one canonical JSON fixture and exact copies in both native test bundles.
- Added a synchronization script that fails if any fixture copy diverges.
- Assigned distinct stable command IDs for writer race, input, release, post-release
  acquisition, denied input, reconnect acquisition, resize, and stop.
- Added a real Rust Session Host scenario proving one writer, idempotent input, one
  output application, explicit release, stale-writer denial, acknowledged-watermark
  reconnect without duplicate output, resize, and graceful stop.
- Bound the existing authenticated revocation test to the same fixture's controller
  generation and revocation data.

## Verification

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `./scripts/verify-rust.sh workspace` | PASS, including 474 desktop tests, all workspace targets, docs, advisories, bans, licenses, and sources |
| `cargo test -p termirust-client --test universal_session_golden --locked` after deterministic-ID refinement | PASS, 1 test |
| `./scripts/sync-universal-session-fixture.sh --check` | PASS |
| Swift `./scripts/verify-ios-controller.sh --stage universal-session` | PASS strict source/test type-check |
| Swift focused runtime XCTest | NOT RUN; no iOS simulator runtime is installed |
| Kotlin `./scripts/verify-android-controller.sh --stage universal-session` | PASS, focused Controller JVM test and no-legacy-SSH check |
| `git diff --check` in all repositories | PASS |

The canonical Rust gate emits existing Objective-C macro and legacy dead-code warnings.
No new warning suppression was added; changed Rust lines and clean architecture crates
passed their strict checks.

## Failure and recovery coverage

- Legacy Session summaries decode to unknown origin and an empty capability set.
- Unknown protocol variants and unknown fields remain closed at strict wire boundaries.
- Runtime strings and capability counts are bounded; duplicates and unstable ordering
  are rejected.
- Two Controllers cannot hold the writer lease concurrently.
- Background/foreground loss clears queued input and invalidates local writer state.
- Reconnect starts with no reconstructed acknowledged input.
- Old or revoked authority cannot enqueue another mutation.
- Rust command retry is idempotent and produces terminal output exactly once.
- Reconnect from the acknowledged output sequence yields no duplicate frame.

## Remaining E01 work

Build one disposable live Rust Controller listener/Host fixture and execute the native
Swift and Kotlin pair/list/attach/acquire/input/resize/background/reconnect/revoke flows
against it. The fixture must remain local-only, synthetic, bounded, credential-free, and
must leave all three repositories clean. That work depends on the unresolved D01/D06
native platform and route decisions and is not approved by this conformance slice.

An official arm64 iOS 26.5 simulator download was attempted under a 17 GiB automatic
free-space stop guard after removing only disposable Rust build output. The package is
8.52 GB and made bursty progress, but it was cancelled before installation because it
cannot prove the separate live Rust-Host transport gate and D01/D06 remain unresolved.
No runtime was installed; free space returned to 26 GiB.
