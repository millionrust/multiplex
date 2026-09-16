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

## Not proven here

- **(device)** Whether the listener worker inherits the app's Screen Recording and Accessibility
  grants or asks for its own. Capture and injection run in that worker, so this decides whether
  the first watch prompts twice.
- **(device)** Core Graphics injection was checked for permission only; no test drives a real
  pointer or keyboard, because doing so would take over the machine running the tests.
- The desktop viewer, which needs this Mac to be a device *of* another Mac (todo 2.16).
- The phone interface; only the boundary it will call exists (todo 3.2 onwards).
- Everything Stage B: the motion path, bandwidth estimation, and iroh.
