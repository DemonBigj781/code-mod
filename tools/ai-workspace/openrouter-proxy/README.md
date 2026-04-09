# OpenRouter Proxy Service

This service exposes a small Express HTTP proxy that funnels `/api/*` requests through OpenRouter's API endpoint at `https://openrouter.ai/api/v1`. It centralizes billing keys, enforces a simple upstream rate limit, and lets you gate which clients may reach the upstream API.

## Features

- Rotates through one or more OpenRouter API keys and penalizes keys that hit 429/401 responses so they recover before being reused.
- Optional `PROXY_SECRET` requirement so only authorized callers (HTTP header, query parameter, or legacy `x-api-key`) can send requests.
- The proxy remembers the last model that succeeded and tries it first on the next request for faster repeat responses.
- When that working model fails, the proxy falls back through the rest of the discovered free models before returning the failure.
- If every model attempt fails, the proxy performs one last anonymous AI Horde check using the `0000000000` key unless you override `AIHORDE_API_KEY`.
- Per-client rate limiting with `express-rate-limit`.
- Transparent bubbling of streaming responses (SSE) and standard completions.
- Hosts a bare-bones interface (HTTP UI) on port 3000 that includes a quick text chat powered by the proxy so you can send messages without writing curl.
- Provides `GET /models` so the UI (and other clients) can refresh the dynamically-discovered `:free` models before starting a chat. The UI intentionally hides `openrouter/free` and only shows the other eligible `:free` models you have discovered.

## Code assist endpoint

The proxy exposes a structured code-assist entrypoint on both:

- `POST /api/code-assist`
- `POST /v1/code-assist`

This endpoint assembles a structured prompt, injects RAG context from the local index, and then sends the request through the same model routing and fallback logic used by normal proxy traffic.

### Request schema

The request body must be a JSON object. Common fields:

- `mode` - required logical mode: `explain`, `modify`, `fix`, `debug`, or `repair`
- `task` - freeform task description
- `error` - error text or stack trace
- `currentCode` - code to inspect or modify
- `constraints` - optional string array of requirements
- `expectedOutput` - optional output preference text
- `model` - optional model override
- `previewOnly` - when `true`, return the assembled prompt without sending to a model
- `dryRun` - alias for `previewOnly`
- `promptPreview` - alias for `previewOnly`

Retrieval controls:

- `topK` - integer from `1` to `20`, default `6`
- `maxContextChars` - integer from `1000` to `100000`, default `12000`
- `queryOverride` - optional explicit retrieval query
- `sourceKinds` - optional string array or comma-separated list

If `queryOverride` is omitted, the proxy uses `task`, `error`, and `currentCode` to build the retrieval query.

### Mode behavior

Each mode changes the instruction block intentionally:

- `explain` - explain the code and highlight failure points, with no edits by default
- `modify` - produce a minimal, reviewable change
- `fix` - focus on the specific bug or failure and the smallest safe correction
- `debug` - separate symptoms from root cause before suggesting a fix
- `repair` - restore correctness and robustness while preserving surrounding behavior

### Validation errors

Malformed requests fail loudly with `400` responses. Common errors include:

- `mode must be one of: explain, modify, fix, debug, repair`
- `topK must be an integer between 1 and 20`
- `maxContextChars must be an integer between 1000 and 100000`
- `request body must be a JSON object`
- `invalid JSON body: ...`
- `task, code, or error is required`

### Preview mode

When `previewOnly`, `dryRun`, or `promptPreview` is `true`, the endpoint returns the assembled prompt instead of calling a model.

Preview response fields:

- `mode`
- `model`
- `previewOnly`
- `query`
- `retrieval`
- `prompt`
- `messages`

### Example: preview request

```json
{
  "mode": "fix",
  "task": "Fix the crash when the config file is missing",
  "error": "TypeError: Cannot read properties of undefined",
  "currentCode": "function loadConfig() { ... }",
  "topK": 4,
  "maxContextChars": 6000,
  "previewOnly": true
}
```

### Example: preview response

```json
{
  "mode": "fix",
  "model": "some-free-model",
  "previewOnly": true,
  "query": "Fix the crash when the config file is missing\nTypeError: Cannot read properties of undefined\nfunction loadConfig() { ... }",
  "retrieval": {
    "topK": 4,
    "maxContextChars": 6000,
    "sourceKinds": [],
    "matches": []
  },
  "prompt": "ROLE\nYou are a careful coding assistant.\n\nMODE\nfix\n\nINSTRUCTION\nFix the specific bug or failure described by the error. Focus on the smallest safe correction and include the exact code change needed.\n...",
  "messages": [
    {
      "role": "system",
      "content": "You are in fix mode. Focus on the exact bug and the smallest safe correction."
    },
    {
      "role": "user",
      "content": "ROLE\n...\n"
    }
  ]
}
```

### Example: model submission request

```json
{
  "mode": "modify",
  "task": "Refactor this function to reduce duplication",
  "currentCode": "function a() { ... }",
  "queryOverride": "duplicate logic in helper functions",
  "topK": 6,
  "maxContextChars": 12000
}
```

If `previewOnly` is omitted, the endpoint submits the assembled prompt through the existing proxy path and keeps the current fallback behavior intact.

## Dynamic free-model discovery

The proxy periodically queries `https://openrouter.ai/api/v1/models` (default every 300 seconds; configurable via `FREE_MODEL_REFRESH_MS`) and automatically keeps only the models whose name ends with `:free`. That way the UI and proxy stay in sync with whatever OpenRouter makes available without editing `.env`, while the UI continues to hide `openrouter/free`.

## Configuration

| Variable | Description | Default |
|---------|-------------|---------|
| `OPENROUTER_API_KEY` | Primary OpenRouter key. | *required* |
| `OPENROUTER_API_KEYS` | Comma-separated list to rotate. | `undefined` |
| `PROXY_SECRET` | Caller token to protect the proxy. | `disabled` |
| `PORT` | Legacy proxy port fallback. | `8080` |
| `PROXY_PORT` | HTTP port the proxy listens on (default `8080`). | `8080` |
| `UI_PORT` | Port where the status UI is served (default `3000`). | `3000` |
| `RATE_WINDOW_MS` | Window for rate limiter. | `60000` |
| `RATE_MAX` | Requests allowed per window. | `120` |
| `KEY_COOLDOWN_MS` | Cooldown for penalized keys. | `120000` |
| `REQUEST_TIMEOUT_MS` | Upstream timeout in ms. | `120000` |
| `FREE_MODEL_REFRESH_MS` | How often (ms) to refresh the `:free` model list; `0` disables automatic refresh. | `300000` |
| `AIHORDE_API_KEY` | Anonymous fallback key used only after all model retries fail. | `0000000000` |

## Running

```bash
cd openrouter-proxy
npm install
Copy `.env.example` to `.env`, set `OPENROUTER_API_KEY(S)` (and optional `PROXY_SECRET`), then `npm start`.

Start the UI at `http://localhost:3000` (or `UI_PORT`) to view the health check while the proxy listens on port 8080 (or `PROXY_PORT`). You can also launch everything via the included `start-proxy.bat` once your `OPENROUTER_API_KEY` (and optional overrides) are in the environment.
```

For local development with auto restart use `npm run dev` after installing the dev dependencies.

## Browser automation

If you need a local browser runner for this proxy, use Camoufox instead of Playwright. Camoufox is wired up as a headless-only server entrypoint so it cannot launch a visible browser window.

Run it with either:

```bash
npm run camoufox:server
```

or:

```bat
start-camoufox-headless.bat
```

The websocket endpoint defaults to `ws://127.0.0.1:8765/openrouter-proxy` and is always launched with `headless=True`.

## Security notes

- The proxy strips client-supplied `Authorization` headers and rewrites them with your configured OpenRouter key(s).
- A client secret prevents unauthorized access and can be provided via `Authorization: Bearer <PROXY_SECRET>` or `x-api-key`.
- Downstream requests are rate limited to curb abuse; tune `RATE_WINDOW_MS`/`RATE_MAX` per deployment.

## OpenRouter context

All traffic is forwarded to the official OpenRouter service (`https://openrouter.ai/api/v1`). The proxy now prefers the most recently successful model, but still falls back through the discovered free models if that model stops working. If you want to update the upstream host (for regional endpoints or custom routers), change `OPENROUTER_BASE_URL` in the environment before starting the proxy.

Reference docs:
- [Puter llms.txt](https://docs.puter.com/llms.txt)
