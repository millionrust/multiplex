fn main() {
    // Screen sharing links ScreenCaptureKit, whose Swift bridge needs the Swift runtime on the
    // loader path. Each crate's build script only affects its own targets, so the app and its
    // tests add the path again, or loading fails with "libswift_Concurrency.dylib not loaded".
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
