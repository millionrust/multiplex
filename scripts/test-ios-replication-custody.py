"""Run production-target custody tests on an explicitly selected iOS simulator."""
import argparse
import json
from pathlib import Path
import shutil
import signal
import tempfile
from owned_process import run_owned

ROOT = Path(__file__).resolve().parents[1]
IOS = ROOT / "mobile/ios"


def run(*args, cwd=ROOT):
    result = run_owned(args, cwd=cwd, timeout=1200)
    if result.returncode:
        print(result.stdout)
        result.check_returncode()
    return result.stdout


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--simulator", required=True, help="Available simulator UDID; never selects a physical phone")
    parser.add_argument("--provider-only", action="store_true", help="Run C06 coordinated-document fixtures only")
    args = parser.parse_args()
    devices = json.loads(run("xcrun", "simctl", "list", "devices", "available", "-j"))["devices"]
    device = next((d for group in devices.values() for d in group if d["udid"] == args.simulator), None)
    if device is None:
        raise RuntimeError("Selected simulator is unavailable")
    if shutil.disk_usage(ROOT).free < 16 * 1024**3:
        raise RuntimeError("At least 16 GiB free required")
    run("python3", "scripts/ios-replication-artifacts.py", "sync")
    run("xcodegen", "generate", "--spec", "project.yml", cwd=IOS)
    common = ("xcodebuild", "-project", "TermiRustMobile.xcodeproj", "-scheme", "TermiRustMobile", "-configuration", "Debug")
    owned_boot = device["state"] == "Shutdown"
    try:
        if owned_boot:
            run("xcrun", "simctl", "boot", args.simulator)
        run("xcrun", "simctl", "bootstatus", args.simulator, "-b")
        run(*common, "build", "-destination", "generic/platform=iOS", "CODE_SIGNING_ALLOWED=NO", cwd=IOS)
        print("PASS: production iOS device build", flush=True)
        with tempfile.TemporaryDirectory(prefix="replication-ios-tests-") as temp:
            result = str(Path(temp) / "custody.xcresult")
            suites = ["ReplicationDocumentReaderTests"] if args.provider_only else [
                "ReplicationCustodyTests", "ControllerBindingConformanceTests", "UnifiedRouteLifecycleTests",
                "AppleControllerRouteTests", "AppleControllerRouteViewModelTests"]
            run(*common, "test", "-destination", f"platform=iOS Simulator,id={args.simulator}",
                "-parallel-testing-enabled", "NO", *[f"-only-testing:TermiRustMobileTests/{suite}" for suite in suites],
                "-resultBundlePath", result, cwd=IOS)
            summary = json.loads(run("xcrun", "xcresulttool", "get", "test-results", "summary", "--path", result))
            expected = 7 if args.provider_only else 21
            if summary.get("passedTests") != expected or summary.get("failedTests") != 0 or summary.get("skippedTests") != 0:
                raise RuntimeError(f"Unexpected custody test counts: {summary}")
            coverage = "C06 coordinated document fixtures" if args.provider_only else "6 real-framework custody/enrollment, 15 Controller/lifecycle"
            print(f"PASS: {expected} tests ({coverage}), zero skipped; {device['name']} ({args.simulator})")
    finally:
        if owned_boot:
            run("xcrun", "simctl", "shutdown", args.simulator)


if __name__ == "__main__":
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    main()
