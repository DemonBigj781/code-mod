from __future__ import annotations

import os
import threading
from dataclasses import dataclass, field
from typing import Any

import numpy as np
import onnxruntime as ort
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field
from transformers import AutoTokenizer, pipeline
from optimum.onnxruntime import ORTModelForFeatureExtraction


DEFAULT_MODEL = os.getenv("EMBEDDING_MODEL", "sentence-transformers/paraphrase-MiniLM-L3-v2")
DEFAULT_PROVIDER = os.getenv("EMBEDDING_PROVIDER", "").strip() or (
    "DmlExecutionProvider" if "DmlExecutionProvider" in ort.get_available_providers() else "CPUExecutionProvider"
)


class EmbeddingRequest(BaseModel):
    input: str | list[str] = Field(..., description="One string or a list of strings to embed.")
    model: str | None = Field(default=None, description="Optional model hint.")


class EmbeddingItem(BaseModel):
    object: str = "embedding"
    index: int
    embedding: list[float]


class EmbeddingResponse(BaseModel):
    object: str = "list"
    model: str
    data: list[EmbeddingItem]
    usage: dict[str, int]


@dataclass
class EmbeddingRuntime:
    model_name: str
    provider: str
    tokenizer: Any | None = None
    extractor: Any | None = None
    resolved_provider: str | None = None
    load_error: str | None = None
    lock: threading.RLock = field(default_factory=threading.RLock)

    def ensure_loaded(self) -> None:
        with self.lock:
            if self.extractor is not None:
                return

            available = ort.get_available_providers()
            preferred_provider = self.provider if self.provider in available else "CPUExecutionProvider"

            try:
                tokenizer = AutoTokenizer.from_pretrained(self.model_name)
                model = ORTModelForFeatureExtraction.from_pretrained(
                    self.model_name,
                    export=True,
                    provider=preferred_provider,
                )
                extractor = pipeline("feature-extraction", model=model, tokenizer=tokenizer)
            except Exception as error:
                if preferred_provider != "CPUExecutionProvider":
                    tokenizer = AutoTokenizer.from_pretrained(self.model_name)
                    model = ORTModelForFeatureExtraction.from_pretrained(
                        self.model_name,
                        export=True,
                        provider="CPUExecutionProvider",
                    )
                    extractor = pipeline("feature-extraction", model=model, tokenizer=tokenizer)
                    preferred_provider = "CPUExecutionProvider"
                else:
                    self.load_error = str(error)
                    raise

            self.tokenizer = tokenizer
            self.extractor = extractor
            self.resolved_provider = preferred_provider
            self.load_error = None

    def embed(self, texts: list[str]) -> tuple[list[list[float]], int]:
        if not texts:
            return [], 0

        self.ensure_loaded()
        assert self.extractor is not None
        assert self.tokenizer is not None

        outputs = self.extractor(texts, truncation=True)
        sequences = self._normalize_outputs(outputs)
        embeddings = [self._pool_sequence(sequence) for sequence in sequences]
        tokenized = self.tokenizer(texts, truncation=True, add_special_tokens=True)
        usage = self._count_tokens(tokenized)
        return embeddings, usage

    @staticmethod
    def _normalize_outputs(outputs: Any) -> list[Any]:
        if outputs is None:
            return []
        if isinstance(outputs, list) and outputs:
            first = outputs[0]
            if isinstance(first, (int, float)):
                return [outputs]
            if isinstance(first, list) and first and isinstance(first[0], (int, float)):
                return [outputs]
            return list(outputs)
        return [outputs]

    @staticmethod
    def _pool_sequence(sequence: Any) -> list[float]:
        array = np.asarray(sequence, dtype=np.float32)
        if array.ndim == 0:
            return [float(array)]
        if array.ndim == 1:
            pooled = array
        elif array.ndim == 2:
            pooled = array.mean(axis=0)
        else:
            pooled = array.reshape(-1, array.shape[-1]).mean(axis=0)
        norm = float(np.linalg.norm(pooled))
        if norm > 0:
            pooled = pooled / norm
        return pooled.astype(np.float32).tolist()

    @staticmethod
    def _count_tokens(tokenized: Any) -> int:
        input_ids = tokenized.get("input_ids") if hasattr(tokenized, "get") else None
        if input_ids is None:
            return 0
        if input_ids and isinstance(input_ids[0], int):
            return len(input_ids)
        return sum(len(ids) for ids in input_ids)


runtime = EmbeddingRuntime(model_name=DEFAULT_MODEL, provider=DEFAULT_PROVIDER)
app = FastAPI(title="RAG Optimum Embedding Service", version="0.1.0")


@app.get("/health")
def health() -> dict[str, Any]:
    return {
        "ok": True,
        "model": runtime.model_name,
        "provider": runtime.resolved_provider or runtime.provider,
        "availableProviders": ort.get_available_providers(),
        "loaded": runtime.extractor is not None,
        "loadError": runtime.load_error,
    }


@app.post("/v1/embeddings", response_model=EmbeddingResponse)
def create_embeddings(request: EmbeddingRequest) -> EmbeddingResponse:
    if isinstance(request.input, str):
        texts = [request.input]
    else:
        texts = [str(item) for item in request.input]

    try:
        embeddings, token_count = runtime.embed(texts)
    except Exception as error:
        raise HTTPException(status_code=500, detail=str(error)) from error

    return EmbeddingResponse(
        model=request.model or runtime.model_name,
        data=[
            EmbeddingItem(index=index, embedding=embedding)
            for index, embedding in enumerate(embeddings)
        ],
        usage={
            "prompt_tokens": token_count,
            "total_tokens": token_count
        }
    )


@app.get("/")
def root() -> dict[str, Any]:
    return {
        "service": "rag-optimum-embeddings",
        "health": "/health",
        "embeddings": "/v1/embeddings"
    }
