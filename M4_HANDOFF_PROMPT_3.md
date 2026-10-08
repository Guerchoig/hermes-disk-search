# M4_HANDOFF_PROMPT_3.md — промт для нового чата (продолжение M4: треки T2–T6)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, правила и грабли.
> Предыдущие промты — `M4_HANDOFF_PROMPT.md` (T0 не выполнен) и `M4_HANDOFF_PROMPT_2.md`
> (T0 выполнен, T1 не начат). Детальный план — `PLAN_M4_CONCURRENCY_SLA.md` (§3 контракты,
> §4 треки, §5 приёмка, §6 порядок, §7 риски, §8a журнал). Код-ревью M3 — `M3_CODE_REVIEW.md`.
> Контекст проекта — `PLAN_SERVER_MULTIUSER.md`, `README_SERVER_MULTIUSER.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка `server-multiuser`.
Состояние: **T0 и T1 закрыты**, HEAD — последний коммит из списка ниже (этот промт), дерево чистое.
Станция: Windows, RTX 3060 12 ГБ (+ AMD-карта), Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1.
Ядро на Rust (crates/), Python — только sidecar.

## Коммиты этапа M4 по порядку
258cb74 → 9d14980 → f2cf449 → 1a6e80d → f07fbea → d4af8b3 → 86df668 → 4ba31ec →
bcd689e (доки T0) → df37fa8 (тест-премиса T1) → dfc0530 (доки: промт-2) →
**0395ab7 (feat(db) T1)** → **ca15b7e (доки T1)** → последний коммит — этот промт.

## Что уже сделано (НЕ переделывать)

### T0 — хвосты ревью M3/M2 (закрыт)
`token_issue` в аудите (+ отпечаток токена); `admin_op` после маршрута с фактическим
статусом; `unauthorized` с method/path; дочитывание тела перед 413/503 + сокет-тесты;
«тяжёлый» запрос разбором JSON-RPC; `/health` вне rate-limit; пункт «периметр» в
`hds check`; PID-файл mcp-http с проверкой личности; F10 (версия убрана из `/health`,
`X-RateLimit-*`, tool_call без access.enabled, 404 без эха пути, read-timeout); хвосты M2.
Открыто осознанно: **F9 (Host)** — не реализовано (правильнее поправить формулировку
PLAN_M3 §3.1, боевые PLAN не переписываем); **T0.9** — вопросы заказчику
(автотранскрибация в server-профиле, `hds login` без пароля в argv).

### T1 — БД под конкурентность (закрыт, 0395ab7 + ca15b7e)
- **Ключи `db.*`** (`crates/hds-core/src/config.rs`): `db.busy_timeout_ms` (server → **5000**,
  desktop → **30000** — прежнее зашитое значение; решение владельца 08.10.2026 — desktop
  не менять), `db.read_only_conns` (server → true, desktop → false), `db.journal_mode`
  (дефолт `wal`; значение фильтруется `safe_journal_mode` перед подстановкой в PRAGMA).
- **Разделение соединений** (`crates/hds-core/src/db.rs`): `DbOptions{busy_timeout_ms,
  journal_mode}`; `open_rw(path, dim, &opts)` — единственный писатель (конвейер
  индексации/watcher; DDL+WAL+synchronous+foreign_keys, busy_timeout из конфига);
  `open_ro(path, &opts)` — читатели read-only без DDL (отсутствующая база → явная ошибка;
  грабля `-shm`/`immutable=1` задокументирована в док-комментарии). `connect()` сохранён
  как обёртка `open_rw` с дефолтами (Python-паритет, ~25 существующих тестов не тронуты).
- **Читатели → `open_ro`**: MCP (`search_local_files`, `ask_my_files`, `index_status`),
  UI (`/api/status`, `/api/search`, `/api/ask`, `tree.rs`), CLI (`status`, `search`, `ask`
  через `support::open_conn_ro`). Писатели → `open_rw`: CLI (`index`, `reindex`,
  `reindex-fts`, `forget`, `clip-index`, `watch` через `support::open_conn`), MCP
  (`reindex_path`, `run_indexing`), UI (`run_index_bg`), `hds check` (`check_db` —
  показывает фактические `journal_mode`/`busy_timeout`).
- **Стресс-тест** `crates/hds-core/tests/db_concurrency.rs` (4 теста): «5 параллельных
  читателей + писатель» без `SQLITE_BUSY`/«database is locked» (прогнан в debug **и**
  release), RO-отказ записи, явная ошибка на отсутствующей базе, тесты ключей/дефолтов.
- **Рантайм-проверки** (правило этапа): песочницы в `%TEMP%` через `HDS_ROOT`/`HDS_CONFIG`
  — живой писатель `hds index` + параллельные `hds status`/`hds search`: 0 «locked»;
  `hds check`: server → `journal_mode=wal, busy_timeout=5000 мс`, desktop →
  `busy_timeout=30000 мс`. Песочницы: `%TEMP%\hds-m4-t1-verify` (server, ключи db.*),
  `%TEMP%\hds-m4-t1-desktop` (desktop) — можно переиспользовать или удалить.
- README.md: пометка «бэкап — только `VACUUM INTO`/`.backup` (WAL нельзя копировать
  файлом на горячую)» — для Э10; `config.example.yaml` — секция `db:` (закомментирована).
- Журнал §8a: строки T1 (премиса + реализация); `STATUS.md` обновлён.

## ⚠ Важные факты для T2 (проверено по коду/плану)
1. Бюджет VRAM уже умеет KV под n_parallel: `kv_cache_mib(meta, n_ctx, n_parallel,
   kv_bits)`, `estimate_need_mib`, `check_fit(free, need, reserve)` —
   `crates/hds-llama/src/budget.rs:25-105`. T2 — интеграция fit-gate в старт роли,
   а не написание бюджета.
2. `llm_server.parallel: 1` сейчас — дефолт в `config.example.yaml` (секция
   llm_server); серверный дефолт должен стать 5 (desktop — 1). Ключи `llm_server.*`
   читаются в `hds-llama/src/config.rs`.
3. Роли/фасад: chat `:8010`, embedding `:8011`, rerank `:8012`; владелец портов —
   `bin\llm_host.exe`; арбитр VRAM (`gpu.policy`, `priorities`, `evict_idle_sec`,
   `abandoned_request_sec`, `pause_index_on_query`) уже работает.
4. Индексация «уступает» частично: `gpu.policy: query_priority` +
   `pause_index_on_query: true` (файл `index.pause`) уже есть.
5. RAG `ask` ходит в фасад с таймаутом `chat.timeout`; потолок — `chat.rag_max_tokens`,
   бюджет контекста — `chat.max_context_chars`.

## Точка входа (10 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -14`;
   `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short` (должно быть чисто).
2. Прочитать `PLAN_M4_CONCURRENCY_SLA.md` §3.1 (слоты/VRAM-контракт), §4 T2, §5, §6;
   затем `crates/hds-llama/src/{config.rs,budget.rs,host.rs,facade.rs,registry.rs}` —
   как читаются `llm_server.*`, где стартует роль chat, что передаётся движку
   (`--parallel`/`-np`), как diag получает данные о ролях/VRAM.
3. `tools/parity/HANDOFF_PROMPT.md` (грабли окружения) + `tools/parity/W2_REPORT.md`
   (§NVML/замеры) — образец сэмплера VRAM.
4. `cargo build --workspace` и `cargo test --workspace` — база зелёная
   (⚠ сначала см. грабли env ниже — иначе тесты hds-llama упадут на грязном env).

### T2 — Слоты чата и VRAM-fit-gate (feat(llmhost)), §3.1
- Ключи `llm_server.parallel` (server-дефолт **5**, desktop — 1) и
  `llm_server.chat.ctx_per_slot` (подобрать 16384…32768 под RAG).
- Fit-gate на старте роли: `estimate_need_mib` + `check_fit(free, need, reserve)`;
  **не хватает → явная ошибка с числами** (МиБ нужно/свободно) и подсказкой
  «снизить parallel/ctx_per_slot»; молчаливая деградация запрещена (решение заказчика).
- В `hds check`/`/api/diagnostics` — фактическое число слотов и занятая/свободная VRAM.
- Замеры VRAM/латентности при `parallel: 2/3/5` на 12 ГБ (NVML-сэмплер, как в W2) →
  числа в `config.example.yaml`/README. Резидента снимать/возвращать по
  `cline-rules/dev-machine.md` (`llm_host.exe stop` → замеры → `llm_host run` или задача
  Планировщика `installers/install_llm_host_task.ps1`). Замеры можно выделить в подшаг
  в конце T2 (или вместе с T5) — но числа в конфиге/README обязательны.
- Файлы: `crates/hds-llama/src/{host.rs,config.rs,budget.rs,registry.rs}`,
  `crates/hds-index/src/diag.rs`, `config.example.yaml`, README.md.


### T3 — Очередь и fair-share по пользователю (feat(queue)), §3.2
- Очередь в MCP-процессе перед тяжёлым `ask_my_files` (сейчас сразу 429). Ключ —
  пользователь (`AuthCtx.user`; без токена — `local`); политика `rr` (round-robin между
  пользователями, FIFO внутри) | `fifo`; опц. приоритет в access.yaml.
- Конфиг `queue: { enabled, max_depth, wait_timeout_sec, policy }` (server — on/20/300;
  desktop — off). Переполнение → 429, таймаут → 503, оба с Retry-After + hint; отмена
  клиентом освобождает место (`gpu.abandoned_request_sec`). Слоты `max_inflight` (M3)
  остаются — ограничивают исполнение, очередь — ожидание.
- Метрики (глубина, ожидание p50/p95, отказы, пер-пользователь) — в `/api/diagnostics`,
  `hds check`, лог. Ограничение документируем: очередь в MCP; UI-ask лимитируется M3.
- Файлы: `crates/hds-mcp/src/{http.rs,server.rs}`, `crates/hds-index/src/diag.rs`,
  `crates/hds-ui/src/lib.rs`. Модуль очереди — чистая логика, тестируемая без сети.

### T4 — Квоты и приоритеты (feat(quotas)), §3.3
- Ключ лимитов = пользователь (`user:<login>`), иначе `ip:<addr>`; опция `rate_limit.per_token`.
- `rate_limit.max_inflight_per_user` (дефолт 2), `requests_per_min_per_user`;
  3-й одновременный RAG одного пользователя → 429 + hint. Два токена одного пользователя
  делят одну квоту (регресс на дефект F10.3). Индексация уступает запросам (инвариант,
  совместно с T1/T3).
- Файлы: `crates/hds-core/src/ratelimit.rs`, `crates/hds-mcp/src/http.rs`,
  `crates/hds-ui/src/lib.rs`.

### T5 — Нагрузочная приёмка 5 × ask_my_files (test(load)), §5
- Скрипт `tools/parity/load5.py` (5 клиентов/токенов шлют `ask_my_files` одновременно):
  латентность каждого (p50/p95), коды, глубина/ожидание очереди, VRAM/слоты →
  отчёт `tools/parity/out/m4_load.json` (out/ гитигнорится). Критерий p95 ≤ 5 мин.
- Прогон на реальном индексе и карте; регресс при `parallel: 2/3/5` и с параллельной
  индексацией. Чек-лист внешнего HTTPS (перенос из M3) — на машине с доменом.

### T6 — Доклад (docs(server)), §6
- README.md (§«Слоты и SLA», очередь/квоты, WAL), `README_SERVER_MULTIUSER.md`,
  `STATUS.md`, `PLAN_SERVER_MULTIUSER.md` §9 (статус M4), журнал §8a.

## Контракты (кратко; полностью — §3)
- Слоты: `llm_server.parallel` (server — 5), `ctx_per_slot`; fit-gate с явной ошибкой с числами.
- Очередь: `queue.{enabled,max_depth,wait_timeout_sec,policy}`; ключ — пользователь,
  rr/FIFO; переполнение → 429, таймаут → 503 (+Retry-After); метрики в диагностике.
- Квоты: `user:<login>` (иначе `ip:<addr>`), `rate_limit.max_inflight_per_user`,
  `requests_per_min_per_user`; пер-токенные лимиты — опция.
- БД: WAL + `busy_timeout` + read-only читатели, один писатель; ключи `db.*` (T1, готово).
- Всё — по конфигу, с серверными дефолтами и сохранением десктоп-поведения.

## Ключевые решения (не менять)
- SLA ≤5 мин для 5 одновременных RAG; при нехватке VRAM — явная ошибка с числами, не деградация.
- Периметр — reverse-proxy с TLS; наружу только `/mcp` и `/ui`; UI — только админ (M3).
- Идентификация — локальные токены; индексация — только админ; проект — единица выдачи (M1).
- Внешний API — задел M5 (`chat.provider: remote`); в M4 только контракт ключей (§3.5).
- Десктоп-профиль не ломаем: очередь/квоты выключены, `llm_server.parallel` — 1,
  `db.*` — 30000/false (T1), лимиты/аудит — как в M3.
- Дефолт `db.busy_timeout_ms`: server 5000 / desktop 30000 (решение владельца 08.10.2026).


## Грабли окружения (важно! — пополнено сессией T1)
- **`run_commands`: каждый элемент массива = ОТДЕЛЬНАЯ PowerShell-сессия** — переменные
  (`$sb`, `$code`) НЕ живут между элементами; связка «присвоил в одном элементе,
  использовал в другом» молча даёт `$null`. Любая многошаговая проба — ОДНИ элементом
  через `;`, а лучше — `.ps1`-скриптом.
- **Сложное цитирование в inline-PowerShell ломает код тихо** (кейс: python-probe с
  `-replace '"', "'"` дал пустой код без ошибок; here-strings в inline-командах ломаются
  парсером). Пробы с кодом: писать файл инструментом editor (ASCII-only), запускать
  `powershell -NoProfile -ExecutionPolicy Bypass -File <файл>`.
- env-переменные: `HDS_ROOT`/`HDS_CONFIG` от прошлых прогонов персистентны в сессии —
  снимать в каждом прогоне:
  `Remove-Item Env:HDS_ROOT,Env:HDS_CONFIG -ErrorAction SilentlyContinue`.
- Устаревший бинарь: `cargo build -p <крейт>` НЕ пересобирает `hds.exe` (он линкует
  hds-ui/hds-mcp/etc). Для рантайм-проверок CLI всегда `cargo build --workspace`.
- git — всегда `--no-pager`. PowerShell портит первый токен команды; кириллица в
  `Select-String` по `.ps1` без BOM не ищется (скрипты — ASCII-only); `**` в PS 5.1
  не разворачивается (использовать `Get-ChildItem -Recurse | Select-String`).
- `git diff` в PowerShell-выводе показывает UTF-8 как кракозябры (cp866) — содержимое
  читайте `Get-Content -Encoding UTF8` или read-инструментом; read-инструмент отвечает
  `[outdated]` на диапазон уже прочитанного файла — читайте файл целиком или диапазонами
  через `Get-Content` с индексами (`$c[85..300]`).
- Скрипты `.ps1` без BOM PowerShell 5.1 читает как ANSI — держите пробы ASCII (или BOM).
- `config.yaml` — в `.gitignore`; изоляция проб: `HDS_ROOT` + `HDS_CONFIG` (песочница в %TEMP%).
- Запущенные `llm_host.exe`/`hds mcp-http`/`hds watch` держат `target\release\*.exe` →
  сборка падает «отказано в доступе»; на dev-машине их можно останавливать
  (`cline-rules/dev-machine.md`), кроме чужих. Замеры GPU: снять резидента и вернуть
  после (`llm_host run` / `installers/install_llm_host_task.ps1`).
- Проверка HTTP «как есть» — сырым сокетом (`System.Net.Sockets.TcpClient`);
  `Invoke-WebRequest` скрывает детали.
- **Урок сессии T1:** в начале сессии оказалось несколько «неожиданных» незакоммиченных
  изменений — это была работа предыдущей сессии того же этапа (владелец подтвердил).
  Не паниковать и не откатывать: сверить дифф с областью текущего трека, спросить
  владельца, доращивать. Один активный чат на этап.

## Правила
- Сначала план по файлам, при согласовании — код. После каждого шага — тесты + коммит.
- Не ломать десктоп-профиль; не трогать чужие незакоммиченные изменения и боевые
  `PLAN_*.md` (обновлять `STATUS.md`, `README*.md`, журнал `PLAN_M4_CONCURRENCY_SLA.md`
  §8a; статус M4 в `PLAN_SERVER_MULTIUSER.md` §9 — по завершении этапа).
- Обязательно проверять изменения в рантайме (сеть/лимиты/очередь/БД — на живом сценарии).
- Держать хвосты: незакрытое переносить явно (список «не входит» в плане M4), а не «терять».

