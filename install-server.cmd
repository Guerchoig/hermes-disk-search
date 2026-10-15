@echo off
rem ============================================================
rem  hermes-disk-search - SERVER INSTALL wrapper (installers\install_server.ps1)
rem
rem  Fresh install of the SERVER profile: SCM services (watch, mcp-http, ui,
rem  llm-host[, transcribe]) that start at boot WITHOUT logon. For a plain
rem  DESKTOP install use setup.cmd; for UPDATES use update.cmd.
rem
rem  What it does: clears the Mark-of-the-Web flags of this tree, then runs
rem  installers\install_server.ps1 with -ExecutionPolicy Bypass. Defaults are
rem  the script's own: state dir %ProgramData%\hermes-disk-search, install
rem  root = this folder, roles watch,mcp-http,ui,llm-host,transcribe.
rem
rem  Usage (Administrator):  install-server.cmd [install_server.ps1 options]
rem    install-server.cmd -WhatIf                       preview (no admin needed)
rem    install-server.cmd -Roles "watch,mcp-http,ui,llm-host" -Roots "D:\"
rem    install-server.cmd -EnableAccess -AdminLogin admin -AdminPassword secret
rem
rem  NOTE: keep this file ASCII-only: cmd.exe parses batch files in
rem  the legacy OEM codepage, and UTF-8 Cyrillic text breaks it.
rem ============================================================
setlocal EnableExtensions
cd /d "%~dp0"
set "HERE=%~dp0"
rem drop the trailing backslash: 'C:\dir\' inside "..." would escape the quote
if "%HERE:~-1%"=="\" set "HERE=%HERE:~0,-1%"

if not exist "%HERE%\bin\hds.exe" (
  echo [!!] "%HERE%\bin\hds.exe" not found - unpack the release archive first.
  goto :error
)

echo Clearing "downloaded from the internet" flags (Unblock-File)...
powershell -NoProfile -Command "Get-ChildItem -LiteralPath '%HERE%' -Recurse -File | Unblock-File" >nul 2>&1

echo Installing the SERVER profile from "%HERE%" (Administrator required)...
echo.
powershell -NoProfile -ExecutionPolicy Bypass -File "%HERE%\installers\install_server.ps1" %*
if errorlevel 1 goto :error
goto :eof

:error
echo.
echo [ERROR] Server install failed. Common causes:
echo  1. Not elevated: SCM services need Administrator (install_server.ps1
echo     prints "[!!] Run as Administrator").
echo  2. Execution policy set by Group Policy (AllSigned): ask IT, or run
echo     install_server.ps1 manually from an elevated PowerShell.
echo.
pause
exit /b 1
