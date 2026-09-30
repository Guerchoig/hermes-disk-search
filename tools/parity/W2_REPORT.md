# W2_REPORT.md — журнал волны W2 (трек A: `llm-host`, трек B: ядро)

> Ветка `w2-llm-host`, дата 30.09.2026, рабочая станция: Windows, RTX 3060 12 ГБ
> (занято штатными ролями 8,5–8,6 ГБ на момент замеров), Ryzen 7 5700G, 64 ГБ RAM,
> rustc/cargo 1.97.1. План: `PLAN_W2_LLM_HOST.md`; факты W0 — `SPIKES.md`.
> Правило волны: Python-версия — источник истины по поведению, паритет проверяется
> скриптами и тестами, а не «на глаз».
>
> **Статус на 30.09.2026:** закрыты **A1** (обвязка движка + выбор устройства) и
> **B1/B2** (обход/лимиты/exclude + `content_hash`). Дальше — A2 (реестр инстансов
> и маппинг конфига), B3 (чанкер).

## 1. A1 — обвязка движка и первый тест выбора устройства

### 1.1. Что сделано

Крейт `crates/hds-llama` (первый боевой Rust-крейт воркспейса):

| Модуль | Что внутри |
|---|---|
| `engine_dir` | поиск каталога движка (`index.whisper_engine_dir` → `%APPDATA%\OpenResearchTools\TranscribeOffline\Engine` → рядом с `.exe`), имена библиотек в одной константе, список каталогов поиска DLL |
| `ffi` | точные `repr(C)`-структуры/константы cluster API по `bridge/llama_server_cluster.h` (§11 плана W2) |
| `engine` | загрузка `multi-node-server.dll` через `libloading` + подготовка пути поиска DLL, `EngineCwd`, `Engine::create_cluster` |
| `cluster` | `devices/instances/create/load/unload/find/remove/set_retention/wait_loaded`, `embeddings/rerank/chat_complete`, проверка `ok`/`error` |
| `device` | `DeviceSelection`, маппинг `gpu.device_index` ↔ устройства, `allow_cpu` |
| `vram` | NVML как источник истины (`NvmlProbe`, `VramSampler`), фолбэк по `list_devices`, точка расширения под Metal (W2-7) |

Первый тест — бинарь `crates/hds-llama/src/bin/a1_device_probe.rs`: один и тот же
embedding-инстанс (bge-m3-Q8_0, `n_ctx=8192`, `n_gpu_layers=-1`, `KEEP_LOADED`)
создаётся пятью способами, для каждого замеряются NVML (пик сэмплером 200 мс) и
скорость инференса. Отчёт: `tools/parity/out/w2_a1_device.json`.

```powershell
cargo run -p hds-llama --release --bin a1_device_probe            # полный прогон
cargo run -p hds-llama --release --bin a1_device_probe -- --no-cwd-fix   # воспроизведение R32
```

### 1.2. Результат: какой способ задаёт устройство

| Вариант | `manual_devices_csv` | `allow_cpu` | Вердикт | Состояние | Пик VRAM | Инференс | Загрузка | Запрос |
|---|---|---|---|---|---|---|---|---|
| `none` (устройство не задано) | — | движок | **GPU** | LOADED | 9 266 МиБ (+660) | ok | 2 010 мс | 311 мс |
| `csv_bridge_index` | `0` | движок | **GPU** | LOADED | 9 266 МиБ (+636) | ok | 1 848 мс | **253 мс** |
| `csv_bridge_index_plus1` | `1` | движок | **CPU** | LOADED | 8 630 МиБ (+0) | ok | 1 869 мс | **4 282 мс** |
| `csv_device_name` | `CUDA0` | движок | failed_create | — | — | — | — | — |
| `csv_bridge_index_strict` | `0` | `false` | **GPU** | LOADED | 9 266 МиБ (+636) | ok | 1 959 мс | 473 мс |

Устройства движка (`list_devices`): `index=0 backend=CUDA name=CUDA0` (free 11 255 МиБ)
и `index=1 backend=CPU name=CPU`. NVML на том же прогоне: free **3 508** МиБ.

**Выводы (закрывают «осталось выяснить» п.1 из §9 плана W2):**

1. **Устройство задаётся числовым `manual_devices_csv` = bridge-индекс устройства**:
   `"0"` → CUDA0 (VRAM +636 МиБ, инференс ×17 быстрее CPU), `"1"` → CPU (VRAM +0,
   4 282 мс против 253 мс).
2. **Имя устройства не принимается**: `manual_devices_csv = "CUDA0"` → `rc=-1`,
   `last_error = "manual device selection is no longer available"`. Это и объясняет
   провал W0: через `example-cli` работал `--devices CUDA0` (bridge-API), а индекс
   `--gpu 1` не помогал — в cluster API адресация **только** индексами.
3. **`gpu` не задан ⇒ CPU-only** — на проверенной сборке **не так**: вариант `none`
   тоже ушёл на GPU (группа по умолчанию включает CUDA). Поэтому в продакшене
   устройство задаём **всегда явно** (`gpu.device_index`), а `0` (CPU) превращаем в
   CSV с индексом CPU-устройства, а не в «не задавать».
4. Соответствие нашему конфигу: **`gpu.device_index = 1` → `manual_devices_csv="0"`**
   (первый ускоритель), `gpu.device_index = 0` → CSV с индексом CPU-устройства.
   Формула живёт в `hds_llama::device::selection_from_config_index`.
5. `allow_cpu = false` не мешает загрузке на GPU и **обязателен** в продакшене:
   он превращает «молчаливый откат на CPU» в явную ошибку (проверено вариантом
   `csv_bridge_index_strict`).

### 1.3. Находки, которых не было в плане (важны для `llm-host`)

1. **Движок грузит ggml-бэкенды относительно ТЕКУЩЕГО каталога процесса.** Первый
   прогон из корня репозитория: библиотека загрузилась, но `list_devices` вернул
   **пустой список**, а в stderr появился только `load_backend: loaded RPC backend`
   (`ggml-cuda.dll` не подхватился) — то есть инференс молча ушёл бы на CPU при
   `n_gpu_layers = -1` (корень R32). Тот же бинарь с текущим каталогом `Engine`
   (проверено отдельно: exe при этом лежал в `target/release`, то есть дело именно
   в cwd, а не в каталоге exe) вернул `CUDA0` + `CPU`. Решение:
   `Engine::activate() -> EngineCwd` (RAII: переключает cwd и возвращает прежний при
   `Drop`). **Требование к `llm-host`: держать текущим каталог движка весь процесс**
   и работать только с абсолютными путями (sidecar-воркеру пути передаём абсолютные).
2. **Каталог движка не самодостаточен: нужны вендорские каталоги.** `LoadLibraryExW`
   падал во всех четырёх режимах с кодом **126** (`ERROR_MOD_NOT_FOUND`), потому что
   цепочка `multi-node-server.dll` → `llama-server-bridge.dll` → `llama-server-audio.dll`
   статически требует `avcodec-62.dll`/`avformat-62.dll`/`avutil-60.dll`/`swresample-6.dll`,
   а лежат они в `Engine\vendor\ffmpeg\bin`. Решение: `engine_dir::dll_search_dirs`
   добавляет `Engine`, `Engine\vendor\*\bin` и `Engine\vendor\*` через `AddDllDirectory`
   (+ `SetDefaultDllDirectories(DEFAULT_DIRS|USER_DIRS)`: в этом режиме
   `SetDllDirectoryW` **игнорируется** и пути не даёт). Шаги логируются (`load_notes`),
   ошибка загрузки печатает диагностику по режимам `LoadLibraryExW` — «LoadLibraryExW
   failed» больше не требует отдельного разбора.
3. **R29 переподтверждён на instance-пути**: `list_devices` сообщил free **11 255 МиБ**
   против реальных **3 508 МиБ** по NVML (расхождение **+7 747 МиБ** при нулевом
   дрейфе `nvidia-smi`). Бюджет VRAM — только NVML; `memory_free` движка годится
   лишь как контрольная точка.
4. **Метрики embeddings не заполняют счётчики токенов** (`prompt_tokens = 0`,
   `prompt_tokens_per_second = 0`), в отличие от чата: в отчёте видно только
   `request_total_ms` (224–284 мс на GPU против 2 034 мс на CPU). Для критерия A-2
   (паритет эмбеддингов) брать косинус по JSON и латентность, а не токены/с.
5. Загрузка модели в этом окне: **1,85–2,0 с** (bge-m3-Q8_0, 605 МБ) независимо от
   устройства; разница CPU/GPU видна на инференсе (×17) и в VRAM (+636 МиБ).
   Замеры сняты при занятых 8,5 ГБ VRAM — абсолютные пики не эталон, эталон — дельта.

### 1.4. Что изменить в критериях/плане по итогам A1

* **A-1** дополнить: `hdsw llm-host status --json` обязан показывать не только
  `devices: ["CUDA0"]`/`n_gpu_layers: 99`, но и `engine_cwd` (каталог движка текущий)
  и `load_notes` (какие каталоги добавлены в путь поиска DLL) — без них GPU-путь
  недостижим, а причина не видна.
* Требование «все инстансы создаются с явным `manual_devices_csv`» (§3 плана W2)
  уточняется: значение — **bridge-индекс**, а `gpu.device_index = 0` даёт индекс
  CPU-устройства (не «пусто»).
* В §11.3 плана (правила устройств) зафиксированное документацией «`gpu` не задан
  ⇒ CPU-only» для **cluster** API на проверенной сборке не подтвердилось — оставляем
  как расхождение документации движка, а не как основание для дефолта.

## 2. B1 — обход, исключения, лимиты (`crates/hds-index`)

### 2.1. Что сделано

| Файл | Содержимое |
|---|---|
| `kinds.rs` | таблицы расширений → вид файла (порт `extractors.py`/`extract_static.py`/`extract_av.py`, порядок `_kind_of`), `ext_of` как `os.path.splitext`, `~$`-локи Office, виды медиа |
| `walk.rs` | `IndexLimits` (`_limit_mb`: `max_media_mb` для медиа), `normalize_path` (порт `_norm_path` + `os.path.normpath`), `Excludes` (`exclude_dirs` по имени, `exclude_paths` по границе компонента), `walk_files` (порт `iter_files`: `[scan]`/`[skip]`, отсечение поддеревьев, симлинки, ошибки доступа), `FileFilter::precheck` (вид → exclude → `~$` → лимит) |

`walk_files` отдаёт **все** файлы (как Python-генератор), фильтрация — отдельным
шагом (`FileFilter`): так же, как `iter_files` и предполётные проверки `_extract_file`
в Python-версии. `stat` делает вызывающий (в Python ошибка stat даёт статус
`stat_error` — этот путь появится в конвейере B4).

### 2.2. Паритет (Python-эталон против Rust)

Инструмент: `tools/parity/walk_parity.py` (пишет `out/walk_parity.json`: корни,
исключения, лимиты, список файлов от `hds.indexer.iter_files` и карта решений
`_extract_file`). Тесты: `crates/hds-index/tests/walk_parity.rs`.

| Сценарий | Что покрывает | Результат |
|---|---|---|
| `synthetic` (собирается скриптом в `out/walk_tree`) | `exclude_dirs` по имени в другом регистре (`NODE_MODULES`), `exclude_paths` по границе компонента (`backup` исключён, `backup2` — нет), `~$`-лок, нетиповой вид, лимит обычного файла и медиа, каталог с исключённым именем на глубине | **14/14** файлов, все решения `precheck` совпали |
| `real` (боевой `D:\` из `config.yaml`: 2 исключённых префикса + 22 каталога по имени) | реальные имена (кириллица, скобки, «лишние» точки), длинные пути | **96 318/96 318** файлов за **5,2 с** (Python — 5,1 с), решения `precheck` совпали |

Гейт: сценарий `real` идёт только при `HDS_WALK_PARITY_REAL=1` (обход всего диска
не нужен в каждом прогоне):

```powershell
.\.venv\Scripts\python.exe tools\parity\walk_parity.py --real
$env:HDS_WALK_PARITY_REAL='1'; cargo test -p hds-index --test walk_parity -- --nocapture
```

**Два настоящих расхождения, найденных паритетом:**

1. `ext_of` для имён вида `..pdf` (реальный файл `D:\…\объявления\..pdf`): CPython
   игнорирует ведущие точки (`splitext("..pdf") == ("..pdf", "")`), первая версия
   порта возвращала `".pdf"` → файл попадал в вид `pdf` вместо «нетиповой».
   Исправлено в `kinds::ext_of`, покрыто сценарием `real`.
2. **Осознанное расхождение** (зафиксировано, не исправляем): 3 файла с путями
   262–272 символа (> MAX_PATH). Python `os.path.getsize` падает (`WinError 3`) →
   в эталоне `SkippedStat`, Rust `fs::metadata` (расширенные пути `\\?\`) работает
   → файл проходит в пайплайн. Для B4 это значит: записей со
   `status='error', error='stat_error…'` в Rust-версии может не быть — Rust
   индексирует то, что Python не смог даже открыть. В тесте такие пути
   пропускаются со счётчиком, в B4 отражаем в отчёте сравнения.

## 3. B2 — `content_hash` (`crates/hds-index/src/hash.rs`)

Порт `hds/indexer.py:content_hash` в `hds_index::hash`: `Blake2b<U16>` (BLAKE2b с
параметром длины 16, не усечение), `str(size)` + голова/хвост по 256 КБ, короткие
чтения. Плюс `hash_of_parts(size, head, tail)` — для конвейера B4 (файл уже прочитан)
и для тестов.

Инструмент: `tools/parity/hash_vectors.py` → `out/hash_vectors.json` — байтовые
векторы `content[i] = i % 251` длин `0, 1, 1024, 256К−1, 256К, 256К+1, 512К−1, 512К,
768К+7` и их Python-хэши (покрывают все ветки: файл короче окна, ровно окно,
перекрытие головы и хвоста).

| Проверка | Результат |
|---|---|
| Фиксированные векторы (9 значений вписаны в тест, работает без `out/`) | **9/9** |
| `out/hash_vectors.json` (расширяемая проверка, файл есть — гоняем) | ok |
| 50 реальных файлов (`spike2_hash.py`, 23 крупнее 512 КБ; эталон переснят прямо перед прогоном) | **50/50** ⇒ переиндексация БД не требуется (R28 закрыт) |

**Найденный баг порта (тест поймал, исправлено):** первая версия читала хвост файла
в **тот же буфер**, что и голову, и передавала в хэш уже перезаписанные байты —
расхождение с Python ровно на векторе `256 КБ + 1` (`62ae69…` против `4f9cd1…`).
Именно для этого в плане (§3.1 п.2) и требовались фиксированные векторы.

```powershell
.\.venv\Scripts\python.exe tools\parity\hash_vectors.py
cargo test -p hds-index --test hash_parity -- --nocapture
.\.venv\Scripts\python.exe tools\parity\spike2_hash.py
cargo test -p hds-index --test hash_parity -- --ignored --nocapture   # 50/50
```

## 4. Состояние и следующий шаг

* Тесты: `cargo test --workspace` — 6 проверок green (hash 4 + walk 2), тяжёлый
  паритет 50 файлов и сценарий `real` — по флагу/`#[ignore]`.
* Артефакты A1/B1/B2: `crates/hds-index`, `crates/hds-llama`, `tools/parity/walk_parity.py`,
  `tools/parity/hash_vectors.py`, `tools/parity/W2_REPORT.md` (этот файл);
  сырые отчёты локально — `tools/parity/out/w2_a1_device.json`,
  `out/walk_parity.json`, `out/hash_vectors.json`, `out/hash_parity.jsonl`
  (каталог `out/` в git не коммитится, как и в W0).
* Следующее по графику (§6 плана W2): **A2** — реестр инстансов и маппинг
  `llm_server.*`/`llm.*`/`gpu.*` → `instance_params` (с учётом выводов §1.2–1.4),
  **B3** — чанкер (дословный порт `hds/chunker.py`, паритет по `*.chunks.json`).
* Открытые вопросы для A2: подтвердить на chat-модели, что `reasoning=off` без
  блоков размышлений работает через cluster-инстанс (в W0 проверялось через
  bridge-API), и решить, нужен ли `hdsw llm-host` собственный `EngineCwd`-лог.


