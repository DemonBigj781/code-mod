#!/usr/bin/env python3
"""Copy the APE build inputs from a working tree and identify the snapshot."""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
from pathlib import Path


SOURCE_PATHS = (
    Path("code-rs"),
    Path("codex-rs/models-manager/models.json"),
    Path("announcement_tip.toml"),
)


def _copy_source(source: Path, destination: Path) -> None:
    if source.is_dir():
        shutil.copytree(
            source,
            destination,
            dirs_exist_ok=True,
            symlinks=True,
            ignore=shutil.ignore_patterns("target", "__pycache__", "*.pyc"),
        )
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination, follow_symlinks=False)


def _snapshot_hash(destination: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(destination.rglob("*")):
        relative = path.relative_to(destination).as_posix().encode()
        if path.is_symlink():
            kind = b"symlink"
            payload = os.readlink(path).encode()
        elif path.is_file():
            kind = b"executable" if path.stat().st_mode & 0o111 else b"file"
            payload = path.read_bytes()
        else:
            continue
        digest.update(len(relative).to_bytes(8, "big"))
        digest.update(relative)
        digest.update(kind)
        digest.update(len(payload).to_bytes(8, "big"))
        digest.update(payload)
    return digest.hexdigest()


def snapshot_worktree(repo_root: Path, destination: Path) -> str:
    repo_root = repo_root.resolve()
    destination.mkdir(parents=True, exist_ok=True)
    for relative in SOURCE_PATHS:
        source = repo_root / relative
        if not source.exists() and not source.is_symlink():
            raise FileNotFoundError(f"required APE source path is missing: {source}")
        _copy_source(source, destination / relative)
    return _snapshot_hash(destination)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo_root", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    print(snapshot_worktree(args.repo_root, args.destination))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
