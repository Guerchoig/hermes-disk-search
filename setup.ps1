# hermes-disk-search - Windows installer for the Rust build (W4).
#
# Installs the Rust stack shipped in bin\ (hds.exe, hds_mcp.exe, llm_host.exe):
# system dependencies, sidecar worker check, engine runtime, GGUF models and the
# logon tasks (file watcher, MCP server, llm-host resident).
#
# Usage (double-click is preferred):
#   setup.cmd
# or:
#   powershell -NoProfile -ExecutionPolicy Bypass -File setup.ps1
#
# NOTE: ASCII-only on purpose - Windows PowerShell 5.1 reads a BOM-less .ps1 as
# ANSI and mangles non-ASCII text (tools/parity/README.md section 3, item 14).
param(
    [switch]$SkipModels,
    [switch]$SkipEngine,
    [switch]$NoAutostart,
    [switch]$SkipIntegrations,
    [switch]$SmokeTest
)

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
Set-Location $root

function Step($m) { Write-Host ""; Write-Host "== $m ==" -ForegroundColor Cyan }
function Ok($m) { Write-Host "[ok] $m" -ForegroundColor Green }
function Warn($m) { Write-Host "[--] $m" -ForegroundColor Yellow }
function Note($m) { Write-Host "     $m" -ForegroundColor DarkGray }

# --- 1. Mark of the Web -----------------------------------------------------
# A release archive unpacked by Explorer marks every file as "downloaded from the
# internet"; the RemoteSigned policy then demands a signature for scripts. This
# run already uses -ExecutionPolicy Bypass, so clearing MotW is safe.
try {
    Get-ChildItem -Path $root -Recurse -File -ErrorAction SilentlyContinue |
        Unblock-File -ErrorAction SilentlyContinue
} catch { }
Ok "cleared the 'downloaded from internet' flag (MotW)"

# --- 2. Verify the Rust artifact -------------------------------------------
$binDir = Join-Path $root "bin"
$hdsExe = Join-Path $binDir "hds.exe"
$llmHostExe = Join-Path $binDir "llm_host.exe"
$mcpExe = Join-Path $binDir "hds_mcp.exe"

Step "Checking the Rust artifact"
if (-not (Test-Path $hdsExe)) {
    Write-Host "[!!] bin\hds.exe not found - this is not a Rust release archive." -ForegroundColor Red
    Warn "If you unpacked the SOURCE archive (Python version), use the Python setup instead."
    Warn "A Rust build is produced by: installers\build_rust_release.ps1 -Version <ver>"
    Read-Host "Press Enter to exit"
    exit 1
}
Ok "found $hdsExe"
if (-not (Test-Path $llmHostExe)) { Warn "bin\llm_host.exe not found - the GPU resident cannot start" }
if (-not (Test-Path $mcpExe)) { Warn "bin\hds_mcp.exe not found - stdio MCP integration will be limited" }

# From here on native tools are invoked: keep going on non-zero exit codes and
# check $LASTEXITCODE explicitly (native stderr in a pipeline under 'Stop' is a
# terminating NativeCommandError - tools/parity/README.md section 3, item 14).
$ErrorActionPreference = "Continue"

# --- 3. System dependencies via winget -------------------------------------
$haveWinget = [bool](Get-Command winget -ErrorAction SilentlyContinue)

Step "System dependencies"
# ffmpeg (video transcription) - installed automatically
if (Get-Command ffmpeg -ErrorAction SilentlyContinue) {
    Ok "ffmpeg found"
} elseif ($haveWinget) {
    Write-Host "[..] ffmpeg not found - installing via winget (needed for video transcription)..."
    winget install -e --id Gyan.FFmpeg --accept-source-agreements --accept-package-agreements
    Note "If ffmpeg is still missing from PATH, open a NEW PowerShell window."
} else {
    Warn "ffmpeg not found and winget is unavailable (video without transcription)."
    Note "Install manually: https://www.gyan.dev/ffmpeg/builds/"
}

# Tesseract OCR (text inside images/scans) - with user consent
if ((Get-Command tesseract -ErrorAction SilentlyContinue) -or (Test-Path "$env:ProgramFiles\Tesseract-OCR\tesseract.exe")) {
    Ok "Tesseract OCR found"
} else {
    $ans = Read-Host "[?] Install Tesseract OCR (text inside images/scans)? [y/N]"
    if ($ans -match '^[Yy]') {
        if ($haveWinget) {
            winget install -e --id UB-Mannheim.TesseractOCR --accept-source-agreements --accept-package-agreements
            Note "Russian OCR data: if missing, download 'rus' from https://github.com/tesseract-ocr/tessdata"
        } else {
            Warn "winget unavailable: https://github.com/UB-Mannheim/tesseract/wiki"
        }
    } else {
        Warn "skipped: images will be indexed without OCR (can be added later)."
    }
}

# Visual C++ runtime - the Rust build links against the dynamic MSVC CRT
$vcOk = (Test-Path "$env:SystemRoot\System32\vcruntime140.dll") -and (Test-Path "$env:SystemRoot\System32\vcruntime140_1.dll")
if ($vcOk) {
    Ok "Visual C++ runtime present"
} elseif ($haveWinget) {
    Write-Host "[..] Microsoft Visual C++ runtime not found - installing..."
    winget install -e --id Microsoft.VCRedist.2015+.x64 --accept-source-agreements --accept-package-agreements
} else {
    Warn "Visual C++ runtime not found; if hds.exe fails to start, install 'Microsoft Visual C++ Redistributable (x64)'."
}

# --- 4. sidecar worker (Python) --------------------------------------------
Step "Extraction worker (sidecar)"
function Find-WorkerPython {
    if ($env:HDS_EXTRACT_PYTHON -and (Test-Path $env:HDS_EXTRACT_PYTHON)) { return $env:HDS_EXTRACT_PYTHON }
    $bundled = Join-Path $root "sidecar\python"
    if (Test-Path $bundled) {
        $exe = Get-ChildItem -Path $bundled -Recurse -Filter python.exe -ErrorAction SilentlyContinue |
            Sort-Object { $_.FullName.Length } | Select-Object -First 1
        if ($exe) { return $exe.FullName }
    }
    $venv = Join-Path $root ".venv\Scripts\python.exe"
    if (Test-Path $venv) { return $venv }
    return $null
}
function Test-Worker {
    param([string]$Python)
    $worker = Join-Path $root "sidecar\hds_extract\worker.py"
    if (-not (Test-Path $worker)) { return $false }
    $req = '{"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":1}}'
    try { $out = $req | & $Python $worker --root $root 2>$null | Select-Object -First 1 } catch { return $false }
    if (-not $out) { return $false }
    try { $j = $out | ConvertFrom-Json } catch { return $false }
    return [bool]$j.result
}

$workerPython = Find-WorkerPython
if ($workerPython -and (Test-Worker -Python $workerPython)) {
    Ok "worker is ready ($workerPython)"
} elseif ($workerPython) {
    Warn "worker did not answer 'hello': $workerPython"
    Note "The bundled sidecar may be incomplete. Fallback: install Python 3.10+ and set HDS_EXTRACT_PYTHON."
} else {
    Warn "no Python for the extraction worker found."
    Note "Expected sidecar\python\python.exe (bundled) or .venv\Scripts\python.exe."
    Note "Fallback: install Python 3.10+ and set the HDS_EXTRACT_PYTHON environment variable."
}

# --- 5. config.yaml ---------------------------------------------------------
Step "config.yaml"
$cfg = Join-Path $root "config.yaml"
if (-not (Test-Path $cfg)) {
    $example = Join-Path $root "config.example.yaml"
    if (Test-Path $example) {
        Copy-Item $example $cfg -Force
        Ok "created config.yaml from config.example.yaml"
        Note "Set index.roots in the web UI (or edit config.yaml)."
    } else {
        Warn "config.example.yaml not found - create config.yaml manually."
    }
}

# A config.yaml copied from another computer may point at missing drives.
if (Test-Path $cfg) {
    $enc = New-Object System.Text.UTF8Encoding($false)
    $text = [System.IO.File]::ReadAllText($cfg, $enc)
    $missing = @()
    foreach ($m in [regex]::Matches($text, "[`"']?([A-Za-z]):\\\\")) {
        $letter = $m.Groups[1].Value
        $drive = $letter + ":\"
        if (-not (Test-Path ($letter + ":\")) -and ($missing -notcontains $drive)) { $missing += $drive }
    }
    if ($missing.Count -gt 0) {
        Warn "config.yaml points at missing drives: $($missing -join ', ')"
        Note "The file was probably copied from another computer."
        $ans = Read-Host "     Replace those paths with this profile ($env:USERPROFILE)? [Y/n]"
        if ($ans -notmatch '^[Nn]') {
            $up = $env:USERPROFILE
            $rxDb = [regex]::new("(?m)^db_path:.*$")
            $text = $rxDb.Replace($text, { param($mm) "db_path: '$up\hermes-disk-search-db\index.db'" }, 1)
            $rxList = [regex]::new('(?m)^(\s*-\s*)(["'']?)([A-Za-z]):\\+(.*?)\2\s*$')
            $text = $rxList.Replace($text, {
                param($mm)
                if ($missing -contains ($mm.Groups[3].Value + ":\")) {
                    $rest = $mm.Groups[4].Value -replace '\\+', '\'
                    if ($rest) { "$($mm.Groups[1].Value)'$up\$rest'" } else { "$($mm.Groups[1].Value)'$up'" }
                } else { $mm.Value }
            })
            [System.IO.File]::WriteAllText($cfg, $text, (New-Object System.Text.UTF8Encoding($false)))
            Ok "config.yaml updated (index.roots can be refined in the web UI)"
        }
    } else {
        Ok "all drives referenced by config.yaml exist"
    }
}

# --- 6. Engine runtime (LLM host + ASR) -------------------------------------
if (-not $SkipEngine) {
    Step "Engine runtime (llm-host + ASR)"
    try {
        # -PatchEngine: our overlay is applied right after the stock runtime is
        # installed (idempotent; the script also handles the missing-runtime case
        # by installing it first and then patching).
        & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "installers\fetch_engine_runtime.ps1") -PatchEngine
        if ($LASTEXITCODE -ne 0) { throw "fetch_engine_runtime.ps1 exited with code $LASTEXITCODE" }
    } catch {
        Warn "engine runtime not ready: $_"
        Note "Repeat later: powershell -File installers\fetch_engine_runtime.ps1"
    }
} else {
    Step "Engine runtime (skipped)"
}

# --- 7. Models --------------------------------------------------------------
if (-not $SkipModels) {
    Step "GGUF models (chat/embedding/rerank)"
    try {
        & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "installers\fetch_llm_models.ps1") -Models chat,embedding,rerank
        if ($LASTEXITCODE -ne 0) { throw "fetch_llm_models.ps1 exited with code $LASTEXITCODE" }
    } catch {
        Warn "GGUF models not fully ready: $_"
        Note "Repeat later: powershell -File installers\fetch_llm_models.ps1"
    }

    Step "Whisper model (ASR, optional)"
    $ans = Read-Host "[?] Download the whisper model now (about 1.5 GB)? [y/N]"
    if ($ans -match '^[Yy]') {
        try { & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "installers\fetch_whisper_model.ps1") }
        catch { Warn "whisper model not downloaded: $_" }
    } else {
        Warn "skipped: the model is downloaded on first transcription."
    }

    Step "CLIP ONNX models (image search, optional)"
    $ans = Read-Host "[?] Download the CLIP models now (about 850 MB)? [y/N]"
    if ($ans -match '^[Yy]') {
        try { & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "installers\fetch_clip_models.ps1") }
        catch { Warn "CLIP models not downloaded: $_" }
    } else {
        Warn "skipped: CLIP image search stays disabled until the models are present."
    }

    Step "Diarization model (auto-transcription, optional)"
    $ans = Read-Host "[?] Download the Sortformer diarization model now (about 449 MB)? [y/N]"
    if ($ans -match '^[Yy]') {
        try { & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "installers\fetch_diarization_model.ps1") }
        catch { Warn "diarization model not downloaded: $_" }
    } else {
        Warn "skipped: auto-transcription needs it (installers\fetch_diarization_model.ps1)."
    }
} else {
    Step "Models (skipped)"
}

# --- 8. Logon tasks ---------------------------------------------------------
if (-not $NoAutostart) {
    Step "Logon tasks"
    $ans = Read-Host "[?] Set up autostart at logon (watcher + MCP + llm-host)? [Y/n]"
    if ($ans -notmatch '^[Nn]') {
        try {
            & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "installers\install_llm_host_task.ps1") -Start
            if ($LASTEXITCODE -ne 0) { Warn "llm-host task: exit code $LASTEXITCODE" }
        } catch { Warn "llm-host task: $_" }
        try {
            & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "install_autostart.ps1")
        } catch { Warn "watcher/MCP tasks: $_" }
        $ansUi = Read-Host "[?] Start the web UI automatically at logon (port 8765)? [y/N]"
        if ($ansUi -match '^[Yy]') {
            try {
                & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "installers\install_ui_task.ps1")
            } catch { Warn "UI task: $_" }
        } else {
            Warn "skipped: the desktop shortcut still starts the UI on demand."
        }
    } else {
        Warn "skipped: start the resident manually with bin\llm_host.exe run"
    }
} else {
    Step "Logon tasks (skipped)"
}

# --- 9. Hermes / Cline integration -----------------------------------------
if (-not $SkipIntegrations) {
    Step "Hermes Desktop / Cline integration"
    try { & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "install_hermes.ps1") } catch { Warn "Hermes: $_" }
    try { & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "install_cline.ps1") } catch { Warn "Cline: $_" }
} else {
    Step "Integrations (skipped)"
}

# --- 10. Desktop shortcut ---------------------------------------------------
Step "Desktop shortcut"
try { & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "shortcuts\windows\create_shortcut.ps1") }
catch { Warn "shortcut: $_" }

# --- 11. Diagnostics --------------------------------------------------------
Step "Diagnostics"
& $hdsExe check

# --- 12. Optional smoke test (mini index on test_data) ----------------------
if ($SmokeTest) {
    Step "Smoke test (mini index)"
    $data = Join-Path $root "test_data"
    if (-not (Test-Path $data)) {
        Warn "test_data\ not found - smoke test skipped"
    } else {
        $stamp = [Guid]::NewGuid().ToString("N")
        $tmpCfg = Join-Path $env:TEMP ("hds-smoke-$stamp.yaml")
        $tmpDb = Join-Path $env:TEMP ("hds-smoke-$stamp.db")
        $yaml = "index:`r`n  roots:`r`n    - '$data'`r`n  ocr: false`r`n  transcribe: false`r`ndb_path: '$tmpDb'`r`n"
        [IO.File]::WriteAllText($tmpCfg, $yaml, (New-Object System.Text.UTF8Encoding($false)))
        $env:HDS_CONFIG = $tmpCfg
        & $hdsExe index --quiet
        if ($LASTEXITCODE -ne 0) { Warn "index exited with code $LASTEXITCODE (a running llm-host is required)" }
        & $hdsExe status
        Remove-Item Env:\HDS_CONFIG -ErrorAction SilentlyContinue
        Remove-Item $tmpCfg, $tmpDb -Force -ErrorAction SilentlyContinue
        Note "Smoke test uses a temporary database; the production index is not touched."
    }
}

Write-Host ""
Write-Host "== Done. Next step: the web UI ==" -ForegroundColor Cyan
Write-Host "   Use the 'Hermes Disk Search' desktop shortcut: index roots, Start indexing," -ForegroundColor Cyan
Write-Host "   watcher, model switching - all in one place." -ForegroundColor Cyan
Read-Host "Press Enter to exit"
