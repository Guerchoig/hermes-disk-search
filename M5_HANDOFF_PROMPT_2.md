# M5_HANDOFF_PROMPT_2.md — промт для нового чата (M5 почти закрыт: T0–T5)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, правила и грабли.
> Предыдущий промт — `M5_HANDOFF_PROMPT.md` (старт этапа M5; согласование с владельцем).
> Детальный план M5 с журналом исполнения — `PLAN_M5_EXTERNAL_CHAT.md`
> (§1 вводные-факты, §2 границы, §3 контракты, §4 треки, §5 приёмка, §7 риски,
> **§8a журнал — что именно сделано и чем проверено**).
> Код-ревью M4 (источник хвостов F1–F9) — `M4_CODE_REVIEW.md` (**§8 — отметки
> о закрытии F1–F9 в M5**); читать вместе с этим промтом.
> Контекст проекта и дорожная карта — `PLAN_SERVER_MULTIUSER.md` (§5 этапы Э1–Э11,
> §9 статусы), сценарии — `README_SERVER_MULTIUSER.md` (§8a — remote), сводка — `STATUS.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние: **M5 в работе, треки T0–T5 выполнены** (хвосты M4 F1–F9 закрыты +
внешний chat-provider `remote` реализован и принят на стенде), формальное закрытие
этапа (обновление §9 дорожной карты, возможный код-ревью M5) — за следующим чатом.
HEAD — коммиты M5 (см. ниже), дерево чистое. Станция: Windows, RTX 3060 12 ГБ
(+ AMD-карта), Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1. Ядро на Rust
(`crates/`), Python — только sidecar. `.venv` — для проб/sidecar.

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -12`   # HEAD — этот промт
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`     # чисто
3. `cargo test --workspace` — ⚠ **живые сервисы держат `target\debug\hds.exe`** (см. «Грабли»):
   либо остановить `hds watch` / `hds mcp-http --http ...` / `hds transcribe-watch`, либо
   `cargo test --workspace --exclude hds-cli`; после работ сервисы **вернуть**.
   Ожидание: 87 наборов тестов, 0 failed.
4. `cargo clippy --workspace --all-targets -- -D warnings` и `cargo fmt --all --check`
5. Прочитать: `PLAN_M5_EXTERNAL_CHAT.md` (целиком, короткий) → `STATUS.md` (блок M5,
   после блока код-ревью M4) → `M4_CODE_REVIEW.md` §8 (закрытие F1–F9) →
   `README.md` §«Нагрузочная приёмка» (root-cause/фикс SLA) и §«Внешний chat-provider» →
   `README_SERVER_MULTIUSER.md` §8/§8a → `config.example.yaml` (`chat.provider/api_key`,
   `llm:` комментарий про ретенцию, `rate_limit.max_inflight`).
6. Проверить факт боевого состояния (не менять!):
   `& .\target\release\llm_host.exe status` (в этой сессии: `parallel 1`, chat LOADED
   `retention=keep` n_ctx=65536, embedding/rerank `on_demand` — desktop-профиль),
   `Test-Path .\index.pause` (**боевая пауза — не снимать**, сейчас есть),
   `& .\target\debug\hds.exe check --json` — новый пункт `chat_provider: ok` (local).

## Коммиты этапа M5 (по порядку)
601618e (план M5) → c2dc2ea (F2 auth-до-очереди + T0.9/F9-тесты) → 47b79c1 (F3 дефолты
+ warn рассинхрона) → 6e9e1cf (F4/F5/F7/F8 полировка очереди/БД) → ee58b19 + 6fa51d6
(F6 мёртвый путь удалён) → **204a88b** (F1 root-cause+фикс; задел remote T1–T3:
`chat.{provider,api_key}`, TLS rustls, llm-host без chat-роли, пункт `chat_provider`)
→ ae0c664 (T4 стенд `--chat-remote fake` + приёмка remote) → 0b5aa48 (доки T5) —
последний перед этим промтом.


## Что закрыто (НЕ переделывать)
* **F1 (SLA ≤5 мин) — root-cause найден и устранён.** База до фикса: 3 прогона
  `p5_16384` → 315,6 / 315,7 / 46,5 с (выброс вероятностный, 2 из 3; отчёты
  `m4_load_f1_r1..r3.json`). По логам резидента (`<sand>/ixroot/data/logs/llm-host.log`):
  on-demand **перезагрузка роли `embedding`** при тесной VRAM; fit-шаг движка
  `llama_params_fit_impl` при «свободно ~1900 МиБ» завершается **~299 с**
  (рост ×~8 с каждой загрузкой в процессе: 0,57 → 4,2 → 27,3 → 299; знак «−» — баг
  печати движка); unlucky-запрос ждал перезагрузку целиком (315 ≈ 299+16), очередь
  `granted=5, total_wait_ms=0`. Фикс: **server-профиль — embedding/rerank
  `retention: keep` по умолчанию** (явный `llm.<role>.retention` сильнее; desktop —
  `load_on_demand` как раньше) + **прогрев embedding/rerank при старте llm-host**
  (`host.rs`, после загрузки чата; ошибка прогрева — НЕ молчаливая). Приёмка:
  **3/3 прогона в SLA — 34,1 / 34,0 / 33,9 с** (`m4_load_f1_fix_r1..r3.json`).
* **F2 (DoS):** при `access.enabled` Bearer проверяется **до** `queue.acquire` —
  неавторизованный тяжёлый запрос получает мгновенный `401`, место в очереди
  не занимает (аудит `unauthorized` сохранён). Сокет-тест
  `unauthorized_heavy_rejected_before_queue` (`quota_socket.rs`).
* **F3:** дефолт `rate_limit.max_inflight` по профилю — **server 5** (= дефолт
  `llm_server.parallel`), desktop 4; `hds check` (пункт `queue`) warn'ит рассинхрон
  `parallel` ↔ `max_inflight` с числами.
* **F4–F8:** FIFO внутри пользователя по `seq` (`ahead_of_me` в `acquire`);
  `cv.notify_all()` при постановке; `Retry-After` таймаута = `wait_timeout/2`
  (min 5); allowlist `journal_mode` {delete,truncate,persist,memory,wal,off} +
  фолбэк `wal`; мёртвый путь `Limiter::inflight_*`/`InflightGuard` удалён
  (тесты переведены на `user_inflight_*`).
* **F9 (покрытие):** сокет-тест `aborted_client_releases_queue_slot` (обрыв
  клиента в ожидании → `aborted`, место освобождается) + сценарий F2.
* **T1–T3 (remote):** контракт §3.5 плана M4 реализован дословно —
  `chat.provider: local|remote` (дефолт `local`), `chat.api_key`
  (`hds_core::config::{chat_provider,chat_api_key}`); RAG (`hds-search/src/rag.rs`)
  ходит в `chat.base_url` через `hds-core::http::request_url` — **TLS-клиент
  rustls** (корни webpki, `parse_url` с дефолт-портами 80/443 и IPv6) +
  `Authorization: Bearer` при непустом ключе (`-` = без ключа); llm-host при
  `remote` **не планирует chat-роль** (`config.rs::build` — предупреждение в
  `warnings`); `hds check` — пункт `chat_provider` (remote без `base_url`/
  `api_key` → warn; секрет не печатается; напоминание про анонимизацию W9).
* **T4 (стенд):** `load5.py --chat-remote fake|<url>` (+`--remote-port` 8799,
  `--remote-latency`) и встроенный фейковый OpenAI-сервер; при remote ожидание
  готовности — по embedding-фасаду (`:8011`). Приёмка `m4_load_m5_remote_r1.json`:
  5 × `ask_my_files` → **200×5, p95 6,8 с** (fake 3 с), `requests=5, auth_ok=5`,
  **VRAM 1102 → 1121 МиБ** (было 8916 → 9643 — chat-роль освободила ~7,8 ГиБ),
  ответы с источниками; очередь/квоты не изменились.
* **T5 (доки):** `STATUS.md` (блок M5), `README.md` (root-cause/фикс SLA + раздел
  remote), `README_SERVER_MULTIUSER.md` §8/§8a, `M4_CODE_REVIEW.md` §8 (отметки
  закрытия F1–F9), журнал `PLAN_M5_EXTERNAL_CHAT.md` §8a, `config.example.yaml`.


## Что осталось / с чего продолжать

### 0) Сначала согласовать с владельцем
1. **Формальное закрытие M5:** обновить `PLAN_SERVER_MULTIUSER.md` §9 (статус M5 —
   сейчас этап НЕ отмечен там; правило «статус в §9 — по завершении этапа»),
   сверить приёмку §5 плана M5 с фактом (всё зелёное), решить — нужно ли
   **код-ревью M5** (по образцу `M4_CODE_REVIEW.md`: независимые рантайм-проверки,
   пересчёт отчётов стенда; обратить внимание на rustls-зависимость и пункт
   `chat_provider`). Если ревью найдёт хвосты — завести T0-трек следующего этапа.
2. **Выбор следующего этапа** (дорожная карта `PLAN_SERVER_MULTIUSER.md` §5):
   по порядку идёт **Э5** «GPU: per-role/instance, AMD+NVIDIA (+спайк), Apple Silicon,
   VRAM по картам, пресеты (W4)» (6–10 дн). Альтернативы: **анонимизация (W9)** —
   без неё `chat.provider: remote` допустим только для доверенных API (хвост Э2/Э6);
   либо хвост «рост fit-шага движка» (`engine-patch` P5, см. ниже).

### 1) Явные хвосты (не «потерять»)
* **Движок: рост fit-шага от числа загрузок в процессе.** `llama_params_fit_impl`
  за процесс: 0,57 с → 4,2 → 27,3 → **299 с** (замеры f1_r1/r2, «−» — баг печати).
  Фикс M5 обошёл его ретенцией/прогревом, но любая будущая выгрузка/загрузка
  (например ARB-3 при индексации, `llm.model_policy: auto`, смена модели) снова
  может дать минутные паузы. Кандидат: патч движка P5 (`engine-patch/README.md`
  — там уже P1–P4). Якоря: `tools/parity/out/f1_r{1,2}_loads.log`, `f1_r1_ts.log`.
* **Анонимизация (W9)** — отдельный этап/трек: без неё наружу уходят вопрос +
  фрагменты локальных файлов как есть (README/`hds check` предупреждают).
  Контекст: `PLAN_SERVER_MULTIUSER.md` §7 п.3, §9 «осталось из Э2».
* **`chat.provider: remote` на реальном доверенном API** — на стенде принят с
  фейковым сервером; живой прогон с настоящим API (например OpenRouter) — по
  желанию владельца, ключ в `config.yaml` (не коммитить).
* **Чек-лист внешнего HTTPS** (перенос из M3/M4): `https://<домен>/mcp` из Cline
  (Bearer), `/ui` за прокси, XFF-IP в аудите, `429` через прокси — на машине
  с доменом/сертификатом. Якоря: `installers/proxy/{Caddyfile,nginx.conf}`,
  `trusted_proxy`, `hds check` (пункт «периметр»).
* **F9 (M3, `Host` не читается)** — по ревью правильнее поправить формулировку

## Ключевые решения (не менять)
* SLA **≤5 мин** на 5 одновременных RAG; при нехватке VRAM — **явная ошибка с числами**,
  не деградация. Приёмка SLA: **≥3 прогона одного конфига; при n=5 `p95 = max`**;
  критерий — все прогоны max ≤ 300 с (числа в README/§8a).
* **Ретенция (M5, F1):** server — embedding/rerank `keep` по умолчанию + прогрев при
  старте llm-host; desktop — `load_on_demand` (экономия VRAM, поведение прежнее).
  Явный `llm.<role>.retention` сильнее дефолта профиля.
* Слоты чата: `llm_server.parallel` (server 5 / desktop 1); `ctx_per_slot` под RAG
  (замеры: 5×16384 либо 2–3×32768 на 12 ГБ); fit-гейт VRAM с числами.
* **T3/T4 (действующие контракты M4):** `rate_limit.max_inflight` — лимит
  **исполнения** (глобально, лимит слотов очереди); дефолт **server 5** (=`parallel`),
  desktop 4; `max_inflight_per_user` — на пользователя (учитывается, только если
  строже глобального; отказ — немедленный `429` + hint); `queue.max_depth` — лимит
  ожидания; `queue.wait_timeout_sec` дефолт 240 (< клиентского 300); ключ
  лимитов/квот = `user:<login>` → `ip:<addr>`; `Full`/`PerUser` → `429`, таймаут →
  `503`, оба с `Retry-After`; метрики — файл-снапшот `<state_dir>/data/mcp-queue.json`;
  очередь живёт в `hds mcp-http`; auth — **до** очереди (M5 F2).
* **Remote-контракт (M5, §3.5 плана M4 — не менять):** `chat.provider: local|remote`;
  при `remote` RAG ходит в `chat.base_url` с `Authorization: Bearer <chat.api_key>`
  (`-` = без ключа); llm-host **не держит chat-роль в VRAM**; embedding/rerank
  локальные; очередь/квоты MCP без изменений; TLS — rustls, только исходящие
  (входящие — без TLS, периметр = reverse-proxy M3). Анонимизация (W9) — вне M5.
* Периметр — reverse-proxy с TLS; наружу только `/mcp` и `/ui`; UI — только админ.
* Идентификация — локальные токены; индексация — только админ; проект — единица
  выдачи (M1). Отправка во внешний API — допустима (решение заказчика; анонимизация —
  следующий шаг).
* Десктоп-профиль **не ломаем**: очередь/квоты выключены (лимитер `None`),
  `llm_server.parallel: 1`, `db.*` 30000/false, retention embedding/rerank —
  `load_on_demand`, лимиты/аудит — как в M3. Боевой конфиг этой машины — desktop.

## Грабли окружения (важно! — пополнено сессиями M5)
* **`run_commands`: каждый элемент массива = ОТДЕЛЬНАЯ PowerShell-сессия** — переменные
  не живут между элементами; многошаговая проба — одним элементом через `;` или
  `.ps1`-файлом.
* **Пробы с кодом — файлом** (`editor`, ASCII-only для `.ps1`; для python — `.py`
  в `tools/parity/out/`, он гитигнорится), запуск
  `powershell -NoProfile -ExecutionPolicy Bypass -File <файл>` / `python <файл>`.
* **`.ps1` только ASCII:** PS 5.1 читает файл без BOM как ANSI — кириллица ломает
  парсер. Проверка разбора — `[System.Management.Automation.Language.Parser]::ParseFile`.
* **PowerShell портит кириллицу программ (cp866-мусор) и грызёт первый токен** —
  числа читаемы, а логи/файлы с кириллицей фильтровать **питоном** в файл в
  `tools/parity/out/` и читать read-инструментом (не `Select-String`).
* **`HDS_ROOT` переопределяет `project_root()`** и ломает поиск воркера
  (`.venv`/`sidecar`) → для MCP/RAG/индексации **не ставить**. Изолированный корень —
  junction'ы `.venv`/`sidecar`/`hds` + свой `config.yaml` (см.
  `tools/parity/load5.py::make_ixroot`); `index.pause`/`index.stop`/
  `index.heartbeat.json` читаются из `project_root()` — песочной индексации нужен
  СВОЙ корень, иначе «зависнет» на боевой паузе.
* **Пустой `projects` у не-админа = «не видит ничего»** (контракт M1) → в песочных

### Остановить / вернуть боевые сервисы (проверено)
```powershell
# остановить (освободить target\debug\hds.exe для сборки и 8010-8012 для песочниц)
Get-CimInstance Win32_Process -Filter "Name='hds.exe'" | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
& .\target\release\llm_host.exe stop
# вернуть (порядок: резидент → watch → mcp-http → transcribe-watch)
Start-Process -FilePath '.\target\release\llm_host.exe' -ArgumentList 'run' -WindowStyle Hidden
Start-Sleep 20
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'watch' -WindowStyle Hidden
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'mcp','--http','--host','127.0.0.1','--port','8787','--path','/mcp' -WindowStyle Hidden
Start-Process -FilePath '.\target\debug\hds.exe' -ArgumentList 'transcribe-watch' -WindowStyle Hidden
# проверить: status → parallel 1; Test-Path .\index.pause → True; http://127.0.0.1:8787/health → 200
```

### Перезапуск стенда (по нужде)
```powershell
python tools\parity\load5.py --tag p5_16384   --parallel 5 --ctx 16384                 # базовый (SLA-приёмка)
python tools\parity\load5.py --tag p5_idx     --parallel 5 --ctx 16384 --index-during # индексация параллельно
python tools\parity\load5.py --tag r1         --parallel 5 --ctx 16384 --chat-remote fake --remote-latency 3  # remote
```
* Требует свободных 8010–8012 (сначала `llm_host stop`, вернуть после) и не занятого
  8797/8799. Пишет `tools/parity/out/m4_load_<tag>.json`, сам удаляет системный
  state-каталог, если создал его.
* Прогон ~1–6 мин (с фиксом F1: базовый ~34 с RAG + ~1,5 мин песочный индекс и старты)
  → фоном: `Start-Process python -ArgumentList ... -RedirectStandardOutput <лог>` и
  поллить лог (`STEP WROTE ...`, `statuses=...`).
* Отчёт: `latencies_ms`/`p50`/`p95`/`sla_ok`; при remote — блок `remote`
  (`requests`/`auth_ok`) и `vram_before/after` (~1100 МиБ = chat не в VRAM).
* Фильтрация лога резидента песочницы — питоном в файл (cp866-мусор в консоли):
  сигнатуры `fitting params to free memory took`, `loading model`, `прогрев роли`.

## Правила
* Сначала план по файлам, при согласовании — код. После каждого шага — тесты + коммит.
* Не ломать десктоп-профиль; не трогать чужие незакоммиченные изменения и боевые
  `PLAN_*.md` (обновлять `STATUS.md`, `README*.md`, журнал плана §8a; статус в
  `PLAN_SERVER_MULTIUSER.md` §9 — по завершении этапа).
* Обязательно проверять изменения в рантайме (сеть/лимиты/очередь/БД/слоты/роли/
  remote — на живом сценарии), а не только юнитами. GPU-пробы — по
  `cline-rules/dev-machine.md`: останавливать и **возвращать** резидент, освобождать
  занятые `.exe`, после работ не оставлять мусора (артефакты — в `tools/parity/out/`).
* Держать хвосты: незакрытое переносить явно (раздел «Что осталось»), а не «терять».

  токенах `admin: true` (`load5.py::make_tokens`); **непустой список проектов у
  пользователя делает search «настоящим»** (трогает БД) — в сокет-тестах это рычаг
  детерминизма (тест F2 в `quota_socket.rs`).
* **Боевые индексы пусты** (0 files/chunks и в `D:\hermes-disk-search-db\index.db`, и в
  репо-`index.db`) → «настоящий RAG» воспроизводится только на песочном индексе.
* **Живые сервисы держат `target\debug\hds.exe`** → `cargo test --workspace` падает
  сборкой; либо `--exclude hds-cli`, либо остановить сервисы и вернуть. При работающем
  стенде (`load5.py`) НЕ пересобирать `target\debug\{hds,llm_host}.exe` и НЕ запускать
  `cargo test -p hds-llama` (собирает бины) — только `--lib`/`cargo check`.
* **Порты 8010–8012 — резидент**; песочницы: 8797 (mcp), 8799 (fake remote);
  `load5.py` требует свободный 8010.
* **`Stop-Process -Force` = TerminateProcess** → код очистки не выполняется (ждать
  «статус устарел» — норм).
* **env персистентны в сессии** — снимать (`Remove-Item Env:HDS_ROOT,Env:HDS_CONFIG`).
* **git — всегда `--no-pager`**; `config.yaml` в `.gitignore`; `tools/parity/out/`
  гитигнорится целиком; **спокойно редактировать `load5.py` можно ТОЛЬКО когда ни
  один прогон не запущен** (python читает файл при старте каждого прогона).
* **`hds check --json`** → `{"checks":[{id,status,title,msg,fix}],"ok":bool}` —
  фильтровать `Where-Object { $_.id -eq 'queue' }` (новый id: `chat_provider`).
* **После server-профильных проб чистить `%ProgramData%\hermes-disk-search`**
  (`access.yaml` с тестовыми токенами, снапшот очереди, `data/logs/audit.jsonl`) —
  `load5.py` это делает сам, ручные пробы — вручную.
* **Урок сессий T1–T5/M5:** в начале работы в дереве могут быть «неожиданные»
  незакоммиченные изменения — это работа предыдущей сессии того же этапа. Не
  откатывать: сверить дифф и доращивать. Один активный чат на этап.

  `PLAN_M3_NETWORK_PERIMETER.md` §3.1; боевые PLAN-документы не переписываем →
  оставлено открытым.
* **T0.9 — вопросы заказчику (не код):** автотранскрибация в server-профиле
  (`installers/install_server.ps1:69` снимает задачу без замены) и
  `hds login --password` в argv. Якоря: `M3_CODE_REVIEW.md` §5 (M2-F7/M2-F9), §7.

### 2) Задел под Э5 (GPU per-role/multi-vendor) — читать заранее
* Контракт раскладки ролей по картам уже в конфиг-примере (`gpu.devices`:
  `{ index, roles }` — `PLAN_SERVER_MULTIUSER.md` §8.1); сейчас фактический выбор —
  один `gpu.device_index` (`crates/hds-llama/src/{device.rs,registry.rs}`).
* Спайк CUDA+Vulkan ранее откладывался (риск одновременных бэкендов в одном
  `llm-host` — §6 рисков). Пробы: `tools/parity/spike6_*.py`, `a1_device_probe`.
* С `remote`-чатом VRAM freed ~8 ГиБ — раскладка embedding/rerank на AMD (Vulkan)
  стала реальнее; whisper — отдельный путь (`whisper.rs`, bridge).
