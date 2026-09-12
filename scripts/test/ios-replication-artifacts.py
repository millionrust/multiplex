"""Metadata/publication unit tests; mocked binaries are not native runtime proof."""
import importlib.util
import json
from pathlib import Path
import plistlib
import sys
import tempfile
import unittest
from unittest.mock import patch

# The builder under test and its process helpers live in scripts/build.
BUILD = Path(__file__).resolve().parents[1] / "build"
sys.path.insert(0, str(BUILD))
spec = importlib.util.spec_from_file_location("ios_artifacts", BUILD / "ios-replication-artifacts.py")
artifacts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(artifacts)


class ArtifactTests(unittest.TestCase):
    def fixture(self, root):
        framework = root / f"{artifacts.NAME}.xcframework"
        framework.mkdir(parents=True)
        entries = [
            {"LibraryIdentifier": "device", "LibraryPath": "ffi.framework", "SupportedPlatform": "ios", "SupportedArchitectures": ["arm64"]},
            {"LibraryIdentifier": "simulator", "LibraryPath": "ffi.framework", "SupportedPlatform": "ios", "SupportedArchitectures": ["arm64", "x86_64"], "SupportedPlatformVariant": "simulator"},
        ]
        (framework / "Info.plist").write_bytes(plistlib.dumps({"AvailableLibraries": entries}))
        (root / "Sources").mkdir()
        (root / "Sources" / f"{artifacts.NAME}.swift").write_text("// test fixture\n")
        self.seal(root)
        return framework

    def seal(self, root):
        (root / "artifacts.json").write_text(json.dumps(artifacts.shared.inventory(root)))

    @staticmethod
    def command(*args, **kwargs):
        if args[0] == "lipo":
            return "arm64 x86_64" if "/simulator/" in args[-1] else "arm64"
        custody = [f"uniffi_{artifacts.STEM}_fn_method_replicationcustody_{op}" for op in (
            "create_device_identity", "device_public_key", "delete_device_identity")]
        product = [f"uniffi_{artifacts.STEM}_fn_method_mobilereplicationproduct_{op}" for op in (
            "prepare_enrollment", "pending_enrollment", "cancel_pending_enrollment")]
        return "\n".join(custody + product)

    def test_complete_set_verifies_with_mocked_symbol_inspection(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(artifacts, "symbol_tool", return_value="llvm-nm"), patch.object(artifacts, "run", side_effect=self.command):
            self.fixture(Path(temp))
            artifacts.verify(Path(temp))

    def test_duplicate_device_slices_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            framework = self.fixture(root)
            info = plistlib.loads((framework / "Info.plist").read_bytes())
            info["AvailableLibraries"][1] = dict(info["AvailableLibraries"][0])
            (framework / "Info.plist").write_bytes(plistlib.dumps(info))
            self.seal(root)
            with self.assertRaises(RuntimeError):
                artifacts.verify(root)

    def test_tamper_cannot_replace_packaged_set(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source, dest = root / "source", root / "dest"
            self.fixture(source)
            dest.mkdir()
            (dest / "sentinel").write_text("existing")
            (source / "Sources" / f"{artifacts.NAME}.swift").write_text("tampered")
            with self.assertRaises(RuntimeError):
                artifacts.shared.promote(source, dest, artifacts.verify)
            self.assertEqual((dest / "sentinel").read_text(), "existing")


if __name__ == "__main__":
    unittest.main()
