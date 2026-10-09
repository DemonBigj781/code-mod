import unittest
import hashlib
import json
from pathlib import Path
import sys
import tempfile


SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

from artifact_contract import ArtifactContractError, validate_artifact

PINS = json.loads((SCRIPT_DIR / "pins.json").read_text(encoding="utf-8"))


class ArtifactContractTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp_dir.cleanup)
        self.directory = Path(self.temp_dir.name)
        self.artifact = self.directory / PINS["artifact_name"]
        self.manifest = self.directory / "manifest.json"
        self.payload = b"contract fixture, not an executable"
        self.artifact.write_bytes(self.payload)
        self.write_manifest()

    def write_manifest(self, **overrides):
        manifest = {
            "schema_version": 1,
            "source_commit": "a" * 40,
            "source_clean": True,
            "cosmopolitan_commit": PINS["cosmopolitan_commit"],
            "rust_ape_commit": PINS["rust_ape_commit"],
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


if __name__ == "__main__":
    unittest.main()
