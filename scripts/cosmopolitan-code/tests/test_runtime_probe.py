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

    def test_split_workflows_gate_one_cross_platform_build(self):
        root = Path(__file__).resolve().parents[3]
        workflows = root / ".github/workflows"
        test_workflow = (workflows / "test-only.yml").read_text()
        compile_workflow = (workflows / "compile-only.yml").read_text()
        artifact_workflow = (workflows / "complete-artifact-creation.yml").read_text()
        linux_smoke_job = artifact_workflow.split("\n  linux-smoke:\n", 1)[1].split(
            "\n  termux-bionic-smoke:\n", 1
        )[0]
        termux_smoke_job = artifact_workflow.split("\n  termux-bionic-smoke:\n", 1)[1].split(
            "\n  windows-smoke:\n", 1
        )[0]
        self.assertFalse((workflows / "cosmopolitan-code-ape.yml").exists())
        self.assertIn("uses: ./.github/workflows/test-only.yml", artifact_workflow)
        self.assertIn("uses: ./.github/workflows/compile-only.yml", artifact_workflow)
        self.assertIn(
            "needs:\n      - validate-inputs\n      - check\n      - test",
            artifact_workflow,
        )
        builder_image = (
            "ghcr.io/demonbigj781/cosmopolitan-code-builder@sha256:"
            "a76d2a7b9ace2a1ec3d874dd8e0751d0ddb677245459a6e1e864a9940fd66f9b"
        )
        self.assertEqual(test_workflow.count(f"image: {builder_image}"), 1)
        self.assertEqual(compile_workflow.count(f"image: {builder_image}"), 1)
        self.assertEqual(test_workflow.count("mozilla-actions/sccache-action@"), 1)
        self.assertEqual(compile_workflow.count("mozilla-actions/sccache-action@"), 1)
        self.assertIn("sh scripts/build-cosmopolitan-runtime.sh", test_workflow)
        self.assertIn("sh scripts/test-cosmopolitan-code.sh", test_workflow)
        self.assertIn("needs: test", test_workflow)
        self.assertIn("sh scripts/build-cosmopolitan-code.sh", compile_workflow)
        self.assertIn("o/cosmopolitan-code/runtime-smoke.com", compile_workflow)
        self.assertIn("needs: build", linux_smoke_job)
        self.assertIn("actions/download-artifact@", linux_smoke_job)
        self.assertIn("sh 'scripts/smoke-test cosmopolitan-code.sh'", linux_smoke_job)
        self.assertIn("check-rustwright-mcp.py", linux_smoke_job)
        self.assertIn(
            "- name: Smoke-test embedded Camoufox through Rustwright MCP\n"
            "        continue-on-error: true\n"
            "        run: python3 scripts/cosmopolitan-code/check-rustwright-mcp.py",
            linux_smoke_job,
        )
        self.assertIn("needs: build", termux_smoke_job)
        self.assertIn("actions/download-artifact@", termux_smoke_job)
        self.assertIn("termux/termux-docker-pacman@sha256:", termux_smoke_job)
        self.assertIn("chmod 755 o/cosmopolitan-code/*.com", termux_smoke_job)
        self.assertIn("sh \"scripts/smoke-test cosmopolitan-code.sh\"", termux_smoke_job)


if __name__ == "__main__":
    unittest.main()
