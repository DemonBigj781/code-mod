$ErrorActionPreference = 'Stop'

$serviceRoot = Resolve-Path (Join-Path $PSScriptRoot '..\python-optimum-embeddings')
$pidPath = Join-Path $serviceRoot 'optimum-embeddings.pid'

if (-not (Test-Path $pidPath)) {
  Write-Output 'Optimum embedding service is not running.'
  exit 0
}

$processId = (Get-Content $pidPath -ErrorAction SilentlyContinue | Select-Object -First 1)
if (-not $processId) {
  Remove-Item $pidPath -Force -ErrorAction SilentlyContinue
  Write-Output 'Optimum embedding service pid file was empty.'
  exit 0
}

$process = Get-Process -Id [int]$processId -ErrorAction SilentlyContinue
if ($process) {
  Stop-Process -Id $process.Id -Force
  Write-Output "Stopped Optimum embedding service (PID $processId)."
} else {
  Write-Output "Optimum embedding service process $processId was not running."
}

Remove-Item $pidPath -Force -ErrorAction SilentlyContinue
