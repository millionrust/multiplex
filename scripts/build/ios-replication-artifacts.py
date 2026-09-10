"""Package the replication Swift binding and static iOS framework as one verified set."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import tempfile

spec = importlib.util.spec_from_file_location("artifacts", Path(__file__).with_name("mobile-replication-artifacts.py"))
shared = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shared)
run = shared.run
ROOT = shared.ROOT
OUTPUT = ROOT / "dist/mobile/replication-ios"
DEST = ROOT / "apps/ios/Replication"
NAME = "TermiRustReplicationSecurity"
STEM = shared.STEM


def symbol_tool():
    sysroot = Path(run("rustc", "--print", "sysroot", capture=True).strip())
    host = next(line.split(": ", 1)[1] for line in run("rustc", "-vV", capture=True).splitlines() if line.startswith("host: "))
    tool = sysroot / "lib/rustlib" / host / "bin/llvm-nm"
    if not tool.is_file():
        raise RuntimeError("Install the pinned Rust llvm-tools-preview component")
    return str(tool)


def verify(directory):
    expected = json.loads((directory / "artifacts.json").read_text())
    if expected != shared.inventory(directory):
        raise RuntimeError("iOS replication artifact checksum mismatch")
    if f"Sources/{NAME}.swift" not in expected:
        raise RuntimeError("Missing generated Swift")
    framework = directory / f"{NAME}.xcframework"
    info = plistlib.loads((framework / "Info.plist").read_bytes())
    slices = info["AvailableLibraries"]
    if len(slices) != 2 or {entry.get("SupportedPlatformVariant", "device") for entry in slices} != {"device", "simulator"}:
        raise RuntimeError("Expected device and universal simulator slices")
    for entry in slices:
        simulator = entry.get("SupportedPlatformVariant") == "simulator"
        arches = {"arm64", "x86_64"} if simulator else {"arm64"}
        if entry["SupportedPlatform"] != "ios" or set(entry["SupportedArchitectures"]) != arches:
            raise RuntimeError("Unexpected iOS slice")
        binary = framework / entry["LibraryIdentifier"] / entry["LibraryPath"] / f"{NAME}FFI"
        if set(run("lipo", "-archs", str(binary), capture=True).split()) != arches:
            raise RuntimeError("Archive architecture mismatch")
        # Rust's standard archives may contain bitcode newer than Apple's nm.
        symbols = run(symbol_tool(), "--defined-only", "--extern-only", "--just-symbol-name", str(binary), capture=True)
        for operation in ("create_device_identity", "device_public_key", "delete_device_identity"):
            if f"uniffi_{STEM}_fn_method_replicationcustody_{operation}" not in symbols:
                raise RuntimeError("Missing custody export")
        for operation in ("prepare_enrollment", "pending_enrollment", "cancel_pending_enrollment"):
            if f"uniffi_{STEM}_fn_method_mobilereplicationproduct_{operation}" not in symbols:
                raise RuntimeError("Missing mobile product export")


def build():
    shared.space()
    if not run("rustc", "--version", capture=True).startswith("rustc 1.97.1 "):
        raise RuntimeError("Rust 1.97.1 required")
    run("rustup", "component", "add", "llvm-tools-preview")
    run("cargo", "build", "--locked", "-p", "termirust-replication-bindings", "--lib")
    run("cargo", "build", "--locked", "-p", "termirust-controller-bindings", "--features", "bindgen-cli", "--bin", "uniffi-bindgen")
    target = Path(json.loads(run("cargo", "metadata", "--no-deps", "--format-version", "1", capture=True))["target_directory"])
    generator = str(target / "debug/uniffi-bindgen")
    if run(generator, "--version", capture=True).strip() != "uniffi-bindgen 0.32.0":
        raise RuntimeError("UniFFI version mismatch")
    with tempfile.TemporaryDirectory(prefix="replication-ios-") as temp:
        work = Path(temp)
        staged = work / "staged"
        sources = staged / "Sources"
        sources.mkdir(parents=True)
        generated = work / "generated"
        run(generator, "generate", str(target / f"debug/lib{STEM}.dylib"), "--language", "swift", "--no-format", "--out-dir", str(generated))
        swift = generated / f"{NAME}.swift"
        (sources / swift.name).write_text("\n".join(line.rstrip() for line in swift.read_text().splitlines()) + "\n")
        headers = work / "headers"
        headers.mkdir()
        shutil.copy2(generated / f"{NAME}FFI.h", headers)
        shutil.copy2(generated / f"{NAME}FFI.modulemap", headers / "module.modulemap")
        for rust_target in ("aarch64-apple-ios", "aarch64-apple-ios-sim", "x86_64-apple-ios"):
            shared.space()
            run("rustup", "target", "add", rust_target)
            with tempfile.TemporaryDirectory(prefix="slice-", dir=work) as build_dir:
                # Ship machine-code archives, not Rust LLVM bitcode newer than Apple's tools.
                env = dict(os.environ, CARGO_TARGET_DIR=build_dir, IPHONEOS_DEPLOYMENT_TARGET="17.0", CARGO_PROFILE_RELEASE_LTO="false")
                run("cargo", "build", "--locked", "-p", "termirust-replication-bindings", "--release", "--lib", "--target", rust_target, env=env)
                shutil.copy2(Path(build_dir) / rust_target / "release" / f"lib{STEM}.a", work / f"{rust_target}.a")
        run("lipo", "-create", str(work / "aarch64-apple-ios-sim.a"), str(work / "x86_64-apple-ios.a"), "-output", str(work / "simulator.a"))
        run("bash", "scripts/build/ios-static-xcframework.sh", f"{NAME}FFI", str(work / "aarch64-apple-ios.a"), str(work / "simulator.a"), str(headers), str(staged / f"{NAME}.xcframework"))
        (staged / "provenance.json").write_text(json.dumps({"rust": "1.97.1", "uniffi": "0.32.0", "minimum_ios": "17.0", "lto": False, "xcode": run("xcodebuild", "-version", capture=True).strip()}, sort_keys=True) + "\n")
        (staged / "artifacts.json").write_text(json.dumps(shared.inventory(staged), sort_keys=True, indent=2) + "\n")
        shared.promote(staged, OUTPUT, verify)
    print("PASS: iOS replication artifact set built and verified")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("operation", choices=["build", "sync"])
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if args.operation == "build":
        if args.write:
            parser.error("--write applies only to sync")
        build()
    else:
        verify(OUTPUT)
        if args.write:
            shared.promote(OUTPUT, DEST, verify)
        verify(DEST)
        if shared.inventory(OUTPUT) != shared.inventory(DEST):
            raise RuntimeError("Packaged iOS replication differs")
        print("PASS: packaged iOS replication matches verified artifacts")


if __name__ == "__main__":
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    main()
