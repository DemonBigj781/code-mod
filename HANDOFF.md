<!-- markdownlint-disable MD013 MD060 -->

# Project Handoff: Code Camoufox/Rustwright Cosmopolitan APE

> **Status:** The requested browser/display work was validated in the portable
> `code.com` artifact on 2026-11-04. Explicit project scopes, bounded Git
> snapshots/reviews, native memories, and guarded image attachments are committed
> to `Main`. Compilation and test evidence for the current revision is provided by
> the `Cosmopolitan Code APE` and `Provider Integration Tests` GitHub Actions
> workflows.

## Requested Outcome

The active project is Code, a Rust terminal coding agent with a portable
Cosmopolitan APE packaging path. The completed work covers:

1. Fixing the built-in headless browser's black-screen/rendering failure.
2. Rendering terminal images as colored text cells rather than persistent terminal
   graphics overlays.
3. Replacing Chromium/CDP with embedded Camoufox controlled through native Juggler.
4. Exposing browser automation through the Rustwright MCP surface without Node or
   Playwright.
5. Packaging Camoufox, Mesa, and LLVM inside the Cosmopolitan APE.
6. Preserving embedded credential storage and KWallet migration behavior.
7. Adding popup/tab control and aggregate file-input limits.
8. Producing and validating the complete portable `code.com` artifact.

Do not resume the unrelated Servo experiment or turn this into a separate browser
product. The deliverable is Code's built-in browser tool.

## Repository and Safety Boundaries

- User-facing path: `/var/home/jack/projects/code`
- Resolved repository root:
  `/run/media/jack/5e90bafb-087a-4fbc-8e81-0a09dee62978/projects/AI_Project/code`
- Branch: `Main`
- Current revision: inspect `git rev-parse HEAD` and the corresponding
  `Cosmopolitan Code APE` workflow run rather than relying on a copied SHA.
- Current state: inspect `git status --short`; the unrelated pre-existing deletion
  of `build-fast.zsh.backup` remains intentionally uncommitted.
- Nested repositories: none found by the handoff inventory.
- `build-fast.zsh.backup` was already deleted before this work; do not restore or
  otherwise alter it implicitly.
- `o/servo-ape-probe` is unrelated retained work; do not modify or delete it.
- Do not create a branch, push, or open a PR unless the operator explicitly requests
  it.
- Preserve unrelated dirty changes and the existing `code-rs/target/dev-fast` cache.
- Use `apply_patch` for repository edits.
- Never compile, build, or run tests for this repository on the local system.
  Validate the exact intended source through GitHub Actions. If the workflow is
  inadequate, repair it instead of falling back to a local or alternate remote
  build host.
- Do not contact remote build hosts or the live Project Zomboid panel host implicitly.

## Explicit Project Scope and Git Snapshots

Code now has a persistent project registry at `CODE_HOME/projects.json`.
`/project add <master|group|single> <dir> <name> <git_search_depth>` registers a
scope; `/project use <name>`, `/project <name>`, and the direct `/<name>` alias
select it. Registered scopes replace recursive startup-CWD discovery for Git
snapshots, undo previews, and automatic review. A `single` entry targets exactly
one repository, while `group` and `master` entries use bounded discovery.

Changing the active scope clears queued snapshot work and detaches any stale
in-flight job, so the previous project cannot leave snapshotting permanently
busy. Results that return for a stale scope are discarded.

The integration committed to `Main` on 2026-11-08 extends that contract with:

- `code project ...` registry management and `code --project NAME` startup
  selection for interactive, `exec`, `auto`, and `review` sessions;
- a native **Settings → Projects** page for listing, adding, selecting, and
  removing bounded scopes;
- master-as-fallback activation precedence, so adding a broad master does not
  displace an active group or single scope;
- changed-path-only temporary-index updates for ghost snapshots, avoiding a
  full tracked-file rehash in massive repositories;
- a native read-only `memories` tool that resolves the current published
  generation on every call, plus cross-working-directory session-history
  regression coverage;
- shared image normalization for user/function-output images and `image_view`, so
  oversized attachments are downscaled below the provider's 30,000-patch limit
  before they enter pending model input.

The repository policy requires the exact committed revision to pass the relevant
GitHub Actions workflows before it is described as build/test validated. Always
inspect the current revision's workflow runs rather than relying on this document
for a copied validation result.

## Architecture and Build Path

The canonical product path is:

```text
root makefile
  -> scripts/cosmopolitan-code/commands.py
  -> require a clean Main checkout matching origin/Main
  -> dispatch .github/workflows/cosmopolitan-code-ape.yml
  -> GitHub-hosted runner checks out the exact committed revision
  -> scripts/build-cosmopolitan-code.sh
  -> Rust/Cosmopolitan build
       -> Code Rust workspace
       -> native rustwright-mcp
            -> vendored camoufox-juggler
            -> embedded Camoufox runtime ZIP
            -> embedded Mesa/LLVM software-rendering runtime
       -> embedded keyring and TUI Halfblocks rendering
  -> Cosmopolitan link/package
  -> code.com + code.com.json
  -> first-run runtime extraction under CODE_HOME
  -> Camoufox child controlled through Juggler pipes
```

Important paths:

| Path | Role |
| --- | --- |
| `makefile` | Canonical Actions-only operator entry point. |
| `code-rs/rustwright-mcp/` | Native Rustwright MCP and Camoufox runtime integration. |
| `code-rs/vendor/camoufox-juggler/` | Pinned Juggler attachment/popup support. |
| `code-rs/tui/` | Colored text-cell terminal image rendering. |
| `code-rs/keyring-store/` | Embedded credentials and keyring migration. |
| `scripts/cosmopolitan-code/` | Build, packaging, manifest, and runtime validators. |
| `.github/workflows/cosmopolitan-code-ape.yml` | Clean-revision scope tests, APE build, and smoke workflow. |
| `.github/workflows/test-only.yml` | Manual `Main`-only source and contract test helper without APE packaging. |
| `.github/workflows/check-only.yml` | Manual `Main`-only product/test-target `cargo check` helper. |
| `.github/workflows/compile-only.yml` | Manual `Main`-only APE compile/package helper that uploads the `code.com` artifact without running test gates. |
| `.github/workflows/pre-release-only.yml` | Manual or reusable prerelease publisher for the exact validated `code.com` artifact. |
| `.github/workflows/release-only.yml` | Manual or reusable stable promotion of an existing matching prerelease. |
| `.github/workflows/code-comeplete-pipeline.yml` | Complete check/test/cross-platform build pipeline; prerelease is the default release type and `release` is the secondary switch. |
| `.github/workflows/create-build-images.yml` | Publishes the reusable Linux x86-64 APE builder image to GHCR. |
| `.github/workflows/update-internal-codex-repo.yml` | Manual entry point that dispatches the existing conflict-aware upstream merge workflow. |
| `o/` | Local artifacts, caches, reports, screenshots, and logs. |

Do not replace the canonical APE build with direct Cargo. Compilation and tests are
GitHub-Actions-only for this repository.

Linux compilation jobs run in the digest-pinned GHCR builder image published by
`.github/workflows/create-build-images.yml`. The image carries the stable OS packages
and Rust toolchain, while the GitHub Actions-backed `sccache` layer reuses both Rust
and C/C++ compilation outputs across clean hosted runners.

The APE workflow uploads the packaged artifact before running downloaded-artifact
smoke jobs. Linux and Windows therefore validate the same uploaded bytes, and a late
embedded-browser failure cannot prevent retrieval of the build artifact and manifest.
An x86-64 Termux Docker image also runs those uploaded bytes in the Termux Bionic
userland. The container does not include Android runtime components, so this
portability smoke does not replace the separate native `target_os = "android"` build
described under `code-rs/docs/`.

Rust-APE bootstraps with the official Cosmocc 4.0.2 release from the exact HTTPS
URL and SHA-256 pinned in `scripts/cosmopolitan-code/pins.json`; the matching
official GitHub release is only a network fallback for the same bytes. The build
records the official tag commit, archive identity, compatibility stamp, and fresh
installed-tree fingerprint before replacing the bootstrap runtime with the runtime
built from Code's pinned `DemonBigj781/cosmopolitan` fork.

Clean Actions runners obtain the deterministic schema-2 browser runtime from the
versioned GitHub release asset pinned in `pins.json`. The cache helper accepts only
HTTPS and verifies the archive byte size, SHA-256, embedded component identities,
file count, tree SHA-256, and ZIP inventory before the expensive Rust compile. The
current contract is:

- Release tag: `cosmopolitan-browser-runtime-v1`
- Asset: `browser-runtime_x86_64.zip`
- Archive size: `813836364` bytes
- Archive SHA-256:
  `d1b2c46f7916e4cdd07bb4ee09ba1931f9901f5095e06a2536eef9b016dd866f`
- Runtime files: `1364`
- Runtime tree SHA-256:
  `1acea8b7ead597408baf475432c1abcfdf18319b62e6288bc9e68dcbe8eecc89`

The preserved `o/camoufox-runtime-prototype` tree regenerates exactly those bytes
with `package-browser-runtime.py`. The older local
`o/camoufox-runtime/browser-runtime.zip` is a schema-1 archive and is not a valid
input for the current contract.

## Historical Portable Artifact

This artifact predates the Actions-only policy and remains historical acceptance
evidence. Do not use its local build procedure as the current validation path.

- Artifact:
  `o/cosmopolitan-code-camoufox-postfix-final-20261104/code.com`
- Manifest:
  `o/cosmopolitan-code-camoufox-postfix-final-20261104/code.com.json`
- Build log:
  `o/cosmopolitan-code-camoufox-postfix-final-20261104-build.log`
- Build status:
  `o/cosmopolitan-code-camoufox-postfix-final-20261104-build.status` (`0`)
- Artifact size: `937961589` bytes
- Artifact SHA-256:
  `98926561ae71de1294485614b963aef5461091132c7a5c18760b47c302fc96e5`
- Manifest SHA-256:
  `a203569f1a17bf8002adccd8786a72a8e71a10b483c8c3f9eda7a7ae0a0c44d9`
- Reported version: `code 0.153.4`
- Source snapshot SHA-256:
  `57c023c39dacd00faa2e35ff97702a468c68b530edec0a0017a77efa741d8e98`
- Target architecture: `x86_64`
- Build profile: release, one codegen unit, LTO off, optimization level 2

Embedded browser runtime:

- Camoufox: `152.0.4 beta.28`
- Camoufox source commit:
  `0583c3ec94f5a9df5cb2d09553fbfe80589b6e2d`
- camoufox-rust/Juggler revision:
  `933ae2e8ba7a00e254b4d4075eb0e85a527289fc`
- Automation contract: `juggler-pipe` through native `rustwright-mcp`
- Runtime files: `1364`
- Runtime archive SHA-256:
  `d1b2c46f7916e4cdd07bb4ee09ba1931f9901f5095e06a2536eef9b016dd866f`
- Runtime tree SHA-256:
  `1acea8b7ead597408baf475432c1abcfdf18319b62e6288bc9e68dcbe8eecc89`
- Mesa: `26.2.4`
- LLVM: `22.1`

The manifest deliberately records `source_clean: false`; this is a development
artifact built from the requested dirty worktree snapshot.

## Final Runtime Acceptance Evidence

The acceptance harness is
`scripts/cosmopolitan-code/check-rustwright-mcp.py`. It validates MCP initialization
and the 27-tool catalog, Camoufox launch, navigation, viewport resize, interaction,
evaluation, fingerprint surfaces, popup list/select/close, valid PNG dimensions, and
non-uniform image content.

### First-Use Extraction

- Fresh home:
  `o/camoufox-postfix-final-home-first-20261104`
- Report: `o/camoufox-postfix-final-first-report.json`
- Screenshot: `o/camoufox-postfix-final-first.png`
- Result: passed from an empty `CODE_HOME`.
- Popup API: passed.
- Screenshot: `1024x768`, `4383` unique colors, dominant-color ratio
  `0.013296763102213541`.
- Screenshot SHA-256:
  `5a809d7a2313420b58acfb6c88a73691f86cc44a022b87b39a0619e198ac3c6a`
- Visual inspection: readable colored gradient page, button, and activated result;
  not black or uniform.

### Cache Reuse

- Reused the same extracted runtime and `CODE_HOME`.
- Report: `o/camoufox-postfix-final-reuse-report.json`
- Screenshot: `o/camoufox-postfix-final-reuse.png`
- Result: passed in about 18 seconds without re-extraction.
- Popup API and all 27 MCP tools remained available.

### Corruption Recovery

- Corrupted only the isolated extracted
  `browser/runtime/camoufox/properties.json` test copy.
- Corrupted SHA-256:
  `f417cf55edab24168bd272e9e56b23ece9effe4b03ffae95a8e21662c0cbe5e5`
- Repaired/manifest SHA-256:
  `c0573d7b47b3f4f217e459916f0feba461aba3816699727f216779a2c4988018`
- Report: `o/camoufox-postfix-final-recovery-report.json`
- Screenshot: `o/camoufox-postfix-final-recovery.png`
- Result: the APE re-extracted the invalid runtime and the full browser gate passed.

One recovery attempt reached the harness's 120-second response timeout after repair
and left a stale Firefox profile lock. That profile was preserved at
`browser/profile-timeout-backup-20261104`; a fresh profile using the repaired runtime
passed in about 20 seconds. No browser logic was changed in response to the
contaminated profile.

### Stale Extraction-Lock Recovery

- Fresh isolated home:
  `o/camoufox-postfix-final-home-stale-lock-20261104`
- Seeded `browser/runtime.extract.lock` with a timestamp from 2000.
- Report: `o/camoufox-postfix-final-stale-lock-report.json`
- Screenshot: `o/camoufox-postfix-final-stale-lock.png`
- Result: the stale lock was removed, the runtime extracted, the lock was cleaned up,
  and the full popup/fingerprint/screenshot gate passed.

### Aggregate File-Input Rejection

- Report: `o/camoufox-postfix-final-aggregate-report.json`
- Workspace:
  `o/camoufox-postfix-final-aggregate-workspace-20261104`
- Inputs: two sparse `11534336`-byte files with zero allocated data blocks.
- `browser_file_upload` rejected them with
  `file upload files exceed 20971520 bytes in total`.
- `browser_drop` rejected them with
  `drop files exceed 20971520 bytes in total`.
- The metadata-based aggregate checks ran before file contents were read.

### Baseline Discrimination

The known pre-fix artifact
`o/cosmopolitan-code-camoufox-complete/code.com` fails the stabilized popup fixture
with `popup-created page was not exposed by browser_tabs`, while current native source
and the final APE pass. Preserve the baseline evidence:

- `o/pre-fix-stable-popup-smoke.log`
- `o/pre-fix-stable-popup-home-20261104/`

## Verification Matrix

The historical verification below predates the GitHub-Actions-only policy. Do
not repeat these compilation or test commands locally.

| Verification | Result |
| --- | --- |
| `make check` | 70 Python packaging tests passed. |
| Focused `code-rustwright-mcp` library suite | 66 tests passed. |
| `make test-keyring` | 2 migration/vault tests passed. |
| TUI `terminal_images_are_forced_to_color_text_cells` | Passed. |
| `python3 -m py_compile scripts/cosmopolitan-code/check-rustwright-mcp.py` | Passed. |
| Native current-source browser smoke | Passed popup/fingerprint/non-black screenshot gate. |
| Canonical `make build-local` | Completed successfully; status `0`. |
| `test-development-artifact.sh` | Passed manifest contract, `pecheck`, version, and help checks. |
| Final APE first extraction | Passed. |
| Final APE cache reuse | Passed. |
| Final APE corruption recovery | Passed. |
| Final APE stale-lock recovery | Passed. |
| Final APE aggregate upload/drop rejection | Passed. |

The canonical build also ran its serial validation phase, packaged the schema-2
browser runtime, checked the manifest contract, and passed the native history worker
validation. Existing deprecation/dead-code/future-incompatibility warnings were not
failures.

## Source and Documentation Changes

The completed implementation includes:

- aggregate file-size enforcement for both upload and drop paths in
  `code-rs/rustwright-mcp/src/actor.rs`;
- popup attachment delivery through `code-rs/vendor/camoufox-juggler/` and actor page
  synchronization;
- schema-2 embedded runtime validation/extraction in
  `code-rs/rustwright-mcp/src/camoufox_runtime.rs`;
- Halfblocks terminal image rendering in `code-rs/tui/src/tui.rs`;
- the complete browser runtime acceptance gate in
  `scripts/cosmopolitan-code/check-rustwright-mcp.py`;
- Camoufox/Juggler packaging and manifest support in `scripts/cosmopolitan-code/`;
- embedded Camoufox wording in `README.md` and `docs/config.md`;
- updated Camoufox tasks in
  `docs/changes/0002-code-feature-parity-roadmap.md`;
- embedded-browser CI smoke coverage in
  `.github/workflows/cosmopolitan-code-ape.yml`.
- persistent project registry and bounded repository scopes in
  `code-rs/core/src/project_registry.rs` and `code-rs/tui/src/chatwidget/`;
- CommonMark blockquote rendering for generated memory summaries in
  `code-rs/core/src/memories/storage.rs`;
- fail-closed Actions-only entry points in `makefile` and
  `scripts/cosmopolitan-code/commands.py`.

The broader feature-parity roadmap remains `in-progress`, so its documentation index
status intentionally did not change.

## Validation Commands

- Dispatch the committed exact source: `make build`.
- The `Test explicit project scope and memory Markdown` Actions job runs the
  registry, native Memories, bounded discovery, changed-path snapshot,
  Projects settings, root CLI project-selection, undo-scope, background-review,
  alias, and generated-memory-Markdown regressions.
- The remaining workflow jobs run packaging checks, Rustwright tests, the APE build,
  Linux browser acceptance, and Windows runtime checks.
- Local build and test targets intentionally refuse to run.

## Requirement Coverage

| Requirement | Evidence | Status |
| --- | --- | --- |
| Colored text-cell images | Halfblocks implementation and focused test | Validated |
| Native Rustwright MCP | 66 tests and 27-tool final APE catalog | Validated |
| Embedded Camoufox/Juggler | Manifest plus first-use and reuse smokes | Validated |
| Software rendering | Embedded Mesa/LLVM and non-black PNGs | Validated |
| Popup/tab control | Baseline failure plus native/final APE runtime passes | Validated |
| Runtime cache/recovery | Reuse, corruption, and stale-lock reports | Validated |
| Aggregate upload/drop limit | Unit tests plus final APE sparse-file probe | Validated |
| Keyring/credential path | Migration/vault tests | Validated |
| Portable Cosmopolitan APE | Canonical build, manifest, artifact validator, runtime smokes | Validated |
| Public docs and CI | README/config/roadmap/workflow reconciled | Validated |

## Remaining Limitations and Next Actions

- The final artifact is a dirty-worktree development build, not a release from a clean
  commit.
- The new project-scope implementation is not yet remotely validated because it is
  still uncommitted. GitHub Actions cannot execute these exact bytes until the operator
  authorizes committing and pushing the intended worktree state.
- The embedded browser runtime target is x86_64 Linux. No other architecture is
  claimed by this artifact.
- No functional blocker remains for the requested black-screen, popup, runtime,
  aggregate-input, or terminal-image behavior.
- If the operator wants publication, the next distinct task is to review and organize
  the large dirty worktree, commit the intended changes, and optionally prepare a PR.
  Do not do that implicitly.

## Last Verified

- Date: 2026-11-04
- Branch/revision: `Main` at
  `fef5163e5444ecf1fb30c4ae6ed04be91ae7f7b7`
- Active build/browser processes after validation: none
- Final artifact SHA-256:
  `98926561ae71de1294485614b963aef5461091132c7a5c18760b47c302fc96e5`
- Completion gate: final portable APE passed first extraction, cache reuse, corruption
  recovery, stale-lock recovery, popup control, fingerprint checks, non-black PNG
  capture, MCP catalog validation, and aggregate upload/drop rejection.
