"""Bound subprocess trees to the lifetime of their invoking verification step."""
import os
import signal
import subprocess
import shutil
import time


def run_owned(args, *, cwd, timeout, env=None, capture=True, minimum_free_bytes=None):
    def check_space():
        if minimum_free_bytes is not None and shutil.disk_usage(cwd).free < minimum_free_bytes:
            raise RuntimeError("Verification stopped to preserve minimum free disk space")

    check_space()
    process = subprocess.Popen(args, cwd=cwd, env=env, text=True,
                               stdout=subprocess.PIPE if capture else None,
                               stderr=subprocess.STDOUT if capture else None,
                               start_new_session=True)
    try:
        deadline = time.monotonic() + timeout
        while True:
            check_space()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired(args, timeout)
            try:
                output, _ = process.communicate(timeout=min(2, remaining) if minimum_free_bytes is not None else remaining)
                break
            except subprocess.TimeoutExpired:
                if time.monotonic() >= deadline:
                    raise
        return subprocess.CompletedProcess(args, process.returncode, output)
    except BaseException:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.communicate(timeout=10)
        raise
