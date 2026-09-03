# N03 Real Android Controller Golden Run

**Evidence date:** 2026-09-03
**Status:** Complete for the Android emulator exit gate

## Scope

N03 proves that the native Kotlin application can pair with and control a real
Rust Session Host over the production private-network Controller protocol. The
test uses the production Rust-generated Android crypto binding, Java socket
transport, Android Keystore-backed secret store, protocol codecs, terminal
attach path, and writer commands. No mock transport or mock cryptography is used.

This record contains bounded command results and environment metadata only. It
does not contain pairing offers, SAS values, keys, terminal payloads, application
state, or private network addresses.

## Environment

- Host: macOS Darwin 25.5.0, arm64
- Rust: `rustc 1.97.1`, `cargo 1.97.1`
- Android JVM: OpenJDK 17.0.16
- ADB: 36.0.0-13206524
- Android emulator: 36.6.11.0
- AVD: Pixel 9, arm64-v8a, 1080 x 2424, Android 37.1 Google Play image
- Free repository volume space after the run: 18 GiB

## Golden Workflow

The reproducible command is:

```bash
./scripts/test-mobile-android-controller-host.sh --avd Pixel_9
```

The runner starts an owned headless emulator when required, builds the real
`termirust-session-host` and Controller listener fixture, injects an ephemeral
configuration into the instrumentation APK, installs both APKs, and executes one
bounded AndroidJUnit test. The test:

1. performs production Controller pairing and compares the exact SAS with the
   Host fixture before confirmation;
2. stores the device secret through Android Keystore and proves it is readable;
3. authenticates, lists the real terminal Session, and verifies the initial
   read-only capability grant;
4. applies a Host-side input grant and verifies Android refreshes the negotiated
   capability bits instead of retaining the pairing-time snapshot;
5. attaches to output, acquires writer authority, sends typed input and a
   multiline paste payload, and observes each output marker exactly once;
6. applies a 118 x 34 terminal resize and receives authoritative completion;
7. interrupts the terminal transport, reconnects from the exact output cursor,
   and reacquires writer authority without replaying a mutation;
8. revokes the device and proves both stale mutation and fresh authentication
   fail closed; and
9. deletes the test device secret and terminates only fixture-owned resources.

The test channel uses a bounded 64-event queue, a 64 KiB transcript cap, bounded
socket/stage deadlines, and command IDs for every mutation. Emulator fixture
coordination uses Android's host-loopback address; production Controller traffic
continues to use the signed private-network route from the pairing offer.

## Results

The final strengthened workflow passed three consecutive complete executions:

| Assertion group | Result |
|---|---|
| Production SAS pairing and Host confirmation | PASS, 3/3 |
| Android Keystore secret create/read/delete | PASS, 3/3 |
| Real Session list and read-only attach | PASS, 3/3 |
| Host capability grant refresh | PASS, 3/3 |
| Writer acquire, typed input, and multiline paste | PASS, 3/3 |
| Exact single output for all input markers | PASS, 3/3 |
| PTY resize completion | PASS, 3/3 |
| Exact-cursor disconnect/reconnect and writer reacquire | PASS, 3/3 |
| Revoked stale mutation and reauthentication denial | PASS, 3/3 |
| Owned fixture, emulator, resource, and secret cleanup | PASS, 3/3 |
| Real iOS Controller/Host regression after fixture change | PASS, 1/1 |
| `./scripts/verify-mobile-mvp.sh` | PASS |
| `./scripts/verify-product-model.sh --local` | PASS |

Android unit tests, production APK assembly, instrumentation APK assembly, and
`lintDebug` also passed before the live run. The focused snippet and desktop-pane
golden race regressions passed 10 consecutive executions each.

## Defects Found And Fixed

The runtime gate found two defects that source-only tests did not expose:

- Android requested only observe capability during fleet refresh, so a Host
  input/resize grant could never reach the saved Host record. Fleet
  authentication now requests all supported capabilities, validates the grant,
  returns it with the snapshot, and persists changes before opening a terminal.
- The fixture's nonblocking listener could yield a nonblocking accepted control
  stream on macOS. A scheduling race then returned `WouldBlock` before Android's
  request arrived. Accepted streams now switch explicitly to blocking mode while
  retaining a bounded read timeout.

Unit coverage now rejects unsupported capability bits, and the live workflow
proves the refreshed write grant against the real Host.

## Cleanup And Limits

After each run the injected asset returned to the inert
`{"schema_version":0}` placeholder. No fixture, Session Host, emulator, ADB
reverse, or `/tmp/tri.*` resource remained. The runner never removes an emulator
or device that it did not start.

This gate proves the Android emulator protocol, crypto, Keystore, and terminal
lifecycle path. It does not claim physical Android hardware, OEM-specific
background policy, touch/IME ergonomics, rotation, accessibility, or complete
full-screen TUI behavior. Those remain explicit N05/mobile qualification work.
