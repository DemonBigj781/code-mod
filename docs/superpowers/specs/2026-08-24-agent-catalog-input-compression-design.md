# Agent Catalog and Operator Input Pipeline Design

Date: 2026-08-24
Status: Approved

## Goals

- Ensure one operator submission reaches the model exactly once.
- Surface every known model as a model-backed agent without maintaining a
  second static catalog.
- Allow each model-backed agent to be enabled or disabled independently.
- Restore simple custom model-slug creation.
- Separate OpenRouter free and paid variants in the model store.
- Compress operator text deterministically before model submission without
  using an LLM.

## Non-Goals

- Do not remove the advanced custom executable agent editor.
- Do not deduplicate identical prompts submitted in separate turns.
- Do not alter the original operator text shown in local history.
- Do not infer OpenRouter pricing through network calls beyond the existing
  model catalog response.
- Do not use an LLM, embedding model, or external service for prompt compression.

## Architecture

### Unified Agent Catalog

The runtime will expose one canonical catalog of agent entries. Each
model-backed entry will have a stable identity composed of its provider
identifier and model slug. The catalog will merge:

- curated built-in external agents such as Claude, Gemini, and Qwen;
- built-in Code models;
- models discovered from configured provider catalogs;
- user-created custom model slugs;
- advanced custom executable agents already stored in configuration.

Model settings, agent settings, the agent tool schema, and agent execution will
consume this catalog instead of independently reconstructing partial lists.

Curated agents will preserve their current enabled defaults. Newly discovered
model-backed agents will be present but disabled by default. This keeps all
models discoverable and bindable without adding roughly one hundred enabled
entries to the agent tool schema automatically.

### Model-Backed Agent Configuration

A model-backed agent will retain its provider identity as well as its model
slug. Execution will launch the Code CLI with both values so a model selected
from OpenRouter or another direct provider cannot silently fall back to the
active chat provider.

The simple creation flow will ask for:

- provider;
- model slug;
- optional display name or generated stable name.

The advanced executable editor will remain available for arbitrary commands
and external CLIs.

### Agent Toggles

Every model-agent row in Agents settings will expose an immediate
enabled/disabled control. Keyboard and mouse behavior will follow the existing
settings toggle conventions:

- Space toggles the selected row;
- Left enables;
- Right disables;
- mouse activation toggles the row.

Changes will persist immediately and refresh the agent tool's allowed model
values. Both enable and disable paths will be tested independently.

### OpenRouter Store Grouping

The OpenRouter catalog will render two subsections under the provider:

1. Free
2. Paid

A model slug ending in `:free` will be classified as Free. Other OpenRouter
slugs will be classified as Paid. The two variants of the same base model will
remain separate selectable entries. Classification will be isolated behind a
helper so richer pricing metadata can replace suffix classification later
without changing rendering code.

### Duplicate Submission Repair

Each operator submission will have a stable submission identity from the TUI
boundary through request assembly. The current submission must appear once in
the outbound model payload.

The fix will remove the duplicate insertion at its source. It will not perform
broad text-based deduplication because two separate turns may intentionally
contain identical text.

The local history entry and persistent message-history entry remain independent
side effects and must not become extra model input items.

### Deterministic Operator Input Compression

Operator input compression will have two settings:

- standard compression: enabled by default;
- aggressive compression: disabled by default and applied on top of standard
  compression.

Compression will run on a model-bound copy after local history captures the
original text and before user prompt hooks and model request assembly inspect
the final prompt.

Protected content will remain byte-for-byte unchanged wherever practical:

- fenced code blocks;
- inline code;
- quoted strings and quoted blocks;
- URLs;
- filesystem paths;
- commands and command-line arguments;
- numbers and identifiers;
- JSON, TOML, YAML, XML, and similar structured blocks;
- bullet and numbered requirement lists;
- explicit requirement terms such as MUST, MUST NOT, NEVER, and ALWAYS.

Standard compression will conservatively:

- normalize redundant whitespace outside protected spans;
- remove adjacent duplicate lines, paragraphs, and sentences;
- remove a small allowlist of non-semantic filler phrases;
- preserve sentence ordering and punctuation needed for meaning.

Aggressive compression will additionally:

- convert eligible prose sentences into terse clause-oriented text;
- remove repeated subjects and transition phrases when deterministic rules
  prove the reference is local;
- compact ordinary prose lists while preserving item boundaries.

Short prompts and prompts composed entirely of protected content will remain
unchanged. Compression will be deterministic and idempotent: compressing
already compressed text must produce the same output.

## Configuration

The input-compression settings will be represented as a dedicated configuration
section rather than unrelated feature flags:

```toml
[input_compression]
enabled = true
aggressive = false
```

The settings UI will expose both controls and explain that history keeps the
original prompt while the model receives the compacted copy.

## Error Handling

- Catalog refresh failures will retain cached model-agent entries and their
  persisted toggle state.
- A custom model with an empty provider or slug will not be saved.
- Compression failures or invariant violations will fall back to the original
  operator text and emit a diagnostic log entry.
- A compressed prompt that becomes empty will fall back to the original prompt.
- Unknown OpenRouter pricing variants will default to Paid rather than being
  labeled Free.

## Verification

Automated coverage will include:

- one submission identity produces one user message in the outbound model payload;
- identical text submitted in separate turns remains two legitimate messages;
- every model-store entry can resolve to a model-backed agent entry;
- generated model agents default disabled while curated defaults retain current
  behavior;
- enabling and disabling a model agent both persist and update tool
  availability;
- custom provider and model slug creation produces executable Code CLI
  arguments;
- OpenRouter `:free` variants render under Free and other variants under Paid;
- standard compression golden cases;
- aggressive compression golden cases;
- protected content remains unchanged;
- compression is deterministic and idempotent;
- local history keeps the original text while outbound input receives the
  compressed text.

The repository's required final verification remains `./build-fast.sh`, run
with compilation restricted to one CPU thread.
