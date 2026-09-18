# Implementation plan: Remote Screens — see and drive every paired computer from a phone or another desktop

Status: **proposal, nothing built.** Research date 2026-09-15.

Handoff document for the implementing agent. Companion to
[remote-terminals.md](remote-terminals.md) (text terminals on the phone, which is built) and
the Controller decision records under `decisions/`. This file is the product shape, the
research that shaped it, the architecture, the build order, and the gates.

Every path is relative to the repository root.

---

## 1. Goal and non-goals

**Goal.** From the phone, or from a second TermiRust desktop, a person sees every paired
computer on one Devices screen with a live thumbnail of each, opens one, and gets an
interactive view of its whole desktop: pointer, keyboard, all displays folded into one
canvas with a minimap, plus the text terminals that already work today. It has to stay
usable on a bad link (hotel Wi-Fi, cellular at one bar, 200 kbps, 5% loss, 300 ms RTT):
degrade quality and frame rate, never freeze, never fall behind by seconds, and recover
from loss without a full-screen refresh.

This is the shape of Astropad Workbench (macOS 15 host, iOS/iPadOS client, "unified
display", minimap, relay network, LIQUID engine). We build our own, on the Controller trust
model and routes that already exist.

**What makes the bandwidth problem tractable.** Three facts, each verified below:

1. Desktops are mostly static. The OS already tells us which rectangles changed
   (ScreenCaptureKit `dirtyRects`, DXGI dirty and move rects, Wayland damage), and reports
   an idle status when nothing did. We never scan or send an unchanged pixel.
2. About 80% of desktop content is text and flat UI (Microsoft's figure for RDP
   sessions). Text compresses losslessly to a few KB per changed tile and repeats
   constantly, so a content-hash tile cache turns most updates into an 8-byte reference.
3. TermiRust's own terminals are already synced as text over the Controller. A terminal
   pane costs ~100 bytes per changed line as text and ~10–50 KB as pixels. The pixel
   path therefore never carries TermiRust terminal panes; it composes them client-side
   from the text stream.

**Licensing.** The whole feature, including the low-latency machinery (hardware video
path, rate controller, FEC), is open source in this repository under the workspace
licence. There is no paid tier and no private module.

**Non-goals for this work.**

- Remote audio, file transfer (SFTP exists), clipboard sync beyond text, multi-viewer
  collaboration, recording. Each is a later, separate goal.
- Being a general VNC/RDP client. We connect TermiRust hosts to TermiRust clients only.
- A browser client. The transport choice keeps that door open (see 5.4) but it is not in
  scope.
- Replacing the existing Controller channel, relay, or the text terminal path. Screens
  are a new capability layered on the same pairing.

### 1.1 Two stages, one product

The build is staged so that something useful ships early:

- **Stage A, the tile path** (milestones 1–3): damage-driven capture, tile modes,
  cache, scroll detection, terminal-pane masking, the Devices dashboard, input, LAN and
  relay, reconnect diff-sync. Motion is handled by lossy tiles at a capped rate, and the
  send interval floor is 66 ms. Sharp and cheap, a little laggy on motion. Shippable on
  its own.
- **Stage B, the motion path** (milestones 4–5): the 16 ms send interval, hardware video
  regions with long-term reference recovery and forward error correction, the
  frame-paced rate controller, continuous refinement, viewport-priority budget, AV1 when
  negotiated, roaming and multipath.

Both stages speak one protocol: Stage B adds message kinds (video frames, FEC symbols,
LTR acks, burst reports) that a peer advertises support for, so a Stage A client and a
Stage B host, or the reverse, degrade to the tile path without any negotiation failure.

---

## 2. What the industry does (research findings)

Findings are the load-bearing facts for the design in section 4. Read the sources before
changing a decision that cites them.

### 2.1 Astropad Workbench and LIQUID

- Workbench (April 2026): remote Mac on iPhone/iPad/Mac, "unified display" that folds
  multiple Mac displays into one canvas sized to the client, an interactive minimap,
  voice and keyboard input, "a global relay network across 11 regions" with no port
  forwarding, end-to-end AES-256. Positioned for watching AI agents on a headless Mac
  mini. Free tier 20 min/day.
- LIQUID is tile based: the screen is split into independent 64×64 or 128×128 tiles,
  only changed tiles are processed and sent, and tiles are coded **without inter-frame
  prediction**. Packet loss therefore damages one tile, not a whole GOP, and recovery is
  local. It runs over UDP with a "Velocity Control" loop that samples the network dozens
  of times a second and trades quality against latency, plus progressive rendering
  (resolution improves in place while the network allows).
- It is a **multi-codec** system: for "constant, full-screen motion like playing a
  YouTube video" it switches to a traditional video codec, because tile coding without
  temporal prediction loses badly on motion. Astropad quotes under 16 ms end-to-end on a
  LAN.

Sources: Astropad's public Workbench product page, technology page and "Meet LIQUID"
post, and press coverage of the April 2026 launch (reviewed 2026-09-15).

### 2.2 RDP Graphics Pipeline (MS-RDPEGFX): the reference multi-codec design

RDP's EGFX pipeline is the most complete public specification of exactly this problem:

- **ClearCodec**, mandatory for every EGFX client: lossless, built for text. Three
  layers: a residual layer (RLE of pixel runs for few-colour areas), a bands layer with
  a **4000-entry dictionary of vertical pixel bars** that hits >80% for text once warm,
  and a subcodec for the rest. "Pixel-perfect text" at ratios competing with lossy H.264.
- **RemoteFX Progressive**: wavelet (DWT) tiles with quantisation tiers; the first pass
  is coarse, later passes refine tiles that stopped changing, and the decoder keeps
  persistent per-tile progressive state. Refinement is deferred under congestion.
- **AVC420 / AVC444**: H.264 for motion. 4:4:4 is achieved by sending two 4:2:0 streams
  (luma plus packed chroma) because phone decoders only do 4:2:0.
- Commands that cost almost nothing: `SolidFill`, `SurfaceToSurface` (moves, scrolling),
  `CacheToSurface` / `CacheImportOffer` (bitmap cache that survives sessions).
- Microsoft's own number: about 80% of a typical session is text and UI. Azure Virtual
  Desktop defaults to mixed mode (codec per region).

Sources: [MS-RDPEGFX progressive codec](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/1dcd953d-672b-457e-9dec-a6fe639bae8f),
[MS-RDPEGFX ClearCodec](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/6fa49bae-192f-4e25-888a-7cacfae303cf),
[IronRDP issue 1158, multi-codec pipeline design (Rust)](https://github.com/Devolutions/IronRDP/issues/1158),
[Lamco multi-codec whitepaper](https://lamco.ai/products/lamco-rdp-server/technology/multi-codec-pipeline/).

### 2.3 xpra, the practical heuristics

xpra's `auto` encoding picks per region: raw RGB with LZ4 for tiny updates, PNG/WebP
lossless for text, WebP lossy for pictures, and a video codec (h264/vp8/vp9) **only when a
region is updating fast enough**; small updates still bypass video. It merges adjacent
damage rectangles (`XPRA_MERGE_REGIONS`) and batches damage into windows whose length
grows with backlog. Source: [xpra Encodings.md](https://github.com/Xpra-org/xpra/blob/master/docs/Usage/Encodings.md).

### 2.4 Game streaming: loss recovery without keyframes

- Sunshine/Moonlight add Reed–Solomon parity per video frame (`fec_percentage`), and
  recover from unrecoverable loss with **Reference Frame Invalidation**: the client names
  the lost frame and the encoder predicts the next frame from an older, delivered one
  instead of sending an IDR. The Moonlight maintainers are moving to **long-term
  reference (LTR) frames** because with an 8-frame DPB the good references get evicted
  before the RFI request arrives; with LTR a 3-frame DPB (2 LTR + 1 short-term) gives
  "almost bulletproof" recovery, and H.264, HEVC, AV1, NVENC, VPL and AMF all support it.
- Apple's VideoToolbox has exactly this loop since WWDC21: `EnableLTR`, the encoder tags
  frames with `RequireLTRAcknowledgementToken`, the sender reports
  `AcknowledgedLTRTokens`, and `ForceLTRRefresh` produces a P-frame from the last
  acknowledged LTR (or a keyframe if none is acknowledged). Low-latency rate control
  mode is a separate session property.
- Parsec's BUD protocol is UDP + DTLS with predictive congestion control and no video
  buffering: it must detect congestion before it happens from RTT/loss trends.

Sources: [moonlight-common-c issue 120 (LTR over RFI)](https://github.com/moonlight-stream/moonlight-common-c/issues/120),
[WWDC21 low-latency VideoToolbox](https://developer.apple.com/videos/play/wwdc2021/10158/),
[Apple: encoding for low-latency conferencing](https://developer.apple.com/documentation/VideoToolbox/encoding-video-for-low-latency-conferencing),
[Parsec BUD](https://parsec.app/blog/a-networking-protocol-built-for-the-lowest-latency-interactive-game-streaming-1fd5a03a6007).

### 2.5 Codecs for the motion path

- **AV1 screen-content tools** (palette mode, intra block copy) are mandatory in every
  AV1 decoder and get screen content down to 100–500 kbps at 1080p; Google Meet runs
  AV1 down to 40 kbps. Hardware AV1 decode: iPhone 15 Pro and every iPhone 16/17, M3+
  Macs, Pixel 6+, Galaxy S21+; Android 12+ ships dav1d in software. Apple has **no**
  software AV1 decoder, so AV1 is a capability-negotiated upgrade, not the baseline.
- **HEVC** decodes in hardware on every supported iPhone/iPad and Mac, and encodes in
  hardware on every Apple Silicon Mac; Jump Desktop's Fluid 2.0 moved to HEVC and
  quotes ~50% less bandwidth than H.264. **H.264** is the universal fallback.
- Chroma: phone hardware decoders are 4:2:0 only. Coloured text through 4:2:0 looks
  smeared, which is why text must never go through the video path (2.2, 2.1).

Sources: [Visionular AV1 SCC](https://visionular.ai/av1-screen-content-coding/),
[Meta AV1 for RTC (June 2026)](https://engineering.fb.com/2026/06/22/video-engineering/adopting-av1-for-real-time-communication-rtc-meta/),
[Apple AV1 decode devices](https://olliewilliams.xyz/blog/apple-devices-av1-decoding/),
[Jump Fluid 2.0](https://changelog.jumpdesktop.com/jump-desktop-with-fluid-2.0-1yMoBG).

### 2.6 Mosh's State Synchronization Protocol, the model for "always converge"

Mosh does not ship a byte stream; both ends hold a screen state and the server sends
**diffs against the last state the client acknowledged**, as idempotent UDP datagrams
whose rate follows the RTT, with a heartbeat every 3 s and roaming by accepting a
higher sequence number from a new source address. Loss costs nothing but time: the next
diff supersedes the lost one. `decisions/mosh-lifecycle.md` rejects Mosh as a shipped
transport; it also names, as prerequisite 2 for any screen-state protocol, "a
transport-neutral session runtime so screen-state protocols do not masquerade as SSH byte
streams". Section 4.5 satisfies that by construction. Source: [mosh.org](https://mosh.org/).

### 2.7 Congestion control for interactive video

Delay-based controllers win here. GCC (WebRTC) is stable and loss tolerant but converges
slowly; Copa (Meta) fills the pipe ~3× better than GCC but can build seconds of queue;
SQP (Google, 2022) couples bandwidth probing to **frame-paced packet trains** with
one-way-delay feedback and beats GCC/Sprout/Vivace by 2–3× while keeping queueing delay
bounded. The cheap, robust takeaway: send each frame as a paced burst, measure the
burst's arrival spread and one-way-delay trend, and use that as the bandwidth estimate.
Sources: [SQP paper](https://arxiv.org/abs/2207.11857),
[Meta on Copa](https://engineering.fb.com/2019/11/17/video-engineering/copa/).

### 2.8 Transport: QUIC with relays and hole punching

iroh 1.0 (June 2026, Rust, MIT/Apache) is QUIC (its own quinn fork) with NAT traversal
inside the QUIC connection, a home-relay fallback that is end-to-end encrypted (the relay
cannot read anything), self-hostable relay binaries, and ~90% direct-connection success.
Its `Connection` exposes bidirectional/unidirectional streams, **unreliable datagrams**
(`send_datagram`, `max_datagram_size`), 0-RTT, and multipath/migration. The relay carries
QUIC over HTTPS, so it works where UDP is blocked. Sources: [docs.rs iroh
Connection](https://docs.rs/iroh/latest/iroh/endpoint/struct.Connection.html),
[iroh relays](https://docs.iroh.computer/concepts/relays),
[iroh 1.0 announcement](https://www.techtimes.com/articles/318490/20260616/peer-peer-library-iroh-10-ships-dial-devices-key-not-ip-address.htm).

WebRTC in Rust: `str0m` (sans-IO, sane) and `webrtc-rs` (heavy, callback-based). WebRTC
would give us a browser client but costs SDP/ICE/SRTP surface we do not need for
native-to-native. Rejected for v1; see 5.4.

### 2.9 Platform capture

- **macOS ScreenCaptureKit** (13+): per-frame attachments carry `status` (`.complete`,
  `.idle` when the display did not change, so skip it), `dirtyRects` (union of redrawn
  and moved rectangles), `contentRect`, `scaleFactor`. It can emit **420v (NV12)
  IOSurfaces directly**, which VideoToolbox consumes with no CPU copy. The
  `screencapturekit` crate is at 10.x (macOS 13+, MIT/Apache), exposes frame info
  including dirty rects and status, IOSurface/CVPixelBuffer access, and BGRA or 420v
  output. Note GPUI 0.2 already pulls `screencapturekit 0.2.8`, `windows-capture` and
  `ashpd` transitively through `zed-scap` (Zed's own screen-share capture); the versions
  differ, so they will coexist in the lock.
- **Windows Desktop Duplication (DXGI)**: `GetFrameDirtyRects` plus `GetFrameMoveRects`
  (source point + destination rect, exactly what scrolling needs), pointer shape and
  position delivered separately.
- **Linux**: PipeWire via the XDG ScreenCast portal on Wayland (damage varies wildly by
  compositor: Sway ~3.5 fps versus Hyprland ~21.7 fps in Lamco's measurements), X11
  via XDamage or polling. Linux gets the tile-hash differ fallback (4.2).
- Cross-platform crates: `pinray` 0.2 (MIT; SCK/DXGI+WGC/PipeWire, frames with stride,
  format, timestamp, explicit `Gap` events, but **no dirty rects**), `xcap` (screenshots,
  recording WIP). Neither exposes damage, so we write thin platform backends ourselves
  and keep the differ as the portable floor.

Sources: [SCFrameStatus.idle](https://developer.apple.com/documentation/screencapturekit/scframestatus/idle),
[SCStreamFrameInfo.dirtyRects](https://developer.apple.com/documentation/screencapturekit/scstreamframeinfo/dirtyrects?language=objc),
[Desktop Duplication API](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api),
[screencapturekit crate](https://docs.rs/crate/screencapturekit/latest),
[pinray](https://github.com/Itz-Agasta/pinray).

### 2.10 Screen-content classification

Text/graphics blocks are separable from photographic blocks with cheap per-block
statistics: distinct-colour count (text blocks concentrate on a handful of colours;
under 64 colours is a strong signal), gradient/edge density (text has many high-gradient
pixels), and a histogram with a few sharp peaks. Working at 16×16 or larger blocks is the
norm. This is enough to route tiles between lossless and lossy paths; no learned model.
Sources: [Image segmentation for lossless screen content
compression](https://arxiv.org/pdf/2305.05996), [enhanced colour palette modelling for
lossless SCC](https://arxiv.org/pdf/2312.14491).

---

## 3. What we already have (verified in the tree)

- **Trust and pairing.** Controller-v1: Noise XX (`clatter`), six-digit CPace code
  pairing, per-device capability bits, revocation epochs, single-writer lease
  (`decisions/controller-security-v1.md`). Capabilities are a closed set and unknown bits
  fail closed, so screens need an ADR amendment and new vectors (section 4.7).
- **Three routes** to a host: private LAN/VPN listener with Bonjour, Controller-over-SSH,
  self-hosted WebSocket relay that forwards ciphertext only. Every route serves the same
  session sources (`decisions/controller-session-sources.md`).
- **A macOS background listener** (`termirust controller-service`, LaunchAgent) that
  keeps the LAN route up when the app is closed.
- **Text terminals**: `ListSessions` / `Attach` (replay from a watermark) / `Input` /
  `Resize` over `crates/termirust-controller-listener/src/protocol.rs`, terminal frames up
  to 1 MiB, `controller_snapshot_bytes()` in `crates/termirust-desktop/src/terminal.rs`
  for mid-session attach, and a `vt100` emulator on the phone in
  `crates/termirust-mobile-ffi/src/terminal.rs`.
- **Mobile apps** with a Controller layer (`apps/ios/TermiRustMobile/Controller/*`:
  discovery, connection actor, retry policy, read-only attach, writer control) and an
  Android equivalent. No video decode anywhere yet.
- **A relay reconnect policy** (`crates/termirust-relay-client/src/reconnect.rs`:
  idempotent reads retry with jittered backoff, mutations report unknown completion).
- `crates/termirust-accessibility-macos` (semantic UI snapshots; not input injection).
- Workspace already locks `quinn 0.11.9` (transitive), `rav1e 0.8.1` (via `image`
  AVIF), `objc2` 0.6, `core-video`, `png`, `image`. No zstd, lz4, xxhash, FEC, or
  VideoToolbox bindings yet.

---

## 4. Design

### 4.1 Surfaces: text where we can, pixels where we must

A **surface** is one thing a client can subscribe to. Two kinds:

| Kind | Source | Wire representation | Cost |
|---|---|---|---|
| `TextSurface` | existing Controller sessions (desktop panes, durable, tmux) | existing `Output`/`Snapshot` byte stream, unchanged | ~100 B per changed line |
| `PixelSurface` | a display, the unified canvas of all displays, or one window | tile updates + optional video region (4.3) | see 4.8 |

The client composes them: the unified desktop canvas is a `PixelSurface`, and where a
TermiRust terminal pane sits on screen the host publishes a **pane placement** (pane
session id, rectangle on the canvas, cell size) so the client draws that rectangle from
the text stream and the host **masks those tiles out of the pixel path**. Result: a
desktop full of TermiRust terminals costs almost nothing over the pixel path, and the
phone renders terminal text at its own resolution, sharp, with its own font. When the
client lacks the pane's text stream (not attached, or a non-TermiRust terminal) the tiles
are sent as pixels like anything else.

The Devices dashboard subscribes to every paired host's canvas at the **thumbnail
profile** (longest side 320 px, 1 fps, lossy first pass only, ≤ 5 KB/s budget). Opening
a host switches the same subscription to the **interactive profile**.

### 4.2 Capture and damage (`termirust-screen-capture`, GPUI-free)

One trait, three backends, one portable fallback:

```rust
pub struct CapturedFrame {
    pub surface: SurfaceId,
    pub sequence: u64,
    pub timestamp: Instant,
    pub pixels: FrameBuffer,          // BGRA in system memory, or a platform GPU handle
    pub damage: Damage,               // Idle | Rects(Vec<Rect>) | Unknown
    pub moves: Vec<MoveRect>,         // DXGI only; empty elsewhere
    pub cursor: Option<CursorUpdate>, // position and, when it changed, shape
    pub scale: f32,
}
```

- **macOS**: ScreenCaptureKit, `minimumFrameInterval` set from the current target rate,
  `.idle` frames dropped before they reach the pipeline, `dirtyRects` → `Damage::Rects`.
  Measured on macOS 27.0 (26A428): no dirty rectangles were attached, so frames arrive with
  unknown damage and the differ hashes every tile (`engineering-evidence/RS2-screen-capture.md`).
  BGRA for the tile path; a second 420v stream is opened only while a video region is
  active (4.3), fed straight to VideoToolbox from the IOSurface.
- **Windows**: Desktop Duplication, dirty + move rects, separate pointer.
- **Linux**: PipeWire/portal; damage when the compositor gives it, otherwise `Unknown`.
- **Differ fallback** (`Damage::Unknown`, and always as a cross-check in debug builds):
  64×64 tile grid, xxh3-64 per tile, compare with the previous frame's hashes. This is
  also how we compute the **tile version map** every backend needs, so the differ runs on
  the dirty rects anyway; the fallback just widens it to the whole frame.
- **Scroll detection** where the OS gives no move rects: when many tiles in a column
  changed, test whether the new tile hashes match the previous frame's hashes at the
  vertical offsets seen in recent scroll wheel input (and ±1..±3 tiles); a match becomes
  a `Move`, which costs 16 bytes instead of re-sending the column. This is the single
  biggest saver for reading code and logs.

Capture stays at the host's native scale. Down-scaling for the client's viewport happens
in the codec stage per subscription, so two clients at different sizes share one capture.

### 4.3 Tile codec and per-tile mode selection (`termirust-screen-codec`, pure Rust, fuzzable)

Fixed 64×64 tiles on the surface grid (LIQUID's choice; 32×32 for the thumbnail profile).
For every dirty tile the encoder picks one **mode**; the client's framebuffer applies
them in order: moves, then tiles, then video region blits, then the cursor.

| Mode | When | Encoding | Typical bytes |
|---|---|---|---|
| `Skip` | hash unchanged | nothing | 0 |
| `Solid` | one colour | 4 B colour | 8 |
| `Cached` | hash in the client's tile cache | 8 B hash | 12 |
| `Move` | scroll/move detected | src point + dst rect | 16 |
| `Lossless` | ≤ 64 distinct colours, or high edge density (text/UI) | palette + indices, run-length, then deflate (`miniz_oxide`, already locked) | 300–3 000 |
| `Lossy` | photographic, ≥ 64 colours and low edge density | v1: half-resolution quantised pass, deflated; refinement upgrades it to exact pixels while idle. A real image codec replaces it only if the workload report shows it is needed | 500–4 000 first pass |
| `Video` | tile lies inside an active video region | owned by the video stream | amortised |

Rules that matter:

- Classification is per tile from colour count and gradient density (2.10), computed
  on the fly during hashing; no second pass.
- The **tile cache** is per client, LRU by content hash, size negotiated (phone 64 MB,
  desktop 256 MB). The host tracks what each client holds; a `Cached` reference to an
  evicted tile is answered by a `Missing` NACK and a full send. Window switching,
  tab switching and re-opened dialogs become near-free.
- **Progressive refinement** is a separate low-priority queue: a `Lossy` tile that has
  not changed for 250 ms is re-sent lossless when the rate controller has spare budget.
  The rate controller may starve this queue indefinitely; the screen stays usable, just
  softer.
- **Video region promotion**: a connected region of ≥ 12 tiles that has been damaged at
  ≥ 12 Hz for ≥ 300 ms becomes a video region (bounded to one per surface in v1).
  Demotion after 500 ms below 4 Hz, followed by one lossless pass over the region so
  text comes back sharp. This is LIQUID's and xpra's rule, made explicit.
- **Viewport priority**: a client sends its viewport (rect on the canvas, scale) with
  every input message and at least every 250 ms. Tiles outside the viewport plus a
  one-tile margin are sent only at thumbnail quality and only when idle; tiles inside
  get the full pipeline. Zooming in on the phone therefore raises quality where you look
  and nowhere else.

Codec output is a list of `TileOp`s per frame with `surface_generation`, `tile_index`,
`tile_version` on each. That triple is what makes the protocol idempotent (4.5).

### 4.4 Video path for motion

- Encoders behind one trait: VideoToolbox H.264/HEVC on macOS (via `objc2-video-toolbox`,
  low-latency rate control, `EnableLTR`, `ExpectedFrameRate`, `MaxKeyFrameInterval` set
  very high so keyframes only happen on demand), Media Foundation on Windows, VA-API on
  Linux, `openh264` as the software floor everywhere. AV1 (rav1e/SVT software, or
  hardware where present) only when both sides advertise it; its SCC tools make it the
  best codec for this content, but it is an upgrade, never required.
- Decoders on the phone are native (VideoToolbox on iOS, MediaCodec on Android) behind
  the FFI; on the desktop VideoToolbox / Media Foundation / dav1d.
- Frames go as **QUIC datagrams** with **per-frame FEC** (RaptorQ or Reed–Solomon
  parity, ratio 5–40% tracked to the measured loss rate). Each frame carries the LTR
  token when the encoder set one; the client acknowledges LTR frames over the control
  stream; on unrecoverable loss the client reports the lost frame and the host calls
  `ForceLTRRefresh`. No IDR unless there is no acknowledged LTR at all. Encoders without
  LTR use periodic intra refresh (a rolling stripe of intra blocks) instead of keyframes.
- Video regions are always 4:2:0. That is acceptable because text never enters them
  (demotion ends with a lossless pass).

### 4.5 Session state and convergence (the Mosh idea applied to tiles)

Each client subscription keeps, on the host, the **last state the client acknowledged**:
per tile, the highest `tile_version` the client has confirmed applied. Every `TileOp`
batch is a diff from that acknowledged state, not from what the host last sent.
Consequences:

- Tile batches travel on a reliable QUIC stream, so within one connection there is no
  loss to handle; but the acknowledged map is what makes **reconnect** cheap: after a
  QUIC migration, a relay switch, or a fresh connection with the same subscription id,
  the host resends only tiles whose version is above the acknowledged one. A phone that
  wakes from the lock screen does not receive a full-screen refresh; it receives what
  changed while it slept.
- If the acknowledged state is too old (surface generation changed: resolution change,
  display added, host restarted), the host sends a new generation and the client drops its
  framebuffer and cache.
- Rate follows the link: the host emits at most one batch per **send interval**, which
  starts at 16 ms and is raised toward 250 ms as the rate controller sees backlog or
  RTT growth, exactly Mosh's behaviour. Coalescing happens naturally: a tile that
  changed five times inside one interval is sent once.
- Cursor position, viewport, and "typing echo" hints are latest-wins datagrams; a lost
  one is superseded by the next.

### 4.6 Transport and rate control (`termirust-screen-transport`)

- **Stage A** carries the screen session over the existing authenticated Controller
  channel, as screen frames (kind 3) on the LAN, SSH and relay routes. A frame carries a
  chunk of `termirust-screen-protocol`'s byte stream, so a batch larger than one frame
  spans two, and each frame claims the capability its contents exercise: `ObserveScreens`
  for everything that is not input, `ControlPointer` or `ControlKeyboard` for input. The
  listener checks that claim against the device's current record on every frame, and the
  host checks it again against the message it decodes. See 5.3.
- **QUIC via iroh**, from Stage B, for the screen plane on every route. Streams: one control stream
  (bidirectional, ordered: subscribe, acks, input, capabilities); one unidirectional
  stream per priority class for tile batches (viewport tiles, off-viewport tiles,
  refinement) so a slow refinement stream never head-of-line-blocks the viewport;
  datagrams for video frames, FEC symbols, cursor, and viewport updates.
- **Connectivity**: LAN direct when the Bonjour/private address is reachable; otherwise
  iroh's hole punching; otherwise its relay. We run our own iroh relay next to the
  existing `termirust relay-host` (different protocol, same deployment guide), and the
  n0 public relays are an opt-in default for people who do not self-host. The relay is
  end-to-end blind. Migration keeps a session alive across Wi-Fi ↔ cellular.
- **Rate control**: QUIC's built-in controller (quinn ships BBR and Cubic; use BBR) is
  the safety net for the reliable streams. The application-level controller that sets
  the send interval, first-pass quality, video bitrate and FEC ratio uses the SQP
  approach: each batch goes out as a paced burst, the client reports arrival spread and
  one-way-delay trend per burst on the control stream, and the host's estimate is the
  harmonic mean of the last few burst rates minus a 15% headroom. Loss rate and RTT come
  from QUIC path stats for free.
- **Degradation ladder**, applied in order as the estimate falls and reversed as it
  recovers: stop refinement → drop off-viewport updates → raise the send interval → lower
  first-pass quality → send viewport tiles at half scale (client upsamples) → cap video
  at 300 kbps / 15 fps → video at 480p. The text terminal path is never degraded; it is
  the cheapest thing on the wire and the thing people need most.
- **Stage A** runs the same transport with the burst estimator and the video rungs
  absent: QUIC's controller (BBR) is the only bandwidth signal, the ladder is driven by
  stream backlog and RTT growth, and the send interval floor is 66 ms. That is
  deliberately simple and deliberately good enough for reading, typing and clicking,
  and it stays as the fallback whenever a peer lacks the Stage B message kinds.

### 4.7 Trust, permissions, privacy

- Screens ride on the **existing pairing**. New Controller capability bits
  `ObserveScreens` (5), `ControlPointer` (6), `ControlKeyboard` (7) are requested at
  pairing or granted later from Devices. Adding bits requires the ADR amendment
  procedure in `decisions/controller-security-v1.md`: update the ADR first, regenerate
  vectors, repin checksums. Screen viewing is **off by default** and separate from
  input, exactly like `SendInput` today.
- **Bootstrap**: the client asks for a screen ticket over the authenticated Controller
  channel (any route). The host answers with its iroh endpoint id, relay hint, a
  32-byte session secret derived from the Noise channel with HKDF and a fresh nonce, and
  the subscription's surface list. The client's first control-stream message proves the
  secret; the host pins the QUIC peer's endpoint id to the paired device. The QUIC
  connection is TLS 1.3 between those endpoint keys, so pixels are end-to-end encrypted
  and the Controller relay, the iroh relay, and SSH never see them.
- **Host-side visibility**: a persistent indicator in the desktop chrome while any
  screen subscription is live, with the device name, and a one-click "Stop sharing".
  Thumbnails on the Devices dashboard are opt-in per host ("Show a live preview to
  paired devices").
- **OS permissions**: macOS Screen Recording (TCC) for capture and Accessibility for
  input injection (CGEvent). The background `controller-service` binary needs its own
  TCC grant if it is to serve screens with the app closed; that is milestone 6 and a
  documented limitation until then. Windows: `SendInput`. Linux: the portal's
  RemoteDesktop interface.
- **Diagnostics** never store pixels, tile payloads, or window titles; only counts,
  bytes, rates and error codes, consistent with `docs/diagnostics.md`.
- **Input**: pointer moves coalesced to one per send interval, latest-wins; clicks,
  keys and scroll on the ordered control stream. Input requires the same writer lease
  the text path uses; observe-only devices cannot send it, in the protocol, not just the
  UI.

### 4.8 Bandwidth targets

Targets to design and test against, per interactive subscription at 1440×900 on the
client. They become measurements in milestone 1's bench and gates in section 6.

| Workload | Without this design (H.264 desktop stream) | Target |
|---|---|---|
| Idle desktop | 50–200 kbps (encoder floor) | < 1 kbps (keepalives) |
| Typing in an editor | 300–800 kbps | 10–40 KB/s |
| Scrolling a code file | 1–3 Mbps | 30–120 KB/s (moves + lossless new rows) |
| Switching windows | 1–4 Mbps burst | one burst ≤ 300 KB, then ~0 (cache hits on return) |
| Watching a video region (Stage B) | 1.5–4 Mbps | 300 kbps–2 Mbps, adaptive |
| Watching a video region (Stage A, lossy tiles at 8/s) | 1.5–4 Mbps | ≤ 600 kbps, visibly choppy |
| Devices thumbnail, per host | n/a | ≤ 5 KB/s |
| TermiRust terminal panes | as pixels | text path, ~100 B per changed line |

Latency targets (input to glass, P95, on the 100 ms / 1% / 5 Mbps profile): Stage A
under RTT + 150 ms; Stage B under RTT + 40 ms.

### 4.9 Seams between the stages

`termirust-screen-host` defines `MotionEncoder`, `RateController` and
`RefinementPolicy` traits and a `ScreenModules` struct of boxed implementations.
Stage A ships the tile-only implementations; Stage B adds the hardware encoders, the
burst estimator and the budgeted refinement policy as further implementations in the
same crate, selected at runtime from platform capability (encoder present, decoder
advertised by the peer). The seams exist for testability (a fake encoder and a scripted
rate controller drive the state machines in unit tests) and so the tile path stays
independently runnable on platforms without a hardware encoder, not for any packaging
split. The session header shows which path is active so a laggy session can be
explained.

---

## 5. Decisions and alternatives

### 5.1 Tiles first, video second (not a single video stream)

A whole-desktop H.264/HEVC stream (RustDesk, Jump, Parsec, Sunshine) is the simplest
build and the wrong fit: its bitrate floor on an idle screen is not zero, text goes
through 4:2:0 chroma, every loss event costs a keyframe or an LTR round trip for the
entire frame, and none of it benefits from the OS damage information. Every product
that optimises for text and low bandwidth (RDP, xpra, LIQUID) is tile-and-mode based
with video as a region-level special case. We follow them.

### 5.2 Text terminals never go through pixels

Already decided by the existing product; here it becomes the mask in 4.1. It is also the
answer to "why not just use Workbench": TermiRust knows which rectangles are terminals.

### 5.3 QUIC (iroh) for Stage B; Stage A rides the Controller channel

Controller-v1 is a reliable ordered stream on TCP, SSH, or WebSocket. Video and
latest-wins state on a reliable ordered stream stall on every loss (head-of-line
blocking), which is the failure mode the user described. QUIC gives independent streams,
unreliable datagrams, migration and 0-RTT in one connection; iroh adds NAT traversal and
blind relays. The Controller channel stays the trust root and the signalling path.

**Decided on 2026-09-16**: Stage A carries screen sessions over the Controller channel
itself, as a new screen frame kind (amendment 1 of `decisions/controller-security-v1.md`),
on the LAN, SSH and relay routes that already exist. The tile path is reliable and ordered
anyway, so the cost is head-of-line blocking under loss, which the acknowledged-state
resume already tolerates, and the gain is a working desktop-to-desktop path with no new
dependency and no device spike first. iroh arrives with Stage B, where datagrams and
migration earn their keep; `termirust-screen-protocol` stays transport-neutral so that
change is additive.

### 5.4 Not WebRTC, for now

WebRTC would give a browser client and battle-tested RTP/FEC/GCC. It also brings ICE,
SDP, DTLS-SRTP and a large dependency (`webrtc-rs`) or a sans-IO one we would have to
wire ourselves (`str0m`). The codec and session layers in 4.3–4.5 are transport-neutral;
a WebRTC data-channel transport can be added if a browser client is ever wanted.

### 5.5 Not Mosh, but Mosh's synchronisation idea

`decisions/mosh-lifecycle.md` stands. 4.5 is a screen-state protocol with its own
runtime, lifecycle and resize semantics, which is what that decision asked for.

### 5.6 Own capture backends, not a cross-platform crate

None of `pinray`, `xcap`, `scap` exposes damage rects, and damage is the whole point.
Backends are thin; the differ is the portable floor.

### 5.7 Everything open source, one repository

An earlier draft split the motion path into a paid module in a private repository. That
is withdrawn: the whole feature lives here under the workspace licence. The trait seams
in 4.9 are kept because they make the pipeline testable and keep the tile path runnable
without a hardware encoder, not because anything is packaged separately.

---

## 6. Milestones

Each milestone ends with tests in the same change, `cargo fmt`, `cargo check`, the
focused tests, and a short evidence note under `docs/engineering-evidence/`.

**M0 — Spikes (throwaway code, keep the numbers).** (1) ScreenCaptureKit: dirty-rect
count, idle ratio and bytes-per-second of the raw dirty area for three recorded workloads
(typing, scrolling, video). (2) VideoToolbox LTR round trip from Rust via
`objc2-video-toolbox` with forced loss. (3) iroh phone-to-Mac over cellular with a
self-hosted relay: connection time, direct vs relayed, migration on Wi-Fi off. Go/no-go
on iroh depends on (3).

**M1 — Codec core.** `termirust-screen-codec`: tile grid, xxh3 differ, classifier,
`Lossless` and `Lossy` tile coders, tile cache with host-side shadow, scroll detection,
video-region promotion state machine, progressive queue, `TileOp` serialisation as a hand-written bounded binary layout (no serde)
with golden vectors. Fuzz targets for every decoder. A **replayable bench**:
a fixture format that records captured frames plus damage (`tests/fixtures/screens/`,
a few hundred MB kept out of git via `git lfs` or generated synthetically), and a bench
that reports bytes per workload against the 4.8 targets. No network yet.

**M2 — Host and desktop viewer on a LAN (Stage A).** `termirust-screen-capture`
(macOS backend + differ), `termirust-screen-transport` (iroh, streams, acks, the
Stage A ladder), `termirust-screen-host` (capture → codec → transport, runs inside the
desktop app, with the `MotionEncoder` / `RateController` / `RefinementPolicy` traits and
their tile-only implementations), `termirust-screen-client` (framebuffer, cache), a GPUI
viewer in the desktop app (Devices → open computer). Desktop-to-desktop first because
both ends are Rust and the whole path is testable in one process with a loopback iroh
endpoint. Controller ADR amendment, new capability bits, ticket bootstrap, Devices
toggles and the sharing indicator land here.

**M3 — Phone client (Stage A).** FFI in `termirust-mobile-ffi` (subscribe, apply tile
ops, expose the framebuffer as a Metal/Vulkan texture, input, viewport), SwiftUI/Compose
viewer with pinch zoom, minimap, keyboard accessory row, pointer modes. Devices dashboard
with thumbnails. Terminal-pane masking and composition from the text stream. **Stage A
is shippable at the end of M3** for macOS hosts.

**M4 — Motion path (Stage B, part 1).** VideoToolbox encode with LTR, datagram framing,
FEC, the video, FEC and LTR message kinds in the protocol with golden vectors, native
decode on iOS and Android, region promotion/demotion end to end, the video rungs of the
degradation ladder, and the active-path indicator in the session header.

**M5 — Rate control, roaming and bad networks (Stage B, part 2).** The frame-paced burst
estimator, 16 ms interval, continuous refinement, viewport-priority budget,
roaming/multipath. Self-hosted iroh relay in the deployment docs, public relay opt-in,
reconnect diff-sync and the network matrix in section 7 run on real devices for both
stages; each has its own pass criteria (4.8).

**M6 — Windows and Linux hosts, background hosting.** DXGI and PipeWire backends,
input injection, software encoder floor, TCC for the macOS background service, Windows
scheduled-task equivalent.

---

## 7. Test strategy

- **Unit and golden**: every tile mode encodes and decodes to identical pixels
  (lossless) or within a PSNR floor (lossy first pass); cache eviction and `Missing`
  recovery; scroll detection on synthetic frames; promotion/demotion timing with a mock
  clock; acknowledged-state diffing after simulated reconnect.
- **Fuzz**: tile decoders, `TileOp` parser, control-stream messages, FEC decoder.
- **Bench** (M1, then a CI trend): bytes per workload from the replayable fixtures,
  encode time per frame, cache hit rate.
- **Network matrix** (M5), with `tc netem` on Linux or Network Link Conditioner on
  Apple devices: (RTT, loss, cap) ∈ {(20 ms, 0, ∞), (100 ms, 1%, 5 Mbps),
  (300 ms, 5%, 1 Mbps), (500 ms, 10%, 200 kbps)} × {typing, scrolling, video} ×
  {Stage A, Stage B}. Pass criteria for both: no stall > 1 s, converged screen within
  3 s after a 10 s outage. **Converged means the viewer is showing the current screen, with
  picture regions at their lossy first pass** (owner, 2026-09-18) — not every tile refined to
  exact pixels. The two differ by seconds on a slow link, and exactness is bounded by the link
  rather than by this codec: 200 kbps cannot make a 1280 x 800 screen exact any faster. A stall
  likewise means the screen stopped moving, not that it is imprecise; a playing video is never
  exact, because refinement only runs once a region goes idle. Stage A: input-to-glass P95 under RTT + 150 ms on the first
  three profiles. Stage B: P95 under RTT + 40 ms on the first three profiles, and no
  keyframe on the video path after a single lost datagram.
- **Capability negotiation**: a host never emits a video, FEC or LTR message kind to a
  peer that did not advertise it; a client that receives an unadvertised kind (hostile
  host) drops the session with a stable error code.
- **Security**: a device without `ObserveScreens` gets `capability_denied` on subscribe
  on every route; a stale epoch is refused; a client with a wrong session secret is
  dropped before any surface data; the relay test asserts the relay process never sees a
  plaintext tile signature; diagnostics bundle contains no pixel data.
- **Privacy UI**: sharing indicator visible for the entire life of a subscription in the
  UI audit inventory under `tests/ui/`.

---

## 8. Gates (all must pass before the feature is on by default)

1. M1 bench within the 4.8 targets for typing, scrolling and window switching.
2. Network matrix pass criteria met on a real iPhone over cellular and on Wi-Fi with
   the conditioner, for the stage being released (Stage A at M3, Stage B at M5).
2a. Stage A is judged usable on its own: a 30-minute session of editing and reading over
   the 300 ms / 5% / 1 Mbps profile with no complaint other than speed.
3. ADR amendment for capability bits accepted with regenerated vectors and checksums.
4. Independent review note for the ticket bootstrap and endpoint pinning (the same
   D06-style gate the Controller carries; screen data is more sensitive than terminal
   text, not less).
5. New dependencies recorded with licence and reason: `iroh` (MIT/Apache), `screencapturekit`
   10.x (MIT/Apache), `objc2-video-toolbox` (MIT), `xxhash-rust`
   (BSL-1.0), `raptorq` (Apache-2.0), `openh264` (BSD-2 with Cisco's patent grant when
   using their binary), `windows-capture`/`ashpd` (already
   locked).
   As built at M6, the capture backends took: on Windows, `windows` 0.61 (MIT/Apache) — already
   in the lock file, so nothing new entered the tree; on Linux, `ashpd` 0.13 (MIT, already locked)
   with its `screencast` feature, `async-io` 2 (MIT/Apache, already locked), and `pipewire` 0.8
   (MIT), which is new and brings `libspa`, `pipewire-sys`, `libspa-sys` (all MIT),
   `cookie-factory` (MIT), `nix` (MIT), and a build-time tree of `system-deps` (MIT/Apache) and
   `bindgen` (BSD-3-Clause). All permissive; `bindgen` and `system-deps` are build-time only.
   `pipewire-sys` links the system `libpipewire-0.3` (MIT), which is not vendored — a Linux build
   needs `libpipewire-0.3-dev` present, and `windows-capture` was **not** taken, because it wraps
   Windows.Graphics.Capture, which reports no damage.
6. `cargo test --workspace --all-targets --locked` and `cargo deny check` green.

---

## 9. Risks and open questions

- **iroh relay economics.** n0's public relays rate-limit; sustained screen streaming
  through them is not a product we can promise. Self-hosted relay is the supported path;
  public relays are best effort. Decide before M5 whether to bundle an iroh relay into the
  existing `termirust relay-host` deployment story.
- **TCC for the background service.** macOS grants Screen Recording per binary; the
  LaunchAgent will need its own approval and Apple may prompt again after updates.
  Until M6, screens are served only while the app runs.
- **Wayland damage quality.** Some compositors deliver few frames or no damage; Linux
  hosts rely on the differ and will use more CPU. Acceptable for v1.
- **Battery on the phone.** Continuous thumbnails for many hosts add up; thumbnails
  pause when the Devices screen is not visible, and the dashboard polls at 1 fps at most.
- **Intel Macs.** VideoToolbox HEVC encode is slower and LTR support may be absent;
  H.264 with intra refresh is the floor there, as Astropad also caveats.
- **Codec licensing.** H.264/HEVC hardware encoders on Apple devices are licensed by the
  platform; the `openh264` software fallback must use Cisco's prebuilt binary to be
  covered by their patent grant, or be omitted from builds that cannot ship it. AV1 has no
  royalty.
- **Scope creep toward collaboration.** One viewer with the writer lease, others
  observe. Multi-cursor, annotation and audio are explicitly later.
