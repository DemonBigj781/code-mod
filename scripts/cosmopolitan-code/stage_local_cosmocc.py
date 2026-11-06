#!/usr/bin/env python3
"""Install a fingerprinted local Cosmopolitan compiler SDK without networking."""

from __future__ import annotations

import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import sys
import tempfile
from typing import Any, BinaryIO
import zipfile


SHA256_RE = re.compile(r"[0-9a-f]{64}\Z")
COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")
METADATA_RESOURCE = "toolchain-metadata/BUILD.json"
ALLOWED_COMPILER_ROOTS = {
    "LICENSE.gpl2",
    "LICENSE.gpl3",
    "LICENSE.lgpl2",
    "LICENSE.lgpl3",
    "README.md",
    "aarch64-linux-cosmo",
    "bin",
    "include",
    "lib",
    "libexec",
    "x86_64-linux-cosmo",
}
REQUIRED_COMPILER_PATHS = (
    "bin/cosmocc",
    "bin/apelink",
    "bin/x86_64-linux-cosmo-gcc",
    "bin/x86_64-unknown-cosmo-cc",
    "x86_64-linux-cosmo/lib/libcosmo.a",
)
REQUIRED_COMPILER_DIRECTORIES = ("include", "lib")


def sha256_stream(stream: BinaryIO) -> str:
    digest = hashlib.sha256()
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(block)
    return digest.hexdigest()


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return sha256_stream(stream)


def tree_sha256(root: Path) -> str:
    """Match the source toolchain's name/mode/content/link fingerprint."""
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
                raise ValueError(f"unsupported compiler SDK file type: {path}")
            digest.update((json.dumps(value, ensure_ascii=True) + "\n").encode())
            if stat.S_ISDIR(mode):
                visit(path)

    visit(root)
    return digest.hexdigest()


def open_regular(path: Path, label: str) -> BinaryIO:
    try:
        before = path.lstat()
    except OSError as exc:
        raise ValueError(f"{label} is unavailable: {path}: {exc}") from exc
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
        raise ValueError(f"{label} must be a regular file, not a symlink: {path}")
    try:
        stream = path.open("rb")
    except OSError as exc:
        raise ValueError(f"cannot open {label} {path}: {exc}") from exc
    opened = os.fstat(stream.fileno())
    if (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino):
        stream.close()
        raise ValueError(f"{label} changed while it was being opened: {path}")
    return stream


def read_json_bytes(payload: bytes, label: str) -> dict[str, Any]:
    try:
        value = json.loads(payload.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"invalid {label}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def read_json_file(path: Path, label: str) -> tuple[dict[str, Any], str]:
    with open_regular(path, label) as stream:
        payload = stream.read()
    return read_json_bytes(payload, label), hashlib.sha256(payload).hexdigest()


def read_rust_constant(source: str, name: str) -> str:
    marker = f"pub const {name}: &str ="
    if source.count(marker) != 1:
        raise ValueError(f"missing or ambiguous SDK compatibility pin: {name}")
    try:
        return json.loads(source.split(marker, 1)[1].split(";", 1)[0].strip())
    except (IndexError, json.JSONDecodeError) as exc:
        raise ValueError(f"invalid SDK compatibility pin: {name}") from exc


def compatibility_stamp(config: Path) -> str:
    source = config.read_text(encoding="utf-8")
    version = read_rust_constant(source, "COSMOCC_VERSION")
    digest = read_rust_constant(source, "COSMOCC_SHA256")
    if not all(part.isascii() and part.isdecimal() for part in version.split(".")):
        raise ValueError("invalid Cosmocc compatibility version")
    if not SHA256_RE.fullmatch(digest):
        raise ValueError("invalid Cosmocc compatibility SHA-256")
    return f"cosmocc-{version} sha256:{digest}\n"


def provenance_identity(value: dict[str, Any], label: str) -> dict[str, Any]:
    if value.get("schema_version") != 1:
        raise ValueError(f"{label} has an unsupported schema_version")
    commit = value.get("source_commit")
    dirty = value.get("source_worktree_dirty")
    compiler = value.get("inputs", {}).get("cosmocc") if isinstance(value.get("inputs"), dict) else None
    tree = compiler.get("tree_sha256") if isinstance(compiler, dict) else None
    verification = compiler.get("verification") if isinstance(compiler, dict) else None
    if not isinstance(commit, str) or not COMMIT_RE.fullmatch(commit):
        raise ValueError(f"{label} has an invalid source_commit")
    if type(dirty) is not bool:
        raise ValueError(f"{label} has an invalid source_worktree_dirty value")
    if not isinstance(tree, str) or not SHA256_RE.fullmatch(tree):
        raise ValueError(f"{label} has an invalid Cosmocc tree fingerprint")
    if not isinstance(verification, str) or not verification:
        raise ValueError(f"{label} has no Cosmocc verification description")
    return {
        "source_commit": commit,
        "source_worktree_dirty": dirty,
        "cosmocc_tree_sha256": tree,
        "cosmocc_verification": verification,
    }


def validate_pins(pins_path: Path, identity: dict[str, Any]) -> dict[str, Any]:
    pins, _ = read_json_file(pins_path, "Code compiler bootstrap pins")
    if pins.get("compiler_bootstrap_kind") != "local-cosmopolitan-sdk":
        raise ValueError("Code is not pinned to a local Cosmopolitan SDK bootstrap")
    if pins.get("compiler_bootstrap_source_commit") != identity["source_commit"]:
        raise ValueError("local compiler source commit does not match Code's bootstrap pin")
    if pins.get("compiler_bootstrap_cosmocc_tree_sha256") != identity["cosmocc_tree_sha256"]:
        raise ValueError("local compiler tree does not match Code's bootstrap pin")
    for field in (
        "compiler_bootstrap_archive_sha256",
        "compiler_bootstrap_extracted_tree_sha256",
    ):
        if not isinstance(pins.get(field), str) or not SHA256_RE.fullmatch(pins[field]):
            raise ValueError(f"Code has an invalid {field}")
    return pins


def safe_member_path(name: str) -> tuple[PurePosixPath, bool]:
    directory = name.endswith("/")
    stripped = name[:-1] if directory else name
    if not stripped or "\\" in stripped or "\0" in stripped:
        raise ValueError(f"unsafe archive path: {name!r}")
    path = PurePosixPath(stripped)
    if path.is_absolute() or any(part in ("", ".", "..") for part in path.parts):
        raise ValueError(f"unsafe archive path: {name!r}")
    return path, directory


def validate_symlink_target(member: PurePosixPath, target: str) -> None:
    if not target or "\\" in target or "\0" in target:
        raise ValueError(f"unsafe symlink target for {member}: {target!r}")
    target_path = PurePosixPath(target)
    if target_path.is_absolute():
        raise ValueError(f"unsafe symlink target for {member}: {target!r}")
    resolved = list(member.parent.parts)
    for part in target_path.parts:
        if part in ("", "."):
            continue
        if part == "..":
            if not resolved:
                raise ValueError(f"unsafe symlink target for {member}: {target!r}")
            resolved.pop()
        else:
            resolved.append(part)


def member_mode(info: zipfile.ZipInfo, directory: bool) -> tuple[str, int]:
    mode = (info.external_attr >> 16) & 0xFFFF
    kind = stat.S_IFMT(mode)
    permissions = stat.S_IMODE(mode)
    if directory or kind == stat.S_IFDIR:
        return "directory", permissions or 0o755
    if kind == stat.S_IFLNK:
        return "symlink", permissions or 0o777
    if kind in (0, stat.S_IFREG):
        return "file", permissions or 0o644
    raise ValueError(f"unsupported archive file type: {info.filename}")


def inspect_archive(archive: zipfile.ZipFile) -> tuple[list[tuple[zipfile.ZipInfo, PurePosixPath, str, int]], dict[str, Any]]:
    selected: list[tuple[zipfile.ZipInfo, PurePosixPath, str, int]] = []
    seen: set[PurePosixPath] = set()
    embedded: dict[str, Any] | None = None
    for info in archive.infolist():
        path, directory = safe_member_path(info.filename)
        if path in seen:
            raise ValueError(f"duplicate archive path: {path}")
        seen.add(path)
        kind, permissions = member_mode(info, directory)
        if kind == "symlink":
            try:
                target = archive.read(info).decode("utf-8")
            except UnicodeError as exc:
                raise ValueError(f"invalid symlink target for {path}") from exc
            validate_symlink_target(path, target)
        if path.as_posix() == METADATA_RESOURCE:
            if kind != "file":
                raise ValueError("embedded SDK provenance is not a regular file")
            embedded = read_json_bytes(archive.read(info), "embedded SDK provenance")
        if path.parts[0] in ALLOWED_COMPILER_ROOTS:
            selected.append((info, path, kind, permissions))
    if embedded is None:
        raise ValueError("local compiler archive has no embedded SDK provenance")
    selected_paths = {path.as_posix() for _, path, _, _ in selected}
    for required in REQUIRED_COMPILER_PATHS:
        if required not in selected_paths:
            raise ValueError(f"local compiler archive is missing {required}")
    for required in REQUIRED_COMPILER_DIRECTORIES:
        if not any(path == required or path.startswith(required + "/") for path in selected_paths):
            raise ValueError(f"local compiler archive is missing {required}")
    return selected, embedded


def extract_compiler(
    archive: zipfile.ZipFile,
    members: list[tuple[zipfile.ZipInfo, PurePosixPath, str, int]],
    destination: Path,
) -> None:
    destination.mkdir(mode=0o755)
    directories = sorted(
        (item for item in members if item[2] == "directory"),
        key=lambda item: (len(item[1].parts), item[1].as_posix()),
    )
    for _, relative, _, permissions in directories:
        path = destination.joinpath(*relative.parts)
        path.mkdir(parents=True, exist_ok=True)
        path.chmod(permissions)
    for info, relative, kind, permissions in sorted(members, key=lambda item: item[1].as_posix()):
        if kind == "directory":
            continue
        path = destination.joinpath(*relative.parts)
        path.parent.mkdir(parents=True, exist_ok=True)
        if kind == "symlink":
            path.symlink_to(archive.read(info).decode("utf-8"))
        else:
            with archive.open(info) as source, path.open("xb") as output:
                shutil.copyfileobj(source, output, length=1024 * 1024)
            path.chmod(permissions)


def compatibility_archive_payload(
    archive: zipfile.ZipFile,
    members: list[tuple[zipfile.ZipInfo, PurePosixPath, str, int]],
) -> bytes:
    """Build the symlink-only archive required by Rust-APE's repair probe."""
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as compatibility:
        for info, relative, kind, permissions in sorted(
            members, key=lambda item: item[1].as_posix()
        ):
            if kind != "symlink":
                continue
            link = zipfile.ZipInfo(relative.as_posix(), date_time=(1980, 1, 1, 0, 0, 0))
            link.create_system = 3
            link.external_attr = (stat.S_IFLNK | permissions) << 16
            compatibility.writestr(link, archive.read(info))
    return output.getvalue()


def write_atomic_bytes(path: Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def write_atomic(path: Path, payload: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def ensure_compatibility_archive(path: Path, payload: bytes) -> None:
    expected = hashlib.sha256(payload).hexdigest()
    if path.exists() or path.is_symlink():
        if path.is_symlink() or not path.is_file() or sha256(path) != expected:
            raise ValueError(
                "existing Rust-APE Cosmocc compatibility archive differs; "
                "choose a fresh COSMO_RUST_BUILD_ROOT"
            )
        return
    write_atomic_bytes(path, payload)


def validate_installed(compiler: Path, expected_tree: str) -> None:
    for relative in (*REQUIRED_COMPILER_PATHS, *REQUIRED_COMPILER_DIRECTORIES):
        path = compiler / relative
        if not (path.exists() or path.is_symlink()):
            raise ValueError(f"installed local compiler SDK is missing {relative}")
    for relative in ("bin/cosmocc", "bin/apelink", "bin/x86_64-linux-cosmo-gcc"):
        if not os.access(compiler / relative, os.X_OK):
            raise ValueError(f"installed local compiler is not executable: {relative}")
    actual_tree = tree_sha256(compiler)
    if actual_tree != expected_tree:
        raise ValueError(
            "compiler tree hash does not match local SDK provenance: "
            f"expected {expected_tree}, found {actual_tree}"
        )


def stage_local_sdk(
    archive_path: Path,
    provenance_path: Path,
    config_path: Path,
    pins_path: Path,
    sdk: Path,
) -> dict[str, Any]:
    external, external_sha = read_json_file(provenance_path, "local SDK provenance")
    identity = provenance_identity(external, "local SDK provenance")
    pins = validate_pins(pins_path, identity)
    stamp = compatibility_stamp(config_path)

    with open_regular(archive_path, "local compiler SDK archive") as stream:
        archive_sha = sha256_stream(stream)
        stream.seek(0)
        try:
            with zipfile.ZipFile(stream) as archive:
                members, embedded = inspect_archive(archive)
                embedded_identity = provenance_identity(embedded, "embedded SDK provenance")
                if embedded_identity != identity:
                    raise ValueError("embedded SDK provenance does not match external BUILD.json")
                if archive_sha != pins["compiler_bootstrap_archive_sha256"]:
                    raise ValueError(
                        "local compiler archive does not match Code's bootstrap pin: "
                        f"expected {pins['compiler_bootstrap_archive_sha256']}, found {archive_sha}"
                    )
                compatibility_payload = compatibility_archive_payload(archive, members)
                compatibility_name = stamp.split(" ", 1)[0] + ".zip"
                compatibility_path = sdk / "cache" / compatibility_name
                compatibility_sha = hashlib.sha256(compatibility_payload).hexdigest()

                record: dict[str, Any] = {
                    "schema_version": 1,
                    "kind": "local-cosmopolitan-sdk",
                    **identity,
                    "archive_path": str(archive_path.resolve()),
                    "archive_sha256": archive_sha,
                    "installed_tree_sha256": pins[
                        "compiler_bootstrap_extracted_tree_sha256"
                    ],
                    "compatibility_archive_sha256": compatibility_sha,
                    "provenance_path": str(provenance_path.resolve()),
                    "provenance_sha256": external_sha,
                    "upstream_compatibility_stamp": stamp.rstrip("\n"),
                }
                compiler = sdk / "vendor/cosmocc"
                stamp_path = sdk / "vendor/.stamps/cosmocc"
                record_path = sdk / "generated/local-cosmocc-bootstrap.json"
                existing = compiler.exists() or compiler.is_symlink() or stamp_path.exists() or record_path.exists()
                if existing:
                    try:
                        recorded = json.loads(record_path.read_text(encoding="utf-8"))
                    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
                        raise ValueError(
                            "existing compiler SDK is not the pinned local bootstrap; "
                            "choose a fresh COSMO_RUST_BUILD_ROOT"
                        ) from exc
                    mutable_record_fields = {
                        "archive_path",
                        "compatibility_archive_sha256",
                        "provenance_path",
                        "provenance_sha256",
                    }
                    recorded_identity = {
                        key: value
                        for key, value in recorded.items()
                        if key not in mutable_record_fields
                    }
                    expected_identity = {
                        key: value
                        for key, value in record.items()
                        if key not in mutable_record_fields
                    }
                    if recorded_identity != expected_identity or stamp_path.read_text(
                        encoding="utf-8"
                    ) != stamp:
                        raise ValueError(
                            "existing compiler SDK is not the pinned local bootstrap; "
                            "choose a fresh COSMO_RUST_BUILD_ROOT"
                        )
                    validate_installed(
                        compiler, pins["compiler_bootstrap_extracted_tree_sha256"]
                    )
                    ensure_compatibility_archive(compatibility_path, compatibility_payload)
                    if recorded != record:
                        write_atomic(
                            record_path, json.dumps(record, indent=2, sort_keys=True) + "\n"
                        )
                    return record

                vendor = sdk / "vendor"
                vendor.mkdir(parents=True, exist_ok=True)
                temporary_root = Path(tempfile.mkdtemp(prefix=".cosmocc-stage-", dir=vendor))
                staged = temporary_root / "compiler"
                try:
                    extract_compiler(archive, members, staged)
                    validate_installed(
                        staged, pins["compiler_bootstrap_extracted_tree_sha256"]
                    )
                    staged.replace(compiler)
                finally:
                    shutil.rmtree(temporary_root, ignore_errors=True)
        except zipfile.BadZipFile as exc:
            raise ValueError(f"invalid local compiler SDK archive: {archive_path}") from exc

    try:
        ensure_compatibility_archive(compatibility_path, compatibility_payload)
        write_atomic(stamp_path, stamp)
        write_atomic(record_path, json.dumps(record, indent=2, sort_keys=True) + "\n")
    except Exception:
        shutil.rmtree(compiler, ignore_errors=True)
        stamp_path.unlink(missing_ok=True)
        record_path.unlink(missing_ok=True)
        raise
    print(f"Installed local Cosmopolitan GCC bootstrap from {archive_path}")
    print(
        "Verified installed compiler tree SHA-256: "
        f"{pins['compiler_bootstrap_extracted_tree_sha256']}"
    )
    return record


def main() -> int:
    if len(sys.argv) != 6:
        print(
            "usage: stage_local_cosmocc.py ARCHIVE BUILD.json CONFIG.rs PINS.json SDK",
            file=sys.stderr,
        )
        return 2
    try:
        stage_local_sdk(*(Path(argument) for argument in sys.argv[1:]))
    except (OSError, ValueError, zipfile.BadZipFile) as exc:
        print(f"local compiler bootstrap failed: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
