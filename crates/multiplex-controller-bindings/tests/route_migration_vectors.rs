//! The migration cases both phone apps replay through their own bindings, so a session moves —
//! or stays put — for the same reasons on iOS and Android. Regenerate with
//! `cargo test -p multiplex-controller-bindings --test route_migration_vectors -- --ignored
//! write_route_migration_vectors` after a deliberate rule change, and review the diff.

use std::path::PathBuf;

use multiplex_controller_bindings::{
    LiveRoute, MigrationCandidate, MigrationDecision, ProbeReason, RouteAddress, RouteKind,
    migration_decision, next_probe_after_millis,
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
    live: Live,
    candidate: Candidate,
    millis_since_last_migration: Option<u64>,
    writer_command_in_flight: bool,
    probe_reason: String,
    quiet_probes: u32,
    expected: Expected,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Live {
    address: String,
    port: u16,
    kind: String,
    round_trip_millis: u32,
    over_relay: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Candidate {
    address: String,
    port: u16,
    kind: String,
    round_trip_millis: u32,
    over_relay: bool,
    millis_since_failed: Option<u64>,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct Expected {
    decision: String,
    /// What the phone should wait before its next look, or nothing when it should stop looking.
    next_probe_after_millis: Option<u32>,
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/controller-routes/route-migration-v1.json")
}

fn kind(name: &str) -> RouteKind {
    match name {
        "local_network" => RouteKind::LocalNetwork,
        "tailscale" => RouteKind::Tailscale,
        "other_private" => RouteKind::OtherPrivate,
        other => panic!("unknown route kind {other}"),
    }
}

fn reason(name: &str) -> ProbeReason {
    match name {
        "network_changed" => ProbeReason::NetworkChanged,
        "host_discovered" => ProbeReason::HostDiscovered,
        "quiet" => ProbeReason::Quiet,
        other => panic!("unknown probe reason {other}"),
    }
}

fn decision_name(decision: MigrationDecision) -> &'static str {
    match decision {
        MigrationDecision::Migrate => "migrate",
        MigrationDecision::NotBetter => "not_better",
        MigrationDecision::LocalRouteKept => "local_route_kept",
        MigrationDecision::TooSoon => "too_soon",
        MigrationDecision::RecentlyFailed => "recently_failed",
        MigrationDecision::Busy => "busy",
    }
}

fn live_route(live: &Live) -> LiveRoute {
    LiveRoute {
        route: RouteAddress {
            address: live.address.clone(),
            port: live.port,
        },
        kind: kind(&live.kind),
        round_trip_millis: live.round_trip_millis,
        over_relay: live.over_relay,
    }
}

fn decide(case: &Case) -> Expected {
    Expected {
        decision: decision_name(migration_decision(
            live_route(&case.live),
            MigrationCandidate {
                route: RouteAddress {
                    address: case.candidate.address.clone(),
                    port: case.candidate.port,
                },
                kind: kind(&case.candidate.kind),
                round_trip_millis: case.candidate.round_trip_millis,
                over_relay: case.candidate.over_relay,
                millis_since_failed: case.candidate.millis_since_failed,
            },
            case.millis_since_last_migration,
            case.writer_command_in_flight,
        ))
        .to_owned(),
        next_probe_after_millis: next_probe_after_millis(
            live_route(&case.live),
            reason(&case.probe_reason),
            case.quiet_probes,
        ),
    }
}

/// The situations the plan's real-device table puts a running session in.
fn cases() -> Vec<Case> {
    let live = |kind: &str, round_trip_millis: u32, over_relay: bool| Live {
        address: match kind {
            "local_network" => "192.168.1.20",
            "tailscale" => "100.101.102.103",
            _ => "relay.example",
        }
        .to_owned(),
        port: 7_420,
        kind: kind.to_owned(),
        round_trip_millis,
        over_relay,
    };
    let candidate = |kind: &str, round_trip_millis: u32, over_relay: bool| Candidate {
        address: match kind {
            "local_network" => "192.168.1.20",
            "tailscale" => "100.101.102.103",
            _ => "relay.example",
        }
        .to_owned(),
        port: 7_420,
        kind: kind.to_owned(),
        round_trip_millis,
        over_relay,
        millis_since_failed: None,
    };
    let case = |name: &str,
                live: Live,
                candidate: Candidate,
                millis_since_last_migration: Option<u64>,
                writer_command_in_flight: bool,
                probe_reason: &str,
                quiet_probes: u32| Case {
        name: name.to_owned(),
        live,
        candidate,
        millis_since_last_migration,
        writer_command_in_flight,
        probe_reason: probe_reason.to_owned(),
        quiet_probes,
        expected: Expected {
            decision: String::new(),
            next_probe_after_millis: None,
        },
    };
    let mut cases = vec![
        case(
            "walked home: the local address takes the session off Tailscale",
            live("tailscale", 100, false),
            candidate("local_network", 8, false),
            None,
            false,
            "network_changed",
            0,
        ),
        case(
            "on the local network already: nothing to look for",
            live("local_network", 6, false),
            candidate("tailscale", 2, false),
            None,
            false,
            "quiet",
            0,
        ),
        case(
            "a direct route always beats the relay",
            live("other_private", 120, true),
            candidate("tailscale", 119, false),
            None,
            false,
            "host_discovered",
            0,
        ),
        case(
            "barely faster is not worth the reattach",
            live("tailscale", 100, false),
            candidate("local_network", 71, false),
            None,
            false,
            "quiet",
            1,
        ),
        case(
            "a second move inside thirty seconds waits",
            live("tailscale", 100, false),
            candidate("local_network", 10, false),
            Some(10_000),
            false,
            "quiet",
            2,
        ),
        case(
            "nothing moves while a keystroke is on the wire",
            live("tailscale", 100, false),
            candidate("local_network", 10, false),
            None,
            true,
            "quiet",
            3,
        ),
    ];
    cases.push(Case {
        name: "a route that just failed is left alone".to_owned(),
        candidate: Candidate {
            millis_since_failed: Some(20_000),
            ..candidate("local_network", 10, false)
        },
        ..case(
            "",
            live("tailscale", 100, false),
            candidate("local_network", 10, false),
            None,
            false,
            "quiet",
            8,
        )
    });
    for case in &mut cases {
        case.expected = decide(case);
    }
    cases
}

#[test]
fn every_case_decides_as_recorded() {
    let document: Document =
        serde_json::from_str(&std::fs::read_to_string(fixture()).expect("route-migration fixture"))
            .expect("route-migration fixture parses");
    assert_eq!(document.schema_version, 1);
    assert!(document.cases.len() >= 6);
    for case in &document.cases {
        assert_eq!(decide(case), case.expected, "{}", case.name);
    }
    assert_eq!(
        document.cases,
        cases(),
        "the fixture no longer matches the cases here; regenerate it"
    );
}

#[test]
#[ignore = "writes the fixture"]
fn write_route_migration_vectors() {
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
