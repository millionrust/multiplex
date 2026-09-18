//! Giving things up, in order, to fit the link.
//!
//! [`crate::RateEstimator`] measures what the link delivers. This decides what to do about it.
//!
//! The order is the plan's, and it is an order of least regret: the first things to go are the
//! ones nobody would notice, and the last are the ones that change what the screen looks like.
//! Refinement only makes an already-readable screen sharper. Pixels outside the viewport are not
//! being looked at. A longer interval costs smoothness, not content. Only after all of that does
//! the picture itself get coarser.
//!
//! The terminal text path is never on this ladder. It is the cheapest thing on the wire and the
//! thing people actually need, so it keeps its bandwidth while everything around it gives ground.
//!
//! Two rules keep it from thrashing, which is the real failure mode of a controller like this. A
//! rung has to hold for [`DWELL_MS`] before another can be taken, and climbing back up needs more
//! headroom than staying put — so a link hovering at the boundary settles rather than oscillating
//! between two pictures.

use termirust_screen_codec::LossyDetail;
use termirust_screen_session::Limits;

/// How long a rung holds before the ladder moves again. Long enough for the estimate to reflect
/// the change that was just made: reacting to a measurement of the old behaviour is how a
/// controller talks itself into the floor.
pub const DWELL_MS: u64 = 1_500;
/// Demand above this share of the estimate means the link is not keeping up.
const CROWDED: f64 = 0.95;
/// Demand below this share means there is room to give a rung back. The gap between the two is
/// the hysteresis; without it a link sitting exactly at its capacity would climb up and down
/// forever.
const ROOMY: f64 = 0.60;
/// Refinement budget per idle frame when refinement is on at all.
const REFINE_BYTES: usize = 2_000;
/// The interval the plan's Stage A floor names, used once the ladder starts slowing frames down.
const SLOW_INTERVAL_MS: u64 = 66;
/// Video bitrate once the ladder caps it, in bits per second.
pub const CAPPED_VIDEO_BITRATE: u32 = 300_000;

/// How much has been given up, worst last.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub enum Rung {
    /// Everything on.
    #[default]
    Full,
    /// No progressive refinement. The screen stops getting sharper while idle; it never stops
    /// being correct, because refinement was only ever replacing a good approximation with an
    /// exact copy.
    NoRefinement,
    /// Only what the viewer says it is looking at. Free when the viewer shows the whole screen.
    ViewportOnly,
    /// Fewer frames. Damage is folded into the next batch rather than dropped, so this costs
    /// smoothness and nothing else.
    SlowerFrames,
    /// A coarser first pass on picture tiles. Text tiles are unaffected — they do not go through
    /// the lossy path at all — so this makes photographs muddier and leaves words sharp.
    LowerQuality,
    /// The motion region is capped hard. The last rung, because a video that stutters is still
    /// more use than a screen that has stopped arriving.
    CappedVideo,
}

impl Rung {
    /// The worst rung there is, for tests and for a session header that has to name it.
    pub const WORST: Self = Self::CappedVideo;

    const ORDER: [Self; 6] = [
        Self::Full,
        Self::NoRefinement,
        Self::ViewportOnly,
        Self::SlowerFrames,
        Self::LowerQuality,
        Self::CappedVideo,
    ];

    fn down(self) -> Self {
        let at = Self::ORDER
            .iter()
            .position(|rung| *rung == self)
            .unwrap_or(0);
        Self::ORDER[(at + 1).min(Self::ORDER.len() - 1)]
    }

    fn up(self) -> Self {
        let at = Self::ORDER
            .iter()
            .position(|rung| *rung == self)
            .unwrap_or(0);
        Self::ORDER[at.saturating_sub(1)]
    }

    /// What the session may spend at this rung.
    pub fn limits(self) -> Limits {
        Limits {
            refine_budget_bytes: if self >= Self::NoRefinement {
                0
            } else {
                REFINE_BYTES
            },
            viewport_only: self >= Self::ViewportOnly,
            minimum_interval_ms: if self >= Self::SlowerFrames {
                SLOW_INTERVAL_MS
            } else {
                0
            },
            lossy_detail: if self >= Self::LowerQuality {
                LossyDetail::LOW
            } else {
                LossyDetail::STANDARD
            },
        }
    }

    /// The bitrate the motion encoder should use, given what it would otherwise have asked for.
    pub fn video_bitrate(self, wanted: u32) -> u32 {
        if self >= Self::CappedVideo {
            wanted.min(CAPPED_VIDEO_BITRATE)
        } else {
            wanted
        }
    }

    /// Whether anything has been given up, for a session header that should say so.
    pub fn is_degraded(self) -> bool {
        self != Self::Full
    }
}

/// Decides which rung to stand on.
#[derive(Clone, Copy, Debug)]
pub struct Ladder {
    rung: Rung,
    /// When the current rung was taken, so it can be held for a while.
    since_ms: u64,
    /// Bytes sent since the window opened, and when it opened.
    sent: u64,
    window_start_ms: u64,
}

impl Default for Ladder {
    fn default() -> Self {
        Self::new()
    }
}

impl Ladder {
    pub const fn new() -> Self {
        Self {
            rung: Rung::Full,
            since_ms: 0,
            sent: 0,
            window_start_ms: 0,
        }
    }

    pub const fn rung(&self) -> Rung {
        self.rung
    }

    /// Records a burst the host just sent.
    pub const fn sent(&mut self, bytes: u64) {
        self.sent = self.sent.saturating_add(bytes);
    }

    /// Reconsiders the rung, given what the link is measured to deliver.
    ///
    /// `estimate` of `None` means nothing has been measured, and `backlogged` is then the only
    /// evidence there is: the session has batches outstanding that the viewer has not
    /// acknowledged, so it is already refusing to encode new frames. That happens on a link too
    /// small for the screen — and it happens to every Stage A viewer, because reporting bursts is
    /// a Stage B feature and a protocol-v1 phone never measures anything. Without this the ladder
    /// returned early on every call for such a peer and the session simply queued: a screen that
    /// lagged further and further behind rather than one that gave something up. Measured in
    /// [RS7](../../../docs/engineering-evidence/RS7-network-matrix.md): 251 kbps offered into a
    /// 200 kbps link, sat at `Full`.
    ///
    /// An unmeasured link with nothing backed up is still not a slow one, and the ladder holds
    /// where it is rather than guessing.
    pub fn consider(&mut self, now_ms: u64, estimate: Option<u64>, backlogged: bool) -> Rung {
        let elapsed = now_ms.saturating_sub(self.window_start_ms);
        if elapsed < DWELL_MS {
            return self.rung;
        }
        let demand = self.sent * 1_000 / elapsed.max(1);
        self.sent = 0;
        self.window_start_ms = now_ms;

        if now_ms.saturating_sub(self.since_ms) < DWELL_MS {
            return self.rung;
        }
        let next = match estimate.filter(|estimate| *estimate > 0) {
            Some(estimate) => {
                let share = demand as f64 / estimate as f64;
                if share > CROWDED {
                    self.rung.down()
                } else if share < ROOMY {
                    self.rung.up()
                } else {
                    self.rung
                }
            }
            // No number, so no share to compare: one rung at a time while the backlog lasts, and
            // back up once it clears. Coarser than the measured path deliberately — it knows that
            // the link is too small, not by how much, and guessing a size from a queue depth would
            // be inventing precision.
            None if backlogged => self.rung.down(),
            None => self.rung.up(),
        };
        if next != self.rung {
            self.rung = next;
            self.since_ms = now_ms;
        }
        self.rung
    }

    /// Goes back to the top, because the link changed and what it used to do says nothing about
    /// what it does now.
    pub const fn reset(&mut self) {
        self.rung = Rung::Full;
        self.since_ms = 0;
        self.sent = 0;
        self.window_start_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the ladder for `seconds`, sending `bytes_per_second` against a link of `estimate`.
    fn settle(bytes_per_second: u64, estimate: u64, seconds: u64) -> Ladder {
        let mut ladder = Ladder::new();
        let mut now = 0;
        for _ in 0..seconds * 2 {
            now += 500;
            ladder.sent(bytes_per_second / 2);
            ladder.consider(now, Some(estimate), false);
        }
        ladder
    }

    #[test]
    fn a_link_with_room_gives_nothing_up() {
        let ladder = settle(100_000, 1_000_000, 30);
        assert_eq!(ladder.rung(), Rung::Full);
        assert!(!ladder.rung().is_degraded());
    }

    #[test]
    fn the_first_thing_given_up_is_the_one_nobody_would_notice() {
        let mut ladder = Ladder::new();
        let mut now = 0;
        // Just over capacity, for long enough to take exactly one rung.
        for _ in 0..4 {
            now += 500;
            ladder.sent(60_000);
            ladder.consider(now, Some(100_000), false);
        }
        assert_eq!(ladder.rung(), Rung::NoRefinement);
        assert_eq!(ladder.rung().limits().refine_budget_bytes, 0);
        assert!(
            !ladder.rung().limits().viewport_only,
            "nothing further was given up for one bad window"
        );
    }

    #[test]
    fn a_link_far_too_slow_ends_up_at_the_bottom_and_stops() {
        let ladder = settle(1_000_000, 50_000, 60);
        assert_eq!(ladder.rung(), Rung::WORST);
        let limits = ladder.rung().limits();
        assert_eq!(limits.refine_budget_bytes, 0);
        assert!(limits.viewport_only);
        assert_eq!(limits.minimum_interval_ms, SLOW_INTERVAL_MS);
        assert_eq!(limits.lossy_detail, LossyDetail::LOW);
        assert_eq!(ladder.rung().video_bitrate(8_000_000), CAPPED_VIDEO_BITRATE);
    }

    #[test]
    fn a_link_that_recovers_gets_its_rungs_back() {
        let mut ladder = settle(1_000_000, 50_000, 60);
        assert_eq!(ladder.rung(), Rung::WORST);
        let mut now = 100_000;
        for _ in 0..80 {
            now += 500;
            ladder.sent(10_000);
            ladder.consider(now, Some(1_000_000), false);
        }
        assert_eq!(
            ladder.rung(),
            Rung::Full,
            "the link came back and so should the picture"
        );
    }

    #[test]
    fn a_link_sitting_exactly_at_capacity_settles_rather_than_oscillating() {
        let mut ladder = Ladder::new();
        let mut now = 0;
        let mut seen = Vec::new();
        // Demand right in the band between crowded and roomy: the ladder should pick a rung and
        // stay on it, because moving would only change what it measures next.
        for _ in 0..120 {
            now += 500;
            ladder.sent(40_000);
            seen.push(ladder.consider(now, Some(100_000), false));
        }
        let settled = *seen.last().unwrap();
        let changes = seen.windows(2).filter(|pair| pair[0] != pair[1]).count();
        assert!(
            changes <= 1,
            "the rung moved {changes} times on a steady link, ending at {settled:?}"
        );
    }

    #[test]
    fn nothing_is_given_up_before_anything_is_measured() {
        let mut ladder = Ladder::new();
        let mut now = 0;
        for _ in 0..40 {
            now += 500;
            ladder.sent(1_000_000);
            ladder.consider(now, None, false);
        }
        assert_eq!(
            ladder.rung(),
            Rung::Full,
            "an unmeasured link is not a slow one"
        );
    }

    /// The Stage A case, which had no rate control at all until this existed.
    ///
    /// Reporting bursts is a Stage B feature, so a protocol-v1 phone never measures the link and
    /// the host never gets an estimate. The session still knows it is stuck, because the viewer
    /// has stopped acknowledging batches. RS7 measured what the old behaviour did: 251 kbps
    /// offered into a 200 kbps link, sitting at `Full` while the queue grew.
    #[test]
    fn a_backlogged_link_gives_things_up_even_with_nothing_measured() {
        let mut ladder = Ladder::new();
        let mut now = 0;
        for _ in 0..12 {
            now += DWELL_MS + 1;
            ladder.sent(1_000_000);
            ladder.consider(now, None, true);
        }
        assert_eq!(
            ladder.rung(),
            Rung::WORST,
            "a session that cannot get its batches acknowledged should have given everything up"
        );

        // And it comes back when the backlog clears, or a link that hiccupped once would stay
        // degraded for the life of the session.
        for _ in 0..12 {
            now += DWELL_MS + 1;
            ladder.sent(1_000);
            ladder.consider(now, None, false);
        }
        assert_eq!(
            ladder.rung(),
            Rung::Full,
            "the backlog cleared and nothing came back"
        );
    }

    /// One rung per dwell, not a fall to the floor the moment a queue appears.
    #[test]
    fn a_backlog_costs_one_rung_at_a_time() {
        let mut ladder = Ladder::new();
        let mut now = DWELL_MS + 1;
        ladder.sent(1_000_000);
        assert_eq!(ladder.consider(now, None, true), Rung::NoRefinement);
        // Within the dwell time nothing more is given up, however backed up it looks.
        now += 10;
        ladder.sent(1_000_000);
        assert_eq!(ladder.consider(now, None, true), Rung::NoRefinement);
    }

    #[test]
    fn a_rung_is_held_long_enough_to_see_what_it_did() {
        let mut ladder = Ladder::new();
        // Two windows of hopeless overload back to back. Without a dwell time the ladder would
        // fall several rungs before the first one had any chance to help.
        ladder.sent(10_000_000);
        ladder.consider(DWELL_MS + 1, Some(1_000), false);
        let first = ladder.rung();
        ladder.sent(10_000_000);
        ladder.consider(DWELL_MS + 2, Some(1_000), false);
        assert_eq!(
            ladder.rung(),
            first,
            "a second rung was taken before the first had time to work"
        );
    }

    #[test]
    fn a_new_link_starts_from_the_top() {
        let mut ladder = settle(1_000_000, 50_000, 60);
        assert!(ladder.rung().is_degraded());
        ladder.reset();
        assert_eq!(ladder.rung(), Rung::Full);
    }

    #[test]
    fn the_rungs_are_ordered_worst_last() {
        let mut rung = Rung::Full;
        let mut seen = vec![rung];
        for _ in 0..10 {
            rung = rung.down();
            if *seen.last().unwrap() != rung {
                seen.push(rung);
            }
        }
        assert_eq!(seen, Rung::ORDER.to_vec());
        assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));
        // And back up again, which is the same order reversed.
        let mut back = vec![rung];
        for _ in 0..10 {
            rung = rung.up();
            if *back.last().unwrap() != rung {
                back.push(rung);
            }
        }
        back.reverse();
        assert_eq!(back, Rung::ORDER.to_vec());
    }
}
