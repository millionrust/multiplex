use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;

use termirust_controller_listener::{
    ControllerBinder, GeneratedPortSource, InterfaceProvider, ListenerError, ListenerErrorCode,
    bind_selected_route,
};
use termirust_domain::{
    AddressFamily, ControllerListenPolicy, ControllerPort, DiscoveryPolicy,
    MAX_GENERATED_PORT_ATTEMPTS, NetworkInterfaceCandidate, NetworkInterfaceId,
    NetworkInterfaceKind,
};

#[derive(Clone)]
struct Interfaces(Vec<NetworkInterfaceCandidate>);

impl InterfaceProvider for Interfaces {
    fn eligible_interfaces(&self) -> Result<Vec<NetworkInterfaceCandidate>, ListenerError> {
        Ok(self.0.clone())
    }
}

#[derive(Default)]
struct RecordingBinder {
    attempts: Mutex<Vec<SocketAddr>>,
    conflicts: usize,
}

impl ControllerBinder for RecordingBinder {
    type Listener = SocketAddr;

    fn bind_exact(&self, address: SocketAddr) -> io::Result<Self::Listener> {
        let mut attempts = self.attempts.lock().unwrap();
        attempts.push(address);
        if attempts.len() <= self.conflicts {
            Err(io::Error::new(io::ErrorKind::AddrInUse, "fixture conflict"))
        } else {
            Ok(address)
        }
    }
}

struct Ports(Vec<u16>);

impl GeneratedPortSource for Ports {
    fn next_port(&mut self) -> Result<u16, ListenerError> {
        if self.0.is_empty() {
            return Err(ListenerError::new(ListenerErrorCode::RandomUnavailable));
        }
        Ok(self.0.remove(0))
    }
}

fn candidate(id: &str, address: &str) -> NetworkInterfaceCandidate {
    let address: IpAddr = address.parse().unwrap();
    NetworkInterfaceCandidate {
        id: NetworkInterfaceId::new(id).unwrap(),
        label: "Private network".to_owned(),
        kind: NetworkInterfaceKind::Lan,
        address_family: if address.is_ipv4() {
            AddressFamily::Ipv4
        } else {
            AddressFamily::Ipv6
        },
        address,
    }
}

fn policy(enabled: bool, id: &str, address: &str, port: ControllerPort) -> ControllerListenPolicy {
    let address: IpAddr = address.parse().unwrap();
    ControllerListenPolicy {
        enabled,
        interface_id: Some(NetworkInterfaceId::new(id).unwrap()),
        address_family: Some(if address.is_ipv4() {
            AddressFamily::Ipv4
        } else {
            AddressFamily::Ipv6
        }),
        selected_address: Some(address),
        port: Some(port),
        discovery: DiscoveryPolicy::Off,
    }
}

#[test]
fn exact_selected_address_is_the_only_bind_and_discovery_remains_off() {
    let interfaces = Interfaces(vec![
        candidate("4:en0", "192.168.1.10"),
        candidate("9:utun3", "10.9.0.2"),
    ]);
    let binder = RecordingBinder::default();
    let mut ports = Ports(Vec::new());
    let bound = bind_selected_route(
        &policy(
            true,
            "9:utun3",
            "10.9.0.2",
            ControllerPort::UserFixed(9_999),
        ),
        &interfaces,
        &binder,
        &mut ports,
    )
    .unwrap();

    assert_eq!(bound.listener, "10.9.0.2:9999".parse().unwrap());
    assert_eq!(bound.route.discovery, DiscoveryPolicy::Off);
    assert_eq!(
        *binder.attempts.lock().unwrap(),
        vec!["10.9.0.2:9999".parse().unwrap()]
    );
}

#[test]
fn disabled_missing_changed_and_public_routes_fail_before_bind() {
    let interfaces = Interfaces(vec![candidate("4:en0", "192.168.1.10")]);
    for (input, code) in [
        (
            policy(
                false,
                "4:en0",
                "192.168.1.10",
                ControllerPort::UserFixed(9_999),
            ),
            ListenerErrorCode::Disabled,
        ),
        (
            policy(
                true,
                "4:en0",
                "192.168.1.11",
                ControllerPort::UserFixed(9_999),
            ),
            ListenerErrorCode::InterfaceGone,
        ),
    ] {
        let binder = RecordingBinder::default();
        let error =
            bind_selected_route(&input, &interfaces, &binder, &mut Ports(Vec::new())).unwrap_err();
        assert_eq!(error.code, code);
        assert!(binder.attempts.lock().unwrap().is_empty());
    }

    let mut public = policy(
        true,
        "4:en0",
        "192.168.1.10",
        ControllerPort::UserFixed(9_999),
    );
    public.selected_address = Some("0.0.0.0".parse().unwrap());
    let binder = RecordingBinder::default();
    assert_eq!(
        bind_selected_route(&public, &interfaces, &binder, &mut Ports(Vec::new()))
            .unwrap_err()
            .code,
        ListenerErrorCode::InvalidPolicy
    );
    assert!(binder.attempts.lock().unwrap().is_empty());
}

#[test]
fn generated_port_uses_persisted_value_then_at_most_fifteen_fresh_candidates() {
    let interfaces = Interfaces(vec![candidate("4:en0", "192.168.1.10")]);
    let binder = RecordingBinder {
        conflicts: 2,
        ..RecordingBinder::default()
    };
    let mut ports = Ports(vec![50_001, 50_002]);
    let bound = bind_selected_route(
        &policy(
            true,
            "4:en0",
            "192.168.1.10",
            ControllerPort::Generated(50_000),
        ),
        &interfaces,
        &binder,
        &mut ports,
    )
    .unwrap();
    assert_eq!(bound.route.port, ControllerPort::Generated(50_002));
    assert_eq!(binder.attempts.lock().unwrap().len(), 3);

    let binder = RecordingBinder {
        conflicts: MAX_GENERATED_PORT_ATTEMPTS,
        ..RecordingBinder::default()
    };
    let mut ports = Ports((50_001..50_001 + 15).collect());
    assert_eq!(
        bind_selected_route(
            &policy(
                true,
                "4:en0",
                "192.168.1.10",
                ControllerPort::Generated(50_000),
            ),
            &interfaces,
            &binder,
            &mut ports,
        )
        .unwrap_err()
        .code,
        ListenerErrorCode::PortConflict
    );
    assert_eq!(
        binder.attempts.lock().unwrap().len(),
        MAX_GENERATED_PORT_ATTEMPTS
    );
}
