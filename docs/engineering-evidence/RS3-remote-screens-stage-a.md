# RS3 Remote Screens Stage A Evidence

Date: 2026-09-16

## Outcome

A paired device can watch this computer's screen and drive it, over the authenticated Controller
channel, with the permission enforced in the protocol rather than in application code. Steps 2.1,
2.2, 2.6, 2.7, 2.9, 2.11, 2.14, 2.15 and 3.1 of
[remote-screens-todo.md](../remote-screens-todo.md) are covered here; 2.12, 2.13 and 2.16 are not
built, and 3.2 onwards is the phone interface.

What a device needs, in order:

1. A pairing that grants `ObserveScreens`, and `ControlPointer` or `ControlKeyboard` to drive.
   Devices grants them per device; taking one away stops that traffic on the open connection.
2. Screen sharing turned on for this computer, in Devices. It is off until someone turns it on.
3. An `OpenScreen` command, which returns a one-time 32-byte ticket bound to that connection.
4. Screen frames (Controller frame kind 3) carrying the screen protocol, each claiming the
   capability its contents exercise.

## What the protocol enforces, not the application

- Amendment 1 of [controller-security-v1.md](../decisions/controller-security-v1.md) adds the
  three capability bits and the screen frame kind. Watching, pointing and typing are separate
  grants, and all three are separate from `SendInput`, which stays a terminal capability.
- Every screen frame is authorized against the device's **current** record, so withdrawing a
  capability stops that traffic without waiting for the session to end.
- The host checks the frame's claim against the message it decodes, so a frame labelled
  "watching" cannot carry a keystroke.
- Input reaches the operating system only while the device holds the writer lease, and every key
  and button it left pressed is released when the lease moves.

## Automated evidence

```text
cargo test -p termirust-screen-codec -p termirust-screen-protocol -p termirust-screen-session \
  -p termirust-screen-capture -p termirust-screen-input -p termirust-screen-host \
  -p termirust-screen-bindings -p termirust-controller-security -p termirust-controller-bindings \
  -p termirust-controller-listener -p termirust-domain
PASS: 365 tests, 0 failed

./scripts/verify/controller-security-vectors.sh --check
PASS: amendment vectors, and every vector published before it, unchanged

./scripts/verify/localization.sh --locales en-US,en-XA,ar-XB --no-new-baseline
PASS
```

The end-to-end tests are the ones worth naming:

| Test | What it proves |
|---|---|
| `termirust-screen-host` `a_viewer_watches_and_drives_a_computer_over_the_controller_channel` | Three captured frames arrive pixel-exact over a real authenticated connection; control is asked for and granted; a click reaches the host; typing is refused because that device was never granted the keyboard. |
| `termirust-screen-bindings` `a_phone_watches_a_computer_and_draws_only_what_changed` | The phone boundary receives the pixels that were captured, and a moved box repaints a fraction of the screen. |
| `termirust-controller-listener` `screen_frames_carry_the_session_and_stop_when_a_capability_is_withdrawn` | A batch larger than one frame spans two frames; withdrawing `ControlPointer` ends the connection on the next pointer frame. |
| `termirust-screen-session` `an_attached_pane_costs_nothing_while_its_text_changes` | Typing in a terminal pane the viewer draws from text costs under a twentieth of the pixels. |

## Bandwidth, from the codec workloads (RS1)

Measured on synthetic content at 1512 × 982, unchanged by this milestone:

| Workload | Bytes per second |
|---|---|
| Idle | 0 |
| Typing | 1.9 KB/s |
| Scrolling | 30.8 KB/s |
| Window switching | 4.5 KB/s, largest batch 33.5 KB |
| Video on the tile path | 205.8 KB/s — above target, and why Stage B exists |

Real capture on this Mac (RS2) settles at 32.8 KB/s at native scale with no dirty rectangles
reported by macOS 27.0, so the tile hashing carries the whole diff.

## The phone, on a simulator

```text
xcodebuild -scheme TermiRustMobile -destination 'platform=iOS Simulator,id=<iOS 27.0 device>' \
  -only-testing:TermiRustMobileTests/RemoteScreenViewModelTests \
  -only-testing:TermiRustMobileTests/ControllerScreenSessionTests test
PASS: 11 tests, 0 failures, on iOS 27.0 (24A434)
```

Reading the phone's connection path to write them found a defect the Rust tests could not:
the app refused any granted capability set containing a bit it did not recognise, so granting a
paired phone `ObserveScreens` would have broken its **terminal** connection with
`authenticationFailed`. Fixed by teaching the app the three bits.

## The phone against a real host

```text
./scripts/test/mobile-ios-controller-host.sh
PASS: the fixture saw the phone watch and drive its screen
      (opened=1 pointer=1 keyboard=1 control_requests=1)
PASS: real iOS Controller pairing, terminal lifecycle, and revocation completed
```

The live fixture (`crates/termirust-controller-listener/examples/mobile_controller_fixture/`)
now serves a synthetic 320 × 200 screen: no capture and no platform code, just a caret that moves
every frame, which is enough to prove pixels reach the phone. One simulator run pairs, lists
sessions, drives a terminal, then watches that screen: it asserts the welcome names the surface,
that three *distinct* pictures arrived, that control asked for and given reaches the phone, and
that the pointer and the typing it sent were recorded by the host. It then re-attaches a terminal
and confirms revocation closes both the terminal and a new screen session.

Two defects surfaced only because a real phone drove a real host:

- The listener makes a screen session for **every** authenticated Controller connection, before
  any device asks to watch. The desktop started its ScreenCaptureKit threads there, so a phone
  that opened only a terminal would have switched this Mac's screen recording on with nobody
  watching. Capture and injection now start on `ScreenHostEvent::Opened`.
- A script that only reads the test's exit status cannot tell a pass from a skip, and a missing
  fixture makes this test skip itself. The script now asks the fixture what it saw and fails
  unless the phone really opened, watched and drove the screen.

## The real desktop, on this Mac

Run against the running app rather than a fixture, with
`crates/termirust-controller-listener/examples/controller_device_probe.rs` — a paired device on
the command line. It pairs with the six-digit code Devices shows, opens a screen session, and
points and types.

```text
controller_device_probe watch --address 192.168.88.4:63322
connected: granted=0x00e3
screen session opened
welcomed: 2 display(s); first is Main Display at 3024x1964
asked for control
control: You
sent a pointer move and a keystroke
watched: 230 batch(es); picture Some((3024, 1964)); drove=true
```

### Which process macOS holds responsible

The listener is a **child process of the app**: the same executable, re-run as
`termirust --controller-listener`. Capture and injection happen there. `tccd` was read with
`log show --predicate 'subsystem == "com.apple.TCC"'` while the probe watched:

```text
AUTHREQ_CTX:         msgID=13959.1, service=kTCCServiceAccessibility, preflight=yes
AUTHREQ_ATTRIBUTION: accessing={identifier=com.termirust.desktop.dev, pid=13959,
                                binary_path=.../target/debug/termirust},
                     responsible={identifier=dev.zed.Zed, pid=1894}
AUTHREQ_SUBJECT:     subject=dev.zed.Zed
AUTHREQ_RESULT:      authValue=2, authReason=4
```

The same shape appeared for `kTCCServiceScreenCapture`, `kTCCServiceListenEvent` and
`kTCCServicePostEvent`. Two things follow:

- **The worker is not a separate client.** It is the same signed binary under the same identifier
  as the app, so a grant to TermiRust covers the worker. macOS does not prompt twice, and this
  question is settled.
- **The grant is recorded against the *responsible* process, not the accessor.** TermiRust runs
  here as a bare binary started by `cargo run`, so macOS held the editor that owns that shell
  responsible and the access rode on *its* Screen Recording and Accessibility grants
  (`authReason=4`, an existing grant). A shipped `.app` opened from the Dock is its own
  responsible process and is prompted for, and listed, under its own name. Two consequences:
  in development nobody sees a TermiRust entry in System Settings, and the macOS LaunchAgent
  (`termirust controller-service`), which launchd starts with no responsible parent, is its own
  subject and needs its own grant.

### Three defects the fixture could not show

- **The screen was captured before anyone asked to watch.** `displays()` runs in
  `ScreenSharing::open`, which the listener calls for *every* authenticated connection, and it
  calls `SCShareableContent::get()`. So with sharing on, a phone that only opens a terminal makes
  this Mac ask for Screen Recording. The capture threads no longer start until a device opens a
  screen session, but `displays()` still runs at connection time, so the prompt is still earlier
  than the UI's "the first time a device watches" promises. Left as it is for now and recorded
  here: the surfaces have to be known before the session can welcome anyone.
- **Control was never handed over.** `ControlRequested` only updated the sharing indicator, so
  `HostSession` kept the lease at `Nobody` and refused every pointer and keystroke. A device the
  person had granted "Allow pointer and keyboard" could watch but never drive. The app now
  answers the request by giving the lease, from a short-lived thread, because the session calls
  its observer while holding the lock that `set_control` needs. The probe shows the difference:
  `drove=false` before, `control: You` and `drove=true` after.
- **The sharing indicator kept naming a watcher who had left.** A device that hangs up never sends
  `CloseScreen`; the listener simply drops the session, and nothing told the application. Devices
  went on saying "Watching now: Probe device" after the device was gone, which is exactly the
  wrong thing for a privacy indicator to be wrong about. `ScreenHost` now closes on drop, so the
  application always hears it. After the fix the same run ends with "Nobody is watching this
  screen."

## Not proven here

- **(device)** What a shipped `.app` and the LaunchAgent are prompted for by name. The rule is
  established above; only the development shape was observed.
- The desktop viewer, which needs this Mac to be a device *of* another Mac (todo 2.16).
- The phone interface; only the boundary it will call exists (todo 3.2 onwards).
- Everything Stage B: the motion path, bandwidth estimation, and iroh.
