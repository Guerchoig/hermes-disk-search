# Подключение disk-search к Cline Desktop / Cline CLI — можно запускать В ЛЮБОЙ
# МОМЕНТ: до установки Cline (тогда запустите повторно, когда Cline появится на
# машине) или после. Регистрирует:
#   1) MCP-сервер disk-search в настройках Cline:
#      %USERPROFILE%\.cline\data\settings\cline_mcp_settings.json (Desktop/CLI)
#      и %USERPROFILE%\.cline\mcp.json (вариант CLI из docs.cline.bot/mcp);
#   2) скилл disk-search: hermes-skill\disk-search.md ->
#      %USERPROFILE%\.cline\skills\disk-search\SKILL.md
# Идемпотентно: повторный запуск обновляет записи, не дублируя их.
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot

$clineDir = Join-Path $env:USERPROFILE ".cline"
$clineCmd = Get-Command cline -ErrorAction SilentlyContinue
if (-not (Test-Path $clineDir) -and -not $clineCmd) {
    Write-Host "[--] Cline не найден ($clineDir отсутствует, 'cline' в PATH нет)." -ForegroundColor Yellow
    Write-Host "    Это нормально, если disk-search установлен раньше Cline."
    Write-Host "    Когда установите Cline Desktop (https://cline.bot/desktop), подключение"
    Write-Host "    делается одной командой:"
    Write-Host "      powershell -File `"$root\install_cline.ps1`""
    Write-Host "    Либо вручную (см. README, раздел «Интеграция с Cline Desktop»)."
    exit 0
}
Write-Host "== Подключение disk-search к Cline ($clineDir) =="

$py = Join-Path $root ".venv\Scripts\python.exe"
if (-not (Test-Path $py)) { throw "venv не найден ($py) — сначала запустите setup.ps1" }

$targets = @(
    (Join-Path $clineDir "data\settings\cline_mcp_settings.json"),
    (Join-Path $clineDir "mcp.json")
)
# Общий http-инстанс MCP (:8787): Cline подключается по URL и НЕ запускает
# собственный процесс. При stdio каждая сессия клиента рождала новый процесс,
# а долгоживущий hub-демон (code-sidecar) оставлял их сиротами.
$mcpUrl = (& $py -c "import sys; sys.path.insert(0, r'$root'); from hds import mcp_http; from hds.config import load; print(mcp_http.url(load()))").Trim()
if (-not $mcpUrl) { throw "Не удалось определить URL MCP-сервера (см. config.yaml) — настройки Cline не изменены" }
# restart-if-stale вместо start: живой инстанс переиспользуется, но если на порту
# работает СТАРЫЙ код (проект обновили) — сервер перезапускается.
Push-Location $root
$mcpRaw = & $py -m hds.cli mcp-http restart-if-stale
Pop-Location
$mcpInfo = $null
try { $mcpInfo = $mcpRaw | ConvertFrom-Json } catch { }
if ($mcpInfo -and $mcpInfo.action) {
    $verbs = @{ started = "поднят"; restarted = "перезапущен (на порту был старый код)"; reused = "уже актуален — переиспользован" }
    $verb = $verbs["$($mcpInfo.action)"]
    if (-not $verb) { $verb = "$($mcpInfo.action)" }
    Write-Host "[ok] Общий MCP-сервер ($mcpUrl) $verb"
    if ($mcpInfo.reason -and "$($mcpInfo.action)" -eq "restarted") {
        Write-Host "     причина: $($mcpInfo.reason)" -ForegroundColor DarkGray
    }
} elseif ($mcpInfo -and $mcpInfo.error) {
    Write-Host "[!!] MCP-сервер: $($mcpInfo.error)" -ForegroundColor Yellow
} else {
    Write-Host ($mcpRaw | Out-String).Trim() -ForegroundColor Yellow
}
& $py (Join-Path $root "installers\cline_mcp_merge.py") --mode http --url $mcpUrl `
    --targets $targets
if ($LASTEXITCODE -ne 0) { throw "Ошибка правки настроек MCP Cline — см. сообщение выше" }

# --- Контроль глазами самого Cline (если CLI в PATH) ---
# Cline валидирует файл настроек ЦЕЛИКОМ: одна неверная запись = теряются ВСЕ
# MCP-серверы, поэтому проверяем не только форму (её контролирует
# cline_mcp_merge.py), но и то, что клиент принимает файл. Без $ErrorActionPreference
# = "Continue" stderr CLI (node) обрывает скрипт, поэтому временно ослабляем его.
if ($clineCmd) {
    $eap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $clineCheck = (& cline config mcp --json 2>&1 | Out-String).Trim()
    $ErrorActionPreference = $eap
    if ($clineCheck -match 'Invalid MCP settings' -or $clineCheck -match '"type"\s*:\s*"error"') {
        Write-Host "[!!] Cline считает настройки MCP невалидными — он отбросит файл целиком:" -ForegroundColor Yellow
        Write-Host "     $clineCheck" -ForegroundColor Yellow
        Write-Host "     Файл: $($targets[0])" -ForegroundColor Yellow
    } elseif ($clineCheck -match 'disk-search') {
        Write-Host "[ok] Cline видит сервер disk-search"
    } else {
        Write-Host "[--] Cline не перечислил disk-search — проверьте MCP Servers в приложении" -ForegroundColor Yellow
    }
}

# --- Скилл disk-search ---
$skillSrc = Join-Path $root "hermes-skill\disk-search.md"
$skillDst = Join-Path $clineDir "skills\disk-search\SKILL.md"
if (Test-Path $skillSrc) {
    New-Item (Split-Path $skillDst) -ItemType Directory -Force | Out-Null
    Copy-Item $skillSrc $skillDst -Force
    Write-Host "[ok] Скилл установлен: $skillDst"
} else {
    Write-Host "[--] hermes-skill\disk-search.md не найден в проекте — скилл пропущен" -ForegroundColor Yellow
}

Write-Host "Перезапустите Cline Desktop (или начните новую сессию), чтобы изменения вступили в силу."
