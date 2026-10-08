# Cosmopolitan APE for Code-Mod

**Status:** Approved for an experimental port

## Goal

Build the existing Code-Mod `code` CLI as a Cosmopolitan Actually Portable
Executable (APE) that can run on Linux and Windows, while leaving the native
Linux and Windows builds intact.

## Evidence and pinned starting point

- Reference repository: `DemonBigj781/cosmopolitan`, branch `webgpu-cpu`,
  inspected at commit `c2a7b3f8c871e44ae1e3fc8db36759c7b1829326`.
- The reference branch vendors `starccy/rust-ape` at
  `0e2d0a89f4c089c7f21377a9c7ce51a6e82c2791` and Cosmocc 4.0.2.
- The reference Rust SDK rebuilds patched Rust `std` and `libc`; ordinary
  Linux-target Rust objects are not made portable merely by linking them with
  Cosmopolitan.
- The reference branch demonstrates a single APE tested on both operating
  systems. Its WebGPU sample is not evidence that the Code-Mod CLI or its
  security-sensitive features work under APE.
- Code-Mod is a multi-crate Rust workspace pinned to Rust 1.93.0. Its CLI
  depends on process execution, PTYs, platform credential stores, TLS roots,
  and platform-specific sandbox implementations.

## Approach

1. Keep the normal native builds and release artifacts unchanged.
2. Add an opt-in, reproducible Cosmopolitan build path, initially for x86-64
   and hosted by Linux x86-64.
3. Build the existing `code` binary through the pinned Rust-APE SDK, then
   package it as an APE using the SDK's Cosmopolitan linker/packager. Produce
   one artifact and test the same bytes on Linux and Windows.
4. Introduce runtime-host detection only where Cosmopolitan's Linux-personality
   target would otherwise select the wrong behavior. Use the Rust-APE host API;
   do not infer the executing host from Rust `cfg(target_os = ...)`.
5. Preserve security behavior. Do not route agent execution to an unsandboxed
   subprocess or claim sandbox support unless the host-specific restrictions
   are independently tested on both Linux and Windows.
6. Treat unsupported host integrations as explicit port blockers. Do not
   silently disable them or alter native-platform behavior to make the APE
   build pass.

## Scope

The initial target is the existing `code` CLI as a single x86-64 APE, with
startup and noninteractive commands verified on both Linux and Windows. The
port must then establish host-correct behavior for process execution, terminal
and PTY handling, credential storage, TLS trust, and sandbox selection before
the APE can be considered feature-complete.

The native builds remain the default and are not replaced. ARM64, macOS,
embedded WebGPU/lavapipe, and general-purpose Cosmopolitan toolchain support are
outside this first target.

## Security and compatibility gates

- A successful link is not a successful port. The binary must start and return
  the expected version/help output on Linux and Windows.
- Agent command execution must fail closed until a secure, host-appropriate
  sandbox path is available. No implicit unsandboxed fallback is allowed.
- Credential storage must not select a Linux D-Bus backend when running on
  Windows, and TLS must use an intentional trust-root policy on both hosts.
- PTY-dependent execution must be verified independently; a missing PTY must
  not be disguised as successful interactive execution.
- All native Linux and Windows build/test paths must remain unchanged.

## Verification

The validation sequence is:

1. Pin and bootstrap the reference Rust-APE toolchain in the isolated build
   output directory.
2. Attempt a single-job build of the existing `code` binary without changing
   application code; record compiler and linker blockers.
3. Add narrowly-scoped compatibility changes with tests before each behavior
   change.
4. Package an APE, inspect its format, and record its SHA-256.
5. Run that exact artifact on Linux and Windows. Validate CLI startup, version,
   help, config, network/TLS, credentials, PTY/process behavior, and sandbox
   behavior separately.
6. Keep the CI workflow restricted to `main`; it builds once on Linux and
   executes the uploaded artifact on both supported operating systems.

The `rust-ape` integration is experimental. Until every security and runtime
gate passes, the artifact and CI must be labeled experimental and must not be
presented as a production replacement for native builds.
