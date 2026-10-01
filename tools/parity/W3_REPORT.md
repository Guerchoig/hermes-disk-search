# W3_REPORT.md — журнал волны W3 (медиа: whisper/ASR и CLIP)

> Ветка `w2-llm-host` (W3 продолжаем в ней), начато **01.10.2026**. Рабочая станция:
> Windows, RTX 3060 12 ГБ, Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1.
> План W3 — `MIGRATION_PLAN_RUST.md` §4.2 (W3) и §7 (CLIP), §8.3 (вызов движка).
> Правило: Python-версия — источник истины; паритет проверяется скриптами/тестами.

## 0. Передача в новый чат (01.10.2026)

**Где мы.** Ветка `w2-llm-host`, HEAD **`39762b7`**, 37 коммитов впереди `main` (`main` не тронут).
`cargo test --workspace` — **125 passed / 0 failed (+6 ignored)**. Живая машина: владелец портов
8010–8012 — `llm-host` (Rust); Python-watcher запущен (`watch.lock=8028`), `index.pause`
заказчика стоит — **не снимать**.

**W2 закрыт:** A1–A6, B1–B7, B-2 (пилот 10 000 файлов, полный паритет ±7 %), B-4 (память) —
`W2_REPORT.md` §9–§17. Подробности W2 читать в `W2_REPORT.md` §9 (передача) и §10–§17.

**W3 (медиа) — сделано (коммиты `3b8ffd9`…`39762b7`):**
* crates.io **доступен** (перепроверено); крейт **`ort`** собирается и создаёт сессию
  (`W2_REPORT` §17, `W3_REPORT` §1.1);
* найдены **экспорты аудио-API** движка и **точные C-структуры** из открытого SDK;
* **ASR из Rust РАБОТАЕТ**: `llama-server-bridge` создаётся **audio-only** (без `model_path`),
  whisper-модель — в `metadata_json.whisper_model` (**GGML `.bin`**, GGUF не нужен);
  `mode: subtitle` → `.srt` → сегменты `{text,t_start,t_end}` (`crates/hds-llama/src/whisper.rs`);
* постоянный `Bridge` (`bridge_audio.rs`) + **`/internal/transcribe`** в фасаде +
  ленивый `Whisper` в `ClusterBackend` (`host.rs`), маршрут покрыт тестом.

**Что дальше (шаг 2 → 3):**
1. **медиа-ветка `hds-index`**: для аудио/видео вместо Python-воркера звать `/internal/transcribe`
   (проксировать сегменты в конвейер) + ключи `index.whisper_*` (`model`,`mode`,`custom`,`gpu`);
2. `whisper-check` (CLI);
3. **live-приёмка**: 3 реальных медиа (вкл. русское имя в русском каталоге), ASCII-стейджинг
   на боевом пути, замеры;
4. **CLIP** через `ort` (vision→`images_vec` dim 512, text резидентный; препроцессинг как
   `CLIPImageProcessor`), `clip-index`.

**Не переоткрывать (факты W3):**
* bridge для audio создаётся **без модели** («For audio-only use, `model_path` may be omitted»,
  `docs/bridge-audio-dll.md`); whisper — через `metadata_json.whisper_model`;
* модель whisper — **GGML `.bin`** (маршрут whisper.cpp), **не** llama.cpp GGUF;
* `mode: subtitle` (+`custom`=сек) → `.srt` с таймкодами; `speech` → `.md` (сплошной текст);
* кластерный аудио-путь требует **execution group** (single-node manual-инстанс даёт
  `unknown execution_group_id: cluster:manual`) → для ASR используем **bridge**, не cluster;
* текст движок пишет в `output.path` (читаем файл), JSON — метаданные/статистика;
* ASCII-стейджинг обязателен (спайк 5);
* секреты эндпоинтов фасада: `/internal/*` только при `ServerConfig.internal=true`.

**Грабли окружения (нового чата):** `git` — всегда `--no-pager`; PowerShell иногда искажает
первый токен команды (повторить / короткая пара команд); `curl -o` — только в act-режиме;
кириллица в `Select-String` не ищется (ASCII-шаблон или чтение файла). `HDS_CONFIG` в сессии
PowerShell персистентна — если поднять watcher/python, убедиться, что она **не** указывает
на удалённый temp-файл.

**Артефакты/команды:** прогон ASR — `cargo run -p hds-llama --bin audio_probe -- [<audio>] [<whisper.bin>] [<gpu>]`;
справочник SDK (`docs/bridge-audio-dll.md`, `bridge/*.h`, `README.md`) — клон
`github.com/openresearchtools/engine` (в `%TEMP%\engine_sdk`).

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

### 1.3. Probe транскрибации — первый прогон (01.10.2026)

Реализованы: FFI `AudioRawRequestRaw` + символы `llama_server_cluster_default_audio_raw_request`
/ `llama_server_cluster_audio_transcriptions_raw` (`crates/hds-llama/src/engine.rs`,
`ffi.rs`), метод `Cluster::transcribe_audio_raw` (`cluster.rs`), бинарь
`crates/hds-llama/src/bin/audio_probe.rs`. Прогон на `test_data/jfk.wav`:

* движок загружен, устройства: **CUDA0** (`bridge_index=0`, accel) и CPU (`1`);
* роль `whisper` (`model_kind=4`, `LOAD_ON_DEMAND`, gpu=0) создана (instance id=1);
* вызов вернул `rc=-1 ok=false status=500`: **`unknown execution_group_id for native
  transcription: cluster:manual`**; движок также ругнулся `invalid magic 'lmgg',
  expected 'GGUF'`.

**Выводы и следующие шаги (до рабочей транскрибации):**

1. **`execution_group_id`.** Нативный путь транскрибации требует execution-group; инстанс
   создан через `manual_devices_csv` (группа не задана) → движок дефолтит на несуществующий
   `cluster:manual`. Варианты: (а) задавать `execution_group_id` (получить список через
   `llama_server_cluster_list_execution_groups[_with_rpc]`), (б) идти через bridge-API
   (`llama_server_bridge_audio_transcriptions_raw`, без кластера). Разобрать по
   `bridge/llama_server_cluster.cpp` (в открытом SDK).
2. ~~**Формат модели whisper**~~ — ✅ **решено**: GGML корректен, ошибка была в способе передачи.

**Bridge-путь — реализован, проверен, РАБОТАЕТ (01.10.2026).** Ответ найден в самом
репозитории движка (`docs/bridge-audio-dll.md`): «For audio-only use, `model_path` may be
omitted» — bridge создаётся **без модели**, а whisper-модель задаётся в `metadata_json`
ключом **`whisper_model`** (пример из доков: `"./models/whisper.bin"` ⇒ это **GGML `.bin`**
через маршрут whisper.cpp, а не llama.cpp GGUF). Реализовано в
`crates/hds-llama/src/bridge_audio.rs` (`BridgeAudio::load` → `create` →
`audio_transcriptions_raw`; `model: Option<&Path>` — для audio-only `None`).

Прогон `audio_probe` (jfk.wav 11,13 с, whisper-large-v3-turbo, GPU):
`bridge: ok=true status=200`, **транскрипт получен**:
`And so, my fellow Americans, ask not what your country can do for you, ask what you can do for your country.`
(`stats: num_words=22, num_segments=3`; timings whisper **1,94 с**, всего 2,0 с).
Ответ — JSON, а сам текст пишется в `output.path` (`.md`) — его и надо читать.
Диаризация выключена (`mode: speech`).
Итог: **ASR-путь движка из Rust работает; модель GGML (GGUF не нужен)**.

**Сегменты с таймкодами (шаг 1 — готов).** `mode: subtitle` (окно задаётся `custom`, сек)
пишет `.srt`; `crates/hds-llama/src/whisper.rs` парсит его в `{text,t_start,t_end}`.
Прогон `audio_probe` (jfk.wav) → **3 сегмента**:
```
[0.83  ->  5.02] And so, my fellow Americans, ask not
[5.71  -> 10.09] what your country can do for you, ask what you can
[10.09 -> 11.13] do for your country.
```
Реализовано: `Whisper::transcribe_file(path, mode, custom) -> Transcript{json, segments}`
(+ `parse_srt`, ASCII-стейджинг внутри); `WhisperSegment{text,t_start,t_end}`.

### 1.4. Интеграция в владельца (шаг 2, начато)

* `crates/hds-llama/src/bridge_audio.rs`: `BridgeAudio::create(model,gpu,ngl) -> Bridge`
  — **постоянный** handle (`Bridge::transcribe_raw`, `Drop`→destroy); модель whisper больше
  не пересоздаётся на каждый файл;
* `crates/hds-llama/src/whisper.rs`: `Whisper` держит постоянный `Bridge`;
* **фасад `/internal/transcribe`**: `Route::InternalTranscribe` + `Backend::internal_transcribe`;
  в `ClusterBackend` — ленивый `Whisper` (создаётся при первом запросе), тело
  `{path,mode,custom,gpu,model}`, ответ `{segments:[{text,t_start,t_end}],stats}`.
  Маршрут покрыт тестом (`tests/facade_core.rs`).

**Осталось по шагу 2:** медиа-ветка `hds-index` (зовёт фасад вместо Python-воркера),
`whisper-check`, ключи `index.whisper_*`, приёмка на 3 реальных медиа.

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
