# hermes-disk-search

Локальный поиск и «спроси по своим файлам» — по дискам машины. **Ядро на Rust**:
индексация, гибридный поиск (FTS5 + вектор + CLIP), RAG-ответы, MCP-сервер и
веб-интерфейс. Python остаётся **только там, где он нужен по делу** — извлечение текста
из офисных форматов/PDF/OCR и русская лемматизация (автономный воркер `sidecar`).

> Приватный проект (`Guerchoig/hermes-disk-search`). Состояние и журналы волн миграции:
> `STATUS.md`, `tools/parity/W4_REPORT.md` (передача + §1–§13), `MIGRATION_PLAN_RUST.md`.

## Как это работает

```
        llm-host.exe  (резидент, владелец GPU)             :8010 chat
        движок openresearchtools (LLM + ASR)               :8011 embedding
              │  OpenAI-совместимый фасад + /internal/*     :8012 rerank
              │
  hds.exe ────┤  index / search / ask / status / check / ui (:8765)
  hds_mcp.exe │  MCP stdio
  hds mcp     ┘  --http -> :8787     ┌────────────────────────────────┐
                                     │ sidecar (Python-воркер, stdio)  │
                                     │  extract / normalize (.mpp,     │
                                     │  pdf/docx/xlsx/pptx/OCR)        │
                                     └────────────────────────────────┘
```

* **Индексация** (`hds index` / watcher / UI): обход диска → извлечение текста →
  чанки → эмбеддинги (`:8011`) → SQLite (`index.db`) + FTS5 + `sqlite-vec` (+ опц. CLIP).
* **Поиск** (`hds search`): RRF-слияние FTS5 и векторного KNN; опц. CLIP по картинкам.
* **RAG** (`hds ask`, MCP `ask_my_files`): гибридный поиск → опц. реранк (`:8012`) →
  ответ чат-моделью (`:8010`) со ссылками `[N]`.
* **llm-host** — единый резидент: держит роли чат/эмбеддинги/реранк на GPU и **сам ASR**
  (движок вместо llama.cpp), раздаёт OpenAI-совместимый HTTP на 8010–8012 и владеет
  VRAM-диспетчером. Кросс-процессной адресации инстансов у движка нет — поэтому всё
  общение идёт через этот фасад.
* **sidecar** — автономный Python-процесс (stdio + JSON-RPC 2.0) для того, что решено
  оставить в Python: офисные форматы/PDF/OCR и `pymorphy3`-лемматизация FTS. Ядро
  запускает его лениво и владеет процессом. `.mpp` (MS Project) требует Java 11+ (см. ниже).

## Требования (Windows x64)

* **Windows 10/11 x64**. macOS (Apple Silicon) — код есть, но **артефакт не проверен**
  (`MIGRATION_PLAN_RUST.md` §10.0; из релиза выведен).
* **GPU** — не обязательна: без CUDA-карты движок работает на Vulkan/CPU (медленнее).
  Рекомендуется NVIDIA ≥8 ГБ VRAM (проверено на RTX 3060 12 ГБ: чат 9B + эмбеддинги + ASR).
* **Microsoft Visual C++ Redistributable (x64)** — установщик поставит при отсутствии.
* **Java 11+** — только для `.mpp` (MS Project). Без неё `.mpp` индексируется с пометкой,
  остальное не затрагивается. Воркер сам находит `%LOCALAPPDATA%\jdk-21\*\bin\server\jvm.dll`
  (`JAVA_HOME` не обязателен).
* **ffmpeg** — для транскрипции медиа (движок ASR); `winget install Gyan.FFmpeg` (ставит установщик).
* **Tesseract OCR** — по желанию (текст на картинках/сканах); ставится по согласию.
* Сеть — для разовой загрузки: рантайм движка (по манифесту), GGUF-модели, модели CLIP.

## Установка — Windows

1. Распакуйте архив релиза `hds-<версия>-windows-x64.zip` в любую папку (например `D:\hds`).
2. Запустите **`setup.cmd`** (двойным щелчком) — он снимет пометку «скачано из интернета» (MotW)
   и вызовет `setup.ps1` с `-ExecutionPolicy Bypass`.

`setup.ps1` (ASCII-only) делает по шагам:

1. **Проверка артефакта** — есть ли `bin\hds.exe` (иначе понятное сообщение: это не Rust-сборка).
2. **Системные зависимости через winget** — `ffmpeg` (авто), `Tesseract OCR` (по согласию),
   Visual C++ Redistributable (если нет).
3. **Проверка sidecar-воркера** — рукопожатие `hello` (интерпретатор: `HDS_EXTRACT_PYTHON` →
   `sidecar\python` → `.venv`); при неудаче — подсказка про системный Python 3.10+.
4. **`config.yaml`** — из `config.example.yaml`, если нет; эвристика «диски из конфига отсутствуют».
5. **Рантайм движка** — `installers\fetch_engine_runtime.ps1` (по `runtime-manifests\engine-manifest.json`,
   выбор `cuda`/`vulkan`, проверка **sha256**; кладётся в `%APPDATA%\OpenResearchTools\TranscribeOffline\Engine`).
6. **Модели** — `fetch_llm_models.ps1` (GGUF chat/embedding/rerank в общий `%LOCALAPPDATA%\llama-runtime`),
   опц. `fetch_whisper_model.ps1` (ASR), опц. `fetch_clip_models.ps1` (CLIP ~850 МБ).
7. **Задачи Планировщика** (по согласию): `HermesDiskSearchLlmHost` (`bin\llm_host.exe run`),
   `HermesDiskSearchWatch` (`bin\hds.exe watch`), `HermesDiskSearchMcp` (`bin\hds.exe mcp-http run`),
   опц. `HermesDiskSearchUi`.
8. **Интеграции** — `install_hermes.ps1` / `install_cline.ps1` (MCP по URL `:8787` или stdio `bin\hds_mcp.exe`).
9. **Ярлык** на рабочем столе («Hermes Disk Search» → `run_ui.ps1`).
10. **Диагностика** — `bin\hds.exe check`.

Флаги: `-SkipModels`, `-SkipEngine`, `-NoAutostart`, `-SkipIntegrations`, `-SmokeTest`
(мини-индекс на временной БД).

### Установка с версионированием (`app\<версия>`)

Чтобы обновлять, **не перезаписывая запущенный `hds.exe`/`llm_host.exe`** (на Windows нельзя),
используйте версионную раскладку:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_app_version.ps1 `
    -From <hds-<версия>-windows-x64 | .zip> -Version <версия> -Root D:\hds -SetCurrent
```

Раскладка: `app\<версия>\` (код версии + `config.yaml`), `app\current` — junction на активную
версию, общие `data\` и `models\` (junction-ы). `db_path` должен быть абсолютным или
`data\index.db` — иначе индекс будет «на версию» (скрипт предупредит). Обновление целиком —
**отдельным** процессом (процесс не может заменить сам себя):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File installers\update.ps1 `
    -From hds-<новая>-windows-x64.zip -Version <новая> -Root D:\hds
```

`update.ps1` останавливает задачи/резидента → ставит новую версию → переключает `app\current` →
запускает задачи. Прошлые версии сохраняются — откат = вернуть `current` на прежнюю.

### Блокировки «скачано из интернета» и подпись

Бинарники не подписаны (подпись кода в бюджет не входит). Поэтому:

* `setup.cmd`/`setup.ps1` снимают MotW (`Unblock-File`) — иначе политика `RemoteSigned`
  потребует цифровую подпись для `.ps1`;
* SmartScreen может показать «Windows protected your PC» для `hds.exe` — «More info → Run anyway».

## Архивы релиза

| Ассет | Содержимое |
|---|---|
| `hds-<версия>-windows-x64.zip` (+ `.sha256.txt`) | `bin\{hds,hds_mcp,llm_host}.exe`, `installers\`, `runtime-manifests\`, `sidecar\`, `assets\`, `shortcuts\`, `hermes-skill\`, скрипты, `config.example.yaml`, `README.md`, `NOTICE.md`, `sha256.txt` |
| `hds-engine-runtime-windows-x64-cuda.zip` | рантайм движка (LLM-хост + ASR) по манифесту (CI-джоба `fetch-engine-runtime`) |
| `hermes-disk-search-<версия>-windows.zip` | архив исходников (legacy) |

Модели CLIP в релиз не входят (≈850 МБ) — отдельный тег `clip-onnx-v1`, установщик скачивает по
`runtime-manifests\clip-manifest.json`. GGUF/whisper — по URL из `installers\fetch_llm_models.ps1` /
`fetch_whisper_model.ps1`. Атрибуция и лицензии — `NOTICE.md`.

## Использование (CLI `hds`)

```powershell
# Первичная индексация (запустить на ночь; прогресс в консоли)
hds index
hds index --roots "D:\;E:\" --kinds text,pdf --progress-sec 3

# Поиск и RAG
hds search "техническое задание" --limit 8 [--kinds text,pdf] [--json]
hds ask "какие требования к срокам?" [--limit 8] [--json]

# Состояние и диагностика
hds status [--json]
hds check  [--json]      # компоненты окружения + роли (то же, что /api/diagnostics в UI)

# Инкремент/обслуживание
hds reindex <путь> [--no-force]
hds reindex-fts [--progress-sec N]     # перестроить лемматизированный FTS
hds forget <путь>
hds db-move --to <новый путь index.db> [--force]
hds clip-index                          # дозаполнить CLIP-векторы картинок
hds whisper-check [--file <медиа>] [--json]

# Процессы/сервисы
hds stop                                # создать index.stop (остановить индексацию)
hds watch [--roots a;b]                 # наблюдатель ФС
hds ui [--host H] [--port N]            # веб-интерфейс (default 127.0.0.1:8765)
hds mcp [--http --host H --port N --path /mcp]   # MCP stdio или streamable-http
hds mcp-http check|start|stop|status|restart|run
```

Общее: `-h`/`--help`. Конфиг — `config.yaml` (путь можно переопределить `HDS_CONFIG`).
Индексация кооперативно останавливается файлом `index.stop` и приостанавливается `index.pause`.

## Веб-интерфейс (`hds ui`)

Ярлык «Hermes Disk Search» (рабочий стол) запускает `run_ui.ps1` → `bin\hds.exe ui` и открывает
`http://127.0.0.1:8765`. Что умеет: статус индекса и ролей `llm-host`, поиск, `ask`, дерево
индексации по БД (`/api/tree`, кэш 30 с, `?refresh=1`), диагностика (`/api/diagnostics` = полный
`hds check`), управление индексацией (`start`/`stop`/`pause`/`resume`), правка `config.yaml`
(roots/exclude_paths) **с сохранением комментариев**. Страница шлёт `X-HDS-UI: 1` (CSRF-защита POST).

Логи сервера: `%LOCALAPPDATA%\hermes-disk-search\ui.log` и `ui.err.log`.

## MCP-сервер

* **stdio** — `bin\hds_mcp.exe` (он же `hds mcp`): каждый клиент поднимает свой процесс.
  Инструменты: `search_files`, `ask_my_files`, `get_file`, `index_status`, `start_indexing`, `stop_indexing`.
* **streamable-http** — ОДИН инстанс на машину: `hds mcp --http` (URL по умолчанию
  `http://127.0.0.1:8787/mcp`); управление — `hds mcp-http check|start|stop|status|restart|run`
  (`restart` переиспользует живой инстанс, но перезапускает устаревший).

Интеграции подключают клиентов **по URL** — они не запускают свои процессы (иначе плодятся сироты).

## Интеграция с Hermes / Cline

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File install_hermes.ps1   # Hermes Desktop
powershell -NoProfile -ExecutionPolicy Bypass -File install_cline.ps1    # Cline Desktop/CLI
```

Идемпотентно: регистрируют MCP-сервер (URL `:8787`, иначе stdio `bin\hds_mcp.exe`), кладут скилл,
при необходимости правят `.env` Hermes — блок `NO_PROXY=localhost,127.0.0.1,::1` (иначе httpx2
шлёт локальные запросы в системный прокси, и MCP отвечает 503). Можно запускать до установки
клиента и повторить позже.

## llm-host — резидент (владелец GPU и портов 8010–8012)

`bin\llm_host.exe run` — единый процесс: держит роли **chat** (`:8010`), **embedding** (`:8011`),
**rerank** (`:8012`) на движке openresearchtools и **сам ASR** (whisper через движок), раздаёт
OpenAI-совместимый HTTP и `/internal/*` (`status`, `transcribe`).

```powershell
bin\llm_host.exe run       # запустить резидент (владелец портов)
bin\llm_host.exe status    # отчёт: роли, VRAM, пауза
bin\llm_host.exe stop      # graceful: выгрузить инстансы, снять pid
```

* **VRAM-диспетчер** (`gpu.policy`): `query_priority` (запрос важнее — индексация на паузу),
  `indexing_priority`, `manual`. Вытесняет роли по приоритетам и **уважает** пользовательскую
  `index.pause` (не снимает её).
* Рантайм движка — `%APPDATA%\OpenResearchTools\TranscribeOffline\Engine` (переопределяется
  `HDS_ENGINE_DIR`/`index.whisper_engine_dir`); GGUF-модели — общий `%LOCALAPPDATA%\llama-runtime`.
* Автозапуск — задача Планировщика `HermesDiskSearchLlmHost` (`installers\install_llm_host_task.ps1`).

## sidecar — Python-воркер извлечения

Автономный процесс `sidecar\hds_extract\worker.py` (stdio + JSON-RPC 2.0, NDJSON): `hello`,
`extract`, `normalize`, `clip_image` (CLIP в Rust — отвечает «не поддерживаю»), `shutdown`.
Ядро запускает его лениво и владеет процессом. Извлечение — PDF/DOCX/XLSX/PPTX/OCR и `.mpp`
(MS Project через mpxj/Java); лемматизация (`pymorphy3`) — для FTS.

**Самодостаточность.** `installers\build_sidecar.ps1` собирает дерево, работающее **без**
Python-ядра проекта: портативный CPython (python-build-standalone) + зависимости +
**копия** нужных модулей `hds\` под `sidecar\hds\`. Воркер кладёт `sidecar\` в `sys.path`
перед корнем проекта — в поставке `hds` берётся из копии, в разработке — из корня.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_sidecar.ps1 -OutDir dist\sidecar -SelfTest
```

## Индексная БД и файлы-сигналы

* **`index.db`** — SQLite: `files`, `chunks`, `chunks_fts` (FTS5, лемматизированный), `chunks_vec`
  (`sqlite-vec`, dim `embedding.dim`), `images_vec` (CLIP, dim 512). Путь — `db_path` (по умолчанию
  относительный — от корня проекта; в версионной раскладке лучше абсолютный или `data\index.db`).
* **`index.stop`** — файл-команда: аккуратно остановить текущую индексацию.
* **`index.pause`** — приостановить индексацию (ставит UI/диспетчер; **пользовательскую не снимать сами**).
* **`index.heartbeat.json`** — кросс-процессный статус/прогресс (свежесть < 30 с).
* **`watch.lock`** — pid наблюдателя; **`data\llm-host.pid`** — pid резидента; логи — `data\logs\`.

## Настройки (`config.yaml`)

Полный образец — `config.example.yaml`. Основное:

* **`index`** — `roots` (корни), `exclude_dirs`/`exclude_paths`, `max_file_mb`/`max_media_mb`,
  `ocr`/`ocr_lang`/`ocr_tesseract_cmd`, `transcribe` + `whisper_*` + `transcribe_url` (ASR движком,
  владелец — llm-host), `clip`/`clip_*` (ONNX-модели; по умолчанию `models\clip_onnx`), `max_chunks`.
* **`db_path`** — путь к `index.db` (относительный — от корня проекта).
* **`chunk`** — `size`/`overlap` (структурный чанкер).
* **`embedding`** — `base_url` (`:8011/v1`), `model` (`text-embedding-bge-m3`), `batch_size`, `dim` (1024).
* **`chat`** — `base_url` (`:8010/v1`), `model`, `thinking` (`off|auto`), `temperature`, `max_context_chars`.
* **`llm_server`** — `host`, `autostart`, порты/модели/`ctx_per_slot`/`extra_args` по ролям
  (`chat`/`embedding`/`rerank`). Модель роли — `shared:<role>` (из общего рантайма) или путь.
* **`gpu`** — `policy`, `device_index` (0=CPU, 1=первый GPU), `n_gpu_layers`, `reserve_mb`,
  `evict_idle_sec`, `pause_index_on_query`, `priorities`.
* **`mcp_http`** — `host`/`port`/`path`/`autostart`/`start_timeout`.
* **`search`** — `vec_k`/`fts_k`/`rrf_k`/веса/`snippet_chars`.
* **`rerank`** — `enabled`/`url`/`model`/`timeout`/`max_latency`.
* **`watch`** — `debounce_seconds`, `reconcile_on_start`, `max_stable_wait`.
* **`extract`** (опц.) — `idle_timeout` воркера, сек.

Переменные окружения: `HDS_CONFIG` (путь к конфигу), `HDS_EXTRACT_PYTHON` (интерпретатор воркера),
`HDS_ENGINE_DIR` (каталог движка), `HDS_ROOT` (корень проекта).

## Разработка

Воркспейс — `crates/`:
`hds-core` (config/db/http/диагностика), `hds-extract` (клиент воркера), `hds-index`
(обход/хэш/чанкер/конвейер/watch/transcribe/**diag**), `hds-llama` (движок, фасад, `llm_host`),
`hds-clip` (ONNX vision+text), `hds-search` (fts/snippet/rerank/rag), `hds-mcp`, `hds-ui`,
`hds-cli` (бинарь `hds`).

```powershell
cargo build --release -p hds-cli -p hds-mcp -p hds-llama
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                     # 174 passed / 0 failed (+9 ignored)

# релизный пакет (плана §10.1). Резидент держит target\release\llm_host.exe ->
# либо -SkipBuild (готовые бинарники), либо предварительный `llm_host stop`.
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_rust_release.ps1 -Version 0.1.0 [-SkipBuild]

# портативный sidecar (нужен uv, ~280 МБ загрузки)
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_sidecar.ps1 -OutDir dist\sidecar -SelfTest
```

CI (`release.yml`): `test-rust` (fmt/clippy/tests) → `build-sidecar` → `build-windows`
(package + smoke) → `fetch-engine-runtime` → `release`.

## Диагностика

* `hds check` / в UI «Проверка компонентов» — БД, корни, роли chat/embedding, OCR, ffmpeg,
  лемматизатор (воркер), **`.mpp`** (mpxj), реранк; отдельный пункт `gpu-manual`
  (whisper/Vulkan — проверяются вручную, GPU-чек-лист).
* `hds status` — состояние индекса; `hds whisper-check` — готовность ASR.

**Грабли.** `git` — всегда `--no-pager`. PowerShell 5.1 читает `.ps1` без BOM как ANSI — наши
скрипты ASCII-only; в строках писать `${var}`, а не `$var:` (последнее парсится как имя диска).
Кириллица в `Select-String` не ищется (ASCII-шаблон или чтение файла инструментом).

## Лицензии

Сторонние компоненты (движок openresearchtools, NVIDIA CUDA EULA, FFmpeg, Tesseract,
PyMuPDF — **AGPL-3.0**, mpxj/JPype + Java, модели) и их лицензии — `NOTICE.md`.

## Осталось в Python

По плану миграции в Python остаётся **только sidecar** (извлечение/лемматизация) — §2.5/§5.
Python-ядро (поиск/RAG/MCP/UI/индексатор/менеджер ролей) переписано на Rust и удалено;
Python-джобы выведены из CI. Паритет-эталон — `tools/parity/` (golden **заморожен**, регенерация
невозможна — генератор удалён); журналы волн — `tools/parity/W1_REPORT.md`…`W4_REPORT.md`, `STATUS.md`.
