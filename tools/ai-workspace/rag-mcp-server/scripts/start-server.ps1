$ErrorActionPreference = 'Stop'

$projectRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $projectRoot

$optimumUrl = $env:RAG_OPTIMUM_EMBEDDING_URL
if (-not $optimumUrl) {
  $optimumUrl = 'http://127.0.0.1:8123'
}

if (-not $env:RAG_EMBEDDING_PROVIDER) {
  try {
    Invoke-WebRequest -UseBasicParsing "$optimumUrl/health" -TimeoutSec 2 | Out-Null
    $env:RAG_EMBEDDING_PROVIDER = 'optimum'
    $env:RAG_OPTIMUM_EMBEDDING_URL = $optimumUrl
    if (-not $env:RAG_OPTIMUM_EMBEDDING_MODEL) {
      $env:RAG_OPTIMUM_EMBEDDING_MODEL = 'sentence-transformers/paraphrase-MiniLM-L3-v2'
    }
    Write-Output "Using Optimum embeddings at $optimumUrl."
  } catch {
    Write-Output 'Optimum embeddings service not detected; falling back to the configured or default backend.'
  }
}

node dist/server.js
