# Cosmopolitan APE for Code-Mod Implementation Plan

> **For agentic workers:** Execute inline in the isolated `feat/cosmopolitan-code-ape` worktree. Keep each step reviewable and do not modify the original dirty checkout.

**Goal:** Build and validate the existing Code-Mod `code` CLI as one Cosmopolitan APE for Linux and Windows without weakening security behavior or changing native builds.

**Architecture:** Use the pinned Cosmopolitan `webgpu-cpu` source and its Rust-APE SDK to build the existing Rust workspace on Linux x86-64, package one APE, and run that exact artifact on Linux and Windows. Add host-aware adapters only after a baseline build identifies the concrete incompatibilities; preserve host sandbox guarantees and block agent execution where equivalent security cannot be established.

**Tech Stack:** Rust workspace (`code-rs`, Rust 1.93 native build), Rust-APE nightly SDK and patched `std`/`libc`, Cosmocc 4.0.2, Cosmopolitan `apelink`, GitHub Actions.

---

## Task 1: Reproduce the reference toolchain

**Files:**
- Read: `third_party/rust_ape/run.sh` from Cosmopolitan commit `c2a7b3f8c871e44ae1e3fc8db36759c7b1829326`
- Read: `third_party/rust_ape/README.cosmo` and its pinned Rust-APE SDK commit
- Output only: `o/cosmopolitan-source/`, `o/rust-ape/`, and the disposable workspace `o/cosmo-code-rs/`

- [x] Fetch only the pinned Rust-APE SDK directory from the reference commit into the ignored `o/cosmopolitan-source/` area; do not add generated toolchains or compiled outputs to Git.
- [x] From the repository root, run SDK setup with one Cargo job and its default x86-64 target:

```sh
CARGO_BUILD_JOBS=1 COSMO_RUST_BUILD_ROOT="$PWD/o/rust-ape" bash o/cosmopolitan-source/third_party/rust_ape/run.sh setup
```

- [ ] From `code-rs/`, run the direct baseline command against the native lockfile and capture the initial result:

```sh
CARGO_BUILD_JOBS=1 COSMO_RUST_BUILD_ROOT="$PWD/../o/rust-ape" bash ../o/cosmopolitan-source/third_party/rust_ape/run.sh cargo check --manifest-path Cargo.toml --locked -j1 -p code-cli --bin code
```

- [ ] If Cargo rejects the pinned `libc` patch because the native lockfile selects another version, preserve the tracked native `code-rs/Cargo.lock`, copy `code-rs/` to `o/cosmo-code-rs/`, and update only that disposable copy:

```sh
export CARGO_BUILD_JOBS=1
export CARGO_HOME="$PWD/o/rust-ape/toolchain/cargo"
export RUSTUP_HOME="$PWD/o/rust-ape/toolchain/rustup"
export RUSTUP_TOOLCHAIN=nightly-2026-07-28
"$CARGO_HOME/bin/cargo" --config "$PWD/o/rust-ape/sdk/generated/cargo.cosmo.toml" update --manifest-path "$PWD/o/cosmo-code-rs/Cargo.toml" -p libc --precise 0.2.189
```

- [ ] Re-run the one-job `cargo check --locked` from `o/cosmo-code-rs/` and record the first compiler/linker blocker. Do not modify application source during this task.

## Task 2: Establish a failing CLI portability smoke test

**Files:**
- Create: `scripts/cosmopolitan-code/test-artifact.sh`
- Test: `scripts/cosmopolitan-code/tests/test_artifact_contract.py`

- [ ] Write a failing Python test that rejects missing artifacts and checks that an artifact manifest records the pinned source SHA, target architecture, and output SHA-256.
- [ ] Run `python3 -m unittest discover -s scripts/cosmopolitan-code/tests -p 'test_*.py'` and confirm the expected missing-manifest failure.
- [x] Implement the smallest artifact test script and JSON manifest contract needed by the observed Rust-APE packaging output.
- [x] Re-run `python3 -m unittest discover -s scripts/cosmopolitan-code/tests -p 'test_*.py'` and require it to pass for both a valid temporary fixture and malformed/missing manifest cases.

## Task 3: Build and package the existing CLI

**Files:**
- Create: `scripts/build-cosmopolitan-code.sh`
- Create: `scripts/cosmopolitan-code/README.md`
- Modify only if demonstrated necessary: `code-rs/cli/Cargo.toml`, shared workspace manifests, and the exact crates named by compiler diagnostics
- Test: `scripts/cosmopolitan-code/tests/test_artifact_contract.py`

- [x] Add a build script that verifies the exact Cosmopolitan and Rust-APE pins, uses an output directory under `o/`, and invokes the existing `code-cli` workspace target with `CARGO_BUILD_JOBS=1` and `-j1`.
- [x] Use the Rust-APE SDK's actual linker/APE packaging path; do not substitute `mold` or GCC for Cosmocc/`apelink` unless an isolated compatibility test proves that the replacement preserves APE output.
- [ ] Resolve each build blocker narrowly. Add a regression test before changing runtime behavior, and leave native platform manifests and builds unchanged.
- [x] Produce a distinct APE artifact and manifest; never overwrite the native `code` executable.
- [x] Run the artifact contract test, `file`, Cosmopolitan `pecheck`, and the APE's `--version` and `--help` commands on the Linux host.

## Task 4: Add host-aware compatibility without reducing security

**Files to inspect before changing:**
- `code-rs/core/src/unified_exec/mod.rs`
- `code-rs/core/src/windows_sandbox.rs`
- `code-rs/linux-sandbox/src/lib.rs`
- `code-rs/utils/pty/src/lib.rs`
- `code-rs/keyring-store/src/lib.rs`
- `code-rs/core/src/http_client.rs`
- `code-rs/core/src/command_safety/is_safe_command.rs`
- `code-rs/arg0/src/lib.rs`
- `code-rs/package-manager/src/platform.rs`

- [ ] Add unit tests for an explicit runtime-host selector covering Linux and Windows decisions before wiring runtime detection into call sites.
- [ ] Use the pinned Rust-APE `ape` runtime API for host detection; retain compile-time `cfg` only for ABI and code that is truly target-specific.
- [ ] Route credentials, certificate roots, executable discovery, process/PTY behavior, and sandbox creation through host-correct implementations.
- [ ] Until a host sandbox adapter is proven equivalent, make agent command execution return an explicit unsupported error in the APE build instead of starting without the configured sandbox.
- [ ] Run focused tests for every changed boundary and re-run the baseline APE build after each group of fixes.

## Task 5: Validate one artifact on Linux and Windows

**Files:**
- Create: `.github/workflows/cosmopolitan-code-ape.yml`
- Modify: `.github/workflows/README.md`
- Test: `scripts/cosmopolitan-code/test-artifact.sh`

- [x] Configure the workflow to run only for `main`. Build and upload one APE artifact on a Linux x86-64 runner with the source and SDK pinned to immutable commits.
- [x] Download the same named artifact in Linux and Windows validation jobs; verify the recorded hash before execution.
- [ ] Test startup, `--version`, `--help`, TLS/network, credentials, PTY/process handling, and sandbox behavior as separate checks; fail closed if an expected security guarantee is missing.
- [ ] Run the artifact manifest and package checks locally, validate workflow YAML, and review that the native build workflows are unchanged.
- [x] Document the tested commands, supported host/architecture scope, experimental status, and any features blocked by missing equivalent runtime support.

## Audit notes — 2026-10-11

- Verified complete items above are based on successful GitHub Actions run `38159610363` from `Main` commit `c32df7a9b241e3c4c3643d41208e630d583dba66`, plus installed artifact manifest validation for `/var/home/jack/bin/code.com.json`.
- Items left unchecked were not independently verified in this audit, even where code may already exist.
- The Task 4 requirement to return an unsupported error for APE agent command execution conflicts with the later command-parity requirement. Current source intentionally restores normal command execution paths for the APE build; do not mark that Task 4 item complete without rewriting the plan to match the current parity decision.
- The workflow currently gates on `refs/heads/Main`, matching this repository's actual default branch name, rather than lowercase `main` as written in the original task wording.

## Completion criteria

- The isolated worktree produces a verified APE from the existing Code-Mod CLI.
- The exact same artifact passes Linux and Windows runtime checks.
- No sandbox bypass or credential/TLS downgrade is introduced.
- The main-only workflow preserves the native build/release paths.
- Any unsupported CLI surface is explicitly blocked and documented rather than silently degraded.
