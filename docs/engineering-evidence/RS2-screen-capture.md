# RS2 Remote Screens Capture Evidence

Date: 2026-09-15

## Outcome

`crates/termirust-screen-capture` delivers captured frames to the codec (steps 2.3 and 2.4 of
[remote-screens-todo.md](../remote-screens-todo.md)):

- `FrameSource` yields opaque BGRA frames with damage (`Rects` or `Unknown`), a timestamp, and
  the reported scale. Unchanged frames are not delivered.
- The macOS backend uses ScreenCaptureKit (`screencapturekit 10.0.3`). Idle, blank, and
  suspended frames are skipped; alpha is forced opaque; a frame dropped because the consumer fell
  behind makes the next frame's damage `Unknown`, so no change is lost.
- `ReplaySource` replays frames for tests; `downscale` averages blocks for Devices previews.
- The crate's build script adds `/usr/lib/swift` to the loader path, which binaries that link the
  ScreenCaptureKit Swift bridge need. The desktop app will need the same when it links capture.

## Automated evidence

```text
cargo test -p termirust-screen-capture
PASS: 6 tests, 0 failed

cargo clippy -p termirust-screen-capture --all-targets -- -D warnings
PASS

cargo deny check licenses
PASS

./scripts/verify/controller-security-vectors.sh --check
PASS: lockfile and ADR checksums repinned for the new packages
```

## Spike 0.2: real capture on this Mac

`cargo run -p termirust-screen-capture --release --example capture_stats -- <seconds> <scale>`
on macOS 27.0 (26A428), MacBook display of 1512 × 982 points, with Screen Recording allowed for
the terminal. The screen showed this development session, which updates continuously.

| Scale | Frame size | Seconds | Frames delivered | Dirty rects attached | Tiles changed per frame | First frame | After first frame |
|---:|---|---:|---:|---|---:|---:|---:|
| 1.0 | 1512 × 982 | 6.0 | 115 (19.1/s) | none | 5.24 % | — | 185.1 KB/s (incl. first) |
| 2.0 | 3024 × 1964 | 10.0 | 154 (15.4/s) | none | 0.22 % | 525.3 KB | 32.8 KB/s |

Findings:

- **ScreenCaptureKit attached no dirty rectangles** on this macOS build, at either scale. Every
  frame therefore carries `Damage::Unknown` and the codec hashes all tiles, which is the planned
  fallback. At native scale that is about 24 MB of pixels hashed per frame.
- **Native scale is cheaper per change than 1×.** Scaling to points smears one-pixel changes over
  more tiles (5.24 % against 0.22 % of tiles per frame).
- **Steady-state cost with a busy terminal on screen is 32.8 KB/s** at full Retina detail, inside
  the typing target of 10–40 KB/s. The first full frame is 525 KB.

## Open items

- Re-check dirty rectangles on a released macOS; if they stay absent, measure hashing CPU at
  native scale and consider hashing only on a changed-frame signal.
- The host decides the capture scale: the frame's reported scale is available on the first frame.
