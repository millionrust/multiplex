//! The spike calls VideoToolbox, Core Media, Core Video and Core Foundation directly, so the
//! frameworks holding those symbols have to be on the link line.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        for framework in [
            "VideoToolbox",
            "CoreMedia",
            "CoreVideo",
            "CoreFoundation",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
