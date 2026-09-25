//! The announcement has to survive the computer changing network.
//!
//! A phone that moves with its computer has one way back: the saved address stops answering, and
//! the Bonjour announcement is what tells it where the computer went. When the computer's own
//! address changes underneath a running listener, that announcement is the whole recovery path,
//! so it is checked here against a real mDNS daemon rather than against bookkeeping.

use std::net::IpAddr;
use std::time::Duration;

use multiplex_controller_listener::{BONJOUR_SERVICE_TYPE, BonjourAnnouncement, discovery_id};
use multiplex_domain::{HostPublicKey, ListeningAddress, NetworkInterfaceId, NetworkInterfaceKind};

/// A LAN address on a real interface of this machine, so the daemon has somewhere to announce.
fn lan_address(interface: &str, address: IpAddr, port: u16) -> ListeningAddress {
    ListeningAddress {
        interface_id: NetworkInterfaceId::new(format!("1:{interface}")).unwrap(),
        label: interface.into(),
        kind: NetworkInterfaceKind::Lan,
        address: std::net::SocketAddr::new(address, port),
    }
}

/// The first non-loopback IPv4 interface, or `None` on a machine without one.
fn first_lan_interface() -> Option<(String, IpAddr)> {
    for interface in if_addrs::get_if_addrs().ok()? {
        if interface.is_loopback() {
            continue;
        }
        if let std::net::IpAddr::V4(v4) = interface.ip()
            && !v4.is_link_local()
        {
            return Some((interface.name.clone(), interface.ip()));
        }
    }
    None
}

/// Whether the announcement for `id` is visible on this network within `timeout`.
fn announced(id: &str, timeout: Duration) -> bool {
    let Ok(daemon) = mdns_sd::ServiceDaemon::new() else {
        return false;
    };
    let Ok(receiver) = daemon.browse(BONJOUR_SERVICE_TYPE) else {
        return false;
    };
    let deadline = std::time::Instant::now() + timeout;
    let mut found = false;
    while std::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match receiver.recv_timeout(left) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                if info.get_property_val_str("id") == Some(id) {
                    found = true;
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    found
}

#[test]
#[ignore = "needs a real LAN interface and multicast on it"]
fn the_announcement_follows_the_computer_onto_a_new_address() {
    let Some((interface, address)) = first_lan_interface() else {
        eprintln!("no LAN interface; skipping");
        return;
    };
    let host = HostPublicKey([11; 32]);
    let id = discovery_id(host);
    let mut announcement = BonjourAnnouncement::start(host).expect("daemon");

    announcement
        .update(&[lan_address(&interface, address, 55_101)])
        .expect("first announcement");
    assert!(
        announced(&id, Duration::from_secs(5)),
        "the computer is not announced at all on {interface}"
    );

    // The same interface, a different address: what a laptop does when it joins another network.
    announcement
        .update(&[lan_address(&interface, address, 55_102)])
        .expect("second announcement");
    assert!(
        announced(&id, Duration::from_secs(5)),
        "the computer stopped announcing itself after its address changed, so a phone whose \
         saved address no longer answers has no way left to find it"
    );
}
