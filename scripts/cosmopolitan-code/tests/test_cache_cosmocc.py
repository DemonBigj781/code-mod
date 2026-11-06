import hashlib
import json
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
            'pub const COSMOCC_BASE_URL: &str = "https://cosmo.zip/pub/cosmocc/";\n',
            encoding="utf-8",
        )
        self.pins = self.root / "pins.json"
        self.pin_values = {
            "compiler_bootstrap_kind": "official-cosmocc-release",
            "compiler_bootstrap_version": "4.0.2",
            "compiler_bootstrap_source_commit": "a" * 40,
            "compiler_bootstrap_archive_url": (
                "https://cosmo.zip/pub/cosmocc/cosmocc-4.0.2.zip"
            ),
            "compiler_bootstrap_archive_sha256": self.digest,
        }
        self.write_pins()

    def write_pins(self):
        self.pins.write_text(json.dumps(self.pin_values), encoding="utf-8")

    def download(self, command, **kwargs):
        Path(command[command.index("--output") + 1]).write_bytes(self.payload)
        return subprocess.CompletedProcess(command, 0)

    def test_primary_official_release_is_verified(self):
        with patch.object(
            cache_cosmocc.subprocess, "run", side_effect=self.download
        ) as download:
            result = cache_cosmocc.cache_archive(
                self.config, self.pins, self.cache
            )
        self.assertEqual(result.read_bytes(), self.payload)
        self.assertEqual(download.call_count, 1)
        self.assertEqual(
            download.call_args.args[0][-1],
            "https://cosmo.zip/pub/cosmocc/cosmocc-4.0.2.zip",
        )

    def test_official_github_fallback_preserves_sdk_hash(self):
        calls = []

        def download(command, **kwargs):
            calls.append(command)
            if len(calls) == 1:
                raise subprocess.CalledProcessError(28, command)
            return self.download(command, **kwargs)

        with patch.object(cache_cosmocc.subprocess, "run", side_effect=download):
            result = cache_cosmocc.cache_archive(
                self.config, self.pins, self.cache
            )
        self.assertEqual(result.read_bytes(), self.payload)
        self.assertEqual(len(calls), 2)
        self.assertEqual(
            calls[1][-1],
            "https://github.com/jart/cosmopolitan/releases/download/4.0.2/cosmocc-4.0.2.zip",
        )
        self.assertEqual(list(self.cache.glob("*.part")), [])

    def test_valid_cache_never_downloads(self):
        self.cache.mkdir()
        archive = self.cache / "cosmocc-4.0.2.zip"
        archive.write_bytes(self.payload)
        with patch.object(cache_cosmocc.subprocess, "run") as download:
            self.assertEqual(
                cache_cosmocc.cache_archive(self.config, self.pins, self.cache),
                archive,
            )
        download.assert_not_called()

    def test_tampered_cache_is_preserved_and_rejected(self):
        self.cache.mkdir()
        archive = self.cache / "cosmocc-4.0.2.zip"
        archive.write_bytes(b"invalid")
        with patch.object(cache_cosmocc.subprocess, "run") as download:
            with self.assertRaisesRegex(ValueError, "Existing compiler cache fails"):
                cache_cosmocc.cache_archive(self.config, self.pins, self.cache)
        download.assert_not_called()
        self.assertEqual(archive.read_bytes(), b"invalid")

    def test_download_hash_mismatch_fails_closed(self):
        self.payload = b"wrong compiler"
        with patch.object(
            cache_cosmocc.subprocess, "run", side_effect=self.download
        ) as download:
            with self.assertRaisesRegex(ValueError, "fails pinned SHA-256"):
                cache_cosmocc.cache_archive(self.config, self.pins, self.cache)
        self.assertEqual(download.call_count, 1)
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_config_and_code_pins_must_match(self):
        original = dict(self.pin_values)
        mismatches = {
            "compiler_bootstrap_kind": "local-cosmopolitan-sdk",
            "compiler_bootstrap_version": "4.0.1",
            "compiler_bootstrap_archive_url": "https://example.com/cosmocc.zip",
            "compiler_bootstrap_archive_sha256": "0" * 64,
        }
        for field, value in mismatches.items():
            with self.subTest(field=field):
                self.pin_values = dict(original)
                self.pin_values[field] = value
                self.write_pins()
                with self.assertRaisesRegex(ValueError, field):
                    cache_cosmocc.read_release(self.config, self.pins)
        self.pin_values = original
        self.write_pins()

    def test_bootstrap_caches_before_setup_and_records_afterward(self):
        script = (
            Path(__file__).resolve().parents[1] / "prepare-runtime.sh"
        ).read_text(encoding="utf-8")
        self.assertLess(
            script.index('"$support_dir/cache_cosmocc.py"'),
            script.index('bash "$run_script" setup'),
        )
        self.assertLess(
            script.index('bash "$run_script" setup'),
            script.index('"$support_dir/record_cosmocc_bootstrap.py"'),
        )
        self.assertIn("sdk/generated/cosmocc-bootstrap.json", script)


if __name__ == "__main__":
    unittest.main()
