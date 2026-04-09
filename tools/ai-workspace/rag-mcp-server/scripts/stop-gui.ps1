$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$pidFile = Join-Path $root 'data\gui-server.pid'

if (Test-Path $pidFile) {
  $processId = [int](Get-Content $pidFile -Raw).Trim()
  if ($processId -gt 0) {
    Stop-Process -Id $processId -Force -ErrorAction SilentlyContinue
    Write-Output "Stopped RAG GUI backend process $processId."
  }
  Remove-Item $pidFile -Force -ErrorAction SilentlyContinue
} else {
  Write-Output "No GUI backend PID file found."
}
