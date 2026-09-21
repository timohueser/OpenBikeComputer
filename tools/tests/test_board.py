"""Board command safety at the process boundary; no connected hardware required."""

from contextlib import redirect_stdout, redirect_stderr
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from tools import board


class BoardTests(unittest.TestCase):
    def test_programming_always_verifies_without_double_buffering(self):
        for action in ("run", "download"):
            with self.subTest(action=action):
                command = board.probe_command(action, Path("firmware.elf"), "1366:1068:123")
                self.assertIn("--verify", command)
                self.assertIn("--disable-double-buffering", command)
                self.assertIn("nRF54LM20A", command)
                self.assertIn("--non-interactive", command)
                self.assertEqual(command[command.index("--probe") + 1], "1366:1068:123")
                self.assertNotIn("--allow-erase-all", command)

    def test_preverify_reaches_programming_and_nothing_else(self):
        self.assertIn("--preverify", board.probe_command("run", Path("firmware.elf"), preverify=True))
        self.assertNotIn("--preverify", board.probe_command("run", Path("firmware.elf")))
        for action in ("attach", "reset"):
            with self.subTest(action=action), redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    board.main([action, "--preverify"])

    def test_attach_does_not_download_reset_or_retry(self):
        with tempfile.TemporaryDirectory() as tmp:
            elf = Path(tmp) / "exact firmware.elf"
            elf.write_bytes(b"firmware")
            with patch.object(board, "lock_path", return_value=Path(tmp) / "lock"), \
                    patch.object(board.subprocess, "run", return_value=subprocess.CompletedProcess([], 7)) as run, \
                    redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(board.main(["attach", str(elf)]), 7)
                run.assert_called_once()
                command = run.call_args.args[0]
                self.assertEqual(command[1], "attach")
                self.assertEqual(command[-1], str(elf.resolve()))
                self.assertNotIn("--verify", command)
                self.assertTrue(run.call_args.kwargs["pass_fds"])

    def test_failed_flash_is_not_retried_or_followed_by_reset(self):
        with tempfile.TemporaryDirectory() as tmp:
            elf = Path(tmp) / "firmware.elf"
            elf.write_bytes(b"firmware")
            with patch.object(board, "lock_path", return_value=Path(tmp) / "lock"), \
                    patch.object(board.subprocess, "run", return_value=subprocess.CompletedProcess([], 1)) as run, \
                    redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(board.main(["download", str(elf)]), 1)
                run.assert_called_once()

    def test_busy_session_names_owner_and_never_runs_probe(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "lock"
            with board.board_lock(path, ["probe-rs", "attach", "other.elf"]):
                with patch.object(board, "lock_path", return_value=path), \
                        patch.object(board.subprocess, "run") as run, redirect_stdout(io.StringIO()):
                    with self.assertRaisesRegex(RuntimeError, "Board is in use: PID .*other.elf"):
                        board.main(["reset"])
                    run.assert_not_called()
            # A stale owner record must not prevent the next session acquiring the lock.
            with board.board_lock(path, ["reset"]):
                self.assertIn("reset", path.read_text())

    def test_usb_inventory_separates_probe_and_native_device(self):
        tree = [{"IORegistryEntryChildren": [
            {"USB Product Name": "J-Link", "idVendor": 0x1366, "idProduct": 0x1068},
            {"USB Product Name": "OBC", "idVendor": 0x1209, "idProduct": 0x0001},
            {"USB Product Name": "Other", "idVendor": 0x1209, "idProduct": 0x9999},
        ]}]
        self.assertEqual([d["connection"] for d in board.usb_devices(tree)], ["J4 probe", "J3 device"])

    def test_child_retains_lock_after_wrapper_closes_its_descriptor(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "lock"
            with board.board_lock(path, ["child"]) as fd:
                child = subprocess.Popen(
                    [sys.executable, "-c", "import sys; print('ready', flush=True); sys.stdin.read()"],
                    pass_fds=(fd,), stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                )
            try:
                self.assertEqual(child.stdout.readline(), "ready\n")
                with self.assertRaisesRegex(RuntimeError, "Board is in use"):
                    with board.board_lock(path, ["competing reset"]):
                        self.fail("live child lost its lock")
            finally:
                child.stdin.close()
                child.wait(timeout=5)
                child.stdout.close()
            with board.board_lock(path, ["next session"]):
                pass


if __name__ == "__main__":
    unittest.main()
