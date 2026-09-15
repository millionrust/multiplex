use std::collections::VecDeque;
use std::time::Duration;

use crate::{CaptureError, CapturedFrame, FrameSource};

/// Delivers a fixed list of frames in order, for tests and recorded workloads.
#[derive(Clone, Debug, Default)]
pub struct ReplaySource {
    frames: VecDeque<CapturedFrame>,
}

impl ReplaySource {
    pub fn new(frames: impl IntoIterator<Item = CapturedFrame>) -> Self {
        Self {
            frames: frames.into_iter().collect(),
        }
    }

    pub fn remaining(&self) -> usize {
        self.frames.len()
    }
}

impl FrameSource for ReplaySource {
    fn next_frame(&mut self, _timeout: Duration) -> Result<Option<CapturedFrame>, CaptureError> {
        self.frames.pop_front().map(Some).ok_or(CaptureError::Ended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Damage;
    use termirust_screen_codec::Size;

    #[test]
    fn replays_in_order_then_ends() {
        let size = Size::new(1, 1).unwrap();
        let mut source = ReplaySource::new([
            CapturedFrame::tight(size, vec![1, 1, 1, 255], Damage::Unknown, 0),
            CapturedFrame::tight(size, vec![2, 2, 2, 255], Damage::Unknown, 33),
        ]);
        assert_eq!(
            source
                .next_frame(Duration::ZERO)
                .unwrap()
                .unwrap()
                .timestamp_ms,
            0
        );
        assert_eq!(source.remaining(), 1);
        assert_eq!(
            source.next_frame(Duration::ZERO).unwrap().unwrap().pixels[0],
            2
        );
        assert_eq!(source.next_frame(Duration::ZERO), Err(CaptureError::Ended));
    }
}
