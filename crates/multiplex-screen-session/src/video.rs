//! Repairing the motion stream on the viewer.
//!
//! The host sends a few parity shards with every group of video frames. A viewer that received
//! everything throws them away. A viewer that lost one frame of the group rebuilds it here,
//! without a round trip and without a keyframe.
//!
//! The shards are whole encoded messages, so a rebuilt frame comes back with its own sequence,
//! keyframe flag and reference token. Nothing about a repaired frame is inferred.
//!
//! A rebuilt frame arrives **after** frames that came later, because the parity that repairs it
//! is sent after the group. Its sequence says where it belongs; a decoder that cannot take frames
//! out of order should drop it rather than decode it in the wrong place.

use std::collections::BTreeMap;

use multiplex_screen_protocol::{Fec, Message, Parity, VideoFrame, pack_shard, unpack_shard};

/// Frames kept per surface, which bounds how far back a repair can reach. Four groups of the
/// largest group size the host is likely to use.
const REMEMBERED_FRAMES: usize = 64;
/// Parity groups kept per surface, waiting for the frames they cover.
const REMEMBERED_GROUPS: usize = 8;
/// Surfaces tracked at once. A viewer watches one screen; the rest is slack.
const TRACKED_SURFACES: usize = 4;
/// How many group-lengths a gap may stay open before it is reported lost. A gap whose parity was
/// itself lost never gets a repair attempt, so something has to give up on it.
const GAP_PATIENCE_GROUPS: u64 = 2;
/// Assumed group size before any parity has said otherwise, for ageing gaps only.
const ASSUMED_GROUP: u64 = 4;

/// What a repair produced.
#[derive(Debug, Default)]
pub(crate) struct Repaired {
    /// The surface these frames belong to.
    pub surface: u32,
    /// Frames rebuilt from parity, in sequence order.
    pub frames: Vec<VideoFrame>,
    /// Sequences that could not be rebuilt and should be reported to the host.
    pub lost: Vec<u64>,
}

#[derive(Default)]
struct Surface {
    /// Encoded video frame messages by sequence, oldest first.
    frames: BTreeMap<u64, Vec<u8>>,
    /// Parity shards by group, each with the index it was sent under.
    parity: BTreeMap<u64, Vec<(usize, Vec<u8>)>>,
    /// Data shards per group, from whichever parity shard arrived first.
    shape: BTreeMap<u64, usize>,
    /// The highest sequence seen, so a gap is noticed even when its parity is lost too.
    highest: u64,
    /// Sequences noticed missing and not yet repaired or reported.
    gaps: Vec<u64>,
    /// The last group size a parity shard declared, for ageing gaps.
    group: u64,
}

/// Collects video frames and parity, and rebuilds what the link dropped.
#[derive(Default)]
pub(crate) struct VideoRepair {
    surfaces: BTreeMap<u32, Surface>,
}

impl VideoRepair {
    /// Forgets everything about a surface, because its stream restarted: a new configuration
    /// means a new encoder, whose sequences and groups start again.
    pub fn restart(&mut self, surface: u32) {
        self.surfaces.remove(&surface);
    }

    /// Records a frame that arrived, and notices any gap before it.
    pub fn frame(&mut self, frame: &VideoFrame) -> Repaired {
        let Ok(encoded) = Message::VideoFrame(frame.clone()).encode() else {
            return Repaired::default();
        };
        let surface = self.surface(frame.surface);
        // Anything between the highest seen and this one never arrived. A frame that repeats or
        // arrives late is not a gap; it is simply already known.
        if frame.sequence > surface.highest {
            for missing in (surface.highest + 1)..frame.sequence {
                if !surface.gaps.contains(&missing) {
                    surface.gaps.push(missing);
                }
            }
            surface.highest = frame.sequence;
        }
        surface.gaps.retain(|gap| *gap != frame.sequence);
        surface.frames.insert(frame.sequence, encoded);
        while surface.frames.len() > REMEMBERED_FRAMES {
            let oldest = *surface.frames.keys().next().expect("not empty");
            surface.frames.remove(&oldest);
        }
        let mut repaired = Repaired {
            surface: frame.surface,
            ..Repaired::default()
        };
        Self::age_gaps(surface, &mut repaired);
        repaired
    }

    /// Records a parity shard and tries to close the group it covers.
    pub fn parity(&mut self, parity: &Parity) -> Repaired {
        if parity.data_shards == 0 || parity.payload.is_empty() {
            return Repaired::default();
        }
        let shards = parity.data_shards as usize;
        let surface = self.surface(parity.surface);
        surface.group = shards as u64;
        surface.shape.insert(parity.group, shards);
        let held = surface.parity.entry(parity.group).or_default();
        if !held
            .iter()
            .any(|(index, _)| *index == parity.index as usize)
        {
            held.push((parity.index as usize, parity.payload.clone()));
        }
        while surface.parity.len() > REMEMBERED_GROUPS {
            let oldest = *surface.parity.keys().next().expect("not empty");
            surface.parity.remove(&oldest);
            surface.shape.remove(&oldest);
        }
        let mut repaired = Repaired {
            surface: parity.surface,
            ..Repaired::default()
        };
        Self::close(surface, parity.group, &mut repaired);
        Self::age_gaps(surface, &mut repaired);
        repaired
    }

    fn surface(&mut self, id: u32) -> &mut Surface {
        if !self.surfaces.contains_key(&id) && self.surfaces.len() >= TRACKED_SURFACES {
            let oldest = *self.surfaces.keys().next().expect("not empty");
            self.surfaces.remove(&oldest);
        }
        self.surfaces.entry(id).or_default()
    }

    /// Reports a gap that has been open too long to still be waiting for its parity.
    fn age_gaps(surface: &mut Surface, repaired: &mut Repaired) {
        let group = if surface.group == 0 {
            ASSUMED_GROUP
        } else {
            surface.group
        };
        let patience = group * GAP_PATIENCE_GROUPS;
        surface.gaps.retain(|gap| {
            if surface.highest > gap + patience {
                repaired.lost.push(*gap);
                false
            } else {
                true
            }
        });
    }

    /// Rebuilds a group, if everything it needs has arrived.
    fn close(surface: &mut Surface, group: u64, repaired: &mut Repaired) {
        let Some(&shards) = surface.shape.get(&group) else {
            return;
        };
        let Some(held) = surface.parity.get(&group) else {
            return;
        };
        let first = group * shards as u64 + 1;
        let sequences: Vec<u64> = (first..first + shards as u64).collect();
        let present: Vec<bool> = sequences
            .iter()
            .map(|sequence| surface.frames.contains_key(sequence))
            .collect();
        let missing = present.iter().filter(|held| !**held).count();
        if missing == 0 {
            // Nothing to repair, and nothing more will be asked of this group.
            surface.parity.remove(&group);
            surface.shape.remove(&group);
            return;
        }
        if missing > held.len() {
            // More gone than the parity can repair. Wait: another shard of this group may still
            // arrive. `age_gaps` gives up on it if one never does.
            return;
        }
        let length = held[0].1.len();
        let mut data: Vec<Option<Vec<u8>>> = sequences
            .iter()
            .map(|sequence| {
                surface
                    .frames
                    .get(sequence)
                    .map(|bytes| pack_shard(bytes, length).unwrap_or_default())
            })
            .collect();
        if data.iter().flatten().any(|shard| shard.len() != length) {
            // A frame longer than the group's shards means this parity does not belong to these
            // frames. Repairing from it would produce bytes that decode to nothing good.
            return;
        }
        let Ok(fec) = Fec::new(shards, held.len().max(1)) else {
            return;
        };
        if fec.rebuild(&mut data, held).is_err() {
            return;
        }
        let expected = expected_surface(surface);
        // Decoded first, so nothing borrows the surface while it is being written back.
        let rebuilt: Vec<(u64, Option<VideoFrame>)> = sequences
            .iter()
            .enumerate()
            .filter(|(at, _)| !present[*at])
            .map(|(at, sequence)| {
                let frame = data[at].as_ref().and_then(|shard| {
                    let bytes = unpack_shard(shard).ok()?;
                    match Message::decode(&bytes) {
                        // A repaired frame must say it is the frame that was missing. Anything
                        // else means the group was rebuilt from shards that never belonged
                        // together, and decoding it would corrupt the picture.
                        Ok(Message::VideoFrame(frame))
                            if frame.sequence == *sequence
                                && expected.is_none_or(|id| frame.surface == id) =>
                        {
                            Some(frame)
                        }
                        _ => None,
                    }
                });
                (*sequence, frame)
            })
            .collect();

        for (sequence, frame) in rebuilt {
            match frame {
                Some(frame) => {
                    if let Ok(encoded) = Message::VideoFrame(frame.clone()).encode() {
                        surface.frames.insert(sequence, encoded);
                    }
                    surface.gaps.retain(|gap| *gap != sequence);
                    repaired.frames.push(frame);
                }
                None => {
                    surface.gaps.retain(|gap| *gap != sequence);
                    repaired.lost.push(sequence);
                }
            }
        }
        surface.parity.remove(&group);
        surface.shape.remove(&group);
    }
}

/// The surface these frames belong to, taken from one that actually arrived, so a rebuilt frame
/// claiming a different surface is recognised as coming from mismatched shards.
fn expected_surface(surface: &Surface) -> Option<u32> {
    surface
        .frames
        .values()
        .next()
        .and_then(|bytes| match Message::decode(bytes) {
            Ok(Message::VideoFrame(frame)) => Some(frame.surface),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use multiplex_screen_protocol::{Fec, shard_length};

    const SURFACE: u32 = 1;

    fn frame(sequence: u64, bytes: usize) -> VideoFrame {
        VideoFrame {
            surface: SURFACE,
            sequence,
            keyframe: sequence == 1,
            token: Some(sequence as u32),
            payload: vec![(sequence % 241 + 1) as u8; bytes],
        }
    }

    /// Builds the parity the host would send for a group, so the viewer is tested against the
    /// real shape rather than one invented here.
    fn parity_for(group: u64, frames: &[VideoFrame], shards: usize) -> Vec<Parity> {
        let encoded: Vec<Vec<u8>> = frames
            .iter()
            .map(|frame| Message::VideoFrame(frame.clone()).encode().unwrap())
            .collect();
        let length = shard_length(&encoded);
        let packed: Vec<Vec<u8>> = encoded
            .iter()
            .map(|message| pack_shard(message, length).unwrap())
            .collect();
        Fec::new(frames.len(), shards)
            .unwrap()
            .encode(&packed)
            .unwrap()
            .into_iter()
            .enumerate()
            .map(|(index, payload)| Parity {
                surface: SURFACE,
                group,
                index: index as u8,
                data_shards: frames.len() as u8,
                payload,
            })
            .collect()
    }

    #[test]
    fn a_dropped_frame_comes_back_exactly() {
        let frames: Vec<VideoFrame> = (1..=4).map(|at| frame(at, 20 + at as usize)).collect();
        let parity = parity_for(0, &frames, 1);
        let mut repair = VideoRepair::default();

        // The third frame never arrives.
        for frame in frames.iter().filter(|frame| frame.sequence != 3) {
            assert!(repair.frame(frame).frames.is_empty());
        }
        let repaired = repair.parity(&parity[0]);
        assert_eq!(repaired.frames, vec![frames[2].clone()]);
        assert!(repaired.lost.is_empty(), "nothing had to be reported");
    }

    #[test]
    fn a_repaired_frame_keeps_its_keyframe_flag_and_reference_token() {
        let frames: Vec<VideoFrame> = (1..=4).map(|at| frame(at, 16)).collect();
        let parity = parity_for(0, &frames, 1);
        let mut repair = VideoRepair::default();
        for frame in frames.iter().skip(1) {
            repair.frame(frame);
        }
        let repaired = repair.parity(&parity[0]);
        let rebuilt = repaired.frames.first().expect("the first frame came back");
        assert!(rebuilt.keyframe, "the flag came back with it, not guessed");
        assert_eq!(rebuilt.token, Some(1));
        assert_eq!(rebuilt.payload, frames[0].payload);
    }

    #[test]
    fn a_group_that_arrived_whole_costs_nothing() {
        let frames: Vec<VideoFrame> = (1..=4).map(|at| frame(at, 12)).collect();
        let parity = parity_for(0, &frames, 2);
        let mut repair = VideoRepair::default();
        for frame in &frames {
            repair.frame(frame);
        }
        let repaired = repair.parity(&parity[0]);
        assert!(repaired.frames.is_empty() && repaired.lost.is_empty());
        // The group is finished with, so the second shard is ignored rather than re-repairing.
        assert!(repair.parity(&parity[1]).frames.is_empty());
    }

    #[test]
    fn two_losses_need_two_parity_shards() {
        let frames: Vec<VideoFrame> = (1..=4).map(|at| frame(at, 24)).collect();
        let parity = parity_for(0, &frames, 2);
        let mut repair = VideoRepair::default();
        for frame in frames.iter().filter(|frame| frame.sequence % 2 == 1) {
            repair.frame(frame);
        }
        // One shard is not enough for two gaps; the group waits rather than guessing.
        assert!(repair.parity(&parity[0]).frames.is_empty());
        let repaired = repair.parity(&parity[1]);
        assert_eq!(
            repaired.frames,
            vec![frames[1].clone(), frames[3].clone()],
            "both came back once the second shard arrived"
        );
    }

    #[test]
    fn a_gap_whose_parity_never_arrives_is_reported_lost() {
        let mut repair = VideoRepair::default();
        repair.frame(&frame(1, 8));
        // Frame two is gone, and so is its parity. The viewer keeps going and eventually says so.
        let mut reported = Vec::new();
        for sequence in 3..=14 {
            reported.extend(repair.frame(&frame(sequence, 8)).lost);
        }
        assert_eq!(reported, vec![2], "the gap was reported exactly once");
    }

    #[test]
    fn an_unrepairable_group_is_reported_rather_than_guessed() {
        let frames: Vec<VideoFrame> = (1..=4).map(|at| frame(at, 16)).collect();
        // Only one shard was sent, and three frames of the group went missing.
        let parity = parity_for(0, &frames, 1);
        let mut repair = VideoRepair::default();
        repair.frame(&frames[0]);
        assert!(repair.parity(&parity[0]).frames.is_empty());
        // The stream carries on, and the gap ages out into a report.
        let mut reported = Vec::new();
        for sequence in 5..=16 {
            reported.extend(repair.frame(&frame(sequence, 16)).lost);
        }
        assert_eq!(reported, vec![2, 3, 4]);
    }

    #[test]
    fn a_restart_forgets_the_old_stream() {
        let frames: Vec<VideoFrame> = (1..=4).map(|at| frame(at, 16)).collect();
        let parity = parity_for(0, &frames, 1);
        let mut repair = VideoRepair::default();
        for frame in frames.iter().skip(1) {
            repair.frame(frame);
        }
        repair.restart(SURFACE);
        // The parity is about sequences this viewer no longer knows anything about, and the old
        // frames are gone, so nothing is rebuilt and nothing is claimed lost.
        let repaired = repair.parity(&parity[0]);
        assert!(repaired.frames.is_empty() && repaired.lost.is_empty());
    }

    #[test]
    fn nonsense_parity_never_produces_a_frame() {
        let frames: Vec<VideoFrame> = (1..=4).map(|at| frame(at, 16)).collect();
        let mut repair = VideoRepair::default();
        for frame in frames.iter().skip(1) {
            repair.frame(frame);
        }
        let repaired = repair.parity(&Parity {
            surface: SURFACE,
            group: 0,
            index: 0,
            data_shards: 4,
            payload: vec![0xAB; 64],
        });
        assert!(
            repaired.frames.is_empty(),
            "bytes that rebuild to nothing decodable are refused"
        );
        assert_eq!(repaired.lost, vec![1], "and the frame is reported instead");
    }

    #[test]
    fn what_is_remembered_stays_bounded() {
        let mut repair = VideoRepair::default();
        for sequence in 1..=(REMEMBERED_FRAMES as u64 * 3) {
            repair.frame(&frame(sequence, 8));
        }
        let surface = repair.surfaces.get(&SURFACE).expect("the surface");
        assert_eq!(surface.frames.len(), REMEMBERED_FRAMES);
        assert!(surface.gaps.is_empty());

        for group in 0..(REMEMBERED_GROUPS as u64 * 3) {
            repair.parity(&Parity {
                surface: SURFACE,
                group: 10_000 + group,
                index: 0,
                data_shards: 4,
                payload: vec![1; 32],
            });
        }
        let surface = repair.surfaces.get(&SURFACE).expect("the surface");
        assert!(surface.parity.len() <= REMEMBERED_GROUPS);
        assert!(surface.shape.len() <= REMEMBERED_GROUPS);

        for id in 0..(TRACKED_SURFACES as u32 * 3) {
            repair.frame(&VideoFrame {
                surface: id,
                ..frame(1, 8)
            });
        }
        assert!(repair.surfaces.len() <= TRACKED_SURFACES);
    }
}
