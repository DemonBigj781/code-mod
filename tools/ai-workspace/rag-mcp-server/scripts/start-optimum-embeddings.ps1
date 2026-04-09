$ErrorActionPreference = 'Stop'

$serviceRoot = Resolve-Path (Join-Path $PSScriptRoot '..\python-optimum-embeddings')
$pidPath = Join-Path $serviceRoot 'optimum-embeddings.pid'
$stdoutPath = Join-Path $serviceRoot 'optimum-embeddings.stdout.log'
$stderrPath = Join-Path $serviceRoot 'optimum-embeddings.stderr.log'

if (Test-Path $pidPath) {
  $existingPid = (Get-Content $pidPath -ErrorAction SilentlyContinue | Select-Object -First 1)
  if ($existingPid) {
    $existingId = [int]$existingPid
    $existing = Get-Process -Id $existingId -ErrorAction SilentlyContinue
    if ($existing) {
      Write-Output "Optimum embedding service already running (PID $existingPid)."
      exit 0
    }
  }
}

$uv = Get-Command uv -ErrorAction SilentlyContinue
if (-not $uv) {
  throw 'uv was not found on PATH. Install it with `pip install uv` or `pipx install uv`.'
}

$arguments = @(
  'run',
  'uvicorn',
  'optimum_embedding_service.app:app',
  '--host',
  '127.0.0.1',
  '--port',
  '8123'
)

$process = Start-Process `
  -FilePath $uv.Source `
  -ArgumentList $arguments `
  -WorkingDirectory $serviceRoot `
  -WindowStyle Hidden `
  -PassThru `
  -RedirectStandardOutput $stdoutPath `
  -RedirectStandardError $stderrPath

Set-Content -Path $pidPath -Value $process.Id -Encoding ASCII
Write-Output "Started Optimum embedding service on http://127.0.0.1:8123 (PID $($process.Id))."
