# AGENT_A4_A5_CODE_REVIEW_PROMPT.md — код-ревью фаз A4 и A5 (зеркало + вкладка «Агент»)

> **Кому:** новый чат Cline в режиме Act. **Дата среза:** 21.11.2026.
> **Ветка:** `agent-privacy-zone`, **HEAD:** `49913f73` (этот промт; код A4–A5 — на
> `75912a0e`, промт его не менял), дерево чистое, `origin/agent-privacy-zone` — **ahead 89**,
> push не делался.
> **Что это:** самодостаточный промт для **независимого код-ревью** двух последних фаз
> основного проекта `PLAN_AGENT_PRIVACY_ZONE.md` — **A4** (двустороннее зеркало
> `hds agent sync`) и **A5** (вкладка «Агент» в `hds ui`). Ревью A0–A3 уже было
> (`AGENT_A0_A3_CODE_REVIEW.md`, находки AG-1…AG-19 исправлены) — их **не переоткрывать**,
> нумерация новых находок начинается с **AG-20**.
> **Где готовый промт:** §14 (копировать целиком).

## 0. Задача

Проверить **фактический код** A4 и A5, а не отчёты о них:

1. найти дефекты и риски (логика, гонки, безопасность путей и секретов, соответствие
   требованиям плана TR-5.x/6.x/7.x/8.x и инвариантам I1–I8);
2. проверить, что заявленные в отчётах фазы (`AGENT_A4_REPORT.md`,
   `AGENT_A5_REPORT.md`) свойства **действительно** обеспечены кодом и тестами;
3. найти пробелы в тестах и воспроизвести то, что вызывает сомнение **своими**
   прогонами/пробами;
4. оформить `AGENT_A4_A5_CODE_REVIEW.md` в формате ревью A0–A3: вердикт по фазам,
   таблица независимых проверок, находки с severity и `file:line`, «что сделано
   хорошо», «соответствие плану», «порядок закрытия» (до A6 / фаза 2), вопросы владельцу.

Ревью — **диагностика**, а не рефакторинг: продуктовый код не менять, пока владелец
не согласился с находкой (правки — отдельным коммитом после согласования).

## 1. Состояние репозитория (проверено 21.11.2026)

| Что | Значение |
|---|---|
| Ветка / HEAD | `agent-privacy-zone`, `49913f73` (этот промт), код фаз — на `75912a0e`; ahead `origin/agent-privacy-zone` на 89, push не делался |
| Объём A4+A5 | `git --no-pager log --oneline 00da7c54~1..HEAD` — **39 коммитов**; `git --no-pager diff --shortstat 00da7c54~1..HEAD` — **68 файлов, +28 014 / −693**; только код (`crates/` + `sidecar/`) — **51 файл, +20 974 / −691** |
| Коммиты A4 | `00da7c54` (A4.1 реестр) → `81961936` (A4.2 прямой путь) → `d84571ee` (A4.3 обратный) → `43305867` (A4.4 решения) → `e676bb89` (A4.5 валидатор) → feat+docs A4.6 (демон, CLI, документация) |
| Коммиты A5 | `be398515` (A5.1) → A5.2 feat+docs → `fd2852b2` (A5.3 docs) → `b5daab0b`+`fb699982` (A5.4) → A5.5 feat+docs → `bc790bb4`+`82f07587` (A5.6) → `891385d8` (feat A5.7, CRLF) + `75912a0e` (docs A5.7) |
| Тесты (срез A5.7) | `cargo test --workspace --no-fail-fast -- --test-threads=4` — **128 наборов / 932 теста: 918 passed, 13 ignored**; единственный упавший — `agent_acceptance` (известный флейк `.cline`), **одиночным прогоном зелёный**; `npm test` 23/23; python **333 OK (skip 1)**; fmt/clippy чисто |
| Стенд | `tools\parity\out\agent_a42` — 34 оригинала, реестр **35 записей** (32 `synced`, 2 `accepted`, 1 `unsupported`), `anonymize.db` **47 меток** (`keep=46`, `aliases=0`, `merges=0`), индекс зоны `state\index.db` (34 файла), `mcp_http.port: 8788`, UI `8799` |
| Отчёты фаз | `AGENT_A4_REPORT.md`, `AGENT_A5_REPORT.md`; стенды: `A43_E2E.md`…`A57_E2E.md`, `a56-checks.txt` (49/49), `a57-checks.txt` (**76/76**) |
| Ревью A0–A3 | `AGENT_A0_A3_CODE_REVIEW.md` — AG-1…AG-19 (все закрыты), формат и уровни severity — оттуда |
| `sidecar/hds_agent` (TS) | **в A4–A5 не менялся** (инструменты `office_*`/`ask_user`, `hooks`, `egress`, `guard` — из A3). Читать как контекст для проверки взаимодействий |

## 2. Объём ревью

**Изменено в A4–A5** (по `git --no-pager diff --name-only 00da7c54~1..HEAD`):

| Область | Файлов | Что именно |
|---|---|---|
| `crates/hds-agent` | 14 | `mirror.rs`, `registry_db.rs`, `results.rs`, `validator.rs`, `history.rs`, `labels.rs`, `secrets.rs`, `settings.rs`, `mcp.rs`, `registry.rs`, `run.rs`, `factory.rs`, `projects.rs`, `paths.rs` |
| `crates/hds-ui` | 12 | `agent.rs`, `agent_session.rs`, `page.rs`, `config_edit.rs`, `lib.rs` (маршруты/SSE), тесты `agent_*` |
| `crates/hds-cli` | 10 | `cmd/agent*.rs`, `support.rs`, тесты `agent_*` |
| `crates/hds-anonymize` | 5 | клиент воркера: `anonymize_path`, `label_for_value`, `vault_project_labels`, `unmerge_entities`, `office_*` |
| `sidecar/hds_anonymize` | 4 | `handlers.py` (режим `agent`, метки проекта), `office_ops.py`, `vault.py` |
| `sidecar/tests` | 4 | `test_anonymize_path.py`, `test_anonymize_reverse.py`, `test_anonymize_vault.py`, `test_anonymize_worker.py` |
| `crates/hds-core` | 2 | `labels.rs` (падежи/разбор), `config.rs` (секция `agent:`) |
| Прочее | — | `README.md`, `config.example.yaml`, `RELEASE_NOTES_0.26.0.md`, `PLAN_AGENT_PRIVACY_ZONE.md` (правки решений), документы фаз |

**Не менялось, но ревью касается** (читать как контракт): `crates/hds-mcp` (маршрут
`/mcp/agent/<P>`, `scope`, `search_local_files`), `sidecar/hds_agent` (TS-воркер),
`crates/hds-index` (корни зоны, `implicit_excludes`), `crates/hds-extract`
(чтение docx/xlsx для `/compare` и валидатора).

## 3. Обязательное чтение (по порядку)

1. `PLAN_AGENT_PRIVACY_ZONE.md` — §4.2 (потоки), §4.4 (раскладка), §5.1–§5.9
   (TR-1.1…TR-8.6, D1–D23, инварианты I1–I8), **§6** (план работ A4/A5), §7 (приёмка MVP),
   §8 (фаза 2), §12 (журнал решений).
2. `AGENT_A4_STAGES.md` — §1, §2 (A4.1–A4.6), §3 (журнал), **§4.2 (долги 1–32)**,
   §5 (контракты), **§6 (грабли 1–32)**, §7 (окружение).
3. `AGENT_A4_REPORT.md` — итог, §4 (живые прогоны), §8 (долги).
4. `AGENT_A5_STAGES.md` — §1, §2 (A5.1–A5.7), §3.1–§3.7 (факты), **§4.1 (решения
   В-A5-1…8, В-A5.2…6, В-A5.7…6)**, **§4.2 (долги 1–39)**, **§5 (контракты — не ломать)**,
   **§6 (грабли 1–60)**, §7 (команды).
5. `AGENT_A5_REPORT.md` — итог, §2 (под-этапы), §3 (контракты), **§4 (инварианты I1–I8,
   включая находку по I1)**, §5 (живой сценарий), §6 (проверки), §7–§8 (что осталось, долги).
6. `AGENT_A0_A3_CODE_REVIEW.md` — **формат отчёта**, уровни severity, стиль находок;
   AG-1…AG-19 закрыты — не переоткрывать, но проверить, что закрытие не сломано в A4–A5.

## 4. Карта кода (строки — на HEAD `75912a0e`)

### 4.1 Ядро агента — `crates/hds-agent/src` (A4 + A5)

| Файл | Строк | Что внутри | На что смотреть в ревью |
|---|---|---|---|
| `mirror.rs` | 2888 | обход проекта, `kind_for`, `fit_component`, `unique_name`, `wait_stable_at`, прямой/обратный путь, маркер и петли `issued`/`seen`, `sync_once`, `compare`, `decide`, `labels_report`, `refresh_label_images` | TOCTOU оригинал/вариант, `normalize_rel`, обход junction, `_conflicts`, `orphan`, границы длин имён |
| `results.rs` | 1758 | `variant_paths`, `pending`, решения `accept/reject/conflict` (три исхода), backup, `ResultsError`, `CompareReport/FileInfo` | атомарность замены и backup, состояние реестра при сбое посреди решения, `replace_file` vs «файл занят» |
| `gateway.rs` | 1111 | шлюз реплик: `spawn/open/send/send_anonymized/next_event/abort/stop/clear_history`, `GatewayHost` (`egress`, `office_*`, `ask_user`), `write_egress`, `push_redactions`, `RefreshState` | блокировки, порядок `cancel_ask`→`abort`, egress-файлы, fail-closed |
| `validator.rs` | 1082 | отчёт результата: метки вне проекта, падеж, `[IMAGE_n]`, «текст пропал», `verdict` | ложные «ок», значения в отчёте (I8), стоимость (3 извлечения текста) |
| `filter.rs` | 886 | словарь (Aho-Corasick), границы слова, `ё→е`, секреты, `keep_terms`, снимок по `vault_rev`, `take_redactions`, fail-closed | longest match, неизвестные формы (`label/block`), гонка снимка, блокировка словаря на IPC |
| `registry_db.rs` | 886 | SQLite реестр: `Artifact`, статусы, `insert/upsert/mark_seen/set_issued/set_baseline/list_pending/by_orig`, WAL | два писателя (демон + UI), `busy_timeout`, `user_version`, аддитивная `merge_log` |
| `registry.rs` | 714 | `AgentRegistry` живых сессий: LRU, `get_or_open`, `reap_idle`, `unload`, `live` | выгрузка vs живой поток, замок на время `next_event` |
| `secrets.rs` | 691 | `Secrets`: чтение/слияние/запись, `has_provider_key`, `set_provider_key/base_url`, ACL | порядок «токен → ключи», удаление ключей нет, значения в ошибках |
| `run.rs` | 630 | `RunOptions/RunPlan/TurnReport`, `plan_with`, `prepare`, `build_filter`, `open_client(_at)`, `clear_history` | секреты в плане, `resolve_node/worker`, единый путь CLI/UI |
| `worker.rs` | 596 | Python-воркер `hds_anonymize` (JSON-RPC, таймауты, рестарт) | поздние `done`, `shutdown_with(grace)` |
| `settings.rs` | 435 | `AgentSettings` (дефолты `config.yaml`: `agent:`) | диапазоны, `ask_timeout_sec`, `egress_log`, `max_live_projects` |
| `deanon.rs` | 385 | метки → значения, падеж, пути `workspace` ↔ проект, неизвестные метки | `[SYS_n]`/пути, регистр, оба разделителя |
| `mcp.rs` | 303 | `McpSpec::from_secrets/to_json/write_settings` (`cline_mcp_settings.json`) | токен проекта, `timeout`, приоритет встроенных |
| `labels.rs` | 302 | панель «Метки»: `report`, `occurrences` | объём выдачи, значения только в панели |
| `paths.rs` | 265 | матрица опасных путей (UNC, `\\?\`, ADS, устройства, `..`, junction) | паритет с Node-guard |
| `projects.rs` | 239 | `projects.json`: `add/list/get/remove/validate`, `next_id` | `P`+4 цифры, путь вне `agent.root` |
| `history.rs` | 225 | история SDK: `latest_session`, `valid_session_id`, `clear` | обход `..`, бэкапы `*.bak` |
| `factory.rs` | 72 | ленивые фабрики Python (`anonymizer`, `extractor`) | один процесс на проект |

### 4.2 Вкладка и маршруты — `crates/hds-ui/src`

| Файл | Строк | Что внутри | На что смотреть |
|---|---|---|---|
| `agent.rs` | 2219 | все маршруты `/api/agent/*`: `projects/state/files/preview/label/keep/attach/message/abort/stop/answer/pending/compare/decision/labels/merge/unmerge/settings/history/clear`, коды ошибок | CSRF/loopback, server 403 (D20), коды 400/404/409/503, декодирование `rel` |
| `page.rs` | 2078 | разметка вкладки и JS: лента, «как видит модель», «Результаты», «Метки», «Настройки», «Очистить историю», `agraw`, SSE | свои регулярки меток (запрещены), `innerHTML`/XSS, 500 событий в DOM |
| `agent_session.rs` | 1665 | `AgentApi`/`Turn`/кольцо, насос, `preview`, `attach`, `message`, `answer`, `AskState`, SSE-кадры | busy/Condvar, таймаут вопроса, утечка значений в логи |
| `lib.rs` | 3059 | HTTP-сервер (`run_http_listener`), SSE-ветка, гейты, `route_full` | поток на соединение, `max_conns`, `Content-Length` в SSE |
| `config_edit.rs` | 629 | построчный редактор `config.yaml` (`apply_scalars`, `set_agent_values`) | CRLF (исправлено в A5.7), `dig`-проверка, атомарная запись |

7. Стенд: `A43_E2E.md`, `A44_E2E.md`, `A45_E2E.md`, `A46_E2E.md`, `A54_E2E.md`,
   `A55_E2E.md`, `A56_E2E.md`, **`A57_E2E.md`** (в каталоге стенда, он гитигнорится).
8. `AGENT_A5_7_HANDOFF_PROMPT.md` — методика живого прогона A5.7 (стенд, mock, инварианты).
9. Код — по карте §4, начиная с `mirror.rs` и `agent_session.rs` (самые крупные и рискованные).


### 4.3 CLI и sidecar

| Файл | Строк | Что внутри |
|---|---|---|
| `crates/hds-cli/src/cmd/agent_sync.rs` | 834 | демон зеркала: `--once/--interval`, `index.stop`, `sync.lock`, журнал, ротация |
| `crates/hds-cli/src/cmd/agent_run.rs` | 553 | разовый прогон агента (тонкая обёртка `hds_agent::run`) |
| `crates/hds-cli/src/cmd/agent_result.rs` | 233 | `result accept/reject/conflict` — то же ядро, что кнопки UI |
| `sidecar/hds_anonymize/handlers.py` | — | режим `agent`: метки проекта, `[IMAGE_n]`, маркер; `office_*`; `vault_*` RPC |
| `sidecar/hds_agent/src/{worker,session,hooks,tools,guard,egress,rpc,events}.ts` | 265/156/170/127/127/77/165/123 | TS-воркер (из A3): политика инструментов, guard, единственный `egressFetch`, `mcp_in`, `office_*`, `ask_user` |

### 4.4 Тесты, которые надо прочитать и (выборочно) прогнать

| Набор | Строк | Что покрывает |
|---|---|---|
| `crates/hds-agent/tests/gateway_worker.rs` | 635 | шлюз + Node-воркер, mock-провайдер, фильтр |
| `crates/hds-agent/tests/lifecycle_worker.rs` | 704 | жизненный цикл воркера, abort/stop, история |
| `crates/hds-agent/tests/refresh.rs` | 119 | обновление снимка vault (AG-2) |
| `crates/hds-ui/tests/agent_api.rs` | 200 | каркас вкладки (A5.1) |
| `crates/hds-ui/tests/agent_chat.rs` | 1041 | preview/message/SSE/ask/abort (A5.2–A5.3) |
| `crates/hds-ui/tests/agent_results.rs` | 689 | pending/compare/decision, busy, CSRF, server (A5.4) |
| `crates/hds-ui/tests/agent_labels.rs` | 510 | merge/unmerge, пересборка образов (A5.5) |
| `crates/hds-ui/tests/agent_settings.rs` | 593 | settings/history/clear, CRLF-конфиг (A5.6–A5.7) |
| `crates/hds-cli/tests/agent_sync.rs` | 530 | зеркало: прямой/обратный путь, решения, демон |
| `crates/hds-cli/tests/agent_result.rs` | 488 | CLI-решения |
| `crates/hds-cli/tests/agent_run.rs` | 932 | разовый прогон, mock-провайдер |
| `crates/hds-cli/tests/agent_acceptance.rs` | 893 | сквозная приёмка A3 (единственный известный флейк — `.cline`) |

## 5. Чеклист A4 — зеркало (TR-7.1…7.9, D2/D5/D18/D22)

Для каждого пункта: прочитать код, проверить тесты, при сомнении — воспроизвести на стенде.

| # | Требование | Где | Что проверить |
|---|---|---|---|
| 1 | **TR-7.1** прямой путь | `mirror.rs::sync_once` + `kind_for/fit_component/unique_name` | структура папок повторяется; имена — метки **без падежного суффикса**; `pdf/pptx/mpp` → `<имя>.<ext>.md`; `unsupported`/`too_big` не ломают проход; файл, который дописывается, откладывается, а не анонимизируется половиной |
| 2 | **TR-7.2** реестр | `registry_db.rs` | `orig_sha256`, `anon_sha256_issued/seen`, статусы; идемпотентность (повторный проход — 0 изменений); конкурентная запись демона и UI; WAL/`busy_timeout`; схема и `user_version` |
| 3 | **TR-7.3** текстовые образы | `mirror.rs` (release), `paths.rs`, `guard.ts` | образ только для чтения; `editor`/`apply_patch` по нему блокируются; маркер `hds-agent/<P>/<id>` в первой строке (md/txt) и в `docProps` (Office) |
| 4 | **TR-7.4/7.5** обратный путь | `mirror.rs` (`agent_new`, `agent_changed`, `make_variant`) | правка образа → вариант в `.hds-agent\pending` + снимок основания; новый файл агента → сразу в папку проекта, на своё место; имя деанонимизировано; петли `issued`/`seen` не дают зеркалить собственный выпуск |
| 5 | **TR-7.6** решения | `results.rs` (`decide`, `variant_paths`) | «Принять»: backup → атомарная замена → пересоздание образа → очистка варианта; «Отклонить»: удалить вариант, вернуть образ, сообщение агенту; три исхода конфликта; «файл занят» (`os error 32`) — без потерь, вариант на месте |
| 6 | **TR-7.7** петли | `registry_db.rs` (`set_issued`, `mark_seen`), `mirror.rs` | изменение = `seen != issued`; выпущенный файл хранит `orig_sha256` и не порождает новый «результат»; принятый вариант — новый образ, но не новый результат |
| 7 | **TR-7.8** валидатор | `validator.rs` | метки вне проекта, согласованность падежа, непомещённые `[IMAGE_n]`, доля пропавшего текста; `verdict: check` не блокирует; **в отчёте нет значений** (I8) |
| 8 | **TR-7.9** нет «Входящих» | `mirror.rs`, `agent_sync.rs` | новые файлы агента идут на место; `.hds-agent\pending` — только варианты |
| 9 | **A4.6** демон | `agent_sync.rs` | `--interval` (0/минус/мусор/вместе с `--once` → код 2), `sync.lock`, `index.stop` (общий с индексатором), журнал ≤ 1 МБ и `.1`, ошибка прохода не убивает демон |
| 10 | Обход и безопасность | `mirror.rs::walk_files`, `paths.rs`, `rules.rs` | junction/симлинк не обходится; `.hds-agent`/`_attachments`/`_conflicts`/`~$*`/`*.tmp`/`Thumbs.db` пропускаются; правила обхода не расходятся с `index_filter` |


## 6. Чеклист A5 — вкладка (TR-5.1…5.3, TR-6.1…6.7, TR-8.1/8.3/8.4, D20)

| # | Требование | Где | Что проверить |
|---|---|---|---|
| 1 | **TR-5.1** реплика | `agent_session::preview/message`, `agent.rs::preview_json` | предпросмотр показывает ровно то, что уйдёт (NER **и** словарный проход — иначе значение в косвенном падеже уходит открытым); «Скрыть ещё…» (`label`) даёт метку нужного типа; «Не скрывать» (`keep`) пишет в общий `keep_terms` |
| 2 | **TR-5.2** пути и вложения | `agent_session::attach`, `paths.rs`, `mirror.rs` | файл проекта → путь образа по реестру; файл вне проекта → `workspace\_attachments`; абсолютные пути/UNC в тексте распознаются |
| 3 | **TR-5.3** события и деанонимизация | `agent_session` (лента), `deanon.rs`, `page.rs` (`agraw`) | в ленте значения, в `raw` — метки; пути `workspace` → папка проекта (оба разделителя); неизвестная метка видна с пометкой; **UI не разбирает метки своими регулярками** |
| 4 | **TR-6.1** точки фильтра | `hooks.ts` (`egressFetch`, `beforeTool/afterTool`), `gateway.rs::egress` | `model` — каждый запрос (все `messages`, system, `tools`, аргументы `tool_calls`); `mcp_out`; `mcp_in` — **весь** результат (AG-4) |
| 5 | **TR-6.2** словарь | `filter.rs` | а/б/в/г; границы слова, `ё→е`, неразрывные пробелы, кавычки, дефисы; `keep_terms` исключены; значения < 3 символов не входят; секрет в тексте **блокирует** запрос (`model`/`mcp_out`) |
| 6 | **TR-6.3** особенность `mcp_in` | `hooks.ts`, `filter.rs::Point::McpIn` | результат внешнего MCP фильтруется целиком; новые настоящие названия из открытых источников не анонимизируются (по решению) |
| 7 | **TR-6.4** аудит и уведомления | `filter.rs` (`agent_egress_redacted`), `agent_session` (`take_redactions`) | метка и источник — **без значения**; срабатывания не теряются между запросами; сводка в панели |
| 8 | **TR-6.5** fail-closed | `gateway.rs::RefreshState`, `refresh.rs`, `filter.rs` | снимок по `vault_rev` перед **каждым** запросом; нет снимка/ошибка/старше `max_stale_sec` → `privacy_filter_unavailable`, ноль запросов к провайдеру |
| 9 | **TR-6.6** «Очистить историю» | `history.rs`, `run::clear_history`, `agent.rs::history_clear` | стоп → выгрузка → перезапись файлов истории метками → бэкапы `*.bak` → возобновление сессии; отчёт `{session, files, backups, replaced}` без значений; 409, если остановить не удалось |
| 10 | **TR-8.1/8.3** проекты и секреты | `projects.rs`, `secrets.rs`, `settings.rs`, `config_edit.rs` | провайдер/модель в `projects.json`; ключ **только** в `secrets.json` (слияние), в `config.yaml` — никогда; в ответах — «задан/нет»; `hds agent token` не теряет чужие ключи (известный долг — проверить, где ещё пишется файл) |
| 11 | **TR-8.4** один процесс анонимизации | `factory.rs`, `run.rs`, `registry.rs` | один Python на проект/сеанс; `/compare`, метки и решения не поднимают второй; выгрузка по простою не рвёт ленту |
| 12 | **D20** server-профиль | `agent::gate`, `lib.rs` | все маршруты и SSE отдают 403; кнопка вкладки скрывается |
| 13 | Маршруты и коды | `agent.rs` | CSRF (`X-HDS-UI` + `Content-Type`), loopback; 400/404/409/503/500 по смыслу; `rel` из query декодируется (`%5C`, кириллица); тело ограничено; в JSON нет значений, кроме разрешённых контрактом (лента, предпросмотр, панель меток) |
| 14 | Лента и SSE | `agent_session`, `lib.rs` | кольцо 500; `Last-Event-ID` важнее `after`; поток не держит `max_conns`; закрытие по 300 с и переподключение; JSON-фолбэк |
| 15 | Вопрос агента | `agent_session::AskState`, `gateway::deanon_question` | один вопрос на проект; 400/404/409; таймаут 600 с → отказ модели, ход продолжается; «Стоп» снимает вопрос **до** команды воркеру |
| 16 | Метки проекта | `labels.rs`, `agent.rs` (`labels/merge/unmerge`), `mirror::refresh_label_images` | старшая метка, `merge_log`, `unmerge_entities`, пересборка образов, `messages.jsonl` и `notice` без значений; склейки до A5.5 необратимы (404 с текстом) |
| 17 | UI/JS | `page.rs` | `textContent`, а не `innerHTML` для текста модели; нет своих регулярок меток; буфер событий для «как видит модель»; 500 записей в DOM |

## 7. Инварианты I1–I8: где код и как проверять независимо

| # | Кто отвечает в коде | Как проверить своими руками |
|---|---|---|
| **I1** | `mirror.rs` (образ = NER-режим `anonymize`), `deanon`, `paths` | скан `<agent.root>` без `agent-state`/`*.bak`/имён — `a57_probe.py scan … --types=…`; **учесть находку A5.7**: образы содержат формы, которые NER не выделил (22 в 12 образах, свежий образ — 5). Проверить, что это by design (нет словарного прохода) и что фильтр закрывает их на границе |
| **I2** | `filter.rs` + `hooks.ts` | `MOCK_REQUESTS`/`egress`: скан тех же форм; метки в запросах есть; события `redacted` |
| **I3** | `paths.rs`, `guard.ts`, `gateway::office_*` | `office_read`/`editor` по пути вне `workspace` → отказ; повторная проверка в Rust, не только в Node |
| **I4** | `env.rs`, `session.ts` (подменённый HOME), приёмка | снимок `%USERPROFILE%\.cline` до/после e2e; `agent_acceptance` одиночным прогоном |
| **I5** | `hds-mcp` (`Scope`, `project_ctx`), `projects.rs` | не в фокусе A4/A5, но проверить, что вкладка не расширяет область поиска |
| **I6** | `results.rs`, `mirror.rs` | sha256 всех оригиналов до/после сценария; изменился только принятый файл (+ новый файл агента) |
| **I7** | `secrets.rs::secret_strings`, `filter.rs` | ключ в реплике → запрос блокируется, ноль запросов к провайдеру |
| **I8** | `validator.rs` (отчёт), `history.rs`, `agent_sync` (журнал), `messages.jsonl`, ответы API | скан `sync.log`, `messages.jsonl`, `pending/settings/labels` — значения не должны встречаться (в `labels` они разрешены: панель — зона пользователя) |

**Важно:** `agent-state\.cline` (история SDK) — зона агента (В-A5.7-2), значения там
норма; в скан I8 она не входит, но `*.bak` исключать обязательно.


## 8. Контракты, которые нельзя «улучшать» молча

Из `AGENT_A5_STAGES.md` §5 (и §5 `AGENT_A4_STAGES.md`); нарушение — находка, а не рефакторинг.

* **Маршруты `/api/agent/*`** — loopback/CSRF как у остальных POST; в `is_user_route` не входят; server-профиль 403 (D20).
* **Значения в HTTP** — вкладка зона пользователя (лента, предпросмотр, панель меток); журналы/`sync.log`/`messages.jsonl` — без значений (I8).
* **Секреты** — только `data\agent\secrets.json` через `hds_agent::secrets`; в `config.yaml` — никогда; в ответах — маска/признак.
* **Решения** — только `hds_agent::results`; UI не заменяет оригиналы сам; «файл занят» — `ResultsError::Busy`.
* **Статусы файлов** — только `registry_db`; UI не вычисляет статус по наличию файла.
* **Воркер** — только через `Gateway`; один активный ход на проект; UI не читает stdout напрямую.
* **Деанонимизация** — только `hds_agent::deanon`; своих регулярок меток в UI нет.
* **Метки** — разбор `hds_core::labels`, падеж `gram_to_suffix`, в именах файлов суффикса нет.
* **Проекты** — `projects.json` единственный список; `path` — абсолютная папка пользователя; `id` = `P`+4 цифры (D21).
* **Процессы ОС** — UI не запускает процессы и не открывает файлы (В-A5-7); максимум путь в буфер.
* **Фильтр** — снимок по `vault_rev` перед каждым запросом, fail-closed; срабатывания через `take_redactions()`.
* **История** — `hds_agent::history`; «Очистить историю» = путь `run::clear_history`, не ручная правка файлов.
* **Зеркало** — `Mirror::sync_once/decide/compare`; CLI и UI решают одним ядром (`agent_result` = кнопки).

## 9. Гипотезы: что проверять в первую очередь

Это **не** список известных дефектов, а места, где ошибки вероятны. По каждой —
прочитать код, прогнать тест, при сомнении воспроизвести. Найденное — находка с severity;
опровергнутое — строка «проверено, дефекта нет» в §2 отчёта (это тоже результат).

### 9.1 Данные и атомарность (A4)

1. **TOCTOU в «Принять».** Между проверкой «оригинал не менялся» (`by_orig` / `orig_sha256`) и заменой (`replace_file`) файл может измениться (демон, пользователь, Word). Под каким замком идёт проверка+замена? Что будет, если оригинал изменился в этом окне?
2. **Сбой посреди решения.** Backup сделан, замена прошла, пересоздание образа упало (Python недоступен): каковы статус реестра, содержимое `pending`, что видит пользователь при повторном «Принять»? Есть ли идемпотентность?
3. **`replace_file` и «файл занят».** 20 попыток × 50 мс. Любой `PermissionDenied` трактуется как `Busy`? Может ли реальная ошибка прав превратиться в «закройте файл»?
4. **`normalize_rel`.** Защита от `..`, абсолютных путей, UNC, ADS (`file.docx:stream`), `\\?\`, `/` vs `\`. Применяется ли **везде** (compare, decision, registry, `variant_paths`)?
5. **`paths.rs` vs `guard.ts`.** Матрица одна? Проверить расхождения: регистр, короткие имена 8.3, устройства (`CON`, `NUL`), `%TEMP%`, junction внутри `workspace`.
6. **`registry_db`.** Два писателя (демон и вкладка) на один файл: `busy_timeout`, ретраи, `SQLITE_BUSY`; что видит читатель в середине транзакции; `user_version` и аддитивные таблицы (A5.5 не повышал версию — почему это безопасно).
7. **`mark_seen`/`set_issued`.** Порядок «прочитать файл → посчитать sha → записать»: можно ли потерять изменение, если файл изменился между шагами? Размер+mtime как быстрый фильтр — где он используется и не даёт ли ложного «без изменений».
8. **Длинные имена и коллизии.** `fit_component` ≤ 120 + 8 hex; `unique_name` ` (2)`. Что при 300-символьном имени в папке проекта (выпуск обратно) — обрезается ли и в проекте (долг A4 п.14)?
9. **`orphan`.** Образ потерян (реестр есть, файла нет): что делает проход, видит ли пользователь, есть ли «пересоздать» (долг A4 п.15) — и не ломает ли это I6.

### 9.2 Фильтр и деанонимизация (A5, ядро безопасности)

10. **Свежесть снимка.** `RefreshState` + `refresh_if_needed`: обновляется ли снимок при появлении метки в **том же** ходе (new label из `label_for_value`)? Тест `refresh.rs` — что именно он проверяет?
11. **Длиннейшее совпадение.** Aho-Corasick: перекрытия (короткое имя внутри полного ФИО), `keep_terms`, формы с дефисами и кавычками; что происходит с формой, которая одновременно в `keep_terms` и в словаре.
12. **Секреты.** `secret_strings`: только секретные поля; блокировка по границам слова — не ослабляет ли её пунктуация (`key,`, `key.`, `key)`) и варианты кодирования.
13. **`take_redactions`.** Drain: могут ли срабатывания потеряться между запросом и чтением? Что видит второй проект/сеанс.
14. **`deanon`.** Падеж: `[PERSON_n|дат]`, `|мн`, неизвестный суффикс; неизвестная метка — как показывается; пути: оба разделителя, регистр; **найденное в A5.7**: путь образа в результате MCP остаётся путём зоны (`mcp_in` закрыл системные строки метками) — подтвердить и предложить лечение.
15. **Fail-closed.** Реально ли «ноль запросов к провайдеру» при: нет снимка, ошибка Python, снимок старше `max_stale_sec`, таймаут IPC. Проверить счётчиком в mock-провайдере.
16. **`mcp_in` целиком.** `hooks.ts::afterTool` подменяет весь `result`: а если результат — объект с вложенными `content`/`structuredContent` (как у `disk-search`)? Значения во вложении фильтруются?


### 9.3 Сессии, лента, вопрос (A5)

17. **`Turn` и кольцо.** `Condvar`/`busy`: что при двух `message` подряд, при `abort` во время `ask`, при выгрузке по простою посреди хода; не теряются ли события между `push` и `events_json`.
18. **Насос.** Поток живёт, пока жив шлюз: что при `reap_idle` (выгрузка) — лента замолкает или рвётся? `session_id` возобновляется?
19. **SSE.** `Content-Length` не пишется; `Connection`; `Last-Event-ID` важнее `after`; что при 500 и при разрыве; `max_conns` 32 и одна подписка на вкладку.
20. **`ask_user`.** `cancel_ask` → `abort`; таймаут 600 с; поздний `POST /answer` (409); вопрос после перезапуска UI; кто держит замок в это время.
21. **`preview`.** Стоимость (NER + снимок + словарь) на каждый клик; кэш не сделан (долг п.17) — оценить, не станет ли UI «залипать» на слабой машине.
22. **Значения в логах.** `agent_session` обещает не логировать значения: проверить `tracing`/`eprintln`/`stderr` воркера (`anonymize-stderr.log`, `worker-stderr.log`), `sync.log` и `messages.jsonl` — каналом и сканом.

### 9.4 Секреты, настройки, MCP (A5.6)

23. **Слияние `secrets.json`.** `save` читает диск и сливает: гонка двух писателей (CLI + панель); нет ли удаления ключей; ACL best-effort — что на Windows, если ACL не применился.
24. **`hds agent token`.** Перезаписывает файл только блоком `projects` (грабля 63): проверить, что после него ключи провайдера/MCP **пропадают**, и оценить цену исправления в `hds_mcp::agent` (сохранять незнакомые ключи).
25. **`set_agent_values`.** `dig`-проверка каждого пути; комментарии и соседние секции; CRLF (исправлено `891385d8` — проверить, что нет регрессий на LF, смешанных переводах, файле без секции `agent:`); атомарная запись (`replace_file`).
26. **Диапазоны.** `ask_timeout_sec`, `filter.max_stale_sec`, `max_live_projects`, `request_timeout_sec`: валидация на сервере (400) и на клиенте (без дубля правил).
27. **`mcp.rs`.** Токен в `cline_mcp_settings.json`; `timeout`; приоритет встроенных над `mcp.servers`; что если порт занят/сервер не поднят — **инструмента просто нет**, ход не падает (подтверждено A5.7).

### 9.5 Тесты, производительность, риск I1

28. **Три исхода конфликта.** A4-отчёт говорит «на стенде требуют двух версий документа». Есть ли тест каждого исхода (`agent|new|delegate`) в `agent_result.rs`/`agent_results.rs`? Если нет — находка «пробел тестов» с оценкой.
29. **`agent_acceptance`.** Флейк `.cline`: подтвердить, что причина — IDE, а не код (сравнить volatile-список с тем, что пишет IDE); не «лечить» в ревью.
30. **E2E-детерминизм.** `a54…a57` зависят от стенда (метки, реестр): повторный прогон без `a57_restore.ps1` ломается (письмо/реестр уже есть) — стоит ли зафиксировать это в сценарии (находка low).
31. **Производительность.** Реестр открывается на каждый запрос (долг п.15/26), `preview` — NER, зеркало — sha каждого файла каждый проход (A4 п.8). Оценить: 34 файла ок, а 5 000?
32. **I1 как риск.** Подтвердить находку A5.7 независимым сканом и оценить стоимость словарного прохода при сборке образа (точка расширения: `sidecar/hds_anonymize` или `mirror`), не нарушая контракт «образ = NER».

## 10. Независимые проверки и пробы (не верить отчётам)

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager log -1 --oneline; git --no-pager status --porcelain -uall
git --no-pager diff --shortstat 00da7c54~1..HEAD
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast -- --test-threads=4 *> tools\parity\out\review-a45-cargo-test.log
cargo test -p hds-cli --test agent_acceptance            # I4 — отдельным прогоном (флейк .cline)
cargo test -p hds-agent; cargo test -p hds-ui; cargo test -p hds-cli
npm --prefix sidecar/hds_agent test
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
```

Стенд (вымышленный проект; данные не трогать без нужды — ревью диагностическое):

```powershell
$env:HDS_CONFIG='<repo>\tools\parity\out\agent_a42\config.yaml'
$env:HDS_ROOT='<repo>\tools\parity\out\agent_a42\root'
target\debug\hds.exe agent sync --once --project P0001
target\debug\hds.exe ui --port 8799
target\debug\hds.exe mcp-http start          # порт 8788 (8787 занят службой машины)
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a57_e2e.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a57_restore.ps1
Remove-Item Env:HDS_ROOT, Env:HDS_CONFIG     # перед cargo test
```

Пробы инвариантов — `tools\parity\out\agent_a42\a57_probe.py` (интерпретатор —
`root\sidecar\venv-anonymize\Scripts\python.exe`):

| Режим | Что даёт |
|---|---|
| `forms <db> <out.json>` | формы проекта из `Vault.project_labels` (AG-1: своих регулярок нет) |
| `scan <dir|file> <forms.json> [--skip=a,b] [--types=…] [--nopath]` | скан целыми словами (семантика фильтра), исключения каталогов/типов/путей |
| `shas <dir> <out.json>` | sha256 файлов (без `.hds-agent`) — I6 |
| `hashdir <dir> <out.json>` | sha256/size/mtime — снимки профиля (I4) |
| `docx` / `xlsx` | текст docx, заголовки/строки xlsx (артефакты сценария) |

Как воспроизводить то, что вызывает сомнение:

* **фильтр и `mcp_in`** — mock-провайдер `a57_mock.mjs` (`MOCK_SCRIPT`/`MOCK_REQUESTS`/`MOCK_RESOLVE`): шаг `{fromTools}` берёт имя MCP-инструмента из `tools` запроса;
* **«файл занят»** — открыть оригинал на запись из PowerShell (`[IO.File]::Open(...,'Open','ReadWrite','None')`) и нажать «Принять»/`hds agent result accept`;
* **fail-closed** — поднять mock-провайдер и включить `egress_check` без снимка (или остановить Python-воркер), проверить, что к провайдеру **0** запросов;
* **конфликт** — изменить оригинал руками между `sync --once` и «Принять» (три исхода);
* **I1/I2/I8** — `a57_probe.py scan` по `<agent.root>` (без `agent-state`/`*.bak`), по `a57-mock-requests.jsonl`, `egress\*`, `sync.log`, `messages.jsonl`.


## 11. Формат отчёта `AGENT_A4_A5_CODE_REVIEW.md`

Структура — как `AGENT_A0_A3_CODE_REVIEW.md`:

1. **Шапка**: дата, ветка/HEAD, ahead, объём (`00da7c54~1..HEAD`: коммиты, файлы, строки),
   метод (чтение кода и документов, независимые прогоны, живые пробы), где артефакты
   (`tools/parity/out/review-*`, каталог гитигнорится).
2. **§1 Итог** — таблица «фаза → вердикт (принята / принята с замечанием / принята
   с обязательными доработками) → обоснование»; отдельной строкой — что закрыть
   **до A6** и что в **фазу 2**.
3. **§2 Независимые проверки** — таблица «проверка → как → результат» (fmt, clippy,
   тесты с числами, npm, python, приёмка одиночным прогоном, пробы: скан форм,
   счётчик запросов к провайдеру, `disk-search`, воспроизведение busy/конфликта/CRLF).
   Здесь же — строки «проверено, дефекта нет» по гипотезам §9 (коротко).
4. **§3 Находки** — по одной подголовкой, **ID от AG-20**, уровни:
   * **high** — ломает пользовательское поведение или требование плана/инвариант;
   * **medium-high** — риск безопасности данных (I1/I2/I6/I8) без немедленного проявления;
   * **medium** — заметный дефект/риск, обходится;
   * **low** — качество, производительность, документация;
   * **info** — наблюдение.
   Шаблон:

   ```markdown
   ### AG-20 (severity). Короткое имя дефекта

   **Где.** `crates/…/file.rs:1234` (`fn_name`), `crates/…/other.rs:567`.
   **Что.** Что именно происходит (факт из кода: условие, порядок вызовов, отсутствие проверки).
   **Почему это плохо.** Какое требование плана (TR-x.y), инвариант (I1…I8) или сценарий
   пользователя нарушается; что увидит человек.
   **Как проверить.** Команда/проба/тест, которым дефект воспроизводится (и что уже
   воспроизведено в этом ревью).
   **Как исправить.** Вариант правки (файл/функция), цена, что сломается при неверном
   исправлении, нужен ли тест.
   **Цена/риск исправления.** Оценка (S/M/L) и что проверить после.
   ```

5. **§4 Что сделано хорошо** — список свойств, которые нельзя потерять при доработках
   (по образцу A0–A3: единственный сетевой выход, fail-closed, guard путей, одно ядро
   решений, «значения не пишутся в журналы», слияние `secrets.json`, детерминизм реестра…).
6. **§5 Соответствие плану** — таблица «требование (TR-5.x/6.x/7.x/8.x, D, I) →
   состояние (выполнено / частично / нарушено) → комментарий со ссылкой на находку».
7. **§6 Порядок закрытия** — что до A6, что в фазу 2, что можно отложить; для каждого
   пункта — почему именно в этом порядке.
8. **§7 Вопросы владельцу** — решения, которые ревью не может принять сам (например:
   словарный проход при сборке образа против контракта «образ = NER»; удаление ключей
   из `secrets.json`; пересборка образов при появлении метки).
9. **§8 Что проверено по прошлому ревью** — короткая таблица AG-1…AG-19: закрытие
   не сломано в A4–A5 (или сломано — тогда ссылка на новую находку).

Правила оформления: только метки и имена файлов, **значения fixtures в тексте отчёта
не цитировать** (I8/AG-1); ссылки `file:line` — на HEAD `75912a0e`; артефакты проб —
`tools/parity/out/review-*` (`review-a45-cargo-test.log`, `review-scan-*.txt`, …).

## 12. Границы и правила

* **Ревью, не рефакторинг.** Продуктовый код не менять. Если находка требует правки —
  сначала согласовать с владельцем, потом править **отдельным** коммитом с тестом.
* **Не переписывать** `PLAN_AGENT_PRIVACY_ZONE.md` и боевые `PLAN_*.md`; правки решений —
  только через вопросы владельцу.
* **Не откатывать** чужие незакоммиченные изменения и не «чинить» данные стенда
  (метки, реестр, образы) без нужды; артефакты проб — только `tools/parity/out/` (гитигнорится).
* **Не лечить известные флейки** (`agent_acceptance` под IDE, `quota_socket` под нагрузкой) —
  гонять одиночным прогоном и проверять, что причина именно нагрузка/IDE.
* **Коммит ревью** — только отчёт: `docs(agent): A4–A5 code review — …` (git-identity:
  `git -c user.name='Alex Guerchoig' -c user.email='Guerchoig@gmail.com' commit -F <файл UTF-8>`,
  пути — явным списком). **push не делать.**
* Машина разработческая: `hds ui`, `hds mcp-http`, `hds agent sync`, `llm_host` можно
  останавливать и возвращать (см. `cline-rules/dev-machine.md`).


## 13. Окружение и грабли (коротко)

* **Перед `cargo test` снять** `HDS_ROOT`/`HDS_CONFIG` — иначе тесты идут по стенду (грабля A4 §6 п.12).
* **PowerShell 5.1**: `.ps1` с кириллицей — UTF-8 **с BOM**, иначе cp1251 и «мусор» (A5 §6 п.50);
  `param()` — первой инструкцией (п.60); тела POST — UTF-8 байтами; `Invoke-RestMethod` декодирует
  JSON как Latin-1 — читать `RawContentStream`; `& native 2>&1` при `$ErrorActionPreference='Stop'`
  даёт `NativeCommandError` (локально `Continue`, как в `a57_e2e.ps1::RunNative`).
* **Консоль врёт про кириллицу** — проверять файлами (`read_files`), а не выводом терминала.
* **Порт 8787** занят экземпляром службы машины (PID 4768, без PID-файла в стенде; `taskkill`
  без прав администратора — «отказано в доступе»); стенд работает на **8788**.
* **Индекс зоны** требует доступного эмбеддера: в конфиге стенда — локальный фасад
  `127.0.0.1:8011` (`dim: 1024`); с закрытым портом `hds index --full` падает на каждом файле.
* **`agent-state` — в зоне** (`zone\P0001\agent-state`), а не в `HDS_ROOT`; `*.bak` и `.cline`
  (история SDK) — зона агента: в сканах I1/I8 исключать.
* Флейки: `agent_acceptance` (`.cline` пишет IDE) и `quota_socket::unauthorized_heavy_rejected_before_queue`
  — одиночным прогоном, не «лечить».

## 14. Промт (копировать целиком)

```text
Ты — независимый ревьюер проекта hermes-disk-search (Rust + Python/Node sidecar).
Задача — код-ревью двух фаз основного проекта «ИИ-агент в анонимной зоне»
(PLAN_AGENT_PRIVACY_ZONE.md): A4 (двустороннее зеркало `hds agent sync`) и A5
(вкладка «Агент»). Ветка agent-privacy-zone, HEAD 49913f73 (документный коммит этого промта;
код A4–A5 — на 75912a0e), дерево чистое, ahead origin на 89, push не делался. Объём: 39 коммитов
(`00da7c54~1..HEAD`), 68 файлов, +28 014/−693 (только код — 51 файл, +20 974/−691).

Ревью A0–A3 уже было (AGENT_A0_A3_CODE_REVIEW.md, AG-1…AG-19 закрыты) — не переоткрывать;
нумерация новых находок — с AG-20.

Сначала прочитай целиком AGENT_A4_A5_CODE_REVIEW_PROMPT.md — это контекст и методика:
карта кода (строки/функции), чеклисты A4 (TR-7.1…7.9) и A5 (TR-5.1…5.3, TR-6.1…6.7,
TR-8.1/8.3/8.4, D20), инварианты I1–I8, контракты, 32 гипотезы, команды проб и формат
отчёта. Затем: PLAN_AGENT_PRIVACY_ZONE.md (§4.2, §4.4, §5.1–5.9, §6, §7, §8),
AGENT_A4_STAGES.md (§2, §4.2, §5, §6), AGENT_A4_REPORT.md, AGENT_A5_STAGES.md
(§2, §3.1–3.7, §4.1, §4.2, §5, §6), AGENT_A5_REPORT.md, A57_E2E.md на стенде.

Проверяй код, а не отчёты: независимые прогоны (fmt/clippy/test/npm/python, приёмка
одиночным прогоном), пробы a57_probe.py (скан форм целыми словами, sha256, снимки .cline),
mock-провайдер для фильтра и mcp_in, воспроизведение: «файл занят», три исхода конфликта,
fail-closed (0 запросов к провайдеру), CRLF-конфиг, I1-скан зоны.

Не меняй продуктовый код: это диагностика. Находку оформляй как AG-NN (severity:
high / medium-high / medium / low / info) с file:line, «что», «почему плохо» (TR/I), «как
проверить», «как исправить», «цена». Отчёт — AGENT_A4_A5_CODE_REVIEW.md (разделы 1–8 по
формату A0–A3: итог с вердиктом по A4 и A5, независимые проверки, находки, что сделано
хорошо, соответствие плану, порядок закрытия до A6/фазы 2, вопросы владельцу, проверка
закрытия AG-1…AG-19). Значения fixtures в отчёте не цитировать — только метки.

Границы: не переписывать PLAN_*.md, не откатывать чужие правки, артефакты — только
tools/parity/out/ (гитигнорится), фиксы (если владелец согласится) — отдельным коммитом,
коммит отчёта: docs(agent): A4–A5 code review — …, push не делать.
```
