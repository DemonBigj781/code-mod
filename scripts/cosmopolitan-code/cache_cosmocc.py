#!/usr/bin/env python3
"""Cache the pinned compiler archive, with an official release fallback."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile

from artifact_contract import sha256


def read_pins(config: Path) -> tuple[str, str, str]:
    source = config.read_text(encoding="utf-8")

    def constant(name: str) -> str:
        marker = f"pub const {name}: &str ="
        if source.count(marker) != 1:
            raise ValueError(f"Missing or ambiguous SDK pin: {name}")
        return json.loads(source.split(marker, 1)[1].split(";", 1)[0].strip())

    version = constant("COSMOCC_VERSION")
    digest = constant("COSMOCC_SHA256")
    base_url = constant("COSMOCC_BASE_URL")
    if not all(part.isascii() and part.isdecimal() for part in version.split(".")):
        raise ValueError("Invalid pinned Cosmocc version")
    if len(digest) != 64 or any(char not in "0123456789abcdef" for char in digest):
        raise ValueError("Invalid pinned Cosmocc SHA-256")
    if not base_url.startswith("https://") or not base_url.endswith("/"):
        raise ValueError("Invalid pinned Cosmocc download URL")
    return version, digest, base_url


def cache_archive(config: Path, cache: Path) -> Path:
    version, expected, base_url = read_pins(config)
    name = f"cosmocc-{version}.zip"
    destination = cache / name
    if destination.exists() or destination.is_symlink():
        if destination.is_symlink() or not destination.is_file() or sha256(destination) != expected:
            raise ValueError(f"Existing compiler cache fails SHA-256 verification: {destination}")
        return destination

    cache.mkdir(parents=True, exist_ok=True)
    urls = [base_url + name, f"https://github.com/jart/cosmopolitan/releases/download/{version}/{name}"]
    for url in urls:
        with tempfile.NamedTemporaryFile(dir=cache, suffix=".part", delete=False) as stream:
            partial = Path(stream.name)
        try:
            print(f"Fetching pinned compiler from {url}", flush=True)
            try:
                subprocess.run([
                    "curl", "--fail", "--location", "--silent", "--show-error",
                    "--proto", "=https", "--proto-redir", "=https",
                    "--connect-timeout", "20", "--max-time", "600",
                    "--output", str(partial), url,
                ], check=True)
            except subprocess.CalledProcessError as error:
                print(f"Compiler download failed ({error.returncode}): {url}", file=sys.stderr)
                continue
            if sha256(partial) != expected:
                raise ValueError(f"Compiler download fails pinned SHA-256 verification: {url}")
            partial.replace(destination)
            print(f"Verified compiler SHA-256: {expected}", flush=True)
            return destination
        finally:
            partial.unlink(missing_ok=True)
    raise RuntimeError("Pinned compiler download failed at both official locations")


if __name__ == "__main__":
    cache_archive(Path(sys.argv[1]), Path(sys.argv[2]))
