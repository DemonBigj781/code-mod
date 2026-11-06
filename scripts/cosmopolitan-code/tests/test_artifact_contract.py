import unittest
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import zipfile


SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

from artifact_contract import (
    ArtifactContractError,
    validate_artifact,
    validate_development_artifact,
    write_artifact_manifest,
)

PINS = json.loads((SCRIPT_DIR / "pins.json").read_text(encoding="utf-8"))


class ArtifactContractTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp_dir.cleanup)
        self.directory = Path(self.temp_dir.name)
        self.artifact = self.directory / PINS["artifact_name"]
        self.manifest = self.directory / "manifest.json"
        self.browser_archive = self.directory / "browser-runtime.zip"
        browser_manifest = {
            "schemaVersion": PINS["browser_runtime"]["schema_version"],
            "target": PINS["browser_runtime"]["target"],
            "archivePrefix": PINS["browser_runtime"]["archive_prefix"],
            "camoufox": {
                "version": PINS["browser_runtime"]["camoufox_version"],
                "release": PINS["browser_runtime"]["camoufox_release"],
                "sourceRevision": PINS["browser_runtime"]["camoufox_source_commit"],
                "assetUrl": PINS["browser_runtime"]["camoufox_asset_url"],
                "assetSha256": PINS["browser_runtime"]["camoufox_asset_sha256"],
                "rustRevision": PINS["browser_runtime"]["camoufox_rust_revision"],
            },
            "automation": {
                "protocol": PINS["browser_runtime"]["automation_protocol"],
                "driver": PINS["browser_runtime"]["automation_driver"],
                "driverRevision": PINS["browser_runtime"]["camoufox_rust_revision"],
                "entrypoint": PINS["browser_runtime"]["automation_entrypoint"],
                "transport": PINS["browser_runtime"]["automation_transport"],
            },
            "firefox": {"version": PINS["browser_runtime"]["camoufox_version"]},
            "mesa": {
                "version": PINS["browser_runtime"]["mesa_version"],
                "sourceKind": PINS["browser_runtime"]["renderer_source_kind"],
                "sourceRevision": PINS["browser_runtime"]["renderer_source_revision"],
            },
            "llvm": {
                "version": PINS["browser_runtime"]["llvm_version"],
                "soname": "libLLVM.so.22.1",
            },
            "licenses": {},
            "treeSha256": "f" * 64,
            "files": [
                {
                    "path": "camoufox/camoufox-bin",
                    "type": "file",
                    "size": 7,
                    "sha256": hashlib.sha256(b"browser").hexdigest(),
                    "mode": "0755",
                }
            ],
        }
        with zipfile.ZipFile(self.browser_archive, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr(
                "browser-runtime/manifest.json",
                json.dumps(browser_manifest, sort_keys=True),
            )
            archive.writestr("browser-runtime/camoufox/camoufox-bin", b"browser")
        browser_bytes = self.browser_archive.read_bytes()
        self.browser_runtime = {
            "archive_sha256": hashlib.sha256(browser_bytes).hexdigest(),
            "archive_size_bytes": len(browser_bytes),
            "tree_sha256": browser_manifest["treeSha256"],
            "file_count": len(browser_manifest["files"]),
            "target": PINS["browser_runtime"]["target"],
            "camoufox": {
                "version": PINS["browser_runtime"]["camoufox_version"],
                "release": PINS["browser_runtime"]["camoufox_release"],
                "source_commit": PINS["browser_runtime"]["camoufox_source_commit"],
                "asset_url": PINS["browser_runtime"]["camoufox_asset_url"],
                "asset_sha256": PINS["browser_runtime"]["camoufox_asset_sha256"],
                "rust_revision": PINS["browser_runtime"]["camoufox_rust_revision"],
            },
            "automation": {
                "protocol": PINS["browser_runtime"]["automation_protocol"],
                "driver": PINS["browser_runtime"]["automation_driver"],
                "driver_revision": PINS["browser_runtime"]["camoufox_rust_revision"],
                "entrypoint": PINS["browser_runtime"]["automation_entrypoint"],
                "transport": PINS["browser_runtime"]["automation_transport"],
            },
            "mesa": {
                "version": PINS["browser_runtime"]["mesa_version"],
                "source_kind": PINS["browser_runtime"]["renderer_source_kind"],
                "source_revision": PINS["browser_runtime"]["renderer_source_revision"],
            },
            "llvm": {"version": PINS["browser_runtime"]["llvm_version"]},
        }
        self.payload = b"contract fixture, not an executable" + browser_bytes
        self.artifact.write_bytes(self.payload)
        self.bootstrap = {
            "schema_version": 1,
            "kind": PINS["compiler_bootstrap_kind"],
            "source_commit": PINS["compiler_bootstrap_source_commit"],
            "source_worktree_dirty": False,
            "cosmocc_tree_sha256": PINS["compiler_bootstrap_cosmocc_tree_sha256"],
            "installed_tree_sha256": PINS[
                "compiler_bootstrap_extracted_tree_sha256"
            ],
            "cosmocc_verification": "bootstrap-record-matches-tree",
            "archive_path": "/local/cosmocc-sdk.zip",
            "archive_sha256": PINS["compiler_bootstrap_archive_sha256"],
            "compatibility_archive_sha256": "e" * 64,
            "provenance_path": "/local/BUILD.json",
            "provenance_sha256": "c" * 64,
            "upstream_compatibility_stamp": "cosmocc-4.0.2 sha256:" + "d" * 64,
        }
        self.write_manifest()

    def write_manifest(self, **overrides):
        manifest = {
            "schema_version": 1,
            "source_commit": "a" * 40,
            "source_clean": True,
            "cosmopolitan_commit": PINS["cosmopolitan_commit"],
            "rust_ape_commit": PINS["rust_ape_commit"],
            "compiler_bootstrap": self.bootstrap,
            "cosmopolitan_runtime": {
                "source_built": True, "source_commit": PINS["cosmopolitan_commit"], "mode": "x86_64",
                "hashes": {name: "d" * 64 for name in (
                    "libraries_sha256", "headers_sha256", "libcosmo_sha256", "libcxx_sha256", "loader_sha256", "apelink_sha256", "tlscc_sha256",
                )},
            },
            "browser_runtime": self.browser_runtime,
            "target_architectures": PINS["target_architectures"],
            "build_profile": PINS["build_profile"],
            "release_opt_level": PINS["release_opt_level"],
            "release_lto": PINS["release_lto"],
            "release_codegen_units": PINS["release_codegen_units"],
            "artifact_name": PINS["artifact_name"],
            "artifact_size_bytes": len(self.payload),
            "artifact_sha256": hashlib.sha256(self.payload).hexdigest(),
        }
        manifest.update(overrides)
        with self.manifest.open("w", encoding="utf-8") as stream:
            json.dump(manifest, stream)

    def test_accepts_matching_pins_and_artifact_hash(self):
        result = validate_artifact(self.artifact, self.manifest)
        self.assertEqual(result["target_architectures"], PINS["target_architectures"])
        validate_artifact(self.artifact, self.manifest, expected_source_commit="a" * 40)

    def test_rejects_source_revision_mismatch(self):
        with self.assertRaisesRegex(ArtifactContractError, "source_commit does not match"):
            validate_artifact(self.artifact, self.manifest, expected_source_commit="b" * 40)

    def test_rejects_optimization_level_mismatch(self):
        pins = dict(PINS, release_opt_level=2)
        pins_path = self.directory / "pins.json"
        pins_path.write_text(json.dumps(pins), encoding="utf-8")
        self.write_manifest(release_opt_level=3)
        with self.assertRaisesRegex(ArtifactContractError, "release_opt_level"):
            validate_artifact(self.artifact, self.manifest, pins_path=pins_path)

    def test_rejects_missing_artifact(self):
        with self.assertRaisesRegex(ArtifactContractError, "artifact is missing"):
            validate_artifact(self.directory / "missing.com", self.manifest)

    def test_rejects_prebuilt_or_unidentified_cosmopolitan_runtime(self):
        for runtime in (None, {}, {"source_built": False}, {
            "source_built": True, "source_commit": "f" * 40, "mode": "x86_64",
        }, {
            "source_built": True, "source_commit": PINS["cosmopolitan_commit"], "mode": "x86_64", "hashes": {},
        }):
            with self.subTest(runtime=runtime):
                self.write_manifest(cosmopolitan_runtime=runtime)
                with self.assertRaisesRegex(ArtifactContractError, "cosmopolitan_runtime"):
                    validate_artifact(self.artifact, self.manifest)

    def test_rejects_missing_or_unpinned_compiler_bootstrap(self):
        for bootstrap in (
            None,
            {},
            dict(self.bootstrap, kind="official-release"),
            dict(self.bootstrap, source_commit="f" * 40),
            dict(self.bootstrap, cosmocc_tree_sha256="0" * 64),
        ):
            with self.subTest(bootstrap=bootstrap):
                self.write_manifest(compiler_bootstrap=bootstrap)
                with self.assertRaisesRegex(ArtifactContractError, "compiler_bootstrap"):
                    validate_artifact(self.artifact, self.manifest)

    def test_release_rejects_dirty_or_unverified_local_compiler(self):
        for bootstrap in (
            dict(self.bootstrap, source_worktree_dirty=True),
            dict(self.bootstrap, cosmocc_verification="unverified-local-input"),
        ):
            with self.subTest(bootstrap=bootstrap):
                self.write_manifest(compiler_bootstrap=bootstrap)
                with self.assertRaisesRegex(ArtifactContractError, "release compiler bootstrap"):
                    validate_artifact(self.artifact, self.manifest)

    def test_rejects_missing_or_malformed_manifest(self):
        with self.assertRaisesRegex(ArtifactContractError, "cannot read manifest"):
            validate_artifact(self.artifact, self.directory / "missing.json")
        with self.manifest.open("w", encoding="utf-8") as stream:
            stream.write("{")
        with self.assertRaisesRegex(ArtifactContractError, "cannot read manifest"):
            validate_artifact(self.artifact, self.manifest)

    def test_rejects_empty_artifact(self):
        self.artifact.write_bytes(b"")
        with self.assertRaisesRegex(ArtifactContractError, "artifact is empty"):
            validate_artifact(self.artifact, self.manifest)

    def test_rejects_tampered_hash_pins_size_architecture_or_source_state(self):
        for field, value in (
            ("artifact_sha256", "0" * 64),
            ("cosmopolitan_commit", "b" * 40),
            ("rust_ape_commit", "c" * 40),
            ("target_architectures", ["x86_64", "aarch64"]),
            ("artifact_size_bytes", len(self.payload) + 1),
            ("source_clean", False),
        ):
            with self.subTest(field=field):
                self.write_manifest(**{field: value})
                with self.assertRaises(ArtifactContractError):
                    validate_artifact(self.artifact, self.manifest)

    def test_rejects_tampered_embedded_browser_runtime(self):
        for browser_runtime in (
            None,
            {},
            dict(self.browser_runtime, archive_sha256="0" * 64),
            dict(self.browser_runtime, archive_size_bytes=self.browser_runtime["archive_size_bytes"] - 1),
            dict(self.browser_runtime, tree_sha256="0" * 64),
            dict(self.browser_runtime, target="aarch64-unknown-linux-gnu"),
            dict(
                self.browser_runtime,
                camoufox=dict(self.browser_runtime["camoufox"], version="0.0.0"),
            ),
        ):
            with self.subTest(browser_runtime=browser_runtime):
                self.write_manifest(browser_runtime=browser_runtime)
                with self.assertRaisesRegex(ArtifactContractError, "browser_runtime"):
                    validate_artifact(self.artifact, self.manifest)

    def test_rejects_browser_archive_that_is_not_the_artifact_suffix(self):
        self.artifact.write_bytes(self.payload + b"trailing bytes")
        changed = self.artifact.read_bytes()
        self.write_manifest(
            artifact_size_bytes=len(changed),
            artifact_sha256=hashlib.sha256(changed).hexdigest(),
        )

        with self.assertRaisesRegex(ArtifactContractError, "browser_runtime"):
            validate_artifact(self.artifact, self.manifest)

    def test_accepts_identified_working_tree_artifact_for_local_development(self):
        development_bootstrap = dict(
            self.bootstrap,
            source_worktree_dirty=True,
            cosmocc_verification="unverified-local-input",
        )
        self.write_manifest(
            source_clean=False,
            development_build=True,
            source_snapshot_sha256="e" * 64,
            compiler_bootstrap=development_bootstrap,
        )

        result = validate_development_artifact(self.artifact, self.manifest)

        self.assertFalse(result["source_clean"])
        self.assertTrue(result["development_build"])

    def test_development_contract_rejects_release_or_unidentified_source(self):
        for overrides in (
            {},
            {"source_clean": False, "development_build": True},
            {
                "source_clean": False,
                "development_build": True,
                "source_snapshot_sha256": "not-a-digest",
            },
        ):
            with self.subTest(overrides=overrides):
                self.write_manifest(**overrides)
                with self.assertRaises(ArtifactContractError):
                    validate_development_artifact(self.artifact, self.manifest)

    def test_manifest_writer_identifies_working_tree_snapshot(self):
        existing = json.loads(self.manifest.read_text(encoding="utf-8"))
        runtime = self.directory / "runtime.json"
        runtime.write_text(json.dumps(existing["cosmopolitan_runtime"]), encoding="utf-8")
        bootstrap = self.directory / "bootstrap.json"
        bootstrap.write_text(json.dumps(self.bootstrap), encoding="utf-8")

        write_artifact_manifest(
            self.artifact,
            self.manifest,
            runtime,
            bootstrap,
            "a" * 40,
            browser_runtime_path=self.browser_archive,
            source_clean=False,
            source_snapshot_sha256="e" * 64,
        )

        result = validate_development_artifact(
            self.artifact,
            self.manifest,
            expected_source_commit="a" * 40,
        )
        self.assertTrue(result["development_build"])
        self.assertEqual(result["source_snapshot_sha256"], "e" * 64)
        self.assertEqual(
            result["compiler_bootstrap"]["compatibility_archive_sha256"],
            "e" * 64,
        )
        self.assertEqual(
            result["browser_runtime"]["archive_sha256"],
            hashlib.sha256(self.browser_archive.read_bytes()).hexdigest(),
        )
        self.assertEqual(result["browser_runtime"]["tree_sha256"], "f" * 64)

    def test_manifest_writer_rejects_unidentified_working_tree(self):
        existing = json.loads(self.manifest.read_text(encoding="utf-8"))
        runtime = self.directory / "runtime.json"
        runtime.write_text(json.dumps(existing["cosmopolitan_runtime"]), encoding="utf-8")
        bootstrap = self.directory / "bootstrap.json"
        bootstrap.write_text(json.dumps(self.bootstrap), encoding="utf-8")

        with self.assertRaisesRegex(ArtifactContractError, "snapshot"):
            write_artifact_manifest(
                self.artifact,
                self.manifest,
                runtime,
                bootstrap,
                "a" * 40,
                browser_runtime_path=self.browser_archive,
                source_clean=False,
            )


if __name__ == "__main__":
    unittest.main()
