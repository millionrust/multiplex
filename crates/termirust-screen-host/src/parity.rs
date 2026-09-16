//! Choosing how much parity to send, and building it.
//!
//! A long-term reference recovers from loss, but only after the viewer has noticed and said so,
//! which costs a round trip and shows as a stall. Parity gets ahead of that: a few extra shards
//! with each group of frames, and a viewer that loses one rebuilds it without asking.
//!
//! Parity that is never needed is wasted bandwidth, so the ratio follows the loss the viewer
//! actually reports, between the plan's five and forty percent. Both ends of that are deliberate:
//! never zero, because the first loss on an idle link is the one worth covering, and never more
//! than forty, because past that the repair costs more than the retransmission it saves.

use termirust_screen_protocol::{
    Fec, MAX_PARITY_BYTES, Message, Parity, ProtocolError, VideoFrame, pack_shard, shard_length,
};

/// Video frames per group. Small enough that a group completes within a few frames of capture —
/// waiting for a group is waiting for repair — and large enough that one parity shard covers
/// several frames.
pub const GROUP_FRAMES: usize = 4;
/// The least parity that is ever sent, as a fraction of the group.
const MINIMUM_RATIO: f32 = 0.05;
/// The most, past which retransmission would be cheaper.
const MAXIMUM_RATIO: f32 = 0.40;
/// How fast the measured rate follows what the viewer reports. Low, because one lost frame on an
/// otherwise clean link should not double the bandwidth.
const SMOOTHING: f32 = 0.2;

/// Tracks reported loss and turns it into a shard count.
#[derive(Clone, Copy, Debug)]
pub struct ParityPolicy {
    /// Frames per group.
    group: usize,
    /// The share of frames recently reported lost, smoothed.
    observed: f32,
    /// Losses reported since the last group closed.
    reported: usize,
}

impl Default for ParityPolicy {
    fn default() -> Self {
        Self::new(GROUP_FRAMES)
    }
}

impl ParityPolicy {
    pub const fn new(group: usize) -> Self {
        Self {
            group,
            observed: 0.0,
            reported: 0,
        }
    }

    pub const fn group(&self) -> usize {
        self.group
    }

    /// The share of frames recently lost, as the policy currently believes it.
    pub const fn observed_loss(&self) -> f32 {
        self.observed
    }

    /// The viewer could not rebuild something.
    pub const fn lost(&mut self) {
        self.reported += 1;
    }

    /// Folds what was reported during the group that just closed into the measured rate, and says
    /// how many parity shards the next one gets.
    pub fn close_group(&mut self) -> usize {
        let share = (self.reported as f32 / self.group as f32).min(1.0);
        self.observed += SMOOTHING * (share - self.observed);
        self.reported = 0;
        self.shards()
    }

    /// Parity shards for a group at the current measured rate. Never zero: on a link that has not
    /// lost anything yet, the first loss is exactly the one worth covering.
    pub fn shards(&self) -> usize {
        // Twice the loss rate, because a group needs cover for more than the average when loss
        // arrives in bursts, which on a real network it does.
        let ratio = (MINIMUM_RATIO + 2.0 * self.observed).clamp(MINIMUM_RATIO, MAXIMUM_RATIO);
        ((self.group as f32 * ratio).ceil() as usize).max(1)
    }
}

/// Builds the parity messages for one closed group of video frames.
///
/// The shards are the **encoded messages**, not the media payloads, so a rebuilt shard carries
/// its own sequence, keyframe flag and reference token. A viewer that repairs a group ends up
/// with exactly the message that was sent, and nothing has to be guessed.
///
/// Returns nothing when the group cannot be covered: a group whose largest frame is near the
/// message limit would need a parity shard past it. Those are keyframes, which are rare, and the
/// reference path still covers them.
pub fn parity_for(
    surface: u32,
    group: u64,
    frames: &[VideoFrame],
    shards: usize,
) -> Result<Vec<Parity>, ProtocolError> {
    if frames.is_empty() || shards == 0 {
        return Ok(Vec::new());
    }
    let encoded: Vec<Vec<u8>> = frames
        .iter()
        .map(|frame| Message::VideoFrame(frame.clone()).encode())
        .collect::<Result<_, _>>()?;
    let length = shard_length(&encoded);
    if length > MAX_PARITY_BYTES {
        return Ok(Vec::new());
    }
    let packed: Vec<Vec<u8>> = encoded
        .iter()
        .map(|message| pack_shard(message, length))
        .collect::<Result<_, _>>()?;
    let fec = Fec::new(frames.len(), shards)?;
    Ok(fec
        .encode(&packed)?
        .into_iter()
        .enumerate()
        .map(|(index, payload)| Parity {
            surface,
            group,
            index: index as u8,
            data_shards: frames.len() as u8,
            payload,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(sequence: u64, bytes: usize) -> VideoFrame {
        VideoFrame {
            surface: 1,
            sequence,
            keyframe: sequence == 1,
            token: Some(sequence as u32),
            payload: vec![(sequence % 251) as u8; bytes],
        }
    }

    #[test]
    fn a_clean_link_still_gets_one_shard() {
        let mut policy = ParityPolicy::default();
        assert_eq!(policy.shards(), 1);
        for _ in 0..20 {
            assert_eq!(policy.close_group(), 1);
        }
        assert_eq!(policy.observed_loss(), 0.0);
    }

    #[test]
    fn sustained_loss_raises_the_ratio_to_its_ceiling_and_no_further() {
        let mut policy = ParityPolicy::default();
        for _ in 0..200 {
            for _ in 0..GROUP_FRAMES {
                policy.lost();
            }
            policy.close_group();
        }
        assert!(policy.observed_loss() > 0.9);
        assert_eq!(
            policy.shards(),
            (GROUP_FRAMES as f32 * MAXIMUM_RATIO).ceil() as usize,
            "the ratio stops at forty percent however bad the link gets"
        );
    }

    #[test]
    fn the_rate_falls_again_once_the_link_recovers() {
        let mut policy = ParityPolicy::default();
        for _ in 0..50 {
            policy.lost();
            policy.lost();
            policy.close_group();
        }
        let bad = policy.observed_loss();
        assert!(bad > 0.4, "measured {bad}");
        for _ in 0..50 {
            policy.close_group();
        }
        assert!(
            policy.observed_loss() < 0.01,
            "still {} after the link cleared",
            policy.observed_loss()
        );
    }

    #[test]
    fn a_group_produces_the_shards_it_was_asked_for() {
        let frames: Vec<VideoFrame> = (1..=4)
            .map(|at| frame(at, 100 + at as usize * 10))
            .collect();
        let parity = parity_for(1, 0, &frames, 2).unwrap();
        assert_eq!(parity.len(), 2);
        for (index, shard) in parity.iter().enumerate() {
            assert_eq!(shard.index, index as u8);
            assert_eq!(shard.data_shards, 4);
            assert_eq!(shard.group, 0);
            assert!(!shard.payload.is_empty());
            assert_eq!(
                shard.payload.len(),
                parity[0].payload.len(),
                "every shard of a group is the same length"
            );
        }
        // Each shard has to fit the message limit, or the group could not be sent at all.
        assert!(parity[0].payload.len() <= MAX_PARITY_BYTES);
    }

    #[test]
    fn a_group_too_large_to_cover_is_skipped_rather_than_refused() {
        let huge = vec![frame(1, MAX_PARITY_BYTES - 16), frame(2, 32)];
        assert_eq!(
            parity_for(1, 0, &huge, 1).unwrap(),
            Vec::new(),
            "a group that cannot be covered sends no parity, and is not an error"
        );
    }

    #[test]
    fn an_empty_group_sends_nothing() {
        assert!(parity_for(1, 0, &[], 2).unwrap().is_empty());
        assert!(parity_for(1, 0, &[frame(1, 8)], 0).unwrap().is_empty());
    }
}
