$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$distGui = Join-Path $root 'dist\gui-server.js'
$pidFile = Join-Path $root 'data\gui-server.pid'

if (!(Test-Path $distGui)) {
  throw "Missing build output: $distGui. Run `npm run build` first."
}

try {
  $listener = Get-NetTCPConnection -LocalPort 8787 -State Listen -ErrorAction Stop | Select-Object -First 1
  if ($listener) {
    Write-Output "RAG GUI backend is already listening on port 8787."
    exit 0
  }
} catch {
  # No listener, continue.
}

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
    # Leave the existing/local backend in place.
  }
}

$node = (Get-Command node).Source
$process = Start-Process -FilePath $node -ArgumentList @($distGui) -WorkingDirectory $root -WindowStyle Hidden -PassThru

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $pidFile) | Out-Null
Set-Content -Path $pidFile -Value $process.Id -Encoding ASCII

Start-Sleep -Seconds 2
Write-Output "RAG GUI backend started. PID $($process.Id)."
