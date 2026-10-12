# Transitive YAML parser review — 2026-10-12

The v0.0.14 Linux verification job passed all 2,136 tests and then failed
`cargo deny` on RUSTSEC-2024-0449. This is an informational unmaintained
advisory, with no patched version listed:
https://raw.githubusercontent.com/RustSec/advisory-db/main/crates/unsafe-libyaml/RUSTSEC-2024-0449.md

`cargo tree -i unsafe-libyaml --locked --offline` shows this route:

```text
unsafe-libyaml 0.2.11
  serde_yaml 0.9.34+deprecated
    rust-i18n-macro / rust-i18n-support 3.1.5
      rust-i18n 3.1.5
        gpui-component 0.5.1
          multiplex
```

The checked gpui-component source invokes `rust_i18n::i18n!("locales",
fallback = "en")`. The macro loads dependency translation files at compile
time through `rust-i18n-support`. Multiplex has no direct serde_yaml or
unsafe-libyaml dependency; this is not the terminal conversation parser.
This review does not claim that an unmaintained unsafe parser is risk-free.

The existing dependency policy permits specific, owned, expiring exceptions
for transitive libraries awaiting upstream migrations. A single exception
for this advisory follows that policy: owner desktop maintainers, expiry
2026-11-30. It neither suppresses other advisories nor changes shipped code
or Cargo.lock. Replace the upstream translation dependency with a maintained
parser during the gpui-component migration and remove this exception; do
not renew it without a fresh review.

The immutable v0.0.14 tag predates this policy-only review. Its CI result must
still be reported as failed on this advisory; a passing later policy check
does not turn that historical run green. Release notes must disclose the
exception when publishing that candidate.
