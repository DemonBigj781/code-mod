#!/usr/bin/env python3
"""Verify the source identity and checksum of the small cross-host probe."""

from pathlib import Path
import sys

from artifact_contract import PINS_PATH, SHA256_RE, read_json, sha256, validate_runtime_metadata


def verify_probe(directory: Path, source_commit: str) -> None:
    if (directory / "code-source-commit").read_text().strip() != source_commit:
        raise ValueError("Runtime probe was built from a different Code source revision")
    validate_runtime_metadata(read_json(directory / "fork-runtime.json", "runtime record"), read_json(PINS_PATH, "pins"))
    artifact = directory / "runtime-smoke.com"
    if artifact.is_symlink() or not artifact.is_file():
        raise ValueError("Runtime probe must be a regular file")
    record = (directory / "runtime-smoke.sha256").read_text().split()
    if len(record) != 2 or record[1] != artifact.name or not SHA256_RE.fullmatch(record[0]):
        raise ValueError("Invalid runtime probe checksum record")
    if record[0] != sha256(artifact):
        raise ValueError("Runtime probe SHA-256 mismatch")


if __name__ == "__main__":
    verify_probe(Path(sys.argv[1]), sys.argv[2])
    print("Source runtime probe identity and checksum passed")
