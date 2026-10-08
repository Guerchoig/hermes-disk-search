# M4_HANDOFF_PROMPT_2.md — промт для нового чата (продолжение M4: треки T1–T6)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, правила и грабли.
> Предыдущий промт — `M4_HANDOFF_PROMPT.md` (в нём T0 ещё не был выполнен). Детальный план —
> `PLAN_M4_CONCURRENCY_SLA.md` (**§3** контракты, **§4** треки, **§5** приёмка, **§6** порядок,
> **§7** риски, **§8a** журнал). Код-ревью M3 — `M3_CODE_REVIEW.md` (**§7** — статус закрытия
> хвостов в T0). Контекст проекта — `PLAN_SERVER_MULTIUSER.md`, `README_SERVER_MULTIUSER.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние: **трек T0 этапа M4 закрыт** — коммиты `258cb74`…`df37fa8`, **HEAD `df37fa8`**, дерево чистое.
Станция: Windows, RTX 3060 12 ГБ (+ AMD-карта), Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1.
Ядро на **Rust** (`crates/`), Python — только `sidecar`.
Тесты: `cargo test --workspace` — **84 набора, все зелёные**; `cargo fmt --all --check` и
`cargo clippy --workspace --all-targets` — чисто.

## Что уже сделано (НЕ переделывать)
Трек **T0** (хвосты ревью M3/M2) закрыт; детали и коммиты — `PLAN_M4_CONCURRENCY_SLA.md` §8a,
`M3_CODE_REVIEW.md` §7:
- **F1** `token_issue` в аудите (+ поле `token` — префикс sha256, без самого токена);
- **F2/F3** UI: `admin_op` после маршрута с фактическим статусом; `unauthorized` с реальными `method`/`path`;
- **F4** дочитывание тела/запроса перед `413`/`503` (иначе RST) + сокет-тесты (детектор RST);
- **F6** «тяжёлый» запрос — разбором JSON-RPC (`hds_mcp::is_heavy_request`); **F7** `/health` вне rate-limit;
- **F8** пункт «периметр» в `hds check`/`/api/diagnostics`;
- **M2-F5** PID-файл `mcp-http` (пишется в `run`, чистится, проверка личности процесса);
- **F10.1/2/4/5/6** (версия убрана из `/health`, заголовки `X-RateLimit-*`, `tool_call` без `access.enabled`,
  404 без эха пути, read-timeout `http.read_timeout_sec`);
- **Хвосты M2** (логи LaunchDaemon, тест `role_from_args`, `access_path_from`, паритет `system_state_dir`).

Коммиты T0 по порядку: `258cb74` → `9d14980` → `f2cf449` → `1a6e80d` → `f07fbea` → `d4af8b3` →
`86df668` → `4ba31ec` → `bcd689e` (доки) → `df37fa8` (тест-премиса T1).

**Открыто осознанно (не код / не терять):**
- **F9 (`Host`)** — не реализовано: по ревью правильнее поправить формулировку `PLAN_M3` §3.1,
  а боевые `PLAN_*.md` не переписываем.
- **Вопросы заказчику (T0.9):** автотранскрибация в server-профиле (`install_server.ps1:69` снимает
  задачу и не заменяет службой); `hds login` без пароля в argv (`cmd/auth.rs`).

## ⚠ Важные ПРАВКИ к фактам плана M4 §1 (проверено по коду — учитывать, планируя T1)
1. **§1 М4 ошибается про БД:** `crates/hds-core/src/db.rs::connect` **уже** ставит
   `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=30000` (порт Python).
   Закреплено тестом `df37fa8`. Значит **T1 — это НЕ «включить WAL»**, а: read-only читатели +
   ключи `db.*` + стресс «5 чтений + писатель».
2. **Грабля read-only + WAL:** `SQLITE_OPEN_READONLY` на WAL-базе требует доступа к `-shm`
   (обычно нужна запись в каталог) либо флага `immutable=1`. И read-only соединение **нельзя**
   открывать через `connect()` — там DDL `CREATE TABLE IF NOT EXISTS` (на RO упадёт). Нужны
   отдельные `open_rw` (единственный писатель — pipeline/watcher) и `open_ro`.
3. Ключ rate-limit всё ещё **токен**, не пользователь (**F10.3**) — это задача **T4**, не T0.

## Точка входа (10 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -12`;
   `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short` (должно быть чисто).
2. Прочитать `PLAN_M4_CONCURRENCY_SLA.md` **целиком** (особенно §3.2–3.5, §4 T1–T6, §5, §6),
   затем `M3_CODE_REVIEW.md` §7, `PLAN_SERVER_MULTIUSER.md` §2 W3 / §5 Э4 / §9,
   `README_SERVER_MULTIUSER.md`.
3. `tools/parity/HANDOFF_PROMPT.md` (грабли окружения — §«Грабли»).
4. `cargo build --workspace` и `cargo test --workspace` — убедиться, что база зелёная
   (⚠ сначала см. грабли env ниже — иначе 2 теста `hds-llama` упадут).

## Что делаем дальше (T1 → T6; детали — `PLAN_M4_CONCURRENCY_SLA.md` §3–§6)

### T1 — БД под конкурентность (`feat(db)`), §3.4
- Ключи `db: { busy_timeout_ms, journal_mode, read_only_conns }` (server-дефолты; desktop — прежнее
  поведение). `busy_timeout` по плану — дефолт **5000** (сейчас зашито 30000 — сделать ключом).
- Разделить `open_rw` (единственный писатель — pipeline/watcher: WAL+DDL) и `open_ro` (читатели:
  MCP/UI/CLI/поиск — read-only, **без DDL**, pragmas совместимо; учесть `-shm`/`immutable`, см. §1 выше).
- Стресс-тест: 5 параллельных чтений + писатель → нет `SQLITE_BUSY`/«database is locked».
- Бэкап — только `VACUUM INTO`/`.backup` (пометка для Э10: WAL нельзя копировать «на горячую»).
- Файлы: `crates/hds-core/src/db.rs` (ядро), вызовы в `crates/hds-search`, `hds-mcp`, `hds-ui`, `hds-cli`.

### T2 — Слоты чата и VRAM-fit-gate (`feat(llmhost)`), §3.1
- `llm_server.parallel` (server — **5**), `ctx_per_slot`; проверка через `estimate_need_mib`/`check_fit`
  (`crates/hds-llama/src/budget.rs`). Не хватает VRAM → **явная ошибка с числами** (МиБ нужно/свободно)
  и подсказкой снизить `parallel`/`ctx_per_slot`; молчаливая деградация запрещена (решение заказчика).
- В `hds check`/`/api/diagnostics` — фактическое число слотов и занятая/свободная VRAM; числа в README.
- Файлы: `crates/hds-llama/src/{host.rs,config.rs,budget.rs,registry.rs}`, `crates/hds-index/src/diag.rs`,
  `config.example.yaml`.

### T3 — Очередь и fair-share по пользователю (`feat(queue)`), §3.2
- Очередь живёт в MCP-процессе перед тяжёлым `ask_my_files` (сейчас сразу `429`).
- Ключ — пользователь (`AuthCtx.user`; без токена — `local`); политика `rr` (round-robin между
  пользователями, FIFO внутри) | `fifo`; опц. приоритет в `access.yaml`.
- Конфиг `queue: { enabled, max_depth, wait_timeout_sec, policy }` (server — on/20/300; desktop — off).
- Переполнение → `429`, таймаут → `503`, оба с `Retry-After` + понятным `hint`; отмена клиентом
  освобождает место (`gpu.abandoned_request_sec`). Слоты `max_inflight` (M3) остаются — ограничивают
  исполнение, очередь — ожидание.
- Метрики (глубина очереди, ожидание p50/p95, отказы, пер-пользователь) — в `/api/diagnostics`, `hds check`, лог.
- Ограничение (документируем): очередь в MCP; UI-`ask` (админ) лимитируется своими лимитами M3.
- Файлы: `crates/hds-mcp/src/{http.rs,server.rs}`, `crates/hds-index/src/diag.rs`, `crates/hds-ui/src/lib.rs`.

### T4 — Квоты и приоритеты (`feat(quotas)`), §3.3
- Ключ лимитов/квот = **пользователь** (`user:<login>`), иначе `ip:<addr>`; опция `rate_limit.per_token`.
- `rate_limit.max_inflight_per_user` (дефолт 2), `requests_per_min_per_user`; 3-й одновременный RAG
  одного пользователя → `429` + `hint` про пер-пользовательский предел.
- **Два токена одного пользователя делят одну квоту** (регресс на дефект F10.3).
- Индексация уступает запросам (инвариант под нагрузкой, совместно с T1/T3).
- Файлы: `crates/hds-core/src/ratelimit.rs`, `crates/hds-mcp/src/http.rs`, `crates/hds-ui/src/lib.rs`.

### T5 — Нагрузочная приёмка 5 × `ask_my_files` (`test(load)`), §5
- Скрипт `tools/parity/load5.py` (5 клиентов/токенов шлют `ask_my_files` одновременно): латентность
  каждого (p50/p95), коды, глубина/ожидание очереди, VRAM/слоты → отчёт `tools/parity/out/m4_load.json`.
- Прогон на реальном индексе и карте; критерий **p95 ≤ 5 мин**. Снятие/возврат резидента —
  `cline-rules/dev-machine.md` (`llm_host run` или задача Планировщика, `installers/install_llm_host_task.ps1`).
- Регресс при `parallel: 2/3/5` и с параллельной индексацией. Чек-лист внешнего HTTPS (перенос из M3).

### T6 — Доклад (`docs(server)`), §6
- `README.md` (§«Слоты и SLA», очередь/квоты, WAL), `README_SERVER_MULTIUSER.md`, `STATUS.md`,
  `PLAN_SERVER_MULTIUSER.md` §9 (статус M4), журнал `PLAN_M4_CONCURRENCY_SLA.md` §8a.

## Контракты (кратко; полностью — §3)
- **Слоты:** `llm_server.parallel` (server — 5), `ctx_per_slot`; fit-gate с явной ошибкой с числами.
- **Очередь:** `queue.{enabled,max_depth,wait_timeout_sec,policy}`; ключ — пользователь, rr/FIFO;
  переполнение → `429`, таймаут → `503` (+`Retry-After`); метрики в диагностике.
- **Квоты:** `user:<login>` (иначе `ip:<addr>`), `rate_limit.max_inflight_per_user`,
  `requests_per_min_per_user`; пер-токенные лимиты — опция.
- **БД:** `journal_mode=WAL` + `busy_timeout` (уже есть) + read-only читатели, один писатель; ключи `db.*`.
- Всё — по конфигу, с серверными дефолтами и сохранением десктоп-поведения.

## Ключевые решения (не менять)
- **SLA ≤5 мин** для 5 одновременных RAG; при нехватке VRAM — **явная ошибка с числами**, не деградация.
- Периметр — reverse-proxy с TLS; наружу только `/mcp` и `/ui`; UI — только админ (M3).
- Идентификация — локальные токены; индексация — только админ; проект — единица выдачи (M1).
- Внешний API — **задел M5** (`chat.provider: remote`); в M4 только контракт ключей (§3.5).
- **Десктоп-профиль не ломаем:** очередь/квоты выключены, лимиты/аудит — как в M3 (выключены).

## Порядок работы
T1 → T2 → T3 → T4 → T5 → T6, по одному коммиту на шаг (`feat(db)`, `feat(llmhost)`, `feat(queue)`,
`feat(quotas)`, `test(load)`, `docs(server)`). После каждого шага — `cargo test --workspace` +
**рантайм-проверка** (правило этапа: сеть/лимиты/очередь/БД проверяются на живом сценарии, не только юнитами).

## Грабли окружения (важно!)
- **env-переменные:** в системе могут остаться `HDS_ROOT`/`HDS_CONFIG` от прошлых сессий (у меня были —
  указывали на `%TEMP%\hds-m3-verify`). Они загрязняют `host::repo_root()` и роняют **2 теста `hds-llama`**.
  В прогонах снимать: `Remove-Item Env:HDS_ROOT, Env:HDS_CONFIG -ErrorAction SilentlyContinue`.
- **Устаревший бинарь:** `cargo build -p <крейт>` **НЕ** пересобирает `hds.exe` (он линкует
  hds-ui/hds-mcp/etc). Для рантайм-проверок CLI всегда `cargo build --workspace` — иначе проверяете
  старый код (у меня так «всплыл» ложный `status:200` в аудите при ответе 404).
- `git` — **всегда** `--no-pager`. PowerShell портит первый токен команды; кириллица в `Select-String`
  **не ищется** (читать файл инструментом); `**` в PowerShell 5.1 не разворачивается.
- Скрипты `.ps1` без BOM PowerShell 5.1 читает как ANSI — держите временные пробы ASCII (или BOM).
- `config.yaml` — в `.gitignore`; изоляция проб: `HDS_ROOT` + `HDS_CONFIG` (песочница в `%TEMP%`).
- Запущенные `llm_host.exe`/`hds mcp-http`/`hds watch` держат `target\release\*.exe` → сборка падает
  «отказано в доступе»; на dev-машине их можно останавливать (`cline-rules/dev-machine.md`), кроме чужих.
- Замеры GPU: снять резидент и **вернуть** после (`llm_host run` / `installers/install_llm_host_task.ps1`).
- Проверка HTTP «как есть» — сырым сокетом (`System.Net.Sockets.TcpClient`); `Invoke-WebRequest` скрывает детали.
- read-инструмент может отвечать `[outdated]` на диапазон уже прочитанного файла — читайте файл целиком
  или через `Get-Content -Encoding UTF8`.

## Правила
- **Сначала план по файлам**, при согласовании — код. После каждого шага — тесты + коммит.
- Не ломать десктоп-профиль; не трогать чужие незакоммиченные изменения и боевые `PLAN_*.md`
  (обновлять `STATUS.md`, `README*.md`, журнал `PLAN_M4_CONCURRENCY_SLA.md` §8a; статус M4 в
  `PLAN_SERVER_MULTIUSER.md` §9 добавляется по завершении этапа, как в M1–M3).
- Обязательно проверять изменения **в рантайме** (сеть/лимиты/очередь/БД — на живом сценарии).
- Держать хвосты: незакрытое переносить явно (список «не входит» в плане M4), а не «терять».



