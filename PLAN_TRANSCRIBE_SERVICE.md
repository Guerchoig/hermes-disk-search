# PLAN_TRANSCRIBE_SERVICE.md — детальный план этапа «Автотранскрибация+диаризация как служба пакета»

> Продолжает `PLAN_AUTO_TRANSCRIBE.md` (там автотранскрибация реализована как **desktop-демон**
> + задача Планировщика) и дополняет `PLAN_SERVER_MULTIUSER.md` (§2 W8/Э10 — смежный этап).
> Повод: отчёт о том, что автоматическая транскрибация+диаризация **не согласована с остальными
> службами пакета и не ставится серверным установщиком**; ASR по медиа также остаётся в конвейере
> индексации и не нужна там.
> **Статус:** запланирован 14.10.2026. Ветка — **`server-multiuser`**, HEAD `a2a7b0e` (tag
> `v0.19.0`). **Объём и решения согласованы владельцем 14.10.2026** (см. §8). Промт для старта —
> `TRANSCRIBE_SERVICE_HANDOFF_PROMPT.md`.
>
> **Прогресс 14.10.2026 (второй заход):** выполнены **все пункты R0–R5** — хвосты первого захода
> (R2.5, R3.8, R3.9), весь R4 (пользовательский UI: логин, два гейта, кабинет `/ui/user/`,
> установка клиента, транскрипты/спикеры, reverse-proxy) и R5 (живая приёмка кабинета, релиз
> 0.20.0, доки). Проверки: `cargo test --workspace` (0 падений), clippy `-D warnings`/fmt чисты;
> живые прогоны — `tools/parity/out/r5/` (`check.txt`, `watch_help.txt`, `watch_dryrun.txt`,
> `live/results.txt`). **Не выполнена** только приёмка SCM-службы из неадминской сессии
> (§5 п.3 — нужна администраторская сессия; установленный стенд `%ProgramData%` не трогали).
>
> **Зависимости:** `PLAN_AUTO_TRANSCRIBE.md` (демон, конфиг `auto_transcribe.*`, контракт
> `/internal/transcribe`, UI «Транскрибация»), `PLAN_SERVER_MULTIUSER.md` (M1 доступ/токены,
> M2 профиль `server`, M7 UI-контроль, Э8 серверный пакет), `PLAN_M9_CLIENT_PACKAGE.md`
> (тонкий клиент — переиспользуется в польз. UI).
> **Ядро — Rust**; любое изменение поведения — через тесты + живой прогон.

## 1. Вводные (факты из кода, а не предположения)

| # | Факт | Где |
|---|---|---|
| Ф1 | Ролей служб **ровно 4**: `ROLES = [watch, mcp-http, ui, llm-host]`; тест фиксирует `ROLES.len() == 4` | `crates/hds-core/src/service.rs:27`, тесты |
| Ф2 | `hds service run <role>` — единая точка; маппинг роль→функция в `run_role_blocking`; демона транскрибации там нет | `crates/hds-cli/src/cmd/service.rs:55-66` |
| Ф3 | `GET /api/services` UI **итерирует `ROLES`** — 5-я роль появится в карточке автоматически | `crates/hds-ui/src/lib.rs::services_json_profile` |
| Ф4 | Серверный установщик ставит `-Roles "watch,mcp-http,ui,llm-host"`; `-Fetch` тянет `engine\|llm_models\|whisper\|clip` (sortformer **не** качается); конфиг генерит `New-ServerConfig` (без `auto_transcribe`/исключений) | `installers/install_server.ps1:25,159,68-96`; `install_server.command:95+` |
| Ф5 | Desktop-автозапуск **уже** создаёт задачу `HermesDiskSearchTranscribe` (`hds transcribe-watch`); `install_server.ps1` её удаляет | `install_autostart.ps1:48`, `installers/install_server.ps1:137` |
| Ф6 | Подкоманда `transcribe-watch` **игнорирует остальные аргументы** — поэтому `--help` запускал демон (баг) | `crates/hds-cli/src/main.rs:123` |
| Ф7 | ASR в индексации живёт в `MediaRouter` (kind==`media` → `/internal/transcribe`); подставляется в 4 боевых точках: CLI `index`/`watch`, MCP `tools.rs` (×2), UI `index_bg` | `hds-index/src/transcribe.rs:703-773`; `hds-cli/src/support.rs:84-91`; `cmd/{index,watch}.rs`; `hds-mcp/src/tools.rs:343,370`; `hds-ui/src/lib.rs:419` |
| Ф8 | Ключ `index.transcribe: true` по умолчанию; `index.whisper_*`/`transcribe_url` — там же | `config.example.yaml:20-30` |
| Ф9 | `index.exclude_paths` — обычный список, редактор сохраняет его из UI; дефолт пуст | `config.example.yaml:14`, `hds-ui/src/config_edit.rs` |
| Ф10 | Демон: `transcribe.lock` (`WatchLock::acquire_named`), очередь JSONL, манифест, `source_disposal: delete\|keep\|move`, `PauseGate` (`index.pause`) | `hds-index/src/autotranscribe.rs:656,633-635,954` |
| Ф11 | Модели ищутся **только** в `%APPDATA%\OpenResearchTools\models` (whisper — `hds-llama/src/host.rs:993`; sortformer — `host.rs:1022`, `hds-index/src/autotranscribe.rs:566`); у каталога **движка** есть конфиг-ключ и env `HDS_ENGINE_DIR` | `crates/hds-llama/src/engine_dir.rs:42` |
| Ф12 | Fetch-скрипты моделей параметризованы `-ModelsDir` (whisper и sortformer) | `installers/fetch_whisper_model.ps1:16-31`, `installers/fetch_diarization_model.ps1:18-33` |
| Ф13 | UI сейчас **только админский**: `ui_auth_gate` пускает лишь `ctx.admin`; логина по паролю в UI нет (есть в MCP `POST /auth/login`) | `hds-ui/src/lib.rs:1597-1695`; `hds-mcp/src/auth.rs`; `hds-core/src/access.rs::authenticate/new_token` |
| Ф14 | Страница транскрибации + присвоение имён **уже реализованы** (`list/file/speakers/apply`, path-safety внутри `out_dir`, `.bak`, sidecar) — переиспользовать под пользовательским гейтом | `hds-ui/src/{transcribe.rs,page.rs:165-215}`, `route` (`lib.rs:1737-1750`) |
| Ф15 | Тонкий клиент и его установка **уже есть и публикуются сервером**: `/mcp/client-install.ps1\|sh`, `/mcp/client-package.zip?platform=`; мастера сверяют `version` из `/health` | `hds-mcp/src/http.rs:382,478,1002`; `installers/{client-install.ps1,build_client_release.ps1}`; `hds-core/src/client.rs` |

## 2. Область

**Входит:**
- **R1.** Медиа **полностью исключается** из конвейера индексации (ранний `skipped_media`); ASR
  уходит из боевого пути индексации; ключ `index.transcribe` — legacy.
- **R2.** `inbox_dir`/`out_dir`/`state_dir` **неявно исключаются** из индексации; дефолты конфига
  и установщиков; проверка `hds check`.
- **R3.** `transcribe-watch` становится **5-й ролью службы** пакета (`ROLES`), с SCM-зависимостью от
  `llm-host`; починка CLI-контракта (`--help`/`--dry-run`); машинный каталог моделей
  (`%ProgramData%` + `HDS_MODELS_DIR`); установщики; безопасный порядок обновления.
- **R4.** **Пользовательский (не админский) UI**: логин по паролю → токен; страница пользователя
  (поиск + установка тонкого клиента + транскрипты/спикеры); разделение гейтов.
- **R5.** Живая приёмка, релизная сборка, доки.

**Не входит:**
- Доставка транскриптов в индекс (решение владельца №1): `index-queue.jsonl`, `index.no_walk_paths`,
  требование «`out_dir` внутри `index.roots`» — **не делаем**; перенос `.md` из `out_dir` в
  индексируемые папки — **вручную**, вне службы.
- Кнопка-помощник «перенести в индексируемую папку» (решение №2).
- Ключ `index.media_metadata` и индексация метаданных медиа (решение №3).
- Гейт `confirm_delete`/`move` для исходника (решение №4) — удаление безусловно, как сейчас.
- Per-user карта имён спикеров и «правка общего файла только админом» (решение №5) — отменяются.
- Изменения формата меток/вывода движка (`SPEAKER_00…`, `.md`) — фиксированы в
  `PLAN_AUTO_TRANSCRIBE.md` §5.4.
- Эксплуатация healthz/бэкап (Э10), анонимизация (Э6), нагрузочная приёмка (Э11).

## 3. Контракты (код — источник истины)

* **Роли служб:** `ROLES = [watch, mcp-http, ui, llm-host, transcribe]` (5 ролей).
  `parse_role("transcribe")` валиден; `service_name`/`daemon_label`/SCM-имя —
  `hermes-disk-search-transcribe`; SCM `depend= hermes-disk-search-llm-host`. Тесты на количество
  ролей обновляются осознанно (Ф1).
* **CLI `hds transcribe-watch`:** без флагов — демон, как сейчас (нужен службе/задаче);
  `--help`/`-h` → usage и код `0` **без запуска**; `--dry-run` — проверка конфига/моделей/владельца
  без работы (Ф6 — фикс обязателен).
* **Исключения индексации:** единственный ключ `index.exclude_paths` («не обходить и не
  индексировать»). Рантайм неявно добавляет к нему `auto_transcribe.inbox_dir`, `out_dir`,
  `state_dir`; неявные значения отдаются в `/api/config` (`implicit_excludes`) и в `hds check`.
* **Медиа в индексации:** `Early { status: "skipped_media", kind: Some("media") }` сразу после
  определения `kind` — ни стата, ни `content_hash`, ни эмбеддингов, ни обращений к
  `/internal/transcribe`.
* **`index.transcribe`:** legacy — при `true` предупреждение в лог и `hds check` («ASR в индексации
  отключена, используйте `auto_transcribe`»); поведение как при `false`; дефолт шаблона — `false`.
* **Модели (приоритет):** `auto_transcribe.whisper_model`/`diarization_model` (абс. пути) → env
  `HDS_MODELS_DIR` → `%ProgramData%\hermes-disk-search\models` → `%APPDATA%\OpenResearchTools\models`.
* **Демон:** `source_disposal: delete` — безусловно, без флагов (решение №4); `pause_indexing`
  сохраняется (VRAM-приоритет); единственный демон (`transcribe.lock` + проверка живого PID);
  на старте — ожидание владельца (`/health`/проба порта с backoff); корректное снятие
  `transcribe.lock`/`index.pause` при остановке.
* **UI-гейты:** `ui_user_gate` — любой валидный токен (для страницы пользователя и «безопасных»
  API); `ui_auth_gate` (админ) — для config/services/access/index/diagnostics. Не-админ на
  админском API → `403`; без токена → `401`. Логин `POST /api/auth/login {login,password}` →
  `hds_core::access::authenticate` + `new_token` (TTL `access.token_ttl_days`); rate-limit; аудит
  `login_ok`/`login_fail`; плейн-пароли/токены в логи не писать.
* **Права на транскрипты:** авторизованный не-админ может и заменять спикеров, и править `.md`
  (переиспользуя `hds-ui/src/transcribe.rs`: path-safety внутри `out_dir`, `.bak`) — решение №5.
* **Установка клиента:** переиспользовать публичные `/mcp/client-install.ps1\|sh` и
  `/mcp/client-package.zip?platform=…`; сверка `version` из `/health` (Ф15).
* **Совместимость:** desktop-профиль сохраняет поведение (задача Планировщика; демон как процесс);
  server-профиль — служба SCM; изменения аддитивны, чужие контракты не ломаются.

## 4. Треки R0–R5

### R0. Сброс dev-стенда (разрешено, `cline-rules/dev-machine.md`) — 0,5 дн — ✅ **выполнено 14.10.2026**
- [x] Остановить службы/задачи: SCM-службы `hermes-disk-search-*` (установленный стенд `C:\Users\Sasha\hds-server`)
      из неадминской сессии не останавливаются — **оставлены как есть** (отдельный стенд, не репозиторий).
- [x] Удалены стендовые данные репозитория: `index.db`, `index.pause`, `index.heartbeat.json`,
      `transcribe.lock`, `watch.lock`, `data/auto-transcribe*` (все lock — устаревшие PID). `D:\media-out`
      не тронут (вне репозитория, содержит документы «встреч»).
- [x] Гигиена мусора в корне: `bt.log`, `clip_*.log`, `ui_*.log`, `stop_log.log`, `test_*.log`, `w_err.log`, `_clip_*.log`.

### R1. Исключить транскрибацию из индексации — 1–1,5 дн — ✅ **выполнено 14.10.2026**
- [x] **R1.1** Ранний пропуск медиа в `extract_file` (`skipped_media`) — до стата/хэша/эмбеддингов.
      `crates/hds-index/src/pipeline.rs`.
- [x] **R1.2** Убран `MediaRouter` из боевого пути: `build_media_extractor` упразднён, CLI/MCP/UI
      передают `&sidecar` как `&dyn Extractor` (`support.rs`, `cmd/{index,watch}.rs`,
      `hds-mcp/src/tools.rs`, `hds-ui/src/lib.rs`).
- [x] **R1.3** `MediaRouter`/`transcribe_media`/`lead_segment` **оставлены** только для `hds whisper-check`/проб
      (вариант «оставить», не удалять).
- [x] **R1.4** `index.transcribe` → legacy (предупреждение в `run_index` + пункт `index-transcribe` в
      `hds check`); дефолт шаблона — `false`. `config.example.yaml`, `hds-index/src/diag.rs`, `pipeline.rs`.
- [x] **R1.5** Диагностика: `skipped_media`-статус в `/api/diagnostics` (пункт `index-transcribe`).
- [x] Тесты: кейсы `media_skipped_before_extractor` и `implicit_excludes_*` (`pipeline_core.rs`);
      `transcribe_media.rs` не тронут (MediaRouter оставлен — так и задумано).
- ⚠ Признать явно: исходники медиа больше не находятся поиском по имени.

### R2. Дефолтные исключения inbox/out — 0,5 дн — ✅ **выполнено (кроме R2.5)**
- [x] **R2.1** Неявное (рантайм) исключение в `pipeline::index_filter`: `auto_transcribe.inbox_dir`,
      `out_dir`, `state_dir` + `implicit_excludes()` (отдаётся в `/api/config`).
- [x] **R2.2** `config.example.yaml`: `index.exclude_paths` — с закомментированными подсказками.
- [x] **R2.3** Установщики: `install_server.ps1` (`-MediaInbox`, `-MediaOut`, `-EnableTranscribe`,
      `-SourceDisposal`; `index.exclude_paths` + `auto_transcribe` в `New-ServerConfig`) и паритет в
      `install_server.command`.
- [x] **R2.4** Проверка `hds check`/`/api/diagnostics`, пункт `auto-transcribe-dirs` (абс., существуют,
      различны, не вложены, исключены; иначе `warn`).
- [x] **R2.5** UI-индикатор «исключена из индексации ✓/✗» у полей папок: значения отдаются
      в `/api/config` (`implicit_excludes` + `exclude_paths`), индикатор обновляется при вводе
      (`trInd`/`trNorm` в `page.rs`). Кнопку-помощник «перенести в индексируемую папку» **не делаем** (решение №2).

### R3. Транскрибация как полноправная служба — 3–4 дн
- [x] **R3.1** `ROLES` += `"transcribe"` (5 ролей); тесты (`roles_and_names`, `ROLES.len() == 5`)
      обновлены; UI-карточка получит роль автоматически (Ф3). `crates/hds-core/src/service.rs`.
- [x] **R3.2** `run_role_blocking`: `"transcribe" => …cmd_transcribe_watch()`; usage-тексты обновлены.
      `crates/hds-cli/src/cmd/service.rs`.
- [x] **R3.3** **CLI-контракт починен** (Ф6): `transcribe-watch --help/-h` → справка, код 0, без запуска
      (проверено живым запуском); `--dry-run` (проверка каталогов/модели/владельца). `main.rs`,
      `cmd/transcribe.rs`; тест `watch_cli_help_exits_without_running`.
- [x] **R3.4** **Машинный каталог моделей**: приоритет env `HDS_MODELS_DIR` → `%ProgramData%\hermes-disk-search\models`
      → `%APPDATA%\OpenResearchTools\models` (`models_roots()` в `hds-llama/src/host.rs` и
      `hds-index/src/autotranscribe.rs`); абс. пути из конфига — приоритетнее (как раньше).
- [x] **R3.5** **Установщик сервера**: `-Roles` по умолчанию `watch,mcp-http,ui,llm-host,transcribe`;
      `-Fetch` += `fetch_diarization_model.ps1`; `New-ServerConfig` пишет `index.exclude_paths` +
      `auto_transcribe`; SCM-зависимость `depend= hermes-disk-search-llm-host` (`sc_install_raw`, тест).
      Паритет в `install_server.command`.
- [x] **R3.6** Порядок ролей в обновлении: stop — `transcribe` до `llm-host`; start — `transcribe`
      после `llm-host`. `installers/update.ps1`.
- [x] **R3.7** Демон: на старте ждёт владельца (проба порта с backoff, до 60 с); `sc.exe stop` для
      `transcribe` → кооперативная остановка через `transcribe.stop`. `autotranscribe.rs`, `cmd/service.rs`.
- [x] **R3.8** Desktop-ветка: `install_autostart.ps1` создаёт задачу `HermesDiskSearchTranscribe` (Ф5);
      блок **«Automatic transcription»** в `setup.ps1`: запрос whisper + sortformer через
      `-ModelsDir` (`HDS_MODELS_DIR` учитывается), затем `inbox_dir`/`out_dir` (+ `enabled`) пишутся
      в `config.yaml` строковым редактором `Set-ConfigScalar` (комментарии строк сохраняются,
      проверено на копии `config.example.yaml`: 3 изменённые строки из 393).
- [x] **R3.9** UI-карточка «Демон автотранскрибации»: в ответе — **форма** управления
      (`form: service|task|process`, `task`, `service` = состояние роли SCM), действия в
      server-профиле уходят в SCM (`hds service … transcribe`, restart = stop → wait → start).
- [x] **R3.10** `hds check`: пункт `transcribe-daemon` (залипший `transcribe.lock` без живого PID). `diag.rs`.
- [x] Тесты: `parse_role("transcribe")`/SCM-`depend`/`ROLES.len()==5`; `--help`; `--dry-run` (живой);
      приоритет путей моделей (`models_roots`); залипший lock (косвенно).

### R4. Пользовательский UI (не админский) — 2,5–3,5 дн
- [x] **R4.1** **Логин по паролю в UI**: `POST /api/auth/login {login,password}` →
      `hds_core::access::authenticate` + `new_token` (TTL `access.token_ttl_days`), ответ
      `{token,user,expires,admin,projects}`; строгий rate-limit по IP (`login_per_min`, ёмкость 5);
      аудит `login_ok`/`login_fail`/`token_issue`. `ui_login` в `hds-ui/src/lib.rs`.
- [x] **R4.2** **Разделение гейтов**: `ui_user_gate` (любой валидный токен) для страницы
      пользователя и «безопасных» API (`is_user_route`); `ui_auth_gate` (админ) — для config/
      services/access/index/diagnostics/`transcribe/daemon`/ask; не-админ → `403` (+аудит `forbidden`).
- [x] **R4.3** **Страница пользователя** (`/ui/user/`, константа `PAGE_USER`): шапка с
      пользователем/выходом; блоки: (а) «Поиск» (под user-гейтом, выдача фильтруется проектами
      токена — `project_filter`), (б) «Установка клиента», (в) «Транскрипты/Спикеры».
- [x] **R4.4** **Установка клиента**: ссылки на `/mcp/client-install.ps1|sh` (токен аргументом),
      ссылка на `/mcp/client-package.zip?platform=…`, адрес сервера и сверка `version` (`/health`);
      переиспользован `hds-core::client` (`/api/user/client`).
- [x] **R4.5** **Спикеры/правка `.md`** — переиспользован `hds-ui/src/transcribe.rs`
      (list/превью/`speakers`/apply, path-safety внутри `out_dir`, `.bak`, sidecar). **Права
      (решение №5): авторизованный не-админ может и заменять спикеров, и править `.md`** (тест).
- [x] **R4.6** Reverse-proxy: наружу `/mcp`, `/ui` и публичная проба `/health`
      (`client-install.*` публично на уровне приложения); фасад `:8010–8012` не публикуется.
      `installers/proxy/{Caddyfile,nginx.conf}`, `README_SERVER_MULTIUSER.md` §1a/§6/§11.
- [x] **R4.7** `hds login` (CLI) — единый контракт пароль→токен с UI и MCP (`hds-cli/src/cmd/auth.rs`).
- [x] Тесты: `crates/hds-ui/tests/ui_user.rs` — логин (200/401/400), `403` не-админу, `401` без
      токена, rate-limit (`429`), правка `.md` не-админом (`.bak`, sidecar, path-safety),
      состав `PAGE_USER`; обновлены `ui_core.rs`/`ui_auth.rs` (гейты).

### R5. Приёмка и релиз — 1 дн
- [x] Живая приёмка на стенде (см. §5): **UI-часть выполнена** (`tools/parity/out/r5/live_ui.ps1`
      → `results.txt`); SCM-часть (перезагрузка/SYSTEM-служба) на dev-машине не выполнялась —
      из неадминской сессии `sc.exe start/stop` недоступны (см. §5 п.3, помечен `[ ]`).
- [x] Релизная сборка `installers/build_rust_release.ps1 -Version 0.20.0` (22.9 с, release
      профиль): `dist\hds-0.20.0-windows-x64{,.zip,.zip.sha256.txt}` +
      `dist\hds-client-0.20.0-windows-x64{,.zip,…}`, `version.txt = 0.20.0`, в `client\` —
      оба скрипта-бутстрапа и клиентский пакет. Версия — `hds/__init__.py` 0.20.0,
      заметки — `RELEASE_NOTES_v0.20.0.md`.
- [x] Доки: `README.md` (блок установщика «Automatic transcription», раздел «Автотранскрибация»:
      служба/роль, media вне индексации, `index.transcribe` legacy, ручной перенос `.md`,
      кабинет `/ui/user/`), `README_SERVER_MULTIUSER.md` (§1a «два гейта», §6 кабинет, §10
      шпаргалка, §11 публикация `/mcp`+`/ui`+`/health`), `STATUS.md` (сводка этапа),
      `config.example.yaml` (`auto_transcribe`: `out_dir` не индексируется, `index_outputs`
      legacy, кто запускает конвейер). **Разделы «Транскрибация» правлены по факту** (медиа
      вне индексации; `index.transcribe` legacy; ручной перенос `.md`).
- ⚠ `PLAN_AUTO_TRANSCRIBE.md` в части auto_transcribe остаётся в силе, **кроме** `index_outputs`
      (ключ deprecated: транскрипты переносятся в индексируемые папки вручную) и `index.transcribe`
      (legacy). Эти правки фиксируются в докладе R5, сам боевой план не переписывается.

**Итого: ~8–10 дней** одним исполнителем.

## 5. Приёмка
- [x] Прогон `hds index`/`watch` по корню с медиа: в БД нет `kind=media` (только `skipped_media`),
      в логе `llm-host` **нет** запросов `/internal/transcribe`, время прогона на медиа не растёт,
      не-медиа паритет не сломан (`pipeline_parity`). — **проверено живьём**
      (`tools/parity/out/r5/idx/`): песочница из 4 медиа (`mp4/mkv/mp3/wav`) + 3 текстовых файлов
      со своим `db_path`; `hds index` → rc 0, итог `{'indexed': 3, 'skipped_media': 4}`, каждая
      медиа — `-> skipped_media (0.0 с)`; `hds status` → `Файлы по типам: text=3` (ни одного `media`),
      `Чанков всего: 3`; в логе резидента (`%ProgramData%\hermes-disk-search\data\logs\llm-host.log`)
      запросов `/internal/transcribe` — **0** до и после прогона (лог вырос только на эмбеддинги
      текста). Заодно убрана устаревшая подсказка «извлечение аудио + Whisper-транскрипция,
      может занять несколько минут» (`pipeline.rs`): теперь «медиа вне индексации: пропускается,
      тексты даёт `hds transcribe-watch`». Паритет (`pipeline_parity`/`walk_parity`) зелёный.
- [x] Свежий конфиг (example/установщики): `inbox_dir`/`out_dir` исключены; в `hds check` оба пути
      видны как исключённые — **проверено живьём** (`tools/parity/out/r5/check.txt`:
      «каталоги автотранскрибации корректны и исключены», `transcribe-daemon` — «демон не запущен»;
      `--dry-run` даёт те же два пункта). При удалении из исключений проверка краснеет (логика
      `check_auto_transcribe_dirs` + `implicit_excludes` покрыта тестами `pipeline_core`).
- [ ] `sc.exe query hermes-disk-search-transcribe` → `RUNNING` после перезагрузки без входа
      пользователя; файл в `inbox_dir` → `.md` с `SPEAKER_00…`/`UNASSIGNED` в `out_dir`, строка в
      `data/auto-transcribe-queue.jsonl`; `watch`/`mcp-http`/`ui` живы; VRAM-порядок «поиск/чат →
      transcribe → индексация»; повторный запуск не создаёт второго демона; `sc.exe stop` не
      оставляет залипший `transcribe.lock`/`index.pause`. — **не выполнено: SCM-службы проекта
      на этой машине не установлены вовсе** (`sc query hermes-disk-search-{watch,mcp-http,ui,llm-host,transcribe}`
      → «служба не установлена»; `hds service status transcribe` → 1060), а `install`/`start` служб
      требуют админской сессии и профиля `server` — из неадминской сессии недоступно.
      Косвенно: `transcribe-watch --dry-run`/`--help` (rc 0), `hds check` (пункты `diarization`,
      `auto-transcribe-dirs`, `transcribe-daemon`), тесты SCM-установки (`sc_install_raw` с
      `depend= …-llm-host`), кооперативная остановка (`on_service_stop`, `transcribe.stop`), R3.5/R3.6/R3.10.
- [x] Пользовательский UI: с ноутбука логин/пароль → токен; видны только 3 функции (поиск,
      установка клиента, спикеры); админские API → `403`; без токена → `401`; не-админ может
      заменять спикеров **и** править `.md` (с `.bak`); установка клиента подключает Cline к
      внешнему серверу. — **проверено живьём** (`tools/parity/out/r5/live/results.txt`:
      логин 200 + токен, `401` без токена, `403` на `/`, `/api/config`, `/api/transcribe/daemon`,
      `/ui/user/` открывается с токеном и содержит три блока, правка `.md` не-админом
      `ok:true, replaced:1`, `.bak=true, sidecar=true, renamed_Ivan=true`, path-safety `../evil.md`
      → `ok:false`; rate-limit логина → `429` после исчерпания bucket; аудит
      `login_ok, token_issue, login_fail, unauthorized, forbidden, user_op`).
      Установку Cline-клиента на ноутбуке проверяет владелец (клиентский стенд — не эта машина);
      раздача скрипта/пакета сервером уже принималась в 0.19.0 (W7).

## 6. Тесты
* Юниты: `skipped_media`; неявные исключения; `parse_role("transcribe")`;
  SCM-install/`daemon_plist`/`ROLES` для роли; разбор `--help`/`--dry-run` у `transcribe-watch`;
  приоритет путей моделей (config→env→ProgramData→APPDATA); форма демона (`daemon_form`);
  BOM в `.md`; user-гейт (`crates/hds-ui/tests/ui_user.rs`, плюс обновлён `ui_auth.rs`):
  логин (200/401/400), `403` не-админу, `401` без токена, rate-limit (`429`), правка `.md`
  не-админом (`.bak`, sidecar, path-safety), состав `PAGE_USER` и индикаторы исключений.
* Интеграция/стенд: `cargo test --workspace` + `clippy --workspace --all-targets -- -D warnings`
  + `cargo fmt --all --check`; e2e: SCM-служба на тестовой inbox, UI через `curl` с токеном.

## 7. Риски
| Риск | Митигация |
|---|---|
| LocalSystem не видит модели (Ф11) | машинный каталог моделей + абс. пути в конфиге (R3.4); junction'ы не нужны |
| `source_disposal: delete` в службе = потеря данных | **принято владельцем** (№4): удаление безусловно; приёмка — на текущей политике |
| Служба стартует раньше `llm-host` | SCM `depend=` + ожидание владельца на старте (R3.5, R3.7) |
| Два демона (задача Планировщика + служба) | `install_server.ps1` удаляет задачи, `transcribe.lock`, проверка в `hds check` (R3.10) |
| Регресс паритета индексации | правки только на медиа-ветке; `pipeline_parity`/`walk_parity` в обязательном прогоне |
| Не-админ правит общий `.md` в `out_dir` | **принято владельцем** (№5): оригиналы медиа хранятся нетронутыми в другом месте; правки — с `.bak` |
| «Отказано в доступе» при пересборке | резидент/службы держат `target\{release,debug}\*.exe` — остановить и вернуть (правила окружения) |
| Раздел «Транскрибация» в доках устареет | R5 правит `README`/`STATUS` по факту; `PLAN_AUTO_TRANSCRIBE.md` не переписывается (§4 R5) |

## 8. Решения владельца (утверждены 14.10.2026 — менять нельзя)
1. **Службы не доставляют `.md` в индекс**; перенос из `out_dir` в индексируемые папки — вручную
   (нет `index-queue.jsonl`, `index.no_walk_paths`, требования «`out_dir` внутри `index.roots`»).
2. **Кнопка «перенести в индексируемую папку» — не нужна.**
3. **Ключ `index.media_metadata` — не нужен**; медиа полностью пропускается (`skipped_media`).
4. **Исходник аудио/видео удаляется безусловно, как сейчас** — без флагов (`confirm_delete` и пр.).
   Оригиналы медиа хранятся нетронутыми в другом месте (вне контура службы).
5. **Права пользовательского UI:** авторизованный не-админ может и заменять спикеров, и править
   `.md`; per-user карта имён и «правка общего файла только админом» отменяются.
6. **Форма роли службы — 5-я роль `transcribe`** в `ROLES` (SCM `hermes-disk-search-transcribe`,
   `depend= hermes-disk-search-llm-host`); в desktop — задача `HermesDiskSearchTranscribe`;
   `--help` у `transcribe-watch` чиним обязательно.

## 9. Ссылки
- `PLAN_AUTO_TRANSCRIBE.md` — текущее устройство демона/конфига/UI (кроме правок §4 R1.4/R5).
- `PLAN_SERVER_MULTIUSER.md` — §2 W8/Э10 (смежный этап), M1/M2/M7.
- `PLAN_M9_CLIENT_PACKAGE.md` — тонкий клиент (переиспользуется в польз. UI).
- Код: `crates/hds-core/src/service.rs`, `crates/hds-cli/src/{main.rs,cmd/service.rs,cmd/transcribe.rs,support.rs}`,
  `crates/hds-index/src/{pipeline.rs,diag.rs,autotranscribe.rs,transcribe.rs}`,
  `crates/hds-ui/src/{lib.rs,page.rs,transcribe.rs,config_edit.rs}`,
  `crates/hds-llama/src/{host.rs,engine_dir.rs}`, `installers/*.ps1`.
- Промт для старта — `TRANSCRIBE_SERVICE_HANDOFF_PROMPT.md`. Артефакты — `tools/parity/out/` (гитигнор).
