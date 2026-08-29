use serde::Deserialize;
use termirust_controller_listener::{
    BridgeAuthorization, BridgeCommand, BridgeCommandKind, ListenerErrorCode,
};
use termirust_domain::{
    ControllerAuthorizationRequest, ControllerCapabilities, ControllerCapability,
    ControllerDeviceAuthority, ControllerDeviceId, ControllerProtocolRange, DevicePublicKey,
    HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef, HostIdentityState,
    HostPublicKey, OccupantGeneration, PairedDeviceRecord, PairedDeviceStatus, PairingOfferId,
};
use uuid::Uuid;

#[derive(Deserialize)]
struct GoldenFixture {
    session: GoldenSession,
    controller: GoldenController,
    scenarios: Vec<String>,
}

#[derive(Deserialize)]
struct GoldenSession {
    session_generation: u64,
}

#[derive(Deserialize)]
struct GoldenController {
    device_id: Uuid,
    revocation_epoch: u64,
    capability_bits: u16,
}

fn golden() -> GoldenFixture {
    serde_json::from_slice(include_bytes!(
        "../../../tests/fixtures/universal-session-v1/golden.json"
    ))
    .unwrap()
}

fn authority() -> (
    ControllerDeviceAuthority,
    ControllerDeviceId,
    DevicePublicKey,
) {
    let golden = golden();
    let device_id = ControllerDeviceId::from_uuid(golden.controller.device_id);
    let public_key = DevicePublicKey([8; 32]);
    let capabilities =
        ControllerCapabilities::from_bits(golden.controller.capability_bits).unwrap();
    (
        ControllerDeviceAuthority {
            identity: Some(HostIdentityPublic::new(
                HostIdentityGeneration::INITIAL,
                HostPublicKey([7; 32]),
            )),
            secret_ref: Some(HostIdentitySecretRef::new("identity:test").unwrap()),
            state: HostIdentityState::Ready,
            revocation_epoch: golden.controller.revocation_epoch,
            session_generation: golden.session.session_generation,
            devices: vec![PairedDeviceRecord {
                device_id,
                public_key,
                display_name: "Test phone".to_owned(),
                capabilities,
                protocol_range: ControllerProtocolRange::V1,
                created_at: 1,
                last_seen_at: None,
                revocation_epoch: golden.controller.revocation_epoch,
                identity_generation: HostIdentityGeneration::INITIAL,
                status: PairedDeviceStatus::Online,
                source_offer_id: PairingOfferId::new(),
            }],
            offers: Vec::new(),
            attempts: Default::default(),
        },
        device_id,
        public_key,
    )
}

fn request(
    device_id: ControllerDeviceId,
    public_key: DevicePublicKey,
) -> ControllerAuthorizationRequest {
    let golden = golden();
    ControllerAuthorizationRequest {
        device_id,
        public_key,
        identity_generation: HostIdentityGeneration::INITIAL,
        capability: ControllerCapability::ObserveSessions,
        revocation_epoch: golden.controller.revocation_epoch,
        session_generation: golden.session.session_generation,
        now_millis: 100,
        deadline_millis: 200,
    }
}

fn command(kind: BridgeCommandKind) -> BridgeCommand {
    BridgeCommand {
        kind,
        session_id: None,
        occupant_generation: (kind != BridgeCommandKind::ListSessions)
            .then_some(OccupantGeneration::new(9)),
        session_generation: golden().session.session_generation,
        deadline_millis: 200,
    }
}

#[test]
fn every_command_rechecks_capability_epoch_deadline_generation_and_writer_lease() {
    let (authority, device_id, public_key) = authority();
    let bridge = BridgeAuthorization::new(&authority);
    assert!(
        bridge
            .authorize(
                request(device_id, public_key),
                command(BridgeCommandKind::ListSessions),
                None,
                false,
            )
            .is_ok()
    );
    assert!(
        bridge
            .authorize(
                request(device_id, public_key),
                command(BridgeCommandKind::AcquireWriter),
                Some(OccupantGeneration::new(9)),
                false,
            )
            .is_ok()
    );
    assert!(
        bridge
            .authorize(
                request(device_id, public_key),
                command(BridgeCommandKind::Input),
                Some(OccupantGeneration::new(9)),
                true,
            )
            .is_ok()
    );

    let mut stale_epoch = request(device_id, public_key);
    stale_epoch.revocation_epoch = 2;
    assert_eq!(
        bridge
            .authorize(
                stale_epoch,
                command(BridgeCommandKind::Input),
                Some(OccupantGeneration::new(9)),
                true,
            )
            .unwrap_err()
            .code,
        ListenerErrorCode::Unauthorized
    );

    let mut expired = request(device_id, public_key);
    expired.now_millis = 201;
    assert_eq!(
        bridge
            .authorize(
                expired,
                command(BridgeCommandKind::Input),
                Some(OccupantGeneration::new(9)),
                true,
            )
            .unwrap_err()
            .code,
        ListenerErrorCode::Unauthorized
    );

    assert_eq!(
        bridge
            .authorize(
                request(device_id, public_key),
                command(BridgeCommandKind::Input),
                Some(OccupantGeneration::new(10)),
                true,
            )
            .unwrap_err()
            .code,
        ListenerErrorCode::StaleGeneration
    );
    assert_eq!(
        bridge
            .authorize(
                request(device_id, public_key),
                command(BridgeCommandKind::Input),
                Some(OccupantGeneration::new(9)),
                false,
            )
            .unwrap_err()
            .code,
        ListenerErrorCode::WriterLeaseRequired
    );
    assert_eq!(
        bridge
            .authorize(
                request(device_id, public_key),
                command(BridgeCommandKind::ReleaseWriter),
                Some(OccupantGeneration::new(9)),
                false,
            )
            .unwrap_err()
            .code,
        ListenerErrorCode::WriterLeaseRequired
    );
}

#[test]
fn revocation_immediately_invalidates_existing_peer_claims() {
    assert!(
        golden()
            .scenarios
            .iter()
            .any(|scenario| scenario == "revocation_stops_mutation")
    );
    let (mut authority, device_id, public_key) = authority();
    authority.revoke_device(device_id).unwrap();
    let bridge = BridgeAuthorization::new(&authority);
    assert_eq!(
        bridge
            .authorize(
                request(device_id, public_key),
                command(BridgeCommandKind::ListSessions),
                None,
                false,
            )
            .unwrap_err()
            .code,
        ListenerErrorCode::Unauthorized
    );
}
