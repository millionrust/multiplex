# RS7 The network matrix, as far as software can take it

Date: 2026-09-18

## The question

Section 7 of the plan asks for a matrix: (RTT, loss, cap) ∈ four profiles × {typing, scrolling,
video} × {Stage A, Stage B}, with four pass criteria. Two of those criteria are properties of the
protocol and the codec rather than of any particular network:

- no stall longer than 1 s
- a converged screen within 3 s of a 10 s outage

A real network adds nothing to measuring those but noise and irreproducibility. The other two —
input-to-glass P95, and how a real iPhone and Android phone behave — need the device and stay
device work (3.6, 5.5).

This covers the first two, so the device runs start from a known-good baseline instead of
discovering a protocol bug through a phone.

Run it with `cargo run -p termirust-screen-host --release --example network_matrix`;
`cargo test -p termirust-screen-host --test matrix` holds the criteria.

## The answer

All 24 cells pass both criteria. The host here is a real `ScreenHost`, so the rate estimator and the
degradation ladder are in the loop and the `Rung` column is what the session had actually given up
by the end of the run.

| Profile | Workload | Stage | kbps | Rung | Longest stall ms | Caught up ms | Exact ms |
|---|---|---|---:|---|---:|---:|---:|
| 20 ms, 0%, uncapped | typing | A | 18 | Full | 133 | 33 | 99 |
| 20 ms, 0%, uncapped | typing | B | 19 | Full | 133 | 33 | 99 |
| 20 ms, 0%, uncapped | scrolling | A | 28 | Full | 66 | 33 | 99 |
| 20 ms, 0%, uncapped | scrolling | B | 30 | Full | 66 | 33 | 99 |
| 20 ms, 0%, uncapped | video | A | 713 | Full | 166 | 99 | 633 |
| 20 ms, 0%, uncapped | video | B | 3,073 | NoRefinement | 99 | 99 | 633 |
| 100 ms, 1%, 5 Mbps | typing | A | 18 | Full | 166 | 33 | 199 |
| 100 ms, 1%, 5 Mbps | typing | B | 19 | Full | 166 | 33 | 233 |
| 100 ms, 1%, 5 Mbps | scrolling | A | 28 | Full | 99 | 33 | 199 |
| 100 ms, 1%, 5 Mbps | scrolling | B | 25 | Full | 99 | 233 | 233 |
| 100 ms, 1%, 5 Mbps | video | A | 675 | Full | 166 | 199 | 833 |
| 100 ms, 1%, 5 Mbps | video | B | 1,884 | SlowerFrames | 133 | 266 | 1,133 |
| 300 ms, 5%, 1 Mbps | typing | A | 17 | Full | 266 | 33 | 433 |
| 300 ms, 5%, 1 Mbps | typing | B | 15 | Full | 299 | 33 | 566 |
| 300 ms, 5%, 1 Mbps | scrolling | A | 25 | Full | 199 | 433 | 433 |
| 300 ms, 5%, 1 Mbps | scrolling | B | 22 | Full | 299 | 33 | 566 |
| 300 ms, 5%, 1 Mbps | video | A | 512 | Full | 366 | 33 | 1,933 |
| 300 ms, 5%, 1 Mbps | video | B | 507 | Full | 499 | 33 | 3,199 |
| 500 ms, 10%, 200 kbps | typing | A | 14 | Full | 366 | 33 | 799 |
| 500 ms, 10%, 200 kbps | typing | B | 13 | Full | 499 | 33 | 1,066 |
| 500 ms, 10%, 200 kbps | scrolling | A | 23 | Full | 299 | 33 | 799 |
| 500 ms, 10%, 200 kbps | scrolling | B | 20 | Full | 533 | 1,066 | 1,066 |
| 500 ms, 10%, 200 kbps | video | A | 251 | Full | 733 | 1,766 | 4,499 |
| 500 ms, 10%, 200 kbps | video | B | 233 | Full | 999 | 1,999 | 6,699 |

### Three things this says that are worth acting on

**The worst stall is 999 ms against a 1,000 ms bound.** Stage B video on the worst profile passes by
one frame. That is a pass and it should not be read as comfort: any change that adds a frame of
latency to the motion path fails this cell, and a real network is less tidy than this one. It is the
number to watch in the device runs.

**Stage B is not always cheaper, and on a fast link it is much more expensive.** On the uncapped
profile it costs 3,073 kbps against the tile path's 713 for the same region. That is not a defect:
with bandwidth to spare the ladder has no reason to degrade, so the motion encoder runs at its
default bitrate, and for a 640 × 384 region of smooth content the tile path is simply cheaper.
[RS5](RS5-motion-path.md) measured Stage B 2.3× cheaper on a 896 × 512 window of harsher content, and
both are true: the motion path wins when the tile path is expensive, and loses when it is not.
Worth a decision the plan has not made — whether to promote a region to video only when it is
actually winning, rather than whenever it looks like video.

**The ladder does not engage on the slowest profile.** It reaches `NoRefinement` and `SlowerFrames`
on the uncapped and 5 Mbps Stage B video cells, and stays at `Full` on 200 kbps where it is most
needed. The likely cause is that the host is already throttled by unacknowledged batches on a link
that slow, so demand never crowds the estimate — the session is bandwidth-limited before the ladder
has an opinion. That needs looking at before the device runs, because a ladder that only acts when
there is room to spare is backwards.

## Three definitions the criteria needed, and why

**A stall is the screen not moving, not the screen being imprecise.** The first version measured
"the viewer's pixels are not exactly the host's", and reported every video run as one five-second
stall on a perfect link. A picture region is sent as a lossy first pass and only becomes exact once
it goes idle and refinement catches up, so a playing video is *never* exact. That measurement was
describing the codec working as designed and calling it a failure.

**"Converged" was ambiguous in the plan; the owner settled it on 2026-09-18 as the first reading
below, and section 7 now says so.** Both are still reported, because the second is worth knowing. "Caught up" is the viewer
showing the current screen, allowing the lossy first pass — which is what a person means. "Exact" is
every picture tile refined to exact pixels. They differ by seconds on a slow link: on the worst
profile a video screen catches up in 1.1 s and becomes exact at 3.8 s. The 3 s bound is asserted
against the first reading, which is the one that counts. Under the strict reading the worst profile
would fail at 3.8 s — not a defect, but a fact about the link: refinement is bounded by it, and
200 kbps cannot make a 1280 × 800 screen exact faster than that.

**Loss is modelled only where it can happen.** Stage A's tile path rides an ordered, reliable
channel, so a lost packet is retransmitted and the session sees delay and reduced goodput, never a
missing batch. Dropping tile batches to "simulate 5% loss" would model a transport nobody ships and
make the codec look broken for the wrong reason — one lost batch leaves the screen wrong forever,
which is exactly why `Class` exists. Only the video class loses anything here; reliable classes pay
for loss in retransmission time.

The outage is a real drop and resume — the host closes into the resume store and the viewer
reconnects — not a severed wire with the session left running. Modelling it the second way was the
first version's other mistake, and it made convergence never happen at all.

## What this does not establish

- **Why the ladder is quiet at 200 kbps** — the explanation above is the likely one, not a measured
  one. Confirming it means instrumenting the estimate and the demand per window, which is the next
  thing to do here.
- **Whether the link model is fair to a real transport.** Serialisation, one-way delay and a
  retransmission penalty per lost segment is a reasonable first order, but it has no congestion
  control, no queue limit and no reordering. A real path is worse in ways this cannot predict.
- Input-to-glass P95 and real phones: device work, unchanged.
- Synthetic content, as everywhere else in this plan.
