from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import build_fork_runtime as runtime


class ForkRuntimeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / "source"
        self.sdk = self.root / "sdk"
        self.compiler = self.sdk / "vendor/cosmocc"
        self.commit = "a" * 40
        for name in runtime.LIBRARIES:
            self.write(self.source / "o/x86_64" / name, name.encode())
        for name in runtime.TOOLS:
            self.write(self.source / "o/x86_64/tool/build" / name, name.encode())
        self.write(self.source / "o/x86_64/ape/ape.elf", b"source-built loader")
        self.write(self.source / "libc/isystem/stdio.h", b"source stdio")
        self.write(self.source / "libc/integral/normalize.inc", b"source normalize")
        self.write(self.source / "libc/calls/example.h", b"source internal header")
        self.write(self.source / "o/cosmocc.h.txt", b"libc/calls/example.h")
        self.write(self.source / "tool/cosmocc/bin/cosmocross", b"source wrapper")
        self.write(self.compiler / "include/stale-release.h", b"release")
        self.write(self.compiler / "x86_64-linux-cosmo/lib/tiny/libcosmo.a", b"release")
        self.write(self.compiler / "bin/gcc", b"bootstrap compiler")
        (self.sdk / "generated").mkdir()

    @staticmethod
    def write(path, payload):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)

    def install(self):
        with patch.object(runtime.subprocess, "check_output", side_effect=[self.commit + "\n", ""]), \
             patch.object(runtime.subprocess, "run") as make:
            result = runtime.build_runtime(self.source, self.sdk, self.commit, 1)
        self.assertIn("-j1", make.call_args.args[0])
        self.assertIn("toolchain", make.call_args.args[0])
        return result

    def test_installs_source_runtime_headers_and_tools_not_release_outputs(self):
        record = self.install()
        self.assertTrue(record["source_built"])
        self.assertEqual(record["source_commit"], self.commit)
        self.assertEqual(record["hashes"], runtime.runtime_hashes(self.compiler))
        self.assertEqual((self.compiler / "x86_64-linux-cosmo/lib/libcosmo.a").read_bytes(), b"cosmopolitan.a")
        self.assertEqual((self.compiler / "bin/ape-x86_64.elf").read_bytes(), b"source-built loader")
        self.assertEqual((self.compiler / "bin/ar.ape").read_bytes(), b"ar")
        self.assertEqual((self.compiler / "include/libc/calls/example.h").read_bytes(), b"source internal header")
        self.assertFalse((self.compiler / "include/stale-release.h").exists())
        self.assertFalse((self.compiler / "x86_64-linux-cosmo/lib/tiny").exists())
        self.assertEqual((self.compiler / "include-bootstrap/stale-release.h").read_bytes(), b"release")
        self.assertEqual((self.compiler / "bin/gcc").read_bytes(), b"bootstrap compiler")

    def test_recorded_runtime_is_rechecked_before_reuse(self):
        self.install()
        with patch.object(runtime.subprocess, "run") as make:
            runtime.build_runtime(self.source, self.sdk, self.commit, 1)
        make.assert_not_called()
        (self.compiler / "include/stdio.h").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "differs from its record"):
            runtime.build_runtime(self.source, self.sdk, self.commit, 1)

    def test_incomplete_source_outputs_leave_bootstrap_untouched(self):
        (self.source / "o/x86_64/cosmopolitan.a").unlink()
        with self.assertRaises(FileNotFoundError):
            self.install()
        self.assertTrue((self.compiler / "include/stale-release.h").exists())
        self.assertFalse((self.sdk / "generated/fork-runtime.json").exists())

    def test_changed_or_dirty_source_cannot_build(self):
        for commit, dirty in (("b" * 40, ""), (self.commit, " M Makefile")):
            with patch.object(runtime.subprocess, "check_output", side_effect=[commit, dirty]), \
                 patch.object(runtime.subprocess, "run") as make:
                with self.assertRaisesRegex(ValueError, "clean and match"):
                    runtime.build_runtime(self.source, self.sdk, self.commit, 1)
            make.assert_not_called()

    def test_local_parallel_compilation_is_rejected(self):
        with patch.dict(runtime.os.environ, {"GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "self-hosted"}):
            with self.assertRaisesRegex(ValueError, "only on GitHub-hosted"):
                runtime.build_runtime(self.source, self.sdk, self.commit, 2)

    def test_fork_is_built_before_code_compilation(self):
        script = (Path(__file__).resolve().parents[2] / "build-cosmopolitan-code.sh").read_text()
        self.assertLess(script.index('"$support_dir/build_fork_runtime.py"'), script.index('bash "$run_script" cargo'))
        self.assertIn('"cosmopolitan_runtime":', script)


if __name__ == "__main__":
    unittest.main()
