@echo off
setlocal
set CAMOUFOX_PORT=8765
set CAMOUFOX_WS_PATH=openrouter-proxy
python "%~dp0scripts\camoufox-headless-server.py"
