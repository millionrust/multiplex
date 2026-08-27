# Controller LAN Verification Fixture

The executable fixtures live in the focused Rust tests so socket ownership,
timeouts, interface changes, encrypted pairing, and queue boundaries remain
deterministic and type checked. The verification script requires this marker
before running those fixtures and the Remote Devices state-machine tests.

No test opens a wildcard listener, advertises discovery, changes firewall
rules, or contacts an account or relay service.
