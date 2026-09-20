# Colibri OpenAI-Compatible Provider Implementation Plan

<!-- markdownlint-disable MD013 -->

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Colibri as a built-in local OpenAI-compatible provider that participates in existing `/v1/models` discovery while retaining the host's current disk-swap configuration.

**Architecture:** Register Colibri through the existing `ModelProviderInfo::direct_openai_compatible` constructor and route it through the current Chat Completions and remote-model catalog paths. Code will not launch Colibri, manage model files, or alter swap/zram; documentation will provide a conservative `coli serve --ram 6` example for the Steam Deck.

**Tech Stack:** Rust, `ModelProviderInfo`, OpenAI Chat Completions compatibility, remote `/v1/models` discovery, Markdown documentation, Cargo tests with `CARGO_BUILD_JOBS=1`.

**Design:** `docs/superpowers/specs/2026-09-18-colibri-openai-provider-design.md`

> **Reconciled 2026-09-20:** The implementation was completed in the earlier
> repair session but this checklist was never updated. A current-branch audit
> found the planned constants, built-in provider registration, core exports,
> TUI catalog classification, tests, and operator documentation in place. The
> full core provider group passes 14/14 and the direct-provider TUI group passes
> 4/4 with one compiler worker. The one-thread CLI build also covers both
> affected crates. `zramctl` remains empty, `/var/home/swapfile` remains the
> active 16 GiB swap tier, and the zram generator configuration remains
> disabled. This reconciliation does not claim that a live Colibri endpoint was
> launched or exercised.

**Worktree safety:** The branch already contains extensive protected uncommitted work, including changes in `code-rs/core/src/lib.rs`, `code-rs/tui/src/direct_provider.rs`, and `code-rs/config.md`. Implementation MUST use surgical patches, MUST inspect each final hunk, MUST NOT stage or commit unrelated work, and MUST leave implementation uncommitted if an isolated commit cannot be proven safe.

---

## File Map

- `code-rs/core/src/model_provider_info.rs`: define the Colibri provider constants, register the provider, and test its exact contract.
- `code-rs/core/src/lib.rs`: re-export the provider identifier and base URL for consumers.
- `code-rs/tui/src/direct_provider.rs`: classify the built-in Colibri provider as a remote model-catalog source and extend discovery coverage.
- `code-rs/config.md`: document explicit configuration, independent server startup, `--ram 6`, disk-swap behavior, and the zram exclusion.

No Colibri source, model files, systemd units, `/etc` files, swap devices, or zram devices will be modified.

### Task 1: Register the Core Provider

**Files:**

- Modify: `code-rs/core/src/model_provider_info.rs:861`
- Test: `code-rs/core/src/model_provider_info.rs:1313`

- [x] **Step 1: Add a failing provider-contract test**

Add this test beside the existing built-in provider tests:

```rust
#[test]
fn built_in_model_providers_include_colibri() {
    let providers = built_in_model_providers(None);
    let colibri = providers
        .get(COLIBRI_PROVIDER_ID)
        .expect("Colibri provider should exist");

    assert_eq!(colibri.name, "Colibri");
    assert_eq!(
        colibri.base_url.as_deref(),
        Some(COLIBRI_API_BASE_URL),
    );
    assert_eq!(colibri.wire_api, WireApi::Chat);
    assert!(colibri.env_key.is_none());
    assert!(!colibri.requires_openai_auth);
}
```

- [x] **Step 2: Run the focused test and verify RED**

Run:

```text
CARGO_BUILD_JOBS=1 cargo test -p code-core built_in_model_providers_include_colibri -- --nocapture
```

Working directory: `code-rs`

Expected: compilation fails because `COLIBRI_PROVIDER_ID` and
`COLIBRI_API_BASE_URL` do not exist, or the assertion fails because the provider
is absent.

- [x] **Step 3: Define constants and register the provider**

Add these constants near the existing local-provider constants:

```rust
pub const COLIBRI_PROVIDER_ID: &str = "colibri";
pub const COLIBRI_API_BASE_URL: &str = "http://127.0.0.1:8000/v1";
```

Add this entry to the array in `built_in_model_providers` before the OSS
provider entry:

```rust
(
    COLIBRI_PROVIDER_ID,
    P::direct_openai_compatible(
        "Colibri",
        COLIBRI_API_BASE_URL,
        None,
        WireApi::Chat,
    ),
),
```

Do not add custom headers, authentication, retry values, or Colibri-specific
request handling.

- [x] **Step 4: Run the focused test and verify GREEN**

Run:

```text
CARGO_BUILD_JOBS=1 cargo test -p code-core built_in_model_providers_include_colibri -- --nocapture
```

Expected: one matching test passes with zero failures.

- [x] **Step 5: Inspect the surgical diff**

Run:

```text
git diff --check -- code-rs/core/src/model_provider_info.rs
git diff -- code-rs/core/src/model_provider_info.rs
```

Expected: only the constants, provider entry, and focused test are new.

### Task 2: Expose Colibri To TUI Model Discovery

**Files:**

- Modify: `code-rs/core/src/lib.rs:103`
- Modify: `code-rs/tui/src/direct_provider.rs:71`
- Test: `code-rs/tui/src/direct_provider.rs:248`

- [x] **Step 1: Re-export the core constants**

Add these exports beside the existing model-provider exports:

```rust
pub use model_provider_info::COLIBRI_API_BASE_URL;
pub use model_provider_info::COLIBRI_PROVIDER_ID;
```

- [x] **Step 2: Extend the existing discovery test before implementation**

Add this assertion to `built_in_remote_catalog_providers_are_discoverable`:

```rust
assert!(is_model_catalog_provider_definition(
    code_core::COLIBRI_PROVIDER_ID,
    providers
        .get(code_core::COLIBRI_PROVIDER_ID)
        .expect("Colibri provider"),
));
```

- [x] **Step 3: Run the focused TUI test and verify RED**

Run:

```text
CARGO_BUILD_JOBS=1 cargo test -p code-tui built_in_remote_catalog_providers_are_discoverable -- --nocapture
```

Working directory: `code-rs`

Expected: the new Colibri assertion fails because the provider is not yet
classified as a model-catalog source.

- [x] **Step 4: Add Colibri to catalog-provider classification**

Change the explicit provider match to include Colibri:

```rust
matches!(
    provider_id,
    code_common::model_presets::OPENROUTER_PROVIDER_ID
        | code_core::STABLEHORDE_PROVIDER_ID
        | code_core::COLIBRI_PROVIDER_ID
) || is_direct_provider_definition(provider_id, provider)
```

Do not special-case Colibri anywhere else; the existing remote-model manager
must perform `GET /v1/models` and cache handling.

- [x] **Step 5: Run the focused TUI test and verify GREEN**

Run:

```text
CARGO_BUILD_JOBS=1 cargo test -p code-tui built_in_remote_catalog_providers_are_discoverable -- --nocapture
```

Expected: the discovery test passes with the OpenRouter, Stable Horde,
Colibri, and OpenAI expectations intact.

- [x] **Step 6: Inspect protected-file diffs**

Run:

```text
git diff --check -- code-rs/core/src/lib.rs code-rs/tui/src/direct_provider.rs
git diff -- code-rs/core/src/lib.rs code-rs/tui/src/direct_provider.rs
```

Expected: the Colibri exports, match arm, and assertion are the only additions
made by this task. Existing unrelated hunks remain untouched.

### Task 3: Document Server And Swap Operation

**Files:**

- Modify: `code-rs/config.md:51`

- [x] **Step 1: Add a Colibri provider example after the Ollama example**

Add this configuration:

```toml
model_provider = "colibri"
model = "local-colibri"

[model_providers.colibri]
name = "Colibri"
base_url = "http://127.0.0.1:8000/v1"
wire_api = "chat"
```

- [x] **Step 2: Document the independent server command**

Add this command as a single-line example:

```text
coli serve --model /var/home/jack/projects/AI_Project/models/colibri/default --model-id local-colibri --ram 6 --host 127.0.0.1 --port 8000
```

State explicitly:

- `--ram 6` is a conservative Steam Deck starting point, not a universal optimum.
- Colibri remains independently managed and MUST be running before Code refreshes models.
- The existing 16 GiB `/var/home/swapfile` remains the overflow tier.
- zram remains disabled and is not required by the integration.
- Model files must stay on normal storage rather than a RAM-backed filesystem.

- [x] **Step 3: Validate the documentation diff**

Run:

```text
git diff --check -- code-rs/config.md
rg -n 'Colibri|coli serve|swapfile|zram' code-rs/config.md
```

Expected: no whitespace errors and all required operational terms are present.

### Task 4: Run Final Verification

**Files:**

- Verify: `code-rs/core/src/model_provider_info.rs`
- Verify: `code-rs/core/src/lib.rs`
- Verify: `code-rs/tui/src/direct_provider.rs`
- Verify: `code-rs/config.md`

- [x] **Step 1: Run core provider tests**

Run:

```text
CARGO_BUILD_JOBS=1 cargo test -p code-core model_provider_info::tests -- --nocapture
```

Expected: all model-provider tests pass. If an unrelated pre-existing assertion
fails, record the exact failure and run the Colibri-specific test separately;
do not change unrelated provider behavior.

- [x] **Step 2: Run direct-provider TUI tests**

Run:

```text
CARGO_BUILD_JOBS=1 cargo test -p code-tui direct_provider::tests -- --nocapture
```

Expected: all direct-provider tests pass.

- [x] **Step 3: Compile affected crates with one worker**

Run:

```text
CARGO_BUILD_JOBS=1 cargo check -p code-core -p code-tui
```

Expected: both crates compile successfully with no new errors.

- [x] **Step 4: Prove no system memory configuration changed**

Run:

```text
zramctl
swapon --show
sed -n '1,40p' /etc/systemd/zram-generator.conf
```

Expected: no zram device is active, `/var/home/swapfile` remains active, and the
zram generator override remains disabled.

- [x] **Step 5: Review the final scoped diff**

Run:

```text
git diff --check -- code-rs/core/src/model_provider_info.rs code-rs/core/src/lib.rs code-rs/tui/src/direct_provider.rs code-rs/config.md
git status --short
```

Expected: no whitespace errors, no generated artifacts, and no files outside
the four implementation targets changed by this feature.

- [x] **Step 6: Preserve the operator's existing worktree**

Do not create an implementation commit if staging any target path would include
pre-existing user changes. Report the verified source changes as uncommitted and
leave branch consolidation to the existing release workflow.
