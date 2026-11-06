# Cosmopolitan Code APE (experimental)

This adds a separate x86-64 Cosmopolitan APE build of the existing `code` CLI.
It does not replace or alter the native Linux and Windows build outputs.

## Build

Use the root `make build` target or the command entry point with Python 3, Git,
and GitHub CLI (`gh`). Compilation and tests for this repository run only in
GitHub Actions against the exact committed source revision. Local build and test
targets fail closed instead of falling back to this machine or another host.

- `build-auto`: Dispatch the active `Main` GitHub workflow for the exact
  committed revision.
- `build-action`: Dispatch the `Main` workflow, wait for Linux and Windows
  startup checks, and download the single executable and manifest. Push the
  commit first.
- `build-local`: Refuse with an explanation that local compilation is disabled.
- `test`: Verify the source commit, SDK pins, size, hash, and executable startup
  on the current host.
- `run`: Verify the manifest and forward CLI arguments to the executable.
- `publish-prerelease`: Publish an executable identical to the artifact tested
  on Linux and Windows.
- `publish-release`: Promote an existing prerelease with that same executable
  to the latest stable release.

make build

python3 scripts/cosmopolitan-code/commands.py build-auto

python3 scripts/cosmopolitan-code/commands.py build-action

python3 scripts/cosmopolitan-code/commands.py test

python3 scripts/cosmopolitan-code/commands.py run -- --version

python3 scripts/cosmopolitan-code/commands.py publish-prerelease --tag YOUR-TAG

python3 scripts/cosmopolitan-code/commands.py publish-release --tag YOUR-TAG

Each command accepts `--output DIRECTORY`; the default is
`o/cosmopolitan-code/<source-commit>`. `COSMOPOLITAN_CODE_OUTPUT` also selects
the artifact directory. Publishing requires a clean checkout and a successful
Actions run for the current commit, and compares the executable with the
downloaded Actions artifact before changing any release. Stable publication
also verifies the executable already uploaded to the prerelease.

On Linux, the shell entry point accepts the same commands, for example:

sh scripts/build-cosmopolitan-code.sh build-action

The low-level shell build remains an implementation detail of the GitHub Actions
runner. Do not invoke it locally for development validation.

The script verifies the pinned Cosmopolitan fork and Rust-APE source revisions,
then builds the fork's `toolchain` target for x86-64. Its runtime, C++ library,
startup objects, headers, APE loader, and linker tools replace the bootstrap SDK
runtime before any Code compilation. The temporary compiler is the official
Cosmocc 4.0.2 release archive, pinned by exact URL and SHA-256; the final runtime
still comes from Code's pinned fork source.

The script creates a disposable source snapshot and applies the APE-specific
dependency adjustments and lockfile. Actions builds archive the checked-out
commit and may use all available GitHub-hosted runner workers. The native
`code-rs/Cargo.lock` remains unchanged. The source-runtime SDK and Cargo outputs
use a pin-addressed
`o/rust-ape-source-<cosmopolitan-commit>-<rust-ape-commit>/` directory, separate
from older or incompatible runtime caches. The release build disables fat LTO
and keeps one codegen unit.

The APE also carries a deterministic Linux x86-64 Camoufox runtime ZIP as its
final byte range. Clean Actions runners download the versioned GitHub release
asset pinned in `pins.json`, then fail closed unless its URL, byte size,
SHA-256, file count, tree SHA-256, embedded component identities, and ZIP
inventory all match. Existing cache entries receive the same validation and a
tampered entry is preserved and rejected rather than silently replaced.

When the local `o/camoufox-runtime-prototype` staging tree exists, the low-level
packager regenerates the same deterministic archive before compilation.
`COSMOPOLITAN_BROWSER_RUNTIME_SOURCE` selects an equivalent staged tree,
`COSMOPOLITAN_BROWSER_RUNTIME_ARCHIVE` selects an explicitly supplied packaged
ZIP, and `COSMOPOLITAN_BROWSER_RUNTIME_CACHE` changes the download cache
directory. These are development inputs; the GitHub workflow uses the pinned
release asset. The packager fixes ZIP timestamps and ordering, excludes browser
profiles, and records the size, mode, SHA-256, and symlink target of every
payload entry.

The default-branch GitHub workflow checks out the full Cosmopolitan source at
the revision pinned in `pins.json` and configures an APE interpreter on its
disposable runner for the fork's build helpers.

Compiler bootstrap downloads only the official Cosmocc release URL pinned in
`pins.json`, with the matching official GitHub release as a network fallback.
Both locations must provide the exact SHA-256 required by the pinned Rust-APE
configuration. Existing cache entries are reused only after the same hash check;
tampered or unidentified entries are preserved and rejected instead of silently
replaced. After Rust-APE installs the archive, the build verifies the complete
compiler-tree fingerprint and compatibility stamp before recording provenance.

The compiler identity is recorded separately from the source-built fork runtime
in the final artifact manifest. Release validation rejects unidentified or dirty
compiler inputs.

The output is placed in `o/cosmopolitan-code/`:

- `code.com` — the x86-64 APE artifact.
- `code.com.json` — source pins, architecture, artifact size, SHA-256, and hashes
  identifying the source-built runtime, headers, loader, and linker. The
  manifest also records the embedded browser archive size and SHA-256, runtime
  tree SHA-256, file count, and Camoufox, Mesa, and LLVM pins.

The build refuses to overwrite an existing output directory. Set
`COSMOPOLITAN_CODE_OUTPUT` to a new directory to create another artifact.

## Validation

The `Test explicit project scope and memory Markdown` and existing APE workflow
jobs run the Python and Rust validation on GitHub Actions. Local `make check`,
`make test`, and focused Rust test targets intentionally refuse to run.

The build runs `file`, Cosmopolitan `pecheck`, `code.com --version`, and
`code.com --help` before publishing its output directory. To repeat those
checks:

sh scripts/cosmopolitan-code/test-artifact.sh o/cosmopolitan-code/code.com o/cosmopolitan-code/code.com.json

The manifest validator also opens the appended ZIP, verifies that it is the
exact artifact suffix, checks its inventory against the embedded runtime
manifest, and rechecks the pinned component identities. On first browser use,
Code extracts that same inventory under `$CODE_HOME/browser/runtime` using a
lock and staging directory. Every cached file, mode, relative symlink, size,
and SHA-256 is validated before reuse; an invalid cache is quarantined and
replaced atomically. `CODE_BROWSER_RUNTIME_DIR` can point development builds at
an unpacked runtime with the same manifest contract.

The `Main`-branch or manually dispatched workflow is restricted to `Main`. It
first builds a small source-runtime probe on Linux, verifies its identity and
checksum, and runs that identical probe on Windows. Only then does it build and
test Code and run the identical Code artifact on Windows. A runtime failure
therefore blocks the expensive full build without bypassing any final
verification.

The runtime stage is also available as `sh scripts/build-cosmopolitan-runtime.sh`
and writes `o/cosmopolitan-runtime/`. The full build uses the same preparation
code and resource limits. A cached probe is reused only when its source identity,
checksum, and SDK runtime record match; an incomplete or stale cache fails closed.

## Supported scope and limitations

- Source-built integration uses the fork's adapted Rust-APE shims, not the
  unmodified 4.0.2-specific snapshot. Constant tables are regenerated from the
  source-built headers, and every shim is compiled before the full Code build.
  A small Rust probe checks files, threads, subprocesses, and loader symbols;
  the same probe also checks native child exit codes in the Windows job.
  Stock headers, dummy globals, and disabled checks are not valid substitutes
  for the source-built runtime.
- The artifact architecture is x86-64. ARM64 is not included in this build.
- The Rust-APE SDK is experimental; this CLI port is an experimental
  pre-release, not a production support claim.
- Native file pickers, file-manager reveal, and clipboard-image paste are
  unavailable in the APE profile; users can enter paths directly.
- External process tools use the same command execution paths as native builds,
  including the four-worker shared subagent limit and cancellation of running
  child processes. Changes to sandboxing or PTY behavior must be validated on
  both Linux and Windows for the APE artifact.
- This workflow checks startup on Windows; it does not establish Windows
  sandbox, credential-store, system-certificate, or PTY parity.
