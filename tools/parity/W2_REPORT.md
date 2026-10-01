# W2_REPORT.md — журнал волны W2 (трек A: `llm-host`, трек B: ядро)

> Ветка `w2-llm-host`, дата 30.09.2026, рабочая станция: Windows, RTX 3060 12 ГБ
> (занято штатными ролями 8,5–8,6 ГБ на момент замеров), Ryzen 7 5700G, 64 ГБ RAM,
> rustc/cargo 1.97.1. План: `PLAN_W2_LLM_HOST.md`; факты W0 — `SPIKES.md`.
> Правило волны: Python-версия — источник истины по поведению, паритет проверяется
> скриптами и тестами, а не «на глаз».
>
> **Статус на 01.10.2026 (после B7/B-2/B-4):** закрыты **A1–A6** (обвязка движка и
> устройство, реестр инстансов, адресация — замером, бюджет VRAM, диспетчер и замер KV,
> фасад `:8010–8012`, резидентный `llm-host`) и **B1–B7** (обход/лимиты, `content_hash`,
> чанкер, конвейер `process_file`, watcher, sidecar-воркер, db-move и подкоманды CLI).
> Ветка `w2-llm-host`, `cargo test --workspace` — **125 green (+5 `#[ignore]`)**;
> паритет golden 16/16 (§11), watcher — 6 сценариев (§12), sidecar — контракт §5 (§13);
> B7 — `hds`-CLI (§14); пилот 10 000 файлов — полный паритет, +7,1 % (§15);
> память — в допуске (§16). **Владельцем портов 8010–8012 стал `llm-host`** (§10.2a),
> Python-роли остановлены.
> Дальше по плану — **W3** (whisper/CLIP).
> **Новому чату:** §9 «Передача в новый чат» (состояние, карта кода, команды, грабли) →
> **§10 «A6»** (§10.6 живая машина, §10.7 грабли) → **§11 «B4»** → **§12 «B5»** →
> **§13 «B6»** → **§14 «B7»** → **§15 «B-2»** → **§16 «B-4»**.

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

**Следствие для W2 (важное, было решением заказчика — закрыто замером в §7.2):**
чат-модель при `n_ctx = 32768` и **f16**-KV, по этой (завышенной) оценке, не влезала в 12 ГБ
даже на пустой карте (11,5 ГБ + системный оверхед). Прежний llama-server работал потому, что
его запускали с `--cache-type-k/v q8_0` (KV 2176 МиБ → ≈9,6 ГБ), а **cluster API ручки типа KV
не имеет** (`kv_unified=1`, `no_kv_offload=0`, `cache-type-*` нет в `instance_params`).
Варианты (оба — за пользователем, авто-деградации нет): измерить фактический KV движка или
задать `llm.chat.n_ctx: 16384` (KV f16 2048 МиБ → ≈9,5 ГБ).
**Итог 30.09.2026: выбран замер; он показал, что модель гибридная и KV = 1024 МиБ —
`n_ctx: 32768` остаётся (см. §7.2).**

```powershell
cargo test -p hds-llama                                    # включает калибровку VRAM
cargo run -p hds-llama --release --bin vram_budget -- --json tools\parity\out\w2_a4_budget.json
```

### 7.1. A4 (шаг 2) — диспетчер VRAM: вытеснение по приоритетам, `index.pause`, `status`

Что сделано (то, что дальше позовёт резидентный `llm-host` (A5/A6)):

| Модуль | Что внутри |
|---|---|
| `config.rs` (`gpu.*`) | `gpu.policy` (`query_priority`/`indexing_priority`/`manual`), `gpu.pause_index_on_query`, `gpu.external_vram_mb`, `gpu.vram_source`; `effective_free_mib()` — `vram_budget_mb` **сужает** замер (ручка критерия A-7), `external_vram_mb` вычитается, `priority_of()` (роль вне списка = 0) |
| `vram.rs` | `VramSource::parse/as_config_key/is_trusted`: `gpu.vram_source: engine` даёт warning про R29, а не молчаливую подмену |
| `pause.rs` | `IndexPause`: файл `index.pause` создаётся **пустым** (как UI), счётчик вложенности для нескольких одновременных запросов, RAII-аренда (`PauseLease`), `read_heartbeat` (свежесть 30 с + флаг `paused`, R30), `stop_requested` |
| `dispatch.rs` | чистые решения `plan_query`/`plan_indexing`/`idle_evictions`/`eviction_order` + исполнение `apply` (кластер + шлюз паузы) |
| `status.rs` | `StatusReport::build/lines/json`: поля §A4 (`state/retention/active/queued/last_error`), бюджет, «NVML − baseline», пауза/heartbeat, прогноз диспетчера |
| `bin/llm_host_status` | наблюдаемость: устройства, NVML-бюджет, пауза и heartbeat, роли, **прогноз** «если запрос придёт сейчас», `--json` для UI |
| `bin/llm_host_dispatch` | прогон решения на живом движке: `--apply` (пауза → выгрузка → загрузка → снятие паузы), `--budget-mb` (сужение бюджета), `--kind query|indexing`, замер NVML до/после |

Правила решений (проверяются `tests/arbiter.rs`, 14 тестов — без движка и GPU):

| Ситуация | Решение |
|---|---|
| свободной VRAM хватает | ничего не трогаем (`Fits`), индексные роли не выгружаем |
| на запрос не хватает | `PauseIndex` → выгрузка `whisper → rerank → embedding` ровно до достатка → `FitsAfterEviction` |
| не хватает и после вытеснения | `ReportShortage` с цифрами (нужно/свободно/резерв/освобождено/недостаёт) и явной строкой «авто-деградации нет (`llm.model_policy: fixed`)» — `NotEnough` |
| замера VRAM нет | `Unknown`: не гадаем, вытеснение и пауза не выполняются |
| `gpu.policy: manual` | только отчёт, никаких автоматических действий |
| `gpu.policy: indexing_priority` | индексные роли не вытесняем, запрос ждёт |
| индексации не хватает | первым выгружается резидент (`chat`), index-роли не трогаем (ARB-3) |
| простой роли | `min(grace роли, gpu.evict_idle_sec)` (ARB-5) |
| инстанс занят запросом / чужой (`owned = false`) | не вытесняется никогда |

Живые прогоны на этой машине (артефакты — `out/w2_a4_*.json`):

| Прогон | Цифры |
|---|---|
| `llm_host_status --json out/w2_a4_status.json` | NVML: занято 8552 / 12288 МиБ, свободно 3562; чат: файл 7112 МиБ, 32 слоя, 4 головы KV, `n_ctx 32768`, KV f16 4096 → **нужно 11564 МиБ** (+резерв 1024); `index.pause` стоит (это пауза **пользователя**, вложенность 0), heartbeat отсутствует; прогноз запроса чата: `not_enough`, недостаёт **9026 МиБ** |
| `llm_host_dispatch --role chat --json out/w2_a4_dispatch.json` | вердикт `not_enough`, выгрузок 0 (своих инстансов в процессе нет — факт A3), отчёт в лог |
| `llm_host_dispatch --role chat --budget-mb 3000 --json out/w2_a4_dispatch_budget.json` | cap сузил бюджет: свободно **3000** МиБ → «недостаёт **9588** МиБ»; действия: пауза + обеспечить загрузку + отчёт (критерий A-7) |
| `llm_host_dispatch --role embedding --kind indexing --apply --json out/w2_a4_indexing.json` | вердикт `fits`: нужно 1660 МиБ (модель+KV 636 + резерв 1024), свободно 3562; пауза не ставится; чужой `index.pause` **не тронут**, NVML-дельта 0 |

**Находка живого прогона (регресс в собственном коде, исправлен):** `resume()` вызывался «на всякий
случай» после запроса и **удалял `index.pause`, поставленный пользователем** — то есть запрос молча
возобновлял индексацию, которую остановил человек (§8.6.2 запрещает именно это). Исправление:
удаляется только файл, **созданный нами** (флаг `ours_created`); вызов `resume()` без активной аренды —
no-op. Тесты: `resume_without_our_pause_keeps_user_file`, `lease_over_user_pause_keeps_it_until_force`
(`tests/pause_gate.rs`). Файл `index.pause` на машине восстановлен.

```powershell
# --- A4 шаг 2: диспетчер VRAM и наблюдаемость ---
cargo test -p hds-llama --test arbiter --test pause_gate --test status_report   # 14 + 6 + 2
cargo run -p hds-llama --release --bin llm_host_status -- --json tools\parity\out\w2_a4_status.json
cargo run -p hds-llama --release --bin llm_host_dispatch -- --role chat
cargo run -p hds-llama --release --bin llm_host_dispatch -- --role chat --budget-mb 3000
cargo run -p hds-llama --release --bin llm_host_dispatch -- --role embedding --kind indexing --apply
```

### 7.2. A4 — замер фактического KV движка (решение заказчика 30.09.2026 по `llm.chat.n_ctx`)

**Решение заказчика:** `n_ctx: 32768` остаётся, вопрос закрывается **замером** (не переходом на 16384).
Замер выполнен; попутно нашлась ошибка оценки — модель **гибридная**.

**Находка 1 (аудит метаданных, `bin/gguf_dump`):** `Qwen3.5-9B` — не обычный трансформер:
`qwen35.ssm.*` (state_size 128, conv_kernel 4, group_count 16, time_step_rank 32, inner_size 4096) и
**`qwen35.full_attention_interval = 4`**. Полное внимание — только в каждом 4-м слое, значит растущий
KV-кэш держат **8 слоёв из 32**, а не все: прежняя оценка KV (4096 МиБ) была завышена **в 4 раза**.
llama.cpp делает так же (`llama_hparams::has_kv(il)` через `n_layer_kv_from_start`; в отчётах по
Qwen3-Next: «only 12 of 48 layers enter the growing KV-cache calculation»).

**Исправление оценки (`gguf.rs`/`budget.rs`):** читаем `*.full_attention_interval`, считаем
`kv_layer_count() = block_count / interval` (и `kv_layer_count_within(offloaded)` — для частичного
офлоада). Проверка: `tests/budget_calibration.rs` и юнит-тест на синтетическом GGUF.

**Находка 2 (фактический KV, данные самого движка).** `bin/kv_probe` поднял кластерный инстанс
(`n_gpu_layers = 8`, `allow_cpu`), и движок напечатал в лог собственные размеры буферов:

| n_ctx | `llama_kv_cache: size` | слоёв KV | K (f16) | V (f16) | CPU KV | CUDA0 KV |
|---|---|---|---|---|---|---|
| 4096 | **128,00 МиБ** | 8 | 64,00 | 64,00 | 96,00 | 32,00 |
| 32768 | **1024,00 МиБ** | 8 | 512,00 | 512,00 | 768,00 | 256,00 |

Ровно совпадает с исправленной формулой (32768 × 8 слоёв × 4 головы × 512 × 2 Б = 1024 МиБ), и
`CUDA0 KV = 256 МиБ` — доля **двух** офлоаднутых full-attention слоёв (движок ведёт KV по слоям
устройства, как и предполагала предварительная проверка).

**Находка 3 (независимый замер NVML).** Дифференциальный замер (`kv_probe --ngl 8 --n-ctx 4096,32768`):
Δn_ctx 28672 → **ΔVRAM +224 МиБ** на 2 GPU-KV-слоях = **4,00 КиБ/токен/слой** — расхождение с
формулой **0,0 %**. Машинная занятость при этом: 4074 → 4298 МиБ. Экстраполяция на полный офлоад:
**KV ≈ 1024 МиБ**, всего «модель + KV + 5 %» ≈ **8492 МиБ** → в 12288 МиБ с резервом 1024 **влезает**
(вердикт `kv_probe` и `budget_calibration`).

Калибровка «веса» по тому же прогону: `ngl = 8` → рост 4074 МиБ ≈ CUDA0 model buffer 1998,52
(8/33 слоёв + output) + KV 64 + compute ≈1972 + RS 10 — сходится.

**Находка 4 (два фиксированных блока, которые оценка A4 не покрывает).** В том же логе:
`llama_memory_recurrent: size = 50,25 МиБ (1 cells, 32 layers)` — SSM-состояние гибридных слоёв
(**не растёт с `n_ctx`**) и `sched_reserve: CUDA0 compute buffer size = 1972,00 МиБ` при
`n_batch/n_ubatch = 2048`. Итого полный офлоад ≈ 8492 + 50 + 1972 ≈ **10,5 ГБ** — в 12 ГБ влезает,
но запас ~0,75 ГБ. Для A6: рассмотреть `n_batch` для чата (меньше `n_batch` → меньше compute-буфер)
и учесть эти блоки в `hdsw check`/UI. Отдельно замечено: движок сам зовёт `llama_params_fit`
(«fitting params to device memory»), т.е. при тесной карте может **сам** изменить офлоад — в A6 это
надо проверить и, при необходимости, запретить (`--fit off`-эквивалент), иначе «без авто-деградации»
нарушится со стороны движка.

**Грабли замера (в журнал):** NVML на Windows даёт **машинную** занятость (per-process VRAM нет),
поэтому при почти полной карте WDDM вытесняет чужие буферы и разница «не растёт» (первый прогон дал
−32 МиБ). В `kv_probe` введён порог шума 64 МиБ: ниже него дифференциал не считается, а фактический
KV берётся из данных движка (второй прогон на той же машине дал чистые +224 МиБ).

```powershell
# --- замер KV (A4): аудит метаданных и загрузка движком ---
cargo run -p hds-llama --release --bin gguf_dump -- --role chat --json tools\parity\out\w2_chat_meta.json
cargo run -p hds-llama --release --bin kv_probe  -- --role chat --ngl 8 --n-ctx 4096,32768 --json tools\parity\out\w2_kv_probe.json
cargo run -p hds-llama --release --bin vram_budget -- --json tools\parity\out\w2_a4_budget.json   # chat: KV 1024, нужно 8492
```

### 7.3. A5 — фасад `:8010–8012` (OpenAI-совместимый)

**Зачем фасад.** Замер A3 показал, что **кросс-процессной адресации инстансов нет**:
инстансы видит только создавший их процесс. Значит единственный гарантированный способ
обслуживать UI/MCP/внешних агентов — HTTP-фасад на тех же портах, что были у
`llama-server` (`:8010` чат, `:8011` эмбеддинги, `:8012` реранк).

**Разведка контракта чата (`bin/chat_probe`, артефакт `out/w2_chat_contract.json`)** —
замер, а не догадки. Кластерный `chat_complete` принимает **готовый `prompt`**, но движок
**сам применяет шаблон чата модели**:

| Вход | Токенов на входе (движок) | Ответ |
|---|---|---|
| «плоский» текст (system+user+слово `assistant`) | **56** | «4» |
| то же с маркерами ChatML (угл. скобки `im_start`) | **68** | «4» |
| ChatML + `reasoning=on, format=none` | 66 | 48 токенов «Thinking Process: …» |
| ChatML, `reasoning` не задан | 68 | «4» (без размышлений) |

Выводы: (1) шаблон накладывается поверх нашего текста (плоский текст вырос с ~35 «сырых»
токенов до 56), поэтому маркеры ставить **нельзя** — иначе модель видит их как обычный
текст; (2) `reasoning=off` через кластерный инстанс даёт ответ **без** размышлений —
это закрывает открытый вопрос §9.5 плана W2 (в W0 проверялось только через bridge-API);
(3) `reasoning=on` + `format=none` даёт видимые размышления — на этом работает `chat-think`.

**Что сделано:**

| Модуль | Что внутри |
|---|---|
| `src/http.rs` | свой минимальный HTTP/1.1 (свой — потому что `crates.io` на машине недоступен: `Could not resolve host: index.crates.io`): `Content-Length`, `Expect: 100-continue`, keep-alive, `Connection: close`, лимит тела 32 МБ, явный `411` на chunked-тело, поток на соединение, паника обработчика не роняет сервер |
| `src/facade.rs` | маршрутизация как у `llama-server` (`/health`, `/props`, `/v1/models`, `/v1/chat/completions`, `/v1/embeddings`, `/v1/rerank` — с `/v1` и без); сборка `prompt` из `messages` (system первым абзацем, история «Пользователь:/Ассистент:», последняя реплика как есть — для пары system+user вход совпадает с Python-RAG); режимы размышлений (`chat_template_kwargs.enable_thinking` → `reasoning` → алиас `chat-think` → конфиг); `strip_think` — порт `hds/rag.py::_strip_think`; ответы OpenAI (`choices[0].message.content`, `usage`, `reasoning_content` при включённых размышлениях); `trait Backend` + `handle()` + `serve()` |
| `bin/llm_host_facade` | живой фасад: инстансы по `registry::plan` (A2), диспетчер `dispatch` (A4) на каждом запросе (пауза+вытеснение+`PauseLease` до конца запроса), `--port-base` (проверки на альтернативных портах), `--ngl` (экономия VRAM), `--hold`, `--json` |
| `tools/parity/facade_smoke.ps1` | живая проверка одной командой (ASCII-only: PowerShell 5.1 читает .ps1 без BOM как ANSI и ломает кириллицу) |

**Живой прогон** (`facade_smoke.ps1 -PortBase 8020 -HoldSec 20`, чат на CPU, чтобы не
отбирать VRAM и не занимать порты 8010–8012 у Python-версии):

```
== /health ok
== /props: total_slots=1 n_ctx=32768 model_path=…\llama-runtime\models\chat\Qwen3.5-9B-Q6_K.gguf
== /v1/models: chat, chat-think, embedding, rerank
== chat (thinking off): 226 ms, answer: 4        (usage: prompt=29 completion=2)
== chat (chat-think): reasoning_content yes (103 chars)
== embeddings: 2 vectors, dim 1024
[dispatcher] вердикт not_enough: нужно 1660 МиБ …, свободно 303 МиБ
[dispatcher] выгрузить 'chat' (роль chat, ≈8492 МиБ): приоритет 100 (chat), retention keep
[dispatcher] index.pause уже стоял (пауза пользователя) — переиспользуем, снимать не будем
инстансы сняты: 3; index.pause сейчас: стоит     ← пауза пользователя не тронута
```

То есть в одном прогоне сошлось всё: фасад отвечает как `llama-server`, диспетчер при
нехватке VRAM выгружает роль по приоритету, `index.pause` пользователя **не** снимается,
а после прогона инстансы и пауза в исходном состоянии (`out/w2_facade.json`).

**Находка (грабля Windows, поймана тестом `tests/facade_http.rs`):** `accept()` от
**неблокирующего** слушателя отдаёт неблокирующий сокет, поэтому вторую строку запроса
читать было нельзя — соединение закрывалось сразу после первого ответа (keep-alive
не работал). Исправлено `set_nonblocking(false)` в обработчике соединения + закрытие
соединения по таймауту простоя. Тест оставлен как страж (`http_transport_handles_keep_alive_continue_and_chunked`).

**Ограничения A5 (осознанные, задокументированы в коде):** `stream=true` не поддерживается
(cluster API отдаёт ответ целиком) → честная 400; chunked-тело → `411 Length Required`;
TLS нет (фасад слушает только localhost); `/props` отдаёт `total_slots`/`model_path`/`n_ctx`
именно в той форме, по которой Python-версия (`hds/llama_server.py::probe`) считает
инстанс «своим», а UI показывает фактический контекст.

```powershell
# --- A5: фасад ---
cargo test -p hds-llama --test facade_core --test facade_http   # 10 + 3 (1 #[ignore] — ручная диагностика)
cargo run -p hds-llama --release --bin chat_probe -- --json tools\parity\out\w2_chat_contract.json
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\facade_smoke.ps1 -PortBase 8020 -HoldSec 20
```

**Один инстанс — два режима (требование §8.7 основного плана).** Внешний агент должен
получать чат **с размышлениями**, а MCP в тот же момент — **без**; у нас это один и тот же
инстанс `chat`, потому что `reasoning`/`reasoning_budget`/`reasoning_format` — **поля запроса**
(`struct llama_server_cluster_chat_request`), а не свойство инстанса. Доказательства:

* `bin/chat_probe` прогнал по одному инстансу 4 запроса в разном режиме (`off`, `off`, `on+none`,
  без флага) — режимы не «залипают» (артефакт `out/w2_chat_contract.json`);
* живой фасад: на одном порту/инстансе прошли `chat` (thinking off, «4» за 226 мс) и
  `chat-think` (`reasoning_content` 103 символа) — один прогон `facade_smoke.ps1`;
* тест-страж `facade_http::facade_serves_llama_server_compatible_endpoints`: три запроса на
  **один** порт дают журнал режимов `[off, on, off]`, причём третий — это MCP-путь
  (`chat_template_kwargs.enable_thinking = false`), который перебивает даже алиас `chat-think`.

Почему это важно: второй чат-инстанс (≈8,5 ГБ «модель + KV») в 12 ГБ не влезает, так что
разделение режимов «по инстансам» было бы невозможно. Оговорка: при `n_parallel = 1`
одновременные запросы стоят в очереди (стриминга у cluster API нет), но режимы при этом
не конфликтуют.

## 8. Состояние и следующий шаг

* Тесты: `cargo test --workspace` — **61 проверка green** (facade_core 10 + facade_http 3 + arbiter 14
  + pause_gate 6 + registry_plan 6 + gguf 5 + chunker 6 + walk 4 + hash 2 + config 2
  + status_report 2 + VRAM-калибровка 2 + chunker-parity 1), тяжёлые (паритет 50 файлов,
  сценарий `real`) и ручная диагностика keep-alive — `#[ignore]`.
* Артефакты W2 (шаги 1–10): `crates/hds-index` (kinds/walk/hash/chunker),
  `crates/hds-llama` (обвязка движка, `a1_device_probe`, `runtime`/`config`/`registry`,
  `llm_host_plan`, `a3_instance_probe`, `gguf`/`budget`/`vram_budget`, `pause`/`dispatch`/`status`
  + `llm_host_status`/`llm_host_dispatch`, `gguf_dump`/`kv_probe`/`chat_probe`,
  **`http`/`facade`/`llm_host_facade`**),
  `tools/parity/walk_parity.py`, `tools/parity/hash_vectors.py`, `tools/parity/facade_smoke.ps1`,
  `tools/parity/W2_REPORT.md` (этот файл); в git из `out/` идут только маленькие
  эталоны и отчёты (`walk_parity_synthetic.json`, `hash_vectors.json`,
  `w2_a1_device.json`, `w2_a2_plan.json`, `w2_a4_budget.json`, `w2_a4_status.json`,
  `w2_a4_dispatch*.json`, `w2_a4_indexing.json`, `w2_chat_meta.json`, `w2_kv_probe.json`,
  **`w2_chat_contract.json`, `w2_facade.json`**).
* Риски: **W2-2 закрыт замером** (кросс-процессной адресации нет → фасад обязателен и уже готов);
  R29/R32 подтверждены повторно (§1.2–1.3); R28 (хэш) закрыт паритетом 50/50;
  вопрос `llm.chat.n_ctx` закрыт замером KV (§7.2).
* Следующее: **A6** — резидентный `llm-host` (автозапуск, `data/llm-host.pid`, `data/logs/llm-host.log`,
  подкоманды CLI `status/load/unload/devices`, перевод портов на «боевые» 8010–8012 при выключенной
  Python-версии) + два пункта из §7.2 (`n_batch` чата и поведение `llama_params_fit`);
  затем **ARB-сценарии** автоматизацией (`arb_scenarios.py`) и **B4** — конвейер `process_file`.

## 9. Передача в новый чат (состояние W2 на 30.09.2026)

### 9.1. Где мы

| Коммит | Что сделано | Проверки |
|---|---|---|
| `154c298` | **A1** обвязка движка (`libloading`, путь поиска DLL, `EngineCwd`, cluster-обёртка) + проба `a1_device_probe`; **B1** обход/исключения/лимиты; **B2** `content_hash` | 14/14 синтетика, 96 318/96 318 боевой `D:\`, 9 векторов + 50/50 хэш |
| `7b32b14` | **B3** чанкер (дословный порт) | 16/16 фикстур, 6 363 чанка golden |
| `7b8c36e` | **A2** `runtime`/`config`/`registry` + `llm_host_plan` | 8 тестов + прогон по боевому `config.yaml` |
| `efe1168` | **A3** проба кросс-процессной адресации | замер: адресации нет → фасад обязателен (W2-2 закрыт) |
| `3418ade` | **A4 шаг 1** GGUF-метаданные + бюджет VRAM + `vram_budget` | калибровка bge-m3: 636 МиБ против замера +636 МиБ |
| `ff130a8` | передача в новый чат (шапка плана, §9, README, STATUS) | — |
| `822ceed` | **A4 шаг 2** диспетчер VRAM (`dispatch`/`pause`/`status`) + `llm_host_status`/`llm_host_dispatch` | 14 + 6 + 2 теста; живые прогоны §7.1 (в т.ч. найден и закрыт регресс «пауза пользователя») |
| `0fb6f03` | **A4: замер KV** по решению заказчика — `bin/gguf_dump` (метаданные, гибридные слои) + `bin/kv_probe` (фактический KV движка), исправление оценки в `gguf.rs`/`budget.rs` | §7.2: KV = 1024 МиБ (KV держат 8 слоёв из 32), NVML-дифференциал 4,00 КиБ/токен/слой (0,0 % к формуле), «модель+KV» 8492 МиБ → в 12 ГБ влезает |
| `f4215c9` | перенос подтверждения полного офлоада в A6 (решение заказчика) | — |
| `8343153` | **A5 фасад** — `http` (свой мини-HTTP), `facade` (маршрутизация/prompt/thinking/ответы), `bin/llm_host_facade`, `bin/chat_probe`, `facade_smoke.ps1` | `facade_core` 10 + `facade_http` 3 теста; живой прогон §7.3 (чат «4» за 226 мс, `chat-think` с размышлениями, эмбеддинги 1024, диспетчер не снял чужую паузу) |
| `13e176e` | **A5: тест-страж** «один инстанс — два режима» (агент thinking ON, MCP thinking OFF) | §7.3: журнал режимов `[off, on, off]` на одном порту |
| `ddb37e8` | **A6 резидентный `llm-host`** — `src/host.rs` (`Host`/`HostConfig`/`ClusterBackend`), `src/resident.rs` (pid/лог), `/internal/*` + прокси режима `facade`, мини-клиент `http::client_json`, `bin/llm_host` (CLI), тонкие `llm_host_facade`/`llm_host_status`, `llm.<role>.n_batch`, `installers/install_llm_host_task.ps1`, `resident_smoke.ps1` | `host_resident` 12 тестов, всего 74 green; живой прогон §10.2 (второй экземпляр отказ, `/internal/*`, `stop` освобождает pid, пауза пользователя цела); находки §10.3 (`--ngl 0` ≠ CPU, `llama_params_fit`, роль на CPU = 0 МиБ) |
| `ee6ddf2`, `c010d45` | передача в новый чат после A6 (README §4, §10 отчёт A6, §9.1/§9.7, шапки планов, STATUS) | — |
| `bdeb29f` | **боевые порты + ARB-1…6** (`arb_scenarios.py`), фон арбитра (ARB-3/ARB-5) и **три живых бага**: запрос к загруженной роли (ветка + 3 регресс-теста), «висящая» пауза после отказа, `--no-residency` затирал `--log` | 77 green; ARB **6/6** (`out/w2_arb.json`); на 8010–8012: чат «4» за 388 мс, `chat-think` 162 симв., embeddings 2×1024, rerank; `stop` убирает инстансы и не снимает паузу пользователя (§10.2a) |
| `57dce3e`, `e3a02df` | документация по A6: офлоад **9384 МиБ**, `n_batch` −1503 МиБ, §9.7 +3 грабли, состояние машины | — |
| `f8895da` | **перенос портов навсегда**: владелец `llm-host`, `n_batch: 512` в конфиге, установщик понимает «нашего» владельца и падает в Автозагрузку | `hds.cli ask` проходит целиком (§10.2b) |
| `65a4338` | **B4 конвейер `process_file`**: новый крейт `crates/hds-core` (`config`/`db`/`http`), `hds-index` (`pipeline`/`progress`/`heartbeat`/`embed`/`sidecar`), Python-мост для паритета | 92 green; паритет golden **16/16 (6363 чанка)**, инкремент на копии боевой БД, Python читает Rust-БД — §11 |
| `67686e4` | **B5 watcher**: свой backend `ReadDirectoryChangesW` (FFI, `notify` нет в кэше offline), `watch.lock`, `wait_stable`, `handle_event`, `run_watch` | 100 green; 7 детерминированных + **6 сценариев live** — §12 |
| `0144f25` | **B6 sidecar**: автономный `sidecar/hds_extract/worker.py` + крейт `hds-extract` (JSON-RPC 2.0), `hds-index::sidecar` — адаптер | 108 green; старт 0,17 с, RSS 32,6→61,3 МБ, EOF 0,07 с; паритет B4 сохранён — §13 |

Ветка `w2-llm-host` — **23 коммита**, `main` (`9ed8452`) и боевой индекс **не тронуты**.
`cargo test --workspace` — **108 проверок green** (+6 `#[ignore]`: паритет 50 файлов,
сценарий `real` по флагу `HDS_WALK_PARITY_REAL=1`, паритет/инкремент B4, боевая БД).
**Владельцем портов 8010–8012 стал `llm-host`** (§10.2a; Python-роли остановлены,
откат — `python -m hds.llama_server start all`).

### 9.2. Статус задач W2

| Задача | Статус | Остаток |
|---|---|---|
| A1 обвязка движка | ✅ | — |
| A2 реестр инстансов и маппинг конфига | ✅ | подкоманды `llm_host devices/status` — в A6 (`/internal/*`, §10); `hdsw`-обёртка — B7 |
| A3 кросс-процессная адресация | ✅ | — (вывод: фасад обязателен) |
| A4 диспетчер VRAM | ✅ | шаги 1–2 + фон арбитра (ARB-3/ARB-5) + ARB-1…6 автоматизацией (`arb_scenarios.py`, 6/6); остаток: ретраи/`FAILED` без бесконечного цикла (§8.6.2) |
| A4 замер KV (`llm.chat.n_ctx`) | ✅ | 32768 остаётся, KV = 1024 МиБ (§7.2); полный офлоад подтверждён — **9384 МиБ** (§10.2a) |
| A5 фасад `:8010–8012` | ✅ | готов и проверен живьём (§7.3); **порты переведены на фасад 30.09.2026** — владелец `llm-host`, Python-роли остановлены (§10.2a) |
| A6 резидентный `llm-host` | ✅ | `Host`, pid/лог, CLI, `/internal/*`, режимы, автозапуск, ARB-1…6 автоматизацией (6/6), боевые порты + полный офлоад 9384 МиБ + `n_batch` (−1503 МиБ) — §10 |
| B1 обход/лимиты/exclude | ✅ | — |
| B2 `content_hash` | ✅ | — |
| B3 чанкер | ✅ | — |
| B4 конвейер `process_file` | ✅ | фазы extract/commit, атомарный коммит, `clip_for_embedding`, `max_chunks`, прогресс+heartbeat, R30; паритет golden 16/16 (6363 чанка) — **§11** |
| B5 watcher (`notify`) | ✅ | свой backend `ReadDirectoryChangesW` (крейта `notify` нет в кэше offline); debounce, `watch.lock`, reconcile, rename/удаление/корзина — **§12** |
| B6 sidecar-клиент + Python-воркер | ✅ | автономный воркер `sidecar/hds_extract/worker.py` + крейт `hds-extract` (JSON-RPC 2.0, idle/restart/EOF); старт 0,17 с, RSS 32,6→61,3 МБ — **§13** |
| B7 `db-move` и подкоманды CLI | ✅ | новый крейт `crates/hds-cli` (бинарь `hds`): `status`/`check`/`reindex`/`reindex-fts`/`forget`/`stop`/`clip-index`/`index`/`watch`/`db-move`; `check`/`status` совпадают с Python, `reindex-fts` на копии боевой БД (10 000 чанков, Python 500/500), `db-move` (комментарии `config.yaml` целы), `forget`; 125 green — **§14** |
| B-2 пилот индексации | ✅ | 10 000 файлов: полный паритет (файлы/чанки/`chunk_count`/`content_hash`/тексты/FTS — 0 расхождений), **+7,1 %** по времени — **§15** |
| B-4 память | ✅ | index-500: время +5,2 %, ws +13 %, commit +0,7 %; простой watch: ws +17 %, commit **−93 %** (эталон — все в допуске +20 %) — **§16** |

### 9.3. Карта кода W2

| Путь | Что |
|---|---|
| `Cargo.toml` | воркспейс: `members = crates/*`, `exclude = tools/parity/spikes` (спайки W0 живут отдельно) |
| `crates/hds-index/src/kinds.rs` | таблицы расширений → вид файла, `ext_of` как `os.path.splitext`, `~$`-локи |
| `crates/hds-index/src/walk.rs` | `walk_files` (порт `iter_files`), `Excludes` (`_norm_path`), `IndexLimits`, `FileFilter::precheck` |
| `crates/hds-index/src/hash.rs` | `content_hash` (Blake2b-16), `hash_of_parts` |
| `crates/hds-index/src/chunker.rs` | `make_chunks` (порт `hds/chunker.py`), `Segment`/`Chunk` |
| `crates/hds-index/tests/*` | `hash_parity`, `walk_parity` (паритет с Python), `chunker_parity` (golden); **B4**: `pipeline_core`, `heartbeat_progress`, `pipeline_parity` (`#[ignore]`, golden), `pipeline_incremental` (`#[ignore]`, копия боевой БД) |
| `crates/hds-core/*` | **B4**: общий слой ядра (§2.4) — `config` (порт `hds/config.py`: `PROJECT_ROOT`, `load`, `dig`, `db_abs_path`, `replace_file`), `db` (схема `index.db` 1:1, `PRAGMA`, vec0 через `sqlite3_auto_extension`, `meta.vec_dim`, бэкфилл `indexed_at`, CRUD), `http` (свой мини-HTTP) |
| `crates/hds-index/src/pipeline.rs` | **B4**: `process_file` (фазы `extract_file`/`commit_file`), `clip_for_embedding`, `run_index` (пауза/стоп, heartbeat, прогресс, prune), `reindex_path` |
| `crates/hds-index/src/progress.rs` | **B4**: `ProgressReporter` (порт `hds/progress.py`, поля `heartbeat_data`, рендер в отдельном потоке — вывод не зависит от читателя stdout) |
| `crates/hds-index/src/heartbeat.rs` | **B4**: `index.heartbeat.json` (атомарная запись, рефреш 5 с), `SessionState`/`index_running` — R30 |
| `crates/hds-index/src/embed.rs` | **B4**: клиент эмбеддингов через фасад `:8011` (порт `hds/embedder.py`: батчи, ретраи, хинты) |
| `crates/hds-index/src/sidecar.rs` | **B6**: адаптер `hds-extract` к трейтам конвейера (`Extractor`/`Lemmatizer`) |
| `crates/hds-extract/*` | **B6**: клиент воркера — `protocol` (JSON-RPC 2.0 NDJSON), `worker` (`Worker`: интерпретатор, `hello`, `extract`/`normalize`/`clip_image`, перезапуск/таймаут/EOF, `shutdown`); тесты `protocol`, `mock_worker`, `worker_live` |
| `crates/hds-index/src/watch.rs` | **B5**: watcher — свой backend `ReadDirectoryChangesW` (FFI), разбор `FILE_NOTIFY_INFORMATION`, `watch.lock` (атомарный + устаревший), `wait_stable`, `handle_event`, `run_watch`, reconcile |
| `crates/hds-index/tests/watch_core.rs`, `tests/watch_live.rs` | **B5**: разбор событий/`watch.lock`/`wait_stable`/`handle_event` (детерминированные) и 6 сценариев на реальных событиях ОС |
| `sidecar/hds_extract/worker.py`, `requirements.lock`, `sidecar/README.md` | **B6**: автономный Python-воркер (извлечение+лемматизация), зависимости, документация контракта §5 |
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
| `crates/hds-llama/src/pause.rs` | `IndexPause`/`PauseLease` (`index.pause`, вложенность, чужую паузу не снимаем), `read_heartbeat` |
| `crates/hds-llama/src/dispatch.rs` | арбитр VRAM: `plan_query`/`plan_indexing`/`idle_evictions`, `Action`/`Verdict`/`Plan`, `apply` (кластер) |
| `crates/hds-llama/src/status.rs` | `StatusReport` (роли/бюджет/пауза/прогноз, `lines` + `json`) |
| `crates/hds-llama/src/http.rs` | минимальный HTTP/1.1 (свой: crates.io недоступен): keep-alive, `100-continue`, лимиты, `411` на chunked |
| `crates/hds-llama/src/facade.rs` | маршрутизация `llama-server`, сборка prompt, thinking-режимы, ответы OpenAI, `Backend`/`handle`/`serve`, **внутренний API `/internal/*`** (A6) и проксирование режима `facade` |
| `crates/hds-llama/src/host.rs` | A6: резидентный хост — сборка машины (`Host::start`), режимы, уборка, `role_needs`/`port_roles`/`local_status` |
| `crates/hds-llama/src/resident.rs` | A6: `PidFile` (защита от второго экземпляра) и `Log` (консоль + файл) |
| `crates/hds-llama/src/bin/*` | `a1_device_probe`, `llm_host_plan`, `a3_instance_probe`, `vram_budget`, `llm_host_status`, `llm_host_dispatch`, `gguf_dump`, `kv_probe`, `chat_probe`, `llm_host_facade`, **`llm_host`** (CLI резидента) |
| `crates/hds-llama/tests/*` | `registry_plan`, `budget_calibration`, `arbiter`, `pause_gate`, `status_report`, `facade_core`, `facade_http`, **`host_resident`** |
| `tools/parity/facade_smoke.ps1` | живая проверка фасада (альтернативные порты, чат на CPU) |
| `tools/parity/resident_smoke.ps1` | A6: живая проверка резидентности (pid/лог, второй экземпляр, `/internal/*`, `stop`) |
| `installers/install_llm_host_task.ps1` | A6: задача Планировщика `HermesDiskSearchLlmHost` (после остановки Python-ролей) |
| `tools/parity/W2_REPORT.md` | **этот журнал** (числа, находки, команды; §10 — отчёт A6) |

### 9.4. Команды проверки (copy-paste, из корня репозитория)

> Живая машина (кто владеет портами, как остановить/поднять `llm-host`, что нельзя ломать) —
> **§10.6**; грабли окружения нового чата (git-пагер, кириллица в grep) — **§10.7**.

```powershell
# --- базовые проверки ---
cargo test --workspace                                # 92 проверки (быстрые, +6 #[ignore])
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

# --- A4 шаг 2: диспетчер VRAM (вытеснение по приоритетам, index.pause, статус) ---
cargo run -p hds-llama --release --bin llm_host_status  -- --json tools\parity\out\w2_a4_status.json
cargo run -p hds-llama --release --bin llm_host_dispatch -- --role chat                       # решение без действий
cargo run -p hds-llama --release --bin llm_host_dispatch -- --role chat --budget-mb 3000      # сужение бюджета (A-7)
cargo run -p hds-llama --release --bin llm_host_dispatch -- --role embedding --kind indexing --apply

# --- A4: замер KV (решение по llm.chat.n_ctx) ---
cargo run -p hds-llama --release --bin gguf_dump -- --role chat --json tools\parity\out\w2_chat_meta.json
cargo run -p hds-llama --release --bin kv_probe  -- --role chat --ngl 8 --n-ctx 4096,32768 --json tools\parity\out\w2_kv_probe.json

# --- A5: контракт чата и живой фасад ---
cargo test -p hds-llama --test facade_core --test facade_http
cargo run -p hds-llama --release --bin chat_probe -- --json tools\parity\out\w2_chat_contract.json
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\facade_smoke.ps1 -PortBase 8020 -HoldSec 20

# --- A6: резидентный llm-host (проверки без VRAM, порты 8030-8032) ---
cargo test -p hds-llama --test host_resident            # 12 тестов: pid/лог/внутренний API/прокси/off
cargo run -p hds-llama --release --bin llm_host -- status --local --no-engine   # локальный отчёт
cargo run -p hds-llama --release --bin llm_host -- run --port-base 8030 --ngl 0 --no-residency --hold 60
cargo run -p hds-llama --release --bin llm_host -- status --port 8030            # отчёт резидента
cargo run -p hds-llama --release --bin llm_host -- stop --port 8030
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\resident_smoke.ps1 -PortBase 8030 -HoldSec 600

# --- A6: ARB-1...6 на живом движке (свои порты/конфиги/сигналы, боевые не трогает) ---
.\.venv\Scripts\python.exe tools\parity\arb_scenarios.py --port-base 8070   # 6/6 green, out/w2_arb.json
# --- B4: конвейер process_file (ядро) ---
cargo test -p hds-core                                              # схема/PRAGMA/CRUD/vec0
cargo test -p hds-index --test pipeline_core --test heartbeat_progress   # инкремент/ошибки/R30
cargo test -p hds-core --test db_schema -- --ignored --nocapture    # Python читает Rust-БД; Rust читает боевую
cargo test -p hds-index --test pipeline_parity -- --ignored --nocapture      # golden 16/16, 6363 чанка (.venv + фасад :8011)
cargo test -p hds-index --test pipeline_incremental -- --ignored --nocapture # копия боевой БД: unchanged, чанки совпали

# --- B5: watcher (наблюдатель ФС) ---
cargo test -p hds-index --test watch_core                                  # разбор событий/watch.lock/wait_stable/handle_event
cargo test -p hds-index --test watch_live -- --nocapture                   # 6 сценариев на реальных событиях ReadDirectoryChangesW

# --- B6: sidecar-воркер извлечения/лемматизации ---
cargo test -p hds-extract                                                  # протокол/mock/реальный воркер (idle, restart, EOF)
'{"jsonrpc":"2.0","id":1,"method":"hello"}' | .\.venv\Scripts\python.exe sidecar\hds_extract\worker.py --root .
```

### 9.5. Открытые вопросы и решения

* **Закрыт 30.09.2026 (замер §7.3):** `reasoning = off` через **кластерный** инстанс даёт ответ
  без блоков размышлений (в W0 проверялось только bridge-API) — фасад на этом и построен.
* **Закрыт 30.09.2026 (замер):** `llm.chat.n_ctx` — `32768` **остаётся**. Модель гибридная
  (`full_attention_interval = 4`), KV держат 8 слоёв из 32 → 1024 МиБ (f16); подтверждено
  данными движка и NVML-дифференциалом. Числа — §7.2.
* **Решено и не переоткрывать:** без авто-деградации кванта; llama-server удаляется
  в конце W2; кросс-процессной адресации нет → фасад обязателен; устройство — числовым
  `manual_devices_csv`; cwd движка + вендорские каталоги обязательны; бюджет VRAM — по NVML.
* **Не переоткрывать после A4 шага 2:** `index.pause` снимает только тот, кто его
  поставил (`ours_created`) — чужую (пользовательскую) паузу запросы не трогают; занятые
  запросом и чужие (не наши) инстансы не вытесняются никогда; роль вне `gpu.priorities`
  получает приоритет 0; при `gpu.policy: manual` диспетчер только сообщает.
* **Не переоткрывать после замера KV (§7.2):** KV считаем по слоям с полным вниманием
  (`full_attention_interval`), а не по всем; NVML на Windows — машинная занятость
  (per-process VRAM нет), при почти полной карте WDDM двигает чужие буферы → порог шума 64 МиБ;
  у гибридных моделей есть фиксированные SSM-состояние (≈50 МиБ) и compute-буфер (≈1972 МиБ
  при `n_batch` 2048), которые оценка «модель + KV + 5 %» не покрывает.
* **Не переоткрывать после A5 (§7.3):** thinking — **поле запроса**, а не свойство инстанса,
  поэтому агент (thinking ON) и MCP (thinking OFF) обслуживает **один** чат-инстанс
  (второй в 12 ГБ не влез бы); движок **сам применяет шаблон чата** к нашему `prompt`
  (маркеры ChatML ставить нельзя); HTTP-сервер **свой** (`crates.io` недоступен:
  `Could not resolve host: index.crates.io`), стриминг не поддерживаем (400), chunked-тело — 411.
* **Осталось измерить (A6, решение заказчика 30.09.2026 — отложено):** полный офлоад чата на
  **свободной** карте — подтвердить суммарные ≈10,5 ГБ (модель + KV + SSM + compute) и проверить,
  не меняет ли `llama_params_fit` офлоад сам; при необходимости уменьшить `n_batch` чата.
  Делаем в A6, когда роли Python-версии выключаются штатно (сейчас карту занимают они,
  а на почти полной карте машинный NVML недостоверен — §7.2).

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
8. **`index.pause` — чужой не снимать.** `resume()` без активной аренды удалял файл,
   поставленный пользователем (регресс A4 шага 2, пойман живым прогоном). Теперь удаляется
   только файл, созданный нами (`ours_created`); в `llm-host` пауза берётся через
   `PauseLease` (RAII), и снимается на `Drop` только своя.
9. **Прогон с `--apply` меняет состояние машины.** Перед прогоном проверяйте `index.pause`
   и что выгружается: `llm_host_dispatch` сначала показывает решение, и только `--apply`
   его выполняет (без флага ничего не трогается).
10. **NVML на Windows — машинная занятость, и при почти полной карте WDDM двигает чужие
    буферы** (наблюдено: Δn_ctx 28672 дал −32 МиБ вместо +224 МиБ). Для замеров KV
    использовать порог шума (в `kv_probe` — 64 МиБ) и/или свободную карту; фактические
    размеры буферов движок печатает сам (`llama_kv_cache: size = …` — это и есть замер).
11. **Гибридные модели:** KV держат не все слои (`full_attention_interval`), плюс есть
    фиксированные SSM-состояние и compute-буфер — оценка «модель + KV + 5 %» их не видит,
    а на решение «влезает/не влезает» они влияют (см. §7.2).
12. **`accept()` от неблокирующего слушателя (Windows)** отдаёт неблокирующий сокет:
    keep-alive рвался после первого ответа. В `http.rs` сокет переводится в блокирующий
    режим; тест-страж — `facade_http::http_transport_handles_keep_alive_continue_and_chunked`.
13. **`crates.io` недоступен на машине заказчика** (`Could not resolve host: index.crates.io`):
    новые зависимости не добавить, поэтому HTTP-сервер фасада свой (`src/http.rs`).
    Проверять доступность сети до попытки `cargo add`, а не после.
14. **PowerShell 5.1 читает `.ps1` без BOM как ANSI** и ломает кириллицу в комментариях/строках
    (скрипт «падает» на парсинге). Скрипты harness держим **ASCII-only** (`facade_smoke.ps1`,
    `resident_smoke.ps1`, `installers/install_llm_host_task.ps1`).
15. **PS 5.1 + `$ErrorActionPreference='Stop'` + stderr нативной программы** = падение скрипта:
    строки stderr становятся `ErrorRecord` и терминальной ошибкой (`NativeCommandError`),
    даже если процесс просто напечатал предупреждение. В `resident_smoke.ps1` обёртка
    `Run` временно ставит `Continue` вокруг вызова `llm_host.exe`.
16. **`"$var: текст"` в строке PS** читает `$var:` как имя диска → `ParserError`
    «Variable reference is not valid». Писать `${var}` (поймано в `resident_smoke.ps1`).
17. **`--ngl 0` не уводит роль с GPU** (A6, §10.3): движок вызывает `llama_params_fit` и
    может офлоаднуть модель обратно, если **устройство** — CUDA. Держать роль на CPU нужно
    устройством (`manual_devices_csv` = bridge-индекс CPU), а не нулём слоёв.
18. **Ошибка движка при нехватке VRAM невнятная** (`llama_model_load: error loading model:
    invalid vector subscript`): грузить роль в обход диспетчера нельзя — сначала
    `prepare`/`plan_query`, потом `load_instance` (сделано в `/internal/load`).
19. **Запрос к уже загруженной роли не требует памяти.** `plan_query` спрашивает «хватит ли
    на роль целиком», а не «сколько дозагрузить» — без отдельной ветки на заполненной карте
    падали даже запросы к загруженному чату (боевые порты, §10.2a; регресс-тесты в
    `tests/arbiter.rs`).
20. **Отклонённый запрос не должен оставлять `index.pause`.** План ставит паузу до проверки
    вердикта; при `NotEnough` аренда не берётся, и файл оставался навсегда (R30: «индексация
    встала»). `prepare` снимает свою паузу перед отчётом о нехватке (чужую не трогает).
21. **Порядок флагов CLI не должен решать семантику**: `--no-residency` затирал `--log`
    (разовый прогон гасил оба). Теперь `--no-residency` = только pid-файл, `--no-log` = лог.

### 9.8. План шага A6 — по файлам (выполнен 30.09.2026, отчёт — §10)

**A6 (трек A, следующий шаг).** Основа есть: `bin/llm_host_facade` умеет всё, кроме
резидентности, — его и доделываем. Порядок:

1. **Вынести тело `bin/llm_host_facade::run()` в библиотечный модуль** `src/host.rs`
   (`Host::start(&cfg) -> Host` + `Host::stop()`), бинарь оставить тонким — по образцу того,
   как `dispatch::apply` вынесено из бинаря (тестируется без сокетов).
2. **Резидентность:** `data/llm-host.pid` (атомарно + проверка живого PID — образец
   `hds/watcher.py::_acquire_lock`), `data/logs/llm-host.log`, защита от второго экземпляра,
   аккуратное завершение: `facade::serve` → `stop`, `force_resume` паузы,
   `unload`+`remove` инстансов (этот код уже есть в конце `run()` — переиспользовать).
3. **CLI `llm-host status|load|unload|devices`** (A6/B7): проще всего через сам фасад —
   внутренние маршруты `/internal/status|load|unload` на loopback (вариант «файл-отчёт»
   менее живой). `status` печатает готовый `StatusReport` (`src/status.rs`).
4. **`llm_server.mode`**: `embedded` (сейчас) | `facade` (инстансы не создаём, проксируем на
   внешний OpenAI-сервер) | `off`. Поле уже разбирается в `src/config.rs`.
5. **Установка:** задача `HermesDiskSearchLlmHost` в `install_autostart.ps1`; перевод портов
   8010–8012 на фасад — после остановки Python-ролей (llama-server удаляем в конце W2).
6. **Проверки A6:** живой прогон на боевых портах; `arb_scenarios.py` (ARB-1…6 из §8.6 основного
   плана — пауза + вытеснение на живом `llm-host`); полный офлоад чата на **свободной** карте
   (§7.2: подтвердить ≈10,5 ГБ и поведение `llama_params_fit`).
7. **Что учесть:** compute-буфер ≈1972 МиБ при `n_batch 2048` (есть смысл задать `n_batch` чата
   меньше); движок зовёт `llama_params_fit` и при тесной карте может **сам** менять офлоад —
   проверить и, если это нарушает «без авто-деградации», отключать.

**Трек B независим** и может идти параллельно: **B4** — конвейер `process_file` (фазы
extract → commit, атомарность на файл, `clip_for_embedding`, `max_chunks`, прогресс +
`index.heartbeat.json`), затем **B5** watcher (`notify`, debounce, `watch.lock`, reconcile),
**B6** sidecar. Кирпичи готовы: `hds-index::walk/kinds/hash/chunker` + golden-эталоны.
Эмбеддинги в B4 берутся **через фасад** `:8011` — кросс-процессной адресации нет (факт A3).

### 9.9. Что НЕ проверено и ограничения (честно)

* ✅ ~~Фасад на боевых портах~~ — **проверено 30.09.2026**, порты переведены навсегда
  (§10.2a): Python-менеджер видит наши инстансы как `state=llama`, `hds.cli ask` проходит.
* ✅ ~~Автоматизация ARB-1…6~~ — **сделана 30.09.2026** (`arb_scenarios.py`, 6/6, §10.2c).
* ✅ ~~Полный офлоад на свободной карте~~ — **замерен: 9384 МиБ** при `n_batch 2048`
  (7881 при 512), `llama_params_fit` ничего не менял (§10.2a).
* `stream=true` не поддержан (400), chunked-тело — 411, TLS нет (фасад только localhost).
* `--ngl N` (проверочный режим) — не боевой переключатель: он правит **план** инстансов,
  а не конфиг; при `N = 0` роль уходит на CPU-устройство (§10.3, находка 1).
* `llm_server.mode: facade` — покрыт тестами (проксирование «как есть», `/props` от
  апстрима), но живьём не гонялся: нужен внешний владелец GPU.
* macOS-часть (Metal-бюджет, dylib, LaunchAgent) — только код-заделы, исполнением не
  проверялись (§10.0 основного плана, R34; чек-лист `MAC_CHECKLIST.md`).
* Python-версия — источник истины по поведению: расхождения Rust сначала фиксируем
  паритет-замером, потом правим (правило §9.6).
* `rerank`-роль остаётся на CPU (legacy `-ngl 0`) — **осознанно**: с GPU `ask` не влезает
  в 12 ГБ (§10.2b). Такую роль `role_needs` считает «нужно 0 МиБ», она не попадает в
  вытеснение (§10.3, находка 3).
* Whisper-роль (ARB-1/ARB-3 в буквальной формулировке — вытеснение `whisper`) — W3.

Открытые вопросы к заказчику (осталось только это):

* ~~`rerank` на CPU~~ — **решено**: остаётся на CPU, пока Python-клиент тянет torch
  (перенести на GPU после Rust-клиента, тогда перепроверить `ask`).
* ~~Владелец портов~~ — **решено**: `llm-host` (§10.2a).

### 9.9. План шага B7 — по файлам (следующий чат)

**Цель.** `db-move` и подкоманды CLI (`check/reindex/reindex-fts/forget/stop/clip-index/
status`); схема БД и `PRAGMA` — **1:1** с Python, `sqlite-vec` через
`sqlite3_auto_extension` (спайк 1). Ветка та же (`w2-llm-host`), коммит — после тестов.

**Python — источник истины:**

| Python | Что портируем |
|---|---|
| `hds/cli.py` | `cmd_status`, `cmd_reindex` (`reindex_path` уже есть), `cmd_reindex_fts` (перестроить `chunks_fts` через лемматизатор, `busy_timeout=600000`, `meta.fts_normalized`), `cmd_forget` (`remove_path`), `cmd_clip_index` (W3), `cmd_index` (обёртка над `run_index`), `cmd_stop`/`cmd_serve`/`cmd_watch` |
| `hds/dbops.py` | `move_db`: остановить watcher/index, перенести `index.db` (+`-wal`/`-shm`), правка `db_path` в `config.yaml` **текстовой подстановкой** (комментарии сохраняются, `serde_yaml` не подходит — §3.2 плана) |
| `hds/diag.py` | `check`: компоненты (движок/модели/роли/БД/FTS/OCR/whisper), `fts-norm` — контентная проверка (сравнение сэмплов с `chunks_fts`) |

**Файлы (предложение).**
* новый крейт **`crates/hds-cli`**: `[[bin]] hds` со подкомандами (`clap` недоступен
  offline → свой разбор argv, как в `hds-llama::bin`), `hdsw` (windows subsystem) — позже;
* `src/cmd/status.rs`, `check.rs`, `reindex.rs`, `reindex_fts.rs`, `forget.rs`,
  `db_move.rs`, `index.rs` (обёртка `pipeline::run_index` + `hds-extract` worker +
  `embed::Embedder`), `cmd/stop.rs` (снятие `index.pause`/`watch.lock`/`index.stop` —
  сверять с Python);
* переиспользовать: `hds_core::{config,db}`, `hds_index::pipeline`, `hds_extract::Worker`,
  `hds_llama` — только по HTTP (фасад), не как библиотеку.

**Приёмка B7.**
1. `check` — вывод совпадает по смыслу с `python -m hds.cli check` (компоненты/предупреждения);
2. `status` — `db::stats` теми же полями (kind/status/chunks/last_indexed_at/errors);
3. `reindex-fts` — перестраивает FTS и ставит `meta.fts_normalized='1'`; прогон по копии
   боевой БД, Python читает результат;
4. `db-move` — БД (+WAL/SHM) переносится, `config.yaml` сохраняет комментарии;
5. `forget` — удаляет запись+чанки (проверка `db::remove_path`);
6. `sqlite-vec` — через `sqlite3_auto_extension` (без `load_extension`), vec0 0.1.9.

**Грабли/рамки.** `crates.io` недоступен → `clap`/`tokio` не добавлять (свой argv);
лемматизация — только через `hds-extract` воркер; в `reindex-fts` — `busy_timeout`
600 с (watcher может держать write-lock); `db-move` обязан остановить watcher/index.


## 10. A6 — резидентный `llm-host` (отчёт, 30.09.2026)

### 10.1. Что сделано (по файлам)

| Путь | Что |
|---|---|
| `crates/hds-llama/src/host.rs` | `Host` (старт/остановка/ожидание/статус) + `HostConfig` + `ClusterBackend` + `ClusterShared`; тело `run()` из бинаря A5 переехало сюда; `role_needs`/`instance_uses`/`port_roles`/`client_ports`/`dig*`; `local_status` (общий с `llm_host_status`); режимы `embedded`/`facade`/`off`; `apply_ngl_override` (проверочный `--ngl`) |
| `crates/hds-llama/src/resident.rs` | `PidFile` (эксклюзивный `create_new`, снятие устаревшего файла, проверка живого PID через kernel32/`kill -0`), `Log` (консоль + файл), пути `data/llm-host.pid` и `data/logs/llm-host.log` |
| `crates/hds-llama/src/facade.rs` | внутренние маршруты `/internal/{status,devices,load,unload,stop}` (+`/v1/…`), `ServerConfig.internal`, методы `Backend` с дефолтами (501/409), `proxy` для режима `facade`, `serve(cfg, backend, stop)` |
| `crates/hds-llama/src/http.rs` | мини-HTTP-клиент `client_json` (без зависимостей) + новые тексты статусов (403/409/501/502) |
| `crates/hds-llama/src/bin/llm_host.rs` | **новый CLI**: `run` (резидент), `status` (резидент → иначе локальный отчёт), `load`/`unload`/`devices`/`stop` — через `/internal/*` |
| `crates/hds-llama/src/bin/llm_host_facade.rs` | стал тонким: разбор argv → `Host` (разовый прогон: pid-файл не занимает, `--residency` включает) |
| `crates/hds-llama/src/bin/llm_host_status.rs` | стал тонким: `host::local_status` (тот же отчёт, что у `llm_host status --local`) |
| `crates/hds-llama/src/config.rs` | новые ключи `llm.<role>.{n_batch,n_ubatch,n_threads}` (приоритет над legacy `extra_args`, предупреждение при `n_ubatch > n_batch`) — для уменьшения compute-буфера чата (§7.2) |
| `crates/hds-llama/tests/host_resident.rs` | 12 тестов: pid-файл/устаревший pid/лог/умолчания путей/`/internal/*` (403, вызовы, 501, «не тот маршрут»)/прокси-режим (ответ апстрима как есть, `/props` от апстрима)/мини-клиент/`port_roles`/`role_needs`/режим `off` без движка (живой сокет) |
| `installers/install_llm_host_task.ps1` | задача Планировщика `HermesDiskSearchLlmHost` (ASCII-only): проверка свободных портов 8010–8012, `-Status`/`-Remove`/`-Force` |
| `tools/parity/resident_smoke.ps1` | живая проверка резидентности одной командой (ASCII-only) |

Итого в A6: `cargo test --workspace` — **77 green** (+2 `#[ignore]`, среди них 3 новых
регресс-теста диспетчера); предупреждений сборки нет.



### 10.2. Живой прогон (`resident_smoke.ps1 -PortBase 8030`, чат на CPU)

```
== pid file: 48864 (expected 48864)
== second instance refused (exit 1) - ok          ← защита от второго владельца GPU
== llm_host status --port 8030                    ← отчёт резидента через /internal/status
  наша занятость (NVML − baseline 2396 МиБ): 93 МиБ
  chat port=8010 state=LOADED retention=keep n_ctx=32768 devices=1 ngl=0 нужно 0 МиБ
== llm_host devices --port 8030
  index=0 backend=CUDA name=CUDA0 free=11255 МиБ; index=1 backend=CPU name=CPU free=19395 МиБ
== llm_host load embedding --port 8030 → состояние роли: LOADED
== llm_host unload embedding --port 8030 → состояние роли: UNLOADED
== llm_host stop --port 8030
  [internal] роль 'embedding': выгружена (UNLOADED), id=2
  [internal] получена команда stop
  инстансы сняты: 3 (chat/embedding/rerank: remove_instance -> Ok(()))
  index.pause сейчас: стоит (не наша — не снимаем)   ← пауза пользователя цела
  pid-файл C:\...\llm-host-smoke.pid: освобождён
  остановлен (проработал 10 с)
== pid file released - ok
```

Артефакт: `tools/parity/out/w2_resident.json` (6,5 КБ) — отчёт `status --json` от резидента.

### 10.2a. Боевые порты 8010–8012 (перенос выполнен 30.09.2026)

**Что сделано:** Python-роли остановлены (`hds.llama_server stop embedding` + снятие
осиротевшего `llama-server` чата, pid 20364, у которого не было pid-файла), затем
резидент поднят на боевых портах **из конфига заказчика** (KV f16, `-ngl 99`, `n_batch`
2048 — как есть), pid `data/llm-host.pid`, лог `data/logs/llm-host.log`.

**Паритет с Python-версией — она считает наш фасад «своим»:**

```
.\.venv\Scripts\python.exe -m hds.llama_server check chat   →  state=llama total_slots=1 (exit 0)
.\.venv\Scripts\python.exe -m hds.llama_server status       →  chat/embedding/rerank: state=llama,
                                                               running=true, ctx_actual 32768/8192/8192
```

То есть менеджер ролей Python-версии (который «своими» считает только инстансы с
совпавшим `model_path` из `/props`) видит наши инстансы как `llama-server`. Клиенты
проверены живьём: `/health`, `/props` (`model_path`, `n_ctx=32768`, `total_slots=1`),
чат (ответ «4» за 388 мс, usage 29/2), `chat-think` (162 симв. размышлений), эмбеддинги
2×1024, реранк (`0:3.04, 1:2.29` — порядок верный), `llm_host status`/`--json`,
`llm_host stop` (уборка: «инстансы сняты: 3», «index.pause … не наша — не снимаем»,
«pid-файл … освобождён»).

**Полный офлоад чата — подтверждён (данные движка, свободная карта):**

| n_batch / n_ubatch | CUDA0 model | KV (f16) | SSM (`RS`) | compute | **итого (проекция `llama_params_fit`)** |
|---|---|---|---|---|---|
| 2048 (как в боевом конфиге) | 6306,63 МиБ | 1024,00 | 50,25 | 2004,00 | **9384 МиБ** (`vs. 11203 free … no changes needed`) |
| 512 (проверка) | 6306,63 | 1024,00 | 50,25 | **501,00** | **7881 МиБ** |

* `llama_params_fit: successfully fit params to free device memory` — но `no changes
  needed`: движок **ничего не подменил**, наш офлоад устоял (риск §7.2 снят для этого случая);
* «наша занятость» по NVML на боевом прогоне: **9523 МиБ** (baseline 392) при оценке
  A4 «модель + KV» = 8492 → расхождение +1031 МиБ = compute-буфер + SSM + контекст CUDA;
* **цена `n_batch: 512` — ничего:** один и тот же RAG-запрос (5802 токена входа)
  прошёл за **5535 мс** при 2048 и **5366 мс** при 512 (в пределах шума ±3 %).
  Значит `llm.chat.n_batch: 512` + `n_ubatch: 512` — бесплатная экономия **1503 МиБ**.

**Новая находка (баг, исправлен):** запрос к **уже загруженной** роли на заполненной
карте падал с 503 «не хватает VRAM»: `plan_query` считал потребность роли целиком
(8492 МиБ), не замечая, что модель уже в памяти (свободно 206 МиБ). Живой пример — тот
же `hds.cli ask`: поиск отработал, а ответ упал. Правка: ветка «роль уже загружена →
новая VRAM не нужна» в `plan_query` (и симметрично в `plan_indexing`); регресс-тесты
`request_to_loaded_role_needs_no_new_vram`, `indexing_with_loaded_role_needs_no_new_vram`,
`unloaded_role_still_needs_memory`. После правки запрос к загруженному чату отвечает
при 206 МиБ свободных (388 мс).

**Состояние машины (итог 30.09.2026):** владельцем портов 8010–8012 **навсегда** стал
`llm-host` (Python-роли `llama-server` остановлены: они были нужны только для разработки,
а `llama-server` удаляется в конце W2). Автозапуск — ярлык
`…\Startup\HermesDiskSearchLlmHost.lnk` (Планировщик требует прав администратора —
скрипт сам делает фолбэк, как `install_autostart.ps1`). Python MCP/watcher оставлены:
они ходят на те же порты и наши инстансы принимают за «свои», поэтому больше не поднимают
`llama-server`. Пауза индексации заказчика (`index.pause`) не тронута.


### 10.2b. `hds.cli ask` на Rust-владельце: было 503 → стало «работает целиком»

Первый прогон (конфиг «как есть») упал на ответе: 503 с точным отчётом «нужно 8492 МиБ,
свободно 8323, вытеснение освободило 636 — недостаёт 557 МиБ». Причина не в деградации,
а в арифметике 12 ГБ: Python-чат жил с **KV q8_0** (≈512 МиБ вместо 1024) и меньшими
compute-накладками, а `ask` держит в GPU ещё ~3,4 ГБ (torch/CLIP). Наш чат с KV f16 +
compute 2004 не оставлял места (реранк на CPU при этом отработал за 18,4 с — выше лимита
Python-клиента 15 с, и клиент сам отключил реранкер).

**После правки конфига (`llm.chat.n_batch: 512` + `n_ubatch: 512`, §10.2a) `ask` проходит
целиком**: поиск → реранк → ответ с проектами и цитатами по файлам:
«1С:Документооборот использовался (или планировался к внедрению) в проектах: Касторама,
КАРТЭКС, Курган-Синтез, Сибиантрацит … Источники: …».

**Реранкер оставлен на CPU — осознанно** (проверено): если убрать legacy `-ngl 0`,
реранк уходит на GPU (637 МиБ) и `ask` снова падает 503 (чат + embeddings + реранк + CLIP
Python-клиента > 12 ГБ; это проверено повторным прогоном). Поэтому в конфиге роль остаётся
на CPU, а цена — реранк ~18 с и самоотключение реранкера в Python-клиенте. В Rust-клиенте
(W2+) этот тормоз уйдёт вместе с torch: тогда реранк можно будет перенести на GPU.

Вопрос политики вытеснения (нужен ответ заказчика только если карта снова окажется тесной):
наш диспетчер при нехватке памяти под **индексную** роль вытесняет резидентный `chat`
(по приоритетам он последний, но иных кандидатов в конфиге нет — `whisper` появится в W3).
Варианты: (а) оставить как есть (ARB-1/ARB-3 буквально так и описаны); (б) запретить
вытеснять `KEEP_LOADED` роли под запросы индексных ролей (поиск деградирует, ответ чата
всегда готов); (в) как (а) плюс уменьшить потребление — **выбрано**: `n_batch: 512`,
при котором всё влезает.

### 10.2c. ARB-1…6 автоматизацией (`tools/parity/arb_scenarios.py`)

План требовал автоматизацию ARB-сценариев на живом `llm-host` — сделано:
`tools/parity/arb_scenarios.py` (только stdlib) поднимает **свои** хосты на
`--port-base 8070` со **своими** временными конфигами (копия боевого + правки) и
**своим** каталогом сигналов, поэтому боевые порты, боевой `index.pause` и конфиг
заказчика не затрагиваются. Артефакт: `tools/parity/out/w2_arb.json` (**6 из 6 green**,
30.09.2026).

| Сценарий | Что проверено живьём (доказательство) |
|---|---|
| **ARB-1** | индексные роли на GPU загружены → запрос чата при суженном бюджете: **пауза индексации**, вытеснение строго по приоритету (**rerank 30 → embedding 40**), роль запроса (`chat`) не вытесняется, вместо деградации — отчёт с точными цифрами («нужно 8492, свободно 3717»), вытесненная роль **вернулась по запросу** (HTTP 200) |
| **ARB-2** | после запросов нашего `index.pause` нет; **пауза пользователя не снята** (в логе: «переиспользуем, не снимаем») |
| **ARB-3** | при живом heartbeat индексации (`paused=false`, свежий `ts`) фоновый арбитр через 15 с выгрузил резидент `chat` (`[arbiter/indexing]`), паузу при этом не ставил |
| **ARB-4** | запрос при суженном бюджете → 503 с отчётом: «не хватает VRAM … авто-деградации нет (`llm.model_policy: fixed`)», точные числа; после снятия ограничения роль грузится и отвечает «4» |
| **ARB-5** | роль простояла дольше `gpu.evict_idle_sec` (при grace 600 с, чтобы движок сам не выгрузил) → **сработал наш предохранитель** (`[arbiter/idle]`), роль выгружена |
| **ARB-6** | на одном порту: `chat-think` → `reasoning_content` (484 симв.), MCP-путь (`chat_template_kwargs.enable_thinking=false`) → размышлений нет, ответ текстом |

Чтобы ARB-3/ARB-5 стали возможны, в `llm-host` добавлен **фоновый арбитр**
(`ClusterBackend::spawn_arbiter`, такт 15 с): раньше решения принимались только внутри
запроса, а эти два сценария к запросам не привязаны. Проверено: `gpu.policy: manual`
и `evict_idle_sec: 0` его выключают (это решает сама `dispatch`), остановка хоста не
ждёт такт (сон мелкими шагами — иначе `llm-host stop` висел бы до 15 с, поймано тестом).

Запуск:
```powershell
.\\.venv\\Scripts\\python.exe tools\\parity\\arb_scenarios.py --port-base 8070
```

(≈512 МиБ вместо 1024) и без наших compute-накладок, а `ask` при этом держит в GPU
ещё ~3,4 ГБ (torch/CLIP). Наш чат с KV f16 занимает ~9,5–10,3 ГБ, и вместе с ними в
12 ГБ не влезает. Выводы:

* **боевой конфиг стоит дополнить** `llm.chat.n_batch: 512` (и `n_ubatch: 512`):
  −1503 МиБ без потери скорости (замер выше);
* при желании — `llm.chat.n_ctx: 16384` (−512 МиБ), но это уже про качество контекста;
* вопрос политики (решение за заказчиком): наш диспетчер при нехватке памяти под
  **индексную** роль (поиск) вытесняет резидентный `chat` — по приоритетам он последний,
  но иных кандидатов в конфиге нет (`whisper` появится в W3). У Python-версии роли
  просто не отбирали память друг у друга. Варианты: (а) оставить как есть (ARB-1/ARB-3
  буквально так и описаны); (б) запретить вытеснять `KEEP_LOADED` роли под запросы
  индексных ролей (тогда поиск деградирует до FTS, но ответ чата всегда готов);
  (в) как (а) + уменьшить потребление (n_batch/n_ctx) — рекомендация.


### 10.3. Находки (важные, не были в плане)

1. **`--ngl 0` не уводит роль с GPU.** Первый прогон (до правки) показал в логе движка
   `offloaded 33/33 layers to GPU`, `CUDA0 model buffer 6306,63 МиБ` + KV 1024 + compute 2004 —
   при `n_gpu_layers = 0`, потому что **устройство оставалось CUDA0**, а движок зовёт
   `llama_params_fit` («fitting params to device memory»). Итог: «наша занятость» 9459 МиБ
   вместо нуля. Правка: `--ngl 0` переводит роль на **CPU-устройство**
   (`manual_devices_csv` = bridge-индекс CPU) — после неё занятость **93 МиБ** (контекст CUDA),
   `devices=1 ngl=0`. То же правило применено к ролям конфига с legacy `-ngl 0`.
2. **Движок действительно сам пересобирает офлоад** — подтверждение риска из §7.2 (находка 4):
   «без авто-деградации» со стороны движка не гарантировано. Надёжный способ держать роль
   на CPU — задавать **устройство** (находка A1), а не только `n_gpu_layers`.
3. **Роль на CPU нельзя считать «нужна N МиБ».** Раньше `role_needs` считала реранкеру
   (legacy `-ngl 0`) 637 МиБ, и диспетчер мог «вытеснять» роль, которая VRAM не держит.
   Теперь такие роли — «нужно 0 МиБ» и в вытеснение не попадают.
4. **`--ngl` должен править план, а не копию спеки.** Иначе `status` показывал значения
   конфига (`ngl=99`, `devices=0`), а работало другое — теперь правка идёт в `planned`,
   и отчёт показывает применённое.
5. **Ошибка движка при нехватке VRAM невнятная:** `llama_model_load: error loading model:
   invalid vector subscript` (embedding, свободно 305 МиБ, нужно 636 МиБ). Поэтому
   `/internal/load` теперь **сначала спрашивает диспетчер** (`prepare`: пауза + вытеснение
   по приоритетам + отчёт `NotEnough`), и только потом грузит роль.
6. **Запрос к загруженной роли падал от «нехватки»** (боевые порты, §10.2a): `plan_query`
   считал потребность роли целиком, не замечая, что модель уже в памяти. Исправлено
   (+3 регресс-теста).
7. **Отклонённый запрос оставлял `index.pause`** навсегда: план ставил паузу (`PauseIndex`),
   а аренду мы не брали (вердикт `NotEnough` → ранний выход) — индексация встала бы «сама»
   (риск R30). Теперь `prepare` снимает свою паузу перед отчётом о нехватке.
8. **Порядок флагов CLI решал, будет ли лог**: `--no-residency` затирал `--log`
   (потому что «разовый прогон» гасил оба). Разделено: `--no-residency` — только pid-файл,
   `--no-log` — лог. Поймано отладкой `arb_scenarios.py`, когда лог хоста «не появлялся».
9. **PowerShell 5.1 + `ErrorActionPreference=Stop` + stderr нативной команды = падение
   скрипта** (ErrorRecord от `llm_host.exe run` в pipeline): вызовы обёрнуты временным
   `Continue`. Плюс `"$code: $what"` в строке PS читает `$code:` как имя диска — нужно
   `${code}`. Обе грабли — в `resident_smoke.ps1` (и в журнал §9.7 при следующей правке).


### 10.4. Что осталось по A6 (по шагам §9.8)

Сделано 30.09.2026: **боевые порты** (перенос + паритет с Python-менеджером, §10.2a),
**полный офлоад** (замер 9384 МиБ, §10.2a), **`n_batch`** (замер 2048→512: −1503 МиБ,
§10.2a), **ARB-1…6 автоматизацией** (6/6 green, §10.2c), фон арбитра (ARB-3/ARB-5).

Осталось (требует решения заказчика или относится к поздним волнам):

1. ✅ **Владелец портов 8010–8012 — `llm-host`** (решение принято и выполнено 30.09.2026:
   Python-роли `llama-server` остановлены, автозапуск — ярлык `HermesDiskSearchLlmHost.lnk`
   в папке «Автозагрузка»; Python MCP/watcher оставлены и работают через наш фасад).
   `llama-server` удаляется в конце W2 вместе с менеджером ролей.
2. ✅ **Политика вытеснения** — выбрано (в): уменьшить потребление (`llm.chat.n_batch: 512`),
   при котором и `ask`, и реранк, и чат влезают; политика диспетчера оставлена как в ARB-1/ARB-3.
3. ⏳ **`rerank` на GPU**: пока остаётся на CPU (иначе `ask` не влезает в 12 ГБ, §10.2b);
   перенос имеет смысл после Rust-клиента (без torch) — тогда перепроверить.
4. **`llm_server.mode: facade`** реализован и покрыт тестами (проксирование «как есть»,
   `/props` от апстрима), но живьём не гонялся: нужен внешний владелец GPU.
5. **Whisper-роль**: ARB-1/ARB-3 в буквальной формулировке (вытеснение `whisper`)
   проверятся в W3, когда появится аудио-роль; сейчас вытесняются `rerank`/`embedding`.
6. **Журнал граблей** §9.7 дополнен пунктами из §10.3.6–10.3.9 (сделано в этом коммите).

### 10.6. Живая машина: что знать перед продолжением (для нового чата)

**Кто владеет GPU прямо сейчас:** `llm-host` (Rust) — порты 8010/8011/8012, pid-файл
`data/llm-host.pid`, лог `data/logs/llm-host.log`, автозапуск — ярлык
`%APPDATA%\…\Startup\HermesDiskSearchLlmHost.lnk`. Python-роли `llama-server` остановлены и
сами не поднимаются (MCP/watcher Python-версии видят наши инстансы как «свои»).

```powershell
# состояние владельца и ролей
cargo run -p hds-llama --release --bin llm_host -- status
cargo run -p hds-llama --release --bin llm_host -- devices
Get-Content data\logs\llm-host.log -Tail 20
# остановить/поднять (остановка — graceful: инстансы + своя пауза + pid-файл)
cargo run -p hds-llama --release --bin llm_host -- stop
cargo run -p hds-llama --release --bin llm_host -- run
# откат на Python-роли (если нужно сравнить/перестраховаться)
.\.venv\Scripts\python.exe -m hds.llama_server start all
```

**Правила работы с живой машиной:**

* **ARB-сценарии требуют свободной VRAM**: сначала `llm_host stop`, потом
  `arb_scenarios.py` (он поднимает свои хосты на 8070–8072 и грузит модели), затем снова `run`.
* **Боевой `ask` занят на грани**: чат 7,9 ГБ (n_batch 512) + embedding 0,64 + CLIP-часть
  Python-клиента ~3,4 ГБ. Поэтому реранк **намеренно на CPU** (`-ngl 0` в конфиге) — с GPU
  `ask` падает 503 (§10.2b). Не «исправляйте» это, не проверив `ask` целиком.
* `config.yaml` **не в git** (личный): там уже `llm.chat.n_batch/n_ubatch: 512` и комментарий
  про реранк; в новом чате конфиг читать с диска, а не из репозитория.
* `index.pause` заказчика стоит (индексация на паузе) — не снимать без его решения; файл
  боевой, не временный.
* `hds.cli ask` — главный сквозной тест (`python -m hds.cli ask "<вопрос>"`): он проверяет
  поиск (наш :8011) → реранк (наш :8012, CPU) → ответ (наш :8010).

### 10.7. Грабли окружения (нового чата, не проекта)

* **`git` в этом терминале вешает любой pager**: всегда `git --no-pager -C <репо> …`
  (иначе `log`/`diff` зависают, и следующие команды «уходят» в pager).
* **PowerShell иногда добавляет посторонний символ к первому токену** команды
  (`сcargo …`) → команда падает с «не распознано как имя командлета»; помогает повтор
  запуска или начало команды с присваивания/`Start-Process`.
* **Кириллица в поиске по файлам через PowerShell ненадёжна** (кодировка консоли):
  `Select-String` с русским шаблоном может не найти совпадение, хотя оно есть. Ищите
  ASCII-шаблонами (`n_batch`, `[arbiter`) или читайте файл инструментом `read_files`.
* Многократные команды лучше объединять через `;` и писать вывод в файл
  (`Out-File -Encoding utf8`), затем читать его — буферизация PowerShell иначе «съедает» вывод.

```powershell
# боевой резидент (после остановки Python-ролей)
cargo run -p hds-llama --release --bin llm_host -- run
# управление (тот же процесс, ничего не запускает)
cargo run -p hds-llama --release --bin llm_host -- status
cargo run -p hds-llama --release --bin llm_host -- status --local --no-engine   # без резидента
cargo run -p hds-llama --release --bin llm_host -- devices
cargo run -p hds-llama --release --bin llm_host -- load rerank
cargo run -p hds-llama --release --bin llm_host -- unload rerank
cargo run -p hds-llama --release --bin llm_host -- stop
# проверочный прогон без VRAM (все роли на CPU-устройстве) и живой smoke
cargo run -p hds-llama --release --bin llm_host -- run --port-base 8030 --ngl 0 --no-residency --hold 60
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\resident_smoke.ps1 -PortBase 8030 -HoldSec 600
# задача Планировщика: регистрация, состояние, удаление
powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_llm_host_task.ps1 -Start
powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_llm_host_task.ps1 -Status
powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_llm_host_task.ps1 -Remove
```

## 11. B4 — отчёт (конвейер `process_file`, 01.10.2026)

### 11.1. Что сделано (по файлам)

* **новый крейт `crates/hds-core`** (§2.4): `config` (порт `hds/config.py`:
  `PROJECT_ROOT`, `APP_NAME`, `EMB_CONTEXT`, `config_path`/`HDS_CONFIG`, `load`,
  `dig`, `db_abs_path`, `replace_file` с ретраями), `db` (схема `index.db`
  **байт-в-байт**, `PRAGMA` WAL/NORMAL/FK/busy_timeout, vec0 через
  `sqlite3_auto_extension`, `meta.vec_dim` с пробой `SAVEPOINT`/`ROLLBACK`,
  бэкфилл `indexed_at`, CRUD `files`/`chunks`/`chunks_fts`/`chunks_vec`/`images_vec`),
  `http` (свой мини-HTTP — `crates.io` недоступен);
* **`crates/hds-index`**: `pipeline.rs` (`process_file` = фазы `extract_file` +
  `commit_file`, `clip_for_embedding`, `run_index` с паузой/стопом/heartbeat/
  прогрессом/prune, `reindex_path`), `progress.rs` (`ProgressReporter` —
  все поля `heartbeat_data`), `heartbeat.rs` (`index.heartbeat.json` + `SessionState`),
  `embed.rs` (клиент фасада `:8011`), `sidecar.rs` (трейты `Extractor`/`Lemmatizer`,
  клиент Python-воркера — прототип B6);
* **`hds/extract_sidecar.py`** — в B4 был тонкий Python-мост для паритета; **в B6
  заменён** автономным воркером `sidecar/hds_extract/worker.py` + клиентом
  `crates/hds-extract` (см. §13);
* воркспейс: `rusqlite 0.37` + `sqlite-vec 0.1.9` (из локального кэша cargo,
  `cargo --offline`), профиль `[profile.dev.package.sqlite-vec] opt-level = 2`.

### 11.2. Поведение (дословный порт `hds/indexer.py`)

Разделение фаз сохранено; `unchanged` (size + |Δmtime| < 2 + `status=indexed`),
`moved` (по `content_hash`, только если старый путь исчез), `force/full`,
`skipped_type`/`skipped_big`/`skipped_excluded`/`stat_error`, обрезка `max_chunks`
с предупреждением, `clip_for_embedding` (`(EMB_CONTEXT−256)·2.4`), батчи
эмбеддингов (`embedding.batch_size`, в `_commit_file` дефолт 32), запись
`chunks` + `chunks_fts` (лемматизированный) + `chunks_vec`, `finish_file`,
`prune` с блокировкой >20 %, `index.stop`/`index.pause`.

### 11.3. Осознанные отличия от Python

1. **R30**: `SessionState` (`Live`/`Paused`/`Stale`) — паузная/зависшая сессия
   **не блокирует** новый прогон (в Python свежий heartbeat на паузе блокировал всё).
2. **Вывод прогресса не зависит от читателя stdout** (грабля W0): рендер живёт в
   отдельном потоке, поэтому переполненный pipe не тормозит конвейер.
3. **Heartbeat пишется атомарно** (временный файл + `rename`) — UI не увидит частичный JSON.
4. `_clip_store` — **задел** (CLIP переезжает в Rust на ONNX в W3); в B4 CLIP-векторы не создаются.
5. `chunker` оставлен в `hds-index` (задача B3 принята) — перенос в `hds-core` (§2.4) отдельной задачей.
6. Поиск (`search_*.json`) в B4 не проверяется — порт поиска в W1.

### 11.4. Паритет и приёмка (числа)

| Проверка | Команда | Результат |
|---|---|---|
| Весь воркспейс | `cargo test --workspace` | **92 passed / 6 ignored / 0 failed**, предупреждений нет |
| Схема/БД | `cargo test -p hds-core` | `PRAGMA`, объекты схемы, vec0 0.1.9, `meta.vec_dim`, CRUD round-trip |
| Python↔Rust БД | `cargo test -p hds-core --test db_schema -- --ignored` | Python пишет чанк+FTS+vec в Rust-БД (`PY_OK 1 0.1.9`); Rust читает боевую |
| Конвейер (без сети) | `cargo test -p hds-index --test pipeline_core` | `skipped_*`, `unchanged`/`force`, `moved`, `error` изолирован, `indexed(0 чанков)`, `prune` >20 % |
| R30/прогресс | `cargo test -p hds-index --test heartbeat_progress` | `Live`/`Paused`/`Stale`, поля `heartbeat_data`, ETA/счётчики |
| **Паритет golden** | `cargo test -p hds-index --test pipeline_parity -- --ignored` | **16/16 файлов, 6363 чанка, 0 несовпадений** (segments/chunks/fts/hash строго) |
| Инкремент на копии | `cargo test -p hds-index --test pipeline_incremental -- --ignored` | копия боевой БД 4,9 ГБ: 3 файла `unchanged`, чанки совпали, Python читает (`612 220` чанков, vec 0.1.9) |

### 11.5. Грабли B4 (новые, стоило времени)

1. **Python на Windows читает stdin в ANSI-кодировке** — кириллические имена файлов
   приходят мозаикой; воркер принудительно переводит stdin в UTF-8.
2. **Библиотеки и ffmpeg пишут в `fd 1` напрямую** (не через `sys.stdout`) — воркер
   дублирует `fd 1`, уводит `fd 1` в stderr, а протокол пишет в дубликат (иначе
   «stream did not contain valid UTF-8» на `.mp4`/`.mpp`).
3. **Боевой `exclude_dirs` содержит `hermes-disk-search`** — фикстуры внутри проекта
   исключались (`skipped_excluded`); для паритета исключения сняты (golden.py их и
   так обходит, вызывая `extractors.extract` напрямую).
4. **Волатильность golden**: `видео_заставка_2сек` содержит **имя временного wav**
   (`tmpXXXX.wav`) в тексте ошибки транскрипции — сравнение нормализует `tmpXXXX`.
5. **`crates.io` недоступен** → `rusqlite`/`sqlite-vec` берутся из локального кэша
   cargo: сборка и тесты идут с `--offline`.

### 11.6. Что осталось на B4 (по желанию, не блокирует приёмку)

* замер памяти индексации 500 файлов против эталона W0 (`measure_run.py`) — после B5/B6;
* `keep-alive` HTTP-клиента эмбеддингов (сейчас `Connection: close`) — оптимизация;
* унификация `hds-core::http` с `hds-llama::http` — после W1.
## 12. B5 — отчёт (watcher, 01.10.2026)

### 12.1. Что сделано (по файлам)

* **`crates/hds-index/src/watch.rs`** — порт `hds/watcher.py`:
  * события ОС — **свой backend `ReadDirectoryChangesW`** (минимальный FFI
    `kernel32`, как проверка PID в `hds-llama::resident`), разбор
    `FILE_NOTIFY_INFORMATION` (ADDED/MODIFIED → `Modified`, REMOVED → `Deleted`,
    RENAMED_OLD+NEW → `Moved`); на не-Windows — опрос (macOS вне DoD W2);
  * `watch.lock` — **атомарный** (`create_new`) + снятие устаревшего (`lock_is_stale`);
  * `wait_stable` (debounce по размеру), `handle_event` (порт `_worker`),
    `run_watch` (наблюдатель → reconcile → цикл), `is_excluded`, `WatchState`;
* **тесты**: `tests/watch_core.rs` (разбор, lock, `wait_stable`, `handle_event` —
  без реальных событий), `tests/watch_live.rs` (**6 сценариев** на реальных событиях).

### 12.2. Почему свой backend, а не `notify`

`PLAN_W2_LLM_HOST.md` §2.4 предполагал `notify 8.x`, но на машине заказчика
**`crates.io` недоступен** (§9.7 п.13), а крейта `notify` **нет в локальном кэше**
cargo (есть лишь его транзитивные `filetime`/`mio`/`same-file`/`winapi-util`).
`watchdog`/`notify` на Windows и есть обёртка над `ReadDirectoryChangesW` —
реализовали её напрямую, без новых зависимостей.

### 12.3. Приёмка (числа)

| Проверка | Команда | Результат |
|---|---|---|
| Детерминированные | `cargo test -p hds-index --test watch_core` | **7 passed**: разбор ADDED/MODIFIED/REMOVED/RENAMED, атомарный lock + устаревший, `wait_stable`, `handle_event` (indexed/`.tmp`/корзина/rename/delete/ошибка) |
| Live (реальные события ОС) | `cargo test -p hds-index --test watch_live` | **6 сценариев**: create, modify, rename, delete, mass-write (10 файлов), корзина — 50,8 с |
| Весь воркспейс | `cargo test --workspace` | **100 passed / 6 ignored / 0 failed**, предупреждений нет |

### 12.4. Осознанные отличия от Python

1. Остановка — файл `index.stop` (как у индексатора) либо завершение процесса;
   `watch.lock` при жёстком убийстве снимается следующим стартом как устаревший
   (проверено тестом `stale_lock_is_reclaimed`).
2. `_lock_pid_is_watcher` (имя/командная строка процесса через `psutil`) не
   воспроизводим без новых зависимостей: живой PID считаем владельцем —
   **пессимистично**, как Python без `psutil`.
3. `status()` (глобальный `_state`) — пока библиотечный `WatchState::snapshot`;
   общий процесс-«модуль» появится в W1 вместе с MCP/UI.

### 12.5. Что осталось на B5 (по желанию)

* `watch.lock` на не-Windows (сейчас общий путь — тот же код, проверено на Windows);
* reconcile-прогресс в heartbeat (сейчас `quiet`) — при переносе UI в W1.
## 13. B6 — отчёт (sidecar-воркер и client, 01.10.2026)

### 13.1. Что сделано (по файлам)

* **`sidecar/hds_extract/worker.py`** — автономный Python-воркер (извлечение +
  лемматизация) по контракту §5: JSON-RPC 2.0/NDJSON, методы `hello`/`extract`/
  `normalize`/`clip_image`/`shutdown`, idle-timeout (`extract.idle_timeout`, 60 с),
  структурированные ошибки `{code, message, hint}`;
* **`sidecar/hds_extract/requirements.lock`** — зависимости воркера (вариант A,
  сняты с рантайма спайка 3: pymupdf 1.28.2, python-docx 1.2.0, openpyxl 3.1.5,
  python-pptx 1.0.2, pillow 12.3.0, pymorphy3 2.0.6 + dicts, PyYAML 6.0.3, lxml,
  pytesseract); **`sidecar/README.md`** — контракт и установка (A/C);
* **`crates/hds-extract`** — клиент: `protocol` (кадрирование/разбор, `Capabilities`,
  `ExtractResult`, `RpcError`), `worker` (`WorkerConfig`, `discover_python`,
  `Worker`: запуск + `hello`, `extract`/`normalize`/`clip_image`, перезапуск после
  N запросов / таймаута, поток-читатель с `recv_timeout`, `shutdown` через EOF,
  `pid` для замера RSS);
* **`crates/hds-index/src/sidecar.rs`** — стал тонким адаптером `hds-extract` к
  трейтам конвейера (`Extractor`/`Lemmatizer`); **`hds/extract_sidecar.py`** (B4-мост)
  удалён — заменён воркером B6.

### 13.2. Приёмка (числа)

| Проверка | Команда | Результат |
|---|---|---|
| Протокол (без процесса) | `cargo test -p hds-extract --test protocol` | 5 passed (кадрирование, id, ошибка+hint, разбор capabilities/extract/lemmas) |
| Клиент на mock-воркере | `cargo test -p hds-extract --test mock_worker` | 1 passed (hello/normalize/extract/ошибка/shutdown), 0,10 с |
| Реальный воркер | `cargo test -p hds-extract --test worker_live` | 2 passed (hello+extract+normalize+error+shutdown; перезапуск при `max_requests=1`), 0,6 с |
| Весь воркспейс | `cargo test --workspace` | **108 passed / 6 ignored / 0 failed**, предупреждений нет |
| Паритет B4 на воркере B6 | `cargo test -p hds-index --test pipeline_parity -- --ignored` | **16/16 файлов, 6363 чанка** — не сломался |
| Контракт §5 живьём (замер) | `data/_measure.py` (proc_tree) | старт `hello` **0,17 с**, RSS **32,6 → 61,3 МБ**, извлечение md 0,01 с / pdf 0,18 с, выход по EOF **0,07 с** (rc=0) |

Замер совпадает со спайком 3 (0,22 с / 34,5→72,7 МБ / 0,08 с) — контракт подтверждён
на автономном воркере, а не на пробном скрипте.

### 13.3. Грабли B6 (новые, стоило времени)

1. **Протокольный stdin наследуется подпроцессами.** `extract pdf` вешался на 120 с:
   `extract_pdf` зовёт `_tesseract_ready` → `pytesseract.get_tesseract_version()` →
   `tesseract.exe`, который наследует stdin-пайп воркера и **блокируется на чтении**.
   Решение в воркере: протокол читаем с **дубликата fd 0**, а сам fd 0 уводим в
   `os.devnull` — тогда любой подпроцесс (tesseract/ffmpeg/java) получает nul, а не
   канал протокола.
2. **`sys.stdin.reconfigure(encoding=...)` на Windows-pipe** ломает построчное
   чтение (строка доходила только после EOF) — читаем бинарно и декодируем UTF-8 сами.
3. **`hello` не должен запускать tesseract**: проверка OCR-возможности сделана
   дешёвой (`ocr_tesseract_cmd` или `shutil.which`), а не `get_tesseract_version()`.
4. Тест-клиент обязан давать воркеру таймаут и перезапуск: при ошибке тест «падал»
   на 120 с ожидания — теперь `request_timeout` ограничен, а регресс закрыт тестами.

### 13.4. Что осталось на B6 (не блокирует)

* вариант A (портативный python-build-standalone) ставит установщик — в dev
  используется `.venv`; поиск интерпретатора уже учитывает `sidecar/python` и
  `HDS_EXTRACT_PYTHON`;
* `clip_image` воркером не поддерживается по решению §7 (CLIP — в Rust на ONNX);
* `mpp` (Java) и `ffmpeg/whisper` внутри воркера работают через штатные
  `hds.extract_static`/`hds.extract_av` — отдельная изоляция (ASCII-стейджинг
  whisper, спайк 5) остаётся в W3.

## 14. B7 — отчёт (db-move и подкоманды CLI, 01.10.2026)

### 14.1. Что сделано (по файлам)

* **новый крейт `crates/hds-cli`** — библиотека `hds_cli` + тонкий `[[bin]] hds`
  (свой разбор argv: `clap` недоступен offline). Зависимости: `hds-core`,
  `hds-index`, `hds-extract`, `rusqlite`, `serde_json`; **на `hds-llama` зависимости
  нет** — фасад `:8010–8012` только по HTTP;
  * `src/main.rs` — диспетчер подкоманд (`exit`-коды как у Python; неизвестная
    подкоманда → 2);
  * `src/support.rs` — `open_conn`/`build_embedder`/`build_sidecar`,
    `parse_roots`/`parse_kinds`, `resolve_model` (порт `llama_runtime.resolve_model`
    + `_abs_model`, вкл. фолбэк на единственный `*.gguf`), `probe_role`
    (порт `llama_server.probe`), `props_context`, `tesseract_ready`, `which`,
    `fmt_local_datetime` (локально через `GetLocalTime`, с микросекундами как Python);
  * `src/cmd/{status,check,reindex,reindex_fts,forget,stop,clip_index,index,watch,db_move}.rs`;
* **`crates/hds-index/src/sidecar.rs`** — добавлен `Sidecar::spawn_with(py,cwd,parity,
  idle_timeout)` (аддитивно; `spawn` делегирует с 60 с);
* **`crates/hds-extract/src/worker.rs`** — `start_process` передаёт воркеру
  `--idle-timeout <sec>` (из `WorkerConfig`);
* **`sidecar/hds_extract/worker.py`** — `--idle-timeout` от клиента приоритетнее
  `extract.idle_timeout` из конфига.

### 14.2. Поведение (дословный порт Python)

* `status` — `db::stats`, те же поля (`by_kind/by_status/chunks/last_indexed_at/
  errors`), `--json` и человекочитаемый вид; дата — локальная, с микросекундами;
* `check` — компоненты db/roots/chat/emb/embctx/ocr/ffmpeg/lemmatizer/rerank,
  формат `[ok]/[--]/[!!]` + `-> fix`, итог и код возврата (порт `diag.run_checks`
  «по смыслу»);
* `reindex` — `pipeline::reindex_path` + воркер/эмбеддер, печать статусов;
* `reindex-fts` — `busy_timeout=600000`, `DELETE FROM chunks_fts` (3 попытки),
  батчи по 500 через воркер, `meta.fts_normalized='1'`;
* `forget` — `db::remove_path(conn, abspath)`; `stop` — создаёт `index.stop`;
* `index` — обёртка `pipeline::run_index`; `watch` — `hds_index::run_watch`
  (нужен и `db-move` для перезапуска);
* `db-move` — стоп watcher/index, копия, сверка счётчиков, **текстовая** правка
  `db_path` (комментарии целы), `.moved-<stamp>`, перезапуск watcher.

### 14.3. Осознанные отличия от Python (в `W2_REPORT`/док-комментариях)

1. **`db-move`: остановка процессов** без psutil — watcher по PID из `watch.lock`
   (`TerminateProcess`), индексация кооперативно (`index.stop` + ожидание).
2. **`db-move`: копия** — `VACUUM INTO` (у `rusqlite` фича `backup` не подключена);
   результат — консистентная копия, как `Connection::backup`.
3. **`db-move`: guard** — если исходной БД нет, понятная ошибка (Python создавал
   пустой файл и падал на «no such table»).
4. **`db-move`: перезапуск watcher** — наш `hds watch` (а не `pythonw -m hds.cli
   watch`); stdio фонового процесса отвязан (`null`) — иначе он держит пайпы
   вызывающего.
5. **`clip-index`** — заглушка (CLIP в Rust — W3); **`index --rechunk`** — «не
   поддерживается» (`run_rechunk` вне B7).
6. **`check`** не проверяет whisper/mpxj/Vulkan (нет возможности в воркере) —
   одна поясняющая заметка; эти пункты остаются в Python-версии до W5.
7. **Воркер в CLI** держится с `idle_timeout=3600` (batch; см. граблю 14.5.1).

### 14.4. Приёмка (числа)

| Проверка | Команда | Результат |
|---|---|---|
| Весь воркспейс | `cargo test --workspace` | **125 passed / 5 ignored / 0 failed**, предупреждений нет |
| Новые тесты B7 | `cargo test -p hds-cli` | 17 passed (db_move 4, reindex_fts 2, forget_status 2, check_core 4, support 5) |
| `status` vs Python | `hds status` / `python -m hds.cli status` (боевой конфиг, чтение) | поля совпадают (docx=2278 … indexed=71026, чанков 612220, дата `2026-10-01 08:35:22.569434`) |
| `check` vs Python | `hds check` / `python -m hds.cli check` | совпадает по смыслу (db/roots/chat/emb/ocr/ffmpeg/lemmatizer ok, rerank warn, итог «готовы») |
| **`reindex-fts`** | копия боевой БД (10 000 чанков) → `hds reindex-fts` → Python | `chunks_fts` перестроен (612 220 «мусорных» → 10 000), `meta.fts_normalized='1'`; Python: **500/500** сэмплов `chunks_fts == lemmatizer.normalize(chunks.text)`; время **131,8 с** |
| **`db-move`** | копия → `hds db-move --to …\moved\index.db` | БД перенесена, счётчики сошлись (71 993 файла / 9 999 чанков), **комментарий сохранён**, `db_path` обновлён, старый → `index.db.moved-20261001-130550`; Python читает новую БД |
| **`forget`** | `hds forget "<боевой путь>"` на копии | rc=0 «Удалено из индекса»; файлов 71 994→71 993, чанков 10 000→9 999 (у файла 1 чанк) |
| `sqlite-vec` 0.1.9 | `db::connect` | через `sqlite3_auto_extension` (без `load_extension`) — как в B4 |

### 14.5. Грабли B7 (новые, стоило времени)

1. **Idle-timeout воркера vs долгие операции родителя.** `reindex-fts` падал
   «воркер: запись: Идёт закрытие канала (os error 232)»: `DELETE FROM chunks_fts`
   на 612 220 старых строк занимал > 60 с, а воркер всё это время не получал
   запросов и **выходил по idle-timeout**. Причём таймаут задаёт **сам воркер**
   (`extract.idle_timeout`, по умолчанию 60 с, из проектного `config.yaml` — не из
   `HDS_CONFIG` и не из `WorkerConfig`). Решение: клиент передаёт `--idle-timeout`
   (worker.py его читает и он приоритетнее конфига), CLI ставит 3600 с.
   Тот же риск был и в `index` (долгая транскрипция между `normalize`) — закрыт тем же.
2. **Отсоединённый watcher наследует stdio родителя.** `db-move` перезапускал
   `hds watch` через `Command::spawn` без редиректа — фоновый процесс держал пайпы
   вызывающего, и команда «не завершалась». Решение: `stdin/stdout/stderr → null`.
3. **`VACUUM INTO` вместо backup API** — у `rusqlite` фича `backup` не подключена
   (`Cargo.toml`: `bundled`+`load_extension`), а включать новую фичу offline
   рискованно; `VACUUM INTO` даёт тот же результат без изменений зависимостей.
4. **Кириллица в argv через PowerShell** искажается (проверка `forget` на боевом
   пути) — приёмку гоняли Python-драйвером (`data/_b7_live.py`), где argv передаётся
   wide-API Windows; `std::env::args()` в Rust читает Unicode корректно.
5. **`resolve_model` для `shared:<role>`** без `current.json`: Python берёт
   **единственный** `*.gguf` каталога (иначе каталог) — без этого фолбэка `check`
   ложно называл живую embedding-роль «посторонним сервисом».

### 14.6. Что осталось на B7 (не блокирует)

* `index --kinds` (фильтр видов в `run_index` B4 не портирован) и `index --rechunk`
  (`run_rechunk`) — отдельные задачи; ключи принимаются с понятным сообщением;
* `clip-index` — W3 (CLIP на ONNX);
* `serve`/`ui`/`mcp-http`/`whisper-check`/`vulkan-setup` — вне B7 (MCP/UI — W1, медиа — W3);
* полноразмерный `reindex-fts` по всей боевой БД (612k чанков, ~30–90 мин) — на
  приёмку заказчика; код-путь проверен на подмножестве из тех же реальных чанков.

## 15. B-2 — пилот паритета индексации (10 000 файлов, 01.10.2026)

### 15.1. Методика

* Полигон §12.3 (реальные папки) — ~2 500 файлов и **~38 ГБ** (доминирует
  `Журнал_MC_2013`: 36,5 ГБ, 378 `.jpg`+270 `.tif` → OCR). Для детерминированного
  паритета **масштаба** построено синтетическое дерево `D:\_hds_pilot`: **10 000**
  текстовых файлов (`.txt/.md/.log/.csv`, 100 каталогов, каждый 10-й — с
  кириллическим именем; содержимое разной длины даёт 1…20 чанков при `size=800`).
  Дерево — **вне репозитория** (иначе `exclude_dirs: hermes-disk-search` его
  исключает); текст без OCR/медиа (транскрипция — W3).
* Два **идентичных** конфига (roots=`D:\_hds_pilot`, `ocr:false`, `transcribe:false`,
  стандартные `exclude_dirs`, `llm_server.autostart:false`), различается только
  `db_path`.
* Прогоны: `python -m hds.cli index --quiet` и `target\debug\hds.exe index --quiet`
  — оба по фасаду эмбеддингов `:8011` (bge-m3, 1024). Сверка —
  `tools/parity/pilot_parity.py compare` (файлы, чанки, `chunk_count`,
  `content_hash`, тексты, лемматизированный FTS).

### 15.2. Результат

| Метрика | Python | Rust | Итог |
|---|---|---|---|
| файлов | 10 000 | 10 000 | = |
| чанков | 58 450 | 58 450 | = |
| `(chunk_count,content_hash)` расхождений | — | — | **0** |
| тексты чанков (ключ = путь,`ord`) | — | — | **0** расхождений |
| `chunks_fts` (лемматизация) | — | — | **0** расхождений |
| время индексации | 1033 с | 1106 с | **+7,1 %** (порог ≤ +10 %) |
| БД читается Python-версией | — | — | да (`sqlite3` open + join FTS) |

`PARITY: OK`. `indexed_at` различается по определению (разное время прогонов);
сравнивались `chunk_count`/тексты/хэши/FTS.

### 15.3. Вывод и воспроизведение

B-2 (пилот) закрыт на 10 000 файлах: конвейер Rust даёт **те же** чанки, тексты,
FTS и `content_hash`, что Python, и укладывается в допуск по времени (+7,1 %; цена —
батчевые round-trip'ы к Python-воркеру вместо in-process лемматизатора). Полный
прогон по реальному полигону §12.3 (включая OCR-журнал) — ночным заданием.

```powershell
.\.venv\Scripts\python.exe tools\parity\pilot_parity.py gen   # D:\_hds_pilot (10 000 файлов)
# два конфига (§15.1), затем:
.\.venv\Scripts\python.exe -m hds.cli index --quiet           # HDS_CONFIG=<py.yaml>
target\debug\hds.exe index --quiet                            # HDS_CONFIG=<rust.yaml>
.\.venv\Scripts\python.exe tools\parity\pilot_parity.py compare <py.db> <rust.db>
```

## 16. B-4 — замеры памяти (01.10.2026)

### 16.1. Методика

* Метрики и сценарии — как в W0 (§13.1): `WorkingSet64` (ws) и `PrivateMemorySize64`
  (commit) **по дереву процесса** (`tools/parity/sample_tree.ps1`), изолированные
  конфиг/БД в `tools/parity/out/`, один bench из 500 файлов (текст + копии
  docx/xlsx/pptx/pdf). Драйвер — `tools/parity/measure_tree.py` (гнёт сценарии A/B
  **и для Python, и для Rust** на одной машине/bench — честное сравнение).
* Перед прогоном останавливается боевой watcher (иначе держит `watch.lock` и
  heartbeat блокирует Python-индекс); `index.pause` скрипт убирает и возвращает сам.

### 16.2. Результат

| Сценарий | Python (сейчас) | Rust (`hds`) | Эталон W0 | Итог |
|---|---|---|---|---|
| индексация 500 файлов | 48,3 с, ws **124**, commit **595** | 50,8 с, ws **137,9**, commit **601** | 48,2 с, ws 122, commit 597 | время **+5,2 %**, ws **+13 %**, commit **+0,7 %** — в допуске +20 % |
| простой (watch) | ws **50,7**, commit **518** | ws **54**, commit **37,7** | ws 46, commit 518 | ws **+17 %**, commit **−93 %** — в допуске |

Числа сходимы с B-2 (§15: +7,1 % по времени). В простое Rust-дерево включает
**заранее запущенный sidecar-воркер**, но commit в разы ниже, чем у Python-процесса
(в Rust-владельце нет torch/pymorphy3 — они в лёгком воркере). Результаты —
`tools/parity/out/measure_tree_results.json`.

### 16.3. Воспроизведение

```powershell
# остановить боевой watcher (иначе watch.lock/heartbeat мешают), затем:
.\.venv\Scripts\python.exe tools\parity\measure_tree.py   # index.pause вернёт сам
# после — поднять watcher обратно
```



