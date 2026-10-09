# M5_HANDOFF_PROMPT.md — промт для нового чата (M4 закрыт, старт следующего этапа)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, правила и грабли.
> Предыдущие промты — `M4_HANDOFF_PROMPT.md`…`_5.md` (**этап M4 закрыт целиком: T0–T6**).
> Детальный план M4 с журналом исполнения — `PLAN_M4_CONCURRENCY_SLA.md` (§3 контракты,
> §4 треки, §5 приёмка, §7 риски, **§8a журнал — что именно сделано и чем проверено**).
> Код-ревью M3 (включая закрытие хвостов T0) — `M3_CODE_REVIEW.md` (§7 — статус хвостов).
> Контекст проекта и дорожная карта — `PLAN_SERVER_MULTIUSER.md` (§5 этапы Э1–Э11, §9 статусы),
> сценарии — `README_SERVER_MULTIUSER.md`, сводка — `STATUS.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние: **M4 закрыт** (T0–T6), HEAD — коммит этого промта (предыдущий — `212128a`, доклад T6),
дерево чистое. Станция: Windows, RTX 3060 12 ГБ (+ AMD-карта), Ryzen 7 5700G, 64 ГБ RAM,
rustc/cargo 1.97.1. Ядро на Rust (`crates/`), Python — только sidecar. `.venv` — для проб/sidecar.

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -8`   # HEAD — этот промт
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`     # чисто
3. `cargo test --workspace` — ⚠ **живые сервисы держат `target\debug\hds.exe`** (см. «Грабли»):
   либо остановить `hds watch` / `hds mcp-http --http ...` / `hds transcribe-watch`, либо
   `cargo test --workspace --exclude hds-cli`; после работ сервисы **вернуть**.
4. `cargo clippy --workspace --all-targets -- -D warnings` и `cargo fmt --all --check`
5. Прочитать: `PLAN_SERVER_MULTIUSER.md` §5 (этапы) и §9 (статусы M1–M4) → **`STATUS.md`**
   (M4 целиком) → `PLAN_M4_CONCURRENCY_SLA.md` **§8a** (журнал: контракты в коде и числа) →
   `README.md` §«Слоты и SLA» (слоты/очередь/**квоты по пользователю**/нагрузочная приёмка) →
   `config.example.yaml` (`db.*`, `llm_server.parallel`, `rate_limit.*`, `queue.*`).
6. Проверить факт боевого состояния (не менять!):
   `& .\target\release\llm_host.exe status` (в этой сессии: `parallel 1`, chat LOADED n_ctx=65536,
   embedding/rerank `on_demand`), `Test-Path .\index.pause` (**боевая пауза — не снимать**, сейчас есть),
   `& .\target\debug\hds.exe check` (desktop-профиль: пункт `queue` = «выключена»).

## Коммиты этапа M4 (по порядку; T0–T6 — закрыто)
258cb74 → 9d14980 → f2cf449 → 1a6e80d → f07fbea → d4af8b3 → 86df668 → 4ba31ec (T0),
bcd689e (доки T0), df37fa8 (тест-премиса T1), dfc0530 (промт-2), **0395ab7** (feat T1),
**ca15b7e** (доки T1), e0f4d13 (промт-3), **c07a367** (feat T2), **d54091e** (доки T2),
425016a (промт-4), 2779164 (промт-4: якоря T3), **dd2e9af** (feat T3), **c4542f6** (доки T3),
36e0e46 (промт-5), **dd093fc** (feat **T4**), **9f1de05** (доки T4), **1e4d5c5** (стенд **T5**),
**212128a** (доки **T6**), последний — этот промт.

## Что закрыто (НЕ переделывать)
* **T0** — хвосты ревью M3 F1–F8/F10 (аудит, доставка 413/503, разбор JSON-RPC «тяжёлый»,
  `/health` вне лимитов, пункт «периметр», PID-файл mcp-http, read-timeout, 404 без эха, `X-RateLimit-*`).
* **T1** — БД под конкурентность: `db.{busy_timeout_ms,journal_mode,read_only_conns}`
  (server 5000/wal/true, desktop 30000/false), `open_rw` (один писатель) / `open_ro` (читатели),
  стресс «5 читателей + писатель» без `SQLITE_BUSY`.
* **T2** — слоты чата: `llm_server.parallel` (server 5 / desktop 1), `n_parallel` доходит до
  движка, шлюз `RwLock` (генерация разделяемо), fit-гейт VRAM с числами; замеры —
  `tools/parity/out/m4_slots_*.json`.
* **T3** — очередь тяжёлых RAG `hds-core::queue`: fair-share round-robin по пользователю
  (`user:<login>`→`ip:<addr>`→`local`), `429` (полна) / `503` (таймаут) + `Retry-After` +
  `X-Queue-Depth`, снапшот `<state_dir>/data/mcp-queue.json` (2 с), пункт `queue` в `hds check`.
* **T4** — квоты и приоритеты: **ключ лимитов/квот = ПОЛЬЗОВАТЕЛЬ** (`hds-mcp::http::limit_key`:
  `user:<login>` по валидному Bearer, иначе `ip:<addr>`; это закрыло M3 F10.3),
  `rate_limit.{max_inflight_per_user=2, requests_per_min_per_user, per_token}`,
  `Limiter::user_inflight_limit() = min(max_inflight_per_user, max_inflight)`,
  `QueueReject::PerUser` → немедленный `429` + hint (в очереди; только если предел строже
  глобального), `rejected_user`/`max_inflight_per_user` в снапшоте и диагностике.
  Тесты: `hds-core::{ratelimit,queue}`, `crates/hds-mcp/tests/quota_socket.rs`.
* **T5** — стенд **`tools/parity/load5.py`**: 5 одновременных РЕАЛЬНЫХ `ask_my_files` через `/mcp`
  (песочный индекс + песочный `llm_host` + `hds mcp-http`, 5 токенов/пользователей), отчёты
  `tools/parity/out/m4_load_*.json`; флаги `--index-during`, `--no-rerank`.
  Числа: `parallel 5/ctx 16384` → 200×5, p50 25,7 с, **p95 49,6 с** (SLA ≤5 мин выполнен);
  `3` → p95 48,9 с; `2` → p95 58,1 с; индексация параллельно → 200×5, p95 315 с (за порогом),
  но «индексация уступает» (25 с → 333 с) и «database is locked» нет.
* **T6** — доклад: `STATUS.md`, журнал `PLAN_M4_CONCURRENCY_SLA.md` §8a, `README.md`
  (§«Слоты и SLA» + таблица приёмки), `README_SERVER_MULTIUSER.md` §8/§11, `PLAN_SERVER_MULTIUSER.md` §9,
  `M3_CODE_REVIEW.md` §7 (F10.3 закрыт).

## Что осталось / с чего продолжать

### 0) Сначала согласовать этап с владельцем (дорожная карта `PLAN_SERVER_MULTIUSER.md` §5)
M4 = Э4 закрыт. Дальше по порядку §5: **Э5** «GPU: per-role/instance, AMD+NVIDIA (+спайк),
Apple Silicon, VRAM по картам, пресеты (W4)», затем **Э6** «Внешняя чат-модель `provider: remote` (W9)»,
Э7 (управление службами/токенами из UI), Э8/Э9 (пакеты установки), Э10 (эксплуатация),
Э11 (нагрузочная приёмка на 1/2 картах + Apple Silicon).
При этом `M4_HANDOFF_PROMPT_5.md` и `PLAN_M4_CONCURRENCY_SLA.md` §3.5 уже называют **M5 = внешний
чат-provider (`chat.provider: remote`)**; а в §9 `PLAN_SERVER_MULTIUSER.md` из Э2 осталась
**отправка во внешний API (анонимизация — W9)**. Первый шаг — завести `PLAN_M5_<тема>.md` по образцу
`PLAN_M4_CONCURRENCY_SLA.md` (разделы: §1 вводные-факты, §2 границы, §3 контракты ключей,
§4 треки, §5 приёмка, §7 риски, §8a журнал) и получить у владельца выбор M5/Э5/сначала хвосты.

### 1) Явные хвосты M4 (не «потерять»)
* **T5: одиночный выброс p95 ~305–315 с.** В части прогонов `parallel 5` один из пяти запросов
  отвечает ~305–315 с, остальные ~27 с; исключён реранкер (`--no-rerank` → те же числа), при
  индексации параллельно — тоже. Признак в логе резидента: `[dispatcher] роль 'embedding':
  LOAD_ON_DEMAND` / `[busy] двигатель занят: load:embedding`. Гипотеза: on-demand загрузка и
  перепланирование роли `embedding` при 5 занятых слотах чата на 12 ГБ. Кандидаты:
  (а) `llm_server.embedding.retention: keep` / прогрев перед приёмом тяжёлых задач;
  (б) поднять приоритет прогрева или не отдавать слоты чата до готовности ролей поиска;
  (в) измерить с `HDS_CONFIG` боевого набора сервисов и без него (в «чистом» стенде p95 = 49,6 с).
  Якоря: `tools/parity/load5.py`, `tools/parity/out/m4_load_*.json`, `crates/hds-llama/src/host.rs`
  (арбитр/диспетчер), `crates/hds-llama/src/config.rs` (retention роли).
* **Чек-лист внешнего HTTPS** (перенос из M3 §5 / M4 §4 T5): `https://<домен>/mcp` из Cline
  (Authorization Bearer), `/ui` в браузере за прокси, XFF-IP в аудите, `429` (квоты) через прокси —
  **выполняется на машине с доменом/сертификатом** (на dev-стенде не воспроизводится).
  Якоря: `installers/proxy/{Caddyfile,nginx.conf}`, `trusted_proxy`, `hds check` (пункт «периметр»).
* **F9 (`Host` не читается)** — по ревью правильнее поправить формулировку
  `PLAN_M3_NETWORK_PERIMETER.md` §3.1; боевые PLAN-документы не переписываем → оставлено открытым.
* **T0.9 — вопросы заказчику (не код):** автотранскрибация в server-профиле
  (`installers/install_server.ps1:69` снимает задачу без замены) и `hds login --password` в argv.
  Якоря: `M3_CODE_REVIEW.md` §5 (M2-F7/M2-F9), §7.

### 2) Задел под M5 (внешняя чат-модель / внешний API)
* Контракт ключей уже определён — `PLAN_M4_CONCURRENCY_SLA.md` §3.5 (`chat.provider: local|remote`,
  `chat.remote.*`); при `remote` `llm-host` **не держит chat-роль в VRAM** — это и есть «рычаг SLA»
  на слабом железе (W3/W9, `PLAN_SERVER_MULTIUSER.md` §2 W9, §5 Э6).
* Связано с «анонимизацией» (§7 п.3 плана; W9) — без неё внешний сервис не считается допустимым.
* Проверять на рантайме: RAG работает без локальной chat-модели (chat-роль выгружена), SLA и
  `429/503`-поведение не меняются; тесты — контракт конфига + живой прогон (стенд T5 подходит:
  `load5.py` умеет подменять `chat.base_url`/`provider` в песочном `HDS_CONFIG`).

## Ключевые решения (не менять)
* SLA **≤5 мин** на 5 одновременных RAG; при нехватке VRAM — **явная ошибка с числами**, не деградация.
* Слоты чата: `llm_server.parallel` (server 5 / desktop 1); `ctx_per_slot` подбирается под RAG
  (замеры T2: 5×16384 либо 2–3×32768 на 12 ГБ).
* **T3/T4 (действующие контракты):** `rate_limit.max_inflight` — лимит **исполнения** (глобально,
  он же лимит слотов очереди); `rate_limit.max_inflight_per_user` — на пользователя (учитывается,
  только если **строже** глобального; отказ — немедленный `429` + hint); `queue.max_depth` — лимит
  **ожидания**; `queue.wait_timeout_sec` дефолт **240** (< клиентского 300, иначе `503` позже обрыва);
  **ключ лимитов/квот = `user:<login>` → `ip:<addr>`** (`limit_key`; `per_token: true` возвращает
  пер-токенный ключ, но частота дополнительно ограничивается `requests_per_min_per_user`);
  отказ по пер-пользовательскому пределу (`QueueReject::PerUser`) и `Full` → `429`, таймаут → `503`,
  оба с `Retry-After`;
  метрики очереди — **файл-снапшот** `<state_dir>/data/mcp-queue.json` (читают `hds check` и
  `/api/diagnostics`), коды/заголовки — не HTTP-эндпоинт; очередь живёт в `hds mcp-http`
  (админский `ask` UI — вне неё, документированное ограничение).
* Периметр — reverse-proxy с TLS; наружу только `/mcp` и `/ui`; UI — только админ.
* Идентификация — локальные токены; индексация — только админ; проект — единица выдачи (M1).
* Десктоп-профиль **не ломаем**: очередь/квоты выключены (лимитер `None`), `llm_server.parallel: 1`,
  `db.*` 30000/false, лимиты/аудит — как в M3.

## Грабли окружения (важно! — пополнено сессиями T4/T5)
* **`run_commands`: каждый элемент массива = ОТДЕЛЬНАЯ PowerShell-сессия** — переменные не живут
  между элементами; многошаговая проба — одним элементом через `;` или `.ps1`-файлом.
* **Пробы с кодом — файлом** (`editor`, ASCII-only для `.ps1`; для python — `.py` в
  `tools/parity/out/`, он гитигнорится), запуск
  `powershell -NoProfile -ExecutionPolicy Bypass -File <файл>` / `python <файл>`.
* **`.ps1` только ASCII:** PS 5.1 читает файл без BOM как ANSI — кириллица ломает парсер.
  Проверка разбора — через `[System.Management.Automation.Language.Parser]::ParseFile`.
* **PowerShell портит первый токен команды** и печатает кириллицу программ как cp866-мусор —
  числа читаемы, файлы читать read-инструментом (не `Select-String` по содержимому).
* **`HDS_ROOT` переопределяет `project_root()`** и ломает поиск воркера (`.venv`/`sidecar`) →
  для MCP/RAG/индексации **не ставить**. Нужен изолированный корень — делайте junction'ы
  `.venv`/`sidecar`/`hds` в песочный корень + кладите туда `config.yaml` (см.
  `tools/parity/load5.py::make_ixroot`) — так боевая `index.pause` не видна.
* **`index.pause`/`index.stop`/`index.heartbeat.json` читаются из `project_root()`, НЕ из
  `state_dir`** (`hds-index/src/pipeline.rs:622-625`): песочная индексация с корнем = репозиторий
  «зависает» на боевой паузе (лечится своим корнем, а не снятием паузы).
* **Пустой `projects` у не-админа = «не видит ничего»** (контракт M1) → в песочных токенах
  ставьте `admin: true` (`load5.py::make_tokens`).
* **Боевые индексы пусты** (0 files/chunks и в `D:\hermes-disk-search-db\index.db`, и в репо-`index.db`)
  → «настоящий RAG» на этой машине воспроизводится только на песочном индексе.
* **Живые сервисы держат `target\debug\hds.exe`** → `cargo test --workspace` падает **сборкой**;
  либо `--exclude hds-cli`, либо остановить сервисы и вернуть после.
* **Порты 8010–8012 — резидент**; для песочниц удобны 8797–8799 (`load5.py` сам требует свободный 8010).
* **`Stop-Process -Force` = TerminateProcess** → код очистки не выполняется (ждать «статус устарел» —
  норм).
* **env персистентны в сессии** — снимать (`Remove-Item Env:HDS_ROOT,Env:HDS_CONFIG`).
* **git — всегда `--no-pager`**; `config.yaml` в `.gitignore`; `tools/parity/out/` гитигнорится целиком.
* **`hds check --json`** → `{"checks":[{id,status,title,msg,fix}],"ok":bool}` — фильтровать
  `Where-Object { $_.id -eq 'queue' }`.
* **После server-профильных проб чистить `%ProgramData%\hermes-disk-search`** (там остаются
  `access.yaml` с тестовыми токенами, снапшот очереди, `data/logs/audit.jsonl`) — `load5.py`
  это делает сам, ручные пробы — вручную.
* **Урок сессий T1–T5:** в начале работы в дереве могут быть «неожиданные» незакоммиченные
  изменения — это работа предыдущей сессии того же этапа. Не откатывать: сверить дифф и доращивать.
  Один активный чат на этап.

### Остановить / вернуть боевые сервисы (проверено)
```powershell
# остановить (освободить target\debug\hds.exe для сборки и 8010-8012 для песочниц)
Get-CimInstance Win32_Process -Filter "Name='hds.exe'" | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
& .\target\release\llm_host.exe stop
# вернуть (порядок: резидент → watch → mcp-http → transcribe-watch)
Start-Process -FilePath '.\target\release\llm_host.exe' -ArgumentList 'run' -WindowStyle Hidden
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'watch' -WindowStyle Hidden
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'mcp','--http','--host','127.0.0.1','--port','8787','--path','/mcp' -WindowStyle Hidden
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'transcribe-watch' -WindowStyle Hidden
# проверить: status → parallel 1; Test-Path .\index.pause → True; http://127.0.0.1:8787/health → 200
```

### Перезапуск стенда T5 (по нужде)
```powershell
python tools\parity\load5.py --tag p5_16384  --parallel 5 --ctx 16384                 # базовый
python tools\parity\load5.py --tag p5_idx     --parallel 5 --ctx 16384 --index-during # индексация параллельно
python tools\parity\load5.py --tag p5_norerank --parallel 5 --ctx 16384 --no-rerank   # без реранкера
```
Требует свободных 8010–8012 (сначала `llm_host stop`, вернуть после). Пишет
`tools/parity/out/m4_load_<tag>.json` и сам удаляет созданный системный state-каталог.
Прогон ~5–6 мин → запускать `Start-Process ... -RedirectStandardOutput <лог>` и поллить лог.

## Правила
* Сначала план по файлам, при согласовании — код. После каждого шага — тесты + коммит.
* Не ломать десктоп-профиль; не трогать чужие незакоммиченные изменения и боевые `PLAN_*.md`
  (обновлять `STATUS.md`, `README*.md`, журнал плана §8a; статус в `PLAN_SERVER_MULTIUSER.md` §9 —
  по завершении этапа).
* Обязательно проверять изменения в рантайме (сеть/лимиты/очередь/БД/слоты/роли — на живом
  сценарии), а не только юнитами. GPU-пробы — по `cline-rules/dev-machine.md`: останавливать и
  **возвращать** резидент, освобождать занятые `.exe`, после работ не оставлять мусора.
* Держать хвосты: незакрытое переносить явно (раздел «Что осталось» выше), а не «терять».


