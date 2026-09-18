//! What carries a Remote Screens session.
//!
//! Stage A puts the whole session on the Controller channel: one authenticated, ordered, reliable
//! byte stream. That is enough for reading, typing and clicking, and it is what ships today.
//!
//! Stage B wants something different for one part of it. A motion region is built to lose frames —
//! parity repairs what it can and long-term references recover the rest — so waiting for a
//! retransmission of a frame that is already stale is the one thing it must not do. That means
//! datagrams, and datagrams mean a transport that can offer more than one kind of delivery.
//!
//! This crate is the seam between those two worlds, and it exists before the QUIC transport does
//! so that the session stops assuming a single ordered pipe while there is still only one. What
//! it is **not** is a claim that everything tolerates loss:
//!
//! - Tile batches are differences from the last picture. Applying one without its predecessor
//!   leaves the screen wrong, and nothing later corrects it, because the host's shadow believes
//!   the viewer holds what it sent. They need delivery and order, on any transport.
//! - Control messages decide what the session *is*. Losing one desynchronises both sides silently.
//! - Only the motion path may be sent by a route that drops things, and only because it was
//!   designed around that.
//!
//! So the seam's job is to keep those three apart, not to blur them. A transport that cannot
//! offer unreliable delivery simply reports [`Delivery::Reliable`] for every class and carries
//! video the same way it carries everything else, which is exactly what Stage A does.
//!
//! The QUIC route that motivated all of this lives in [`quic`], behind the `iroh` feature and off
//! by default. Stage A is what ships today and it has no use for a QUIC stack, so a default build
//! is missing a route rather than missing a capability.

#![forbid(unsafe_code)]

use std::fmt;

pub use termirust_screen_protocol::Class;

#[cfg(feature = "iroh")]
pub mod quic;

/// What a route promises.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    /// Everything sent arrives, in the order it was sent.
    Reliable,
    /// Things may be lost, duplicated or reordered. Only [`Class::Video`] may be given this.
    Unreliable,
}

/// Why a send failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportError {
    /// The far side is gone.
    Closed,
    /// The chunk is larger than this route carries. A datagram route has a hard limit; a stream
    /// does not.
    TooLarge,
    /// The route refused it for a reason that is not worth distinguishing here.
    Io,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Closed => "the transport is closed",
            Self::TooLarge => "the chunk is too large for this route",
            Self::Io => "the transport refused the chunk",
        })
    }
}

impl std::error::Error for TransportError {}

/// Somewhere a session's bytes go.
///
/// One call is one chunk of that class's byte stream: on a reliable route it is appended, and on
/// an unreliable one it is a single datagram that stands alone.
pub trait Transport: Send {
    /// What this route promises for `class`.
    ///
    /// A transport must never claim [`Delivery::Unreliable`] for [`Class::Control`] or
    /// [`Class::Tiles`]. [`check_delivery`] is the assertion of that, and the session calls it
    /// rather than trusting the answer.
    fn delivery(&self, class: Class) -> Delivery;

    /// Sends one chunk.
    fn send(&mut self, class: Class, bytes: &[u8]) -> Result<(), TransportError>;

    /// The largest chunk this route takes for `class`, when there is a limit. `None` means a
    /// stream, where a large chunk is merely slow rather than impossible.
    fn maximum_chunk(&self, _class: Class) -> Option<usize> {
        None
    }
}

/// Refuses a transport that offers a class less than it needs.
///
/// Called once when a session opens, because the alternative is discovering it the first time a
/// tile batch goes missing — which looks like a rendering bug, at a moment that has nothing to do
/// with the transport that caused it.
pub fn check_delivery(transport: &dyn Transport) -> Result<(), WrongDelivery> {
    for class in [Class::Control, Class::Tiles] {
        if transport.delivery(class) == Delivery::Unreliable {
            return Err(WrongDelivery { class });
        }
    }
    Ok(())
}

/// A transport that offered a class a delivery it cannot work with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WrongDelivery {
    pub class: Class,
}

impl fmt::Display for WrongDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} cannot be carried by a route that loses things",
            self.class
        )
    }
}

impl std::error::Error for WrongDelivery {}

/// Groups one flush by class, so each goes out by the route that suits it.
///
/// On Stage A every class ends up on the same stream and this changes nothing about the bytes.
/// That is the point: the grouping is correct before there is anything to group.
#[derive(Debug, Default)]
pub struct Outbound {
    control: Vec<u8>,
    tiles: Vec<u8>,
    video: Vec<Vec<u8>>,
}

impl Outbound {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one encoded frame, already length-prefixed.
    ///
    /// Video is kept as separate chunks rather than concatenated, because on a datagram route
    /// each one travels alone: concatenating them would make one lost datagram cost several
    /// frames instead of one, which is the opposite of what the motion path wants.
    pub fn push(&mut self, class: Class, frame: Vec<u8>) {
        match class {
            Class::Control => self.control.extend(frame),
            Class::Tiles => self.tiles.extend(frame),
            Class::Video => self.video.push(frame),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.control.is_empty() && self.tiles.is_empty() && self.video.is_empty()
    }

    /// Total bytes, for whoever is measuring what a burst cost.
    pub fn len(&self) -> usize {
        self.control.len() + self.tiles.len() + self.video.iter().map(Vec::len).sum::<usize>()
    }

    /// Sends everything, control first.
    ///
    /// The order matters on a transport that shares one route: a viewer that learns who holds
    /// control before it draws the frame that control changed is never briefly wrong about it.
    pub fn flush(&mut self, transport: &mut dyn Transport) -> Result<(), TransportError> {
        if !self.control.is_empty() {
            transport.send(Class::Control, &std::mem::take(&mut self.control))?;
        }
        if !self.tiles.is_empty() {
            transport.send(Class::Tiles, &std::mem::take(&mut self.tiles))?;
        }
        for chunk in std::mem::take(&mut self.video) {
            // A frame too large for a datagram route is dropped rather than failing the flush:
            // the motion path treats it as loss, which it already knows how to answer, and the
            // tile path still has the region.
            match transport.send(Class::Video, &chunk) {
                Ok(()) | Err(TransportError::TooLarge) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        sent: Vec<(Class, Vec<u8>)>,
        video_unreliable: bool,
        limit: Option<usize>,
    }

    impl Transport for Recorder {
        fn delivery(&self, class: Class) -> Delivery {
            match class {
                Class::Video if self.video_unreliable => Delivery::Unreliable,
                _ => Delivery::Reliable,
            }
        }

        fn send(&mut self, class: Class, bytes: &[u8]) -> Result<(), TransportError> {
            if self.limit.is_some_and(|limit| bytes.len() > limit) {
                return Err(TransportError::TooLarge);
            }
            self.sent.push((class, bytes.to_vec()));
            Ok(())
        }

        fn maximum_chunk(&self, _class: Class) -> Option<usize> {
            self.limit
        }
    }

    /// A transport that claims it may lose the things that cannot be lost.
    struct Reckless;

    impl Transport for Reckless {
        fn delivery(&self, _class: Class) -> Delivery {
            Delivery::Unreliable
        }

        fn send(&mut self, _class: Class, _bytes: &[u8]) -> Result<(), TransportError> {
            Ok(())
        }
    }

    #[test]
    fn a_transport_that_could_lose_tiles_is_refused_before_it_loses_any() {
        assert_eq!(
            check_delivery(&Reckless),
            Err(WrongDelivery {
                class: Class::Control
            })
        );
        let ordered = Recorder::default();
        assert_eq!(check_delivery(&ordered), Ok(()));
        // Unreliable video alone is exactly what a QUIC transport will offer, and is fine.
        let datagrams = Recorder {
            video_unreliable: true,
            ..Recorder::default()
        };
        assert_eq!(check_delivery(&datagrams), Ok(()));
    }

    #[test]
    fn control_goes_before_the_pixels_it_explains() {
        let mut out = Outbound::new();
        out.push(Class::Tiles, vec![1, 2, 3]);
        out.push(Class::Control, vec![9]);
        out.push(Class::Video, vec![7, 7]);
        let mut transport = Recorder::default();
        out.flush(&mut transport).unwrap();
        assert_eq!(
            transport
                .sent
                .iter()
                .map(|(class, _)| *class)
                .collect::<Vec<_>>(),
            vec![Class::Control, Class::Tiles, Class::Video]
        );
        assert!(out.is_empty(), "a flush leaves nothing behind");
    }

    #[test]
    fn video_frames_travel_separately_so_one_loss_costs_one_frame() {
        let mut out = Outbound::new();
        out.push(Class::Video, vec![1]);
        out.push(Class::Video, vec![2]);
        out.push(Class::Tiles, vec![3]);
        out.push(Class::Tiles, vec![4]);
        assert_eq!(out.len(), 4);
        let mut transport = Recorder::default();
        out.flush(&mut transport).unwrap();
        let video: Vec<_> = transport
            .sent
            .iter()
            .filter(|(class, _)| *class == Class::Video)
            .map(|(_, bytes)| bytes.clone())
            .collect();
        assert_eq!(video, vec![vec![1], vec![2]], "two frames, two chunks");
        let tiles: Vec<_> = transport
            .sent
            .iter()
            .filter(|(class, _)| *class == Class::Tiles)
            .map(|(_, bytes)| bytes.clone())
            .collect();
        assert_eq!(
            tiles,
            vec![vec![3, 4]],
            "tiles share a stream, so they share a chunk"
        );
    }

    #[test]
    fn a_frame_too_large_for_a_datagram_is_treated_as_loss_rather_than_a_failure() {
        let mut out = Outbound::new();
        out.push(Class::Video, vec![0; 4_000]);
        out.push(Class::Video, vec![1; 10]);
        let mut transport = Recorder {
            limit: Some(1_200),
            ..Recorder::default()
        };
        assert_eq!(out.flush(&mut transport), Ok(()));
        assert_eq!(
            transport.sent.len(),
            1,
            "the oversized frame was dropped and the small one still went"
        );
    }

    #[test]
    fn a_closed_transport_stops_the_flush() {
        struct Closed;
        impl Transport for Closed {
            fn delivery(&self, _class: Class) -> Delivery {
                Delivery::Reliable
            }
            fn send(&mut self, _class: Class, _bytes: &[u8]) -> Result<(), TransportError> {
                Err(TransportError::Closed)
            }
        }
        let mut out = Outbound::new();
        out.push(Class::Control, vec![1]);
        assert_eq!(out.flush(&mut Closed), Err(TransportError::Closed));
    }

    #[test]
    fn every_message_is_classed_and_only_video_may_be_lost() {
        use termirust_screen_protocol::{Message, Parity, VideoFrame};

        assert_eq!(
            Message::VideoFrame(VideoFrame {
                surface: 1,
                sequence: 1,
                keyframe: true,
                token: None,
                payload: vec![1],
            })
            .class(),
            Class::Video
        );
        assert_eq!(
            Message::Parity(Parity {
                surface: 1,
                group: 0,
                index: 0,
                data_shards: 2,
                payload: vec![1],
            })
            .class(),
            Class::Video
        );
        // The things that cannot be lost.
        assert_eq!(Message::RequestControl.class(), Class::Control);
        assert_eq!(
            Message::MotionRegion {
                surface: 1,
                rect: None
            }
            .class(),
            Class::Control,
            "where the video is, is not itself video"
        );
        assert_eq!(
            Message::VideoAcknowledge {
                surface: 1,
                tokens: vec![1],
            }
            .class(),
            Class::Control,
            "a report about lost video must not itself be losable"
        );
    }
}
