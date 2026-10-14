# TRANSCRIBE_SERVICE_HANDOFF_PROMPT_2.md — промт для нового чата (продолжение: хвосты R2.5/R3.8/R3.9 + R4/R5)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, состояние, что уже сделано,
> что осталось, точки входа, контракты и грабли окружения.
>
> **Контекст.** Идёт этап «Автотранскрибация+диаризация как служба пакета» (план —
> `PLAN_TRANSCRIBE_SERVICE.md`, ветка `server-multiuser`). Первый заход (14.10.2026) выполнил
> **R0, R1, R2 (кроме R2.5), R3.1–R3.7, R3.10** — всё проверено (`cargo test --workspace` зелёный,
> clippy `-D warnings`/fmt чисты, живой CLI `--help`/`--dry-run`). **Задача второго захода:**
> закрыть хвосты **R2.5, R3.8, R3.9** и пройти **R4 (пользовательский UI)** и **R5 (приёмка/релиз/доки)**.

## Состояние репозитория

* Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**, HEAD `a2a7b0e`
  (tag `v0.19.0`).
* **Рабочее дерево содержит незакоммиченные правки первого захода** (20 изменённых файлов +
  `PLAN_TRANSCRIBE_SERVICE.md`, `TRANSCRIBE_SERVICE_HANDOFF_PROMPT.md`). Их **не откатывать**;
  коммит — по вашему решению (атомарные коммиты `TR/N: …`, `docs(transcribe): …`).
* Dev-стенд репозитория сброшен (`index.db`, `*.lock`, `index.pause`, `data/auto-transcribe*` удалены).
* **Отдельный установленный стенд НЕ трогать:** `C:\Users\Sasha\hds-server` + `C:\ProgramData\hermes-disk-search`
  (SCM-службы `hermes-disk-search-*`, `index.db` ~99 МБ, профиль `server`) — службы из неадминской
  сессии не останавливаются. Репозиторий и установленный стенд независимы (`target\debug`/`release`
  свободны для сборки).

## Станция
Windows, RTX 3060 12 ГБ, AMD Radeon iGPU, Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1. Ядро — Rust
(`crates/`: hds-core, hds-cli, hds-index, hds-llama, hds-mcp, hds-ui, hds-search, hds-extract, hds-clip);
Python — только sidecar. Сессия Cline — **не админ**. Git Bash — `C:\Program Files\Git\bin\bash.exe`.
Артефакты — `tools/parity/out/` (гитигнор). Это **dev-машина** (`cline-rules/dev-machine.md`).

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`   # 20 M + 2 ??
2. Прочитать: **`PLAN_TRANSCRIBE_SERVICE.md`** (§4 R0–R5 — статусы, §3 контракты, §8 решения),
   `PLAN_AUTO_TRANSCRIBE.md` (устройство демона/UI), `README_SERVER_MULTIUSER.md` (доступ/токены/прокси),
   `PLAN_M9_CLIENT_PACKAGE.md` (тонкий клиент — переиспользуется в польз. UI).
3. `cargo test --workspace` (живые SCM-службы repo не держат — `target` свободен); затем
   `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
4. Живая проверка CLI: `target\debug\hds.exe transcribe-watch --help` (rc 0), `… --dry-run`.

## Что уже сделано (первый заход — не переделывать)
* **R1:** медиа пропускается в `pipeline::extract_file` как `skipped_media`; `MediaRouter` убран из
  боевого пути (CLI/MCP/UI → `&sidecar`); `index.transcribe` — legacy (пункт `index-transcribe` в `hds check`).
* **R2:** `pipeline::implicit_excludes()` (inbox/out/state_dir) + подмешивание в `index_filter`;
  `/api/config` отдаёт `implicit_excludes`; установщики пишут `exclude_paths` + `auto_transcribe`;
  пункт `auto-transcribe-dirs` в `hds check`.
* **R3:** `ROLES` из 5 ролей (+`transcribe`); `run_role_blocking`; `--help`/`--dry-run` у `transcribe-watch`;
  `models_roots()` (env `HDS_MODELS_DIR` → ProgramData → APPDATA); установщик ставит роль + качает
  sortformer + SCM `depend= …-llm-host`; порядок stop/start в `update.ps1`; `wait_for_owner` + кооперативная
  остановка (`transcribe.stop`); пункт `transcribe-daemon` (залипший lock).

## Осталось сделать

### R2.5 — UI-индикатор «исключена из индексации» (мелкий)
- В админ-UI (закладка «Транскрибация», форма папок) рядом с `inbox_dir`/`out_dir` показать
  ✓/✗ «исключена из индексации» на основе `implicit_excludes` из `/api/config` (уже отдаётся).
- Точки: `crates/hds-ui/src/page.rs` (HTML/JS вкладки «Транскрибация»), `crates/hds-ui/src/config_edit.rs`
  (`get_config` уже содержит `implicit_excludes`).

### R3.8 — desktop-ветка установки (мелкий)
- `install_autostart.ps1` уже создаёт задачу `HermesDiskSearchTranscribe` — ок.
- Добавить в `setup.ps1` (≈стр. 250–275) блок «Transcription service»: запрос whisper+sortformer через
  `-ModelsDir` (`fetch_whisper_model.ps1`, `fetch_diarization_model.ps1`) + предложить задать `inbox_dir`/`out_dir`.
- `*.ps1` — **ASCII-only** (PS 5.1 читает BOM-less .ps1 как ANSI).

### R3.9 — UI-карточка демона (мелкий)
- Карточка «Демон автотранскрибации»: в `profile: server` действия идут в SCM
  (`hds service start|stop transcribe`), в desktop — как сейчас (detached процесс/задача); в ответе —
  признак формы (service/process/task). Точки: `crates/hds-ui/src/{lib.rs,transcribe.rs}`,
  `crates/hds-core/src/service.rs` (статус роли `transcribe` уже виден в `GET /api/services`, Ф3).

### R4 — пользовательский (не админский) UI (крупный трек)

**Задача.** Авторизованный **не-админ** с ноутбука получает ровно три функции: поиск, установка тонкого
клиента, транскрипты/спикеры (замена спикеров **и** правка `.md`). Админские функции (config/services/
access/index/diagnostics) — только админ.

**R4.1 — логин по паролю в UI.**
- `POST /api/auth/login {login,password}` → `hds_core::access::authenticate(&acc, login, password)`
  (`crates/hds-core/src/access.rs:255`), затем `new_token(user, ttl_days, now)` (там же `:201`);
  `token_ttl_days(cfg)` — `access.rs:185`.
- Ответ как у MCP (`crates/hds-mcp/src/http.rs::handle_login`, `:172`): `{token, expires, user, projects}`
  (+ полезно `admin`); 401 при неверных данных; 400 на битый JSON. **Плейн-пароль/токен не логировать.**
- Rate-limit антибрутфорс: `hds_core::ratelimit::Limiter`, отдельный жёсткий `login_per_min` по IP —
  смотри, как это делает MCP (`crates/hds-mcp/src/auth.rs`).
- Аудит: `login_ok`/`login_fail` (без пароля; как в MCP). `AuditEvent` — `hds-core/src/audit.rs`.

**R4.2 — разделение гейтов.** Сейчас `ui_auth_gate` (`crates/hds-ui/src/lib.rs:1602`) пускает только
`ctx.admin` (иначе 401), и на нём висит весь `route_with_access` (`:1703`). Ввести:
- `ui_user_gate` — любой **валидный** токен (админ/не-админ) → пускает `/ui/user/` и «безопасные» API
  (поиск, транскрипты/спикеры, клиентский пакет);
- `ui_auth_gate` (админ) — оставить для `/api/config`, `/api/services`, `/api/access/*`, `/api/index/*`,
  `/api/diagnostics` и корневой админ-страницы `/`;
- не-админ на админском API → **403** (+аудит); без токена → **401**.
- Браузер не шлёт `Authorization` при навигации — первый GET приходит с `?token=`; фротенд (`page.rs`)
  кладёт токен в `localStorage` и подставляет заголовок (механика уже есть — переиспользовать).

**R4.3 — страница пользователя `/ui/user/`.** Новая константа `PAGE_USER` (`crates/hds-ui/src/page.rs`),
не мешать админской `PAGE`. Шапка с логином/выходом; блоки: (а) «Поиск» (существующий `/api/search`
под user-гейтом; выдача фильтруется проектами токена), (б) «Установка клиента», (в) «Транскрипты/Спикеры».

**R4.4 — установка клиента.** Переиспользовать публичные маршруты MCP:
- `/mcp/client-install.ps1` | `/mcp/client-install.sh` (публично, без токена) —
  `hds_core::client::install_script_route(mcp_path, file)`, `INSTALL_SCRIPTS` (`crates/hds-core/src/client.rs:185`);
- скачивание пакета `/mcp/client-package.zip?platform=windows|macos` — `package_route`/`package_url_platform`
  (`client.rs:175`); за гейтом токена;
- показать адрес сервера и сверить `version` клиента/сервера (`GET /health`); текст «что дальше» —
  перезапустить Cline. Логика мастера — `installers/client-install.ps1`, `hds cline-sync`.

**R4.5 — спикеры и правка `.md`.** Переиспользовать `crates/hds-ui/src/transcribe.rs` (маршруты
`/api/transcribe/{list,file,speakers,apply}` — `crates/hds-ui/src/lib.rs:1736-1748`): list/превью,
path-safety внутри `out_dir`, `.bak`, sidecar. **Права (решение №5):** авторизованный не-админ может
и заменять спикеров, и править `.md` (per-user карту имён НЕ делать). Под user-гейтом.

**R4.6 — reverse-proxy.** Наружу `/mcp` и `/ui` (+ `/health`, `/mcp/client-install.*` публично);
фасад `:8010–8012` не публикуется. Обновить `installers/proxy/{Caddyfile,nginx.conf}` и
`README_SERVER_MULTIUSER.md`.

**R4.7 — единый контракт с `hds login` (CLI).** Тот же формат тела/ответа/ошибок, что в
`crates/hds-cli/src/cmd/auth.rs`.

**Тесты R4:** логин (успех/401/400), `403` не-админу на админском API, `401` без токена, rate-limit
логина; правка `.md` не-админом (`.bak`, path-safety); страница `/ui/user/` содержит три блока.
Образцы тестов: `crates/hds-ui/tests/ui_auth.rs`, `ui_core.rs`, `crates/hds-mcp/tests/audit.rs`.

### R5 — приёмка, релиз, доки
- Живая приёмка (`PLAN_TRANSCRIBE_SERVICE.md` §5): SCM-служба `hermes-disk-search-transcribe` `RUNNING`
  после перезагрузки; файл в `inbox_dir` → `.md` c `SPEAKER_00…`/`UNASSIGNED` в `out_dir`; `watch`/`mcp-http`/`ui`
  живы; VRAM-порядок «поиск/чат → transcribe → индексация»; повторный запуск не создаёт второго демона;
  `sc stop` не оставляет `transcribe.lock`/`index.pause`.
- Релизная сборка (`installers/build_rust_release.ps1`), бамп версии, `RELEASE_NOTES`.
- Доки: `README.md`, `README_SERVER_MULTIUSER.md`, `STATUS.md`, `config.example.yaml` (разделы
  «Транскрибация» правятся по факту: медиа вне индексации; `index.transcribe` legacy; ручной перенос `.md`).
- `PLAN_AUTO_TRANSCRIBE.md` **не переписывать** (в силе, кроме `index_outputs` и `index.transcribe`).

## Правила и грабли окружения (важно, платил временем)
* **Не трогать установленный стенд** `C:\Users\Sasha\hds-server` / `C:\ProgramData\hermes-disk-search` и
  его SCM-службы (из неадминской сессии не остановить).
* Живых процессов из `target\` репозитория сейчас нет → сборка/`cargo test` свободны; если поднимешь
  свои — **вернуть** после работ (dev-машина позволяет).
* **`HDS_CONFIG`/`HDS_ROOT` переживают между элементами `run_commands`** — задавать и **очищать**.
* Плейн-токены/пароли/секреты не печатать в stdout/логи/аудит.
* **PowerShell 5.1:** `Invoke-WebRequest` без `-UseBasicParsing` зависает; кириллица в консоли искажается
  (читать файлы инструментом, не эхом); `*.ps1` — ASCII-only; `if/else` не в конвейер.
* `bash` — только Git Bash; LF для `*.command`/`*.sh`. Артефакты — `tools/parity/out/` (гитигнор).
* Решения §8 `PLAN_TRANSCRIBE_SERVICE.md` **не менять**; чужие незакоммиченные правки (в т.ч. первого
  захода) не откатывать.
* Ядро — Rust; `cargo fmt --all` перед сдачей; `cargo test --workspace` + `clippy -D warnings` — обязательны.

## Порядок работ
1. Хвосты: **R2.5 → R3.8 → R3.9** (мелкие, быстро).
2. **R4** по шагам R4.1 → R4.7 с тестами (основной объём).
3. **R5**: живая приёмка, релиз, доки. По ходу — обновлять чекбоксы в `PLAN_TRANSCRIBE_SERVICE.md`.
