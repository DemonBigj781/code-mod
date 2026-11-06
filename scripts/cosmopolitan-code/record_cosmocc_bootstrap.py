#!/usr/bin/env python3
"""Record and verify the official Cosmocc compiler installed by Rust-APE."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
from typing import Any

from artifact_contract import sha256
from cache_cosmocc import read_release


REQUIRED_COMPILER_PATHS = (
    "bin/cosmocc",
    "bin/apelink",
    "bin/x86_64-linux-cosmo-gcc",
    "bin/x86_64-unknown-cosmo-cc",
    "x86_64-linux-cosmo/lib/libcosmo.a",
)


def tree_sha256(root: Path) -> str:
    """Hash every name, kind, mode, file payload, and relative symlink target."""
    digest = hashlib.sha256()

    def visit(directory: Path) -> None:
        for path in sorted(directory.iterdir()):
            mode = path.lstat().st_mode
            name = path.relative_to(root).as_posix()
            if stat.S_ISLNK(mode):
                value: list[Any] = [name, "link", os.readlink(path)]
            elif stat.S_ISREG(mode):
                value = [name, "file", stat.S_IMODE(mode), sha256(path)]
            elif stat.S_ISDIR(mode):
                value = [name, "directory", stat.S_IMODE(mode)]
            else:
                raise ValueError(f"Unsupported compiler SDK file type: {path}")
            digest.update((json.dumps(value, ensure_ascii=True) + "\n").encode())
            if stat.S_ISDIR(mode):
                visit(path)

    visit(root)
    return digest.hexdigest()


def read_pins(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"Cannot read Code compiler bootstrap pins: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError("Code compiler bootstrap pins must be a JSON object")
    return value


def record_bootstrap(
    config: Path,
    pins_path: Path,
    archive: Path,
    sdk: Path,
    output: Path,
) -> dict[str, Any]:
    release = read_release(config, pins_path)
    pins = read_pins(pins_path)
    expected_archive = sdk / "cache" / release["name"]
    if archive.resolve() != expected_archive.resolve():
        raise ValueError("Compiler archive is outside the pinned Rust-APE SDK cache")
    if archive.is_symlink() or not archive.is_file():
        raise ValueError(f"Compiler archive must be a regular file: {archive}")
    if sha256(archive) != release["sha256"]:
        raise ValueError("Compiler archive does not match the pinned official release")

    compiler = sdk / "vendor" / "cosmocc"
    if compiler.is_symlink() or not compiler.is_dir():
        raise ValueError(f"Installed compiler tree is unavailable: {compiler}")
    for relative in REQUIRED_COMPILER_PATHS:
        path = compiler / relative
        if not path.exists():
            raise ValueError(f"Installed official compiler is missing {relative}")
        if relative.startswith("bin/") and not os.access(path, os.X_OK):
            raise ValueError(f"Installed official compiler is not executable: {relative}")

    compatibility = f"cosmocc-{release['version']} sha256:{release['sha256']}"
    stamp = sdk / "vendor" / ".stamps" / "cosmocc"
    if stamp.is_symlink() or not stamp.is_file():
        raise ValueError("Rust-APE did not record the Cosmocc compatibility stamp")
    if stamp.read_text(encoding="utf-8").strip() != compatibility:
        raise ValueError("Rust-APE Cosmocc compatibility stamp does not match the pin")

    installed_tree = tree_sha256(compiler)
    if installed_tree != pins.get("compiler_bootstrap_installed_tree_sha256"):
        raise ValueError(
            "Installed official compiler tree does not match Code's bootstrap pin: "
            f"expected {pins.get('compiler_bootstrap_installed_tree_sha256')}, "
            f"found {installed_tree}"
        )
    verification = pins.get("compiler_bootstrap_verification")
    if not isinstance(verification, str) or not verification:
        raise ValueError("Code has no compiler bootstrap verification description")

    metadata = {
        "schema_version": 1,
        "kind": pins["compiler_bootstrap_kind"],
        "version": release["version"],
        "source_commit": release["source_commit"],
        "source_worktree_dirty": False,
        "archive_url": release["archive_url"],
        "archive_sha256": release["sha256"],
        "installed_tree_sha256": installed_tree,
        "cosmocc_verification": verification,
        "upstream_compatibility_stamp": compatibility,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w", encoding="utf-8", dir=output.parent, delete=False
    ) as stream:
        temporary = Path(stream.name)
        json.dump(metadata, stream, indent=2, sort_keys=True)
        stream.write("\n")
    try:
        temporary.replace(output)
    finally:
        temporary.unlink(missing_ok=True)
    return metadata


def main() -> int:
    if len(sys.argv) != 6:
        print(
            "usage: record_cosmocc_bootstrap.py CONFIG.rs PINS.json ARCHIVE SDK OUTPUT",
            file=sys.stderr,
        )
        return 2
    try:
        record_bootstrap(
            Path(sys.argv[1]),
            Path(sys.argv[2]),
            Path(sys.argv[3]),
            Path(sys.argv[4]),
            Path(sys.argv[5]),
        )
    except (OSError, ValueError) as exc:
        print(f"official compiler bootstrap verification failed: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
