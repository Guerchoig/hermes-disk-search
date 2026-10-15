# Состояние проекта hermes-disk-search — контекст для нового чата

> Рабочая папка: `C:\Users\Sasha\hermes-disk-search` · GitHub: Guerchoig/hermes-disk-search (публичный).
>
> **Релиз 0.20.2 выпущен (15.10.2026, ветка `server-multiuser`, тег `v0.20.2`,
> `RELEASE_NOTES_v0.20.2.md`):** PATCH серверной ветки. **Закрыт корень инцидента 15.10.2026**
> (служба «переключалась» на посторонний `config.yaml` в корне поставки: UI без токена, чужая БД,
> «llm-host не найден»): кэш конфига на процесс `hds_core::config::load_cached` (путь + mtime +
> размер), фиксация пути конфига при старте UI (`gate_config_path`), `install_server.ps1`/`.command`
> переименовывают «тень» (`<корень>\config.yaml` и `<корень>\app\current\config.yaml` → `*.bak`)
> и закрепляют `HDS_CONFIG` + машинные `LLAMA_RUNTIME_DIR`/`HDS_ENGINE_DIR`/`HDS_EXTRACT_PYTHON`
> в per-service `Environment` (`-NoConfigPin`/`--no-config-pin` отключает),
> `install_app_version.ps1` больше не копирует `config.example.yaml` в каталог версии,
> `hds check` — пункт `config-shadow`. Далее: идемпотентная установка служб (`sc config` вместо
> `sc create`; принимается и версионная раскладка `app\current\bin`), машинные зависимости служб
> (tesseract/ffmpeg `--scope machine`, пути в server-конфиге: `index.ocr_tesseract_cmd`,
> `index.ocr_tessdata`, `index.ffmpeg_cmd`; пропуск — `-SkipDeps`), настольный `setup.ps1` без
> вопросов (отказ — явными ключами `-SkipOcr`, `-SkipWhisper`, `-SkipClip`, `-SkipDiarization`,
> `-SkipTranscribe`, `-NoAutostart`, `-NonInteractive`; отсутствующие диски заменяются
> автоматически), новый сценарий `installers\migrate_to_machine_root.ps1` (перенос серверной
> установки из профиля в машинный каталог: останов ролей, `robocopy /XJ`, junction'ы, перенос
> моделей и движка со ссылкой обратно, закрепление переменных, перерегистрация служб и приёмка;
> идемпотентен, при сбое возвращает роли), уточнённая диагностика (стандартные каталоги
> tesseract/ffmpeg, вердикт по ffmpeg с учётом декодирования движком, каталоги автотранскрибации,
> состояние демона `transcribe`), **разделённые руководства** — `README.md` (настольное, 19 глав)
> и `README_SERVER_MULTIUSER.md` (серверное, 21 глава), оба читаются независимо; серверное
> руководство **обязательно в поставке** (сборщики падают без него, `RELEASING.md` §2 обновлён);
> устранено мигание теста `queue::tests::fifo_policy_is_strict_order` (красный CI на macOS
> 15.10.2026: порядок прихода задавался `sleep`; теперь заявки ставятся в очередь по одной —
> двадцать прогонов подряд зелёные). **Проверено:** `cargo fmt --check`, `clippy -D warnings`,
> `cargo test --workspace` — зелёные локально и в CI (все джобы `rust.yml`, включая macOS);
> `hds check` на живой машине — 0 критических («основные компоненты готовы»); `ask` по файлам —
> ответ со ссылками `[N]`, путями и таймкодами за 14 с. **Ассеты:** Windows x64 235 МБ (+ sha256),
> оба рантайма движка (CUDA 566 МБ / Vulkan 86 МБ), macOS 271 МБ и Metal 52 МБ (best-effort),
> клиентские пакеты 11/12 МБ и `client-install.ps1|sh`, архивы исходников. **Живьём не
> проверено:** несколько дискретных карт и Apple Silicon; HTTPS через TLS-прокси.
>

> **Ветка `server-multiuser`, этап «Автотранскрибация как служба пакета» закрыт (14.10.2026,
> план `PLAN_TRANSCRIBE_SERVICE.md`, R0–R5):** транскрибация+диаризация стала полноправной
> **службой пакета** — 5-я роль `transcribe` (`hermes-disk-search-transcribe`, SCM
> `depend= …-llm-host`, кооперативная остановка через `transcribe.stop`), desktop-ветка —
> задача `HermesDiskSearchTranscribe`, `setup.ps1` получил блок «Automatic transcription»
> (модели whisper+sortformer + папки конвейера в `config.yaml`), `transcribe-watch --help`
> больше не запускает демон (есть `--dry-run`). **Медиа вне индексации**: ASR в конвейере
> индексации убран (`skipped_media`), `index.transcribe` — legacy, папки конвейера
> (`inbox_dir`/`out_dir`/`state_dir`) исключаются неявно (в UI — индикатор «исключена ✓/✗»),
> выходы `.md` в индекс не кладутся (перенос вручную, §8.1). **Кабинет пользователя**
> `/ui/user/` (R4): вход по логину/паролю (`POST /api/auth/login` — контракт `hds login`
> и MCP, антибрутфорс `login_per_min`), три блока (поиск по проектам токена, установка
> клиента Cline, транскрипты/спикеры), разделение гейтов: админские маршруты → `403`
> не-админу, без токена → `401`, не-админ правит `.md`/имена спикеров (`.bak`, path-safety);
> образцы прокси публикуют `/mcp`, `/ui`, `/health`. Проверки: `cargo fmt --check`,
> `clippy -D warnings`, `cargo test --workspace` — зелёные; живая приёмка кабинета
> (`tools/parity/out/r5/`: логин 200/401, rate-limit 429, `403`/`401` матрица, правка `.md`
> не-админом с `.bak`/sidecar, аудит `login_ok|token_issue|login_fail|unauthorized|forbidden|user_op`,
> `form: process` в карточке демона). **Релиз 0.20.0 выпущен (14.10.2026, ветка
> `server-multiuser`, тег `v0.20.0`, `RELEASE_NOTES_v0.20.0.md`):** ассеты — Windows x64
> (`hds-0.20.0-windows-x64.zip` + sha256), оба рантайма движка (CUDA 566 МБ / Vulkan 85,7 МБ),
> macOS (best-effort), клиентские пакеты `hds-client-0.20.0-{windows-x64,macos-arm64}.zip` и
> скрипты `client-install.ps1|sh` (оба пакета вложены в `client/` серверной поставки).
> Первый прогон упал на флейке `queue::tests::fifo_within_user_by_seq` (порядок выдачи
> `[1, 0]` вместо `[0, 1]`): нити спавнились вразнобой, а `seq` выдаётся в момент постановки
> в очередь — тест сделан детерминированным (следующая заявка встаёт в очередь после
> предыдущей; 60 прогонов подряд зелёные), после переноса тега релиз собран заново.
> **Обёртки установки/обновления сервера (15.10.2026):** обновление — один файл, без ручного
> `-Root`/`-Version` и без борьбы с execution policy/MotW: `update.cmd` (Windows) и
> `installers/update.command` (macOS) снимают метку «из интернета», берут корень из новой
> подкоманды `hds service root` (корень читается из `binPath` SCM-службы / плейста LaunchDaemon:
> `hds_core::service::install_root_{binpath,sc_qc,plist}` + 3 теста) и версию из `version.txt`,
> затем зовут `update.ps1 -Profile server`; первичная установка — `install-server.cmd`.
> **Релиз 0.20.1 выпущен (15.10.2026, тег `v0.20.1`, `RELEASE_NOTES_v0.20.1.md`):** PATCH только
> логистики установки — обёртки в архиве (`update.cmd`, `install-server.cmd`, `installers/update.command`),
> подкоманда `hds service root`, `bash -n` в блокирующей CI-джобе, README/RELEASING по установке и
> обновлению. Поведение поиска/транскрибации и схема данных не менялись; живьём проверено: обёртка
> `update.cmd -WhatIf` сама находит корень и версию, `bin\hds.exe service root` из опубликованного
> архива отвечает `C:\Users\Sasha\hds-server`.
> ⚠ Приёмка SCM-службы (`sc query …transcribe → RUNNING` после перезагрузки) требует
> админской сессии и на dev-машине из неадминской сессии не выполнялась.
>
> **Релиз 0.19.0 (13.10.2026, ветка `server-multiuser`, тег `v0.19.0`):** логистика тонкого
> клиента закрыта — установка в **три шага** (админ выдаёт токен → пользователь скачивает
> скрипт-установщик `client-install.ps1|sh` из общедоступного места (токен не нужен) →
> запускает с токеном); скрипт забирает пакет **с сервера**
> (`<mcp-path>/client-package.zip`, платформа по `?platform=`/`User-Agent`, Bearer-гейт как у
> `/mcp`), в поставке лежат **оба** пакета (windows-x64 + macos-arm64), версия продукта
> (`version.txt`, `/health.version`) сверяется мастером (W7). Детали — `RELEASE_NOTES_v0.19.0.md`,
> правила — `RELEASING.md`.
>
> **Релиз 0.18.0 выпущен (13.10.2026, ветка `server-multiuser`, тег `v0.18.0`):** вся
> серверная подсистема M1–M9 в релизе; ассеты — Windows x64, **два** рантайма движка
> (CUDA и новый **Vulkan** для машин без NVIDIA), macOS (best-effort) + исходники. Правила
> выпуска переписаны (`RELEASING.md`), релизный CI — `.github/workflows/release.yml`:
> джоба `check-version` (тег = `__version__`), оба Windows-рантайма **блокирующие**,
> идемпотентная публикация (повторный прогон перезаливает ассеты). Ветка
> `server-multiuser` прогоняется в `rust.yml` — релиз впервые собрался по-настоящему
> зелёным на Linux/macOS/Windows; что нового — `RELEASE_NOTES_v0.18.0.md`.
>
> **M1 «Фундамент доступа» выполнен (07.10.2026, ветка `server-multiuser`):** проекты
> (`index.roots` → `files.project`, переиндексация обязательна), Argon2id-пароли +
> авто-токены (`data/access.yaml`), Bearer-авторизация MCP/UI в server-режиме
> (`access.enabled`), фильтрация по проектам во всех ветках поиска (тесты на утечку),
> admin-гейтинг индексации, `hds user|token|login`, карточка «Доступ» в UI.
> Детали — `PLAN_M1_ACCESS_PROJECTS.md` (исполнение), `PLAN_SERVER_MULTIUSER.md` §9.
>
> **M2 «Серверный профиль и службы» начат (07.10.2026, ветка `server-multiuser`):**
> трек **T0** (дефекты M1 из код-ревью) закрыт — UI парсит `Authorization`
> (server-гейт проверен рантаймом: 401 без токена / 200 с админ-Bearer; сокет-тест),
> `data/access.example.yaml` в репозитории (`data/*` + negation, живой `data/`
> игнорируется), `hds login` строит тело через `serde_json::json!` (проверено живым
> сервером с `"`,`\` в пароле). **T1** — профиль `desktop|server` (`hds_core::config::profile`,
> пункт `profile` в `hds check`: server без `access.enabled`/`ui` → warn). **T2** —
> каталог состояния (`state_dir`: server → `%ProgramData%\hermes-disk-search` /
> `/Library/Application Support/...`, `HDS_ROOT` сильнее всего; БД, `access.yaml`,
> PID/логи/heartbeat — через резолвер; фолбэк конфига в системный каталог). **T3** —
> `hds ui`/`hds_ui` слушают `ui.host:ui.port` из конфига (флаги переопределяют).
> **T4** — службы: `hds service run|install|uninstall|start|stop|status <role>`
> (watch/mcp-http/ui/llm-host); Windows — SCM через крейт `windows-service`
> (автостарт + авторестарт), macOS — LaunchDaemon; `installers/install_server.ps1`
> (AtStartup-фолбэк, снимает десктопные AtLogOn-задачи) и `install_server.command`.
> **T5** — headless-UI (`run_ui.ps1` в server-профиле не открывает браузер) +
> README «Серверный профиль и службы». Все шаги проверены рантаймом (полный
> SCM-цикл — только из-под администратора, см. PLAN_M2 §4 T4); десктоп-поведение
> не изменилось. Детали — `PLAN_M2_SERVER_PROFILE.md` (§4, коммиты).
>
> **M3 «Сетевой периметр» выполнен (07.10.2026, ветка `server-multiuser`):**
> **T0** — фиксы дефектов M2 из код-ревью: `DisplayName` службы без литерала
> `<role>`, `run_ui.ps1` берёт `ui.host/ui.port` из конфига (без жёсткого
> `--port 8765`, проверено e2e в песочнице: UI слушает порт конфига),
> `HDS_ROOT` переносит состояние резидента даже при `profile: server`.
> **T1** — reverse-proxy/TLS: образцы `installers/proxy/{Caddyfile,nginx.conf}`
> (наружу только `/mcp` и `/ui`; фасад `:8010–8012` не публикуется), ключ
> `trusted_proxy` — без него заголовки прокси игнорируются (подделка IP
> невозможна), при нём реальный клиент — правая запись `X-Forwarded-For`;
> `/health` MCP отдаёт `client_ip` (живая диагностика прокси); опциональный
> шаг прокси в `install_server.*`. **T2** — аудит: `hds-core::audit`, JSONL
> `<state_dir>/data/logs/audit.jsonl` с ротацией по размеру (`audit.max_mb`);
> события `login_ok`/`login_fail`/`unauthorized`/`tool_call` (кто/что/проекты)/
> `admin_op`/`token_revoke`; server — включён по умолчанию, desktop — выключен;
> пункт в `hds check` и `/api/diagnostics` (проверено живым сервером).
> **T3** — rate-limit: token-bucket в `hds-core::ratelimit`, ключ — токен
> иначе IP; `/auth/login` жёстче (по IP, антибрутфорс); `max_inflight` на
> `ask_my_files`; `429` + `Retry-After` + `X-RateLimit-Limit` (проверено
> живьём: burst 5 → 5×200 + 429). **T4** — hardening: `http.max_body_kb` →
> `413` без чтения тела, `http.max_conns` → короткий `503` без спавна потоков
> (ревью F8), единый JSON ошибок `{error,hint}`, F2: `ui_auth_gate`
> fail-closed в server-поставке при нечитаемом конфиге. Десктоп-профиль не
> изменён (аудит/лимиты выключены, заголовки прокси игнорируются). Детали —
> `PLAN_M3_NETWORK_PERIMETER.md` (§4, журнал), `README_SERVER_MULTIUSER.md` §11.
>
> **M4 «Конкурентность и SLA ≤5 мин» начат (08.10.2026, ветка `server-multiuser`):**
> трек **T0** (хвосты код-ревью M3/M2) закрыт коммитами `258cb74`…`4ba31ec`:
> `token_issue` в аудите (+ отпечаток токена), `admin_op` с фактическим статусом,
> UI-`unauthorized` с реальными method/path, доставка `503`/`413` (дочитывание
> запроса + сокет-тесты, ловящие RST), определение «тяжёлого» запроса разбором
> JSON-RPC, `/health` вне rate-limit, пункт «периметр» в `hds check`/диагностике,
> PID-файлы `mcp-http` с проверкой личности процесса, мелочи F10 (версия в
> `/health`, заголовки `X-RateLimit-*`, read-timeout сокета, 404 без эха пути) и
> хвосты M2 (логи LaunchDaemon, тест `role_from_args`, `access_path_from`, тест
> паритета `system_state_dir`). Открыто: F9 (`Host`) и вопросы заказчику
> (автотранскрибация в server-профиле, пароль в argv `hds login`). Трек **T1**
> (БД под конкурентность) закрыт `0395ab7`: ключи
> `db.{busy_timeout_ms,journal_mode,read_only_conns}` (server: 5000/wal/true;
> desktop: 30000/false — прежнее поведение), разделение соединений `open_rw`
> (единственный писатель) / `open_ro` (читатели read-only), `hds check` с
> фактическими PRAGMA, стресс «5 читателей + писатель» без `SQLITE_BUSY`
> (debug+release) + рантайм (писатель `hds index` + читатели — 0 «locked»).
> Трек **T2** (слоты чата и VRAM) закрыт `c07a367`: `llm_server.parallel` — дефолт по
> профилю (server **5** / desktop 1, явное значение сильнее); `n_parallel` теперь
> **доходит до инстанса движка** (до правки поле не выставлялось вовсе — движок
> поднимал 1 слот, и 5 запросов шли строго по очереди, хотя `/props` показывал 5);
> шлюз движка переведён с `Mutex` на `RwLock` — **генерация** (chat/embeddings/rerank)
> идёт разделяемо (параллельно), управление кластером (загрузка/выгрузка/арбитр) —
> эксклюзивно, `Busy` показывает число держателей; на старте — **fit-гейт** с явной
> ошибкой «нужно/свободно/резерв» и подсказкой (KV считается от контекста слота, не
> завышается в `parallel` раз); `hds check` показывает фактические слоты и настроенный
> `parallel` (расхождение → warn). Замеры 5 одновременных чат-запросов на RTX 3060
> (`tools/parity/out/m4_slots_*.json`): 2 слота × 32768 → wall 33,7 с; 3 × 32768 → 27,1 с;
> 5 × 16384 → **21,3 с** (все параллельно); 5 × 32768 и 5 × 65536 — **отказ** гейта с
> числами. Числа — в `README.md` (§«Слоты и SLA») и `config.example.yaml`.
> Трек **T3** (очередь тяжёлых RAG и fair-share по пользователю) закрыт `dd2e9af`:
> `hds-core::queue` — round-robin между пользователями и FIFO внутри (политика
> `queue.policy: rr|fifo`), глубина ожидания (`max_depth`) и потолок
> (`wait_timeout_sec`), лимит исполнения остаётся `rate_limit.max_inflight`;
> интеграция в `serve_request` — запрос ЖДЁТ слот: очередь полна → `429`,
> таймаут ожидания → `503` (оба с `Retry-After` + `X-Queue-Depth`), обрыв клиента
> освобождает место, ожидание пишется в аудит (`queue_wait`); метрики (глубина,
> ожидание p50/p95, отказы, «кто кого ждёт») MCP-процесс пишет снапшотом
> `data/mcp-queue.json`, пункт `queue` в `hds check`/`/api/diagnostics`
> предупреждает при отсутствии/устаревании статуса. Дефолты: server — включена
> (20/240/rr), desktop — выключена (поведение M3). Проверено: 9 юнит-тестов
> `hds-core::queue` (в т.ч. потоковый fair-share и fifo), сокет-тесты
> `queue_socket.rs` (503/429 доходят до клиента, desktop-поведение), живой прогон
> `tools/parity/out/m4_t3_live.ps1` (реальный индекс + резидент: `503`
> `X-Queue-Depth=1`, `429`, ожидание p50/p95 3155 мс, `hds check` → ok с числами,
> после остановки MCP → warn «статус устарел»).
> Трек **T4** (квоты и приоритеты по пользователю) закрыт `dd093fc`:
> ключ лимитов/квот сменился с токена на **пользователя** (`user:<login>` →
> `ip:<addr>`), поэтому два токена одного пользователя делят одну квоту
> (регресс M3 F10.3); `per_token: true` возвращает пер-токенный ключ, но частота
> дополнительно ограничивается агрегатом `requests_per_min_per_user`. Новые
> ключи `rate_limit.{max_inflight_per_user (2), requests_per_min_per_user,
> per_token}`; пер-пользовательский предел исполнения — `429`+hint в очереди
> (`QueueReject::PerUser`) и в лимитере без очереди; `rejected_user` +
> `max_inflight_per_user` в снапшоте и `hds check`. Проверено юнит- и
> сокет-тестами (`quota_socket.rs`: пер-пользовательский `429` доходит до
> клиента; два токена делят квоту).
> Трек **T5** (нагрузочная приёмка) закрыт harness `tools/parity/load5.py`
> (`1e4d5c5`): 5 одновременных РЕАЛЬНЫХ `ask_my_files` через MCP `/mcp` на
> песочном индексе и реальной карте. `parallel 5 / ctx 16384` → все `200`,
> p50 25,7 с, **p95 49,6 с** (SLA ≤5 мин выполнен), VRAM 8916→9669 МиБ;
> `parallel 2` → p95 58,1 с, `parallel 3` → p95 48,9 с (очередь реально ждёт,
> отказов/таймаутов 0); индексация параллельно → все `200`, но p95 315 с и
> «database is locked» нет (индексатор уступает: 25 с → 333 с). **Остаточный
> риск:** при 5 занятых слотах + боевом наборе сервисов наблюдается одиночный
> выброс ~305–315 с на одном запросе (загрузка/перепланирование on-demand роли
> `embedding`) при 4 быстрых (~27 с) — нужен отдельный разбор (кандидаты:
> retention `keep` для embedding, тёплый прогрев перед приёмом тяжёлых задач).
> Чек-лист внешнего HTTPS — на машине с доменом/сертификатом (вне dev-стенда).
> Трек **T6** (доклад) — этот коммит: `README.md` §«Слоты и SLA»,
> `README_SERVER_MULTIUSER.md` §8/§11, `STATUS.md`, `PLAN_SERVER_MULTIUSER.md` §9,
> журнал `PLAN_M4_CONCURRENCY_SLA.md` §8a, закрытие `M3_CODE_REVIEW.md` (T0).
> **Этап M4 закрыт** (T0–T6). Открыто осознанно: F9 (`Host`) и вопросы заказчику
> (T0.9), выброс p95 T5 (см. выше). Детали — `PLAN_M4_CONCURRENCY_SLA.md` §8a.
> **Промт для следующего чата — `M5_HANDOFF_PROMPT.md`** (M4 закрыт; точка входа,
> хвосты, контракты T3/T4, грабли, порядок возврата сервисов, перезапуск стенда T5).
>
> **Код-ревью M4 проведено (09.10.2026) — `M4_CODE_REVIEW.md`:** M4 (T0–T6) —
> **принять с условиями**. Код закрыт и проверен независимо (`cargo test --workspace`
> = 369 passed / 0 failed, `fmt`/`clippy` чисто; T1–T4 подтверждены рантаймом:
> PRAGMA на живой БД, сокет-тесты `429/503`/квот, снапшот очереди, `/props` с
> фактическим `n_parallel`), но **приёмка T5 «SLA ≤5 мин» не воспроизводима**: чистый
> повтор `parallel 5 / ctx 16384` (`tools/parity/out/m4_load_p5_final.json`, без
> индексации и без `--no-rerank`) дал p95 **315,5 с** > 300 с (`sla_ok=false`); за
> порогом **3 отчёта из 5**, в каждом — один запрос ~315 с при `granted=5`,
> `total_wait_ms=0` (очередь не ждала → стопор в движке/слоте; таймаут клиента 600 с,
> так что это не обрыв). Другие находки: **F2** — аутентификация применяется ПОСЛЕ
> очереди (неавторизованный тяжёлый запрос ждёт слот до 240 с и занимает `max_depth`);
> **F3** — дефолты server (`parallel 5` при `rate_limit.max_inflight 4`) дают 4
> одновременных RAG, а не 5; **F4–F9** — полировка очереди, док ≠ код про «FIFO внутри
> пользователя», мёртвый дубликат `Limiter::inflight_*`, `Retry-After` таймаута,
> allowlist `journal_mode`, покрытие. Открытые хвосты F1–F9 перенесены в
> `M5_HANDOFF_PROMPT.md` («Что осталось») — разбирать до новых фич; статус M3-хвостов и
> вопросов заказчику без изменений (см. `M4_CODE_REVIEW.md` §5).
>
> **Этап M5 — «Хвосты M4 + внешняя чат-модель remote» (T0-хвосты + Э6/W9) выполнен
> (10.10.2026, детали и коммиты — `PLAN_M5_EXTERNAL_CHAT.md` §8a; статус —
> `PLAN_SERVER_MULTIUSER.md` §9).** Согласовано с владельцем:
> **M5 = T0-хвосты M4 + `chat.provider: remote`** (дорожная карта §5, контракт —
> `PLAN_M4_CONCURRENCY_SLA.md` §3.5). Закрыто к 10.10.2026:
> **F1 (SLA) — root-cause найден и устранён**: три прогона до фикса дали
> 315,6 / 315,7 / 46,5 с (выброс вероятностный 2 из 3); по логам резидента —
> on-demand **перезагрузка `embedding`** при тесной VRAM, fit-шаг движка
> `llama_params_fit_impl` (~1900 МиБ свободно) завершается **~299 с** (рост ×~8 с
> каждой загрузкой: 0,57 → 4,2 → 27,3 → 299; «−» — баг печати движка); unlucky-запрос
> ждал перезагрузку целиком (315 ≈ 299 + 16), очередь не ждала (`total_wait_ms=0`).
> Фикс: server-профиль — embedding/rerank **`retention: keep`** по умолчанию +
> **прогрев** embedding/rerank при старте `llm-host` (desktop не меняется). Приёмка
> **3/3 в SLA: 34,1 / 34,0 / 33,9 с** (`m4_load_f1_fix_r1..r3.json`). Хвост: рост
> fit-шага от числа загрузок — кандидат в `engine-patch` (P5).
> **F2** — auth ДО очереди (`401` мгновенно, аудит сохранён) + сокет-тесты
> (в т.ч. `Aborted` освобождает место — F9-покрытие). **F3** — дефолт
> `rate_limit.max_inflight`: server **5** (=`parallel`), desktop 4; `hds check`
> warn'ит рассинхрон. **F4–F8** — FIFO внутри пользователя по `seq`,
> `notify_all` при постановке, `Retry-After` от `wait_timeout`, allowlist
> `journal_mode`, удалён мёртвый путь `inflight_*`. **Remote-задел (T1–T4)**:
> `chat.{provider,api_key}` (дефолт `local`), TLS-клиент rustls (только исходящие),
> Bearer на remote-запросы, llm-host не планирует chat-роль при `remote`, пункт
> `chat_provider` в `hds check`; приёмка стенда `load5.py --chat-remote fake`:
> 200×5, Bearer доходит (5/5), **VRAM 1102 → 1121 МиБ** — chat-роль освободила
> ~7,8 ГиБ VRAM («рычаг SLA»). Тесты: `cargo test --workspace` зелёный (87
> наборов), fmt/clippy `-D warnings` чисты. Десктоп-профиль не изменён.
> Не закрыто в M5 (следующий шаг): анонимизация W9, Э5 (GPU multi-vendor),
> чек-лист внешнего HTTPS, вопросы заказчику T0.9, F9 (`Host`).
> Формально закрыт 10.10.2026: приёмка §5 плана M5 сверена с фактом —
> `cargo test --workspace` 87 наборов / 0 failed, fmt/clippy `-D warnings` чисты;
> F1 — 3/3 в SLA; remote — 200×5 (p95 6,8 с), VRAM ~1,1 ГиБ; `hds check`
> (`chat_provider`) — ok на боевом desktop-конфиге. Выбор следующего этапа —
> за владельцем (Э5 «GPU per-role/multi-vendor» / анонимизация W9 / патч
> движка P5 — рост fit-шага).
> **Код-ревью M5 проведено (10.10.2026) — `M5_CODE_REVIEW.md`: принято без условий**
> (независимые рантайм-проверки: 87 наборов / 0 failed, пересчёт отчётов стенда,
> повторные прогоны `m5rev_r1` — p95 33,8 с и `m5rev_remote` — 200×5/Bearer 5/5/
> VRAM ~1,1 ГиБ; секрет в `hds check` не печатается). Некритичные замечания
> M5-1…M5-3 и хвосты — §3/§5 ревью.
>
> **Этап M6 — «GPU: раскладка ролей по картам» (Э5/W4) в работе (10.10.2026).**
> **Ревизия требований владельца (10.10.2026):** прод — несколько **дискретных** GPU одного
> семейства (NVIDIA / AMD) либо один Mac (Metal); смешанные вендоры и iGPU/UMA — вне
> поддержки (warn); чат-реплики одинаковые (масштаб под пользователей), индексация —
> **один** экземпляр, транскрибация — экземпляр на карту, один фасад с внутренней
> маршрутизацией. План — `PLAN_M6_GPU_MULTIVENDOR.md` (**ревизия 2**); актуальный промт —
> `M6_HANDOFF_PROMPT_4.md` (T0/T1/T2/T3 закрыты; промты 1–3 устарели).
> **Закрыто:** **T0** — спайк **GO** (Vulkan-бэкенд берём только из нашего дерева: чужие
> `ggml-vulkan.dll` несовместимы — b11528 `api_version=3`, b11200 крашит процесс, апстрим
> сменил layout `ggml_backend_buffer_i` без бампа версии) — `39980b7`, `d635d03`;
> **T1** — `gpu.devices` + `llm_server.<role>.device`, валидация с числами, пункт `gpu_roles`
> в `hds check` (`a919a78`); **T2** — раскладка в реестре/хосте, чат-реплики, whisper по
> картам, один фасад (`7469d72`); **T3** — бюджет и вытеснение **по картам**
> (`f14b226`): `role_cards` из плана, `free_for_role` (проба ↔ числа движка по карте
> роли), карт-скоуп вытеснения и предохранителя простоя, `cards` в `status`/`/internal/status`;
> рантайм: лог «бюджет и вытеснение по картам ролей: …», `cards: ["карта 0 (CUDA, …)"]`,
> живой запрос к фасаду 200, на одной карте числа не поехали.
> **Независимая верификация T1 (второй чат, 10.10.2026):** `cargo test --workspace` —
> **88 наборов / 0 failed**, fmt/clippy `-D warnings` чисто; пробы `gpu_roles`: роль по
> `llm_server.<role>.device` → ok, индекс вне диапазона → warn с числами, компат
> `gpu.device_index` → ok. Замечания (низкие): косметика «CUDA ,»; при недоступном резиденте
> диапазон карт не проверяется. Дальше — **T4** (пресеты и автодетект).
> **T4 — пресеты раскладки и автодетект (10.10.2026).** Новые ключи `gpu.preset`
> (`none|nvidia-one|nvidia-n|amd-n|apple`, вендор-гомогенные) и `gpu.autodetect`
> (по умолчанию **false** — раскладка только по явному ключу). `crates/hds-llama`:
> `config::GpuPreset` (+`parse`/`autodetect`/`layout`/`required_vendor`) и `AccelFacts`
> (число карт + вендоры из устройств движка); `GpuConfig::{preset,autodetect}`,
> `effective_layout` (приоритет: явная `gpu.devices` → `gpu.preset` → `gpu.autodetect`
> → компат `gpu.device_index`) и `preset_note`; `device::vendor_of`; реестр строит
> раскладку через `effective_layout` (ключ роли `llm_server.<role>.device` перекрывает
> пресет; для явной `gpu.devices` двойное назначение — по-прежнему fail-fast);
> fail-fast с числами: пресет без карт или на «чужих» картах (напр. `nvidia-n` на AMD).
> Пресет раскладывает **существующие** роли (реплики `chat-N`/`whisper-N` — явно, как в T2):
> `nvidia-one`/`apple` — всё на первую карту; `nvidia-n`/`amd-n` — чат-реплики и whisper
> round-robin по картам, индексация (`embedding`/`rerank`) — на выделенной карте.
> Печать с числами: лог старта `llm-host` («пресет: …»), `llm_host_plan` (+`preset`/
> `autodetect`/`devices` в JSON), `hds check` — пункт `gpu_roles` даёт подсказку
> (число ускорителей/бэкендов, уместный пресет, что смешанные вендоры/UMA вне
> поддержки), **статус не меняет** (боевой `hds check` остаётся ok). Тесты: `config.rs` +2
> (резолв пресета + override `gpu.devices`; `autodetect` off/on), `diag.rs` +1
> (`preset_hint_text` по вендорам/числам), `gpu_roles_check_statuses` +подсказка;
> `cargo test --workspace` — **88 наборов / 0 failed**, clippy `-D warnings`/fmt чисто.
> Рантайм: `llm_host_plan` на песочном конфиге (`gpu.preset: nvidia-n`) → «пресет:
> gpu.preset=nvidia-n (ускорителей: 1, вендоры: nvidia)» + раскладка с числами
> (`chat→0, embedding→0, rerank→1(CPU), chat-2→0`; одно карта — распределение по картам
> проверено юнитами); `hds check --json` (боевой) → `gpu_roles` **ok** + «уместен
> gpu.preset=nvidia-one»; на песочном конфиге → ok + «задан gpu.preset=nvidia-n»;
> стенд возвращён (`:8010`/`:8787` → 200, `index.pause` цел). Артефакты —
> `tools/parity/out/m6_t4_*`. **Хвост:** распределение по 2+ картам живьём не проверено
> (нет железа) — юниты + хвост ревизии 2.
> **T5 — рантайм-приёмка (10.10.2026, приоритет владельца — транскрибация).** Живой стенд:
> throwaway-владелец (порты 8020/8021; боевые 8010–8012 и `index.pause` не тронуты), конфиги/проба/отчёт —
> `tools/parity/out/m6_t5_{owner,accept}.yaml`, `m6_t5_direct.py`, `m6_t5_report.json`. Модель
> whisper-large-v3-turbo-GGML.bin (1549 МиБ); раскладка: `chat/embedding/whisper → карта 0 (CUDA)`,
> `rerank → карта 1 (CPU)`; порог whisper = модель + буфер 256 + резерв 1024 = **2829 МиБ**.
> Прогоны (6, ok 5): 60-сек файл **рядом с загруженным чатом** (свободно 3227 ≥ 2829 → GPU) — 10,48 с,
> 14 сегментов, пик занято ~**11028 МиБ**; JFK 2,45 с; прямой `POST /internal/transcribe` **без** `gpu`
> → host-выбор («выбрана карта 0 … 3227 ≥ 2829») 6,8 с; после выгрузки чата 6,35 с (пик ~**2983 МиБ**);
> mp4 без аудио → ожидаемая ошибка «ffmpeg: no audio stream found»; JFK 2,62 с. **Паралича резидента
> нет** (`/health`/`/internal/status` отвечали во время и после; R1 — 25 опросов подряд). Боевой
> `index.pause` переиспользован и **не снимался**. `hds check` — 17 ok / 1 warn (`gpu-manual`),
> `gpu_roles` ok. **Находка F-m6-t5-1 (medium):** штатные клиенты (`hds whisper-check`/`auto_transcribe`)
> **всегда** шлют `gpu` → host-выбор карты (`pick_whisper_card`, §3.6) не срабатывает; на N картах все
> файлы уйдут на одну (рекомендация и решение — за владельцем). Стенд возвращён (боевой `config.yaml`,
> chat LOADED `card=0 backend=CUDA` n_ctx=65536, `:8010`/`:8787` → 200).
> **T6 — доклад (10.10.2026).** `README.md` (раздел «Несколько GPU, вендоров и пресеты раскладки (M6)»),
> `README_SERVER_MULTIUSER.md` (§8b «Несколько карт одного семейства»), `engine-patch/README.md`
> (сборка Vulkan-бэкенда), `PLAN_SERVER_MULTIUSER.md` §9 (статус M6), `config.example.yaml` (пресеты).
> **Этап M6/Э5 закрыт** (T0–T6). **Хвосты:** мультикарточность живьём не проверена (нет железа) —
> юниты + «машина с N картами»; Apple Silicon — без живой проверки; дискретной AMD нет (Vulkan-путь
> частично); F-m6-t5-1 (host-выбор карты под файл не срабатывает у штатных клиентов) — ждёт решения
> владельца; рост fit-шага движка (кандидат `engine-patch` P5).
>
> **Требования Э5 пересмотрены владельцем (10.10.2026, в ходе работы над T0).** На проде
> одновременно используются **несколько дискретных GPU только одного семейства**: либо
> одна/несколько дискретных карт **NVIDIA** под Windows, либо одна/несколько дискретных
> **AMD** под Windows, либо **один Mac с Metal**. Смешанные вендоры на одной машине на проде
> не встречаются (наша станция «AMD iGPU + NVIDIA dGPU» — непродовая). Железо для проверки
> одновременной работы нескольких карт владелец предоставить не может → мультикарточность
> принимается **без живой проверки** (явный хвост на машину с несколькими картами).
> Следствия для M6: сценарий «CUDA+Vulkan в одном `llm-host`» из приёмки T0 снимается;
> T0 переориентируется на «Vulkan-бэкенд доступен сборке, и роли AMD-инсталляции реально
> идут на Vulkan-карты»; выбор карт/пресеты — **вендор-специфичные** (пин по `backend+name`;
> компат `gpu.device_index` не ломаем); UMA/iGPU-сценарий AMD (общая память) — вне
> прод-области. Нормативные разделы `PLAN_M6` §2/§4 T0 пересматриваются после подтверждения
> владельцем формулировки (журнал §8a — ниже).
>
> **Уточнения владельца (10.10.2026, ответы на 6 вопросов):** (1) смешанные вендоры **не
> поддерживаются**; (2) AMD и NVIDIA — **только дискретные** карты (iGPU/UMA вне области);
> (3) поставка — **отдельными сборками** под NVIDIA / AMD / Metal; (4) несколько карт —
> ради **одновременного обслуживания разных пользователей** (то же может дать одна карта
> 24 ГБ с двумя чат-моделями); **индексация — единственный экземпляр** и «один набор моделей
> на один файл одновременно»; **транскрибация — экземпляр на карту** как лёгкая добавка к
> чат-моделям; (5) Mac/Metal — подготовлен без живой проверки, один GPU; (6) план обновлять
> **разрешено** — нормативные разделы `PLAN_M6` переписаны (ревизия 2: §2 границы, §3.4
> пресеты, новый §3.5 «реплики/лёгкие добавки», T0/T2/T3/T5, §5 приёмка, §7 риски).
> Стенд: владельца сейчас интересует прежде всего **транскрибация** (использует изредка) —
> приёмка T5 переориентирована на неё; боевой резидент возвращён в работу, `index.pause` не снят.
>
> **M6/T0 закрыт — GO (10.10.2026, промт `M6_HANDOFF_PROMPT_2.md`).** `ggml-vulkan.dll`
> собран из нашего дерева (`ENGINEbuilds`; скрипт `tools/parity/out/build_ggml_vulkan.cmd` +
> `-DLLAMA_BUILD_{SERVER,TOOLS,COMMON}=OFF`; 56 МБ; импорты ⊆ нашей `ggml-base.dll`, 0 missing)
> и подложен в песочницу `tools/parity/out/engine-vulkan` (чужие сборки непригодны — краш без
> бампа `GGML_BACKEND_API_VERSION`, §8a). С Vulkan-бэкендом движок перечисляет CUDA0 /
> Vulkan0=AMD iGPU (UMA) / Vulkan1=RTX 3060 / CPU — ловушка компата подтверждена: индекс 2
> теперь AMD → пин по `backend+name` обязателен (T1). `a7_multiinstance_probe --no-chat`:
> embed (bge-m3) на Vulkan0 — 2/2 раунда без ошибок (load 2,1–3,1 с, embeddings 183–243 мс,
> unload ~0,3 с, NVML не тронута). В пробу добавлено `embed_spec.embedding = Some(true)`
> (без него 501 «server does not support embeddings»). Отчёт — `tools/parity/SPIKES.md` §15;
> числа — `tools/parity/out/m6_spike_vulkan_embed.json`. Свидетельство на iGPU (дискретный AMD
> — хвост). Следующий шаг — **T1**: конфиг `gpu.devices` + `llm_server.<role>.device`,
> компат-пин по `backend+name`, валидация §3.5, пункт `gpu_roles` в `hds check`.
>
> **M6/T1 выполнен (10.10.2026).** Конфиг: `gpu.devices: [{ index, roles }]` +
> `llm_server.<role>.device` (`llm.<role>.device` приоритетнее); приоритет карты:
> role-ключ → `gpu.devices[].roles` → компат `gpu.device_index` (десктоп не изменён).
> Валидация §3.5 — fail-fast с числами: «одна роль — ровно одна карта» (двойное
> назначение → `RolePlan::Failed` с перечнем источников), недоступная карта → «роль 'X':
> … ускорителей найдено N». В плане/статусе — `backend=` карты
> (`PlannedInstance.device_backend`, `summary`). Пункт `gpu_roles` в `hds check`
> (без HTTP, если раскладки нет): с раскладкой — живые устройства фасада
> `/internal/devices` (+ поле `description`), warn на недоступную карту, двойное
> назначение, смешанные вендоры и iGPU/UMA. `config.example.yaml` — образец.
> Тесты: config.rs +2, `registry_plan.rs` +4 (на устройствах T0: CUDA0/Vulkan0/Vulkan1/CPU),
> diag.rs +1; clippy/fmt чисто. Рантайм: `llm_host_plan` с песочной раскладкой
> (chat → `backend=CUDA` + note «явная карта 1»; embedding → fail-fast «ускорителей
> найдено 1 (CUDA0)» — боевой движок без Vulkan); `hds check --json` живьём —
> `gpu_roles` ok, боевой desktop-профиль не изменён. Следующий шаг — **T2**:
> раскладка в `host.rs`, чат-реплики, whisper по картам, один фасад + внутренняя
> маршрутизация (§3.6).
>
> **M6/T2 выполнен (10.10.2026).** Per-role раскладка в реестре/хосте (§3.5/§3.6).
> Конфиг: роли-реплики `chat-N`/`whisper-N` (модель наследуется от базовой, свой
> порт/контекст; дефолт порта чат-реплики `8010+N`), `LlmHostConfig.errors` —
> fail-fast §3.5 **с числами** (реплика индексной роли `embedding-2`/`rerank-2`,
> дубли портов ролей): `Host::start` и `llm_host_plan` не запускаются с текстом.
> `registry.rs`: `PlannedInstance.bridge_device_index` (+`card=` в `summary`),
> `role_base` в слотах/CPU-роли, `cross_validate` — whisper не более одного
> экземпляра **на карту** (роли и карта названы), индексация — один экземпляр.
> `host.rs`: печать раскладки «роль → карта/backend» при старте (`layout_line`),
> реплики чата в `ClusterBackend` + маршрутизация **least-busy → round-robin**
> (`pick_replica` — чистая функция + юнит), наружу только базовые порты (один
> фасад `chat` → 8010; порты реплик внутренние/справочные), статус печатает
> `card=`/`backend=` (и в JSON). Whisper: транскрибаторы **по картам**
> (`BTreeMap<bridge, Arc<WhisperCell>>` — один файл на карту, разные карты
> параллельно), карту под файл выбирает хост по свободной VRAM (`pick_whisper_card`,
> числа в логе), явный `gpu` в теле сильнее. Тесты: config.rs +4, `registry_plan.rs`
> +4, host.rs +1 — зелёные; clippy `-D warnings`, fmt чисто. Рантайм
> (`llm_host_plan`, песочные конфиги): раскладка «chat → карта 0 (CUDA), embedding →
> 0 (CUDA), rerank → 1 (CPU), chat-2 → карта 0 (CUDA)» (chat-2: порт 8030, та же
> модель, `retention=keep`); `embedding-2` → exit 1 с текстом §3.5;
> `whisper`+`whisper-2` на одной карте → роль не спланирована с числами; боевой
> desktop-конфиг — раскладка не изменилась, exit 0. Следующий шаг — **T3**:
> VRAM-бюджет и вытеснение **по картам** (учёт реплик на одной карте).
>

> Ветка миграции **`w2-llm-host`** (`main` = `9ed8452` не тронут). **Миграция завершена: W1–W5.**
>
> **Релиз v0.13.0 (02.10.2026):** CI переписан под Rust-first (Windows — блокирующий,
> macOS — best-effort), опубликован релиз с ассетами **Windows и macOS**; правила выпуска —
> `RELEASING.md`, журнал — `W4_REPORT.md` §17.
>
> **Сейчас:** ядро и резидентные компоненты — Rust (`crates/`): `hds-search` (поиск/RAG),
> `hds-mcp` (stdio + http), `hds-ui`, `hds-llama` (`llm-host` — владелец портов 8010–8012 и GPU,
> VRAM-диспетчер, ASR), `hds-index` (обход/конвейер/watcher/`db-move`/`diag`), `hds-clip` (ONNX),
> `hds-cli`. **Python остался только как `sidecar`** (извлечение/лемматизация; самодостаточный
> через `installers\build_sidecar.ps1`; `.mpp` — Java 11+). Python-ядро и Python-джобы CI удалены.
> `cargo test --workspace` — **174 passed / 0 failed (+9 `#[ignore]`)**; `fmt`/`clippy -D warnings`
> — блокирующие. `README.md` переписан под Rust-first; передача — `tools/parity/W4_REPORT.md §0`.
>
> **W4 начат (01.10.2026):** CI `rust.yml` (test-rust + build-windows) и
> `installers/build_rust_release.ps1` (стейджинг `dist\hds-<ver>-windows-x64\` с
> `hds/hds_mcp/llm_host` + sha256) — журнал `tools/parity/W4_REPORT.md` §1.
> **W4 шаг 1 (02.10.2026):** установщик под Rust-бинарники (`setup.ps1`/`install_windows.ps1`,
> ASCII-only English): `bin\hds.exe`, winget-зависимости, проверка sidecar-воркера, рантайм
> движка (`installers\fetch_engine_runtime.ps1` + `runtime-manifests\engine-manifest.json`
> v1.15, sha256), GGUF/whisper (`fetch_llm_models.ps1`/`fetch_whisper_model.ps1`), автозапуск
> `llm-host` (`install_llm_host_task.ps1`, теперь `bin\llm_host.exe`) + watcher/MCP
> (`install_autostart.ps1`), интеграции Hermes/Cline → `bin\hds_mcp.exe`/`mcp-http restart`,
> ярлык, `hds check` — `W4_REPORT.md` §2.
> **W4 шаг 2 (02.10.2026):** ONNX-модели CLIP — `runtime-manifests\clip-manifest.json`
> (release-ассеты `clip-onnx-v1`, sha256) + `installers\fetch_clip_models.ps1`
> (+`publish_clip_models.ps1` для мейнтейнера); `default_onnx_dir` → `models\clip_onnx`
> (dev-фолбэк `tools\parity\out\clip_onnx`) — `W4_REPORT.md` §3. Тесты — 170/0 (+9 ignored).
> **W4 шаг 3 (02.10.2026):** `package` — `installers\build_rust_release.ps1` (полная раскладка
> `dist\hds-<ver>-windows-x64\` + zip + sha256); `release.yml` переписан (test-rust, build-windows
> + smoke, `fetch-engine-runtime`, `release` с бинарными ассетами; Python-джобы оставлены до W5);
> `releasing.md` обновлён — `W4_REPORT.md` §4.
> **W4 шаг 4 (02.10.2026):** UI-дополнения — `/api/diagnostics` = полный `hds check`
> (`hds check --json`; общий модуль невозможен из-за цикла hds-cli→hds-ui), кэш `/api/tree`
> (TTL 30 с, `?refresh=1`), `installers\install_ui_task.ps1` (+ опция автозапуска UI в setup.ps1)
> — `W4_REPORT.md` §5. Тесты — 170/0 (+9 ignored). Дальше — W5 (fmt/clippy, `-D warnings`).
> **W5 (очистка, 02.10.2026):** `cargo fmt --all` (104 файла) + clippy **61 → 0**; в CI
> (`rust.yml`, `release.yml`) `cargo fmt --check` и `clippy -D warnings` — **блокирующие**.
> Тесты — 170/0 (+9 ignored). `W4_REPORT.md` §6. **W4 закрыт** (шаги 1–4 + W5).
> **Доп. (02.10.2026):** диагностика перенесена в `hds-index::diag` — UI зовёт `run_checks`
> **in-process** (убран запуск `hds.exe check --json`); добавлен `NOTICE.md`. Тесты — 170/0.
> `W4_REPORT.md` §7. Остаток: сплит `hds/` для self-contained sidecar (`build-sidecar`),
> версионные каталоги `app\<ver>` (§10.6).
> **Сплит `hds/` (02.10.2026):** воркер самодостаточен — `installers\build_sidecar.ps1`
> собирает `sidecar\` (портативный CPython + deps + **копия** модулей `hds\` под `sidecar/hds`;
> воркер кладёт `sidecar\` в `sys.path`); джоба CI `build-sidecar` → `build-windows
> -SidecarDir`. Живой прогон: сборка + воркер на встроенном Python 3.12.14 (hello/extract);
> Python-тесты 269 OK, Rust 170/0. `W4_REPORT.md` §8. Остаток: `.mpp` (jpype/mpxj+JDK),
> `build-macos`, версионные каталоги §10.6.
> **Корень проекта в поставке (02.10.2026):** `project_root()` больше не build-time —
> `HDS_ROOT` → рядом с exe (`bin\`/`target\{debug,release}\{deps\}`) → build-time (dev).
> Иначе распакованная сборка искала `config.yaml` по пути машины сборки (блокер поставки).
> Плюс `[db]`-сообщение → stderr (чистый `--json`). Rust-тесты **174/0 (+9)**. `W4_REPORT.md` §9.
> **Версионные каталоги §10.6 (02.10.2026):** `installers\install_app_version.ps1` (раскладка
> `app\<ver>` + junction-ы общих `data`/`models` + перенос `config.yaml` + указатель `app\current`)
> и `installers\update.ps1` (отдельный процесс: stop задач/резидента → install → switch → start).
> Живой прогон: junction `current`, write-through общих `data`/`models`, перенос config,
> переключение версии (обе версии остаются — откат). Rust **174/0**, Python 269 OK. `W4_REPORT.md` §10.
> **`.mpp`/jpype в sidecar (02.10.2026):** `requirements.lock` += `jpype1==1.7.1`, `mpxj==16.7.0`
> (Java 11+; воркер сам находит JDK 21); `_capabilities` репортит `mpp`; `build_sidecar` self-test
> импортирует jpype/mpxj; `hds check` получил пункт **`mpp`**. Живой `.mpp` из фикстуры извлечён
> (задачи/даты). Rust 174/0, Python 269 OK. `W4_REPORT.md` §11. Остаток: `build-macos`, тег `clip-onnx-v1`.
> **Пакет (dry-run, 02.10.2026):** `build_rust_release.ps1` += `-SkipBuild` (взять готовые
> `target\release\*.exe`, не трогая резидента). Прогон 0.1.0 → `dist\hds-0.1.0-windows-x64\`
> (46 файлов, все разделы §10.1) + zip + sha256; `bin\hds.exe --help` работает. `releasing.md`,
> `W4_REPORT.md` §12.
> **W5-финал (02.10.2026):** Python выведен из CI (`ci.yml` удалён; в `release.yml` убраны
> `test-py`/`test-macos`/`build-macos`) и из продукта — `hds\` оставлен только под sidecar
> (`config/extractors/extract_av/extract_static/lemmatizer/whisper_cpp`), удалены ядровые модули,
> `mcp_start.py`, `ensure_llama_runtime.*`, `tests\`, генераторы golden. golden **заморожен**.
> Rust-строки с `python -m hds.*` поправлены. Rust — 174/0 (+9), clippy 0/0. `W4_REPORT.md` §13.
> **README переписан (02.10.2026)** под Rust-first (1011 → ~306 строк): установка
> (`setup.cmd`→`setup.ps1`: движок/GGUF/whisper/CLIP, задачи, интеграции), версионная установка
> `app\<ver>`/`update.ps1`, CLI `hds`, UI, MCP, llm-host, sidecar (вкл. `.mpp`/Java), БД/сигналы,
> `config.yaml`, разработка, диагностика, «осталось в Python» (только sidecar).
> **`hds-ui` перенесён (перепроектированный):** `crates/hds-ui` — свой HTTP-сервер +
> страница (статус/поиск/ask/управление индексацией), роли — из фасада `llm-host`;
> `hds ui`/`hds_ui`; `/api/tree` (по БД, 0 с), `/api/diagnostics`, CSRF; правка `config.yaml`
> (roots/exclude_paths, с сохранением комментариев) — `W1_REPORT.md` §7.
> `cargo test --workspace` — **170 green (+9 `#[ignore]`)**.
> **Контекст нового чата — `tools/parity/W1_REPORT.md` §0 (передача, актуально на 02.10.2026);
> W3 (медиа/CLIP) — `W3_REPORT.md`; W4 (упаковка/CI) — `W4_REPORT.md`; W0 — `SPIKES.md`.**
> **Оверлей-поставка патча (02.10.2026):** `runtime-manifests/engine-patch.json` (база v1.15,
> 10 файлов с sha256) + `installers/fetch_engine_runtime.ps1 -PatchEngine|-RollbackEnginePatch`
> (скачивание/бэкап `*.orig`/откат) + `installers/publish_engine_patch.ps1` (мейнтейнер: падает
> при дрейфе хэшей, создаёт тег `engine-patch-v1`, заливает ассеты). Доки: README (раздел «Патч
> движка», `gpu-observability`), NOTICE.md. Проверки: `.ps1` ASCII+parse ok, манифест ↔ DLL
> `mismatches=0`.
> **L1 шаг 3 закрыт (02.10.2026):** `load_instance` больше не зовётся на каждый запрос
> LOAD_ON_DEMAND (движок грузит по запросу), в `hds check`/UI появился пункт
> **`gpu-observability`** («heartbeat … с назад; движок: занят/свободен; VRAM: наш процесс …
> чужие …; лог …» — работает даже при молчащем HTTP), `memory_free` движка помечен R29
> прямо в строке устройств. Живой `hds check --json` → `"ok": true`, пункт `ok`. Тесты 194/0.
> **Патч установлен в боевой каталог движка (02.10.2026):** наши DLL (bridge/multi-node/audio/
> llama/mtmd/ggml*) скопированы в `%APPDATA%\OpenResearchTools\TranscribeOffline\Engine`, штатные
> сохранены рядом как `*.orig` (9 файлов) — откат одной командой. Штатный резидент (без
> `--engine-dir`) это подтверждает: в логе `KV q8_0 272 МиБ`, `status` — «наш процесс 7263 МиБ
> (94 %), чужие 454 МиБ», свободно 4397 МиБ. `engine-patch/README.md`.
> **L2b выполнен и проверен живьём (02.10.2026):** патч движка v1.15 собран (CUDA 13.4 + Ninja
> Multi-Config), сборка воспроизводит апстрим (DLL 5,18/0,28 МБ, экспортов 39 = 39 → ABI цел).
> На копии рантайма: **KV q8_0 272 МиБ** вместо 512 f16 (VRAM чата 7905 → **7704 МиБ**),
> **compute-буфер embedding ~1,4 ГиБ → 90 МиБ** (конфиг: `--ubatch-size 512`), чат и эмбеддинги
> работают. Решения заказчика (вменяемые `n_batch/n_ubatch` + KV-квант для V) применены.
> Осталось: апстрим-отчёт (EN), оверлей-ассет с откатом, установка патча в боевой каталог,
> `NOTICE.md`/README. `W4_REPORT.md` §15.
> **Патч движка (02.10.2026):** текст готов и лежит в `engine-patch/` (`hds-engine-patch-v1.15.patch`
> — 4 файла, 22 хунка; база — тег **v1.15** `2683eb6`). Классы правок: ограниченное ожидание слота
> (P1), загрузка модели **вне** `instance->mutex` с отдельным `load_mutex` (P2 — причина инцидента),
> вынос `set_cluster_error` из-под instance-лока (P3, ABBA против `remove_instance`), поля типа KV
> + Flash Attention для квантованного V (KV). Сборка/проверка — после базового билда; воспроизведение
> и откат — `engine-patch/README.md`. **Итерация 2 (02.10.2026):** хвосты пяти путей запросов
> перепроверены — `set_cluster_error` там уже был вне instance-лока (со стока); снят единственный
> реальный вызов под локом (guard диаризации в `audio_transcriptions_raw`), патч стал 24 хунка —
> `W4_REPORT.md` §16. Патч **пересобран**, внедрён в боевой каталог и **опубликован** (тег
> `engine-patch-v1`, 10 ассетов; `-PatchEngine` проверен end-to-end). Апстрим-отчёт (EN) **написан** —
> `engine-patch/UPSTREAM_REPORT.md` (публикация — по решению заказчика).
> **KV-квант чата (02.10.2026):** причина — в API движка нет полей под тип KV (KV всегда f16,
> 512 МиБ у чата). Наша сторона закрыта: `--cache-type-k/v` и `llm.<role>.cache_type_k/v`
> распознаются, поля едут в `InstanceSpec`/FFI, бюджет считает `KvBits::Q8_0`; предупреждение
> «флаг не распознан» ушло — боевой `config.yaml` работает без правок. Сторона движка — в L2b
> (сборка). `W4_REPORT.md` §15. Тесты — **191/0 (+10 ignored)**.
> **L1 шаг 2 — устойчивость (02.10.2026):** ожидание загрузки шагами без удержания шлюза
> (`wait_loaded_stepwise`), `try_with(500 мс)` в `prepare` с **отложенным вытеснением**
> (`[deferred]`), bounded-арбитр, `503 «движок занят»` вместо ожидания для `/internal/*`,
> честный диагноз из heartbeat и `stop --force`. Живьём: зависший резидент (10 416 МиБ) снят
> `stop --force` (VRAM 10 674 → 361 МиБ), `/props` отдаёт `state`/`busy`. `W4_REPORT.md` §15.
> **L1 шаг 1 — наблюдаемость движка (02.10.2026):** по следам инцидента §14 добавлены шлюз
> движка с меткой занятости и ожиданием по бюджету (`crates\hds-llama\src\gate.rs`),
> атрибуция VRAM по процессам через PDH (`gpuattr.rs`; сверено с `Get-Counter` байт-в-байт:
> 10 416 МиБ у нашего pid), heartbeat резидента (`data\llm-host.heartbeat.json` — состояние
> видно даже при мёртвом HTTP), строка атрибуции в `status`, диагноз + **`stop --force`**
> (taskkill по pid-файлу) в CLI. Тесты — **190/0 (+10 ignored)**, clippy 0/0. `W4_REPORT.md` §15.
> **Живой порядок на машине (02.10.2026):** `index.pause` в передаче числился стоящим, но **файла
> не было** — боевую `D:\hermes-disk-search-db\index.db` продолжал индексировать legacy-Python
> (`watch.lock` = 8028, `pythonw -m hds.cli watch`), а `:8787` держал `pythonw -m hds.cli mcp-http`
> (модули ядра удалены в W5 → перезапуск невозможен). По решению заказчика рудимент снят:
> 4 процесса `pythonw` остановлены, создан пустой `index.pause` (резидент видит «пауза ДА»),
> владельцем `:8787` стал Rust `hds mcp-http` (`/health` → `{"app":"disk-search","version":"0.1.0"}`);
> освободилось ~1,3 ГБ VRAM (12005 → 10765 МиБ). Боевой индекс не тронут (записей нет с 13:50:17).
> Детали и грабли — `tools/parity/W4_REPORT.md` §14.
> Ниже (вплоть до §1) — накопительная сводка волн; раздел 0 `W1_REPORT.md` — самый свежий.
> **Важно (снято ограничение):** «`crates.io` недоступен» — перепроверено живьём 01.10.2026:
> cargo обновляет индекс, качает и собирает крейты (например `ort` `2.0.0-rc.13`); прокси и
> `CARGO_NET_OFFLINE` не заданы. Новые зависимости добавлять можно; заметки поправлены
> (`tools/parity/README.md` §3 п.13, `W2_REPORT.md` §17).
> **Автотранскрибация + диаризация (04.10.2026, `PLAN_AUTO_TRANSCRIBE`, T0.1–T5):**
> спайк зафиксировал формат вывода движка (`tools/parity/T0_1_DIARIZATION_FORMAT.md`:
> `### SPEAKER_00 [hh:mm:ss - hh:mm:ss]`, неприсвоенная реплика `UNASSIGNED`, выход `.md`);
> спайки идут **нашей** пробой `crates\hds-llama\src\bin\diar_probe.rs` — стоковый
> `example-cli.exe` несовместим с патчеными DLL (`engine-patch/README.md`). Добавлено:
> секция `auto_transcribe.*`; контракт `/internal/transcribe` (`diarization*`, `return_text`,
> жёсткий отказ без sortformer); `hds-index::transcribe::transcribe_to_file` (выход + `.orig.md`
> + `.speakers.json`); демон `hds transcribe-watch` (+ `transcribe-once`, `transcribe-stop`,
> `transcribe.lock`, манифест/очередь JSONL, утилизация исходника delete/keep/move); арбитр
> VRAM — `dispatch::plan_transcribe` и `gpu.priorities.transcribe = 60` (1 поиск/чат →
> 2 автотранскрибация → 3 индексация) с защитой транскрибатора от индексации; UI — закладка
> «Транскрибация» (`/api/transcribe/{list,file,speakers,apply}`, path-safety, `.bak`, частичная
> подстановка имён, правка `inbox_dir`/`out_dir`); установка — `installers\fetch_diarization_model.ps1`,
> задача `HermesDiskSearchTranscribe`, пункт `diarization` в `hds check`/`whisper-check`.
> Живые прогоны: `transcribe-once` и демон (исходник удалён, манифест `done/delete`),
> переименование `SPEAKER_00→Иван→Пётр`, арбитр в логе резидента (`модель+KV 2254`, приоритет
> `transcribe = 60`). clippy 0/0, `cargo test --workspace` — зелёные.
> **Управление демонами из UI (04.10.2026):** на закладке «Транскрибация» — блок «Демон
> автотранскрибации», на закладке «Поиск» — «Демон индексации»: состояние по `*.lock`
> (`lock_is_stale` распознаёт мёртвый процесс) + кнопки **Запустить/Остановить/Перезапустить**
> (`GET/POST /api/transcribe/daemon`, `GET/POST /api/watch/daemon`; запуск detached, стоп —
> `transcribe.stop`/`index.stop`, POST — под CSRF). Заодно **исправлен баг**: `pipeline::run_index`
> снимал `index.stop` в конце (в т.ч. при выходе из-за `index.stop` **во время паузы**), из-за
> чего `hds watch` не останавливался — теперь остановка помечается счётчиком `stopped`, и
> watcher завершается (`watch.rs`). Живой прогон: `stop` → `pending=false` за 2.5–4 с, `restart`
> и повторный `stop` — ок.
> **Передача контекста:** `tools/parity/README.md` §4 (чек-лист на 5 минут + что читать) →
> `tools/parity/W2_REPORT.md` §9 (состояние, карта кода, команды, грабли), **§10 (отчёт A6:
> боевые порты, офлоад 9384 МиБ, `n_batch` −1503 МиБ, ARB 6/6; §10.6 — живая машина,
> §10.7 — грабли окружения)** и **§14 (отчёт B7: файлы, приёмка, грабли)** →
> `PLAN_W2_LLM_HOST.md` (§A4–§A6 с цифрами) →
> `MIGRATION_PLAN_RUST.md` (шапка: факты замера W2 и «не переоткрывать»).
> Ниже — **архив**: состояние Python-версии (ядро удалено в W5).
>
> **Этап M7 — «Управление службами и токенами из UI» (Э7/W10) выполнен (10.10.2026,**
> **ветка `server-multiuser`; детали и коммиты — `PLAN_M7_UI_CONTROL.md` §4).** **T0** — хвосты
> ревью M6 (M6-1: `gpu_roles` читает `llm.<role>.device`; M6-4; F-m6-t5-1 — поведение не меняем).
> **T1** — общий модуль `hds-core::service` (роли/имена, `sc_args`, LaunchDaemon-плейст, чтение
> состояния SCM/launchd, управление); `hds-cli::cmd::service` — тонкая обёртка, `service run`
> остался в CLI. **T2** — `GET/POST /api/services` (состояние 4 ролей; `install|uninstall|start|
> stop|restart`; admin-гейт/CSRF/аудит `admin_op`; честные ошибки прав/профиля; `ui`-restart —
> отложенный helper, не виснет). **T3** — `hds token new --user L [--ttl-days N]` и
> `POST /api/access/token/new` (plain-токен один раз; в файл — хэш; в аудит — без токена).
> **T4** — карточки «Службы» и «Выдать токен» в `page.rs` (попутно восстановлены
> `accessList/Add/Del`). **T5** — рантайм-приёмка: полный SCM-цикл по 4 ролям **из-под
> администратора** (UAC-хелпер), сверка `/api/services` ↔ `sc query`; токен+Bearer+отзыв+аудит.
> **Находки живого прогона (исправлены):** `sc.exe create` принимает `binPath` только в
> «классическом» виде (в коде — `raw_arg`); `sc qc` локализован → состояние/автозапуск читаются
> по значениям. **T6** — доклад: `README.md` (раздел «Управление службами и токенами из UI (M7)»),
> `README_SERVER_MULTIUSER.md` §8c, `PLAN_SERVER_MULTIUSER.md` §9. Тесты: **88 наборов / 0 failed**;
> fmt/clippy `-D warnings` чисто. **Хвост:** перезапуск роли `ui` из самого UI на живом SCM-стенде
> не прогоняли. Десктоп-профиль не изменён; боевой стенд возвращён (`index.pause` не снимался).
>
> **Этап M8 — «Серверный пакет установки» (Э8/W6) выполнен (10.10.2026, ветка
> `server-multiuser`; детали и коммиты — `PLAN_M8_INSTALL_PACKAGES.md` §4).** **T0** — хвосты
> ревью M7 (M7-1: `permission_denied` — Unicode `to_lowercase()` + `"failed 5"`/`"ошибка: 5"`;
> M7-2: `restart` ждёт `STOPPED`; M7-4: macOS `uninstall` сохраняет код `bootout`). **T1** —
> unattended `install_server.ps1` (`-StateDir/-McpHost/-McpPort/-UiHost/-UiPort/-Parallel/
> -GpuDevice/-GpuPreset/-Roots/-EnableAccess/-AdminLogin/-AdminPassword/-Fetch/-NoFetch/-WhatIf`),
> генерация server-`config.yaml` (не затирает существующий), админ только хэшем, оркестрация
> `fetch_*` (в них нет `Read-Host`). **T2** — macOS-аналог `install_server.command` (тот же
> набор флагами, `pwsh`-fetch, ad-hoc `codesign`, `xattr`), фикс потери последнего корня в
> `--roots`. **T4** — `update.ps1 -Profile server` (стоп/старт 4 SCM-служб с ожиданием,
> окно обслуживания), `-RollbackTo`/`-ListVersions`/`-WhatIf`, фикс `-Force:$Force`.
> **T5** — приёмка: **(a)** без админа — `-WhatIf`/`--whatif` (план+конфиг,
> YAML ok, `bash -n`), config-only песочница (админ-хэш argon2id, plain не утёк), реальные
> обновление/откат (junction `app\current`, health 200), `cargo test --workspace` **89/0**
> (в первом прогоне — флейк `quota_socket`, изолированно зелёный), fmt/clippy чисто;
> **(b)** с админом — UAC-хелпер `m8_t5_scm.ps1` (живой прогон): **4 службы SCM**
> (`kind=service`/`autostart=True`; `mcp-http`/`ui` running), `/api/services` (Bearer admin) ↔
> `sc query`, **MCP-гейт** без токена `401`/с токеном `200`, `update.ps1 -Profile server` +
> `-RollbackTo` без «отказано в доступе» (health 200), desktop не тронут; TLS-прокси — вручную.
> **T6** — доклад: `README.md`
> (раздел «Установка серверного пакета (M8)»), `README_SERVER_MULTIUSER.md` §8d,
> `PLAN_SERVER_MULTIUSER.md` §9. **Э9/T3 (клиентский пакет, Cline без Hermes) отложены** в
> следующий этап. Десктоп-профиль не изменён; боевой стенд возвращён (`index.pause` не снимался).
>
> **Следующий этап — Э9/M9 «Клиентский пакет установки» (Cline без Hermes):** план —
> `PLAN_M9_CLIENT_PACKAGE.md` (треки T0–T5), промт — `M8_HANDOFF_PROMPT_3.md`. Отложенный хвост
> Э8: HTTPS через TLS-прокси (машина с сертификатом) и живой цикл «клиент + внешний сервер».
>
> **Этап M9 — «Клиентский пакет установки» (Э9/W7) выполнен (10.10.2026, ветка `server-multiuser`;
> детали и коммиты — `PLAN_M9_CLIENT_PACKAGE.md` §4).** **T0** — `hds cline-sync` получил клиентский
> режим `--server-url/--token/--mcp-path`: MCP-запись с **внешним** URL и заголовком
> `Authorization: Bearer` (`mcp_entry_at`), шаг окон моделей пропускается, конфиг необязателен
> (тонкий клиент); попутно исправлено, что `--dry-run` всё же писал правило/скилл; токен — также из
> `HDS_CLINE_TOKEN` (не в argv). **T1/T2** — мастера `setup-client.ps1`/`.cmd` (Windows) и
> `installers/install_client.command` (macOS): `GET <url>/health` → `hds cline-sync` → «перезапустите
> Cline»; без админа, идемпотентно. **T3** — тонкая сборка `build_client_release.ps1` (+ `_macos.sh`):
> только `bin\hds`, правило/скилл, мастеры, README, sha256; сборщик отвергает «тяжёлые» части.
> **T4** — приёмка: локальная песочница (`--dry-run` ничего не пишет; реальный прогон — внешний URL
> + Bearer; идемпотентность), `GET /health` → 200; живой цикл «клиент + внешний сервер» и HTTPS через
> TLS-прокси — хвост (машина с сертификатом). **T5** — доклад. Тесты: **89 наборов / 0 failed**;
> fmt/clippy `-D warnings` чисто.
> **Живая проверка хвоста (12.10.2026):** клиентский пакет развёрнут на ноутбуке (Windows,
> `192.168.0.102`) → `setup-client.ps1 -ServerUrl http://192.168.0.101:8790/ -Token …` → Cline видит
> `disk-search` (внешний URL + `Authorization: Bearer`) и **поиск вернул реальные результаты**
> (2 ТЗ из тестового индекса). Входящие блокировал брандмауэр Windows (правило для 8790;
> AdGuard VPN ни при чём). Остаётся только HTTPS через TLS-прокси.
> **Следующий этап — Э10/W8 «Эксплуатация»** (healthz, бэкап, диагностика, обновление).

---

## АРХИВ (до W5) — состояние и планы Python-ядра

> Разделы 1–6 ниже описывают **удалённое** Python-ядро (W5). Упомянутые там файлы
> (`hds/indexer.py`, `progress.py`, `watcher.py`, `ui_server.py`, `db.py`, `dbops.py`, `cli.py`,
> `search.py`, `rag.py`, `chunker.py`, `embedder.py`, `clip_index.py`, `mcp_server.py`,
> `llama_server.py`, `rerank.py`, `diag.py`, `mcp_http.py`) в репозитории **отсутствуют** — остался
> только sidecar-набор `hds\` (`config`, `extractors`, `extract_av`, `extract_static`,
> `lemmatizer`, `whisper_cpp`). Актуальное состояние — шапка выше, `README.md`,
> `tools/parity/W4_REPORT.md`.

## 1. ЗАДАЧА

Система хранения и поиска информации на локальных дисках по свободному запросу
на естественном языке. Работает поверх Hermes Agent Desktop (MCP) + llama.cpp
(llama-server; порты 8010/8011/8012, менеджер hds/llama_server.py — переход с
LM Studio выполнен в 0.9.0, локальные модели на CUDA RTX 3060 12ГБ).

Пример: «Найди на моём компе, в каких проектах использовался 1С:Документооборот»
→ агент ищет по индексу и отвечает со ссылками на файлы (PDF, Office, Project,
видео с транскрипцией, картинки с OCR).

## 2. ПЛАН ДЕЙСТВИЙ (текущий спринт)

### Рефакторинг W2 (ветка `w2-llm-host`, Rust; журнал — `tools/parity/W2_REPORT.md`)
- ✅ A1–A4 (устройство/VRAM/бюджет/диспетчер + замер KV), A5 (фасад `:8010–8012`), B1–B3
- ✅ **A6 (30.09.2026) — резидентный `llm-host`**: `Host` в библиотеке (`src/host.rs`),
  pid-файл `data/llm-host.pid` + лог `data/logs/llm-host.log` (защита от второго
  владельца GPU), CLI `llm_host run|status|load|unload|devices|stop` через внутренний
  API фасада `/internal/*`, режимы `llm_server.mode: embedded|facade|off`, задача
  Планировщика `installers/install_llm_host_task.ps1`, живой smoke
  `tools/parity/resident_smoke.ps1`, 12 новых тестов (`tests/host_resident.rs`);
  `cargo test --workspace` — 74 green (+2 `#[ignore]`)
- ✅ **владельцем портов 8010–8012 стал `llm-host`** (30.09.2026): Python-роли остановлены,
  автозапуск — ярлык `HermesDiskSearchLlmHost.lnk`; Python MCP/watcher оставлены и ходят в наш
  фасад; `hds.cli ask` проходит целиком (ответ с проектами и цитатами), `llm.chat.n_batch: 512`
  в конфиге (−1503 МиБ), реранк осознанно на CPU (иначе 12 ГБ не хватает)
- ⏳ дальше: B4 (конвейер `process_file`), затем реранк на GPU после Rust-клиента
- ✅ **проверено на боевых портах 8010–8012** (30.09.2026): Python-роли остановлены, резидент
  поднят из боевого конфига, Python-менеджер видит наши инстансы как `state=llama`
  (`llama_server check/status`), чат/embeddings/rerank/`chat-think` работают, `stop` убирает
  инстансы и не снимает паузу пользователя; **ARB-1…6 автоматизированы** (`tools/parity/arb_scenarios.py`,
  6/6 green, `out/w2_arb.json`), полный офлоад чата = **9384 МиБ**, `llm.chat.n_batch: 512`
  даёт **−1503 МиБ** без потери скорости (подробности — `W2_REPORT.md` §10.2a–§10.2c);
  найдено и исправлено 3 живых бага (запрос к загруженной роли, «висящая» пауза после отказа,
  `--no-residency` затирал лог)

### Выполнено:
- ✅ Полная система: индексатор, watcher, поиск, RAG, MCP для Hermes, UI
- ✅ Релиз v0.2.0 через GitHub Actions (71 тест OK)
- ✅ ETA/скользящая скорость в heartbeat и статус-строке
- ✅ Деревья папок с раскраской в UI (none/partial/done)
- ✅ Переиндексация по пути из UI
- ✅ Параметры: transcribe, max_media_mb, max_chunks — редактируемые из UI
- ✅ Heartbeat-файл для кросс-процессного статуса
- ✅ chunk_count записывается в БД (был баг)
- ✅ indexed_at записывается при завершении + бэкфилл по mtime

### Выполнено в этом чате (завершение прерванного рефакторинга):
1. ✅ **Откат конвейера батч-эмбеддинга** в `hds/indexer.py`:
   - удалены `pending`, `bs_threshold`, `_flush_pending()` из `run_index`;
   - тело цикла заменено на `status, kind2 = process_file(conn, emb, cfg, path,
     force=full, progress_cb=progress_cb)` — обработка по одному файлу, надёжно;
   - `_extract_file` (фаза 1) + `_commit_file` (фаза 2) сохранены, `process_file`
     вызывает их с правильным порядком аргументов (раньше был перепутан — баг).
2. ✅ **Исправлен дедлок в `hds/progress.py`** — причина висящих тестов:
   `heartbeat_data()` и `_status_line()` держали `self._lock` и вызывали
   `rate_window()`/`eta_sec()` (тот же замок) → deadlock на каждом `_hb()`.
   Замок заменён на `threading.RLock()`.
3. ✅ Инициализация `self._seen_ts` (deque) и `self.total` в `__init__`
   (раньше `AttributeError` при heartbeat без пре-подсчёта).
4. ✅ Кодировочно-устойчивый вывод (`ProgressReporter._write`): строка статуса
   (⏱ и т.п.) перекодируется с replace, если кодировка потока
   не поддерживает юникод (cp1251-консоль/pipe) — раньше `UnicodeEncodeError`
   ронял `run_index` в `rep.finish()` (в т.ч. в фоновом потоке теста heartbeat,
   из-за чего `res == {}`).
5. ✅ Все 71 тест зелёные (`python -m unittest discover -s tests`).

### Исправлено в этом чате (инцидент «UI не запускается, watcher умер»):
1. ✅ **Корневая причина падения watcher'а**: при старте reconcile-сверка
   (run_index по D:\) дошла до служебного lock-файла Office
   `~$Регистр НСИ.xlsx`, openpyxl бросил `BadZipFile: File is not a zip file`,
   исключение никто не перехватил → `process_file` → `run_index` →
   `run_watch` — процесс watcher'а умер. Отсюда: stale `watch.lock`
   (PID мёртв), «watcher не запущен», прерванная на середине сверка.
2. ✅ `hds/indexer.py::_extract_file`:
   - lock-файлы Office (`~$*`) пропускаются сразу (статус skipped_type);
   - извлечение обёрнуто в try/except: битый файл → `finish_file(status='error')`
     и статус `error: ...`, прогон индексации НЕ останавливается.
3. ✅ `hds/watcher.py::run_watch`: reconcile обёрнут в try/except — падение
   сверки не убивает наблюдателя (события ФС важнее).
4. ✅ `hds/ui_server.py::run`: перед bind пробуем подключиться к порту —
   если кто-то уже слушает, второй экземпляр молча выходит (на Windows
   SO_REUSEADDR позволял двум серверам молча делить порт 8765 — из-за
   зависшего старого сервера браузер открывал мёртвую страницу).
5. ✅ Регрессионные тесты (+3): lock-файл пропускается, битый xlsx →
   статус error (не падение), run_index доходит до конца с битым файлом.
   Итого 74 теста — все зелёные.
6. ✅ Восстановлены процессы: watcher (PID в watch.lock живой, сверка идёт,
   w_err.log чистый) и UI (api/status отвечает). Найдена причина падения
   watcher'а ранее — «forrtl: window-CLOSE event» в старом w.log (окно закрыли).
7. ✅ Автозапуск watcher: задачи HermesDiskSearchWatch в Планировщике НЕ было
   (не установлена / нет прав администратора). Установлен ярлык
   `HermesDiskSearchWatch.lnk` в папку автозагрузки (запуск при входе).
8. ✅ Диагностическая заметка: `.venv\Scripts\pythonw.exe` — шим, порождающий
   реальный `C:\Python314\pythonw.exe`, поэтому каждый сервис = ДВА процесса
   с одинаковой командной строкой (не дубликат!); пары процессов с одинаковым
   CommandLine — норма. Проверять liveness по watch.lock через _pid_alive.
9. ⚠️ Дисковый шум при старте watcher — норма: reconcile-сверка читает
   начало/конец всех файлов диска (content_hash); на ~205k файлов —
   минуты-часы, видно в w.log (ETA в статусной строке).

### Выполнено в этом чате (ускорение индексации + exclude_paths в UI):
1. ✅ **Найдено и измерено узкое место**: каждый HTTP-запрос к LM Studio открывал
   новое TCP-соединение ценой ~2 с (keep-alive-запрос — 0.01 с). GPU простаивал:
   ~6 chunks/s фактических против 82-147 chunks/s при батчах по живому соединению.
2. ✅ **`hds/embedder.py`: requests.Session (keep-alive)** — соединение
   переиспользуется; ускоряет и индексацию, и каждый поисковый запрос.
   Факт после фикса: 63 чанка за 1.9 с, типичный файл 0.1-0.4 с (было 2+ с).
3. ✅ **`embedding.batch_size` 32 → 64** (config.yaml, README, дефолт make_embedder).
4. ✅ **Новая настройка `index.exclude_paths`** (исключаемые пути-префиксы):
   - `indexer.py`: `_excluded_prefixes`/`_prefix_excluded` (нормализация
     регистра/слэшей, граница префикса по компоненту пути: `D:\Backup2` ≠
     `D:\Backup`); `iter_files` отсекает поддеревья целиком + одиночные файлы;
     `path_excluded` проверяет и dirs, и paths (покрывает watcher и process_file);
   - `ui_server.py`: `_set_exclude_paths` (нормализация, дедуп, сохранение в
     YAML с сохранением комментариев, блочный и inline-вид, атомарная запись,
     проверка итогового YAML, предупреждение при исключении корня) +
     эндпоинт `POST /api/config/excludes` + `exclude_paths` в `/api/status`;
   - `assets/ui.html`: textarea «Исключённые из индексации пути» в
     «Параметрах обработки» + кнопка сохранения (fill/save из refresh());
   - `config.yaml`: ключ `exclude_paths: []` + закомментированный пример.
5. ✅ Тесты +9 (префикс/граница, отсечение поддерева/файла, process_file,
   вставка/замена блока YAML, inline `[]`, пустой список, отказ не-списка,
   дедуп) — **итого 83, все зелёные**.
6. ✅ Сервисы перезапущены с новым кодом; замер: индексация ~30-50× быстрее
   (файл 0.1-0.4 с вместо 2+ с), скорость обхода 48k файлов/мин.

### Исправлено дополнительно (кнопки «Остановить watcher» / перезапуск):
1. ✅ `_hds_pids` (поиск процессов watcher'а) переписан на psutil: вызов
   PowerShell Get-CimInstance через subprocess ломался кавычками WQL
   («Invalid query») и **тихо возвращал пустой список** — taskkill никогда
   не запускался, кнопка «Остановить watcher» не работала.
2. ✅ `_watch_stop`: taskkill → psutil kill (TerminateProcess) + kill детей
   (shim → реальный интерпретатор) + ожидание фактического завершения.
3. ✅ `_watch_running` и `_acquire_lock` больше не доверяют watch.lock:
   PID из lock мог быть переиспользован ОС → «watcher уже запущен» при
   мёртвом watcher'е (старт молча не срабатывал). Теперь проверяется
   командная строка живого процесса ('hds.cli watch').
4. ✅ Живой цикл проверен через API: старт → процессы найдены → стоп
   (оба PID убиты, статус False) → повторный старт (работает).
5. ✅ Тесты +3 (_hds_pids находит процесс по cmdline; замена блока
   exclude_paths не съедает соседние ключи YAML) — **итого 86, все зелёные**.

### Выполнено дополнительно (интеграция с Hermes Desktop):
1. ✅ Диагностика «агент не использует MCP»: в сессии 13-14.09 агент на все 5
   запросов использовал search_files/terminal вместо disk-search (лог agent.log);
   причина — новая сессия (history=0), модель выбирала встроенные инструменты,
   наши описания были слишком скромными.
2. ✅ Усилены описания MCP-инструментов (search_local_files = «ГЛАВНЫЙ инструмент
   для "найди на этом компе…"», подсказки kinds=media/image/mpp).
3. ✅ Скилл `disk-search` в Hermes (skills\disk-search\SKILL.md): правило «любые
   "найди на компе" → MCP disk-search, не grep/terminal» + подсказки по kinds.
4. ✅ `install_hermes.ps1` — подключение disk-search к Hermes в один запуск
   (MCP-блок в config.yaml с сохранением комментариев + валидация YAML + скилл);
   идемпотентен; работает и когда Hermes установлен позже disk-search.
5. ✅ `setup.ps1` вызывает install_hermes.ps1 автоматически (последний шаг).
6. ✅ Скилл хранится в репозитории: hermes-skill\SKILL.md (источник для установки).
7. ✅ README: раздел «Интеграция с Hermes» переработан (авто/позже/вручную/проверка).

### Выполнено дополнительно (CLIP: поиск картинок по содержанию):
1. ✅ Вопрос пользователя: «удастся ли найти изображения по описанию содержания
   ("живые цветы")?» — проверено: НЕТ (чанк картинки = размеры+EXIF+OCR-текст;
   запрос «цветок» давал 0 результатов).
2. ✅ Быстрый фикс: в чанк картинки добавлено «папка: <имя>» (фото из папки
   «Цветы» находятся по названию папки).
3. ✅ **CLIP-индекс**: hds/clip_index.py (image-энкодер clip-ViT-B-32 +
   text-энкодер multilingual-v1 — понимает русский; 512-dim; CPU);
   таблица images_vec в БД; хук в run_index (CLIP-вектор при индексации
   каждой картинки); поиск: при kinds=image (или без фильтра) текстовый
   запрос → images_vec → RRF в общий рейтинг.
4. ✅ CLI `clip-index`: дозаполнение векторов по всем картинкам
   (busy_timeout 30 с + ретраи — параллельная запись watcher'а);
   vec0 не поддерживает OR REPLACE → DELETE+INSERT.
5. ✅ Живой тест: русские запросы «живые цветы»/«цветы» находят картинки
   (раньше — 0-3 случайных). Backfill 27 тыс. картинок идёт в фоне
   (clip_out.log), по завершении весь архив будет искать по содержанию.
6. ✅ Тест +1 (папка в чанке картинки) — **итого 88, все зелёные**.
7. ✅ requirements.txt: +sentence-transformers (torch, transformers — зависимости).
8. ⚠️ Первый прогон backfill упал: vec0 не поддерживает INSERT OR REPLACE
   и «database is locked» при конкуренции с watcher'ом — исправлено
   (DELETE+INSERT, busy_timeout 30 с, ретраи).

### Выполнено дополнительно (инсталлятор: tools.tool_search):
1. ✅ Диагностика: Hermes прячет 27 из 49 инструментов (включая все MCP) за
   discovery-протоколом tool_search/tool_describe/tool_call (~8k токенов промпта);
   облачная Qwen3.7-plus проходит его сама, локальная Qwen3.5-9B — нет
   (ошибки формата в логах, начинает ripgrep/terminal до явного «используй tools»).
2. ✅ Исправление применено вживую: tools.tool_search.enabled: "off" в config.yaml
   Hermes — все 49 инструментов всегда в промпте, discovery-барьер убран.
3. ✅ install_hermes.ps1: шаг 3 — настройка tools.tool_search (три ветки: блок
   есть → обновить; tools: есть → вставить внутрь; нет → добавить в конец),
   YAML-валидация, идемпотентно.
4. ✅ Протестированы 4 сценария (реальный Hermes идемпотентно, свежий Hermes
   без tools:, tools: без tool_search, tool_search: "on" → "off").
5. ✅ README: ручной шаг 3 добавлен в раздел «Интеграция с Hermes».

### НЕ выполнено (следующий шаг):
1. Заполнить `exclude_paths` (UI → «Параметры обработки» → «Исключённые
   пути») — кандидаты: `D:\Backup\Downloads\opencv`, `D:\Backup\Downloads\cmake-4.3.1`,
   возможно `D:\Backup\Downloads` целиком (решает пользователь); после
   сохранения перезапустить watcher/индексацию кнопками.
2. Релиз v0.2.2 по releasing.md (фича + оптимизация → MINOR или PATCH по вкусу).
3. Ручная проверка «спроси в Hermes → ответ со ссылками».

## 3. ПРОБЛЕМЫ И БАГИ

### РЕШЁННЫЕ (все в main)
1. os.kill(pid,0) на Windows УБИВАЕТ — ctypes OpenProcess
2. mcp 2.x FastMCP→MCPServer — обе версии поддержаны
3. PS 5.1 + кириллица .ps1 без BOM — все .ps1 с BOM
4. re.sub с Windows-путём в replacement — lambda-подстановка
5. Предупреждения HuggingFace подавлены, русские сообщения
6. Windows-путь в YAML двойных кавычках — одинарные кавычки
7. indexed_at не записывался → записывается + бэкфилл mtime
8. chunk_count не записывался → записывается в finish_file
9. Не начатые папки показывались как partial → fresh-проверка на NULL iat
10. fts_query игнорировал однобуквенные слова → тест исправлен
11. Node 20 deprecation в Actions → checkout@v5, setup-python@v6
12. Make_icon print кириллица падала на runner → utf-8 reconfigure

### АКТИВНЫЕ ПРОБЛЕМЫ
1. UI-статусы бейджей могут врать (heartbeat удаляется при завершении;
   возможное решение — показывать last_reporter-снапшот с пометкой «завершён»).
2. Опционально: аккуратная оптимизация батч-эмбеддинга (можно позже, с полным
   тестовым покрытием; базовый путь через _commit_file уже батчит внутри файла).

### РЕШЁННЫЕ В ЭТОМ ЧАТЕ
13. Дедлок в ProgressReporter: не-RLock + вложенные rate_window/eta_sec
    под замком — висели ~20 тестов (heartbeat, пауза, стоп)
14. `_seen_ts`/`total` без инициализации в __init__ → AttributeError
15. UnicodeEncodeError (⏱ в cp1251-консоли/pipe) ронял run_index в rep.finish()
16. process_file вызывал _commit_file с перепутанным порядком аргументов

---

## 4. ТЕКУЩЕЕ СОСТОЯНИЕ ФАЙЛОВ

### hds/indexer.py — РАБОТАЕТ (после отката конвейера)
- process_file = _extract_file (фаза 1: проверки+извлечение+чанки)
  → _commit_file (фаза 2: эмбеддинги+запись), возвращает (статус, kind)
- _extract_file: пропуск ~$-lock-файлов Office, try/except вокруг извлечения
  (битый файл → статус error + finish_file, прогон не падает)
- run_index: обход, пауза/стоп-файлы, heartbeat (_hb), пре-подсчёт total
  в daemon-потоке (ETA), prune с защитой от массового удаления;
  конвейер pending/_flush_pending удалён
- path_excluded, content_hash, iter_files, reindex_path, _prune_deleted — ок

### hds/progress.py — РАБОТАЕТ
- RLock (вложенные rate_window/eta_sec безопасны), _seen_ts/total в __init__
- _write(): кодировочно-устойчивая печать (cp1251/pipe не падают)
- _fmt_eta, rate_window(300), eta_sec, heartbeat_data (total/eta/remaining)
- _status_line включает ETA

### hds/watcher.py — РАБОТАЕТ
- reconcile_on_start обёрнут в try/except: сбой сверки не убивает наблюдателя
- _acquire_lock/_pid_alive: stale watch.lock (мёртвый PID) подчищается при старте

### hds/ui_server.py — БЫЛ ПОВРЕЖЁН, ВОССТАНОВЛЕН
- Проверить py_compile + ast.parse перед использованием!
- _index_state (репортёр + heartbeat + last_reporter), _db_stats
- _build_trees (данные + агрегация), _set_simple_config, _save_config
- _start_index, _db_move, _watch_start/stop/autostart_set

### hds/db.py — РАБОТАЕТ
- finish_file(status, error, chunks) — записывает indexed_at + chunk_count
- connect(): sqlite-vec + FTS5 + vec_dim миграция + backfill + busy_timeout
- upsert_file, add_chunk, add_vector, get_file_by_path/hash
- rename_path, remove_path, stats

### hds/dbops.py — РАБОТАЕТ
- move_db(): атомарный перенос БД через psutil, kill_processes параметр

### Остальные файлы — РАБОТАЮТ
- hds/watcher.py, search.py, rag.py, extractors.py, extract_static.py,
  extract_av.py, chunker.py, embedder.py, mcp_server.py, cli.py, config.py
- assets/ui.html, tools/make_icon.py, gen_fixtures.py, test_mcp.py

### config.yaml
- db_path = D:\hermes-disk-search-db\index.db
- roots = ["D:\\"], max_media_mb: 2500, max_chunks: 3000, transcribe: true
- exclude_dirs включает $RECYCLE.BIN, .Trash, .Trashes, hermes-disk-search-db

---

## 5. ЧТО ПРЕДСТОИТ СДЕЛАТЬ

### КРИТИЧНО (следующий шаг)
1. Релиз v0.2.1 по releasing.md (тесты зелёные, check пройден):
   `__version__` в hds/__init__.py → 0.2.1, RELEASE_NOTES_v0.2.1.md,
   commit+push, `gh workflow run release.yml -f version=v0.2.1`
2. Ручная проверка сценария «спроси в Hermes → ответ со ссылками»

### ВАЖНО (после релиза)
3. UI-бейджи: не показывать «идёт индексация» по устаревшему heartbeat —
   показывать last_reporter-снапшот с пометкой «завершён»
4. Обновить README раздел про ETA (если требуется)

### ЖЕЛАТЕЛЬНО
5. Оптимизация батч-эмбеддинга (аккуратно, с полным тестовым покрытием —
   прошлая попытка вызвала каскад багов, откатила см. раздел 2)
6. UI-кнопка для запуска watcher'а с новыми настройками
7. Поддержка macOS-инсталлятора в тестах

---

## 6. КОМАНДЫ ДЛЯ БЫСТРОГО СТАРТА

```powershell
cd C:\Users\Sasha\hermes-disk-search
.\.venv\Scripts\python.exe -m hds.cli check      # диагностика
.\.venv\Scripts\python.exe -m unittest discover -s tests  # тесты
.\.venv\Scripts\python.exe -m hds.cli status     # состояние индекса
# UI: ярлык «Hermes Disk Search» на рабочем столе
# LM Studio: localhost:1234 (text-embedding-bge-m3 + qwen3.5-9b@q6_k)
# Hermes MCP: disk-search (в config.yaml Hermes Desktop)
```

### Ключевые технические детали
- HDS_CONFIG env переопределяет путь к config.yaml
- db_abs_path(cfg) разрешает db_path относительно PROJECT_ROOT
- indexer._ACTIVE_REPORTER — текущий репортёр (для UI/MCP статуса)
- indexer._LAST_REPORTER — снапшот последнего завершённого прогона
- index.heartbeat.json — кросс-процессный статус (ts свежесть < 30с)
- index.stop, index.pause, watch.lock — файлы-сигналы
- WAL-режим SQLite: параллельная запись безопасна (busy_timeout=5000)
- config.yaml — одинарные кавычки YAML для Windows-путей (не двойные!)
- ui_server.py восстанавливался после повреждения — при сомнении прогоните
  py_compile + test_ui_server
- Тесты импортируют helpers из папки tests/ → запускать discover или из tests/
