<#
.SYNOPSIS
  S1: прогон сценария изоляции SDK с мониторингом процессов/сети и снимками профиля.

.DESCRIPTION
  1) снимок %USERPROFILE%\.cline и каталога расширения Cline (до);
  2) запуск node --import fs_trace.mjs s1_isolation.mjs с HDS_TRACE_LOG;
  3) пока процесс жив — опрос дочерних процессов (Win32_Process) и TCP-соединений;
  4) снимок после и сравнение: добавленные / удалённые / изменённые файлы.
  Пишет s1\ps-monitor.json, печатает сводку.
  Только ASCII в сообщениях: PowerShell 5.1 читает UTF-8 без BOM как ANSI.
#>
[CmdletBinding()]
param(
    [string] $OutRoot,
    [string] $SdkDir
)

$ErrorActionPreference = 'Stop'
# $PSScriptRoot в блоке param ещё пуст — вычисляем после его входа
if (-not $PSScriptRoot) { $PSScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $SdkDir) { $SdkDir = $PSScriptRoot }
if (-not $OutRoot) {
    # скрипт лежит либо в исходниках (tools\parity\agent_a0_sdk), либо в рабочей
    # копии (out\agent_a0\sdk с node_modules) — различаем по node_modules
    if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'node_modules')) {
        $OutRoot = Split-Path -Parent $PSScriptRoot
    } else {
        $OutRoot = Join-Path $PSScriptRoot '..\..\out\agent_a0'
    }
}
$outRootAbs = (Resolve-Path -LiteralPath $OutRoot).Path
$s1 = Join-Path $outRootAbs 's1'
New-Item -ItemType Directory -Force -Path $s1 | Out-Null

$snapshotScript = Join-Path $PSScriptRoot 'snapshot_profile.ps1'
$before = Join-Path $s1 'profile-before.json'
$after = Join-Path $s1 'profile-after.json'

Write-Host '[s1] snapshot before'
& powershell -NoProfile -ExecutionPolicy Bypass -File $snapshotScript -OutFile $before -Quiet

$traceLog = Join-Path $s1 'fs-trace.jsonl'
Remove-Item -LiteralPath $traceLog -ErrorAction SilentlyContinue
$env:HDS_TRACE_LOG = $traceLog

$stdout = Join-Path $s1 'scenario.out.log'
$stderr = Join-Path $s1 'scenario.err.log'

Write-Host '[s1] run scenario'
$proc = Start-Process -FilePath (Get-Command node).Source `
    -ArgumentList '--import', './fs_trace.mjs', 's1_isolation.mjs' `
    -WorkingDirectory $SdkDir `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr `
    -PassThru -WindowStyle Hidden

$seenChildren = @{}
$connections = New-Object System.Collections.Generic.List[object]
$samples = 0
while (-not $proc.HasExited) {
    $samples++
    $children = @()
    try {
        $children = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" -ErrorAction SilentlyContinue
    } catch { }
    foreach ($child in $children) {
        if (-not $seenChildren.ContainsKey($child.ProcessId)) {
            $seenChildren[$child.ProcessId] = [ordered]@{
                pid         = $child.ProcessId
                name        = $child.Name
                commandLine = $child.CommandLine
            }
        }
    }
    $pids = @($proc.Id) + @($seenChildren.Keys)
    foreach ($childPid in $pids) {
        $conns = @()
        try {
            $conns = Get-NetTCPConnection -OwningProcess $childPid -ErrorAction SilentlyContinue
        } catch { $conns = @() }
        foreach ($conn in $conns) {
            if ($null -eq $conn) { continue }
            $connections.Add([ordered]@{
                pid    = $childPid
                local  = "$($conn.LocalAddress):$($conn.LocalPort)"
                remote = "$($conn.RemoteAddress):$($conn.RemotePort)"
                state  = $conn.State
            })
        }
    }
    Start-Sleep -Milliseconds 250
}
$proc.WaitForExit()
Remove-Item Env:HDS_TRACE_LOG -ErrorAction SilentlyContinue

Write-Host '[s1] snapshot after'
& powershell -NoProfile -ExecutionPolicy Bypass -File $snapshotScript -OutFile $after -Quiet

# --- сравнение снимков -----------------------------------------------------
$beforeJson = Get-Content -LiteralPath $before -Raw | ConvertFrom-Json
$afterJson = Get-Content -LiteralPath $after -Raw | ConvertFrom-Json

function Compare-Tree {
    param($Before, $After, [string] $Tag)
    $mapBefore = @{}
    foreach ($f in $Before.files) { $mapBefore[$f.rel] = $f }
    $mapAfter = @{}
    foreach ($f in $After.files) { $mapAfter[$f.rel] = $f }

    $added = @()
    $removed = @()
    $changed = @()
    foreach ($rel in $mapAfter.Keys) {
        if (-not $mapBefore.ContainsKey($rel)) { $added += $rel; continue }
        $b = $mapBefore[$rel]; $a = $mapAfter[$rel]
        if ($null -ne $b.sha256 -and $null -ne $a.sha256) {
            if ($b.sha256 -ne $a.sha256) { $changed += $rel }
        } elseif ($b.size -ne $a.size -or $b.mtime -ne $a.mtime) {
            $changed += "$rel (no-hash)"
        }
    }
    foreach ($rel in $mapBefore.Keys) {
        if (-not $mapAfter.ContainsKey($rel)) { $removed += $rel }
    }
    return [ordered]@{
        tag     = $Tag
        added   = @($added | Sort-Object)
        removed = @($removed | Sort-Object)
        changed = @($changed | Sort-Object)
    }
}

$diff = [ordered]@{
    createdAtUtc = (Get-Date).ToUniversalTime().ToString('o')
    clineHome    = Compare-Tree -Before $beforeJson.clineHome -After $afterJson.clineHome -Tag 'cline_home'
    extension    = Compare-Tree -Before $beforeJson.extension -After $afterJson.extension -Tag 'cline_vscode_extension'
    children     = @($seenChildren.Values)
    tcpSamples   = $samples
    tcpRemote    = @($connections | Where-Object { $_.remote -notmatch '^(127\.0\.0\.1|0\.0\.0\.0|\[::1\]|::):' -and $_.remote -notmatch ':0$' } | Sort-Object remote -Unique)
    tcpAll       = @($connections | Sort-Object remote -Unique)
}
$diff | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $s1 'ps-monitor.json') -Encoding UTF8

# --- сводка ---------------------------------------------------------------
$resultFile = Join-Path $s1 'result.json'
$scenario = $null
if (Test-Path -LiteralPath $resultFile) { $scenario = Get-Content -LiteralPath $resultFile -Raw | ConvertFrom-Json }

Write-Host ''
Write-Host '=== S1 summary ==='
Write-Host ("scenario.ok            : {0}" -f $(if ($scenario) { $scenario.ok } else { 'no result.json' }))
if ($scenario) {
    Write-Host ("fs events              : {0} (sandbox {1}, outside {2})" -f $scenario.traceSummary.fsEvents, $scenario.traceSummary.sandboxPaths, @($scenario.traceSummary.outsidePaths).Count)
    Write-Host ("real .cline touches    : {0}" -f @($scenario.violations.realClineHome).Count)
    Write-Host ("real AppData touches   : {0}" -f @($scenario.violations.realAppData).Count)
    Write-Host ("external net           : {0}" -f @($scenario.violations.externalNet).Count)
    Write-Host ("mock requests          : {0}" -f $scenario.mockRequestCount)
}
Write-Host ("children spawned       : {0}" -f @($diff.children).Count)
foreach ($child in @($diff.children)) { Write-Host ("   - {0} {1}" -f $child.name, $child.pid) }
Write-Host ("tcp non-loopback       : {0}" -f @($diff.tcpRemote).Count)
foreach ($conn in @($diff.tcpRemote)) { Write-Host ("   - {0}" -f $conn.remote) }
Write-Host ("cline_home added       : {0}" -f @($diff.clineHome.added).Count)
foreach ($rel in @($diff.clineHome.added)) { Write-Host ("   + {0}" -f $rel) }
Write-Host ("cline_home changed     : {0}" -f @($diff.clineHome.changed).Count)
foreach ($rel in @($diff.clineHome.changed)) { Write-Host ("   ~ {0}" -f $rel) }
Write-Host ("cline_home removed     : {0}" -f @($diff.clineHome.removed).Count)
Write-Host ("extension added        : {0}" -f @($diff.extension.added).Count)
Write-Host ("extension changed      : {0}" -f @($diff.extension.changed).Count)
foreach ($rel in @($diff.extension.changed)) { Write-Host ("   ~ {0}" -f $rel) }
Write-Host ''
Write-Host ("stdout: {0}" -f $stdout)
Write-Host ("stderr: {0}" -f $stderr)

