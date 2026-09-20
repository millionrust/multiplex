fn main() {
    // Apple platforms all carry VideoToolbox. The iPhone matters as much as the Mac here: it is
    // the decoder side, and hardware decode is the difference between a smooth remote screen and
    // a hot phone. Everywhere else the crate compiles to the unsupported stub, which links
    // nothing.
    //
    // This covers what Cargo links: the desktop application, tests, benchmarks. It does not reach
    // the iOS application, which links a static library out of an xcframework itself and is told
    // nothing by Cargo about what that library expects; those frameworks are named again in
    // `apps/ios/project.yml`. Removing them there brings back undefined VideoToolbox symbols at
    // link time, with nothing in this crate to suggest why.
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target == "macos" || target == "ios" {
        for framework in ["VideoToolbox", "CoreMedia", "CoreVideo", "CoreFoundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
