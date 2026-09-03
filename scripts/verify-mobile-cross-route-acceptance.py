#!/usr/bin/env python3
import json
import os
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "tests/fixtures/mobile/mobile-cross-route-acceptance-v1.json"
SWIFT_ROOT = Path(
    os.environ.get("TERMIRUST_IOS_DIR", ROOT / "mobile/ios")
)
KOTLIN_ROOT = Path(
    os.environ.get("TERMIRUST_ANDROID_DIR", ROOT / "mobile/android")
)
NATIVE_COPIES = [
    SWIFT_ROOT / "TermiRustMobileTests/Fixtures/mobile-cross-route-acceptance-v1.json",
    KOTLIN_ROOT / "app/src/test/resources/mobile-cross-route-acceptance-v1.json",
]


def fail(message: str) -> None:
    raise SystemExit(message)


document = json.loads(FIXTURE.read_text())
if document.get("schema_version") != 1:
    fail("mobile cross-route acceptance schema version drift")

cases = document.get("cases")
if not isinstance(cases, list) or len(cases) < 15:
    fail("mobile cross-route acceptance requires at least 15 cases")
if len({case.get("name") for case in cases}) != len(cases):
    fail("mobile cross-route acceptance case names must be unique")
if {case.get("route") for case in cases} != {"direct_ssh", "device_session"}:
    fail("mobile cross-route acceptance must cover both terminal routes")

required_events = {
    "connect", "failure", "cancel", "background", "reconnect", "route_switch",
    "host_key_mismatch", "missing_tmux", "authority_revoked",
}
if {case.get("event") for case in cases} != required_events:
    fail("mobile cross-route acceptance event coverage drift")

for case in cases:
    expected = case.get("expected")
    if not isinstance(expected, dict):
        fail(f"{case.get('name')}: missing expected decision")
    if expected.get("replay_terminal_input") is not False:
        fail(f"{case.get('name')}: terminal input replay must always fail closed")
    if case["route"] == "direct_ssh" and expected.get("replay_terminal_output") is not False:
        fail(f"{case.get('name')}: direct SSH cannot claim Host output replay")
    if expected.get("release_writer") and case["route"] != "device_session":
        fail(f"{case.get('name')}: only Device Sessions can release a Host writer")
    if expected.get("fallback_to_normal_shell") and case["event"] != "missing_tmux":
        fail(f"{case.get('name')}: normal-shell fallback is reserved for missing tmux")

source = FIXTURE.read_bytes()
for copy in NATIVE_COPIES:
    if not copy.is_file() or copy.read_bytes() != source:
        fail(f"native cross-route fixture drift: {copy}")

print("Mobile cross-route acceptance v1 is structurally valid and synchronized.")
