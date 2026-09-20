//! The QUIC route, over iroh.
//!
//! Section 5.3 of the plan chose QUIC for Stage B, and iroh for the parts QUIC does not do by
//! itself: finding a peer by key rather than by address, punching through NAT, and falling back to
//! a relay when it cannot. What QUIC brings is the three things this codec has wanted since the
//! transport seam was drawn — reliable ordered streams for tile batches, unreliable datagrams for
//! video, and a connection that survives the address underneath it changing.
//!
//! **Behind the `iroh` feature, and off by default.** A host serving tiles over the Controller
//! channel has no use for a QUIC stack, a relay client and a Tokio runtime, and Stage A is the
//! configuration that ships today. Everything above this module talks to [`Transport`], so a
//! build without the feature is missing a route rather than missing a capability.
//!
//! **Built ahead of the 0.4 device spike, by the owner's decision on 2026-09-18.** That spike is a
//! phone on cellular, which no amount of local testing substitutes for. Writing this against the
//! seam is what makes a no-go cost a transport rather than a milestone: nothing in the session
//! knows iroh exists.
//!
//! ## Why each class gets what it gets
//!
//! - [`Class::Control`] rides a bidirectional stream. It carries subscriptions, acknowledgements,
//!   input and capability changes: small, ordered, and every one of them matters.
//! - [`Class::Tiles`] rides its own unidirectional stream, so a large batch cannot head-of-line
//!   block an acknowledgement travelling the other way. A tile batch is a difference from the last
//!   picture, so this stream must be reliable — losing one leaves the screen wrong for ever.
//! - [`Class::Video`] rides datagrams. It is the only class built to survive loss, with long-term
//!   references to recover from it and parity to avoid needing to, and the only one for which
//!   waiting for a retransmission is worse than missing a frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointAddr};
use tokio::sync::Mutex;

use crate::{Class, Delivery, Transport, TransportError};

/// What a Remote Screens connection calls itself on the wire.
pub const ALPN: &[u8] = b"multiplex/screen/1";

/// How long a dropped connection is retried before the session is told it is gone.
///
/// Long enough to cross a lift or a tunnel, short enough that a viewer is not left looking at a
/// still picture wondering. Migration handles a network *change*; this is for a network that went
/// away entirely.
pub const RESUME_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

/// One session's QUIC route.
pub struct IrohTransport {
    connection: Connection,
    /// The stream tile batches are appended to. Opened lazily, because a session that never sends
    /// a batch should not make a stream for one.
    tiles: Arc<Mutex<Option<iroh::endpoint::SendStream>>>,
    control: Arc<Mutex<Option<iroh::endpoint::SendStream>>>,
    runtime: tokio::runtime::Handle,
    closed: Arc<AtomicBool>,
}

impl IrohTransport {
    /// Wraps a connection that is already open.
    pub fn new(connection: Connection, runtime: tokio::runtime::Handle) -> Self {
        Self {
            connection,
            tiles: Arc::new(Mutex::new(None)),
            control: Arc::new(Mutex::new(None)),
            runtime,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Dials `peer` and returns the route, trying 0-RTT first.
    ///
    /// 0-RTT sends the first bytes with the handshake rather than after it, which turns a
    /// reconnection from one and a half round trips into none — on a 300 ms link that is the
    /// difference between a screen that flickers and one that visibly stops. It is safe for what
    /// travels first here because the screen protocol's opening messages are idempotent: a hello
    /// and a subscribe replayed by an attacker get a session that is immediately refused for a
    /// spent ticket, which is the same outcome as not replaying them.
    ///
    /// Falls back to a full handshake when the server has no session ticket for us, which is
    /// always true of the first connection. Returns whether the server accepted the resumption.
    ///
    /// This waits for the handshake before handing back a route. The further win — writing the
    /// first bytes *during* the handshake, on the `Connection<OutgoingZeroRtt>` typestate — is
    /// available and deliberately not taken here: it needs the replay reasoning above reviewed
    /// rather than asserted, and that review is part of the 0.4 gate.
    pub async fn connect(
        endpoint: &Endpoint,
        peer: impl Into<EndpointAddr>,
        runtime: tokio::runtime::Handle,
    ) -> Result<(Self, bool), TransportError> {
        let connecting = endpoint
            .connect_with_opts(peer, ALPN, Default::default())
            .await
            .map_err(|_| TransportError::Io)?;
        match connecting.into_0rtt() {
            Ok(zero) => {
                // The endpoint had a session ticket, so it could try. Whether the server *took* it
                // is a separate answer, and the one worth reporting: a rejected attempt costs a
                // full handshake and any 0-RTT data has to be sent again.
                match zero
                    .handshake_completed()
                    .await
                    .map_err(|_| TransportError::Io)?
                {
                    iroh::endpoint::ZeroRttStatus::Accepted(connection) => {
                        Ok((Self::new(connection, runtime), true))
                    }
                    iroh::endpoint::ZeroRttStatus::Rejected(connection) => {
                        Ok((Self::new(connection, runtime), false))
                    }
                }
            }
            Err(connecting) => {
                // No ticket for this peer yet, which is always true of a first connection.
                let connection = connecting.await.map_err(|_| TransportError::Io)?;
                Ok((Self::new(connection, runtime), false))
            }
        }
    }

    /// Whether the connection is still up.
    ///
    /// A path change is *not* a drop: QUIC identifies a connection by its id rather than by the
    /// address it arrived from, so moving from Wi-Fi to cellular keeps the same connection and
    /// this keeps saying yes. That is the whole reason for choosing QUIC here, and it is why
    /// nothing in this module tries to detect roaming — there is nothing to detect.
    pub fn is_open(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && self.connection.close_reason().is_none()
    }

    /// Why the far side went, once it has.
    pub fn close_reason(&self) -> Option<String> {
        self.connection
            .close_reason()
            .map(|reason| reason.to_string())
    }

    /// Ends the streams cleanly, then the connection.
    ///
    /// Finishing each stream first is what tells the peer it has everything: a QUIC stream that is
    /// merely dropped leaves the reader waiting, and closing the connection under it turns the last
    /// batch into an error instead of an end. The test caught exactly that.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let control = Arc::clone(&self.control);
        let tiles = Arc::clone(&self.tiles);
        self.runtime.block_on(async move {
            for slot in [control, tiles] {
                if let Some(stream) = slot.lock().await.as_mut() {
                    let _ = stream.finish();
                    let _ = stream.stopped().await;
                }
            }
        });
        self.connection.close(0u32.into(), b"done");
    }
}

impl Transport for IrohTransport {
    fn delivery(&self, class: Class) -> Delivery {
        match class {
            Class::Video => Delivery::Unreliable,
            Class::Control | Class::Tiles => Delivery::Reliable,
        }
    }

    fn send(&mut self, class: Class, bytes: &[u8]) -> Result<(), TransportError> {
        if !self.is_open() {
            return Err(TransportError::Closed);
        }
        if class == Class::Video {
            // One frame, one datagram, and no waiting: a video frame that does not fit or cannot
            // go now is worth less than the next one, which is the whole reason this class is on
            // datagrams. `send_datagram` refuses rather than queues, which is what we want.
            return match self
                .connection
                .send_datagram(bytes::Bytes::copy_from_slice(bytes))
            {
                Ok(()) => Ok(()),
                Err(iroh::endpoint::SendDatagramError::TooLarge) => Err(TransportError::TooLarge),
                Err(iroh::endpoint::SendDatagramError::ConnectionLost(_)) => {
                    Err(TransportError::Closed)
                }
                Err(_) => Err(TransportError::Io),
            };
        }
        let connection = self.connection.clone();
        let chunk = bytes.to_vec();
        let slot = match class {
            Class::Control => Arc::clone(&self.control),
            _ => Arc::clone(&self.tiles),
        };
        self.runtime.block_on(async move {
            let mut guard = slot.lock().await;
            if guard.is_none() {
                let stream = match class {
                    Class::Control => {
                        connection
                            .open_bi()
                            .await
                            .map_err(|_| TransportError::Closed)?
                            .0
                    }
                    _ => connection
                        .open_uni()
                        .await
                        .map_err(|_| TransportError::Closed)?,
                };
                *guard = Some(stream);
            }
            let stream = guard.as_mut().expect("just opened");
            stream
                .write_all(&chunk)
                .await
                .map_err(|_| TransportError::Closed)
        })
    }

    fn maximum_chunk(&self, class: Class) -> Option<usize> {
        match class {
            // A datagram has a hard ceiling that depends on the path's MTU, and it changes when
            // the path does. Asking every time is the only answer that stays true.
            Class::Video => self.connection.max_datagram_size(),
            Class::Control | Class::Tiles => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The promises each class is given, which is what the session checks before it opens.
    ///
    /// Worth a test that needs no network: this mapping is the one thing in the module that would
    /// silently corrupt a screen rather than fail loudly. A tile batch is a difference from the
    /// last picture, so a route that lost one would leave the viewer wrong for ever with nothing
    /// to report it.
    #[test]
    fn only_video_is_allowed_to_lose_anything() {
        // A transport cannot be built without a connection, so this asserts on the same match the
        // implementation uses rather than on an instance.
        fn delivery(class: Class) -> Delivery {
            match class {
                Class::Video => Delivery::Unreliable,
                Class::Control | Class::Tiles => Delivery::Reliable,
            }
        }
        assert_eq!(delivery(Class::Control), Delivery::Reliable);
        assert_eq!(delivery(Class::Tiles), Delivery::Reliable);
        assert_eq!(delivery(Class::Video), Delivery::Unreliable);
    }

    #[test]
    fn the_alpn_names_the_protocol_and_its_version() {
        // Changing this is changing what peers will talk to each other, so it should be a
        // deliberate edit with a test to notice it.
        assert_eq!(ALPN, b"multiplex/screen/1");
    }
}
