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
- [ ] 0.3 VideoToolbox example from Rust: HEVC low-latency session with LTR round trip under forced loss
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
- [ ] 2.6 `feat(screen-transport): carry screen sessions as Controller screen frames`
  Stage A decision (2026-09-16): the existing LAN, SSH and relay routes, not iroh. The QUIC
  transport moves to M4/M5 with the motion path; spike 0.4 gates it there, not here.
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
- [ ] 2.12 `feat(desktop): show computers with live previews in Devices`
- [ ] 2.13 `feat(desktop): open a remote screen tab with zoom, minimap and inspector`
- [ ] 2.14 `feat(desktop): add screen sharing settings, grants and the sharing indicator`
- [ ] 2.15 `test(screen-host): loopback end to end, capability denial on every route, stale epoch`

## M3 — Phone clients (Stage A) [4.1]

- [ ] 3.1 `feat(mobile-ffi): expose screen subscribe, tile apply, viewport and input`
- [ ] 3.2 `feat(ios): show live previews in Fleet and a computer detail screen`
- [ ] 3.3 `feat(ios): add the remote screen viewer with zoom, minimap, pointer modes and keyboard`
- [ ] 3.4 `feat(ios): show weak-connection details and reconnect from the last picture`
- [ ] 3.5 `feat(android): the same four surfaces in Compose`
- [ ] 3.6 **(device)** Stage A network matrix on a real iPhone and Android phone [7]

## M4 — Motion path (Stage B, part 1) [4.4]

- [ ] 4.1 `feat(screen-transport): add video, FEC and LTR acknowledgement messages` + vectors
- [ ] 4.2 `feat(screen-host): encode motion regions with VideoToolbox and LTR recovery`
- [ ] 4.3 `feat(screen-transport): send video as datagrams with forward error correction`
- [ ] 4.4 `feat(screen-client): decode motion regions natively on desktop, iOS and Android`

## M5 — Rate control, roaming, bad networks (Stage B, part 2) [4.6, 7]

- [ ] 5.1 `feat(screen-transport): estimate bandwidth from frame-paced bursts`
- [ ] 5.2 `feat(screen-host): drive the degradation steps from the estimate`
- [ ] 5.3 `feat(screen-transport): survive network changes with migration and 0-RTT resume`
- [ ] 5.4 `docs(self-hosted-relay): deploy an iroh relay next to relay-host`
- [ ] 5.5 **(device)** Full network matrix for both stages; tune the steps

## M6 — Windows and Linux hosts, background hosting [4.2, 4.7]

- [ ] 6.1 `feat(screen-capture): capture with Desktop Duplication on Windows`
- [ ] 6.2 `feat(screen-capture): capture through the PipeWire portal on Linux`
- [ ] 6.3 `feat(screen-host): inject input on Windows and Linux`
- [ ] 6.4 `feat(controller-service): serve screens from the macOS background service`

## Release gates [8]

- [ ] Workload report within [4.8] targets for typing, scrolling and window switching
- [ ] Network matrix pass criteria for the stage being released
- [ ] Stage A 30-minute usability session over 300 ms / 5 % / 1 Mbps
- [ ] Capability ADR amendment accepted
- [ ] Independent review of ticket bootstrap and endpoint pinning
- [ ] New dependencies recorded with licence and reason
- [ ] `cargo test --workspace --all-targets --locked` and `cargo deny check` green
