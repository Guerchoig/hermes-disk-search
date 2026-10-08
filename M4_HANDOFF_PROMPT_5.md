# M4_HANDOFF_PROMPT_5.md — промт для нового чата (продолжение M4: треки T4–T6)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, правила и грабли.
> Предыдущие промты — `M4_HANDOFF_PROMPT.md`…`_4.md` (T0–T2 закрыты, T3 не начат).
> Детальный план — `PLAN_M4_CONCURRENCY_SLA.md` (§3 контракты, §4 треки, §5 приёмка,
> §6 порядок, §7 риски, §8a журнал исполнения). Код-ревью M3 — `M3_CODE_REVIEW.md`.
> Контекст проекта — `PLAN_SERVER_MULTIUSER.md`, `README_SERVER_MULTIUSER.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка `server-multiuser`.
Состояние: **T0, T1, T2 и T3 закрыты**, HEAD — последний коммит из списка ниже (этот промт),
дерево чистое. Станция: Windows, RTX 3060 12 ГБ (+ AMD-карта), Ryzen 7 5700G, 64 ГБ RAM,
rustc/cargo 1.97.1. Ядро на Rust (`crates/`), Python — только sidecar.

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -8`  # HEAD — промт T4–T6
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`    # чисто
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe` (см. «Грабли»):
   либо остановить `hds watch` / `hds mcp-http` / `hds transcribe-watch`, либо
   `cargo test --workspace --exclude hds-cli`; после работ сервисы **вернуть**.
4. `cargo clippy --workspace --all-targets -- -D warnings` и `cargo fmt --all --check`
5. Прочитать: `PLAN_M4_CONCURRENCY_SLA.md` §3 (контракты) → **§4 T4–T6** → §5 (приёмка) →
   §8a (журнал: что уже сделано в T0–T3); затем `README.md` §«Слоты и SLA» (там подраздел
   «Очередь тяжёлых RAG и fair-share (M4 T3)»), `config.example.yaml` (`db.*`,
   `llm_server.parallel`, `rate_limit.*`, `queue.*` — уже есть), `STATUS.md` (M4).
6. Проверить факт боевого состояния (не менять!):
   `& .\target\release\llm_host.exe status` (роли, VRAM, `parallel 1`),
   `Test-Path .\index.pause` (**боевая пауза индексации — не снимать**),
   `& .\target\debug\hds.exe check` (боевой профиль desktop: пункт `queue` = «выключена»).

## Коммиты этапа M4 по порядку
258cb74 → 9d14980 → f2cf449 → 1a6e80d → f07fbea → d4af8b3 → 86df668 → 4ba31ec →
bcd689e (доки T0) → df37fa8 (тест-премиса T1) → dfc0530 (промт-2) →
**0395ab7 (feat(db) T1)** → **ca15b7e (доки T1)** → e0f4d13 (промт-3) →
**c07a367 (feat(llmhost) T2)** → **d54091e (доки T2)** → 425016a (промт-4) →
2779164 (промт-4: якоря T3) → **dd2e9af (feat(queue) T3)** → **c4542f6 (доки T3)** →
последний коммит — этот промт.

## Что уже сделано (НЕ переделывать)

### T0 — хвосты ревью M3/M2 (закрыт)
`token_issue` в аудите (+ отпечаток токена); `admin_op` после маршрута с фактическим
статусом; `unauthorized` с method/path; дочитывание тела перед 413/503 + сокет-тесты;
«тяжёлый» запрос разбором JSON-RPC (`hds_mcp::is_heavy_request`); `/health` вне
rate-limit; пункт «периметр» в `hds check`; PID-файл mcp-http с проверкой личности;
F10 (версия убрана из `/health`, `X-RateLimit-*`, tool_call без access.enabled, 404 без
эха пути, read-timeout); хвосты M2.
Открыто осознанно: **F9 (Host)** — не реализовано (правильнее поправить формулировку
PLAN_M3 §3.1, боевые PLAN не переписываем); **T0.9** — вопросы заказчику
(автотранскрибация в server-профиле, `hds login` без пароля в argv).

### T1 — БД под конкурентность (закрыт, 0395ab7 + ca15b7e)
Ключи `db.{busy_timeout_ms,journal_mode,read_only_conns}` (server: 5000/wal/true;
desktop: 30000/false — прежнее поведение); `open_rw` (единственный писатель — конвейер
индексации/watcher) и `open_ro` (читатели MCP/UI/CLI — read-only, без DDL); `hds check`
показывает фактические PRAGMA; стресс «5 читателей + писатель» без `SQLITE_BUSY`
(`hds-core/tests/db_concurrency.rs`, debug и release); README: бэкап только
`VACUUM INTO`/`.backup` (пометка Э10).

### T2 — слоты чата и VRAM (закрыт, c07a367 + d54091e)
Дефолт `llm_server.parallel` по профилю: **server 5 / desktop 1** (явное значение
сильнее); хелперы `hds_core::config::{llm_parallel, llm_parallel_default}`. Два корневых
дефекта (найдены замером и исправлены): (1) `InstanceSpec.n_parallel` не выставлялся →
движок поднимал 1 слот; (2) шлюз движка (`Gate`) был `Mutex` и держался всю генерацию →
вызовы сериализовались; теперь `RwLock` (генерация — разделяемый режим, управление
кластером — эксклюзив) + быстрый путь «роль загружена → сразу генерация»; **fit-гейт** на
старте (KV от контекста **слота**, явная ошибка «нужно/свободно/резерв» + подсказка);
`hds check` показывает фактические слоты из `/props` рядом с настроенным `parallel`.
Замеры (RTX 3060, 5 одновременных чат-запросов по 300 токенов, KV q8_0): 2×32768 →
8129 МиБ / 33,7 с; 3×32768 → 8723 МиБ / 27,1 с; 5×16384 → 8553 МиБ / **21,3 с**;
5×32768 и 5×65536 — **отказ** гейта с числами. Отчёты — `tools/parity/out/m4_slots_*.json`,
числа — `README.md` (§«Слоты и SLA») и `config.example.yaml`. Боевой `config.yaml` НЕ
менялся (явный `parallel: 1`).

### T3 — очередь и fair-share по пользователю (закрыт, dd2e9af + c4542f6)
Модуль **`crates/hds-core/src/queue.rs`** (чистая логика, тестируемая без сети):
* `QueueConfig::from_config` — ключи `queue.{enabled,max_depth,wait_timeout_sec,policy}`;
  дефолты по профилю: **server — вкл (20 / 240 / `rr`)**, desktop — выкл (= поведение M3
  «сразу 429»); лимит **исполнения** берётся из `rate_limit.max_inflight` (единый источник
  дефолта — `RateLimit::from_config` стал `pub`);
* **round-robin между пользователями + FIFO внутри** (`QueuePolicy::{RoundRobin,Fifo}`;
  при выдаче слота пользователь уезжает в конец курсора, если у него ещё есть ожидающие);
  `HeavyQueue::{acquire,stats,snapshot_json}`, RAII-`QueueTicket` (Drop освобождает слот и
  будит ожидающих), отказ `QueueReject::{Full{max_depth,depth},Timeout{waited_sec,depth},
  Aborted}`;
* метрики: `depth`, `executing`, ожидание `p50/p95` (окно 256 последних, **включая
  ожидания, закончившиеся таймаутом**), `granted/rejected_full/timeouts/aborted`,
  `per_user` («кто кого ждёт»); **атомарный снапшот** `<state_dir>/data/mcp-queue.json`
  (`spawn_snapshot_writer`, интервал 2 с, первый снапшот сразу, `pid`+`ts`;
  `SNAPSHOT_STALE_SEC = 10`).

Интеграция **`crates/hds-mcp/src/http.rs`** (`serve_request`, тяжёлая ветка ~705):
ключ fair-share `user_key` (~494): валидный Bearer → `user:<login>`, иначе при
`access.enabled` → `ip:<addr>`, без авторизации → `local`; запрос **ждёт** слот
(`q.acquire(&ukey, || client_alive(stream, read_timeout))`); коды: очередь полна → `429`
+ `Retry-After: 5` + `X-Queue-Depth`, таймаут → `503` + `Retry-After:
clamp(wait_timeout,1,60)` (оба — `queue_reply` ~428 / `queue_headers` ~470 /
`queue_retry_after` ~460); обрыв клиента в ожидании освобождает место (`client_alive`
~508: `peek` срезом 150 мс, read-timeout восстанавливается; обрыв **во время** генерации
вызов движка не отменяет — гасит страж `gpu.abandoned_request_sec`); ожидание пишется в
аудит событием `queue_wait` (`audit_queue_wait` ~533, `ms` = ожидание, статус ответа);
`enabled: false` — прежний путь M3 (`inflight_acquire` → мгновенный `429` через
`too_many` ~385).

Диагностика **`crates/hds-index/src/diag.rs`**: `check_queue(cfg)` + ядро
`queue_check(cfg, &q, Option<Value>)` (файловый ввод-вывод отделён — тестируется без
песочницы) — пункт `queue` в `hds check` и `/api/diagnostics` показывает настройку и числа
снапшота; **warn** при отсутствии/устаревании (>10 с) статуса и при
`wait_timeout_sec ≥ 300` (таймаут MCP-клиента).

Прочее: `config.example.yaml` — блок `queue:`; `README.md` §«Слоты и SLA» → подраздел
«Очередь тяжёлых RAG и fair-share (M4 T3)» + список ключей (`db`/`queue`/`rate_limit`/
`http`); `README_SERVER_MULTIUSER.md` §11 → «Очередь тяжёлых RAG и SLA (M4 T3)»;
`STATUS.md`; журнал §8a.

Тесты и проверки T3:
* 9 юнит-тестов `hds-core::queue` (дефолты/мусорные значения, быстрый путь,
  `Full`/`Timeout`/`Aborted`, потоковый fair-share `rr` = `[a,a,b,a]` против `fifo` =
  `[a,a,a,b]`, поля снапшота, файловый roundtrip) + `diag::tests::queue_check_statuses`;
* сокет-тесты **`crates/hds-mcp/tests/queue_socket.rs`** (держатель слота — реальный
  `ask_my_files`, висящий на блокировке БД в песочнице): `503`+`Retry-After`+
  `X-Queue-Depth: 1` и `429` **доходят до клиента** (детектор RST, регресс F4);
  desktop-конфиг → мгновенный `429` без очереди; снапшот с `executing/depth/p50`;
* **живой прогон** `tools/parity/out/m4_t3_live.ps1` (реальный `index.db` read-only +
  резидент; песочница только по `HDS_CONFIG`): `REQ2 → 503 Retry-After=3
  X-Queue-Depth=1`, `REQ3 → 429 Retry-After=5 X-Queue-Depth=1`; снапшот `granted 1,
  rejected_full 1, timeouts 1, wait_p50/p95 3155 мс, per_user [{local: executing 1,
  waiting 1}]`; `hds check` при живой очереди — ok с числами и «кто ждёт», после
  остановки MCP — warn «статус устарел»;
* `cargo test --workspace` зелёный, clippy `-D warnings`/`fmt` чисты.

## Что осталось (T4–T6) — по §3/§4/§5 плана

### T4. Квоты и приоритеты — 1–1,5 дн
* Ключ лимитов/квот = **пользователь**: `user:<login>` (валидный Bearer), иначе
  `ip:<addr>`; опция `rate_limit.per_token: true` возвращает прежнее (пер-токенное) поведение.
  **Готовый `user_key(env, header, ip)` уже есть в `crates/hds-mcp/src/http.rs` (~494,
  сейчас `fn` — для T4 сделать `pub`), но частота сейчас ключуется иначе:** в
  `serve_request` (705) лимит частоты идёт по `key = token|ip` (**725**), а тяжёлая ветка
  с очередью начинается на `is_heavy_request(b)` (**749**, `if let Some(q) = queue` —
  **754**) — перевести и частоту на `user_key`, иначе «два токена одного пользователя» не
  делят квоту (регресс из `M3_CODE_REVIEW.md` F10.3).
* Новые ключи в `crates/hds-core/src/ratelimit.rs` (`RateLimit` ~27-38, `from_config`
  теперь `pub`; дефолты — по профилю, как `enabled`/`max_inflight`):
  `rate_limit.max_inflight_per_user` (дефолт 2), `rate_limit.requests_per_min_per_user`
  (дефолт — как `requests_per_min`), `rate_limit.per_token`.
* Проверка: 3-й одновременный RAG одного пользователя → `429` + `hint` про
  пер-пользовательский предел. В очереди (T3) пер-пользовательский лимит исполнения
  встраивается в `can_admit` (`crates/hds-core/src/queue.rs`): сейчас проверяется только
  общий `st.executing >= cfg.max_inflight` и курсор RR.
* Инвариант «индексация уступает»: запрос не ждёт индексного писателя БД (T1: WAL +
  `open_ro`), индекс встаёт на паузу (`gpu.policy: query_priority`,
  `pause_index_on_query` → файл `index.pause`, владелец — `hds-llama`); нужна
  **проверка под нагрузкой/тестом**, что это так и что тяжёлый RAG не блокируется
  писателем (совместно с T3-очередью: слот держится на генерации, а не на БД).
* Тесты: два токена одного пользователя **не** удваивают квоту (регресс F10.3);
  пер-пользовательский 429 с `hint`; инвариант «индекс уступает».

### T5. Нагрузочная приёмка 5 × `ask_my_files` — 1–2 дн
* Скрипт нагрузки `tools/parity/load5.py` (или расширение `tools/parity/measure_run.py`,
  рядом уже есть `measure_live.py`): **5 клиентов (5 токенов/пользователей) шлют РЕАЛЬНЫЙ
  `ask_my_files` через MCP `/mcp`** (не сырой чат!) одновременно; фиксирует латентность
  каждого (p50/p95), коды, глубину/ожидание очереди и VRAM/слоты.
* **Готовые заготовки (проверены, на месте):** `%TEMP%\hds-m4-t2\run_variant.ps1`
  (ASCII-скрипт замера слотов T2: песочница `HDS_ROOT=%TEMP%\hds-m4-t2\root`,
  `HDS_CONFIG=<config>`, запуск `target\debug\llm_host.exe run`, ожидание `/health`,
  `/props.total_slots`, heartbeat VRAM, 5 параллельных клиентов через `Start-Job`,
  результат → `tools/parity/out/m4_slots_<tag>.json`) и `config_p2/p3/p5/p5_big.yaml`
  (копии боевого конфига с другим `parallel`/`ctx_per_slot`). Стиль живой пробы и
  кодов/заголовков см. в `tools/parity/out/m4_t3_live.ps1` (ASCII-only!).
* Прогон на реальном индексе и реальной карте; резидент снимать/возвращать по
  `cline-rules/dev-machine.md`; отчёт `tools/parity/out/m4_load.json` + сводка
  **p95 ≤ 5 мин** (напоминание: 300 токенов чата = 21 с при 5 слотах; реальный RAG
  добавляет поиск + 27k-токенный промпт).
* Регресс при `parallel: 2/3/5` и **с индексацией параллельно** (проверка «индекс
  уступает»: `hds index` в песочнице + 5 RAG-запросов; «database is locked» быть не должно).
* Для нагрузки помнить: `queue.wait_timeout_sec` (дефолт 240) < клиентского таймаута
  (300 с), иначе клиент обрывается раньше `503`; следить за `per_user` в снапшоте —
  это и есть «кто кого ждёт».
* Чек-лист внешнего HTTPS (перенос из M3 §5) — на машине с доменом/сертификатом:
  `https://<домен>/mcp` из Cline (Authorization Bearer), `/ui` в браузере за прокси,
  XFF-IP в аудите, `429` от лимитов через прокси.

### T6. Доклад — 0,5 дн
* `README.md` (§«Слоты и SLA» уже дополнен очередью — добавить квоты/WAL-итоги T5),
  `README_SERVER_MULTIUSER.md` (§11 — квоты; §8 «Портрет отказов» — новые коды/hint),
  `STATUS.md` (M4 целиком), `PLAN_SERVER_MULTIUSER.md` §9 (статус M4 — только по
  завершении этапа), журнал `PLAN_M4_CONCURRENCY_SLA.md` §8a.
* Закрыть `M3_CODE_REVIEW.md` в части T0 (отметить закрытые находки), не удаляя историю.

## Ключевые решения (не менять)
* SLA **≤5 мин** для 5 одновременных RAG; при нехватке VRAM — **явная ошибка с числами**,
  не деградация.
* Слоты чата: `llm_server.parallel` (server 5 / desktop 1); `ctx_per_slot` подбирается под
  RAG (замеры T2: 5×16384 либо 2–3×32768 на 12 ГБ).
* Периметр — reverse-proxy с TLS; наружу только `/mcp` и `/ui`; UI — только админ (M3).
* Идентификация — локальные токены; индексация — только админ; проект — единица выдачи (M1).
* Внешний API — задел M5 (`chat.provider: remote`); в M4 только контракт ключей (§3.5).
* Десктоп-профиль не ломаем: очередь/квоты выключены, `llm_server.parallel` — 1,
  `db.*` — 30000/false, лимиты/аудит — как в M3.
* **T3 (действующие контракты):** `rate_limit.max_inflight` — лимит **исполнения**,
  `queue.max_depth` — лимит **ожидания**; `queue.wait_timeout_sec` дефолт **240** (план
  называл 300; 240 выбрано сознательно — клиентский таймаут 300 с, иначе `503` приходит
  позже обрыва клиента, §7 плана); ключ fair-share `user:<login>` → `ip:<addr>` → `local`;
  коды `429` (очередь полна) / `503` (таймаут) с `Retry-After`; метрики очереди наружу —
  **файл-снапшот** `<state_dir>/data/mcp-queue.json` (читают `hds check` и
  `/api/diagnostics`), а не HTTP-эндпоинт; очередь живёт в MCP-процессе (админский `ask`
  UI — вне неё, документированное ограничение).

## Грабли окружения (важно! — пополнено сессией T3)
* **`run_commands`: каждый элемент массива = ОТДЕЛЬНАЯ PowerShell-сессия** — переменные
  (`$d`, `$c`) НЕ живут между элементами; связка «присвоил в одном, использовал в другом»
  молча даёт `$null`. Многошаговая проба — ОДНИМ элементом через `;` или `.ps1`-файлом.
* **Пробы с кодом — файлом.** Сложное inline-цитирование PowerShell ломает код тихо;
  писать `.ps1` инструментом editor (ASCII-only), запускать
  `powershell -NoProfile -ExecutionPolicy Bypass -File <файл>`.
* **`.ps1` только ASCII (T3):** PowerShell 5.1 читает файл без BOM как ANSI — кириллица в
  строках/комментариях превращается в мусор и **ломает парсер** (`Непредвиденная лексема
  ")"`, «отсутствует завершающий символ»). Проверка разбора:
  `powershell -NoProfile -Command "$null = [System.Management.Automation.Language.Parser]::ParseFile('<файл>', [ref]$null, [ref]$errs)"`.
* **`HDS_ROOT` переопределяет `project_root()`** (`hds-core::config`): с ним в песочнице
  **не находится Python-воркер**, и `ask_my_files` мгновенно отвечает «не найден
  интерпретатор воркера». Для живых проб сети/RAG ставьте **только `HDS_CONFIG`**
  (тогда `state_dir` = `%ProgramData%\hermes-disk-search` для server-профиля — после пробы
  каталог чистить). `HDS_ROOT` — для изоляции состояния (БД/аудит/снапшоты).
* **Живой «держатель» слота:** чёрная дыра (`TcpListener`, принимает и не отвечает) как
  `embedding.base_url` держит тяжёлый запрос гарантированно; чёрная дыра только в
  `chat.base_url` — лишь если поиск нашёл ≥1 фрагмент (иначе ранний возврат).
* **Снапшот очереди пишется раз в 2 с:** короткие состояния (<2 с) в него не попадают —
  для проб `wait_timeout_sec` заметно больше 2 с, состояние ждать поллингом файла
  (в сокет-тестах держатель — `ask_my_files` на блокировке БД в песочнице:
  `db.journal_mode: delete` + `BEGIN EXCLUSIVE` от теста).
* **`Stop-Process -Force` = TerminateProcess** → код очистки (`remove snapshot`) не
  выполняется; «статус устарел» в `hds check` после жёсткого убийства — ожидаемо.
* **Пересобрать `target\debug\hds.exe` перед пробами:** живые `hds watch` / `hds mcp-http`
  (:8787) / `hds transcribe-watch` (debug) держат exe (иначе `cargo build` → «отказано в
  доступе»). Порядок: остановить их → `cargo build --workspace` → прогнать пробу → вернуть
  сервисы (`Start-Process -WindowStyle Hidden`: `hds watch`; `hds mcp --http --host
  127.0.0.1 --port 8787 --path /mcp`; `hds transcribe-watch`). Резидент — release
  `target\release\llm_host.exe run` (в T3 не трогали). ⚠ В T3 первая живая проба молча
  проверяла **старый** exe — «ничего не работает» лечится пересборкой.
* **PID из `Start-Process -PassThru` может быть родителем**, а сервить — дочерний процесс
  CLI: проверяйте фактические pid
  (`Get-CimInstance Win32_Process -Filter "Name='hds.exe'" | Select ProcessId,CommandLine`).
* **`cargo test --workspace` при живых сервисах падает не тестами, а сборкой**
  («failed to remove `target\debug\hds.exe`: отказано в доступе»). Либо остановить
  сервисы, либо `cargo test --workspace --exclude hds-cli`.
* Порты 8010–8012 заняты резидентом — замеры/песочницы только после его остановки
  (`llm_host stop` — graceful, чистит pid), потом вернуть; для песочниц MCP удобны
  свободные 8797–8799.
* env-переменные персистентны в сессии — снимать в каждом прогоне
  (`Remove-Item Env:HDS_ROOT,Env:HDS_CONFIG -ErrorAction SilentlyContinue`).
* git — всегда `--no-pager`; PowerShell портит первый токен команды (например
  `Select-String` → `сSelect-String`) — начинайте команду с `Get-Content … |`/
  `Select-Object`; кириллица в выводе программ выглядит как cp866-мусор (числа читаемы),
  читать файлы — read-инструментом (не `git diff`/`Select-String`).
* `hds check --json` → `{"checks":[{id,status,title,msg,fix}],"ok":bool}` — фильтровать в
  пробах через `Where-Object { $_.id -eq 'queue' }`.
* `config.yaml` — в `.gitignore`; `tools/parity/out/` **целиком** гитигнорится
  (`.gitignore:50: out/`) — артефакты проб кладите туда.
* **Урок сессий T1–T3:** в начале работы в дереве могут быть «неожиданные»
  незакоммиченные изменения — это работа предыдущей сессии того же этапа (владелец
  подтверждает). Не откатывать: сверить дифф с областью трека и доращивать. Один активный
  чат на этап.

## Правила
* Сначала план по файлам, при согласовании — код. После каждого шага — тесты + коммит.
* Не ломать десктоп-профиль; не трогать чужие незакоммиченные изменения и боевые
  `PLAN_*.md` (обновлять `STATUS.md`, `README*.md`, журнал `PLAN_M4_CONCURRENCY_SLA.md`
  §8a; статус M4 в `PLAN_SERVER_MULTIUSER.md` §9 — по завершении этапа).
* Обязательно проверять изменения в рантайме (сеть/лимиты/очередь/БД/слоты — на живом
  сценарии), а не только юнитами.
* Держать хвосты: незакрытое переносить явно, а не «терять».



