# AI Horde Auxiliary Client Design

<!-- markdownlint-disable MD013 -->

## Goal

Expose AI Horde text generation, image generation, image interrogation, and
image vectorization to the agent without replacing Code's primary tool-capable
model.

## Authentication

The client MUST resolve credentials in this order:

1. An AI Horde key stored in Code's encrypted secret store.
2. `AI_HORDE_API_KEY` from the process environment.
3. The anonymous API key `0000000000`.

The TUI MUST indicate whether requests are authenticated or anonymous. When the
anonymous key is active, the model selector MUST show this notice immediately
below the AI Horde section:

> Anonymous requests receive the lowest queue priority.

The API key MUST NOT be written into `config.toml`, logs, tool results, or
conversation history.

## TUI Model Selection

AI Horde models MUST be selectable directly from the TUI model-selection
surface as auxiliary defaults. They MUST be visually separated from primary
Code models so selecting one does not imply that it can drive Code's structured
agent protocol.

The first text and image option MUST be:

```text
Auto (AI Horde selects worker)
```

Every explicit model name MUST include its current available worker count in
the same label:

```text
Llama-3.1-8B (12 workers)
MythoMax-L2-13B (4 workers)
Stable Diffusion XL (27 workers)
Unavailable Model (0 workers)
```

The count MUST represent currently online workers advertising that model. A
loading or request-failure state MUST remain distinguishable from a verified
zero-worker result. Counts MUST refresh when the selector opens, through a
manual refresh action, and through a short-lived cache.

## Text Worker Selection

When `Auto` is selected, the client MUST omit explicit model and worker
restrictions and allow AI Horde to control scheduling.

When an explicit text model is selected, the client MUST:

1. Fetch online text workers advertising that model.
2. Filter out workers whose `max_context_length` is less than the requested
   context length.
3. Sort eligible workers by `max_context_length` ascending.
4. Attempt the smallest sufficient worker first.
5. Keep the largest-context worker as the final fallback.

The request's `max_context_length` MUST include conservative headroom for the
prompt and requested output. The caller MAY provide an explicit value.

Because AI Horde worker whitelists do not define execution order, ordered
fallback MUST submit to one selected worker at a time. Before advancing to the
next worker, Code MUST cancel the unfinished request. Code MUST advance only
after the request is impossible, faulted, or exceeds the configured queue-wait
threshold. This prevents duplicate generations and duplicate Kudos use.

## Image And Vector Operations

The client MUST expose auxiliary operations for:

- Listing text and image models.
- Text generation.
- Image generation.
- Image interrogation.
- Image vectorization through AI Horde's `vectorize` interrogation form.

Image requests MUST filter explicit workers by model, maximum pixel capacity,
and required capabilities such as img2img or inpainting. `Auto` MUST leave
worker selection to AI Horde.

Generated images MUST be written beneath Code's managed cache and returned as
artifact paths rather than embedding large base64 payloads into conversation
history.

AI Horde image vectorization MUST be described as an image embedding. The
client MUST NOT advertise general text embeddings because the public API does
not provide a general text-embedding operation.

## Tool Surface

The initial built-in tools SHOULD be:

- `horde_list_models`
- `horde_generate_text`
- `horde_generate_image`
- `horde_interrogate_image`
- `horde_vectorize_image`

Tool calls MUST expose queue state, worker selection, cancellation, faults, and
Kudos consumption without exposing credentials.

## Reliability

The client MUST use the official asynchronous request lifecycle, poll with
bounded backoff, cancel abandoned requests, and retain no background polling
task after the caller has stopped waiting.

Cached model and worker data MAY be used after a temporary API failure, but the
TUI MUST mark cached data stale.

## Verification Scenarios

- GIVEN no configured key, WHEN a request is submitted, THEN the anonymous key
  is used and the queue-priority notice is visible.
- GIVEN an explicit model and workers with 4K, 8K, and 32K context, WHEN a 6K
  request is submitted, THEN the 8K worker is attempted before the 32K worker.
- GIVEN `Auto`, WHEN a request is submitted, THEN no worker or model whitelist
  is added by Code.
- GIVEN a timed-out worker attempt, WHEN fallback begins, THEN the previous
  request is cancelled before the next request is created.
- GIVEN model metadata with five online workers, WHEN the TUI renders the row,
  THEN the label ends with `(5 workers)`.
