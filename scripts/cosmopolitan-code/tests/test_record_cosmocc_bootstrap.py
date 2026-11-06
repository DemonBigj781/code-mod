import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from record_cosmocc_bootstrap import record_bootstrap, tree_sha256


class CompilerBootstrapRecordTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.sdk = self.root / "sdk"
        self.compiler = self.sdk / "vendor/cosmocc"
        for relative in (
            "bin/cosmocc",
            "bin/apelink",
            "bin/x86_64-linux-cosmo-gcc",
            "bin/x86_64-unknown-cosmo-cc",
            "x86_64-linux-cosmo/lib/libcosmo.a",
        ):
            path = self.compiler / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(relative.encode())
            if relative.startswith("bin/"):
                path.chmod(0o755)
        self.archive_payload = b"official compiler archive"
        self.archive_digest = hashlib.sha256(self.archive_payload).hexdigest()
        self.archive = self.sdk / "cache/cosmocc-4.0.2.zip"
        self.archive.parent.mkdir(parents=True)
        self.archive.write_bytes(self.archive_payload)
        self.compatibility = f"cosmocc-4.0.2 sha256:{self.archive_digest}"
        stamp = self.sdk / "vendor/.stamps/cosmocc"
        stamp.parent.mkdir(parents=True)
        stamp.write_text(self.compatibility + "\n", encoding="utf-8")
        self.config = self.root / "config.rs"
        self.config.write_text(
            'pub const COSMOCC_VERSION: &str = "4.0.2";\n'
            f'pub const COSMOCC_SHA256: &str = "{self.archive_digest}";\n'
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
            "compiler_bootstrap_archive_sha256": self.archive_digest,
            "compiler_bootstrap_installed_tree_sha256": tree_sha256(self.compiler),
            "compiler_bootstrap_verification": (
                "official-release-archive-sha256-checked"
            ),
        }
        self.write_pins()
        self.output = self.sdk / "generated/cosmocc-bootstrap.json"

    def write_pins(self):
        self.pins.write_text(json.dumps(self.pin_values), encoding="utf-8")

    def test_records_exact_official_release_identity(self):
        result = record_bootstrap(
            self.config, self.pins, self.archive, self.sdk, self.output
        )
        self.assertEqual(result, json.loads(self.output.read_text(encoding="utf-8")))
        self.assertEqual(result["kind"], "official-cosmocc-release")
        self.assertEqual(result["version"], "4.0.2")
        self.assertEqual(result["source_commit"], "a" * 40)
        self.assertFalse(result["source_worktree_dirty"])
        self.assertEqual(result["archive_sha256"], self.archive_digest)
        self.assertEqual(result["upstream_compatibility_stamp"], self.compatibility)

    def test_rejects_unpinned_installed_tree(self):
        self.pin_values["compiler_bootstrap_installed_tree_sha256"] = "0" * 64
        self.write_pins()
        with self.assertRaisesRegex(ValueError, "installed official compiler tree"):
            record_bootstrap(
                self.config, self.pins, self.archive, self.sdk, self.output
            )

    def test_rejects_incompatible_rust_ape_stamp(self):
        stamp = self.sdk / "vendor/.stamps/cosmocc"
        stamp.write_text("cosmocc-4.0.1 sha256:" + "0" * 64, encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "compatibility stamp"):
            record_bootstrap(
                self.config, self.pins, self.archive, self.sdk, self.output
            )

    def test_rejects_non_executable_compiler_driver(self):
        driver = self.compiler / "bin/cosmocc"
        driver.chmod(0o644)
        self.assertFalse(os.access(driver, os.X_OK))
        with self.assertRaisesRegex(ValueError, "not executable"):
            record_bootstrap(
                self.config, self.pins, self.archive, self.sdk, self.output
            )


if __name__ == "__main__":
    unittest.main()
