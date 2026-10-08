#!/usr/bin/env python3
"""Validate the immutable manifest shipped beside the Code APE artifact."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any


PINS_PATH = Path(__file__).with_name("pins.json")
SHA256_RE = re.compile(r"[0-9a-f]{64}\Z")
GIT_COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")


class ArtifactContractError(ValueError):
    """The artifact or its manifest does not meet the release contract."""


def read_json(path: Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ArtifactContractError(f"cannot read {label} {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ArtifactContractError(f"{label} must be a JSON object")
    return value


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for chunk in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_artifact(
    artifact_path: str | Path,
    manifest_path: str | Path,
    pins_path: str | Path = PINS_PATH,
    expected_source_commit: str | None = None,
) -> dict[str, Any]:
    artifact = Path(artifact_path)
    manifest = read_json(Path(manifest_path), "manifest")
    pins = read_json(Path(pins_path), "pin file")

    if artifact.is_symlink() or not artifact.is_file():
        raise ArtifactContractError(f"artifact is missing or not a regular file: {artifact}")
    size_bytes = artifact.stat().st_size
    if size_bytes == 0:
        raise ArtifactContractError("artifact is empty")

    if type(manifest.get("schema_version")) is not int or manifest["schema_version"] != 1:
        raise ArtifactContractError("unsupported or missing schema_version")
    if manifest.get("artifact_name") != artifact.name or artifact.name != pins.get("artifact_name"):
        raise ArtifactContractError("artifact_name does not match the pinned output name")
    if type(manifest.get("artifact_size_bytes")) is not int or manifest["artifact_size_bytes"] != size_bytes:
        raise ArtifactContractError("artifact_size_bytes does not match the artifact")

    recorded_hash = manifest.get("artifact_sha256")
    if not isinstance(recorded_hash, str) or not SHA256_RE.fullmatch(recorded_hash):
        raise ArtifactContractError("artifact_sha256 must be a lowercase SHA-256 hex digest")
    if recorded_hash != sha256(artifact):
        raise ArtifactContractError("artifact_sha256 does not match the artifact")

    for field in ("cosmopolitan_commit", "rust_ape_commit"):
        if manifest.get(field) != pins.get(field):
            raise ArtifactContractError(f"{field} does not match the pinned source")

    if manifest.get("target_architectures") != pins.get("target_architectures"):
        raise ArtifactContractError("target_architectures does not match the pinned build target")
    if manifest.get("build_profile") != pins.get("build_profile"):
        raise ArtifactContractError("build_profile does not match the pinned profile")
    if manifest.get("release_lto") != pins.get("release_lto"):
        raise ArtifactContractError("release_lto does not match the pinned profile settings")
    if manifest.get("release_codegen_units") != pins.get("release_codegen_units"):
        raise ArtifactContractError("release_codegen_units does not match the pinned profile settings")

    source_commit = manifest.get("source_commit")
    if not isinstance(source_commit, str) or not GIT_COMMIT_RE.fullmatch(source_commit):
        raise ArtifactContractError("source_commit must be a full lowercase Git commit SHA")
    if expected_source_commit is not None and source_commit != expected_source_commit:
        raise ArtifactContractError("source_commit does not match the checked-out source revision")
    if manifest.get("source_clean") is not True:
        raise ArtifactContractError("artifact must be built from a clean committed source tree")

    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--source-commit")
    args = parser.parse_args()
    try:
        validate_artifact(
            args.artifact,
            args.manifest,
            expected_source_commit=args.source_commit,
        )
    except ArtifactContractError as exc:
        print(f"artifact contract failed: {exc}", file=sys.stderr)
        return 1
    print("artifact manifest contract passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
