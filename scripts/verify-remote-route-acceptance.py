#!/usr/bin/env python3
import json
import pathlib
import sys


ROOT = pathlib.Path(__file__).resolve().parents[1]
SWIFT = ROOT.parent / "terminal_app" / "terminal_swift"
KOTLIN = ROOT.parent / "terminal_app" / "terminal_kotlin"
ROUTE_FIXTURE = ROOT / "tests/fixtures/controller-routes/route-selection-v1.json"
ACCEPTANCE_FIXTURE = ROOT / "tests/fixtures/controller-routes/remote-route-acceptance-v1.json"
ROUTES = ["private_network", "ssh", "self_hosted_relay"]


def fail(message: str) -> None:
    raise SystemExit(message)


def load(path: pathlib.Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load {path}: {error}")


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def check_copy(source: pathlib.Path, destination: pathlib.Path) -> None:
    require(destination.is_file(), f"missing synchronized fixture: {destination}")
    require(source.read_bytes() == destination.read_bytes(), f"fixture drift: {destination}")


def main() -> None:
    route = load(ROUTE_FIXTURE)
    acceptance = load(ACCEPTANCE_FIXTURE)
    require(route.get("schema_version") == 1, "route fixture schema must be v1")
    require(acceptance.get("schema_version") == 1, "acceptance fixture schema must be v1")
    require(acceptance.get("routes") == ROUTES, "acceptance routes must be canonical and ordered")

    cases = acceptance.get("lifecycle_cases")
    require(isinstance(cases, list) and len(cases) == 8, "exactly eight lifecycle programs are required")
    names = [case.get("name") for case in cases]
    require(len(set(names)) == len(names), "lifecycle names must be unique")
    event_kinds = set()
    required_metrics = {
        "phase",
        "transport_starts",
        "transport_disconnects",
        "input_clears",
        "writer_releases",
        "idempotent_read_retries",
        "mutation_queries",
        "mutation_replays",
        "automatic_switches",
        "explicit_actions",
        "terminal_allowed",
    }
    for case in cases:
        steps = case.get("steps")
        expected = case.get("expected")
        require(isinstance(steps, list) and steps, f"{case.get('name')}: steps are required")
        require(steps[0].get("kind") == "select", f"{case.get('name')}: selection must be explicit")
        require(set(expected or {}) == required_metrics, f"{case.get('name')}: metric shape differs")
        require(expected["mutation_replays"] == 0, f"{case.get('name')}: mutation replay is forbidden")
        require(expected["automatic_switches"] == 0, f"{case.get('name')}: automatic switching is forbidden")
        event_kinds.update(step.get("kind") for step in steps)

    required_events = {
        "select",
        "connect",
        "transport_ready",
        "authenticated",
        "set_writer",
        "failure",
        "retry",
        "cancel",
        "revoke",
        "set_available",
        "authorization_restored",
    }
    require(event_kinds == required_events, f"lifecycle event coverage differs: {event_kinds}")

    switch = acceptance.get("switch_matrix", {})
    confirmed = switch.get("confirmed", {})
    require(switch.get("unconfirmed_error") == "explicit_confirmation_required", "unconfirmed switch must fail")
    require(switch.get("unavailable_error") == "target_unavailable", "unavailable switch must fail")
    require(confirmed.get("source_phase") == "online", "switch source must be online")
    require(confirmed.get("writer_held") is True, "switch matrix must exercise writer release")
    require(confirmed.get("source_disconnects") == 1, "switch must close exactly one source")
    require(confirmed.get("target_starts") == 0, "switch must not start target implicitly")
    require(confirmed.get("input_clears") == 1, "switch must clear pending input")
    require(confirmed.get("writer_releases") == 1, "switch must release writer")
    require(confirmed.get("automatic_switches") == 0, "switch must remain explicit")

    copies = [
        (ROUTE_FIXTURE, SWIFT / "TermiRustMobileTests/Fixtures/route-selection-v1.json"),
        (ROUTE_FIXTURE, KOTLIN / "app/src/test/resources/route-selection-v1.json"),
        (ACCEPTANCE_FIXTURE, SWIFT / "TermiRustMobileTests/Fixtures/remote-route-acceptance-v1.json"),
        (ACCEPTANCE_FIXTURE, KOTLIN / "app/src/test/resources/remote-route-acceptance-v1.json"),
    ]
    for source, destination in copies:
        check_copy(source, destination)

    print("Remote route fixtures pass schema, hostile-case, no-replay, and synchronization checks.")


if __name__ == "__main__":
    main()
