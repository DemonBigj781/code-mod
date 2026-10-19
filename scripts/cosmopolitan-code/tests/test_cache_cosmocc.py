import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import cache_cosmocc


class CompilerCacheTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.cache = self.root / "cache"
        self.payload = b"pinned compiler fixture"
        self.digest = hashlib.sha256(self.payload).hexdigest()
        self.config = self.root / "config.rs"
        self.config.write_text(
            'pub const COSMOCC_VERSION: &str = "4.0.2";\n'
            f'pub const COSMOCC_SHA256: &str =\n    "{self.digest}";\n'
            'pub const COSMOCC_BASE_URL: &str = "https://cosmo.zip/pub/cosmocc/";\n'
        )

    def download(self, command, **kwargs):
        Path(command[command.index("--output") + 1]).write_bytes(self.payload)
        return subprocess.CompletedProcess(command, 0)

    def test_official_release_fallback_preserves_sdk_hash(self):
        calls = []

        def download(command, **kwargs):
            calls.append(command)
            if len(calls) == 1:
                raise subprocess.CalledProcessError(28, command)
            return self.download(command, **kwargs)

        with patch.object(cache_cosmocc.subprocess, "run", side_effect=download):
            result = cache_cosmocc.cache_archive(self.config, self.cache)
        self.assertEqual(result.read_bytes(), self.payload)
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[1][-1], "https://github.com/jart/cosmopolitan/releases/download/4.0.2/cosmocc-4.0.2.zip")
        self.assertEqual(list(self.cache.glob("*.part")), [])

    def test_valid_cache_never_downloads(self):
        self.cache.mkdir()
        archive = self.cache / "cosmocc-4.0.2.zip"
        archive.write_bytes(self.payload)
        with patch.object(cache_cosmocc.subprocess, "run") as download:
            self.assertEqual(cache_cosmocc.cache_archive(self.config, self.cache), archive)
        download.assert_not_called()

    def test_invalid_cache_is_preserved_and_rejected(self):
        self.cache.mkdir()
        archive = self.cache / "cosmocc-4.0.2.zip"
        archive.write_bytes(b"invalid")
        with patch.object(cache_cosmocc.subprocess, "run") as download:
            with self.assertRaisesRegex(ValueError, "Existing compiler cache fails"):
                cache_cosmocc.cache_archive(self.config, self.cache)
        download.assert_not_called()
        self.assertEqual(archive.read_bytes(), b"invalid")

    def test_download_hash_mismatch_fails_closed(self):
        self.payload = b"wrong compiler"
        with patch.object(cache_cosmocc.subprocess, "run", side_effect=self.download) as download:
            with self.assertRaisesRegex(ValueError, "fails pinned SHA-256"):
                cache_cosmocc.cache_archive(self.config, self.cache)
        self.assertEqual(download.call_count, 1)
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_both_network_failures_leave_no_cache(self):
        with patch.object(cache_cosmocc.subprocess, "run", side_effect=subprocess.CalledProcessError(28, "curl")) as download:
            with self.assertRaisesRegex(RuntimeError, "both official locations"):
                cache_cosmocc.cache_archive(self.config, self.cache)
        self.assertEqual(download.call_count, 2)
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_bootstrap_caches_before_running_sdk_setup(self):
        script = (Path(__file__).resolve().parents[1] / "prepare-runtime.sh").read_text()
        self.assertLess(script.index('"$support_dir/cache_cosmocc.py"'), script.index('bash "$run_script" setup'))


if __name__ == "__main__":
    unittest.main()
