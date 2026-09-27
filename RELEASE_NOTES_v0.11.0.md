# RELEASE_NOTES v0.11.0 — общий MCP-сервер машины (streamable-http) и интеграция Cline

## Новое

### Один MCP-сервер на машину: `hds.mcp_http` (streamable-http, :8787)

Раньше MCP-сервер работал только по stdio: каждый клиент (Cline Desktop, Hermes,
CLI) поднимал свой процесс на каждую сессию, а долгоживущие hub-демоны оставляли
после себя процессы-сироты. Теперь штатный режим — **один общий http-инстанс**:

- клиенты подключаются по URL `http://127.0.0.1:8787/mcp` и собственных
  процессов не плодят; stdio остаётся для обратной совместимости
  (`mcp_start.py`, `python -m hds.cli serve`);
- менеджер **`python -m hds.cli mcp-http`**:
  `status` (URL, PID, версия, метка сборки, `stale`), `check`, `start`, `stop`,
  `restart`, `restart-if-stale`, `run` (foreground — для автозапуска ОС),
  `stop-stdio` (разовая чистка stdio-сирот);
- **`restart-if-stale` — штатная точка обновления проекта**: по метке сборки
  (mtime `hds/*.py` + `requirements.txt` на момент старта процесса, `hds.build_stamp()`)
  и health-эндпоинту (`/health` → `{"app": "disk-search", "version", "build"}`) менеджер
  отличает свой живой инстанс от чужого сервиса на том же порту и понимает, что
  на порту работает СТАРЫЙ код после обновления проекта — тогда инстанс
  перезапускается; актуальный живой переиспользуется;
- конфиг `mcp_http` (`host` / `port` / `path` / `autostart` / `start_timeout`);
  при старте UI и MCP-сервера инстанс поднимается автоматически
  (`autostart: true`);
- в веб-интерфейсе — карточка состояния MCP-сервера (рядом с llama-серверами);
- автозапуск Windows (`install_autostart.ps1`): две задачи планировщика —
  `HermesDiskSearchWatch` и `HermesDiskSearchMcp` (`-m hds.cli mcp-http run`);
- `python -m hds.cli serve --transport stdio|sse|streamable-http` с
  `--host/--port/--path` (по умолчанию — stdio, как раньше).

### Интеграция с Cline Desktop / Cline CLI (Windows и macOS)

`install_cline.ps1` / `installers/install_cline_macos.sh` регистрируют
disk-search в настройках Cline:
`%USERPROFILE%\.cline\data\settings\cline_mcp_settings.json` и
`~/.cline/data/settings/cline_mcp_settings.json` (единый файл Desktop/CLI/IDE)
+ `~/.cline/mcp.json`, и ставят скилл
`~/.cline/skills/disk-search/SKILL.md`.

- **Форма записи — плоская, из документации Cline**:
  `{"type": "streamableHttp", "url": ...}` (stdio: `{"command", "args", "env"}`).
  Устаревшая обёртка `"transport": {"type": "http", ...}` невалидна: во вложенном
  `transport` Cline принимает только `stdio|sse|streamableHttp`, и одна неверная
  запись приводила к отбрасыванию ВСЕГО файла настроек (`Invalid MCP settings ...
  mcpServers.disk-search: Invalid input`) — клиент терял ВСЕ MCP-серверы, а агент
  уходил искать файлы терминалом;
- после записи контролируется форма (`installers/cline_mcp_merge.py`) и, если в
  PATH есть CLI `cline`, настройки дополнительно проверяются самим клиентом
  (`cline config mcp --json`) — при невалидном файле печатается предупреждение;
- инсталляторы идемпотентны: другие серверы (например tavily) сохраняются;
- stdio-вариант доступен как раньше (`--mode stdio`).

### Интеграция с Hermes Desktop (Windows и macOS)

- Hermes регистрируется по **URL общего инстанса** (`url: http://127.0.0.1:8787/mcp`)
  вместо stdio-процесса на каждую сессию (stdio — fallback, если venv не найден);
- перед подключением Hermes общий сервер поднимается/обновляется
  (`mcp-http restart-if-stale` — после обновления проекта агент не ходит к старой
  версии на порту);
- **обход системного прокси для loopback**: `httpx2` берёт прокси из реестра
  Windows (`urllib.request.getproxies()`), игнорируя ProxyOverride, — запросы к
  `127.0.0.1:8787` уходили в системный прокси (xray/Clash), сервер отвечал 503 и
  «парковался». `install_hermes.ps1` пишет идемпотентный блок в `<Hermes>\.env`:
  `NO_PROXY` с loopback + зеркало системного прокси в `HTTP(S)_PROXY`.

## Исправления

- **watcher: дубли при двойном старте.** Захват `watch.lock` теперь атомарный
  (`O_CREAT|O_EXCL`): раньше «есть файл → жив ли PID» и запись были разными
  шагами, и два одновременных старта (автозапуск при входе + кнопка/ярлык)
  успевали оба пройти проверку. Устаревший lock (умерший/чужой процесс) снимается
  и старт повторяется; без psutil PID считается живым watcher'ом пессимистично —
  дубль хуже пропущенного старта.
- **Инсталляторы macOS (Cline и Hermes): `mcp-http start` → `restart-if-stale`.**
  После обновления проекта `start` переиспользовал бы инстанс со СТАРЫМ кодом на
  порту; теперь он перезапускается, как на Windows.
- **llama-server (роль chat): `ctx_per_slot` 16384 → 32768.** 32K требует
  общий с `anonymizer_proxy` инстанс для длинных файлов (одному HDS хватало 16K,
  но один инстанс на две программы экономнее).

## Требования к обновлению

- Переиндексация **не** требуется (схема БД не менялась).
- Общий MCP-сервер поднимется автоматически (`mcp_http.autostart`). Если сервер
  уже работал — обновите его: `python -m hds.cli mcp-http restart-if-stale`
  (инсталляторы делают это сами).
- Перенастройте интеграцию Cline (важно: прежняя запись с обёрткой `transport`
  ломала ВСЕ MCP-серверы Cline):
  `powershell -File install_cline.ps1` (Windows) /
  `bash installers/install_cline_macos.sh` (macOS), затем перезапустите Cline
  Desktop или начните новую сессию.
- Перенастройте Hermes: `powershell -File install_hermes.ps1` (Windows) /
  `bash installers/install_hermes_macos.sh` (macOS).

## Проверки

- 269 регрессионных тестов (`python -m unittest discover -s tests`) — зелёные,
  включая новые: `tests/test_mcp_http.py` (менеджер: конфиг, URL, probe,
  restart-if-stale) и `tests/test_cline_mcp_merge.py` (форма записи MCP-настроек
  Cline);
- ручная приёмка на реальной машине: `hds.cli check` — без критических проблем;
- MCP-протокол end-to-end: `initialize` → `tools/list` (6 инструментов) →
  `search_local_files` возвращает результаты из индекса.
