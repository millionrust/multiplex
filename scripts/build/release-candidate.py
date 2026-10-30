"""Reuse complete shipping artifacts only from successful runs of this workflow and commit."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


MANIFEST = "release-candidate.json"
ROOT = Path(__file__).resolve().parents[2]


def api(endpoint):
    return json.loads(subprocess.check_output(["gh", "api", endpoint], text=True))


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def workspace_version():
    package = (ROOT / "Cargo.toml").read_text().split("[workspace.package]", 1)[1]
    return re.search(r'^version = "([^"]+)"', package, re.MULTILINE)[1]


def expected_packages(version):
    return {
        "Multiplex-macos-universal.zip",
        "Multiplex-linux-x86_64.tar.gz",
        f"multiplex_{version}_amd64.deb",
        "Multiplex-windows-x86_64.zip",
        "Multiplex-windows-x86_64.msi",
        "Multiplex-windows-aarch64.zip",
        "Multiplex-windows-aarch64.msi",
        f"Multiplex-{version}-android.apk",
        f"Multiplex-{version}-ios-unsigned.ipa",
    }


def expected_sboms():
    return {f"Multiplex-{platform}.spdx.json" for platform in (
        "macos-universal", "linux-x86_64", "windows-x86_64", "windows-aarch64",
    )}


def discover():
    repo, sha, current = (os.environ[key] for key in ("GITHUB_REPOSITORY", "GITHUB_SHA", "GITHUB_RUN_ID"))
    # Manual publication must never attach binaries to a tag pointing at different source.
    tag = os.environ.get("RELEASE_TAG", "")
    if os.environ.get("GITHUB_REF", "").startswith("refs/tags/"):
        tag = tag or os.environ["GITHUB_REF"].removeprefix("refs/tags/")
    if tag:
        if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?", tag):
            raise ValueError("Release tag must be a version beginning with v")
        if tag != "v" + workspace_version():
            raise ValueError("Release tag does not match the workspace version")
        subprocess.run(["git", "fetch", "--no-tags", "origin", f"refs/tags/{tag}"], check=True)
        tagged = subprocess.check_output(["git", "rev-parse", "FETCH_HEAD^{commit}"], text=True).strip()
        if tagged != sha:
            raise ValueError("Release tag does not point to the checked-out commit")
    run = api(f"repos/{repo}/actions/runs/{current}")
    workflow = run["workflow_id"]
    candidates = api(f"repos/{repo}/actions/workflows/{workflow}/runs?head_sha={sha}&status=success&per_page=30")
    selected = ""
    for candidate in candidates["workflow_runs"]:
        if (str(candidate["id"]) == current or candidate["head_sha"] != sha
                or candidate["event"] not in {"push", "workflow_dispatch"}
                or candidate["head_repository"]["full_name"] != repo):
            continue
        artifacts = api(f"repos/{repo}/actions/runs/{candidate['id']}/artifacts?per_page=100")
        if any(a["name"] == "release-candidate" and not a["expired"] for a in artifacts["artifacts"]):
            selected = str(candidate["id"])
            break
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"run_id={selected}\n")
    print(f"Promoting shipping candidate from run {selected}" if selected else "No complete candidate: build and populate shipping caches")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
            summary.write(f"Reusing release candidate from run {selected} for `{sha}`; compilation skipped.\n"
                          if selected else f"Building shipping candidate for `{sha}` with shared compiler caches.\n")


def create(source, destination):
    destination.mkdir(parents=True, exist_ok=True)
    if any(destination.iterdir()):
        raise ValueError("Candidate destination must be empty")
    for directory in sorted(source.iterdir()):
        if not directory.is_dir():
            raise ValueError(f"Unexpected artifact entry: {directory}")
        for path in sorted(directory.iterdir()):
            if not path.is_file() or path.is_symlink() or (destination / path.name).exists():
                raise ValueError(f"Non-flat artifact or duplicate release filename: {path}")
            shutil.copy2(path, destination / path.name)
    inventory = {p.name: digest(p) for p in sorted(destination.iterdir())}
    (destination / MANIFEST).write_text(json.dumps({
        "schema": 1, "commit": os.environ["GITHUB_SHA"], "files": inventory,
    }, indent=2) + "\n")


def verify(directory):
    manifest = json.loads((directory / MANIFEST).read_text())
    if manifest["schema"] != 1 or manifest["commit"] != os.environ["GITHUB_SHA"]:
        raise ValueError("Candidate schema or commit mismatch")
    files = manifest["files"]
    actual = {p.name for p in directory.iterdir() if p.name != MANIFEST}
    if set(files) != actual:
        raise ValueError("Candidate inventory has missing or unexpected files")
    version = workspace_version()
    required = expected_packages(version) | expected_sboms()
    if not required <= actual:
        raise ValueError(f"Missing release files: {required - actual}")
    checksummed = set()
    for name, checksum in files.items():
        path = directory / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size == 0 or digest(path) != checksum:
            raise ValueError(f"Candidate checksum or file mismatch: {name}")
        if name.endswith(".sha256"):
            for line in path.read_text().splitlines():
                match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *]([^/\\]+)", line)
                if not match or files.get(match[2]) != match[1].lower():
                    raise ValueError(f"Invalid release checksum in {name}: {line}")
                checksummed.add(match[2])
    if not expected_packages(version) <= checksummed:
        raise ValueError("Every release package must have a valid downloadable checksum")
    print(f"Verified {len(files)} release files for {manifest['commit']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("discover", "create", "verify"))
    parser.add_argument("paths", type=Path, nargs="*")
    args = parser.parse_args()
    expected = {"discover": 0, "create": 2, "verify": 1}[args.command]
    if len(args.paths) != expected:
        parser.error(f"{args.command} expects {expected} paths")
    {"discover": discover, "create": create, "verify": verify}[args.command](*args.paths)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        print(f"Release candidate rejected: {error}", file=sys.stderr)
        sys.exit(1)
