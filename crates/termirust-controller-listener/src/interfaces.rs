use std::io;

use termirust_domain::{
    AddressFamily, ControllerListenPolicy, NetworkInterfaceCandidate, NetworkInterfaceId,
    NetworkInterfaceKind, is_private_controller_address,
};

use crate::{ListenerError, ListenerErrorCode};

pub trait InterfaceProvider: Send + Sync {
    fn eligible_interfaces(&self) -> Result<Vec<NetworkInterfaceCandidate>, ListenerError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemInterfaceProvider;

impl InterfaceProvider for SystemInterfaceProvider {
    fn eligible_interfaces(&self) -> Result<Vec<NetworkInterfaceCandidate>, ListenerError> {
        let mut candidates = if_addrs::get_if_addrs()
            .map_err(ListenerError::from)?
            .into_iter()
            .filter(|interface| {
                interface.is_oper_up()
                    && !interface.is_loopback()
                    && is_private_controller_address(interface.ip())
            })
            .filter_map(|interface| {
                let id = stable_interface_id(&interface).ok()?;
                let address = interface.ip();
                let candidate = NetworkInterfaceCandidate {
                    id,
                    label: interface.name.clone(),
                    kind: classify_interface(&interface),
                    address_family: if address.is_ipv4() {
                        AddressFamily::Ipv4
                    } else {
                        AddressFamily::Ipv6
                    },
                    address,
                };
                candidate.validate().ok().map(|_| candidate)
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            left.id
                .as_str()
                .cmp(right.id.as_str())
                .then_with(|| left.address.cmp(&right.address))
        });
        candidates.dedup_by(|left, right| {
            left.id == right.id
                && left.address_family == right.address_family
                && left.address == right.address
        });
        Ok(candidates)
    }
}

pub fn resolve_selected_interface(
    policy: &ControllerListenPolicy,
    interfaces: &[NetworkInterfaceCandidate],
) -> Result<NetworkInterfaceCandidate, ListenerError> {
    let route = policy
        .route()?
        .ok_or_else(|| ListenerError::new(ListenerErrorCode::Disabled))?;
    interfaces
        .iter()
        .find(|candidate| {
            candidate.id == route.interface_id
                && candidate.address_family == route.address_family
                && candidate.address == route.address
        })
        .cloned()
        .ok_or_else(|| ListenerError::new(ListenerErrorCode::InterfaceGone))
}

fn stable_interface_id(interface: &if_addrs::Interface) -> Result<NetworkInterfaceId, io::Error> {
    #[cfg(windows)]
    let stable = format!("{}:{}", interface.adapter_name, interface.name);
    #[cfg(not(windows))]
    let stable = format!("{}:{}", interface.index.unwrap_or_default(), interface.name);
    NetworkInterfaceId::new(stable)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid interface identifier"))
}

fn classify_interface(interface: &if_addrs::Interface) -> NetworkInterfaceKind {
    let name = interface.name.to_ascii_lowercase();
    if interface.is_p2p()
        || ["utun", "tun", "tap", "wg", "ppp", "tailscale", "zerotier"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
    {
        NetworkInterfaceKind::Vpn
    } else {
        NetworkInterfaceKind::Lan
    }
}
