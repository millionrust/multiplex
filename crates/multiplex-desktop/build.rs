fn main() {
    // Screen sharing links ScreenCaptureKit, whose Swift bridge needs the Swift runtime on the
    // loader path. Each crate's build script only affects its own targets, so the app and its
    // tests add the path again, or loading fails with "libswift_Concurrency.dylib not loaded".
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }

    // The commit shown in Settings → About. A build outside a Git checkout shows "unknown".
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        && output.status.success()
        && let Ok(commit) = String::from_utf8(output.stdout)
    {
        println!("cargo:rustc-env=MULTIPLEX_BUILD_COMMIT={}", commit.trim());
    }
}
