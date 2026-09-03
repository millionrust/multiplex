#!/usr/bin/env python3
"""Enforce that lower-layer domain, store, and protocol crates remain UI-free."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
FORBIDDEN = {"gpui", "gpui-component"}


def cargo_metadata() -> dict:
    output = subprocess.check_output(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--all-features",
        ],
        cwd=ROOT,
        text=True,
    )
    return json.loads(output)


def governed_package(name: str) -> bool:
    return name in {"termirust-domain", "termirust-store"} or name.endswith("-protocol")


def dependency_path(
    root_id: str,
    package_names: dict[str, str],
    dependencies: dict[str, list[str]],
) -> list[str] | None:
    queue: list[tuple[str, list[str]]] = [(root_id, [root_id])]
    visited = {root_id}
    while queue:
        package_id, path = queue.pop(0)
        if package_id != root_id and package_names.get(package_id) in FORBIDDEN:
            return [package_names[item] for item in path]
        for dependency_id in dependencies.get(package_id, []):
            if dependency_id not in visited:
                visited.add(dependency_id)
                queue.append((dependency_id, [*path, dependency_id]))
    return None


def main() -> int:
    metadata = cargo_metadata()
    package_names = {
        package["id"]: package["name"] for package in metadata.get("packages", [])
    }
    dependencies = {
        node["id"]: [dependency["pkg"] for dependency in node.get("deps", [])]
        for node in (metadata.get("resolve") or {}).get("nodes", [])
    }
    roots = sorted(
        package_id
        for package_id, name in package_names.items()
        if name.startswith("termirust-") and governed_package(name)
    )
    if not roots:
        print("No governed TermiRust packages found.", file=sys.stderr)
        return 1

    failures = []
    for root_id in roots:
        path = dependency_path(root_id, package_names, dependencies)
        if path is not None:
            failures.append(" -> ".join(path))
    if failures:
        print("GPUI dependency boundary violations:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1

    governed = ", ".join(package_names[root_id] for root_id in roots)
    print(f"GPUI dependency boundaries passed: {governed}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
