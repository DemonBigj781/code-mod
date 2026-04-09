# RAG MCP Server

Standalone MCP server for retrieval over local files, source code, URLs, and inline content.

## What it exposes

- `rag_ingest`: add or refresh sources in the persistent index
- `rag_ingest_dir`: recursively ingest a directory of files
- `rag_query`: retrieve ranked snippets with citations
- `rag_sources`: list indexed sources
- `rag_chunks`: inspect the chunks for a specific source

## Setup

```bash
cd tools/ai-workspace/rag-mcp-server
npm install
npm run build
```

## Run

```bash
npm start
```

The server speaks MCP over stdio, so it is meant to be launched by another MCP client.

## Web GUI

```bash
npm run gui
```

Then open `http://localhost:8787`.

`npm run gui` now runs in watch mode, so edits to `src/gui-server.ts` will reload automatically. If you want a one-off launch without watching, use `npm run gui:once`.

The web UI gives you:

- directory ingest
- bulk file/URL ingest
- query/search
- source listing
- chunk inspection via the API

If you only want the backend running in the background and will open the browser yourself, use:

```bash
npm run gui:backend
```

That starts the HTTP server on `http://localhost:8787` without opening a browser. To stop it later, use:

```bash
npm run gui:backend:stop
```

If the Optimum embeddings service is already running on `http://127.0.0.1:8123`, the backend will automatically use it for embeddings.

The MCP server `npm start` also checks for that service and uses it automatically when available.

## Optimum Embeddings Service

For faster local embeddings on Windows, you can run a small Python service backed by Optimum + ONNX Runtime and point the RAG server at it.

```bash
npm run embeddings:optimum
```

This starts a local OpenAI-compatible embeddings endpoint at `http://127.0.0.1:8123/v1/embeddings`.

To use it, set:

| Variable | Purpose | Default |
| --- | --- | --- |
| `RAG_OPTIMUM_EMBEDDING_URL` | Optimum embeddings service URL | `http://127.0.0.1:8123` |
| `RAG_OPTIMUM_EMBEDDING_MODEL` | Optimum service model name | `sentence-transformers/paraphrase-MiniLM-L3-v2` |

## Configuration

| Variable | Purpose | Default |
| --- | --- | --- |
| `RAG_DATA_DIR` | Directory used to persist the index | `./data` |
| `RAG_EMBEDDING_PROVIDER` | `auto`, `local`, `openai`, or `optimum` | `optimum` when `RAG_OPTIMUM_EMBEDDING_URL` is set, otherwise `auto` |
| `RAG_EMBEDDING_MODEL` | Embedding model name | local: `Xenova/paraphrase-MiniLM-L3-v2`, openai: `text-embedding-3-small`, optimum: `sentence-transformers/paraphrase-MiniLM-L3-v2` |
| `RAG_EMBEDDING_API_KEY` | OpenAI-compatible embedding key | `OPENAI_API_KEY` fallback |
| `RAG_EMBEDDING_BASE_URL` | OpenAI-compatible base URL | `https://api.openai.com` |
| `RAG_LOCAL_EMBEDDING_MODEL` | Local embedding model override | `Xenova/paraphrase-MiniLM-L3-v2` |
| `RAG_EMBEDDING_DEVICE` | Local embedding device | `auto` |
| `RAG_OPTIMUM_EMBEDDING_URL` | Optimum embeddings service URL | `http://127.0.0.1:8123` |
| `RAG_OPTIMUM_EMBEDDING_MODEL` | Optimum service model name | `sentence-transformers/paraphrase-MiniLM-L3-v2` |
| `RAG_EMBEDDING_WORKERS` | Local embedding worker count | CPU cores minus 2, minimum 1 |

## Notes

- Source code is treated as a first-class input type.
- The index is persisted to `data/index.json` and reloaded on startup.
- Query results return evidence and citations, not synthesized answers.
