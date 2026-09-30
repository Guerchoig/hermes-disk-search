# tools/parity — паритет-harness W0 и карта артефактов

> **Коротко.** Здесь живёт всё, что нужно, чтобы (а) проверять паритет новой реализации с текущей
> Python-версией и (б) воспроизводить замеры W0. Журнал результатов с цифрами —
> `SPIKES.md` (читать первым), журнал W2 — `W2_REPORT.md`, детальный план W2 —
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
| `crates/hds-index` (`tests/hash_parity`, `tests/walk_parity`, `tests/chunker_parity`) | B1/B2/B3: фиксированные векторы хэша, 50 реальных файлов, обход/исключения/лимиты против Python-дампа, чанкер против golden (16 фикстур / 6 363 чанка) |
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
13. **`crates.io` на машине заказчика недоступен** (`index.crates.io` не резолвится) — новые
    зависимости не добавить; поэтому HTTP-фасада сервер свой (`crates/hds-llama/src/http.rs`).
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

## 4. Что читать первым в новом чате

**Чек-лист на 5 минут (копипаст):**
```powershell
git -C <репозиторий> log --oneline -3        # ветка w2-llm-host (12 коммитов на 30.09.2026)
cargo test --workspace                       # должно быть 74 green (+2 #[ignore])
cargo run -p hds-llama --release --bin llm_host_status                       # состояние ролей и VRAM
cargo run -p hds-llama --release --bin llm_host -- status --local --no-engine  # то же, но A6-путём
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\facade_smoke.ps1 -PortBase 8020 -HoldSec 20
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\resident_smoke.ps1 -PortBase 8030 -HoldSec 600
```
Если третья/четвёртая команда не запускаются — смотрите «Грабли» (ниже) и `W2_REPORT.md` §9.7.

1. `SPIKES.md` — журнал W0: замеры (§1, §14), спайки (§3–§10), риски/находки (§11, §14.7),
   go/no-go (§12), остаток (§13).
2. `W2_REPORT.md` — журнал W2: **§9 «Передача в новый чат»** (состояние, коммиты,
   карта кода, команды, открытые вопросы, грабли; **§9.8 — план A6 по файлам,
   §9.9 — что не проверено**) и **§10 «A6 — отчёт»** (что сделано по файлам, живой
   прогон резидентно процесса, находки: `--ngl 0` и `llama_params_fit`, «роль на CPU
   = 0 МиБ», остаток A6). Затем A1 (устройство/VRAM/скорость + находки про cwd движка и
   вендорские DLL), B1 (паритет обхода 96 318 файлов), B2 (`content_hash`), B3 (чанкер),
   A2 (реестр инстансов), A3 (кросс-процессная адресация — её нет), **A4 §7/§7.1**
   (бюджет VRAM и диспетчер), **§7.2** (замер KV: модель гибридная, KV = 1024 МиБ),
   **§7.3** (фасад A5: контракт чата, «один инстанс — два режима», живой прогон).
3. `../PLAN_W2_LLM_HOST.md` — план W2: треки A/B, критерии приёмки, график, DoD, приложение
   с точными структурами движка (§11).
4. `../MIGRATION_PLAN_RUST.MD` — §10.0 (статус платформ), §8.6 (диспетчер VRAM), §12–§13
   (приёмка и память), риск-регистр (R26–R34).

