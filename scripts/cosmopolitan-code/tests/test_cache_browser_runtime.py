import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import cache_browser_runtime


class BrowserRuntimeCacheTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.cache = self.root / "cache"
        self.source = self.root / "source.zip"
        self.pins = self.root / "pins.json"

        browser = b"browser"
        tree_sha256 = "f" * 64
        runtime = {
            "schema_version": 2,
            "target": "x86_64-unknown-linux-gnu",
            "archive_prefix": "browser-runtime/",
            "camoufox_version": "152.0.4",
            "camoufox_release": "beta.28",
            "camoufox_source_commit": "a" * 40,
            "camoufox_asset_url": "https://example.com/camoufox.zip",
            "camoufox_asset_sha256": "b" * 64,
            "camoufox_rust_revision": "c" * 40,
            "automation_protocol": "juggler-pipe",
            "automation_driver": "camoufox-juggler",
            "automation_entrypoint": "rustwright-mcp",
            "automation_transport": "mcp-stdio-json-rpc",
            "mesa_version": "26.2.4",
            "llvm_version": "22.1",
            "renderer_source_kind": "fixture",
            "renderer_source_revision": "d" * 64,
        }
        manifest = {
            "schemaVersion": runtime["schema_version"],
            "target": runtime["target"],
            "archivePrefix": runtime["archive_prefix"],
            "camoufox": {
                "version": runtime["camoufox_version"],
                "release": runtime["camoufox_release"],
                "sourceRevision": runtime["camoufox_source_commit"],
                "assetUrl": runtime["camoufox_asset_url"],
                "assetSha256": runtime["camoufox_asset_sha256"],
                "rustRevision": runtime["camoufox_rust_revision"],
            },
            "automation": {
                "protocol": runtime["automation_protocol"],
                "driver": runtime["automation_driver"],
                "driverRevision": runtime["camoufox_rust_revision"],
                "entrypoint": runtime["automation_entrypoint"],
                "transport": runtime["automation_transport"],
            },
            "firefox": {"version": runtime["camoufox_version"]},
            "mesa": {
                "version": runtime["mesa_version"],
                "sourceKind": runtime["renderer_source_kind"],
                "sourceRevision": runtime["renderer_source_revision"],
            },
            "llvm": {"version": runtime["llvm_version"], "soname": "libLLVM.so.22.1"},
            "licenses": {},
            "treeSha256": tree_sha256,
            "files": [
                {
                    "path": "camoufox/camoufox-bin",
                    "type": "file",
                    "size": len(browser),
                    "sha256": hashlib.sha256(browser).hexdigest(),
                    "mode": "0755",
                }
            ],
        }
        with zipfile.ZipFile(self.source, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("browser-runtime/manifest.json", json.dumps(manifest))
            archive.writestr("browser-runtime/camoufox/camoufox-bin", browser)

        self.payload = self.source.read_bytes()
        runtime.update(
            {
                "archive_url": (
                    "https://github.com/example/project/releases/download/"
                    "cosmopolitan-browser-runtime-v1/browser-runtime-x86_64.zip"
                ),
                "archive_sha256": hashlib.sha256(self.payload).hexdigest(),
                "archive_size_bytes": len(self.payload),
                "archive_file_count": 1,
                "archive_tree_sha256": tree_sha256,
            }
        )
        self.pin_values = {"browser_runtime": runtime}
        self.write_pins()

    def write_pins(self):
        self.pins.write_text(json.dumps(self.pin_values), encoding="utf-8")

    def download(self, command, **kwargs):
        Path(command[command.index("--output") + 1]).write_bytes(self.payload)
        return subprocess.CompletedProcess(command, 0)

    def test_pinned_release_is_downloaded_and_verified(self):
        with patch.object(
            cache_browser_runtime.subprocess, "run", side_effect=self.download
        ) as download:
            result = cache_browser_runtime.cache_archive(self.pins, self.cache)

        self.assertEqual(result.read_bytes(), self.payload)
        command = download.call_args.args[0]
        self.assertIn("=https", command)
        self.assertEqual(command[-1], self.pin_values["browser_runtime"]["archive_url"])

    def test_valid_cache_never_downloads(self):
        self.cache.mkdir()
        archive = self.cache / "browser-runtime_x86_64.zip"
        archive.write_bytes(self.payload)

        with patch.object(cache_browser_runtime.subprocess, "run") as download:
            self.assertEqual(
                cache_browser_runtime.cache_archive(self.pins, self.cache), archive
            )

        download.assert_not_called()

    def test_tampered_cache_is_preserved_and_rejected(self):
        self.cache.mkdir()
        archive = self.cache / "browser-runtime_x86_64.zip"
        archive.write_bytes(b"invalid")

        with patch.object(cache_browser_runtime.subprocess, "run") as download:
            with self.assertRaisesRegex(ValueError, "Existing browser runtime cache fails"):
                cache_browser_runtime.cache_archive(self.pins, self.cache)

        download.assert_not_called()
        self.assertEqual(archive.read_bytes(), b"invalid")

    def test_download_hash_mismatch_fails_closed(self):
        self.payload = b"wrong runtime"
        with patch.object(
            cache_browser_runtime.subprocess, "run", side_effect=self.download
        ):
            with self.assertRaisesRegex(ValueError, "fails the pinned runtime contract"):
                cache_browser_runtime.cache_archive(self.pins, self.cache)

        self.assertEqual(list(self.cache.glob("*.part")), [])
        self.assertEqual(list(self.cache.glob("*.zip")), [])

    def test_archive_identity_must_match_every_distribution_pin(self):
        runtime = self.pin_values["browser_runtime"]
        original = dict(runtime)
        mismatches = {
            "archive_sha256": "0" * 64,
            "archive_size_bytes": len(self.payload) + 1,
            "archive_file_count": 2,
            "archive_tree_sha256": "0" * 64,
        }
        for field, value in mismatches.items():
            with self.subTest(field=field):
                runtime.clear()
                runtime.update(original)
                runtime[field] = value
                self.write_pins()
                distribution = cache_browser_runtime.read_distribution(self.pins)
                with self.assertRaisesRegex(ValueError, "does not match the pin"):
                    cache_browser_runtime.verify_archive(self.source, distribution)
        runtime.clear()
        runtime.update(original)
        self.write_pins()

    def test_distribution_pin_rejects_non_https_urls(self):
        self.pin_values["browser_runtime"]["archive_url"] = "http://example.com/runtime.zip"
        self.write_pins()

        with self.assertRaisesRegex(ValueError, "HTTPS"):
            cache_browser_runtime.read_distribution(self.pins)

    def test_build_fetches_runtime_before_expensive_cargo_work(self):
        script = (
            Path(__file__).resolve().parents[2] / "build-cosmopolitan-code.sh"
        ).read_text(encoding="utf-8")
        self.assertIn('"$support_dir/cache_browser_runtime.py"', script)
        self.assertLess(
            script.index('"$support_dir/cache_browser_runtime.py"'),
            script.index('bash "$run_script" cargo'),
        )


if __name__ == "__main__":
    unittest.main()
