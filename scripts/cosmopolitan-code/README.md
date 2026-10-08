# Cosmopolitan Code APE (experimental)

This adds a separate x86-64 Cosmopolitan APE build of the existing `code` CLI.
It does not replace or alter the native Linux and Windows build outputs.

## Build

Run from a clean, committed repository checkout on Linux x86-64:

```sh
sh scripts/build-cosmopolitan-code.sh
```

The script verifies the pinned Cosmopolitan and Rust-APE source revisions,
creates a disposable copy of the committed Rust workspace, applies only the
APE-specific dependency adjustments, and builds with one Cargo job. The Rust
toolchain and Cosmocc SDK stay under the ignored `o/` directory; the system
toolchain is not changed. The release build disables fat LTO and keeps one
codegen unit so it stays within the configured local build budget.

Local builds require the sparse Cosmopolitan source checkout at
`o/cosmopolitan-source` pinned to the revision in `pins.json`. The main-only
GitHub workflow checks it out automatically.

The output is placed in `o/cosmopolitan-code/`:

- `code.com` — the x86-64 APE artifact.
- `code.com.json` — source pins, architecture, artifact size, and SHA-256.

The build refuses to overwrite an existing output directory. Set
`COSMOPOLITAN_CODE_OUTPUT` to a new directory to create another artifact.

## Validation

Run the manifest contract tests with:

```sh
python3 -m unittest discover -s scripts/cosmopolitan-code/tests -p 'test_*.py'
```

The build runs `file`, Cosmopolitan `pecheck`, `code.com --version`, and
`code.com --help` before publishing its output directory. To repeat those
checks:

```sh
sh scripts/cosmopolitan-code/test-artifact.sh o/cosmopolitan-code/code.com o/cosmopolitan-code/code.com.json
```

The workflow is restricted to `main`; it builds once on Linux and runs the same
artifact's startup checks on Linux and Windows. A feature-branch pre-release is
not evidence of a completed Windows runtime check.

## Supported scope and limitations

- The artifact architecture is x86-64. ARM64 is not included in this build.
- The Rust-APE SDK is experimental; this CLI port is an experimental
  pre-release, not a production support claim.
- Native file pickers, file-manager reveal, and clipboard-image paste are
  unavailable in the APE profile; users can enter paths directly.
- External process tools are deliberately rejected in the APE profile until
  host-specific sandboxing and PTY behavior are validated. Native builds retain
  their normal process execution behavior, including the four-worker shared
  subagent limit and cancellation of running child processes.
- This workflow checks startup on Windows; it does not establish Windows
  sandbox, credential-store, system-certificate, or PTY parity.
