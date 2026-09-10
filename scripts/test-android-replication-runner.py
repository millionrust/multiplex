"""Exercise runner resource guards without building or touching any Android device."""
import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch


SPEC = importlib.util.spec_from_file_location(
    "replication_runner", Path(__file__).with_name("test-android-replication-custody.py")
)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class ResourceGuards(unittest.TestCase):
    def test_low_space_refuses_to_start_emulator(self):
        with patch("sys.argv", ["runner", "--avd", "fixture"]), \
                patch.object(runner.shutil, "disk_usage", return_value=SimpleNamespace(free=16 * 1024**3)), \
                patch.object(runner, "command") as command, \
                patch.object(runner.subprocess, "Popen") as spawn:
            with self.assertRaisesRegex(RuntimeError, "At least 17 GiB"):
                runner.main()
            command.assert_not_called()
            spawn.assert_not_called()

    def test_space_loss_during_boot_stops_only_owned_emulator(self):
        owned = MagicMock()
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            if args[-1] == "-list-avds":
                return SimpleNamespace(stdout="fixture\n")
            if args[-1] == "devices":
                return SimpleNamespace(stdout="List of devices attached\nemulator-5554\tdevice\n")
            if args[-2:] == ("emu", "kill"):
                return SimpleNamespace(stdout="OK\n")
            self.fail(f"Unexpected device operation: {args}")

        with patch("sys.argv", ["runner", "--avd", "fixture"]), \
                patch.object(runner.shutil, "disk_usage", side_effect=[
                    SimpleNamespace(free=18 * 1024**3), SimpleNamespace(free=15 * 1024**3)
                ]), \
                patch.object(runner, "command", side_effect=command), \
                patch.object(runner.socket, "socket"), \
                patch.object(runner.subprocess, "Popen", return_value=owned) as spawn:
            with self.assertRaisesRegex(RuntimeError, "Emulator startup stopped"):
                runner.main()
            spawn.assert_called_once()
            self.assertIn("-read-only", spawn.call_args.args[0])
            self.assertIn("5560", spawn.call_args.args[0])
            owned.wait.assert_called_once_with(timeout=20)
            self.assertEqual([call[1:] for call in calls if "emu" in call], [
                ("-s", "emulator-5560", "emu", "kill")
            ])
            self.assertFalse(any("emulator-5554" in call for call in calls))

    def test_picker_requires_enrollment_mode_before_device_access(self):
        with patch("sys.argv", ["runner", "--avd", "fixture", "--system-picker"]), \
                patch.object(runner, "command") as command, \
                patch.object(runner.subprocess, "Popen") as spawn:
            with self.assertRaises(SystemExit) as error:
                runner.main()
            self.assertEqual(error.exception.code, 2)
            command.assert_not_called()
            spawn.assert_not_called()


if __name__ == "__main__":
    unittest.main()
