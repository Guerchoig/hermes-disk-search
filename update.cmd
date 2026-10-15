@echo off
rem ============================================================
rem  hermes-disk-search - SERVER UPDATE wrapper (installers\update.ps1)
rem
rem  Run THIS file (from an elevated prompt), not update.ps1 directly:
rem   1) it clears the Mark-of-the-Web flag Windows puts on files
rem      extracted from a downloaded ZIP (otherwise the RemoteSigned
rem      policy demands a digital signature);
rem   2) it finds the INSTALL ROOT by itself: "bin\hds.exe service root"
rem      reads the binPath of the installed SCM service, so the path
rem      never has to be typed;
rem   3) it calls installers\update.ps1 -Profile server with
rem      -From <this folder> and -Version from version.txt.
rem
rem  Usage (Administrator):  update.cmd [update.ps1 options]
rem    update.cmd -WhatIf                preview, no changes
rem    update.cmd -Force                 reinstall the same version
rem    update.cmd -RollbackTo 0.19.0     roll back to a kept version
rem    update.cmd -Root D:\hds           explicit root (services absent)
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
set "VER="
if exist "%HERE%\version.txt" set /p VER=<"%HERE%\version.txt"
if not defined VER (
  echo [!!] version.txt not found in "%HERE%" - this is not a release archive.
  goto :error
)

echo Clearing "downloaded from the internet" flags (Unblock-File)...
powershell -NoProfile -Command "Get-ChildItem -LiteralPath '%HERE%' -Recurse -File | Unblock-File" >nul 2>&1

set "ROOTARG="
set "ROOTHINT="
echo %* | findstr /i /c:"-Root" >nul 2>&1
if not errorlevel 1 goto :root_ready

rem NB: values below are read on their OWN lines, not inside a ( ) block: inside a
rem block cmd would expand %ROOT% when parsing the whole block, i.e. still empty.
set "ROOT="
for /f "delims=" %%R in ('""%HERE%\bin\hds.exe" service root" 2^>nul') do set "ROOT=%%R"
if not defined ROOT (
  echo [!!] Could not detect the install root: no role service is registered.
  echo      Pass it explicitly, e.g.  update.cmd -Root "C:\path\to\install"
  goto :error
)
rem NB: no outer quotes here - in the `set "NAME=VALUE"` form cmd would strip the
rem closing quote and the following arguments would glue into one token.
set ROOTARG=-Root "%ROOT%"
set "ROOTHINT=%ROOT%"

:root_ready
if defined ROOTHINT echo Install root: %ROOTHINT%
echo New version:  %VER%
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%HERE%\installers\update.ps1" %ROOTARG% -Profile server -From "%HERE%" -Version "%VER%" %*
if errorlevel 1 goto :error
echo.
echo [ok] server update finished.
endlocal
goto :eof

:error
echo.
echo [ERROR] Server update failed. Common causes:
echo  1. Not elevated: stopping/starting SCM services needs Administrator.
echo  2. Execution policy set by Group Policy (AllSigned): the message says
echo     "cannot be overridden" - ask IT or use the manual steps in README.
echo  3. The services were installed from another folder: pass -Root.
echo.
pause
exit /b 1
