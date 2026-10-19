import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from artifact_contract import PINS_PATH
from verify_runtime_probe import verify_probe


class RuntimeProbeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.commit = "a" * 40
        pins = json.loads(PINS_PATH.read_text())
        self.metadata = {
            "source_built": True, "source_commit": pins["cosmopolitan_commit"], "mode": "x86_64",
            "hashes": {name: "b" * 64 for name in (
                "libraries_sha256", "headers_sha256", "libcosmo_sha256", "libcxx_sha256",
                "loader_sha256", "apelink_sha256", "tlscc_sha256",
            )},
        }
        self.artifact = self.directory / "runtime-smoke.com"
        self.artifact.write_bytes(b"probe fixture")
        digest = hashlib.sha256(self.artifact.read_bytes()).hexdigest()
        (self.directory / "runtime-smoke.sha256").write_text(f"{digest}  runtime-smoke.com\n")
        (self.directory / "code-source-commit").write_text(self.commit + "\n")
        (self.directory / "fork-runtime.json").write_text(json.dumps(self.metadata))

    def test_accepts_matching_probe_and_sources(self):
        verify_probe(self.directory, self.commit)

    def test_rejects_tampered_executable(self):
        self.artifact.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            verify_probe(self.directory, self.commit)

    def test_rejects_different_code_revision(self):
        with self.assertRaisesRegex(ValueError, "different Code source"):
            verify_probe(self.directory, "c" * 40)

    def test_rejects_prebuilt_runtime_record(self):
        self.metadata["source_built"] = False
        (self.directory / "fork-runtime.json").write_text(json.dumps(self.metadata))
        with self.assertRaisesRegex(ValueError, "built from the pinned fork"):
            verify_probe(self.directory, self.commit)

    def test_windows_runtime_gate_precedes_full_build(self):
        root = Path(__file__).resolve().parents[3]
        workflow = (root / ".github/workflows/cosmopolitan-code-ape.yml").read_text()
        self.assertIn("needs: runtime-windows", workflow)
        self.assertEqual(workflow.count("if: github.ref == 'refs/heads/Main'"), 4)
        self.assertIn("ref: ${{ steps.fork-pin.outputs.sha }}", workflow)


if __name__ == "__main__":
    unittest.main()
