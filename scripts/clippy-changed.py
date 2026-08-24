#!/usr/bin/env python3
"""Fail on Clippy warnings whose primary span overlaps changed Rust lines."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
HUNK = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


def run(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=ROOT,
        check=check,
        text=True,
        stdout=subprocess.PIPE,
        stderr=None,
    )


def changed_lines() -> dict[str, set[int] | None]:
    base = os.environ.get("TERMIRUST_CLIPPY_BASE", "HEAD")
    diff = run("git", "diff", "--unified=0", base, "--", "*.rs").stdout
    changed: dict[str, set[int] | None] = {}
    current: str | None = None
    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            current = line[6:]
            changed.setdefault(current, set())
            continue
        match = HUNK.match(line)
        if current is None or match is None:
            continue
        start = int(match.group(1))
        length = int(match.group(2) or "1")
        if length:
            assert isinstance(changed[current], set)
            changed[current].update(range(start, start + length))

    untracked = run(
        "git", "ls-files", "--others", "--exclude-standard", "--", "*.rs"
    ).stdout
    for file_name in untracked.splitlines():
        changed[file_name] = None
    return changed


def main() -> int:
    changed = changed_lines()
    command = [
        "cargo",
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--message-format=json",
    ]
    process = subprocess.Popen(
        command,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
    )
    assert process.stdout is not None
    failures: list[str] = []
    for line in process.stdout:
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        diagnostic = message.get("message", {})
        if message.get("reason") != "compiler-message" or diagnostic.get("level") != "warning":
            continue
        for span in diagnostic.get("spans", []):
            if not span.get("is_primary"):
                continue
            file_name = span.get("file_name", "")
            selected = changed.get(file_name)
            if file_name not in changed:
                continue
            start = int(span.get("line_start", 0))
            end = int(span.get("line_end", start))
            if selected is None or any(line in selected for line in range(start, end + 1)):
                code = (diagnostic.get("code") or {}).get("code", "warning")
                failures.append(
                    f"{file_name}:{start}: {code}: {diagnostic.get('message', '')}"
                )
            break

    status = process.wait()
    if status != 0:
        return status
    if failures:
        print("Clippy warnings overlap changed Rust lines:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1
    print("Changed Rust lines are Clippy-clean.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
