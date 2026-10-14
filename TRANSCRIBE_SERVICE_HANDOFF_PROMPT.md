# TRANSCRIBE_SERVICE_HANDOFF_PROMPT.md — промт для нового чата (этап: автотранскрибация+диаризация как служба пакета)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, фактическая база (по коду), финальные решения, треки R0–R5, критерии приёмки
> и грабли окружения.

---

## Что произошло (контекст из предыдущего чата)

Проверен отчёт: **автоматическая транскрибация и диаризация при помещении медиафайла в
назначенную папку не согласована с остальными службами и не устанавливается установщиком.**
Составлен план рефакторинга: вынести `hds transcribe-watch` из «desktop-only задачи
Планировщика» в **полноправную 5-ю роль службы** серверного пакета, убрать ASR из конвейера
индексации, добавить пользовательский (не админский) UI. Пользователь утвердил финальные
решения (§«Финальные решения») — план закрыт. **Первое действие в act-режиме:** оформить
`PLAN_TRANSCRIBE_SERVICE.md` (образец — `PLAN_M9_CLIENT_PACKAGE.md`), затем идти треками
R0 → R5 с тестами и живой приёмкой.

## Репозиторий и состояние

* Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**,
  HEAD `a2a7b0e` (**tag `v0.19.0`**), удалённый `origin/server-multiuser` — синхронно.
* Состояние: **M0–M9 закрыты** (серверный и клиентский пакеты готовы). Рабочее дерево чистое
  (кроме локальных `config.yaml`, `data/`, `index.db`, `*.lock`, логов из корня — см. R0).
* Боевой dev-стенд: резидент `llm_host` (`target\release`), `hds watch` / `hds transcribe-watch`
  / `hds mcp --http` (`target\debug`), `index.pause` — есть, `transcribe.lock` — есть.

## Станция (факты окружения)

Windows, RTX 3060 12 ГБ, AMD Radeon iGPU (UMA), Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1.
Ядро — Rust (`crates/`: hds-core, hds-cli, hds-index, hds-llama, hds-mcp, hds-ui, hds-search,
hds-extract, hds-clip); Python — только sidecar. Сессия Cline — **не админ**. Git Bash —
`C:\Program Files\Git\bin\bash.exe` (WSL нет). Артефакты — `tools/parity/out/` (гитигнор).
Это **dev-машина** (`cline-rules/dev-machine.md`): процессы можно останавливать/запускать,
GPU/VRAM освобождать, `config.yaml`/`data/`/`index.db` перезаписывать, релиз пересобирать.

## Финальные решения пользователя (утверждены, менять нельзя)

1. **Службы не доставляют `.md` в индекс.** Пользователи вручную переносят готовые файлы из
   `out_dir` в индексируемые папки. Никакого `index-queue.jsonl`, догонов, ключа
   `index.no_walk_paths` и требования «`out_dir` внутри `index.roots`» — **не делаем**.
   `index.exclude_paths` остаётся единственным и простым механизмом («не обходить и не
   индексировать»); под него неявно (рантайм) попадают `auto_transcribe.inbox_dir`, `out_dir`,
   `state_dir`.
2. **Кнопка «перенести в индексируемую папку» в UI — НЕ нужна** (перенос строго вручную).
3. **Ключ `index.media_metadata` — НЕ нужен.** Медиа **полностью пропускается** конвейером
   (`skipped_media`), ни метаданных, ни ASR.
4. **Исходный аудио/видео удаляется безусловно, как сейчас** — никаких флагов
   (`confirm_delete` и пр.). Политика `source_disposal: delete` работает без гейта. (Оригиналы
   медиа для установления истины хранятся нетронутыми в другом месте — вне контура службы.)
5. **Пользовательский UI, права:** если в UI есть возможность и заменять спикеров, и править
   `.md`, то **авторизованный пользователь (не админ) может делать и то, и другое**. Per-user
   карта имён и «правка общего файла только админом» — **отменяются** (упрощение: любой валидный
   токен → правка/замена спикеров в `.md`).
6. **Форма роли службы — согласовано:** 5-я роль `transcribe` в `ROLES`
   (SCM `hermes-disk-search-transcribe` с `depend= hermes-disk-search-llm-host`); в desktop —
   существующая задача `HermesDiskSearchTranscribe`. Обязательно починить `--help` у
   `transcribe-watch`.

## Фактическая база (по коду — опора плана)

| # | Факт | Где |
|---|---|---|
| Ф1 | Ролей служб **ровно 4**: `ROLES = [watch, mcp-http, ui, llm-host]`; тест фиксирует `ROLES.len() == 4` | `crates/hds-core/src/service.rs:27`, тесты |
| Ф2 | `hds service run <role>` — единая точка; маппинг роль→функция в `run_role_blocking`; демона транскрибации там нет | `crates/hds-cli/src/cmd/service.rs:55-66` |
| Ф3 | `GET /api/services` UI **итерирует `ROLES`** — 5-я роль появится в карточке автоматически | `crates/hds-ui/src/lib.rs::services_json_profile` |
| Ф4 | Установщик сервера ставит `-Roles "watch,mcp-http,ui,llm-host"`; `-Fetch` тянет `engine|llm_models|whisper|clip` (sortformer **не** качается); конфиг генерит `New-ServerConfig` (без `auto_transcribe`/исключений) | `installers/install_server.ps1:25,159,68-96`; `install_server.command:95+` |
| Ф5 | Desktop-автозапуск **уже** создаёт задачу `HermesDiskSearchTranscribe` (`hds transcribe-watch`); `install_server.ps1` её удаляет | `install_autostart.ps1:48`, `installers/install_server.ps1:137` |
| Ф6 | Подкоманда `transcribe-watch` **игнорирует остальные аргументы** — поэтому `--help` запускал демон (баг) | `crates/hds-cli/src/main.rs:123` |
| Ф7 | ASR в индексации живёт в `MediaRouter` (kind==`media` → `/internal/transcribe`); подставляется в 4 боевых точках: CLI `index`/`watch`, MCP `tools.rs` (×2), UI `index_bg` | `hds-index/src/transcribe.rs:703-773`; `hds-cli/src/support.rs:84-91`; `cmd/{index,watch}.rs`; `hds-mcp/src/tools.rs:343,370`; `hds-ui/src/lib.rs:419` |
| Ф8 | Ключ `index.transcribe: true` по умолчанию; `index.whisper_*`/`transcribe_url` — там же | `config.example.yaml:20-30` |
| Ф9 | `index.exclude_paths` — обычный список, редактор сохраняет его из UI; дефолт пуст | `config.example.yaml:14`, `hds-ui/src/config_edit.rs` |
| Ф10 | Демон: `transcribe.lock` (`WatchLock::acquire_named`), очередь JSONL, манифест, `source_disposal: delete|keep|move`, `PauseGate` (`index.pause`) | `hds-index/src/autotranscribe.rs:656,633-635,954` |
| Ф11 | Модели ищутся **только** в `%APPDATA%\OpenResearchTools\models` (whisper — `hds-llama/src/host.rs:993`; sortformer — `host.rs:1022`, `hds-index/src/autotranscribe.rs:566`); у каталога движка есть конфиг-ключ и env `HDS_ENGINE_DIR` | `crates/hds-llama/src/engine_dir.rs:42` |
| Ф12 | Fetch-скрипты моделей параметризованы `-ModelsDir` (whisper и sortformer) | `installers/fetch_whisper_model.ps1:16-31`, `fetch_diarization_model.ps1:18-33` |
| Ф13 | UI сейчас **только админский**: `ui_auth_gate` пускает лишь `ctx.admin`; логина по паролю в UI нет (есть в MCP `POST /auth/login`) | `hds-ui/src/lib.rs:1597-1695`; `hds-mcp/src/auth.rs`; `hds-core/src/access.rs::authenticate/new_token` |
| Ф14 | Страница транскрибации + присвоение имён **уже реализованы** (`list/file/speakers/apply`, path-safety внутри `out_dir`, `.bak`, sidecar) — переиспользовать под пользовательским гейтом | `hds-ui/src/{transcribe.rs,page.rs:165-215}`, `route` (`lib.rs:1737-1750`) |
| Ф15 | Тонкий клиент и его установка **уже есть и публикуются сервером**: `/mcp/client-install.ps1|sh`, `/mcp/client-package.zip?platform=`; мастера сверяют `version` из `/health` | `hds-mcp/src/http.rs:382,478,1002`; `installers/{client-install.ps1,build_client_release.ps1}`; `hds-core/src/client.rs` |

## Целевая архитектура (концепция)

```
inbox_dir (медиа)                       out_dir (текст, .md)
     │ кладёт пользователь                ▲ пишет служба transcribe
     ▼                                     │
┌───────────────────────────┐  HTTP /internal/transcribe  ┌──────────────┐
│ служба transcribe         │ ──────────────────────────▶ │ llm-host     │
│ (SCM/launchd/Планировщик) │ ◀──── .md с SPEAKER_00… ─── │ (владелец GPU│
│ очередь/манифест/dispose   │                             │ whisper+dia) │
└───────────────┬───────────┘                             └──────────────┘
                │ (перенос .md в индексируемую папку — ВРУЧНУЮ, вне службы)
                ▼
        конвейер индексации (только текст; медиа — skip)
```

Индексация **никогда** не транскрибирует; транскрибация — **единственный** источник текстов
для медиа. Служба — 5-я роль пакета, зависимая от `llm-host`. Доставки в индекс нет.

## Треки работ R0–R5 (план)

### R0 — сброс dev-стенда (0,5 дн; разрешено)
Остановить службы/задачи; удалить `index.db`, `data/auto-transcribe*`, `transcribe.lock`,
`index.pause`, тестовые `media-in/out`; гигиена мусора в корне (`bt.log`, `clip_*.log`,
`ui_out.log`, `stop_log.log`, `*.log`).

### R1 — исключить транскрибацию из индексации (1–1,5 дн)
| Шаг | Правка | Файлы |
|---|---|---|
| R1.1 | Ранний пропуск медиа: сразу после определения `kind` (`is_media(kind)`) → `Early { status: "skipped_media", kind: Some("media") }`. Ни стата, ни `content_hash`, ни эмбеддингов | `hds-index/src/pipeline.rs::extract_file` (после ~стр. 185) |
| R1.2 | Убрать `MediaRouter` из боевого пути: `build_media_extractor` → просто `&sidecar` в CLI; в MCP/UI заменить `MediaRouter::new(&side, &cfg)` на `&side` | `hds-cli/src/support.rs:84-91`, `cmd/{index,watch}.rs`, `hds-mcp/src/tools.rs:343,370`, `hds-ui/src/lib.rs:419` |
| R1.3 | `MediaRouter`/`transcribe_media`/`lead_segment` оставить **только** для `hds whisper-check`/проб (или удалить из `hds-index`, переведя `whisper-check` на `TranscribeClient`) | `hds-index/src/transcribe.rs`, `hds-cli/src/cmd/whisper_check.rs` |
| R1.4 | Ключ `index.transcribe` → **legacy**: при `true` — предупреждение в лог и `hds check` («ASR в индексации отключена, используйте `auto_transcribe`»); поведение как при `false`; дефолт в `config.example.yaml` → `false` | `config.example.yaml:20`, `diag.rs`, `README.md` |
| R1.5 | UI/диагностика: статус «медиа пропускается (skipped_media)» в `/api/diagnostics` | `hds-ui/src/page.rs`, `hds-index/src/diag.rs` |
Признать явно: медиа исчезнет из поиска по имени («найди фильмы» исходники больше не найдёт).

### R2 — дефолтные исключения inbox/out (0,5 дн; упрощён правкой §52)
| Шаг | Правка |
|---|---|
| R2.1 | **Неявное (рантайм) исключение**: `pipeline::index_filter` добавляет к `exclude_paths` значения `auto_transcribe.inbox_dir`, `out_dir`, `state_dir`; видно в `/api/config` (`implicit_excludes`) и в `hds check` |
| R2.2 | Дефолты `config.example.yaml`: `index.exclude_paths` — с закомментированными подсказками; единственный источник путей — `auto_transcribe.inbox_dir/out_dir` |
| R2.3 | Установщики пишут **реальные** пути: `New-ServerConfig` — `index.exclude_paths` + `auto_transcribe` (новые параметры `-MediaInbox`, `-MediaOut`, `-EnableTranscribe`, `-SourceDisposal`); паритет в `install_server.command` |
| R2.4 | Проверка `hds check`/`/api/diagnostics`: `auto-transcribe-dirs` — папки абсолютные, существуют, различны, не вложены друг в друга и **исключены** из индексации; иначе `warn` + подсказка |
| R2.5 | UI: рядом с полями папок индикатор «исключена из индексации ✓/✗» и кнопка «исключить» (через `config_edit.rs`) |
> Кнопка-помощник «перенести в индексируемую папку» — **не делаем** (решение №2).

### R3 — транскрибация как полноправная служба (3–4 дн)
| Шаг | Правка |
|---|---|
| R3.1 | `ROLES` += `"transcribe"` (5 ролей); поправить тесты (`roles_and_names`, `ROLES.len()`); UI-карточка получит роль автоматически (Ф3) |
| R3.2 | `run_role_blocking`: `"transcribe" => crate::cmd::transcribe::cmd_transcribe_watch()`; usage-тексты |
| R3.3 | **Починить CLI-контракт**: `transcribe-watch --help/-h` → usage и код 0 (без запуска); `--dry-run` (проверка конфига/моделей/владельца); обычный запуск — как сейчас. Тест разбора аргументов |
| R3.4 | **Машинный каталог моделей** (лечит Ф11 без junction'ов): приоритет `auto_transcribe.whisper_model`/`diarization_model` (абс. пути) → env `HDS_MODELS_DIR` → `%ProgramData%\hermes-disk-search\models` → текущий `%APPDATA%\...`; 3 места `default_*_model`. Установщик качает модели `-ModelsDir` в ProgramData и пишет абс. пути в `config.yaml` |
| R3.5 | **Установщик сервера**: `-Roles` по умолчанию `watch,mcp-http,ui,llm-host,transcribe`; `-Fetch` += `fetch_whisper_model.ps1` + `fetch_diarization_model.ps1`; `New-ServerConfig` пишет `auto_transcribe` (enabled по флагу, `source_disposal: delete` — см. решение №4, `pause_indexing: true`); SCM `depend= hermes-disk-search-llm-host` |
| R3.6 | Порядок ролей в обновлении: stop — `transcribe` до `llm-host`; start — `transcribe` после `llm-host` (`installers/update.ps1:40-43`) |
| R3.7 | Демон: на старте **ждать владельца** (`/health`/проба порта с backoff); `sc.exe stop` — поставить `transcribe.stop` и корректно снять `transcribe.lock` |
| R3.8 | ~~`confirm_delete`-гейт~~ **ОТМЕНЁН решением №4**: `source_disposal: delete` работает безусловно, как сейчас |
| R3.9 | Desktop-ветка: `install_autostart.ps1` оставляет задачу `HermesDiskSearchTranscribe` (Ф5); `setup.ps1:250-275` — блок «Transcription service» (запрос whisper+sortformer → `-ModelsDir`) + предложение задать inbox/out |
| R3.10 | UI-карточка «Демон автотранскрибации»: в `profile: server` действия идут в SCM (`hds service start|stop transcribe`), в desktop — как сейчас; в ответе — форма (service/process/task) |
| R3.11 | `hds check`: «служба transcribe не запущена дважды / не рядом с задачей Планировщика» (`diag.rs`) |

### R4 — пользовательский UI (не админский) (2,5–3,5 дн)
| Шаг | Правка |
|---|---|
| R4.1 | **Логин по паролю в UI**: `POST /api/auth/login {login,password}` → `hds_core::access::authenticate` + `new_token` (TTL `access.token_ttl_days`), ответ `{token,user,expires,admin,projects}`; строгий rate-limit по IP; аудит `login_ok`/`login_fail`; плейн-пароли в логи не писать |
| R4.2 | **Разделение гейтов**: `ui_user_gate` — любой валидный токен для страницы пользователя и «безопасных» API; `ui_auth_gate` (админ) остаётся для config/services/access/index/diagnostics. Не-админ на админском API → `403` (+аудит) |
| R4.3 | **Страница пользователя** (`/ui/user/`, константа `PAGE_USER`): шапка с пользователем/выходом; блоки: (а) «Поиск» под user-гейтом (выдача фильтруется проектами токена), (б) «Установка клиента», (в) **«Транскрипты/Спикеры»** |
| R4.4 | **Установка клиента**: ссылки на `/mcp/client-install.ps1|sh` (токен аргументом), кнопка `/mcp/client-package.zip?platform=...`, показ адреса сервера и сверка `version` (`/health`); переиспользовать `hds-core::client` |
| R4.5 | **Спикеры/правка `.md`** — переиспользовать `hds-ui/src/transcribe.rs` (list/превью/`speakers`/apply, path-safety, `.bak`, sidecar). **Права (решение №5): авторизованный не-админ может и заменять спикеров, и править `.md`.** Per-user карту имён не делаем |
| R4.6 | Reverse-proxy: наружу `/mcp` и `/ui` (+ `/health`, `/mcp/client-install.*` публично); фасад `:8010–8012` не публикуется (`installers/proxy/{Caddyfile,nginx.conf}`, `README_SERVER_MULTIUSER.md`) |
| R4.7 | `hds login` (CLI) — тот же контракт пароль→токен в UI (`hds-cli/src/cmd/auth.rs`) |

### R5 — приёмка и релиз (1 дн)
Живая приёмка на стенде, релизная сборка, доки (`README`, `README_SERVER_MULTIUSER`, `STATUS`,
`config.example.yaml`, `RELEASE_NOTES`, `PLAN_TRANSCRIBE_SERVICE.md`).

**Итого: ~8–10 дней** одним исполнителем.

## Критерии приёмки
1. Прогон `hds index`/`watch` по корню с медиа: в БД нет файлов `kind=media` (только
   `skipped_media`), в логе `llm-host` **нет** запросов `/internal/transcribe`, время прогона не
   растёт на медиа, не-медиа паритет не сломан (`pipeline_parity`).
2. Свежий конфиг: `inbox_dir`/`out_dir` исключены; в `hds check` оба пути видны как исключённые;
   при удалении их из `exclude_paths` проверка краснеет.
3. `sc.exe query hermes-disk-search-transcribe` → `RUNNING` после перезагрузки без входа
   пользователя; файл в `inbox_dir` → `.md` с `SPEAKER_00…`/`UNASSIGNED` в `out_dir`, строка в
   `data/auto-transcribe-queue.jsonl`; `watch`/`mcp-http`/`ui` живы; VRAM-порядок «поиск/чат →
   transcribe → индексация»; повторный запуск не создаёт второго демона; `sc.exe stop` не
   оставляет залипший `transcribe.lock`/`index.pause`.
4. Пользовательский UI: с ноутбука логин/пароль → токен; видны только 3 функции (поиск,
   установка клиента, спикеры); админские API отдают `403`; без токена — `401`; не-админ может
   заменять спикеров **и** править `.md` (с `.bak`), не портя оригиналы; установка клиента
   подключает Cline к внешнему серверу.

## Тесты
* Юниты: `skipped_media`; неявные исключения; `parse_role("transcribe")`;
  `sc_install_raw`/`daemon_plist`/`ROLES` для роли; разбор `--help` у `transcribe-watch`;
  приоритет путей моделей (config→env→ProgramData→APPDATA); user-гейт (`ui_auth.rs`/`ui_core.rs`):
  логин, `403` не-админу, rate-limit логина.
* Интеграция/стенд: `cargo test --workspace` + `clippy -D warnings` + `fmt --check`; e2e:
  SCM-служба на тестовой inbox, UI через `curl` с токеном.

## Риски
| Риск | Митигация |
|---|---|
| LocalSystem не видит модели (Ф11) | машинный каталог моделей + абс. пути в конфиге (R3.4); junction'ы не нужны |
| `source_disposal: delete` в службе = потеря данных | **принято пользователем** (решение №4): удаление безусловно; приёмка — на текущей политике |
| Служба стартует раньше `llm-host` | SCM `depend=` + ожидание владельца на старте (R3.5, R3.7) |
| Два демона (задача + служба) | `install_server.ps1` удаляет задачи, `transcribe.lock`, проверка в `hds check` |
| Регресс паритета индексации | правки только на медиа-ветке; `pipeline_parity`/`walk_parity` в обязательном прогоне |

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -10`   # HEAD a2a7b0e / v0.19.0
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`
3. `cargo test --workspace` — ⚠ живые службы держат `target\debug\hds.exe`: остановить
   `watch`/`transcribe-watch`/`mcp --http`, после — **вернуть**; либо `--exclude hds-cli`.
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
5. Прочитать: **`PLAN_AUTO_TRANSCRIBE.md`** (текущее устройство), `PLAN_M9_CLIENT_PACKAGE.md`
   (образец плана этапа), `PLAN_SERVER_MULTIUSER.md` (§2 W8/Э10 — смежный этап),
   `crates/hds-core/src/service.rs`, `installers/install_server.ps1`, `crates/hds-ui/src/lib.rs`,
   `crates/hds-index/src/autotranscribe.rs`.
6. Карта кода: `hds-index/src/{pipeline.rs,diag.rs,autotranscribe.rs,transcribe.rs}`,
   `hds-cli/src/{main.rs,cmd/service.rs,cmd/transcribe.rs,support.rs}`,
   `hds-ui/src/{lib.rs,page.rs,transcribe.rs,config_edit.rs}`,
   `hds-llama/src/{host.rs,engine_dir.rs}`, `installers/*.ps1`.

## Правила и грабли окружения (важно, платил временем)
* **Резидент держит `target\release\*.exe`**, живые службы — `target\debug\hds.exe`: перед
  пересборкой/полным `cargo test` остановить и **вернуть** (`… watch` / `… transcribe-watch` /
  `… mcp --http --host 127.0.0.1 --port 8787 --path /mcp`; резидент —
  `Start-Process target\release\llm_host.exe -ArgumentList run -WindowStyle Hidden`).
  Dev-машина (`cline-rules/dev-machine.md`) — можно.
* **`HDS_CONFIG`/`HDS_ROOT` переживают между элементами `run_commands`** — задавать и **очищать**.
* **Боевой `index.pause` не снимать** на боевом контуре; плейн-токены/секреты не печатать.
* **PowerShell 5.1:** `Invoke-WebRequest` без `-UseBasicParsing` зависает; кириллица в консоли
  искажается (читать файлы инструментом); первый вывод команды иногда «клеит» символ — писать в
  файл (`Out-File -Encoding utf8`) и читать; `if/else` не ставить в конвейер напрямую.
* **`bash` — только Git Bash**; LF для `*.command` — `.gitattributes` (`text eol=lf`).
  `*.ps1` — **ASCII-only** (PS 5.1 читает BOM-less .ps1 как ANSI).
* Артефакты — `tools/parity/out/` (гитигнор); коммиты атомарные (`TR/N: …`, `docs(transcribe): …`);
  решения §«Финальные решения» **не менять**; чужие незакоммиченные правки не трогать.
* Ядро — Rust; любое изменение поведения — через тесты + живой прогон.

## Первый шаг в act-режиме
1. Оформить **`PLAN_TRANSCRIBE_SERVICE.md`** (образец `PLAN_M9_CLIENT_PACKAGE.md`), включив
   факты Ф1–Ф15, целевую архитектуру, **финальные решения** и треки R0–R5 с приёмкой.
2. R0 — сброс dev-стенда (разрешено).
3. Далее R1 → R5 с тестами и живой приёмкой.

