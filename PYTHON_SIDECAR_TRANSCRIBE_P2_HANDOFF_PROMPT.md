# PYTHON_SIDECAR_TRANSCRIBE_P2_HANDOFF_PROMPT.md — контекст и промт для P2

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать **§9 целиком**
> и отправить первым сообщением. §§0–8 — карта состояния P1 (читать выборочно).
>
> **Дата среза:** 22.10.2026. **Ветка:** `python-sidecar-transcribe`
> (база `server-multiuser` @ `f202128`, tag v0.21.0).
> **P1 завершён, но НЕ закоммичен** — все изменения в рабочей копии.
> **Ключевые файлы:** `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` (план, untracked),
> `PYTHON_SIDECAR_TRANSCRIBE_P1_HANDOFF_PROMPT.md` (handoff P1),
> `tools/parity/out/py_transcribe/P1_REPORT.md` (отчёт P1, gitignored),
> `tools/parity/out/py_transcribe/spike/REPORT.md` (P0).

---

## 0. Статус на 22.10.2026 (P1 закрыт)

**Работает end-to-end** (`transcribe.engine: python`):

* `hds transcribe-once file.mp3` → `.md` (`### SPEAKER_NN [HH:MM:SS - HH:MM:SS]`),
  `.orig.md`, `.speakers.json`, `transcript.json`, `diarization.json`;
* демон `hds transcribe-watch`: inbox → out, `source_disposal: delete` (проверено),
  манифест, идемпотентность, `transcribe.lock`;
* `hds transcribe-check` (alias `whisper-check`), пункт `transcribe` в `hds check`;
* `engine: legacy` — прежний HTTP-путь не тронут (тесты зелёные).

**Приёмка P1 (факты):**

| Проверка | Факт |
|---|---|
| `meet0924` 18 мин (реальный mp3 48 кГц stereo) | WER **17,82 %**, CER **9,52 %**, DER **13,00 %** (весь miss) |
| Атрибуция по словам | **90,68 %** (P0: 90,8 %) — принято владельцем как шум |
| ASR | 19,6 с, RTF **0,0182**, пик VRAM **6 749 МБ** |
| Диаризация 18 мин | 33,8 с, RTF 0,031 |
| `meet0512` 77 мин | ASR 66,1 с (0,0143), диаризация **149 с** (0,032), **4 блока** 20 мин, **10 спикеров** (P0: 9), пик **5 823 МБ** |
| Кэш диаризации | повтор — `[stage] diar 100% cached=True` |

**Решения владельца (22.10.2026, не пересматривать):**
1. Атрибуция 90,68 % (3 слова «Титунова») — принимается как шум.
2. 10 спикеров на 77 мин — ожидаемое поведение блочной диаризации без RTTM.
3. ASR — `e2e_rnnt` fp16 CUDA; alignment default — `native`; VAD — Silero;
   hotwords `none`; LLM off; профиль CUDA основной, CPU — для iGPU, DML — fallback.

---

## 1. Карта кода (что уже есть)

### Python (общий пакет + два процесса, общий CPython, venv по профилю)

```
sidecar/hds_sidecar/          common: stdio, runtime, config, progress, fs
sidecar/hds_extract/worker.py переведён на пакет; поведение не менялось (11/11)
sidecar/hds_transcribe/
  worker.py        hello/probe/transcribe/postprocess/cancel/shutdown; _VramSampler
  params.py        TranscribeParams/AsrParams/AlignParams/DiarizationParams/DecodeParams/PostprocessParams
  decode.py        ffprobe/ffmpeg; channels auto|downmix|split; window_correlations
  asr_gigaam.py    e2e_rnnt fp16/CPU; speech_chunks (Silero, режем только в паузах); Word
  asr_onnx.py      EP cuda|dml|cpu; SessionOptions ORT_SEQUENTIAL + mem_pattern=False
  diarize_pyannote.py  safe_globals; waveform; блоки; эмбеддинги+union-find; кэш
  align.py         native; whisperx/hybrid — заглушка (warning + native)
  merge.py         чистые функции (max-overlap, ambiguous/overlap, блоки)
  quality.py       флаги aligned/interpolated/low_confidence/failed + summarize
  cache.py         transcript.json/diarization.json; DIARIZATION_VERSION=8
  models.py        gigaam cache/onnx/hf token/offline; gated-проверка
  fs_compat.py     фасад hds_sidecar.fs
sidecar/tests/     38 тестов (params, merge, quality, блоки, каналы, протокол)
```

Контракты: JSON-RPC 2.0 NDJSON, `PROTOCOL=1`, stdout только протокол, прогресс в
stderr (`[stage] …`), ошибки `{code, message, hint, kind}` (машинный `kind`),
параметры приходят **из Rust** (Python `config.yaml` не читает).

### Rust

```
crates/hds-extract/src/worker.rs   Worker::request(method, params, retry_dead),
                                   discover_python_with(env_var, venv_rel), last_error()
crates/hds-transcribe/             типы (params.rs), Settings (settings.rs), Client (client.rs),
                                   ErrorKind/TranscribeError (error.rs)
crates/hds-index/src/transcribe.rs engine python|legacy; рендер .md; python_transcribe_to_file
crates/hds-cli/src/cmd/transcribe_check.rs  transcribe-check (alias whisper-check)
crates/hds-index/src/diag.rs       transcribe_summary + check_transcribe в run_checks
config.example.yaml                секция transcribe.* (аддитивно)
```

`Settings` читает `transcribe.*` и маппит старые ключи (`auto_transcribe.diarization*`,
`index.whisper_*`, `diarization.*`). `transcribe.state_dir` по умолчанию
`data/auto-transcribe`; `stop_file` — `<корень>/transcribe.stop` (как у демона).

---

## 2. Тесты, сборка, окружение

```powershell
# Python (pytest в venv-cuda НЕ установлен; тесты pytest-совместимы)
& .\tools\parity\out\py_transcribe\venv-cuda\Scripts\python.exe `
  -m unittest discover -s sidecar/tests -t sidecar

# Rust (target — junction на D:\hds-py-transcribe\cargo-target)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# live-тест извлечения (нужен рабочий интерпретатор: .venv в репо сломан)
$env:HDS_EXTRACT_PYTHON = (Resolve-Path `
  'tools\parity\out\sidecar_a\venv\Scripts\python.exe').Path
cargo test -p hds-extract --test worker_live
```

**Окружения P0/P1 (готовы, не пересобирать без нужды):**

```
tools/parity/out/py_transcribe/venv-cuda -> D:\hds-py-transcribe\venv-cuda
tools/parity/out/py_transcribe/venv-dml  -> D:\hds-py-transcribe\venv-dml
venv-cuda: torch 2.8.0+cu128, transformers 4.57.1, pyannote 4.0.0,
           whisperx 3.8.7rc1, onnxruntime 1.23.2 (EP Azure/CPU), silero-vad 6.2.3,
           gigaam 0.2.0 @ 7447938d (Word timestamps)
venv-dml : torch 2.8.0+cpu, onnxruntime-directml 1.24.4, БЕЗ onnxruntime и whisperx
```

Переменные для живых прогонов:

```powershell
$env:HDS_TRANSCRIBE_PYTHON = (Resolve-Path 'tools\parity\out\py_transcribe\venv-cuda\Scripts\python.exe').Path
$env:HDS_GIGAAM_CACHE = 'D:\hds-py-transcribe\gigaam-cache'
$env:HF_HOME = 'D:\hds-py-transcribe\hf'
$env:PYANNOTE_METRICS_ENABLED = '0'
```

**Корпус и артефакты:**

* эталон: `D:\hds-py-transcribe\work\src_0924.mp3` (18 мин, 48 кГц stereo),
  `ref_0924.txt` (98 реплик, 7 имён);
* `D:\diar-spike-data\meet0924.wav`, `meet0514.wav` (56 мин), `meet0512.wav` (77 мин), `frag60.wav`;
* скрипты замеров: `tools/parity/out/py_transcribe/` —
  `rpc_once.py` (hello+transcribe+postprocess), `eval_p1.py` (WER/CER/DER/атрибуция
  через `spike/scripts/eval_reference.py`), `diff_p0.py`, `diar_only.py`,
  `diag_vram_p0.py`, `worker_smoke.py` + `compare_workers.py` (паритет извлечения);
* примеры конфигов/параметров: `params_meet0924.json`, `params_meet0512.json`,
  `params_frag60_diar.json`, `p1_e2e/config.yaml`, `p1_watch/config.yaml`.

---

## 3. Грабли P1 (обязательно учитывать в P2)

1. **pyannote community-1: батчи ≥ 8 = ~9–10 ГБ VRAM транзиентно даже на 60 с.**
   В `diarize_pyannote.load()` выставляются `segmentation_batch_size=4`,
   `embedding_batch_size=4` **после** `pipeline.to(device)`; менять только с замером
   (`diag_vram_p0.py <seg> <max_spk> <emb>`).
2. **`torch.cuda.max_memory_allocated()` сбрасывается внутри pyannote** — пик мерить
   своим сэмплером (`_VramSampler` в `worker.py`).
3. **Сшивка блоков по эмбеддингам:** `pipeline._embedding(tensor[(B,1,N)])`
   возвращает **numpy**, вход должен быть на устройстве модели, времена — **внутри
   блока** (до сдвига в абсолютную шкалу). Порог `EMBED_CLUSTER_MIN=0.60`,
   нумерация — по первому появлению.
4. **Silero:** окна группируем, но **не режем регион** Silero насильно (это меняло
   текст: WER 18,01 → 17,82); `transcribe_longform` не используем (gated
   segmentation-3.0 + torchcodec 0.7 не грузится с FFmpeg 9).
5. **torchcodec не полагаться:** аудио — ffmpeg CLI → float32, pyannote получает
   `{"waveform": tensor[1,N], "sample_rate": 16000}`; `pipeline.from_pretrained`
   в 4.0 **не** принимает `local_files_only` — офлайн через `HF_HUB_OFFLINE`.
6. **Gated HF:** без токена работает, если модели в кэше (`model_cached`);
   проверка `hf_token_missing` — до задания, с подсказкой про принятие условий.
7. **Кэш диаризации:** ключ = отпечаток файла + модель + min/max/num + блоки +
   `DIARIZATION_VERSION`; менять алгоритм диаризации — **обязательно** поднимать
   версию, иначе старый `diarization.json` переиспользуется.
8. **WER воспроизводится только на `src_0924.mp3`** (mp3-декод ≠ производный wav);
   сравнение с `spike/asr_mp3_0924_e2e_rnnt_silero_cuda.json`, не с wav-прогоном.
9. **`quality.summarize` считает `max_gap_sec` по всем словам файла** (сейчас 21 с —
   это паузы между чанками, а не разрывы внутри фраз). В P2 это нужно исправить
   до того, как опираться на `alignment_quality`.
10. **Мощности PowerShell:** кириллица в консоли ломается, вывод `>` пишется в
    UTF-16, команды иногда получают посторонние символы. Для замеров — писать в
    лог-файл и читать `Get-Content -Tail`, JSON — с `encoding='utf-16'`/`utf-8`.
11. **Диск:** `target` — junction на `D:\hds-py-transcribe\cargo-target` (на `C:`
    старый target разросся до ~140 ГБ и место кончилось при линковке). Сборки и
    артефакты — только на `D:`; временное — в `tools/parity/out/py_transcribe/`.
12. **`HDS_TRANSCRIBE_PYTHON`** перекрывает поиск venv: в разработке указывать на
    `venv-cuda`, профиль — `--profile cuda` (Rust передаёт из `transcribe.profile`).

---

## 4. Задачи P2 (план §9 P2 + факты P1)

### P2.1 Качество флагов слов — первым делом (измеримо, гейт ≤2 %)

Сейчас на `meet0924`: 2392 слова, **270 low_confidence = 11,3 %**,
`aligned_ratio 0,887`, `max_gap_sec 21,04` (это паузы между чанками).

* починить `quality.summarize`: `max_gap_sec` считать **внутри сегментов/фраз**,
  а не по всему файлу; добавить `low_confidence_ratio`, `failed_ratio`;
* разобрать причины `low_confidence` на реальном `transcript.json` (распределение
  длительностей слов, доля из-за `< min_duration_sec`, доля из-за разрыва
  `> max_gap_sec`); сейчас порог `min_duration_sec = 0.02` помечает короткие
  слова/пунктуационные токены — решить по данным (порог или правило «короткое,
  но с валидными границами — aligned»), **не на глаз**;
* прокинуть `align.quality.*` (`max_gap_sec`, `min_duration_sec`,
  `min_aligned_ratio`) из `config.example.yaml` → `hds_transcribe::Settings` →
  `AlignParams` (сейчас в Rust этих ключей нет, Python берёт дефолты);
* тесты: unit на правило + прогон `meet0924` «до/после»; цель гейта —
  `low_confidence + failed ≤ 2 %` без падения `aligned_ratio`.

### P2.2 Alignment: `whisperx` и `hybrid`

Сейчас `align.mode` кроме `native` даёт warning и нативные границы (`align.py`).
whisperx 3.8.7rc1 есть **только в venv-cuda**.

* `whisperx`: выравнивать **по сегментам ASR** (не пере-распознавать): wav2vec2
  XLSR-53-ru, «произнесённая форма» (числа, «1С», аббревиатуры, латиница), иначе
  CTC-выравнивание «плывёт» на ITN;
* `hybrid`: native базовый, whisperx там, где расхождение `> max_shift_sec`
  (0.35 с) или слово native `low_confidence/failed`; порог — в конфиг и в JSON;
* `display_span` (что показывать) и сквозной `word_id` — в `transcript.json` и в
  блоках результата; `alignment_quality` дополнить `mode_used`, `whisperx_share`,
  `mean_shift_ms`;
* whisperx недоступен (dml/cpu, нет пакета): явный `whisperx` — ошибка
  `align_failed`; `hybrid` — fallback на native с warning (решение владельца §6 п. 1);
* DML-профиль: native остаётся default, whisperx не поддерживается (P0).

### P2.3 VAD: A/B и решение

* сравнить `asr.vad: silero` против `off` (фиксированные окна 25 с) на 18/56/77 мин:
  WER, RTF, «галлюцинации» на тишине, поведение на стыках;
* проверить `max_chunk_sec` 25 vs 30;
* зафиксировать в отчёте, оставлять ли Silero default (ожидаемо — да, P0);
  `transcribe_longform` по-прежнему **не использовать**.

### P2.4 Каналы

* `auto` на реальных файлах даёт `downmix` (r = 0,9998) — уже проверено;
* `split` прогнать на **синтетических** двух дорожках (две реальные записи,
  сведённые в stereo): проверить `speaker = channel`, merge, `.md`; в отчёт — как
  «код-путь проверен на синтетике», **не как приёмку** (реальных двухдорожечных
  записей у владельца нет);
* добавить unit-тест `auto_channel_decision` (дубль-стерео → downmix,
  независимые каналы → split).

### P2.5 (опционально) `rnnt`-параллель

`rnnt` + `difflib`-маппинг к `e2e_rnnt` — только если понадобится для hybrid
(контрольный режим, не боевой путь).

### P2.6 Метрики и отчёт

* расширить `eval_p1.py` → `eval_p2.py`: MAE native vs whisperx (start/end), доля
  слов > 350 мс, флаги, RTF/VRAM по стадиям, спикеры;
* отчёт `tools/parity/out/py_transcribe/P2_REPORT.md` с числами и командами.

---

## 5. Гейт P2 (план §9 P2 + решения P1)

1. **MAE границ ≤ 100 мс** или не хуже `native`; методику указать явно (ручных
   таймкодов в эталоне нет; база сравнения — native, как в P0: MAE 65 мс start /
   167 мс end, 1,75 % слов > 350 мс).
2. **`low_confidence + failed` ≤ 2 %** на `meet0924` без падения `aligned_ratio`.
3. **Атрибуция не падает** (принятая база: 90,68 %); WER ≤ 17,82 %.
4. 77 мин: время ≤ 2× текущего CUDA (сейчас диаризация 149 с + ASR 66 с) и ≤ 3×
   для DML; пик VRAM ≤ 10 ГБ (сейчас 5,8 ГБ).
5. `cargo test --workspace` / `clippy -D warnings` / `fmt` и Python-тесты — зелёные.
6. `engine: legacy` не изменился; контракты `.md`/`.orig.md`/`.speakers.json`/`.bak`
   и UI не тронуты.
7. Кэш `diarization.json` переиспользуется (ключ с версией алгоритма).

---

## 6. Открытые вопросы (спросить владельца, не выдумывать)

1. **WhisperX в поставке.** В `venv-cuda` он есть (P0), в `venv-dml` — нет. Для
   профиля `cpu`/`dml`: явный `align.mode: whisperx` — ошибка `align_failed` или
   тихий fallback на native? (предложение: `hybrid` — fallback с warning, явный
   `whisperx` — ошибка).
2. **`align.mode` по умолчанию** остаётся `native` до результатов P2 (так
   зафиксировано в P1); менять default на `hybrid` — только по гейту MAE.
3. **Порог `low_confidence`** (`min_duration_sec`): уменьшать 0,02 с или менять
   правило? Нужен ли в JSON отдельный флаг «короткое слово» для UI?
4. **Каналы:** достаточно ли синтетики для проверки `split` (приёмка — «реальных
   данных нет»)?
5. **Эталон таймкодов:** для честного MAE нужна ручная разметка 20–30 фраз;
   без неё метрика — только «не хуже native».

---

## 7. Ограничения

* **Не коммитить** без явной просьбы владельца (P1 целиком не закоммичен).
* Не переписывать `PLAN_*.md`; не трогать чужие незакоммиченные правки.
* Артефакты замеров/логов — только `tools/parity/out/py_transcribe/` (gitignored).
* Не менять подтверждённые решения P0/P1 (ASR `e2e_rnnt`, Silero, pyannote
  community-1 с батчами 4/4, hotwords `none`, LLM off, `engine: legacy` работает).
* Не «улучшать» WER подгонкой под эталон: любые изменения ASR/VAD — с A/B.
* Если нужен HF-токен, живая дискретная Radeon, RTTM/ручные таймкоды или решение
  по контракту — **спросить**.

---

## 8. Первые действия нового чата (чек-лист)

```powershell
cd C:\Users\Sasha\hermes-disk-search
git status --short --branch                  # P1 не закоммичен: M/?? файлы P1
git --no-pager log -1 --oneline --decorate   # f202128
Get-Item .\PYTHON_SIDECAR_TRANSCRIBE_P2_HANDOFF_PROMPT.md,
         .\tools\parity\out\py_transcribe\P1_REPORT.md
```

Прочитать: этот файл (§0–§8), `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` §9 P2, §4.3,
§4.5, §16; `P1_REPORT.md`; затем код: `sidecar/hds_transcribe/{align,quality,merge,
worker,asr_gigaam}.py`, `crates/hds-transcribe/src/{settings,params}.rs`,
`crates/hds-index/src/transcribe.rs`.

Затем: прогнать тесты (§2), воспроизвести базовые числа P1 (`rpc_once.py` +
`eval_p1.py`) и **начать с P2.1** (флаги/`alignment_quality`) — от них зависит
остальной гейт.

---

## 9. Промт для нового чата (копировать целиком)

```text
Ты — Cline в режиме Act. Продолжаешь проект hermes-disk-search
(C:\Users\Sasha\hermes-disk-search), ветка python-sidecar-transcribe
(база server-multiuser @ f202128, tag v0.21.0). P0 и P1 завершены, решения владельца
подтверждены. Изменения P1 НЕ закоммичены — не коммить без явной просьбы.
Твоя задача — P2: качество alignment/VAD/флагов слов и каналы.

ПЕРВЫЕ ДЕЙСТВИЯ (не начинай с кода):
1) git status --short --branch и git --no-pager log -1 --oneline --decorate
2) прочитай PYTHON_SIDECAR_TRANSCRIBE_P2_HANDOFF_PROMPT.md целиком
3) прочитай PLAN_PYTHON_SIDECAR_TRANSCRIBE.md §9 P2, §4.3, §4.5, §16
4) прочитай tools/parity/out/py_transcribe/P1_REPORT.md (факты P1)
5) прочитай sidecar/hds_transcribe/{align,quality,merge,worker,asr_gigaam}.py,
   crates/hds-transcribe/src/{settings,params}.rs, crates/hds-index/src/transcribe.rs
6) прогони тесты (Python unittest, cargo test/clippy/fmt), воспроизведи базовые числа
   P1 на meet0924 (src_0924.mp3 + ref_0924.txt) через rpc_once.py + eval_p1.py.
Только после этого пиши код.

ФАКТЫ P1 (для сверки, не пересчитывать):
- meet0924 18 мин: WER 17,82 %, CER 9,52 %, DER 13,00 %, атрибуция по словам 90,68 %
  (принята как шум), ASR 19,6 с RTF 0,0182, пик VRAM 6 749 МБ.
- meet0512 77 мин: ASR 66,1 с (0,0143), диаризация 149 с (0,032), 4 блока 20 мин,
  10 спикеров (P0: 9, принято), пик 5 823 МБ.
- Сейчас на meet0924: 2392 слова, 270 low_confidence (11,3 %), aligned_ratio 0,887,
  max_gap_sec 21,04 (считается по всему файлу — это баг).
- Гейт P2: MAE ≤ 100 мс или не хуже native; low_confidence+failed ≤ 2 %;
  атрибуция не ниже 90,68 %; WER ≤ 17,82 %; 77 мин ≤ 2× текущего CUDA, VRAM ≤ 10 ГБ;
  cargo test/clippy/fmt и Python-тесты зелёные; engine: legacy не тронут.

ПОДТВЕРЖДЁННЫЕ РЕШЕНИЯ (не пересматривать):
- ASR: GigaAM-v3 e2e_rnnt fp16 CUDA (ONNX CPU/DML — для профилей без CUDA);
- Alignment: default native; whisperx/hybrid — режим качества P2;
- VAD: Silero, окна ≤25 с, режем только в паузах; transcribe_longform НЕ использовать;
- Диаризация: pyannote community-1 4.0.0, блоки 15–30 мин, батчи 4/4 после to(device),
  глобальная сшивка по эмбеддингам (EMBED_CLUSTER_MIN=0.60), кэш с версией алгоритма;
- Merge: max-overlap; low_confidence/interpolated/failed не двигают спикера; .md рендерит Rust;
- Hotwords none, LLM off; профили: CUDA основной, CPU для iGPU, DML — fallback.

ЗАДАЧИ P2 (в этом порядке):
P2.1 Флаги качества: починить quality.summarize (max_gap внутри фраз, добавить
     low_confidence_ratio/failed_ratio); разобрать причины 11,3 % low_confidence на
     реальном transcript.json и снизить до ≤2 % по данным, не на глаз; прокинуть
     align.quality.* из config.example.yaml → Rust Settings → AlignParams; unit-тесты
     и отчёт «до/после».
P2.2 Alignment: align.mode whisperx (venv-cuda, whisperx 3.8.7rc1) — выравнивание по
     сегментам ASR, «произнесённая форма» (числа/1С/аббревиатуры); hybrid — native +
     whisperx при расхождении > max_shift_sec; display_span/word_id, mode_used/
     whisperx_share/mean_shift_ms в alignment_quality; при недоступном whisperx:
     явный whisperx → align_failed, hybrid → fallback на native с warning.
P2.3 VAD A/B: silero vs off (фиксированные окна), max_chunk_sec 25 vs 30, на 18/56/77 мин:
     WER, RTF, галлюцинации на тишине, стыки; решение зафиксировать в отчёте.
P2.4 Каналы: split на синтетических двух дорожках (код-путь, НЕ приёмка), unit-тест
     auto_channel_decision (дубль-стерео → downmix, разные каналы → split).
P2.5 (опционально) rnnt+difflib — только если нужен для hybrid.
P2.6 Метрики/отчёт: eval_p2.py (MAE native vs whisperx, флаги, RTF/VRAM по стадиям),
     tools/parity/out/py_transcribe/P2_REPORT.md.

ГРАБЛИ (обязательно):
- pyannote: батчи ≥8 → 9–10 ГБ VRAM даже на 60 с; ставить 4/4 ПОСЛЕ pipeline.to(device);
- max_memory_allocated сбрасывается внутри pyannote — пик мерить _VramSampler;
- эмбеддинги: pipeline._embedding(tensor[B,1,N]) → numpy; времена внутри блока;
- torchcodec 0.7 не грузится с FFmpeg 9 — аудио через ffmpeg CLI, pyannote waveform;
- pipeline.from_pretrained в 4.0 не принимает local_files_only — HF_HUB_OFFLINE; gated-модели
  проверять по кэшу до задания (hf_token_missing);
- кэш диаризации: при смене алгоритма поднимать DIARIZATION_VERSION;
- WER воспроизводится только на src_0924.mp3 (не на производном wav);
- PowerShell: кириллица/UTF-16 в редиректах ломает вывод — писать в лог-файлы;
- target — junction на D:\hds-py-transcribe\cargo-target; сборки только туда (C: переполнялся);
- HDS_TRANSCRIBE_PYTHON → venv-cuda в разработке; HDS_GIGAAM_CACHE/HF_HOME на D:.

ОГРАНИЧЕНИЯ: не коммитить; не переписывать PLAN_*.md; не трогать чужие правки;
артефакты — только tools/parity/out/py_transcribe/; не менять подтверждённые решения;
если нужен HF-токен, RTTM/ручные таймкоды, живая Radeon или решение по контракту — спроси.

Начни с первых действий, затем кратко изложи план P2 (P2.1–P2.6) и приступай к P2.1.
```
