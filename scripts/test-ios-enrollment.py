"""C05 enrollment or C06 external picker tests on an owned disposable simulator."""
import argparse
import json
import os
import plistlib
from pathlib import Path
import shutil
import signal
import tempfile
import threading
from owned_process import run_owned

ROOT = Path(__file__).resolve().parents[1]
IOS = ROOT / "mobile/ios"


def run(*args, cwd=ROOT, check=True, raw=False):
    result = run_owned(args, cwd=cwd, timeout=1200)
    if result.returncode:
        print(result.stdout[-16000:], flush=True)
        if check:
            result.check_returncode()
    return result if raw else result.stdout


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--device-type", default="com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro")
    parser.add_argument("--runtime", default="com.apple.CoreSimulator.SimRuntime.iOS-26-5")
    parser.add_argument("--provider-picker", action="store_true", help="Run C06 picker access using two disposable fixture apps")
    args = parser.parse_args()
    if shutil.disk_usage(ROOT).free < 18 * 1024**3:
        raise RuntimeError("At least 18 GiB free required before creating an owned simulator")
    stop_monitor = threading.Event()

    def monitor_space():
        while not stop_monitor.wait(2):
            if shutil.disk_usage(ROOT).free < 16 * 1024**3:
                print("STOP: free space below 16 GiB safety margin; cancelling owned test work", flush=True)
                os.kill(os.getpid(), signal.SIGTERM)
                return

    project_dir = IOS / "ProviderAccessFixture" if args.provider_picker else IOS
    project = "ProviderAccessFixture.xcodeproj" if args.provider_picker else "TermiRustMobile.xcodeproj"
    scheme = "ProviderReader" if args.provider_picker else "TermiRustMobile"
    expected = 1 if args.provider_picker else 10
    run("xcodegen", "generate", "--spec", "project.yml", cwd=project_dir)
    label = "TermiRust C06 disposable" if args.provider_picker else "TermiRust C05 disposable"
    device = run("xcrun", "simctl", "create", label, args.device_type, args.runtime).strip()
    try:
        threading.Thread(target=monitor_space, daemon=True).start()
        run("xcrun", "simctl", "boot", device)
        run("xcrun", "simctl", "bootstatus", device, "-b")
        build = ["xcodebuild", "-project", project, "-scheme", scheme,
                 "-destination", f"platform=iOS Simulator,id={device}"]
        if args.provider_picker:
            derived = ROOT / "dist/mobile/c06-picker/DerivedData"
            build += ["-derivedDataPath", str(derived)]
            run(*build, "build", cwd=project_dir)
            donor = derived / "Build/Products/Debug-iphonesimulator/C06Documents.app"
            with (donor / "Info.plist").open("rb") as handle:
                metadata = plistlib.load(handle)
            if not all(metadata.get(key) is True for key in ("UIFileSharingEnabled", "LSSupportsOpeningDocumentsInPlace")):
                raise RuntimeError("Fixture donor does not expose its documents through Files")
            run("xcrun", "simctl", "install", device, str(donor))
        with tempfile.TemporaryDirectory(prefix="termirust-c05-") as temporary:
            result = str(Path(temporary) / "enrollment.xcresult")
            print("RUN: C06 external picker" if args.provider_picker else "RUN: production app, Keychain, enrollment UI and restart", flush=True)
            suites = ["ProviderPickerTests"] if args.provider_picker else [
                "TermiRustMobileTests/EnrollmentWorkflowTests", "TermiRustMobileTests/ReplicationCustodyTests",
                "TermiRustMobileUITests/EnrollmentUITests"]
            execution = run(*build, "test",
                "-parallel-testing-enabled", "NO", "-maximum-concurrent-test-simulator-destinations", "1",
                *[f"-only-testing:{suite}" for suite in suites],
                "-resultBundlePath", result, cwd=project_dir, check=False, raw=True)
            summary = json.loads(run("xcrun", "xcresulttool", "get", "test-results", "summary", "--path", result))
            evidence = ROOT / ("dist/mobile/c06-picker" if args.provider_picker else "dist/mobile/c05-evidence") / args.device_type.split(".")[-1] / device
            evidence.mkdir(parents=True, exist_ok=True)
            (evidence / "results.json").write_text(json.dumps(summary, indent=2) + "\n")
            run("xcrun", "xcresulttool", "export", "attachments", "--path", result, "--output-path", str(evidence))
            if execution.returncode != 0 or summary.get("passedTests") != expected or summary.get("failedTests") != 0 or summary.get("skippedTests") != 0:
                counts = {key: summary.get(key) for key in ("passedTests", "failedTests", "skippedTests")}
                raise RuntimeError(f"Test exit {execution.returncode}; counts {counts}; details: {evidence / 'results.json'}")
            print(f"PASS: {expected} tests, zero skipped; artifacts: " + str(evidence), flush=True)
    finally:
        stop_monitor.set()
        try:
            run("xcrun", "simctl", "shutdown", device)
        finally:
            run("xcrun", "simctl", "delete", device)
        print("CLEANUP: only owned simulator removed", flush=True)


if __name__ == "__main__":
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    main()
