# M6_HANDOFF_PROMPT_2.md — промт для нового чата (этап Э5 «GPU: раскладка ролей по картам», ревизия 2)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, факты T0 с числами, правила, грабли окружения и хвосты.
>
> **Заменяет `M6_HANDOFF_PROMPT.md`** — там зафиксирован **снятый** сценарий «CUDA+Vulkan
> в одном `llm-host`» (смешанные вендоры исключены из требований 10.10.2026).
> План этапа — `PLAN_M6_GPU_MULTIVENDOR.md` (**ревизия 2**); сводка — `STATUS.md`;
> дорожная карта — `PLAN_SERVER_MULTIUSER.md` §2 **W4** / §5 **Э5**; журнал — `PLAN_M6` §8a;
> спайки — `tools/parity/SPIKES.md`; терминология «спайк» — там же (вопрос-ответ по терминам
> зафиксирован в переписке этапа: спайк = один вопрос → числа → go/no-go → фолбэк).

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние: **M1–M5 закрыты** (M5 прошёл код-ревью без условий, `M5_CODE_REVIEW.md`).
Этап **M6/Э5 начат**; в предыдущем чате сделано:
* требования владельца **пересмотрены** → план переписан (**ревизия 2**, коммит `c53e1d0`);
* спайк **T0** выполнен частично, получены **числа** (см. «Факты T0»);
* закоммичена проба **`a7_multiinstance_probe`** (`39980b7`) — инструмент T0;
* **код Э5 (T1–T6) не начат**; ближайший шаг — дособрать `ggml-vulkan.dll` из нашего дерева.

Станция: Windows, RTX 3060 12 ГБ (единственный дискретный GPU), AMD Radeon Graphics
(**iGPU**, UMA — в прод-область не входит), Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1.
Ядро на Rust (`crates/`), Python — только sidecar.

## Требования (ревизия 2 — решения владельца 10.10.2026)

1. Прод — несколько **дискретных** GPU **одного семейства**: NVIDIA под Windows **или**
   AMD под Windows; либо **один Mac с Metal**. Смешанные вендоры (AMD+NVIDIA на одной
   машине) и iGPU/UMA — **не поддерживаются** (`hds check` → warn).
2. Поставка — **отдельными сборками**: NVIDIA / AMD / Metal.
3. Несколько карт — чтобы **одновременно обслуживать разных пользователей**. **Чат-реплики
   одинаковые** (одна модель/контекст; различаются портом и слотами); допускаются на одной
   карте («две чат-модели на карте 24 ГБ») и на разных.
4. **Индексация (`embedding` + `rerank`) — ровно один экземпляр на машину**; принцип
   «один набор моделей индексации обрабатывает один файл одновременно» (`n_parallel = 1`).
5. **Транскрибация (`whisper`) — экземпляр на карту**; **один файл — одна карта** (split
   одного файла по картам отвергнут: whisper-инстанс на одном устройстве, синхронизация не
   оправдана при ≤2 картах); карту под файл выбирает **`auto_transcribe`** по свободной VRAM;
   разные файлы могут идти параллельно на разных картах.
6. **Маршрутизация** — **один фасад** `llm-host` (`chat` → 8010), распределение по репликам
   внутри хоста (least-busy, при равенстве round-robin); порты реплик наружу не публикуются
   (авторизация/аудит/rate-limit/очередь остаются в одной точке — M2/M3/M4).
7. Индексация — «лёгкая добавка» рядом с чат-репликой; при нехватке VRAM **пауза**
   (штатный `index.pause`), **чат-реплики не вытесняются** (приоритет чата в `gpu.policy`).
8. Железа для проверки нескольких карт нет → мультикарточность принимается юнит-тестами +
   **явным хвостом**; дискретной AMD на станции нет (только iGPU) → AMD-путь частично;
   Apple Silicon — подготовка без живой проверки.
9. Стенд: владельца сейчас интересует прежде всего **транскрибация** — приёмка T5
   переориентирована на неё (RAG-SLA — по необходимости).

Детали и точные формулировки: `PLAN_M6_GPU_MULTIVENDOR.md` §2, §3.1–§3.6, §4 (T0–T6),
§5 (приёмка), §7 (риски), §8a (журнал).

## Точка входа (5 минут)

1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -4` (ожидание: HEAD
   `39980b7` — проба a7, ниже `c53e1d0` — ревизия 2 плана, `b47694e` — исходный план M6)
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short` — **чисто**
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe`: либо остановить
   `hds watch`/`hds mcp-http`/`hds transcribe-watch`, либо `--exclude hds-cli`; после работ
   сервисы **вернуть**. Ожидание: зелёный (в ревью M5 — 87 наборов / 0 failed).
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`
5. Прочитать: `PLAN_M6_GPU_MULTIVENDOR.md` (целиком, короткий) → `STATUS.md` (блок M6) →
   `PLAN_SERVER_MULTIUSER.md` §2 W4/§5 Э5 → `crates/hds-llama/src/{device.rs,registry.rs}`
   (выбор устройства и раскладка ролей) → `tools/parity/SPIKES.md` (термин «спайк»).
6. Проверить боевое состояние (не менять!):
   `& .\target\release\llm_host.exe status` → desktop-профиль: `parallel 1`, chat `LOADED`
   `retention=keep` n_ctx=65536, embedding/rerank `on_demand`, устройства `CUDA0`+`CPU`;
   `Test-Path .\index.pause` (**боевая пауза — не снимать**, сейчас есть);
   `& .\target\debug\hds.exe check --json` → id `profile`,`db`,`queue`,`chat_provider` = ok.

## Факты T0 (проверено на этой станции, с числами)

* Движок собран с **`GGML_BACKEND_DL=ON`**: бэкенды — отдельные DLL в каталоге движка
  (`ggml-cuda.dll`, `ggml-cpu-*.dll`), загрузчик — `ggml_backend_load_all()` из
  `ggml/src/ggml-backend-reg.cpp` (ищет `ggml-<name>[-variant].dll` в каталоге
  исполняемого файла и **текущем каталоге**; наш `Engine::activate()` переключает cwd).
* Пока в каталоге нет Vulkan-бэкенда, движок перечисляет только `CUDA0` (RTX 3060) + `CPU`.
* **Чужие сборки `ggml-vulkan.dll` подмешивать нельзя** (проверено экспериментом):
  * `b11528`: `api_version = 3` против нашего `GGML_BACKEND_API_VERSION = 2` → загрузчик
    отклоняет **молча** (в release `silent = true`, сообщений нет; выяснено пробой ctypes);
  * `b11200`: `api_version = 2`, все 35 импортов ⊆ наших экспортов (`ggml-base.dll` — 641
    функция), `0 missing` — DLL грузится и **перечисляет устройства**, но **крашит процесс**
    при загрузке модели. Причина: апстрим изменил layout `ggml_backend_buffer_i`
    (добавлены `set_tensor_2d`/`get_tensor_2d` между `get_tensor` и `cpy_tensor`)
    **без бампа `GGML_BACKEND_API_VERSION`** → backend-созданные буферы получают сдвинутую
    таблицу указателей. Оба прогона (a1 и a7) падали в `ggml-base.dll` RVA `0x17A79`
    (`ggml_backend_reg_get_proc_address`, `mov rax,[rcx+0x20]`, мусорный `reg`).
  * Диагностические артефакты: `tools/parity/out/m6_*` (`m6_spike_abi.py`,
    `m6_check_candidate.py`, `m6_diff_impl.py`, `m6_diff_imports.py`, `m6_probe_procs.py`,
    `m6_disasm_crash.py`, `m6_crash_events.txt`).
* **Вывод:** Vulkan-бэкенд собираем **из нашего дерева** (`C:\Users\Sasha\ENGINEbuilds`).
* С рабочей (чужой) DLL движок перечислял 4 устройства: `0 CUDA0 (RTX 3060)`,
  `1 Vulkan0 = AMD Radeon Graphics` (**iGPU, UMA**, free 31087 / total 32723 МиБ),
  `2 Vulkan1 = RTX 3060 через Vulkan`, `3 CPU` → **ловушка компата**: при появлении
  Vulkan-бэкенда `gpu.device_index = 1` начинает указывать на **AMD**, а не на NVIDIA —
  пин по `backend + name` обязателен (§5 приёмки, §3.2).
* Chat Qwen3.5-9B на CUDA0 в нашем движке (замер a7): fit `8616` против `11203` МиБ,
  `LOADED` за ≈2,5 с; крах наступал на **втором** инстансе (Vulkan).
* Vulkan-железо рабочее: `vulkaninfo --summary` → `AMD Radeon Graphics` API 1.3.260,
  `NVIDIA GeForce RTX 3060` API 1.4.351; `vulkan-1.dll` — в `System32`.

## Сборка Vulkan-бэкенда (наш путь; в работе — первый шаг нового чата)

* Дерево llama.cpp: `C:\Users\Sasha\ENGINEbuilds` (то же, что у CUDA-сборки движка;
  снапшот без `.git`, базовый тег неизвестен — поэтому **только своя сборка**).
* Vulkan SDK «облегчённый», **без админа**: `C:\Users\Sasha\VulkanSDK\1.4.335.0`
  (`Bin\glslc.exe`, `Lib\vulkan-1.lib`, `Include\vulkan\*`). Как добыт: official installer
  LunarG — Qt Installer Framework, требует elevation (`Cannot elevate access rights while
  running from command line`) → 7z-payload вырезан из offline-EXE карвингом
  (`tools/parity/out/m6_carve_sdk.py`) и распакован `py7zr` (1.1.3, стоит в `.venv`).
* Скрипт сборки: `tools\parity\out\build_ggml_vulkan.cmd`
  (vcvarsall x64 + `Ninja Multi-Config` из VS18 + `VULKAN_SDK`; Vulkan уже находится:
  `Found Vulkan … components: glslc glslangValidator`). **Осталось добавить флаги**
  `-DLLAMA_BUILD_SERVER=OFF -DLLAMA_BUILD_TOOLS=OFF -DLLAMA_BUILD_COMMON=OFF` — без них
  configure падает: `LLAMA_HTTPLIB is OFF, cannot build llama-server`.
* Команда сборки (цель — только бэкенд): `cmake --build
  C:\Users\Sasha\ENGINEbuilds\build-ggml-vulkan --config Release --target ggml-vulkan`
  (BuildDir уже создан, конфиг неполный; каталог можно удалить и пересоздать).
* Артефакт → `tools\parity\out\engine-vulkan\ggml-vulkan.dll` (**боевой каталог движка
  `%APPDATA%\OpenResearchTools\TranscribeOffline\Engine` не трогать**; в песочной копии
  сейчас лежит чужая b11200-DLL, 45 286 400 байт — заменить).

## Инструменты (готовые)

* **`a7_multiinstance_probe`** (наш инструмент T0, закоммичен `39980b7`):
  `cargo build -p hds-llama --release --bin a7_multiinstance_probe` →
  `target\release\a7_multiinstance_probe.exe [--engine-dir DIR] [--model-chat GGUF]
  [--model-embed GGUF] [--n-ctx-chat N] [--n-ctx-embed N] [--rounds N] [--n-predict N]
  [--json FILE] [--nvml-index N] [--no-chat] [--no-embed]`.
  Поднимает в одном процессе chat на CUDA + embedding на Vulkan, гоняет раунды
  load → chat/embeddings вперемешку → unload, пишет JSON (`tools/parity/out/m6_spike_*`).
  **Для ревизии 2 нужен режим `--no-chat`** (роль embed на Vulkan-устройстве = сценарий
  AMD-сборки). ⚠ `--engine-dir` обязан быть **абсолютным**.
* `a1_device_probe --engine-dir <абс.>` — перечисление устройств + VRAM-пробы по вариантам.
* Песочная копия движка: `tools\parity\out\engine-vulkan` (наши патченые DLL + чужая
  vulkan-DLL, которую надо заменить нашей сборкой; копия сделана `robocopy /E /XF *.orig`).
* `llm_host devices|status|load|unload|stop`; `hds check --json`.
* `tools/parity/load5.py` — стенд (SLA), требует свободные порты 8010–8012.

## Правила

* Сначала план по файлам, при согласовании — код. После каждого шага — тесты + коммит
  (кириллицу в сообщениях коммитов писать **файлом** и `git commit -F file`).
* Не ломать десктоп-профиль (боевой конфиг — desktop, `index.pause` не снимать).
* Боевые `PLAN_*.md` не переписывать без разрешения владельца (для ревизии 2 разрешение
  получено); `STATUS.md` и журнал §8a обновлять всегда.
* Рантайм-проверка обязательна (карты/бэкенды/VRAM/слоты — на живом сценарии), не только юниты.
* GPU-пробы — по `cline-rules/dev-machine.md`: останавливать и **возвращать** резидент,
  освобождать занятые `.exe`, артефакты — в `tools/parity/out/` (гитигнор).
* Хвосты переносить явно (раздел ниже), а не «терять». Один активный чат на этап.

## Грабли окружения (важно! — проверено сессиями M4/M5/M6)

* **`run_commands`: каждый элемент массива = ОТДЕЛЬНАЯ PowerShell-сессия** — переменные не
  живут между элементами; многошаговая проба — одним элементом через `;` или `.ps1`/`.cmd`.
* PowerShell иногда «съедает» первый токен команды — повторять; для диагностики начинать
  с `Write-Output 'x';`.
* `*>` пишет **UTF-16** (читать `encoding='utf-16'`); кириллица в консоли портится (cp866) —
  логи фильтровать **питоном** в файл и читать read-инструментом (не `Select-String`).
* **`AddDllDirectory`/`SetDllDirectoryW` требуют абсолютного пути** (иначе `ошибка 87`) —
  `--engine-dir` и пути к пробам всегда абсолютные (грабля стоила прогона).
* В **release**-сборке движка `ggml_backend_load_all` работает с `silent = true` — «Vulkan
  не видно» без единого сообщения; проверять ctypes-пробой (`m6_probe_procs.py`).
* `Expand-Archive`/`tar` не берут zip64/SFX; LunarG-installer — Qt IFW, требует elevation;
  payload достаётся карвингом 7z + `py7zr`.
* Живые сервисы держат `target\debug\hds.exe` → `cargo test --workspace` падает сборкой;
  либо `--exclude hds-cli`, либо остановить/вернуть сервисы. При работающем стенде
  (`load5.py`) НЕ пересобирать `target\debug\{hds,llm_host}.exe`.
* `Stop-Process -Force` = TerminateProcess (код уборки не выполняется). Резидент иногда
  «висит» (не отвечает на HTTP, держит VRAM) → `llm_host stop --force` по pid-файлу, затем
  `Start-Process .\target\release\llm_host.exe -ArgumentList 'run' -WindowStyle Hidden`.
* Порты 8010–8012 — резидент; песочницы: 8797 (mcp), 8799 (fake remote). `HDS_ROOT`
  переопределяет `project_root()` (осторожно с индексацией); `HDS_CONFIG` — сильнейший
  путь конфига, но сессионный (снимать `Remove-Item Env:HDS_CONFIG,Env:HDS_ROOT`).
* git — всегда `--no-pager`; `config.yaml` и `tools/parity/out/` — в `.gitignore`.

## Что осталось от прошлых этапов (не потерять)

* **Движок: рост fit-шага** от числа загрузок в процессе (0,57 → 4,2 → 27,3 → **299 с**) —
  фикс M5 убрал перезагрузки из боевого пути, но ARB-3/`model_policy: auto`/смена модели
  снова могут дать минутные паузы → кандидат `engine-patch` P5.
* **Анонимизация (W9)** — без неё `chat.provider: remote` только для доверенных API.
* **`remote` на реальном доверенном API**; **чек-лист внешнего HTTPS** (домен/сертификат).
* Замечания M5-1…M5-3 (низкие) — `M5_CODE_REVIEW.md` §3; F9 (`Host` не читается) — `PLAN_M3`.
* **Новые хвосты M6 (ревизия 2):** мультикарточность одного семейства не проверена (нет
  железа) — юниты + хвост «машина с N картами»; дискретной AMD нет — AMD-путь проверяется
  частично (свидетельство с iGPU); Apple Silicon — подготовка без живой проверки;
  смешанные вендоры и UMA — вне области поддержки (warn в `hds check`).

## Ближайшие шаги (по ревизии 2)

1. **T0 (в работе):** дособрать `ggml-vulkan.dll` (см. выше), подложить в
   `tools\parity\out\engine-vulkan`, прогнать
   `a7_multiinstance_probe --no-chat --engine-dir C:\Users\Sasha\hermes-disk-search\tools\parity\out\engine-vulkan`
   (роль embedding на Vulkan-устройстве) и `a1_device_probe` (перечисление устройств);
   зафиксировать числа в `tools/parity/out/m6_spike_*.json` и отчёт в `SPIKES.md`.
   Фолбэк при неудаче — warn в `hds check` («AMD-инсталляции не поддерживаются»).
2. **T1:** конфиг `gpu.devices` + `llm_server.<role>.device` (компат `gpu.device_index` —
   пин по `backend+name`), валидация §3.5 (одна роль — одна карта; реплики — уникальные
   имена/порты; индексация — ровно один экземпляр; `whisper` — не более одного на карту),
   пункт `gpu_roles` в `hds check` + warn про смешанные вендоры/UMA.
3. **T2:** раскладка в `registry.rs`/`host.rs` (роль → карта/backend в статусе), чат-реплики,
   `whisper` по картам, один фасад + внутренняя маршрутизация (§3.6).
4. **T3:** бюджет/вытеснение по картам с учётом нескольких ролей на одной карте.
5. **T4:** вендор-гомогенные пресеты + подсказки/валидация.
6. **T5:** приёмка (приоритет — **транскрибация**: живой файл, ≥3 прогона, карта/латентность/
   пик VRAM), десктоп не изменён. **T6:** доклад (README/`config.example.yaml`/§9 дорожной карты).
7. Не забыть: если Vulkan идёт в поставку — шаг «Vulkan SDK» в `engine-patch/README.md`;
   **старый `M6_HANDOFF_PROMPT.md` не использовать** (снятый сценарий).

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
* Владелец сейчас пользуется прежде всего **транскрибацией** — приёмку строить вокруг неё.

## Текущее состояние на момент передачи

HEAD `39980b7`; дерево чистое; боевой резидент поднят (`chat LOADED`, `embedding`/`rerank`
`on_demand`), `index.pause` — есть; песочная копия `tools\parity\out\engine-vulkan` содержит
чужую b11200-vulkan-DLL (заменить нашей сборкой); сборка Vulkan-бэкенда не завершена
(configure падает на httplib — добавить `-DLLAMA_BUILD_SERVER=OFF` и др.).


