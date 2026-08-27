use termirust_controller_listener::{
    BridgeAuthorization, BridgeCommand, BridgeCommandKind, ListenerErrorCode,
};
use termirust_domain::{
    ControllerAuthorizationRequest, ControllerCapabilities, ControllerCapability,
    ControllerDeviceAuthority, ControllerDeviceId, ControllerProtocolRange, DevicePublicKey,
    HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef, HostIdentityState,
    HostPublicKey, OccupantGeneration, PairedDeviceRecord, PairedDeviceStatus, PairingOfferId,
};

fn authority() -> (
    ControllerDeviceAuthority,
    ControllerDeviceId,
    DevicePublicKey,
) {
    let device_id = ControllerDeviceId::new();
    let public_key = DevicePublicKey([8; 32]);
    let capabilities = ControllerCapabilities::default()
        .with(ControllerCapability::ObserveSessions)
        .with(ControllerCapability::AttachOutput)
        .with(ControllerCapability::SendInput)
        .with(ControllerCapability::Resize)
        .with(ControllerCapability::RespondToApproval);
    (
        ControllerDeviceAuthority {
            identity: Some(HostIdentityPublic::new(
                HostIdentityGeneration::INITIAL,
                HostPublicKey([7; 32]),
            )),
            secret_ref: Some(HostIdentitySecretRef::new("identity:test").unwrap()),
            state: HostIdentityState::Ready,
            revocation_epoch: 3,
            session_generation: 5,
            devices: vec![PairedDeviceRecord {
                device_id,
                public_key,
                display_name: "Test phone".to_owned(),
                capabilities,
                protocol_range: ControllerProtocolRange::V1,
                created_at: 1,
                last_seen_at: None,
                revocation_epoch: 3,
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
    ControllerAuthorizationRequest {
        device_id,
        public_key,
        identity_generation: HostIdentityGeneration::INITIAL,
        capability: ControllerCapability::ObserveSessions,
        revocation_epoch: 3,
        session_generation: 5,
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
        session_generation: 5,
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
}

#[test]
fn revocation_immediately_invalidates_existing_peer_claims() {
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
