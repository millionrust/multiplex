use termirust_controller_listener::{
    BridgeAuthorization, BridgeCommand, BridgeCommandKind, ListenerErrorCode,
};
use termirust_domain::{
    ControllerAuthorizationRequest, ControllerCapabilities, ControllerCapability,
    ControllerDeviceAuthority, ControllerDeviceId, ControllerProtocolRange, DevicePublicKey,
    HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef, HostIdentityState,
    HostPublicKey, PairedDeviceRecord, PairedDeviceStatus, PairingOfferId,
};

#[test]
fn remote_controller_bridge_rechecks_authority_before_host_dispatch() {
    let device_id = ControllerDeviceId::new();
    let public_key = DevicePublicKey([8; 32]);
    let authority = ControllerDeviceAuthority {
        identity: Some(HostIdentityPublic::new(
            HostIdentityGeneration::INITIAL,
            HostPublicKey([7; 32]),
        )),
        secret_ref: Some(HostIdentitySecretRef::new("identity:remote-test").unwrap()),
        state: HostIdentityState::Ready,
        revocation_epoch: 3,
        session_generation: 5,
        devices: vec![PairedDeviceRecord {
            device_id,
            public_key,
            display_name: "Remote test device".into(),
            capabilities: ControllerCapabilities::default()
                .with(ControllerCapability::ObserveSessions),
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
    };
    let request = ControllerAuthorizationRequest {
        device_id,
        public_key,
        identity_generation: HostIdentityGeneration::INITIAL,
        capability: ControllerCapability::ObserveSessions,
        revocation_epoch: 3,
        session_generation: 5,
        now_millis: 100,
        deadline_millis: 200,
    };
    let command = BridgeCommand {
        kind: BridgeCommandKind::ListSessions,
        session_id: None,
        occupant_generation: None,
        session_generation: 5,
        deadline_millis: 200,
    };
    assert!(
        BridgeAuthorization::new(&authority)
            .authorize(request, command, None, false)
            .is_ok()
    );

    let mut revoked = authority;
    revoked.revoke_device(device_id).unwrap();
    assert_eq!(
        BridgeAuthorization::new(&revoked)
            .authorize(request, command, None, false)
            .unwrap_err()
            .code,
        ListenerErrorCode::Unauthorized
    );
}
