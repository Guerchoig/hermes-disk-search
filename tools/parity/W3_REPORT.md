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

**Точные C-структуры** (скачаны из открытого SDK `github.com/openresearchtools/engine`,
MIT: `bridge/llama_server_cluster.h`, `bridge/llama_server_bridge.h`):

```c
// cluster (multi-node-server.dll) — путь с уже существующим владельцем инстансов
struct llama_server_cluster_audio_raw_request {
    int64_t instance_id;
    const uint8_t * audio_bytes; size_t audio_bytes_len;
    const char * audio_format;        // wav/mp3/...
    const char * metadata_json;       // mode/custom/model/whisper_* и пр.
    int32_t ffmpeg_convert;           // 0/1: конверт в WAV 16-bit mono 16 kHz в RAM
    int32_t enable_diarization; const char * diarization_model_path;
};
struct llama_server_cluster_json_result {
    int32_t ok; int32_t status; char * json; char * error;
    struct llama_server_cluster_inference_metrics metrics;   // doubles (по значению!)
};
int32_t llama_server_cluster_audio_transcriptions_raw(cluster, const req*, json_result* out);
void    llama_server_cluster_json_result_free(json_result* out);
struct llama_server_cluster_audio_raw_request llama_server_cluster_default_audio_raw_request(void);
struct llama_server_cluster_json_result        llama_server_cluster_empty_json_result(void);
// model_kind для роли whisper: LLAMA_SERVER_CLUSTER_INSTANCE_MODEL_KIND_WHISPER = 4

// bridge (llama-server-bridge.dll) — прямой путь без кластера (bridge_create → ...)
struct llama_server_bridge_audio_raw_request {
    const uint8_t * audio_bytes; size_t audio_bytes_len;
    const char * audio_format; const char * metadata_json; int32_t ffmpeg_convert;
};
struct llama_server_bridge_json_result { int32_t ok; int32_t status; char * json; char * error_json; };
int32_t llama_server_bridge_audio_transcriptions_raw(bridge, const req*, out*);
void    llama_server_bridge_json_result_free(out*);
```

Вывод ответа — JSON (тот же формат `/v1/audio/transcriptions`), парсим в сегменты
`{text,t_start,t_end}`. Есть и потоковое API (`..._audio_session_*`) — для индексации не нужно.

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

* ✅ **Раскладка структур аудио-API — решена** (структуры выше, из открытого SDK MIT).
  Новый вопрос дизайна: **где выполнять транскрибацию**. Кластер/bridge API — **in-process**
  (кросс-процессной адресации нет, замер A3), а индексирует `hds index` в своём процессе.
  Варианты: (а) транскрибирует **владелец GPU** (`llm-host`, роль `whisper`, model_kind=4) и
  отдаёт текст `hds index` через фасад (`/internal/transcribe`); (б) `hds index` сам создаёт
  cluster/bridge в своём процессе — но тогда два владельца GPU (нарушает §8.6.2). Скорее (а).
* **Препроцессинг картинок** для vision-ONNX (resize/center-crop/normalize) — строго как
  в `sentence-transformers` (`CLIPImageProcessor`), иначе паритет cos поедет.
* **ASCII-стейджинг** — обязателен (спайк 5: не-ASCII путь → испорченное имя результата).
* Устройство инференса ASR (`whisper_gpu_device`/`whisper_no_gpu`) — явно, иначе рантайм
  может выбрать недоступный бэкенд (R32).
