fn main() {
    // Only macOS has an encoder here. Everywhere else the crate compiles to the unsupported
    // stub, which links nothing.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        for framework in ["VideoToolbox", "CoreMedia", "CoreVideo", "CoreFoundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
