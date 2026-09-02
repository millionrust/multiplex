# N04 Real Apple Controller Golden Run

**Evidence date:** 2026-09-02
**Status:** Complete

## Scope

N04 requires the native Swift application to control a real Rust Session Host over
a private network on representative iPhone and iPad simulator classes and on at
least one physical Apple device before hardware behavior is claimed.

The automated simulator and physical-device portions are complete. The paired
iPhone 13 named `Jake` has Developer Mode enabled, CoreDevice reports it as
connected, and the signed native test bundle completed the full live Controller
workflow against the real Rust Host. Physical rotation, initial touch/keyboard
ergonomics, background recovery, app-icon rendering, and basic VoiceOver
navigation passed manual review. The manual live terminal walkthrough also passed,
including pairing, terminal discovery, read-only attach, writer control, terminal
input/output, selection and copy, background recovery, landscape keyboard use,
revocation, and the post-revocation failure state.

## Environment

- Host: macOS 26.5.2, Apple silicon
- Xcode: 26.6
- Runtime: iOS 26.5
- iPhone simulator: iPhone 17 Pro, arm64
- iPad simulator: iPad Pro 11-inch (M5), arm64
- Physical target: iPhone 13, paired, connected, and signed with team `G55A9D98KC`
- Rust: `rustc 1.97.1`, `cargo 1.97.1`
- Docker: available for the disposable OpenSSH fixture
- Free repository volume space after verification: 24 GiB

## Golden Workflow

The live Controller command is:

```bash
TERMIRUST_IOS_DESTINATION='<exact Xcode destination>' \
  ./scripts/test-mobile-ios-controller-host.sh
```

It builds the real Rust `termirust-session-host` and a bounded Controller fixture,
starts a dedicated terminal Session, and publishes a short-lived private-network
pairing offer. The native Swift test then performs the following production path:

1. creates a device identity and performs Controller pairing;
2. compares the exact SAS and waits for explicit Host confirmation;
3. stores the device secret in a fixture-specific Keychain service;
4. authenticates, lists the real typed Session, and refreshes Host capabilities;
5. attaches read-only, acquires writer authority, sends input, observes output,
   and resizes the terminal;
6. suspends the terminal, applies the privacy cover, goes offline, and releases
   writer ownership;
7. resumes, reconnects, and automatically reacquires writer ownership when it is
   still available;
8. revokes the device on the Host and proves stale mutation and fresh
   authentication fail closed; and
9. removes the Keychain secret and terminates only fixture-owned resources.

The ephemeral pairing configuration is copied into an existing declared XCTest
resource before `build-for-testing`, then the source resource is restored
byte-for-byte immediately after the build. This makes the test bundle portable to
a physical device without changing the Xcode project or leaving a pairing secret
in the repository.

## Results

| Verification | Result |
|---|---|
| Real Rust Host/Controller lifecycle on iPhone 17 Pro simulator | PASS, 1/1 |
| Real Rust Host/Controller lifecycle on iPad Pro 11 simulator | PASS, 1/1 |
| Full iPhone simulator suite | PASS, 76 passed, 0 failed, 2 explicit live-fixture skips |
| Final full iPhone simulator suite after manual UX fixes | PASS, 81 passed, 0 failed, 2 explicit live-fixture skips |
| Full iPad simulator suite | PASS, 76 passed, 0 failed, 2 explicit live-fixture skips |
| Real SwiftNIO Direct SSH and persistent tmux smoke on iPhone simulator | PASS, 1/1 |
| Real Rust Host/Controller lifecycle on physical iPhone 13 | PASS, 1/1 |
| Full physical iPhone suite | PASS, 76 passed, 0 failed, 2 explicit live-fixture skips |
| Physical iPhone signed build and installation | PASS |
| Physical iPhone Home Screen app icon | PASS, user-observed |
| Physical iPhone full-screen layout and initial visual usability | PASS, user-observed |
| Physical iPhone portrait/landscape rotation and return | PASS, user-observed |
| Physical iPhone background/foreground UI recovery | PASS, user-observed |
| Physical iPhone touch focus and software keyboard | PASS, user-observed |
| Physical iPhone basic VoiceOver navigation | PASS, user-observed |
| Physical iPhone manual private-network pairing and exact SAS comparison | PASS, user-observed |
| Physical iPhone read-only attach, writer acquisition, and bidirectional terminal I/O | PASS, user-observed |
| Physical iPhone output follow, visible cursor, keyboard toggle, and exact text selection/copy | PASS, user-observed |
| Physical iPhone focused landscape terminal use with software keyboard | PASS, user-observed |
| Physical iPhone Control Center interruption and automatic writer reacquisition | PASS, user-observed |
| Physical iPhone Host revocation and denied reconnect | PASS, user-observed |
| Generic production iOS device build | PASS |
| Strict Swift 6 route/lifecycle verifier | PASS |
| Controller route-contract verifier with required runtime | PASS |
| Rust Controller listener test suite after human-confirmation timeout fix | PASS, 37 total |
| N02 bundled desktop/Host golden run after Host backend change | PASS |
| `./scripts/verify-product-model.sh --local` | PASS |
| Rust, Swift, and Kotlin diff hygiene | PASS |

The structured Xcode summaries recorded during the run are:

- iPhone full suite: `Test-TermiRustMobile-2026.09.02_10-52-59-+0530.xcresult`
- iPad full suite: `Test-TermiRustMobile-2026.09.02_10-53-37-+0530.xcresult`
- iPhone live Direct SSH/tmux: `Test-TermiRustMobile-2026.09.02_10-54-34-+0530.xcresult`
- iPad live Controller/Host: `Test-TermiRustMobile-2026.09.02_10-55-19-+0530.xcresult`
- physical iPhone live Controller/Host: `Test-TermiRustMobile-2026.09.02_11-03-46-+0530.xcresult`
- physical iPhone full suite: `Test-TermiRustMobile-2026.09.02_11-07-49-+0530.xcresult`
- final focused iPhone simulator regression: `Test-TermiRustMobile-2026.09.02_13-10-40-+0530.xcresult`
- final physical iPhone live Controller/Host rerun: `Test-TermiRustMobile-2026.09.02_13-15-44-+0530.xcresult`
- final full iPhone simulator suite: `Test-TermiRustMobile-2026.09.02_13-20-34-+0530.xcresult`

They are created under Xcode's `TermiRustMobile` DerivedData `Logs/Test`
directory. Xcode may remove older bundles according to its retention policy; the
commands above remain the reproducible authority.

## Defects Found And Fixed

The runtime work exposed defects that source-only verification did not reveal:

- the two Rust static libraries used library-style XCFramework layouts whose
  generated module maps collided in Xcode; each slice now uses a named static
  `.framework` inside its XCFramework;
- authentication failures represented as `ControllerFailure` were retried as
  transient network failures and could be remapped incorrectly;
- paired Host capability bits were never refreshed after the Host granted input;
- Swift's exact Session decoder rejected current Rust enriched fields;
- normal terminal Sessions did not expose an occupant generation through the
  Controller Host backend, making remote attach impossible;
- Docker Direct SSH startup used an unguarded `ssh-keyscan` readiness probe;
- Host pairing timed out after 30 seconds even though a real user still needed to
  compare and approve the SAS; the timeout now matches the signed offer's
  five-minute lifetime and a paused-time regression test exercises a 31-second
  human decision;
- pairing failures left a consumed one-use offer looking reusable; the app now
  clears it and presents a direct recovery action for generating a fresh offer;
- the pairing-offer editor did not look actionable; it now has a visible bordered
  field, placeholder, one-tap Paste Offer, Clear, and ready/error states;
- terminal output following targeted an artificial blank row below the terminal,
  hiding useful output; it now follows the cursor or last meaningful row without
  overscrolling;
- the software keyboard action only opened the keyboard; it is now a true
  show/hide toggle and reflects system dismissal;
- the Controller terminal did not render the VT cursor; it now draws a visible
  cursor from the bounded terminal model, including wide-cell handling;
- landscape keyboard presentation left too little terminal space; focused
  landscape mode now removes redundant chrome and retains compact control actions;
- opening Control Center safely released writer authority but required a manual
  reacquisition; resume now requests control again only when this phone previously
  held it, while still respecting another controller that acquired it; and
- a revoked device could remain in a misleading retry loop labeled
  `Reconnecting`; authentication connection closure is now terminal and surfaces
  the failed/degraded route immediately.

Regression tests cover capability refresh, authentication retry classification,
exact Session decoding, bare-LF terminal behavior, and terminal Session occupant
generation.

## Cleanup And Integrity

Post-run checks found no surviving Controller fixture, Session Host, temporary
pairing config, or fixture Docker container. The declared test resource was
restored to SHA-256
`ad210fd0caee1ca0a2aff3e08e700043b2faea443c614179ac79b28b83c4af9a`.

The pre-existing Swift project-file modification remains unchanged at SHA-256
`9cbdb068887ddab65f15080e1932f36658c369f8d7c344371d2f369abb5ea25e`.

After the golden run, a separately requested app-icon integration added the
native asset catalog and regenerated the Xcode project from `project.yml`. That
intentional additive branding change produced project SHA-256
`f7458b9a66536cb08e6ea5488932d4159fe236a5056411dfe58948f7b11aba40`;
the earlier hash remains the integrity value for the N04 execution itself.

Known non-blocking warnings are the SwiftNIO `CNIOWindows` umbrella-header
warning, duplicate NIO Objective-C class warnings in the XCTest host, existing
Rust Objective-C macro `unexpected_cfgs` warnings, and existing Rust dead-code
warnings. The production generic iOS build is unaffected.

## Completed Manual Exit Gate

The live Controller command passes on the physical iPhone. The automated test
proves real-device Keychain storage, private-network pairing, terminal attach,
writer release and automatic reacquisition, reconnect, resize, and revocation.
The following OS and usability observations were also completed by the user:

- Full-screen layout and initial visual usability on iPhone 13: **PASS**, observed
  by the user after the signed physical install.
- Home Screen app-icon rendering on iPhone 13: **PASS**, observed by the user
  after installing the shared desktop/iOS/Android artwork.
- Portrait-to-landscape reflow and return to portrait on iPhone 13: **PASS**;
  the user observed no clipping, overlap, disappearance, or stale layout.
- Backgrounding to the Home Screen and reopening after five seconds on iPhone
  13: **PASS**; the user observed a responsive, correctly sized restored UI.
- Touch activation, secure-field focus, software-keyboard presentation, editing,
  dismissal, and return from the Vault sheet: **PASS** on the physical iPhone;
  the temporary test text was deleted without importing or saving a vault.
- Basic VoiceOver navigation on the physical iPhone: **PASS**; the user heard
  understandable labels, moved focus through the app and Vault sheet without
  becoming trapped, and activated the Done action successfully. VoiceOver was
  disabled afterward, returning the device to its original interaction mode.
- Local Network reachability and exact SAS pairing: **PASS**; a fresh one-use offer
  paired the iPhone with the real Rust Host after explicit matching confirmation.
- Live terminal discovery and read-only attach: **PASS**; the phone displayed the
  Host's managed terminal and its current output.
- Writer authority and bidirectional terminal I/O: **PASS**; input entered on the
  phone reached the managed PTY and its output remained visible on the phone.
- Output following and cursor visibility: **PASS**; new output no longer scrolls
  into blank space and the current VT cursor is visible.
- Keyboard show/hide and portrait interaction: **PASS**.
- Focused landscape interaction with the software keyboard: **PASS** after the
  compact terminal layout change; useful terminal content remains visible.
- Background/foreground and Control Center interruption: **PASS**; privacy and
  writer release remain fail-safe, and the phone automatically reacquires control
  on return when no other controller took ownership.
- Exact terminal text selection and copy into Notes: **PASS**; only the selected
  text was copied.
- Host revocation: **PASS**; the device lost authority, fresh authentication was
  denied, and the app no longer remains indefinitely in `Reconnecting`.

N04 is therefore complete for the tested iPhone 13 and the representative iPhone
and iPad simulator classes. The repeatable physical-device command remains the
release regression gate for future Controller protocol or terminal-UI changes.
