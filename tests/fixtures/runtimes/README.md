# Sanitized runtime contract fixtures

These scripts contain no provider data, credentials, paths, network access, or configuration writes.
Tests copy them to a temporary `PATH`, apply executable/permission modes there, and invoke only the
compiled version-probe argv. The three provider directories cover supported and exact range
boundaries; `generic` covers degraded process behavior shared by every descriptor.
