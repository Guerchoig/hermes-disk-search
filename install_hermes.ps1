# Подключение disk-search к Hermes Desktop — можно запускать В ЛЮБОЙ МОМЕНТ:
#   - до установки Hermes (тогда просто запустите этот скрипт повторно, когда
#     Hermes Desktop появится на машине);
#   - после установки (регистрирует MCP-сервер, скилл и настройку инструментов
#     за один запуск).
# Регистрирует:
#   1) MCP-сервер disk-search в config.yaml (секция mcp_servers)
#   2) скилл disk-search (правило «поиск файлов — через MCP, а не grep»)
#   3) tools.tool_search.enabled: "off" — все инструменты всегда в промпте
#      (локальная 9B-модель не находит MCP-инструменты через discovery-протокол)
# Идемпотентно: повторный запуск обновляет блоки, не дублируя их.
param([string]$HermesDir = "")
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot

if (-not $HermesDir) {
    $HermesDir = Join-Path $env:LOCALAPPDATA "hermes"
}
$cfgPath = Join-Path $HermesDir "config.yaml"
if (-not (Test-Path $cfgPath)) {
    Write-Host "[--] Hermes Desktop не найден ($cfgPath)." -ForegroundColor Yellow
    Write-Host "    Это нормально, если disk-search установлен раньше Hermes."
    Write-Host "    Когда установите Hermes Desktop, подключение делается одной командой:"
    Write-Host "      powershell -File `"$root\install_hermes.ps1`""
    Write-Host "    Либо вручную (см. README, раздел «Интеграция с Hermes»):"
    Write-Host "      1) в <Hermes>\config.yaml в секцию mcp_servers добавить блок disk-search;"
    Write-Host "      2) скопировать hermes-skill\SKILL.md в <Hermes>\skills\disk-search\SKILL.md;"
    Write-Host "      3) добавить tools.tool_search.enabled: \`"off\`" (все инструменты в промпте);"
    Write-Host "      4) если в системе включён HTTP-прокси (xray/Clash) — в <Hermes>\.env добавить"
    Write-Host "         NO_PROXY=localhost,127.0.0.1,::1 : httpx2 игнорирует ProxyOverride и без этого"
    Write-Host "         шлёт запросы к 127.0.0.1 в прокси, MCP отвечает 503 и не подключается."
    exit 0
}
Write-Host "== Подключение disk-search к Hermes ($HermesDir) =="

# --- 1. MCP-сервер disk-search в config.yaml (правка текстом, комментарии сохраняются) ---
$py = Join-Path $root ".venv\Scripts\python.exe"
$mcpUrl = ""
if (Test-Path $py) {
    $mcpUrl = (& $py -c "import sys; sys.path.insert(0, r'$root'); from hds import mcp_http; from hds.config import load; print(mcp_http.url(load()))").Trim()
}
if ($mcpUrl) {
    # Общий http-инстанс MCP (:8787): Hermes подключается по URL и не запускает
    # свой процесс на каждую сессию — сервер один на машину (hds.mcp_http).
    $mcpBlock = @(
        '  disk-search:'
        "    url: $mcpUrl"
        '    timeout: 300'
    ) -join "`r`n"
} else {
    Write-Host "[--] venv не найден — регистрирую stdio-вариант MCP" -ForegroundColor Yellow
    $mcpBlock = @(
        '  disk-search:'
        "    command: $root\.venv\Scripts\python.exe"
        '    args:'
        "      - $root\mcp_start.py"
        '    timeout: 300'
    ) -join "`r`n"
}
$enc = New-Object System.Text.UTF8Encoding($false)
$text = [System.IO.File]::ReadAllText($cfgPath, $enc)
# блок целиком: заголовок + строки с отступом 4+ (аргументы), соседние ключи не задеваем
$rxBlock = [regex]::new('(?m)^  disk-search:\r?\n(?:    [^\r\n]*\r?\n?)*')
if ($rxBlock.IsMatch($text)) {
    $text = $rxBlock.Replace($text, { param($m) $mcpBlock + "`r`n" }, 1)
    Write-Host "[ok] MCP disk-search: блок в config.yaml обновлён"
} elseif ($text -match '(?m)^mcp_servers:\s*$') {
    $text = [regex]::new('(?m)^mcp_servers:\s*$').Replace($text, { param($m) $m.Value + "`r`n" + $mcpBlock }, 1)
    Write-Host "[ok] MCP disk-search: добавлен в существующую секцию mcp_servers"
} else {
    $text = $text.TrimEnd() + "`r`n`r`nmcp_servers:`r`n" + $mcpBlock + "`r`n"
    Write-Host "[ok] MCP disk-search: секция mcp_servers добавлена в конец config.yaml"
}
$tmp = "$cfgPath.tmp"
[System.IO.File]::WriteAllText($tmp, $text, $enc)
Move-Item $tmp $cfgPath -Force

# валидация YAML + проверка, что disk-search на месте
$py = Join-Path $root ".venv\Scripts\python.exe"
if (Test-Path $py) {
    & $py -c "import yaml,sys; d=yaml.safe_load(open(r'$cfgPath', encoding='utf-8-sig')); ds=(d.get('mcp_servers') or {}).get('disk-search'); sys.exit(0 if ds and (ds.get('url') or ds.get('command')) else 1)"
    if ($LASTEXITCODE -ne 0) { throw "config.yaml Hermes повреждён после правки — проверьте вручную" }
    Write-Host "[ok] config.yaml Hermes валиден, disk-search зарегистрирован"
}

# --- 1b. tools.tool_search.enabled: "off" — все инструменты всегда в промпте ---
# Локальные модели (Qwen3.5-9B) не осиливают discovery-протокол tool_search/
# tool_describe/tool_call и не находят MCP-инструменты; облачным не мешает.
$tsSearch = [regex]::new('(?m)^  tool_search:\r?\n(?:    [^\r\n]*\r?\n?)*')
$tsInner = @(
    '  tool_search:'
    '    enabled: "off"'
) -join "`r`n"
if ($tsSearch.IsMatch($text)) {
    $text = $tsSearch.Replace($text, { param($m) $tsInner + "`r`n" }, 1)
    Write-Host "[ok] tools.tool_search: блок обновлён (enabled: off)"
} elseif ($text -match '(?m)^tools:\r?$') {
    $text = [regex]::new('(?m)^(tools:\r?\n)').Replace($text, { param($m) $m.Value + $tsInner + "`r`n" }, 1)
    Write-Host "[ok] tools.tool_search: добавлен в существующую секцию tools"
} else {
    $text = $text.TrimEnd() + "`r`n`r`n# Все инструменты всегда в промпте (локальная 9B-модель не находит MCP-инструменты через discovery-протокол)`r`ntools:`r`n" + $tsInner + "`r`n"
    Write-Host "[ok] секция tools.tool_search добавлена в конец config.yaml"
}
$tmp2 = "$cfgPath.tmp2"
[System.IO.File]::WriteAllText($tmp2, $text, $enc)
Move-Item $tmp2 $cfgPath -Force
# Сервер должен быть поднят ДО подключения агента: Hermes ходит по URL, а не
# запускает процесс. Вместо `start` — `restart-if-stale`: живой инстанс
# переиспользуется, но если на порту работает СТАРЫЙ код (проект обновили),
# сервер перезапускается — иначе Hermes продолжал бы ходить к прежней версии.
if ($mcpUrl) {
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
}
if (Test-Path $py) {
    & $py -c "import yaml,sys; d=yaml.safe_load(open(r'$cfgPath', encoding='utf-8-sig')); ts=((d.get('tools') or {}).get('tool_search') or {}); sys.exit(0 if ts.get('enabled')=='off' else 1)"
    if ($LASTEXITCODE -ne 0) { throw "tools.tool_search.enabled != 'off' после правки — проверьте config.yaml вручную" }
    Write-Host "[ok] tools.tool_search.enabled = off (все инструменты в промпте)"
}

# --- 1c. Обход системного прокси для loopback (обязателен для HTTP-режима MCP) ---
# httpx2 (HTTP-движок Hermes) определяет прокси через urllib.request.getproxies(),
# а та берёт ProxyServer из реестра Windows и ИГНОРИРУЕТ ProxyOverride («не использовать
# прокси для localhost;127.*»). Поэтому запросы к локальному MCP по 127.0.0.1:8787 уходили
# в системный прокси (xray/Clash), сервер отвечал 503, и Hermes писал
# "MCPError: Server returned an error response" → сервер «парковался», а агент заявлял,
# что MCP-инструменты disk-search недоступны.
# Пишем блок в <Hermes>\.env: NO_PROXY с loopback + зеркало системного прокси в
# HTTP(S)_PROXY (чтобы внешний трафик продолжал идти через тот же прокси). Файл .env
# Hermes загружает с override=True (hermes_cli.env_loader), поэтому значения применяются.
# Блок идемпотентный: повторный запуск заменяет его, не дублируя.
$envPath = Join-Path $HermesDir ".env"
$proxyUrl = ""
try {
    $inet = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' -ErrorAction Stop
    if ($inet.ProxyEnable -and $inet.ProxyServer) {
        $first = ("$($inet.ProxyServer)" -split ';')[0]
        if ($first -match '=') { $first = ($first -split '=')[-1] }
        if ($first) { $proxyUrl = "http://$first" }
    }
} catch { }
$envBlock = @(
    '# >>> disk-search: обход системного прокси для loopback >>>'
    '# httpx2 (HTTP-движок Hermes) берёт прокси из реестра Windows через'
    '# urllib.request.getproxies(), которая ИГНОРИРУЕТ ProxyOverride. Без этих строк'
    '# запросы к локальным адресам (MCP disk-search на 127.0.0.1:8787, llama-server)'
    '# уходят в системный прокси и MCP-подключение падает с'
    '# "MCPError: Server returned an error response".'
    '# HTTP(S)_PROXY зеркалят системный прокси, чтобы интернет шёл через него как раньше.'
    '# Блок управляется проектом hermes-disk-search (install_hermes.ps1) — правьте там.'
    'NO_PROXY=localhost,127.0.0.1,::1'
    'no_proxy=localhost,127.0.0.1,::1'
)
if ($proxyUrl) { $envBlock += @("HTTP_PROXY=$proxyUrl", "HTTPS_PROXY=$proxyUrl") }
$envBlock += '# <<< disk-search <<<'
$envText = if (Test-Path $envPath) { [System.IO.File]::ReadAllText($envPath, $enc) } else { "" }
$rxEnv = [regex]::new('(?ms)^# >>> disk-search: обход системного прокси для loopback >>>.*?^# <<< disk-search <<<\r?\n?')
$envText = $rxEnv.Replace($envText, "").TrimEnd() + "`r`n`r`n" + ($envBlock -join "`r`n") + "`r`n"
[System.IO.File]::WriteAllText($envPath, $envText, $enc)
$proxyNote = if ($proxyUrl) { "; системный прокси $proxyUrl зеркалируется в HTTP(S)_PROXY" } else { "" }
Write-Host "[ok] .env Hermes: NO_PROXY для loopback (127.0.0.1/localhost)$proxyNote"

# --- 2. Скилл disk-search ---
$skillSrc = Join-Path $root "hermes-skill\SKILL.md"
$skillDst = Join-Path $HermesDir "skills\disk-search\SKILL.md"
if (Test-Path $skillSrc) {
    New-Item (Split-Path $skillDst) -ItemType Directory -Force | Out-Null
    Copy-Item $skillSrc $skillDst -Force
    Write-Host "[ok] Скилл установлен: $skillDst"
} else {
    Write-Host "[--] hermes-skill\SKILL.md не найден в проекте — скилл пропущен" -ForegroundColor Yellow
}

Write-Host "Перезапустите Hermes Desktop (или начните новую сессию), чтобы изменения вступили в силу."
