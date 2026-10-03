# fetch_llm_models.ps1 - download the GGUF models of the shared llama runtime (W4).
#
# The engine runtime (fetch_engine_runtime.ps1) replaces llama.cpp/llama-server, but
# the chat/embedding/rerank GGUF weights still live in the machine-wide shared runtime
# directory (%LLAMA_RUNTIME_DIR% or %LOCALAPPDATA%\llama-runtime\models\<role>): the
# machine keeps ONE copy of the weights for all roles. This is the model-only
# part of the old installers\ensure_llama_runtime.ps1 (no binary, no llama-server).
#
# Idempotent: existing files larger than the sanity threshold are skipped (unless -Force).
# ASCII-only on purpose: PowerShell 5.1 reads a BOM-less .ps1 as ANSI and mangles
# non-ASCII text (tools/parity/README.md section 3, item 14).
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_llm_models.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_llm_models.ps1 -Models chat
param(
    [string]$RuntimeDir = "",
    [string]$Models = "chat,embedding,rerank",
    [switch]$Force
)
$ErrorActionPreference = "Continue"

if (-not $RuntimeDir) {
    if ($env:LLAMA_RUNTIME_DIR) { $RuntimeDir = $env:LLAMA_RUNTIME_DIR }
    else { $RuntimeDir = Join-Path $env:LOCALAPPDATA "llama-runtime" }
}
Write-Host "[..] shared llama runtime: $RuntimeDir"
New-Item -ItemType Directory -Force -Path (Join-Path $RuntimeDir "models") | Out-Null

# presets: role -> file + source + a size threshold used as a sanity check
$presets = @{
    "chat" = @{
        file  = "Qwen3.5-9B-Q6_K.gguf"
        url   = "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q6_K.gguf"
        minMB = 4000
    }
    "embedding" = @{
        file  = "bge-m3-Q8_0.gguf"
        url   = "https://huggingface.co/lm-kit/bge-m3-gguf/resolve/main/bge-m3-Q8_0.gguf"
        minMB = 300
    }
    "rerank" = @{
        file  = "bge-reranker-v2-m3-q8_0.gguf"
        url   = "https://huggingface.co/klnstpr/bge-reranker-v2-m3-Q8_0-GGUF/resolve/main/bge-reranker-v2-m3-q8_0.gguf"
        minMB = 300
    }
}

$wanted = @($Models -split "," | ForEach-Object { $_.Trim().ToLower() } | Where-Object { $_ })
foreach ($role in $wanted) {
    $p = $presets[$role]
    if (-not $p) { Write-Host "[--] unknown model role: $role"; continue }
    $dir = Join-Path $RuntimeDir "models\$role"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $dest = Join-Path $dir $p.file
    $minBytes = [int64]$p.minMB * 1MB
    if ((-not $Force) -and (Test-Path $dest) -and ((Get-Item $dest).Length -gt $minBytes)) {
        Write-Host "[ok] ${role}: $($p.file) already present"
        continue
    }
    # fast path: a copy already downloaded by an older LM Studio install
    $lms = Join-Path $env:USERPROFILE ".lmstudio\models"
    if (Test-Path $lms) {
        $old = Get-ChildItem -Path $lms -Recurse -Filter $p.file -ErrorAction SilentlyContinue |
            Sort-Object Length -Descending | Select-Object -First 1
        if ($old -and $old.Length -gt $minBytes) {
            Write-Host "[..] ${role}: copying from LM Studio: $($old.FullName)"
            Copy-Item $old.FullName $dest -Force
            Write-Host "[ok] ${role}: copied"
            continue
        }
    }
    Write-Host "[..] ${role}: downloading $($p.file) (about $($p.minMB) MB+)..."
    if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
        & curl.exe -L --fail --progress-bar -o "$dest.part" $p.url
    } else {
        $ProgressPreference = "SilentlyContinue"
        Invoke-WebRequest -Uri $p.url -OutFile "$dest.part" -UseBasicParsing -TimeoutSec 3600
    }
    if ((Test-Path "$dest.part") -and ((Get-Item "$dest.part").Length -gt $minBytes)) {
        Move-Item "$dest.part" $dest -Force
        Write-Host "[ok] ${role}: $dest"
    } else {
        Remove-Item "$dest.part" -Force -ErrorAction SilentlyContinue
        Write-Host "[!!] ${role}: download failed - get the file manually:" -ForegroundColor Yellow
        Write-Host "     $($p.url)  ->  $dest"
    }
}

# active chat model manifest (shared with the other projects)
$manifestChat = Join-Path $RuntimeDir "models\chat\current.json"
if (-not (Test-Path $manifestChat)) {
    $any = Get-ChildItem (Join-Path $RuntimeDir "models\chat") -Filter *.gguf -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($any) {
        $json = [pscustomobject]@{ file = $any.Name; switched_at = (Get-Date -Format s) }
        [IO.File]::WriteAllText($manifestChat, ($json | ConvertTo-Json), (New-Object System.Text.UTF8Encoding($false)))
        Write-Host "[ok] active chat model: $($any.Name) (models\chat\current.json)"
    }
}
