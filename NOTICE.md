# NOTICE — атрибуция и лицензии

`hermes-disk-search` (Rust-ядро) поставляется с компонентами третьих сторон. Этот файл
перечисляет их и подсказывает, где лежат полные тексты лицензий; он **не заменяет**
исходные лицензии — при распространении сохраняйте их вместе с артефактом.

## Собственный код
Проект `hermes-disk-search` (приватный репозиторий `Guerchoig/hermes-disk-search`) — см.
`LICENSE` репозитория.

## Рантайм движка (LLM-хост + ASR) — скачивается установщиком
`installers/fetch_engine_runtime.ps1` по `runtime-manifests/engine-manifest.json` кладёт
архив **Openresearchtools Engine** (`github.com/openresearchtools/engine`, сборки
`Openresearchtools-Engine-v1.15-*`). Внутри архива — свои уведомления, сохраняйте их:
* `LICENSE-ENGINE.txt`, `LICENSES.md`, `Third-Party-Notices.md` — лицензии движка и
  входящих llama.cpp / whisper.cpp / ggml;
* `NVIDIA-CUDA-EULA.txt`, `NVIDIA-CUDA-RUNTIME-NOTICE.txt` — для сборок `*-cuda`
  (`cudart64_*`, `cublas*64_*`, `ggml-cuda.dll`).

### Патч движка (наш, временный — до апстрима)
Мы поставляем **собственный патч** сборки v1.15 (каталог `engine-patch/`, файлы и sha256 —
`runtime-manifests/engine-patch.json`): ограниченное ожидание слота, загрузка модели **вне**
мьютекса инстанса, порядок блокировок и поля типа KV-кэша (+ Flash Attention для квантованного
V). База — тот же MIT-лицензированный движок; изменения только в
`bridge/llama_server_cluster.{h,cpp}` и `bridge/llama_server_bridge.{h,cpp}`
(`engine-patch/hds-engine-patch-v1.15.patch`). Установка/откат —
`installers/fetch_engine_runtime.ps1 -PatchEngine` / `-RollbackEnginePatch` (штатные DLL
сохраняются рядом как `*.orig`). До апстрим-фикса ставим свой вариант, после — переходим на
стоковый (`engine-patch/README.md`).

## Медиа и OCR
* **FFmpeg** (`avcodec-*`/`avformat-*`/`avutil-*`/`swresample-*` в `vendor/ffmpeg` рантайма,
  а также системный `ffmpeg` для транскрипции) — LGPL/GPL: лицензия зависит от конкретной
  сборки (у Gyan.FFmpeg — GPL). См. лицензию установленной сборки.
* **Tesseract OCR** — Apache-2.0 (языковые данные `rus`/`eng` — `tesseract-ocr/tessdata`).

## Python-sidecar (воркер извлечения/лемматизации)
Зависимости зафиксированы в `sidecar/hds_extract/requirements.lock`; полные тексты — в
соответствующих пакетах после установки. Обратите внимание:
* **PyMuPDF** — AGPL-3.0 (или коммерческая лицензия Artifex) — важное условие;
* python-docx / openpyxl / python-pptx / DAWG2-Python / et_xmlfile / xlsxwriter — MIT;
* Pillow — HPND; pymorphy3 / pymorphy3-dicts-ru — MIT; PyYAML — MIT; lxml — BSD;
* pytesseract — GPL-3.0 (обёртка над Tesseract);
* **mpxj** (MS Project, Java) и **JPype1** — Apache-2.0; для `.mpp` нужна **Java 11+**
  (у нас JDK 21 в `%LOCALAPPDATA%\jdk-21`) — OpenJDK/Oracle JDK (GPLv2 + Classpath Exception).

## Базы данных
* **SQLite** (bundled в `rusqlite`) — public domain;
* **sqlite-vec** — MIT / Apache-2.0.

## Модели (в git не входят, скачиваются отдельно)
* GGUF chat/embedding/rerank (Qwen3.5-9B, bge-m3, bge-reranker-v2-m3) — лицензии исходных
  моделей на Hugging Face;
* **whisper-large-v3-turbo-GGML** (`openresearchtools/whisper-large-v3-turbo-GGML`);
* **CLIP ONNX** (`runtime-manifests/clip-manifest.json`) — производные от
  `openai/clip-vit-base-patch32` и
  `sentence-transformers/clip-ViT-B-32-multilingual-v1`
  (экспорт — `tools/parity/clip_onnx_w3.py`).

## Прочее
Лицензии Rust-крейтов и прочих зависимостей — в их пакетах. При выпуске релиза приложите
полные тексты перечисленных лицензий рядом с артефактом.
