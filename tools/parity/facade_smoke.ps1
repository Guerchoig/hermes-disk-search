# tools/parity/facade_smoke.ps1 -- live check of the llm-host facade (A5).
#
# Starts `llm_host_facade` on ALTERNATIVE ports (8020-8022 by default) with the chat
# model on CPU (`--ngl 0`), so it does not take VRAM from the Python-version roles and
# does not steal ports 8010-8012 they listen on.
#
# Checked: /health, /props (Python version uses it to recognise "its" instance),
# /v1/models, /v1/chat/completions (thinking off and chat-think), /v1/embeddings.
#
# NOTE: ASCII-only on purpose - Windows PowerShell 5.1 reads .ps1 without BOM as ANSI
# and mangles non-ASCII comments/strings (learned the hard way).
#
# Example:
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\facade_smoke.ps1 -PortBase 8020
param(
    [int]$PortBase = 8020,
    [int]$WaitSec = 240,
    [int]$HoldSec = 60,
    [int]$MaxTokens = 24,
    [string]$Json = "tools\parity\out\w2_facade.json"
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Push-Location $root
$log = Join-Path $env:TEMP "facade_smoke.log"
$errPath = Join-Path $env:TEMP "facade_smoke.err.log"
$embPort = $PortBase + 1

Write-Host "== facade: ports $PortBase-$($PortBase + 2), chat on CPU, log: $log"
$proc = Start-Process -FilePath "cargo" `
    -ArgumentList @("run", "-q", "-p", "hds-llama", "--release", "--bin", "llm_host_facade", "--",
                   "--port-base", $PortBase, "--ngl", "0", "--hold", $HoldSec, "--json", $Json) `
    -RedirectStandardOutput $log -RedirectStandardError $errPath -PassThru -NoNewWindow

function Wait-Health([string]$url, [int]$sec) {
    $deadline = (Get-Date).AddSeconds($sec)
    while ((Get-Date) -lt $deadline) {
        try {
            $r = Invoke-RestMethod -Uri $url -TimeoutSec 3
            if ($r.status -eq "ok") { return $true }
        } catch { Start-Sleep -Seconds 2 }
    }
    return $false
}

try {
    Write-Host "== waiting for /health (up to $WaitSec s; CPU load of the model takes 30-60 s)"
    if (-not (Wait-Health "http://127.0.0.1:$PortBase/health" $WaitSec)) {
        Write-Host "!! facade did not come up; tail of the log:"
        Get-Content $log -Tail 25
        Get-Content $errPath -Tail 25
        exit 1
    }
    Write-Host "== /health ok"

    $props = Invoke-RestMethod -Uri "http://127.0.0.1:$PortBase/props" -TimeoutSec 10
    Write-Host ("== /props: total_slots={0} n_ctx={1} model_path={2}" -f $props.total_slots, $props.n_ctx, $props.model_path)

    $models = Invoke-RestMethod -Uri "http://127.0.0.1:$PortBase/v1/models" -TimeoutSec 10
    $ids = ($models.data | ForEach-Object { $_.id }) -join ", "
    Write-Host "== /v1/models: $ids"

    $chatBody = @{
        model       = "chat"
        max_tokens  = $MaxTokens
        temperature = 0.2
        messages    = @(
            @{ role = "system"; content = "Answer briefly." },
            @{ role = "user"; content = "What is 2+2? Answer with a single number." }
        )
    } | ConvertTo-Json -Depth 6
    $t0 = Get-Date
    $chat = Invoke-RestMethod -Uri "http://127.0.0.1:$PortBase/v1/chat/completions" -Method Post `
        -Body $chatBody -ContentType "application/json" -TimeoutSec 300
    $ms = [int]((Get-Date) - $t0).TotalMilliseconds
    $answer = $chat.choices[0].message.content.Trim()
    Write-Host "== chat (thinking off): $ms ms, answer: $answer"
    Write-Host ("   usage: prompt={0} completion={1}" -f $chat.usage.prompt_tokens, $chat.usage.completion_tokens)

    $thinkBody = @{
        model       = "chat-think"
        max_tokens  = $MaxTokens
        temperature = 0.2
        messages    = @(@{ role = "user"; content = "What is 2+2?" })
    } | ConvertTo-Json -Depth 6
    $think = Invoke-RestMethod -Uri "http://127.0.0.1:$PortBase/v1/chat/completions" -Method Post `
        -Body $thinkBody -ContentType "application/json" -TimeoutSec 300
    $rc = $think.choices[0].message.reasoning_content
    $rcInfo = "NO"
    if ($rc) { $rcInfo = "yes (" + $rc.Length + " chars)" }
    Write-Host "== chat (chat-think): reasoning_content $rcInfo"

    $embBody = @{ input = @("facade check", "second text"); model = "embedding" } | ConvertTo-Json -Depth 4
    $emb = Invoke-RestMethod -Uri "http://127.0.0.1:$embPort/v1/embeddings" -Method Post `
        -Body $embBody -ContentType "application/json" -TimeoutSec 300
    $dim = $emb.data[0].embedding.Count
    Write-Host ("== embeddings: {0} vectors, dim {1}" -f $emb.data.Count, $dim)
} finally {
    Write-Host "== waiting for the process to exit (--hold $HoldSec)"
    if (-not $proc.WaitForExit(($HoldSec + 240) * 1000)) { $proc.Kill() }
    Write-Host "== facade log tail:"
    Get-Content $log -Tail 12
    if (Test-Path $Json) { Write-Host "== report: $Json" }
    Pop-Location
}
