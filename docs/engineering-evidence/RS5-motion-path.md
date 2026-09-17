# RS5 What the Stage B motion path costs

Date: 2026-09-17

## The question

Section 4.8 of the plan sets a target for "watching a video region" — 300 kbps to 2 Mbps,
adaptive — and nothing had ever measured it, because until M4 there was no motion path to measure.
`termirust-screen-codec`'s `workload_report` covers the tile path; this covers the other half.

Run it with `cargo run -p termirust-screen-host --release --example motion_report`. It drives a
real `HostSession`, a real `MotionSender` and this Mac's hardware HEVC encoder over a synthetic
desktop: a static document, a clock, and an 896 × 512 window of sliding bands standing in for a
video.

## The answer

| Path | Tiles kbps | Video kbps | Parity kbps | Total kbps | Target |
|---|---:|---:|---:|---:|---|
| Stage A, tiles only | 707 | — | — | 707 | ≤ 600 kbps, visibly choppy |
| Stage B, motion path | 14 | 147 | 42 | 202 | 300 kbps – 2 Mbps, adaptive |

Stage B is **3.5× cheaper** than the tile path on the workload the motion path exists for, and
lands inside its target. Parity adds 28% on top of the video, which is the ratio the policy picks
for a link that has reported no loss — the floor, not a reaction to anything.

The first second is excluded from both. A session opens by sending the whole screen once, and on a
1440 × 900 desktop that one frame is large enough to swamp a short run and make every workload
measure roughly the same. That is how a benchmark can be precise and meaningless at once.

Stage A is now **over** its ≤ 600 kbps target on this workload. That figure is honest rather than
new: it is what the tile path has always cost for a window of this size, and it only became
visible once the region was tracked correctly (below). The synthetic content — every row of the
window changing every frame — is also harsher than most real video.

## Two bugs the measurement found, which review had not

Both were found by the numbers refusing to move, not by reading the code.

**The motion region never grew.** A region is promoted the moment enough of it is hot, which is
before all of it is: a video that has just started has only warmed the rows it has drawn.
`MotionTracker::observe` then only ever checked for demotion, so the region kept whatever extent it
had at that instant. Here the window was 896 × 512 and the promoted region was 896 × **256** —
exactly half — for as long as it played. The other half went as tiles at full rate, which measured
as 8,400 tile operations inside the video window over 150 batches, about 180 kbps of pure waste. A
region may now grow, and only grow: shrinking while playing would move the boundary back and forth
and re-key the encoder each time.

**The region was sent twice.** Once promoted, the tile path throttles the region to 8 Hz rather
than stopping, which is right while nothing else is carrying it — a viewer with no decoder still
sees a moving picture. But once the motion path *is* carrying it, those tiles are pixels paid for
twice. The encoder now takes `set_motion_carried`, and the host turns it on **only when the viewer
has acknowledged a video frame**. Acknowledgement is proof the decoder started; assuming it would
leave a viewer whose decoder never ran looking at a frozen rectangle with the one path that could
have fixed it switched off.

Together these took the tile path from 330 kbps to 14 kbps while the video plays, and tile
operations inside the window from 8,400 to zero.

Growth had a bug of its own, which the existing demotion test caught within a minute: taking the
largest hot component anywhere on screen let a clock ticking in the far corner be absorbed, and a
region stretched across the screen to reach it never went quiet enough to demote. Only tiles
touching the region may join it.

## What this does not answer

- Real captured pixels. The content is synthetic, so the byte counts are indicative.
- Latency. This measures what goes on the wire, not when it arrives; the 4.8 latency targets need
  a real network.
- The degradation ladder under a squeeze. On a 320 kbps link it settles at `NoRefinement` and the
  workload does not come down, because on this content refinement was never a significant cost and
  the viewport covers the window. A workload that the ladder can actually shrink is its own test.
- Any machine but this one. The encoder is Apple's; Windows and Linux have neither.
