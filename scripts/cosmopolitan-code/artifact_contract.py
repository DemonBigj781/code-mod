#!/usr/bin/env python3
"""Validate the immutable manifest shipped beside the Code APE artifact."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from pathlib import PurePosixPath
from typing import Any
import zipfile


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


def sha256_suffix(path: Path, size: int) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        artifact.seek(-size, 2)
        remaining = size
        while remaining:
            chunk = artifact.read(min(1024 * 1024, remaining))
            if not chunk:
                raise ArtifactContractError("browser_runtime archive is truncated")
            digest.update(chunk)
            remaining -= len(chunk)
    return digest.hexdigest()


def _browser_runtime_identity(pins: dict[str, Any]) -> dict[str, Any]:
    runtime = pins.get("browser_runtime")
    if not isinstance(runtime, dict):
        raise ArtifactContractError("pin file has no browser_runtime contract")
    return {
        "target": runtime.get("target"),
        "camoufox": {
            "version": runtime.get("camoufox_version"),
            "release": runtime.get("camoufox_release"),
            "source_commit": runtime.get("camoufox_source_commit"),
            "asset_url": runtime.get("camoufox_asset_url"),
            "asset_sha256": runtime.get("camoufox_asset_sha256"),
            "rust_revision": runtime.get("camoufox_rust_revision"),
        },
        "automation": {
            "protocol": runtime.get("automation_protocol"),
            "driver": runtime.get("automation_driver"),
            "driver_revision": runtime.get("camoufox_rust_revision"),
            "entrypoint": runtime.get("automation_entrypoint"),
            "transport": runtime.get("automation_transport"),
        },
        "mesa": {
            "version": runtime.get("mesa_version"),
            "source_kind": runtime.get("renderer_source_kind"),
            "source_revision": runtime.get("renderer_source_revision"),
        },
        "llvm": {"version": runtime.get("llvm_version")},
    }


def _read_browser_runtime_manifest(
    archive: zipfile.ZipFile,
    pins: dict[str, Any],
) -> tuple[dict[str, Any], int]:
    runtime = pins["browser_runtime"]
    prefix = runtime["archive_prefix"]
    manifest_name = f"{prefix}manifest.json"
    try:
        info = archive.getinfo(manifest_name)
    except KeyError as exc:
        raise ArtifactContractError(
            f"browser_runtime archive is missing {manifest_name}"
        ) from exc
    if info.file_size > 16 * 1024 * 1024:
        raise ArtifactContractError("browser_runtime manifest is unreasonably large")
    try:
        manifest = json.loads(archive.read(info).decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError, RuntimeError, zipfile.BadZipFile) as exc:
        raise ArtifactContractError(f"browser_runtime manifest is invalid: {exc}") from exc
    if not isinstance(manifest, dict):
        raise ArtifactContractError("browser_runtime manifest must be a JSON object")

    expected_inner = {
        "schemaVersion": runtime["schema_version"],
        "target": runtime["target"],
        "archivePrefix": prefix,
    }
    for field, expected in expected_inner.items():
        if manifest.get(field) != expected:
            raise ArtifactContractError(
                f"browser_runtime manifest {field} does not match the pin"
            )
    camoufox = manifest.get("camoufox")
    expected_camoufox = {
        "version": runtime["camoufox_version"],
        "release": runtime["camoufox_release"],
        "sourceRevision": runtime["camoufox_source_commit"],
        "assetUrl": runtime["camoufox_asset_url"],
        "assetSha256": runtime["camoufox_asset_sha256"],
        "rustRevision": runtime["camoufox_rust_revision"],
    }
    if camoufox != expected_camoufox:
        raise ArtifactContractError("browser_runtime Camoufox identity does not match the pins")
    automation = manifest.get("automation")
    if automation != {
        "protocol": runtime["automation_protocol"],
        "driver": runtime["automation_driver"],
        "driverRevision": runtime["camoufox_rust_revision"],
        "entrypoint": runtime["automation_entrypoint"],
        "transport": runtime["automation_transport"],
    }:
        raise ArtifactContractError("browser_runtime automation API does not match the pins")
    mesa = manifest.get("mesa")
    if mesa != {
        "version": runtime["mesa_version"],
        "sourceKind": runtime["renderer_source_kind"],
        "sourceRevision": runtime["renderer_source_revision"],
    }:
        raise ArtifactContractError("browser_runtime Mesa identity does not match the pins")
    llvm = manifest.get("llvm")
    if not isinstance(llvm, dict) or llvm.get("version") != runtime["llvm_version"]:
        raise ArtifactContractError("browser_runtime LLVM identity does not match the pins")

    tree_sha256 = manifest.get("treeSha256")
    if not isinstance(tree_sha256, str) or not SHA256_RE.fullmatch(tree_sha256):
        raise ArtifactContractError("browser_runtime treeSha256 is invalid")
    files = manifest.get("files")
    if not isinstance(files, list) or not files:
        raise ArtifactContractError("browser_runtime manifest has no file inventory")
    expected_names = {manifest_name}
    for record in files:
        if not isinstance(record, dict) or not isinstance(record.get("path"), str):
            raise ArtifactContractError("browser_runtime manifest has an invalid file record")
        relative = PurePosixPath(record["path"])
        if (
            relative.is_absolute()
            or not relative.parts
            or any(part in {"", ".", ".."} for part in relative.parts)
        ):
            raise ArtifactContractError(
                f"browser_runtime manifest has an unsafe path: {record['path']!r}"
            )
        name = f"{prefix}{record['path']}"
        if name in expected_names:
            raise ArtifactContractError(f"browser_runtime manifest repeats {name}")
        expected_names.add(name)
    names = [entry.filename for entry in archive.infolist()]
    if len(names) != len(set(names)):
        raise ArtifactContractError("browser_runtime archive contains duplicate entries")
    if set(names) != expected_names:
        raise ArtifactContractError("browser_runtime archive inventory does not match its manifest")
    return manifest, len(files)


def inspect_browser_runtime_archive(
    archive_path: str | Path,
    pins: dict[str, Any],
) -> dict[str, Any]:
    path = Path(archive_path)
    if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
        raise ArtifactContractError(
            f"browser_runtime archive is missing, empty, or not a regular file: {path}"
        )
    try:
        with zipfile.ZipFile(path) as archive:
            manifest, file_count = _read_browser_runtime_manifest(archive, pins)
    except (OSError, zipfile.BadZipFile) as exc:
        raise ArtifactContractError(f"browser_runtime archive is invalid: {exc}") from exc
    return {
        "archive_sha256": sha256(path),
        "archive_size_bytes": path.stat().st_size,
        "tree_sha256": manifest["treeSha256"],
        "file_count": file_count,
        **_browser_runtime_identity(pins),
    }


def validate_browser_runtime_metadata(
    artifact: Path,
    metadata: Any,
    pins: dict[str, Any],
) -> dict[str, Any]:
    if not isinstance(metadata, dict):
        raise ArtifactContractError("browser_runtime metadata is missing")
    expected_identity = _browser_runtime_identity(pins)
    for field, expected in expected_identity.items():
        if metadata.get(field) != expected:
            raise ArtifactContractError(f"browser_runtime {field} does not match the pins")
    size = metadata.get("archive_size_bytes")
    if type(size) is not int or size <= 0 or size > artifact.stat().st_size:
        raise ArtifactContractError("browser_runtime archive_size_bytes is invalid")
    archive_hash = metadata.get("archive_sha256")
    if not isinstance(archive_hash, str) or not SHA256_RE.fullmatch(archive_hash):
        raise ArtifactContractError("browser_runtime archive_sha256 is invalid")
    if sha256_suffix(artifact, size) != archive_hash:
        raise ArtifactContractError(
            "browser_runtime archive SHA-256 does not match the artifact suffix"
        )
    try:
        with zipfile.ZipFile(artifact) as archive:
            manifest, file_count = _read_browser_runtime_manifest(archive, pins)
    except (OSError, zipfile.BadZipFile) as exc:
        raise ArtifactContractError(f"browser_runtime archive is invalid: {exc}") from exc
    if metadata.get("tree_sha256") != manifest["treeSha256"]:
        raise ArtifactContractError("browser_runtime tree_sha256 does not match the archive")
    if metadata.get("file_count") != file_count:
        raise ArtifactContractError("browser_runtime file_count does not match the archive")
    return metadata


def validate_runtime_metadata(runtime: Any, pins: dict[str, Any]) -> None:
    if not isinstance(runtime, dict) or runtime.get("source_built") is not True:
        raise ArtifactContractError("cosmopolitan_runtime must be built from the pinned fork")
    if runtime.get("source_commit") != pins["cosmopolitan_commit"] or runtime.get("mode") != "x86_64":
        raise ArtifactContractError("cosmopolitan_runtime does not match the pinned fork and target")
    hashes = runtime.get("hashes")
    for name in ("libraries_sha256", "headers_sha256", "libcosmo_sha256", "libcxx_sha256", "loader_sha256", "apelink_sha256", "tlscc_sha256"):
        if not isinstance(hashes, dict) or not isinstance(hashes.get(name), str) or not SHA256_RE.fullmatch(hashes[name]):
            raise ArtifactContractError(f"cosmopolitan_runtime is missing a valid {name}")


def validate_compiler_bootstrap(bootstrap: Any, pins: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(bootstrap, dict) or bootstrap.get("schema_version") != 1:
        raise ArtifactContractError("compiler_bootstrap must be identified by schema version 1")
    if bootstrap.get("kind") != pins.get("compiler_bootstrap_kind"):
        raise ArtifactContractError("compiler_bootstrap kind does not match the pin")
    if bootstrap.get("source_commit") != pins.get("compiler_bootstrap_source_commit"):
        raise ArtifactContractError("compiler_bootstrap source_commit does not match the pin")
    if bootstrap.get("version") != pins.get("compiler_bootstrap_version"):
        raise ArtifactContractError("compiler_bootstrap version does not match the pin")
    if bootstrap.get("archive_url") != pins.get("compiler_bootstrap_archive_url"):
        raise ArtifactContractError("compiler_bootstrap archive URL does not match the pin")
    if bootstrap.get("archive_sha256") != pins.get("compiler_bootstrap_archive_sha256"):
        raise ArtifactContractError("compiler_bootstrap archive does not match the pin")
    if bootstrap.get("installed_tree_sha256") != pins.get(
        "compiler_bootstrap_installed_tree_sha256"
    ):
        raise ArtifactContractError("compiler_bootstrap installed tree does not match the pin")
    if bootstrap.get("source_worktree_dirty") is not False:
        raise ArtifactContractError("compiler_bootstrap source_worktree_dirty must be false")
    verification = bootstrap.get("cosmocc_verification")
    if verification != pins.get("compiler_bootstrap_verification"):
        raise ArtifactContractError("compiler_bootstrap verification does not match the pin")
    for field in ("archive_sha256", "installed_tree_sha256"):
        if not isinstance(bootstrap.get(field), str) or not SHA256_RE.fullmatch(
            bootstrap[field]
        ):
            raise ArtifactContractError(f"compiler_bootstrap has an invalid {field}")
    compatibility = bootstrap.get("upstream_compatibility_stamp")
    expected_compatibility = (
        f"cosmocc-{pins.get('compiler_bootstrap_version')} "
        f"sha256:{pins.get('compiler_bootstrap_archive_sha256')}"
    )
    if compatibility != expected_compatibility:
        raise ArtifactContractError("compiler_bootstrap has no upstream compatibility stamp")
    return {
        "schema_version": 1,
        "kind": bootstrap["kind"],
        "version": bootstrap["version"],
        "source_commit": bootstrap["source_commit"],
        "source_worktree_dirty": bootstrap["source_worktree_dirty"],
        "archive_url": bootstrap["archive_url"],
        "archive_sha256": bootstrap["archive_sha256"],
        "installed_tree_sha256": bootstrap["installed_tree_sha256"],
        "cosmocc_verification": verification,
        "upstream_compatibility_stamp": compatibility,
    }


def write_artifact_manifest(
    artifact_path: str | Path,
    manifest_path: str | Path,
    runtime_path: str | Path,
    compiler_bootstrap_path: str | Path,
    source_commit: str,
    *,
    browser_runtime_path: str | Path,
    source_clean: bool,
    source_snapshot_sha256: str | None = None,
    pins_path: str | Path = PINS_PATH,
) -> dict[str, Any]:
    """Write the release or development manifest for a completed APE build."""
    artifact = Path(artifact_path)
    manifest_path = Path(manifest_path)
    pins = read_json(Path(pins_path), "pin file")
    runtime = read_json(Path(runtime_path), "Cosmopolitan runtime metadata")
    compiler_bootstrap = validate_compiler_bootstrap(
        read_json(Path(compiler_bootstrap_path), "compiler bootstrap metadata"), pins
    )

    if artifact.is_symlink() or not artifact.is_file() or artifact.stat().st_size == 0:
        raise ArtifactContractError(f"artifact is missing, empty, or not a regular file: {artifact}")
    if artifact.name != pins.get("artifact_name"):
        raise ArtifactContractError("artifact_name does not match the pinned output name")
    if not GIT_COMMIT_RE.fullmatch(source_commit):
        raise ArtifactContractError("source_commit must be a full lowercase Git commit SHA")
    if type(source_clean) is not bool:
        raise ArtifactContractError("source_clean must be a boolean")
    validate_runtime_metadata(runtime, pins)
    browser_runtime = inspect_browser_runtime_archive(browser_runtime_path, pins)

    manifest: dict[str, Any] = {
        "schema_version": pins["schema_version"],
        "source_commit": source_commit,
        "source_clean": source_clean,
        "cosmopolitan_commit": pins["cosmopolitan_commit"],
        "rust_ape_commit": pins["rust_ape_commit"],
        "compiler_bootstrap": compiler_bootstrap,
        "cosmopolitan_runtime": runtime,
        "browser_runtime": browser_runtime,
        "target_architectures": pins["target_architectures"],
        "build_profile": pins["build_profile"],
        "release_opt_level": pins["release_opt_level"],
        "release_lto": pins["release_lto"],
        "release_codegen_units": pins["release_codegen_units"],
        "artifact_name": pins["artifact_name"],
        "artifact_size_bytes": artifact.stat().st_size,
        "artifact_sha256": sha256(artifact),
    }
    if not source_clean:
        if not isinstance(source_snapshot_sha256, str) or not SHA256_RE.fullmatch(
            source_snapshot_sha256
        ):
            raise ArtifactContractError(
                "a valid source snapshot SHA-256 is required for a working-tree build"
            )
        manifest["development_build"] = True
        manifest["source_snapshot_sha256"] = source_snapshot_sha256

    manifest_path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return manifest


def _validate_artifact_metadata(
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

    validate_runtime_metadata(manifest.get("cosmopolitan_runtime"), pins)
    validate_compiler_bootstrap(manifest.get("compiler_bootstrap"), pins)
    validate_browser_runtime_metadata(artifact, manifest.get("browser_runtime"), pins)

    if manifest.get("target_architectures") != pins.get("target_architectures"):
        raise ArtifactContractError("target_architectures does not match the pinned build target")
    if manifest.get("build_profile") != pins.get("build_profile"):
        raise ArtifactContractError("build_profile does not match the pinned profile")
    if manifest.get("release_opt_level") != pins.get("release_opt_level"):
        raise ArtifactContractError("release_opt_level does not match the pinned profile settings")
    if manifest.get("release_lto") != pins.get("release_lto"):
        raise ArtifactContractError("release_lto does not match the pinned profile settings")
    if manifest.get("release_codegen_units") != pins.get("release_codegen_units"):
        raise ArtifactContractError("release_codegen_units does not match the pinned profile settings")

    source_commit = manifest.get("source_commit")
    if not isinstance(source_commit, str) or not GIT_COMMIT_RE.fullmatch(source_commit):
        raise ArtifactContractError("source_commit must be a full lowercase Git commit SHA")
    if expected_source_commit is not None and source_commit != expected_source_commit:
        raise ArtifactContractError("source_commit does not match the checked-out source revision")

    return manifest


def validate_artifact(
    artifact_path: str | Path,
    manifest_path: str | Path,
    pins_path: str | Path = PINS_PATH,
    expected_source_commit: str | None = None,
) -> dict[str, Any]:
    manifest = _validate_artifact_metadata(
        artifact_path,
        manifest_path,
        pins_path,
        expected_source_commit,
    )
    if manifest.get("source_clean") is not True:
        raise ArtifactContractError("artifact must be built from a clean committed source tree")
    return manifest


def validate_development_artifact(
    artifact_path: str | Path,
    manifest_path: str | Path,
    pins_path: str | Path = PINS_PATH,
    expected_source_commit: str | None = None,
) -> dict[str, Any]:
    """Validate a local APE built from an identified working-tree snapshot."""
    manifest = _validate_artifact_metadata(
        artifact_path,
        manifest_path,
        pins_path,
        expected_source_commit,
    )
    if manifest.get("source_clean") is not False or manifest.get("development_build") is not True:
        raise ArtifactContractError(
            "development artifact must be marked as a dirty working-tree build"
        )
    snapshot_hash = manifest.get("source_snapshot_sha256")
    if not isinstance(snapshot_hash, str) or not SHA256_RE.fullmatch(snapshot_hash):
        raise ArtifactContractError(
            "source_snapshot_sha256 must identify the development source snapshot"
        )
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
