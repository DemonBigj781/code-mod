# Colibri OpenAI-Compatible Provider Design

<!-- markdownlint-disable MD013 -->

## Goal

Code MUST expose Colibri as a built-in local OpenAI-compatible model provider
without taking ownership of Colibri installation, model downloads, process
lifecycle, or host swap configuration.

The integration MUST target a separately started Colibri server at
`http://127.0.0.1:8000/v1`. The host's existing 16 GiB disk swapfile MUST remain
the memory-pressure fallback. Code MUST NOT enable or configure zram as part of
this feature.

## Provider Definition

The built-in provider map MUST contain a provider with the identifier
`colibri` and these defaults:

| Field | Value |
| --- | --- |
| Display name | `Colibri` |
| Base URL | `http://127.0.0.1:8000/v1` |
| Wire API | Chat Completions |
| Required API key | None |
| OpenAI account authentication | Disabled |

The provider MUST reuse Code's existing `ModelProviderInfo`, Chat Completions
client, remote `/v1/models` discovery, direct-provider model cache, and model
selection flow. The implementation MUST NOT introduce Colibri-specific HTTP
request code.

Users MAY override the built-in provider through the existing
`[model_providers.colibri]` configuration mechanism. Existing custom-provider
precedence and normalization rules MUST remain unchanged.

## Model Discovery And Selection

When Colibri is reachable, Code MUST request `GET /v1/models` through the
existing remote-model discovery path. Returned model identifiers MUST appear
under the Colibri provider group and MUST be selectable for session, sub-agent,
review, and auto-drive roles according to the existing role controls.

When Colibri is unreachable, Code MUST preserve the existing remote-provider
failure behavior. The provider MAY remain visible without models, and Code MUST
NOT attempt to start Colibri automatically.

## Runtime Boundary

Colibri MUST run as an independent local service. The expected launch shape is:

```text
coli serve --model <model-path> --ram 6 --host 127.0.0.1 --port 8000
```

The explicit `--ram 6` limit is the conservative Steam Deck default for this
design. It prevents Colibri's automatic policy from allocating approximately
88 percent of currently free RAM while leaving the operating system responsible
for moving other anonymous pages into the existing disk swapfile under pressure.

Code MUST NOT pass memory limits to Colibri because Code does not own the
Colibri process. Documentation MUST state that operators MAY choose a different
RAM budget after measuring their model, context size, and host workload.

## Swap Policy

The current host policy is outside the Code repository but defines the tested
deployment assumptions:

- `/var/home/swapfile` remains enabled at 16 GiB.
- `/etc/systemd/zram-generator.conf` remains disabled.
- Code does not execute `swapon`, `swapoff`, `zramctl`, or systemd unit changes.
- Colibri model files remain on normal storage and MUST NOT be copied into a
  RAM-backed or zram-backed filesystem.

Swap is an overflow mechanism, not an extension of Colibri's expert cache.
Performance under sustained swap pressure is expected to degrade, so the
documented RAM cap MUST be treated as a safety default rather than a throughput
optimization.

## Configuration Example

The provider MUST work without user configuration after it is added to the
built-in map. Documentation SHOULD include the equivalent explicit override:

```toml
model_provider = "colibri"
model = "<model-id-from-colibri>"

[model_providers.colibri]
name = "Colibri"
base_url = "http://127.0.0.1:8000/v1"
wire_api = "chat"
```

## Error Handling

- Connection failures MUST use the existing direct-provider error reporting.
- An empty `/v1/models` response MUST NOT create a fabricated model entry.
- Unsupported tool-calling or structured-output behavior MUST surface the
  provider's response error rather than being silently rewritten.
- Code MUST NOT fall back to an unrelated cloud provider when Colibri was
  explicitly selected.
- A configured Colibri API key MAY be supported through the existing custom
  provider secret mechanisms, but the built-in local provider MUST remain
  unauthenticated by default.

## Verification Scenarios

- GIVEN the built-in providers are constructed, WHEN the map is inspected,
  THEN `colibri` exists with the local `/v1` URL, Chat Completions wire API, and
  no required OpenAI authentication.
- GIVEN a mock Colibri-compatible `/v1/models` response, WHEN model discovery
  runs, THEN the returned models appear in the Colibri provider catalog.
- GIVEN Colibri is unavailable, WHEN the selector refreshes providers, THEN
  Code reports the connection failure without starting a process or changing
  host memory configuration.
- GIVEN the user overrides `[model_providers.colibri]`, WHEN configuration is
  loaded, THEN the user definition follows the repository's existing provider
  precedence behavior.
- GIVEN the runtime documentation, WHEN an operator follows the Steam Deck
  example, THEN Colibri starts with `--ram 6` and the existing swapfile remains
  the only swap backend.

## Files Expected To Change

- `code-rs/core/src/model_provider_info.rs` for the built-in provider and unit
  coverage.
- `code-rs/tui/src/direct_provider.rs` only if provider discovery coverage needs
  an explicit Colibri assertion.
- `code-rs/config.md` for the provider and runtime example.

No Colibri source files, operating-system configuration files, service units,
or model artifacts are in scope.

## External Contract

This design follows Colibri's current public CLI and OpenAI-compatible server
contract as of September 18, 2026:

- `coli serve` defaults to `127.0.0.1:8000` and accepts `--model`, `--ram`,
  `--host`, and `--port`.
- `--ram 0` is automatic and uses approximately 88 percent of available RAM.
- The server exposes OpenAI-compatible model discovery and Chat Completions.

Upstream reference: `https://github.com/JustVugg/colibri`.

## Deferred Work

- Installing or updating Colibri.
- Downloading or converting model weights.
- Starting Colibri from Code.
- Adding a Colibri-specific API key onboarding flow.
- Benchmarking Vulkan acceleration on the Steam Deck APU.
- Replacing disk swap with zram or zswap.
