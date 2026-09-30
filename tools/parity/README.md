# tools/parity — паритет-harness W0 и карта артефактов

> **Коротко.** Здесь живёт всё, что нужно, чтобы (а) проверять паритет новой реализации с текущей
> Python-версией и (б) воспроизводить замеры W0. Журнал результатов с цифрами —
> `SPIKES.md` (читать первым), детальный план W2 — `../PLAN_W2_LLM_HOST.md`,
> основной план — `../MIGRATION_PLAN_RUST.MD` (там §4 W0, §10.0 статус платформ, §12/§13 приёмка).
> Все скрипты запускаются из **корня репозитория** интерпретатором проекта:
> `.\.venv\Scripts\python.exe <скрипт>` (Windows) / `python3 <скрипт>` (macOS).

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

## 4. Что читать первым в новом чате

1. `SPIKES.md` — журнал W0: замеры (§1, §14), спайки (§3–§10), риски/находки (§11, §14.7),
   go/no-go (§12), остаток (§13).
2. `../PLAN_W2_LLM_HOST.md` — план W2: треки A/B, критерии приёмки, график, DoD, приложение
   с точными структурами движка (§11).
3. `../MIGRATION_PLAN_RUST.MD` — §10.0 (статус платформ), §8.6 (диспетчер VRAM), §12–§13
   (приёмка и память), риск-регистр (R26–R34).

