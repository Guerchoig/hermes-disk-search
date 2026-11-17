<#
.SYNOPSIS
  Снимок «профильных» каталогов Cline владельца для спайка A0 (S1) и автотестов TR-3.10.

.DESCRIPTION
  Обходит рекурсивно:
    * %USERPROFILE%\.cline
    * каталог расширения Cline в VS Code — тот подкаталог %APPDATA%\Code\User\globalStorage,
      в котором есть settings\cline_mcp_settings.json
  и пишет JSON: относительный путь, размер, LastWriteTime (UTC, ISO), sha256.
  Каталоги, которые не читаются, помечаются в errors и не роняют снимок.

.PARAMETER OutFile
  Путь к JSON-файлу снимка (родительский каталог создаётся автоматически).

.PARAMETER Quiet
  Не печатать сводку в консоль.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\agent_a0_sdk\snapshot_profile.ps1 `
      -OutFile tools\parity\out\agent_a0\snapshots\before.json
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $OutFile,
    [switch] $Quiet
)

$ErrorActionPreference = 'Continue'

function Get-ClineExtensionDir {
    $globalStorage = Join-Path $env:APPDATA 'Code\User\globalStorage'
    if (-not (Test-Path -LiteralPath $globalStorage)) { return $null }
    Get-ChildItem -LiteralPath $globalStorage -Directory -ErrorAction SilentlyContinue |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'settings\cline_mcp_settings.json') } |
        Select-Object -First 1 -ExpandProperty FullName
}

function Get-TreeSnapshot {
    param([string] $Root, [string] $Tag)

    $result = [ordered]@{
        tag     = $Tag
        root    = $Root
        exists  = $false
        files   = @()
        errors  = @()
        totalBytes = 0
    }
    if (-not $Root -or -not (Test-Path -LiteralPath $Root)) { return $result }
    $result.exists = $true

    $files = @()
    try {
        $files = Get-ChildItem -LiteralPath $Root -Recurse -File -Force -ErrorAction SilentlyContinue
    } catch {
        $result.errors += "enumerate: $($_.Exception.Message)"
    }

    $items = New-Object System.Collections.Generic.List[object]
    foreach ($f in $files) {
        $rel = $f.FullName.Substring($Root.Length).TrimStart('\')
        try {
            $hash = (Get-FileHash -LiteralPath $f.FullName -Algorithm SHA256 -ErrorAction Stop).Hash
            $items.Add([ordered]@{
                rel    = $rel
                size   = $f.Length
                mtime  = $f.LastWriteTimeUtc.ToString('o')
                sha256 = $hash
            })
            $result.totalBytes += $f.Length
        } catch {
            # locked file (e.g. the owner's Cline keeps data\db\sessions.db open):
            # keep the entry so before/after can at least compare size and mtime
            $result.errors += "hash: $($f.FullName): $($_.Exception.Message)"
            $items.Add([ordered]@{
                rel    = $rel
                size   = $f.Length
                mtime  = $f.LastWriteTimeUtc.ToString('o')
                sha256 = $null
            })
        }
    }
    $result.files = @($items | Sort-Object { $_.rel })
    return $result
}

$clineHome = Join-Path $env:USERPROFILE '.cline'
$extDir = Get-ClineExtensionDir

$snapshot = [ordered]@{
    createdAtUtc = (Get-Date).ToUniversalTime().ToString('o')
    user         = $env:USERNAME
    clineHome    = (Get-TreeSnapshot -Root $clineHome -Tag 'cline_home')
    extension    = (Get-TreeSnapshot -Root $extDir -Tag 'cline_vscode_extension')
}

$outDir = Split-Path -Parent $OutFile
if ($outDir -and -not (Test-Path -LiteralPath $outDir)) {
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null
}
$snapshot | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $OutFile -Encoding UTF8

# NOTE: messages are ASCII-only on purpose: PowerShell 5.1 reads BOM-less UTF-8 as ANSI.
if (-not $Quiet) {
    foreach ($t in @($snapshot.clineHome, $snapshot.extension)) {
        $label = if ($t.exists) { "files=$($t.files.Count) bytes=$($t.totalBytes)" } else { 'directory not found' }
        Write-Host ("{0}: {1}" -f $t.tag, $label)
        if ($t.errors.Count -gt 0) { Write-Host ("  errors: {0}" -f $t.errors.Count) }
    }
    Write-Host "snapshot: $OutFile"
}
