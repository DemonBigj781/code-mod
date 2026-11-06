#!/usr/bin/env python3
"""Create the deterministic Camoufox runtime ZIP appended to the Code APE."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import stat
import tempfile
import zipfile
from typing import Any, Iterable


FIXED_ZIP_TIME = (1980, 1, 1, 0, 0, 0)
REQUIRED_PATHS = (
    "camoufox/camoufox-bin",
    "camoufox/camoufox.cfg",
    "camoufox/camoucfg.jvv",
    "camoufox/properties.json",
    "camoufox/GeoLite2-City.mmdb",
    "camoufox/fontconfig/linux/fonts.conf",
    "lib/libEGL.so.1",
    "lib/libGL.so.1",
    "lib/libGLX_mesa.so.0",
    "lib/libLLVM.so",
    "lib/libLLVM.so.22.1",
    "lib/dri/libdril_dri.so",
    "lib/dri/swrast_dri.so",
    "licenses/camoufox-MPL-2.0.txt",
    "licenses/mesa-MIT.txt",
    "licenses/llvm-Apache-2.0-WITH-LLVM-exception.txt",
)
EXCLUDED_TOP_LEVEL = {"manifest.json", "smoke-profile"}


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def safe_relative_symlink(path: pathlib.PurePosixPath, target: str) -> None:
    target_path = pathlib.PurePosixPath(target)
    if target_path.is_absolute():
        raise ValueError(f"runtime symlink must be relative: {path} -> {target}")
    resolved: list[str] = []
    for part in (*path.parent.parts, *target_path.parts):
        if part in {"", "."}:
            continue
        if part == "..":
            if not resolved:
                raise ValueError(f"runtime symlink escapes the payload: {path} -> {target}")
            resolved.pop()
        else:
            resolved.append(part)


def iter_runtime_paths(root: pathlib.Path) -> Iterable[pathlib.Path]:
    for path in sorted(root.rglob("*"), key=lambda item: item.as_posix()):
        relative = path.relative_to(root)
        if relative.parts and relative.parts[0] in EXCLUDED_TOP_LEVEL:
            continue
        if path.is_dir() and not path.is_symlink():
            continue
        yield path


def inventory(root: pathlib.Path) -> list[dict[str, Any]]:
    missing = []
    for relative in REQUIRED_PATHS:
        try:
            (root / relative).lstat()
        except FileNotFoundError:
            missing.append(relative)
    if missing:
        raise FileNotFoundError(f"browser runtime is missing required paths: {', '.join(missing)}")

    records: list[dict[str, Any]] = []
    for path in iter_runtime_paths(root):
        relative = path.relative_to(root).as_posix()
        metadata = path.lstat()
        mode = stat.S_IMODE(metadata.st_mode)
        if stat.S_ISLNK(metadata.st_mode):
            target = os.readlink(path)
            safe_relative_symlink(pathlib.PurePosixPath(relative), target)
            payload = target.encode("utf-8")
            records.append(
                {
                    "path": relative,
                    "type": "symlink",
                    "target": target,
                    "size": len(payload),
                    "sha256": sha256_bytes(payload),
                    "mode": f"{mode:04o}",
                }
            )
        elif stat.S_ISREG(metadata.st_mode):
            records.append(
                {
                    "path": relative,
                    "type": "file",
                    "size": metadata.st_size,
                    "sha256": sha256_file(path),
                    "mode": f"{mode:04o}",
                }
            )
        else:
            raise ValueError(f"unsupported browser runtime file type: {path}")
    return records


def tree_digest(records: list[dict[str, Any]]) -> str:
    digest = hashlib.sha256()
    for record in records:
        fields = (
            record["type"],
            record["path"],
            record["mode"],
            str(record["size"]),
            record["sha256"],
            record.get("target", ""),
        )
        digest.update("\0".join(fields).encode("utf-8"))
        digest.update(b"\n")
    return digest.hexdigest()


def runtime_manifest(pins: dict[str, Any], records: list[dict[str, Any]]) -> dict[str, Any]:
    runtime = pins["browser_runtime"]
    return {
        "schemaVersion": runtime["schema_version"],
        "target": runtime["target"],
        "archivePrefix": runtime["archive_prefix"],
        "camoufox": {
            "version": runtime["camoufox_version"],
            "release": runtime["camoufox_release"],
            "sourceRevision": runtime["camoufox_source_commit"],
            "assetUrl": runtime["camoufox_asset_url"],
            "assetSha256": runtime["camoufox_asset_sha256"],
            "rustRevision": runtime["camoufox_rust_revision"],
        },
        "automation": {
            "protocol": runtime["automation_protocol"],
            "driver": runtime["automation_driver"],
            "driverRevision": runtime["camoufox_rust_revision"],
            "entrypoint": runtime["automation_entrypoint"],
            "transport": runtime["automation_transport"],
        },
        "firefox": {"version": runtime["camoufox_version"]},
        "mesa": {
            "version": runtime["mesa_version"],
            "sourceKind": runtime["renderer_source_kind"],
            "sourceRevision": runtime["renderer_source_revision"],
        },
        "llvm": {"version": runtime["llvm_version"], "soname": "libLLVM.so.22.1"},
        "licenses": {
            "camoufox": "licenses/camoufox-MPL-2.0.txt",
            "mesa": "licenses/mesa-MIT.txt",
            "llvm": "licenses/llvm-Apache-2.0-WITH-LLVM-exception.txt",
        },
        "treeSha256": tree_digest(records),
        "files": records,
    }


def zip_info(name: str, mode: int, file_type: int = stat.S_IFREG) -> zipfile.ZipInfo:
    info = zipfile.ZipInfo(name, FIXED_ZIP_TIME)
    info.create_system = 3
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = ((file_type | mode) & 0xFFFF) << 16
    return info


def write_archive(
    root: pathlib.Path,
    output: pathlib.Path,
    manifest_output: pathlib.Path | None,
    pins: dict[str, Any],
) -> dict[str, Any]:
    records = inventory(root)
    manifest = runtime_manifest(pins, records)
    manifest_bytes = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")
    prefix = manifest["archivePrefix"]
    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{output.name}.", dir=output.parent)
    os.close(descriptor)
    temporary = pathlib.Path(temporary_name)
    try:
        with zipfile.ZipFile(
            temporary,
            "w",
            compression=zipfile.ZIP_DEFLATED,
            compresslevel=9,
            allowZip64=True,
        ) as archive:
            archive.writestr(zip_info(f"{prefix}manifest.json", 0o644), manifest_bytes)
            for record in records:
                source = root / record["path"]
                name = f"{prefix}{record['path']}"
                mode = int(record["mode"], 8)
                if record["type"] == "symlink":
                    archive.writestr(
                        zip_info(name, mode, stat.S_IFLNK), record["target"].encode("utf-8")
                    )
                else:
                    with source.open("rb") as input_stream, archive.open(
                        zip_info(name, mode), "w", force_zip64=True
                    ) as output_stream:
                        while chunk := input_stream.read(1024 * 1024):
                            output_stream.write(chunk)
        os.replace(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)

    if manifest_output is not None:
        manifest_output.parent.mkdir(parents=True, exist_ok=True)
        manifest_output.write_bytes(manifest_bytes)
    return {
        "archive": str(output),
        "archiveSize": output.stat().st_size,
        "archiveSha256": sha256_file(output),
        "fileCount": len(records),
        "treeSha256": manifest["treeSha256"],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--pins", type=pathlib.Path, required=True)
    parser.add_argument("--manifest-output", type=pathlib.Path)
    parser.add_argument("--report", type=pathlib.Path)
    args = parser.parse_args()

    pins = json.loads(args.pins.read_text(encoding="utf-8"))
    report = write_archive(args.source, args.output, args.manifest_output, pins)
    if args.report is not None:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
