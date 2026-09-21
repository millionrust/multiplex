use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;

use multiplex_controller_listener::{
    ControllerBinder, GeneratedPortSource, InterfaceProvider, ListenerError, ListenerErrorCode,
    bind_private_addresses,
};
use multiplex_domain::{
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

/// Records every bind and refuses the listed addresses as already in use, and the reserved ports
/// the way Windows refuses a port inside an excluded range: as permission denied.
#[derive(Default)]
struct RecordingBinder {
    attempts: Mutex<Vec<SocketAddr>>,
    in_use: HashSet<SocketAddr>,
    busy_ports: HashSet<u16>,
    reserved_ports: HashSet<u16>,
}

impl ControllerBinder for RecordingBinder {
    type Listener = SocketAddr;

    fn bind_exact(&self, address: SocketAddr) -> io::Result<Self::Listener> {
        self.attempts.lock().unwrap().push(address);
        if self.in_use.contains(&address) || self.busy_ports.contains(&address.port()) {
            Err(io::Error::new(io::ErrorKind::AddrInUse, "fixture conflict"))
        } else if self.reserved_ports.contains(&address.port()) {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "fixture excluded port range",
            ))
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

fn candidate(id: &str, kind: NetworkInterfaceKind, address: &str) -> NetworkInterfaceCandidate {
    let address: IpAddr = address.parse().unwrap();
    NetworkInterfaceCandidate {
        id: NetworkInterfaceId::new(id).unwrap(),
        label: id.split(':').next_back().unwrap().to_owned(),
        kind,
        address_family: if address.is_ipv4() {
            AddressFamily::Ipv4
        } else {
            AddressFamily::Ipv6
        },
        address,
    }
}

fn home_and_tailscale() -> Interfaces {
    Interfaces(vec![
        candidate("4:en0", NetworkInterfaceKind::Lan, "192.168.88.4"),
        candidate("21:utun4", NetworkInterfaceKind::Vpn, "100.81.253.53"),
        candidate(
            "21:utun4",
            NetworkInterfaceKind::Vpn,
            "fd7a:115c:a1e0::8e01:fdaa",
        ),
    ])
}

fn policy(enabled: bool, port: ControllerPort) -> ControllerListenPolicy {
    ControllerListenPolicy {
        enabled,
        port: Some(port),
        discovery: DiscoveryPolicy::Off,
    }
}

fn socket(value: &str) -> SocketAddr {
    value.parse().unwrap()
}

#[test]
fn every_private_address_binds_on_the_same_port() {
    let binder = RecordingBinder::default();
    let bound = bind_private_addresses(
        &policy(true, ControllerPort::Generated(55_000)),
        &home_and_tailscale(),
        &binder,
        &mut Ports(Vec::new()),
    )
    .unwrap();

    assert_eq!(bound.port, ControllerPort::Generated(55_000));
    let expected = vec![
        socket("192.168.88.4:55000"),
        socket("100.81.253.53:55000"),
        socket("[fd7a:115c:a1e0::8e01:fdaa]:55000"),
    ];
    assert_eq!(
        bound
            .bound
            .iter()
            .map(|bound| bound.listener)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        bound
            .addresses()
            .iter()
            .map(|address| (address.label.as_str(), address.kind))
            .collect::<Vec<_>>(),
        [
            ("en0", NetworkInterfaceKind::Lan),
            ("utun4", NetworkInterfaceKind::Vpn),
            ("utun4", NetworkInterfaceKind::Vpn)
        ]
    );
    assert_eq!(*binder.attempts.lock().unwrap(), expected);
}

#[test]
fn disabled_policy_binds_nothing() {
    let binder = RecordingBinder::default();
    let error = bind_private_addresses(
        &policy(false, ControllerPort::UserFixed(9_999)),
        &home_and_tailscale(),
        &binder,
        &mut Ports(Vec::new()),
    )
    .unwrap_err();
    assert_eq!(error.code, ListenerErrorCode::Disabled);
    assert!(binder.attempts.lock().unwrap().is_empty());
}

#[test]
fn wildcard_loopback_and_public_addresses_never_reach_the_socket() {
    let mut interfaces = home_and_tailscale();
    for address in [
        "0.0.0.0",
        "127.0.0.1",
        "8.8.8.8",
        "::",
        "fe80::1",
        "2001:db8::1",
    ] {
        interfaces
            .0
            .push(candidate("9:bad0", NetworkInterfaceKind::Lan, address));
    }
    let binder = RecordingBinder::default();
    let bound = bind_private_addresses(
        &policy(true, ControllerPort::UserFixed(9_999)),
        &interfaces,
        &binder,
        &mut Ports(Vec::new()),
    )
    .unwrap();
    assert_eq!(bound.bound.len(), 3);
    assert!(
        binder
            .attempts
            .lock()
            .unwrap()
            .iter()
            .all(|address| multiplex_domain::is_private_controller_address(address.ip()))
    );
}

#[test]
fn a_computer_without_a_private_network_waits_instead_of_failing() {
    let binder = RecordingBinder::default();
    let bound = bind_private_addresses(
        &policy(true, ControllerPort::Generated(55_000)),
        &Interfaces(Vec::new()),
        &binder,
        &mut Ports(Vec::new()),
    )
    .unwrap();
    assert!(bound.bound.is_empty());
    assert_eq!(bound.port, ControllerPort::Generated(55_000));
    assert!(binder.attempts.lock().unwrap().is_empty());
}

#[test]
fn a_port_taken_on_one_address_stays_for_the_others() {
    let binder = RecordingBinder {
        in_use: HashSet::from([socket("100.81.253.53:55000")]),
        ..RecordingBinder::default()
    };
    let bound = bind_private_addresses(
        &policy(true, ControllerPort::Generated(55_000)),
        &home_and_tailscale(),
        &binder,
        &mut Ports(vec![60_000]),
    )
    .unwrap();
    assert_eq!(bound.port, ControllerPort::Generated(55_000));
    assert_eq!(bound.bound.len(), 2);
}

#[test]
fn a_generated_port_moves_only_when_no_address_can_use_it() {
    let binder = RecordingBinder {
        busy_ports: HashSet::from([55_000, 55_001]),
        ..RecordingBinder::default()
    };
    let bound = bind_private_addresses(
        &policy(true, ControllerPort::Generated(55_000)),
        &home_and_tailscale(),
        &binder,
        &mut Ports(vec![55_001, 55_002]),
    )
    .unwrap();
    assert_eq!(bound.port, ControllerPort::Generated(55_002));
    assert_eq!(bound.bound.len(), 3);

    let every_port_busy = RecordingBinder {
        busy_ports: (55_000..55_000 + MAX_GENERATED_PORT_ATTEMPTS as u16).collect(),
        ..RecordingBinder::default()
    };
    let single = Interfaces(vec![candidate(
        "4:en0",
        NetworkInterfaceKind::Lan,
        "192.168.88.4",
    )]);
    assert_eq!(
        bind_private_addresses(
            &policy(true, ControllerPort::Generated(55_000)),
            &single,
            &every_port_busy,
            &mut Ports((55_001..55_001 + 15).collect()),
        )
        .unwrap_err()
        .code,
        ListenerErrorCode::PortConflict
    );
    assert_eq!(
        every_port_busy.attempts.lock().unwrap().len(),
        MAX_GENERATED_PORT_ATTEMPTS
    );
}

#[test]
fn a_fixed_port_is_never_replaced() {
    let binder = RecordingBinder {
        busy_ports: HashSet::from([9_999]),
        ..RecordingBinder::default()
    };
    assert_eq!(
        bind_private_addresses(
            &policy(true, ControllerPort::UserFixed(9_999)),
            &home_and_tailscale(),
            &binder,
            &mut Ports(vec![60_000]),
        )
        .unwrap_err()
        .code,
        ListenerErrorCode::PortConflict
    );
    assert_eq!(binder.attempts.lock().unwrap().len(), 3);
}

#[test]
fn a_generated_port_inside_a_reserved_range_is_replaced() {
    let binder = RecordingBinder {
        reserved_ports: HashSet::from([55_000]),
        ..RecordingBinder::default()
    };
    let bound = bind_private_addresses(
        &policy(true, ControllerPort::Generated(55_000)),
        &home_and_tailscale(),
        &binder,
        &mut Ports(vec![61_000]),
    )
    .unwrap();
    assert_eq!(bound.port, ControllerPort::Generated(61_000));
    assert_eq!(bound.bound.len(), 3);
}

#[test]
fn a_chosen_port_inside_a_reserved_range_is_reported_not_replaced() {
    let binder = RecordingBinder {
        reserved_ports: HashSet::from([9_999]),
        ..RecordingBinder::default()
    };
    let error = bind_private_addresses(
        &policy(true, ControllerPort::UserFixed(9_999)),
        &home_and_tailscale(),
        &binder,
        &mut Ports(vec![61_000]),
    )
    .unwrap_err();
    assert_eq!(error.code, ListenerErrorCode::PermissionDenied);
    assert!(
        binder
            .attempts
            .lock()
            .unwrap()
            .iter()
            .all(|address| address.port() == 9_999)
    );
}
