#!/usr/bin/env python3
"""Build the pinned fork's x86-64 runtime and install it into the Rust SDK."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from artifact_contract import sha256


LIBRARIES = {
    "cosmopolitan.a": "libcosmo.a",
    "third_party/libcxx/libcxx.a": "libcxx.a",
    "libc/crt/crt.o": "crt.o",
    "libc/crt/crtfastmath.o": "crtfastmath.o",
    "ape/ape.o": "ape.o",
    "ape/ape-no-modify-self.o": "ape-no-modify-self.o",
    "ape/ape.lds": "ape.lds",
}
TOOLS = ("march-native", "ar", "mktemper", "fixupobj", "zipcopy", "apelink", "pecheck", "tlscc")
EMPTY_LIBRARIES = ("c", "dl", "gcc_s", "m", "crypt", "pthread", "resolv", "rt", "unwind", "gomp", "stdc++")


def tree_hash(directory: Path) -> str:
    import hashlib
    digest = hashlib.sha256()
    for path in sorted(directory.rglob("*")):
        if path.is_file():
            digest.update(path.relative_to(directory).as_posix().encode() + b"\0")
            digest.update(sha256(path).encode() + b"\0")
    return digest.hexdigest()


def runtime_hashes(compiler: Path) -> dict[str, str]:
    library = compiler / "x86_64-linux-cosmo/lib"
    return {
        "libraries_sha256": tree_hash(library),
        "headers_sha256": tree_hash(compiler / "include"),
        "libcosmo_sha256": sha256(library / "libcosmo.a"),
        "libcxx_sha256": sha256(library / "libcxx.a"),
        "loader_sha256": sha256(compiler / "bin/ape-x86_64.elf"),
        "apelink_sha256": sha256(compiler / "bin/apelink"),
        "tlscc_sha256": sha256(compiler / "bin/tlscc"),
    }


def assemble(source: Path, stage: Path) -> None:
    output = source / "o/x86_64"
    library = stage / "lib"
    library.mkdir()
    for original, name in LIBRARIES.items():
        shutil.copy2(output / original, library / name)
    for name in EMPTY_LIBRARIES:
        (library / f"lib{name}.a").write_bytes(b"!<arch>\n")

    include = stage / "include"
    shutil.copytree(source / "libc/isystem", include)
    shutil.copytree(source / "libc/integral", include / "libc/integral", dirs_exist_ok=True)
    for name in (source / "o/cosmocc.h.txt").read_text().split():
        relative = Path(name)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"Invalid generated header path: {name}")
        destination = include / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / relative, destination)

    binary = stage / "bin"
    shutil.copytree(source / "tool/cosmocc/bin", binary, symlinks=True)
    for tool in TOOLS:
        name = "ar.ape" if tool == "ar" else tool
        shutil.copy2(output / "tool/build" / tool, binary / name)
    shutil.copy2(output / "ape/ape.elf", binary / "ape-x86_64.elf")


def build_runtime(source: Path, sdk: Path, commit: str, jobs: int) -> dict:
    compiler = sdk / "vendor/cosmocc"
    stamp = sdk / "generated/fork-runtime.json"
    if stamp.exists():
        recorded = json.loads(stamp.read_text())
        if recorded.get("source_commit") != commit or recorded.get("hashes") != runtime_hashes(compiler):
            raise ValueError("Source-built runtime differs from its record; use a fresh build directory")
        return recorded
    if jobs < 1 or (jobs != 1 and not (
        os.environ.get("GITHUB_ACTIONS") == "true"
        and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
    )):
        raise ValueError("Parallel compilation is allowed only on GitHub-hosted runners")
    if not (source / "libc/isystem").is_dir():
        raise ValueError("Building the fork requires a full source checkout, not only third_party/rust_ape")
    actual = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    dirty = subprocess.check_output(["git", "-C", str(source), "status", "--porcelain"], text=True).strip()
    if actual != commit or dirty:
        raise ValueError("The fork must be clean and match the pinned commit")

    subprocess.run([
        "make", "-C", str(source), f"-j{jobs}", "MODE=x86_64", "XARGS=xargs -P1 -rs8000",
        "toolchain", "o/cosmocc.h.txt", *[f"o/x86_64/tool/build/{tool}" for tool in TOOLS],
    ], check=True)
    # Assemble first; never combine new headers with incomplete runtime outputs.
    with tempfile.TemporaryDirectory(prefix="fork-runtime-", dir=sdk) as temporary:
        stage = Path(temporary)
        assemble(source, stage)
        for relative, replacement in (("include", "include"), ("x86_64-linux-cosmo/lib", "lib")):
            destination = compiler / relative
            backup = destination.with_name(destination.name + "-bootstrap")
            if backup.exists():
                raise ValueError("Incomplete runtime installation; use a fresh build directory")
            destination.rename(backup)
            (stage / replacement).rename(destination)
        for path in (stage / "bin").iterdir():
            path.replace(compiler / "bin" / path.name)
    fixtures = Path(__file__).with_name("fixtures")
    for extension, driver in (("c", "cc"), ("cc", "c++")):
        executable = sdk / "generated" / f"fork-runtime-{extension}.com"
        subprocess.run([
            str(compiler / "bin" / f"x86_64-unknown-cosmo-{driver}"),
            str(fixtures / f"fork_runtime_smoke.{extension}"), "-o", str(executable),
        ], check=True)
        subprocess.run(["sh", str(executable)], check=True)
    record = {"source_built": True, "source_commit": commit, "mode": "x86_64", "hashes": runtime_hashes(compiler)}
    stamp.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    return record


if __name__ == "__main__":
    build_runtime(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve(), sys.argv[3], int(sys.argv[4]))
