"""No native build: publication must reject incomplete/tampered sets and roll back failures."""
import importlib.util
import json
from pathlib import Path
import tempfile
import subprocess
import sys
import time
import unittest
from unittest.mock import patch
from types import SimpleNamespace

# The builder under test and its process helpers live in scripts/build.
BUILD = Path(__file__).resolve().parents[1] / "build"
sys.path.insert(0, str(BUILD))
from owned_process import run_owned  # noqa: E402

spec = importlib.util.spec_from_file_location("artifacts", BUILD / "mobile-replication-artifacts.py")
artifacts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(artifacts)


class ArtifactPublicationTests(unittest.TestCase):
    def fixture(self, path, value):
        path.mkdir()
        for abi, _, _ in artifacts.ABIS.values():
            library = path / "jniLibs" / abi / f"lib{artifacts.STEM}.so"
            library.parent.mkdir(parents=True)
            library.write_bytes(value)
        kotlin = path / f"kotlin/com/termirust/replication/security/{artifacts.STEM}.kt"
        kotlin.parent.mkdir(parents=True)
        kotlin.write_bytes(value)
        (path / "artifacts.json").write_text(json.dumps(artifacts.inventory(path)))

    def test_tamper_or_missing_abi_does_not_modify_destination(self):
        with tempfile.TemporaryDirectory() as temp:
            old, new = Path(temp) / "current", Path(temp) / "new"
            self.fixture(old, b"old")
            self.fixture(new, b"new")
            before = artifacts.inventory(old)
            library = new / "jniLibs/x86" / f"lib{artifacts.STEM}.so"
            library.write_bytes(b"tampered")
            with self.assertRaises(RuntimeError):
                artifacts.promote(new, old)
            self.assertEqual(before, artifacts.inventory(old))
            library.unlink()
            (new / "artifacts.json").write_text(json.dumps(artifacts.inventory(new)))
            with self.assertRaises(RuntimeError):
                artifacts.promote(new, old)
            self.assertEqual(before, artifacts.inventory(old))

    def test_failed_promotion_restores_previous_set(self):
        with tempfile.TemporaryDirectory() as temp:
            old, new = Path(temp) / "current", Path(temp) / "new"
            self.fixture(old, b"old")
            self.fixture(new, b"new")
            before = artifacts.inventory(old)
            rename = Path.rename

            def fail_next(path, destination):
                if path.name == "next":
                    raise OSError("simulated publication failure")
                return rename(path, destination)

            with patch.object(Path, "rename", fail_next), self.assertRaises(OSError):
                artifacts.promote(new, old)
            artifacts.verify(old)
            self.assertEqual(before, artifacts.inventory(old))

    def test_complete_set_replaces_only_its_owned_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            old, new = Path(temp) / "current", Path(temp) / "new"
            sentinel = Path(temp) / "controller"
            sentinel.write_bytes(b"unrelated")
            self.fixture(old, b"old")
            self.fixture(new, b"new")
            artifacts.promote(new, old)
            self.assertEqual(artifacts.inventory(new), artifacts.inventory(old))
            self.assertEqual(b"unrelated", sentinel.read_bytes())

    def test_timeout_stops_the_owned_child_tree(self):
        with tempfile.TemporaryDirectory() as temp:
            marker = Path(temp) / "leaked-child"
            child = "import time, pathlib; time.sleep(0.5); pathlib.Path(" + repr(str(marker)) + ").touch()"
            parent = "import subprocess, sys, time; subprocess.Popen([sys.executable, '-c', " + repr(child) + "]); time.sleep(10)"
            with self.assertRaises(subprocess.TimeoutExpired):
                run_owned([sys.executable, "-c", parent], cwd=temp, timeout=0.2)
            time.sleep(0.7)
            self.assertFalse(marker.exists())

    def test_low_space_refuses_to_start_a_process(self):
        with tempfile.TemporaryDirectory() as temp:
            with patch("owned_process.shutil.disk_usage", return_value=SimpleNamespace(free=9)), \
                    patch("owned_process.subprocess.Popen") as start:
                with self.assertRaisesRegex(RuntimeError, "minimum free disk space"):
                    run_owned([sys.executable, "-c", "pass"], cwd=temp, timeout=5, minimum_free_bytes=10)
                start.assert_not_called()

    def test_space_drop_stops_owned_parent_and_child(self):
        with tempfile.TemporaryDirectory() as temp:
            marker = Path(temp) / "leaked-after-space-drop"
            child = "import time, pathlib; time.sleep(2.5); pathlib.Path(" + repr(str(marker)) + ").touch()"
            parent = "import subprocess, sys, time; subprocess.Popen([sys.executable, '-c', " + repr(child) + "]); time.sleep(10)"
            with patch("owned_process.shutil.disk_usage", side_effect=[SimpleNamespace(free=10), SimpleNamespace(free=10), SimpleNamespace(free=9)]):
                with self.assertRaisesRegex(RuntimeError, "minimum free disk space"):
                    run_owned([sys.executable, "-c", parent], cwd=temp, timeout=10, minimum_free_bytes=10)
            time.sleep(0.8)
            self.assertFalse(marker.exists())

    def test_guarded_command_preserves_output_and_exit_status(self):
        with tempfile.TemporaryDirectory() as temp:
            result = run_owned([sys.executable, "-c", "print('fixture'); raise SystemExit(3)"],
                               cwd=temp, timeout=5, minimum_free_bytes=0)
            self.assertEqual(result.returncode, 3)
            self.assertEqual(result.stdout, "fixture\n")


if __name__ == "__main__":
    unittest.main()
