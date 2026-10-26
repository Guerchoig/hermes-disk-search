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
  (`cudart64_*`, `cublas*64_*`, `ggml-cuda.dll`);
* для macOS берётся сборка `*-metal` (Metal — системный фреймворк Apple; файлов CUDA в ней
  нет, отдельного стороннего уведомления Metal не требует). Рантайм кладёт
  `install_macos.command` (на Windows — `installers/fetch_engine_runtime.ps1`; вариант на
  PowerShell требует `pwsh`).

### Патч движка (наш, временный — до апстрима)
Мы поставляем **собственный патч** сборки v1.15 (каталог `engine-patch/`, файлы и sha256 —
`runtime-manifests/engine-patch.json`): ограниченное ожидание слота, загрузка модели **вне**
мьютекса инстанса, порядок блокировок и поля типа KV-кэша (+ Flash Attention для квантованного
V). База — тот же MIT-лицензированный движок; изменения только в
`bridge/llama_server_cluster.{h,cpp}` и `bridge/llama_server_bridge.{h,cpp}`
(`engine-patch/hds-engine-patch-v1.15.patch`). Установка/откат —
`installers/fetch_engine_runtime.ps1 -PatchEngine` / `-RollbackEnginePatch` (штатные DLL
сохраняются рядом как `*.orig`). Патч применяется на **Windows-пути**; macOS-установщик
(`install_macos.command`) ставит **стоковый** движок `*-metal` без патча. До апстрим-фикса
ставим свой вариант, после — переходим на стоковый (`engine-patch/README.md`).

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
  (Windows: `%LOCALAPPDATA%\jdk-21`; macOS: `/opt/homebrew/opt/openjdk` или
  `/Library/Java/JavaVirtualMachines`) — OpenJDK/Oracle JDK (GPLv2 + Classpath Exception).

## Python-sidecar транскрибации (GigaAM + pyannote, P1–P5)
Зависимости — `sidecar/hds_transcribe/requirements.lock` / `requirements-dml.lock`
(точные версии в venv профиля). Лицензии по данным репозиториев на 23.10.2026 —
перед выпуском сверьте тексты из пакетов:
* **GigaAM** (`salute-developers/GigaAM`, веса `ai-sage/GigaAM-v3` в CDN Sber) — MIT;
* **pyannote.audio 4.0.0** — MIT; model `pyannote/speaker-diarization-community-1` —
  **CC BY 4.0** (gated: требуется принятие условий на Hugging Face);
  `pyannote/segmentation-3.0` — MIT (gated, принимается вместе с community-1);
* **wav2vec2-large-xlsr-53-russian** (`jonatasgrosman`) — Apache-2.0;
* **Silero VAD** (`silero-vad`) — MIT;
* **PyTorch 2.8.0** (+cu128/cpu) — BSD-3-Clause; **torchaudio** — BSD-2-Clause;
* **transformers 4.57.1**, **huggingface_hub** — Apache-2.0;
* **onnx / onnxruntime 1.23.2** — MIT/Apache-2.0; **onnxruntime-directml 1.24.4**
  (Python-DML профиль) — MIT + Microsoft DirectML (см. ниже);
* **numpy** — BSD-3, **soundfile** (libsndfile) — BSD-3/LGPL, **sentencepiece** —
  Apache-2.0, **hydra-core/omegaconf** — MIT/BSD, **psutil** — BSD-3, **PyYAML** — MIT.
Модели в архив/репозиторий **не входят**: их ставит `installers/fetch_transcribe_models.ps1`
(пины ревизий и sha256 — в `<models>\transcribe-models.json`), ONNX-экспорт —
`installers/export_gigaam_onnx.ps1`. Для gated-моделей нужен токен владельца
(`-HfToken`) только на первичную загрузку; рантайм работает офлайн (`HF_HUB_OFFLINE=1`).
DirectML в **python-профиле** — это `onnxruntime-directml` внутри venv; `Rust-DirectML.dll`
(legacy Nemotron) в default-пакет **не входит** (P6.2, см. `-LegacyDiarizer`).

## Python-sidecar анонимизации (GLiNER PII + Natasha, P1–P6)

Зависимости — `sidecar/hds_anonymize/requirements.lock` (точные версии в
`sidecar\venv-anonymize`). Лицензии по данным пакетов на 26.10.2026 — перед выпуском сверьте
тексты из пакетов:

* **Natasha**, **navec**, **slovnet** — MIT (модели navec/slovnet — ассеты релизов
  `natasha/navec` и `natasha/slovnet`); **pymorphy2** — MIT (нужен `setuptools<81`);
* **python-docx**, **openpyxl** — MIT; **lxml** — BSD; **numpy** — BSD-3;
* **onnxruntime** / **onnxruntime-gpu** — MIT; **PyTorch** — BSD-3-Clause;
  **transformers**, **huggingface_hub** — Apache-2.0; **setuptools** — MIT;
* **GLiNER** (`gliner`) и модель `knowledgator/gliner-pii-large-v1.0` — лицензии указаны на PyPI
  и в карточке модели на Hugging Face: зафиксируйте их при распространении. В архив модели
  **не входят** — их скачивает `installers/fetch_anonymize_models.ps1` (пины ревизий и sha256 —
  в `<models>\anonymize-models.json`), токен HF не требуется.

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

## Диаризация: Nemotron-3 (Sortformer v3), `hds-diarize`
* **Модель Nemotron-3-Diarization** (`nemotron3_diar_v3.onnx`; Sortformer v3 NVIDIA, до 8
  спикеров) — сторонний ONNX-экспорт `altunenes/parakeet-rs`, файл
  `nemotron-3-diarization/nemotron3_diar_v3.onnx`, ревизия `4d2a8bc7…`. Веса — под
  **OpenMDW 1.1** (`nemotron-3-diarization/LICENSE` того же репозитория): при распространении
  сохраняются текст соглашения и уведомления о происхождении; к **выходам** модели (тексту
  транскриптов) соглашение ограничений не добавляет. Модель **не входит** в репозиторий и в
  релизный архив — её ставит `installers/fetch_nemotron3_model.ps1` (400 506 656 байт,
  sha256 `915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a`).
* **parakeet-rs** 0.4.2 (Rust-обёртка Sortformer) — `MIT OR Apache-2.0`.
* **ONNX Runtime** 2.0.0-rc.15 (крейт `ort`, линкуется статически) — `MIT`.
* **DirectML** — `DirectML.dll` из пакета `Microsoft.AI.DirectML` (его кладёт рядом `ort`);
  лицензия Microsoft «MICROSOFT DIRECTML» — DLL распространяется в составе приложения для
  Windows/Xbox, поэтому она входит **только** в Windows-пакет (`bin\DirectML.dll`). На macOS
  работают CoreML/CPU (best-effort, W8), на прочих платформах — CPU.
* **sortformer 4spk-v2.1** (`openresearchtools/diar_streaming_sortformer_4spk-v2.1-gguf`) —
  прежний backend (`diarization.backend: sortformer`), остаётся рабочим fallback; лицензия —
  NVIDIA Open Model License.

## macOS: карантин и подпись (эксплуатация, не лицензия)
Сборка для macOS **не подписана**. `install_macos.command` при установке снимает карантин
Gatekeeper (`xattr -dr com.apple.quarantine`) с архива и рантайма движка и ставит **ad-hoc**
подпись на `.app`-обёртку (`codesign --force --deep -s -`). Нотаризации/Developer ID нет;
скачанный артефакт открывайте через «ПКМ → Открыть» либо снимите карантин вручную `xattr -dr`.

## Прочее
Лицензии Rust-крейтов и прочих зависимостей — в их пакетах. При выпуске релиза приложите
полные тексты перечисленных лицензий рядом с артефактом.
