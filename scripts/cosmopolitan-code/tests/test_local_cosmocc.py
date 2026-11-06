import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import unittest
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import stage_local_cosmocc


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def tree_sha256(root: Path) -> str:
    digest = hashlib.sha256()

    def visit(directory: Path) -> None:
        for path in sorted(directory.iterdir()):
            mode = path.lstat().st_mode
            name = path.relative_to(root).as_posix()
            if stat.S_ISLNK(mode):
                value: list[object] = [name, "link", os.readlink(path)]
            elif stat.S_ISREG(mode):
                value = [name, "file", stat.S_IMODE(mode), sha256(path)]
            elif stat.S_ISDIR(mode):
                value = [name, "directory", stat.S_IMODE(mode)]
            else:
                raise AssertionError(f"unsupported fixture type: {path}")
            digest.update((json.dumps(value, ensure_ascii=True) + "\n").encode())
            if stat.S_ISDIR(mode):
                visit(path)

    visit(root)
    return digest.hexdigest()


class LocalCosmoccTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / "source"
        (self.source / "bin").mkdir(parents=True)
        (self.source / "include").mkdir()
        (self.source / "lib").mkdir()
        (self.source / "x86_64-linux-cosmo/lib").mkdir(parents=True)
        for directory in (
            self.source,
            self.source / "bin",
            self.source / "include",
            self.source / "lib",
            self.source / "x86_64-linux-cosmo",
            self.source / "x86_64-linux-cosmo/lib",
        ):
            directory.chmod(0o755)
        (self.source / "bin/cosmocc").write_text("#!/bin/sh\nexit 0\n")
        (self.source / "bin/cosmocc").chmod(0o755)
        (self.source / "bin/apelink").write_text("#!/bin/sh\nexit 0\n")
        (self.source / "bin/apelink").chmod(0o755)
        (self.source / "bin/x86_64-linux-cosmo-gcc").write_text("#!/bin/sh\nexit 0\n")
        (self.source / "bin/x86_64-linux-cosmo-gcc").chmod(0o755)
        (self.source / "bin/x86_64-unknown-cosmo-cc").symlink_to("cosmocc")
        (self.source / "include/stdio.h").write_text("/* fixture */\n")
        (self.source / "include/stdio.h").chmod(0o644)
        (self.source / "lib/libc.a").write_bytes(b"archive")
        (self.source / "lib/libc.a").chmod(0o644)
        (self.source / "x86_64-linux-cosmo/lib/libcosmo.a").write_bytes(b"cosmo")
        (self.source / "x86_64-linux-cosmo/lib/libcosmo.a").chmod(0o644)

        self.commit = "1" * 40
        self.compiler_tree = tree_sha256(self.source)
        self.provenance_data = {
            "schema_version": 1,
            "source_commit": self.commit,
            "source_worktree_dirty": True,
            "inputs": {
                "cosmocc": {
                    "tree_sha256": self.compiler_tree,
                    "verification": "unverified-local-input",
                }
            },
        }
        self.archive = self.root / "cosmocc-sdk.zip"
        self.provenance = self.root / "BUILD.json"
        self.provenance.write_text(json.dumps(self.provenance_data))
        self.config = self.root / "config.rs"
        self.config.write_text(
            'pub const COSMOCC_VERSION: &str = "4.0.2";\n'
            'pub const COSMOCC_SHA256: &str =\n'
            '    "85b8c37a406d862e656ad4ec14be9f6ce474c1b436b9615e91a55208aced3f44";\n'
        )
        self.pins = self.root / "pins.json"
        self.pins.write_text(
            json.dumps(
                {
                    "compiler_bootstrap_kind": "local-cosmopolitan-sdk",
                    "compiler_bootstrap_source_commit": self.commit,
                    "compiler_bootstrap_cosmocc_tree_sha256": self.compiler_tree,
                    "compiler_bootstrap_archive_sha256": "0" * 64,
                    "compiler_bootstrap_extracted_tree_sha256": self.compiler_tree,
                }
            )
        )
        self.sdk = self.root / "sdk"
        self.write_archive()

    def write_archive(self, extra=None, embedded=None):
        with zipfile.ZipFile(self.archive, "w") as archive:
            archive.writestr(
                "toolchain-metadata/BUILD.json",
                json.dumps(embedded or self.provenance_data),
            )
            for path in sorted(self.source.rglob("*")):
                if path.is_dir():
                    continue
                relative = path.relative_to(self.source).as_posix()
                mode = path.lstat().st_mode
                info = zipfile.ZipInfo(relative)
                info.create_system = 3
                info.external_attr = mode << 16
                if path.is_symlink():
                    archive.writestr(info, os.readlink(path))
                else:
                    archive.writestr(info, path.read_bytes())
            archive.writestr("rust-ape-sdk/ignored", b"not compiler input")
            if extra:
                name, payload, mode = extra
                info = zipfile.ZipInfo(name)
                info.create_system = 3
                info.external_attr = mode << 16
                archive.writestr(info, payload)
        pins = json.loads(self.pins.read_text())
        pins["compiler_bootstrap_archive_sha256"] = sha256(self.archive)
        self.pins.write_text(json.dumps(pins))

    def stage(self):
        return stage_local_cosmocc.stage_local_sdk(
            self.archive,
            self.provenance,
            self.config,
            self.pins,
            self.sdk,
        )

    def test_stages_verified_local_sdk_without_bundled_rust_payload(self):
        record = self.stage()

        compiler = self.sdk / "vendor/cosmocc"
        self.assertTrue((compiler / "bin/cosmocc").is_file())
        self.assertTrue(os.access(compiler / "bin/cosmocc", os.X_OK))
        self.assertTrue((compiler / "bin/x86_64-unknown-cosmo-cc").is_symlink())
        self.assertEqual(os.readlink(compiler / "bin/x86_64-unknown-cosmo-cc"), "cosmocc")
        self.assertFalse((compiler / "rust-ape-sdk").exists())
        self.assertEqual(tree_sha256(compiler), self.compiler_tree)
        self.assertEqual(
            (self.sdk / "vendor/.stamps/cosmocc").read_text(),
            "cosmocc-4.0.2 sha256:85b8c37a406d862e656ad4ec14be9f6ce474c1b436b9615e91a55208aced3f44\n",
        )
        compatibility_archive = self.sdk / "cache/cosmocc-4.0.2.zip"
        with zipfile.ZipFile(compatibility_archive) as archive:
            self.assertEqual(
                archive.namelist(),
                ["bin/x86_64-unknown-cosmo-cc"],
            )
            self.assertEqual(
                archive.read("bin/x86_64-unknown-cosmo-cc"),
                b"cosmocc",
            )
        self.assertEqual(record["source_commit"], self.commit)
        self.assertTrue(record["source_worktree_dirty"])
        self.assertEqual(record["cosmocc_tree_sha256"], self.compiler_tree)
        self.assertEqual(record["installed_tree_sha256"], self.compiler_tree)
        self.assertEqual(record["archive_sha256"], sha256(self.archive))
        self.assertEqual(
            record["compatibility_archive_sha256"],
            sha256(compatibility_archive),
        )
        self.assertEqual(
            json.loads((self.sdk / "generated/local-cosmocc-bootstrap.json").read_text()),
            record,
        )

    def test_matching_existing_stage_is_reused(self):
        first = self.stage()
        compiler = self.sdk / "vendor/cosmocc/bin/cosmocc"
        before = compiler.stat().st_mtime_ns
        second = self.stage()
        self.assertEqual(second, first)
        self.assertEqual(compiler.stat().st_mtime_ns, before)

    def test_external_provenance_details_can_refresh_without_reinstalling(self):
        first = self.stage()
        compiler = self.sdk / "vendor/cosmocc/bin/cosmocc"
        before = compiler.stat().st_mtime_ns
        refreshed = json.loads(self.provenance.read_text())
        refreshed["artifact"] = {"file": "toolchain.com", "sha256": "4" * 64}
        self.provenance.write_text(json.dumps(refreshed))

        second = self.stage()

        self.assertNotEqual(second["provenance_sha256"], first["provenance_sha256"])
        self.assertEqual(compiler.stat().st_mtime_ns, before)

    def test_existing_unidentified_compiler_is_preserved_and_rejected(self):
        compiler = self.sdk / "vendor/cosmocc"
        compiler.mkdir(parents=True)
        marker = compiler / "user-file"
        marker.write_text("preserve me")

        with self.assertRaisesRegex(ValueError, "existing compiler SDK is not the pinned local bootstrap"):
            self.stage()
        self.assertEqual(marker.read_text(), "preserve me")

    def test_archive_path_traversal_is_rejected(self):
        self.write_archive(extra=("bin/../../escape", b"bad", stat.S_IFREG | 0o644))
        with self.assertRaisesRegex(ValueError, "unsafe archive path"):
            self.stage()
        self.assertFalse((self.root / "escape").exists())

    def test_escaping_symlink_is_rejected(self):
        self.write_archive(extra=("bin/escape", "../../outside", stat.S_IFLNK | 0o777))
        with self.assertRaisesRegex(ValueError, "unsafe symlink target"):
            self.stage()

    def test_compiler_tree_must_match_pinned_provenance(self):
        pins = json.loads(self.pins.read_text())
        pins["compiler_bootstrap_extracted_tree_sha256"] = "2" * 64
        self.pins.write_text(json.dumps(pins))
        with self.assertRaisesRegex(ValueError, "compiler tree hash does not match"):
            self.stage()

    def test_embedded_and_external_provenance_must_agree(self):
        embedded = json.loads(json.dumps(self.provenance_data))
        embedded["source_commit"] = "3" * 40
        self.write_archive(embedded=embedded)
        with self.assertRaisesRegex(ValueError, "embedded SDK provenance does not match"):
            self.stage()

    def test_symlink_archive_or_provenance_is_rejected(self):
        real_archive = self.archive
        linked_archive = self.root / "linked-sdk.zip"
        linked_archive.symlink_to(real_archive.name)
        with self.assertRaisesRegex(ValueError, "regular file, not a symlink"):
            stage_local_cosmocc.stage_local_sdk(
                linked_archive,
                self.provenance,
                self.config,
                self.pins,
                self.sdk,
            )

    def test_prepare_runtime_stages_local_sdk_before_upstream_setup(self):
        support = Path(__file__).resolve().parents[1]
        script = (support / "prepare-runtime.sh").read_text()
        self.assertNotIn("cache_cosmocc.py", script)
        self.assertLess(script.index('"$support_dir/stage_local_cosmocc.py"'), script.index('bash "$run_script" setup'))


if __name__ == "__main__":
    unittest.main()
