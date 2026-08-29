#!/usr/bin/env python3
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "tests/fixtures/mobile/mobile-route-contract-v1.json"


def fail(message: str) -> None:
    raise SystemExit(message)


document = json.loads(FIXTURE.read_text())
if document.get("schema_version") != 1:
    fail("mobile route contract schema version drift")

vocabulary = document.get("capability_vocabulary")
if not isinstance(vocabulary, list) or len(vocabulary) != len(set(vocabulary)):
    fail("mobile route capability vocabulary must be a unique list")

routes = document.get("routes")
if not isinstance(routes, list) or len(routes) != 3:
    fail("mobile route contract must define exactly three product item kinds")
if {route.get("item_kind") for route in routes} != {
    "saved_connection",
    "paired_device",
    "durable_device_session",
}:
    fail("mobile route item-kind coverage drift")

for route in routes:
    capabilities = route.get("capabilities")
    if not isinstance(capabilities, list) or len(capabilities) != len(set(capabilities)):
        fail(f"{route.get('id')}: capabilities must be a unique list")
    unknown = set(capabilities) - set(vocabulary)
    if unknown:
        fail(f"{route.get('id')}: unknown capabilities: {sorted(unknown)}")

by_kind = {route["item_kind"]: route for route in routes}
direct = by_kind["saved_connection"]
if direct["credential_owner"] != "ssh_credential" or direct["continuity_owner"] != "remote_tmux_if_enabled":
    fail("direct SSH ownership contract drift")
if {"durable_replay", "single_writer", "authoritative_activity"} & set(direct["capabilities"]):
    fail("direct SSH cannot claim Host Session capabilities")

session = by_kind["durable_device_session"]
if session["credential_owner"] != "device_pairing_identity" or session["continuity_owner"] != "host_service":
    fail("Device Session ownership contract drift")
if not {"durable_replay", "single_writer", "authoritative_activity"}.issubset(session["capabilities"]):
    fail("Device Session capability contract drift")

if len(document.get("invalid_cases", [])) < 6:
    fail("mobile route contract adversarial coverage drift")

print("Mobile route contract v1 is structurally valid and route-scoped.")
