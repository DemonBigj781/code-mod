import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { randomUUID } from 'node:crypto';

import { createEmbeddingProvider } from './embeddings.js';
import { IngestQueueManager } from './ingest-queue.js';
import { RagIndex, type RagSourceInput } from './rag.js';
import { resolveDataDir, resolveEmbeddingOptions } from './runtime.js';

const PORT = Number(process.env.RAG_GUI_PORT ?? '8787');
const REVISION = `${Date.now()}-${process.pid}`;
const DATA_DIR = resolveDataDir();
const SETTINGS_PATH = path.join(DATA_DIR, 'gui-settings.json');

const embeddingOptions = resolveEmbeddingOptions();
const index = new RagIndex({
  dataDir: DATA_DIR,
  embeddingProvider: createEmbeddingProvider(embeddingOptions),
  embeddingBatchSize: embeddingOptions.embeddingBatchSize
});

const ingestQueue = new IngestQueueManager(index, DATA_DIR);
let shutdownPromise: Promise<void> | null = null;
let optimumSwitchPromise: Promise<void> | null = null;

interface GuiSettings {
  workerCount?: number;
}

const server = createServer(async (req: IncomingMessage, res: ServerResponse) => {
  try {
    const url = new URL(req.url ?? '/', `http://${req.headers.host ?? 'localhost'}`);

    if (req.method === 'GET' && url.pathname === '/') {
      return sendHtml(res, renderPage());
    }

    if (req.method === 'GET' && url.pathname === '/api/health') {
      await ensureOptimumEmbeddingProvider();
      const stats = await index.getStats();
      return sendJson(res, { ok: true, revision: REVISION, ...stats });
    }

    if (req.method === 'GET' && url.pathname === '/api/sources') {
      await ensureOptimumEmbeddingProvider();
      const kindsParam = url.searchParams.get('kinds');
      const sources = await index.listSources(
        kindsParam ? { kinds: kindsParam.split(',').map((kind: string) => kind.trim()).filter(Boolean) } : undefined
      );
      return sendJson(res, { sources });
    }

    if (req.method === 'GET' && url.pathname === '/api/chunks') {
      await ensureOptimumEmbeddingProvider();
      const sourceId = url.searchParams.get('sourceId');
      if (!sourceId) {
        return sendJson(res, { error: 'sourceId is required' }, 400);
      }
      const offset = parseOptionalInt(url.searchParams.get('offset'));
      const limit = parseOptionalInt(url.searchParams.get('limit'));
      const chunks = await index.listChunks(sourceId, { offset: offset ?? undefined, limit: limit ?? undefined });
      return sendJson(res, { sourceId, chunks });
    }

    if (req.method === 'POST' && url.pathname === '/api/ingest') {
      await ensureOptimumEmbeddingProvider();
      const body = (await readJsonBody(req)) as Record<string, unknown>;
      const sources = Array.isArray(body.sources) ? (body.sources as unknown[]) : [];
      if (!sources.length) {
        return sendJson(res, { error: 'sources must be a non-empty array' }, 400);
      }
      const results = await index.ingestSources(sources.map(normalizeSourceInput));
      const cleaned = await index.cleanupBinarySources();
      return sendJson(res, { results, cleaned });
    }

    if (req.method === 'POST' && url.pathname === '/api/queue/ingest') {
      await ensureOptimumEmbeddingProvider();
      const body = (await readJsonBody(req)) as Record<string, unknown>;
      const sources = Array.isArray(body.sources) ? (body.sources as unknown[]) : [];
      if (!sources.length) {
        return sendJson(res, { error: 'sources must be a non-empty array' }, 400);
      }

      const job = await ingestQueue.enqueueSourcesJob({
        sources: sources.map(normalizeSourceInput),
        label: typeof body.label === 'string' ? body.label : 'rag_ingest batch',
        concurrency: parseOptionalInt(body.concurrency) ?? undefined
      });

      return sendJson(res, { jobId: job.id, status: job.status, job });
    }

    if (req.method === 'POST' && url.pathname === '/api/ingest-dir') {
      await ensureOptimumEmbeddingProvider();
      const body = (await readJsonBody(req)) as Record<string, unknown>;
      const directoryPath = normalizePathInput(typeof body.directoryPath === 'string' ? body.directoryPath : '');
      if (!directoryPath) {
        return sendJson(res, { error: 'directoryPath is required' }, 400);
      }
      const job = await ingestQueue.enqueueDirectoryJob({
        directoryPath,
        recursive: body.recursive !== false,
        includeHidden: body.includeHidden === true,
        concurrency: parseOptionalInt(body.concurrency) ?? undefined
      });
      return sendJson(res, { directoryPath, jobId: job.id, status: job.status, job });
    }

    if (req.method === 'GET' && url.pathname === '/api/jobs') {
      await ensureOptimumEmbeddingProvider();
      const jobId = url.searchParams.get('jobId');
      if (!jobId) {
        return sendJson(res, { error: 'jobId is required' }, 400);
      }
      const job = await ingestQueue.getJob(jobId);
      if (!job) {
        return sendJson(res, { error: 'job not found' }, 404);
      }
      return sendJson(res, { job });
    }

    if (req.method === 'GET' && url.pathname === '/api/queue') {
      await ensureOptimumEmbeddingProvider();
      const jobs = await ingestQueue.listJobs();
      const paused = await ingestQueue.isPaused();
      return sendJson(res, { jobs, paused });
    }

    if (req.method === 'POST' && url.pathname === '/api/queue/clear') {
      await ensureOptimumEmbeddingProvider();
      await ingestQueue.clearQueue();
      return sendJson(res, { ok: true });
    }

    if (req.method === 'POST' && url.pathname === '/api/queue/clear-completed') {
      await ensureOptimumEmbeddingProvider();
      const removed = await ingestQueue.clearCompletedJobs();
      return sendJson(res, { ok: true, removed });
    }

    if (req.method === 'POST' && url.pathname === '/api/queue/pause') {
      await ensureOptimumEmbeddingProvider();
      const interrupted = await ingestQueue.pauseQueue();
      return sendJson(res, { ok: true, interrupted });
    }

    if (req.method === 'POST' && url.pathname === '/api/queue/resume') {
      await ensureOptimumEmbeddingProvider();
      const resumed = await ingestQueue.resumeQueue();
      return sendJson(res, { ok: true, resumed });
    }

    if (req.method === 'GET' && url.pathname === '/api/settings') {
      await ensureOptimumEmbeddingProvider();
      const settings = await loadGuiSettings();
      return sendJson(res, { settings });
    }

    if (req.method === 'POST' && url.pathname === '/api/settings') {
      await ensureOptimumEmbeddingProvider();
      const body = (await readJsonBody(req)) as Record<string, unknown>;
      const workerCount = parseOptionalInt(body.workerCount);
      const settings = await saveGuiSettings({
        workerCount: workerCount && workerCount > 0 ? workerCount : undefined
      });
      if (typeof settings.workerCount === 'number') {
        await index.updateEmbeddingWorkerCount(settings.workerCount);
      }
      return sendJson(res, { settings });
    }

    if (req.method === 'POST' && url.pathname === '/api/query') {
      await ensureOptimumEmbeddingProvider();
      const body = (await readJsonBody(req)) as Record<string, unknown>;
      const query = typeof body.query === 'string' ? body.query.trim() : '';
      if (!query) {
        return sendJson(res, { error: 'query is required' }, 400);
      }

      const results = await index.search({
        query,
        topK: parseOptionalInt(body.topK) ?? 5,
        sourceIds: Array.isArray(body.sourceIds) ? body.sourceIds.filter(isNonEmptyString) : undefined,
        sourceKinds: Array.isArray(body.sourceKinds) ? body.sourceKinds.filter(isNonEmptyString) : undefined
      });

      return sendJson(res, { query, matches: results });
    }

    return sendJson(res, { error: 'Not found' }, 404);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return sendJson(res, { error: message }, 500);
  }
});

async function main(): Promise<void> {
  await index.ensureReady();
  await ensureOptimumEmbeddingProvider();
  const settings = await loadGuiSettings();
  if (typeof settings.workerCount === 'number') {
    await index.updateEmbeddingWorkerCount(settings.workerCount);
  }
  setupShutdownHandlers();
  server.listen(PORT, () => {
    console.log(`RAG GUI listening on http://localhost:${PORT}`);
  });
}

async function ensureOptimumEmbeddingProvider(): Promise<void> {
  if (optimumSwitchPromise) {
    return optimumSwitchPromise;
  }

  optimumSwitchPromise = (async () => {
    const embeddingProvider = (await index.getStats()).embeddingProvider;
    if (embeddingProvider.startsWith('optimum:')) {
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

main().catch((error) => {
  console.error('[rag-mcp-server gui] fatal startup error:', error);
  process.exit(1);
});

function setupShutdownHandlers(): void {
  const handle = (signal: NodeJS.Signals): void => {
    void shutdown(signal);
  };

  process.once('SIGINT', handle);
  process.once('SIGTERM', handle);
}

async function shutdown(signal: NodeJS.Signals): Promise<void> {
  if (shutdownPromise) {
    return shutdownPromise;
  }

  shutdownPromise = (async () => {
    try {
      await ingestQueue.interruptActiveJobs(`Interrupted by ${signal}`);
      await index.closeWatchers();
      await new Promise<void>((resolve) => {
        server.close(() => resolve());
      });
    } catch (error) {
      console.error('[rag-mcp-server gui] shutdown error:', error);
    } finally {
      process.exit(0);
    }
  })();

  return shutdownPromise;
}

function renderPage(): string {
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>RAG MCP GUI</title>
  <style>
    :root {
      color-scheme: dark;
      --bg: #0b1020;
      --panel: rgba(15, 23, 42, 0.84);
      --panel-strong: rgba(17, 24, 39, 0.95);
      --border: rgba(148, 163, 184, 0.2);
      --text: #e5eefc;
      --muted: #9fb0c8;
      --accent: #7dd3fc;
      --accent-2: #a78bfa;
      --good: #34d399;
      --bad: #fb7185;
      --shadow: 0 20px 80px rgba(0, 0, 0, 0.38);
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      color: var(--text);
      min-height: 100vh;
      background:
        radial-gradient(circle at top left, rgba(125, 211, 252, 0.18), transparent 30%),
        radial-gradient(circle at top right, rgba(167, 139, 250, 0.18), transparent 30%),
        linear-gradient(180deg, #050816 0%, #0b1020 55%, #070b15 100%);
    }
    header {
      padding: 32px 20px 12px;
      max-width: 1320px;
      margin: 0 auto;
    }
    h1 {
      margin: 0;
      font-size: clamp(28px, 4vw, 50px);
      letter-spacing: -0.04em;
    }
    .subtitle {
      color: var(--muted);
      margin-top: 8px;
      max-width: 72ch;
      line-height: 1.5;
    }
    main {
      max-width: 1320px;
      margin: 0 auto;
      padding: 12px 20px 40px;
      display: grid;
      grid-template-columns: 1.05fr 0.95fr;
      gap: 18px;
    }
    .panel {
      background: var(--panel);
      border: 1px solid var(--border);
      border-radius: 18px;
      box-shadow: var(--shadow);
      backdrop-filter: blur(12px);
      overflow: hidden;
    }
    .panel h2 {
      margin: 0;
      padding: 18px 18px 0;
      font-size: 18px;
      letter-spacing: -0.02em;
    }
    .panel-body {
      padding: 18px;
    }
    .grid {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 14px;
    }
    .field {
      display: flex;
      flex-direction: column;
      gap: 8px;
    }
    .field.full { grid-column: 1 / -1; }
    label {
      font-size: 13px;
      color: var(--muted);
      text-transform: uppercase;
      letter-spacing: 0.08em;
    }
    input, textarea, select, button {
      width: 100%;
      border-radius: 12px;
      border: 1px solid rgba(148, 163, 184, 0.2);
      background: var(--panel-strong);
      color: var(--text);
      padding: 12px 14px;
      font: inherit;
    }
    textarea {
      resize: vertical;
      min-height: 110px;
      line-height: 1.5;
    }
    button {
      cursor: pointer;
      background: linear-gradient(135deg, rgba(125, 211, 252, 0.2), rgba(167, 139, 250, 0.2));
      transition: transform 120ms ease, border-color 120ms ease, background 120ms ease;
      font-weight: 600;
    }
    button:hover {
      transform: translateY(-1px);
      border-color: rgba(125, 211, 252, 0.45);
      background: linear-gradient(135deg, rgba(125, 211, 252, 0.32), rgba(167, 139, 250, 0.26));
    }
    .row {
      display: flex;
      gap: 12px;
      align-items: center;
      flex-wrap: wrap;
    }
    .row > * { flex: 1 1 0; }
    .stack {
      display: grid;
      gap: 14px;
    }
    .meta {
      display: flex;
      gap: 10px;
      flex-wrap: wrap;
      margin-top: 12px;
      color: var(--muted);
      font-size: 13px;
    }
    .pill {
      border: 1px solid rgba(148, 163, 184, 0.18);
      border-radius: 999px;
      padding: 6px 10px;
      background: rgba(15, 23, 42, 0.5);
    }
    pre {
      margin: 0;
      white-space: pre-wrap;
      word-break: break-word;
      font-size: 13px;
      line-height: 1.6;
      color: #dbeafe;
      background: rgba(2, 6, 23, 0.55);
      border: 1px solid rgba(148, 163, 184, 0.15);
      border-radius: 14px;
      padding: 14px;
      min-height: 120px;
    }
    .list {
      display: grid;
      gap: 12px;
    }
    .card {
      border: 1px solid rgba(148, 163, 184, 0.16);
      background: rgba(2, 6, 23, 0.45);
      border-radius: 14px;
      padding: 14px;
    }
    .card h3 {
      margin: 0 0 6px;
      font-size: 15px;
    }
    .card .small {
      color: var(--muted);
      font-size: 12px;
      line-height: 1.45;
    }
    .dropzone {
      border: 1.5px dashed rgba(125, 211, 252, 0.34);
      border-radius: 16px;
      padding: 18px;
      background: rgba(15, 23, 42, 0.45);
      color: var(--muted);
      text-align: center;
      transition: border-color 120ms ease, background 120ms ease, transform 120ms ease;
    }
    .dropzone.dragover {
      border-color: rgba(125, 211, 252, 0.8);
      background: rgba(15, 118, 110, 0.18);
      transform: translateY(-1px);
    }
    .two {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 14px;
    }
    .muted {
      color: var(--muted);
      font-size: 13px;
    }
    @media (max-width: 1100px) {
      main { grid-template-columns: 1fr; }
    }
    @media (max-width: 720px) {
      .grid, .two { grid-template-columns: 1fr; }
    }
  </style>
</head>
<body>
  <header>
    <h1>RAG MCP GUI</h1>
    <div class="subtitle">A local browser console for ingesting files or folders, querying the index, and inspecting retrieved evidence. This talks to the same persistent index used by the MCP server.</div>
    <div class="meta" id="stats"></div>
  </header>
  <main>
    <section class="panel">
      <h2>Ingest</h2>
      <div class="panel-body stack">
        <div class="card">
          <h3>Directory ingest</h3>
          <div class="muted" id="directoryProgress">No directory ingest running.</div>
          <pre id="ingestOutput" class="small" style="margin-top: 12px;">No ingest started yet.</pre>
          <div class="grid">
            <div class="field full">
              <label for="directoryPath">Directory path</label>
              <input id="directoryPath" placeholder="C:/notes or ./docs" />
            </div>
            <div class="field">
              <label for="recursive">Recursive</label>
              <select id="recursive">
                <option value="true">Yes</option>
                <option value="false">No</option>
              </select>
            </div>
            <div class="field">
              <label for="includeHidden">Include hidden</label>
              <select id="includeHidden">
                <option value="false">No</option>
                <option value="true">Yes</option>
              </select>
            </div>
          </div>
          <div class="row" style="margin-top: 12px;">
            <button id="ingestDirBtn">Ingest directory</button>
          </div>
          <div class="card" style="margin-top: 12px;">
            <h3>Ingest performance</h3>
            <div class="muted" id="ingestPerfSummary">No ingest metrics yet.</div>
            <pre id="ingestPerfOutput" class="small" style="margin-top: 12px;">Waiting for an ingest run.</pre>
          </div>
        </div>

        <div class="card">
          <h3>Queue</h3>
          <div class="muted">Queued, scanning, running, interrupted, and completed jobs.</div>
          <pre id="queueOutput">No queue loaded yet.</pre>
          <div class="row" style="margin-top: 12px;">
            <button id="refreshQueueBtn" type="button">Refresh queue</button>
            <button id="pauseQueueBtn" type="button">Pause queue</button>
            <button id="clearQueueBtn" type="button">Clear queue</button>
            <button id="clearCompletedBtn" type="button">Clear completed</button>
            <button id="resumeQueueBtn" type="button">Resume queue</button>
          </div>
        </div>

        <div class="card">
          <h3>Embedding workers</h3>
          <div class="grid">
            <div class="field full">
              <label for="workerCount">Local worker count</label>
              <input id="workerCount" type="number" min="1" max="16" step="1" placeholder="Auto" />
            </div>
            <div class="field full">
              <div class="muted" id="workerStatus">Using automatic worker count.</div>
            </div>
          </div>
          <div class="row" style="margin-top: 12px;">
            <button id="saveWorkersBtn" type="button">Save worker count</button>
          </div>
        </div>

        <div class="card">
          <h3>Upload files</h3>
          <div class="grid">
            <div class="field full">
              <label for="fileInput">Choose files</label>
              <input id="fileInput" type="file" multiple />
            </div>
            <div class="field full">
              <label for="folderInput">Choose folder</label>
              <input id="folderInput" type="file" webkitdirectory directory multiple />
            </div>
            <div class="field full">
              <div class="muted">Files are read in the browser and sent as inline content, so this works even when the browser cannot expose a real local path.</div>
            </div>
          </div>
          <div class="row" style="margin-top: 12px;">
            <button id="ingestFilesBtn">Ingest selected files</button>
            <button id="ingestFolderBtn" type="button">Ingest selected folder</button>
          </div>
        </div>

        <div class="dropzone" id="dropzone">
          Drop files or folders here to ingest immediately
        </div>

        <div class="card">
          <h3>Bulk file ingest</h3>
          <div class="grid">
            <div class="field">
              <label for="bulkKind">Default kind</label>
              <select id="bulkKind">
                <option value="file">file</option>
                <option value="code">code</option>
                <option value="text">text</option>
                <option value="url">url</option>
              </select>
            </div>
            <div class="field">
              <label for="bulkLabel">Default label prefix</label>
              <input id="bulkLabel" placeholder="Optional" />
            </div>
            <div class="field full">
              <label for="bulkSources">Sources, one per line</label>
              <textarea id="bulkSources" placeholder="C:/path/to/a.md&#10;https://example.com/docs&#10;C:/path/to/app.ts"></textarea>
            </div>
          </div>
          <div class="row" style="margin-top: 12px;">
            <button id="ingestBulkBtn">Ingest entries</button>
          </div>
        </div>
      </div>
    </section>

    <section class="panel">
      <h2>Query</h2>
      <div class="panel-body stack">
        <div class="card">
          <h3>Search the index</h3>
          <div class="grid">
            <div class="field full">
              <label for="queryText">Query</label>
              <textarea id="queryText" placeholder="What do we already know about source code ingestion?"></textarea>
            </div>
            <div class="field">
              <label for="queryTopK">Top K</label>
              <input id="queryTopK" type="number" min="1" max="20" value="5" />
            </div>
            <div class="field">
              <label for="sourceFilter">Source filter (comma separated ids)</label>
              <input id="sourceFilter" placeholder="Optional" />
            </div>
          </div>
          <div class="row" style="margin-top: 12px;">
            <button id="queryBtn">Query index</button>
            <button id="refreshBtn" type="button">Search sources</button>
          </div>
          <div class="row" style="margin-top: 12px;">
            <div class="field full">
              <label for="sourceSearchText">Source search</label>
              <input id="sourceSearchText" placeholder="Search indexed sources by label or path" />
            </div>
          </div>
        </div>

        <div class="two">
          <div class="card">
            <h3>Results</h3>
            <pre id="queryOutput">No search has been run yet.</pre>
          </div>
          <div class="card">
            <h3>Indexed sources</h3>
            <pre id="sourcesOutput">No source inventory loaded yet.</pre>
          </div>
        </div>
      </div>
    </section>
  </main>

  <script>
    const el = (id) => document.getElementById(id);
    let currentRevision = null;
    let reloadTimer = null;
    let refreshTimer = null;
    const LAST_QUERY_KEY = 'rag-mcp:lastQuery';
    const LAST_QUEUE_KEY = 'rag-mcp:lastQueue';
    const LAST_PERF_KEY = 'rag-mcp:lastPerf';

    async function jsonFetch(url, options) {
      const response = await fetch(url, {
        headers: { 'content-type': 'application/json' },
        ...options
      });
      const payload = await response.json();
      if (!response.ok) {
        throw new Error(payload.error || \`Request failed with HTTP \${response.status}\`);
      }
      return payload;
    }

    function setStats(stats) {
      currentRevision = stats.revision ?? currentRevision;
      el('stats').innerHTML = [
        \`<span class="pill">Sources: \${stats.sourceCount ?? 0}</span>\`,
        \`<span class="pill">Chunks: \${stats.chunkCount ?? 0}</span>\`,
        \`<span class="pill">Embedding: \${stats.embeddingProvider ?? 'unknown'}</span>\`,
        \`<span class="pill">Updated: \${stats.updatedAt ?? 'n/a'}</span>\`
      ].join('');
    }

    function setIngestPerformance(summary, metrics) {
      if (!summary || !metrics) {
        el('ingestPerfSummary').textContent = 'No ingest metrics yet.';
        el('ingestPerfOutput').textContent = 'Waiting for an ingest run.';
        return;
      }

      const label = summary.label || 'Last ingest';
      const bits = [];
      if (typeof metrics.documentCount === 'number') bits.push(\`docs \${metrics.documentCount}\`);
      if (typeof metrics.chunkCount === 'number') bits.push(\`chunks \${metrics.chunkCount}\`);
      if (typeof metrics.batchCount === 'number') bits.push(\`batches \${metrics.batchCount}\`);
      if (typeof metrics.batchSize === 'number') bits.push(\`batch size \${metrics.batchSize}\`);
      if (typeof metrics.reusedEmbeddings === 'number') bits.push(\`reused \${metrics.reusedEmbeddings}\`);
      if (typeof metrics.embedMs === 'number') bits.push(\`embed \${metrics.embedMs} ms\`);
      el('ingestPerfSummary').textContent = \`\${label}\${bits.length ? \` · \${bits.join(' · ')}\` : ''}\`;
      el('ingestPerfOutput').textContent = formatIngestPerformance(metrics);
      saveStoredText(LAST_PERF_KEY, JSON.stringify({ label, metrics }, null, 2));
    }

    function formatIngestPerformance(metrics) {
      if (!metrics || typeof metrics !== 'object') {
        return 'No ingest metrics yet.';
      }

      return [
        \`filesProcessed: \${metrics.filesProcessed ?? metrics.documentCount ?? 0}\`,
        \`filesSkipped: \${metrics.filesSkipped ?? 0}\`,
        \`chunksCreated: \${metrics.chunksCreated ?? metrics.chunkCount ?? 0}\`,
        \`chunksCacheHits: \${metrics.chunksCacheHits ?? metrics.cacheHits ?? 0}\`,
        \`chunksEmbedded: \${metrics.chunksEmbedded ?? metrics.cacheMisses ?? 0}\`,
        \`documentCount: \${metrics.documentCount ?? 0}\`,
        \`chunkCount: \${metrics.chunkCount ?? 0}\`,
        \`avgChunkChars: \${metrics.avgChunkChars ?? 0}\`,
        \`avgChunkTokens: \${metrics.avgChunkTokens ?? 0}\`,
        \`loadMs: \${metrics.loadMs ?? 0}\`,
        \`chunkMs: \${metrics.chunkMs ?? 0}\`,
        \`embedMs: \${metrics.embedMs ?? 0}\`,
        \`commitMs: \${metrics.commitMs ?? 0}\`,
        \`persistMs: \${metrics.persistMs ?? 0}\`,
        \`totalMs: \${metrics.totalMs ?? 0}\`,
        \`batchSize: \${metrics.batchSize ?? 0}\`,
        \`batchCount: \${metrics.batchCount ?? 0}\`,
        \`cacheHits: \${metrics.cacheHits ?? 0}\`,
        \`cacheMisses: \${metrics.cacheMisses ?? 0}\`,
        \`reusedEmbeddings: \${metrics.reusedEmbeddings ?? 0}\`
      ].join('\\n');
    }

    function applyIngestPerformanceFromPayload(payload, label) {
      if (payload && Array.isArray(payload.results) && payload.results.length) {
        const first = payload.results[0];
        if (first && first.metrics) {
          setIngestPerformance({ label: label || first.label || 'Direct ingest' }, first.metrics);
          return;
        }
      }

      if (payload && payload.job && payload.job.result && payload.job.result.metrics) {
        setIngestPerformance({ label: label || 'Queued ingest' }, payload.job.result.metrics);
      }
    }

    function formatSources(sources) {
      if (!sources.length) return 'No sources are indexed yet.';
      return sources.map((source) => [
        \`- \${source.label} [\${source.kind}] (\${source.chunkCount} chunks)\`,
        \`  sourceId: \${source.sourceId}\`,
        \`  locator: \${source.locator}\`,
        \`  updatedAt: \${source.updatedAt}\`
      ].join('\\n')).join('\\n\\n');
    }

    function formatQueue(jobs, paused) {
      if (!jobs.length) return paused ? 'Queue is paused.\\n\\nNo queue jobs yet.' : 'No queue jobs yet.';
      const header = paused ? ['Queue is paused.'] : [];
      return header.concat(jobs.map((job) => [
        \`- \${job.id} [\${job.status}]\`,
        job.kind === 'sources' ? \`  target: \${job.directoryPath}\` : \`  path: \${job.directoryPath}\`,
        \`  startedAt: \${job.startedAt}\`,
        job.progress ? formatJobProgressLine(job.progress) : null,
        job.progress ? formatQueueEtaLine(job.progress, job.startedAt) : null,
        job.progress ? formatQueueChunksPerSecondLine(job.progress, job.startedAt) : null,
        job.progress ? formatQueueCurrentPathLine(job.progress) : null,
        job.kind === 'sources' && typeof job.totalSources === 'number'
          ? \`  sources: \${job.completedSources ?? 0}/\${job.totalSources}\`
          : null,
        job.progress?.visitedDirectories !== undefined || job.progress?.visitedEntries !== undefined || job.progress?.discoveredFiles !== undefined
          ? \`  scan: dirs=\${job.progress.visitedDirectories ?? 0}, entries=\${job.progress.visitedEntries ?? 0}, files=\${job.progress.discoveredFiles ?? 0}\`
          : null,
        job.progress?.detail ? \`  detail: \${job.progress.detail}\` : null
      ].filter(Boolean).join('\\n'))).join('\\n\\n');
    }

    function formatQueueEtaLine(progress, startedAt) {
      const processed = getEtaProcessed(progress);
      const total = progress.total ?? 0;
      const etaText = getEtaText(progress, startedAt, progress.stageStartedAt ?? startedAt, total, processed);
      if (!etaText) {
        return null;
      }
      return \`  ETA: \${etaText}\`;
    }

    function formatJobProgressLine(progress) {
      const isScanning = progress.stage === 'scanning' || /discover/i.test(progress.detail || '');
      if (isScanning) {
        const scanCounts = [
          progress.visitedDirectories !== undefined ? \`dirs=\${progress.visitedDirectories}\` : null,
          progress.visitedEntries !== undefined ? \`entries=\${progress.visitedEntries}\` : null,
          progress.discoveredFiles !== undefined ? \`files=\${progress.discoveredFiles}\` : null
        ].filter(Boolean);
        if (scanCounts.length) {
          return 'Discovery: ' + scanCounts.join(', ');
        }
        return 'Discovery: ' + progress.processed;
      }

      if (progress.fileProcessed !== undefined && progress.fileTotal !== undefined) {
        return 'File progress: ' + progress.fileProcessed + '/' + progress.fileTotal;
      }

      if (typeof progress.total === 'number' && progress.total > 0) {
        return 'Progress: ' + progress.processed + '/' + progress.total;
      }

      return 'Progress: ' + progress.processed;
    }

    function formatJobCurrentPathLine(progress) {
      if (!progress.currentPath) {
        return null;
      }
      const isScanning = progress.stage === 'scanning' || /discover/i.test(progress.detail || '');
      return isScanning ? 'Current folder: ' + progress.currentPath : 'Current file: ' + progress.currentPath;
    }

    function formatQueueChunksPerSecondLine(progress, startedAt) {
      const rateText = getChunksPerSecondText(progress);
      if (!rateText) {
        return null;
      }
      return \`  chunks/s: \${rateText}\`;
    }

    function formatQueueCurrentPathLine(progress) {
      if (!progress.currentPath) {
        return null;
      }
      const isScanning = progress.stage === 'scanning' || /discover/i.test(progress.detail || '');
      return isScanning
        ? \`  current folder: \${progress.currentPath}\`
        : \`  current file: \${progress.currentPath}\`;
    }

    function loadStoredText(key) {
      try {
        return window.localStorage.getItem(key) || '';
      } catch {
        return '';
      }
    }

    function saveStoredText(key, value) {
      try {
        if (value) {
          window.localStorage.setItem(key, value);
        } else {
          window.localStorage.removeItem(key);
        }
      } catch {
        // Ignore storage failures.
      }
    }

    function filterSources(sources, term) {
      const normalized = (term || '').trim().toLowerCase();
      if (!normalized) {
        return sources;
      }
      return sources.filter((source) => {
        const haystack = [
          source.label,
          source.locator,
          source.title,
          source.kind
        ].filter(Boolean).join(' ').toLowerCase();
        return haystack.includes(normalized);
      });
    }

    function formatMatches(query, matches) {
      if (!matches.length) return \`No matches found for "\${query}".\`;
      return [\`Top filtered matches for "\${query}" (\${matches.length}):\`].concat(matches.map((match, index) => [
        '',
        \`\${index + 1}. \${match.label} [\${match.sourceKind}]\`,
        \`   sourceId: \${match.sourceId}\`,
        \`   citation: \${match.locator}:\${match.startLine}-\${match.endLine}\`,
        typeof match.score === 'number' ? \`   score: \${match.score.toFixed(3)}\` : null,
        match.heading ? \`   heading: \${match.heading}\` : null,
        \`   snippet: \${match.snippet}\`
      ].filter(Boolean).join('\\n'))).join('\\n');
    }

    async function refreshHealth() {
      const stats = await jsonFetch('/api/health');
      if (currentRevision && stats.revision && stats.revision !== currentRevision) {
        scheduleReload();
        return;
      }
      setStats(stats);
    }

    async function refreshSources() {
      const payload = await jsonFetch('/api/sources');
      const term = el('sourceSearchText').value.trim();
      const filtered = filterSources(payload.sources || [], term);
      el('sourcesOutput').textContent = term
        ? \`Search sources for "\${term}"\\n\\n\${formatSources(filtered)}\`
        : 'Source inventory loaded.\\n\\n' + formatSources(filtered);
      await refreshHealth();
    }

    async function refreshQueue() {
      const payload = await jsonFetch('/api/queue');
      const formatted = formatQueue(payload.jobs || [], Boolean(payload.paused));
      el('queueOutput').textContent = formatted;
      saveStoredText(LAST_QUEUE_KEY, formatted);
    }

    async function refreshSettings() {
      const payload = await jsonFetch('/api/settings');
      const workerCount = payload.settings?.workerCount;
      if (typeof workerCount === 'number') {
        el('workerCount').value = String(workerCount);
        el('workerStatus').textContent = 'Using ' + workerCount + ' worker' + (workerCount === 1 ? '' : 's') + '.';
      } else {
        el('workerCount').value = '';
        el('workerStatus').textContent = 'Using automatic worker count.';
      }
    }

    el('ingestDirBtn').addEventListener('click', async () => {
      const body = {
        directoryPath: el('directoryPath').value.trim(),
        recursive: el('recursive').value === 'true',
        includeHidden: el('includeHidden').value === 'true'
      };
      try {
        const payload = await jsonFetch('/api/ingest-dir', {
          method: 'POST',
          body: JSON.stringify(body)
        });
        if (payload.jobId) {
          el('ingestOutput').textContent = \`Directory ingest started for \${payload.directoryPath}\\nJob: \${payload.jobId}\\nStatus: \${payload.status}\`;
          setDirectoryProgress({ status: 'running', processed: 0, total: 0, currentPath: payload.directoryPath }, payload.startedAt);
          await refreshQueue();
          await waitForJob(payload.jobId);
          return;
        }

        el('ingestOutput').textContent = JSON.stringify(payload, null, 2);
        await refreshSources();
        await refreshQueue();
      } catch (error) {
        el('ingestOutput').textContent = error.message;
      }
    });

    const dropzone = el('dropzone');
    const dragDepth = { value: 0 };

    dropzone.addEventListener('dragenter', (event) => {
      event.preventDefault();
      dragDepth.value += 1;
      dropzone.classList.add('dragover');
    });

    dropzone.addEventListener('dragover', (event) => {
      event.preventDefault();
      dropzone.classList.add('dragover');
    });

    dropzone.addEventListener('dragleave', (event) => {
      event.preventDefault();
      dragDepth.value = Math.max(0, dragDepth.value - 1);
      if (dragDepth.value === 0) {
        dropzone.classList.remove('dragover');
      }
    });

    dropzone.addEventListener('drop', async (event) => {
      event.preventDefault();
      dragDepth.value = 0;
      dropzone.classList.remove('dragover');

      try {
        const files = await collectDroppedFiles(event.dataTransfer);
        if (!files.length) {
          el('ingestOutput').textContent = 'Drop files or folders to ingest.';
          return;
        }

        await ingestBrowserFiles(files);
        await refreshQueue();
      } catch (error) {
        el('ingestOutput').textContent = error.message;
      }
    });

    el('ingestFilesBtn').addEventListener('click', async () => {
      const input = el('fileInput');
      const files = Array.from(input.files || []);
      if (!files.length) {
        el('ingestOutput').textContent = 'Choose one or more files first.';
        return;
      }

      try {
        await ingestBrowserFiles(files);
        await refreshQueue();
      } catch (error) {
        el('ingestOutput').textContent = error.message;
      }
    });

    el('ingestFolderBtn').addEventListener('click', async () => {
      const input = el('folderInput');
      const files = Array.from(input.files || []);
      if (!files.length) {
        el('ingestOutput').textContent = 'Choose a folder first.';
        return;
      }

      try {
        await ingestBrowserFiles(files);
      } catch (error) {
        el('ingestOutput').textContent = error.message;
      }
    });

    el('ingestBulkBtn').addEventListener('click', async () => {
      const lines = el('bulkSources').value.split('\\n').map((line) => line.trim()).filter(Boolean);
      const kind = el('bulkKind').value;
      const labelPrefix = el('bulkLabel').value.trim();
      const sources = lines.map((line, index) => {
        const isUrl = /^https?:\\/\\//i.test(line);
        return {
          kind: isUrl ? 'url' : kind,
          locator: line,
          path: isUrl ? undefined : line,
          url: isUrl ? line : undefined,
          label: labelPrefix ? \`\${labelPrefix} \${index + 1}\` : undefined
        };
      });
      try {
        const payload = await jsonFetch('/api/ingest', {
          method: 'POST',
          body: JSON.stringify({ sources })
        });
        el('ingestOutput').textContent = JSON.stringify(payload, null, 2);
        applyIngestPerformanceFromPayload(payload, 'Bulk ingest');
        await refreshSources();
        await refreshQueue();
      } catch (error) {
        el('ingestOutput').textContent = error.message;
      }
    });

    el('queryBtn').addEventListener('click', async () => {
      const query = el('queryText').value.trim();
      saveStoredText(LAST_QUERY_KEY, query);
      if (!query) {
        el('queryOutput').textContent = 'Enter a query first.';
        return;
      }
      const sourceIds = el('sourceFilter').value.split(',').map((value) => value.trim()).filter(Boolean);
      el('queryOutput').textContent = 'Searching index...';
      try {
        const payload = await jsonFetch('/api/query', {
          method: 'POST',
          body: JSON.stringify({
            query,
            topK: Number(el('queryTopK').value || 5),
            sourceIds: sourceIds.length ? sourceIds : undefined
          })
        });
        el('queryOutput').textContent = 'Filtering results...';
        const filteredMatches = filterQueryMatches(payload.matches || []);
        el('queryOutput').textContent = formatMatches(payload.query, filteredMatches);
      } catch (error) {
        el('queryOutput').textContent = error.message;
      }
    });

    el('refreshBtn').addEventListener('click', async () => {
      try {
        await refreshSources();
      } catch (error) {
        el('sourcesOutput').textContent = error.message;
      }
    });

    el('refreshQueueBtn').addEventListener('click', async () => {
      try {
        await refreshQueue();
      } catch (error) {
        el('queueOutput').textContent = error.message;
      }
    });

    el('pauseQueueBtn').addEventListener('click', async () => {
      try {
        el('queueOutput').textContent = 'Pausing queue...';
        const payload = await jsonFetch('/api/queue/pause', { method: 'POST' });
        el('ingestOutput').textContent = payload.interrupted
          ? 'Paused queue and interrupted ' + payload.interrupted + ' active job' + (payload.interrupted === 1 ? '' : 's') + '.'
          : 'Queue paused.';
        await refreshQueue();
      } catch (error) {
        el('queueOutput').textContent = error.message;
      }
    });

    el('clearQueueBtn').addEventListener('click', async () => {
      try {
        el('queueOutput').textContent = 'Clearing queue...';
        await jsonFetch('/api/queue/clear', { method: 'POST' });
        el('directoryProgress').textContent = 'No directory ingest running.';
        el('ingestOutput').textContent = 'Queue cleared.';
        await refreshQueue();
      } catch (error) {
        el('queueOutput').textContent = error.message;
      }
    });

    el('clearCompletedBtn').addEventListener('click', async () => {
      try {
        el('queueOutput').textContent = 'Clearing completed jobs...';
        const payload = await jsonFetch('/api/queue/clear-completed', { method: 'POST' });
        el('ingestOutput').textContent = payload.removed
          ? 'Cleared ' + payload.removed + ' completed job' + (payload.removed === 1 ? '' : 's') + '.'
          : 'No completed jobs to clear.';
        await refreshQueue();
      } catch (error) {
        el('queueOutput').textContent = error.message;
      }
    });

    el('resumeQueueBtn').addEventListener('click', async () => {
      try {
        el('queueOutput').textContent = 'Resuming queue...';
        const payload = await jsonFetch('/api/queue/resume', { method: 'POST' });
        el('ingestOutput').textContent = payload.resumed
          ? 'Resumed ' + payload.resumed + ' queued job' + (payload.resumed === 1 ? '' : 's') + '.'
          : 'No interrupted jobs were waiting to resume.';
        await refreshQueue();
      } catch (error) {
        el('queueOutput').textContent = error.message;
      }
    });

    el('sourceSearchText').addEventListener('keydown', (event) => {
      if (event.key === 'Enter') {
        event.preventDefault();
        el('refreshBtn').click();
      }
    });

    el('queryText').value = loadStoredText(LAST_QUERY_KEY);
    if (!el('queryText').value) {
      const savedQueue = loadStoredText(LAST_QUEUE_KEY);
      if (savedQueue) {
        el('queueOutput').textContent = savedQueue;
      }
    }
    const savedPerf = loadStoredText(LAST_PERF_KEY);
    if (savedPerf) {
      try {
        const parsed = JSON.parse(savedPerf);
        setIngestPerformance({ label: parsed.label || 'Last ingest' }, parsed.metrics);
      } catch {
        // Ignore bad saved performance snapshots.
      }
    }
    el('queryText').addEventListener('input', () => {
      saveStoredText(LAST_QUERY_KEY, el('queryText').value.trim());
    });

    el('saveWorkersBtn').addEventListener('click', async () => {
      const rawValue = el('workerCount').value.trim();
      const workerCount = rawValue ? Number(rawValue) : undefined;
      try {
        const payload = await jsonFetch('/api/settings', {
          method: 'POST',
          body: JSON.stringify({
            workerCount: Number.isFinite(workerCount) && workerCount > 0 ? workerCount : undefined
          })
        });
        const saved = payload.settings?.workerCount;
        if (typeof saved === 'number') {
          el('workerCount').value = String(saved);
          el('workerStatus').textContent = 'Saved ' + saved + ' worker' + (saved === 1 ? '' : 's') + '.';
        } else {
          el('workerCount').value = '';
          el('workerStatus').textContent = 'Using automatic worker count.';
        }
      } catch (error) {
        el('workerStatus').textContent = error.message;
      }
    });

    refreshHealth().then(async () => {
      await refreshSettings();
      await refreshQueue();
    }).catch((error) => {
      el('sourcesOutput').textContent = error.message;
    });

    refreshTimer = window.setInterval(() => {
      refreshHealth().catch(() => {});
    }, 2000);

    function scheduleReload() {
      if (reloadTimer) {
        return;
      }
      reloadTimer = window.setTimeout(() => {
        window.location.reload();
      }, 250);
    }

    async function ingestBrowserFiles(files) {
      const sources = await Promise.all(files.map(async (file) => {
        const locator = file.webkitRelativePath || file.name;
        const content = await file.text();
        return {
          kind: inferKindFromName(locator),
          locator,
          label: locator,
          content,
          mimeType: file.type || undefined,
          metadata: {
            size: file.size,
            mimeType: file.type || undefined
          }
        };
      }));

      const payload = await jsonFetch('/api/ingest', {
        method: 'POST',
        body: JSON.stringify({ sources })
      });
      el('ingestOutput').textContent = JSON.stringify(payload, null, 2);
      applyIngestPerformanceFromPayload(payload, 'Direct ingest');
      await refreshSources();
      await refreshQueue();
    }

    async function waitForJob(jobId) {
      const poll = async () => {
        const payload = await jsonFetch(\`/api/jobs?jobId=\${encodeURIComponent(jobId)}\`);
        const job = payload.job;
        if (!job) {
          return;
        }

        if (job.progress) {
          setDirectoryProgress({
            status: job.status,
            processed: job.progress.processed,
            total: job.progress.total,
            currentPath: job.progress.currentPath,
            detail: job.progress.detail,
            fileProcessed: job.progress.fileProcessed,
            fileTotal: job.progress.fileTotal,
            visitedDirectories: job.progress.visitedDirectories,
            visitedEntries: job.progress.visitedEntries,
            discoveredFiles: job.progress.discoveredFiles
          }, job.startedAt);
        } else {
          setDirectoryProgress({
            status: job.status,
            processed: job.status === 'complete' ? 1 : 0,
            total: job.status === 'complete' ? 1 : 0,
            currentPath: job.error || job.completedAt ? undefined : 'Preparing...'
          }, job.startedAt);
        }

        await refreshQueue();

        el('ingestOutput').textContent = [
          'Directory ingest job ' + job.id,
          'Status: ' + job.status,
          job.progress ? formatJobProgressLine(job.progress) : null,
          job.progress ? formatJobCurrentPathLine(job.progress) : null,
          job.progress?.detail ? 'Detail: ' + job.progress.detail : null,
          job.progress?.fileProcessed !== undefined && job.progress?.fileTotal !== undefined
            ? 'File: ' + job.progress.fileProcessed + '/' + job.progress.fileTotal
            : null,
          job.updatedAt ? 'Updated: ' + job.updatedAt : null,
          job.error ? 'Error: ' + job.error : null
        ].filter(Boolean).join('\\n');

        if (job.status === 'complete') {
          if (job.result) {
            el('ingestOutput').textContent = JSON.stringify(job.result, null, 2);
            if (job.result.metrics) {
              setIngestPerformance({ label: job.kind === 'sources' ? 'Queued source ingest' : 'Queued directory ingest' }, job.result.metrics);
            }
          }
          await refreshSources();
          await refreshQueue();
          return;
        }

        if (job.status === 'error') {
          await refreshSources();
          await refreshQueue();
          return;
        }

        window.setTimeout(() => {
          poll().catch((error) => {
            el('ingestOutput').textContent = error.message;
          });
        }, 1500);
      };

      await poll();
    }

    function setDirectoryProgress(progress, startedAt) {
      const total = progress.total || 0;
      const processed = getEtaProcessed(progress);
      const percentText = getProgressPercentText(progress, total, processed);
      const etaText = getEtaText(progress, startedAt, progress.stageStartedAt ?? startedAt, total, processed);
      const folderParts = [];
      const fileParts = [];
      const isScanning = progress.stage === 'scanning' || /discover/i.test(progress.detail || '');
      if (isScanning) {
        if (progress.visitedDirectories !== undefined || progress.visitedEntries !== undefined || progress.discoveredFiles !== undefined) {
          folderParts.push(\`Scanned dirs: \${progress.visitedDirectories ?? 0}\`);
          folderParts.push(\`Scanned entries: \${progress.visitedEntries ?? 0}\`);
          folderParts.push(\`Discovered files: \${progress.discoveredFiles ?? 0}\`);
        } else {
          folderParts.push(\`Scanned: \${processed}\`);
        }
      } else if (total) {
        folderParts.push(processed + '/' + total);
      } else {
        folderParts.push(String(processed));
      }
      if (percentText) {
        folderParts.push(percentText);
      }
      if (etaText) {
        folderParts.push('ETA ' + etaText);
      }
      const rateText = getChunksPerSecondText(progress);
      if (rateText) {
        folderParts.push('Chunks/s ' + rateText);
      }
      if (progress.currentPath) {
        if (isScanning) {
          folderParts.push('Current folder: ' + progress.currentPath);
        } else {
          fileParts.push('Current file: ' + progress.currentPath);
        }
      }
      if (progress.fileProcessed !== undefined && progress.fileTotal !== undefined) {
        fileParts.push('File ' + progress.fileProcessed + '/' + progress.fileTotal);
      }
      if (!isScanning && progress.discoveredFiles !== undefined) {
        fileParts.push('Files discovered: ' + progress.discoveredFiles);
      }
      if (progress.detail) {
        fileParts.push(progress.detail);
      }
      const stageText = isScanning
        ? 'Scanning folder'
        : progress.stage === 'processing'
          ? 'Processing files'
          : progress.status === 'complete'
            ? 'Directory ingest complete'
            : 'Directory ingest running';
      const folderText = stageText + (folderParts.length ? ': ' + folderParts.join(', ') : '');
      const fileText = fileParts.length ? fileParts.join(', ') : '';
      const suffix = progress.status === 'error' ? '.' : '';
      el('directoryProgress').textContent = [folderText + suffix, fileText].filter(Boolean).join('\\n');
    }

    function filterQueryMatches(matches) {
      const seen = new Set();
      return (matches || [])
        .filter((match) => match && typeof match === 'object')
        .sort((a, b) => {
          const scoreA = typeof a.score === 'number' ? a.score : 0;
          const scoreB = typeof b.score === 'number' ? b.score : 0;
          return scoreB - scoreA;
        })
        .filter((match) => {
          const key = \`\${match.locator || ''}::\${match.chunkId || ''}\`;
          if (seen.has(key)) {
            return false;
          }
          seen.add(key);
          return true;
        })
        .filter((match) => {
          if (typeof match.score === 'number') {
            return match.score >= 0.05;
          }
          return true;
        });
    }

    function getProgressPercentText(progress, total, processed) {
      if (progress.status === 'complete') {
        return '100%';
      }

      if (!total) {
        return '';
      }

      const percent = Math.max(0, Math.min(100, Math.round((processed / total) * 100)));
      return percent + '%';
    }

    function getEtaText(progress, jobStartedAt, stageStartedAt, total, processed) {
      if (!stageStartedAt || !total || processed <= 0 || progress.status === 'complete' || progress.status === 'error') {
        return '';
      }

      const startedMs = Date.parse(stageStartedAt || jobStartedAt);
      if (!Number.isFinite(startedMs)) {
        return '';
      }

      const elapsedMs = Date.now() - startedMs;
      if (elapsedMs <= 0) {
        return '';
      }

      const remaining = Math.max(0, total - processed);
      if (!remaining) {
        return '0s';
      }

      const estimatedTotalMs = (elapsedMs / processed) * total;
      const etaMs = estimatedTotalMs - elapsedMs;
      if (!Number.isFinite(etaMs) || etaMs <= 0) {
        return '0s';
      }

      return formatDuration(etaMs);
    }

    function getEtaProcessed(progress) {
      const completed = progress.processed ?? 0;
      const fileProcessed = progress.fileProcessed ?? 0;
      const fileTotal = progress.fileTotal ?? 0;
      if (progress.stage !== 'processing' || !fileTotal || fileProcessed <= 0) {
        return completed;
      }

      const fractional = Math.max(0, Math.min(1, fileProcessed / fileTotal));
      return completed + fractional;
    }

    function getChunksPerSecondText(progress) {
      const rate = progress.chunksPerSecond;
      if (typeof rate !== 'number' || !Number.isFinite(rate) || rate <= 0) {
        return '';
      }

      return rate.toFixed(rate >= 10 ? 0 : rate >= 1 ? 1 : 2);
    }

    function formatDuration(ms) {
      const totalSeconds = Math.max(0, Math.ceil(ms / 1000));
      const hours = Math.floor(totalSeconds / 3600);
      const minutes = Math.floor((totalSeconds % 3600) / 60);
      const seconds = totalSeconds % 60;
      if (hours > 0) {
        return hours + 'h ' + String(minutes).padStart(2, '0') + 'm';
      }
      if (minutes > 0) {
        return minutes + 'm ' + String(seconds).padStart(2, '0') + 's';
      }
      return seconds + 's';
    }

    async function collectDroppedFiles(dataTransfer) {
      const files = [];
      const items = Array.from(dataTransfer?.items || []);

      for (const item of items) {
        const entry = item.webkitGetAsEntry?.();
        if (entry) {
          await collectEntryFiles(entry, files);
          continue;
        }

        const file = item.getAsFile?.();
        if (file) {
          files.push(file);
        }
      }

      if (!files.length) {
        files.push(...Array.from(dataTransfer?.files || []));
      }

      return files;
    }

    async function collectEntryFiles(entry, files, prefix = '') {
      if (entry.isFile) {
        const file = await new Promise((resolve, reject) => {
          entry.file(resolve, reject);
        });
        if (prefix && !file.webkitRelativePath) {
          Object.defineProperty(file, 'webkitRelativePath', {
            configurable: true,
            value: \`\${prefix}\${file.name}\`
          });
        }
        files.push(file);
        return;
      }

      if (!entry.isDirectory) {
        return;
      }

      const reader = entry.createReader();
      const entries = await readAllDirectoryEntries(reader);
      const nextPrefix = \`\${prefix}\${entry.name}/\`;
      for (const child of entries) {
        await collectEntryFiles(child, files, nextPrefix);
      }
    }

    async function readAllDirectoryEntries(reader) {
      const entries = [];
      while (true) {
        const batch = await new Promise((resolve, reject) => {
          reader.readEntries(resolve, reject);
        });
        if (!batch.length) {
          break;
        }
        entries.push(...batch);
      }
      return entries;
    }

    function inferKindFromName(name) {
      const lower = name.toLowerCase();
      if (/\.(c|cc|cjs|cpp|cs|css|go|h|hpp|html|java|js|jsx|kt|m|md|mdx|php|py|rb|rs|scss|sh|ts|tsx|vue|xml|yml|yaml|json|toml|ini|cfg)$/.test(lower)) {
        return 'code';
      }
      return 'text';
    }
  </script>
</body>
</html>`;
}

function sendHtml(res: ServerResponse, html: string): void {
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
  res.end(html);
}

function sendJson(res: ServerResponse, payload: unknown, status = 200): void {
  res.writeHead(status, { 'content-type': 'application/json; charset=utf-8' });
  res.end(JSON.stringify(payload));
}

async function readJsonBody(req: IncomingMessage): Promise<unknown> {
  const chunks = [];
  for await (const chunk of req) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }

  if (!chunks.length) {
    return {};
  }

  const text = Buffer.concat(chunks).toString('utf8');
  return text ? JSON.parse(text) : {};
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

function parseOptionalInt(value: unknown): number | undefined {
  if (value === null || value === undefined || value === '') {
    return undefined;
  }
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.trunc(parsed) : undefined;
}

function normalizePathInput(value: string): string {
  const trimmed = value.trim();
  if ((trimmed.startsWith('"') && trimmed.endsWith('"')) || (trimmed.startsWith("'") && trimmed.endsWith("'"))) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.trim().length > 0;
}

async function loadGuiSettings(): Promise<GuiSettings> {
  try {
    const raw = await readFile(SETTINGS_PATH, 'utf8');
    const parsed = JSON.parse(raw) as Partial<GuiSettings>;
    const workerCount = parseOptionalInt(parsed.workerCount);
    return {
      workerCount: workerCount && workerCount > 0 ? workerCount : undefined
    };
  } catch (error) {
    if (!isMissingFileError(error)) {
      throw error;
    }
    return {};
  }
}

async function saveGuiSettings(settings: GuiSettings): Promise<GuiSettings> {
  const normalized: GuiSettings = {
    workerCount: typeof settings.workerCount === 'number' && settings.workerCount > 0
      ? Math.trunc(settings.workerCount)
      : undefined
  };

  const payload = JSON.stringify(normalized, null, 2);
  const tempPath = `${SETTINGS_PATH}.tmp`;
  await mkdir(path.dirname(SETTINGS_PATH), { recursive: true });
  await writeFile(tempPath, payload, 'utf8');
  await rm(SETTINGS_PATH, { force: true });
  await rename(tempPath, SETTINGS_PATH);
  return normalized;
}

function isMissingFileError(error: unknown): boolean {
  return Boolean(error && typeof error === 'object' && 'code' in error && (error as { code?: string }).code === 'ENOENT');
}
