"""Publication must reject stale commits, incomplete packages and modified artifacts."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("candidate", ROOT / "scripts/build/release-candidate.py")
candidate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(candidate)
VERSION = candidate.workspace_version()


class ReleaseCandidateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = patch.dict(os.environ, {
            "GITHUB_SHA": "a" * 40, "GITHUB_RUN_ID": "100",
            "GITHUB_REPOSITORY": "millionrust/multiplex", "RELEASE_TAG": "",
            "GITHUB_REF": "refs/heads/dev",
            "GITHUB_OUTPUT": str(self.root / "output"),
            "GITHUB_STEP_SUMMARY": str(self.root / "summary"),
        })
        self.env.start()
        self.addCleanup(self.env.stop)

    def fixture(self):
        source, destination = self.root / "source", self.root / "flat"
        artifact = source / "platform"
        artifact.mkdir(parents=True)
        for name in candidate.expected_packages(VERSION):
            path = artifact / name
            path.write_bytes(b"release bytes")
            (artifact / (name + ".sha256")).write_text(f"{candidate.digest(path)}  {name}\n")
        for name in candidate.expected_sboms():
            (artifact / name).write_text("{}")
        candidate.create(source, destination)
        return source, destination

    def test_complete_candidate_verifies(self):
        _, directory = self.fixture()
        candidate.verify(directory)

    def test_other_commit_is_rejected(self):
        _, directory = self.fixture()
        with patch.dict(os.environ, GITHUB_SHA="b" * 40), self.assertRaises(ValueError):
            candidate.verify(directory)

    def test_tampering_missing_and_extra_files_are_rejected(self):
        _, directory = self.fixture()
        path = directory / "Multiplex-macos-universal.zip"
        for data in (b"changed", b""):
            path.write_bytes(data)
            with self.assertRaises(ValueError):
                candidate.verify(directory)
        path.unlink()
        with self.assertRaises(ValueError):
            candidate.verify(directory)
        (directory / "extra").write_text("unexpected")
        with self.assertRaises(ValueError):
            candidate.verify(directory)

    def test_incomplete_manifest_cannot_hide_missing_platform(self):
        _, directory = self.fixture()
        name = "Multiplex-windows-aarch64.msi"
        (directory / name).unlink()
        path = directory / candidate.MANIFEST
        manifest = json.loads(path.read_text())
        del manifest["files"][name]
        path.write_text(json.dumps(manifest))
        with self.assertRaises(ValueError):
            candidate.verify(directory)

    def test_wrong_downloadable_checksum_is_rejected_even_with_matching_manifest(self):
        _, directory = self.fixture()
        name = "Multiplex-macos-universal.zip.sha256"
        checksum = directory / name
        checksum.write_text("0" * 64 + "  Multiplex-macos-universal.zip\n")
        path = directory / candidate.MANIFEST
        manifest = json.loads(path.read_text())
        manifest["files"][name] = candidate.digest(checksum)
        path.write_text(json.dumps(manifest))
        with self.assertRaises(ValueError):
            candidate.verify(directory)

    def test_flatten_rejects_filename_collisions(self):
        source, _ = self.fixture()
        duplicate = source / "other"
        duplicate.mkdir()
        (duplicate / "Multiplex-macos-universal.zip").write_bytes(b"collision")
        with self.assertRaises(ValueError):
            candidate.create(source, self.root / "collision")

    def test_discovery_ignores_wrong_commit_forks_and_prs(self):
        def run(identifier, **kwargs):
            return dict(id=identifier, head_sha="a" * 40, event="push",
                        head_repository={"full_name": "millionrust/multiplex"}, **kwargs)

        candidates = [run(1), run(2), run(3), run(4), run(5)]
        candidates[0]["head_sha"] = "b" * 40
        candidates[1]["head_repository"]["full_name"] = "fork/multiplex"
        candidates[2]["event"] = "pull_request"

        def api(endpoint):
            if endpoint.endswith("/runs/100"):
                return {"workflow_id": 42}
            if "/workflows/42/" in endpoint:
                return {"workflow_runs": candidates}
            if "/runs/4/" in endpoint:
                return {"artifacts": [{"name": "release-candidate", "expired": True}]}
            if "/runs/5/" in endpoint:
                return {"artifacts": [{"name": "release-candidate", "expired": False}]}
            raise AssertionError(f"Untrusted run inspected: {endpoint}")

        with patch.object(candidate, "api", side_effect=api):
            candidate.discover()
        self.assertEqual((self.root / "output").read_text(), "run_id=5\n")

    def test_cache_miss_builds_instead(self):
        with patch.object(candidate, "api", side_effect=[{"workflow_id": 42}, {"workflow_runs": []}]):
            candidate.discover()
        self.assertEqual((self.root / "output").read_text(), "run_id=\n")

    def test_manual_publication_rejects_tag_at_another_commit(self):
        with patch.dict(os.environ, RELEASE_TAG="v" + VERSION), \
                patch.object(candidate.subprocess, "run"), \
                patch.object(candidate.subprocess, "check_output", return_value="b" * 40), \
                self.assertRaises(ValueError):
            candidate.discover()

    def test_tag_version_must_match_the_binaries(self):
        with patch.dict(os.environ, GITHUB_REF="refs/tags/v999.0.0"), self.assertRaises(ValueError):
            candidate.discover()

    def test_missing_sbom_is_rejected_even_with_matching_manifest(self):
        _, directory = self.fixture()
        name = "Multiplex-macos-universal.spdx.json"
        (directory / name).unlink()
        path = directory / candidate.MANIFEST
        manifest = json.loads(path.read_text())
        del manifest["files"][name]
        path.write_text(json.dumps(manifest))
        with self.assertRaises(ValueError):
            candidate.verify(directory)


if __name__ == "__main__":
    unittest.main()
