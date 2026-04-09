import { createHash } from 'node:crypto';
import { watch } from 'node:fs';
import { mkdir, opendir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

import { createEmbeddingProvider, type EmbeddingProvider } from './embeddings.js';

export type SourceKind = string;

export interface RagSourceInput {
  kind: SourceKind;
  locator: string;
  label?: string;
  path?: string;
  url?: string;
  content?: string;
  mimeType?: string;
  metadata?: Record<string, unknown>;
}

export interface RagQueryOptions {
  query: string;
  topK?: number;
  sourceIds?: string[];
  sourceKinds?: SourceKind[];
}

export interface RagChunkRecord {
  chunkId: string;
  sourceId: string;
  sourceKind: SourceKind;
  locator: string;
  label: string;
  title: string;
  text: string;
  startLine: number;
  endLine: number;
  tokenCount: number;
  heading?: string;
  embedding: number[];
}

export interface RagSourceRecord {
  sourceId: string;
  kind: SourceKind;
  locator: string;
  label: string;
  title: string;
  contentHash: string;
  mimeType?: string;
  metadata?: Record<string, unknown>;
  chunkIds: string[];
  chunkCount: number;
  updatedAt: string;
  createdAt: string;
}

export interface RagIndexSnapshot {
  version: number;
  embeddingProvider: string;
  updatedAt: string;
  sources: Record<string, RagSourceRecord>;
  chunks: Record<string, RagChunkRecord>;
}

export interface RagSearchResult {
  score: number;
  snippet: string;
  sourceId: string;
  sourceKind: SourceKind;
  locator: string;
  label: string;
  title: string;
  chunkId: string;
  startLine: number;
  endLine: number;
  heading?: string;
}

export interface RagIngestResult {
  sourceId: string;
  kind: SourceKind;
  locator: string;
  label: string;
  chunkCount: number;
  updatedAt: string;
  metrics?: RagIngestMetrics;
}

export interface RagIngestMetrics {
  documentCount: number;
  chunkCount: number;
  avgChunkChars: number;
  avgChunkTokens: number;
  loadMs: number;
  chunkMs: number;
  embedMs: number;
  commitMs: number;
  persistMs: number;
  totalMs: number;
  batchSize: number;
  batchCount: number;
  cacheHits: number;
  cacheMisses: number;
  reusedEmbeddings: number;
}

interface RagPreparedIngest {
  document: {
    kind: SourceKind;
    locator: string;
    label: string;
    title: string;
    content: string;
    contentHash: string;
    mimeType?: string;
    metadata?: Record<string, unknown>;
  };
  chunks: Array<{
    text: string;
    startLine: number;
    endLine: number;
    tokenCount: number;
    heading?: string;
  }>;
  embeddings: number[][];
  metrics: Omit<RagIngestMetrics, 'commitMs' | 'totalMs'> & { commitMs?: number; totalMs?: number };
}

export interface RagIndexOptions {
  dataDir: string;
  embeddingProvider?: EmbeddingProvider;
  embeddingProviderOptions?: Parameters<typeof createEmbeddingProvider>[0];
  maxChunkChars?: number;
  maxChunkLines?: number;
  overlapLines?: number;
  embeddingBatchSize?: number;
}

export interface RagDirectoryIngestOptions {
  recursive?: boolean;
  includeHidden?: boolean;
  concurrency?: number;
  onProgress?: (progress: {
    stage: 'scanning' | 'processing' | 'complete';
    processed: number;
    total: number;
    currentPath?: string;
    detail?: string;
    fileProcessed?: number;
    fileTotal?: number;
    chunksPerSecond?: number;
    visitedDirectories?: number;
    visitedEntries?: number;
    discoveredFiles?: number;
  }) => void;
}

export interface RagDirectoryDiscoveryOptions {
  recursive?: boolean;
  includeHidden?: boolean;
  onProgress?: RagDirectoryIngestOptions['onProgress'];
  shouldStop?: () => boolean;
}

const CODE_EXTENSIONS = new Set([
  '.c', '.cc', '.cjs', '.cpp', '.cs', '.css', '.go', '.h', '.hpp', '.html', '.java', '.js',
  '.jsx', '.kt', '.m', '.md', '.mdx', '.php', '.py', '.rb', '.rs', '.scss', '.sh', '.ts',
  '.tsx', '.vue', '.xml', '.yml', '.yaml', '.json', '.toml', '.ini', '.cfg'
]);

const HTML_ENTITY_MAP: Record<string, string> = {
  amp: '&',
  lt: '<',
  gt: '>',
  quot: '"',
  apos: "'",
  nbsp: ' '
};

const DEFAULT_DIRECTORY_IGNORE = new Set([
  '.git',
  '.hg',
  '.svn',
  'node_modules',
  'dist',
  'build',
  'out',
  'coverage',
  '.cache',
  'data'
]);

const BINARY_EXTENSIONS = new Set([
  '.7z', '.a', '.apk', '.assetbundle', '.bin', '.bmp', '.bundle', '.class', '.dds', '.dll', '.dylib',
  '.exe', '.gif', '.ico', '.jpg', '.jpeg', '.mov', '.mp3', '.mp4', '.o', '.obj', '.pdf', '.png',
  '.psd', '.pyc', '.ress', '.so', '.tar', '.tif', '.tiff', '.unity3d', '.wav', '.webm', '.webp',
  '.woff', '.woff2', '.zip', '.gz', '.xz', '.rar',
  '.7zip'
]);

const MAX_UNKNOWN_TEXT_FILE_SIZE = 1024 * 1024;

const TEXT_MIME_PREFIXES = ['text/'];
const TEXT_MIME_TYPES = new Set([
  'application/json',
  'application/ld+json',
  'application/manifest+json',
  'application/xml',
  'application/xhtml+xml',
  'application/x-httpd-php',
  'application/javascript',
  'application/x-javascript',
  'application/typescript',
  'application/x-typescript'
]);

export class RagIndex {
  private readonly dataDir: string;
  private readonly indexPath: string;
  private embeddingProvider: EmbeddingProvider;
  private readonly maxChunkChars: number;
  private readonly maxChunkLines: number;
  private readonly overlapLines: number;
  private readonly embeddingBatchSize: number;
  private snapshot: RagIndexSnapshot;
  private readonly embeddingCache = new Map<string, number[]>();
  private ready: Promise<void>;
  private mutationChain: Promise<unknown> = Promise.resolve();
  private readonly watchers = new Map<string, import('node:fs').FSWatcher>();
  private readonly watchedDirectories = new Set<string>();
  private readonly watchedFiles = new Set<string>();
  private refreshTimer: NodeJS.Timeout | null = null;
  private pendingRefreshPaths = new Set<string>();

  constructor(options: RagIndexOptions) {
    this.dataDir = options.dataDir;
    this.indexPath = path.join(this.dataDir, 'index.json');
    this.embeddingProvider = options.embeddingProvider ?? createEmbeddingProvider(options.embeddingProviderOptions);
    this.maxChunkChars = options.maxChunkChars ?? 4800;
    this.maxChunkLines = options.maxChunkLines ?? 140;
    this.overlapLines = options.overlapLines ?? 8;
    this.embeddingBatchSize = options.embeddingBatchSize ?? 32;
    this.snapshot = {
      version: 1,
      embeddingProvider: this.embeddingProvider.name,
      updatedAt: new Date().toISOString(),
      sources: {},
      chunks: {}
    };
    this.ready = this.initialize();
  }

  async ingestSources(
    inputs: RagSourceInput[],
    options: { onProgress?: RagDirectoryIngestOptions['onProgress']; onSkip?: (input: RagSourceInput, error: Error) => void } = {}
  ): Promise<RagIngestResult[]> {
    await this.ensureReady();
    if (inputs.length <= 1) {
      const results: RagIngestResult[] = [];
      for (const input of inputs) {
        try {
          results.push(await this.ingestSource(input, { onProgress: options.onProgress }));
        } catch (error) {
          if (isSkipFileError(error)) {
            options.onSkip?.(input, error instanceof Error ? error : new Error(String(error)));
            continue;
          }
          throw error;
        }
      }
      return results;
    }

    const results: Array<RagIngestResult | undefined> = new Array(inputs.length);
    const pendingPrepared: RagPreparedIngest[] = [];
    const pendingIndexes: number[] = [];

    for (let index = 0; index < inputs.length; index += 1) {
      const input = inputs[index];
      const ingestStartedAt = Date.now();
      const loadStartedAt = Date.now();
      try {
        const document = await this.loadDocument(input);
        const loadMs = Date.now() - loadStartedAt;
        const sourceId = stableSourceId(document.kind, document.locator, document.contentHash);
        const existingSource = this.snapshot.sources[sourceId];
        if (existingSource) {
          results[index] = {
            sourceId,
            kind: existingSource.kind,
            locator: existingSource.locator,
            label: existingSource.label,
            chunkCount: existingSource.chunkCount,
            updatedAt: existingSource.updatedAt,
            metrics: {
              documentCount: 1,
              chunkCount: existingSource.chunkCount,
              avgChunkChars: existingSource.chunkCount ? Math.round(existingSource.chunkIds.reduce((sum, chunkId) => sum + (this.snapshot.chunks[chunkId]?.text.length ?? 0), 0) / existingSource.chunkCount) : 0,
              avgChunkTokens: existingSource.chunkCount ? Math.round(existingSource.chunkIds.reduce((sum, chunkId) => sum + (this.snapshot.chunks[chunkId]?.tokenCount ?? 0), 0) / existingSource.chunkCount) : 0,
              loadMs,
              chunkMs: 0,
              embedMs: 0,
              commitMs: 0,
              persistMs: 0,
              totalMs: Date.now() - ingestStartedAt,
              batchSize: 0,
              batchCount: 0,
              cacheHits: existingSource.chunkCount,
              cacheMisses: 0,
              reusedEmbeddings: existingSource.chunkCount
            }
          };
          continue;
        }

        const prepared = await this.prepareIngestSource(input, { onProgress: options.onProgress }, document, loadMs);
        pendingPrepared.push(prepared);
        pendingIndexes.push(index);
      } catch (error) {
        if (isSkipFileError(error)) {
          options.onSkip?.(input, error instanceof Error ? error : new Error(String(error)));
          continue;
        }

        throw error;
      }
    }

    if (!pendingPrepared.length) {
      return results.filter((result): result is RagIngestResult => Boolean(result));
    }

    const committed = await this.commitPreparedIngestSources(pendingPrepared);
    for (let index = 0; index < committed.length; index += 1) {
      results[pendingIndexes[index]] = committed[index];
    }

    return results.filter((result): result is RagIngestResult => Boolean(result));
  }

  async cleanupBinarySources(): Promise<number> {
    await this.ensureReady();
    return this.runMutation(async () => this.cleanupBinarySourcesInternal());
  }

  async updateEmbeddingWorkerCount(workerCount: number): Promise<void> {
    await this.ensureReady();
    const provider = this.embeddingProvider as EmbeddingProvider & { setWorkerCount?: (count: number) => Promise<void> | void };
    if (typeof provider.setWorkerCount !== 'function') {
      return;
    }

    await provider.setWorkerCount(workerCount);
  }

  async setEmbeddingProvider(provider: EmbeddingProvider): Promise<void> {
    await this.ensureReady();
    if (provider.name === this.embeddingProvider.name) {
      return;
    }

    const previous = this.embeddingProvider as EmbeddingProvider & { dispose?: () => Promise<void> | void };
    const previousSnapshotProvider = this.snapshot.embeddingProvider;
    this.embeddingProvider = provider;
    this.snapshot.embeddingProvider = provider.name;
    this.snapshot.updatedAt = new Date().toISOString();
    this.embeddingCache.clear();
    if (previousSnapshotProvider === provider.name) {
      this.rebuildEmbeddingCache();
    }
    await this.persist();

    if (typeof previous.dispose === 'function') {
      await previous.dispose();
    }
  }

  private rebuildEmbeddingCache(): void {
    if (this.snapshot.embeddingProvider !== this.embeddingProvider.name) {
      return;
    }

    this.embeddingCache.clear();
    for (const chunk of Object.values(this.snapshot.chunks)) {
      if (!chunk || !Array.isArray(chunk.embedding) || !chunk.embedding.length) {
        continue;
      }

      this.embeddingCache.set(this.embeddingCacheKey(chunk.text), normalizeVector(chunk.embedding));
    }
  }

  private embeddingCacheKey(text: string): string {
    return `${this.embeddingProvider.name}::${hashText(text)}`;
  }

  async ingestSource(
    input: RagSourceInput,
    options: { onProgress?: RagDirectoryIngestOptions['onProgress'] } = {}
  ): Promise<RagIngestResult> {
    await this.ensureReady();
    const ingestStartedAt = Date.now();
    const loadStartedAt = Date.now();
    const document = await this.loadDocument(input);
    const loadMs = Date.now() - loadStartedAt;
    const sourceId = stableSourceId(document.kind, document.locator, document.contentHash);
    const existingSource = this.snapshot.sources[sourceId];
    if (existingSource) {
      return {
        sourceId,
        kind: existingSource.kind,
        locator: existingSource.locator,
        label: existingSource.label,
        chunkCount: existingSource.chunkCount,
        updatedAt: existingSource.updatedAt,
        metrics: {
          documentCount: 1,
          chunkCount: existingSource.chunkCount,
          avgChunkChars: existingSource.chunkCount ? Math.round(existingSource.chunkIds.reduce((sum, chunkId) => sum + (this.snapshot.chunks[chunkId]?.text.length ?? 0), 0) / existingSource.chunkCount) : 0,
          avgChunkTokens: existingSource.chunkCount ? Math.round(existingSource.chunkIds.reduce((sum, chunkId) => sum + (this.snapshot.chunks[chunkId]?.tokenCount ?? 0), 0) / existingSource.chunkCount) : 0,
          loadMs,
          chunkMs: 0,
          embedMs: 0,
          commitMs: 0,
          persistMs: 0,
          totalMs: Date.now() - ingestStartedAt,
          batchSize: 0,
          batchCount: 0,
          cacheHits: existingSource.chunkCount,
          cacheMisses: 0,
          reusedEmbeddings: existingSource.chunkCount
        }
      };
    }

    const prepared = await this.prepareIngestSource(input, options, document, loadMs);
    const committed = await this.runMutation(async () => this.commitPreparedIngestSource(prepared));
    if (committed.kind === 'file' || committed.kind === 'code') {
      this.watchPath(committed.locator);
    }
    return committed;
  }

  async ingestDirectory(directoryPath: string, options: RagDirectoryIngestOptions = {}): Promise<RagIngestResult[]> {
    await this.ensureReady();
    const normalizedDirectoryPath = normalizePathInput(directoryPath);
    const absolutePath = path.isAbsolute(normalizedDirectoryPath)
      ? normalizedDirectoryPath
      : path.resolve(normalizedDirectoryPath);
    const discovered = await this.discoverDirectoryFiles(absolutePath, {
      recursive: options.recursive ?? true,
      includeHidden: options.includeHidden ?? false,
      onProgress: options.onProgress
    });

    const results: RagIngestResult[] = new Array(discovered.length);
    const concurrency = Math.max(1, Math.min(discovered.length, options.concurrency ?? 4));
    let nextIndex = 0;
    let completedCount = 0;
    const workers = Array.from({ length: concurrency }, async () => {
      while (true) {
        const index = nextIndex;
        nextIndex += 1;
        if (index >= discovered.length) {
          return;
        }

        const filePath = discovered[index];
        const fileInput: RagSourceInput = {
          kind: inferKindFromLocator(filePath, undefined, 'file'),
          locator: filePath,
          path: filePath,
          label: path.relative(absolutePath, filePath) || path.basename(filePath)
        };

        try {
          const prepared = await this.prepareIngestSource(fileInput, {
            onProgress: (progress) => {
              options.onProgress?.({
                stage: progress.stage,
                processed: completedCount,
                total: discovered.length,
                currentPath: filePath,
                detail: progress.detail,
                fileProcessed: progress.fileProcessed ?? progress.processed,
                fileTotal: progress.fileTotal ?? progress.total,
                visitedDirectories: progress.visitedDirectories,
                visitedEntries: progress.visitedEntries,
                discoveredFiles: progress.discoveredFiles
              });
            }
          });
          const result = await this.runMutation(async () => {
            const committed = await this.commitPreparedIngestSource(prepared);
            if (committed.kind === 'file' || committed.kind === 'code') {
              this.watchPath(committed.locator);
            }
            return committed;
          });
          results[index] = result;
          completedCount += 1;
          options.onProgress?.({
            stage: 'processing',
            processed: completedCount,
            total: discovered.length,
            currentPath: filePath,
            detail: 'Indexed'
          });
        } catch (error) {
          if (!isSkipFileError(error)) {
            throw error;
          }

          completedCount += 1;
          options.onProgress?.({
            stage: 'processing',
            processed: completedCount,
            total: discovered.length,
            currentPath: filePath,
            detail: error instanceof Error ? error.message : String(error)
          });
        }

        if (completedCount % 25 === 0) {
          await yieldToEventLoop();
        }
      }
    });

    await Promise.all(workers);

    this.watchDirectory(absolutePath);
    options.onProgress?.({
      stage: 'complete',
      processed: discovered.length,
      total: discovered.length,
      currentPath: absolutePath,
      detail: 'Folder ingest complete',
      discoveredFiles: discovered.length
    });

    return results.filter((result): result is RagIngestResult => Boolean(result));
  }

  async discoverDirectoryFiles(directoryPath: string, options: RagDirectoryDiscoveryOptions = {}): Promise<string[]> {
    await this.ensureReady();
    const normalizedDirectoryPath = normalizePathInput(directoryPath);
    const absolutePath = path.isAbsolute(normalizedDirectoryPath)
      ? normalizedDirectoryPath
      : path.resolve(normalizedDirectoryPath);
    return collectFiles(absolutePath, {
      recursive: options.recursive ?? true,
      includeHidden: options.includeHidden ?? false,
      onProgress: options.onProgress,
      shouldStop: options.shouldStop
    });
  }

  async listSources(filter?: { kinds?: SourceKind[] }): Promise<RagSourceRecord[]> {
    await this.ensureReady();
    const kinds = filter?.kinds?.length ? new Set(filter.kinds) : null;
    return Object.values(this.snapshot.sources)
      .filter((source) => !kinds || kinds.has(source.kind))
      .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }

  async listChunks(sourceId: string, options: { offset?: number; limit?: number } = {}): Promise<RagChunkRecord[]> {
    await this.ensureReady();
    const source = this.snapshot.sources[sourceId];
    if (!source) {
      return [];
    }

    const offset = Math.max(0, options.offset ?? 0);
    const limit = Math.max(1, options.limit ?? source.chunkIds.length);
    return source.chunkIds
      .slice(offset, offset + limit)
      .map((chunkId) => this.snapshot.chunks[chunkId])
      .filter((chunk): chunk is RagChunkRecord => Boolean(chunk));
  }

  async search(options: RagQueryOptions): Promise<RagSearchResult[]> {
    await this.ensureReady();

    const query = options.query.trim();
    if (!query) {
      return [];
    }

    const queryEmbedding = await this.embeddingProvider.embed([query]);
    const queryVector = normalizeVector(queryEmbedding[0] ?? []);
    const queryTokens = tokenize(query);
    const queryTerms = new Set(queryTokens);
    const queryLower = query.toLowerCase();
    const candidates = Object.values(this.snapshot.chunks).filter((chunk) => {
      if (options.sourceIds?.length && !options.sourceIds.includes(chunk.sourceId)) {
        return false;
      }
      if (options.sourceKinds?.length && !options.sourceKinds.includes(chunk.sourceKind)) {
        return false;
      }
      return true;
    });

    const ranked = candidates.map((chunk) => {
      const semantic = cosineSimilarity(queryVector, chunk.embedding);
      const lexical = lexicalOverlapScore(queryTerms, tokenize(chunk.text));
      const phraseBoost = chunk.text.toLowerCase().includes(queryLower) ? 0.15 : 0;
      const headingBoost = chunk.heading && queryLower.includes(chunk.heading.toLowerCase()) ? 0.08 : 0;
      const score = semantic * 0.72 + lexical * 0.2 + phraseBoost + headingBoost;

      return {
        score,
        snippet: buildSnippet(chunk.text, query),
        sourceId: chunk.sourceId,
        sourceKind: chunk.sourceKind,
        locator: chunk.locator,
        label: chunk.label,
        title: chunk.title,
        chunkId: chunk.chunkId,
        startLine: chunk.startLine,
        endLine: chunk.endLine,
        heading: chunk.heading
      };
    });

    return ranked
      .sort((a, b) => b.score - a.score)
      .slice(0, Math.max(1, options.topK ?? 5));
  }

  async getStats(): Promise<{
    sourceCount: number;
    chunkCount: number;
    embeddingProvider: string;
    updatedAt: string;
  }> {
    await this.ensureReady();
    return {
      sourceCount: Object.keys(this.snapshot.sources).length,
      chunkCount: Object.keys(this.snapshot.chunks).length,
      embeddingProvider: this.snapshot.embeddingProvider,
      updatedAt: this.snapshot.updatedAt
    };
  }

  async ensureReady(): Promise<void> {
    await this.ready;
  }

  closeWatchers(): void {
    for (const watcher of this.watchers.values()) {
      watcher.close();
    }
    this.watchers.clear();
    this.watchedDirectories.clear();
    this.watchedFiles.clear();
    this.pendingRefreshPaths.clear();
    if (this.refreshTimer) {
      clearTimeout(this.refreshTimer);
      this.refreshTimer = null;
    }
  }

  private async initialize(): Promise<void> {
    await mkdir(this.dataDir, { recursive: true });
    try {
      const raw = await readFile(this.indexPath, 'utf8');
      const parsed = JSON.parse(raw) as Partial<RagIndexSnapshot>;
      if (parsed && typeof parsed === 'object') {
        this.snapshot = {
          version: 1,
          embeddingProvider: typeof parsed.embeddingProvider === 'string'
            ? parsed.embeddingProvider
            : this.embeddingProvider.name,
          updatedAt: typeof parsed.updatedAt === 'string' ? parsed.updatedAt : new Date().toISOString(),
          sources: parsed.sources && typeof parsed.sources === 'object' ? parsed.sources as Record<string, RagSourceRecord> : {},
          chunks: parsed.chunks && typeof parsed.chunks === 'object' ? parsed.chunks as Record<string, RagChunkRecord> : {}
        };
        this.rebuildEmbeddingCache();
        await this.cleanupBinarySourcesInternal();
      }
    } catch (error) {
      if (!isMissingFileError(error)) {
        throw error;
      }

      const tempPath = `${this.indexPath}.tmp`;
      try {
        const raw = await readFile(tempPath, 'utf8');
        const parsed = JSON.parse(raw) as Partial<RagIndexSnapshot>;
        if (parsed && typeof parsed === 'object') {
          this.snapshot = {
            version: 1,
            embeddingProvider: typeof parsed.embeddingProvider === 'string'
              ? parsed.embeddingProvider
              : this.embeddingProvider.name,
            updatedAt: typeof parsed.updatedAt === 'string' ? parsed.updatedAt : new Date().toISOString(),
            sources: parsed.sources && typeof parsed.sources === 'object' ? parsed.sources as Record<string, RagSourceRecord> : {},
            chunks: parsed.chunks && typeof parsed.chunks === 'object' ? parsed.chunks as Record<string, RagChunkRecord> : {}
          };
          this.rebuildEmbeddingCache();
          await rename(tempPath, this.indexPath);
          await this.cleanupBinarySourcesInternal();
        }
      } catch (tempError) {
        if (!isMissingFileError(tempError)) {
          throw tempError;
        }
      }
    }
  }

  private async loadDocument(input: RagSourceInput): Promise<{
    kind: SourceKind;
    locator: string;
    label: string;
    title: string;
    content: string;
    contentHash: string;
    mimeType?: string;
    metadata?: Record<string, unknown>;
  }> {
    if (input.kind === 'file' || input.path) {
      const filePathInput = normalizePathInput(input.path ?? input.locator);
      const filePath = path.isAbsolute(filePathInput)
        ? filePathInput
        : path.resolve(filePathInput);
      const ext = path.extname(filePath).toLowerCase();
      const stats = await stat(filePath);
      const rawBuffer = await readFile(filePath);
      const declaredMimeType = input.mimeType ?? (typeof input.metadata?.mimeType === 'string' ? input.metadata.mimeType : undefined);
      const mimeType = declaredMimeType ?? (ext ? inferMimeTypeFromPath(filePath) : inferMimeTypeFromBuffer(rawBuffer, filePath));
      if (!isTextBasedMimeType(mimeType) && !CODE_EXTENSIONS.has(ext) && stats.size > MAX_UNKNOWN_TEXT_FILE_SIZE) {
        throw createSkipFileError(`Skipping large unknown file ${filePath} (${formatBytes(stats.size)}).`);
      }
      if (ext.length === 0 && !isTextBasedMimeType(mimeType)) {
        throw createSkipFileError(`Skipping extensionless non-text file ${filePath} (${mimeType ?? 'unknown MIME'}).`);
      }
      if (mimeType && !isTextBasedMimeType(mimeType) && !CODE_EXTENSIONS.has(ext)) {
        throw createSkipFileError(`Skipping non-text file ${filePath} (${mimeType}).`);
      }

      const content = new TextDecoder('utf-8', { fatal: false }).decode(rawBuffer);
      const kind = inferKindFromLocator(filePath, undefined, input.kind);
      const label = input.label ?? path.basename(filePath);
      return {
        kind,
        locator: filePath,
        label,
        title: deriveTitle(label, content),
        content,
        contentHash: stableContentHash(content),
        mimeType,
        metadata: input.metadata
      };
    }

    if (input.kind === 'url' || input.url || isHttpUrl(input.locator)) {
      const url = input.url ?? input.locator;
      const response = await fetch(url);
      const mimeType = response.headers.get('content-type') ?? undefined;
      if (mimeType && !isTextBasedMimeType(mimeType)) {
        throw createSkipFileError(`Skipping non-text URL ${url} (${mimeType}).`);
      }
      const rawText = await response.text();
      const content = normalizeRemoteContent(rawText, mimeType, url);
      const kind = inferKindFromLocator(url, mimeType, input.kind);
      const label = input.label ?? deriveLabelFromUrl(url);
      return {
        kind,
        locator: url,
        label,
        title: deriveTitle(label, content),
        content,
        contentHash: stableContentHash(content),
        mimeType,
        metadata: input.metadata
      };
    }

    const content = input.content ?? '';
    const mimeType = input.mimeType ?? (typeof input.metadata?.mimeType === 'string' ? input.metadata.mimeType : undefined);
    if (mimeType && !isTextBasedMimeType(mimeType)) {
      throw createSkipFileError(`Skipping non-text source ${input.locator} (${mimeType}).`);
    }
    const label = input.label ?? input.locator ?? input.kind;
    const kind = inferKindFromLocator(input.locator, undefined, input.kind);
    return {
      kind,
      locator: input.locator || label,
      label,
      title: deriveTitle(label, content),
      content,
      contentHash: stableContentHash(content),
      mimeType,
      metadata: input.metadata
    };
  }

  private chunkDocument(document: {
    content: string;
    kind: SourceKind;
    locator: string;
    label: string;
    title: string;
  }): Array<{
    text: string;
    startLine: number;
    endLine: number;
    tokenCount: number;
    heading?: string;
  }> {
    const lines = normalizeLineEndings(document.content).split('\n');
    const mode = inferDocumentMode(document.kind, document.locator, document.content);
    const maxLines = mode === 'code' ? this.maxChunkLines : Math.max(40, Math.floor(this.maxChunkLines * 0.85));
    const overlap = mode === 'code' ? this.overlapLines : Math.max(6, Math.floor(this.overlapLines / 2));
    const chunks: Array<{
      text: string;
      startLine: number;
      endLine: number;
      tokenCount: number;
      heading?: string;
    }> = [];

    let buffer: string[] = [];
    let bufferStartLine = 1;
    let currentHeading: string | undefined;

    const flush = (nextStartLine: number, carryOverlap: boolean) => {
      const text = buffer.join('\n').trim();
      if (text) {
        chunks.push({
          text,
          startLine: bufferStartLine,
          endLine: bufferStartLine + buffer.length - 1,
          tokenCount: estimateTokenCount(text),
          heading: currentHeading
        });
      }

      if (carryOverlap && buffer.length > overlap) {
        const carry = buffer.slice(-overlap);
        buffer = carry;
        bufferStartLine = Math.max(nextStartLine - carry.length, 1);
      } else {
        buffer = [];
        bufferStartLine = nextStartLine;
      }
    };

    for (let index = 0; index < lines.length; index += 1) {
      const lineNo = index + 1;
      const line = lines[index];
      const trimmed = line.trim();
      const heading = extractHeading(trimmed);
      const codeBoundary = mode === 'code' && isCodeBoundary(trimmed);
      const sectionBoundary = mode !== 'code' && heading && buffer.length >= Math.max(12, Math.floor(maxLines / 2));

      if (codeBoundary && buffer.length >= 14) {
        flush(lineNo, true);
        currentHeading = heading;
      } else if (sectionBoundary) {
        flush(lineNo, true);
        currentHeading = heading;
      }

      if (!buffer.length) {
        bufferStartLine = lineNo;
      }

      if (heading) {
        currentHeading = heading;
      }

      buffer.push(line);

      const bufferText = buffer.join('\n');
      if (buffer.length >= maxLines || bufferText.length >= this.maxChunkChars) {
        flush(lineNo + 1, true);
      }
    }

    flush(lines.length + 1, false);

    return chunks.filter((chunk) => chunk.text.trim().length > 0);
  }

  private async runMutation<T>(operation: () => Promise<T>): Promise<T> {
    const next = this.mutationChain.then(operation, operation);
    this.mutationChain = next.then(
      () => undefined,
      () => undefined
    );
    return next;
  }

  private removeSourceInternal(sourceId: string): void {
    const existing = this.snapshot.sources[sourceId];
    if (!existing) {
      return;
    }

    for (const chunkId of existing.chunkIds) {
      delete this.snapshot.chunks[chunkId];
    }

    delete this.snapshot.sources[sourceId];
  }

  private async persist(): Promise<void> {
    const payload = JSON.stringify(this.snapshot, null, 2);
    const tempPath = `${this.indexPath}.tmp`;
    await mkdir(path.dirname(this.indexPath), { recursive: true });
    await writeFile(tempPath, payload, 'utf8');
    await rm(this.indexPath, { force: true });
    await rename(tempPath, this.indexPath);
  }

  private async cleanupBinarySourcesInternal(): Promise<number> {
    const staleSourceIds: string[] = [];
    for (const source of Object.values(this.snapshot.sources)) {
      if (this.shouldSkipStoredSource(source)) {
        staleSourceIds.push(source.sourceId);
      }
    }

    if (!staleSourceIds.length) {
      return 0;
    }

    for (const sourceId of staleSourceIds) {
      this.removeSourceInternal(sourceId);
    }

    this.snapshot.updatedAt = new Date().toISOString();
    await this.persist();
    return staleSourceIds.length;
  }

  private shouldSkipStoredSource(source: RagSourceRecord): boolean {
    const locator = source.locator.trim();
    const ext = path.extname(locator).toLowerCase();
    if (BINARY_EXTENSIONS.has(ext)) {
      return true;
    }

    if (source.kind === 'code' && ext && !CODE_EXTENSIONS.has(ext) && BINARY_EXTENSIONS.has(ext)) {
      return true;
    }

    const metadataSize = source.metadata && typeof source.metadata === 'object' && 'size' in source.metadata && typeof source.metadata.size === 'number'
      ? source.metadata.size
      : undefined;
    if (typeof metadataSize === 'number' && metadataSize > MAX_UNKNOWN_TEXT_FILE_SIZE && !CODE_EXTENSIONS.has(ext)) {
      return true;
    }

    return false;
  }

  private watchPath(filePath: string): void {
    const absolutePath = path.resolve(filePath);
    if (this.watchedFiles.has(absolutePath) || this.watchers.has(absolutePath)) {
      return;
    }

    try {
      const watcher = watch(absolutePath, () => {
        void this.queueRefresh(absolutePath);
      });
      this.watchers.set(absolutePath, watcher);
      this.watchedFiles.add(absolutePath);
    } catch {
      // Best-effort watch. If a file cannot be watched we still support manual re-ingest.
    }
  }

  private watchDirectory(directoryPath: string): void {
    const absolutePath = path.resolve(directoryPath);
    if (this.watchedDirectories.has(absolutePath) || this.watchers.has(absolutePath)) {
      return;
    }

    try {
      const watcher = watch(absolutePath, { recursive: true }, (_eventType, filename) => {
        if (typeof filename === 'string' && filename.trim()) {
          void this.queueRefresh(path.join(absolutePath, filename));
          return;
        }

        void this.queueRefresh(absolutePath);
      });
      this.watchers.set(absolutePath, watcher);
      this.watchedDirectories.add(absolutePath);
    } catch {
      // Best-effort watch. If recursive watching is unavailable, manual refresh still works.
    }
  }

  private async queueRefresh(targetPath: string): Promise<void> {
    const normalizedTarget = path.resolve(targetPath);
    this.pendingRefreshPaths.add(normalizedTarget);

    if (this.refreshTimer) {
      return;
    }

    this.refreshTimer = setTimeout(() => {
      this.refreshTimer = null;
      const paths = Array.from(this.pendingRefreshPaths);
      this.pendingRefreshPaths.clear();
      this.mutationChain = this.mutationChain.then(async () => {
        for (const changedPath of paths) {
          await this.refreshPath(changedPath);
        }
      }, async () => {
        for (const changedPath of paths) {
          await this.refreshPath(changedPath);
        }
      });
    }, 250);
  }

  private async refreshPath(targetPath: string): Promise<void> {
    const source = this.findSourceByLocator(targetPath);
    if (!source) {
      return;
    }

    try {
      const stats = await stat(targetPath);
      if (!stats.isFile()) {
        return;
      }
      const prepared = await this.prepareIngestSource({
        kind: source.kind,
        locator: source.locator,
        path: source.locator,
        label: source.label,
        metadata: source.metadata
      });
      await this.commitPreparedIngestSource(prepared);
    } catch {
      this.removeSourceInternal(source.sourceId);
      this.snapshot.updatedAt = new Date().toISOString();
      await this.persist();
    }
  }

  private findSourceByLocator(locator: string): RagSourceRecord | undefined {
    const normalized = normalizeLocator(locator);
    return Object.values(this.snapshot.sources).find((source) => normalizeLocator(source.locator) === normalized);
  }

  private async prepareIngestSource(
    input: RagSourceInput,
    options: { onProgress?: RagDirectoryIngestOptions['onProgress'] } = {},
    loadedDocument?: RagPreparedIngest['document'],
    loadMsOverride?: number
  ): Promise<RagPreparedIngest> {
    const loadStartedAt = Date.now();
    const document = loadedDocument ?? await this.loadDocument(input);
    const loadMs = loadMsOverride ?? (loadedDocument ? 0 : Date.now() - loadStartedAt);

    const chunkStartedAt = Date.now();
    const chunks = this.chunkDocument({
      content: document.content,
      kind: document.kind,
      locator: document.locator,
      label: document.label,
      title: document.title || document.label
    });
    const chunkMs = Date.now() - chunkStartedAt;

    const avgChunkChars = chunks.length
      ? Math.round(chunks.reduce((sum, chunk) => sum + chunk.text.length, 0) / chunks.length)
      : 0;
    const avgChunkTokens = chunks.length
      ? Math.round(chunks.reduce((sum, chunk) => sum + chunk.tokenCount, 0) / chunks.length)
      : 0;

    const embedStartedAt = Date.now();
    const { embeddings, stats } = await this.embedChunks(chunks, (processed, total, detail, chunksPerSecond) => {
      options.onProgress?.({
        stage: 'processing',
        processed,
        total,
        currentPath: document.locator,
        detail,
        fileProcessed: processed,
        fileTotal: total,
        chunksPerSecond
      });
    });
    const embedMs = Date.now() - embedStartedAt;

    return {
      document,
      chunks,
      embeddings,
      metrics: {
        documentCount: 1,
        chunkCount: chunks.length,
        avgChunkChars,
        avgChunkTokens,
        loadMs,
        chunkMs,
        embedMs,
        commitMs: 0,
        persistMs: 0,
        totalMs: loadMs + chunkMs + embedMs,
        batchSize: stats.batchSize,
        batchCount: stats.batchCount,
        cacheHits: stats.cacheHits,
        cacheMisses: stats.cacheMisses,
        reusedEmbeddings: stats.reusedEmbeddings
      }
    };
  }

  private async commitPreparedIngestSource(prepared: RagPreparedIngest, options: { persist?: boolean } = {}): Promise<RagIngestResult> {
    const commitStartedAt = Date.now();
    const sourceId = stableSourceId(prepared.document.kind, prepared.document.locator, prepared.document.contentHash);
    const now = new Date().toISOString();
    const existingSource = this.snapshot.sources[sourceId];
    const title = prepared.document.title || prepared.document.label;
    const sourceRecord: RagSourceRecord = {
      sourceId,
      kind: prepared.document.kind,
      locator: prepared.document.locator,
      label: prepared.document.label,
      title,
      contentHash: prepared.document.contentHash,
      mimeType: prepared.document.mimeType,
      metadata: prepared.document.metadata,
      chunkIds: [],
      chunkCount: 0,
      createdAt: existingSource?.createdAt ?? now,
      updatedAt: now
    };

    this.removeSourceInternal(sourceId);

    const chunkIds: string[] = [];
    prepared.chunks.forEach((chunk, index) => {
      const chunkId = stableChunkId(sourceId, index, chunk.startLine, chunk.endLine, chunk.text);
      chunkIds.push(chunkId);
      this.embeddingCache.set(this.embeddingCacheKey(chunk.text), normalizeVector(prepared.embeddings[index] ?? []));
      this.snapshot.chunks[chunkId] = {
        ...chunk,
        chunkId,
        sourceId,
        sourceKind: prepared.document.kind,
        locator: prepared.document.locator,
        label: prepared.document.label,
        title,
        embedding: normalizeVector(prepared.embeddings[index] ?? [])
      };
    });

    sourceRecord.chunkIds = chunkIds;
    sourceRecord.chunkCount = chunkIds.length;
    sourceRecord.updatedAt = now;
    sourceRecord.createdAt = existingSource?.createdAt ?? now;

    this.snapshot.sources[sourceId] = sourceRecord;
    this.snapshot.embeddingProvider = this.embeddingProvider.name;
    this.snapshot.updatedAt = now;
    const commitMs = Date.now() - commitStartedAt;
    let persistMs = 0;
    if (options.persist ?? true) {
      const persistStartedAt = Date.now();
      await this.persist();
      persistMs = Date.now() - persistStartedAt;
    }

    return {
      sourceId,
      kind: sourceRecord.kind,
      locator: sourceRecord.locator,
      label: sourceRecord.label,
      chunkCount: sourceRecord.chunkCount,
      updatedAt: now,
      metrics: {
        ...prepared.metrics,
        commitMs,
        persistMs,
        totalMs: (prepared.metrics.totalMs ?? 0) + commitMs + persistMs
      }
    };
  }

  private async commitPreparedIngestSources(prepareds: RagPreparedIngest[]): Promise<RagIngestResult[]> {
    if (!prepareds.length) {
      return [];
    }

    return this.runMutation(async () => {
      const results: RagIngestResult[] = [];
      for (const prepared of prepareds) {
        results.push(await this.commitPreparedIngestSource(prepared, { persist: false }));
      }

      const persistStartedAt = Date.now();
      await this.persist();
      const persistMs = Date.now() - persistStartedAt;
      if (results.length && results[0].metrics) {
        results[0].metrics.persistMs = persistMs;
        results[0].metrics.totalMs = (results[0].metrics.totalMs ?? 0) + persistMs;
      }

      return results;
    });
  }

  private async embedChunks(
    chunks: Array<{
      text: string;
      startLine: number;
      endLine: number;
      tokenCount: number;
      heading?: string;
    }>,
    onProgress?: (processed: number, total: number, detail?: string, chunksPerSecond?: number) => void
  ): Promise<{ embeddings: number[][]; stats: { batchSize: number; batchCount: number; cacheHits: number; cacheMisses: number; reusedEmbeddings: number } }> {
    if (!chunks.length) {
      return {
        embeddings: [],
        stats: {
          batchSize: 0,
          batchCount: 0,
          cacheHits: 0,
          cacheMisses: 0,
          reusedEmbeddings: 0
        }
      };
    }

    const batchSize = Math.max(8, Math.min(64, this.embeddingBatchSize));
    const embeddings: number[][] = [];
    let batchCount = 0;
    let cacheHits = 0;
    let cacheMisses = 0;
    let reusedEmbeddings = 0;

    for (let index = 0; index < chunks.length; index += batchSize) {
      const batch = chunks.slice(index, index + batchSize);
      const processed = Math.min(index + batch.length, chunks.length);
      batchCount += 1;
      onProgress?.(index, chunks.length, `Embedding chunks ${Math.min(index + 1, chunks.length)}/${chunks.length}`);

      const batchEmbeddings: number[][] = new Array(batch.length);
      const pendingTexts: string[] = [];
      const pendingOffsets: number[] = [];
      for (let offset = 0; offset < batch.length; offset += 1) {
        const chunk = batch[offset];
        const cached = this.embeddingCache.get(this.embeddingCacheKey(chunk.text));
        if (cached) {
          cacheHits += 1;
          reusedEmbeddings += 1;
          batchEmbeddings[offset] = cached;
        } else {
          cacheMisses += 1;
          pendingTexts.push(chunk.text);
          pendingOffsets.push(offset);
        }
      }

      const batchStartedAt = Date.now();
      if (pendingTexts.length) {
        const embedded = await this.embeddingProvider.embed(pendingTexts);
        pendingOffsets.forEach((offset, embeddedIndex) => {
          const vector = normalizeVector(embedded[embeddedIndex] ?? []);
          batchEmbeddings[offset] = vector;
          this.embeddingCache.set(this.embeddingCacheKey(batch[offset].text), vector);
        });
      }

      embeddings.push(...batchEmbeddings.map((vector) => normalizeVector(vector)));
      const elapsedSeconds = (Date.now() - batchStartedAt) / 1000;
      const rate = pendingTexts.length > 0 && elapsedSeconds > 0 ? pendingTexts.length / elapsedSeconds : undefined;
      onProgress?.(processed, chunks.length, `Embedding chunks ${Math.min(index + batch.length, chunks.length)}/${chunks.length}`, rate);
      await yieldToEventLoop();
    }

    return {
      embeddings,
      stats: {
        batchSize,
        batchCount,
        cacheHits,
        cacheMisses,
        reusedEmbeddings
      }
    };
  }
}

function isMissingFileError(error: unknown): boolean {
  return Boolean(error && typeof error === 'object' && 'code' in error && (error as { code?: string }).code === 'ENOENT');
}

function isSkipFileError(error: unknown): boolean {
  return Boolean(error && typeof error === 'object' && 'code' in error && (error as { code?: string }).code === 'RAG_SKIP_FILE');
}

function createSkipFileError(message: string): Error {
  const error = new Error(message);
  (error as Error & { code?: string }).code = 'RAG_SKIP_FILE';
  return error;
}

function isTextBasedMimeType(mimeType: string | undefined): boolean {
  if (!mimeType) {
    return false;
  }

  const normalized = mimeType.trim().toLowerCase().split(';', 1)[0];
  if (!normalized) {
    return false;
  }

  if (TEXT_MIME_PREFIXES.some((prefix) => normalized.startsWith(prefix))) {
    return true;
  }

  return TEXT_MIME_TYPES.has(normalized);
}

function inferMimeTypeFromPath(filePath: string): string | undefined {
  const ext = path.extname(filePath).toLowerCase();
  switch (ext) {
    case '.txt':
    case '.md':
    case '.mdx':
    case '.rst':
    case '.csv':
    case '.log':
    case '.json':
    case '.jsonl':
    case '.xml':
    case '.html':
    case '.htm':
    case '.css':
    case '.scss':
    case '.less':
    case '.js':
    case '.mjs':
    case '.cjs':
    case '.ts':
    case '.tsx':
    case '.jsx':
    case '.py':
    case '.rb':
    case '.go':
    case '.java':
    case '.c':
    case '.cc':
    case '.cpp':
    case '.h':
    case '.hpp':
    case '.cs':
    case '.php':
    case '.rs':
    case '.sh':
    case '.yml':
    case '.yaml':
    case '.toml':
    case '.ini':
    case '.cfg':
      return ext === '.json' || ext === '.jsonl'
        ? 'application/json'
        : ext === '.xml'
          ? 'application/xml'
          : ext === '.html' || ext === '.htm'
            ? 'text/html'
            : ext === '.csv'
              ? 'text/csv'
              : ext === '.js' || ext === '.mjs' || ext === '.cjs'
                ? 'application/javascript'
                : ext === '.ts' || ext === '.tsx'
                  ? 'application/typescript'
                  : 'text/plain';
    default:
      return undefined;
  }
}

function inferMimeTypeFromBuffer(buffer: Buffer, filePath: string): string | undefined {
  if (!buffer.length) {
    return 'text/plain';
  }

  if (!looksLikeTextBuffer(buffer)) {
    return undefined;
  }

  const trimmed = buffer.subarray(0, Math.min(buffer.length, 4096)).toString('utf8').trimStart();
  if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
    return 'application/json';
  }
  if (trimmed.startsWith('<?xml') || trimmed.startsWith('<')) {
    return 'application/xml';
  }
  if (/^(<!doctype html|<html\b)/i.test(trimmed)) {
    return 'text/html';
  }
  if (/^\s*#!/.test(trimmed) || /(?:^|\n)\s*(function|class|interface|export|import|const|let|var|def)\b/.test(trimmed)) {
    return inferKindFromLocator(filePath, 'application/javascript', 'code') === 'code'
      ? 'application/javascript'
      : 'text/plain';
  }

  return 'text/plain';
}

function looksLikeTextBuffer(buffer: Buffer): boolean {
  const sampleLength = Math.min(buffer.length, 4096);
  if (!sampleLength) {
    return true;
  }

  let suspicious = 0;
  for (let index = 0; index < sampleLength; index += 1) {
    const byte = buffer[index];
    if (byte === 0) {
      return false;
    }
    if (byte < 7 || (byte > 14 && byte < 32)) {
      suspicious += 1;
    }
  }

  return suspicious / sampleLength < 0.15;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${Math.round(bytes / 1024)} KB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function stableSourceId(kind: string, locator: string, contentHash: string): string {
  return hashText(`${kind}|${normalizeLocator(locator)}|${contentHash}`);
}

function stableChunkId(sourceId: string, chunkIndex: number, startLine: number, endLine: number, text: string): string {
  return hashText(`${sourceId}|${chunkIndex}|${startLine}|${endLine}|${text.slice(0, 256)}`);
}

function hashText(text: string): string {
  return createHash('sha256').update(text).digest('hex');
}

function stableContentHash(content: string): string {
  return hashText(normalizeLineEndings(content));
}

function normalizeLocator(locator: string): string {
  return locator.trim().replace(/\\/g, '/');
}

function normalizePathInput(value: string): string {
  const trimmed = value.trim();
  if ((trimmed.startsWith('"') && trimmed.endsWith('"')) || (trimmed.startsWith("'") && trimmed.endsWith("'"))) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

function isHttpUrl(value: string): boolean {
  return /^https?:\/\//i.test(value.trim());
}

async function collectFiles(
  rootPath: string,
  options: {
    recursive: boolean;
    includeHidden: boolean;
    onProgress?: RagDirectoryIngestOptions['onProgress'];
    shouldStop?: () => boolean;
  }
): Promise<string[]> {
  const results: string[] = [];
    const stack: string[] = [rootPath];
    let directoriesVisited = 0;
    let entriesVisited = 0;

    while (stack.length) {
      if (options.shouldStop?.()) {
        throw createDirectoryScanPausedError();
      }
      const current = stack.pop()!;
      directoriesVisited += 1;
      options.onProgress?.({
        stage: 'scanning',
        processed: directoriesVisited + entriesVisited,
        total: 0,
        currentPath: current,
        detail: 'Scanning directory',
        visitedDirectories: directoriesVisited,
        visitedEntries: entriesVisited,
        discoveredFiles: results.length
      });

    let directoryHandle: Awaited<ReturnType<typeof opendir>>;
    try {
      directoryHandle = await opendir(current);
    } catch (error) {
        if (isIgnorableDirectoryError(error)) {
          options.onProgress?.({
            stage: 'scanning',
            processed: directoriesVisited + entriesVisited,
            total: 0,
            currentPath: current,
            detail: `Skipped unreadable directory`,
            visitedDirectories: directoriesVisited,
            visitedEntries: entriesVisited,
            discoveredFiles: results.length
          });
          continue;
        }

      throw error;
    }

    try {
      for await (const entry of directoryHandle) {
        if (options.shouldStop?.()) {
          throw createDirectoryScanPausedError();
        }
        entriesVisited += 1;
        if (!options.includeHidden && entry.name.startsWith('.')) {
          continue;
        }

        const absoluteEntry = path.join(current, entry.name);

        if (entry.isDirectory()) {
          if (DEFAULT_DIRECTORY_IGNORE.has(entry.name)) {
            continue;
          }
          if (options.recursive) {
            stack.push(absoluteEntry);
          }
          options.onProgress?.({
            stage: 'scanning',
            processed: directoriesVisited + entriesVisited,
            total: 0,
            currentPath: absoluteEntry,
            detail: 'Queued directory',
            visitedDirectories: directoriesVisited,
            visitedEntries: entriesVisited,
            discoveredFiles: results.length
          });
          continue;
        }

        if (!entry.isFile()) {
          continue;
        }

        if (BINARY_EXTENSIONS.has(path.extname(entry.name).toLowerCase())) {
          continue;
        }

        results.push(absoluteEntry);
        options.onProgress?.({
          stage: 'scanning',
          processed: directoriesVisited + entriesVisited,
          total: 0,
          currentPath: absoluteEntry,
          detail: 'Discovered file',
          visitedDirectories: directoriesVisited,
          visitedEntries: entriesVisited,
          discoveredFiles: results.length
        });

        if (results.length % 25 === 0 || entriesVisited % 250 === 0) {
          if (options.shouldStop?.()) {
            throw createDirectoryScanPausedError();
          }
          await yieldToEventLoop();
        }
      }
    } finally {
      await directoryHandle.close().catch(() => {});
    }
  }

  return results;
}

function createDirectoryScanPausedError(): Error {
  const error = new Error('Directory scan paused');
  (error as { code?: string }).code = 'ERR_DIRECTORY_SCAN_PAUSED';
  return error;
}

function isIgnorableDirectoryError(error: unknown): boolean {
  return Boolean(
    error &&
    typeof error === 'object' &&
    'code' in error &&
    ['ENOENT', 'EACCES', 'EPERM', 'EBUSY'].includes(String((error as { code?: string }).code))
  );
}

async function yieldToEventLoop(): Promise<void> {
  await new Promise<void>((resolve) => setImmediate(resolve));
}

function normalizeLineEndings(text: string): string {
  return text.replace(/\r\n/g, '\n').replace(/\r/g, '\n');
}

function estimateTokenCount(text: string): number {
  return Math.max(1, Math.ceil(text.length / 4));
}

function deriveTitle(label: string, content: string): string {
  const heading = content
    .split('\n')
    .map((line) => line.trim())
    .find((line) => /^#{1,6}\s+/.test(line) || /^(?:class|function|interface|type|enum|def)\b/i.test(line));

  if (heading) {
    return heading.replace(/^#{1,6}\s+/, '').trim();
  }

  return label;
}

function deriveLabelFromUrl(url: string): string {
  try {
    const parsed = new URL(url);
    const lastSegment = parsed.pathname.split('/').filter(Boolean).pop();
    return lastSegment ? `${parsed.host}/${lastSegment}` : parsed.host;
  } catch {
    return url;
  }
}

function inferKindFromLocator(locator: string, mimeType?: string, explicitKind?: string): string {
  const normalizedKind = explicitKind?.trim().toLowerCase();
  if (normalizedKind && ['file', 'url', 'text'].includes(normalizedKind)) {
    return normalizedKind;
  }

  const ext = path.extname(locator).toLowerCase();
  if (CODE_EXTENSIONS.has(ext)) {
    return 'code';
  }

  const normalizedMime = (mimeType ?? '').toLowerCase();
  if (normalizedMime.includes('javascript') || normalizedMime.includes('typescript') || normalizedMime.includes('json') || normalizedMime.includes('xml')) {
    return 'code';
  }

  return normalizedKind || 'text';
}

function inferDocumentMode(kind: string, locator: string, content: string): 'code' | 'text' {
  const ext = path.extname(locator).toLowerCase();
  if (CODE_EXTENSIONS.has(ext)) {
    return 'code';
  }

  const lowerKind = kind.toLowerCase();
  if (lowerKind.includes('code') || lowerKind.includes('script') || lowerKind.includes('source')) {
    return 'code';
  }

  const sample = content.slice(0, 1200);
  if (
    /\b(function|class|interface|export|import|const|let|var|return|public|private|protected|namespace|module)\b/.test(sample) ||
    /[{};]/.test(sample)
  ) {
    return 'code';
  }

  return 'text';
}

function isCodeBoundary(line: string): boolean {
  return (
    /^((export|async)\s+)?(function|class|interface|enum|type|const|let|var|def)\b/.test(line) ||
    /^(\s*(public|private|protected)\s+)?(\s*static\s+)?\w+\s*\(/.test(line) ||
    /^#[^\s]/.test(line)
  );
}

function extractHeading(line: string): string | undefined {
  const markdown = line.match(/^#{1,6}\s+(.+)$/);
  if (markdown) {
    return markdown[1].trim();
  }

  const code = line.match(/^((export|async)\s+)?(function|class|interface|enum|type|const|let|var|def)\s+([A-Za-z0-9_.$-]+)/);
  if (code) {
    return `${code[3]} ${code[4]}`.trim();
  }

  return undefined;
}

function normalizeVector(vector: number[]): number[] {
  const magnitude = Math.sqrt(vector.reduce((sum, value) => sum + value * value, 0));
  if (!magnitude) {
    return vector;
  }

  return vector.map((value) => value / magnitude);
}

function cosineSimilarity(a: number[], b: number[]): number {
  if (!a.length || !b.length) {
    return 0;
  }

  const length = Math.min(a.length, b.length);
  let dot = 0;
  let magnitudeA = 0;
  let magnitudeB = 0;

  for (let index = 0; index < length; index += 1) {
    dot += a[index] * b[index];
    magnitudeA += a[index] * a[index];
    magnitudeB += b[index] * b[index];
  }

  if (!magnitudeA || !magnitudeB) {
    return 0;
  }

  return dot / (Math.sqrt(magnitudeA) * Math.sqrt(magnitudeB));
}

function tokenize(text: string): string[] {
  return text
    .toLowerCase()
    .split(/[^a-z0-9_]+/g)
    .map((token) => token.trim())
    .filter(Boolean);
}

function lexicalOverlapScore(queryTokens: Set<string>, chunkTokens: string[]): number {
  if (!queryTokens.size || !chunkTokens.length) {
    return 0;
  }

  const chunkCounts = new Map<string, number>();
  for (const token of chunkTokens) {
    chunkCounts.set(token, (chunkCounts.get(token) ?? 0) + 1);
  }

  let overlap = 0;
  for (const token of queryTokens) {
    if (chunkCounts.has(token)) {
      overlap += 1;
    }
  }

  return overlap / Math.max(queryTokens.size, 1);
}

function buildSnippet(text: string, query: string): string {
  const normalizedText = normalizeLineEndings(text);
  const queryLower = query.trim().toLowerCase();
  const lines = normalizedText.split('\n');
  const queryTokens = tokenize(query);

  if (queryLower) {
    const phraseIndex = normalizedText.toLowerCase().indexOf(queryLower);
    if (phraseIndex >= 0) {
      return sliceAround(normalizedText, phraseIndex, queryLower.length);
    }
  }

  for (const token of queryTokens) {
    const index = normalizedText.toLowerCase().indexOf(token);
    if (index >= 0) {
      return sliceAround(normalizedText, index, token.length);
    }
  }

  return lines.slice(0, Math.min(lines.length, 12)).join('\n').trim();
}

function sliceAround(text: string, index: number, length: number): string {
  const start = Math.max(0, index - 180);
  const end = Math.min(text.length, index + length + 220);
  const prefix = start > 0 ? '...' : '';
  const suffix = end < text.length ? '...' : '';
  return `${prefix}${text.slice(start, end).trim()}${suffix}`;
}

function normalizeRemoteContent(rawText: string, mimeType: string | undefined, url: string): string {
  const normalizedMime = (mimeType ?? '').toLowerCase();
  if (normalizedMime.includes('html')) {
    return htmlToText(rawText, url);
  }

  return normalizeLineEndings(rawText).trim();
}

function htmlToText(html: string, url: string): string {
  const withoutScripts = html
    .replace(/<script[\s\S]*?<\/script>/gi, ' ')
    .replace(/<style[\s\S]*?<\/style>/gi, ' ')
    .replace(/<noscript[\s\S]*?<\/noscript>/gi, ' ');

  const withLineBreaks = withoutScripts
    .replace(/<\/(p|div|section|article|header|footer|li|h[1-6]|tr|pre|blockquote)>/gi, '\n')
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<[^>]+>/g, ' ');

  const decoded = decodeHtmlEntities(withLineBreaks);
  const title = deriveLabelFromUrl(url);
  return `${title}\n${normalizeLineEndings(decoded).replace(/[ \t]+/g, ' ').replace(/\n\s+\n/g, '\n\n').trim()}`.trim();
}

function decodeHtmlEntities(text: string): string {
  return text.replace(/&([a-z]+);/gi, (_match, entity: string) => HTML_ENTITY_MAP[entity.toLowerCase()] ?? _match);
}
