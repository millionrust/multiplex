# Remote Screens — step-by-step build

Build order for [remote-screens-implementation-plan.md](remote-screens-implementation-plan.md).
Each step is one commit on `feat/remote-screens` and must pass `cargo fmt --check`, `cargo clippy`
for the touched crates, and the focused tests before it is committed. Section numbers in brackets
point into the plan. Interactive prototypes for every surface live in `design/remote-screens/`.

Legend: `[x]` committed · `[ ]` not started · **(device)** needs a real Mac permission, phone, or
network and is run by the owner.

## Gate 0 — design sign-off

- [x] Interactive prototypes: desktop, iOS, Android
- [x] Owner confirmed the prototypes (2026-09-15)

## Step 0 — Plan and prototypes

- [x] 0.1 `docs(remote-screens): add the plan, step list, and interactive prototypes`

## M1 — Codec core, no network (`crates/termirust-screen-codec`) [4.3, 4.5]

Pure Rust, no platform code, fully testable in CI.

- [x] 1.1 `feat(screen-codec): add the crate with surface geometry and the tile grid`
  Rect, surface size, 64×64 tile indexing, partial edge tiles, damage rectangles to tile sets.
- [x] 1.2 `feat(screen-codec): hash tiles and find changed tiles between frames`
  BGRA frame view with stride, xxh3-64 per tile, diff narrowed by damage.
- [x] 1.3 `feat(screen-codec): classify tiles as solid, text and UI, or picture`
  Bounded distinct-colour count and edge density.
- [x] 1.4 `feat(screen-codec): encode text and UI tiles losslessly`
  Palette + run-length when few colours, raw otherwise, then deflate (`miniz_oxide`).
- [x] 1.5 `feat(screen-codec): send pictures as a lossy first pass`
  Half-resolution, quantised pass for picture tiles; exact pixels arrive later by refinement.
- [x] 1.6 `feat(screen-codec): define the bounded tile-op wire format with golden vectors`
  Hand-written big-endian layout, checked lengths, closed op kinds, fixture vectors.
- [x] 1.7 `feat(screen-codec): reuse tiles the viewer already holds`
  Per-viewer LRU cache by content hash, host-side shadow, `Missing` recovery.
- [x] 1.8 `feat(screen-codec): detect vertical scrolls as moves`
- [x] 1.9 `feat(screen-codec): encode frames into ops and apply them to a framebuffer`
  Encoder and decoder end to end; decoded pixels equal the source for lossless tiles.
- [x] 1.10 `feat(screen-codec): send only what changed since the viewer's last acknowledgement`
  A lost scroll still resends the scrolled area: the host has no copy of the viewer's older frame.
- [x] 1.11 `feat(screen-codec): promote fast-changing regions to a motion region`
- [x] 1.12 `feat(screen-codec): refine lossy tiles to exact pixels when idle`
- [x] 1.13 `test(screen-codec): add fuzz targets and a synthetic workload report`
  Evidence: `docs/engineering-evidence/RS1-screen-codec.md`. Video on the tile path is above
  target on synthetic content; tuned with Stage A lower detail and the Stage B motion stream.

## M0 — Spikes (after M1, evidence only) [6]

- [x] 0.2 ScreenCaptureKit example: dirty-rect counts, idle ratio, bytes (`capture_stats`)
  Ran on this Mac: no dirty rects on macOS 27.0, 32.8 KB/s steady at native scale. See RS2.
  Scripted typing, scrolling, and video sessions remain to be captured by the owner.
- [x] 0.3 VideoToolbox example from Rust: HEVC low-latency session with LTR round trip under forced loss
  Ran on this Mac. Recovery with a long-term reference costs 1,141 bytes at 1080p and 4,217 at 4K,
  against 154,116 and 687,913 for the keyframe it replaces: 135x and 163x. M4 can be built on
  `EnableLTR` plus `ForceLTRRefresh`. Evidence, and three traps that would each have cost M4 a day
  — low latency is an encoder specification not a session property, `ForceKeyFrame` is really
  `"EncoderForceKeyframe"` and a wrong key is silently ignored, and the encoder drops frames so
  tokens must never be matched by counting — are in
  [RS4-videotoolbox-ltr.md](engineering-evidence/RS4-videotoolbox-ltr.md).
  The spike is `tools/videotoolbox-spike`, an excluded workspace, so it adds nothing to the
  workspace lockfile.
- [ ] 0.4 **(device)** iroh phone ↔ Mac over cellular with a self-hosted relay; go/no-go note
  Now gates the Stage B transport (M5), not Stage A.
- [ ] 0.5 `docs(remote-screens): record spike results` in `docs/engineering-evidence/`

## M2 — Desktop host and desktop viewer on a LAN (Stage A) [4.1, 4.2, 4.6, 4.7, 4.9]

- [x] 2.1 `docs(controller-security): amend the ADR for screen capability bits` + regenerated vectors
- [x] 2.2 `feat(controller-security): add ObserveScreens, ControlPointer and ControlKeyboard`
  2.1 and 2.2 landed in one commit: the fixture pins the ADR checksum, so the amendment and the
  code it describes cannot be split without a broken commit between them. Amendment 1 also adds
  the screen frame kind for the Stage A transport decision, and new vectors pin the screen offer,
  the first screen frame, and the first values outside each closed set. Every earlier vector is
  unchanged. The owner still has to accept the amendment (release gate).
- [x] 2.3 `feat(screen-capture): add the capture trait and the portable differ backend`
- [x] 2.4 `feat(screen-capture): capture displays with ScreenCaptureKit on macOS`
  2.3 and 2.4 landed in one commit: one crate, one lockfile review. The codec's tile hashing is
  the differ; the crate adds a replay source and preview downscaling.
- [x] 2.5 `feat(screen-protocol): add the session messages and stream framing`
  A transport-neutral crate (`termirust-screen-protocol`) so the protocol does not wait on the
  iroh decision in 0.4.
- [x] 2.6 `feat(controller-listener): carry screen sessions as Controller screen frames`
  Stage A decision (2026-09-16): the existing LAN, SSH and relay routes, not iroh. The QUIC
  transport moves to M4/M5 with the motion path; spike 0.4 gates it there, not here.
  A screen frame carries a chunk of the screen protocol's byte stream, so a batch larger than one
  frame spans two and the codec needs no new size rule. Every frame is authorized against the
  device's current record, so withdrawing a capability stops that traffic at once. The host pushes
  bytes on a channel; the application implements `ControllerScreenSession`, which the desktop app
  does in 2.12.
- [x] 2.7 `feat(controller-listener): issue screen tickets over the Controller channel`
  `OpenScreen` and `CloseScreen` commands, answered by the listener itself, mint and withdraw a
  one-time 32-byte ticket bound to the connection and to what the device's capabilities allow.
  Screen grants come from the paired record, so taking `ObserveScreens` away stops the next
  request on an open connection. The ticket is what the screen protocol's hello proves.
- [x] 2.8 `feat(screen-session): add the host and viewer session state machines`
  Covers the protocol logic of 2.8 and 2.10 in `termirust-screen-session`: ticket and grant
  checks, one encoder per subscription with flow control, previews, refinement, motion region
  messages, input gated on control, and resume across connections. Wiring capture threads and
  drawing into the desktop app happens in 2.12 and 2.13.
- [x] 2.9 `feat(screen-session): mask TermiRust terminal panes and publish their placement`
  Protocol and session logic: the host publishes pane placements (hosted session id, rectangle,
  cell size) and, for panes the viewer says it draws from text, fills them with one colour before
  encoding, so typing in them costs next to nothing. Placements precede the masked pixels, follow a
  moved pane, clip at the surface edge, survive reconnects, and never apply to previews. Reading
  pane rectangles and window occlusion from the desktop app happens in 2.12.
- [x] 2.10 Viewer session: landed with 2.8.
- [x] 2.11 `feat(screen-input): inject pointer and keyboard input behind the writer lease`
  A GPUI-free crate (`termirust-screen-input`): a single-writer injector that maps surface pixels
  onto the display arrangement, counts multi-clicks, keeps scroll fractions, chunks text, and
  releases every held key and button when control moves. The macOS backend posts Core Graphics
  events tagged `TRSI` and checks Accessibility first. Tested against a recording sink and a real
  host session; the Core Graphics sink was checked for permission only, not driven on the desktop.
  Sharing one lease with the text path's writer lease happens in 2.7.
- [x] 2.12 `feat(desktop): show computers with live previews in Devices`
  `controller/watch_session.rs` is the mirror of `screen_sharing.rs`: that serves this Mac's
  displays to a device, this makes this Mac the device. A session runs on its own thread with its
  own runtime, because the Controller channel is async and the interface is not, and publishes the
  latest picture and what it is doing for the interface to read as it draws. Input goes the other
  way on a channel, so a pointer move never waits on the network.
  Devices shows a preview beside every computer that granted screen access, at the computer's
  thumbnail profile. They open when the page appears and close when it goes away: a preview is a
  connection to someone else's computer, and it should last no longer than the page showing it. A
  computer that is asleep or refusing simply never sends a picture, and the row says why.
- [x] 2.13 `feat(desktop): open a remote screen tab with zoom, minimap and inspector`
  "Watch" on a computer in Devices opens a workspace tab showing it at full detail, beside an
  inspector: what the session is doing, the zoom, the computer's size, how many pictures have
  arrived, how many displays it shares, and who holds control, with "Fit" and taking or giving
  back control. The tab owns the session, so closing the tab closes the connection: watching
  somebody else's screen never outlives the window showing it.
  The geometry — fit, magnify to six times, a pan clamped so the picture cannot be thrown off the
  tab, and the rectangle the minimap draws — is kept apart from the session in `ScreenGeometry`,
  so the arithmetic a person's hand depends on is checked without another computer to connect to.
  The inspector says in words that Stage A reports no round-trip time or loss, rather than drawing
  a graph of numbers it does not have.
- [x] 2.16 `feat(desktop): pair this computer as a device of another computer`
  The missing half of pairing. Devices gains "Computers this Mac can watch": enter the address and
  the six-digit code another computer shows, and this Mac pairs as its device. The record is a
  small file per computer in the app's data directory; the device private key goes to the system
  credential store, never on disk beside it, the way the CLI's SSH controller profiles do it.
  The record is written before the pairing is acknowledged, so a pairing this Mac cannot keep is
  refused rather than leaving the other computer trusting a device this one has forgotten. A
  record that names another computer's key, carries no generation, or carries a capability this
  build does not know fails closed and is skipped rather than failing the whole list.
  Prerequisite for 2.12 and 2.13, which can now be built.
- [x] 2.14 `feat(desktop): share this computer's screen from Devices`
  The opt-in and the host wiring landed: Devices has a "Share this screen" choice, off by default,
  which the listener worker reads from its descriptor. Capture and injection run in that worker,
  one capture thread per display and one injection thread, because Core Graphics events need a
  thread that owns the event source. Checked on this Mac: the worker is the same signed binary
  under the same identifier as the app, so macOS treats them as one client and does not prompt
  twice. It records the grant against the process it holds *responsible* — the app that started
  TermiRust — so a shipped `.app` is prompted for by its own name, while the LaunchAgent, which
  launchd starts with no responsible parent, needs its own grant. Evidence, and the two defects
  that only the real app showed, are in
  [RS3-remote-screens-stage-a.md](engineering-evidence/RS3-remote-screens-stage-a.md).
  The sharing indicator landed next: the listener reports who is watching, and who holds control,
  every half second while it changes, and Devices names them and offers "Stop sharing". Reports
  carry device ids only, never screen content, and the last watcher leaving is itself a report.
  Per-device grants finished it: each paired device has "Allow watching" and "Allow pointer and
  keyboard", which write the Controller-v1 capability bits. Control implies watching, taking
  watching away takes control with it, and the terminal input choice no longer wipes either.
- [x] 2.15 `feat(screen-host): serve screens over the Controller channel, end to end`
  `termirust-screen-host` is the screen session the listener carries: it reassembles the protocol
  from screen frames, spends the ticket, checks each frame's capability against what the message
  would do, and answers with tile batches. A test drives a real viewer over a real Controller
  connection: three captured frames arrive pixel-exact, control is asked for and granted, a click
  reaches the host, and typing is refused because the keyboard was never granted. Injection stays
  with the caller, so this crate has no platform code.
  Capability denial per route and stale epochs are covered in the listener's own tests.
  Evidence for everything Stage A: `docs/engineering-evidence/RS3-remote-screens-stage-a.md`.

## M3 — Phone clients (Stage A) [4.1]

Amendment 1 also obliged the mobile bindings to be regenerated, which happened on 2026-09-16 for
both platforms: the phone's Swift and Kotlin now name `ObserveScreens`, `ControlPointer` and
`ControlKeyboard`. The Android halves were built against NDK 27.1.12297006 on the external volume.


- [x] 3.1 `feat(screen-bindings): expose watching and driving a screen to Swift and Kotlin`
  A uniffi boundary of its own (`termirust-screen-bindings`), beside the Controller one rather
  than inside it, so the audited crypto boundary stays narrow. The phone feeds it screen frame
  bytes and takes back bytes to send, each tagged with the capability its frame must claim, and
  copies pixels only for the rectangles an update reported. Tested against a real host session:
  pixels match what was captured, a moved box repaints a fraction of the screen, input waits for
  control, and a preview stays separate from the full view.
  Build, sync and determinism scripts mirror the Controller ones, and the decisions are in
  `docs/decisions/screen-bindings.md`. The iOS half was built on this Mac: an `.xcframework` for
  device and simulator plus `TermiRustRemoteScreens.swift`. The Android half needs NDK 27.1 on the
  machine that runs it.
  Swift replays the same recorded session Rust does (`scripts/test/swift-screen-bindings.sh`,
  fixtures under `crates/termirust-screen-bindings/tests/vectors/`) and rebuilds a
  byte-identical picture. Compiling the generated Swift is what caught an error case named
  `Protocol`, which Swift refuses; it is `InvalidMessage` now.
- [x] 3.2 `feat(ios): show live previews in Fleet and a computer detail screen`
  The phone asks for a screen session (`ControllerScreenSession.swift`: the two commands, the
  ticket parser, and the pump that tags each chunk with the capability its contents need), and
  `ControllerConnectionActor.watchScreen` opens one and pumps it. A real iPhone simulator watches
  and drives a real Rust host end to end: the live fixture
  (`scripts/test/mobile-ios-controller-host.sh`) serves a synthetic 320×200 screen, and the test
  asserts the welcome, three distinct pictures, the control handover, and the pointer and typing
  the host recorded.
  The surfaces landed on top of that. A computer's page gains "This Computer's Screen": a preview
  at the computer's thumbnail profile, about one picture a second, and "Open Screen", which opens
  the full view. Fleet rows show the last picture of each computer, so the list says something
  without holding a connection open for every computer at once —
  `ControllerScreenCoordinator` owns the one session the phone has, and a preview and the viewer
  are that session in two shapes. Only a computer that granted `ObserveScreens` shows any of it.
  Watching a real Mac also needed the surface to come from the welcome rather than a fixed id:
  the fixture shares surface 1, but a Mac names its displays by their own `CGDirectDisplayID`s,
  so `watchScreen(surface: nil)` now subscribes to the first display the computer offers.
- [x] 3.3 `feat(ios): add the remote screen viewer with zoom, minimap, pointer modes and keyboard`
  `RemoteScreenViewModel` and `RemoteScreenView` draw damaged rectangles into one bitmap, fit the
  picture, and offer control only when the computer has given it. On top of that: pinch to zoom up
  to six times, drag to pan with the picture clamped so it cannot be thrown off the view, a
  double tap back to fit, and a zoom chip that reads "Fit" or a percentage. A minimap appears
  once the picture is bigger than the view and shows which part is on screen.
  Two pointer modes: touch, where the pointer goes where the finger lands, and trackpad, where
  the finger drags the pointer from where it was so a fingertip stops hiding small targets. The
  keyboard is a text field for characters plus the row a text field cannot type — esc, tab, ctrl,
  the arrows, pipe and minus — sent as USB HID usages so layouts stay the computer's business.
  A computer sharing more than one display offers the choice.
  The 21 phone tests run on a simulator (iOS 27.0, 24A434) and pass.
- [x] 3.4 `feat(ios): show weak-connection details and reconnect from the last picture`
  A screen session is long-lived and a phone loses those: it changes network, sleeps, or walks out
  of range. A dropped session is opened again with a growing wait, up to five times, and the last
  picture stays on screen under a "Reconnecting…" overlay, because a frozen picture of the right
  computer says more than an empty one. A computer that takes screen access away is not retried;
  that is a decision, not a network problem.
  The connection sheet reports what the phone can actually see: the route, how many pictures have
  arrived, how long ago the last one was, the display and its size. Stage A rides the Controller
  channel, which reports no round-trip time or loss, so none is invented — bandwidth, loss and the
  ladder that reduces detail arrive with the motion path. A banner appears when no picture has
  arrived for four seconds, and says plainly that nothing has been lost.
- [x] 3.5 `feat(android): the same four surfaces in Compose`
  Android had none of Remote Screens: the generated Kotlin bindings and the `.so` were staged but
  nothing used them. It now has the same shape as the phone. `ControllerScreenSession.kt` owns the
  two commands, the ticket, and the pump that tags each chunk with the capability its contents
  need; `ControllerConnection.watchScreen` opens a session and pumps it, taking the surface from
  the welcome rather than a fixed id; `ControllerScreenCoordinator` owns the one session a phone
  has, as a preview or the viewer, and opens it again when it drops.
  `RemoteScreenModel` is the Kotlin counterpart of the phone's view model: damaged rectangles into
  one bitmap, fit, zoom to six times, clamped pan, the minimap rectangle, touch and trackpad
  pointer modes, and the key row a text field cannot type. The Compose surfaces are the preview
  card with "Open Screen" on a computer's page and the viewer with its dock.
  Android carried the same forward-compatibility bug the phone had: it refused any granted
  capability set containing a bit it did not know, so granting a tablet screen access would have
  broken its terminal connection. It knows all eight bits now.
  16 unit tests cover the geometry, the capability gate, and the ticket rules.
- [ ] 3.6 **(device)** Stage A network matrix on a real iPhone and Android phone [7]

## M4 — Motion path (Stage B, part 1) [4.4]

- [x] 4.1 `feat(screen-transport): add video, FEC and LTR acknowledgement messages` + vectors
  Protocol version 2. The hello and the welcome carry a feature set, and a host answers in the
  version the viewer spoke, so a phone built for Stage A is neither locked out nor sent video it
  cannot decode. Five messages: video config, video frame, parity, acknowledge, lost. Both sessions
  fail closed on anything neither side negotiated.
  The recorded Stage A session in `screen-session-v1.json` is kept byte for byte and replayed by
  the current viewer; `screen-session-v2.json` pins what this build records.
- [x] 4.2 `feat(screen-host): encode motion regions with VideoToolbox and LTR recovery`
  A new `termirust-screen-video` crate holds the VideoToolbox session and the one `unsafe` block in
  this path, with no dependencies at all; `termirust-screen-host` drives it behind `MotionEncoder`
  and keeps `#![forbid(unsafe_code)]`. Payloads are Annex B, because Android needs it and Apple does
  not mind. Two things the tests pin: a frozen region stops producing frames rather than re-encoding
  a still picture, and a report of loss asks for a reference refresh, never a keyframe.
  Found along the way: while the rest of the screen is silent the viewer stops acknowledging, the
  tile encoder stalls on back-pressure, and a motion region then never demotes. Real desktops always
  have something ticking, so this is not yet a bug worth its own change, but 5.2 should not assume
  demotion is timely.
- [x] 4.3 `feat(screen-transport): send video as datagrams with forward error correction`
  Systematic Reed-Solomon over GF(256) on a Cauchy matrix, written by hand in
  `termirust-screen-protocol::fec` so the motion path adds no package to a security-reviewed
  lockfile. The shards are whole encoded messages, so a repaired frame carries its own sequence,
  keyframe flag and reference token rather than having them guessed.
  Parity is sent after the group it repairs, never before, so a viewer that lost nothing pays no
  latency for it. The ratio follows reported loss between five and forty percent.
  Still on the ordered Controller stream, where nothing is ever actually lost: the tests drop
  frames on the wire to prove the repair works. Real datagrams arrive with iroh in 5.3.
- [x] 4.4 `feat(screen-client): decode motion regions natively on desktop, iOS and Android`
  The decoded region is drawn into the same framebuffer the tiles go into, so `framebuffer()`
  returns one complete screen and nothing above the session had to change: the desktop viewer and
  the iPhone both got video without a line of UI work. That is safe because the host stops
  claiming to know the region's tiles the moment it promotes it.
  `termirust-screen-video` now builds for iOS as well as macOS, which is what gives the phone
  hardware decode. Android has no decoder here, so its frames are handed up for MediaCodec, which
  is the one piece of 4.4 still to write in Kotlin.
  The acknowledgement loop closes here: a reference is only acknowledged once it has actually
  decoded, so the host never predicts from a picture the viewer does not have.

- [x] 4.5 `feat(android): decode the motion region with MediaCodec`
  `MediaCodec` behind a `MotionDecoders` interface, so the half that can be got wrong silently —
  which references this phone may claim to hold — is unit tested on the JVM without a device.
  The viewer asks for video only when the client says it will draw it. On Apple that promise is
  implicit, because the library decodes for itself; on Android an app that ignored the video
  events would leave the moving part of the screen frozen, so the default is no.
  **(device)** Not yet run on a phone: `MediaCodec` configuration, the YUV to ARGB conversion and
  its colour matrix are the parts a device would settle.

## M5 — Rate control, roaming, bad networks (Stage B, part 2) [4.6, 7]

- [x] 5.1 `feat(screen-transport): estimate bandwidth from frame-paced bursts`
  The host closes each flush with a burst mark; the viewer times the arrival and reports it; the
  estimate is the harmonic mean of the last eight bursts less fifteen percent. A new
  `BANDWIDTH_REPORTS` bit, so a peer that cannot time bursts is never asked to.
  The subtlety worth remembering: the chunk that opens a burst starts the clock and must not be
  counted by it. Counting it read a one megabyte link as 1.44 MB/s — inflated by n/(n-1) over a
  burst of n chunks — which is exactly the kind of wrong that looks plausible.
  Measured over whatever carries the bytes, so today that is the ordered Controller channel; iroh
  datagrams in 5.3 make the spread mean more. Nothing acts on the number yet: that is 5.2.
- [x] 5.2 `feat(screen-host): drive the degradation steps from the estimate`
  Five rungs in the plan's order, taken as demand crowds the estimate and given back as it
  clears: stop refinement, viewport only, slower frames, coarser first pass, capped video. The
  terminal text path is never on the ladder.
  A rung holds for 1.5 s before another is taken, and climbing back needs more headroom than
  staying put. Both exist for the same reason: without them a controller reacts to a measurement
  of its own previous behaviour and walks itself to the floor.
  Wired to real knobs — `Limits` on the session, `LossyDetail::LOW`, an encoder reopened at the
  capped bitrate. Two rungs the plan names are **not** here: half-scale viewport tiles and 480p
  video both need work that does not exist yet (viewport priority in the codec, and a scaler), so
  the ladder stops at five rather than pretending to have seven.
- [x] 5.3a `feat(screen-transport): class each message by the delivery it needs`
  A `termirust-screen-transport` crate holding the seam iroh will fit into: every message carries a
  `Class`, and a transport says what it promises each one. Stage A gives all three the same ordered
  stream, so the bytes are unchanged — the grouping is correct before there is anything to group.
  The seam is about keeping the classes apart, not blurring them. Tile batches are differences from
  the last picture, so one lost batch leaves the screen wrong forever; only the motion path may take
  a route that drops things, because it is the only part built for it. A transport that claims
  otherwise is refused when the session opens rather than when a batch goes missing.
- [ ] 5.3b `feat(screen-transport): survive network changes with migration and 0-RTT resume`
  **Blocked on the 0.4 iroh spike.** The seam above is what it plugs into.
- [ ] 5.4 `docs(self-hosted-relay): deploy an iroh relay next to relay-host`
- [ ] 5.5 **(device)** Full network matrix for both stages; tune the steps

## M6 — Windows and Linux hosts, background hosting [4.2, 4.7]

- [ ] 6.1 `feat(screen-capture): capture with Desktop Duplication on Windows`
- [ ] 6.2 `feat(screen-capture): capture through the PipeWire portal on Linux`
- [ ] 6.3 `feat(screen-host): inject input on Windows and Linux`
  - [x] Windows, with `SendInput`. Hand-declared FFI in one `allow(unsafe_code)` module, so the
    crate keeps `deny(unsafe_code)` everywhere else and the `windows` crate stays out of the
    lockfile for four functions and a struct.
    Keys travel as PS/2 set 1 scancodes rather than virtual keys, which is what keeps them
    positional: a viewer on a French keyboard driving a US host gets what that host's layout
    produces from the position pressed. Text goes as `KEYEVENTF_UNICODE`, because a pasted line or
    an emoji has no position to send.
    The two keymaps are now checked against each other, and the test names every key the two hosts
    differ on rather than letting a gap appear silently — `F13` upwards, Apple's `Help` and keypad
    `=` one way; Print Screen, Scroll Lock and the Menu key the other.
    Cross-checked against `x86_64-pc-windows-msvc`. **(device)** Never run on Windows.
  - [x] Linux, through `uinput`. A virtual device in the kernel, so it works the same under X11,
    Wayland and a bare console, where the X11 and Wayland routes would each need their own.
    **Text is the one thing it cannot do.** A `uinput` device reports key positions and nothing
    here can reach the keymap that turns positions into characters, so a pasted accent or an emoji
    returns `UnmappedKey` rather than silently arriving as the wrong letters. Typing ordinary keys
    works. Doing better needs a custom XKB keymap uploaded with the device, which is its own step.
    `/dev/uinput` is root-only by default; a desktop install wants a udev rule for the `input`
    group rather than a host running as root, and the error says so.
    Writing the third keymap made the set checkable: Linux turns out to be a strict superset of
    both other hosts, including `Pause`, which Windows cannot express and macOS has no event for.
    Type-checked against `x86_64-unknown-linux-gnu`; it cannot be linked or run from a Mac.
    **(device)** Never run on Linux.
- [x] 6.4 `feat(controller-service): serve screens from the macOS background service`
  The app's listener worker already served screens; the LaunchAgent's used the plain worker, so a
  paired phone could watch this Mac only while the app happened to be open — the one thing the
  background service exists to stop being true. Both now get the same provider.
  The permission finding from the `(device)` investigation is what makes this more than a one-line
  change. macOS records a grant against the responsible process, and a LaunchAgent has no
  responsible parent, so granting TermiRust Screen Recording does **not** cover the service.
  Without the grant ScreenCaptureKit returns frames of a blank desktop rather than failing, so
  `controller-service status` now says so, names the label to allow, and says why the app's own
  grant was not enough.
  **(device)** Still to confirm on a real install: that the LaunchAgent prompts under its own name
  and that the grant sticks across a restart.

## Release gates [8]

- [ ] Workload report within [4.8] targets for typing, scrolling and window switching
- [ ] Network matrix pass criteria for the stage being released
- [ ] Stage A 30-minute usability session over 300 ms / 5 % / 1 Mbps
- [ ] Capability ADR amendment accepted
- [ ] Independent review of ticket bootstrap and endpoint pinning
- [ ] New dependencies recorded with licence and reason
- [ ] `cargo test --workspace --all-targets --locked` and `cargo deny check` green
