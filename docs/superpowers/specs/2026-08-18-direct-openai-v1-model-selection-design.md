# Direct OpenAI-Compatible V1 Model Selection Design

<!-- markdownlint-disable MD013 -->

## Goal

Allow an end user to add an OpenAI-compatible `/v1` endpoint directly from the
TUI model selector, discover its models, and immediately select one for the
active session without creating or switching a Code profile.

## Model Selector Flow

The model list MUST include this action after the existing model groups:

```text
+ Add OpenAI-compatible /v1 endpoint
```

Activating it MUST open an inline form within the model-selection flow. The form
MUST collect:

- Endpoint display name.
- `/v1` base URL.
- Optional API key.
- Wire protocol: Chat Completions or Responses.

Chat Completions MUST be the default because it is the most broadly implemented
OpenAI-compatible v1 protocol.

Submitting the form MUST validate the URL, fetch `GET <base-url>/models`, add a
new provider group to the current model list, and return focus to model
selection. The user MUST be able to select a discovered model immediately
without restarting Code.

## Direct Selection

Selecting a discovered model MUST directly update the active session's model
and model-provider identifiers. It MUST NOT:

- Create a profile.
- Switch profiles.
- Change unrelated project settings.
- Require a new Code process.

Endpoint definitions MAY be remembered as model sources for future selector
sessions. Persistence MUST use the existing `ModelProviderInfo` configuration
shape and remote-model cache rather than introducing a second provider runtime.

## Authentication

API keys entered through the selector MUST be stored through Code's encrypted
secret store. The persisted provider definition MUST reference the secret and
MUST NOT contain the key itself.

Endpoints without a key MUST be supported for local services. Authentication
failures MUST be shown inline and MUST NOT remove a previously cached model
list.

## Model Catalog

Models MUST be grouped beneath the endpoint display name:

```text
LocalAI
  qwen2.5-coder:14b
  llama-3.1-8b

Company Gateway
  coding-model-v2
  review-model-v1
```

Internal model identities MUST include the provider identifier so equal model
names from different endpoints do not collide.

The selector MUST distinguish fresh, stale cached, loading, authentication
failure, and connection failure states. Refreshing one endpoint MUST NOT block
or erase models from other providers.

OpenAI-compatible model rows MUST NOT display AI Horde worker counts because
the standard `/v1/models` response does not define them.

## Compatibility

The existing Chat Completions and Responses request paths MUST remain the sole
runtime implementations. The selector MUST create compatible
`ModelProviderInfo` entries and MUST NOT add endpoint-specific request code.

When a selected endpoint or model does not support structured tools, Code MUST
surface the provider error and SHOULD show a compatibility warning. It MUST NOT
silently claim full agent compatibility.

## Verification Scenarios

- GIVEN an unauthenticated local `/v1` endpoint, WHEN it is added, THEN its
  models appear immediately and can be selected for the active session.
- GIVEN an API-key-protected endpoint, WHEN valid credentials are entered, THEN
  the key is stored encrypted and omitted from persisted configuration.
- GIVEN two endpoints exposing the same model ID, WHEN both are added, THEN both
  rows remain independently selectable.
- GIVEN a previously successful endpoint that is temporarily offline, WHEN the
  selector opens, THEN cached models remain visible and are marked stale.
- GIVEN direct model selection, WHEN the user chooses a custom model, THEN no
  profile is created or activated.

## Deferred Work

Prompt compression is not part of this design and MUST remain unchanged until
the end user supplies its separate behavioral requirements.
