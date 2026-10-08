# M4_HANDOFF_PROMPT_4.md — промт для нового чата (продолжение M4: треки T3–T6)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, правила и грабли.
> Предыдущие промты — `M4_HANDOFF_PROMPT.md` (T0 не выполнен), `M4_HANDOFF_PROMPT_2.md`
> (T0 выполнен, T1 не начат), `M4_HANDOFF_PROMPT_3.md` (T0/T1 закрыты, T2 не начат).
> Детальный план — `PLAN_M4_CONCURRENCY_SLA.md` (§3 контракты, §4 треки, §5 приёмка,
> §6 порядок, §7 риски, §8a журнал). Код-ревью M3 — `M3_CODE_REVIEW.md`.
> Контекст проекта — `PLAN_SERVER_MULTIUSER.md`, `README_SERVER_MULTIUSER.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка `server-multiuser`.
Состояние: **T0, T1 и T2 закрыты**, HEAD — последний коммит из списка ниже (этот промт),
дерево чистое. Станция: Windows, RTX 3060 12 ГБ (+ AMD-карта), Ryzen 7 5700G, 64 ГБ RAM,
rustc/cargo 1.97.1. Ядро на Rust (`crates/`), Python — только sidecar.

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -6`  # HEAD — промт T3–T6
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`    # чисто
3. `cargo test --workspace` (если падает «отказано в доступе» на `target\debug\hds.exe` —
   живы `hds watch`/`hds mcp-http`/`hds transcribe-watch`: либо остановить их, либо
   `cargo test --workspace --exclude hds-cli`; см. «Грабли»)
4. `cargo clippy --workspace --all-targets -- -D warnings` и `cargo fmt --all --check`
5. Прочитать: `PLAN_M4_CONCURRENCY_SLA.md` §3 (контракты) → **§4 T3–T6** → §5 (приёмка) →
   §8a (журнал: что уже сделано в T0–T2); затем `README.md` §«Слоты и SLA»,
   `config.example.yaml` (`llm_server.parallel`, `queue.*` пока нет), `STATUS.md` (M4).
6. Проверить факт боевого состояния: `bin\llm_host.exe status` (роли, VRAM, `parallel 1`),
   `Test-Path .\index.pause` (боевая пауза индексации — не снимать без решения владельца).

## Коммиты этапа M4 по порядку
258cb74 → 9d14980 → f2cf449 → 1a6e80d → f07fbea → d4af8b3 → 86df668 → 4ba31ec →
bcd689e (доки T0) → df37fa8 (тест-премиса T1) → dfc0530 (доки: промт-2) →
**0395ab7 (feat(db) T1)** → **ca15b7e (доки T1)** → e0f4d13 (промт-3) →
**c07a367 (feat(llmhost) T2)** → **d54091e (доки T2)** → последний коммит — этот промт.

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
Ключи `db.{busy_timeout_ms,journal_mode,read_only_conns}` (server: 5000/wal/true;
desktop: 30000/false — прежнее поведение); `open_rw` (единственный писатель — конвейер
индексации/watcher) и `open_ro` (читатели MCP/UI/CLI — read-only, без DDL); `hds check`
показывает фактические PRAGMA; стресс «5 читателей + писатель» без `SQLITE_BUSY`
(`hds-core/tests/db_concurrency.rs`, debug и release); README: бэкап только
`VACUUM INTO`/`.backup` (пометка Э10).

### T2 — слоты чата и VRAM (закрыт, c07a367 + d54091e) — подробности в §8a
- Дефолт `llm_server.parallel` по профилю: **server 5 / desktop 1** (явное значение
  сильнее) — `hds-llama::config::build`; хелперы `hds_core::config::{llm_parallel,
  llm_parallel_default}` для диагностик/UI.
- **Найдены и исправлены два корневых дефекта** (до них `parallel` не давал эффекта):
  1. `InstanceSpec.n_parallel` не выставлялся вовсе → движок поднимал **1 слот**, а наш
     `/props` показывал `llm_server.parallel` (своё значение). Теперь слоты идут в
     `registry::plan_role` (у чата — `parallel`, у индексных ролей явная 1: у них
     делится контекст слота, а батч эмбеддингов один — `--batch-size 8192`).
  2. Шлюз движка (`Gate`) был `Mutex` и держался **всю генерацию** → вызовы шли строго по
     очереди. Теперь `RwLock`: генерация (chat/embeddings/rerank) — **разделяемый** режим
     (`ClusterShared::{with_shared,try_with_shared}`), управление кластером (загрузка/
     выгрузка/арбитр/наблюдение) — эксклюзивный. `Busy::holders` показывает число
     держателей; метка генерации осталась `engine` (страж брошенных запросов и heartbeat
     её узнают).
  3. Быстрый путь запроса: роль уже загружена (`ClusterBackend::role_loaded`, разделяемая
     проверка) → сразу генерация; `prepare`+`ensure_loaded` (эксклюзивные) только когда
     роль НЕ загружена. Пауза индексации сохраняется — её держит `PauseLease` (без
     обращения к кластеру).
- **fit-гейт на старте** (`Host::start`, режим embedded): «модель + KV×parallel +
  compute-буфер» против свободной VRAM с `gpu.reserve_mb`. Чат не влез — **явная ошибка**
  с числами (нужно/свободно/резерв/не хватает) и подсказкой «снизить parallel/
  ctx_per_slot или `llm.chat.n_batch: 512`»; индексные роли (LOAD_ON_DEMAND) — громкое
  предупреждение. Молчаливой деградации нет.
- **Честный KV**: `budget::per_slot_ctx(n_ctx_total, parallel)` + KV считается от контекста
  **слота** (llama.cpp делит общий `--ctx-size` между слотами). Прежняя формула умножала
  общий контекст на `parallel` ещё раз → завышение в `parallel` раз (при parallel=1 не
  видно). Тест `budget::kv_is_linear_in_total_tokens`.
- `hds check` (и, значит, `/api/diagnostics` в UI): пункт `chat` показывает **фактические**
  слоты из `/props.total_slots` рядом с настроенным `parallel`; расхождение — warn с
  подсказкой перезапустить резидента. VRAM (наша/чужая/занято) — как было, в
  `gpu-observability`.
- Замеры (RTX 3060, 5 одновременных чат-запросов по 300 токенов, KV q8_0):
  **2 слота × 32768** → VRAM 8129 МиБ, wall 33,7 с; **3 × 32768** → 8723 МиБ, 27,1 с;
  **5 × 16384** → 8553 МиБ, **21,3 с (все пять параллельно, по ~20,8 с)**;
  **5 × 32768** → отказ гейта (нужно 10668 + резерв 1024 > свободно 11385);
  **5 × 65536** (боевой контекст) → отказ (13388 + 1024 > 11385). До правок те же 5
  запросов шли последовательно — 43,4 с. Отчёты — `tools/parity/out/m4_slots_*.json`
  (гитигнор), числа — `README.md` (§«Слоты и SLA») и `config.example.yaml`.
- Развилка для боевого профиля (зафиксирована в README/журнале): либо `parallel: 5` с
  `ctx_per_slot: 16384` (тогда `chat.max_context_chars` ≲ 32000), либо `parallel: 2–3` с
  `ctx_per_slot: 32768`. Боевой `config.yaml` НЕ менялся (там явный `parallel: 1`).


## Что осталось (T3–T6) — по §3/§4/§5 плана

### T3. Очередь и fair-share по пользователю — 2–3 дн
- **Где именно врезаться** (проверено по коду): `crates/hds-mcp/src/http.rs` —
  `serve_request` (≈542), ветка тяжёлого запроса (≈584: `is_heavy_request(b)` →
  `limits.inflight_acquire(&key)` → `route(...)` → `inflight_release`), `too_many(inflight,
  retry)` (≈383) и `rate_limit_headers` (≈400); ключ лимитов сейчас — токен|IP (≈558-560).
  `crates/hds-core/src/ratelimit.rs` — `Limiter::{check,check_login,inflight_acquire,
  inflight_release,inflight_guard}` (≈111-192), конфиг `rate_limit.*` (≈27-57, `max_inflight`
  дефолт 4). Пользователь: `crates/hds-mcp/src/auth.rs::resolve_bearer` (≈87) →
  `AuthCtx.user` (`crates/hds-core/src/access.rs` ≈83-85) — из него `user:<login>`, иначе
  `ip:<addr>`. Роут/инструмент: `crates/hds-mcp/src/schema.rs` (≈81) → `tools::ask_my_files`
  (`tools.rs` ≈175).
- Модуль очереди (в `hds-mcp` или `hds-core`, чтобы переиспользовать; чистая логика —
  тестируемая без сети): ключ пользователя, round-robin между пользователями + FIFO внутри
  пользователя (`queue.policy: rr|fifo`), лимит глубины, таймаут ожидания; слот
  `max_inflight` (M3) остаётся лимитом **исполнения**, очередь — ожидания.
- Интеграция в `serve_request` **перед** тяжёлым `ask_my_files` (вместо нынешнего «сразу
  429»): `429` при переполнении, `503`+`Retry-After` при таймауте; освобождение места при
  обрыве клиента (проверка живости соединения / `gpu.abandoned_request_sec`).
- Конфиг `queue: {enabled, max_depth, wait_timeout_sec, policy}`: server — включено
  (20/300), desktop — выключено (поведение как сейчас).
- Метрики (глубина, ожидание p50/p95, отказы, «кто кого ждёт» пер-пользователь) в
  `/api/diagnostics` и `hds check`; журнал/аудит ожидания.
- Тесты: 5 «пользователей» (5 токенов) → распределение; один «жадный» не морит остальных;
  переполнение/таймаут → корректные коды.
- Ограничение (документируем): очередь живёт в MCP-процессе; `hds ui` (админский `ask`)
  лимитируется своими лимитами M3; общая очередь на оба процесса — вне M4.

### T4. Квоты и приоритеты — 1–1,5 дн
- Ключ лимитов/квот = **пользователь** (`user:<login>`), иначе `ip:<addr>`; опция
  `rate_limit.per_token`.
- `rate_limit.max_inflight_per_user` (дефолт 2) и `requests_per_min_per_user`; проверка:
  3-й одновременный RAG одного пользователя → `429` + `hint` про пер-пользовательский предел.
- Инвариант «индексация уступает»: запрос не ждёт индексного писателя БД (T1), индекс
  встаёт на паузу (`index.pause`, `gpu.pause_index_on_query`).
- Тест: два токена одного пользователя делят одну квоту (регресс на «квота удваивается»,
  `M3_CODE_REVIEW.md` F10.3).

### T5. Нагрузочная приёмка 5 × `ask_my_files` — 1–2 дн
- Скрипт нагрузки (`tools/parity/load5.py` или `measure_run.py`): 5 клиентов (5 токенов)
  шлют **реальный** `ask_my_files` (через MCP `/mcp`, а не сырой чат!) одновременно;
  фиксирует латентность каждого (p50/p95), коды, глубину/ожидание очереди, VRAM/слоты.
  **Готовая заготовка** для измерения слотов (замеры T2) —
  `%TEMP%\hds-m4-t2\run_variant.ps1` + `config_p2/p3/p5*.yaml` (копия боевого конфига с
  другим `parallel`/`ctx_per_slot`; песочница через `HDS_ROOT`/`HDS_CONFIG`).
- Прогон на реальном индексе и реальной карте; отчёт `tools/parity/out/m4_load.json` +
  сводка **p95 ≤ 5 мин** (напоминание: 300 токенов чата = 21 с при 5 слотах; реальный RAG
  добавляет поиск + 27k-токенный промпт).
- Регресс при `parallel: 2/3/5` и с индексацией параллельно (проверка «индекс уступает»).
- Чек-лист внешнего HTTPS (перенос из M3) — на машине с доменом/сертификатом.

### T6. Доклад — 0,5 дн
- `README.md` (§«Слоты и SLA» уже есть — дополнить очередью/квотами/WAL),
  `README_SERVER_MULTIUSER.md`, `STATUS.md`, `PLAN_SERVER_MULTIUSER.md` §9 (статус M4),
  журнал §8a.
- Закрыть `M3_CODE_REVIEW.md` в части T0 (отметить закрытые находки), не удаляя историю.

## Ключевые решения (не менять)
- SLA ≤5 мин для 5 одновременных RAG; при нехватке VRAM — явная ошибка с числами, не деградация.
- Слоты чата: `llm_server.parallel` (server 5 / desktop 1); `ctx_per_slot` подбирается под RAG
  (замеры T2: 5×16384 либо 2–3×32768 на 12 ГБ).
- Периметр — reverse-proxy с TLS; наружу только `/mcp` и `/ui`; UI — только админ (M3).
- Идентификация — локальные токены; индексация — только админ; проект — единица выдачи (M1).
- Внешний API — задел M5 (`chat.provider: remote`); в M4 только контракт ключей (§3.5).
- Десктоп-профиль не ломаем: очередь/квоты выключены, `llm_server.parallel` — 1,
  `db.*` — 30000/false, лимиты/аудит — как в M3.

## Грабли окружения (важно! — пополнено сессией T2)
- **`run_commands`: каждый элемент массива = ОТДЕЛЬНАЯ PowerShell-сессия** — переменные
  (`$d`, `$c`) НЕ живут между элементами; связка «присвоил в одном, использовал в другом»
  молча даёт `$null`. Многошаговая проба — ОДНИМ элементом через `;` или `.ps1`-файлом.
- **Пробы с кодом — файлом.** Сложное inline-цитирование PowerShell ломает код тихо;
  писать `.ps1` инструментом editor (ASCII-only), запускать
  `powershell -NoProfile -ExecutionPolicy Bypass -File <файл>`.
- **Не писать файлы через `Set-Content -Encoding utf8`**: PS 5.1 добавит BOM (Rust его
  компилирует, но диффы «грязные»). Для перезаписи целого файла:
  `[System.IO.File]::WriteAllText($p, $txt, (New-Object System.Text.UTF8Encoding($false)))`.
- **`Start-Process -PassThru` + `-RedirectStandardOutput`**: `$p.ExitCode` может вернуть
  `$null`, а `HasExited` — «живой» процесс. Надёжно: цикл `$p.WaitForExit(0)` (0 мс —
  мгновенная проверка), а причину отказа читать из перенаправленных файлов (`$log`,
  `$log + '.err'`), не из data-лога.
- Запущенные процессы проекта держат exe: релизный резидент — `target\release\*.exe`,
  а `hds watch` / `hds mcp-http` / `hds transcribe-watch` (debug) — `target\debug\hds.exe`.
  Перед `cargo build --workspace` их останавливать (`cline-rules/dev-machine.md` разрешает),
  после работ — возвращать тем же способом:
  `Start-Process -WindowStyle Hidden` для
  `target\release\llm_host.exe run`, `target\debug\hds.exe watch`,
  `target\debug\hds.exe mcp --http --host 127.0.0.1 --port 8787 --path /mcp`,
  `target\debug\hds.exe transcribe-watch`.
- **`cargo test --workspace` при живых сервисах падает не тестами, а сборкой**
  («failed to remove `target\debug\hds.exe`: отказано в доступе»). Либо остановить
  сервисы, либо `cargo test --workspace --exclude hds-cli`.
- **Порты 8010–8012 заняты резидентом** — замеры/песочницы только после его остановки
  (`llm_host stop` — graceful, чистит pid), потом вернуть.
- env-переменные: `HDS_ROOT`/`HDS_CONFIG` персистентны в сессии — снимать в каждом прогоне
  (`Remove-Item Env:HDS_ROOT,Env:HDS_CONFIG -ErrorAction SilentlyContinue`).
- git — всегда `--no-pager`; PowerShell портит первый токен команды; кириллица в
  `Select-String` по `.ps1` без BOM не ищется; `**` в PS 5.1 не разворачивается.
- `git diff` в PowerShell показывает UTF-8 как кракозябры (cp866) — читайте файлы
  read-инструментом или `Get-Content -Encoding UTF8`. Кириллица в выводе самих программ
  (llm-host/hds) в консоли тоже выглядит как cp866-мусор — это нормально, числа читаемы.
- `config.yaml` — в `.gitignore`; изоляция проб: `HDS_ROOT` + `HDS_CONFIG` (песочница в
  `%TEMP%`), артефакты — в `tools/parity/out/` (гитигнор).
- **Урок сессий T1/T2:** в начале работы в дереве могут быть «неожиданные» незакоммиченные
  изменения — это работа предыдущей сессии того же этапа (владелец подтверждает). Не
  откатывать: сверить дифф с областью трека и доращивать. Один активный чат на этап.

## Правила
- Сначала план по файлам, при согласовании — код. После каждого шага — тесты + коммит.
- Не ломать десктоп-профиль; не трогать чужие незакоммиченные изменения и боевые
  `PLAN_*.md` (обновлять `STATUS.md`, `README*.md`, журнал `PLAN_M4_CONCURRENCY_SLA.md`
  §8a; статус M4 в `PLAN_SERVER_MULTIUSER.md` §9 — по завершении этапа).
- Обязательно проверять изменения в рантайме (сеть/лимиты/очередь/БД/слоты — на живом
  сценарии), а не только юнитами.
- Держать хвосты: незакрытое переносить явно, а не «терять».


