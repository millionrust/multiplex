//! The route-plan cases both phone apps replay through their own bindings. Regenerate with
//! `cargo test -p multiplex-controller-bindings --test route_plan_vectors -- --ignored
//! write_route_plan_vectors` after a deliberate rule change, and review the diff.

use std::path::PathBuf;

use multiplex_controller_bindings::{
    PhoneAddress, PhoneLink, PhoneNetwork, RememberedRoute, RouteAddress, RouteKind, plan_routes,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct Document {
    schema_version: u16,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct Case {
    name: String,
    saved: Vec<Route>,
    discovered: Vec<Route>,
    link: String,
    phone_addresses: Vec<Own>,
    fingerprint: Option<String>,
    remembered: Vec<Remembered>,
    expected: Expected,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Route {
    address: String,
    port: u16,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Own {
    address: String,
    prefix_length: u8,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Remembered {
    fingerprint: String,
    route: Route,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct Expected {
    deadline_millis: u32,
    attempts: Vec<Attempt>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct Attempt {
    address: String,
    port: u16,
    kind: String,
    tier: u8,
    start_after_millis: u32,
    timeout_millis: u32,
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/controller-routes/route-plan-v1.json")
}

fn route(address: &str) -> Route {
    Route {
        address: address.to_owned(),
        port: 7_420,
    }
}

fn own(address: &str, prefix_length: u8) -> Own {
    Own {
        address: address.to_owned(),
        prefix_length,
    }
}

fn remembered(fingerprint: &str, address: &str) -> Remembered {
    Remembered {
        fingerprint: fingerprint.to_owned(),
        route: route(address),
    }
}

fn link(name: &str) -> PhoneLink {
    match name {
        "wifi" => PhoneLink::Wifi,
        "ethernet" => PhoneLink::Ethernet,
        "cellular" => PhoneLink::Cellular,
        "other" => PhoneLink::Other,
        "offline" => PhoneLink::Offline,
        other => panic!("unknown link {other}"),
    }
}

fn kind_name(kind: RouteKind) -> &'static str {
    match kind {
        RouteKind::LocalNetwork => "local_network",
        RouteKind::Tailscale => "tailscale",
        RouteKind::OtherPrivate => "other_private",
    }
}

fn addresses(routes: &[Route]) -> Vec<RouteAddress> {
    routes
        .iter()
        .map(|route| RouteAddress {
            address: route.address.clone(),
            port: route.port,
        })
        .collect()
}

fn plan(case: &Case) -> Expected {
    let plan = plan_routes(
        addresses(&case.saved),
        addresses(&case.discovered),
        PhoneNetwork {
            link: link(&case.link),
            addresses: case
                .phone_addresses
                .iter()
                .map(|own| PhoneAddress {
                    address: own.address.clone(),
                    prefix_length: own.prefix_length,
                })
                .collect(),
            fingerprint: case.fingerprint.clone(),
        },
        case.remembered
            .iter()
            .map(|entry| RememberedRoute {
                fingerprint: entry.fingerprint.clone(),
                route: RouteAddress {
                    address: entry.route.address.clone(),
                    port: entry.route.port,
                },
            })
            .collect(),
    );
    Expected {
        deadline_millis: plan.deadline_millis,
        attempts: plan
            .attempts
            .into_iter()
            .map(|attempt| Attempt {
                address: attempt.route.address,
                port: attempt.route.port,
                kind: kind_name(attempt.kind).to_owned(),
                tier: attempt.tier,
                start_after_millis: attempt.start_after_millis,
                timeout_millis: attempt.timeout_millis,
            })
            .collect(),
    }
}

/// The rows of the real-device table in `docs/route-selection-plan.md`, as the planner sees them.
fn cases() -> Vec<Case> {
    let home_lan = route("192.168.1.20");
    let tailscale = route("100.101.102.103");
    let tailscale_v6 = route("fd7a:115c:a1e0::1234");
    let home_wifi = vec![
        own("192.168.1.55", 24),
        own("fe80::1c2d:3e4f:5a6b:7c8d", 64),
    ];
    let case = |name: &str,
                saved: Vec<Route>,
                discovered: Vec<Route>,
                link: &str,
                phone_addresses: Vec<Own>,
                fingerprint: Option<&str>,
                remembered: Vec<Remembered>| Case {
        name: name.to_owned(),
        saved,
        discovered,
        link: link.to_owned(),
        phone_addresses,
        fingerprint: fingerprint.map(str::to_owned),
        remembered,
        expected: Expected {
            deadline_millis: 0,
            attempts: Vec::new(),
        },
    };
    let mut cases = vec![
        case(
            "home Wi-Fi, first visit: the local address leads",
            vec![home_lan.clone(), tailscale.clone()],
            vec![],
            "wifi",
            home_wifi.clone(),
            Some("home"),
            vec![],
        ),
        case(
            "home Wi-Fi with Tailscale on, Tailscale worked last: local still leads",
            vec![tailscale.clone(), tailscale_v6.clone(), home_lan.clone()],
            vec![],
            "wifi",
            home_wifi.clone(),
            Some("home"),
            vec![],
        ),
        case(
            "home Wi-Fi, second visit: the remembered address goes alone at first",
            vec![tailscale.clone(), home_lan.clone()],
            vec![],
            "wifi",
            home_wifi.clone(),
            Some("home"),
            vec![remembered("home", "192.168.1.20")],
        ),
        case(
            "Tailscale remembered at home: the local address still leads",
            vec![home_lan.clone(), tailscale.clone()],
            vec![],
            "wifi",
            home_wifi.clone(),
            Some("home"),
            vec![remembered("home", "100.101.102.103")],
        ),
        case(
            "the computer moved to a new address: Bonjour's leads",
            vec![home_lan.clone(), tailscale.clone()],
            vec![route("192.168.1.31")],
            "wifi",
            home_wifi.clone(),
            Some("home"),
            vec![],
        ),
        case(
            "cellular with Tailscale: Tailscale first, local addresses last",
            vec![home_lan.clone(), tailscale.clone(), tailscale_v6.clone()],
            vec![],
            "cellular",
            vec![own("100.72.14.9", 10)],
            Some("cellular"),
            vec![remembered("home", "192.168.1.20")],
        ),
        case(
            "someone else's Wi-Fi on a different subnet",
            vec![home_lan.clone(), route("10.0.0.8"), tailscale.clone()],
            vec![],
            "wifi",
            vec![own("10.9.0.4", 16)],
            Some("cafe"),
            vec![remembered("home", "192.168.1.20")],
        ),
        case(
            "a host with one address",
            vec![home_lan.clone()],
            vec![],
            "wifi",
            home_wifi.clone(),
            None,
            vec![],
        ),
        case(
            "a duplicate and a link-local address",
            vec![
                home_lan.clone(),
                route("fe80::aa:bbff:fecc:ddee"),
                home_lan.clone(),
                tailscale.clone(),
            ],
            vec![],
            "wifi",
            home_wifi,
            None,
            vec![],
        ),
    ];
    for case in &mut cases {
        case.expected = plan(case);
    }
    cases
}

#[test]
fn every_case_plans_as_recorded() {
    let document: Document =
        serde_json::from_str(&std::fs::read_to_string(fixture()).expect("route-plan fixture"))
            .expect("route-plan fixture parses");
    assert_eq!(document.schema_version, 1);
    assert!(document.cases.len() >= 9);
    for case in &document.cases {
        assert_eq!(plan(case), case.expected, "{}", case.name);
    }
    assert_eq!(
        document.cases,
        cases(),
        "the fixture no longer matches the cases here; regenerate it"
    );
}

#[test]
#[ignore = "writes the fixture"]
fn write_route_plan_vectors() {
    let document = Document {
        schema_version: 1,
        cases: cases(),
    };
    let path = fixture();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = serde_json::to_string_pretty(&document).unwrap();
    text.push('\n');
    std::fs::write(path, text).unwrap();
}
