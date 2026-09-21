"""Build and promote one complete Android replication artifact set, never Controller files."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import sys
import tempfile
from owned_process import run_owned


def host_build():
    """Cargo arguments, environment, and profile directory for the host library and generator.

    Normally a debug build. When CI shares one target directory, the controller and screen scripts
    have already built the same generator there as an unstripped release build, so this matches
    them and reuses it instead of compiling UniFFI and its dependencies a second time. The bindings
    generated from either are byte-identical.
    """
    if os.environ.get("MULTIPLEX_MOBILE_CARGO_TARGET_DIR"):
        return ["--release"], dict(os.environ, CARGO_PROFILE_RELEASE_STRIP="none"), "release"
    return [], dict(os.environ), "debug"


def slice_target(work):
    """The cargo target directory for one slice: new and empty unless CI shares one.

    CI only verifies these artifacts, so it sets MULTIPLEX_MOBILE_CARGO_TARGET_DIR to one cached
    directory that every mobile library builds into; anything that ships starts from nothing.
    """
    shared = os.environ.get("MULTIPLEX_MOBILE_CARGO_TARGET_DIR")
    if shared:
        return contextlib.nullcontext(shared)
    return tempfile.TemporaryDirectory(prefix="slice-", dir=work)

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "dist/mobile/replication"
DEST = ROOT / "apps/android/app/src/main/replication"
STEM = "multiplex_replication_bindings"
ABIS = {
    "aarch64-linux-android": ("arm64-v8a", "aarch64-linux-android", "AArch64"),
    "armv7-linux-androideabi": ("armeabi-v7a", "armv7a-linux-androideabi", "ARM"),
    "i686-linux-android": ("x86", "i686-linux-android", "Intel 80386"),
    "x86_64-linux-android": ("x86_64", "x86_64-linux-android", "Advanced Micro Devices X86-64"),
}


def run(*args, env=None, capture=False):
    result = run_owned(args, cwd=ROOT, env=env, capture=capture, timeout=1200,
                       minimum_free_bytes=16 * 1024**3)
    result.check_returncode()
    return result.stdout


def space():
    if shutil.disk_usage(ROOT).free < 16 * 1024**3:
        raise RuntimeError("At least 16 GiB free is required before another build slice")


def inventory(directory):
    return {str(p.relative_to(directory)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(directory.rglob("*")) if p.is_file() and p.name != "artifacts.json"}


def verify(directory):
    expected = json.loads((directory / "artifacts.json").read_text())
    if expected != inventory(directory):
        raise RuntimeError("Replication artifact checksums differ")
    for abi, _, _ in ABIS.values():
        if f"jniLibs/{abi}/lib{STEM}.so" not in expected:
            raise RuntimeError("Missing replication ABI")
    if f"kotlin/com/multiplex/replication/security/{STEM}.kt" not in expected:
        raise RuntimeError("Missing generated Kotlin binding")


def promote(source, destination, validator=verify):
    # One owned directory contains Kotlin, all four libraries, and the inventory.
    # No app source set is changed until the complete staged set passes verification.
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".replication-publish-", dir=destination.parent) as temp:
        next_dir, old = Path(temp) / "next", Path(temp) / "previous"
        shutil.copytree(source, next_dir)
        validator(next_dir)
        try:
            if destination.exists():
                destination.rename(old)
            next_dir.rename(destination)
        except BaseException:
            if old.exists() and not destination.exists():
                old.rename(destination)
            raise


PINNED_NDK = "27.0.12077973"
# The pin is what makes a shipped artifact reproducible. A build that only has to prove the
# application still compiles does not need it, and a hosted runner does not get to choose its
# NDK, so it may say so — loudly, and never for anything released.
UNPINNED = os.environ.get("MULTIPLEX_REPLICATION_ARTIFACTS_ALLOW_UNPINNED", os.environ.get("TERMIRUST_REPLICATION_ARTIFACTS_ALLOW_UNPINNED", "0")) != "0"


def require_pinned(what, expected, found):
    if found == expected:
        return
    if UNPINNED:
        print(
            f"warning: {what} is {found}, not the pinned {expected}. "
            "These artifacts are not release-reproducible.",
            file=sys.stderr,
        )
        return
    raise RuntimeError(f"Expected {what} {expected}, found {found}")


def locate_ndk(sdk):
    """The pinned NDK, or the newest installed one when the pin is waived."""
    pinned = sdk / "ndk" / PINNED_NDK
    if pinned.is_dir() or not UNPINNED:
        return pinned
    installed = sorted(p for p in (sdk / "ndk").glob("*") if p.is_dir())
    return installed[-1] if installed else pinned


def build():
    require_pinned("rustc", "1.98.1", run("rustc", "--version", capture=True).split()[1])
    sdk = Path(os.environ.get("ANDROID_HOME", Path.home() / "Library/Android/sdk"))
    ndk = locate_ndk(sdk)
    properties = ndk / "source.properties"
    if not properties.is_file():
        raise RuntimeError("Pinned Android NDK metadata missing")
    revision = dict(line.split("=", 1) for line in properties.read_text().splitlines() if "=" in line)
    found = next((value.strip() for key, value in revision.items() if key.strip() == "Pkg.Revision"), None)
    require_pinned("the Android NDK", PINNED_NDK, found)
    host = "darwin-x86_64" if platform.system() == "Darwin" else "linux-x86_64"
    toolchain = ndk / "toolchains/llvm/prebuilt" / host / "bin"
    readelf = str(toolchain / "llvm-readelf")
    if not Path(readelf).is_file():
        raise RuntimeError(f"Android NDK llvm-readelf missing at {readelf}")
    space()
    host_args, host_env, host_profile = host_build()
    run("cargo", "build", "--locked", "-p", "multiplex-replication-bindings", "--lib", *host_args, env=host_env)
    # The existing pinned generator is shared, without changing Controller outputs.
    run("cargo", "build", "--locked", "-p", "multiplex-controller-bindings",
        "--features", "bindgen-cli", "--bin", "uniffi-bindgen", *host_args, env=host_env)
    target = Path(json.loads(run("cargo", "metadata", "--no-deps", "--format-version", "1", capture=True))["target_directory"])
    generator = str(target / host_profile / "uniffi-bindgen")
    if run(generator, "--version", capture=True).strip() != "uniffi-bindgen 0.32.0":
        raise RuntimeError("UniFFI version mismatch")
    with tempfile.TemporaryDirectory(prefix="replication-build-") as temp:
        work = Path(temp)
        staged = work / "staged"
        generated = staged / "kotlin"
        generated.mkdir(parents=True)
        ext = "dylib" if platform.system() == "Darwin" else "so"
        run(generator, "generate", str(target / host_profile / f"lib{STEM}.{ext}"),
            "--language", "kotlin", "--no-format", "--out-dir", str(generated))
        for path in generated.rglob("*.kt"):
            path.write_text("\n".join(line.rstrip() for line in path.read_text().splitlines()).rstrip() + "\n")
        for rust_target, (abi, clang, machine) in ABIS.items():
            space()
            run("rustup", "target", "add", rust_target)
            with slice_target(work) as slice_dir:
                env = dict(os.environ, CARGO_TARGET_DIR=slice_dir)
                env["CARGO_TARGET_" + rust_target.upper().replace("-", "_") + "_LINKER"] = str(toolchain / (clang + "26-clang"))
                env["RUSTFLAGS"] = "-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384"
                run("cargo", "build", "--locked", "-p", "multiplex-replication-bindings",
                    "--release", "--lib", "--target", rust_target, env=env)
                lib = Path(slice_dir) / rust_target / "release" / f"lib{STEM}.so"
                if machine not in run(readelf, "-h", str(lib), capture=True):
                    raise RuntimeError("Incorrect ABI machine")
                segments = run(readelf, "-lW", str(lib), capture=True)
                alignments = [int(line.split()[-1], 16) for line in segments.splitlines() if line.strip().startswith("LOAD ")]
                if not alignments or min(alignments) < 16384:
                    raise RuntimeError("Incorrect native page alignment")
                symbols = run(readelf, "--dyn-syms", "--wide", str(lib), capture=True)
                names = sorted({line.split()[-1] for line in symbols.splitlines()
                                if "uniffi_" + STEM in line and " UND " not in line})
                for operation in ("create_device_identity", "device_public_key", "delete_device_identity", "prepare_enrollment", "pending_enrollment", "cancel_pending_enrollment", "recover_pending_enrollment", "review_enrollment", "accept_enrollment", "review_record_transfer", "apply_record_transfer", "review_host_transfer", "apply_host_transfer", "preview_host_transfers", "imported_hosts"):
                    if not any(operation in name for name in names):
                        raise RuntimeError("Required exported custody symbol missing")
                dest = staged / "jniLibs" / abi
                dest.mkdir(parents=True)
                shutil.copy2(lib, dest / lib.name)
                (staged / f"symbols-{abi}.txt").write_text("\n".join(names) + "\n")
        (staged / "provenance.json").write_text(json.dumps({"rust": "1.98.1", "uniffi": "0.32.0", "ndk": "27.0.12077973", "api": 26, "alignment": 16384}, sort_keys=True) + "\n")
        (staged / "artifacts.json").write_text(json.dumps(inventory(staged), sort_keys=True, indent=2) + "\n")
        promote(staged, OUTPUT)
    print("PASS: all four Android replication artifacts staged and verified")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("operation", choices=["build", "sync"])
    parser.add_argument("--android", action="store_true", required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if args.operation == "build":
        if args.check or args.write:
            parser.error("--check/--write apply only to sync")
        build()
    else:
        verify(OUTPUT)
        if args.write:
            promote(OUTPUT, DEST)
        verify(DEST)
        if inventory(OUTPUT) != inventory(DEST):
            raise RuntimeError("Packaged replication artifacts differ")
        print("PASS: packaged replication artifacts match verified output")


if __name__ == "__main__":
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    main()
