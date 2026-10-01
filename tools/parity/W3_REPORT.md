# W3_REPORT.md — журнал волны W3 (медиа: whisper/ASR и CLIP)

> Ветка `w2-llm-host` (W3 продолжаем в ней), начато **01.10.2026**. Рабочая станция:
> Windows, RTX 3060 12 ГБ, Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1.
> План W3 — `MIGRATION_PLAN_RUST.md` §4.2 (W3) и §7 (CLIP), §8.3 (вызов движка).
> Правило: Python-версия — источник истины; паритет проверяется скриптами/тестами.

## 1. Разведка W3 (факты, 01.10.2026)

### 1.1. crates.io и ONNX Runtime (`ort`) — доступны

Заказчик заметил, что `index.crates.io` открывается в браузере; перепроверили вживую
(настройки не менялись) — см. **§17 `W2_REPORT.md`**. Итог: crates.io доступен,
`ort 2.0.0-rc.13` собирается. **Рантайм-проверка `ort`:**

* собрано во временном крейте без `--offline` (default-features `download-binaries`,
  ort сам скачал ONNX Runtime через `ureq`/`ort-sys`; сборка 18,4 с);
* `Session::builder()?.commit_from_file(...)` на **реальной модели**
  `tools/parity/out/clip_onnx/vision/clip_vision.onnx` (351 МБ) → **`ORT OK: session created`**
  (1 с). То есть CLIP-в-Rust через `ort` (план §7.2) **подтверждён живым запуском**.

Кэш ONNX-моделей (из спайка 4, `spike4_clip_onnx.py`): `vision/clip_vision.onnx` (351 МБ),
`text/clip_text_xlmr.onnx` (539 МБ).

### 1.2. Аудио-API движка — найдены экспорты

`tools/parity/pe_exports.py` по DLL движка (`%APPDATA%\OpenResearchTools\TranscribeOffline\Engine`):

* **`multi-node-server.dll`** (эту DLL уже грузит `hds-llama`): `llama_server_cluster_audio_transcriptions_raw`,
  `llama_server_cluster_audio_transcriptions_native`,
  `llama_server_cluster_default_audio_raw_request`,
  `llama_server_cluster_default_native_audio_transcription_request`;
* **`llama-server-bridge.dll`**: `llama_server_bridge_audio_transcriptions_raw`,
  `llama_server_bridge_default_audio_raw_request` (+ потоковое API `..._audio_session_*`:
  `create/destroy/push_audio/push_encoded/wait_events/drain_events/start_transcription/...`);
* **`llama-server-audio.dll`**: live-захват (`llama_server_audio_live_*`), `list_capture_devices`.

Batch-транскрибация (нужна индексатору) = `default_audio_raw_request` →
`audio_transcriptions_raw` — совпадает с `run_audio_raw` из §8.3. Путь «в духе» уже сделанного
моста к движку: `hds-llama::engine` (libloading + `SetDllDirectory`).

## 2. План W3 (по файлам, черновик — уточняется)

1. **`crates/hds-whisper`** — FFI к аудио-API (batch): `default_audio_raw_request` +
   `audio_transcriptions_raw`; `AudioRunParams` (`mode: speech`, `whisper_model`,
   `whisper_gpu_device`/`whisper_no_gpu`, `ffmpeg_convert`), **ASCII-стейджинг** путей,
   маппинг ответа в сегменты `{text,t_start,t_end}`.
2. **Интеграция в конвейер**: медиа-ветка в `hds-index::pipeline`; `hds index`/`watch`
   транскрибируют аудио/видео; `whisper-check`.
3. **Диспетчер GPU**: роль `whisper` — `create` при первом медиафайле, `destroy` при
   эвикции/простое (приоритет 20 уже в конфиге `gpu.priorities`).
4. **`crates/hds-clip`** — оба энкодера на `ort` (vision при индексации → `images_vec`
   dim=512; text — резидентный для поиска/MCP); препроцессинг картинок — как в
   sentence-transformers (паритет спайка 4: cos ≥ 0,999); `clip-index` дозаполняет;
   деградация без моделей.
5. **Тесты/приёмка**: 3 реальных медиа (вкл. русское имя в русском каталоге),
   ASCII-стейджинг на боевом пути, CLIP топ-10 на 20 запросах, VRAM в бюджете (+CPU-fallback).

## 3. Неизвестные/риски (снять до кода)

* **Точная раскладка структур** аудио-запроса/ответа C-API (header `bridge`/`cluster` не
  документирован в репозитории). Нужен источник: заголовок SDK движка или адаптация
  `bridge.rs` из `transcribeoffline` (MIT). *Это первый шаг реализации `hds-whisper`.*
* **Препроцессинг картинок** для vision-ONNX (resize/center-crop/normalize) — строго как
  в `sentence-transformers` (`CLIPImageProcessor`), иначе паритет cos поедет.
* **ASCII-стейджинг** — обязателен (спайк 5: не-ASCII путь → испорченное имя результата).
* Устройство инференса ASR (`whisper_gpu_device`/`whisper_no_gpu`) — явно, иначе рантайм
  может выбрать недоступный бэкенд (R32).
