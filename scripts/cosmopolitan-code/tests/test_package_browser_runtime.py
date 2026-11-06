import importlib.util
import json
import os
import pathlib
import tempfile
import unittest
import zipfile


SCRIPT = pathlib.Path(__file__).parents[1] / "package-browser-runtime.py"
SPEC = importlib.util.spec_from_file_location("package_browser_runtime", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PackageBrowserRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temp.name) / "runtime"
        self.root.mkdir()
        for relative in MODULE.REQUIRED_PATHS:
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(f"fixture:{relative}\n".encode())
        (self.root / "camoufox/camoufox-bin").chmod(0o755)
        if os.name != "nt":
            (self.root / "lib/libLLVM.so").unlink()
            (self.root / "lib/libLLVM.so").symlink_to("libLLVM.so.22.1")
            (self.root / "lib/dri/swrast_dri.so").unlink()
            (self.root / "lib/dri/swrast_dri.so").symlink_to("libdril_dri.so")
        (self.root / "smoke-profile").mkdir()
        (self.root / "smoke-profile/cookies.sqlite").write_bytes(b"private")
        (self.root / "manifest.json").write_text("stale", encoding="utf-8")
        self.pins = {
            "browser_runtime": {
                "schema_version": 2,
                "target": "x86_64-unknown-linux-gnu",
                "archive_prefix": "browser-runtime/",
                "camoufox_version": "152.0.4",
                "camoufox_release": "beta.28",
                "camoufox_source_commit": "0" * 40,
                "camoufox_asset_url": "https://example.invalid/camoufox.zip",
                "camoufox_asset_sha256": "1" * 64,
                "camoufox_rust_revision": "2" * 40,
                "automation_protocol": "juggler-pipe",
                "automation_driver": "camoufox-juggler",
                "automation_entrypoint": "rustwright-mcp",
                "automation_transport": "mcp-stdio-json-rpc",
                "mesa_version": "26.2.4",
                "llvm_version": "22.1",
                "renderer_source_kind": "fixture",
                "renderer_source_revision": "3" * 64,
            }
        }

    def tearDown(self):
        self.temp.cleanup()

    def test_archive_is_deterministic_and_excludes_profile_state(self):
        first = pathlib.Path(self.temp.name) / "first.zip"
        second = pathlib.Path(self.temp.name) / "second.zip"
        manifest = pathlib.Path(self.temp.name) / "manifest.json"
        first_report = MODULE.write_archive(self.root, first, manifest, self.pins)
        os.utime(self.root / "camoufox/camoufox-bin", (2_000_000_000, 2_000_000_000))
        second_report = MODULE.write_archive(self.root, second, None, self.pins)

        self.assertEqual(first.read_bytes(), second.read_bytes())
        self.assertEqual(first_report["archiveSha256"], second_report["archiveSha256"])
        with zipfile.ZipFile(first) as archive:
            names = archive.namelist()
            self.assertIn("browser-runtime/manifest.json", names)
            self.assertIn("browser-runtime/camoufox/camoufox-bin", names)
            self.assertNotIn("browser-runtime/smoke-profile/cookies.sqlite", names)
            archived_manifest = json.loads(
                archive.read("browser-runtime/manifest.json").decode("utf-8")
            )
        self.assertEqual(archived_manifest, json.loads(manifest.read_text(encoding="utf-8")))
        self.assertEqual(len(archived_manifest["files"]), first_report["fileCount"])
        self.assertTrue(
            any(
                record["path"] == "camoufox/camoufox-bin"
                and record["mode"] == "0755"
                for record in archived_manifest["files"]
            )
        )
        if os.name != "nt":
            llvm_link = next(
                record
                for record in archived_manifest["files"]
                if record["path"] == "lib/libLLVM.so"
            )
            self.assertEqual(llvm_link["type"], "symlink")
            self.assertEqual(llvm_link["target"], "libLLVM.so.22.1")

    def test_symlink_escape_is_rejected(self):
        if os.name == "nt":
            self.skipTest("symlink fixture requires Unix")
        link = self.root / "lib/libLLVM.so"
        link.unlink()
        link.symlink_to("../../outside")
        with self.assertRaisesRegex(ValueError, "escapes the payload"):
            MODULE.inventory(self.root)


if __name__ == "__main__":
    unittest.main()
