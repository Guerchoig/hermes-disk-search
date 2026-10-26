# sidecar — Python-воркер извлечения и нормализации (B6)

Автономный процесс для того, что по плану остаётся в Python
(`MIGRATION_PLAN_RUST.md` §2.5, §5): извлечение сегментов (PDF/DOCX/XLSX/PPTX/OCR)
и лемматизация FTS (`pymorphy3`). Rust-ядро говорит с ним по **stdio + JSON-RPC 2.0**
(NDJSON), запускает лениво и владеет процессом.

## Состав

| Файл | Что |
|---|---|
| `hds_sidecar/` | **общий пакет** (не процесс): `stdio` (fd1→stderr, UTF-8 stdin, NDJSON, reply/ok/err), `runtime` (reader-thread, idle-timeout, EOF, изоляция ошибки одного запроса), `config` (state_dir/config_path/load_config), `progress` (`[stage] …` в stderr), `fs` (temp/безопасные пути/long-path) |
| `hds_extract/worker.py` | воркер: `hello`/`extract`/`normalize`/`clip_image`/`shutdown`, idle-timeout, изоляция stdout; транспорт — `hds_sidecar` |
| `hds_extract/requirements.lock` | зависимости воркера (вариант A, сняты со спайка 3) |
| `hds_transcribe/` | воркер транскрибации/диаризации (GigaAM-v3 + pyannote, профили cuda/cpu/dml), см. `hds_transcribe/README.md` |
| `hds_anonymize/` | воркер анонимизации/деанонимизации (GLiNER PII / Natasha+Slovnet, DOCX/XLSX/TXT/MD/XML, база меток SQLite), см. раздел ниже |

`hds_sidecar` намеренно **не импортирует** torch/fitz/pymorphy3 на уровне
модуля: иначе один воркер тянул бы зависимости другого (правило границы §5.1).

## Запуск (разработка)

```powershell
# воркер использует зависимости окружения проекта (.venv)
'{"jsonrpc":"2.0","id":1,"method":"hello"}' |
  .\.venv\Scripts\python.exe sidecar\hds_extract\worker.py --root .
```

Rust-клиент (`crates/hds-extract`) ищет интерпретатор так:
`HDS_EXTRACT_PYTHON` → `sidecar/python/**/python.exe` (вариант A, установщик) →
`.venv\Scripts\python.exe` (вариант C, fallback) → ошибка с подсказкой.

## Поставка (вариант A, самодостаточный)

`installers/build_sidecar.ps1` собирает дерево, работающее **без** Python-ядра проекта:
`hds_extract/` (воркер + lock), `python/` (портативный CPython из python-build-standalone +
зависимости из `requirements.lock`), `hds/` (**копия** модулей извлечения/лемматизации из
корневого `hds/*.py` — источник истины не переезжает), `README.md`. `worker.py` ставит
каталог `sidecar/` в `sys.path` перед корнем проекта: в поставке `hds` резолвится из копии,
в разработке (`sidecar/hds` отсутствует) — из корня проекта.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_sidecar.ps1 -OutDir dist\sidecar -SelfTest
```

В релизе это делает джоба CI `build-sidecar`; `build-windows` кладёт готовое дерево в пакет.

## Транскрибация (P5): профили, venv и модели

Второй воркер (`hds_transcribe/`) живёт в **своём** venv: torch/pyannote нельзя
тащить в извлечение документов, а профили несовместимы между собой
(`onnxruntime` и `onnxruntime-directml` дают один модуль `onnxruntime`).

| Профиль | venv | Что внутри |
|---|---|---|
| `cuda` | `sidecar\venv-transcribe` | torch 2.8.0 **cu128**, pyannote 4.0, GigaAM (git) |
| `cpu` | `sidecar\venv-transcribe` | torch 2.8.0 **cpu**, тот же стек; ONNX CPU — быстрее torch (P0) |
| `dml` | `sidecar\venv-transcribe-dml` | torch cpu + `onnxruntime-directml==1.24.4`; gigaam `--no-deps` |

Установка (идемпотентна, ровно один профиль на дерево):

```powershell
# 1) портативный CPython + извлечение (если ещё нет)
powershell -File installers\build_sidecar.ps1 -OutDir sidecar -SelfTest
# 2) venv транскрибации
powershell -File installers\build_transcribe_sidecar.ps1 -Profile cuda -SelfTest
# 3) ONNX для dml/cpu (в <models>\gigaam-onnx\<variant>\)
powershell -File installers\export_gigaam_onnx.ps1 -ModelsDir D:\models -Verify
# 4) модели: GigaAM-v3, wav2vec2-ru, community-1, segmentation-3.0
powershell -File installers\fetch_transcribe_models.ps1 -ModelsDir D:\models -HfToken <token>
```

Rust ищет интерпретатор так: `HDS_TRANSCRIBE_PYTHON` → `sidecar\venv-transcribe[-dml]`
→ `sidecar\python` → `.venv`. Профиль `auto` определяется по наличию venv
(`crates/hds-transcribe/src/settings.rs`), установщик выбирает его по GPU.

Модели и HF-кэш: `transcribe.models_dir` (`HDS_MODELS_DIR`) →
`<models>\gigaam`, `<models>\hf-home`, `<models>\gigaam-onnx\<variant>`;
в рантайме воркер работает с `HF_HUB_OFFLINE=1` (сервер — LocalSystem).
Манифест загрузки с пинами ревизий и sha256 — `<models>\transcribe-models.json`.

## Анонимизация (P5–P6): venv, модели и контракт

Третий воркер (`hds_anonymize/`) тоже живёт в **своём** venv: в lock есть `torch`
(его импортирует GLiNER даже для ONNX-пути) и `onnxruntime`, которых нет у извлечения.

| Профиль | venv | Что внутри |
|---|---|---|
| `cpu` | `sidecar\venv-anonymize` | `onnxruntime==1.29.0` (пин из lock) |
| `cuda` | `sidecar\venv-anonymize` | `onnxruntime-gpu==1.24.4`; CPU-пин из lock фильтруется |

```powershell
powershell -File installers\build_anonymize_sidecar.ps1 -Profile cpu -SelfTest
powershell -File installers\fetch_anonymize_models.ps1 -ModelsDir D:\models   # без HF-токена
```

Rust ищет интерпретатор так: `HDS_ANONYMIZE_PYTHON` → `sidecar\venv-anonymize` →
`sidecar\python` → `.venv`. Модели — `<models>\anonymize`: `hf-home` (HF-кэш GLiNER PII),
`navec_news_v1_1B_250K_300d_100q.tar`, `slovnet_ner_news_v1.tar`; манифест с ревизиями и
sha256 — `<models>\anonymize-models.json`.

Методы: `hello` (в т.ч. `formats`), `warmup`, `inspect` (метки/маркер — **без значений**),
`anonymize`, `anonymize_values`, `deanonymize`, `deanonymize_values`, `shutdown`; прогресс —
notifications без `id`. База меток (`mapping_store.py`, SQLite WAL) — единственный модуль доступа:
нумерация per-type, дедуп по (значение, тип). Rust к базе не обращается и передаёт все параметры
аргументами (`--model/--db/--models-dir/--device/--gpu-mem-limit-mb/…`), `config.yaml` воркер не
читает. Проверки воркера — `sidecar/tests/test_anonymize_*.py` (модели не нужны).

## MS Project (.mpp)

`.mpp`/`.mpt`/`.mpx` извлекаются через **mpxj** (Java, JPype). Нужна **Java 11+**: воркер сам
находит `%LOCALAPPDATA%\jdk-21\<ver>\bin\server\jvm.dll` (macOS — Homebrew openjdk или
`/Library/Java/JavaVirtualMachines/*`), `JAVA_HOME` не обязателен. Без Java файл
**индексируется с пометкой** (не падает); `hello.capabilities` тогда не содержит `mpp`, и
`hds check` показывает `mpp: warn`.

## Контракт (§5 плана)

* транспорт — stdio, NDJSON, UTF-8; аргумент `--root <проект>` (воркер не ищет `config.yaml` сам);
* `hello` → `{protocol: 1, python, pid, capabilities}`; родитель сверяет `protocol` и возможности;
* `extract {path, opts}` → `{kind, segments[], warnings[], elapsed_ms}` либо
  структурированная ошибка `{code, message, hint}`; сегменты — как `hds/extractors.seg()`
  (`text/page/t_start/t_end`, опционально `head`);
* `normalize {texts[]}` → `{lemmas[]}`;
* `clip_image` → воркер отвечает «не поддерживаю» (CLIP переезжает в Rust на ONNX, §7);
* `shutdown` либо закрытие stdin → штатный выход;
* простой дольше `extract.idle_timeout` (по умолчанию 60 с) → воркер завершается сам.

## Грабли (W0/спайк 3)

* библиотеки и `ffmpeg` пишут в **fd 1** напрямую — воркер дублирует `fd 1`, уводит
  `fd 1` в stderr, протокол пишет в дубликат;
* Python на Windows читает stdin в ANSI — принудительно UTF-8 (кириллица в путях);
* `select()` по pipe на Windows не работает — читатель в отдельном потоке;
* `uv venv` python.exe — процесс-трамплин: память мерить по дереву процессов
  (`tools/parity/proc_tree.ps1`).