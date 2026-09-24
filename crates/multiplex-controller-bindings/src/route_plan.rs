//! The order and timing in which a phone tries a paired computer's addresses.
//!
//! This is policy only: no sockets, clocks, or threads. Each app races the attempts with its own
//! transports and reports back what happened; the rules live here once, for both apps, and
//! `tests/fixtures/controller-routes/route-plan-v1.json` holds the cases both apps replay.
//!
//! The rules, from `docs/route-selection-plan.md`:
//!
//! - The address that worked last time on this same network goes first.
//! - Then addresses Bonjour resolved just now, then local addresses in the phone's own subnet.
//! - Then Tailscale addresses, then every other saved address.
//! - Attempts are staggered, not queued: each starts at its `start_after_millis`, or as soon as
//!   every attempt started before it has failed, whichever comes first. The first to connect
//!   wins and the rest are cancelled before anything is sent on them, so a lost race never
//!   counts as a failed login on the computer.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use sha2::{Digest as _, Sha256};

/// Attempts start at least this far apart.
const STAGGER_MILLIS: u32 = 100;
/// Tailscale addresses start no earlier than this, giving the local network a head start.
const TAILSCALE_START_MILLIS: u32 = 250;
/// Every other saved address starts no earlier than this.
const OTHER_START_MILLIS: u32 = 500;
/// A local address that has not connected by now is not there.
const LOCAL_TIMEOUT_MILLIS: u32 = 3_000;
/// A tunnel may need to wake up or relay through a DERP server first.
const TUNNEL_TIMEOUT_MILLIS: u32 = 6_000;
/// The whole race gives up after this.
const RACE_DEADLINE_MILLIS: u32 = 12_000;
/// More than a host ever has; anything past it is not tried.
const MAX_ATTEMPTS: usize = 16;
/// Networks remembered per paired computer.
const MAX_REMEMBERED: usize = 8;
/// A better route has to be this much better before a live session moves to it. Below that the
/// difference is noise on a phone's radio, and moving costs a reattach.
const IMPROVEMENT_RATIO: f32 = 0.70;
/// One move per this long, so two routes that keep swapping places cannot flap the session.
const MIGRATION_COOLDOWN_MILLIS: u64 = 30_000;
/// A route that just failed is left alone for this long, however good it then looks.
const FAILURE_QUARANTINE_MILLIS: u64 = 60_000;
/// The first quiet probe for something better, and the longest the backoff reaches.
const FIRST_PROBE_MILLIS: u32 = 15_000;
const MAX_PROBE_MILLIS: u32 = 120_000;
const FINGERPRINT_DOMAIN: &[u8] = b"multiplex-route-network-v1\0";

#[derive(Clone, Debug, Eq, Hash, PartialEq, uniffi::Record)]
pub struct RouteAddress {
    pub address: String,
    pub port: u16,
}

/// What an address is, as far as its value says.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum RouteKind {
    /// RFC 1918, link-local, or a unique local IPv6 address outside Tailscale's range.
    LocalNetwork,
    /// Tailscale's 100.64.0.0/10 or `fd7a:115c:a1e0::/48`.
    Tailscale,
    /// Anything else, including a name rather than an address.
    OtherPrivate,
}

/// The kind of network the phone is on now.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum PhoneLink {
    Wifi,
    Ethernet,
    Cellular,
    Other,
    Offline,
}

/// One of the phone's own addresses on its current physical network, not a VPN's.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct PhoneAddress {
    pub address: String,
    pub prefix_length: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct PhoneNetwork {
    pub link: PhoneLink,
    pub addresses: Vec<PhoneAddress>,
    /// From [`network_fingerprint`]; `None` when the network cannot be told apart from others.
    pub fingerprint: Option<String>,
}

/// The address that last connected on one network.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct RememberedRoute {
    pub fingerprint: String,
    pub route: RouteAddress,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct PlannedAttempt {
    pub route: RouteAddress,
    pub kind: RouteKind,
    /// 0 remembered, 1 Bonjour or this subnet, 2 Tailscale, 3 anything else.
    pub tier: u8,
    pub start_after_millis: u32,
    pub timeout_millis: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct RoutePlan {
    pub attempts: Vec<PlannedAttempt>,
    /// Give up on the whole race this long after it started.
    pub deadline_millis: u32,
}

/// How one attempt ended, as the app's transport saw it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum AttemptResult {
    Connected,
    /// Something answered at the address and turned the connection away.
    Refused,
    /// The phone had no way to send there: no route, or the network said the host is unreachable.
    Unreachable,
    TimedOut,
    /// Cancelled because another attempt won, or the person gave up.
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct AttemptOutcome {
    pub route: RouteAddress,
    pub result: AttemptResult,
}

/// What to tell the person after a race, beyond "connected" or "could not connect".
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum RouteAdvice {
    None,
    /// Connected over Tailscale while the computer's address on this very network did not
    /// answer first, though it started 250 ms ahead. A Tailscale exit node without "Allow local
    /// network access" does exactly this. The local attempt is usually still pending when
    /// Tailscale wins and is cancelled, so a cancelled one counts as not answering.
    LocalNetworkBlockedWhileTailscaleWorks,
    /// Something answered at the computer's address and refused: Remote access is probably off.
    RemoteAccessOff,
    /// On Wi-Fi or Ethernet, with no saved address on this network and no Tailscale address.
    NotOnComputersNetwork,
    /// Away from any local network, with no Tailscale address to try.
    NeedsRemoteRoute,
    /// Every address was tried and none answered.
    ComputerUnreachable,
}

/// Orders and times the attempts for one connection.
///
/// `saved` is the host's saved addresses, most recently working first. `discovered` is what
/// Bonjour resolved for this host just now, if anything.
#[uniffi::export]
pub fn plan_routes(
    saved: Vec<RouteAddress>,
    discovered: Vec<RouteAddress>,
    network: PhoneNetwork,
    remembered: Vec<RememberedRoute>,
) -> RoutePlan {
    let remembered_here = network.fingerprint.as_ref().and_then(|fingerprint| {
        remembered
            .iter()
            .take(MAX_REMEMBERED)
            .find(|entry| &entry.fingerprint == fingerprint)
            .map(|entry| entry.route.clone())
    });
    let discovered: HashSet<RouteAddress> = discovered.iter().cloned().collect();
    let mut seen = HashSet::new();
    let mut ranked: Vec<(u8, RouteAddress, RouteKind)> = discovered_first(&discovered, saved)
        .into_iter()
        .filter(|route| seen.insert(route.clone()))
        .take(MAX_ATTEMPTS)
        .map(|route| {
            let kind = route_kind(&route.address);
            let tier = if discovered.contains(&route)
                || (kind == RouteKind::LocalNetwork && on_phone_subnet(&route.address, &network))
            {
                1
            } else if kind == RouteKind::Tailscale {
                2
            } else {
                3
            };
            (tier, route, kind)
        })
        .collect();
    // What worked here last time goes first, except that a tunnel never jumps ahead of an
    // address on the phone's own network: one slow evening on the local network must not keep
    // the phone on Tailscale at home for good. The local address still gets its head start.
    let local_here = ranked.iter().any(|(tier, _, _)| *tier == 1);
    if let Some(entry) = ranked
        .iter_mut()
        .find(|(_, route, _)| remembered_here.as_ref() == Some(route))
        && (entry.2 == RouteKind::LocalNetwork || !local_here)
    {
        entry.0 = 0;
    }
    // Stable, so saved order (most recently working first) holds within a tier.
    ranked.sort_by_key(|(tier, _, _)| *tier);

    let mut attempts = Vec::with_capacity(ranked.len());
    let mut previous: Option<u32> = None;
    for (tier, route, kind) in ranked {
        let floor = match tier {
            0 | 1 => 0,
            2 => TAILSCALE_START_MILLIS,
            _ => OTHER_START_MILLIS,
        };
        // The first attempt never waits: a head start only matters over an attempt ahead of it.
        let start = previous.map_or(0, |previous| floor.max(previous + STAGGER_MILLIS));
        let limit = if tier <= 1 && kind == RouteKind::LocalNetwork {
            LOCAL_TIMEOUT_MILLIS
        } else {
            TUNNEL_TIMEOUT_MILLIS
        };
        attempts.push(PlannedAttempt {
            route,
            kind,
            tier,
            start_after_millis: start,
            timeout_millis: limit.min(RACE_DEADLINE_MILLIS.saturating_sub(start)),
        });
        previous = Some(start);
    }
    // A host with a single address gets the whole race for it, as it did before.
    if let [only] = attempts.as_mut_slice() {
        only.timeout_millis = RACE_DEADLINE_MILLIS;
    }
    RoutePlan {
        attempts,
        deadline_millis: RACE_DEADLINE_MILLIS,
    }
}

/// Discovered addresses lead, in case one is new; saved ones follow in their own order.
fn discovered_first(
    discovered: &HashSet<RouteAddress>,
    saved: Vec<RouteAddress>,
) -> Vec<RouteAddress> {
    let mut fresh: Vec<RouteAddress> = discovered
        .iter()
        .filter(|route| !saved.contains(route))
        .cloned()
        .collect();
    fresh.sort_by(|left, right| (&left.address, left.port).cmp(&(&right.address, right.port)));
    saved
        .iter()
        .filter(|route| discovered.contains(route))
        .cloned()
        .chain(fresh)
        .chain(
            saved
                .iter()
                .filter(|route| !discovered.contains(route))
                .cloned(),
        )
        .collect()
}

/// A short, salted name for the phone's current network, so the address that worked here can be
/// tried first next time. Built from the phone's own subnets and the gateway, never a Wi-Fi name,
/// so it needs no location permission, and salted per install so it names nothing outside it.
#[uniffi::export]
pub fn network_fingerprint(
    salt: Vec<u8>,
    link: PhoneLink,
    addresses: Vec<PhoneAddress>,
    gateway: Option<String>,
) -> Option<String> {
    let mut parts: Vec<String> = match link {
        PhoneLink::Offline => return None,
        // The carrier changes the phone's address freely; the network is "cellular".
        PhoneLink::Cellular => vec!["cellular".to_owned()],
        PhoneLink::Wifi | PhoneLink::Ethernet | PhoneLink::Other => {
            let mut prefixes: Vec<String> = addresses.iter().filter_map(subnet_of).collect();
            if let Some(gateway) = gateway
                .as_deref()
                .and_then(|gateway| gateway.parse::<IpAddr>().ok())
            {
                prefixes.push(format!("gateway {gateway}"));
            }
            prefixes
        }
    };
    if parts.is_empty() {
        return None;
    }
    parts.sort();
    parts.dedup();
    let mut hash = Sha256::new();
    hash.update(FINGERPRINT_DOMAIN);
    hash.update((salt.len() as u64).to_be_bytes());
    hash.update(&salt);
    for part in &parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    let digest = hash.finalize();
    Some(
        digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// Records `route` as the one that connected on `fingerprint`, keeping the most recent networks.
#[uniffi::export]
pub fn remember_route(
    remembered: Vec<RememberedRoute>,
    fingerprint: String,
    route: RouteAddress,
) -> Vec<RememberedRoute> {
    std::iter::once(RememberedRoute {
        fingerprint: fingerprint.clone(),
        route,
    })
    .chain(
        remembered
            .into_iter()
            .filter(|entry| entry.fingerprint != fingerprint),
    )
    .take(MAX_REMEMBERED)
    .collect()
}

/// What, if anything, to tell the person after a race ended with `outcomes`.
#[uniffi::export]
pub fn route_advice(
    saved: Vec<RouteAddress>,
    network: PhoneNetwork,
    outcomes: Vec<AttemptOutcome>,
) -> RouteAdvice {
    let local_here = |route: &RouteAddress| {
        route_kind(&route.address) == RouteKind::LocalNetwork
            && on_phone_subnet(&route.address, &network)
    };
    let on_lan = matches!(network.link, PhoneLink::Wifi | PhoneLink::Ethernet);
    if let Some(winner) = outcomes
        .iter()
        .find(|outcome| outcome.result == AttemptResult::Connected)
    {
        let local_failed = outcomes.iter().any(|outcome| {
            local_here(&outcome.route)
                && matches!(
                    outcome.result,
                    AttemptResult::Unreachable | AttemptResult::TimedOut | AttemptResult::Cancelled
                )
        });
        return if on_lan
            && route_kind(&winner.route.address) == RouteKind::Tailscale
            && local_failed
        {
            RouteAdvice::LocalNetworkBlockedWhileTailscaleWorks
        } else {
            RouteAdvice::None
        };
    }
    if outcomes
        .iter()
        .any(|outcome| outcome.result == AttemptResult::Refused)
    {
        return RouteAdvice::RemoteAccessOff;
    }
    let has_tailscale = saved
        .iter()
        .any(|route| route_kind(&route.address) == RouteKind::Tailscale);
    if !has_tailscale {
        if on_lan && !saved.iter().any(local_here) {
            return RouteAdvice::NotOnComputersNetwork;
        }
        if matches!(network.link, PhoneLink::Cellular | PhoneLink::Offline) {
            return RouteAdvice::NeedsRemoteRoute;
        }
    }
    RouteAdvice::ComputerUnreachable
}

/// The route a session is running on now.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct LiveRoute {
    pub route: RouteAddress,
    pub kind: RouteKind,
    /// The measured round trip on this route, in milliseconds.
    pub round_trip_millis: u32,
    /// Whether the session is riding the relay rather than reaching the computer directly.
    pub over_relay: bool,
}

/// Why the phone is thinking about a better route.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum ProbeReason {
    /// The phone moved between networks, or a VPN came up or went down.
    NetworkChanged,
    /// Bonjour saw the computer on this network.
    HostDiscovered,
    /// Nothing happened; this is the background look.
    Quiet,
}

/// A route the phone has verified and could move the session to.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct MigrationCandidate {
    pub route: RouteAddress,
    pub kind: RouteKind,
    pub round_trip_millis: u32,
    pub over_relay: bool,
    /// How long ago this route last failed, if it has.
    pub millis_since_failed: Option<u64>,
}

/// Whether to move a live session, and when not, why not.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum MigrationDecision {
    Migrate,
    /// Not enough better to be worth the reattach.
    NotBetter,
    /// A tunnel cannot take a session off the computer's own network.
    LocalRouteKept,
    /// Something moved less than thirty seconds ago.
    TooSoon,
    /// That route failed within the last minute.
    RecentlyFailed,
    /// A paste or a writer command is on the wire, and moving would lose it.
    Busy,
}

/// How long to wait before looking for a better route, or `None` when there is nothing better to
/// find.
///
/// A session already on the computer's own network is as close as it gets, so the phone stops
/// looking; anything else is worth a look, at once when the network changed or Bonjour spoke, and
/// on a backing-off timer otherwise. `quiet_probes` counts the background looks already made
/// since the last change.
#[uniffi::export]
pub fn next_probe_after_millis(
    live: LiveRoute,
    reason: ProbeReason,
    quiet_probes: u32,
) -> Option<u32> {
    if live.kind == RouteKind::LocalNetwork && !live.over_relay {
        return None;
    }
    match reason {
        ProbeReason::NetworkChanged | ProbeReason::HostDiscovered => Some(0),
        ProbeReason::Quiet => Some(
            FIRST_PROBE_MILLIS
                .saturating_mul(1u32.checked_shl(quiet_probes.min(8)).unwrap_or(256))
                .min(MAX_PROBE_MILLIS),
        ),
    }
}

/// Whether a verified candidate should take over a live session.
///
/// The order matters: the reasons not to move are all checked before the reasons to move, so a
/// route that would otherwise win still waits out a cooldown or a recent failure.
#[uniffi::export]
pub fn migration_decision(
    live: LiveRoute,
    candidate: MigrationCandidate,
    millis_since_last_migration: Option<u64>,
    writer_command_in_flight: bool,
) -> MigrationDecision {
    if candidate.route == live.route {
        return MigrationDecision::NotBetter;
    }
    if writer_command_in_flight {
        return MigrationDecision::Busy;
    }
    if candidate
        .millis_since_failed
        .is_some_and(|since| since < FAILURE_QUARANTINE_MILLIS)
    {
        return MigrationDecision::RecentlyFailed;
    }
    if millis_since_last_migration.is_some_and(|since| since < MIGRATION_COOLDOWN_MILLIS) {
        return MigrationDecision::TooSoon;
    }
    // Nothing beats being on the computer's own network, whatever the clock says: a tunnel that
    // measures faster is measuring something else.
    if live.kind == RouteKind::LocalNetwork
        && !live.over_relay
        && candidate.kind != RouteKind::LocalNetwork
    {
        return MigrationDecision::LocalRouteKept;
    }
    // Any direct route beats the relay, however close the times are.
    if live.over_relay && !candidate.over_relay {
        return MigrationDecision::Migrate;
    }
    if candidate.over_relay && !live.over_relay {
        return MigrationDecision::NotBetter;
    }
    if (candidate.round_trip_millis as f32) <= live.round_trip_millis as f32 * IMPROVEMENT_RATIO {
        MigrationDecision::Migrate
    } else {
        MigrationDecision::NotBetter
    }
}

/// Classifies an address by its value alone.
#[uniffi::export]
pub fn route_kind(address: &str) -> RouteKind {
    match parse_address(address) {
        Some(IpAddr::V4(address)) if is_tailscale_v4(address) => RouteKind::Tailscale,
        Some(IpAddr::V4(address)) if address.is_private() || address.is_link_local() => {
            RouteKind::LocalNetwork
        }
        Some(IpAddr::V6(address)) if is_tailscale_v6(address) => RouteKind::Tailscale,
        Some(IpAddr::V6(address))
            if (address.segments()[0] & 0xfe00) == 0xfc00
                || (address.segments()[0] & 0xffc0) == 0xfe80 =>
        {
            RouteKind::LocalNetwork
        }
        _ => RouteKind::OtherPrivate,
    }
}

fn parse_address(address: &str) -> Option<IpAddr> {
    let trimmed = address.trim_start_matches('[').trim_end_matches(']');
    // A scoped link-local address (`fe80::1%en0`) names its interface after the `%`.
    let unscoped = trimmed.split('%').next().unwrap_or(trimmed);
    unscoped.parse().ok()
}

fn is_tailscale_v4(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    first == 100 && (second & 0xc0) == 64
}

fn is_tailscale_v6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
}

/// Whether `address` is on the same subnet as one of the phone's own addresses. A link-local
/// IPv6 address is reachable on any local link the phone has.
fn on_phone_subnet(address: &str, network: &PhoneNetwork) -> bool {
    if !matches!(
        network.link,
        PhoneLink::Wifi | PhoneLink::Ethernet | PhoneLink::Other
    ) {
        return false;
    }
    let Some(target) = parse_address(address) else {
        return false;
    };
    network.addresses.iter().any(|own| {
        let Some(own_address) = parse_address(&own.address) else {
            return false;
        };
        match (own_address, target) {
            (IpAddr::V4(own_address), IpAddr::V4(target)) => {
                !is_tailscale_v4(own_address)
                    && same_prefix(
                        &own_address.octets(),
                        &target.octets(),
                        own.prefix_length.min(32),
                    )
            }
            (IpAddr::V6(own_address), IpAddr::V6(target)) => {
                if (target.segments()[0] & 0xffc0) == 0xfe80 {
                    return true;
                }
                !is_tailscale_v6(own_address)
                    && same_prefix(
                        &own_address.octets(),
                        &target.octets(),
                        own.prefix_length.min(128),
                    )
            }
            _ => false,
        }
    })
}

fn same_prefix(left: &[u8], right: &[u8], prefix_length: u8) -> bool {
    // A /0 or a host route says nothing about which network this is.
    if prefix_length == 0 || usize::from(prefix_length) == left.len() * 8 {
        return false;
    }
    let whole = usize::from(prefix_length / 8);
    let rest = prefix_length % 8;
    if left[..whole] != right[..whole] {
        return false;
    }
    rest == 0 || {
        let mask = 0xffu8 << (8 - rest);
        left[whole] & mask == right[whole] & mask
    }
}

/// The subnet an address belongs to, as text, for the fingerprint. Tailscale, link-local, and
/// host-route addresses are skipped: they are the same on every network.
fn subnet_of(address: &PhoneAddress) -> Option<String> {
    let parsed = parse_address(&address.address)?;
    match parsed {
        IpAddr::V4(own) => {
            let length = address.prefix_length.min(32);
            if is_tailscale_v4(own) || own.is_link_local() || length == 0 || length == 32 {
                return None;
            }
            let mask = u32::MAX << (32 - u32::from(length));
            Some(format!(
                "{}/{length}",
                Ipv4Addr::from(u32::from(own) & mask)
            ))
        }
        IpAddr::V6(own) => {
            let length = address.prefix_length.min(128);
            if is_tailscale_v6(own)
                || (own.segments()[0] & 0xffc0) == 0xfe80
                || length == 0
                || length == 128
            {
                return None;
            }
            let mask = u128::MAX << (128 - u32::from(length));
            Some(format!(
                "{}/{length}",
                Ipv6Addr::from(u128::from(own) & mask)
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(address: &str) -> RouteAddress {
        RouteAddress {
            address: address.to_owned(),
            port: 7_420,
        }
    }

    fn live(address: &str, kind: RouteKind, round_trip_millis: u32, over_relay: bool) -> LiveRoute {
        LiveRoute {
            route: route(address),
            kind,
            round_trip_millis,
            over_relay,
        }
    }

    fn candidate(
        address: &str,
        kind: RouteKind,
        round_trip_millis: u32,
        over_relay: bool,
    ) -> MigrationCandidate {
        MigrationCandidate {
            route: route(address),
            kind,
            round_trip_millis,
            over_relay,
            millis_since_failed: None,
        }
    }

    #[test]
    fn a_session_on_the_computers_own_network_stops_looking_for_better() {
        assert_eq!(
            next_probe_after_millis(
                live("192.168.1.20", RouteKind::LocalNetwork, 4, false),
                ProbeReason::Quiet,
                0
            ),
            None
        );
        // Over the relay it keeps looking, even to a local address.
        assert_eq!(
            next_probe_after_millis(
                live("192.168.1.20", RouteKind::LocalNetwork, 4, true),
                ProbeReason::Quiet,
                0
            ),
            Some(FIRST_PROBE_MILLIS)
        );
    }

    #[test]
    fn probing_is_immediate_after_a_change_and_backs_off_while_quiet() {
        let tailscale = live("100.101.102.103", RouteKind::Tailscale, 90, false);
        assert_eq!(
            next_probe_after_millis(tailscale.clone(), ProbeReason::NetworkChanged, 7),
            Some(0)
        );
        assert_eq!(
            next_probe_after_millis(tailscale.clone(), ProbeReason::HostDiscovered, 7),
            Some(0)
        );
        assert_eq!(
            next_probe_after_millis(tailscale.clone(), ProbeReason::Quiet, 0),
            Some(15_000)
        );
        assert_eq!(
            next_probe_after_millis(tailscale.clone(), ProbeReason::Quiet, 1),
            Some(30_000)
        );
        // It stops growing rather than running away.
        assert_eq!(
            next_probe_after_millis(tailscale, ProbeReason::Quiet, 20),
            Some(MAX_PROBE_MILLIS)
        );
    }

    #[test]
    fn a_route_has_to_be_clearly_better_before_a_session_moves() {
        let tailscale = live("100.101.102.103", RouteKind::Tailscale, 100, false);
        assert_eq!(
            migration_decision(
                tailscale.clone(),
                candidate("10.0.0.5", RouteKind::LocalNetwork, 71, false),
                None,
                false
            ),
            MigrationDecision::NotBetter
        );
        assert_eq!(
            migration_decision(
                tailscale,
                candidate("10.0.0.5", RouteKind::LocalNetwork, 70, false),
                None,
                false
            ),
            MigrationDecision::Migrate
        );
    }

    #[test]
    fn any_direct_route_takes_a_session_off_the_relay() {
        assert_eq!(
            migration_decision(
                live("relay.example", RouteKind::OtherPrivate, 120, true),
                candidate("100.101.102.103", RouteKind::Tailscale, 119, false),
                None,
                false
            ),
            MigrationDecision::Migrate
        );
        // And never the other way round, however the times measure.
        assert_eq!(
            migration_decision(
                live("100.101.102.103", RouteKind::Tailscale, 120, false),
                candidate("relay.example", RouteKind::OtherPrivate, 10, true),
                None,
                false
            ),
            MigrationDecision::NotBetter
        );
    }

    #[test]
    fn a_tunnel_never_takes_a_session_off_the_local_network() {
        assert_eq!(
            migration_decision(
                live("192.168.1.20", RouteKind::LocalNetwork, 40, false),
                candidate("100.101.102.103", RouteKind::Tailscale, 2, false),
                None,
                false
            ),
            MigrationDecision::LocalRouteKept
        );
    }

    #[test]
    fn a_session_does_not_flap_between_two_routes() {
        let live_route = live("100.101.102.103", RouteKind::Tailscale, 100, false);
        let better = candidate("192.168.1.20", RouteKind::LocalNetwork, 10, false);
        assert_eq!(
            migration_decision(live_route.clone(), better.clone(), Some(29_999), false),
            MigrationDecision::TooSoon
        );
        assert_eq!(
            migration_decision(live_route.clone(), better.clone(), Some(30_000), false),
            MigrationDecision::Migrate
        );

        let just_failed = MigrationCandidate {
            millis_since_failed: Some(59_999),
            ..better.clone()
        };
        assert_eq!(
            migration_decision(live_route.clone(), just_failed, None, false),
            MigrationDecision::RecentlyFailed
        );
        let failed_a_while_ago = MigrationCandidate {
            millis_since_failed: Some(60_000),
            ..better.clone()
        };
        assert_eq!(
            migration_decision(live_route.clone(), failed_a_while_ago, None, false),
            MigrationDecision::Migrate
        );

        // Nothing moves while the person's keystrokes are on the wire.
        assert_eq!(
            migration_decision(live_route.clone(), better.clone(), None, true),
            MigrationDecision::Busy
        );
        // And the route it is already on is not a candidate.
        assert_eq!(
            migration_decision(
                live_route.clone(),
                candidate("100.101.102.103", RouteKind::Tailscale, 1, false),
                None,
                false
            ),
            MigrationDecision::NotBetter
        );
    }

    fn wifi(address: &str, prefix_length: u8, fingerprint: Option<&str>) -> PhoneNetwork {
        PhoneNetwork {
            link: PhoneLink::Wifi,
            addresses: vec![PhoneAddress {
                address: address.to_owned(),
                prefix_length,
            }],
            fingerprint: fingerprint.map(str::to_owned),
        }
    }

    fn order(plan: &RoutePlan) -> Vec<(&str, u8, u32)> {
        plan.attempts
            .iter()
            .map(|attempt| {
                (
                    attempt.route.address.as_str(),
                    attempt.tier,
                    attempt.start_after_millis,
                )
            })
            .collect()
    }

    #[test]
    fn addresses_are_classified_by_their_value() {
        assert_eq!(route_kind("192.168.1.20"), RouteKind::LocalNetwork);
        assert_eq!(route_kind("10.0.0.2"), RouteKind::LocalNetwork);
        assert_eq!(route_kind("172.20.1.1"), RouteKind::LocalNetwork);
        assert_eq!(route_kind("169.254.3.4"), RouteKind::LocalNetwork);
        assert_eq!(route_kind("100.101.102.103"), RouteKind::Tailscale);
        assert_eq!(route_kind("100.63.0.1"), RouteKind::OtherPrivate);
        assert_eq!(route_kind("100.128.0.1"), RouteKind::OtherPrivate);
        assert_eq!(route_kind("fd7a:115c:a1e0::1"), RouteKind::Tailscale);
        assert_eq!(route_kind("[fd12:3456::1]"), RouteKind::LocalNetwork);
        assert_eq!(route_kind("fe80::1%en0"), RouteKind::LocalNetwork);
        assert_eq!(route_kind("mac.tailnet.ts.net"), RouteKind::OtherPrivate);
    }

    #[test]
    fn at_home_the_local_address_goes_before_tailscale_even_when_tailscale_worked_last() {
        let plan = plan_routes(
            vec![route("100.101.102.103"), route("192.168.1.20")],
            vec![],
            wifi("192.168.1.55", 24, None),
            vec![],
        );
        assert_eq!(
            order(&plan),
            vec![("192.168.1.20", 1, 0), ("100.101.102.103", 2, 250)]
        );
    }

    #[test]
    fn the_address_that_worked_on_this_network_goes_first() {
        let plan = plan_routes(
            vec![route("192.168.1.20"), route("100.101.102.103")],
            vec![],
            wifi("10.9.0.4", 16, Some("cafe")),
            vec![
                RememberedRoute {
                    fingerprint: "home".to_owned(),
                    route: route("192.168.1.20"),
                },
                RememberedRoute {
                    fingerprint: "cafe".to_owned(),
                    route: route("100.101.102.103"),
                },
            ],
        );
        assert_eq!(
            order(&plan),
            vec![("100.101.102.103", 0, 0), ("192.168.1.20", 3, 500)]
        );
    }

    #[test]
    fn bonjour_results_join_the_first_tier_and_new_ones_are_tried() {
        let plan = plan_routes(
            vec![route("192.168.1.20"), route("100.101.102.103")],
            vec![route("192.168.1.31")],
            wifi("192.168.1.55", 24, None),
            vec![],
        );
        assert_eq!(
            order(&plan),
            vec![
                ("192.168.1.31", 1, 0),
                ("192.168.1.20", 1, 100),
                ("100.101.102.103", 2, 250),
            ]
        );
    }

    #[test]
    fn on_cellular_tailscale_leads_and_local_addresses_wait() {
        let plan = plan_routes(
            vec![route("192.168.1.20"), route("100.101.102.103")],
            vec![],
            PhoneNetwork {
                link: PhoneLink::Cellular,
                addresses: vec![PhoneAddress {
                    address: "100.70.1.2".to_owned(),
                    prefix_length: 10,
                }],
                fingerprint: None,
            },
            vec![],
        );
        assert_eq!(
            order(&plan),
            vec![("100.101.102.103", 2, 0), ("192.168.1.20", 3, 500)]
        );
    }

    #[test]
    fn a_single_address_gets_the_whole_race() {
        let plan = plan_routes(
            vec![route("192.168.1.20")],
            vec![],
            wifi("192.168.1.55", 24, None),
            vec![],
        );
        assert_eq!(plan.attempts[0].timeout_millis, RACE_DEADLINE_MILLIS);
    }

    #[test]
    fn duplicates_are_tried_once_and_the_plan_is_bounded() {
        let saved: Vec<RouteAddress> = (0..40)
            .map(|index| route(&format!("10.0.0.{}", index % 20)))
            .collect();
        let plan = plan_routes(saved, vec![], wifi("192.168.1.55", 24, None), vec![]);
        assert_eq!(plan.attempts.len(), MAX_ATTEMPTS);
        let unique: HashSet<_> = plan.attempts.iter().map(|attempt| &attempt.route).collect();
        assert_eq!(unique.len(), MAX_ATTEMPTS);
        assert!(
            plan.attempts.iter().all(
                |attempt| attempt.start_after_millis + attempt.timeout_millis
                    <= plan.deadline_millis
            )
        );
    }

    #[test]
    fn a_fingerprint_depends_on_subnet_gateway_and_salt_only() {
        let address = |text: &str, prefix_length| PhoneAddress {
            address: text.to_owned(),
            prefix_length,
        };
        let home = network_fingerprint(
            vec![1; 16],
            PhoneLink::Wifi,
            vec![address("192.168.1.55", 24), address("100.70.1.2", 32)],
            Some("192.168.1.1".to_owned()),
        )
        .unwrap();
        assert_eq!(home.len(), 32);
        let same_home_new_lease = network_fingerprint(
            vec![1; 16],
            PhoneLink::Wifi,
            vec![address("192.168.1.99", 24)],
            Some("192.168.1.1".to_owned()),
        );
        assert_eq!(same_home_new_lease.as_deref(), Some(home.as_str()));
        let other_salt = network_fingerprint(
            vec![2; 16],
            PhoneLink::Wifi,
            vec![address("192.168.1.55", 24)],
            Some("192.168.1.1".to_owned()),
        );
        assert_ne!(other_salt.as_deref(), Some(home.as_str()));
        let other_gateway = network_fingerprint(
            vec![1; 16],
            PhoneLink::Wifi,
            vec![address("192.168.1.55", 24)],
            Some("192.168.1.254".to_owned()),
        );
        assert_ne!(other_gateway.as_deref(), Some(home.as_str()));
        assert_eq!(
            network_fingerprint(vec![1; 16], PhoneLink::Wifi, vec![], None),
            None
        );
        assert_eq!(
            network_fingerprint(vec![1; 16], PhoneLink::Offline, vec![], None),
            None
        );
        assert!(network_fingerprint(vec![1; 16], PhoneLink::Cellular, vec![], None).is_some());
    }

    #[test]
    fn remembering_keeps_one_entry_per_network_newest_first() {
        let mut memory = Vec::new();
        for index in 0..12 {
            memory = remember_route(memory, format!("net{index}"), route("10.0.0.1"));
        }
        memory = remember_route(memory, "net5".to_owned(), route("100.101.102.103"));
        assert_eq!(memory.len(), MAX_REMEMBERED);
        assert_eq!(memory[0].fingerprint, "net5");
        assert_eq!(memory[0].route, route("100.101.102.103"));
        assert_eq!(
            memory
                .iter()
                .filter(|entry| entry.fingerprint == "net5")
                .count(),
            1
        );
    }

    #[test]
    fn advice_names_the_exit_node_symptom_only_when_it_fits() {
        let saved = vec![route("192.168.1.20"), route("100.101.102.103")];
        let outcome = |address: &str, result| AttemptOutcome {
            route: route(address),
            result,
        };
        let home = wifi("192.168.1.55", 24, None);
        assert_eq!(
            route_advice(
                saved.clone(),
                home.clone(),
                vec![
                    outcome("192.168.1.20", AttemptResult::TimedOut),
                    outcome("100.101.102.103", AttemptResult::Connected),
                ],
            ),
            RouteAdvice::LocalNetworkBlockedWhileTailscaleWorks
        );
        assert_eq!(
            route_advice(
                saved.clone(),
                home.clone(),
                vec![
                    outcome("192.168.1.20", AttemptResult::Cancelled),
                    outcome("100.101.102.103", AttemptResult::Connected),
                ],
            ),
            RouteAdvice::LocalNetworkBlockedWhileTailscaleWorks
        );
        assert_eq!(
            route_advice(
                saved.clone(),
                home.clone(),
                vec![
                    outcome("192.168.1.20", AttemptResult::Connected),
                    outcome("100.101.102.103", AttemptResult::Cancelled),
                ],
            ),
            RouteAdvice::None
        );
        assert_eq!(
            route_advice(
                saved.clone(),
                wifi("10.9.0.4", 16, None),
                vec![
                    outcome("192.168.1.20", AttemptResult::TimedOut),
                    outcome("100.101.102.103", AttemptResult::Connected),
                ],
            ),
            RouteAdvice::None
        );
    }

    #[test]
    fn advice_after_a_failed_race_says_what_is_missing() {
        let outcome = |address: &str, result| AttemptOutcome {
            route: route(address),
            result,
        };
        assert_eq!(
            route_advice(
                vec![route("192.168.1.20")],
                wifi("192.168.1.55", 24, None),
                vec![outcome("192.168.1.20", AttemptResult::Refused)],
            ),
            RouteAdvice::RemoteAccessOff
        );
        assert_eq!(
            route_advice(
                vec![route("192.168.1.20")],
                wifi("10.9.0.4", 16, None),
                vec![outcome("192.168.1.20", AttemptResult::TimedOut)],
            ),
            RouteAdvice::NotOnComputersNetwork
        );
        assert_eq!(
            route_advice(
                vec![route("192.168.1.20")],
                PhoneNetwork {
                    link: PhoneLink::Cellular,
                    addresses: vec![],
                    fingerprint: None,
                },
                vec![outcome("192.168.1.20", AttemptResult::Unreachable)],
            ),
            RouteAdvice::NeedsRemoteRoute
        );
        assert_eq!(
            route_advice(
                vec![route("192.168.1.20"), route("100.101.102.103")],
                wifi("192.168.1.55", 24, None),
                vec![
                    outcome("192.168.1.20", AttemptResult::TimedOut),
                    outcome("100.101.102.103", AttemptResult::TimedOut),
                ],
            ),
            RouteAdvice::ComputerUnreachable
        );
    }
}
