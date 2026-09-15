fn main() {
    // screencapturekit links a Swift bridge. Its own build script adds the Swift runtime to the
    // loader path only for its own targets, so binaries built from this crate (examples and
    // tests) need the same path, or loading fails with "libswift_Concurrency.dylib not loaded".
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
