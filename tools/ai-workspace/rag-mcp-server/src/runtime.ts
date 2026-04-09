import path from 'node:path';
import { availableParallelism } from 'node:os';

export function resolveDataDir(): string {
  const envDir = process.env.RAG_DATA_DIR?.trim();
  if (envDir) {
    return path.isAbsolute(envDir) ? envDir : path.resolve(process.cwd(), envDir);
  }

  return path.resolve(process.cwd(), 'data');
}

export function resolveEmbeddingOptions() {
  const provider = (process.env.RAG_EMBEDDING_PROVIDER ?? 'auto').trim().toLowerCase();
  return {
    kind: provider === 'openai' || provider === 'local' ? provider : 'auto',
    model: process.env.RAG_EMBEDDING_MODEL?.trim() || undefined,
    baseUrl: process.env.RAG_EMBEDDING_BASE_URL?.trim() || process.env.OPENAI_BASE_URL?.trim() || undefined,
    apiKey: process.env.RAG_EMBEDDING_API_KEY?.trim() || process.env.OPENAI_API_KEY?.trim() || undefined,
    localModel: process.env.RAG_LOCAL_EMBEDDING_MODEL?.trim() || undefined,
    device: process.env.RAG_EMBEDDING_DEVICE?.trim() || undefined,
    optimumUrl: process.env.RAG_OPTIMUM_EMBEDDING_URL?.trim() || undefined,
    optimumModel: process.env.RAG_OPTIMUM_EMBEDDING_MODEL?.trim() || undefined,
    embeddingBatchSize: (() => {
      const value = Number(process.env.RAG_EMBED_BATCH_SIZE ?? '');
      if (Number.isInteger(value) && value > 0) {
        return value;
      }
      return 32;
    })(),
    workerCount: (() => {
      const value = Number(process.env.RAG_EMBEDDING_WORKERS ?? '');
      if (Number.isInteger(value) && value > 0) {
        return value;
      }

      const cpuCount = availableParallelism();
      return Math.max(1, cpuCount - 2);
    })()
  } as const;
}
