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
& $py (Join-Path $root "installers\cline_mcp_merge.py") --python $py `
    --mcp-start (Join-Path $root "mcp_start.py") --targets $targets
if ($LASTEXITCODE -ne 0) { throw "Ошибка правки настроек MCP Cline — см. сообщение выше" }

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
