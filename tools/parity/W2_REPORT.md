# W2_REPORT.md — журнал волны W2 (трек A: `llm-host`, трек B: ядро)

> Ветка `w2-llm-host`, дата 30.09.2026, рабочая станция: Windows, RTX 3060 12 ГБ
> (занято штатными ролями 8,5–8,6 ГБ на момент замеров), Ryzen 7 5700G, 64 ГБ RAM,
> rustc/cargo 1.97.1. План: `PLAN_W2_LLM_HOST.md`; факты W0 — `SPIKES.md`.
> Правило волны: Python-версия — источник истины по поведению, паритет проверяется
> скриптами и тестами, а не «на глаз».
>
> **Статус на 30.09.2026:** закрыты **A1** (обвязка движка + выбор устройства),
> **A2** (реестр инстансов и маппинг конфига), **A3** (адресация клиентов — замером),
> **A4 шаг 1** (бюджет VRAM) и **B1, B2, B3** (обход/лимиты, `content_hash`, чанкер).
> Дальше — A4 шаг 2 (вытеснение/`index.pause`), A5 (фасад), B4 (конвейер), B6 (sidecar).
> **Новому чату: §9 «Передача в новый чат»** (состояние, коммиты, карта кода, команды,
> открытые вопросы, грабли).

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

Дампы: `out/walk_parity_synthetic.json` (маленький, коммитится — быстрый тест
доступен всем) и `out/walk_parity_real.json` (тяжёлый, только локально, в git не идёт).

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

## 4. B3 — чанкер (`crates/hds-index/src/chunker.rs`)

Дословный порт `hds/chunker.py`: рекурсивное разбиение «абзац → строка →
предложение → слово», перекрытие **целыми предложениями**, запрет смешивать
сегменты с разными `page`/`t_start`/`t_end` и разные секции (`head` идёт в начало
каждого чанка секции). Ключевая деталь порта: Python считает **символы**
(`len(str)`), а Rust — байты, поэтому все длины в порте — `chars().count()`
(смещения срезов при этом байтовые: разделители ASCII, куски те же).

| Проверка | Результат |
|---|---|
| Golden-паритет (`tests/chunker_parity.rs`, 16 фикстур) | **16/16 ok, 6 363 чанка** — совпадение пополе (текст/`page`/`t_start`/`t_end`) |
| Числа манифеста (`n_segments`, `n_chunks`, `cut`) | совпали, включая обрезку `max_chunks=3000`: «большой_реестр» 3000 + cut 3778, «журнал_обработки» 3000 + cut 436 |
| Крупные фикстуры (`.json.gz`) | читаются тем же тестом (`flate2`), т.е. паритет покрывает и 8,8 МБ csv |
| Модульные тесты (не требуют golden) | 6 проверок: пустые сегменты, `head` в каждом чанке секции, смена метаданных/секции, перекрытие хвостом, длинное «слово» режется по символам (800/800/400 — поведение Python) |

```powershell
cargo test -p hds-index --test chunker_parity -- --nocapture   # 16/16, 6363 чанка
```

Осознанное расхождение (задокументировано в модуле): `\s` в Python-регулярке
покрывает также `\x1c`–`\x1f` и `\x85`; Rust `char::is_whitespace` их не включает.
На 16 фикстурах расхождения нет (такие символы в документах не встречаются).

## 5. A2 — реестр инстансов и маппинг конфига

Что сделано (`crates/hds-llama`):

| Модуль | Что внутри |
|---|---|
| `runtime.rs` | порт `hds/llama_runtime.py`: `runtime_dir` (env → `%LOCALAPPDATA%\llama-runtime` → macOS/XDG), `models_dir`, `current_file`, `read_current`, `resolve_model` (`shared:<role>`, регистр в манифесте, единственный GGUF роли как активный), `RuntimePaths` |
| `config.rs` | `config.yaml` через `serde_yaml`: `llm_server.*` (legacy) + `llm.*` + `gpu.*`; `n_ctx = parallel * ctx_per_slot` (как было у llama-server); `extra_args` разбираются (`-ngl`, `--batch-size`, `--ubatch-size`, `-t`), остальное — в `warnings` для `hdsw check`; `retention` строкой; `llm_server.mode`, `llm.model_policy` |
| `registry.rs` | `plan()` → по роли `RolePlan::Ready/Failed` (ошибки не блокируют другие роли, как в Python), `plan_strict()`; устройство — `gpu.device_index` → **числовой** CSV; `-ngl 0` → явная CPU-роль (`allow_cpu = true`), иначе GPU-роль с `allow_cpu = false` |
| `bin/llm_host_plan` | сухой прогон: конфиг → модель → устройство → параметры, без загрузки моделей; `--json` для приёмки |

Проверки: `cargo test -p hds-llama` — 8 green (2 модульных в `config.rs` + 6 в
`tests/registry_plan.rs`): резолвинг `shared:<role>`, приоритет новых ключей,
`gpu.device_index = 0` → индекс CPU-устройства, legacy `-ngl 0` → CPU-роль с
пояснением, предупреждения про `--cache-type-k`, негативный кейс «модели нет»
(сообщение содержит путь и подсказку об установщике).

Прогон по боевому `config.yaml` машины (артефакт `out/w2_a2_plan.json`):

| Роль | Модель (общий рантайм) | Устройство | n_ctx | ngl | allow_cpu | retention |
|---|---|---|---|---|---|---|
| chat | `…\llama-runtime\models\chat\Qwen3.5-9B-Q6_K.gguf` | CUDA0 (`"0"`) | 32768 | 99 (legacy) | false | KEEP_LOADED |
| embedding | `…\models\embedding\bge-m3-Q8_0.gguf` | CUDA0 (`"0"`) | 8192 | 99 (legacy) | false | LOAD_ON_DEMAND |
| rerank | `…\models\rerank\bge-reranker-v2-m3-q8_0.gguf` | CPU (`"1"`) | 8192 | `-ngl 0` → CPU | true | LOAD_ON_DEMAND |

Это и есть проверка дефекта `SPIKES.md` §14.7: чат-модель найдена в **общем
рантайме** (`%LOCALAPPDATA%\llama-runtime`), а не в каталоге проекта. Предупреждения
прогона: `--cache-type-k/--cache-type-v` не поддерживаются в W2 (перенести в
`llm.chat.*`/`gpu.*`), `-ngl` взят из legacy — перенести в `gpu.n_gpu_layers`.

```powershell
cargo test -p hds-llama                                   # 8 проверок A2
cargo run -p hds-llama --release --bin llm_host_plan -- --json tools\parity\out\w2_a2_plan.json
```

## 6. A3 — адресация клиентов (кросс-процессная проверка)

Вопрос: если инстанс создан в процессе A, видит ли его процесс B? От этого зависит,
нужно ли клиентам (`index/watch`, `mcp-http`, `ui`) ходить напрямую в инстансы или
только через наш фасад.

Измерение (`bin/a3_instance_probe`, два процесса, 30.09.2026):

| Процесс | Что делал | Результат |
|---|---|---|
| A (pid 20132) | `--hold 45`: создал и загрузил embedding-инстанс `a3_hold` (bge-m3, CUDA0) | `id=1`, `state=LOADED` (в stderr движка — загрузка на CUDA0) |
| B (pid 45440) | `--list` (свой кластер, свой процесс) | `list_instances: 0`; `find_instance_by_name("a3_hold") → НЕ НАЙДЕН` |
| B (pid 43376) | `--call a3_hold` | инстанс не найден → вызов невозможен |

**Вывод (закрывает риск W2-2):** состояние инстансов **принадлежит процессу** —
`list_instances`/`find_instance_by_name` видят только свои инстансы, `instance_id`
нумеруется в пределах процесса. Кросс-процессной адресации через cluster API нет,
и это не лечится флагом. Значит:

* **гарантированный путь клиентов — фасад `llm-host`** (`:8010` chat/chat-think,
  `:8011` embeddings, `:8012` rerank, `:health`, `/props`), как и предусмотрено
  планом (§A5, W2-2);
* альтернатива «клиенты ходят в чужие инстансы» отпадает — не тратить на неё время;
* адресация внутри `llm-host` — **по имени** (`find_instance_by_name`) в границах
  одного процесса-владельца.

```powershell
# воспроизведение (два окна или Start-Process)
cargo run -p hds-llama --release --bin a3_instance_probe -- --hold 45
cargo run -p hds-llama --release --bin a3_instance_probe -- --list
cargo run -p hds-llama --release --bin a3_instance_probe -- --call a3_hold
```

## 7. A4 (шаг 1) — бюджет VRAM: метаданные GGUF и оценка «модель + KV»

Диспетчеру VRAM нужно до загрузки знать потребность роли. Сделано:

| Модуль | Что внутри |
|---|---|
| `gguf.rs` | минимальный читатель метаданных GGUF (magic/версия/KV-пары, пропуск токенизаторов через `seek`); ключи сверяются **по суффиксу**, потому что префикс — архитектура: у bge-m3 это `bert.*`, у чат-модели `qwen35.*` (первая версия искала только `llama.*` и «не находила» параметров); читаются слои, головы, головы KV, `key_length`/`value_length`, `attention.causal` |
| `budget.rs` | `kv_cache_mib` (классическая формула llama.cpp), `estimate_need_mib` (файл + KV + 5 % оверхеда), `check_fit`/`Fit` — вердикт с **точными цифрами** «нужно/доступно/недостаёт» и без авто-деградации |
| `bin/vram_budget` | прогон по ролям конфига: размер файла, параметры модели, KV для f16 и q8_0, потребность, свободная VRAM (NVML), вердикт; `--json` для отчёта |

Калибровка (пункт честности — оценка обязана совпадать с замером):

| Модель | Оценка | Факт | Комментарий |
|---|---|---|---|
| bge-m3 (энкодер) | файл 605 МиБ + 5 % = **636 МиБ** | замер A1: рост VRAM **+636 МиБ** | KV-кэша нет (`attention.causal = false`) — совпадение ±1 % |
| bge-reranker-v2-m3 | 606 + 5 % = 637 МиБ | — | тоже энкодер (`causal = false`) |
| Qwen3.5-9B-Q6_K | KV при 32768: **f16 4096 МиБ / q8_0 2176 МиБ** | требует замера (A4 шаг 2) | фиксируем вход: 32 слоя, 4 головы KV, head_dim 256 |

Прогон по боевому конфигу (`out/w2_a4_budget.json`, свободно 3469 МиБ, резерв 1024 МиБ):

| Роль | Файл | n_ctx | KV f16 | Нужно | Вердикт |
|---|---|---|---|---|---|
| chat | 7112 МиБ | 32768 | 4096 МиБ | **11564 МиБ** | **не хватает** — недостаёт 9119 МиБ |
| embedding | 605 МиБ | 8192 | 0 | 636 МиБ | влезает |
| rerank | 606 МиБ | 8192 | 0 | 637 МиБ | влезает |

**Следствие для W2 (важное, требует решения заказчика):** чат-модель при
`n_ctx = 32768` и **f16**-KV не влезает в 12 ГБ даже на пустой карте (11,5 ГБ +
системный оверхед). Прежний llama-server работал потому, что его запускали с
`--cache-type-k/v q8_0` (KV 2176 МиБ → ≈9,6 ГБ), а **cluster API ручки типа KV не
имеет** (`kv_unified=1`, `no_kv_offload=0`, `cache-type-*` нет в `instance_params`).
Варианты (оба — за пользователем, авто-деградации нет): измерить фактический KV
движка при загрузке (A4 шаг 2) или задать `llm.chat.n_ctx: 16384`
(KV f16 2048 МиБ → ≈9,5 ГБ, влезает).

```powershell
cargo test -p hds-llama                                    # включает калибровку VRAM
cargo run -p hds-llama --release --bin vram_budget -- --json tools\parity\out\w2_a4_budget.json
```

## 8. Состояние и следующий шаг

* Тесты: `cargo test --workspace` — **25 проверок green** (chunker 6 + chunker-parity 1
  + hash 4 + walk 2 + config 2 + gguf 2 + registry 6 + VRAM-калибровка 2), тяжёлые
  (паритет 50 файлов, сценарий `real`) — по флагу/`#[ignore]`.
* Артефакты W2 (шаги 1–7): `crates/hds-index` (kinds/walk/hash/chunker),
  `crates/hds-llama` (обвязка движка, `a1_device_probe`, `runtime`/`config`/`registry`,
  `llm_host_plan`, `a3_instance_probe`, `gguf`/`budget` + `vram_budget`),
  `tools/parity/walk_parity.py`, `tools/parity/hash_vectors.py`,
  `tools/parity/W2_REPORT.md` (этот файл); в git из `out/` идут только маленькие
  эталоны и отчёты (`walk_parity_synthetic.json`, `hash_vectors.json`,
  `w2_a1_device.json`, `w2_a2_plan.json`, `w2_a4_budget.json`).
* Риски: **W2-2 закрыт замером** (кросс-процессной адресации нет → фасад обязателен);
  R29/R32 подтверждены повторно (§1.2–1.3); R28 (хэш) закрыт паритетом 50/50.
  Новый вопрос к заказчику: `llm.chat.n_ctx` при f16-KV (§7) — 32768 не влезает
  в 12 ГБ, нужен либо `16384`, либо замер фактического KV движка.
* Следующее: **A4 шаг 2** — вытеснение по `gpu.priorities`, `index.pause` при
  нехватке VRAM, наблюдаемость в `hdsw llm-host status`; **A5** — фасад `:8010–8012`
  (формат как у llama-server); **B4** — конвейер `process_file` (фазы, атомарный
  коммит на файл, `clip_for_embedding`, `max_chunks`, прогресс и heartbeat).

## 9. Передача в новый чат (состояние W2 на 30.09.2026)

### 9.1. Где мы

| Коммит | Что сделано | Проверки |
|---|---|---|
| `154c298` | **A1** обвязка движка (`libloading`, путь поиска DLL, `EngineCwd`, cluster-обёртка) + проба `a1_device_probe`; **B1** обход/исключения/лимиты; **B2** `content_hash` | 14/14 синтетика, 96 318/96 318 боевой `D:\`, 9 векторов + 50/50 хэш |
| `7b32b14` | **B3** чанкер (дословный порт) | 16/16 фикстур, 6 363 чанка golden |
| `7b8c36e` | **A2** `runtime`/`config`/`registry` + `llm_host_plan` | 8 тестов + прогон по боевому `config.yaml` |
| `efe1168` | **A3** проба кросс-процессной адресации | замер: адресации нет → фасад обязателен (W2-2 закрыт) |
| `3418ade` | **A4 шаг 1** GGUF-метаданные + бюджет VRAM + `vram_budget` | калибровка bge-m3: 636 МиБ против замера +636 МиБ |

Ветка `w2-llm-host` (5 коммитов), `main` и боевой индекс **не тронуты**,
Python-версия продолжает работать. `cargo test --workspace` — **25 проверок green**
(+ `#[ignore]`-паритет 50 файлов и сценарий `real` по флагу `HDS_WALK_PARITY_REAL=1`).

### 9.2. Статус задач W2

| Задача | Статус | Остаток |
|---|---|---|
| A1 обвязка движка | ✅ | — |
| A2 реестр инстансов и маппинг конфига | ✅ | подкоманды `hdsw llm-host devices/status` (войдут с CLI, A6/B7) |
| A3 кросс-процессная адресация | ✅ | — (вывод: фасад обязателен) |
| A4 диспетчер VRAM | 🚧 шаг 1/3 | вытеснение по `gpu.priorities`, `index.pause` при нехватке, наблюдаемость/статус |
| A5 фасад `:8010–8012` | ⏳ | `/health`, `/props`, `chat`/`chat-think` (thinking-маппинг §11.4), embeddings, rerank |
| A6 значения по умолчанию и совместимость | ⏳ | дефолты конфига, `llm_server.mode`, миграция установки |
| B1 обход/лимиты/exclude | ✅ | — |
| B2 `content_hash` | ✅ | — |
| B3 чанкер | ✅ | — |
| B4 конвейер `process_file` | ⏳ | фазы, атомарный коммит, `clip_for_embedding`, `max_chunks`, прогресс+heartbeat |
| B5 watcher (`notify`) | ⏳ | debounce, `watch.lock`, reconcile, rename/удаление |
| B6 sidecar-клиент + Python-воркер | ⏳ | контракт из спайка 3 (§5 основного плана) |
| B7 `db-move` и подкоманды CLI | ⏳ | `check/reindex/reindex-fts/forget/stop/clip-index/status` |

### 9.3. Карта кода W2

| Путь | Что |
|---|---|
| `Cargo.toml` | воркспейс: `members = crates/*`, `exclude = tools/parity/spikes` (спайки W0 живут отдельно) |
| `crates/hds-index/src/kinds.rs` | таблицы расширений → вид файла, `ext_of` как `os.path.splitext`, `~$`-локи |
| `crates/hds-index/src/walk.rs` | `walk_files` (порт `iter_files`), `Excludes` (`_norm_path`), `IndexLimits`, `FileFilter::precheck` |
| `crates/hds-index/src/hash.rs` | `content_hash` (Blake2b-16), `hash_of_parts` |
| `crates/hds-index/src/chunker.rs` | `make_chunks` (порт `hds/chunker.py`), `Segment`/`Chunk` |
| `crates/hds-index/tests/*` | `hash_parity`, `walk_parity` (паритет с Python), `chunker_parity` (golden) |
| `crates/hds-llama/src/engine_dir.rs` | поиск каталога движка, `dll_search_dirs` (вендорские каталоги), имя библиотеки |
| `crates/hds-llama/src/engine.rs` | `ClusterApi::load` (`libloading` + `AddDllDirectory`), `EngineCwd`, `diagnose_load` |
| `crates/hds-llama/src/ffi.rs` | структуры/enum'ы cluster API (по SDK движка) |
| `crates/hds-llama/src/cluster.rs` | `Cluster`: devices/instances/create/load/unload/embeddings/rerank/chat |
| `crates/hds-llama/src/device.rs` | `gpu.device_index` → `manual_devices_csv` (числовой!), `cpu_device` |
| `crates/hds-llama/src/vram.rs` | NVML-проба, `VramSampler`, фолбэк по `list_devices` |
| `crates/hds-llama/src/runtime.rs` | общий llama-рантайм: пути, `current.json`, `shared:<role>` |
| `crates/hds-llama/src/config.rs` | `config.yaml` → роли/GpuConfig/warnings (`extra_args` разбираются) |
| `crates/hds-llama/src/registry.rs` | `plan`/`plan_strict`, `RolePlan::Ready/Failed`, `PlannedInstance` |
| `crates/hds-llama/src/gguf.rs` | метаданные GGUF (ключи по суффиксу: `bert.*`, `qwen35.*`) |
| `crates/hds-llama/src/budget.rs` | `kv_cache_mib`, `estimate_need_mib`, `check_fit`/`Fit` |
| `crates/hds-llama/src/bin/*` | `a1_device_probe`, `llm_host_plan`, `a3_instance_probe`, `vram_budget` |
| `tools/parity/W2_REPORT.md` | **этот журнал** (числа, находки, команды) |

### 9.4. Команды проверки (copy-paste, из корня репозитория)

```powershell
# --- базовые проверки ---
cargo test --workspace                                # 25 проверок (быстрые)
cargo test -p hds-index --test chunker_parity -- --nocapture   # golden 16/16, 6363 чанка
.\.venv\Scripts\python.exe tools\parity\spike2_hash.py
cargo test -p hds-index --test hash_parity -- --ignored --nocapture   # 50/50 (переснимите эталон)
.\.venv\Scripts\python.exe tools\parity\walk_parity.py --real
$env:HDS_WALK_PARITY_REAL='1'; cargo test -p hds-index --test walk_parity -- --nocapture

# --- трек A (движок; нужен движок в %APPDATA% и модели в общем рантайме) ---
cargo run -p hds-llama --release --bin llm_host_plan -- --json tools\parity\out\w2_a2_plan.json
cargo run -p hds-llama --release --bin vram_budget   -- --json tools\parity\out\w2_a4_budget.json
cargo run -p hds-llama --release --bin a1_device_probe            # 5 вариантов устройства, ~2 мин
cargo run -p hds-llama --release --bin a3_instance_probe -- --hold 45   # второй процесс: --list
```

### 9.5. Открытые вопросы и решения

* **Ждёт заказчика:** `llm.chat.n_ctx` — при `n_ctx = 32768` и f16-KV чат требует
  ~11,5 ГБ (не влезает в 12 ГБ), у cluster API нет ручки типа KV (§7).
* **Решено и не переоткрывать:** без авто-деградации кванта; llama-server удаляется
  в конце W2; кросс-процессной адресации нет → фасад обязателен; устройство — числовым
  `manual_devices_csv`; cwd движка + вендорские каталоги обязательны; бюджет VRAM — по NVML.
* **Осталось измерить (A4 шаг 2):** фактический KV чат-модели при загрузке (нужно окно
  со свободной VRAM) — сравнить с оценкой `vram_budget`.

### 9.6. Правило паритета (как проверять новые куски)

1. Python-версия — источник истины. Для новой части сначала снять эталон
   Python-скриптом в `tools/parity/` (по образцу `walk_parity.py`, `hash_vectors.py`,
   `golden.py`) → `tools/parity/out/*.json`.
2. В Rust-тесте сравнивать **строго** (пополе/множествами), а не «похоже»: golden-файлы
   для чанкера, дампы для обхода, фиксированные векторы для хэшей.
3. Волатильные файлы (логи, `index.heartbeat.json`) исключать или переснимать эталон
   прямо перед прогоном (иначе 48/50 вместо 50/50 — грабля W0 §4).
4. Найденные расхождения — либо баг порта (исправить), либо осознанное отличие
   (задокументировать в `W2_REPORT.md`, как длинные пути > 260 и `\s`-класс).
5. Осознанные отличия помечать в коде комментарием и в `W2_REPORT.md` рядом с цифрами.

### 9.7. Грабли W2 (повторяющиеся)

1. `Engine::activate()` меняет текущий каталог процесса: **относительные пути в
   аргументах бинарей приводить к абсолютным до активации** (`absolutize`), иначе файл
   отчёта уедет в каталог движка (поймано в A2).
2. Движок грузит ggml-бэкенды относительно cwd; без этого `list_devices` пуст и всё
   молча уходит на CPU при `n_gpu_layers = -1` (это и был «R32»).
3. Каталог движка не самодостаточен: `Engine\vendor\ffmpeg\bin` обязателен в пути
   поиска DLL, иначе `LoadLibraryExW` даёт код 126 (`diagnose_load` печатает режимы).
4. Устройство — **число** (`manual_devices_csv`), имя (`"CUDA0"`) движок отвергает.
5. Длинные пути (> 260 символов) Python не статит, Rust умеет — в паритете это
   осознанное расхождение, а не дефект (3 файла на боевом `D:\`).
6. `out/` в git: маленькие эталоны и отчёты коммитятся (`w2_*.json`,
   `walk_parity_synthetic.json`, `hash_vectors.json`), тяжёлое (боевой обход, `.gz`,
   бинарные векторы) — нет (см. `tools/parity/.gitignore`).
7. Тесты, которым нужны модели/диск, обязаны **пропускаться** без них (печатать
   `пропуск: …`), иначе `cargo test` ломается на чужой машине.


* Открытые вопросы: подтвердить на chat-модели, что `reasoning=off` без блоков
  размышлений работает через cluster-инстанс (в W0 проверялось через bridge-API);
  решить судьбу `rerank`-роли на CPU (`-ngl 0` в боевом конфиге) — переносить ли
  на GPU вместе с остальными ролями.


