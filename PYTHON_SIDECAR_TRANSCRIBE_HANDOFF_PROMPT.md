# PYTHON_SIDECAR_TRANSCRIBE_HANDOFF_PROMPT.md — контекст и промт для нового чата

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать §7 целиком
> (он самодостаточен) и отправить первым сообщением. Этот документ — «карта»
> состояния: его можно читать целиком, но промт в §7 уже содержит главное.
>
> **Дата среза:** 22.10.2026, 13:05. **Ветка:** `python-sidecar-transcribe`
> (база — `server-multiuser` @ `f202128`). **Код ещё не менялся.**
> План `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` — **untracked** (`??` в git status),
> 1423 строки, ~133 КБ. Рабочее дерево кроме него чистое.

---

## 0. Что происходит в проекте

`hermes-disk-search` — локальный поиск по файлам + автотранскрибация медиа.
Сейчас транскрибация/диаризация живут в Rust/движке (`whisper.cpp` +
Nemotron-3/Sortformer, вызовы `llm_host /internal/transcribe`). Решено перенести
эту часть в **Python-sidecar** на:

* **GigaAM-v3 `e2e_rnnt`** — ASR (русский, пунктуация/нормализация);
* **WhisperX + wav2vec2-large-xlsr-53-russian** — forced alignment (режим качества);
* **pyannote community-1** — диаризация (exclusive + regular);
* **word-level merge** — сшивка слов и спикеров;
* **глоссарий + LLM (`llm_host`)** — пост-коррекция терминов.

Цель — не сломать существующие контракты (`.md`, UI, демон, индексацию), получить
качество не хуже текущего и поддержать **AMD Radeon под Windows через DirectML**
(без обязательного Vulkan для ASR/диаризации).

## 1. Обязательное чтение (в этом порядке)

1. `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` — **весь**, особенно §0, §2, §4.2, §4.7,
   §5.1–5.4, §6, §9, §10, §16.
2. `PLAN_AUTO_TRANSCRIBE.md` §5 — контракты `.md`, `.orig.md`, `.speakers.json`, UI.
3. `PLAN_DIARIZATION_NEMOTRON.md` — что было и почему уходим (не переписывать).
4. `sidecar/README.md`, `crates/hds-extract/src/{worker.rs,protocol.rs}` — общий
   Python-воркер и stdio JSON-RPC.
5. `crates/hds-index/src/{transcribe.rs,autotranscribe.rs}` — текущий клиент/демон.
6. Отчёты-ревью (локально):
   * `C:\Users\Sasha\Downloads\EXAMINATION_OF_CHOOSEN_STACK.txt` (Perplexity);
   * `C:\Users\Sasha\Downloads\EXAMINATION_OF_CHOOSEN_STACK_CHATGPT.md` (ChatGPT).

## 2. Уже принятые решения (не пересматривать без новых данных)

| # | Что | Решение |
|---|---|---|
| 1 | ASR | GigaAM-v3 `e2e_rnnt`; CUDA — PyTorch fp16; AMD/Intel — **ONNX + `onnxruntime-directml`**; CPU — ONNX/PyTorch. Контрольные варианты `rnnt`/`e2e_ctc` — только в P0 |
| 2 | Hotwords | **Не baseline.** Biasing — свойство декодера `gigastt`, не официального GigaAM. Ворота **G0–G3** (§4.2): G0 — реальный эффект, G1 — INT8 не хуже fp16, G2 — польза, G3 — повторное декодирование окон. По умолчанию `backend: none` |
| 3 | Alignment | Сначала **native word-timestamps GigaAM**; WhisperX — `whisperx|hybrid`; у каждого слова флаг `aligned/interpolated/low_confidence/failed`. Aligner не должен выдавать «точные» таймкоды неверным словам |
| 4 | Диаризация | **pyannote community-1**, CUDA (NVIDIA) / **CPU (AMD)**; exclusive + regular; **независимый `diarization.json`** с кэшем; перекрытия сохраняются в JSON |
| 5 | Merge | Python: max-overlap, `ambiguous`/`overlap`, low-confidence не двигает спикера; паузы/границы предложений/короткие реплики |
| 6 | LLM | **Python `postprocess`** → `llm_host` (`/v1/chat/completions`), JSON-патчи, валидатор, `edits.json`; LLM в Python не грузится; по умолчанию `enabled: false` |
| 7 | Sidecar | Общий пакет `sidecar/hds_sidecar` (stdio/runtime/config/progress/fs) + **общий портативный CPython**, но **два процесса** (`hds_extract`, `hds_transcribe`) и **venv по профилю**. Не один процесс и не одно venv |
| 8 | GPU | **CUDA** (NVIDIA) — основной; **DirectML** (AMD Radeon/Intel, Windows) — ASR GigaAM ONNX; pyannote CPU; **Vulkan — только LLM** в `llm_host` (или CPU); Metal — бонус |
| 9 | Упаковка | `build_transcribe_sidecar.ps1 -Profile cuda|cpu|dml`; в архив — **один** venv; `requirements.lock` / `requirements-dml.lock`; `export_gigaam_onnx.ps1` |

## 3. Контракты, которые нельзя ломать

* формат `.md`: `### SPEAKER_00 [00:00:01 - 00:00:11]`, литерал `UNASSIGNED`;
* `.orig.md` — **итоговый текст после postprocess, но до подстановки имён**
  (иначе UI-кнопка «присвоить имена» откатит LLM-правки); исходный ASR — в
  `transcript.json`;
* `.speakers.json`, `.bak`, UI-закладка «Транскрибация» — без изменений;
* демон: `transcribe.lock`, очередь, `source_disposal`, `transcribe.stop`, пауза
  индексации — без изменений;
* `out_dir` не индексируется; схема `index.db` не меняется;
* старые ключи `auto_transcribe.*` совместимы; новые — аддитивные;
* `transcribe.engine: python | legacy` — откат на текущую цепочку.

## 4. Что именно меняется в коде (карта)

| Слой | Сейчас | Цель |
|---|---|---|
| Медиа → текст | `TranscribeClient` → HTTP `llm_host /internal/transcribe` | `hds_transcribe` sidecar по stdio JSON-RPC (протокол 1, аддитивные методы) |
| ASR | whisper.cpp в движке | GigaAM-v3 (PyTorch/ONNX) в Python |
| Диаризация | Nemotron-3 ONNX (Rust `hds-diarize`) | pyannote community-1 в Python |
| Alignment | нет / сегменты движка | native GigaAM + WhisperX (опция) |
| Merge | в движке/`llm_host` | Python `merge.py` |
| Пост-коррекция | LLM-чат отдельно | Python `postprocess.py` → `llm_host` |
| Rust-клиент | `hds-index::transcribe` | новый `crates/hds-transcribe` поверх `hds-extract::Worker` |
| Запись файлов | Rust `transcribe_to_file` | остаётся в Rust (контракт) |

**Не трогать в P0:** `crates/hds-index`, `crates/hds-llama`, UI, установщики,
`config.example.yaml` — только после отчёта P0.

## 5. Ближайшая задача — P0 (спайк)

**Цель:** получить числа и принять решения до написания боевого кода.

### 5.1 Окружения

* CUDA: `uv venv` + `torch==2.8.0` (cu128/cu126), `torchaudio==2.8.0`,
  `transformers==4.57.1`, `pyannote-audio==4.0.0`, `torchcodec==0.7.0`,
  `whisperx==3.8.7rc1`, `hydra-core`, `omegaconf`, `sentencepiece`;
* DML: `torch==2.8.0` CPU + `onnxruntime-directml==1.24.4` + `transformers`,
  `pyannote-audio`, `torchcodec`; **без** `onnxruntime` и **без** `whisperx`;
* проверить, что `onnxruntime` и `onnxruntime-directml` не стоят в одном venv;
* проверить wheel `onnxruntime-directml` под Python 3.12 (иначе 3.11/3.13).

### 5.2 Файлы

* 18 / 56 / 77 мин — те же, что в `PLAN_DIARIZATION_NEMOTRON.md`
  (`D:\diar-spike-data\meet0924.wav`, `meet0512.wav`, `meet0514.wav`, если есть);
* +1–2 коротких: несколько каналов, перекрытия, телефон, термины 1С/МЧД.

### 5.3 Что измерить

* WER/CER (нормализация без пунктуации/регистра), точность сущностей;
* атрибуция спикеров, DER (если есть RTTM), число спикеров;
* MAE границ слов, доля `low_confidence/failed`;
* RTF, пик VRAM/RAM (CUDA), память ORT-сессии/GPU (DML);
* стабильность: 5 файлов подряд, рестарт воркера.

### 5.4 Что сравнить

* `e2e_rnnt` vs `rnnt` vs `e2e_ctc`;
* `torch` vs `onnx` (CUDA), `onnx` (DML) vs CPU;
* `native` vs `whisperx` vs `hybrid`;
* `transcribe_longform` vs Silero VAD vs без VAD;
* `decode.channels: auto/downmix/split`;
* pyannote community-1 vs текущая Nemotron-3 (для контекста).

### 5.5 Ворота

* **CUDA:** пик VRAM ≤ 10 ГБ без LLM; RTF ≤ 2× текущего;
* **DML:** WER ONNX ≤ +0,2 п.п. к PyTorch; RTF **заметно лучше CPU**; иначе
  профиль не объявлять поддержанным;
* **G0 hotwords:** на одном WAV `e2e_rnnt` в gigastt, `hotwords=[]` vs `[фраза]`,
  `boost`; текст/токены должны **реально меняться**; иначе `backend: none`;
* артефакты: `tools/parity/out/py_transcribe/spike/REPORT.md` + JSON-замеры.

### 5.6 Чего НЕ делать в P0

* не менять `crates/`, `sidecar/` и установщики (кроме временных скриптов в
  `tools/parity/out/py_transcribe/`);
* не включать hotwords «на глаз» и не выдавать текстовую замену за biasing;
* не обещать GPU-диаризацию на AMD: pyannote идёт на CPU;
* не тащить `torch-directml` (устарел, не совместим с torch 2.8);
* не запускать `onnxruntime` и `onnxruntime-directml` в одном venv.

## 6. Правила работы

* Это **dev-машина**: можно останавливать/запускать `llm_host`, `hds watch`,
  `hds mcp-http`, `hds ui`, пробы; временно освобождать GPU; перезаписывать
  `config.yaml`, `data/`, `index.db`; пересобирать релиз.
* **Не трогать** чужие незакоммиченные изменения и не переписывать решения в
  боевых `PLAN_*.md`.
* Артефакты экспериментов — только в `tools/parity/out/` (gitignored).
* Коммиты — только по явной просьбе владельца; план сейчас **untracked**, не
  потерять его при переключениях.
* Если для решения нужны HF-токен, эталонный корпус или живая Radeon — спросить,
  не выдумывать.

## 7. Промт для нового чата (скопировать целиком)

```text
Ты — Cline в режиме Act. Продолжаешь работу над проектом hermes-disk-search
(C:\Users\Sasha\hermes-disk-search), ветка python-sidecar-transcribe
(база server-multiuser @ f202128). Код ещё не менялся. План
PLAN_PYTHON_SIDECAR_TRANSCRIBE.md лежит в рабочем дереве и пока НЕ закоммичен —
не потеряй его и не перезаписывай решения без новых данных.

ПЕРВЫЕ ДЕЙСТВИЯ (не начинай с кода):
1) Проверь `git status --short --branch` и `git log -1 --oneline --decorate`.
2) Прочитай PLAN_PYTHON_SIDECAR_TRANSCRIBE.md целиком; особенно §0, §2, §4.2,
   §4.7, §5.1–5.4, §6, §9, §10, §16.
3) Прочитай PLAN_AUTO_TRANSCRIBE.md §5 (контракты .md/.orig.md/.speakers.json/UI),
   sidecar/README.md и crates/hds-extract/src/{worker.rs,protocol.rs}.
4) Только после этого начинай P0.

СУТЬ ПЕРЕНОСА
Сейчас транскрибация/диаризация — в Rust/движке (whisper.cpp + Nemotron-3/Sortformer,
llm_host /internal/transcribe). Переводим в Python-sidecar:
- ASR: GigaAM-v3 e2e_rnnt (PyTorch fp16 на CUDA; ONNX + onnxruntime-directml на AMD;
  CPU fallback). Контрольные варианты: rnnt, e2e_ctc — только в P0.
- Alignment: сначала native word-timestamps GigaAM; WhisperX (wav2vec2-large-xlsr-53-
  russian) — режим качества; у слова флаг aligned/interpolated/low_confidence/failed.
- Диаризация: pyannote community-1 (exclusive + regular), независимый
  diarization.json с кэшем; на AMD — CPU (ONNX-пути у pyannote нет).
- Merge: Python, max-overlap, ambiguous/overlap, low-confidence не двигает спикера.
- Постобработка: Python postprocess -> llm_host /v1/chat/completions, только JSON-
  патчи + валидатор + edits.json; LLM в Python не грузится; по умолчанию off.
- Упаковка: общий пакет sidecar/hds_sidecar и общий CPython, но ДВА процесса
  (hds_extract, hds_transcribe) и venv по профилю; не один процесс и не одно venv.

ЖЕЛЕЗО
- Основной целевой: Windows 11, RTX 3060 12 ГБ, 8 ядер CPU, 64 ГБ RAM.
- AMD Radeon под Windows: GigaAM ONNX + DmlExecutionProvider; pyannote CPU;
  Vulkan — только LLM в llm_host (или CPU). torch-directml НЕ используем
  (устарел, не совместим с torch 2.8). Vulkan для ASR/диаризации не нужен.
- macOS/Metal — необязательный бонус.

ЗАДАЧА P0 (только спайк, без боевого кода)
1) Окружения в tools/parity/out/py_transcribe/ (НЕ в sidecar/ и не в crates/):
   - venv-cuda: torch 2.8.0 cu128/cu126, torchaudio, transformers 4.57.1,
     pyannote-audio 4.0.0, torchcodec 0.7.0, whisperx 3.8.7rc1;
   - venv-dml: torch 2.8.0 CPU + onnxruntime-directml 1.24.4 + transformers +
     pyannote-audio + torchcodec; без onnxruntime и без whisperx.
   Проверь, что onnxruntime и onnxruntime-directml не стоят в одном venv.
2) Файлы: 18/56/77 мин из PLAN_DIARIZATION_NEMOTRON.md (D:\diar-spike-data\*.wav,
   если есть) + 1–2 коротких с перекрытиями/каналами/терминами.
3) Замеры: WER/CER (нормализованный), точность сущностей, атрибуция спикеров,
   DER (если RTTM), MAE границ слов, доля low_confidence/failed, RTF, пик
   VRAM/RAM (CUDA) / память ORT-сессии (DML).
4) Сравни: e2e_rnnt vs rnnt vs e2e_ctc; torch vs onnx (CUDA), onnx-DML vs CPU;
   native vs whisperx vs hybrid; transcribe_longform vs Silero VAD vs без VAD;
   decode.channels auto/downmix/split.
5) G0 hotwords: на одном WAV, e2e_rnnt в gigastt, hotwords=[] vs [фраза],
   boost; текст/токены должны реально меняться. Если нет — hotwords закрыты,
   backend: none; текстовую замену не выдавать за biasing.
6) DML-спайк: экспорт GigaAM e2e_rnnt в ONNX (encoder/decoder/joint), запуск с
   DmlExecutionProvider, SessionOptions с execution_mode=SEQUENTIAL и
   enable_mem_pattern=False; проверить WER ≤ +0,2 п.п. к PyTorch и RTF лучше CPU.
7) Отчёт: tools/parity/out/py_transcribe/spike/REPORT.md + JSON-замеры. В отчёте —
   таблицы, версии, железо, решения: variant, align.mode, longform/VAD, channels,
   hotwords (G0), DML (да/нет/почему).

ОГРАНИЧЕНИЯ
- Не менять crates/, sidecar/, installers/, config.example.yaml в P0; временные
  скрипты — только в tools/parity/out/py_transcribe/.
- Не ломать контракты: .md (### SPEAKER_NN [HH:MM:SS - HH:MM:SS], UNASSIGNED),
  .orig.md (после postprocess, до имён), .speakers.json, .bak, UI, демон,
  transcribe.lock, source_disposal, transcribe.stop, out_dir не индексируется.
- Не трогать чужие незакоммиченные изменения; не переписывать боевые PLAN_*.md.
- Коммиты — только по явной просьбе владельца.
- Если нужны HF-токен, эталонный корпус или живая Radeon — спроси, не выдумывай.

КРИТЕРИЙ ГОТОВНОСТИ P0
Есть REPORT.md с воспроизводимыми числами и однозначными решениями; ни один
файл боевого пути не изменён; `git status` показывает только план/handoff и
артефакты в tools/parity/out/. Дальше — P1 только после подтверждения решений.
```

## 8. Стартовый чек-лист (PowerShell)

```powershell
cd C:\Users\Sasha\hermes-disk-search
git status --short --branch
git log -1 --oneline --decorate

# план и handoff на месте?
Get-Item .\PLAN_PYTHON_SIDECAR_TRANSCRIBE.md, .\PYTHON_SIDECAR_TRANSCRIBE_HANDOFF_PROMPT.md |
  Select-Object Name,Length,LastWriteTime

# инструменты
uv --version
python --version
ffmpeg -version | Select-Object -First 1
nvidia-smi
Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM

# эталонные файлы (если есть)
Get-ChildItem D:\diar-spike-data -Filter *.wav -ErrorAction SilentlyContinue |
  Select-Object Name,Length
```

Если `uv` нет — поставить отдельно (инструкция в `installers/build_sidecar.ps1` /
`sidecar/README.md`), не менять системный Python. Все временные venv и отчёты —
в `tools/parity/out/py_transcribe/`; этот каталог gitignored.

## 9. Открытые вопросы (спросить владельца, не выдумывать)

| # | Вопрос | Что нужно для P0/P1 |
|---|---|---|
| 1 | HF-токен для `pyannote/segmentation-3.0` и `community-1` | токен с принятыми условиями; иначе longform/диаризация не скачаются |
| 2 | Эталонный корпус 1–2 ч + разметка спикеров | без него нельзя проверить WER/атрибуцию/DER |
| 3 | Есть ли живая Radeon RX 6000/7000+ (или Intel Arc/iGPU) | без неё DML-профиль не объявлять приёмочным |
| 4 | Допустим ли Vulkan **только для LLM** на AMD | иначе LLM на CPU (медленно) или отдельный LLM-рантайм вне MVP |
| 5 | Нужен ли LLM-слой по умолчанию | сейчас `enabled: false`; включать точечно |
| 6 | Кто ведёт `data/glossary.yaml` | нужен владелец терминов (1С/МЧД/ЗУП/фамилии) |
| 7 | Оставлять ли лимит 6 ч | сейчас `max_duration_hours: 6` |

## 10. Ссылки (первоисточники, проверены 22.10.2026)

| Что | Ссылка |
|---|---|
| GigaAM-v3 | https://huggingface.co/ai-sage/GigaAM-v3 |
| GigaAM код / `onnx_utils.py` | https://github.com/salute-developers/GigaAM |
| gigastt (hotwords 64×64) | https://github.com/ekhodzitsky/gigastt · `docs/api.md` |
| pyannote community-1 | https://huggingface.co/pyannote/speaker-diarization-community-1 |
| pyannote.audio 4.x | https://github.com/pyannote/pyannote-audio |
| WhisperX | https://github.com/m-bain/whisperX |
| wav2vec2-ru | https://huggingface.co/jonatasgrosman/wav2vec2-large-xlsr-53-russian |
| ONNX Runtime DirectML EP | https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html |
| `onnxruntime-directml` | https://pypi.org/project/onnxruntime-directml/ |
| `torch-directml` (не используем) | https://pypi.org/project/torch-directml/ |
| DirectML (maintenance mode) | https://github.com/microsoft/DirectML |
| llama.cpp backends (нет DirectML) | https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md |
| Qwen2.5-7B-Instruct | https://huggingface.co/Qwen/Qwen2.5-7B-Instruct |

## 11. Главное, что нельзя забыть

1. **Hotwords — не настройка GigaAM.** Это отдельный ASR-рантайм (`gigastt`),
   INT8, с ограничением 64×64. Без G0–G3 — `backend: none`; текстовая замена —
   post-correction, не biasing.
2. **AMD — DirectML, не Vulkan.** GigaAM ONNX + `onnxruntime-directml`; pyannote
   CPU; Vulkan только для LLM. `torch-directml` не использовать.
3. **Контракты важнее скорости.** `.md/.orig.md/.speakers.json`, UI, демон,
   `out_dir` вне индексации — не ломать.
4. **Сначала P0, потом код.** Не начинать P1 без отчёта с числами.
5. **План не закоммичен.** Перед переключением веток/экспериментами не потерять
   `PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` и этот handoff.

---

**Следующий шаг нового чата:** выполнить §8, прочитать план, запустить P0 и
вернуться с `REPORT.md` и списком решений. Не писать боевой код до этого.



