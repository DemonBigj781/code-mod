import { availableParallelism } from 'node:os';
import { Worker } from 'node:worker_threads';

export interface EmbeddingProvider {
  readonly name: string;
  embed(texts: string[]): Promise<number[][]>;
}

export type EmbeddingProviderKind = 'auto' | 'local' | 'openai' | 'optimum';

export interface EmbeddingProviderOptions {
  kind?: EmbeddingProviderKind;
  model?: string;
  baseUrl?: string;
  apiKey?: string;
  localModel?: string;
  device?: string;
  optimumUrl?: string;
  optimumModel?: string;
  workerCount?: number;
}

export function createEmbeddingProvider(options: EmbeddingProviderOptions = {}): EmbeddingProvider {
  const kind = resolveProviderKind(options);

  if (kind === 'openai') {
    return new OpenAICompatibleEmbeddingProvider({
      apiKey: options.apiKey ?? '',
      baseUrl: options.baseUrl ?? 'https://api.openai.com',
      model: options.model ?? 'text-embedding-3-small'
    });
  }

  if (kind === 'optimum') {
    return new OptimumEmbeddingProvider({
      baseUrl: options.optimumUrl ?? options.baseUrl ?? 'http://127.0.0.1:8123',
      model: options.optimumModel ?? options.model ?? 'sentence-transformers/paraphrase-MiniLM-L3-v2'
    });
  }

  return new LocalEmbeddingProvider(
    options.localModel ?? options.model ?? 'Xenova/paraphrase-MiniLM-L3-v2',
    options.device ?? resolveLocalDevice(),
    options.workerCount ?? resolveWorkerCount()
  );
}

function resolveProviderKind(options: EmbeddingProviderOptions): EmbeddingProviderKind {
  if (options.kind && options.kind !== 'auto') {
    return options.kind;
  }

  const optimumUrl = options.optimumUrl ?? process.env.RAG_OPTIMUM_EMBEDDING_URL?.trim();
  if (optimumUrl) {
    return 'optimum';
  }

  const apiKey = options.apiKey ?? process.env.RAG_EMBEDDING_API_KEY ?? process.env.OPENAI_API_KEY;
  if (apiKey) {
    return 'openai';
  }

  return 'local';
}

class LocalEmbeddingProvider implements EmbeddingProvider {
  readonly name: string;
  private readonly model: string;
  private readonly device: string;
  private workerCount: number;
  private poolPromise: Promise<EmbeddingWorkerPool> | null = null;
  private pool: EmbeddingWorkerPool | null = null;

  constructor(model: string, device = resolveLocalDevice(), workerCount = resolveWorkerCount()) {
    this.name = `local:${model}${device ? `:${device}` : ''}`;
    this.model = model;
    this.device = device;
    this.workerCount = workerCount;
  }

  async embed(texts: string[]): Promise<number[][]> {
    if (!texts.length) {
      return [];
    }

    const pool = await this.getPool();
    return pool.embed(texts);
  }

  async setWorkerCount(workerCount: number): Promise<void> {
    const nextCount = Math.max(1, Math.trunc(workerCount));
    if (nextCount === this.workerCount) {
      return;
    }

    this.workerCount = nextCount;
    await this.resetPool();
  }

  async dispose(): Promise<void> {
    await this.resetPool();
  }

  private async getPool(): Promise<EmbeddingWorkerPool> {
    if (!this.poolPromise) {
      this.pool = new EmbeddingWorkerPool(this.model, this.device, this.workerCount);
      this.poolPromise = Promise.resolve(this.pool);
    }

    return this.poolPromise;
  }

  private async resetPool(): Promise<void> {
    const currentPool = this.pool;
    this.pool = null;
    this.poolPromise = null;
    await currentPool?.dispose();
  }
}

class OpenAICompatibleEmbeddingProvider implements EmbeddingProvider {
  readonly name: string;
  private readonly apiKey: string;
  private readonly baseUrl: string;
  private readonly model: string;

  constructor(options: { apiKey: string; baseUrl: string; model: string }) {
    this.name = `openai:${options.model}`;
    this.apiKey = options.apiKey;
    this.baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.model = options.model;
  }

  async embed(texts: string[]): Promise<number[][]> {
    if (!this.apiKey) {
      throw new Error('OpenAI-compatible embedding provider requires an API key.');
    }

    const response = await fetch(`${this.baseUrl}/v1/embeddings`, {
      method: 'POST',
      headers: {
        authorization: `Bearer ${this.apiKey}`,
        'content-type': 'application/json'
      },
      body: JSON.stringify({
        model: this.model,
        input: texts
      })
    });

    const payload = await response.json();

    if (!response.ok) {
      const message = typeof payload?.error?.message === 'string'
        ? payload.error.message
        : `Embedding request failed with HTTP ${response.status}`;
      throw new Error(message);
    }

    const data = Array.isArray(payload?.data) ? payload.data : [];
    return data.map((item: { embedding?: number[] }) => item.embedding ?? []);
  }
}

class OptimumEmbeddingProvider implements EmbeddingProvider {
  readonly name: string;
  private readonly baseUrl: string;
  private readonly model: string;

  constructor(options: { baseUrl: string; model: string }) {
    this.name = `optimum:${options.model}`;
    this.baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.model = options.model;
  }

  async embed(texts: string[]): Promise<number[][]> {
    if (!texts.length) {
      return [];
    }

    const response = await fetch(`${this.baseUrl}/v1/embeddings`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json'
      },
      body: JSON.stringify({
        model: this.model,
        input: texts
      })
    });

    const payload = await response.json();

    if (!response.ok) {
      const message = typeof payload?.error?.message === 'string'
        ? payload.error.message
        : `Optimum embedding request failed with HTTP ${response.status}`;
      throw new Error(message);
    }

    const data = Array.isArray(payload?.data) ? payload.data : [];
    return data.map((item: { embedding?: number[] }) => item.embedding ?? []);
  }
}

class EmbeddingWorkerPool {
  private readonly workers: EmbeddingWorkerHandle[];
  private nextWorkerIndex = 0;

  constructor(model: string, device: string, workerCount: number) {
    this.workers = Array.from({ length: Math.max(1, workerCount) }, () => new EmbeddingWorkerHandle(model, device));
  }

  async embed(texts: string[]): Promise<number[][]> {
    const worker = this.workers[this.nextWorkerIndex];
    this.nextWorkerIndex = (this.nextWorkerIndex + 1) % this.workers.length;
    return worker.embed(texts);
  }

  async dispose(): Promise<void> {
    await Promise.all(this.workers.map((worker) => worker.dispose()));
  }
}

class EmbeddingWorkerHandle {
  private readonly worker: Worker;
  private readonly queue: Array<{
    id: number;
    texts: string[];
    resolve: (value: number[][]) => void;
    reject: (reason?: unknown) => void;
  }> = [];
  private readonly inFlight = new Map<number, {
    resolve: (value: number[][]) => void;
    reject: (reason?: unknown) => void;
  }>();
  private running = false;
  private nextId = 1;

  constructor(model: string, device: string) {
    this.worker = new Worker(createEmbeddingWorkerSource(), {
      eval: true,
      workerData: { model, device }
    });

    this.worker.on('message', (message: { id?: number; result?: number[][]; error?: string }) => {
      if (!message || typeof message.id !== 'number') {
        return;
      }

      const task = this.inFlight.get(message.id);
      if (!task) {
        return;
      }

      this.inFlight.delete(message.id);
      if (typeof message.error === 'string') {
        task.reject(new Error(message.error));
      } else {
        task.resolve(Array.isArray(message.result) ? message.result : []);
      }
      this.running = false;
      void this.processQueue();
    });

    this.worker.on('error', (error) => {
      this.running = false;
      this.failAll(error);
    });

    this.worker.on('exit', (code) => {
      if (code !== 0) {
        this.running = false;
        this.failAll(new Error(`Embedding worker exited with code ${code}`));
      }
    });
  }

  embed(texts: string[]): Promise<number[][]> {
    return new Promise<number[][]>((resolve, reject) => {
      this.queue.push({
        id: this.nextId++,
        texts,
        resolve,
        reject
      });
      void this.processQueue();
    });
  }

  private async processQueue(): Promise<void> {
    if (this.running) {
      return;
    }

    const next = this.queue.shift();
    if (!next) {
      return;
    }

    this.running = true;
    this.inFlight.set(next.id, next);
    this.worker.postMessage({
      id: next.id,
      texts: next.texts
    });
  }

  private failAll(error: unknown): void {
    const message = error instanceof Error ? error : new Error(String(error));
    for (const task of this.queue) {
      task.reject(message);
    }
    this.queue.length = 0;

    for (const task of this.inFlight.values()) {
      task.reject(message);
    }
    this.inFlight.clear();
  }

  async dispose(): Promise<void> {
    this.worker.removeAllListeners();
    await this.worker.terminate();
  }
}

function createEmbeddingWorkerSource(): string {
  return `
    const { parentPort, workerData } = require('node:worker_threads');

    (async () => {
      const { pipeline, env } = await import('@xenova/transformers');
      env.allowLocalModels = true;
      env.useBrowserCache = false;

      let extractorPromise = null;
      let activeDevice = workerData.device || 'auto';

      async function getExtractor() {
        if (!extractorPromise) {
          extractorPromise = (async () => {
            try {
              return await pipeline(
                'feature-extraction',
                workerData.model,
                activeDevice ? { device: activeDevice } : undefined
              );
            } catch (error) {
              if (activeDevice && activeDevice !== 'auto') {
                activeDevice = 'auto';
                return await pipeline('feature-extraction', workerData.model);
              }
              throw error;
            }
          })();
        }

        return extractorPromise;
      }

      parentPort.on('message', async (message) => {
        const id = message && typeof message.id === 'number' ? message.id : null;
        const texts = message && Array.isArray(message.texts) ? message.texts : [];

        if (id === null) {
          return;
        }

        try {
          const extractor = await getExtractor();
          const vectors = [];
          for (const text of texts) {
            const output = await extractor(text, {
              pooling: 'mean',
              normalize: true
            });
            vectors.push(Array.from(output.data));
          }

          parentPort.postMessage({ id, result: vectors });
        } catch (error) {
          parentPort.postMessage({
            id,
            error: error && error.message ? error.message : String(error)
          });
        }
      });
    })().catch((error) => {
      parentPort.postMessage({
        id: -1,
        error: error && error.message ? error.message : String(error)
      });
    });
  `;
}

function resolveLocalDevice(): string {
  if (process.env.RAG_EMBEDDING_DEVICE?.trim()) {
    return process.env.RAG_EMBEDDING_DEVICE.trim().toLowerCase();
  }

  return 'auto';
}

function resolveWorkerCount(): number {
  const envCount = Number(process.env.RAG_EMBEDDING_WORKERS ?? '');
  if (Number.isInteger(envCount) && envCount > 0) {
    return envCount;
  }

  const cpuCount = availableParallelism();
  return Math.max(1, cpuCount - 2);
}
