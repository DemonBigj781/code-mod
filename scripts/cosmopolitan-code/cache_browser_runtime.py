#!/usr/bin/env python3
"""Cache and verify the exact browser runtime embedded in the Code APE."""

from __future__ import annotations

import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tempfile
from typing import Any
from urllib.parse import urlsplit

from artifact_contract import ArtifactContractError, inspect_browser_runtime_archive


SHA256_RE = re.compile(r"[0-9a-f]{64}\Z")


def read_distribution(pins_path: Path) -> dict[str, Any]:
    try:
        pins: Any = json.loads(pins_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"Cannot read browser runtime pins: {exc}") from exc
    if not isinstance(pins, dict) or not isinstance(pins.get("browser_runtime"), dict):
        raise ValueError("Browser runtime pins must contain a browser_runtime object")

    runtime = pins["browser_runtime"]
    url = runtime.get("archive_url")
    if not isinstance(url, str):
        raise ValueError("Browser runtime archive_url is missing")
    parsed = urlsplit(url)
    if parsed.scheme != "https" or not parsed.netloc or parsed.query or parsed.fragment:
        raise ValueError("Browser runtime archive_url must be a plain HTTPS URL")
    name = PurePosixPath(parsed.path).name
    if not name.endswith(".zip") or name in {"", ".", ".."}:
        raise ValueError("Browser runtime archive_url must name a ZIP archive")

    sha256 = runtime.get("archive_sha256")
    if not isinstance(sha256, str) or not SHA256_RE.fullmatch(sha256):
        raise ValueError("Browser runtime archive_sha256 is invalid")
    size = runtime.get("archive_size_bytes")
    if type(size) is not int or size <= 0:
        raise ValueError("Browser runtime archive_size_bytes is invalid")
    file_count = runtime.get("archive_file_count")
    if type(file_count) is not int or file_count <= 0:
        raise ValueError("Browser runtime archive_file_count is invalid")
    tree_sha256 = runtime.get("archive_tree_sha256")
    if not isinstance(tree_sha256, str) or not SHA256_RE.fullmatch(tree_sha256):
        raise ValueError("Browser runtime archive_tree_sha256 is invalid")

    return {
        "pins": pins,
        "url": url,
        "name": name,
        "sha256": sha256,
        "size": size,
        "file_count": file_count,
        "tree_sha256": tree_sha256,
    }


def verify_archive(path: Path, distribution: dict[str, Any]) -> None:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"Browser runtime archive is not a regular file: {path}")
    if path.stat().st_size != distribution["size"]:
        raise ValueError("Browser runtime archive size does not match the pin")
    try:
        metadata = inspect_browser_runtime_archive(path, distribution["pins"])
    except (OSError, ArtifactContractError) as exc:
        raise ValueError(f"Browser runtime archive is invalid: {exc}") from exc
    expected = {
        "archive_sha256": distribution["sha256"],
        "archive_size_bytes": distribution["size"],
        "file_count": distribution["file_count"],
        "tree_sha256": distribution["tree_sha256"],
    }
    for field, value in expected.items():
        if metadata.get(field) != value:
            raise ValueError(f"Browser runtime {field} does not match the pin")


def cache_archive(pins_path: Path, cache: Path) -> Path:
    distribution = read_distribution(pins_path)
    destination = cache / distribution["name"]
    if destination.exists() or destination.is_symlink():
        try:
            verify_archive(destination, distribution)
        except ValueError as exc:
            raise ValueError(
                f"Existing browser runtime cache fails verification: {destination}: {exc}"
            ) from exc
        return destination

    if cache.is_symlink() or (cache.exists() and not cache.is_dir()):
        raise ValueError(f"Browser runtime cache must be a directory, not a symlink: {cache}")
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=cache, suffix=".part", delete=False) as stream:
        partial = Path(stream.name)
    try:
        print(
            f"Fetching pinned browser runtime from {distribution['url']}",
            file=sys.stderr,
            flush=True,
        )
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
                "1800",
                "--output",
                str(partial),
                distribution["url"],
            ],
            check=True,
        )
        try:
            verify_archive(partial, distribution)
        except ValueError as exc:
            raise ValueError(
                f"Browser runtime download fails the pinned runtime contract: {exc}"
            ) from exc
        partial.replace(destination)
        print(
            f"Verified browser runtime SHA-256: {distribution['sha256']}",
            file=sys.stderr,
            flush=True,
        )
        return destination
    finally:
        partial.unlink(missing_ok=True)


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: cache_browser_runtime.py PINS.json CACHE", file=sys.stderr)
        return 2
    try:
        archive = cache_archive(Path(sys.argv[1]), Path(sys.argv[2]))
    except (OSError, subprocess.CalledProcessError, ValueError) as exc:
        print(f"browser runtime cache failed: {exc}", file=sys.stderr)
        return 1
    print(archive)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
