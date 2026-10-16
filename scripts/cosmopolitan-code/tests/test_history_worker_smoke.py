import errno
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock, patch


spec = importlib.util.spec_from_file_location(
    "history_worker_smoke", Path(__file__).resolve().parents[1] / "check-history-worker.py"
)
assert spec is not None and spec.loader is not None
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class HistoryWorkerSmokeTests(unittest.TestCase):
    def test_deadline_check_uses_cosmopolitan_windows_wait_status(self):
        artifact = Path("code.com")
        process = Mock()
        process.wait.return_value = 124 << 8
        process.poll.return_value = 124 << 8
        with patch.object(smoke.os, "name", "nt"), patch.object(
            smoke.subprocess, "Popen", return_value=process
        ) as launch:
            smoke.check_worker_deadline(artifact, {})
        launch.assert_called_once()
        process.kill.assert_not_called()
        process.communicate.assert_called_once_with(timeout=5)

    def test_deadline_check_rejects_unencoded_exit_on_windows(self):
        artifact = Path("code.com")
        process = Mock()
        process.wait.return_value = 124
        process.poll.return_value = 124
        with patch.object(smoke.os, "name", "nt"), patch.object(
            smoke.subprocess, "Popen", return_value=process
        ):
            with self.assertRaisesRegex(AssertionError, "expected 31744, got 124"):
                smoke.check_worker_deadline(artifact, {})
        process.communicate.assert_called_once_with(timeout=5)

    def test_deadline_check_waits_with_stdin_open(self):
        process = Mock()
        process.wait.return_value = 124
        process.poll.return_value = 124
        with patch.object(smoke.subprocess, "Popen", return_value=process) as launch:
            smoke.check_worker_deadline(Path("code.com"), {})
        launch.assert_called_once()
        self.assertEqual(launch.call_args.kwargs["stdin"], subprocess.PIPE)
        process.wait.assert_called_once_with(timeout=40)
        process.kill.assert_not_called()
        process.communicate.assert_called_once_with(timeout=5)

    def test_deadline_check_reaps_a_worker_that_outlives_its_limit(self):
        process = Mock()
        process.wait.side_effect = subprocess.TimeoutExpired("code.com", 40)
        process.poll.return_value = None
        with patch.object(smoke.subprocess, "Popen", return_value=process):
            with self.assertRaises(subprocess.TimeoutExpired):
                smoke.check_worker_deadline(Path("code.com"), {})
        process.kill.assert_called_once()
        process.communicate.assert_called_once_with(timeout=5)

    def test_deadline_check_uses_shell_only_for_posix_exec_format_error(self):
        process = Mock()
        process.wait.return_value = 124
        process.poll.return_value = 124
        artifact = Path("code.com")
        with patch.object(smoke.os, "name", "posix"), patch.object(
            smoke.subprocess, "Popen", side_effect=[OSError(errno.ENOEXEC, "format"), process]
        ) as launch:
            smoke.check_worker_deadline(artifact, {})
        self.assertEqual(launch.call_args.args[0], ["sh", str(artifact), "--code-session-history-worker"])

    def test_deadline_check_rejects_early_worker_exit(self):
        process = Mock()
        process.wait.return_value = 0
        process.poll.return_value = 0
        with patch.object(smoke.subprocess, "Popen", return_value=process):
            with self.assertRaisesRegex(AssertionError, "own deadline"):
                smoke.check_worker_deadline(Path("code.com"), {})
        process.communicate.assert_called_once_with(timeout=5)


if __name__ == "__main__":
    unittest.main()
