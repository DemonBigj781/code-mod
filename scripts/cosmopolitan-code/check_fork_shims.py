#!/usr/bin/env python3
"""Compile every Rust-APE shim against the source-built fork before Code."""

from pathlib import Path
import shlex
import subprocess
import sys
import tempfile


def check_shims(sdk: Path) -> None:
    compiler = sdk / "vendor/cosmocc/bin/x86_64-unknown-cosmo-cc"
    sources = sorted((sdk / "shim").rglob("*.c"))
    if not sources:
        raise ValueError("Rust-APE shim sources are missing")
    failed = []
    with tempfile.TemporaryDirectory(prefix="fork-shim-check-", dir=sdk) as temporary:
        for index, source in enumerate(sources):
            flags = []
            for line in source.read_text().splitlines():
                if line.startswith("// cflags:"):
                    flags.extend(shlex.split(line.removeprefix("// cflags:")))
            if source.name == "dlmalloc.c":
                flags = ["-D_COSMO_SOURCE", "-ffreestanding", "-fdata-sections", "-ffunction-sections", "-O3", "-mgeneral-regs-only"]
            if "-D_COSMO_SOURCE" in flags:
                flags.append("-DSYSDEBUG=1")
            result = subprocess.run([
                str(compiler), "-c", "-O2", "-fno-stack-protector", *flags,
                "-o", str(Path(temporary) / f"{index}.o"), str(source),
            ])
            if result.returncode:
                failed.append(source.relative_to(sdk).as_posix())
    if failed:
        raise RuntimeError("Source-built fork is incompatible with Rust-APE shims: " + ", ".join(failed))
    print(f"All {len(sources)} Rust-APE shims compile against the source-built fork", flush=True)


if __name__ == "__main__":
    check_shims(Path(sys.argv[1]).resolve())
