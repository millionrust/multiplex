//! Measuring how much bandwidth the link actually has.
//!
//! Everything above this guesses: the tile encoder guesses how much to send, the video encoder
//! guesses a bitrate, the parity policy guesses a ratio. They all guess better given a number, and
//! the number has to be measured rather than assumed, because the difference between a home
//! network and a train is two orders of magnitude.
//!
//! The method is the one the plan calls SQP. Each flush goes out as a paced burst closed by a
//! mark; the viewer times how long the burst took to arrive and reports it; a burst's rate is its
//! bytes over that spread. The host keeps the last few and takes the **harmonic mean**, which is
//! the right average for rates — it is the total bytes over the total time, so one slow burst
//! pulls the estimate down about as far as it deserves and one fast burst cannot pull it up.
//!
//! Then it takes 15% off. An estimate used as a budget has to sit below the truth: sending at
//! exactly the measured rate fills the queue that produced the measurement, and the next
//! measurement is worse. The headroom is what keeps that from running away.

use std::collections::VecDeque;

/// Bursts kept. Few enough to follow a link that changes — walking out of range takes seconds —
/// and enough that one unlucky burst does not move the estimate far.
const REMEMBERED: usize = 8;
/// What is taken off the measured rate to get a rate worth sending at.
const HEADROOM: f64 = 0.85;
/// A burst too small to say anything. Rates from a handful of bytes are dominated by whatever the
/// operating system was doing at the time.
const MINIMUM_BYTES: u64 = 2_048;
/// A burst that arrived faster than this was limited by the host, not the link: the bytes went out
/// in one go and the measurement is of memory, not a network.
const MINIMUM_SPREAD_MICROS: u64 = 1_000;

/// What the link has recently delivered.
#[derive(Clone, Debug, Default)]
pub struct RateEstimator {
    /// Bytes per second for each remembered burst, newest last.
    rates: VecDeque<f64>,
}

impl RateEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one burst the viewer measured.
    ///
    /// Ignores bursts too small or too quick to mean anything, which is most of them on an idle
    /// screen: a typing session sends a few hundred bytes at a time, and the rate that produces
    /// says nothing about what a video would get.
    pub fn record(&mut self, bytes: u64, spread_micros: u64) {
        if bytes < MINIMUM_BYTES || spread_micros < MINIMUM_SPREAD_MICROS {
            return;
        }
        let rate = bytes as f64 * 1_000_000.0 / spread_micros as f64;
        self.rates.push_back(rate);
        while self.rates.len() > REMEMBERED {
            self.rates.pop_front();
        }
    }

    /// Bytes per second worth sending at, or `None` until enough has been measured.
    ///
    /// `None` is not zero. It means nothing is known, and a caller should keep whatever default it
    /// started with rather than degrade on a measurement it does not have.
    pub fn estimate(&self) -> Option<u64> {
        if self.rates.len() < 2 {
            return None;
        }
        // The harmonic mean: total bytes over total time, had every burst been the same size.
        let reciprocal: f64 = self.rates.iter().map(|rate| 1.0 / rate).sum();
        let harmonic = self.rates.len() as f64 / reciprocal;
        Some((harmonic * HEADROOM) as u64)
    }

    /// How many bursts the estimate rests on, for a session header that has to explain itself.
    pub fn samples(&self) -> usize {
        self.rates.len()
    }

    /// Forgets everything, because the link changed: a different network is a different link, and
    /// what the old one delivered says nothing about the new one.
    pub fn reset(&mut self) {
        self.rates.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A burst of `bytes` delivered at `bytes_per_second`.
    fn burst(bytes: u64, bytes_per_second: u64) -> (u64, u64) {
        (bytes, bytes * 1_000_000 / bytes_per_second)
    }

    #[test]
    fn nothing_is_claimed_until_something_is_measured() {
        let mut estimator = RateEstimator::new();
        assert_eq!(estimator.estimate(), None);
        let (bytes, spread) = burst(100_000, 1_000_000);
        estimator.record(bytes, spread);
        assert_eq!(
            estimator.estimate(),
            None,
            "one burst is a data point, not a measurement"
        );
        estimator.record(bytes, spread);
        assert!(estimator.estimate().is_some());
    }

    #[test]
    fn a_steady_link_measures_itself_minus_the_headroom() {
        let mut estimator = RateEstimator::new();
        for _ in 0..6 {
            let (bytes, spread) = burst(125_000, 1_000_000);
            estimator.record(bytes, spread);
        }
        let estimate = estimator.estimate().expect("a measured link");
        assert_eq!(
            estimate, 850_000,
            "a megabyte a second, with the fifteen percent that keeps the queue empty"
        );
    }

    #[test]
    fn one_slow_burst_pulls_the_estimate_down_more_than_a_fast_one_pulls_it_up() {
        let fast = {
            let mut estimator = RateEstimator::new();
            for _ in 0..3 {
                let (bytes, spread) = burst(125_000, 1_000_000);
                estimator.record(bytes, spread);
            }
            let (bytes, spread) = burst(125_000, 4_000_000);
            estimator.record(bytes, spread);
            estimator.estimate().unwrap()
        };
        let slow = {
            let mut estimator = RateEstimator::new();
            for _ in 0..3 {
                let (bytes, spread) = burst(125_000, 1_000_000);
                estimator.record(bytes, spread);
            }
            let (bytes, spread) = burst(125_000, 250_000);
            estimator.record(bytes, spread);
            estimator.estimate().unwrap()
        };
        // One burst four times as fast and one four times as slow, against the same three steady
        // ones. An arithmetic mean would move the estimate further for the fast burst, which is
        // the wrong way round: being wrong about how much room there is costs a stall, and being
        // wrong about how little costs some sharpness.
        let steady = 850_000i64;
        let up = fast as i64 - steady;
        let down = steady - slow as i64;
        assert!(
            down > up,
            "a harmonic mean should fear the slow burst more: up {up}, down {down}"
        );
    }

    #[test]
    fn a_link_that_changes_is_followed_rather_than_averaged_forever() {
        let mut estimator = RateEstimator::new();
        for _ in 0..REMEMBERED {
            let (bytes, spread) = burst(125_000, 4_000_000);
            estimator.record(bytes, spread);
        }
        assert!(estimator.estimate().unwrap() > 3_000_000);
        // The link gets much worse and stays that way.
        for _ in 0..REMEMBERED {
            let (bytes, spread) = burst(125_000, 200_000);
            estimator.record(bytes, spread);
        }
        assert_eq!(
            estimator.samples(),
            REMEMBERED,
            "only the recent past is kept"
        );
        let estimate = estimator.estimate().unwrap();
        assert!(
            (150_000..=180_000).contains(&estimate),
            "the estimate followed the link down to {estimate}"
        );
    }

    #[test]
    fn bursts_too_small_or_too_quick_to_mean_anything_are_ignored() {
        let mut estimator = RateEstimator::new();
        // A few hundred bytes of typing, delivered instantly.
        for _ in 0..10 {
            estimator.record(200, 50);
        }
        assert_eq!(estimator.samples(), 0);
        assert_eq!(estimator.estimate(), None);

        // A large burst that the host sent in one go, so the spread measures memory.
        estimator.record(1 << 20, 10);
        assert_eq!(estimator.samples(), 0);
    }

    #[test]
    fn a_new_link_starts_from_nothing_known() {
        let mut estimator = RateEstimator::new();
        for _ in 0..4 {
            let (bytes, spread) = burst(125_000, 1_000_000);
            estimator.record(bytes, spread);
        }
        assert!(estimator.estimate().is_some());
        estimator.reset();
        assert_eq!(
            estimator.estimate(),
            None,
            "what the old network delivered says nothing about the new one"
        );
    }
}
