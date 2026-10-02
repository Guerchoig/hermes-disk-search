# tools/parity — паритет-harness W0 и карта артефактов

> **Коротко.** Здесь живёт всё, что нужно, чтобы (а) проверять паритет новой реализации с текущей
> Python-версией и (б) воспроизводить замеры W0. Журнал результатов с цифрами —
> `SPIKES.md` (читать первым), журнал W2 — `W2_REPORT.md`, журнал W3 (медиа/CLIP) —
> `W3_REPORT.md`, журнал W1 (поиск/MCP/UI) — `W1_REPORT.md`, детальный план W2 —
> `../PLAN_W2_LLM_HOST.md`,
> основной план — `../MIGRATION_PLAN_RUST.MD` (там §4 W0, §10.0 статус платформ, §12/§13 приёмка).
> Все скрипты запускаются из **корня репозитория** интерпретатором проекта:
> `.\.venv\Scripts\python.exe <скрипт>` (Windows) / `python3 <скрипт>` (macOS).
> Rust-проверки — крейты воркспейса `crates/` (`cargo test`, `cargo run -p hds-llama …`).


## 1. Состав каталога

### Генераторы и приёмка (главное)
| Файл | Что делает |
|---|---|
| `gen_fixtures.py` | создаёт `fixtures/` (16 файлов): md/docx/xlsx/pptx/pdf/pdf-скан(OCR)/csv+лог(>max_chunks)/jpg с EXIF и без/wav+mp4/реальный `.mpp`/русская речь 60 с |
| `golden.py` | прогон Python-версии по фикстурам → `golden/` (сегменты, чанки, FTS-текст, `content_hash`, топ-20 по 10 запросам, `manifest.json`) |
| `slim_golden.py` | сжимает крупные золотые файлы в `.json.gz` (28,85 МБ → 1,87 МБ — так они и коммитятся) |
| `compare.py` | сверка новой реализации с golden: строго (сегменты/чанки/FTS/hash) + с допуском (состав топ-20; порядок — только между равными скорами) |
| `golden_queries.py` | golden-прогон двух контрольных запросов заказчика на **боевой БД read-only** → `golden/real_db_queries.json` |
| `queries.txt` | 10 контрольных запросов golden-поиска |

### Спайки (замеры W0)
| Файл | Что делает |
|---|---|
| `spike2_hash.py` + `spikes/` (`hash_parity`) | паритет `content_hash`: Python-эталон (50 файлов, вкл. >512 КБ) → Rust Blake2b-16 (50/50) |
| `spikes/tests/spike1_db.rs` | чтение боевой `index.db` из Rust: FTS5 + KNN + `vec_version` (`#[ignore]`) |
| `spikes/src/bin/dll_probe.rs` | загрузка DLL движка через libloading, резолв символов |
| `probe6_devices.py` | сверка `memory_free` движка с `nvidia-smi` («до → движок → после», контроль дрейфа) |
| `spike6_parity.py` | паритет embeddings/rerank/chat: движок vs текущий llama-server |
| `spike6_vram.py`, `spike6_gpu_diag.py`, `spike5_gpu.py` | устройство инференса: CPU vs GPU (`--devices`, `--whisper-gpu-device`), VRAM, тайминги |
| `spike3_sidecar.py`, `spike3_worker.py` | портативный воркер (python-build-standalone + site-packages): размер, старт, RSS, извлечение |
| `spike4_clip_onnx.py` | экспорт CLIP (vision+text) в ONNX и паритет cos |
| `spike5_whisper.py`, `spike5_compare.py`, `spike5_russian.py`, `spike5_threeway.py` | транскрипция движком: ASCII-стейджинг, WER, сравнение с faster-whisper |
| `probe_engine.py`, `probe_engine_args.py` | разведка `example-cli`: `help`, `list-devices`, подбор аргументов подкоманд |

### Замеры памяти и лемматизация
| Файл | Что делает |
|---|---|
| `measure_live.py` + `sample_procs.ps1` | серия замеров «как есть»: WS + **PrivateWS** + commit + VRAM (тяжёлый сэмплер, интервал 20 с) |
| `measure_run.py` + `sample_procs_light.ps1` | сценарии A–D на **изолированных** `out/measure.yaml` + `out/measure.db`: индексация 500 файлов, простой с watcher, транскрипция, первый поиск |
| `proc_tree.ps1` | память дерева процессов (роли — пары launcher→worker) |
| `lemma_baseline.py` | корпус лемматизации: 100 000 токенов из боевой БД + эталон pymorphy3 + тайминги |

### W2: инструменты трека A (llm-host) и B (ядро)
| Файл / крейт | Что делает |
|---|---|
| `W2_REPORT.md` | журнал W2 с цифрами: A1 (выбор устройства, NVML, скорость), B1 (обход/лимиты), B2 (`content_hash`) и находки, которых не было в плане |
| `crates/hds-llama` (`bin/a1_device_probe`) | A1: инстанс пятью способами (`none`, CSV-индекс, индекс+1, имя, `allow_cpu=false`), NVML-пик и скорость → `out/w2_a1_device.json` |
| `crates/hds-llama` (`bin/llm_host_plan`) | A2: сухой прогон плана инстансов по `config.yaml` (роль → модель из общего рантайма, устройство, `n_ctx`/`ngl`/retention) → `out/w2_a2_plan.json` |
| `crates/hds-llama` (`bin/a3_instance_probe`) | A3: кросс-процессная проверка (`--hold` держит инстанс, `--list`/`--call` из другого процесса) |
| `crates/hds-llama` (`bin/vram_budget`) | A4 (шаг 1): бюджет VRAM по ролям — метаданные GGUF, KV f16/q8_0, вердикт «влезает/не хватает» → `out/w2_a4_budget.json` |
| `crates/hds-llama` (`bin/llm_host_status`) | A4 (шаг 2): `hdsw llm-host status` — устройства, NVML-бюджет, роли/состояния, `index.pause`+heartbeat, **прогноз диспетчера**; `--json` (поля для UI) → `out/w2_a4_status.json` |
| `crates/hds-llama` (`bin/llm_host_dispatch`) | A4 (шаг 2): решение диспетчера на живом движке (`--kind query|indexing`, `--budget-mb` для A-7, `--apply`: пауза → выгрузка по приоритетам → загрузка → снятие паузы) → `out/w2_a4_dispatch*.json` |
| `crates/hds-llama` (`bin/gguf_dump`) | A4 (замер KV): полный дамп метаданных GGUF + сводка по KV (слои с полным вниманием, КиБ/токен, гибридные признаки) → `out/w2_chat_meta.json` |
| `crates/hds-llama` (`bin/kv_probe`) | A4 (замер KV): поднимает кластерный инстанс и мерит фактический KV (NVML-дифференциал по `n_ctx` + данные движка; отказывается грузить при нехватке VRAM) → `out/w2_kv_probe.json` |
| `crates/hds-llama` (`bin/chat_probe`) | A5: разведка контракта чата — применяет ли движок шаблон сам, работает ли `reasoning=off` через кластер, видны ли размышления при `on+format=none` → `out/w2_chat_contract.json` |
| `crates/hds-llama` (`bin/llm_host_facade`) | A5: живой фасад `:8010–8012` (инстансы по конфигу + диспетчер A4 + HTTP), `--port-base`/`--ngl`/`--hold`/`--json` → `out/w2_facade.json`; **с A6 — тонкий бинарь** над `host::Host` (разовый прогон) |
| `crates/hds-llama` (`bin/llm_host`) | A6: резидентный владелец GPU + CLI (`run`/`status`/`load`/`unload`/`devices`/`stop`) через внутренний API фасада; pid `data/llm-host.pid`, лог `data/logs/llm-host.log` |
| `crates/hds-llama/src/host.rs` | A6: сборка живой машины (движок → инстансы → фасад → диспетчер), режимы `embedded`/`facade`/`off`, уборка инстансов и своей паузы |
| `crates/hds-llama/src/resident.rs` | A6: pid-файл (эксклюзивно, устаревший снимается), лог-файл, проверка живого PID |
| `facade_smoke.ps1` | A5: живая проверка фасада одной командой (альтернативные порты, чат на CPU; `/health`, `/props`, чат, `chat-think`, эмбеддинги) |
| `resident_smoke.ps1` | A6: живая проверка резидентности (pid+лог, отказ второму экземпляру, `/internal/*`, `stop` освобождает pid-файл) |
| `arb_scenarios.py` | A6: ARB-1…6 на живом движке (свои порты/конфиги/сигналы) → `out/w2_arb.json` |
| `crates/hds-index` (`tests/hash_parity`, `tests/walk_parity`, `tests/chunker_parity`) | B1/B2/B3: фиксированные векторы хэша, 50 реальных файлов, обход/исключения/лимиты против Python-дампа, чанкер против golden (16 фикстур / 6 363 чанка) |
| `crates/hds-core` | B4: общий слой ядра — `config` (порт `hds/config.py`), `db` (схема `index.db` 1:1, PRAGMA, vec0, CRUD), `http` (свой мини-HTTP) |
| `crates/hds-index` (`src/pipeline.rs`, `progress.rs`, `heartbeat.rs`, `embed.rs`, `sidecar.rs`) | B4: конвейер `process_file` (фазы extract/commit, `clip_for_embedding`, `max_chunks`, prune), прогресс+heartbeat (R30), клиент фасада `:8011`, клиент Python-воркера |
| `hds/extract_sidecar.py` | B4: тонкий Python-мост (`hds.extractors`+`hds.lemmatizer`) для паритета `segments`/`fts` (прототип B6) |
| `crates/hds-index` (`tests/pipeline_parity`, `tests/pipeline_incremental`, `tests/pipeline_core`, `tests/heartbeat_progress`) | B4: паритет golden 16/16 (6363 чанка), инкремент на копии боевой БД, инкремент/ошибки без сети, R30 — `#[ignore]` для внешних зависимостей |
| `crates/hds-index/src/watch.rs` | B5: watcher — свой backend `ReadDirectoryChangesW` (FFI, `notify` недоступен offline), `watch.lock` (атомарный + устаревший), `wait_stable`, `handle_event`, `run_watch`, reconcile |
| `crates/hds-index/tests/watch_core.rs`, `tests/watch_live.rs` | B5: разбор событий/`watch.lock`/`handle_event` + 6 сценариев на реальных событиях ОС (create/modify/rename/delete/mass-write/корзина) |
| `crates/hds-extract` | B6: клиент воркера — `protocol` (JSON-RPC 2.0 NDJSON), `worker` (интерпретатор, `hello`, `extract`/`normalize`/`clip_image`, перезапуск/таймаут/EOF, `shutdown`) |
| `sidecar/hds_extract/worker.py`, `requirements.lock`, `sidecar/README.md` | B6: автономный Python-воркер извлечения/лемматизации, зависимости (вариант A), контракт §5 |
| `crates/hds-extract/tests/{protocol,mock_worker,worker_live}.rs` | B6: протокол, mock-воркер (stdlib-only), реальный воркер (hello/extract/normalize/ошибка/перезапуск/shutdown) |
| `crates/hds-cli` (bin `hds`) | B7: db-move и подкоманды CLI — `status`/`check`/`reindex`/`reindex-fts`/`forget`/`stop`/`clip-index`/`index`/`watch`/`db-move` (свой разбор argv, фасад только по HTTP) |
| `crates/hds-cli/tests/{db_move,reindex_fts,forget_status,check_core,support}.rs` | B7: db-move (комментарии config, `.moved-*`), reindex-fts (перестройка FTS + `meta.fts_normalized`), forget/status, check (db/roots), support (roots/kinds/`resolve_model`) |
| `pilot_parity.py` | B-2: генератор пилота (10 000 текст. файлов) и дифф двух БД индексации (файлы/чанки/тексты/FTS) → паритет Python↔Rust 10 000 файлов / 58 450 чанков, +7,1 % по времени |
| `measure_tree.py` + `sample_tree.ps1` | B-4: замеры памяти по дереву процесса (`WorkingSet64`/`PrivateMemorySize64`) сценариев index-500 и idle-watch для Python и Rust → `out/measure_tree_results.json` |
| `W3_REPORT.md` | журнал волны W3 (медиа): разведка аудио-API движка и рантайма `ort`, план по файлам |
| `pe_exports.py` | список экспортов PE-файла (разведка C-API движка): `pe_exports.py <dll> [подстрока]` |
| `hash_vectors.py` | фиксированные векторы `content_hash` (Python-эталон) → `out/hash_vectors.json` |
| `walk_parity.py` | эталон обхода/`precheck`: синтетическое дерево (все ветки исключений/лимитов) + опционально боевые корни (`--real`) → `out/walk_parity.json` |

### Данные
| Путь | Что это |
|---|---|
| `golden/` | **золотые файлы (в git, 1,87 МБ)**: `*.segments.json`, `*.chunks.json`, `*.fts.json`, `search_NN.json`, `manifest.json`, `hash_manifest.json`, `real_db_queries.json`; крупные — как `.json.gz` |
| `fixtures/` | фикстуры (в git **не** хранятся — воспроизводятся `gen_fixtures.py`) |
| `out/` | результаты замеров (JSON), изолированные конфиг/БД, воркер, ONNX-модели, логи прогонов; тяжёлое исключено в `.gitignore` |
| `measurements/` | сырые серии замеров памяти |
| `MAC_CHECKLIST.md` | чек-лист постпроектной проверки macOS (§10.0 основного плана) |
| `spikes/` | Rust-крейт: `cargo test --test spike1_db -- --ignored`, `cargo run --bin hash_parity`, `cargo run --bin dll_probe -- <dll>` |

## 2. Команды (copy-paste)

```powershell
# --- приёмка паритета (главное) ---
.\.venv\Scripts\python.exe tools\parity\gen_fixtures.py          # фикстуры (16 файлов)
.\.venv\Scripts\python.exe tools\parity\golden.py                # золотые файлы (нужны эмбеддинги)
.\.venv\Scripts\python.exe tools\parity\slim_golden.py           # сжать крупные в .gz (перед коммитом)
.\.venv\Scripts\python.exe tools\parity\compare.py --actual tools\parity\golden   # самопроверка → 59/59

# --- паритет хэша и БД (Rust) ---
.\.venv\Scripts\python.exe tools\parity\spike2_hash.py
cd tools\parity\spikes; .\target\release\hash_parity.exe; cd ..\..\..
cd tools\parity\spikes; cargo test --test spike1_db -- --ignored --nocapture; cd ..\..\..

# --- golden по боевой БД (read-only) ---
.\.venv\Scripts\python.exe tools\parity\golden_queries.py

# --- W2: Rust-ядро (воркспейс `crates/`) ---
cargo test --workspace                                             # 47 проверок (hash/walk/chunker + A2/A4)
cargo test -p hds-index --test chunker_parity -- --nocapture         # B3: 16 фикстур / 6 363 чанка (golden)
cargo test -p hds-index --test hash_parity -- --ignored --nocapture   # 50/50 на реальных файлах
.\\.venv\\Scripts\\python.exe tools\\parity\\walk_parity.py --real
$env:HDS_WALK_PARITY_REAL='1'; cargo test -p hds-index --test walk_parity -- --nocapture
cargo run -p hds-llama --release --bin a1_device_probe             # устройство/VRAM/скорость (A1)
cargo run -p hds-llama --release --bin llm_host_plan               # план инстансов по config.yaml (A2)
cargo run -p hds-llama --release --bin a3_instance_probe -- --hold 45   # A3: два процесса (см. W2_REPORT §6)
cargo run -p hds-llama --release --bin vram_budget                 # A4-1: бюджет «модель + KV» по ролям
cargo run -p hds-llama --release --bin llm_host_status             # A4-2: статус (роли, VRAM, пауза, прогноз)
cargo run -p hds-llama --release --bin llm_host_dispatch -- --role chat            # A4-2: решение диспетчера
cargo run -p hds-llama --release --bin llm_host_dispatch -- --role chat --apply    # A4-2: выполнить (пауза/выгрузка/загрузка)
cargo run -p hds-llama --release --bin gguf_dump -- --role chat                    # A4: метаданные модели (гибридные слои, KV)
cargo run -p hds-llama --release --bin kv_probe -- --role chat --ngl 8 --n-ctx 4096,32768   # A4: фактический KV движка
cargo run -p hds-llama --release --bin chat_probe -- --json tools\parity\out\w2_chat_contract.json   # A5: контракт чата
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\facade_smoke.ps1 -PortBase 8020     # A5: живой фасад

# --- W3: медиа-ветка (ASR движком openresearchtools; владелец GPU — llm-host) ---
cargo run -p hds-llama --bin audio_probe -- <audio> [<whisper.bin>] [<gpu>]   # прямой прогон whisper.rs
cargo run -p hds-cli   --bin hds -- whisper-check [--file <медиа>] [--json]   # движок+модель (+ живой /internal/transcribe)
# throwaway-владелец на альт-портах для приёмки, не трогая боевые роли/паузу:
#   target\debug\llm_host.exe run --port-base 8020 --ngl 0 --no-residency --no-log `
#     --pause-dir tools\parity\out\w3_host_pause --hold 1800     # см. W3_REPORT.md §4

# --- W3: CLIP на ONNX Runtime (модели спайка 4: out/clip_onnx/) ---
.\.venv\Scripts\python.exe tools\parity\clip_onnx_w3.py        # экспорт text-ONNX (pooling+Dense) + golden out/clip_parity.json
cargo test -p hds-clip --test clip_parity -- --ignored --nocapture   # паритет vision+text: cos >= 0,999
.\.venv\Scripts\python.exe tools\parity\w3_clip_smoke.py setup # живая проверка hds clip-index (W3_REPORT.md §5)

# --- W1: порт поиска (hds-search) ---
cargo test -p hds-search --test search_parity -- --ignored --nocapture  # паритет golden search_*.json (требует out/index.db + фасад :8011)
# target\debug\hds.exe search 'запрос' --limit 8   (HDS_CONFIG=out/w1_search.yaml, БД фикстур)
# target\debug\hds.exe ask 'вопрос'  --limit 8    (тот же конфиг; чат-роль :8010)

# --- W1: MCP-сервер (stdio) ---
target\debug\hds.exe mcp      # эквивалент `hds_mcp`; клиенты (Cline/Hermes) общаются по JSON-RPC 2.0
# --- W1: MCP streamable-http (ОДИН инстанс на машину) ---
target\debug\hds.exe mcp-http check|start|stop|status|restart|run [--host --port --path]
target\debug\hds.exe mcp --http --port 8787   # сервер в foreground (endpoint /mcp, probe /health)

# --- W1: веб-интерфейс (перепроектированный под Rust) ---
target\debug\hds.exe ui --port 8765          # http://127.0.0.1:8765 (статус/поиск/ask/индексация)

# --- паритет с движком ---
.\.venv\Scripts\python.exe tools\parity\probe6_devices.py        # memory_free vs nvidia-smi
.\.venv\Scripts\python.exe tools\parity\spike6_parity.py         # embeddings/rerank/chat
.\.venv\Scripts\python.exe tools\parity\spike6_gpu_diag.py       # CPU vs GPU (устройство обязательного задавать!)
.\.venv\Scripts\python.exe tools\parity\spike5_gpu.py            # whisper GPU по умолчанию: ×16

# --- замеры памяти (на изолированной БД, боевой индекс не затрагивается) ---
.\.venv\Scripts\python.exe tools\parity\measure_run.py           # сценарии A–D → out/measure_results.json
.\.venv\Scripts\python.exe tools\parity\measure_live.py          # серия «как есть» → out/measure_live.json
.\.venv\Scripts\python.exe tools\parity\lemma_baseline.py        # корпус + тайминги лемматизации
```

## 3. Грабли, которые уже стоили времени (не повторять)

1. **Не читать вывод долгих процессов из `PIPE`** — переполнение 64 КБ останавливает процесс
   (в W0 индексация «вставала» ровно на 34-м файле). Пишите вывод в файл.
2. **Роли — пары launcher→worker**: `terminate()` убивает только родителя; используйте
   `taskkill /F /T` (иначе выживший watcher держит `index.heartbeat.json` и блокирует
   следующие запуски «в другом процессе уже идёт индексация»).
3. **`index.pause`** в корне проекта заставляет НОВЫЕ прогоны стартовать на паузе; для
   изолированных замеров файл временно убирается и возвращается.
4. **Сэмплер может тормозить замеряемую нагрузку**: `Win32_PerfFormattedData` по всем процессам
   тяжёлый → для сценариев берите `sample_procs_light.ps1` (WS + commit), тяжёлый сэмплер — только
   для точечных замеров PrivateWS.
5. **`WorkingSet` вводит в заблуждение** (llama-server: WS 1,5 ГБ против commit 13,3 ГБ).
   Сравнивайте три метрики: WS, PrivateWS, commit + VRAM.
6. **Паритет хэша**: исключайте волатильные файлы (логи, `index.heartbeat.json`) или переснимайте
   эталон прямо перед прогоном — иначе 48/50 вместо 50/50.
7. **golden.py требует живой embedding-роль** (`:8011`); без неё векторная ветка выключится и
   золотые файлы поиска «поедут».
8. **AV**: на машине заказчика Windows Defender выключен, активен Kaspersky (`avp.com`); папки
   CLI не сканирует — папку воркера проверяют из Проводника (`SPIKES.md` §5).
9. **Движок грузит ggml-бэкенды относительно ТЕКУЩЕГО каталога процесса**: без cwd = каталог
   движка `list_devices` пуст и инференс уходит на CPU при `n_gpu_layers = -1` (находка A1,
   `W2_REPORT.md` §1.3). В обвязке — `Engine::activate()`; отдельно: каталог движка
   **не самодостаточен**, зависимости `avcodec-62.dll` и пр. лежат в `Engine\vendor\ffmpeg\bin`
   (без них `LoadLibraryExW` даёт код 126).
10. **Длинные пути (>260)**: Rust (`\\?\`) статит и читает их, Python — нет (`WinError 3`).
    В паритете такие пути дают `SkippedStat` на стороне Python — это осознанное расхождение
    (Rust-ядро сможет проиндексировать больше файлов), а не дефект.
11. **`precheck` сравнивать с оглядкой на волатильность**: эталон `walk_parity.py` переснимайте
    перед прогоном Rust-теста (как в спайке 2), иначе логи/`index.heartbeat.json` дадут шум.
12. **`accept()` от неблокирующего слушателя (Windows)** отдаёт неблокирующий сокет → keep-alive
    рвётся после первого ответа; в `http.rs` сокет переводится в блокирующий режим.
13. **`crates.io` — перепроверено 01.10.2026: ДОСТУПЕН.** В W0 фиксировалось обратное
    («`index.crates.io` не резолвится»), и под это были приняты решения: свой мини-HTTP
    (`crates/hds-llama/src/http.rs`), свой разбор argv вместо `clap`, свой
    `ReadDirectoryChangesW` вместо `notify`. Живая проверка **01.10.2026** (без изменения
    настроек, прокси/`CARGO_NET_OFFLINE` не заданы): `index.crates.io` и `static.crates.io`
    отвечают **200**, `cargo` обновляет индекс, крейт **`ort 2.0.0-rc.13` скачивается и
    собирается** (подробности и числа — `W2_REPORT.md` §17). **Ограничение считаем снятым:**
    новые зависимости добавлять можно. Уже написанные «под ограничение» части остаются как
    есть (переписывать не обязательно); для будущих задач (W3/W4) посылку не считать аксиомой.
14. **`.ps1` держим ASCII-only**: PowerShell 5.1 читает скрипт без BOM как ANSI и ломается
    на кириллице (пример — `facade_smoke.ps1`, `resident_smoke.ps1`,
    `installers/install_llm_host_task.ps1`). Смежное (поймано A6): при
    `$ErrorActionPreference='Stop'` **stderr нативной программы в pipeline — терминальная
    ошибка** (`NativeCommandError`); в строках писать `${var}`, а не `$var:` (`$var:` = имя
    диска). Живой пример — обёртка `Run` в `resident_smoke.ps1`.
15. **`--ngl 0` ≠ «на CPU»** (A6, `W2_REPORT.md` §10.3): движок зовёт `llama_params_fit` и
    может офлоаднуть модель обратно, если **устройство** — CUDA (`offloaded 33/33 layers`
    при `n_gpu_layers = 0`). Роль держит на CPU только устройство (`manual_devices_csv`),
    и `role_needs` для такой роли — 0 МиБ.
16. **Терминал разработки (не проект), стоило времени в A6** — подробнее `W2_REPORT.md` §10.7:
    `git` без `--no-pager` **зависает в пейджере** (следующие команды «уходят» в него);
    PowerShell иногда добавляет посторонний символ к первому токену команды; `Select-String`
    с **кириллическим** шаблоном молча не находит совпадения (искать ASCII-шаблонами или
    читать файл инструментом).

## 4. Что читать первым в новом чате

**Чек-лист на 5 минут (копипаст):**
```powershell
git --no-pager -C <репозиторий> log --oneline -3   # ветка w2-llm-host (20 коммитов, 30.09.2026)
cargo test --workspace                       # должно быть 108 green (+6 #[ignore])
cargo run -p hds-llama --release --bin llm_host_status                       # состояние ролей и VRAM
cargo run -p hds-llama --release --bin llm_host -- status                    # отчёт РЕЗИДЕНТА (владелец портов)
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\facade_smoke.ps1 -PortBase 8020 -HoldSec 20
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\resident_smoke.ps1 -PortBase 8030 -HoldSec 600
```
Если команды не запускаются — смотрите «Грабли» (ниже), `W2_REPORT.md` §9.7 и **§10.7**.
**ARB-сценарии перед прогоном требуют свободной VRAM**: сначала `llm_host stop`, потом
`.\\.venv\\Scripts\\python.exe tools\\parity\\arb_scenarios.py --port-base 8070`, затем `llm_host run`.

1. `SPIKES.md` — журнал W0: замеры (§1, §14), спайки (§3–§10), риски/находки (§11, §14.7),
   go/no-go (§12), остаток (§13).
2. `W2_REPORT.md` — журнал W2: **§9 «Передача в новый чат»** (состояние, коммиты,
   карта кода, команды, открытые вопросы, грабли) и **§10 «A6 — отчёт»**: что сделано по
   файлам, **§10.2a** (боевые порты 8010–8012, полный офлоад **9384 МиБ**, `n_batch` −1503 МиБ),
   **§10.2b** (`ask` целиком; почему реранк на CPU), **§10.2c** (ARB-1…6, 6/6),
   **§10.6 — живая машина** (кто владеет портами, как останавливать/поднимать),
   **§10.7 — грабли окружения нового чата**. Затем A1 (устройство/VRAM/скорость + находки
   про cwd движка и вендорские DLL), B1 (паритет обхода 96 318 файлов), B2 (`content_hash`),
   B3 (чанкер), A2 (реестр инстансов), A3 (кросс-процессная адресация — её нет),
   **A4 §7/§7.1** (бюджет VRAM и диспетчер), **§7.2** (замер KV: модель гибридная,
   KV = 1024 МиБ), **§7.3** (фасад A5: контракт чата, «один инстанс — два режима»).
3. `../PLAN_W2_LLM_HOST.md` — план W2: треки A/B, критерии приёмки, график, DoD, приложение
   с точными структурами движка (§11).
4. `../MIGRATION_PLAN_RUST.MD` — §10.0 (статус платформ), §8.6 (диспетчер VRAM), §12–§13
   (приёмка и память), риск-регистр (R26–R34).

