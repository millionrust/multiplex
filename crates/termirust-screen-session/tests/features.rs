//! Feature negotiation: what a host may send a viewer, and what a viewer may say back.
//!
//! The motion path is optional on both sides, so every combination has to end somewhere safe.
//! A host must never send video to a phone that cannot decode it, and a viewer must never be
//! disconnected for being older than the host.

use termirust_screen_codec::{Rect, Size};
use termirust_screen_protocol::{
    FeatureSet, Hello, MINIMUM_PROTOCOL_VERSION, Message, MotionCodec, Parity, SurfaceInfo,
    VideoConfig, VideoFrame,
};
use termirust_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, ResumeStore, SessionError, TicketVerifier,
    ViewerEvent, ViewerSession,
};

const TICKET: [u8; 32] = [7; 32];
const CACHE: usize = 64 << 20;

struct Tickets;

impl TicketVerifier for Tickets {
    fn verify(&mut self, proof: &[u8; 32]) -> Option<Grants> {
        (*proof == TICKET).then_some(Grants {
            device: 42,
            can_view: true,
            can_control: true,
        })
    }
}

fn surfaces() -> Vec<SurfaceInfo> {
    vec![SurfaceInfo {
        id: 1,
        size: Size::new(640, 400).unwrap(),
        scale_milli: 2000,
        name: "Built-in Display".to_owned(),
    }]
}

fn everything() -> FeatureSet {
    FeatureSet::from_bits(FeatureSet::KNOWN)
}

fn host_with(features: FeatureSet) -> HostSession<Tickets> {
    HostSession::new(
        surfaces(),
        Tickets,
        HostConfig {
            features,
            ..HostConfig::default()
        },
    )
}

/// Runs a hello and its welcome, and hands back both sides open.
fn open(
    host_features: FeatureSet,
    viewer_features: FeatureSet,
) -> (HostSession<Tickets>, ViewerSession, ResumeStore) {
    let mut host = host_with(host_features);
    let mut viewer = ViewerSession::with_features(CACHE, viewer_features);
    let mut store = ResumeStore::default();
    viewer.connect(TICKET);
    while let Some(message) = viewer.poll_outgoing() {
        host.receive(message, &mut store).expect("hello accepted");
    }
    while let Some(message) = host.poll_outgoing() {
        viewer.receive(message).expect("welcome accepted");
    }
    (host, viewer, store)
}

#[test]
fn both_sides_settle_on_what_they_share() {
    let viewer_can = FeatureSet::none()
        .with(FeatureSet::MOTION_VIDEO)
        .with(FeatureSet::VIDEO_PARITY);
    let host_can = FeatureSet::none()
        .with(FeatureSet::MOTION_VIDEO)
        .with(FeatureSet::LONG_TERM_REFERENCES);
    let (host, viewer, _) = open(host_can, viewer_can);
    for settled in [host.agreed_features(), viewer.agreed_features()] {
        assert!(settled.has(FeatureSet::MOTION_VIDEO));
        assert!(
            !settled.has(FeatureSet::VIDEO_PARITY),
            "the host cannot produce parity"
        );
        assert!(
            !settled.has(FeatureSet::LONG_TERM_REFERENCES),
            "the viewer cannot acknowledge references"
        );
    }
}

#[test]
fn a_stage_a_viewer_agrees_to_nothing_and_stays_open() {
    let (host, viewer, _) = open(everything(), FeatureSet::none());
    assert!(host.is_open(), "an older viewer is still a viewer");
    assert_eq!(host.agreed_features(), FeatureSet::none());
    assert_eq!(viewer.agreed_features(), FeatureSet::none());
}

#[test]
fn a_version_1_hello_is_still_welcomed() {
    let mut host = host_with(everything());
    let mut store = ResumeStore::default();
    let events = host
        .receive(
            Message::Hello(Hello {
                version: MINIMUM_PROTOCOL_VERSION,
                ticket_proof: TICKET,
                cache_bytes: CACHE as u64,
                resume: None,
                features: FeatureSet::none(),
            }),
            &mut store,
        )
        .expect("a version 1 viewer is accepted");
    assert!(!events.is_empty());
    assert!(host.is_open());
    assert_eq!(host.agreed_features(), FeatureSet::none());
    let welcome = host.poll_outgoing().expect("a welcome");
    match welcome {
        Message::Welcome(welcome) => {
            // Answered in the viewer's version: a Stage A build refuses anything else.
            assert_eq!(welcome.version, MINIMUM_PROTOCOL_VERSION);
            assert_eq!(welcome.features, FeatureSet::none());
        }
        other => panic!("expected a welcome, got {other:?}"),
    }
}

#[test]
fn a_negotiated_viewer_receives_video() {
    let (_, mut viewer, _) = open(everything(), everything());
    let config = VideoConfig {
        surface: 1,
        codec: MotionCodec::Hevc,
        rect: Rect::new(0, 0, 320, 200),
        payload: vec![0x40, 0x01],
    };
    assert_eq!(
        viewer.receive(Message::VideoConfig(config.clone())),
        Ok(vec![ViewerEvent::VideoConfig(config)])
    );
    let frame = VideoFrame {
        surface: 1,
        sequence: 1,
        keyframe: true,
        token: Some(3),
        payload: vec![0xAA],
    };
    assert_eq!(
        viewer.receive(Message::VideoFrame(frame.clone())),
        Ok(vec![ViewerEvent::VideoFrame(frame)])
    );
    // Parity is the viewer's own business: it repairs what was lost and produces no event of its
    // own. This group lost nothing, so there is nothing to produce.
    let parity = Parity {
        surface: 1,
        group: 0,
        index: 0,
        data_shards: 4,
        payload: vec![0x5A],
    };
    assert_eq!(viewer.receive(Message::Parity(parity)), Ok(Vec::new()));
}

#[test]
fn video_to_a_viewer_that_never_asked_for_it_ends_the_session() {
    let (_, mut viewer, _) = open(FeatureSet::none(), FeatureSet::none());
    assert_eq!(
        viewer.receive(Message::VideoFrame(VideoFrame {
            surface: 1,
            sequence: 1,
            keyframe: true,
            token: None,
            payload: vec![0xAA],
        })),
        Err(SessionError::ProtocolViolation),
        "a host sending video here is talking to the wrong peer"
    );
}

#[test]
fn parity_needs_its_own_bit_even_when_video_was_agreed() {
    let video_only = FeatureSet::none().with(FeatureSet::MOTION_VIDEO);
    let (_, mut viewer, _) = open(everything(), video_only);
    assert!(
        viewer
            .receive(Message::VideoFrame(VideoFrame {
                surface: 1,
                sequence: 1,
                keyframe: true,
                token: None,
                payload: vec![0xAA],
            }))
            .is_ok()
    );
    assert_eq!(
        viewer.receive(Message::Parity(Parity {
            surface: 1,
            group: 0,
            index: 0,
            data_shards: 4,
            payload: vec![0x5A],
        })),
        Err(SessionError::ProtocolViolation)
    );
}

#[test]
fn acknowledgements_from_a_viewer_that_never_agreed_end_the_session() {
    let (mut host, _, mut store) = open(everything(), FeatureSet::none());
    assert_eq!(
        host.receive(
            Message::VideoAcknowledge {
                surface: 1,
                tokens: vec![3],
            },
            &mut store,
        ),
        Err(SessionError::ProtocolViolation)
    );
    assert!(!host.is_open());

    let (mut host, _, mut store) = open(everything(), everything());
    assert_eq!(
        host.receive(
            Message::VideoAcknowledge {
                surface: 1,
                tokens: vec![3],
            },
            &mut store,
        ),
        Ok(vec![HostEvent::VideoAcknowledged {
            surface: 1,
            tokens: vec![3],
        }]),
        "a viewer that did agree is not disconnected, and what it holds reaches the encoder"
    );
    assert_eq!(
        host.receive(
            Message::VideoLost {
                surface: 1,
                sequence: 9,
            },
            &mut store,
        ),
        Ok(vec![HostEvent::VideoLost {
            surface: 1,
            sequence: 9,
        }])
    );
    assert!(host.is_open());
}
