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

All 24 cells pass both criteria. Worst stall **733 ms** against a 1,000 ms bound; worst catch-up
**1,099 ms** against a 3,000 ms bound.

| Profile | Workload | Stage | kbps | Longest stall ms | Caught up ms | Exact ms |
|---|---|---|---:|---:|---:|---:|
| 20 ms, 0%, uncapped | typing | A/B | 9 | 133 | 33 | 99 |
| 20 ms, 0%, uncapped | scrolling | A/B | 25 | 66 | 33 | 99 |
| 20 ms, 0%, uncapped | video | A | 707 | 166 | 99 | 633 |
| 20 ms, 0%, uncapped | video | B | 3,339 | 99 | 133 | 866 |
| 100 ms, 1%, 5 Mbps | typing | A/B | 9 | 166 | 33 | 199 |
| 100 ms, 1%, 5 Mbps | scrolling | A/B | 25 | 99 | 33 | 199 |
| 100 ms, 1%, 5 Mbps | video | A | 669 | 166 | 266 | 833 |
| 100 ms, 1%, 5 Mbps | video | B | 695 | 99 | 199 | 833 |
| 300 ms, 5%, 1 Mbps | typing | A/B | 8 | 266 | 33 | 399 |
| 300 ms, 5%, 1 Mbps | scrolling | A/B | 22 | 199 | 399 | 399 |
| 300 ms, 5%, 1 Mbps | video | A | 505 | 366 | 33 | 2,033 |
| 300 ms, 5%, 1 Mbps | video | B | 609 | 366 | 499 | 1,899 |
| 500 ms, 10%, 200 kbps | typing | A/B | 5 | 366 | 33 | 599 |
| 500 ms, 10%, 200 kbps | scrolling | A/B | 20 | 299 | 33 | 733 |
| 500 ms, 10%, 200 kbps | video | A/B | 245 | 733 | 1,099 | 3,833 |

Typing and scrolling measure identically on both stages, which is right: neither promotes a motion
region, so there is no video path to differ on.

## Three definitions the criteria needed, and why

**A stall is the screen not moving, not the screen being imprecise.** The first version measured
"the viewer's pixels are not exactly the host's", and reported every video run as one five-second
stall on a perfect link. A picture region is sent as a lossy first pass and only becomes exact once
it goes idle and refinement catches up, so a playing video is *never* exact. That measurement was
describing the codec working as designed and calling it a failure.

**"Converged" is ambiguous in the plan, so both readings are reported.** "Caught up" is the viewer
showing the current screen, allowing the lossy first pass — which is what a person means. "Exact" is
every picture tile refined to exact pixels. They differ by seconds on a slow link: on the worst
profile a video screen catches up in 1.1 s and becomes exact at 3.8 s. The 3 s bound is asserted
against the first reading. **If the owner means the strict one, the worst profile fails it**, and
that is a decision about the criterion rather than a defect: refinement is bounded by the link, and
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

- **The stage comparison is not trustworthy from this harness, and the video rows should not be
  read as a Stage B verdict.** The harness drives `HostSession` directly, so the ladder and the
  rate estimator are not in the loop: nothing here adapts to the link. Stage B's 3,339 kbps on an
  uncapped link is the motion encoder's unregulated default, not what a real session sends — a real
  one runs under `ScreenHost`, which owns both. What Stage B costs when it is being governed is
  [RS5](RS5-motion-path.md), and what the ladder is worth is [RS6](RS6-ladder.md). Putting this
  matrix on `ScreenHost` is the obvious next step and would make the video rows mean something.
- On the slowest profile both stages measure identically, which suggests the motion region never
  promoted there. That may be real — at 200 kbps the send queue backs up and the host encodes fewer
  frames, so the tracker never sees sustained change — or an artefact of this queue model. It is
  not worth a conclusion until the harness is on `ScreenHost`.
- Input-to-glass P95 and real phones: device work, unchanged.
- Synthetic content, as everywhere else in this plan.
