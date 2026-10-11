# Cosmopolitan Code APE (experimental)

This adds a separate x86-64 Cosmopolitan APE build of the existing `code` CLI.
It does not replace or alter the native Linux and Windows build outputs.

## Build

Run from a clean, committed repository checkout. Use the command entry point
with Python 3, Git, and GitHub CLI (`gh`). On Windows, use `python` instead of
`python3` if that is the installed command name.

| Command | Behavior |
| --- | --- |
| `build-action` | Dispatch the `Main` workflow, wait for Linux and Windows startup checks, and download the single executable and manifest. Push the commit first. |
| `build-local` | Fetch the pinned SDK sources if absent and compile on Linux x86-64 with one worker. |
| `test` | Verify the source commit, SDK pins, size, hash, and executable startup on the current host. |
| `run` | Verify the manifest and forward CLI arguments to the executable. |
| `publish-prerelease` | Publish an executable identical to the artifact tested on Linux and Windows. |
| `publish-release` | Promote an existing prerelease with that same executable to the latest stable release. |

python3 scripts/cosmopolitan-code/commands.py build-action

python3 scripts/cosmopolitan-code/commands.py build-local

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

Calling the shell entry point without a command retains its original local
build behavior and output directory.

The script verifies the pinned Cosmopolitan and Rust-APE source revisions,
creates a disposable copy of the committed Rust workspace, applies only the
APE-specific dependency adjustments and the APE-specific lockfile, then builds
with one Cargo job. The native `code-rs/Cargo.lock` remains unchanged. The Rust
toolchain and Cosmocc SDK stay under the ignored `o/` directory; the system
toolchain is not changed. The release build disables fat LTO and keeps one
codegen unit so it stays within the configured local build budget.

Local builds use the sparse Cosmopolitan source checkout at
`o/cosmopolitan-source` pinned to the revision in `pins.json`. `build-local`
fetches it if absent; an existing checkout must match the pin and be clean.
The default-branch GitHub workflow checks it out automatically.

The output is placed in `o/cosmopolitan-code/`:

- `code.com` — the x86-64 APE artifact.
- `code.com.json` — source pins, architecture, artifact size, and SHA-256.

The build refuses to overwrite an existing output directory. Set
`COSMOPOLITAN_CODE_OUTPUT` to a new directory to create another artifact.

## Validation

Run the manifest and command tests with:

python3 -m unittest discover -s scripts/cosmopolitan-code/tests -p 'test_*.py'

The build runs `file`, Cosmopolitan `pecheck`, `code.com --version`, and
`code.com --help` before publishing its output directory. To repeat those
checks:

sh scripts/cosmopolitan-code/test-artifact.sh o/cosmopolitan-code/code.com o/cosmopolitan-code/code.com.json

The manually dispatched workflow is restricted to `Main`; it builds once on Linux and runs the same
artifact's startup checks on Linux and Windows. A feature-branch pre-release is
not evidence of a completed Windows runtime check.

## Supported scope and limitations

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
