@echo off
cd /d "%~dp0"
set "PROXY_PORT=%PROXY_PORT%"
if not defined PROXY_PORT set "PROXY_PORT=2000"
set "UI_PORT=%UI_PORT%"
if not defined UI_PORT set "UI_PORT=3000"
set "EXIT_CODE=0"

if not defined OPENROUTER_API_KEY (
  echo ERROR: OPENROUTER_API_KEY must be set before running this script.
  echo You can set it system-wide or run:
  echo ^  set OPENROUTER_API_KEY=sk-...
  set "EXIT_CODE=1"
  goto :done
)

for /f "tokens=5" %%a in ('netstat -ano ^| findstr /R /C:":%PROXY_PORT% .*LISTENING"') do set "PROXY_PID=%%a"
if defined PROXY_PID (
  echo OpenRouter proxy already appears to be running on port %PROXY_PORT% ^(PID %PROXY_PID%^).
  echo UI should be available at http://localhost:%UI_PORT%
  goto :done
)

echo Starting OpenRouter proxy on port %PROXY_PORT% (UI on %UI_PORT%)...
node src/server.js
set "EXIT_CODE=%ERRORLEVEL%"

:done
echo.
pause
exit /b %EXIT_CODE%
