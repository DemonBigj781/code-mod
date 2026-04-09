import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { spawn, type ChildProcess } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { z } from 'zod';

import { createEmbeddingProvider } from './embeddings.js';
import { RagIndex, type RagSourceInput } from './rag.js';
import { resolveDataDir, resolveEmbeddingOptions } from './runtime.js';

const packageName = 'rag-mcp-server';
const packageVersion = '1.0.0';

const server = new McpServer({
  name: packageName,
  version: packageVersion
});

const moduleDir = path.dirname(fileURLToPath(import.meta.url));
const guiBackendPath = path.resolve(moduleDir, 'gui-server.js');
let guiBackendProcess: ChildProcess | null = null;

const sourceInputSchema = z.object({
  kind: z.string().min(1),
  locator: z.string().min(1),
  label: z.string().optional(),
  path: z.string().optional(),
  url: z.string().optional(),
  content: z.string().optional(),
  metadata: z.record(z.string(), z.unknown()).optional()
});

const ingestInputSchema = z.object({
  sources: z.array(sourceInputSchema).min(1)
});

const ingestDirectoryInputSchema = z.object({
  directoryPath: z.string().min(1),
  recursive: z.boolean().optional(),
  includeHidden: z.boolean().optional()
});

const queryInputSchema = z.object({
  query: z.string().min(1),
  topK: z.number().int().min(1).max(20).optional(),
  sourceIds: z.array(z.string().min(1)).optional(),
  sourceKinds: z.array(z.string().min(1)).optional()
});

const listSourcesSchema = z.object({
  kinds: z.array(z.string().min(1)).optional()
});

const listChunksSchema = z.object({
  sourceId: z.string().min(1),
  offset: z.number().int().min(0).optional(),
  limit: z.number().int().min(1).max(200).optional()
});

const queueSourcesSchema = z.object({
  sources: z.array(sourceInputSchema).min(1),
  label: z.string().optional(),
  concurrency: z.number().int().min(1).max(8).optional()
});

const embeddingOptions = resolveEmbeddingOptions();
const index = new RagIndex({
  dataDir: resolveDataDir(),
  embeddingProvider: createEmbeddingProvider(embeddingOptions),
  embeddingBatchSize: embeddingOptions.embeddingBatchSize
});

const guiBaseUrl = (process.env.RAG_GUI_URL ?? 'http://127.0.0.1:8787').replace(/\/+$/, '');
let optimumSwitchPromise: Promise<void> | null = null;

server.registerTool(
  'rag_ingest',
  {
    title: 'Ingest sources',
    description: 'Queue local files, URLs, source code, or inline content for persistent RAG ingestion.',
    inputSchema: queueSourcesSchema
  },
  async (args) => {
    await ensureOptimumEmbeddingProvider();
    const normalizedSources = args.sources.map(normalizeSourceInput);
    const queued = await queueSourceInGuiBackend({
      sources: normalizedSources,
      label: args.label ?? 'MCP source ingest',
      concurrency: args.concurrency
    });
    return {
      content: [
        {
          type: 'text',
          text: formatQueuedIngestResults(queued)
        }
      ],
      structuredContent: queued
    };
  }
);

server.registerTool(
  'rag_ingest_dir',
  {
    title: 'Ingest directory',
    description: 'Recursively ingest files from a directory into the persistent RAG index.',
    inputSchema: ingestDirectoryInputSchema
  },
  async (args) => {
    await ensureOptimumEmbeddingProvider();
    const queued = await queueDirectoryInGuiBackend({
      directoryPath: args.directoryPath,
      recursive: args.recursive,
      includeHidden: args.includeHidden
    });

    return {
      content: [
        {
          type: 'text',
          text: formatQueuedDirectoryResults(queued)
        }
      ],
      structuredContent: {
        directoryPath: args.directoryPath,
        ...queued
      }
    };
  }
);

server.registerTool(
  'rag_query',
  {
    title: 'Query RAG index',
    description: 'Return ranked snippets and citations from the persistent RAG index.',
    inputSchema: queryInputSchema
  },
  async (args) => {
    await ensureOptimumEmbeddingProvider();
    const results = await index.search({
      query: args.query,
      topK: args.topK,
      sourceIds: args.sourceIds,
      sourceKinds: args.sourceKinds
    });

    return {
      content: [
        {
          type: 'text',
          text: formatQueryResults(args.query, results)
        }
      ],
      structuredContent: {
        query: args.query,
        matches: results
      }
    };
  }
);

server.registerTool(
  'rag_sources',
  {
    title: 'List sources',
    description: 'List indexed sources and their chunk counts.',
    inputSchema: listSourcesSchema
  },
  async (args) => {
    await ensureOptimumEmbeddingProvider();
    const sources = await index.listSources(args);
    return {
      content: [
        {
          type: 'text',
          text: formatSources(sources)
        }
      ],
      structuredContent: {
        sources
      }
    };
  }
);

server.registerTool(
  'rag_chunks',
  {
    title: 'Inspect chunks',
    description: 'Inspect the indexed chunks for a source ID.',
    inputSchema: listChunksSchema
  },
  async (args) => {
    await ensureOptimumEmbeddingProvider();
    const chunks = await index.listChunks(args.sourceId, {
      offset: args.offset,
      limit: args.limit
    });

    return {
      content: [
        {
          type: 'text',
          text: formatChunks(args.sourceId, chunks)
        }
      ],
      structuredContent: {
        sourceId: args.sourceId,
        chunks
      }
    };
  }
);

async function main(): Promise<void> {
  await ensureGuiBackendRunning();
  await index.ensureReady();
  await ensureOptimumEmbeddingProvider();

  const transport = new StdioServerTransport();
  await server.connect(transport);
}

async function ensureGuiBackendRunning(): Promise<void> {
  const autostart = (process.env.RAG_GUI_AUTOSTART ?? '1').trim().toLowerCase();
  if (autostart === '0' || autostart === 'false' || autostart === 'off') {
    return;
  }

  const guiUrl = (process.env.RAG_GUI_URL ?? 'http://127.0.0.1:8787').replace(/\/+$/, '');
  try {
    const response = await fetch(`${guiUrl}/api/health`);
    if (response.ok) {
      return;
    }
  } catch {
    // Launch the backend below.
  }

  guiBackendProcess = spawn(process.execPath, [guiBackendPath], {
    cwd: moduleDir,
    detached: true,
    stdio: 'ignore',
    windowsHide: true
  });
  guiBackendProcess.unref();
}

async function ensureOptimumEmbeddingProvider(): Promise<void> {
  if (optimumSwitchPromise) {
    return optimumSwitchPromise;
  }

  optimumSwitchPromise = (async () => {
    const stats = await index.getStats();
    if (stats.embeddingProvider.startsWith('optimum:')) {
      return;
    }

    const optimumUrl = process.env.RAG_OPTIMUM_EMBEDDING_URL?.trim() || 'http://127.0.0.1:8123';
    try {
      const response = await fetch(`${optimumUrl}/health`);
      if (!response.ok) {
        return;
      }
    } catch {
      return;
    }

    await index.setEmbeddingProvider(
      createEmbeddingProvider({
        kind: 'optimum',
        optimumUrl,
        optimumModel: process.env.RAG_OPTIMUM_EMBEDDING_MODEL?.trim() || 'sentence-transformers/paraphrase-MiniLM-L3-v2'
      })
    );
  })().finally(() => {
    optimumSwitchPromise = null;
  });

  return optimumSwitchPromise;
}

async function queueSourceInGuiBackend(input: { sources: RagSourceInput[]; label?: string; concurrency?: number }): Promise<Record<string, unknown>> {
  return queueJson('/api/queue/ingest', {
    sources: input.sources,
    label: input.label,
    concurrency: input.concurrency
  });
}

async function queueDirectoryInGuiBackend(input: { directoryPath: string; recursive?: boolean; includeHidden?: boolean; concurrency?: number }): Promise<Record<string, unknown>> {
  try {
    return await queueJson('/api/ingest-dir', input);
  } catch (error) {
    return {
      queued: false,
      directoryPath: input.directoryPath,
      error: error instanceof Error ? error.message : String(error)
    };
  }
}

async function queueJson(pathname: string, body: Record<string, unknown>): Promise<Record<string, unknown>> {
  const response = await fetch(`${guiBaseUrl}${pathname}`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json'
    },
    body: JSON.stringify(body)
  });

  const payload = (await response.json().catch(() => ({}))) as Record<string, unknown>;
  if (!response.ok) {
    throw new Error(typeof payload.error === 'string' ? payload.error : `Queue request failed with HTTP ${response.status}`);
  }

  return payload;
}

main().catch((error) => {
  console.error('[rag-mcp-server] fatal startup error:', error);
  process.exit(1);
});

process.once('SIGINT', () => {
  if (guiBackendProcess?.pid) {
    try {
      process.kill(guiBackendProcess.pid);
    } catch {
      // Ignore shutdown race.
    }
  }
});

process.once('SIGTERM', () => {
  if (guiBackendProcess?.pid) {
    try {
      process.kill(guiBackendProcess.pid);
    } catch {
      // Ignore shutdown race.
    }
  }
});

function formatIngestResults(results: Array<{ sourceId: string; kind: string; locator: string; label: string; chunkCount: number; updatedAt: string; metrics?: Record<string, unknown> }>): string {
  const lines = ['Ingested sources:'];
  for (const result of results) {
    lines.push(
      `- ${result.label} [${result.kind}] (${result.chunkCount} chunks)`,
      `  locator: ${result.locator}`,
      `  sourceId: ${shortId(result.sourceId)} updatedAt: ${result.updatedAt}`
    );
    if (result.metrics) {
      lines.push(`  metrics: ${formatMetrics(result.metrics)}`);
    }
  }
  return lines.join('\n');
}

function formatQueuedIngestResults(payload: Record<string, unknown>): string {
  if (payload.jobId && typeof payload.jobId === 'string') {
    return `Queued source ingest job ${payload.jobId}.\nStatus: ${String(payload.status ?? 'queued')}`;
  }
  return `Queued source ingest request accepted.`;
}

function formatQueuedDirectoryResults(payload: Record<string, unknown>): string {
  if (payload.jobId && typeof payload.jobId === 'string') {
    return `Queued directory ingest job ${payload.jobId}.\nStatus: ${String(payload.status ?? 'queued')}`;
  }
  if (payload.queued === false) {
    return `Queued directory ingest unavailable: ${String(payload.error ?? 'unknown error')}`;
  }
  return `Queued directory ingest request accepted.`;
}

function normalizeSourceInput(source: unknown): RagSourceInput {
  if (typeof source !== 'object' || !source) {
    throw new Error('Each source must be an object.');
  }

  const raw = source as Record<string, unknown>;
  const locator = typeof raw.locator === 'string' ? raw.locator.trim() : '';
  if (!locator) {
    throw new Error('Each source needs a locator.');
  }

  return {
    kind: typeof raw.kind === 'string' && raw.kind.trim() ? raw.kind : 'file',
    locator,
    label: typeof raw.label === 'string' && raw.label.trim() ? raw.label : undefined,
    path: typeof raw.path === 'string' && raw.path.trim() ? raw.path : undefined,
    url: typeof raw.url === 'string' && raw.url.trim() ? raw.url : undefined,
    content: typeof raw.content === 'string' ? raw.content : undefined,
    mimeType: typeof raw.mimeType === 'string' && raw.mimeType.trim() ? raw.mimeType : undefined,
    metadata: raw.metadata && typeof raw.metadata === 'object' ? (raw.metadata as Record<string, unknown>) : undefined
  } satisfies RagSourceInput;
}

function formatQueryResults(query: string, results: Array<{ score: number; snippet: string; sourceId: string; sourceKind: string; locator: string; label: string; title: string; chunkId: string; startLine: number; endLine: number; heading?: string }>): string {
  if (!results.length) {
    return `No matches found for "${query}".`;
  }

  const lines = [`Top matches for "${query}":`];
  results.forEach((result, index) => {
    lines.push('');
    lines.push(`${index + 1}. ${result.label} [${result.sourceKind}]`);
    lines.push(`   sourceId: ${shortId(result.sourceId)}  chunkId: ${shortId(result.chunkId)}  score: ${result.score.toFixed(3)}`);
    lines.push(`   citation: ${result.locator}:${result.startLine}-${result.endLine}`);
    if (result.heading) {
      lines.push(`   heading: ${result.heading}`);
    }
    lines.push(`   snippet: ${result.snippet}`);
  });

  return lines.join('\n');
}

function formatSources(sources: Array<{ sourceId: string; kind: string; locator: string; label: string; title: string; chunkCount: number; updatedAt: string }>): string {
  if (!sources.length) {
    return 'No sources are indexed yet.';
  }

  const lines = ['Indexed sources:'];
  for (const source of sources) {
    lines.push(
      `- ${source.label} [${source.kind}] (${source.chunkCount} chunks)`,
      `  locator: ${source.locator}`,
      `  sourceId: ${shortId(source.sourceId)}  updatedAt: ${source.updatedAt}`
    );
  }

  return lines.join('\n');
}

function formatChunks(sourceId: string, chunks: Array<{ chunkId: string; text: string; startLine: number; endLine: number; tokenCount: number; heading?: string }>): string {
  if (!chunks.length) {
    return `No chunks found for source ${shortId(sourceId)}.`;
  }

  const lines = [`Chunks for ${shortId(sourceId)}:`];
  for (const chunk of chunks) {
    lines.push('');
    lines.push(`- chunkId: ${shortId(chunk.chunkId)}  lines: ${chunk.startLine}-${chunk.endLine}  tokens: ${chunk.tokenCount}`);
    if (chunk.heading) {
      lines.push(`  heading: ${chunk.heading}`);
    }
    lines.push(`  text: ${chunk.text}`);
  }

  return lines.join('\n');
}

function formatMetrics(metrics: Record<string, unknown>): string {
  const keys = ['documentCount', 'chunkCount', 'avgChunkChars', 'avgChunkTokens', 'loadMs', 'chunkMs', 'embedMs', 'commitMs', 'totalMs', 'batchSize', 'batchCount', 'cacheHits', 'cacheMisses', 'reusedEmbeddings'];
  return keys
    .filter((key) => metrics[key] !== undefined)
    .map((key) => `${key}=${String(metrics[key])}`)
    .join(', ');
}

function shortId(value: string): string {
  return value.length <= 12 ? value : `${value.slice(0, 12)}...`;
}
