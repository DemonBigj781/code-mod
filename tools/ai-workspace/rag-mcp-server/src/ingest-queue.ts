import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { randomUUID } from 'node:crypto';

import type { RagDirectoryDiscoveryOptions, RagIngestResult, RagIndex, RagSourceInput } from './rag.js';

export type QueueJobStatus = 'queued' | 'scanning' | 'running' | 'complete' | 'error' | 'interrupted';
export type QueueJobKind = 'directory' | 'sources';

export interface QueueJobProgress {
  stage: 'scanning' | 'processing' | 'complete';
  processed: number;
  total: number;
  stageStartedAt?: string;
  currentPath?: string;
  detail?: string;
  fileProcessed?: number;
  fileTotal?: number;
  chunksPerSecond?: number;
  visitedDirectories?: number;
  visitedEntries?: number;
  discoveredFiles?: number;
}

export interface QueueJobRecord {
  id: string;
  kind: QueueJobKind;
  status: QueueJobStatus;
  directoryPath: string;
  recursive: boolean;
  includeHidden: boolean;
  concurrency: number;
  startedAt: string;
  updatedAt: string;
  completedAt?: string;
  interruptedAt?: string;
  interruptedReason?: string;
  error?: string;
  progress?: QueueJobProgress;
  filePaths: string[];
  sourceInputs?: RagSourceInput[];
  nextFileIndex: number;
  nextSourceIndex?: number;
  totalFiles: number;
  totalSources?: number;
  completedFiles: number;
  completedSources?: number;
  skippedFiles: number;
  skippedSources?: number;
  result?: {
    kind: QueueJobKind;
    directoryPath: string;
    totalFiles?: number;
    completedFiles?: number;
    skippedFiles?: number;
    sourceCount?: number;
    completedSources?: number;
    skippedSources?: number;
    resumed: boolean;
    metrics?: {
      filesProcessed: number;
      filesSkipped: number;
      chunksCreated: number;
      chunksCacheHits: number;
      chunksEmbedded: number;
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
    };
  };
}

interface QueueState {
  version: number;
  updatedAt: string;
  paused: boolean;
  order: string[];
  jobs: Record<string, QueueJobRecord>;
}

export interface EnqueueDirectoryJobInput {
  directoryPath: string;
  recursive?: boolean;
  includeHidden?: boolean;
  concurrency?: number;
}

export interface EnqueueSourcesJobInput {
  sources: RagSourceInput[];
  label?: string;
  concurrency?: number;
}

export class IngestQueueManager {
  private readonly index: RagIndex;
  private readonly statePath: string;
  private readonly onJobUpdate?: (job: QueueJobRecord) => void;
  private state: QueueState;
  private ready: Promise<void>;
  private processing = false;
  private saveChain: Promise<void> = Promise.resolve();
  private scanNotifications = 0;

  constructor(index: RagIndex, dataDir: string, onJobUpdate?: (job: QueueJobRecord) => void) {
    this.index = index;
    this.statePath = path.join(dataDir, 'ingest-queue.json');
    this.onJobUpdate = onJobUpdate;
    this.state = {
      version: 1,
      updatedAt: new Date().toISOString(),
      paused: false,
      order: [],
      jobs: {}
    };
    this.ready = this.load();
  }

  async load(): Promise<void> {
    await mkdir(path.dirname(this.statePath), { recursive: true });

    try {
      const raw = await readFile(this.statePath, 'utf8');
      const parsed = JSON.parse(raw) as Partial<QueueState>;
      if (parsed && typeof parsed === 'object') {
        const jobs = parsed.jobs && typeof parsed.jobs === 'object'
          ? (parsed.jobs as Record<string, QueueJobRecord>)
          : {};
        const order = Array.isArray(parsed.order)
          ? parsed.order.filter((jobId): jobId is string => typeof jobId === 'string' && Boolean(jobs[jobId]))
          : Object.keys(jobs);
        this.state = {
          version: 1,
          updatedAt: typeof parsed.updatedAt === 'string' ? parsed.updatedAt : new Date().toISOString(),
          paused: Boolean(parsed.paused),
          order,
          jobs
        };
      }
    } catch (error) {
      if (!isMissingFileError(error)) {
        throw error;
      }
    }

    for (const job of Object.values(this.state.jobs)) {
      job.directoryPath = normalizeDirectoryPathInput(job.directoryPath);
      if (job.status === 'running' || job.status === 'scanning' || job.status === 'queued' || job.status === 'interrupted') {
        job.status = 'interrupted';
        job.interruptedAt ??= new Date().toISOString();
        job.interruptedReason ??= 'Paused after restart';
        if (job.kind === 'sources') {
          const sources = job.sourceInputs ?? [];
          const sourceIndex = Math.max(0, job.nextSourceIndex ?? 0);
          job.progress = {
            stage: 'processing',
            processed: job.completedSources ?? 0,
            total: sources.length,
            currentPath: sources[sourceIndex]?.locator ?? job.directoryPath,
            detail: job.progress?.detail ?? 'Paused after restart',
            discoveredFiles: sources.length
          };
        } else {
          job.progress = {
            stage: 'processing',
            processed: job.completedFiles,
            total: job.totalFiles,
            currentPath: job.filePaths[job.nextFileIndex] ?? job.directoryPath,
            detail: job.progress?.detail ?? 'Paused after restart',
            visitedDirectories: job.progress?.visitedDirectories,
            visitedEntries: job.progress?.visitedEntries,
            discoveredFiles: job.totalFiles
          };
        }
      }
    }

    await this.persist();
  }

  async enqueueDirectoryJob(input: EnqueueDirectoryJobInput): Promise<QueueJobRecord> {
    await this.ready;

    const directoryPath = normalizeDirectoryPathInput(input.directoryPath);
    const job: QueueJobRecord = {
      id: randomUUID(),
      kind: 'directory',
      status: 'queued',
      directoryPath,
      recursive: input.recursive ?? true,
      includeHidden: input.includeHidden ?? false,
      concurrency: input.concurrency ?? 8,
      startedAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      filePaths: [],
      sourceInputs: undefined,
      nextFileIndex: 0,
      nextSourceIndex: undefined,
      totalFiles: 0,
      totalSources: undefined,
      completedFiles: 0,
      completedSources: undefined,
      skippedFiles: 0
    };

    this.state.jobs[job.id] = job;
    this.state.order.push(job.id);
    await this.persist();
    void this.processQueue();
    return this.cloneJob(job);
  }

  async enqueueSourcesJob(input: EnqueueSourcesJobInput): Promise<QueueJobRecord> {
    await this.ready;

    const sources = input.sources.map((source) => ({
      ...source,
      kind: source.kind,
      locator: normalizeDirectoryPathInput(source.locator ?? ''),
      path: source.path ? normalizeDirectoryPathInput(source.path) : source.path,
      label: source.label?.trim() || source.locator
    }));

    const job: QueueJobRecord = {
      id: randomUUID(),
      kind: 'sources',
      status: 'queued',
      directoryPath: (input.label ?? 'rag_ingest batch').trim() || 'rag_ingest batch',
      recursive: false,
      includeHidden: false,
      concurrency: input.concurrency ?? 8,
      startedAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      filePaths: [],
      sourceInputs: sources,
      nextFileIndex: 0,
      nextSourceIndex: 0,
      totalFiles: 0,
      totalSources: sources.length,
      completedFiles: 0,
      completedSources: 0,
      skippedFiles: 0,
      skippedSources: 0
    };

    this.state.jobs[job.id] = job;
    this.state.order.push(job.id);
    await this.persist();
    void this.processQueue();
    return this.cloneJob(job);
  }

  async getJob(jobId: string): Promise<QueueJobRecord | undefined> {
    await this.ready;
    const job = this.state.jobs[jobId];
    return job ? this.cloneJob(job) : undefined;
  }

  async listJobs(): Promise<QueueJobRecord[]> {
    await this.ready;
    return this.state.order
      .map((jobId) => this.state.jobs[jobId])
      .filter((job): job is QueueJobRecord => Boolean(job))
      .map((job) => this.cloneJob(job));
  }

  async isPaused(): Promise<boolean> {
    await this.ready;
    return this.state.paused;
  }

  async clearQueue(): Promise<void> {
    await this.ready;
    this.state = {
      version: 1,
      updatedAt: new Date().toISOString(),
      paused: false,
      order: [],
      jobs: {}
    };
    this.processing = false;
    this.scanNotifications = 0;
    await this.persist();
  }

  async clearCompletedJobs(): Promise<number> {
    await this.ready;
    const removable = this.state.order.filter((jobId) => {
      const job = this.state.jobs[jobId];
      return Boolean(job && (job.status === 'complete' || job.status === 'error'));
    });

    if (!removable.length) {
      return 0;
    }

    for (const jobId of removable) {
      delete this.state.jobs[jobId];
    }

    this.state.order = this.state.order.filter((jobId) => !removable.includes(jobId));
    this.state.updatedAt = new Date().toISOString();
    await this.persist();
    return removable.length;
  }

  async resumeQueue(): Promise<number> {
    await this.ready;
    this.state.paused = false;
    let resumed = 0;
    for (const job of this.state.order.map((jobId) => this.state.jobs[jobId]).filter(Boolean)) {
      if (job.status === 'interrupted') {
        job.status = 'queued';
        job.updatedAt = new Date().toISOString();
        job.interruptedAt = undefined;
        job.interruptedReason = undefined;
        if (job.progress) {
          job.progress = {
            ...job.progress,
            detail: job.kind === 'sources' ? 'Resuming queued source ingest' : 'Resuming queued job'
          };
        }
        resumed += 1;
      }
    }

    if (resumed > 0) {
      await this.persist();
      void this.processQueue();
    } else {
      await this.persist();
    }

    return resumed;
  }

  async pauseQueue(reason = 'Paused by user'): Promise<number> {
    await this.ready;
    this.state.paused = true;
    const interrupted = await this.interruptActiveJobs(reason);
    await this.persist();
    return interrupted;
  }

  async interruptActiveJobs(reason = 'Interrupted by shutdown'): Promise<number> {
    await this.ready;
    let interrupted = 0;
    for (const job of this.state.order.map((jobId) => this.state.jobs[jobId]).filter(Boolean)) {
      if (job.status === 'running' || job.status === 'scanning') {
        job.status = 'interrupted';
        job.interruptedAt = new Date().toISOString();
        job.interruptedReason = reason;
        job.updatedAt = job.interruptedAt;
        job.progress = {
          stage: 'processing',
          processed: job.kind === 'sources' ? (job.completedSources ?? 0) : job.completedFiles,
          total: job.kind === 'sources' ? (job.totalSources ?? 0) : job.totalFiles,
          currentPath: job.kind === 'sources'
            ? job.sourceInputs?.[job.nextSourceIndex ?? 0]?.locator ?? job.directoryPath
            : job.filePaths[job.nextFileIndex] ?? job.directoryPath,
          detail: reason,
          visitedDirectories: job.progress?.visitedDirectories,
          visitedEntries: job.progress?.visitedEntries,
          discoveredFiles: job.kind === 'sources' ? (job.totalSources ?? 0) : job.totalFiles
        };
        interrupted += 1;
      }
    }

    if (interrupted > 0) {
      await this.persist();
    }

    return interrupted;
  }

  private async processQueue(): Promise<void> {
    await this.ready;
    if (this.processing) {
      return;
    }

    this.processing = true;
    try {
      while (true) {
        if (this.state.paused) {
          return;
        }
        const job = this.nextJob();
        if (!job) {
          return;
        }

        await this.runJob(job);
      }
    } finally {
      this.processing = false;
    }
  }

  private nextJob(): QueueJobRecord | undefined {
    return this.state.order
      .map((jobId) => this.state.jobs[jobId])
      .find((job) => job && job.status === 'queued');
  }

  private async runJob(job: QueueJobRecord): Promise<void> {
    if (job.kind === 'sources') {
      await this.runSourceJob(job);
      return;
    }

    this.scanNotifications = 0;
    const resumeMode = job.filePaths.length > 0 && job.nextFileIndex > 0;
    const metricsSummary = createJobMetricsSummary();
    job.interruptedAt = undefined;
    job.interruptedReason = undefined;
    if (!job.filePaths.length) {
      job.status = 'scanning';
      job.updatedAt = new Date().toISOString();
      job.progress = {
        stage: 'scanning',
        processed: 0,
        total: 0,
        stageStartedAt: job.startedAt,
        currentPath: job.directoryPath,
        detail: 'Discovering files',
        visitedDirectories: 0,
        visitedEntries: 0,
        discoveredFiles: 0
      };
      await this.persist();

      let filePaths: string[];
      try {
        filePaths = await this.index.discoverDirectoryFiles(job.directoryPath, {
          recursive: job.recursive,
          includeHidden: job.includeHidden,
          shouldStop: () => this.state.paused,
          onProgress: (progress) => {
            job.progress = {
              ...progress,
              currentPath: progress.currentPath ?? job.directoryPath,
              detail: progress.detail ?? 'Discovering files',
              visitedDirectories: progress.visitedDirectories,
              visitedEntries: progress.visitedEntries,
              discoveredFiles: progress.discoveredFiles
            };
            job.updatedAt = new Date().toISOString();
            if (progress.processed - this.scanNotifications >= 25 || progress.stage === 'complete') {
              this.scanNotifications = progress.processed;
              void this.persist();
            }
          }
        });
      } catch (error) {
        if (isDirectoryScanPausedError(error)) {
          job.status = 'interrupted';
          job.interruptedAt = new Date().toISOString();
          job.interruptedReason = 'Paused by user';
          job.updatedAt = job.interruptedAt;
          job.progress = {
            stage: 'scanning',
            processed: job.completedFiles,
            total: job.totalFiles,
            stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
            currentPath: job.progress?.currentPath ?? job.directoryPath,
            detail: 'Paused by user',
            visitedDirectories: job.progress?.visitedDirectories,
            visitedEntries: job.progress?.visitedEntries,
            discoveredFiles: job.progress?.discoveredFiles ?? job.totalFiles
          };
          await this.persist();
          return;
        }

        throw error;
      }

      job.filePaths = filePaths;
      job.totalFiles = filePaths.length;
      job.nextFileIndex = Math.min(job.nextFileIndex, filePaths.length);
      job.status = 'queued';
      job.updatedAt = new Date().toISOString();
      job.progress = {
        stage: 'scanning',
        processed: filePaths.length,
        total: filePaths.length,
        stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
        currentPath: job.directoryPath,
        detail: 'Discovery complete',
        discoveredFiles: filePaths.length,
        visitedDirectories: job.progress?.visitedDirectories,
        visitedEntries: job.progress?.visitedEntries
      };
      await this.persist();
    }

    job.status = 'running';
    job.updatedAt = new Date().toISOString();
    job.progress = {
      stage: 'processing',
      processed: job.completedFiles,
      total: job.totalFiles,
      stageStartedAt: new Date().toISOString(),
      currentPath: job.filePaths[job.nextFileIndex] ?? job.directoryPath,
      detail: resumeMode ? 'Resuming queued job' : 'Processing files',
      discoveredFiles: job.totalFiles,
      visitedDirectories: job.progress?.visitedDirectories,
      visitedEntries: job.progress?.visitedEntries
    };
    await this.persist();

    const batchSize = getJobBatchSize(job);
    for (let index = job.nextFileIndex; index < job.filePaths.length; index += batchSize) {
      if (this.state.paused) {
        job.status = 'interrupted';
        job.interruptedAt = new Date().toISOString();
        job.interruptedReason = 'Paused by user';
        job.updatedAt = job.interruptedAt;
        job.progress = {
          stage: 'processing',
          processed: job.completedFiles,
          total: job.totalFiles,
          stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
          currentPath: job.filePaths[index] ?? job.directoryPath,
          detail: 'Paused by user',
          discoveredFiles: job.totalFiles,
          visitedDirectories: job.progress?.visitedDirectories,
          visitedEntries: job.progress?.visitedEntries
        };
        await this.persist();
        return;
      }

      const batchPaths = job.filePaths.slice(index, index + batchSize);
      const batchInputs: RagSourceInput[] = batchPaths.map((filePath) => ({
        kind: 'file',
        locator: filePath,
        path: filePath,
        label: path.relative(job.directoryPath, filePath) || path.basename(filePath)
      }));
      let batchSkipped = 0;

      try {
        const results = await this.index.ingestSources(batchInputs, {
          onProgress: (progress) => {
            job.progress = {
              stage: progress.stage,
              processed: job.completedFiles,
              total: job.totalFiles,
              stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
              currentPath: progress.currentPath ?? batchPaths[0] ?? job.directoryPath,
              detail: progress.detail,
              fileProcessed: progress.fileProcessed ?? progress.processed,
              fileTotal: progress.fileTotal ?? progress.total,
              chunksPerSecond: progress.chunksPerSecond,
              visitedDirectories: progress.visitedDirectories,
              visitedEntries: progress.visitedEntries,
              discoveredFiles: progress.discoveredFiles ?? job.totalFiles
            };
            job.updatedAt = new Date().toISOString();
          },
          onSkip: (_input, error) => {
            batchSkipped += 1;
            job.progress = {
              stage: 'processing',
              processed: job.completedFiles,
              total: job.totalFiles,
              stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
              currentPath: batchPaths[0] ?? job.directoryPath,
              detail: error.message,
              discoveredFiles: job.totalFiles,
              visitedDirectories: job.progress?.visitedDirectories,
              visitedEntries: job.progress?.visitedEntries
            };
            job.updatedAt = new Date().toISOString();
          }
        });
        for (const result of results) {
          accumulateJobMetrics(metricsSummary, result.metrics);
        }
        job.completedFiles += results.length;
        job.skippedFiles += batchSkipped;
        job.nextFileIndex = Math.min(index + batchPaths.length, job.filePaths.length);
        job.progress = {
          stage: 'processing',
          processed: job.completedFiles,
          total: job.totalFiles,
          stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
          currentPath: batchPaths[batchPaths.length - 1] ?? job.directoryPath,
          detail: `Indexed ${results.length}/${batchPaths.length} file${batchPaths.length === 1 ? '' : 's'}`,
          discoveredFiles: job.totalFiles,
          visitedDirectories: job.progress?.visitedDirectories,
          visitedEntries: job.progress?.visitedEntries
        };
        job.updatedAt = new Date().toISOString();
        void this.persist();
      } catch (error) {
        job.status = 'error';
        job.error = error instanceof Error ? error.message : String(error);
        job.completedAt = new Date().toISOString();
        job.updatedAt = job.completedAt;
        await this.persist();
        return;
      }
    }

    const cleaned = await this.index.cleanupBinarySources();
    job.status = 'complete';
    job.completedAt = new Date().toISOString();
    job.updatedAt = job.completedAt;
    job.progress = {
      stage: 'complete',
      processed: job.completedFiles,
      total: job.totalFiles,
      stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
      currentPath: job.directoryPath,
      detail: cleaned > 0 ? `Queue complete; cleaned ${cleaned} binary sources` : 'Queue complete',
      discoveredFiles: job.totalFiles,
      visitedDirectories: job.progress?.visitedDirectories,
      visitedEntries: job.progress?.visitedEntries
    };
    job.result = {
      kind: 'directory',
      directoryPath: job.directoryPath,
      totalFiles: job.totalFiles,
      completedFiles: job.completedFiles,
      skippedFiles: job.skippedFiles,
      resumed: resumeMode,
      metrics: {
        ...finalizeJobMetrics(metricsSummary, {
          documentCount: job.completedFiles + job.skippedFiles,
          chunkCount: job.totalFiles,
          totalMs: Date.now() - Date.parse(job.startedAt)
        }),
        filesProcessed: job.completedFiles + job.skippedFiles,
        filesSkipped: job.skippedFiles,
        chunksCreated: metricsSummary.chunkCount,
        chunksCacheHits: metricsSummary.cacheHits,
        chunksEmbedded: metricsSummary.cacheMisses
      }
    };
    await this.persist();
  }

  private async runSourceJob(job: QueueJobRecord): Promise<void> {
    this.scanNotifications = 0;
    const sources = job.sourceInputs ?? [];
    const resumeMode = sources.length > 0 && (job.nextSourceIndex ?? 0) > 0;
    const metricsSummary = createJobMetricsSummary();
    job.interruptedAt = undefined;
    job.interruptedReason = undefined;
    job.totalSources = sources.length;
    job.completedSources ??= 0;
    job.skippedSources ??= 0;

    job.status = 'running';
    job.updatedAt = new Date().toISOString();
    job.progress = {
      stage: 'processing',
      processed: job.completedSources,
      total: sources.length,
      stageStartedAt: new Date().toISOString(),
      currentPath: sources[job.nextSourceIndex ?? 0]?.locator ?? job.directoryPath,
      detail: resumeMode ? 'Resuming queued source ingest' : 'Processing sources',
      discoveredFiles: sources.length
    };
    await this.persist();

    const batchSize = getJobBatchSize(job);
    for (let index = job.nextSourceIndex ?? 0; index < sources.length; index += batchSize) {
      if (this.state.paused) {
        job.status = 'interrupted';
        job.interruptedAt = new Date().toISOString();
        job.interruptedReason = 'Paused by user';
        job.updatedAt = job.interruptedAt;
        job.progress = {
          stage: 'processing',
          processed: job.completedSources ?? 0,
          total: sources.length,
          stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
          currentPath: sources[index]?.locator ?? job.directoryPath,
          detail: 'Paused by user',
          discoveredFiles: sources.length
        };
        await this.persist();
        return;
      }

      const batchSources = sources.slice(index, index + batchSize);
      try {
        const results = await this.index.ingestSources(batchSources, {
          onProgress: (progress) => {
            job.progress = {
              stage: progress.stage,
              processed: job.completedSources ?? 0,
              total: sources.length,
              stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
              currentPath: progress.currentPath ?? batchSources[0]?.locator ?? job.directoryPath,
              detail: progress.detail,
              fileProcessed: progress.fileProcessed ?? progress.processed,
              fileTotal: progress.fileTotal ?? progress.total,
              chunksPerSecond: progress.chunksPerSecond,
              discoveredFiles: sources.length
            };
            job.updatedAt = new Date().toISOString();
          },
          onSkip: (_input, error) => {
            job.skippedSources = (job.skippedSources ?? 0) + 1;
            job.progress = {
              stage: 'processing',
              processed: job.completedSources ?? 0,
              total: sources.length,
              stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
              currentPath: batchSources[0]?.locator ?? job.directoryPath,
              detail: error.message,
              discoveredFiles: sources.length
            };
            job.updatedAt = new Date().toISOString();
          }
        });
        for (const result of results) {
          accumulateJobMetrics(metricsSummary, result.metrics);
        }
        job.completedSources = (job.completedSources ?? 0) + results.length;
        job.nextSourceIndex = Math.min(index + batchSources.length, sources.length);
        job.progress = {
          stage: 'processing',
          processed: job.completedSources,
          total: sources.length,
          stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
          currentPath: batchSources[batchSources.length - 1]?.locator ?? job.directoryPath,
          detail: `Indexed ${results.length}/${batchSources.length} source${batchSources.length === 1 ? '' : 's'}`,
          discoveredFiles: sources.length
        };
        job.updatedAt = new Date().toISOString();
        void this.persist();
      } catch (error) {
        if (isDirectoryScanPausedError(error)) {
          job.status = 'interrupted';
          job.interruptedAt = new Date().toISOString();
          job.interruptedReason = 'Paused by user';
          job.updatedAt = job.interruptedAt;
          job.progress = {
            stage: 'processing',
            processed: job.completedSources ?? 0,
            total: sources.length,
            stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
            currentPath: batchSources[0]?.locator ?? job.directoryPath,
            detail: 'Paused by user',
            discoveredFiles: sources.length
          };
          await this.persist();
          return;
        }

        job.status = 'error';
        job.error = error instanceof Error ? error.message : String(error);
        job.completedAt = new Date().toISOString();
        job.updatedAt = job.completedAt;
        await this.persist();
        return;
      }
    }

    const cleaned = await this.index.cleanupBinarySources();
    job.status = 'complete';
    job.completedAt = new Date().toISOString();
    job.updatedAt = job.completedAt;
    job.progress = {
      stage: 'complete',
      processed: job.completedSources ?? 0,
      total: sources.length,
      stageStartedAt: job.progress?.stageStartedAt ?? job.startedAt,
      currentPath: job.directoryPath,
      detail: cleaned > 0 ? `Queue complete; cleaned ${cleaned} binary sources` : 'Queue complete',
      discoveredFiles: sources.length
    };
    job.result = {
      kind: 'sources',
      directoryPath: job.directoryPath,
      sourceCount: sources.length,
      completedSources: job.completedSources ?? 0,
      skippedSources: job.skippedSources ?? 0,
      resumed: resumeMode,
      metrics: {
        ...finalizeJobMetrics(metricsSummary, {
          documentCount: job.completedSources ?? 0,
          chunkCount: sources.length,
          totalMs: Date.now() - Date.parse(job.startedAt)
        }),
        filesProcessed: (job.completedSources ?? 0) + (job.skippedSources ?? 0),
        filesSkipped: job.skippedSources ?? 0,
        chunksCreated: metricsSummary.chunkCount,
        chunksCacheHits: metricsSummary.cacheHits,
        chunksEmbedded: metricsSummary.cacheMisses
      }
    };
    await this.persist();
  }

  private async persist(): Promise<void> {
    const updatedAt = new Date().toISOString();
    const payload: QueueState = {
      version: 1,
      updatedAt,
      paused: this.state.paused,
      order: [...this.state.order],
      jobs: JSON.parse(JSON.stringify(this.state.jobs)) as Record<string, QueueJobRecord>
    };

    this.state.updatedAt = updatedAt;
    const next = this.saveChain.then(async () => {
      const text = JSON.stringify(payload, null, 2);
      const tempPath = `${this.statePath}.tmp`;
      await mkdir(path.dirname(this.statePath), { recursive: true });
      await writeFile(tempPath, text, 'utf8');
      await rm(this.statePath, { force: true });
      await rename(tempPath, this.statePath);
    }, async () => {
      const text = JSON.stringify(payload, null, 2);
      const tempPath = `${this.statePath}.tmp`;
      await mkdir(path.dirname(this.statePath), { recursive: true });
      await writeFile(tempPath, text, 'utf8');
      await rm(this.statePath, { force: true });
      await rename(tempPath, this.statePath);
    });

    this.saveChain = next.then(() => undefined, () => undefined);
    await next;
  }

  private cloneJob(job: QueueJobRecord): QueueJobRecord {
    return JSON.parse(JSON.stringify(job)) as QueueJobRecord;
  }
}

function isDirectoryScanPausedError(error: unknown): boolean {
  return Boolean(
    error &&
    typeof error === 'object' &&
    'code' in error &&
    (error as { code?: string }).code === 'ERR_DIRECTORY_SCAN_PAUSED'
  );
}

function createJobMetricsSummary(): {
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
} {
  return {
    documentCount: 0,
    chunkCount: 0,
    avgChunkChars: 0,
    avgChunkTokens: 0,
    loadMs: 0,
    chunkMs: 0,
    embedMs: 0,
    commitMs: 0,
    persistMs: 0,
    totalMs: 0,
    batchSize: 0,
    batchCount: 0,
    cacheHits: 0,
    cacheMisses: 0,
    reusedEmbeddings: 0
  };
}

function accumulateJobMetrics(
  summary: ReturnType<typeof createJobMetricsSummary>,
  metrics?: {
    documentCount?: number;
    chunkCount?: number;
    avgChunkChars?: number;
    avgChunkTokens?: number;
    loadMs?: number;
    chunkMs?: number;
    embedMs?: number;
    commitMs?: number;
    persistMs?: number;
    totalMs?: number;
    batchSize?: number;
    batchCount?: number;
    cacheHits?: number;
    cacheMisses?: number;
    reusedEmbeddings?: number;
  }
): void {
  if (!metrics) {
    return;
  }

  summary.documentCount += metrics.documentCount ?? 0;
  summary.chunkCount += metrics.chunkCount ?? 0;
  summary.avgChunkChars += metrics.avgChunkChars ?? 0;
  summary.avgChunkTokens += metrics.avgChunkTokens ?? 0;
  summary.loadMs += metrics.loadMs ?? 0;
  summary.chunkMs += metrics.chunkMs ?? 0;
  summary.embedMs += metrics.embedMs ?? 0;
  summary.commitMs += metrics.commitMs ?? 0;
  summary.persistMs += metrics.persistMs ?? 0;
  summary.totalMs += metrics.totalMs ?? 0;
  summary.batchSize += metrics.batchSize ?? 0;
  summary.batchCount += metrics.batchCount ?? 0;
  summary.cacheHits += metrics.cacheHits ?? 0;
  summary.cacheMisses += metrics.cacheMisses ?? 0;
  summary.reusedEmbeddings += metrics.reusedEmbeddings ?? 0;
}

function finalizeJobMetrics(
  summary: ReturnType<typeof createJobMetricsSummary>,
  fallback: { documentCount: number; chunkCount: number; totalMs: number }
): ReturnType<typeof createJobMetricsSummary> {
  const documentCount = summary.documentCount || fallback.documentCount;
  const chunkCount = summary.chunkCount || fallback.chunkCount;
  return {
    ...summary,
    documentCount,
    chunkCount,
    avgChunkChars: documentCount ? Math.round(summary.avgChunkChars / Math.max(documentCount, 1)) : 0,
    avgChunkTokens: documentCount ? Math.round(summary.avgChunkTokens / Math.max(documentCount, 1)) : 0,
    batchSize: summary.batchCount ? Math.round(summary.batchSize / summary.batchCount) : summary.batchSize,
    totalMs: summary.totalMs || fallback.totalMs
  };
}

function getJobBatchSize(job: QueueJobRecord): number {
  return Math.max(1, Math.min(job.concurrency || 8, 16));
}

function normalizeDirectoryPathInput(value: string): string {
  const trimmed = value.trim();
  if ((trimmed.startsWith('"') && trimmed.endsWith('"')) || (trimmed.startsWith("'") && trimmed.endsWith("'"))) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

function isMissingFileError(error: unknown): boolean {
  return Boolean(error && typeof error === 'object' && 'code' in error && (error as { code?: string }).code === 'ENOENT');
}

function isSkipFileError(error: unknown): boolean {
  return Boolean(error && typeof error === 'object' && 'code' in error && (error as { code?: string }).code === 'RAG_SKIP_FILE');
}
