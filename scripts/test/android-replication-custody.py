"""Bounded C01 runner. Only an owned, read-only emulator receives a fixture PIN."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import shutil
import socket
import subprocess
import tempfile
import time
import sys
import uuid
import zipfile

# Shared process helpers live beside the artifact builders.
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "build"))
from owned_process import run_owned  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
APP = "com.termirust.mobile"
TEST_APP = APP + ".test/androidx.test.runner.AndroidJUnitRunner"
TEST_CLASS = APP + ".replication.ReplicationCustodyInstrumentedTest"
RESTART = APP + ".replication.ReplicationRestartTest"
ACCEPTANCE = APP + ".replication.EnrollmentAcceptanceTest"


def command(*args, timeout=120, check=True, minimum_free_bytes=None):
    result = run_owned(args, cwd=ROOT, timeout=timeout, minimum_free_bytes=minimum_free_bytes)
    if check and result.returncode:
        raise RuntimeError(f"Command failed ({result.returncode}): {args[0]}\n{result.stdout}")
    return result


def main():
    parser = argparse.ArgumentParser()
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--avd")
    target.add_argument("--serial")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--provider-only", action="store_true", help="Run C06 disposable document-provider fixtures only")
    mode.add_argument("--enrollment-only", action="store_true", help="Run C07 desktop/service-to-Android acceptance fixture")
    parser.add_argument("--system-picker", action="store_true", help="Use the real Android document picker for C07 acceptance")
    args = parser.parse_args()
    if args.system_picker and not args.enrollment_only:
        parser.error("--system-picker requires --enrollment-only")
    sdk = Path(os.environ.get("ANDROID_HOME", Path.home() / "Library/Android/sdk"))
    os.environ["ANDROID_HOME"] = str(sdk)
    adb = str(sdk / "platform-tools/adb")
    emulator = str(sdk / "emulator/emulator")
    serial = args.serial
    owned = None
    installed = False
    enrollment_started = False
    token = str(uuid.uuid4())
    results = []

    def shell(*args, **kwargs):
        return command(adb, "-s", serial, "shell", *args, **kwargs)

    def instrument(name, count):
        output = shell("am", "instrument", "-w", "-r", "-e", "class", name,
                       "-e", "fixture", token, TEST_APP, timeout=180).stdout
        print(output, flush=True)
        expected = rf"OK \({count} test{'s' if count != 1 else ''}\)"
        if not re.search(expected, output) or any(marker in output for marker in
            ("FAILURES", "INSTRUMENTATION_FAILED", "INSTRUMENTATION_STATUS_CODE: -3", "INSTRUMENTATION_STATUS_CODE: -4")):
            raise RuntimeError("Native instrumentation failed or skipped required proof")
        results.append({"class": name, "tests": count})

    with tempfile.TemporaryDirectory(prefix="c01-android-") as temp:
        log = open(Path(temp) / "emulator.log", "w+")
        try:
            if args.avd:
                if shutil.disk_usage(ROOT).free < 17 * 1024**3:
                    raise RuntimeError("At least 17 GiB free required to start a disposable emulator")
                if args.avd not in command(emulator, "-list-avds").stdout.splitlines():
                    raise RuntimeError("Selected AVD is not installed")
                devices = command(adb, "devices").stdout
                port = None
                for candidate in range(5560, 5680, 2):
                    if f"emulator-{candidate}" in devices:
                        continue
                    with socket.socket() as first, socket.socket() as second:
                        try:
                            first.bind(("127.0.0.1", candidate))
                            second.bind(("127.0.0.1", candidate + 1))
                            port = candidate
                            break
                        except OSError:
                            pass
                if port is None:
                    raise RuntimeError("No free emulator port pair")
                serial = f"emulator-{port}"
                owned = subprocess.Popen([emulator, "-avd", args.avd, "-port", str(port),
                    "-read-only", "-no-snapshot", "-no-window", "-no-audio", "-no-boot-anim"],
                    stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            else:
                if command(adb, "-s", serial, "get-state", check=False).stdout.strip() != "device":
                    raise RuntimeError("Selected device is missing, unauthorized, or offline")
            deadline = time.monotonic() + 180
            while time.monotonic() < deadline:
                if owned and shutil.disk_usage(ROOT).free < 16 * 1024**3:
                    raise RuntimeError("Emulator startup stopped to preserve minimum free disk space")
                if owned and owned.poll() is not None:
                    log.seek(0)
                    raise RuntimeError("Owned emulator exited:\n" + log.read(4000))
                if shell("getprop", "sys.boot_completed", timeout=10, check=False).stdout.strip() == "1":
                    break
                time.sleep(1)
            else:
                raise RuntimeError("Selected device did not boot in time")
            api = shell("getprop", "ro.build.version.sdk").stdout.strip()
            device_abi = shell("getprop", "ro.product.cpu.abi").stdout.strip()
            pages = shell("getconf", "PAGESIZE").stdout.strip()
            print(f"DEVICE: API {api}, {device_abi}, page size {pages}", flush=True)
            if owned and not args.provider_only:
                # Read-only AVD changes are discarded on shutdown; no personal device is locked.
                # Compilation and instrumentation can outlast the default screen timeout.
                shell("svc", "power", "stayon", "true")
                shell("settings", "put", "system", "screen_off_timeout", "1800000")
                pin = "246813"
                result = shell("locksettings", "set-pin", pin).stdout
                if "pin set" not in result.lower():
                    raise RuntimeError("Disposable emulator needs an unset credential; refusing to replace one")
                shell("input", "keyevent", "KEYCODE_SLEEP")
                shell("input", "keyevent", "KEYCODE_WAKEUP")
                time.sleep(1)
                shell("input", "swipe", "500", "1800", "500", "400", "300")
                shell("input", "text", pin)
                shell("input", "keyevent", "KEYCODE_ENTER")
                time.sleep(1)
            print("RUN: verifying Android package and native custody", flush=True)
            result = command(str(ROOT / "apps/android/gradlew"), "-p", str(ROOT / "apps/android"),
                             "assembleDebug", "assembleDebugAndroidTest", "--no-daemon", "--console=plain", timeout=900,
                             minimum_free_bytes=16 * 1024**3)
            print(result.stdout, flush=True)
            apk = ROOT / "apps/android/app/build/outputs/apk/debug/app-debug.apk"
            tests = ROOT / "apps/android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
            with zipfile.ZipFile(apk) as archive:
                for abi in ("arm64-v8a", "armeabi-v7a", "x86", "x86_64"):
                    name = f"lib/{abi}/libtermirust_replication_bindings.so"
                    actual = hashlib.sha256(archive.read(name)).hexdigest()
                    source = ROOT / "apps/android/app/src/main/replication/jniLibs" / abi / "libtermirust_replication_bindings.so"
                    if actual != hashlib.sha256(source.read_bytes()).hexdigest():
                        raise RuntimeError("APK native custody checksum differs from verified artifact")
            command(adb, "-s", serial, "install", "-r", str(apk))
            command(adb, "-s", serial, "install", "-r", str(tests))
            if args.enrollment_only:
                enrollment_started = True
                instrument(APP + ".replication.ReplicationDocumentReaderTest", 7)
                instrument(ACCEPTANCE + "#prepare", 1)
                remote = "/sdcard/Android/data/" + APP + "/files/c07-" + token
                request = Path(temp) / "request.json"
                command(adb, "-s", serial, "pull", remote + "/request.json", str(request))
                fixture = Path(temp) / "desktop-fixture"
                output = command("cargo", "run", "--locked", "-p", "termirust-replication-bindings",
                                 "--example", "android_enrollment_fixture", "--", str(request), str(fixture),
                                 timeout=600, minimum_free_bytes=16 * 1024**3)
                print(output.stdout, flush=True)
                for name in ("bundle.json", "replica.json", "review.json"):
                    command(adb, "-s", serial, "push", str(fixture / name), remote + "/" + name)
                recovery_stages = ("failPreparedCreation", "recoverPreparedCreation", "failActivation", "recoverActivation", "prepareCommittedRecovery", "finishCommittedRecovery")
                for stage in ("failPreparedCreation", "recoverPreparedCreation", "failActivation", "recoverActivation", "accept", "prepareCommittedRecovery",
                              "finishCommittedRecovery", "importHost", "reopen"):
                    shell("am", "force-stop", APP)
                    if stage in recovery_stages:
                        fixture_class = APP + ".replication.EnrollmentActivationTest"
                    else:
                        fixture_class = APP + ".replication.EnrollmentPickerTest" if args.system_picker and stage != "reopen" else ACCEPTANCE
                    instrument(fixture_class + "#" + stage, 1)
                evidence = ROOT / ("dist/mobile/c07-picker-evidence" if args.system_picker else "dist/mobile/c07-evidence")
                evidence.mkdir(parents=True, exist_ok=True)
                for name in ("accepted.png", "imported.png"):
                    command(adb, "-s", serial, "pull", remote + "/" + name, str(evidence / name))
                instrument(ACCEPTANCE + "#cleanup", 1)
                enrollment_started = False
                (evidence / "results.json").write_text(json.dumps({
                    "api": api, "abi": device_abi, "page_size": pages,
                    "owned_emulator": owned is not None, "runs": results,
                    "system_picker": args.system_picker,
                    "passed": sum(run["tests"] for run in results), "skipped": 0,
                }, indent=2) + "\n")
                print("PASS: 18 provider, recovery and desktop-to-Android enrollment/import executions", flush=True)
                return
            if args.provider_only:
                instrument(APP + ".replication.ReplicationDocumentReaderTest", 7)
                print("PASS: 7 C06 real ContentResolver/DocumentsProvider tests, zero skipped", flush=True)
                return
            installed = True
            instrument(TEST_CLASS, 11)
            workflow = APP + ".replication.EnrollmentWorkflowInstrumentedTest," + APP + ".replication.EnrollmentEntryInstrumentedTest"
            instrument(workflow, 9)
            if owned:
                for size, density in (("1600x2560", "240"), ("1600x900", "240")):
                    shell("wm", "size", size)
                    shell("wm", "density", density)
                    instrument(workflow, 9)
                shell("wm", "size", "reset")
                shell("wm", "density", "reset")
            evidence = ROOT / "dist/mobile/c04-evidence"
            evidence.mkdir(parents=True, exist_ok=True)
            command(adb, "-s", serial, "pull", "/sdcard/Android/data/" + APP + "/files/c04-evidence/.", str(evidence))
            instrument(RESTART + "#prepare", 1)
            shell("am", "force-stop", APP)
            instrument(RESTART + "#reopen", 1)
            instrument(RESTART + "#cleanup", 1)
            installed = False
            (evidence / "results.json").write_text(json.dumps({
                "api": api, "abi": device_abi, "page_size": pages,
                "owned_emulator": owned is not None, "runs": results,
                "passed": sum(run["tests"] for run in results), "skipped": 0,
            }, indent=2) + "\n")
            print(f"PASS: {sum(run['tests'] for run in results)} instrumentation tests, packaged JNI/Keystore, enrollment UI, threads, second process, and process restart", flush=True)
        finally:
            if enrollment_started:
                try:
                    shell("am", "force-stop", APP)
                    instrument(ACCEPTANCE + "#cleanup", 1)
                except Exception as error:
                    print(f"Enrollment fixture cleanup incomplete: {type(error).__name__}", flush=True)
            if installed:
                try:
                    shell("am", "force-stop", APP)
                    instrument(RESTART + "#cleanup", 1)
                except Exception as error:
                    print(f"Fixture cleanup incomplete: {type(error).__name__}", flush=True)
            if owned:
                try:
                    command(adb, "-s", serial, "emu", "kill", timeout=10, check=False)
                except subprocess.TimeoutExpired:
                    pass
                try:
                    owned.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(owned.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    try:
                        owned.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        try:
                            os.killpg(owned.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        owned.wait(timeout=10)
                print("CLEANUP: owned read-only emulator stopped; no app data cleared or uninstalled", flush=True)
            log.close()


if __name__ == "__main__":
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    main()
