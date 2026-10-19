from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import check_fork_shims


class ForkShimTests(unittest.TestCase):
    def test_collects_all_compile_failures_without_masking_them(self):
        with tempfile.TemporaryDirectory() as temporary:
            sdk = Path(temporary)
            (sdk / "shim").mkdir()
            for name in ("a.c", "b.c"):
                (sdk / "shim" / name).write_text("// cflags: -D_COSMO_SOURCE\nint sample;\n")
            with patch.object(check_fork_shims.subprocess, "run", return_value=subprocess.CompletedProcess([], 1)) as compiler:
                with self.assertRaisesRegex(RuntimeError, "shim/a.c, shim/b.c"):
                    check_fork_shims.check_shims(sdk)
            self.assertEqual(compiler.call_count, 2)
            for call in compiler.call_args_list:
                self.assertIn("-D_COSMO_SOURCE", call.args[0])
                self.assertIn("-DSYSDEBUG=1", call.args[0])

    def test_checks_shims_before_compiling_code(self):
        script = (Path(__file__).resolve().parents[2] / "build-cosmopolitan-code.sh").read_text()
        self.assertLess(script.index('"$support_dir/check_fork_shims.py"'), script.index('bash "$run_script" cargo'))


if __name__ == "__main__":
    unittest.main()
