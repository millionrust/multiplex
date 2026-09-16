# RS4 VideoToolbox and long-term references (spike 0.3)

Date: 2026-09-16

## The question

Stage B's motion path only pays for itself if losing a packet does not cost a keyframe. A keyframe
of a 4K screen is the spike in the bandwidth graph the motion path exists to remove, so before
building M4 the plan asks: can a Rust host drive Apple's low-latency HEVC encoder, and does
`ForceLTRRefresh` really recover from loss with a P-frame predicted from an older acknowledged
reference?

## The answer

Yes, on this Mac, and the saving is large.

| | 1920 × 1080 | 3840 × 2160 |
|---|---|---|
| Recovery with a long-term reference | **1,141 bytes** | **4,217 bytes** |
| The same recovery as a keyframe | 154,116 bytes | 687,913 bytes |
| Saving | **135×** | **163×** |

Steady state, for scale: at 1080p the first keyframe is 154 KB and every later frame averages
1.7 KB (worst 8.6 KB); at 4K the first keyframe is 356 KB and later frames average 7.3 KB
(worst 34 KB). Synthetic screen content — a window, rows of text, a caret that moves each frame.

Run it with
`cargo run --release -- 60 1920 1080` in `tools/videotoolbox-spike`, an excluded workspace so the
spike adds nothing to the workspace lockfile.

## What the spike does

1. Creates an HEVC compression session asking for the hardware encoder **and low-latency rate
   control in the encoder specification**.
2. Turns on `EnableLTR` as a session property, so the encoder tags frames with acknowledgement
   tokens.
3. Encodes 60 frames, acknowledging every token as a healthy receiver would.
4. Stops acknowledging two thirds of the way through — what a viewer that lost packets looks like.
5. Two frames later asks for `ForceLTRRefresh`, and separately re-runs the whole sequence asking
   for `ForceKeyFrame` instead, so the two recovery costs sit side by side.

## Three things that would have cost M4 a day each

- **Low latency is an encoder specification, not a session property.** Setting
  `EnableLowLatencyRateControl` with `VTSessionSetProperty` after creation is refused, and
  `EnableLTR` is then refused too, because long-term references are only offered by the
  low-latency encoder. The first run of this spike reported "long-term references REFUSED" and
  still printed a plausible-looking recovery number, which is exactly how a spike talks you into
  the wrong design.
- **VideoToolbox key strings do not all match their symbol names, and a wrong key is silently
  ignored rather than refused.** `kVTEncodeFrameOptionKey_ForceKeyFrame` is `"EncoderForceKeyframe"`,
  not `"ForceKeyFrame"`. Spelling it out by hand produced a comparison run with no forced keyframe
  in it at all, and a confident "1× saving". Every key the spike uses is now the linked constant.
  The LTR keys do match their names, but that is luck, not a rule.
- **The encoder drops frames, so output is not one-to-one with submission.** At 4K it emitted 56
  of 60 frames, which moved the forced keyframe from output index 42 to 38. M4 must match
  acknowledgement tokens to frames **by token**, never by counting frames.

## What this does not answer

- Decode. The spike encodes only; decoding on the desktop, iOS and Android is 4.4.
- Real captured pixels. The frames are synthetic screen content, not ScreenCaptureKit output, so
  the byte counts are indicative rather than a measurement of the real motion path.
- Whether the same loop exists on Windows and Linux encoders (M6), or on the phones as decoders.
- Encode latency under load. The slowest single encode here is 36–93 ms, but that includes session
  warm-up on the first frame and a submission loop with no pacing, so it is not a latency figure.

## Verdict

M4 can be built on `EnableLTR` plus `ForceLTRRefresh`. Recovery costs about a hundredth of a
keyframe, which is what the motion path needs to be worth having.
