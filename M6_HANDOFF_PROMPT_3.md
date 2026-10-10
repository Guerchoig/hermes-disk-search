# M6_HANDOFF_PROMPT_3.md — промт для нового чата (этап Э5, продолжение: T0 и T1 закрыты, следующий шаг T2)

> ⚠️ **Устарел (10.10.2026).** T2 и T3 выполнены — используйте **`M6_HANDOFF_PROMPT_4.md`**
> (T0/T1/T2/T3 закрыты с числами, следующий шаг T4). Ниже — состояние на момент «T1 закрыт».

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, факты T0/T1 с числами, правила, грабли окружения и хвосты.
>
> **Заменяет `M6_HANDOFF_PROMPT_2.md`** — там описан этап в точке «T0 в работе». Сейчас:
> **T0 закрыт (GO, `d635d03`), T1 выполнен (`a919a78`)**; Vulkan-бэкенд собран и лежит
> в песочнице; следующий шаг — **T2** (чат-реплики, whisper по картам, один фасад +
> внутренняя маршрутизация, §3.6).
> План этапа — `PLAN_M6_GPU_MULTIVENDOR.md` (**ревизия 2**, журнал §8a заполнен по T0/T1);
> сводка — `STATUS.md` (блок M6); дорожная карта — `PLAN_SERVER_MULTIUSER.md` §2 **W4** /
> §5 **Э5**; спайки — `tools/parity/SPIKES.md` (§15 — отчёт T0).

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние: **M1–M5 закрыты**; этап **M6/Э5**:
* требования владельца пересмотрены (**ревизия 2**, коммит `c53e1d0`); смешанные вендоры
  и iGPU/UMA — вне поддержки; прод = N дискретных карт одного семейства (NVIDIA или AMD
  под Windows) либо один Mac/Metal; поставка отдельными сборками;
* **T0 закрыт — GO** (`d635d03`): Vulkan-бэкенд нашего дерева собран, роли AMD-инсталляции
  реально идут на Vulkan-устройства (свидетельство на iGPU, числа ниже);
* **T1 выполнен** (`a919a78`): конфиг-контракт раскладки `gpu.devices` +
  `llm_server.<role>.device`, fail-fast валидация §3.5, `backend=` в плане/статусе,
  пункт `gpu_roles` в `hds check`, образец в `config.example.yaml`;
* **T2–T6 не начаты**; ближайший шаг — T2 (§3.6).

Станция: Windows, RTX 3060 12 ГБ (единственный дискретный GPU), AMD Radeon Graphics
(**iGPU, UMA**, в прод-область не входит), Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1.
Ядро на Rust (`crates/`), Python — только sidecar.

## Требования (ревизия 2 — решения владельца 10.10.2026; без изменений)

1. Прод — несколько **дискретных** GPU **одного семейства**: NVIDIA под Windows **или**
   AMD под Windows; либо **один Mac с Metal**. Смешанные вендоры и iGPU/UMA — **не
   поддерживаются** (`hds check` → warn, пункт `gpu_roles` уже делает).
2. Поставка — **отдельными сборками**: NVIDIA / AMD / Metal.
3. Несколько карт — обслуживать **разных пользователей одновременно**. **Чат-реплики
   одинаковые** (одна модель/контекст; различаются портом и слотами); допускаются на
   одной карте («две чат-модели на карте 24 ГБ») и на разных.
4. **Индексация (`embedding` + `rerank`) — ровно один экземпляр на машину**; «один набор
   моделей индексации обрабатывает один файл одновременно» (`n_parallel = 1`).
5. **Транскрибация (`whisper`) — экземпляр на карту**; **один файл — одна карта** (split
   отвергнут); карту под файл выбирает **`auto_transcribe`** по свободной VRAM; разные
   файлы — параллельно на разных картах.
6. **Маршрутизация** — **один фасад** `llm-host` (`chat` → 8010), распределение по
   репликам **внутри хоста** (least-busy, при равенстве round-robin); порты реплик
   наружу не публикуются (авторизация/аудит/rate-limit/очередь остаются в одной точке —
   M2/M3/M4).
7. Индексация — «лёгкая добавка» рядом с чат-репликой; при нехватке VRAM **пауза**
   (штатный `index.pause`), **чат-реплики не вытесняются** (приоритет чата в `gpu.policy`).
8. Железа для нескольких карт нет → мультикарточность принимается юнит-тестами +
   явным хвостом; дискретной AMD нет (только iGPU) → AMD-путь частично (свидетельство
   с iGPU получено в T0); Apple Silicon — подготовка без живой проверки.
9. Стенд: владельца интересует прежде всего **транскрибация** — приёмка T5 вокруг неё.

Детали и точные формулировки: `PLAN_M6_GPU_MULTIVENDOR.md` §2, §3.1–§3.6, §4 (T0–T6),
§5 (приёмка), §7 (риски), §8a (журнал).

## Точка входа (5 минут)

1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -5` (ожидание: HEAD
   `a919a78` — T1, `d635d03` — T0 GO, `2e0f193` — промт 2, `39980b7` — проба a7, `c53e1d0` — ревизия 2 плана)
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short` — **чисто**
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe`: либо
   остановить `hds watch`/`hds mcp-http`/`hds transcribe-watch`, либо
   `--exclude hds-cli`; после работ сервисы **вернуть**. Ожидание: зелёный
   (hds-llama lib 50 passed + ignored 2, `registry_plan` 10, hds-index lib 25 и пр.).
   `target\release\hds.exe` свободен — пересобирается при живых сервисах (проверено).
4. `cargo clippy --workspace --all-targets -- -D warnings` — ошибки «компиляция hds-cli
   debug» из-за лока лечатся `--exclude hds-cli`; `cargo fmt --all --check`
5. Прочитать: `PLAN_M6_GPU_MULTIVENDOR.md` §3.6 + §4 T2 + §5 + §7 + §8a → `STATUS.md`
   (блок M6) → `crates/hds-llama/src/{config.rs,registry.rs,device.rs}` (контракт
   раскладки T1) → `crates/hds-llama/src/host.rs` (бюджет/диспетчер `bind_device_probes`,
   `/internal/*`) → `crates/hds-llama/src/facade.rs` (маршрутизация, `Route`)
6. Проверить боевое состояние (не менять!):
   `& .\target\release\llm_host.exe status` → desktop-профиль: `parallel 1`, chat
   `LOADED` `retention=keep` n_ctx=65536, embedding/rerank `on_demand`, устройства
   `CUDA0`+`CPU` (**без Vulkan — боевой каталог не тронут**);
   `Test-Path .\index.pause` (**боевая пауза — не снимать**, сейчас есть);
   `& .\target\release\hds.exe check --json` → `gpu_roles` = ok «раскладка не задана»,
   `ok: true`.

## Факты T0 (проверено, числа; коммит `d635d03`, отчёт — `tools/parity/SPIKES.md` §15)

* **Vulkan-бэкенд собран из нашего дерева** (`C:\Users\Sasha\ENGINEbuilds`) и лежит в
  песочнице. Скрипт `tools\parity\out\build_ggml_vulkan.cmd` (vcvarsall x64 + Ninja
  Multi-Config из VS18 + `VULKAN_SDK=C:\Users\Sasha\VulkanSDK\1.4.335.0`); к конфигу
  добавлены `-DLLAMA_BUILD_SERVER=OFF -DLLAMA_BUILD_TOOLS=OFF -DLLAMA_BUILD_COMMON=OFF
  -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF` (без первых трёх configure падал:
  «LLAMA_HTTPLIB is OFF, cannot build llama-server»).
* Артефакт: `…\ENGINEbuilds\build-ggml-vulkan\bin\Release\ggml-vulkan.dll` (56 001 024
  байта); зависимости: `ggml-base.dll` + `vulkan-1.dll` (System32) + MSVC runtime.
  Совместимость ABI: 30 импортов `ggml_*` ⊆ экспортов нашей `ggml-base.dll` — **0 missing**
  (`m6_vk_imports.txt` / `m6_base_exports.txt`).
* Чужие сборки **непригодны** (проверено ранее): b11528 — `api_version=3` vs наш `2`
  (молча отклонена, `silent=true` в release); b11200 (`api_version=2`, 0 missing) —
  крашит процесс: апстрим изменил layout `ggml_backend_buffer_i` без бампа
  `GGML_BACKEND_API_VERSION` (падение в `ggml-base.dll+0x17a79`). Она сохранена в
  песочнице как `ggml-vulkan.dll.foreign_b11200`.
* **Боевой каталог движка `%APPDATA%\OpenResearchTools\TranscribeOffline\Engine` не
  тронут** — резидент видит только CUDA0+CPU (как и должно).
* С нашей Vulkan-DLL (песочница) движок перечисляет **4 устройства**:
  `0 CUDA0 (RTX 3060)`, `1 Vulkan0 = AMD Radeon(TM) Graphics` (**iGPU, UMA**,
  free 31087 / total 32723 МиБ), `2 Vulkan1 = RTX 3060 через Vulkan`, `3 CPU`
  → **ловушка компата подтверждена**: `gpu.device_index=2` теперь указывает на AMD.
  Пин по `backend+name` — нужен при-T2-пригодности; в проде раскладка задаётся явно.
* `a7_multiinstance_probe --no-chat` (роль embedding на Vulkan0 = сценарий AMD-сборки):
  **2/2 раунда без ошибок** — load bge-m3 (n_ctx 4096) 3 120 / 2 075 мс, embeddings
  183–243 мс, unload 289–317 мс; NVML RTX 3060 не тронута весь прогон (UMA); свободную
  память AMD мерить движком/DXGI, **не NVML**.
  ⚠ Фикс пробы: в `embed_spec` добавлено `embedding = Some(true)` — без него инстанс
  поднимался без `--embeddings` и все запросы давали 501 «This server does not support
  embeddings».
* Диагностические артефакты (гитигнор): `tools/parity/out/{m6_spike_vulkan_embed.json,
  m6_spike_vulkan_log.txt, m6_a1_vulkan.json, m6_a1_vulkan_log.txt, build_vulkan_log.txt,
  m6_vk_imports.txt, m6_base_exports.txt, m6_spike_abi.py, m6_check_candidate.py, …}`.

## Факты T1 (коммит `a919a78`)

* **Контракт раскладки**: `gpu.devices: [{ index, roles }]` — индексы **наши** (`0` = CPU,
  `1..N` = ускорители в порядке движка, как у `gpu.device_index`); приоритет выбора
  карты: `llm.<role>.device` → `llm_server.<role>.device` → `gpu.devices[].roles` →
  компат `gpu.device_index`. `embedding`/`rerank` легально делят карту («лёгкая
  добавка»); кривые записи/неизвестные роли — warn с номером записи (не молча).
* **Валидация §3.5 — fail-fast с числами** (`registry::plan_role`): «одна роль — ровно
  одна карта» (двойное назначение → `RolePlan::Failed` с перечнем источников);
  недоступная карта → «роль 'X': gpu.device_index=N: ускорителей найдено M».
* В плане/статусе — **backend карты**: `PlannedInstance.device_backend`, `summary`
  печатает `backend=`, notes «явная карта N из раскладки (§3.1)».
* **Пункт `gpu_roles` в `hds check`** (hds-index, без зависимости от hds-llama): без
  раскладки — ok «все роли на gpu.device_index» (HTTP не дёргается); с раскладкой —
  живые устройства фасада `GET /internal/devices` (в ответ добавлено поле
  `description`), warn: недоступная карта, двойное назначение, **смешанные вендоры**
  (эвристика по имени GPU: nvidia/amd/intel/apple), **iGPU/UMA** (`Radeon(TM) Graphics`,
  `UHD/Iris`).
* **Рантайм-проверка**: `llm_host_plan --config <песочная раскладка>` — chat →
  `devices=0 backend=CUDA` + note «явная карта 1»; embedding → fail-fast «ускорителей
  найдено 1 (CUDA0)» (боевой движок без Vulkan); `hds check --json` живьём —
  `gpu_roles` ok, `ok=true`, десктоп-профиль не изменён. Артефакты:
  `tools/parity/out/{m6_t1_layout.yaml, m6_t1_plan.json, m6_t1_plan_log.txt,
  m6_t1_check.json}`.
* Тесты: `config.rs` +2, `crates/hds-llama/tests/registry_plan.rs` +4 (фикстура
  `fake_devices_with_vulkan`: CUDA0/Vulkan0/Vulkan1/CPU), `diag.rs` +1 — зелёные.

## Инструменты (готовые)

* **`a7_multiinstance_probe`**: `cargo build -p hds-llama --release --bin
  a7_multiinstance_probe` → `target\release\a7_multiinstance_probe.exe [--engine-dir DIR]
  [--model-chat GGUF] [--model-embed GGUF] [--n-ctx-chat N] [--n-ctx-embed N]
  [--rounds N] [--n-predict N] [--json FILE] [--nvml-index N] [--no-chat] [--no-embed]`.
  ⚠ `--engine-dir` обязан быть **абсолютным** (грабля AddDllDirectory).
* **`a1_device_probe --engine-dir <абс.>`** — перечисление устройств + VRAM-пробы
  (перезаписывает `tools/parity/out/w2_a1_device.json` — отслеживаемый артефакт W2!:
  копировать результат в `m6_a1_vulkan.json` и делать `git restore` файла W2).
* **Песочная копия движка**: `tools\parity\out\engine-vulkan` — наши патченые DLL +
  наша `ggml-vulkan.dll` (56 МБ); чужая сохранена как `*.foreign_b11200`. Пересборка
  бэкенда — `tools\parity\out\build_ggml_vulkan.cmd` (полный цикл ~15 мин, шейдеры glslc).
* **`llm_host_plan --config FILE [--engine-dir DIR] [--json FILE]`** — сухой план
  (конфиг → модель → устройство → backend=), идеален для рантайм-проверки раскладки
  без VRAM.
* `llm_host devices|status|load|unload|stop`; `hds check --json`; `HDS_CONFIG` — путь
  конфига для песочниц (снимать `Remove-Item Env:HDS_CONFIG,Env:HDS_ROOT`).
* `tools/parity/load5.py` — стенд (SLA), требует свободные порты 8010–8012.

## Ближайшие шаги

1. **T2 (первый шаг нового чата):** реализация §3.6 в `host.rs`/`facade.rs`/`registry.rs`:
   * **чат-реплики**: одинаковые (модель/контекст одни; различаются именем, портом —
     внутренним, и слотами); допускаются на одной карте и на разных; конфиг-эскиз — §3.6
     плана (сверяться с ним!); валидация: уникальные имена/порты реплик, whisper — не
     более одного инстанса на карту, индексация — ровно один экземпляр на машину
     (правила дополнить в том же стиле, что T1 — fail-fast с числами);
   * **whisper по картам**: экземпляр на карту; **один файл — одна карта**; карту под файл
     выбирает `auto_transcribe` по свободной VRAM; разные файлы — параллельно на разных
     картах (`/internal/transcribe` — точка входа; учитывать UMA: free у AMD мерить
     движком/DXGI, не NVML);
   * **один фасад + внутренняя маршрутизация**: `chat` → 8010 остаётся единственной
     публикуемой точкой; внутри хоста — least-busy, при равенстве round-robin;
     порты реплик наружу не публикуются; авторизация/аудит/rate-limit/очередь — в одной
     точке (M2/M3/M4);
   * статус/сердцебиение: роль → карта/backend уже печатается (`backend=`), дополнить
     репликами.
2. **T3:** бюджет/вытеснение по картам с учётом нескольких ролей и реплик на одной карте
   (`bind_device_probes` уже вендор-нейтральный и по-картам; бюджет сейчас считается по
   одному целевому устройству — расширить).
3. **T4:** вендор-гомогенные пресеты (NVIDIA/AMD/Metal) + подсказки/валидация;
   шаг «Vulkan SDK» в `engine-patch/README.md`, если Vulkan идёт в поставку.
4. **T5:** приёмка (приоритет — **транскрибация**: живой файл, ≥3 прогона, карта/
   латентность/пик VRAM); десктоп не изменён. Стенд `load5.py` — по необходимости (RAG-SLA).
5. **T6:** доклад (README/`config.example.yaml`/§9 дорожной карты) — частично уже сделано
   в T1 (образец раскладки); дополнить репликами/whisper по картам.
6. Не забыть: **старые промты не использовать** (`M6_HANDOFF_PROMPT.md` — снятый сценарий;
   `M6_HANDOFF_PROMPT_2.md` — устарел, T0 в работе).

## Правила

* Сначала план по файлам, при согласовании — код. После каждого шага — тесты + коммит
  (кириллицу в сообщениях коммитов писать **файлом** UTF-8 и `git commit -F file`).
* Не ломать десктоп-профиль (боевой конфиг — desktop, `index.pause` не снимать).
* Боевые `PLAN_*.md` не переписывать без разрешения владельца (для ревизии 2 разрешение
  получено); `STATUS.md` и журнал §8a обновлять всегда.
* Рантайм-проверка обязательна (карты/бэкенды/VRAM/слоты — на живом сценарии), не только
  юниты; сухой прогон — `llm_host_plan`.
* GPU-пробы — по `cline-rules/dev-machine.md`: останавливать и **возвращать** резидент,
  освобождать занятые `.exe`, артефакты — в `tools/parity/out/` (гитигнор).
* Хвосты переносить явно, а не «терять». Один активный чат на этап.

## Грабли окружения (важно! — проверено сессиями M4/M5/M6)

* **`run_commands`: каждый элемент массива = ОТДЕЛЬНАЯ PowerShell-сессия** — переменные
  не живут между элементами; многошаговая проба — одним элементом через `;` или
  `.ps1`/`.cmd`. Долгие команды — `Start-Process … -RedirectStandardOutput <лог>` фоном
  и поллинг лога.
* PowerShell иногда «съедает» первый токен команды — повторять; для диагностики
  начинать с `Write-Output 'x';`.
* `*>`/`Out-File` пишут UTF-16/UTF-8-BOM (читать питоном с `encoding='utf-16'` /
  `utf-8-sig`); кириллица в консоли портится (cp866) — логи фильтровать **питоном**
  в файл и читать read-инструментом (не `Select-String`).
* **`AddDllDirectory`/`SetDllDirectoryW` требуют абсолютного пути** (иначе ошибка 87) —
  `--engine-dir` и пути к пробам всегда абсолютные.
* В **release**-сборке движка `ggml_backend_load_all` работает с `silent = true` —
  «Vulkan не видно» без единого сообщения; проверять ctypes-пробой (`m6_probe_procs.py`).
* `Set-Content` пишет ANSI (не UTF-16!) — читать такие файлы `cp1251`, а не `utf-16`.
* **`a1_device_probe` перезаписывает отслеживаемый** `tools/parity/out/w2_a1_device.json`
  — сохранять M6-копию и `git restore` файл W2 (иначе грязное дерево).
* `serde_json::Value` — `as_array()`, а не `as_sequence()` (то — serde_yaml);
  `Response::json()` возвращает `Result` — нужен `.ok()`.
* Живые сервисы держат `target\debug\hds.exe` → `cargo test --workspace` падает сборкой;
  либо `--exclude hds-cli`, либо остановить/вернуть сервисы. `target\release\hds.exe`
  свободен. При работающем стенде (`load5.py`) НЕ пересобирать
  `target\debug\{hds,llm_host}.exe`.
* `Stop-Process -Force` = TerminateProcess (код уборки не выполняется). Резидент иногда
  «висит» (не отвечает на HTTP, держит VRAM) → `llm_host stop --force` по pid-файлу,
  затем `Start-Process .\target\release\llm_host.exe -ArgumentList 'run' -WindowStyle Hidden`.
* Порты 8010–8012 — резидент; песочницы: 8797 (mcp), 8799 (fake remote). `HDS_ROOT`
  переопределяет `project_root()` (осторожно с индексацией); `HDS_CONFIG` — сильнейший
  путь конфига, но сессионный (снимать `Remove-Item Env:HDS_CONFIG,Env:HDS_ROOT`).
* git — всегда `--no-pager`; `config.yaml` и `tools/parity/out/` — в `.gitignore`
  (но отдельные файлы в `out/` исторически отслеживаются — проверять `git status`
  после прогонов).

## Что осталось от прошлых этапов (не потерять)

* **Движок: рост fit-шага** от числа загрузок в процессе (0,57 → 4,2 → 27,3 → **299 с**) —
  фикс M5 убрал перезагрузки из боевого пути, но ARB-3/`model_policy: auto`/смена модели
  снова могут дать минутные паузы → кандидат `engine-patch` P5.
* **Анонимизация (W9)** — без неё `chat.provider: remote` только для доверенных API.
* **`remote` на реальном доверенном API**; **чек-лист внешнего HTTPS** (домен/сертификат).
* Замечания M5-1…M5-3 (низкие) — `M5_CODE_REVIEW.md` §3; F9 (`Host` не читается) — `PLAN_M3`.
* **Хвосты M6 (ревизия 2):** мультикарточность одного семейства не проверена живьём (нет
  железа) — юниты + хвост «машина с N картами»; чат-реплики на двух картах — без живой
  проверки; дискретной AMD нет — AMD-путь проверен частично (свидетельство с iGPU,
  `a7 --no-chat` 2/2 раунда); Apple Silicon — подготовка без живой проверки; смешанные
  вендоры и UMA — вне области поддержки (warn в `hds check`).

## Остановить / вернуть боевые сервисы (проверено)

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
# проверить: status → parallel 1, chat LOADED; Test-Path .\index.pause → True; http://127.0.0.1:8787/health → 200
```

## Стенд (по нужде; требует свободных 8010–8012)

```powershell
python tools\parity\load5.py --tag p5_16384 --parallel 5 --ctx 16384
python tools\parity\load5.py --tag remote1  --parallel 5 --ctx 16384 --chat-remote fake --remote-latency 3
```
* Пишет `tools/parity/out/m4_load_<tag>.json`; сам удаляет системный state-каталог, если создал.
* Прогон 1–6 мин → фоном (`Start-Process python ... -RedirectStandardOutput <лог>`) и поллить лог.
* Владелец пользуется прежде всего **транскрибацией** — приёмку строить вокруг неё.

## Текущее состояние на момент передачи

HEAD `a919a78`; дерево чистое; боевой резидент поднят (`chat LOADED`, embedding/rerank
`on_demand`), `index.pause` — есть; боевой каталог движка без Vulkan (CUDA0+CPU). В
песочнице `tools\parity\out\engine-vulkan` — наша `ggml-vulkan.dll` (готова к T2/T5-
прогонам AMD-сценария). T0 (GO) и T1 закрыты и закоммичены; журнал §8a и `STATUS.md`
обновлены. Ближайший шаг — **T2** (§3.6).
