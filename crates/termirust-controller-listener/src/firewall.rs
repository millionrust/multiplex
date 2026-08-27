use termirust_domain::RouteCandidate;

use crate::{ListenerError, ListenerErrorCode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirewallObservation {
    Allowed,
    Blocked,
    Unknown,
}

pub trait FirewallObserver: Send + Sync {
    fn observe(&self, route: &RouteCandidate) -> Result<FirewallObservation, ListenerError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemFirewallObserver;

impl FirewallObserver for SystemFirewallObserver {
    fn observe(&self, route: &RouteCandidate) -> Result<FirewallObservation, ListenerError> {
        route
            .port
            .validate()
            .map_err(|_| ListenerError::new(ListenerErrorCode::InvalidPolicy))?;
        if route.address.is_unspecified()
            || route.address.is_loopback()
            || route.discovery != termirust_domain::DiscoveryPolicy::Off
        {
            return Err(ListenerError::new(ListenerErrorCode::InvalidPolicy));
        }

        // A successful local bind cannot prove inbound reachability. Querying or changing
        // platform firewall state would require broader authority, so v1 reports Unknown.
        Ok(FirewallObservation::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use termirust_domain::{
        AddressFamily, ControllerListenPolicy, ControllerPort, DiscoveryPolicy, NetworkInterfaceId,
    };

    use super::*;

    #[test]
    fn system_observation_is_read_only_unknown_for_an_exact_private_route() {
        let policy = ControllerListenPolicy {
            enabled: true,
            interface_id: Some(NetworkInterfaceId::new("4:en0").unwrap()),
            address_family: Some(AddressFamily::Ipv4),
            selected_address: Some("192.168.1.20".parse::<IpAddr>().unwrap()),
            port: Some(ControllerPort::Generated(55_555)),
            discovery: DiscoveryPolicy::Off,
        };
        let route = policy.route().unwrap().unwrap();
        assert_eq!(
            SystemFirewallObserver.observe(&route).unwrap(),
            FirewallObservation::Unknown
        );
    }
}
