fn main() {
    // Apple platforms all carry VideoToolbox. The iPhone matters as much as the Mac here: it is
    // the decoder side, and hardware decode is the difference between a smooth remote screen and
    // a hot phone. Everywhere else the crate compiles to the unsupported stub, which links
    // nothing.
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target == "macos" || target == "ios" {
        for framework in ["VideoToolbox", "CoreMedia", "CoreVideo", "CoreFoundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
