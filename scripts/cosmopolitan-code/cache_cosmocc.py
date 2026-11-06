#!/usr/bin/env python3
"""Cache the exact official Cosmocc release required by the pinned Rust-APE SDK."""

from __future__ import annotations

import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from typing import Any

from artifact_contract import sha256


SHA256_RE = re.compile(r"[0-9a-f]{64}\Z")
COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")


def read_rust_constant(source: str, name: str) -> str:
    marker = f"pub const {name}: &str ="
    if source.count(marker) != 1:
        raise ValueError(f"Missing or ambiguous SDK pin: {name}")
    try:
        value = json.loads(source.split(marker, 1)[1].split(";", 1)[0].strip())
    except (IndexError, json.JSONDecodeError) as exc:
        raise ValueError(f"Invalid SDK pin: {name}") from exc
    if not isinstance(value, str):
        raise ValueError(f"Invalid SDK pin: {name}")
    return value


def read_release(config: Path, pins_path: Path) -> dict[str, str]:
    source = config.read_text(encoding="utf-8")
    version = read_rust_constant(source, "COSMOCC_VERSION")
    digest = read_rust_constant(source, "COSMOCC_SHA256")
    base_url = read_rust_constant(source, "COSMOCC_BASE_URL")
    if not all(part.isascii() and part.isdecimal() for part in version.split(".")):
        raise ValueError("Invalid pinned Cosmocc version")
    if not SHA256_RE.fullmatch(digest):
        raise ValueError("Invalid pinned Cosmocc SHA-256")
    if not base_url.startswith("https://") or not base_url.endswith("/"):
        raise ValueError("Invalid pinned Cosmocc download URL")

    try:
        pins: Any = json.loads(pins_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"Cannot read Code compiler bootstrap pins: {exc}") from exc
    if not isinstance(pins, dict):
        raise ValueError("Code compiler bootstrap pins must be a JSON object")

    name = f"cosmocc-{version}.zip"
    archive_url = base_url + name
    expected = {
        "compiler_bootstrap_kind": "official-cosmocc-release",
        "compiler_bootstrap_version": version,
        "compiler_bootstrap_archive_url": archive_url,
        "compiler_bootstrap_archive_sha256": digest,
    }
    for field, value in expected.items():
        if pins.get(field) != value:
            raise ValueError(f"Rust-APE {field} does not match Code's bootstrap pin")
    source_commit = pins.get("compiler_bootstrap_source_commit")
    if not isinstance(source_commit, str) or not COMMIT_RE.fullmatch(source_commit):
        raise ValueError("Code has an invalid compiler_bootstrap_source_commit")

    return {
        "version": version,
        "sha256": digest,
        "name": name,
        "archive_url": archive_url,
        "fallback_url": (
            f"https://github.com/jart/cosmopolitan/releases/download/{version}/{name}"
        ),
        "source_commit": source_commit,
    }


def cache_archive(config: Path, pins_path: Path, cache: Path) -> Path:
    release = read_release(config, pins_path)
    destination = cache / release["name"]
    if destination.exists() or destination.is_symlink():
        if (
            destination.is_symlink()
            or not destination.is_file()
            or sha256(destination) != release["sha256"]
        ):
            raise ValueError(
                f"Existing compiler cache fails SHA-256 verification: {destination}"
            )
        return destination

    if cache.is_symlink() or (cache.exists() and not cache.is_dir()):
        raise ValueError(f"Compiler cache must be a directory, not a symlink: {cache}")
    cache.mkdir(parents=True, exist_ok=True)
    for url in (release["archive_url"], release["fallback_url"]):
        with tempfile.NamedTemporaryFile(
            dir=cache, suffix=".part", delete=False
        ) as stream:
            partial = Path(stream.name)
        try:
            print(f"Fetching pinned compiler from {url}", file=sys.stderr, flush=True)
            try:
                subprocess.run(
                    [
                        "curl",
                        "--fail",
                        "--location",
                        "--silent",
                        "--show-error",
                        "--proto",
                        "=https",
                        "--proto-redir",
                        "=https",
                        "--connect-timeout",
                        "20",
                        "--max-time",
                        "600",
                        "--output",
                        str(partial),
                        url,
                    ],
                    check=True,
                )
            except subprocess.CalledProcessError as error:
                print(
                    f"Compiler download failed ({error.returncode}): {url}",
                    file=sys.stderr,
                )
                continue
            if sha256(partial) != release["sha256"]:
                raise ValueError(
                    f"Compiler download fails pinned SHA-256 verification: {url}"
                )
            partial.replace(destination)
            print(
                f"Verified compiler SHA-256: {release['sha256']}",
                file=sys.stderr,
                flush=True,
            )
            return destination
        finally:
            partial.unlink(missing_ok=True)
    raise RuntimeError("Pinned compiler download failed at both official locations")


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: cache_cosmocc.py CONFIG.rs PINS.json CACHE", file=sys.stderr)
        return 2
    try:
        cache_archive(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]))
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"official compiler cache failed: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
