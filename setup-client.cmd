@echo off
rem setup-client.cmd - Windows wrapper for the thin client installer (M9 / Э9 / W7).
rem Passes all arguments to setup-client.ps1 (see it for -ServerUrl / -Token / -McpPath).
setlocal
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup-client.ps1" %*
exit /b %ERRORLEVEL%
