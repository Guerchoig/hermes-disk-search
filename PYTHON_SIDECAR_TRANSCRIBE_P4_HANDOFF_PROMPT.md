# PYTHON_SIDECAR_TRANSCRIBE_P4_HANDOFF_PROMPT.md — контекст и промт для P4

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать **§8 целиком**
> и отправить первым сообщением. §§0–7 — карта состояния (читать выборочно).
>
> **Дата среза:** 22.10.2026 (после P3, код-ревью и исправления дефектов P1–P3).
> **Ветка:** `python-sidecar-transcribe`, **HEAD `f1525d0`**, база `server-multiuser`
> @ `f202128` (tag `v0.21.0`). Локально **2 коммита впереди** origin, **не запушено**.
> Рабочая копия чистая.
>
> **Что закоммичено:** `c0d6915` — P1–P3 (python-sidecar, Rust-клиент, глоссарий),
> `f1525d0` — исправления дефектов код-ревью P1–P3 (DML-атрибуция, долгоживущий
> воркер, конфиг). План и хендоффы P0–P3 тоже в репозитории.
>
> **Ключевые файлы:** `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` (§4.6 — LLM, §9 P4, §6),
> `sidecar/hds_transcribe/README.md` (стек, протокол, «Эксплуатационные детали»),
> `tools/parity/out/py_transcribe/P3_REPORT.md` (**gitignored**: числа G0–G3,
> формат `edits.json`, команды).

---

## 0. Статус на 22.10.2026 (P1–P3 закрыты, ревью исправлено)

### 0.1 Коммиты

```
f1525d0  fix(transcribe): устранены дефекты код-ревью P1-P3 (DML-атрибуция,
         долгоживущий воркер, конфиг)                       ← HEAD
c0d6915  feat(transcribe): P1-P3 python-sidecar (GigaAM/pyannote, Rust-клиент,
         глоссарий)
f202128  fix(diarize): as_chunks вместо chunks_exact в parse_wav   ← v0.21.0
```

### 0.2 Что работает end-to-end (`transcribe.engine: python`)

`transcribe-watch` / `transcribe-once` / `transcribe-check` / `hds check` →
`hds-index::transcribe::TranscribeClient` → sidecar-воркер `hds_transcribe`:
ASR GigaAM-v3 `e2e_rnnt` fp16 CUDA → Silero VAD (окна ≤25 с, режем только в
паузах) → alignment (native/whisperx/hybrid) → флаги качества → pyannote
community-1 (блоки 20 мин, батчи 4/4) → merge (max-overlap) →
`<stem>.transcript.json` → `postprocess` (глоссарий) → `<stem>.edits.json`,
`needs_review` в транскрипте → Rust рендерит `.md`/`.orig.md`/`.speakers.json`.
Демон держит **один долгоживущий воркер** (модели грузятся по стадиям и
выгружаются, процесс остаётся; `idle_unload_sec`/`max_requests` — на транспорте).

### 0.3 Приёмка P1–P3 (факты, `P2_REPORT.md` / `P3_REPORT.md`)

| Проверка | Факт |
|---|---|
| `meet0924` 18 мин (`src_0924.mp3`, эталон `ref_0924.txt`) | WER **17,82 %**, CER **9,52 %**, DER **13,00 %**, атрибуция по словам **90,68 %** — одинаково во всех режимах alignment |
| `low_confidence + failed` (гейт ≤2 %) | native **0,46 %** (11 слов), hybrid 0,71 %, whisperx 0,63 % |
| MAE native↔whisperx | **64,3 мс start / 164,7 мс end**, 1,64 % слов > 350 мс |
| `hybrid` | 12,5 % слов из whisperx, `align_sec` 24,7 с на 18 мин, текст блоков идентичен native, блоков 286 → 237 |
| 77 мин hybrid (холодный кэш) | **313,6 с** (ASR 74,8 + align 77,5 + diar 150,9) = 1,46× P1, пик VRAM **5 823 МБ**, 10 спикеров |
| VAD A/B | Silero vs off: WER 17,82 vs 18,56; `max_chunk_sec` 25 vs 30 — в пределах шума ⇒ 25 |
| Каналы | `auto` → downmix на реальных файлах (r=0,9998); `split` проверен на синтетике |
| **G0** biasing (gigastt) | «RP-система» → **«ERP-система»** при `boost 8`, hotword-файл UTF-8 **без BOM** |
| **G1** INT8 без hotwords vs fp16 | WER 17,78 % (−0,04 п.п.) ✅, но recall сущностей **0,70 vs 0,90** ❌ (теряется `ERP`), 97,9 с на 18 мин (≈3,6× медленнее) |
| **G2** hotwords boost 5/8/12 | recall 0,90 (+28,6 % отн. к INT8 без hotwords), вставки 0 (0,042 % при 12), WER 17,78 % — но **= fp16, выигрыша нет** |
| **G3** пилот (окна) | 4 окна: принято 2, «ерпи» не исправлено ⇒ маршрут закрыт |
| **Решение** | `transcribe.hotwords.backend: none` (код: `HOTWORDS_GATES_PASSED = false`) |

### 0.4 Проверка **после** ревью (22.10.2026, `p4/smoke_*`)

Свежий прогон закоммиченного кода (`rpc_once.py`, `src_0924.mp3`, чистый state):

```
ok: 105,1 s, words=2392, blocks=286, speakers=[SPEAKER_00, SPEAKER_02, SPEAKER_01], rtf=0.0533
WER 17.82 % · CER 9.52 % · атрибуция 90.7 %
DER 13.00 % (miss 135.1 s, fa 0.0 s, conf 0.0 s)
low_confidence 11 (0.46 %), failed 0 · ASR 23.5 с · диаризация 33.9 с · пик VRAM 6 737.7 МБ
```

То есть ревью-фиксы поведение ASR/диаризации/merge не изменили (CUDA-путь).
Тесты на этом срезе: **Python 123** (`unittest`), `cargo test --workspace` —
все наборы `ok` (включая golden `.md`), `clippy -D warnings`/`fmt` — чисто.

### 0.5 Исправления код-ревью (`f1525d0`, 28 файлов, +787/−224)

| # | Дефект | Что сделано |
|---|---|---|
| 1 | Воркер перезапускался на каждый файл → модели грузились заново | `TranscribeClient` держит `Arc<Mutex<Option<Client>>>`, ленивый `spawn_python`, `shutdown` только при ошибке (`hds-index/transcribe.rs`); один клиент на демон (`autotranscribe.rs`) |
| 2 | `cancel` не работал во время `transcribe` (главный поток занят) | `hds_sidecar.runtime.serve(handle, idle, control=_control)` — `cancel` обрабатывается в reader-потоке |
| 3 | `transcribe.stop` отменял одиночные задания | `TranscribeConfig.stop_file: Option<PathBuf>`; демон задаёт файл, `transcribe_once` — `None`; `Settings::params(..., stop_file)` |
| 4 | DML/ONNX без word timestamps → весь текст `UNASSIGNED` | `merge.assign_speakers`: если `aligned`-слов нет вообще, атрибуция по пересечению **сегмента** слова с turns (`speaker_source="segment"`), в stats — `speaker_fallback_words` |
| 5 | `diarization.enabled` брался из `auto_transcribe`, а не из `transcribe.diarization` | `python_transcribe_to_file` читает `settings.diarization.enabled`; заголовки `.md` только при `status=ok|split` (`worker_diarized`) |
| 6 | `transcribe.profile: auto` всегда → `cuda` | `resolve_profile()`: профиль по фактически собранным venv (`venv-transcribe` → cuda, только `-dml` → dml), env приоритетнее конфига |
| 7 | `models_dir`/`hf_token_env` игнорировались | `Settings.models_dir`, `hf_token_env`, `worker_env()` → env воркера (`HDS_MODELS_DIR`, `HF_HOME=<models_dir>/hf-home`), `WorkerConfig.env` |
| 8 | `asr.revision` трактовался как HF-revision | `models.resolve_gigaam_source()`: локальный `.ckpt`/каталог со снапшотом, иначе `bad_request`/`model_missing` |
| 9 | `decode.sample_rate` мог сдвинуть таймкоды | жёстко 16 000 в `DecodeParams` + notice в `Settings` |
| 10 | `asr.vad` с опечаткой молча ломал VAD | whitelist `silero|off` в params/Settings + notice |
| 11 | `device_id`/`intra_threads` не доходили из конфига | читаются в `Settings` (DML-пин адаптера, ORT-потоки) |
| 12 | Коллизии `transcript.json`/`diarization.json` для разных файлов с одним stem | `cache.transcript_path` сверяет `media` и добавляет `-<sha8>`; `diarization_path(..., key)` — по ключу |
| 13 | `use_exclusive: false` игнорировался | merge по `regular`-turns, `exclusive` пустой |
| 14 | `gigastt_path()` искал бинарник в dev-пути | только `HDS_GIGASTT_CLI` (в поставке — env от установщика) |
| 15 | `hds transcribe-check` в legacy-режиме возвращал 1 без venv | `ok = is_python ? python.is_some() : true`, probe только для python |
| 16 | Мёртвый код диаризации/декодера (`map_labels`, `match_embeddings`, `_renumber`, `load_sources`), проглоченное исключение | удалено; `EMBED_CLUSTER_MIN = 0.60` (алгоритм не менялся ⇒ `DIARIZATION_VERSION` остался **8**) |
| 17 | `is_python()` чувствителен к регистру | `eq_ignore_ascii_case` в `Settings` и `TranscribeConfig` |
| 18 | Тесты зависели от установленных моделей/путей машины | `diag.rs`-тест с несуществующим Nemotron-3; новые `test_cache.py`, `test_models.py`, доп. тесты merge/protocol/params/glossary |

### 0.6 Решения владельца (не пересматривать)

1. **ASR:** GigaAM-v3 `e2e_rnnt` fp16 CUDA (ONNX CPU/DML — для профилей без CUDA).
2. **VAD:** Silero, окна ≤25 с, режем только в паузах; `transcribe_longform` не используем.
3. **Alignment default — `native`**; whisperx/hybrid доступны явно (только CUDA-venv).
   Пороги: gap 1,5 / dur 0,02 / timing_overlap 0,12 / unreliable 2 %.
4. **Диаризация:** pyannote community-1 4.0.0, блоки 20 мин, батчи 4/4 после
   `to(device)`, сшивка по эмбеддингам (**0,60**, union-find), кэш
   `DIARIZATION_VERSION=8`.
5. **Merge:** max-overlap; флаги слов не двигают спикера; `.md` рендерит Rust.
6. **Hotwords:** `backend: none` — G1/G2 не пройдены (18-мин корпуса достаточно,
   список сущностей подтверждён 22.10.2026). G3 закрыт.
7. **LLM:** off по умолчанию; P4 — только по явному включению (`llm.enabled: false`).
8. **UI не трогаем**; причины флагов — в `transcript.json`, агрегат — в
   `transcribe-check` и журнале. `needs_review` **дублируется** в `transcript.json`
   (метка на слове + секция `postprocess`) — решение 22.10.2026.
9. **Глоссарий:** рабочий файл `data/glossary.yaml` (`data/*` в `.gitignore`),
   в репозитории — пример `sidecar/hds_transcribe/glossary.example.yaml`.
10. **Legacy** (`transcribe.engine: legacy`) работает и не трогается.

---

## 1. Карта кода (после ревью)

### 1.1 Python (общий пакет + два процесса, общий CPython, venv по профилю)

```
sidecar/hds_sidecar/          common: stdio, runtime (serve(handle, idle, control=)),
                              config, progress, fs
sidecar/hds_extract/worker.py переведён на пакет
sidecar/hds_transcribe/
  worker.py        hello/probe/transcribe/postprocess/cancel/shutdown; _VramSampler;
                   _run_asr (CPU-fallback), _run_diarization, _align_accumulator,
                   _params_snapshot, edits_path, _load_glossary, _hotwords_info,
                   _annotate_review, _control (cancel в reader-потоке),
                   gigastt_path (только HDS_GIGASTT_CLI), RPC_BY_KIND
  params.py        TranscribeParams (asr/decode/align/diarization), AsrParams
                   (variant/vad whitelists), DecodeParams (sample_rate = 16000),
                   AlignQuality, PostprocessParams (glossary.path/terms,
                   hotwords.*, llm.*), LlmParams
  decode.py        ffprobe/ffmpeg; channels auto|downmix|split; window_correlations
  asr_gigaam.py    e2e_rnnt fp16/CPU (models.resolve_gigaam_source); speech_chunks (Silero)
  asr_onnx.py      EP cuda|dml|cpu; ORT_SEQUENTIAL + mem_pattern=False; без word timestamps
  align.py         native | whisperx | hybrid; map_segment (difflib); apply() → (segments, warnings, info)
  diarize_pyannote.py  safe_globals; блоки 15–30 мин; эмбеддинги + union-find (0.60); кэш
  merge.py         assign_speakers (fallback по сегменту, speaker_source),
                   join_words, build_blocks, merge_transcript (max-overlap)
  quality.py       classify(); флаги aligned/interpolated/low_confidence/failed +
                   alignment_reason; display_span; summarize(phrases)
  glossary.py      Glossary.load/from_dict/apply; normalize/strip_punct/tokens;
                   CONTEXT_WINDOW_WORDS=6; hotword_phrases; validate_hotwords;
                   write_hotwords_file (UTF-8 без BOM)
  cache.py         transcript.json/diarization.json (коллизии по media/key);
                   DIARIZATION_VERSION = 8
  models.py        gigaam cache/onnx/hf_home (models_dir → hf-home)/token;
                   resolve_gigaam_source (.ckpt); gated-проверка
  spoken.py        «произнесённая форма» для wav2vec2 (числа, «1С»→«один эс», латиница)
sidecar/tests/     Python 123 теста (params, merge, quality, align, spoken, блоки,
                   каналы, протокол, cache, models, glossary, postprocess)
```

**Контракты:** JSON-RPC 2.0 NDJSON, `PROTOCOL=1`, stdout только протокол, прогресс
в stderr (`[stage] …`), ошибки `{code, message, hint, kind}`; параметры приходят
**из Rust** (Python `config.yaml` не читает). Один воркер переиспользуется между
заданиями; `cancel` работает во время стадии; `idle_unload_sec`/`max_requests` —
на транспорте (`hds-extract::Worker`).

**Порядок конвейера** (`worker.transcribe`): `decode.probe` → `plan_channels` →
`_run_asr` → `align.apply` → `_words_from_segments` (quality) → `_run_diarization`
(кэш) → `merge.merge_transcript` / `_single_block` → `quality.summarize(phrases=blocks)`
→ `transcript.json` → ответ; `stats`: `decode/asr/align/diar/elapsed`, `rtf`
(ASR+diar), `rtf_total` (+align), `vram_peak_mb`.

**`postprocess` (P3, точка входа P4):** читает `transcript.json`, применяет
глоссарий по словам, пишет `<stem>.edits.json`, дублирует `needs_review` в
транскрипт (метка слова + секция `postprocess`), текст ASR **не** переписывает;
ASR не запускает. LLM пока только предупреждение (`llm.enabled` игнорируется).

### 1.2 Rust

```
crates/hds-extract/src/worker.rs   Worker::request, discover_python_with,
                                   WorkerConfig{..., env: Vec<(String,String)>}
crates/hds-transcribe/src/
  params.rs      TranscribeParams, AlignParams/AlignQuality, AsrParams (device_id,
                 intra_threads), DecodeParams, DiarizationParams, GlossaryParams{path,
                 terms: Vec<Value>}, HotwordsParams, LlmParams, PostprocessParams
                 {job_id, transcript_path, glossary, hotwords, llm}, Word (+needs_review)
  settings.rs    Settings{engine, profile, request_timeout_sec, idle_unload_sec,
                 max_requests, fallback_cpu, max_duration_hours, state_dir, language,
                 asr, align, decode, diarization, models_dir, hf_token_env,
                 glossary, hotwords, llm, notices}; resolve_profile(profile, root)
                 (auto → по собранным venv); params(media, job_id, diarization,
                 stop_file: Option<&Path>); worker_env(); python(root);
                 HOTWORDS_GATES_PASSED = false
  client.rs      ClientConfig{python, root, profile, idle_timeout, request_timeout,
                 max_requests, stderr_log, env}; Client::{spawn, hello, probe,
                 transcribe, postprocess, cancel, shutdown}
  error.rs       ErrorKind (bad_request/model_missing/cuda_oom/…/align_failed)
crates/hds-index/src/
  transcribe.rs  TranscribeConfig{engine, settings: Option<Settings>, stop_file,
                 out_dir, service_dir, diarization(legacy), …}; TranscribeClient
                 {cfg, worker: Arc<Mutex<Option<Client>>>} — python_params/
                 spawn_python/transcribe/transcribe_to_file; render_markdown;
                 worker_diarized(status ok|split); segments_from_blocks;
                 detect_speaker_slots; next_free_path
  autotranscribe.rs  transcribe_once (stop_file=None), run_transcribe_watch
                 (один клиент на демон), process_job(..., &client)
  diag.rs        hds check: backend диаризации/модель/EP
crates/hds-cli/src/cmd/transcribe_check.rs   alias whisper-check: engine/profile/
                 probe/alignment/glossary/hotwords + последние транскрипции
                 (mode/aligned/low/failed/reasons/rtf/vram/review)
```

`Config = serde_yaml::Value`; serde игнорирует неизвестные поля JSON. После правок —
`cargo fmt`, `clippy -D warnings`.

### 1.3 Артефакты и файлы (контракты)

| Файл | Кто пишет | Что внутри |
|---|---|---|
| `state_dir/<stem>.transcript.json` | Python | блоки, слова (флаги качества/спикеры/`needs_review`), `alignment_quality`, `stats`, параметры, `postprocess` (после P3) |
| `state_dir/<stem>.diarization.json` | Python | ключ кэша, `exclusive`/`regular`, `status`, `rtf`, VRAM |
| `state_dir/<stem>.edits.json` | Python | `edits[]` (глоссарий; в P4 — +LLM), `stats`, `reasons`, `hotwords`, `edits_path` |
| `out_dir/<stem>.md` / `.orig.md` / `.speakers.json` | Rust | `.md` — с метками спикеров; `.orig.md` — текст после postprocess до подстановки имён; коллизии — суффикс `-2` |
| `state_dir/transcribe-worker.log` | Rust | stderr воркера (`[stage] …`) |

## 2. Тесты, окружение, команды

```powershell
cd C:\Users\Sasha\hermes-disk-search
$py = 'D:\hds-py-transcribe\venv-cuda\Scripts\python.exe'
$env:HDS_TRANSCRIBE_PYTHON = $py
$env:HDS_GIGAAM_CACHE = 'D:\hds-py-transcribe\gigaam-cache'
$env:HF_HOME = 'D:\hds-py-transcribe\hf'
$env:PYANNOTE_METRICS_ENABLED = '0'

# Python (123 теста; PowerShell может показать «exit 1» из-за ResourceWarning в stderr —
# смотреть на строку OK)
$env:PYTHONPATH = 'sidecar'
& $py -m unittest discover -s sidecar\tests -t sidecar

# Rust
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# e2e без Rust-клиента (18 мин):
$e = 'tools\parity\out\py_transcribe'
& $py $e\rpc_once.py $py sidecar\hds_transcribe\worker.py $e\p4\params_smoke_meet0924.json $e\p4\smoke_result.json
& $py $e\eval_p1.py $e\p4\state\src_0924.transcript.json $e\p4\state\src_0924.diarization.json `
    D:\hds-py-transcribe\work\ref_0924.txt 1076.2 $e\p4\smoke_eval.json

# диагностика стека (боевой конфиг; в dev-конфиге нужен transcribe.engine: python)
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8   # иначе кириллица в логе двоится
.\target\debug\hds.exe transcribe-check                     # или --json
```

* `target` — junction на `D:\hds-py-transcribe\cargo-target`; сборки только туда.
* Артефакты замеров — только `tools/parity/out/` (в `.gitignore`: `out/`, `data/*`).
* venv: `D:\hds-py-transcribe\venv-cuda` (torch 2.8.0+cu128, gigaam, pyannote
  4.0.0, whisperx опционально), `venv-dml`; `sidecar/venv-transcribe[-dml]` —
  «поставочные» имена для `profile: auto` и `hds`.
* `llm_host` (резидент, владелец GPU) поднят: `http://127.0.0.1:8010/v1`,
  роли `chat`, `chat-think`, `embedding`, `rerank` (`/v1/models`).

## 3. Факты и грабли (обновлено после ревью)

1. **pyannote:** батчи 4/4 **после** `to(device)`; пик мерить `_VramSampler`;
   при смене алгоритма диаризации поднимать `DIARIZATION_VERSION` (сейчас **8**).
   Сшивка блоков — глобальная кластеризация эмбеддингов (`EMBED_CLUSTER_MIN=0.60`),
   per-block `map_labels`/`match_embeddings` удалены.
2. **WER воспроизводится только на `src_0924.mp3`** (+`ref_0924.txt`); 18 мин
   достаточно для решений P3 (подтверждено владельцем).
3. **gigastt:** hotwords только CLI/HTTP, Python-биндинг их не имеет; hotword-файл
   **UTF-8 без BOM** (иначе фразы молча отбрасываются); бинарник ищется **только**
   по `HDS_GIGASTT_CLI`; в Rust не встраивать (`ort` rc.13 vs rc.15).
   `hotwords.backend: none` — G1/G2 не пройдены, «включим на глаз» запрещено.
4. **WhisperX** — только CUDA-venv, в `requirements.lock` закомментирован;
   `hybrid` на cpu/dml деградирует в native с warning, явный `whisperx` →
   `align_failed`. Default остаётся `native`.
5. **Долгоживущий воркер:** Rust держит один процесс; не вызывать `shutdown`
   после каждого файла (в P1–P3 это был дефект). `idle_unload_sec`/`max_requests`
   работают на транспорте.
6. **Отмена:** `cancel` обрабатывается в reader-потоке; `transcribe.stop` слушает
   **только демон** (`transcribe_once` передаёт `stop_file=None`), иначе старый
   файл отменял бы одиночное задание.
7. **ONNX/DML без word timestamps:** все слова `interpolated`; merge атрибутирует
   по пересечению ASR-сегмента с turns (`speaker_source="segment"`), в
   `stats.speaker_fallback_words`. Это не доказательство смены спикера.
8. **Декодирование всегда 16 кГц.** `decode.sample_rate` с другим значением
   игнорируется с notice.
9. **Имена артефактов:** при коллизии stem (другой `media`) — `-<sha8>` в имени
   `transcript.json`; `diarization.json` — по ключу кэша. Повторные прогоны в тот
   же `state_dir` могут дать `-2` в `.md` — для чистых замеров отдельные каталоги.
10. **`hds transcribe-check`** читает до 3 последних `*.transcript.json` из
    `transcribe.state_dir`; в legacy-режиме `ok = true` без venv.
11. **PowerShell:** `>`/`*>` пишут **UTF-16** (читать `encoding='utf-16'` или
    `[Console]::OutputEncoding = UTF8` + `Out-File -Encoding utf8`); многострочный
    `python -c` ломается — писать скрипт-файлы; `rpc_once.py` буферизует stderr до
    конца задания; длинные прогоны — `Start-Process` + опрос лога.
12. **Rust:** `Config = serde_yaml::Value` (тесты — `serde_yaml::from_str`);
    неизвестные поля JSON игнорируются; после правок `cargo fmt`, clippy.
13. **Dev-конфиг:** корневой `config.yaml` (gitignored) **не содержит** секции
    `transcribe:` ⇒ CLI идёт `engine: legacy`. Для живых прогонов добавить
    `transcribe.engine: python`, `state_dir`, при необходимости `models_dir`.
14. **LLM (для P4):** у `llm_host` роли `chat`/`chat-think`; по плану §4.6 —
    Qwen2.5-7B-Instruct Q4_K_M (сравнить с Q5_K_M или ролью `chat`), одна LLM на
    машине; перед вызовом LLM Python выгружает ASR/alignment/диаризацию
    (`del`, `gc.collect()`, `torch.cuda.empty_cache()`), иначе пик VRAM суммируется.
15. **`needs_review`:** в `edits.json` (полный журнал) + в `transcript.json`
    (`words[].needs_review` и `postprocess.needs_review[]`); при пустом глоссарии
    разметка снимается, текст ASR в файле не переписывается никогда.
16. **Тесты не должны зависеть от установленных моделей машины** (`diag.rs`
    фиксирует несуществующий путь Nemotron-3) — правило после ревью.
17. **VRAM к моменту LLM:** ASR-модель выгружается (`asr_gigaam.unload`), сэмплы
    удаляются, но кэш-аллокатор torch держит память — перед вызовом `llm_host`
    нужен `torch.cuda.empty_cache()` (и закрытие ORT-сессий на DML).

---

## 4. Следующий этап — P4: LLM-постобработка (план §4.6, §9 P4)

**Цель:** второй слой postprocess — LLM предлагает **только JSON-патчи**, Python
их валидирует и применяет; глоссарий остаётся первым (детерминированным) слоем.
LLM **не** загружается в Python: инференс — в `llm_host` (`/v1/chat/completions`),
Rust только оркестрирует. По умолчанию `llm.enabled: false`.

### P4.0 Проводка конфига (Rust)

* `Settings` сейчас отдаёт `llm: LlmParams::default()` и notice
  «`transcribe.llm.enabled=true` игнорируется в P1 (LLM — P4)» — **читать
  `transcribe.llm.*`** (`enabled`, `base_url`, `model`, `temperature`,
  `max_tokens`, `timeout_sec`, `max_edits_per_1000`) и пробрасывать в
  `PostprocessParams` (поле есть, `hds-index` его уже передаёт);
* notice заменить на честный: LLM включён/выключен, роль/модель, лимит правок;
  при `enabled: true` и недоступном `llm_host` — предупреждение, не падение;
* таймауты: `transcribe.request_timeout_sec` покрывает всё задание,
  `llm.timeout_sec` — один HTTP-запрос.

### P4.1 Клиент LLM + кандидаты (Python)

* новый модуль `sidecar/hds_transcribe/postprocess.py` (или `llm_patch.py`):
  stdlib `urllib.request`, OpenAI-совместимый `/v1/chat/completions`,
  `temperature: 0`, `max_tokens`, `timeout_sec`;
* **кандидаты — только подозрительные фрагменты**, не весь транскрипт: попадания
  вариантов глоссария, слова `low_confidence`/`failed`, паттерны «Индинес/1С»,
  редкие слова; контекст ±N слов и `word_id`/спикер/таймкоды — из транскрипта;
* промпт требует **строго JSON** `{"edits": [{"word_ids": [...],
  "replacement": "...", "reason": "...", "needs_review": bool}]}`; ответ парсится
  терпимо (markdown-обёртки, лишние поля), мусор → патч отклонён;
* `llm_host` недоступен/таймаут ⇒ warning в `warnings` и результат как без LLM
  (глоссарий применён), задание не падает.

### P4.2 Валидатор (Python, без LLM) — гейт P4

Патч принимается целиком или отклоняется целиком (причина — в журнал):

* `word_ids` существуют и лежат в одном блоке/у одного спикера;
* замена **не меняет** `speaker`, `start`, `end`, `alignment`;
* не исчезают отрицания (`не`, `нет`, `без`), не меняются числа/даты/суммы без
  отдельного разрешения (нормализованное сравнение числительных);
* длина замены ≤ 3× длины фрагмента (кроме канонов глоссария);
* контекст совпадает с `context` глоссария либо `allow_global: true`;
* не более `llm.max_edits_per_1000` правок на 1000 слов;
* патчи не пересекаются по `word_ids` с уже применёнными глоссарием;
* идемпотентность: повторный `postprocess` даёт тот же результат (LLM-правки в
  `transcript.json` не пишутся, как и глоссарные).

### P4.3 Применение и журнал

* замена — по словам (`word_id`), затем `block.text = merge.join_words(...)`;
* `edits.json` расширяется: `source: "glossary" | "llm"`, `status:
  "applied" | "rejected" | "needs_review"`, `reason` (включая отказ валидатора),
  `model`, `latency_ms`; формат P3 не ломать (поля аддитивные);
* `transcript.json.postprocess` — сводка: `llm: {enabled, model, patches,
  applied, rejected}`, `needs_review[]` (глоссарий + LLM);
* `.orig.md` = текст после postprocess **до** подстановки имён (уже так);
  исходный ASR-текст остаётся в `transcript.json`.

### P4.4 VRAM и жизненный цикл

* перед HTTP-вызовом освободить GPU: `gc.collect()` + `torch.cuda.empty_cache()`
  (на DML — закрыть ORT-сессии), иначе пик = ASR + диаризация + LLM;
* одна LLM на машине: `llm_host` сам вытесняет роль; Python ничего не грузит;
* замер пика — `_VramSampler` / `hds check`.

### P4.5 Модель и замеры

* базовый кандидат — **Qwen2.5-7B-Instruct Q4_K_M**, сравнить с Q5_K_M и/или
  ролью `chat` (сейчас `llm_host` отдаёт `chat`, `chat-think`, `embedding`, `rerank`);
* метрики на `meet0924` (18 мин): предложено/принято/отклонено патчей, precision
  правок (ручной просмотр ≤30), WER/CER **не растут** (ASR не перезапускается),
  latency на файл, пик VRAM; артефакты — `tools/parity/out/py_transcribe/p4/`;
* стоимость: 7–9B Q4 ≈ 20–40 токенов/с ⇒ полный 77-мин транскрипт — десятки
  минут; LLM остаётся opt-in (`enabled: false` по умолчанию).

### P4.6 Rust-обвязка

* `postprocess` вызывать при `llm.enabled` **или** непустом глоссарии (сейчас
  вызывается всегда — убедиться, что при выключенном LLM и пустом глоссарии
  результат = P2);
* `warnings` postprocess → stderr задания (как сейчас); агрегат правок — в
  `transcribe-check` (llm-патчи/отклонённые) и `transcript.json`;
* `transcribe-check` / `hds check`: строка `llm: enabled=false, model=…, host=…`,
  проверка `llm_host` (`/v1/models`) без падения при выключенном.

### P4.7 Тесты

* Python: фейковый OpenAI-сервер (`http.server` в тесте) — валидный патч;
  отклонения (спикер, таймкоды, числа, отрицания, длина, лимит); мусор в ответе;
  таймаут; недоступный хост (деградация без падения); идемпотентность;
  «LLM off → результат идентичен P3»; пустой глоссарий + LLM off → P2;
* Rust: чтение `transcribe.llm.*` и проброс в `PostprocessParams`; тексты
  notices; golden `.md` не меняется.

### P4.8 Отчёт

`tools/parity/out/py_transcribe/P4_REPORT.md`: модель/роль, число патчей,
precision, latency, VRAM, решение по `llm.enabled`/модели, команды, остатки.

### Остатки (не блокируют P4)

* P5: упаковка venv (cuda/cpu/dml), whisperx в пакет, установщики моделей;
* P6: удаление legacy-цепочки, релиз;
* UI: word-level подсветка `needs_review`/флагов — после P5 (в `hds-ui` сейчас
  нет работы со словами);
* ручная разметка таймкодов (абсолютный MAE) — по желанию.

---

## 5. Открытые вопросы владельцу (решить до/во время P4)

1. **Модель для LLM:** Qwen2.5-7B-Instruct **Q4_K_M** (план) vs Q5_K_M vs
   текущая роль `chat`? Есть ли GGUF нужной модели в `models_dir`?
2. **Формат промпта/правок:** ок ли `word_ids` + `replacement` (§4.6), или
   предпочитаете плоский `before/after` (проще для UI)?
3. **Лимит правок:** `llm.max_edits_per_1000: 10` (план) — оставляем?
4. **Что считать «числами/отрицаниями»:** запрещать **любое** изменение цифр и
   частицы `не`, или разрешать, если слово из глоссария?
5. **Куда смотреть `needs_review` из LLM:** только `transcript.json`+`edits.json`
   (как сейчас для глоссария) — подтверждаете?
6. **Ручная разметка 20–30 фраз** для абсолютного MAE и возврата к `hybrid` —
   делаем в P4 или оставляем на P6?

---

## 6. Ограничения

* **Коммитить по завершении этапа** (осмысленными блоками, сообщения в стиле
  репозитория: `feat(transcribe): …`, `fix(transcribe): …`). Push — только по
  явной просьбе владельца.
* Не переписывать `PLAN_*.md` и не менять решения P0–P3 (§0.6).
* Не ломать `engine: legacy` и контракты `.md`/`.orig.md`/`.speakers.json`.
* UI (`hds-ui`) не трогать; `needs_review` уже есть в JSON — UI позже.
* Артефакты/логи — только `tools/parity/out/py_transcribe/` (gitignored).
* Не «улучшать» метрики подгонкой: любые изменения качества — с A/B на `meet0924`.
* Если нужен HF-токен, новые эталонные тексты, RTTM/ручные таймкоды, живая
  Radeon/DML-проверка или решение по контракту — **спросить владельца**.

## 7. Первые действия в новом чате (чек-лист)

```powershell
cd C:\Users\Sasha\hermes-disk-search
git status --short --branch                  # чисто, ветка python-sidecar-transcribe
git --no-pager log -3 --oneline --decorate   # f1525d0 (HEAD), c0d6915, f202128
git rev-list --count origin/server-multiuser..HEAD   # 2 — не запушено
```

Прочитать: этот файл (§0–§6), `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` §4.6, §9 P4, §6;
`sidecar/hds_transcribe/README.md`; `tools/parity/out/py_transcribe/P3_REPORT.md`;
код: `worker.postprocess`, `_load_glossary`, `_hotwords_info`, `_annotate_review`
(`sidecar/hds_transcribe/worker.py`), `PostprocessParams`/`LlmParams`
(`params.py`), `glossary.py`, `merge.join_words`; Rust: `hds-transcribe/src/
{params,settings,client}.rs`, `hds-index/src/transcribe.rs`
(`python_transcribe_to_file`), `hds-cli/src/cmd/transcribe_check.rs`,
`config.example.yaml`.

Затем (не начиная с кода):
1. Прогнать тесты: Python (123, `unittest`), `cargo test --workspace`,
   `clippy -D warnings`, `fmt --check`.
2. Проверить `llm_host`: `Get-Process llm_host`, `GET /v1/models` (роли
   `chat`/`chat-think`), `GET /health`; зафиксировать свободную VRAM.
3. Быстрый e2e-смоук: `frag60` (≤1 мин) через `rpc_once.py`, чтобы убедиться,
   что воркер жив; при необходимости — 18 мин на `params_smoke_meet0924.json`.
4. Только после этого — план P4 (P4.0–P4.8) и код, начиная с **P4.0** (конфиг).

## 8. Промт для нового чата (копировать целиком)

```text
Ты — Cline в режиме Act. Продолжаешь проект hermes-disk-search
(C:\Users\Sasha\hermes-disk-search), ветка python-sidecar-transcribe.
HEAD f1525d0: P1–P3 закоммичены (c0d6915), дефекты код-ревью исправлены
(f1525d0); база — server-multiuser @ f202128 (tag v0.21.0). Ветка на 2 коммита
впереди origin, НЕ запушена, рабочая копия чистая. Твоя задача — P4:
LLM-постобработка (JSON-патчи через llm_host) при включённом transcribe.llm.*.

ПЕРВЫЕ ДЕЙСТВИЯ (не начинай с кода):
1) git status --short --branch; git --no-pager log -3 --oneline --decorate;
   git rev-list --count origin/server-multiuser..HEAD
2) прочитай PYTHON_SIDECAR_TRANSCRIBE_P4_HANDOFF_PROMPT.md целиком
3) прочитай PLAN_PYTHON_SIDECAR_TRANSCRIBE.md §4.6, §9 P4, §6
4) прочитай sidecar/hds_transcribe/README.md (§ «Эксплуатационные детали») и
   tools/parity/out/py_transcribe/P3_REPORT.md (числа G0–G3, формат edits.json)
5) прочитай код: sidecar/hds_transcribe/{worker.py (postprocess, _load_glossary,
   _hotwords_info, _annotate_review), params.py (PostprocessParams, LlmParams),
   glossary.py, merge.py (join_words)};
   crates/hds-transcribe/src/{params,settings,client}.rs;
   crates/hds-index/src/transcribe.rs (python_transcribe_to_file);
   crates/hds-cli/src/cmd/transcribe_check.rs; config.example.yaml
6) прогони тесты: Python unittest (123), cargo test --workspace,
   clippy -D warnings, fmt --check; и быстрый e2e-смоук через
   tools/parity/out/py_transcribe/rpc_once.py (frag60 или 18 мин meet0924)
7) проверь llm_host: GET http://127.0.0.1:8010/v1/models (chat, chat-think,
   embedding, rerank) и /health; зафиксируй свободную VRAM
Только после этого пиши код.

ФАКТЫ P0–P3 (база для сравнения, не пересчитывать):
- meet0924 18 мин (src_0924.mp3 + ref_0924.txt): WER 17,82 %, CER 9,52 %,
  DER 13,00 %, атрибуция по словам 90,68 %, low_confidence 11 (0,46 %),
  блоки 286, пик VRAM 6 739–6 749 МБ, ASR 19–27 с (RTF ~0,02–0,026).
- meet0512 77 мин: ASR ~75 с, диаризация ~151 с, 4 блока, 10 спикеров,
  пик 5 823 МБ; hybrid: +align ~76 с (итого 313,6 с = 1,46× P1).
- low_confidence+failed: native 0,46 %, hybrid 0,71 %, whisperx 0,63 % (≤2 %).
- G0: biasing gigastt работает («RP-система» → «ERP-система», boost 8), но
  G1 не пройден (INT8 теряет ERP: recall 0,70 vs 0,90), G2 даёт лишь +28,6 %
  отн. к INT8 без hotwords (= уровень fp16), G3 пилот без выигрыша.
  Решение: hotwords.backend = none (код: HOTWORDS_GATES_PASSED = false).
- Проверка после ревью (p4/smoke_*): WER 17,82 / CER 9,52 / DER 13,00 /
  атрибуция 90,7, low 11, блоки 286, VRAM 6 737,7 МБ — совпадает с P2.
- Тесты на срезе: Python 123; cargo test --workspace, clippy, fmt — зелёные.
- Ревью-фиксы f1525d0: долгоживущий воркер (Arc<Mutex<Option<Client>>>),
  cancel в reader-потоке, stop_file только демону, DML-атрибуция по сегменту
  (speaker_source), diarization.enabled из transcribe.*, profile auto по venv,
  models_dir/hf_token_env → env воркера, asr.revision = локальный .ckpt,
  sample_rate фиксирован 16 кГц, vad whitelist, device_id/intra_threads,
  коллизии transcript/diarization по media/key, use_exclusive, gigastt только
  через HDS_GIGASTT_CLI, transcribe-check ok в legacy, удалён мёртвый код.

ПОДТВЕРЖДЁННЫЕ РЕШЕНИЯ (не пересматривать):
- ASR GigaAM-v3 e2e_rnnt fp16 CUDA; VAD Silero окна ≤25 с; align default native
  (пороги gap 1,5 / dur 0,02 / overlap 0,12 / unreliable 2 %);
- диаризация pyannote community-1 4.0.0, блоки 20 мин, батчи 4/4, сшивка
  эмбеддингами 0,60, DIARIZATION_VERSION=8; merge max-overlap; .md рендерит Rust;
- hotwords none; LLM off по умолчанию; UI не трогаем; legacy не трогаем;
- needs_review дублируется в transcript.json (решение 22.10.2026);
- глоссарий: data/glossary.yaml (gitignored), пример — sidecar/hds_transcribe/
  glossary.example.yaml; порядок слоёв: глоссарий → LLM.

ЗАДАЧИ P4 (в этом порядке):
P4.0 Rust Settings: читать transcribe.llm.{enabled,base_url,model,temperature,
     max_tokens,timeout_sec,max_edits_per_1000} → LlmParams →
     PostprocessParams; заменить notice «игнорируется в P1» на честный статус.
P4.1 Python-модуль LLM-постобработки: urllib → /v1/chat/completions
     (temperature 0), кандидаты только по подозрительным фрагментам
     (варианты глоссария, low_confidence/failed, паттерны), промпт «только JSON
     edits: word_ids/replacement/reason/needs_review», терпимый разбор ответа.
P4.2 Валидатор патчей (без LLM): word_ids в одном блоке/у одного спикера; не
     менять speaker/start/end/alignment; не терять отрицания и числа/даты;
     длина ≤3×; контекст глоссария или allow_global; ≤max_edits_per_1000;
     пересечения запрещены; патч принимается/отклоняется целиком с причиной.
P4.3 Применение по словам + пересборка block.text (merge.join_words);
     edits.json аддитивно: source=glossary|llm, status=applied|rejected|
     needs_review, model, latency_ms; transcript.json.postprocess — сводка llm
     и needs_review; идемпотентность; «LLM off + пустой глоссарий» = P2.
P4.4 Перед вызовом llm_host освободить GPU (gc + torch.cuda.empty_cache,
     на DML — закрыть ORT-сессии); одна LLM на машине.
P4.5 Замеры: Qwen2.5-7B-Instruct Q4_K_M против Q5_K_M/роли chat на meet0924:
     предложено/принято/отклонено, precision (ручной просмотр ≤30), WER/CER не
     растут, latency, пик VRAM; артефакты p4/.
P4.6 Rust-обвязка: вызов postprocess при llm.enabled или непустом глоссарии;
     warnings → журнал; transcribe-check/hds check показывают llm.enabled,
     model, host и счётчики патчей.
P4.7 Тесты: фейковый OpenAI-сервер (http.server): валидный патч, отказы по
     спикеру/таймкодам/числам/отрицаниям/длине/лимиту, мусор, таймаут,
     недоступный хост (деградация), идемпотентность, LLM off = P3, пустой
     глоссарий = P2; Rust — чтение llm.* и проброс; golden .md не меняется.
P4.8 Отчёт tools/parity/out/py_transcribe/P4_REPORT.md: модель, числа, precision,
     latency, VRAM, решение по llm.enabled/модели, команды, остатки.

ГЕЙТ P4: ни один патч не меняет спикера/таймкоды/alignment; отклонённые патчи
не портят текст; llm.enabled=false → результат идентичен P3; повторный
postprocess не запускает ASR и идемпотентен; cargo test/clippy/fmt и
Python-тесты зелёные; engine: legacy и контракты .md/.orig.md/.speakers.json/UI
не тронуты. Если llm_host недоступен или патчи не проходят валидатор —
деградация до глоссария с warning, без падения задания.

ГРАБЛИ (обязательно):
- pyannote: батчи 4/4 после to(device), пик — _VramSampler; менять алгоритм
  диаризации — только с DIARIZATION_VERSION++;
- WER воспроизводится только на src_0924.mp3; сравнение — с baseline P2/P3;
- gigastt: hotwords только CLI/HTTP, файл UTF-8 без BOM, бинарник только через
  HDS_GIGASTT_CLI, backend остаётся none;
- Rust держит ОДИН долгоживущий воркер (не shutdown после файла); cancel идёт
  через reader-поток; transcribe.stop слушает только демон;
- DML/ONNX без word timestamps: слова interpolated, атрибуция по сегменту
  (speaker_source), это не доказательство смены спикера;
- PowerShell: `>`/`*>` пишут UTF-16 (encoding='utf-16' / Console.OutputEncoding);
  многострочный python -c ломается — скрипт-файлы; rpc_once буферизует stderr;
  длинные прогоны — Start-Process + опрос лога;
- target — junction на D:\hds-py-transcribe\cargo-target; сборки только туда;
- HDS_TRANSCRIBE_PYTHON → venv-cuda; HDS_GIGAAM_CACHE/HF_HOME на D:;
- Rust: Config = serde_yaml::Value, неизвестные поля JSON игнорируются,
  после правок cargo fmt + clippy; тесты не должны зависеть от установленных
  моделей машины.

ОГРАНИЧЕНИЯ: коммитить изменения по завершении этапа (push — только по явной
просьбе); не переписывать
PLAN_*.md; не менять решения P0–P3; не трогать UI и legacy; артефакты — только
tools/parity/out/; если нужен HF-токен, новые эталонные тексты, RTTM/ручные
таймкоды, живая Radeon или решение по контракту (формат патчей, модель LLM) —
спроси.

Начни с первых действий, затем кратко изложи план P4 (P4.0–P4.8) и приступай
к P4.0 — проводке transcribe.llm.* в Rust.
```
