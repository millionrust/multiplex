# Browser Engine Feasibility Decision

- Decision: **No-Go**
- Scope: Goal 19.1 research and synthetic local spike only
- Owner: TermiRust desktop maintainers
- Decision date: 2026-08-27
- Review date: 2026-11-27
- Product authority: none; Goal 19.2 remains frozen

## Decision

TermiRust will not add a production browser engine or browser-control feature from this spike. None of the fixed three routes has candidate-specific evidence for every mandatory isolation, interception, stale-document, cancellation, packaging, and license gate. The reference machine has no Chromium-family test browser or ChromeDriver installed, so documentation and generic process-harness results cannot be promoted to live engine proof.

`headless_chrome` 1.0.22 is additionally rejected by the current repository license policy: its build graph includes `auto_generate_cdp` 0.4.6, detected as GPL-3.0-or-later, and `webpki-roots` 1.0.9 under CDLA-Permissive-2.0. `chromiumoxide` passes the automated controller dependency policy, but branded Chrome for Testing distribution terms and all live security gates remain unresolved. Standards-based WebDriver has the strongest official maintenance/security route, but its required network-interception behavior depends on evolving WebDriver BiDi support and was not measured here.

This is a product No-Go, not a claim that browser automation is technically impossible. It is the only valid binary decision while mandatory evidence is missing.

## Fixed Comparison

| Route | Exact pin | Maintenance/security evidence | License result | Isolation/interception evidence | Packaging evidence | Result |
|---|---|---|---|---|---|---|
| `chromiumoxide` | chromiumoxide 0.9.1, tag commit `a7e2bb835b9643410f9e3dc044f0d947e96cbfa4` | Released 2026-02-25; repository latest observed commit `afcc3a4313f2087249b4490d94e54bf8e3bfaccf` on 2026-04-03; no repository `SECURITY.md` at the pin | Crate MIT OR Apache-2.0; pinned transitive graph passes `cargo-deny`; Chrome terms unresolved | CDP exposes relevant primitives, but the route was not run and every candidate-specific mandatory cell remains unknown | Exact CfT archive pin exists; no signed/notarized TermiRust packaging/update path tested | No-Go |
| `headless_chrome` | headless_chrome 1.0.22, tag commit `0a5c307a85debc450378a1f19e4dac1838d7b22d` | Released 2026-06-11; no later source commit and no repository `SECURITY.md` at the pin | Crate MIT, but pinned graph fails policy on GPL-3.0-or-later and CDLA-Permissive-2.0 dependencies | README advertises request interception/profile controls but also lists frame and WebSocket inspection gaps; no live mandatory gate proof | Automatic browser fetching is unsuitable for production supply-chain policy; no package/update proof | No-Go |
| W3C WebDriver + ChromeDriver | WebDriver Working Draft 2026-07-02, WebDriver BiDi Working Draft 2026-06-29, ChromeDriver/CfT 152.0.7977.64 | W3C issue process plus official Chromium private vulnerability reporting and ChromeDriver issue route | No additional Rust controller dependency in the spike; W3C document terms identified; Chrome executable/distribution terms unresolved | Classic WebDriver is insufficient for the required network policy; BiDi specifies network intercepts, but exact ChromeDriver support was not measured | CfT publishes matching macOS/Linux/Windows browser and driver archives; no TermiRust redistribution/update proof | No-Go |

## Mandatory Gates

The generated report contains all 15 gates for every route. `scripts/verify-browser-spike-report.sh` rejects an incomplete gate set and forbids `Go` unless every selected-candidate status is `pass`.

| Mandatory gate | Fixture-only result | Evidence needed to close |
|---|---|---|
| OS-user and ephemeral-profile isolation | Unknown per candidate | Launch under the intended OS isolation boundary, prove a new non-personal profile, verify sandbox state, and inspect filesystem access on every claimed platform |
| Owned process-tree termination | Unknown per candidate | The generic process-group harness passes; repeat with each browser/driver/controller process tree and a sentinel process |
| Navigation/subresource/redirect interception | Unknown per candidate | Live deny-before-request assertions for main navigation, redirect hops, images/scripts/fetch, private/metadata ranges, and rebinding |
| Iframe/popup interception | Unknown per candidate | Live cross-target auto-attach and deny-before-request proof |
| WebSocket/service-worker interception | Unknown per candidate | Live target/request coverage including worker-created and cached requests |
| Download interception | Unknown per candidate | Live redirect-aware deny/quota proof with no unreviewed file publication |
| Stale-document detection | Unknown per candidate | Generation-bound element references invalidated on navigation and DOM replacement |
| Cancellation within 30 seconds | Unknown per candidate | Forced browser/driver hang with exact owned-tree termination and profile cleanup |
| Compatible license | Failed for `headless_chrome`; unknown for the other routes | Legal approval for the full controller plus browser/driver redistribution and notices |
| Maintained release/security route | Pass only for the official W3C/Chromium route | Named controller vulnerability channel and response owner for a selected implementation |
| Reproducible packaging path | Unknown per candidate | Verified cross-platform archives, checksums/signatures, SBOM/notices, updater/CVE response, and CI install tests |

## Browser Pin

The spike records Chrome for Testing 152.0.7977.64 (Stable) for `mac-arm64`, published by the official last-known-good manifest observed 2026-08-27.

- Browser archive: `chrome-mac-arm64.zip`
- Browser SHA-256: `10033804338bd0a5aa098149a8dd64f3f2e0e8b201bf3d400d7c17d067ff696f`
- Driver archive: `chromedriver-mac-arm64.zip`
- Driver SHA-256: `9e8b67036bf3d744feb97d5711a6f6ce40855d9554e93adfa4a869aa69677ef3`
- The checksums were calculated by streaming the official archives through SHA-256; archives were not retained or installed.
- Chrome for Testing deliberately has no auto-update. A production route would own authenticated updates, CVE response, platform packaging, notices, rollback, and disk cost.

The Chrome for Testing repository's Apache-2.0 license applies to that repository's tooling, not automatically to the branded Chrome executable. Google's Chrome terms apply to the executable and require explicit review before redistribution.

## Synthetic Harness

`tests/fixtures/browser-hostile/index.json` freezes seed `0x19012026` and exactly 11 cases: redirect, rebinding, iframe, popup, WebSocket, service worker, download, huge DOM, stalled response, crash signal, and stale element. The standalone tool:

- binds only kernel-assigned `127.0.0.1` ports;
- serves only committed files and bounded dynamic responses;
- uses no DNS query, public URL, analytics, credentials, cookies, personal profile, or browser;
- caps manifest input at 64 KiB, responses and reports at 256 KiB, and runs at 10-100;
- launches fixture children with an empty environment allowlist;
- establishes and verifies an owned process group containing a descendant;
- terminates only that group, proves an unrelated sentinel survives, and removes the temporary profile;
- preserves a previous named report in timestamped `target/browser-spike/history/` before replacement.

The required reference run completed 10 deterministic warm fixture passes at p50 185 ms and p95 191 ms. Candidate cold/warm startup, RSS, CPU, binary size, and cancellation fields remain `null` rather than reporting generic harness numbers as engine measurements.

## Required Follow-Up

Reassessment requires all of the following; partial completion cannot change this decision:

1. D08 approval defines product domains, profiles, credentials, downloads, and headless behavior.
2. Legal approves the selected controller graph, Chrome/Chromium binary route, redistribution, notices, and update obligations. `headless_chrome` cannot proceed under the current allowlist without a separately reviewed dependency change.
3. A quarantined test job verifies the pinned browser and driver checksums, runs each available fixed route against the same corpus on every claimed OS/architecture, and captures cold plus at least 10 warm measurements.
4. The selected route proves all request classes, DNS/rebinding policy, profile/sandbox state, document generations, owned process-tree termination, 30-second cancellation, download containment, and cleanup.
5. A named security owner and controller vulnerability route exist, with SBOM, CVE intake, browser update SLA, package verification, and rollback evidence.
6. A new reviewed decision changes this record to Conditional Go or Go. Goal 19.2 is not automatically authorized.

## Later UX Contract

If a later reviewed route is authorized, Goal 19.2 must expose localized semantic states `off`, `starting`, `ready`, `blocked`, `crashed`, and `policy denied`. Diagnostics must show the engine and browser versions plus content-free failure codes. Status must use text and semantics rather than color alone, keyboard focus must remain outside hostile page content by default, browser prompts cannot impersonate product approvals, and recording-friendly mode must mask URLs, screenshots, console data, cookies, and profile details.

## Spike Disposition

Retain the standalone tool, fixtures, runner, verifier, and this decision as non-production evidence. They are not workspace members and add no browser dependency or feature to the application. Delete them only if the browser program is permanently removed; otherwise promote code into a production crate only after a later decision passes every mandatory gate. No spike code may be linked into the shipped binary as-is.

## Primary Sources

All sources were accessed 2026-08-27.

- [chromiumoxide crate API](https://crates.io/api/v1/crates/chromiumoxide), [0.9.1 release](https://github.com/mattsse/chromiumoxide/releases/tag/v0.9.1), and [pinned source](https://github.com/mattsse/chromiumoxide/tree/a7e2bb835b9643410f9e3dc044f0d947e96cbfa4)
- [headless_chrome crate API](https://crates.io/api/v1/crates/headless_chrome), [1.0.22 release](https://github.com/rust-headless-chrome/rust-headless-chrome/releases/tag/1.0.22), and [pinned source](https://github.com/rust-headless-chrome/rust-headless-chrome/tree/0a5c307a85debc450378a1f19e4dac1838d7b22d)
- [Chrome for Testing availability and platform matrix](https://github.com/GoogleChromeLabs/chrome-for-testing), [last-known-good JSON](https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json), [browser archive](https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.64/mac-arm64/chrome-mac-arm64.zip), and [driver archive](https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.64/mac-arm64/chromedriver-mac-arm64.zip)
- [Chrome for Testing rationale](https://developer.chrome.com/docs/automation-and-testing/chrome-for-testing), [ChromeDriver documentation](https://developer.chrome.com/docs/chromedriver), and [ChromeDriver security considerations](https://sites.google.com/chromium.org/driver/security-considerations)
- [WebDriver 2026-07-02 Working Draft](https://www.w3.org/TR/2026/WD-webdriver2-20260702/) and [WebDriver BiDi 2026-06-29 Working Draft](https://www.w3.org/TR/2026/WD-webdriver-bidi-20260629/)
- [CDP Fetch](https://chromedevtools.github.io/devtools-protocol/tot/Fetch/), [Network](https://chromedevtools.github.io/devtools-protocol/tot/Network/), and [Target](https://chromedevtools.github.io/devtools-protocol/tot/Target/) domains
- [Chromium security reporting](https://www.chromium.org/Home/chromium-security/reporting-security-bugs/), [secure architecture](https://www.chromium.org/Home/chromium-security/guts/), and [site isolation](https://www.chromium.org/developers/design-documents/site-isolation/)
- [Chromium source license](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/LICENSE) and [Google Chrome executable terms](https://www.google.com/chrome/terms/)
