# PYTHON_SIDECAR_TRANSCRIBE_P3_HANDOFF_PROMPT.md — контекст и промт для P3

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать **§9 целиком**
> и отправить первым сообщением. §§0–8 — карта состояния (читать выборочно).
>
> **Дата среза:** 22.10.2026 (после P2). **Ветка:** `python-sidecar-transcribe`
> (база `server-multiuser` @ `f202128`, tag v0.21.0).
> **P1 и P2 завершены, но НЕ закоммичены** — все изменения в рабочей копии.
> **Ключевые файлы:** `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` (план, untracked),
> `PYTHON_SIDECAR_TRANSCRIBE_P2_HANDOFF_PROMPT.md` (контекст P2),
> `tools/parity/out/py_transcribe/P2_REPORT.md` (отчёт P2, gitignored),
> `tools/parity/out/py_transcribe/spike/REPORT.md` (P0, §11 — G0/hotwords).

---

## 0. Статус на 22.10.2026 (P2 закрыт)

**Работает end-to-end** (`transcribe.engine: python`): ASR `e2e_rnnt` fp16 CUDA +
Silero VAD (25 с) + pyannote community-1 (блоки 20 мин, батчи 4/4) + merge +
`.md`/`.orig.md`/`.speakers.json`/`transcript.json`/`diarization.json`;
демон `transcribe-watch`, `transcribe-once`, `transcribe-check`, `hds check`.

**Приёмка P2 (факты, `P2_REPORT.md`):**

| Проверка | Факт |
|---|---|
| `meet0924` 18 мин (mp3 48 кГц stereo, эталон 98 реплик / 7 имён) | WER **17,82 %**, CER **9,52 %**, DER **13,00 %**, атрибуция по словам **90,68 %** — во всех режимах alignment одинаково |
| `low_confidence + failed` (гейт ≤ 2 %) | native **0,46 %** (11 слов), hybrid **0,71 %** (17), whisperx **0,63 %** (15); было 11,29 % (баг «залипания») |
| MAE native↔whisperx | **64,3 мс start / 164,7 мс end**, 1,64 % слов > 350 мс (воспроизводит P0 65/167/1,75 %) |
| `hybrid` | 12,5 % слов из whisperx, `align_sec` 24,7 с на 18 мин, `rtf_total` 0,0406; текст блоков **идентичен** native, блоков 286 → 237 |
| 77 мин hybrid (холодный кэш) | **313,6 с** (ASR 74,8 + align 77,5 + diar 150,9) = 1,46× P1, пик VRAM **5 823 МБ**, 10 спикеров, `low_confidence` 1,06 % |
| VAD A/B | Silero vs off: WER 17,82 vs 18,56 (цена ~1,8× ASR); `max_chunk_sec` 25 vs 30 — в пределах шума ⇒ оставлено 25 |
| Каналы | `auto` → downmix на реальных файлах (r=0,9998); `split` проверен на **синтетике** (L=frag60, R=первая минута meet0514) — код-путь, не приёмка |
| Диагностика | `hds transcribe-check`: секция `alignment: mode=… (пороги)` + «последние транскрипции» (`mode/aligned/low/failed/reasons/rtf_total/vram`), то же в `--json`; в `transcribe-worker.log` — `[stage] done … mode=… low_conf=… reasons=…` |
| Тесты | Python **71** (`unittest`), `cargo test --workspace` / `clippy -D warnings` / `fmt` — зелёные; `engine: legacy` не тронут |

**Решения владельца 22.10.2026 (не пересматривать):**

1. **Default alignment — `native`** (whisperx/hybrid доступны явно; пересмотр — только
   после ручной разметки 20–30 фраз, методика в `P2_REPORT.md` §1/§12.1).
2. **WhisperX только в CUDA-venv.** На `cpu`/`dml`: `hybrid` → native + warning,
   явный `whisperx` → `align_failed`. Данные: CPU `RTF_align 0,235`, RSS 3,25 ГБ.
   Пересмотр — в P5 (упаковка).
3. **UI не трогаем.** Причины флагов живут в `transcript.json`; агрегат виден в
   `transcribe-check` и журнале. Word-level UI — отдельная задача после P5.
4. **`timing_overlap`** — флаг конфликта таймкодов (не перекрытая речь), допуск
   `align.quality.overlap_tolerance_sec = 0,12` с; перекрытая речь — `Word.overlap`.

**Осталось от P2 (не блокирует P3):** word-level UI; ручная разметка таймкодов;
`rnnt`-параллель не понадобилась; упаковка whisperx в venv — P5.

---

## 1. Карта кода (что уже есть)

### 1.1 Python (общий пакет + два процесса, общий CPython, venv по профилю)

```
sidecar/hds_sidecar/          common: stdio, runtime, config, progress, fs
sidecar/hds_extract/worker.py переведён на пакет (поведение не менялось)
sidecar/hds_transcribe/
  worker.py        hello/probe/transcribe/postprocess/cancel/shutdown; _VramSampler;
                   _run_asr (CPU-fallback), _run_diarization, _align_accumulator,
                   _params_snapshot, RPC_BY_KIND (align_failed = -32020)
  params.py        TranscribeParams (asr/decode/align/diarization), AlignQuality,
                   PostprocessParams (glossary.path/terms, llm.*)
  decode.py        ffprobe/ffmpeg; channels auto|downmix|split; window_correlations
  asr_gigaam.py    e2e_rnnt fp16/CPU; speech_chunks (Silero, режем только в паузах); Word
  asr_onnx.py      EP cuda|dml|cpu; ORT_SEQUENTIAL + mem_pattern=False
  diarize_pyannote.py  safe_globals; waveform; блоки 15–30 мин; эмбеддинги+union-find; кэш
  spoken.py        «произнесённая форма»: числа, «1С»→«один эс», «МЧД»→«эм че дэ»,
                   латиница/словарь, символы (P2.2)
  align.py         native | whisperx | hybrid; map_segment (difflib по токенам);
                   guard от вырожденных span'ов; apply() → (segments, warnings, info)
  merge.py         чистые функции (max-overlap, ambiguous/overlap, блоки, join_words)
  quality.py       classify(); флаги aligned/interpolated/low_confidence/failed +
                   alignment_reason (short_duration/timing_overlap/gap/empty/
                   no_timestamps/non_monotonic); display_span; summarize(phrases)
  cache.py         transcript.json/diarization.json; DIARIZATION_VERSION=8
  models.py        gigaam cache/onnx/hf token/offline; gated-проверка
sidecar/tests/     71 тест (params, merge, quality, align, spoken, блоки, каналы, протокол)
```

**Контракты:** JSON-RPC 2.0 NDJSON, `PROTOCOL=1`, stdout только протокол, прогресс
в stderr (`[stage] …`), ошибки `{code, message, hint, kind}`; параметры приходят
**из Rust** (Python `config.yaml` не читает).

**Порядок конвейера** (`worker.transcribe`): `decode.probe` → `plan_channels` →
`_run_asr` → `align.apply` → `_words_from_segments` (quality) → `_run_diarization`
(кэш) → `merge.merge_transcript` / `_single_block` → `quality.summarize(phrases=blocks)`
→ `transcript.json` → ответ; `stats` содержит `decode/asr/align/diar/elapsed`,
`rtf` (ASR+diar, как в P1) и `rtf_total` (+align).

**`postprocess` — сейчас passthrough** (`worker.py`, ~стр. 650): читает
`transcript.json`, отдаёт `text`/`blocks` без изменений, `edits: []`, и пишет
warning про глоссарий/LLM. **Это и есть точка входа P3** — повторный вызов не
запускает ASR (смысл отдельного метода).

### 1.2 Rust

```
crates/hds-extract/src/worker.rs   Worker::request(method, params, retry_dead),
                                   discover_python_with(env_var, venv_rel), last_error()
crates/hds-transcribe/             params.rs (TranscribeParams, AlignParams, AlignQuality,
                                   AsrParams, DecodeParams, DiarizationParams,
                                   PostprocessParams{glossary{path,terms}, llm}, Word, Block),
                                   settings.rs (transcribe.* → параметры, notices),
                                   client.rs (spawn/probe/transcribe/postprocess),
                                   error.rs (ErrorKind, align_failed)
crates/hds-index/src/transcribe.rs engine python|legacy; render_markdown;
                                   python_transcribe_to_file: transcribe → **postprocess**
                                   (сейчас `glossary: Default::default()`) → `.md`;
                                   `.orig.md` = текст ПОСЛЕ postprocess, ДО имён
crates/hds-index/src/diag.rs       transcribe_summary + check_transcribe в run_checks
crates/hds-cli/src/cmd/transcribe_check.rs  transcribe-check (alias whisper-check);
                                   секции `alignment` и «последние транскрипции»
                                   (TranscriptSummary/summarize_transcript/latest_transcripts)
crates/hds-cli/src/cmd/mod.rs      регистрация команды; main.rs — argv
config.example.yaml                transcribe.* (asr/decode/align/diarization/cache/
                                   hotwords{backend: none}/llm); glossary — НЕТ (P3)
```

`Settings` маппит старые ключи (`auto_transcribe.diarization*`, `index.whisper_*`,
`diarization.*`) и **пишет notice**, если `transcribe.hotwords.backend != none`
(«G1/G2 не пройдены — biasing выключен») или `align.mode` неизвестен. При
реализации P3 эти notice нужно снять/заменить по факту ворот.

### 1.3 Что уже готово под P3

* `PostprocessParams` (Rust и Python) уже содержит `glossary.path` + `terms` (list),
  `llm.*` — расширять контейнер почти не нужно, только код замен и проброс из конфига;
* `transcript.json` хранит `blocks[].words[]` с `word_id/text/start/end/alignment/
  alignment_reason/display_span/speakers/overlap` — замены можно делать **по словам**,
  не ломая таймкоды и спикеров; текст блока пересобирается `merge.join_words`;
* `eval_reference.py` (в `tools/parity/out/py_transcribe/scripts/`) уже считает
  WER/CER/DER/атрибуцию по эталону `ref_0924.txt` — база для G1/G2;
* `gigastt` окружение P0 живо (см. §2.4);
* `.md` рендерит Rust: `.orig.md` — «после postprocess, до имён», `.bak` — имена UI.

### 1.4 Конфиг: чего ещё нет (P3 должен добавить)

По плану §6: `transcribe.hotwords.{enabled,backend,phrases,max_phrases,
max_phrase_chars,boost,url}` и `transcribe.glossary.path` — **в `config.example.yaml`
сейчас только `hotwords.backend: none`**, `glossary` отсутствует; в `Settings`
этих ключей нет. Формат глоссария — план §4.2 (см. §4 ниже).

---

## 2. Тесты, сборка, окружение

```powershell
# Python (pytest в venv-cuda НЕ установлен; тесты pytest-совместимы)
& .\tools\parity\out\py_transcribe\venv-cuda\Scripts\python.exe `
  -m unittest discover -s sidecar/tests -t sidecar        # 71 тест

# Rust (target — junction на D:\hds-py-transcribe\cargo-target)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

**Переменные для живых прогонов:**

```powershell
$e = 'tools\parity\out\py_transcribe'
$env:HDS_TRANSCRIBE_PYTHON = (Resolve-Path "$e\venv-cuda\Scripts\python.exe").Path
$env:HDS_GIGAAM_CACHE = 'D:\hds-py-transcribe\gigaam-cache'
$env:HF_HOME = 'D:\hds-py-transcribe\hf'
$env:PYANNOTE_METRICS_ENABLED = '0'
```

**Окружения (готовы, не пересобирать без нужды):**

```
tools/parity/out/py_transcribe/venv-cuda -> D:\hds-py-transcribe\venv-cuda
venv-cuda: torch 2.8.0+cu128, transformers 4.57.1, pyannote 4.0.0,
           whisperx 3.8.7rc1, onnxruntime 1.23.2, silero-vad 6.2.3,
           gigaam 0.2.0 @ 7447938d, jiwer 4.0.0, psutil 7.2.2
tools/parity/out/py_transcribe/venv-dml  -> D:\hds-py-transcribe\venv-dml
venv-dml : torch 2.8.0+cpu, onnxruntime-directml 1.24.4, БЕЗ onnxruntime/whisperx
tools/parity/out/py_transcribe/venv-gigastt -> D:\hds-py-transcribe\venv-gigastt
           (без pip; для G0 не нужен — использовался CLI)
```

**2.4 gigastt (нужен для G1/G2/G3):** версия 2.22.0, бинарник
`D:\hds-py-transcribe\gigastt-cli\bin\gigastt.exe`, INT8-модели
`D:\hds-py-transcribe\gigastt-models` (`v3_e2e_rnnt_encoder_int8.onnx`,
`decoder/joint/vocab`, `wespeaker_resnet34.onnx`, `optimized_cache`).
Команды: `transcribe`, `transcribe-batch`, `serve`, `download`, `watch`.
Флаги `transcribe`: `--model-dir`, `--model-variant e2e_rnnt`, `--punctuation
auto|on|off`, `--itn`, `--hotwords-file` (одна фраза на строку, опционально
`\t<weight>`, `#` — комментарий), `--hotwords-boost` (default 5.0),
`--hotwords-default`, `--vad`, `--encoder-intra-threads`, `--file-window-concurrency`,
`--format json --output <file>`. **Python-биндинг hotwords не имеет** — только CLI/HTTP.

**Корпус и артефакты:**

* эталон: `D:\hds-py-transcribe\work\src_0924.mp3` (18 мин, 48 кГц stereo),
  `ref_0924.txt` (98 реплик, 7 имён);
* `D:\diar-spike-data\meet0924.wav`, `meet0514.wav` (56 мин), `meet0512.wav` (77 мин), `frag60.wav`;
* G0-клипы: `D:\hds-py-transcribe\work\g0_clip.wav` (24,5 с, «1С»),
  `g0_erp.wav` (25 с, «RP-система»); hotword-файлы `hw_1c.txt`, `hw_mchd.txt`
  (**UTF-8 без BOM** — иначе gigastt молча отбрасывает фразы);
* скрипты замеров (`tools/parity/out/py_transcribe/`): `rpc_once.py`,
  `eval_p1.py`/`eval_p2.py`, `align_probe_p21.py`, `vad_ab_p21.py`,
  `analyze_p21.py`, `analyze_gaps_p21.py`, `dump_asr_p21.py`, `diff_p21.py`,
  `overlap_report.py`, `overlap_context.py`, `tail_words.py`, `align_cpu_probe.py`,
  `run_g0.ps1` (P0), `scripts/eval_reference.py`;
* параметры: `params_meet0924.json`, `params_meet0924_hybrid.json`,
  `params_meet0924_whisperx.json`, `params_meet0512*.json`, `params_frag60*.json`,
  `params_p24_split.json`; конфиги: `p1_e2e/`, `p2_e2e/`, `p2_diag/`;
* артефакты P2: `p1_meet/`, `p21_before/`, `p22_hybrid/`, `p22_whisperx/`,
  `p23_77/`, `p24_split/`, `P2_EVAL.json`, `p22_align.json`, `p23_vad_*.json`.

---

## 3. Грабли (обязательно учитывать в P3)

1. **pyannote community-1: батчи ≥ 8 = ~9–10 ГБ VRAM транзиентно даже на 60 с.**
   В `diarize_pyannote.load()` выставляются 4/4 **после** `pipeline.to(device)`.
2. **`torch.cuda.max_memory_allocated()` сбрасывается внутри pyannote** — пик
   мерить своим сэмплером (`_VramSampler` в `worker.py`).
3. **Сшивка блоков по эмбеддингам:** `pipeline._embedding(tensor[(B,1,N)])`
   возвращает numpy, вход на устройстве модели, времена — **внутри блока**;
   `EMBED_CLUSTER_MIN = 0.60`; при смене алгоритма диаризации поднимать
   `DIARIZATION_VERSION` (сейчас 8), иначе старый `diarization.json` переиспользуется.
4. **Silero:** окна группируем, но **не режем регион** Silero насильно (это меняло
   текст: WER 18,01 → 17,82); `transcribe_longform` не используем (gated
   segmentation-3.0 + torchcodec 0.7 не грузится с FFmpeg 9).
5. **torchcodec не полагаться:** аудио — ffmpeg CLI → float32; pyannote получает
   `{"waveform": tensor[1,N], "sample_rate": 16000}`; `pipeline.from_pretrained` в
   4.0 **не** принимает `local_files_only` — офлайн через `HF_HUB_OFFLINE`.
6. **Gated HF:** без токена работает, если модели в кэше (`model_cached`);
   проверка `hf_token_missing` — до задания.
7. **WER воспроизводится только на `src_0924.mp3`** (mp3-декод ≠ производный wav);
   сравнение — с `spike/asr_mp3_0924_e2e_rnnt_silero_cuda.json`.
8. **`low_confidence` не «залипает»** (фикс P2): `prev_end` обновляется для любого
   слова с конечным `end`. Причина `timing_overlap` — конфликт таймкодов, **не**
   перекрытая речь (это `Word.overlap`); допуск 0,12 с.
9. **WhisperX только в CUDA-venv**, `whisperx` не отдаёт `__version__` (версия —
   из метаданных), модель `jonatasgrosman/wav2vec2-large-xlsr-53-russian` в
   `HF_HOME`; вырожденные span'ы (<20 мс) не переносим, таймкод копируется только
   при полном совпадении токенов (`difflib`), наследования по позиции нет.
10. **PowerShell:** кириллица в консоли ломается (это только отображение);
    `>` и `*>` пишут **UTF-16** — читать в Python с `encoding='utf-16'`
    (и в `Get-Content -Encoding utf8` — с потерей, лучше `utf-16`); многострочный
    `python -c` в PS ломается — писать скрипт-файл; длинные прогоны — в лог-файл.
11. **`rpc_once.py` буферизует stderr воркера до конца задания** — прогресс не
    виден; для 77-мин прогонов запускать `Start-Process` + опрос лога, помня, что
    лог появится только в конце.
12. **Диск:** `target` — junction на `D:\hds-py-transcribe\cargo-target`; сборки и
    артефакты только на `D:`; временное — в `tools/parity/out/py_transcribe/`.
13. **`HDS_TRANSCRIBE_PYTHON`** перекрывает поиск venv; профиль — `--profile cuda`.
14. **Rust:** `hds_core::config::Config = serde_yaml::Value` (тесты —
    `serde_yaml::from_str`); serde **игнорирует неизвестные поля** (новые поля
    JSON безопасны); после правок — `cargo fmt`, clippy гоняет `sort_by_key`.
15. **gigastt:** Python-биндинг `gigastt_uniffi` **не имеет hotwords** (только
    CLI/HTTP); `ort` в gigastt несовместим с воркспейсом (rc.13 vs rc.15) —
    **не встраивать**, только отдельный процесс/CLI; hotword-файл **UTF-8 без BOM**
    (иначе фразы молча отбрасываются: «not representable in the active vocab»);
    biasing меняет текст только там, где акустика уже допускает токены.
16. **`hds transcribe-once` не перезаписывает `.md`**: повторный прогон даёт
    `frag60-2.md`; для чистых замеров — отдельные `state_dir`/`out_dir`.
17. **`hds transcribe-check`** читает до 3 последних `*.transcript.json` из
    `transcribe.state_dir` — для проверки агрегата нужен каталог с артефактами.

---

## 4. Задачи P3 (план §9 P3 + §4.2)

### P3.1 Глоссарий: формат и загрузка

`data/glossary.yaml` (план §4.2):

```yaml
terms:
  - canonical: "1С"
    variants: ["один эс", "индинес", "индес", "1 эс", "1c"]
    type: product            # product | org | person | abbreviation | domain
    context: ["бухгалтерия", "ERP", "документооборот"]
    priority: high           # high | normal | low
    case_sensitive: false
    allow_global: false      # «один эс» НЕ заменять без контекста
    pronunciations: ["один эс"]
forbid: []                   # фразы, запрещённые как replacements вообще
```

* модуль `sidecar/hds_transcribe/glossary.py`: загрузка/валидация (неизвестные
  ключи — warning, не падение), нормализация для сопоставления (регистр, `ё/е`,
  латиница/кириллица `1C/1С`, границы слов, пунктуация), поиск кандидатов в
  пределах окна ±N слов (N — константа, документировать), `priority`,
  `allow_global`, `forbid`, `case_sensitive`;
* `variants` — **кандидаты**, а не гарантия: замена только если рядом есть слово
  из `context` **или** `priority: high` и нет конкурирующего термина; иначе
  `needs_review` (текст сохраняется);
* канон сохраняется как в файле (регистр/написание).

### P3.2 Детерминированные замены в `postprocess`

* применять **по словам** (`blocks[].words[]`): менять `word.text`, затем
  пересобирать `block.text` через `merge.join_words` — таймкоды, спикеры,
  `alignment`, `word_id` **не трогать**; иначе `.md` и `transcript.json` разъедутся;
* `edits.json` (в `state_dir` рядом с `transcript.json`): `word_id`, `before`,
  `after`, `term`, `reason` (`context`/`priority`/`allow_global`), `needs_review`;
* повторный `postprocess` идемпотентен; ASR не запускается; `hotwords.backend:
  none` и пустой глоссарий → результат **идентичен** P2 (байт-в-байт по тексту).

### P3.3 G1 — INT8 gigastt vs официальный fp16

Тот же корпус, `e2e_rnnt`, без hotwords: `gigastt transcribe --model-dir
D:\hds-py-transcribe\gigastt-models --model-variant e2e_rnnt --punctuation auto
--itn auto --format json --output ...`; эталон — `ref_0924.txt` (18 мин).
Гейт: WER **≤ +0,3 п.п.** к fp16 (17,82 %), recall сущностей не ниже.
Артефакты — `tools/parity/out/py_transcribe/p3/g1_*.json`.

### P3.4 G2 — польза hotwords

Тот же gigastt, `--hotwords-file` (словарь встречи из `pronunciations`/`canonical`
глоссария, ≤ 64 фраз × ≤ 64 символов, UTF-8 **без BOM**) vs без файла,
`--hotwords-boost` 5/8/12. Метрики: recall сущностей (**≥ +10 % относительного**),
ложные вставки (**≤ 1 %**), WER (**не растёт > 0,3 п.п.**). Список сущностей и
их эталонные вхождения — из `ref_0924.txt` (+ подтвердить у владельца, §6).

### P3.5 G3 (опционально) — повторное декодирование окон

Если G0 пройден, а G1 — нет: основной путь остаётся официальный fp16, а gigastt
с hotwords зовём только на короткие подозрительные участки (4–12 с, контекст 1 с):
триггер — нечёткое совпадение с `variants` (edit distance ≤ 2) или
`low_confidence` рядом с термином; принимаем замену только если в окне появился
`canonical`, соседние слова совпали (`SequenceMatcher ≥ 0,8`), таймкоды монотонны
и не выходят за окно; всё — в `edits.json`. Критерий: рост recall без роста WER и
ложных вставок на полном файле.

### P3.6 Проводка конфига и валидация

* Rust `Settings`: `transcribe.glossary.path` (+ `terms` для тестов),
  `transcribe.hotwords.{enabled,backend,phrases,max_phrases,max_phrase_chars,boost,url}`;
  проброс в `PostprocessParams` (сейчас `glossary: Default::default()` в
  `hds-index::transcribe`);
* Python `postprocess`: лимиты 64/64 проверять **до** вызова gigastt →
  `bad_request` с hint (`too_many_hotwords` / `hotword_phrase_too_long`);
* `hello.capabilities`: `hotwords:gigastt` только если backend сконфигурирован и
  бинарник/сервер доступен; `transcribe-check` — показывать путь глоссария и
  состояние hotwords (backend, число фраз);
* снять/заменить notice `Settings` «hotwords.backend=%s не поддержан» по факту
  ворот (если G1/G2 не пройдены — notice остаётся, но с честной причиной).

### P3.7 Тесты

Python: нормализация/сопоставление (регистр, `ё/е`, `1C/1С`, границы слов),
`context`/`priority`/`allow_global`/`forbid`/`needs_review`, лимиты 64/64,
идемпотентность `postprocess`, «`hotwords.backend: none` → поведение P2 без
изменений», пустой глоссарий → `.md` не меняется. Rust: чтение новых ключей
`Settings`, проброс в params, 2–3 unit-теста. Golden `.md` (`transcribe_markdown`)
не должен меняться.

### P3.8 Отчёт

`tools/parity/out/py_transcribe/P3_REPORT.md`: числа G1/G2/G3 (WER, recall,
ложные вставки, RTF), решения по `hotwords.backend`, формат `edits.json`,
команды воспроизведения и что осталось (упаковка gigastt — P5).

---

## 5. Гейт P3 (план §9 P3)

1. **Precision сущностей не ниже baseline**, ложные вставки **≤ 1 %**;
   WER не растёт > 0,3 п.п. при включённом глоссарии/hotwords.
2. При `hotwords.backend: none` (default) поведение **идентично P2**;
   пустой/отсутствующий глоссарий — `.md` не меняется.
3. Если G1/G2 не пройдены — `hotwords.backend: none` **зафиксировано в отчёте**,
   без «включим на глаз»; текстовая замена не выдаётся за biasing.
4. `postprocess` не запускает ASR (повторный вызов — секунды), идемпотентен,
   `edits.json` валиден.
5. `cargo test --workspace` / `clippy -D warnings` / `fmt` и Python-тесты —
   зелёные; `engine: legacy` не изменён; контракты `.md`/`.orig.md`/
   `.speakers.json` и UI не тронуты.

---

## 6. Открытые вопросы (спросить владельца, не выдумывать)

1. **Эталон для G1/G2.** Есть только `ref_0924.txt` (18 мин, 98 реплик). План
   требует 1–2 ч корпуса: достаточно ли 18 мин для вывода «INT8 не хуже fp16»
   (гейт 0,3 п.п.), или нужны эталонные тексты для 56/77 мин?
2. **Список сущностей для G2.** 1С, МЧД, ЗУП, названия продуктов, 7 имён из
   эталона — полный ли это список? Нужны ли приоритеты и «запрещённые» варианты?
3. **Форма gigastt в P3.** CLI (`transcribe --hotwords-file`) или WS-сервер
   (`serve` + `hotwords`/`hotwords_boost`, лимиты 64/64)? Для замеров предлагаю
   CLI; в бой — только при G0+G1+G2, отдельным процессом.
4. **Где живёт глоссарий.** `data/glossary.yaml` в репозитории (пример
   `data/glossary.example.yaml`?) или пользовательский файл в `state_dir`?
   Коммитить ли реальный глоссарий (в нём могут быть ФИО клиентов)?
5. **Слой замен.** Предложение: менять `word.text` + `block.text` (через
   `join_words`), таймкоды/спикеры не трогать. ОК? Альтернатива — отдельный
   `display_text` без изменения `.md` (тогда UI/потребители должны его читать).
6. **`needs_review`.** Только в `edits.json`/отчёте или ещё в `transcript.json`
   (для будущего UI)?

---

## 7. Ограничения

* **Не коммитить** без явной просьбы владельца (P1 и P2 не закоммичены).
* Не переписывать `PLAN_*.md`; не трогать чужие незакоммиченные правки.
* Артефакты замеров/логов — только `tools/parity/out/py_transcribe/` (gitignored).
* Не менять подтверждённые решения P0–P2: ASR `e2e_rnnt` fp16 CUDA, Silero/25 с,
  pyannote community-1 4/4, `align.mode: native` default, пороги качества
  (gap 1,5 / dur 0,02 / overlap 0,12 / unreliable 2 %), hotwords `none` до ворот,
  LLM off, `engine: legacy` работает.
* Не «улучшать» WER подгонкой под эталон: любые изменения ASR/глоссария — с A/B.
* Если нужен HF-токен, эталонные тексты, RTTM/ручные таймкоды, живая Radeon или
  решение по контракту — **спросить**.

---

## 8. Первые действия нового чата (чек-лист)

```powershell
cd C:\Users\Sasha\hermes-disk-search
git status --short --branch                  # P1+P2 не закоммичены: M/?? файлы
git --no-pager log -1 --oneline --decorate   # f202128
Get-Item .\PYTHON_SIDECAR_TRANSCRIBE_P3_HANDOFF_PROMPT.md,
         .\tools\parity\out\py_transcribe\P2_REPORT.md
```

Прочитать: этот файл (§0–§8), `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` §4.2, §9 P3, §6;
`P2_REPORT.md` §11–§12; код: `sidecar/hds_transcribe/worker.py` (`postprocess`,
`transcribe`), `params.py` (`PostprocessParams`), `merge.py` (`join_words`),
`quality.py`; Rust: `crates/hds-transcribe/src/settings.rs` (notices),
`crates/hds-index/src/transcribe.rs` (вызов postprocess, `.orig.md`),
`crates/hds-cli/src/cmd/transcribe_check.rs`, `config.example.yaml`.

Затем: прогнать тесты (§2), **проверить, что gigastt жив** (`gigastt.exe --help`,
`transcribe --help`, повторить G0 через `run_g0.ps1` на `g0_clip.wav`/`g0_erp.wav`,
hotword-файл UTF-8 без BOM), воспроизвести базовые числа P2 (`rpc_once.py` +
`eval_p1.py`) — они база для G1. Только после этого писать код, начиная с **P3.1**
(глоссарий).

---

## 9. Промт для нового чата (копировать целиком)

```text
Ты — Cline в режиме Act. Продолжаешь проект hermes-disk-search
(C:\Users\Sasha\hermes-disk-search), ветка python-sidecar-transcribe
(база server-multiuser @ f202128, tag v0.21.0). P0, P1 и P2 завершены, решения
владельца подтверждены. Изменения P1 и P2 НЕ закоммичены — не коммить без явной
просьбы. Твоя задача — P3: глоссарий (детерминированные замены) и ворота hotwords
G1/G2 (+G3 опционально).

ПЕРВЫЕ ДЕЙСТВИЯ (не начинай с кода):
1) git status --short --branch и git --no-pager log -1 --oneline --decorate
2) прочитай PYTHON_SIDECAR_TRANSCRIBE_P3_HANDOFF_PROMPT.md целиком
3) прочитай PLAN_PYTHON_SIDECAR_TRANSCRIBE.md §4.2, §9 P3, §6
4) прочитай tools/parity/out/py_transcribe/P2_REPORT.md (§11 решения, §12 факты)
5) прочитай sidecar/hds_transcribe/{worker,params,merge,quality}.py,
   crates/hds-transcribe/src/{settings,params}.rs,
   crates/hds-index/src/transcribe.rs, crates/hds-cli/src/cmd/transcribe_check.rs,
   config.example.yaml
6) прогони тесты (Python unittest — 71 тест; cargo test/clippy/fmt) и воспроизведи
   базовые числа P2 на meet0924 (src_0924.mp3 + ref_0924.txt) через rpc_once.py +
   eval_p1.py: WER 17,82 %, CER 9,52 %, DER 13,00 %, атрибуция 90,68 %,
   low_confidence 11 (0,46 %), блоки 286, RTF 0,0189
7) проверь, что gigastt жив: D:\hds-py-transcribe\gigastt-cli\bin\gigastt.exe
   transcribe --help; повтори G0 (run_g0.ps1: g0_clip.wav/g0_erp.wav, hotword-файл
   UTF-8 БЕЗ BOM) и зафиксируй, что «RP-система» → «ERP-система» при boost 8
Только после этого пиши код.

ФАКТЫ P0–P2 (для сверки, не пересчитывать):
- meet0924 18 мин: WER 17,82 %, CER 9,52 %, DER 13,00 %, атрибуция по словам 90,68 %;
  ASR 19–21 с (RTF ~0,019), диаризация ~34 с, пик VRAM 6 739–6 749 МБ.
- meet0512 77 мин: ASR ~75 с, диаризация ~151 с, 4 блока, 10 спикеров, пик 5 823 МБ;
  hybrid: +align ~76 с (итого 313,6 с холодный прогон = 1,46× P1).
- low_confidence+failed: native 0,46 %, hybrid 0,71 %, whisperx 0,63 % (гейт ≤2 %).
- G0 (P0): biasing у gigastt РАБОТАЕТ («RP-система» → «ERP-система», boost 8), но
  меняет текст только где акустика допускает токены; G1/G2 НЕ проверялись.
- gigastt 2.22.0: CLI D:\hds-py-transcribe\gigastt-cli\bin\gigastt.exe, INT8-модели
  D:\hds-py-transcribe\gigastt-models; hotwords только CLI/HTTP (в Python-биндинге их
  нет); лимиты 64 фразы × 64 символа; hotword-файл UTF-8 без BOM.

ПОДТВЕРЖДЁННЫЕ РЕШЕНИЯ (не пересматривать):
- ASR: GigaAM-v3 e2e_rnnt fp16 CUDA (ONNX CPU/DML — для профилей без CUDA);
- VAD: Silero, окна ≤25 с, режем только в паузах; transcribe_longform НЕ использовать;
- Alignment: default native; whisperx — только CUDA-venv, hybrid деградирует в native
  с warning на cpu/dml; пороги качества: gap 1,5 / dur 0,02 / timing_overlap 0,12 /
  unreliable 2 %; default на hybrid НЕ переключаем;
- Диаризация: pyannote community-1 4.0.0, батчи 4/4 после to(device), блоки 20 мин,
  сшивка по эмбеддингам (0,60), кэш с DIARIZATION_VERSION=8;
- Merge: max-overlap; флаги не двигают спикера; .md рендерит Rust;
- Hotwords: backend none до ворот; LLM off (P4); профили: CUDA основной, CPU для iGPU,
  DML — fallback;
- UI не трогаем; причины флагов — в transcript.json + агрегат в transcribe-check/журнале.

ЗАДАЧИ P3 (в этом порядке):
P3.1 Глоссарий: glossary.py — загрузка/валидация data/glossary.yaml (формат §4.2:
     canonical/variants/type/context/priority/case_sensitive/allow_global/
     pronunciations/forbid), нормализация (регистр, ё/е, 1C/1С, границы слов),
     окно контекста ±N слов, needs_review вместо безусловной замены.
P3.2 Детерминированные замены в postprocess: по словам (word.text) + пересборка
     block.text через merge.join_words; таймкоды/спикеры/alignment не трогать;
     edits.json (word_id/before/after/term/reason/needs_review); идемпотентность;
     пустой глоссарий и hotwords=none → поведение P2 без изменений.
P3.3 G1: gigastt INT8 e2e_rnnt (без hotwords) vs официальный fp16 на ref_0924:
     WER ≤ +0,3 п.п., recall сущностей не ниже; артефакты p3/g1_*.json.
P3.4 G2: gigastt + hotwords-file (≤64×64, UTF-8 без BOM) vs без, boost 5/8/12:
     recall сущностей ≥ +10 % отн., ложные вставки ≤ 1 %, WER +≤0,3 п.п.
P3.5 G3 (опционально): повторное декодирование окон 4–12 с по триггеру (edit distance
     ≤2 к variants или low_confidence рядом); принимать только canonical + соседи
     ≥0,8 + монотонные таймкоды; всё в edits.json.
P3.6 Проводка: transcribe.glossary.path/terms и transcribe.hotwords.* в Rust Settings →
     PostprocessParams; лимиты 64/64 → bad_request с hint; capabilities
     hotwords:gigastt; transcribe-check показывает глоссарий/бэкенд; notice в Settings
     обновить по факту ворот.
P3.7 Тесты: нормализация/контекст/priority/allow_global/forbid/limits/идемпотентность/
     «none → как P2»; Rust — новые ключи Settings + params; golden .md не меняется.
P3.8 Отчёт tools/parity/out/py_transcribe/P3_REPORT.md: числа G1–G3, решение по
     hotwords.backend, формат edits.json, команды.

ГЕЙТ P3: precision сущностей не ниже baseline; ложные вставки ≤1 %; WER не растёт
>0,3 п.п.; hotwords=none → идентично P2; postprocess не запускает ASR и идемпотентен;
cargo test/clippy/fmt и Python-тесты зелёные; engine: legacy и контракты .md/.orig.md/
.speakers.json/UI не тронуты. Если G1/G2 не пройдены — hotwords.backend: none
фиксируется в отчёте, без «включим на глаз».

ГРАБЛИ (обязательно):
- pyannote: батчи 4/4 после to(device); пик мерить _VramSampler; при смене алгоритма
  диаризации поднимать DIARIZATION_VERSION;
- WER воспроизводится только на src_0924.mp3; сравнение G1 — с fp16-прогоном P2;
- gigastt: hotwords только CLI/HTTP; hotword-файл UTF-8 без BOM (иначе фразы молча
  отбрасываются); не встраивать gigastt-core в Rust (ort rc.13 vs rc.15);
- PowerShell: `>`/`*>` пишут UTF-16 (читать encoding='utf-16'); многострочный
  python -c ломается — скрипт-файлы; rpc_once.py буферизует stderr до конца задания;
  длинные прогоны — Start-Process + опрос лога;
- target — junction на D:\hds-py-transcribe\cargo-target; сборки только туда;
- HDS_TRANSCRIBE_PYTHON → venv-cuda; HDS_GIGAAM_CACHE/HF_HOME на D:;
- Rust: Config = serde_yaml::Value (тесты serde_yaml::from_str); неизвестные поля JSON
  игнорируются; после правок cargo fmt, clippy.

ОГРАНИЧЕНИЯ: не коммитить; не переписывать PLAN_*.md; не трогать чужие правки;
артефакты — только tools/parity/out/py_transcribe/; не менять решения P0–P2; если нужен
HF-токен, эталонные тексты (для G1/G2), RTTM/ручные таймкоды, живая Radeon или решение
по контракту (слой замен/путь глоссария) — спроси.

Начни с первых действий, затем кратко изложи план P3 (P3.1–P3.8) и приступай к P3.1.
```


