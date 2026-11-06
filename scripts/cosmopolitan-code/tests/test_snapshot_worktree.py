import tempfile
import unittest
from pathlib import Path
import sys


SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

from snapshot_worktree import snapshot_worktree


class SnapshotWorktreeTests(unittest.TestCase):
    def test_snapshot_copies_sources_excludes_build_outputs_and_hashes_content(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repo"
            destination = Path(directory) / "snapshot"
            (root / "code-rs/src").mkdir(parents=True)
            (root / "code-rs/target/debug").mkdir(parents=True)
            (root / "codex-rs/models-manager").mkdir(parents=True)
            (root / "code-rs/src/lib.rs").write_text("pub fn value() -> u8 { 1 }\n")
            (root / "code-rs/target/debug/huge").write_bytes(b"ignored")
            (root / "codex-rs/models-manager/models.json").write_text("{}\n")
            (root / "announcement_tip.toml").write_text('tip = "hello"\n')

            first = snapshot_worktree(root, destination)

            self.assertTrue((destination / "code-rs/src/lib.rs").is_file())
            self.assertFalse((destination / "code-rs/target").exists())
            self.assertRegex(first, r"^[0-9a-f]{64}$")

            (root / "code-rs/src/lib.rs").write_text("pub fn value() -> u8 { 2 }\n")
            second_destination = Path(directory) / "snapshot-2"
            second = snapshot_worktree(root, second_destination)
            self.assertNotEqual(first, second)


if __name__ == "__main__":
    unittest.main()
