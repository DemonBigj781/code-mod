# Workspace Overview

This workspace contains two sibling subsystems that work together for retrieval-augmented code assistance.

## Subsystems

- `rag-mcp-server`: owns ingestion, chunking, hashing, indexing, and semantic retrieval.
- `openrouter-proxy`: owns request handling, key rotation, model fallback, and OpenAI-style chat forwarding.

## How They Interact

The proxy exposes a structured code-assist endpoint that queries the RAG server for relevant snippets, assembles a prompt, and then sends that prompt through the existing proxy routing path.

The RAG subsystem stays isolated and continues to manage its own index, caches, and retrieval behavior.

## Starting The Workflow

From the monorepo root, use the convenience scripts:

```bash
pnpm run ai:install
pnpm run ai:status
pnpm run ai:rag
pnpm run ai:proxy
pnpm run ai:dev
pnpm run ai:check
pnpm run ai:smoke
pnpm run ai:help
```

`ai:install` checks whether each AI package already has local dependencies and only installs the missing subtree package, so the AI workspace does not inherit unrelated monorepo packages during setup.

If you are already inside this Every Code checkout, you can also use the CLI entrypoint:

```powershell
coder ai status
coder ai dev
coder ai smoke
```

`coder ai dev` now starts the AI services first and then hands off to the normal Every Code CLI once the services are reachable.

The CLI handoff uses a separate AI home directory by default:

- Windows: `%USERPROFILE%\.every-code-ai`
- Other platforms: `~/.every-code-ai`

That AI home is independent from Codex's `~/.codex` / `CODEX_HOME` storage. The launcher generates the AI runtime config inside that separate folder so the AI workflow does not depend on the main Codex config area.
The generated `config.toml` inside that AI home declares a dedicated `local-proxy` provider that points at the local OpenRouter proxy. The proxy URL and optional API key can come from the separate AI settings file, so the interactive CLI flow uses the proxy instead of talking directly to GPT/OpenAI or asking you to log in on launch.

If you want to edit the AI settings themselves, start from `tools/ai-workspace/ai-settings.example.json` and copy it to `tools/ai-workspace/ai-settings.json`.

If you prefer to run the package commands directly:

```bash
cd tools/ai-workspace/rag-mcp-server
npm install
npm run build
npm start
```

In a second terminal:

```bash
cd tools/ai-workspace/openrouter-proxy
npm install
npm start
```

Recommended startup order:

1. Start the RAG GUI backend first so the proxy has a live retrieval target.
2. Start the proxy second so `/api/code-assist` can query the RAG index.
3. Use preview mode when you want the exact prompt and retrieval payload without model submission.

Expected local endpoints:

- RAG GUI backend: `http://127.0.0.1:8787`
- Proxy: `http://127.0.0.1:8080` unless `PORT` or `PROXY_PORT` overrides it
- Proxy startup requires `OPENROUTER_API_KEY` or `OPENROUTER_API_KEYS` to be set in the environment.

## Code Assist

Use `POST /api/code-assist` or `POST /v1/code-assist` on the proxy. The request can include `mode`, `task`, `error`, `currentCode`, `topK`, `maxContextChars`, `queryOverride`, `sourceKinds`, and preview flags such as `previewOnly`.

When preview mode is enabled, the proxy returns the fully assembled prompt and retrieval payload without sending a model request.

The seam-level smoke test checks the proxy code-assist preview path and fails loudly if the local seam is not responding. It does not invent or require separate health routes.

## Monorepo Integration Notes

- The AI workspace is registered in the root `pnpm-workspace.yaml`.
- Root scripts `ai:install`, `ai:status`, `ai:rag`, `ai:proxy`, `ai:dev`, `ai:check`, `ai:smoke`, and `ai:help` are thin wrappers around the two isolated packages.
- `ai:smoke` probes the proxy preview path directly and does not require a separate health route.
- `coder ai dev` uses a dedicated AI home folder instead of reusing Codex's config directory.
- That AI home contains its own generated `config.toml` with a `local-proxy` provider entry, so launch does not depend on Codex's normal `config.toml` or a login prompt.
- No cross-imports were added between `rag-mcp-server` and `openrouter-proxy`.
- The code-assist seam and all RAG/proxy internals remain unchanged.
- Do not refactor the prompt builder or retrieval system unless the seam contract changes.
