# PLAN_PYTHON_SIDECAR_TRANSCRIBE.md — транскрибация/диаризация на Python-sidecar

> **Статус:** план предложен 22.10.2026. Ветка — **`python-sidecar-transcribe`**
> (база — `server-multiuser` @ `f202128`, рабочее дерево чистое). Код ещё не менялся.
>
> **Что заменяется.** Стек ASR/диаризации из `PLAN_AUTO_TRANSCRIBE.md` §2–§5 и
> `PLAN_DIARIZATION_NEMOTRON.md` (whisper.cpp + Nemotron-3/Sortformer в движке и
> `llm_host`) уходит в **Python-sidecar** на GigaAM-v3 + WhisperX + pyannote.
>
> **Что НЕ меняется (контракты):**
> * демон `hds transcribe-watch`: очередь, идемпотентность, `transcribe.lock`,
>   `source_disposal`, пауза индексации, `transcribe.stop`;
> * формат `.md`: `### SPEAKER_00 [00:00:01 - 00:00:11]`, литерал `UNASSIGNED`;
> * служебные файлы `.orig.md`, `.speakers.json`, `.bak` и UI-закладка
>   «Транскрибация» (присвоение имён, «Перезапустить задание», папки);
> * исключение `auto_transcribe.inbox_dir`/`out_dir`/`state_dir` из индексации;
> * ключи `auto_transcribe.*` — совместимы; новые ключи только аддитивные.
>
> **Вводные:** стек предложен заказчиком 22.10.2026; отчёты-ревью Perplexity и
> ChatGPT — `C:\Users\Sasha\Downloads\EXAMINATION_OF_CHOOSEN_STACK*.txt|md` (учтены,
> см. §14). Первоисточники изучены 22.10.2026 (ссылки — §15).
>
> **Железо (целевое):** Windows 11, RTX 3060 **12 ГБ** (CUDA — основной путь),
> 8 ядер CPU, 64 ГБ RAM. **AMD Radeon под Windows — через DirectML** (GigaAM ONNX),
> без обязательного Vulkan; Vulkan остаётся только для LLM в `llm_host`
> (альтернатива — CPU). macOS/Metal — необязательный бонус. Подробно — §4.7.

---

## 0. Краткие решения (TL;DR)

| # | Компонент | Решение | Почему |
|---|---|---|---|
| 1 | **ASR** | **GigaAM-v3 `e2e_rnnt`**: PyTorch fp16 на CUDA, **ONNX + DirectML на AMD/Intel**, ONNX/CPU — fallback; ревизия закреплена. Контрольный режим — `rnnt` | Русская SOTA, MIT, 220–240M; `e2e_rnnt` сразу даёт пунктуацию и нормализацию. WER 8,4 % в карточке относится к `rnnt`, а не к `e2e_rnnt` — это разные метрики |
| 2 | **Hotwords** | **Не baseline и не настройка официального GigaAM.** Biasing живёт в декодере `gigastt` (Rust/ONNX), поэтому hotwords = **смена ASR-рантайма** (INT8), а не параметр к fp16 GigaAM. Ворота G0–G3 (§4.2); по умолчанию `hotwords.backend: none` + глоссарий/пост-коррекция | Нельзя одновременно «официальный GigaAM + hotword biasing»: у `transformers`-пути такого API нет. `DEFAULT_HOTWORDS` — лексикон, а не доказательство совместимости с декодером; бенчмарки gigastt «pending» |
| 3 | **Alignment** | **Сначала нативные word-timestamps GigaAM**; **WhisperX** — режим качества (`whisperx|hybrid`); у каждого слова — **флаг качества** (`aligned/interpolated/low_confidence/failed`) | Нативные таймкоды не требуют второго прохода. WhisperX не выравнивает цифры/латиницу; кроме того, aligner может «уверенно» разложить неверно распознанное слово — это ложная точность, её надо помечать, а не выдавать за факт |
| 4 | **Диаризация** | **pyannote community-1** (`pyannote.audio` 4.x), CUDA/CPU; **на AMD — CPU** (ONNX-пути нет); **exclusive** + обычная разметка; результат — **самостоятельный артефакт** (`diarization.json`) с кэшем | Gated-модель CC-BY-4.0; `exclusive_speaker_diarization` создана для стыковки с ASR; независимый артефакт позволяет менять ASR/aligner без повторной диаризации |
| 5 | **Merge** | Python: атрибуция слова по максимальному пересечению с turns + `ambiguous`/`overlap`; низкоуверенные слова **не** двигают спикера; разбиение по смене спикера/паузе/границе предложения | Простое «по смене speaker_id» теряет перекрытия, короткие реплики и невыравненные слова |
| 6 | **LLM-постобработка** | **Python-воркер** (`postprocess`) → **существующий `llm_host`** (`/v1/chat/completions`, одна LLM в VRAM), JSON-патчи + валидатор в Python; Rust только оркестрирует и пишет файл | Python удобнее для промптов/валидации/тестов; LLM не грузится в Python (инференс в `llm_host`), поэтому VRAM не складывается. Это же рекомендует ChatGPT (§14.1) |
| 7 | **Упаковка** | **Общий** пакет `sidecar/hds_sidecar` (протокол/stdio/конфиг/логи) и **общий портативный CPython**; но **два процесса** и **два venv**: `hds_extract` (как сейчас) и `hds_transcribe` | Переиспользуем решённые грабли транспорта, но не тащим torch в извлечение документов и не блокируем его длинной транскрибацией |
| 8 | **GPU-профили** | **CUDA** (NVIDIA) — основной. **AMD Radeon/Intel под Windows — DirectML**: GigaAM ONNX + `onnxruntime-directml`; pyannote — CPU; alignment — native (или ONNX-wav2vec2, P2). **CPU** — fallback. **Vulkan** — только для LLM в `llm_host`; Metal — бонус | Vulkan для ASR/diarization не нужен и не обещается. DirectML даёт AMD GPU-ускорение ASR без `torch-directml`; pyannote ONNX-пути не имеет, поэтому его CPU — осознанный компромисс |

**Целевой результат:** на той же машине и тех же файлах качество текста и атрибуции
спикеров **не хуже** текущей цепочки (метрики §10), при этом стек становится
расширяемым в Python и не требует движка для ASR/диаризации.

---

## 1. Область

**Входит:**
* P0 — спайк и фиксация версий/ревизий моделей, замер VRAM/RTF на 3 эталонных файлах.
* P1 — общий пакет `sidecar/hds_sidecar` + перевод на него существующего
  `hds_extract` (поведение не меняется), новый воркер, Rust-клиент, замена
  `TranscribeClient`; вывод `.md` через существующий `transcribe_to_file`; UI/демон
  без изменений.
* P2 — качество alignment: нативные таймкоды → WhisperX (`hybrid`), «произнесённая
  форма» и обратное отображение в текст для чтения.
* P3 — hotwords/глоссарий: спайк `gigastt`, сравнение с детерминированным глоссарием.
* P4 — LLM-патчи через `llm_host` (JSON-edits + валидатор + журнал).
* P5 — упаковка: портативный Python, модели, установщик, `hds check`, NOTICE/README,
  **CUDA / DirectML (AMD/Intel) / CPU**-профили; macOS/Metal — best-effort.
* P6 — вывод legacy-ветки из боевого пути (флаг `transcribe.engine: legacy|python`),
  удаление `hds-diarize`/`parakeet-rs`/`ort`/`DirectML.dll` из default-сборки.

**Не входит:**
* изменение формата `.md`, UI-контрактов и схемы БД;
* стриминг/онлайн-транскрибация (только файлы);
* диаризация на нескольких устройствах одновременно;
* замена поиска/индексации/LLM-чата;
* поддержка языков кроме русского в ASR (GigaAM-v3 — ru/en; интерфейс оставляем
  `language`, но качество для других языков не заявляем).

## 2. Факты текущего кода (что именно придётся менять)

| # | Факт | Где |
|---|---|---|
| Ф1 | Демон `hds transcribe-watch` зовёт `TranscribeClient::transcribe_to_file` (HTTP `POST /internal/transcribe` к `llm_host`), пишет `.md`/`.orig.md`/`.speakers.json`, утилизирует исходник | `crates/hds-index/src/autotranscribe.rs`; `crates/hds-index/src/transcribe.rs` |
| Ф2 | Формат и имена файлов фиксированы в Rust (`next_free_path`, `write_text_atomic`, `detect_speaker_slots`) — UI и тесты зависят от них | `transcribe.rs` (`TranscribeOutcome`), `crates/hds-ui/src/transcribe.rs` |
| Ф3 | ASR/диаризация сейчас — в `llm_host`: `internal_transcribe`, ветки `transcribe_nemotron3` и legacy sortformer, выбор backend, VRAM-арбитраж, роли `whisper`/`transcribe` | `crates/hds-llama/src/host.rs` (2252–2520, 2615–2826), `diar.rs` |
| Ф4 | Rust-диаризация: `hds-diarize` (Nemotron-3 ONNX через `parakeet-rs`/`ort`), `hds-ort`, `DirectML.dll` в поставке | `crates/hds-diarize`, `crates/hds-ort`, `installers/build_rust_release.ps1` |
| Ф5 | Python-воркер извлечения документов: stdio JSON-RPC 2.0, ленивый запуск, `hello`/`extract`/`normalize`/`shutdown`, настраиваемый `request_timeout`, перезапуск | `sidecar/hds_extract/worker.py`, `crates/hds-extract/src/{worker.rs,protocol.rs}` |
| Ф6 | Портативный Python для sidecar собирается `uv python install` + `uv pip install -r requirements.lock`; в поставке `sidecar/python/**/python.exe` | `installers/build_sidecar.ps1`, `sidecar/README.md` |
| Ф7 | Модели сейчас: whisper GGML и Nemotron-3 ONNX/sortformer GGUF в `%APPDATA%\OpenResearchTools\models`; установщики `fetch_whisper_model.ps1`, `fetch_nemotron3_model.ps1`, `fetch_diarization_model.ps1` | `installers/*` |
| Ф8 | `hds check`/`hds whisper-check` показывают backend, модель, sha256, EP, ffmpeg | `crates/hds-cli/src/cmd/whisper_check.rs`, `crates/hds-index/src/diag.rs` |
| Ф9 | Индексация медиа отключена (`skipped_media`); `index.transcribe` — legacy | `crates/hds-index/src/pipeline.rs` |
| Ф10 | LLM-чат уже резидент: роль `chat`, OpenAI-совместимый фасад `127.0.0.1:8010`, модель `shared:chat` (в NOTICE — Qwen3.5-9B) | `config.example.yaml`, `crates/hds-llama/src/facade.rs`, `NOTICE.md` |

**Вывод:** меняется «начинка» `TranscribeClient` (HTTP → sidecar) и удаляется
`llm_host`-ветка транскрибации. Демон, UI, имена файлов, индексация и БД — не трогаем.

---

## 3. Целевая архитектура

```
inbox_dir
   │  hds transcribe-watch (Rust: очередь, lock, disposal, pause)
   ▼
hds-transcribe (Rust-клиент)
   │  stdio JSON-RPC 2.0 (NDJSON)
   ▼
hds_transcribe/worker.py (Python, venv по профилю, общий CPython)
   ├─ decode: ffmpeg → WAV 16 кГц mono (один раз)
   ├─ ASR: GigaAM-v3 e2e_rnnt (CUDA fp16 | ONNX-DML | CPU)
   ├─ VAD/нарезка: pyannote segmentation-3.0 (для longform) или silero
   ├─ align: нативные word timestamps | WhisperX + wav2vec2-ru
   ├─ diarize: pyannote community-1 (CUDA/CPU; на AMD — CPU), exclusive + regular
   └─ merge: слова × turns → блоки реплик (JSON)
   │
   ▼  transcript.json: blocks + words + speakers + alignment quality
hds_transcribe/worker.py — postprocess (Python)
   ├─ глоссарий: кандидаты → контекст → canonical / needs_review
   ├─ LLM: llm_host /v1/chat/completions → JSON-патчи
   └─ валидатор патчей + edits.json (без изменения спикеров/таймкодов)
   │
   ▼  final text + edited blocks
hds-transcribe (Rust)
   └─ пишет out_dir/<stem>.md, .orig.md, .speakers.json (как сейчас)
   ▼
UI «Транскрибация»: имена спикеров, перезапуск, превью (без изменений)
```

Общий Python-пакет — **не процесс и не транспорт**, а библиотека:

```
sidecar/hds_sidecar/{stdio,runtime,config,progress,fs}.py
        ▲ import                         ▲ import
hds_extract/worker.py              hds_transcribe/worker.py
(документы; своё окружение)        (медиа; venv по профилю)
```

**Разделение ответственности:**
* Python — акустика (ASR/alignment/diarization), слияние и `postprocess`
  (глоссарий/LLM-патчи/валидатор); **не пишет пользовательские файлы** и **не
  загружает LLM** (только HTTP-вызов);
* Rust — оркестрация, файлы, очередь, вызовы `transcribe`/`postprocess`, запись
  `.md`; в промпты и патчи не лезет;
* `llm_host` — единственная LLM в памяти (инференс); после акустических стадий
  Python выгружает модели, поэтому пик VRAM не складывается.

### 3.1 Почему именно такой шов

1. **Контракты уже стабильны.** `transcribe_to_file` — единственная точка записи
   файлов; меняя только транспорт, мы сохраняем UI, тесты, `.orig/.speakers.json`
   и поведение демона.
2. **Изоляция зависимостей.** `torch 2.8` (CUDA/CPU) или
   `onnxruntime-directml`, `transformers`, `pyannote`, `whisperx` — тяжёлый и
   конфликтный набор. В воркере извлечения документов ему
   не место: там `PyMuPDF`, `pymorphy3`, Java (`mpxj`), и любое обновление torch
   ломало бы извлечение.
3. **Один владелец GPU для LLM.** `llm_host` уже считает VRAM, приоритеты и
   вытеснение. Python делает только HTTP-вызов; вторая копия LLM в Python-процессе
   конкурировала бы с ним «вслепую».
4. **Откат.** Флаг `transcribe.engine: python|legacy` позволяет вернуть текущую
   цепочку без переустановки и без миграции данных.

## 4. Стек по компонентам

### 4.1 ASR — GigaAM-v3 `e2e_rnnt`

**Факты (карточка + код, 22.10.2026):**
* модель: Conformer, **220–240M**, предобучение HuBERT-CTC на 700 тыс. ч русской речи;
  варианты `ssl`, `ctc`, `rnnt`, `e2e_ctc`, `e2e_rnnt`; лицензия **MIT**;
* `e2e_ctc`/`e2e_rnnt` отдают **пунктуацию и нормализацию**; сравнение с
  Whisper-large-v3 — 70:30 по оценке LLM-судьи (Gemini 2.5 Pro), это **не WER**;
* WER 8,4 % в таблице — столбец **V3_RNNT**; для `e2e_rnnt` отдельного WER в карточке
  нет ⇒ **нельзя** переносить 8,4 % на выбранный вариант;
* `transcribe_longform` режет аудио по `LONGFORM_THRESHOLD = 25 с`, батчирует
  (`fr_batch_size=16`), использует **`pyannote/segmentation-3.0`** для VAD и требует
  `HF_TOKEN` (если модели нет локально);
* `word_timestamps=True` даёт `Word(text, start, end)`; таймкоды считаются по
  токенам декодера (`timestamps_utils.frames_to_words`);
* загрузка аудио — собственный `load_audio`: `ffmpeg -f s16le -ac 1 -ar 16000`,
  т.е. **16 кГц mono**; рекомендуемые версии: `torch==2.8.0`, `torchaudio==2.8.0`,
  `transformers==4.57.1`, `pyannote-audio==4.0.0`, `torchcodec==0.7.0`,
  `hydra-core`, `omegaconf`, `sentencepiece`.

**Решения:**
* основной режим — `e2e_rnnt`, fp16 на CUDA (`fp16_encoder=True`), CPU — fp32;
* `language="ru"` фиксируем (как сейчас `index.whisper_language`), пустая строка —
  автоопределение;
* **вариант, ревизия модели и версии пакетов пинуются**: `transcribe.asr.variant`
  (`e2e_rnnt`; P0 также проверяет `e2e_ctc` и `rnnt`), `transcribe.asr.revision`
  (пусто = variant; иначе явный HF revision/локальный snapshot), `requirements.lock`;
* контрольные режимы `rnnt`/`e2e_ctc` — только для спайка/сложных участков, не для
  всего архива;
* **backends:** `asr.backend: torch` — официальный пакет/`transformers` (CUDA/CPU);
  `asr.backend: onnx` — тот же `variant` через `to_onnx`/`load_onnx` и
  `onnxruntime`/`onnxruntime-directml` (EP `cuda|dml|cpu`). ONNX **обязателен** для
  DML-профиля (§4.7); на CUDA он опционален и сравнивается в P0 по RTF/качеству.
  Экспорт encoder/decoder/joint для RNNT — отдельные `.onnx`; версии `onnx`/`onnxruntime`
  пинуются в `requirements-dml.lock`. Обёртка `asr_onnx.py` сама создаёт
  `SessionOptions` (`execution_mode=SEQUENTIAL`, `enable_mem_pattern=False`) —
  стоковый `load_onnx` этого не делает, а DirectML EP без этого падает.
  **Важно:** `transcribe_longform` — PyTorch-метод; для ONNX-пути longform
  реализуется в `asr_onnx.py` своим VAD/окнами (Silero или pyannote segmentation)
  и `infer_onnx` по окнам, с теми же 25-с-окнами и батчем;
* **длинные файлы**: используем `transcribe_longform`, но с двумя оговорками:
  1. `pyannote/segmentation-3.0` — дополнительная gated-модель; её наличие и токен
     проверяем в `hds check` (иначе longform упадёт на 26-й секунде);
  2. альтернатива — собственный VAD-чанкинг (silero-vad ONNX на CPU) с окнами
     `20–30 с` и контекстом `1–2 с`; решение принимает спайк P0 по качеству стыков
     и скорости. В любом случае **декодирование ffmpeg одно**, дальше — массивы.

**Почему не `gigastt` как основной ASR:** проект молодой (2.22.0), по умолчанию
INT8-модель ~310 МиБ, собственные RNNT-бенчмарки **сняты до публикации provenance**,
а `e2e_rnnt` в нём — отдельная сборка. Для цели «максимальное качество» сначала
берём официальный fp16 GigaAM; `gigastt` — кандидат на hotword-рантайм (§4.2).

### 4.2 Hotwords / биасинг сущностей — риск интеграции (главный)

**Что подтверждено (docs.rs + `docs/api.md` + quickstarts, 22.10.2026):**
* `gigastt-core` — **Rust**-рантайм (ONNX Runtime, GigaAM v3), не Python-пакет;
* у сервера `gigastt serve` есть `hotwords` (до **64 фраз**, до **64 символов**)
  и `hotwords_boost`; поддерживаются головы `rnnt` и `e2e_rnnt` (жадный
  трансдьюсер); ошибки `too_many_hotwords` / `hotword_phrase_too_long`;
* в gigastt есть лексикон `DEFAULT_HOTWORDS` (русские бренды/аббревиатуры), но это
  **список**, а не публичный API «включить biasing в GigaAM»;
* Python-биндинг `gigastt_uniffi` в quickstart **не показывает** параметр hotwords —
  они доступны у HTTP-сервера, а не «просто в Python»;
* `gigastt-core` тянет `ort =2.0.0-rc.13`, в проекте уже `rc.15` ⇒ встраивать в
  Rust-воркспейс нельзя; только отдельный процесс/HTTP.

**Что НЕ подтверждено (и почему это критично):**
1. Что hotword biasing можно применить к **официальному** GigaAM
   (`transformers`/fp16). Нельзя: biasing — часть декодера **gigastt**, а не модели.
   Это **не настройка**, а **смена ASR-рантайма** (INT8-сборка GigaAM в gigastt).
2. Что INT8-голова `e2e_rnnt` в gigastt не хуже официальной fp16 на наших записях.
   Собственные RNNT-бенчмарки gigastt **сняты до публикации provenance**.
3. Что `hotwords_boost` реально меняет нужные слова, а не вставляет ложные.
4. Что 64 фраз × 64 символа хватает на домен (1С/МЧД/ЗУП/фамилии/продукты).
5. Что `DEFAULT_HOTWORDS` вообще включён/подходит; по умолчанию — нет.

**Следствие для архитектуры:**
* **baseline** — официальный GigaAM `e2e_rnnt` fp16 **без logit biasing**;
* **hotwords — отдельный эксперимент** (`hotwords.backend: gigastt`), а не часть
  основного пути; по умолчанию `none`;
* **нельзя** смешивать «ASR официальным GigaAM + biasing от gigastt»;
* **нельзя** называть текстовую замену «Индинес → 1С» hotword-biasing: это
  **post-correction**, она не влияет на акустику и не эквивалентна boost.

#### Ворота G0–G3 (закрывают вопрос без веры в документацию)

| Ворота | Что проверяем | Как | Проходной критерий |
|---|---|---|---|
| **G0. Интеграция** | Существует ли реальный эффект biasing на выбранной голове | Зафиксировать версию `gigastt-core` (2.22.0), прочитать код `lexicon`/`inference`; один и тот же WAV, `e2e_rnnt`, `hotwords=[]` vs `[фраза]`, `boost`; сравнить текст/токены | текст/токены **реально меняются**; иначе маршрут закрыт |
| **G1. Качество ASR** | Не хуже ли INT8-рантайм официального fp16 | 1–2 ч корпуса §10: gigastt `e2e_rnnt` (без hotwords) vs официальный GigaAM | WER ≤ **+0,3 п.п.**, recall сущностей не ниже; иначе маршрут закрыт |
| **G2. Польза hotwords** | Даёт ли biasing выигрыш без ложных вставок | Тот же gigastt, `hotwords=[]` vs `[словарь встречи]`, `boost` из P0 | recall сущностей **≥ +10 % относительного**, ложные вставки ≤ **1 %**, WER не растёт > 0,3 п.п. |

Только при G0+G1+G2 включаем `transcribe.hotwords.backend: gigastt`. Иначе —
`none`, а сущности правим глоссарием/LLM (§4.6).

**G3 (опционально, если G0 прошли, но G1 — нет): повторное декодирование окон.**
Вместо перевода всего файла на INT8-рантайм можно оставить официальный GigaAM
основным, а gigastt с hotwords звать только на короткие подозрительные участки:
* триггер — нечёткое совпадение с `variants` глоссария (edit distance ≤ 2) или
  `low_confidence` alignment рядом с термином;
* окно 4–12 с с контекстом 1 с; ответ — текст + слова + таймкоды;
* замена принимается, только если в окне появился `canonical`, соседние слова
  совпали (SequenceMatcher ≥ 0,8), а таймкоды монотонны и не выходят за окно;
* иначе окно остаётся как есть; всё логируется в `edits.json`;
* критерий: рост recall сущностей без роста WER/ложных вставок на полном файле.
Это **не** baseline и не повод включать hotwords везде; при отсутствии выигрыша
маршрут закрывается.

#### Глоссарий: кандидаты, а не безусловные замены

Формат `data/glossary.yaml` (расширен против первого варианта):

```yaml
terms:
  - canonical: "1С"
    variants: ["один эс", "индинес", "индес", "1 эс", "1c"]
    type: product            # product | org | person | abbreviation | domain
    context: ["бухгалтерия", "ERP", "документооборот"]
    priority: high           # high | normal | low
    case_sensitive: false
    allow_global: false      # "один эс" НЕ заменять без контекста
    pronunciations: ["один эс"]
forbid: []                   # фразы, запрещённые как replacements вообще (слишком общие)
```

Правила:
* `variants` — **кандидаты** для исправления, не гарантия;
* замена выполняется, только если рядом (окно ±N слов) есть слово из `context`
  **или** `priority: high` и нет конкурирующего термина; иначе — `needs_review`,
  исходный текст сохраняется;
* `allow_global: true` — только для однозначных аббревиатур (МЧД, ЗУП), не для
  «один эс»;
* границы слов, регистр, кириллица/латиница (`1C`/`1С`) — нормализуются, но
  канон сохраняется как в файле;
* для hotwords уходит `pronunciations` (что слышно), но не более 64 фраз;
  полный глоссарий для пост-коррекции может быть больше.

**Если hotwords не прошли ворота:** никакой имитации. Работает детерминированный
глоссарий + LLM-патчи; при появлении у GigaAM официального API prompt/biasing —
отдельная задача, не «дорисовываем» его сами.

### 4.3 Forced alignment — WhisperX + wav2vec2-ru

**Факты:**
* WhisperX `3.8.7rc1`: Python `>=3.10,<3.14`, `torch~=2.8.0`, `torchaudio~=2.8.0`,
  `pyannote-audio>=4.0.0`, `transformers>=4.48`, BSD-2-Clause;
* для `ru` по умолчанию — **`jonatasgrosman/wav2vec2-large-xlsr-53-russian`**
  (Apache-2.0, `Wav2Vec2ForCTC`, 16 кГц, обучен на Common Voice 6.1 + CSS10);
* `align(transcript, model, metadata, audio, device, interpolate_method="nearest",
  return_char_alignments=False, ...)` принимает **произвольные сегменты** с
  `text/start/end` — Whisper для этого не нужен; `audio` передаём уже
  декодированным массивом 16 кГц mono, `metadata` — из `load_align_model("ru", device)`
  (иначе WhisperX декодирует файл ещё раз);
* **GPU-профили:** на CUDA — WhisperX/wav2vec2 на GPU как обычно; на AMD (§4.7)
  по умолчанию `align.mode: native` (таймкоды GigaAM), а `whisperx` — CPU либо
  отдельный ONNX-wav2vec2 + ORT DML (P2, опционально). Диаризация от этого не зависит;
* **ограничение карточки WhisperX:** слова с символами вне словаря модели
  («2014.», «£13.60») **не получают таймкод**; перекрытия обрабатываются плохо.

**Главная проблема:** `e2e_rnnt` отдаёт **нормализованный текст** («1С», «25 %»,
«МЧД»), а wav2vec2-словарь — строчная кириллица. Подать `e2e`-текст в `align()`
напрямую = потерять именно те слова, ради которых делается выравнивание.

**Три режима (`transcribe.align.mode`):**

| Режим | Что делаем | Когда |
|---|---|---|
| `native` (default) | `word_timestamps=True` у GigaAM; таймкоды относятся к тем же токенам, что и текст | всегда, если качество приемлемо |
| `whisperx` | выравниваем **произнесённую форму**: `rnnt`-транскрипт (без нормализации) или де-нормализация `e2e` (числа → слова, `1С` → `один эс`, латиница → транслит), затем `align()` | если нативные таймкоды «плывут» |
| `hybrid` | нативные + WhisperX; расхождения больше порога (например, `>0.35 с` на границе) разрешаем в пользу WhisperX; у каждого слова храним `source` | режим качества |

**Отображение обратно в текст для чтения:**
* каждому слову дисплея храним `display_span = (char_start, char_end)` и
  `spoken_words[]`;
* при `rnnt`-параллели — сопоставление `e2e` ↔ `rnnt` через `difflib.SequenceMatcher`
  по нормализованным словоформам: равные блоки копируют времена, замены получают
  времена по пропорции внутри блока, вставки — от соседей;
* **никогда** не наследуем таймкод по позиции при изменении числа слов (правило
  Perplexity §7);
* результат merge — всегда исходные слова с `word_id`, а не «переписанный» текст.

**Качество alignment — обязательный слой, а не побочный результат.** Aligner
возвращает таймкоды даже для неверно распознанного слова; это **ложная точность**.
Поэтому каждому слову/сегменту присваиваем:

| Флаг | Когда | Что делаем |
|---|---|---|
| `aligned` | CTC-путь уверен, длительность > 0, нет аномалий | используем как обычный таймкод |
| `interpolated` | слово не выровнено, время получено от соседей | в merge **не** двигает спикера |
| `low_confidence` | нулевая/отрицательная длительность, наложение на соседнее слово, разрыв > 1,5 с внутри фразы, низкий CTC-score | оставляем, но помечаем; в статистику |
| `failed` | выравнивание сегмента не удалось | берём границы ASR-сегмента, **не** выдаём точные времена |

Дополнительно:
* `alignment_quality` считаем по сегменту: доля `aligned`, число аномалий, средний
  score; при плохом качестве сегмента — fallback на границы сегмента и предупреждение;
* **WER 13,3 % / 40,22 % карточки wav2vec2 — это ASR-метрика, а не качество
  выравнивания**; ранжировать aligner по ней нельзя. Критерии — MAE границ слов,
  доля невыровненных/аномальных слов, устойчивость на цифрах и аббревиатурах;
* в P0 сравнить три варианта: нативные таймкоды GigaAM, WhisperX XLS-R, и (если
  найдётся поддерживающий русский и произвольный текст) более лёгкий CTC-aligner —
  как **эксперимент**, без обещания замены;
* нативные таймкоды GigaAM проходят те же проверки: «native» не значит «правильный».

**Спайк P0 решает:** нужен ли `whisperx` вообще. Критерий: MAE границ слов на ручной
выборке ≤ **100 мс**, доля `low_confidence`/`failed` ≤ **2 %**, отсутствие
«слипшихся» реплик; если нативные таймкоды дают хуже — включаем `hybrid` по умолчанию.

**Стоимость:** `rnnt`-параллель — второй проход ASR (та же модель, без диаризации);
на 77-мин файле это ориентировочно десятки секунд на CUDA (замерить в P0). Поэтому
`hybrid` — режим качества, а не default; `native` должен закрывать большинство файлов.

### 4.4 Диаризация — pyannote community-1

**Факты:**
* пайплайн `pyannote/speaker-diarization-community-1`, `pyannote.audio` **4.x**;
  лицензия **CC-BY-4.0**, модель **gated**: нужно принять условия и передать
  `token=` (HF access token);
* API: `Pipeline.from_pretrained(..., token=...)` → `pipeline.to(torch.device("cuda"))`
  → `output = pipeline(audio, hook=ProgressHook())`;
* результат: `output.speaker_diarization` (обычная, с перекрытиями) и
  **`output.exclusive_speaker_diarization`** (без перекрытий — для стыковки с ASR);
* параметры числа спикеров: `num_speakers` либо `min_speakers`/`max_speakers`;
* на NVIDIA GPU — через PyTorch/CUDA; CPU работает, но медленнее (штатный fallback);
  на AMD/Intel под Windows официального ONNX/DirectML-пути у pyannote нет, поэтому
  диаризация идёт на CPU даже при `profile: dml` (§4.7) — ASR при этом остаётся на GPU;
  метрики телеметрии отключаем `PYANNOTE_METRICS_ENABLED=0`;
* заявлено превосходство над 3.1 (в т.ч. DIHARD 3: 20,2 % vs 21,4 %) — **проверить
  на своём корпусе**, а не принимать на веру.

**Решения:**
* декодируем аудио **один раз** и подаём в пайплайн словарь
  `{"waveform": tensor[1,N] float32, "sample_rate": 16000}` — это исключает второй
  проход `torchcodec`/`ffmpeg` и расхождение шкал;
* основной результат для merge — **exclusive**; обычная разметка нужна для флага
  `overlap=true` и диагностики;
* `num_speakers` берём из конфига, если известно; иначе `min_speakers: 1`,
  `max_speakers: 20` (не фиксируем 8/15 «на глаз» — Perplexity §5);
* спикеры перенумеровываются в `SPEAKER_00…` по первому появлению (стабильно между
  запусками при одинаковом входе);
* сбой диаризации: при `transcribe.diarization.required: true` — задание падает
  (как сейчас); при `false` — `.md` без меток, в статистике `diarization.enabled=false`
  и текст ошибки;
* VRAM: держим пайплайн на GPU только на время стадии; после — `del`, `gc.collect()`,
  `torch.cuda.empty_cache()` (см. §4.7).

**Диаризация — независимый артефакт, а не часть ASR:**
* результат сохраняем в `state_dir/<stem>.diarization.json` (обычная + exclusive
  разметка, модель/ревизия, параметры, время);
* ключ кэша: `sha256(файл) + размер + mtime + длительность + ревизия модели +
  min/max/num_speakers`; при изменении ASR/aligner/глоссария диаризация **не**
  пересчитывается (`transcribe.cache.diarization: true`);
* это позволяет повторно прогнать ASR/alignment/пост-коррекцию без затрат на
  pyannote и сравнивать варианты на одном и том же diarization-артефакте;
* при `diarization.enabled: false` артефакт не создаётся; при `required: false` и
  сбое — сохраняем ошибку в артефакте, а не молча теряем.

**Перекрытия не «схлопываем» в одного спикера:**
* в JSON у слова/блока может быть `speakers: ["SPEAKER_00", "SPEAKER_01"]`,
  `primary` и `overlap: true`;
* в `.md` (контракт не меняем) остаётся один `primary`, но `overlap` виден в
  статистике и JSON; UI/формат с двумя метками — отдельное решение, не в MVP;
* нельзя выдавать exclusive-разметку за «второго говорящего нет»: это лишь
  представление для стыковки, а не факт отсутствия перекрытия.

**Почему не Nemotron-3/Sortformer:** остаются legacy-веткой (флаг), но новый стек
не должен зависеть от ONNX Runtime/DirectML в Rust. Удаление — P6, после успешной
приёмки Python-пути.

### 4.5 Word-level merge (Python)

**Вход:** слова ASR `{word_id, text, start, end, display_span, confidence?}`;
exclusive-turns `{speaker, start, end}`; обычные turns — для overlap.

**Алгоритм (детерминированный, тестируемый без моделей):**

1. Для каждого слова считаем `overlap(word, turn)` по всем спикерам.
2. `speaker = argmax(overlap)`. Условия неопределённости:
   * максимальное пересечение `< 0.5 × длительности слова` ⇒ `UNASSIGNED`;
   * разница между первым и вторым кандидатом `< 10 %` длительности ⇒ `ambiguous=true`
     (в `.md` — `UNASSIGNED`, в JSON — оба кандидата);
   * слово с `alignment in {interpolated, low_confidence, failed}` **не** служит
     доказательством смены спикера: берём спикера предыдущего/следующего
     `aligned`-слова, а само слово помечаем;
3. Если слово пересекает turns **двух и более** спикеров ⇒ `overlap=true`
   (это не ошибка, а перекрытая речь; в `.md` остаётся один спикер — по max).
4. Разбиение на блоки:
   * смена спикера;
   * пауза между словами `> 0.8 с`;
   * граница предложения (если `e2e` дал пунктуацию) — предпочтительно;
   * принудительно: блок `> 30 с` или `> 500` символов;
   * склейка соседних блоков одного спикера при паузе `≤ 0.3 с`.
5. Короткие реплики (`< 0.4 с`, «да», «нет», «угу») **не сглаживаем** и не
   приклеиваем к соседу: это может быть другой участник (Perplexity §6).
6. Слова без таймкода (`start/end = None`) не выбрасываем: они попадают в блок по
   соседям, но помечаются `alignment="interpolated"` и **не используются** как
   акустическое доказательство при смене спикера.
7. Если диаризация отключена — весь текст одним блоком без заголовков (семантика
   текущего `backend=off`), а не «всё в `UNASSIGNED`».

**Многоканальное аудио — проверить до downmix.** Если в видео несколько аудиодорожек
или каналов, они могут быть уже разделены по участникам (запись с микшера, две
петлички). Тогда `ffmpeg -ac 1` убивает готовую диаризацию. Правило:
* `decode.channels: auto` (default) — `ffprobe` показывает ≥2 каналов; проверяем,
  различаются ли они (корреляция/энергия); если да — обрабатываем **поканально**,
  канал становится спикером, сшивка по времени; если это дубли/стерео-копия —
  downmix;
* `downmix` — всегда один канал (текущее поведение);
* `split` — принудительно поканально;
* для поканального пути GigaAM/pyannote получают mono-дорожку канала; merge
  объединяет по времени, `speaker = channel`, `overlap` не нужен.

**VAD — отдельный проверяемый компонент, а не «обязательный проход»:**
* сравниваем в P0: `transcribe_longform` (pyannote segmentation-3.0), свой Silero VAD
  и вариант без VAD; критерии — WER, стыки слов, RTF;
* VAD **не режет речь на границах сегментов**: окна с контекстом 1–2 с, границы
  только в паузах; исходная шкала времени сохраняется;
* если GigaAM на целых файлах не хуже, VAD оставляем выключенным
  (`asr.longform: auto`), а не «для галочки».

**Формат результата (JSON от воркера):**
```json
{
  "blocks": [
    {"speaker": "SPEAKER_00", "start": 1.24, "end": 11.8,
     "text": "…", "words": [{"word_id": 12, "text": "…", "start": 1.24,
     "end": 1.6, "alignment": "aligned", "overlap": false,
     "speakers": ["SPEAKER_00"]}]}
  ],
  "speakers": ["SPEAKER_00", "SPEAKER_01"],
  "unassigned_words": 3,
  "ambiguous_words": 1,
  "overlap_words": 4,
  "alignment_quality": {"aligned_ratio": 0.97, "low_confidence": 4, "failed": 1}
}
```

**Рендер `.md` — в Rust** (существующий `render`-контракт), чтобы формат не
разъезжался между языками. Python отдаёт только структуру.

### 4.6 LLM-постобработка — Python-воркер + `llm_host`

**Решение (изменено против первой версии плана):** постобработка живёт **в Python**
(`postprocess.py`), а инференс — в существующем `llm_host`
(`/v1/chat/completions`). Python **не загружает** LLM, поэтому VRAM не складывается;
`llm_host` остаётся единственным владельцем LLM и сам вытесняет/загружает роль.
Rust только оркестрирует (`transcribe` → `postprocess`) и пишет итоговый `.md`.

**Почему Python, а не Rust:**
* промпты, глоссарий, валидация и тесты — это быстро меняющаяся логика; в Python
  её проще итерировать и покрывать `pytest`;
* Python уже владеет `word_id`/`display_span`/таймкодами — валидатору не нужно
  гонять структуру через JSON-RPC в Rust и обратно;
* Rust не должен знать про форматы патчей; его роль — очередь, файлы, VRAM и
  вызовы. Это совпадает с выводами ChatGPT (§5 отчёта) и Perplexity;
* цена — один HTTP-вызов из Python в loopback; `urllib` из stdlib достаточен,
  новых зависимостей нет.

**Два метода, а не один длинный:**
* `transcribe` — ASR/alignment/diarization/merge → `state_dir/<stem>.transcript.json`
  (слова, блоки, спикеры, флаги качества);
* `postprocess` — читает `transcript.json`, применяет глоссарий/LLM, возвращает
  финальный текст и `edits`; его можно перезапустить **без повторного ASR**.
  Rust вызывает его только при `transcribe.llm.enabled: true` или непустом
  глоссарии; при выключенном — текст не меняется.

**Порядок:**
1. **Детерминированный слой (без LLM):** точные замены по глоссарию
   (`variants → canonical`), регулярные выражения для чисел/дат/аббревиатур;
   правила контекста/`allow_global` из §4.2; неоднозначное — `needs_review`,
   исходный текст сохраняется.
2. **Кандидаты для LLM:** только подозрительные фрагменты (попадания глоссария,
   `low_confidence` alignment, паттерны «Индинес/1С», редкие слова), а не весь
   транскрипт.
3. LLM возвращает **только JSON-патчи**, не текст:
```json
{"edits": [{"word_ids": [128, 129], "replacement": "1С",
            "reason": "глоссарий + контекст ERP", "needs_review": true}]}
```
4. **Валидатор (Python, без LLM):**
   * `word_ids` существуют и лежат в одном блоке/у одного спикера;
   * замена не меняет `speaker`, `start`, `end`;
   * не исчезают отрицания, не меняются числа/даты/суммы без отдельного разрешения;
   * длина замены ≤ 3× длины исходного фрагмента (кроме аббревиатур из глоссария);
   * контекст замены совпадает с `context` глоссария либо `allow_global: true`;
   * не более N правок на 1000 слов (`transcribe.llm.max_edits_per_1000`);
   * при любом нарушении патч **отклоняется целиком**, причина — в журнал.
5. **Журнал и контракт файлов:** `state_dir/<stem>.edits.json` — что предложено,
   что применено, что отклонено и почему. `.orig.md` — **итоговый текст после
   postprocess, но до подстановки имён** (иначе UI-кнопка «присвоить имена»
   откатила бы LLM-правки); исходный ASR-текст остаётся в `transcript.json`,
   `.bak` — как сейчас в UI.
6. **Модель:** начинаем с **Qwen2.5-7B-Instruct Q4_K_M** (GGUF, llama.cpp), в P4
   сравниваем с Q5_K_M; текущую роль `shared:chat` (Qwen3.5-9B) можно использовать
   как альтернативу, но **не держим две LLM одновременно**. `temperature: 0`,
   `max_tokens` ограничен, таймаут; при недоступном `llm_host` — предупреждение,
   текст остаётся без LLM-правок (деградация, не падение задания).
7. **Стоимость:** 7–9B Q4 на 12 ГБ — примерно 20–40 токенов/с; полный транскрипт
   77-мин встречи — десятки минут. Поэтому LLM **не обязательна**: по умолчанию
   `enabled: false`, только глоссарий; LLM — «качество» для отдельных файлов.

**Перед вызовом LLM Python выгружает ASR/alignment/diarization**
(`del`, `gc.collect()`, `torch.cuda.empty_cache()` на CUDA; закрытие ORT-сессий на
DML), затем вызывает `llm_host` —
так пик VRAM не складывается. Если LLM не нужна, `postprocess` работает только
с глоссарием и не трогает GPU.

### 4.7 Железо: CUDA, DirectML (AMD/Intel), CPU, Metal

**Принцип:** модели **не живут в памяти одновременно**. Стадии идут последовательно,
после каждой — выгрузка **по профилю**: CUDA — `del`, `gc.collect()`,
`torch.cuda.empty_cache()`; DML — закрыть ORT-сессии, `gc.collect()`; CPU — `del`,
`gc.collect()`. Это важнее «микрооптимизаций»: на 12 ГБ попытка держать ASR +
alignment + diarization + LLM даёт OOM.

**Профили (выбирает установщик/конфиг, не пользователь вручную):**

| Профиль | ASR | Alignment | Диаризация | LLM | Статус |
|---|---|---|---|---|---|
| **CUDA** (NVIDIA) | GigaAM PyTorch fp16 (`torch cu128`) | WhisperX/wav2vec2 CUDA или native | community-1 CUDA | `llm_host` CUDA | основной, целевой |
| **DML** (AMD Radeon / Intel, Windows 10/11) | GigaAM **ONNX** + `onnxruntime-directml` (`DmlExecutionProvider`) | native GigaAM; опционально wav2vec2 ONNX+DML или CPU | community-1 **CPU** | `llm_host` **Vulkan** (или CPU) | поддержка AMD — цель этого профиля |
| **CPU** | GigaAM ONNX/PyTorch CPU | native; WhisperX CPU | community-1 CPU | CPU (медленно) | fallback |
| **Metal** (macOS) | GigaAM PyTorch MPS | WhisperX MPS/CPU | CPU (MPS best-effort) | `llm_host` Metal | бонус, без живой приёмки |

**DML-профиль: что реально работает (проверено по документации, 22.10.2026):**
* `onnxruntime-directml` **1.24.4** (Windows, DX12; AMD/Intel/NVIDIA);
* GigaAM официально умеет `to_onnx(...)`: для RNNT экспортируются **encoder /
  decoder / joint** отдельными файлами; `gigaam.onnx_utils.load_onnx(...,
  provider="DmlExecutionProvider")` принимает произвольный provider (в коде —
  `_providers_list`), т.е. ASR на DirectML **не требует torch-directml**;
* DirectML EP: обязательны `execution_mode=SEQUENTIAL`, `enable_mem_pattern=False`,
  один поток на сессию; лучше фиксированные формы (наши окна 25 с и батч — фикс.,
  последний батч паддить); fp16 — только если GPU/драйвер поддерживает, иначе fp32;
  проверить в P0;
* **`torch-directml` не используем**: последний релиз `0.2.5.dev240914` (сентябрь
  2024), он отстаёт от upstream и не совместим с `torch 2.8`, на котором стоят
  WhisperX/pyannote;
* DirectML **в maintenance mode**, ORT DirectML EP — в sustained engineering;
  новые фичи — в WinML. Это не «мертво», но версии пинуются и проверяются.

**Что на AMD остаётся CPU и почему:**
* **pyannote community-1** не имеет официального ONNX/DirectML-пути (в трекере
  pyannote нет ONNX-экспорта). Запускать его через `torch-directml` нельзя из-за
  версии torch. Поэтому диаризация — **CPU, 8 ядер**, с замером RTF; это осознанный
  компромисс, а не «забыли»;
* **WhisperX alignment**: по умолчанию на AMD — **нативные таймкоды GigaAM**
  (без второго прохода). Если нужен режим качества — либо WhisperX на CPU, либо
  отдельный ONNX-wav2vec2 + ORT DML и собственный CTC-aligner (P2, опционально);
* **LLM**: в llama.cpp **нет DirectML-бэкенда** (есть CUDA/Vulkan/HIP/SYCL/Metal/
  CPU). На AMD под Windows практический GPU-путь — **Vulkan** в существующем
  `llm_host`; если Vulkan принципиально не нужен — LLM на CPU (7B Q4 будет
  медленно). Это единственное место, где Vulkan остаётся, и оно не относится к
  Python-стеку ASR. Альтернатива на будущее (не MVP): отдельный LLM-рантайм на
  ONNX Runtime GenAI/DirectML или OpenCL-бэкенд llama.cpp — потребует замены
  `llm_host`-движка, а не «переключения флага».

**Оценка пиков (не замеры; проверить в P0):**

| Стадия | CUDA | DML (AMD) | CPU |
|---|---|---|---|
| ffmpeg decode + WAV | 0 | 0 | 0 |
| GigaAM `e2e_rnnt` | 2–3 ГБ | **1–2 ГБ** (ONNX) | 0 |
| WhisperX wav2vec2-large | 1,5–2 ГБ | 0 / CPU | 0 |
| pyannote community-1 | 2–4 ГБ | 0 (CPU) | 0 |
| LLM Qwen 7–9B Q4 + KV | 6–8 ГБ | 6–8 ГБ (Vulkan) | 0 |
| CUDA/DML context + буферы | 0,5–1 ГБ | 0,3–0,8 ГБ | 0 |
| **Пик без LLM** | 4–6 ГБ | **1–3 ГБ** | 0 |
| **Пик с LLM** | 7–9 ГБ | 7–9 ГБ (Vulkan) | RAM |

Пик с LLM на AMD такой же, как на CUDA, потому что LLM живёт в `llm_host` на
Vulkan; acoustic-модели к этому моменту выгружены. Резерв 1–2 ГБ обязателен.

**CPU (8 ядер), без GPU:**
* GigaAM fp32/ONNX CPU — рабочий, но медленнее; `torch.set_num_threads(6)`
  (оставить 2 ядра системе и ffmpeg); ORT `intra_op_num_threads=6`;
* pyannote на CPU — основной fallback; время растёт, память нет;
* WhisperX на CPU — только в режиме `whisperx`/`hybrid`;
* общий бюджет: **не более одного тяжёлого процесса**; параллельные задания не
  запускаем (демон и так последовательный).

**CUDA-колёса и драйвер:** `torch 2.8` ставится из индекса `cu128` (или `cu126` —
зависит от драйвера машины). В P0 фиксируем пару «драйвер → индекс колёс» и
записываем в `requirements.lock`/установщик; «поставили cu128 на старый драйвер»
даёт `CUDA driver version is insufficient` уже на первом задании.

**DirectML-драйвер:** Windows 10/11 + актуальный AMD Adrenalin/Intel driver,
DX12-совместимая Radeon (практически RX 6000/7000+; RX 5000 — проверить).
`onnxruntime-directml` ставится **вместо** `onnxruntime` (оба дают модуль
`onnxruntime`, конфликт), поэтому для DML-профиля — отдельное venv.

**macOS/Metal (бонус):** `torch` MPS для GigaAM/wav2vec2; pyannote на MPS может
не поддерживать часть операций ⇒ CPU fallback; `torchcodec` для macOS есть, но мы
подаём waveform напрямую. Сборка/установка — best-effort, без приёмки на живой
машине (как сейчас в проекте).

**Диск:**
* **CUDA-профиль:** `torch` CUDA + зависимости ≈ 4–6 ГБ; модели ≈ 3–4 ГБ (GigaAM
  ~1 ГБ, wav2vec2 ~1,2 ГБ, community-1 ~1 ГБ, segmentation-3.0 ~0,2 ГБ);
* **DML-профиль:** `torch` CPU + `onnxruntime-directml` ≈ 1–1,5 ГБ; модели те же,
  но GigaAM — в ONNX (~1 ГБ, без PyTorch-весов) ⇒ экономия ~1–2 ГБ;
* портативный Python + venv ≈ 1–2 ГБ;
* **ставится только один профиль**, не оба сразу; если позже меняется GPU —
  переустановка venv, модели переиспользуются.

Итого **+8–12 ГБ (CUDA)** или **+6–9 ГБ (DML)** к установке; в README/установщике
обновить требование «около 25 ГБ» и указать оба профиля.

**ffmpeg:** нужен CLI (GigaAM вызывает `ffmpeg` из PATH). Установщик уже ставит
Gyan.FFmpeg машинно; воркер дополнительно получает `PATH` с каталогом ffmpeg и
проверяет `ffmpeg -version` на старте (ошибка — понятная, до задания).

**Ориентир по времени (проверить в P0, не обещание):**

| Файл | Текущая цепочка (Rust/движок) | CUDA | DML (AMD) | CPU |
|---|---|---|---|---|
| 18 мин | ~1 мин | 1–2 мин | 2–4 мин | 3–6 мин |
| 77 мин | 3,6 мин (замер 0.21.0) | 4–8 мин | 6–12 мин | 15–30 мин |
| + LLM-слой | — | +5–20 мин | +5–20 мин (Vulkan) | не включать |

Критерий приёмки: **≤ 2× текущего времени на CUDA** для того же файла; DML —
**заметно быстрее CPU и ≤ 3× текущего времени** (иначе профиль не имеет смысла);
CPU — «работает и укладывается в разумное время», без обязательства паритета.

## 5. Sidecar: устройство и протокол

### 5.1 Состав и раскладка

```
sidecar/
  hds_sidecar/            # НОВЫЙ общий пакет (не процесс!)
    __init__.py           # пустой: без импортов, чтобы не тянуть чужие зависимости
    stdio.py              # fd 1→stderr, UTF-8 stdin, NDJSON, reply/ok/err, serve()
    runtime.py            # reader-thread, idle-timeout, EOF, защита от падения по одному запросу
    config.py             # state_dir(), config_path(root), load_config() — общее с извлечением;
                          #   yaml/hds.config импортируются лениво, только в load_config()
    progress.py           # [stage] … в stderr, redirect_stdout
    fs.py                 # temp-каталоги, безопасные пути, long-path
  hds_extract/            # существует: извлечение документов
    worker.py             # после P1 импортирует hds_sidecar.*; методы не меняются
    requirements.lock
  hds_transcribe/         # новый воркер
    worker.py             # serve() из hds_sidecar + hello/transcribe/cancel/shutdown
    params.py             # датаклассы параметров (приходят из Rust, не из config.yaml)
    decode.py             # ffprobe каналов, ffmpeg → WAV 16 кГц mono/поканально
    asr_gigaam.py         # e2e_rnnt / rnnt / e2e_ctc через PyTorch, longform, word_timestamps
    asr_onnx.py           # те же варианты через onnxruntime (EP cuda|dml|cpu) + export-хелперы
    align_whisperx.py     # native | whisperx | hybrid, spoken form, quality-флаги
    diarize_pyannote.py   # community-1, exclusive + regular, независимый артефакт
    merge.py              # слова × turns → блоки (чистые функции, без моделей)
    glossary.py           # кандидаты/контекст/канон, детерминированные замены
    postprocess.py        # postprocess: глоссарий → llm_host → валидатор патчей
    quality.py            # alignment/ASR quality-флаги и пороги
    cache.py              # transcript/diarization JSON, ключи и инвалидация
    models.py             # абсолютные пути/ревизии; без поиска по конфигу
    hotwords_gigastt.py   # опциональный клиент gigastt serve (G0–G3)
    requirements.lock     # CUDA/CPU: torch 2.8 + onnxruntime + whisperx
    requirements-dml.lock # DML: torch CPU + onnxruntime-directml, без whisperx
    README.md
  python/                 # ОБЩИЙ портативный CPython (уже есть)
  venv-transcribe/        # пакеты транскрибации: CUDA или CPU (от sidecar/python)
  venv-transcribe-dml/    # вариант для DML-профиля: onnxruntime-directml вместо onnxruntime
```

**Профиль и окружения.** В **поставке** ставится ровно один venv под выбранный
профиль: `venv-transcribe` (CUDA/CPU, PyTorch) либо `venv-transcribe-dml` (torch CPU +
`onnxruntime-directml`; ASR — GigaAM ONNX). Оба venv создаются от одного
`sidecar/python`; в **разработке/P0** допустимо держать оба для сравнения — это
разные каталоги, и конфликт `onnxruntime`/`onnxruntime-directml` возникает только
внутри одного venv. Профиль фиксируется установщиком и читается Rust; смена GPU =
переустановка venv, модели переиспользуются.

**Почему отдельный процесс, а не метод в `hds_extract`:**
* `Worker` обрабатывает **один запрос за раз**; 30-минутная транскрибация
  заблокировала бы извлечение документов и индексацию;
* torch и CUDA/DML-контекст (~0,5–1 ГБ RSS/VRAM) не нужны процессу, который
  запускается ради каждого PDF; импорт torch добавил бы секунды к `hello`;
* падение CUDA/DML/OOM не должно убивать извлечение документов;
* разные `idle_timeout` (60 с у извлечения, 600 с у транскрибации);
* конфликт пинов (`numpy`/`torch` против `PyMuPDF`/`Pillow`) не должен становиться
  фатальным для обеих задач.

**Что переиспользуем, а что нет:**

| Компонент | Переиспользуем? | Как |
|---|---|---|
| Портативный CPython `sidecar/python` | **да** | один интерпретатор; venv по профилю создаётся от него |
| Изоляция fd 1 / UTF-8 stdin / NDJSON / idle-timeout / EOF | **да, обязательно** | вынести из `hds_extract/worker.py` в `hds_sidecar.stdio/runtime` |
| `state_dir()` / `config_path()` / `load_config()` | **да** | `hds_sidecar.config`; извлечение сохраняет своё поведение |
| `_reply/_ok/_err`, `arg()`, обработка битого JSON | **да** | `hds_sidecar.stdio` |
| Rust-транспорт `Worker`/`protocol` | **да** | `hds-extract` становится общим «Python worker transport»; новый `hds-transcribe` — типизированный слой |
| `WorkerConfig.stderr_log`, перезапуск, таймаут | **да** | без изменений |
| Поиск `ffmpeg`, каталогов моделей, `config.yaml` | **нет (в Python)** | Rust — единственный источник истины; в запросе приходят абсолютные пути |
| Один процесс на обе задачи | **нет** | блокировки, CUDA/DML-контекст, blast radius |
| Одно venv на обе задачи | **нет** | torch ~5 ГБ и конфликты пинов в извлечении не нужны |
| `hds/extractors.py`, `lemmatizer.py` | **нет** | транскрибации не нужны |
| Модели ASR/диаризации | **нет** | извлечению не нужны |

**Правило границы:** общий пакет содержит только транспорт/процесс/пути/логи и
**не импортирует** ни `torch`, ни `fitz`/`pymorphy3` на уровне модуля — иначе один
из воркеров потянет чужие зависимости. Всё тяжёлое — лениво внутри методов.

### 5.2 Протокол (stdio, JSON-RPC 2.0, NDJSON)

Контракт повторяет существующий (`sidecar/README.md`), но добавляет долгую операцию.
**Версию протокола не поднимаем** (`PROTOCOL_VERSION = 1` в `hds-extract`): новые
методы и поля — аддитивные, а `Worker::handshake` сверяет ровно `1`; иначе пришлось
бы менять и воркер извлечения документов.

**Параметры приходят из Rust.** Python-воркер транскрибации **не читает
`config.yaml`**: `asr.*`, `align.*`, `diarization.*`, `hotwords.*`, абсолютные пути
к моделям и `ffmpeg` передаются в `transcribe.params`. Это убирает второй парсер
конфига и гарантирует, что `hds check` и боевой путь видят одно и то же.

| Метод | Параметры | Результат |
|---|---|---|
| `hello` | `{protocol: 1}` | `{protocol: 1, python, pid, profile, capabilities: ["transcribe","postprocess","diarize","align:native","align:whisperx","quality","asr:torch\|asr:onnx","ep:cuda\|ep:dml\|ep:cpu"], versions: {...}, devices: {...}}` |
| `transcribe` | `{job_id, path, language, align:"native\|whisperx\|hybrid", decode:{channels:"auto\|downmix\|split", ffmpeg}, asr:{model,revision,variant,batch_size,longform}, diarization:{enabled,required,min_speakers,max_speakers,num_speakers}, cache:{diarization}, stop_file}` | `{job_id, transcript_path, blocks[], speakers[], stats, alignment_quality, warnings[], elapsed_ms}` |
| `postprocess` | `{job_id, transcript_path, glossary:{path, terms[]}, llm:{enabled,base_url,model,timeout,max_tokens,max_edits_per_1000}}` | `{job_id, text, blocks[], edits[], warnings[], elapsed_ms}` |
| `cancel` | `{job_id}` | `{cancelled: true}` (проверка на границах стадий) |
| `shutdown` | `{}` | `{}` |

`transcribe` не применяет глоссарий/LLM: он только создаёт `transcript.json`.
`postprocess` можно вызвать позже (в т.ч. после смены глоссария) без повторного ASR.

**Правила:**
* **stdout — только JSON-RPC.** Всё остальное (ffmpeg, tqdm, warnings torch) — в
  stderr; воркер дублирует `fd 1` в stderr, как `hds_extract` (грабли W0).
* **Прогресс** — строки в stderr вида `[stage] asr 42%`; `WorkerConfig.stderr_log`
  уже умеет писать их в журнал задания. В протокол их не кладём (иначе
  `parse_response` по `id` сломается); живой прогресс в UI — отдельная задача
  (piped stderr + читатель), не блокирует MVP.
* **Отмена** — файл `stop_file` (по умолчанию `data/auto-transcribe/transcribe.stop`,
  как сейчас), проверяется перед каждой стадией и внутри батчей longform; жёсткий
  таймаут Rust убивает процесс и перезапускает воркер.
* **Таймаут запроса** — `transcribe.request_timeout` (по умолчанию 6 ч + 10 мин;
  текущий `WorkerConfig` уже умеет это поле).
* **Идемпотентность** — воркер не пишет в `out_dir`; повторный вызов с тем же
  файлом ничего не ломает (Rust решает, перезаписывать ли).
* **Без автоповтора при падении** — в отличие от `extract`, падение воркера во время
  транскрибации **не** повторяет задание внутри одного вызова (`retry_dead=false`);
  повтор — только на уровне демона (`auto_transcribe.max_attempts`, как сейчас).
* **Ошибки** — `{code, message, hint}`: `model_missing`, `hf_token_missing`,
  `ffmpeg_missing`, `cuda_oom`, `dml_oom`, `onnx_error`, `diarization_failed`,
  `cancelled`, `bad_audio`. Rust превращает их в текст задания (как
  `RpcError::to_core`); `dml_oom`/`onnx_error` при `fallback_cpu` ретраятся на CPU.

**Пример обмена:**
```json
{"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":1}}
{"jsonrpc":"2.0","id":1,"result":{"protocol":1,"python":"3.12.14","pid":4120,
 "capabilities":["transcribe","diarize","align:native","align:whisperx"],
 "versions":{"gigaam":"...","pyannote":"4.0.0","whisperx":"3.8.7rc1"}}}
{"jsonrpc":"2.0","id":2,"method":"transcribe","params":{"job_id":"j7",
 "path":"D:\\media_in\\meet.wav","language":"ru","align":"native",
 "diarization":{"enabled":true,"required":true,"min_speakers":1,"max_speakers":20}}}
{"jsonrpc":"2.0","id":2,"result":{"job_id":"j7","text":"…","blocks":[…],
 "speakers":["SPEAKER_00"],"stats":{"asr_sec":86.2,"diar_sec":41.7},
 "warnings":[]}}
```

### 5.3 Жизненный цикл и VRAM

1. Rust запускает воркер лениво (первый файл) и владеет процессом.
2. Воркер грузит модели **по стадиям**, а не все сразу:
   `asr` → выгрузка → `align` (если нужен) → выгрузка → `diarize` → выгрузка →
   `merge` (CPU).
3. После `transcribe.idle_unload_sec` (по умолчанию 600) воркер освобождает
   модели и остаётся жив; после `max_requests` (например, 20) — перезапуск.
4. Освобождение памяти **по профилю**: CUDA — `del`, `gc.collect()`,
   `torch.cuda.empty_cache()`, в stderr `torch.cuda.max_memory_allocated()`;
   DML — закрыть ORT-сессии (`del session`), `gc.collect()`, без вызовов
   `torch.cuda.*`; CPU — `del`, `gc.collect()`. В stderr — фактическая память
   (для `hds check` и отчёта).
5. При OOM/ошибке сессии (`cuda_oom`, `dml_oom`) — один автоматический повтор
   стадии на CPU (если `transcribe.fallback_cpu: true`), с предупреждением в
   результате; иначе задание падает с понятной ошибкой.
6. Параллельных заданий нет: демон последовательный; второй `transcribe` в воркере
   отвергается `busy` (защита от случайного параллелизма).

### 5.4 Изменения в Rust (минимальный диф)

| Файл/крейт | Что |
|---|---|
| `crates/hds-extract/src/worker.rs` | сделать `Worker::request(method, params, retry_dead)` публичным (сейчас `call` приватный); обобщить `discover_python` → `discover_python_with(env_var, venv_rel)` (извлечение вызывает со своими значениями, транскрибация — со своими); остальное — без изменений. **Для транскрибации `retry_dead=false`**: иначе падение воркера автоматически повторит 30-минутное задание |
| **новый** `crates/hds-transcribe` | типы `TranscribeParams/Result`, `PostprocessParams/Result`, клиент поверх `hds_extract::Worker`, путь к `sidecar/hds_transcribe/worker.py`, проверка `hello.capabilities`, маппинг ошибок; свой `WorkerConfig` (таймаут 7 ч, `stderr_log`, `max_requests`) |
| `crates/hds-index/src/transcribe.rs` | `TranscribeClient`: HTTP → `hds_transcribe::Client`; **два вызова**: `transcribe` → `postprocess` (если глоссарий/LLM включены) → запись `.md`; `transcribe_to_file` сохраняет имена/атомарную запись; `detect_speaker_slots` остаётся |
| `crates/hds-index/src/autotranscribe.rs` | без изменений (кроме выбора `engine`) |
| `crates/hds-cli/src/cmd/whisper_check.rs` | переименовать в `transcribe-check` (alias `whisper-check`), показывать версии Python-стека, модели, HF-token, ffmpeg, устройство |
| `crates/hds-index/src/diag.rs` | новый пункт `transcribe` (вместо/рядом с `diarization`) |
| `crates/hds-llama/src/host.rs` | ветки `internal_transcribe`/`transcribe_nemotron3` остаются только при `transcribe.engine: legacy`; роль `whisper` не создаётся в python-режиме |
| `crates/hds-ui` | без изменений; опционально — показ стадии/прогресса из журнала |
| `Cargo.toml` | новый член `crates/hds-transcribe` |

**Поиск интерпретатора** (по аналогии с `discover_python`, но свой):
`HDS_TRANSCRIBE_PYTHON` → `sidecar/<venv>/Scripts/python.exe`
(macOS/Linux — `bin/python3`) → `sidecar/python/**/python.exe` (портативный CPython) →
`.venv` проекта (только разработка). Ошибка — с подсказкой, какой файл не найден.
`<venv>` выбирается по `transcribe.profile`/`HDS_TRANSCRIBE_PROFILE`: `cuda|cpu` →
`venv-transcribe`, `dml` → `venv-transcribe-dml`. Явный `HDS_TRANSCRIBE_PYTHON`
перекрывает всё (отладка и нестандартные сборки).

**Почему новый крейт, а не «допилить hds-extract»:** `hds-extract` — про документы;
его API и тесты не должны знать про ASR. `Worker` переиспользуем, типы — отдельно.
**Технический долг имени:** после P6 `hds-extract` логично переименовать в
`hds-sidecar` (транспорт) + `hds-extract` (доменные методы), но в P1 этого не делаем:
переименование крейта — большой диф без пользы для приёмки.

**Совместимость конфигов:** старые ключи `auto_transcribe.diarization*`,
`index.whisper_*`, `diarization.*` читаются и маппятся в новые; при
`transcribe.engine: legacy` работает старый путь без предупреждений, при `python` —
старые ключи диаризации игнорируются с одной строкой в журнале.

## 6. Конфиг (проект `config.example.yaml`)

```yaml
# --- Транскрибация/диаризация на Python-sidecar (новый боевой путь) ---
transcribe:
  engine: python              # python | legacy (legacy = текущая Rust/движок-цепочка)
  profile: auto               # auto | cuda | dml | cpu; auto — по GPU/драйверу (§4.7)
  request_timeout_sec: 25200  # 7 ч: длинные записи + запас
  idle_unload_sec: 600        # простой → освободить VRAM, процесс оставить
  max_requests: 20            # после N файлов — перезапуск воркера
  fallback_cpu: true          # cuda_oom/dml_oom/onnx_error → одна попытка стадии на CPU
  max_duration_hours: 6       # как сейчас; больше — только явным ключом

  asr:
    model: ai-sage/GigaAM-v3
    variant: e2e_rnnt         # e2e_rnnt | rnnt | e2e_ctc (P0 сравнивает)
    revision: ""              # "" = variant; иначе явный HF revision/локальный snapshot
    language: ru              # "" = автоопределение
    device: auto              # auto | cuda | dml | cpu
    backend: auto             # auto | torch | onnx (dml ⇒ onnx; auto = torch, кроме dml)
    provider: auto            # auto | cuda | dml | cpu — ORT EP при backend: onnx
    batch_size: 8             # longform-батч (16 — только если VRAM позволяет)
    longform: auto            # auto (родной transcribe_longform) | vad (свой silero) | off
    fp16: true                # CUDA — да; DML — если GPU/драйвер поддерживает; CPU — нет

  decode:
    channels: auto            # auto (ffprobe: раздельные каналы → split) | downmix | split
    ffmpeg: ""                # абсолютный путь от Rust; в Python не ищем

  align:
    mode: native              # native | whisperx | hybrid
    model: jonatasgrosman/wav2vec2-large-xlsr-53-russian
    device: auto
    max_shift_sec: 0.35       # hybrid: порог расхождения native vs whisperx
    quality:
      max_gap_sec: 1.5        # разрыв внутри фразы → low_confidence
      min_duration_sec: 0.02  # короче → low_confidence
      min_aligned_ratio: 0.80 # доля aligned в сегменте; ниже → fallback на границы

  diarization:
    enabled: true
    required: true            # true: нет модели/токена ⇒ задание падает (как сейчас)
    model: pyannote/speaker-diarization-community-1
    device: auto              # auto | cuda | cpu; на AMD auto ⇒ cpu (ONNX-пути нет)
    num_speakers: 0           # 0 = неизвестно
    min_speakers: 1
    max_speakers: 20
    use_exclusive: true       # exclusive для merge, обычная — для overlap

  hotwords:
    enabled: false            # только после ворот G0–G3, см. §4.2
    backend: none             # none | gigastt
    phrases: []               # максимум 64, каждая ≤ 64 символов
    max_phrases: 64           # жёсткий лимит gigastt
    max_phrase_chars: 64
    boost: 5.0                # значение из ТЗ; дефолт/эффект проверить в G0
    url: "http://127.0.0.1:9876"

  glossary:
    path: data/glossary.yaml  # формат — §4.2 (кандидаты, контекст, allow_global)

  cache:
    transcript: true          # state_dir/<stem>.transcript.json (для postprocess)
    diarization: true         # state_dir/<stem>.diarization.json (не пересчитывать)

  llm:
    enabled: false            # по умолчанию только глоссарий, без LLM
    base_url: "http://127.0.0.1:8010/v1"
    model: chat               # роль llm_host; для эксперимента — Qwen2.5-7B Q4_K_M
    temperature: 0
    max_edits_per_1000: 10
    max_tokens: 2048
    timeout_sec: 300

  models_dir: ""              # "" → HDS_MODELS_DIR → %ProgramData% → %APPDATA%;
                              # HF-кэш: <models_dir>/hf-home (HF_HOME), офлайн-режим
  hf_token_env: HF_TOKEN      # токен нужен только для скачивания gated-моделей
```

**Глоссарий:** формат и правила — §4.2 (`canonical`, `variants`, `type`, `context`,
`priority`, `allow_global`, `pronunciations`, `forbid`). `variants` — кандидаты,
а не безусловные замены; «один эс» без контекста не меняется.

**Старые ключи:** `auto_transcribe.*` (inbox/out/state, disposal, debounce и т.д.)
не меняются. `index.whisper_*`/`diarization.*` в python-режиме не используются,
но читаются для `engine: legacy`.

---

## 7. Файлы, установщики, модели

| Область | Изменение |
|---|---|
| `sidecar/hds_sidecar/*` | **новый общий пакет**: `stdio.py` (fd/UTF-8/NDJSON/`serve`), `runtime.py` (reader/idle/EOF), `config.py` (`state_dir`/`config_path`/`load_config`), `progress.py`, `fs.py`; без тяжёлых импортов |
| `sidecar/hds_extract/worker.py` | перевод на `hds_sidecar.*`; методы/ответы/поведение **не меняются**; `_config`, `_capabilities`, `_handle` остаются доменными |
| `sidecar/hds_transcribe/*` | новый воркер (см. §5.1) |
| `sidecar/hds_transcribe/requirements.lock` | CUDA/CPU-профиль: `torch==2.8.0` (+cu128/cu126), `torchaudio==2.8.0`, `transformers==4.57.1`, `pyannote-audio==4.0.0`, `torchcodec==0.7.0`, `whisperx==3.8.7rc1` (rc: при выходе стабильной 3.8.x заменить и перепроверить), `hydra-core`, `omegaconf`, `sentencepiece`, `numpy<3`, `onnx`/`onnxruntime` (для ONNX-экспорта) |
| `sidecar/hds_transcribe/requirements-dml.lock` | DML-профиль: `torch==2.8.0` **CPU**, `torchaudio==2.8.0` **CPU**, `onnxruntime-directml==1.24.4` (**вместо** `onnxruntime`), `transformers==4.57.1`, `pyannote-audio==4.0.0`, `torchcodec==0.7.0`, `numpy<3`; **без** `whisperx` — в MVP alignment `native`; `whisperx` на CPU или ONNX-wav2vec2 — отдельная опция P2 |
| `installers/sidecar_common.ps1` | **новый общий helper**: `Get-SidecarPython` (портативный CPython), `Install-SidecarRequirements`, `Copy-SidecarCommon`, `Test-SidecarImports`, `Get-GpuVendor` (NVIDIA/AMD/Intel/нет) |
| `installers/build_sidecar.ps1` | перевести на `sidecar_common.ps1`; **состав и результат прежние** (извлечение не меняется) |
| `installers/build_transcribe_sidecar.ps1` | **новый**: `sidecar_common.ps1` + портативный CPython из `sidecar/python`; `-Profile cuda\|cpu\|dml`; создаёт `venv-transcribe` (CUDA/CPU) или `venv-transcribe-dml`; ставит `requirements.lock`/`requirements-dml.lock` из индекса `https://download.pytorch.org/whl/cu128` или `/cpu`. **Порядок сборки:** сначала `build_sidecar.ps1` (создаёт `sidecar/python`), потом этот скрипт |
| `installers/export_gigaam_onnx.ps1` | **новый**: `model.to_onnx(...)` для `e2e_rnnt` (encoder/decoder/joint) в `<models_dir>/gigaam-onnx/<variant>/`; нужен для DML/CPU-профиля; идемпотентно, версия/ревизия в штампе |
| `sidecar/README.md` | описать общий `hds_sidecar`, два воркера, venv по профилю (CUDA/CPU/DML), контракт протокола не меняется |
| `data/glossary.yaml` + пример | **новый** файл глоссария (формат §4.2); в релиз — пример, рабочая копия не перезаписывается |
| `config.example.yaml`, `server_config.example.yaml` | секция `transcribe.*` (§6); старые `auto_transcribe.*` сохраняются |
| `state_dir/*.transcript.json`, `*.diarization.json`, `*.edits.json` | служебные артефакты (не в `out_dir`); описаны в README; кэш можно удалить |
| `installers/fetch_transcribe_models.ps1` | **новый**: GigaAM-v3 (`e2e_rnnt`), wav2vec2-ru, community-1, `pyannote/segmentation-3.0`; `-ModelsDir`, `-HfToken`, пины ревизий, sha256 где опубликован; идемпотентно |
| `setup.ps1` | шаг «Python transcription stack»: venv + модели + `HF_TOKEN`; `-SkipTranscribeStack`; отдельно `-SkipTranscribeModels` |
| `installers/build_rust_release.ps1` | включить `sidecar/hds_sidecar`, `sidecar/hds_transcribe` и **один** venv по профилю (`venv-transcribe` или `venv-transcribe-dml`); Windows — предсобранный, иначе сборка на установке |
| `installers/install_server.ps1` | профиль по GPU (NVIDIA → cuda, AMD/Intel → dml, нет → cpu); **модели — в `%ProgramData%\hermes-disk-search\models`**, т.к. служба LocalSystem не видит пользовательский HF-кэш; venv — выбранный профиль |
| `crates/hds-cli/src/cmd/whisper_check.rs` | `transcribe-check`: профиль (cuda/dml/cpu), EP, вендор GPU, версии, модели, токен, ffmpeg, фактический VRAM/RTF |
| `crates/hds-index/src/diag.rs` | пункт `transcribe` вместо `diarization` (старый — только legacy) |
| `NOTICE.md` | GigaAM MIT; pyannote community-1 CC-BY-4.0 (атрибуция!); wav2vec2 Apache-2.0; WhisperX BSD-2; torch BSD-3; Qwen2.5-7B-Instruct Apache-2.0; **ONNX Runtime / onnxruntime-directml MIT**; ffmpeg LGPL/GPL; segmentation-3.0 — проверить лицензию |
| `README.md` §8, §11, §18 | профили **CUDA / DirectML (AMD/Intel) / CPU / Metal**; диск 25–30 ГБ; HF-token; Vulkan — только LLM в `llm_host`, для ASR/диаризации не нужен |
| `RELEASE_NOTES` | минорный релиз с флагом `engine: legacy` |

**Размер поставки.** Варианты:
* **A (рекомендуется):** предсобранный venv выбранного профиля в Windows-архиве —
  воспроизводимо; CUDA +4–6 ГБ, DML +1–2 ГБ;
* **B:** `uv pip install` на установке — архив меньше, но нужен интернет и точные
  индексы; риск «поставилось не то».
Для релиза выбираем A (как сейчас с `sidecar`), B — fallback для разработки.
**В один архив кладём только один профиль** (NVIDIA или AMD/Intel); универсальный
архив с двумя venv не делаем — конфликт `onnxruntime`/`onnxruntime-directml` и лишние
гигабайты. macOS-пакет собирается отдельно (CPU/MPS) и остаётся best-effort.

**HF-token и gated-модели.** `community-1` и `pyannote/segmentation-3.0` требуют
принятия условий. На сервере служба работает не от пользователя ⇒ токен не должен
быть обязательным в рантайме: установщик скачивает модели заранее в машинный
каталог, а воркер работает `local_files_only=True`. Если модели нет — понятная
ошибка `hf_token_missing`/`model_missing` **до** задания.

**Раскладка снапшотов:** используем **стандартный кэш HF**:
`<models_dir>/hf-home/hub/models--<org>--<model>/snapshots/<rev>/…`.
Воркер **до импорта** `transformers`/`pyannote` выставляет
`HF_HOME=<models_dir>/hf-home` и `HF_HUB_OFFLINE=1`; тогда:
* `Pipeline.from_pretrained(<локальный путь>)` / `AutoModel.from_pretrained(<путь>)`
  работают без сети;
* `GigaAM.transcribe_longform` внутри вызывает
  `snapshot_download("pyannote/segmentation-3.0", local_files_only=True)` и находит
  тот же кэш — патчить GigaAM не нужно.

Путь к `community-1`/`wav2vec2` Rust передаёт абсолютным; repo_id остаётся только
там, где GigaAM жёстко его использует (`segmentation-3.0`).

## 8. Совместимость и контракты

| Контракт | Как сохраняем |
|---|---|
| `.md` с `### SPEAKER_NN [HH:MM:SS - HH:MM:SS]` и `UNASSIGNED` | рендер в Rust по блокам Python; тест на golden-файлах |
| `.orig.md` — текст до присвоения имён | Rust пишет **после** postprocess, но до UI-подстановки; исходный ASR — в `transcript.json` (иначе UI откатит LLM-правки) |
| `.speakers.json` — карта метка→имя | Rust, без изменений |
| `.bak` перед правкой в UI | UI, без изменений |
| UI: список, превью, таблица имён, «Перезапустить задание» | без изменений API `/api/transcribe/*` |
| Демон: lock, очередь, disposal, pause | без изменений |
| Индексация: `out_dir` не индексируется | без изменений |
| `hds transcribe-once` / `transcribe-watch` | CLI без изменений; `--dry-run` показывает новый стек |
| БД `index.db` | схема не меняется |
| Серверная роль `transcribe` | остаётся; Python-воркер — дочерний процесс роли; модели из `HDS_MODELS_DIR`/`%ProgramData%` |

**Длительность:** текущий предел 6 ч (RIFF/WAV, `MAX_DIAR_SEC`). В Python-пути
жёсткого RIFF-ограничения нет, но RAM на 6 ч 16 кГц float32 ≈ 1,4 ГБ + модели;
оставляем `max_duration_hours: 6` по умолчанию, больше — только явным ключом
(и с предупреждением в UI/журнале).

**Имена спикеров:** Python отдаёт стабильные `SPEAKER_00…`, Rust рендерит заголовки;
UI подставляет имена. Если спикеров больше 20 — предупреждение и обрезка
`max_speakers` (не молчаливое «все в один»).

---

## 9. Этапы, объём, приёмка

Оценки — в рабочих днях одного разработчика; включают тесты и документацию, но не
ожидание загрузок моделей.

### P0. Спайк и фиксация решений (4–5 д)

* окружение CUDA: `uv venv`, `torch 2.8 + cu128`, `transformers 4.57.1`,
  `pyannote 4.0`, `whisperx 3.8.7rc1`; проверить, что все ставятся **вместе**
  (главный риск); проверить оба venv (`venv-transcribe`, `venv-transcribe-dml`)
  от портативного CPython;
* окружение DML: `torch 2.8 CPU`, `onnxruntime-directml 1.24.4`, `pyannote 4.0`
  (**без** `onnxruntime` рядом), `venv-transcribe-dml`; экспортировать GigaAM
  `e2e_rnnt` в ONNX (encoder/decoder/joint) и прогнать с `DmlExecutionProvider`;
* 3 файла: 18 / 56 / 77 мин (те же, что в `PLAN_DIARIZATION_NEMOTRON.md`) + 1–2
  коротких с несколькими каналами/перекрытиями;
* замеры: WER/CER (нормализованный), точность сущностей, атрибуция спикеров,
  MAE границ слов, доля `low_confidence/failed`, DER (если есть RTTM), RTF,
  пик VRAM/RAM (CUDA) / GPU/RAM (DML);
* сравнение: `e2e_rnnt` vs `rnnt` vs `e2e_ctc`; `torch` vs `onnx` (CUDA) и `onnx`
  (DML) — качество/RTF; `native` vs `whisperx` vs `hybrid`;
  `transcribe_longform` vs Silero VAD vs без VAD; `decode.channels: auto/downmix/split`;
  community-1 vs текущая Nemotron-3;
* **G0 hotwords:** проверить эффект `hotwords_boost` на `e2e_rnnt` в gigastt (см. §4.2);
* **решения:** вариант ASR/бэкенд, режим alignment, longform/VAD, каналы, нужен ли
  hotword-рантайм, укладывается ли в 12 ГБ, годится ли DML-профиль для AMD;
* артефакт: `tools/parity/out/py_transcribe/spike/REPORT.md` + JSON-замеры.

**Гейт:** все замеры воспроизводимы, версии зафиксированы, пик VRAM ≤ 10 ГБ
(без LLM), RTF на CUDA ≤ 2× текущего; **DML: WER ONNX не хуже PyTorch более чем
на 0,2 п.п., RTF заметно лучше CPU**; G0 имеет однозначный ответ «да/нет».

### P1. MVP на Python + общий пакет (7–9 д)

* `sidecar/hds_sidecar` (stdio/runtime/config/progress/fs) + **перевод
  `hds_extract` на него без изменения поведения** (golden B4 + `worker_live`);
* воркер `hds_transcribe`: `hello/transcribe/cancel/shutdown`;
* `decode.py` (ffprobe каналов), ASR `e2e_rnnt` (вариант — по P0),
  `transcribe_longform`, нативные тайм-коды;
* community-1 (exclusive + regular), независимый `diarization.json`, merge,
  `transcript.json`, JSON-блоки;
* `hds-transcribe` (Rust) + замена `TranscribeClient`; `.md/.orig/.speakers.json`;
* конфиг `transcribe.*`, маппинг старых ключей; `engine: python|legacy`;
* тесты: protocol (общий), merge (unit), golden `.md`, демон end-to-end на 1 файле;
* `hds check`/`transcribe-check` — минимальный набор (модели, ffmpeg, устройство).

**Гейт:** `hds transcribe-once` даёт `.md` того же формата; UI открывает, имена
присваиваются; демон обрабатывает файл и утилизирует исходник; **извлечение
документов не изменилось** (golden/паритет); `cargo test`/`clippy`/`fmt` зелёные.

### P2. Качество alignment, VAD и каналы (4–5 д)

* `align.mode: whisperx|hybrid`, «произнесённая форма»;
* на DML: `native` остаётся default; режим `whisperx` в MVP **не поддерживается**,
  P2 — либо отдельная CPU-установка WhisperX, либо ONNX-wav2vec2 + DML
  (собственный CTC-aligner); сравнить с native;
* `rnnt`-параллель (опционально) + `difflib`-маппинг;
* `display_span`/`word_id`, флаги `aligned/interpolated/low_confidence/failed`,
  `alignment_quality`, `overlap/ambiguous`;
* Silero VAD как отдельный компонент: A/B против `transcribe_longform`, без резки
  речи на границах; `decode.channels: auto/downmix/split` для многоканальных записей;
* метрики P0 повторяются; сравнение с нативным режимом;
* решение: оставить `native` default или переключить на `hybrid`; нужен ли VAD.

**Гейт:** MAE границ ≤ 100 мс или не хуже нативного; доля `low_confidence/failed`
≤ 2 %; атрибуция не падает; на 77-мин файле время ≤ 2× текущего (CUDA) / ≤ 3× (DML).

### P3. Глоссарий и hotwords: G1–G3 (3–4 д)

* `glossary.yaml` (кандидаты/контекст/`allow_global`/`forbid`), детерминированные
  замены; неоднозначное — `needs_review`;
* **G1:** gigastt INT8 `e2e_rnnt` (без hotwords) vs официальный GigaAM — порог §4.2;
* **G2:** gigastt + hotwords vs без — recall/ложные вставки/WER;
* **G3 (опционально):** повторное декодирование коротких подозрительных окон с
  hotwords; принять только при росте recall без WER/ложных вставок;
* `transcribe.hotwords.*`, проверка лимитов 64/64, ошибки;
* тесты: ложные вставки, регистр, границы слов, кириллица/латиница, «один эс» без
  контекста не меняется.

**Гейт:** precision сущностей не ниже baseline; ложные вставки ≤ 1 %; при
отключённом hotwords поведение не меняется; если G1/G2 (и применимая G3) не
пройдены — `hotwords.backend: none` и это зафиксировано в отчёте, а не «включим
на глаз».

### P4. Postprocess: глоссарий + LLM (3–4 д)

* Python `postprocess.py`: `llm_host` (OpenAI-совместимый), промпт «только JSON-edits»;
* валидатор в Python, журнал `edits.json`, лимиты, контекстные правила;
* кэш `transcript.json` → повторный postprocess без ASR;
* Qwen2.5-7B-Instruct Q4_K_M vs Q5_K_M (или текущая роль `shared:chat`);
* тесты: допустимый/запрещённый патч, смена спикера, числа/отрицания, таймауты,
  недоступный `llm_host` (деградация без падения);
* `transcribe.llm.enabled: false` по умолчанию.

**Гейт:** ни один патч не меняет спикера/таймкоды; отклонённые патчи не портят
текст; при выключенном LLM результат идентичен P2/P3; повторный postprocess
не запускает ASR.

### P5. Упаковка и железо (3–5 д)

* `build_transcribe_sidecar.ps1 -Profile cuda|cpu|dml`, предсобранный venv;
* `export_gigaam_onnx.ps1` для DML/CPU, `fetch_transcribe_models.ps1`,
  `setup.ps1`, `install_server.ps1`, `update.ps1`;
* `hds check`/`transcribe-check` полностью (профиль, EP, вендор GPU, VRAM/RTF);
  README/NOTICE/RELEASE_NOTES;
* **CUDA-профиль** (3060 12 ГБ), **DML-профиль** (Radeon/Intel), **CPU-профиль**
  (8 ядер); замеры и отчёт по каждому; DML — обязательная живая проверка на AMD;
* macOS/Metal — best-effort, без блокировки релиза.

**Гейт:** чистая установка на dev-машине: `setup.cmd` → `hds check` → `transcribe-once`
без ручных шагов; повторная установка идемпотентна.

### P6. Legacy и релиз (2–3 д)

* `engine: legacy` сохраняет старую цепочку на один минорный релиз;
* `hds-diarize`/`parakeet-rs`/`ort`/**Rust-DirectML.dll** (legacy Nemotron) — вне
  default-сборки (feature/флаг). Это **не** Python-профиль DirectML: там
  `onnxruntime-directml` в отдельном venv (§4.7), и он остаётся;
* удаление `whisper`-роли из плана `llm_host` в python-режиме;
* релизные заметки, миграция конфигов, откат.

**Итого:** MVP (P0–P1) — **11–14 д**, полный объём — **25–35 д**. Дополнительные
1–2 дня P1 — цена общего пакета и рефакторинга `hds_extract`; 1–2 дня P0/P3 —
ворота hotwords и A/B VAD/каналов. Это окупается тем, что платформенные грабли
(fd/stdin/idle) существуют в одном экземпляре, а сомнительные улучшения не попадают
в боевой путь без чисел.

## 10. Приёмка: метрики и контрольный корпус

**Корпус:** **1–2 часа** (18/56/77 мин = 151 мин) + 2–3 коротких фрагмента
(перекрытия, несколько каналов, телефон, термины 1С/МЧД, фамилии). Референс:
текст + разметка спикеров вручную; для 100 слов — ручные границы.

| Метрика | Как считаем | Порог |
|---|---|---|
| WER/CER | нормализованный текст (без пунктуации/регистра), `jiwer` или собственный скрипт | **не хуже** текущей цепочки; цель — ≤ baseline |
| Точность сущностей | precision/recall/F1 по глоссарию на 1–2 ч | precision ≥ baseline, ложные вставки ≤ 1 % |
| Hotwords G0–G3 | эффект/качество/польза/окна по §4.2 | пройдены применимые ворота, иначе `none` |
| Атрибуция спикеров | доля слов, отнесённых к верному спикеру | **не хуже** 74,4 % на том же материале |
| DER | по RTTM, если есть эталон | не хуже текущего (19,7 % в спайке Nemotron-3) |
| Число спикеров | абсолютная ошибка | 0–1 на файл; без «все в одного» |
| Границы слов | MAE start/end на 100 словах | ≤ **100 мс** для `hybrid`; `native` — зафиксировать |
| Качество alignment | доля `aligned`, `low_confidence`, `failed`, средний score по сегментам | `low_confidence+failed` ≤ **2 %**; нет «точной» разметки у failed |
| Ложная точность | доля слов с таймкодом, но неверным словом (ручная выборка) | не хуже baseline; фиксировать |
| Перекрытия | доля слов с `overlap`, корректность `primary` | не хуже baseline; не «схлопывать» |
| Каналы | split vs downmix на многоканальном файле | split не хуже по WER/атрибуции; иначе auto→downmix |
| LLM-правки | принято/отклонено, ложные замены, изменение смысла | 0 изменений спикера/таймкода; ≤ N/1000 |
| Время | RTF на 18/56/77 мин: CUDA / DML / CPU | CUDA ≤ 2× текущего; DML ≤ 3× и **заметно быстрее CPU**; CPU — зафиксировать |
| VRAM/GPU | CUDA: `torch.cuda.max_memory_allocated` + `nvidia-smi`; DML: память ORT-сессии + Adrenalin/GPU-Z | CUDA ≤ 10 ГБ без LLM; DML acoustic ≤ 4 ГБ; LLM — отдельно |
| DML-паритет | ONNX-DML vs PyTorch-CUDA/CPU: WER, RTF, доля fallback-операторов | WER ≤ +0,2 п.п.; RTF лучше CPU; падений сессий нет |
| RAM/CPU | пик процесса/дерева, средняя загрузка 8 ядер | ≤ 4 ГБ на 6-часовом файле; без 100 % CPU |
| Стабильность | 5 последовательных файлов, рестарт воркера | нет OOM/утечек; повтор детерминирован |
| Контракты | golden `.md`, UI, демон, индексация | 100 % совпадение формата; UI без изменений |

**Матрица сравнения (обязательна):**

| Конфигурация | Что включено |
|---|---|
| A. Текущая цепочка | Rust/движок (whisper.cpp + Nemotron-3) — baseline |
| B. Python-минимум | официальный GigaAM + native timestamps + community-1, без VAD/hotwords/LLM |
| C. Python + качество | B + WhisperX/hybrid + quality-флаги + VAD/каналы (где выигрывают) |
| D. Python + сущности | C + глоссарий + (если G0–G3) hotwords + postprocess/LLM |

Каждую конфигурацию гоняем на одном корпусе и одном diarization-артефакте (кэш §4.4);
фиксируем качество, RTF, VRAM/RAM. «Улучшение» без числа — не улучшение.
B/C/D прогоняем на **CUDA и DML** (если есть AMD): DML сравниваем с CPU (должен быть
быстрее) и с CUDA (WER ≤ +0,2 п.п.); если DML не выигрывает у CPU — профиль не
объявляем поддержанным, а документируем как экспериментальный.

**Обязательные регрессы:**
* `hds transcribe-once` на 3 файлах → `.md` открывается в UI, имена присваиваются,
  `.bak` создаётся;
* `hds transcribe-watch`: файл в inbox → `.md` в out → исходник удалён (disposal);
* `out_dir` не попал в индекс; `index.db` не изменился по схеме;
* при `engine: legacy` старый путь работает без изменений;
* **извлечение документов после перевода на `hds_sidecar`**: golden B4
  (`tools/parity`) и `crates/hds-extract/tests/worker_live.rs` — без расхождений;
  `hello.capabilities`, `extract`, `normalize`, idle-timeout и EOF — как до рефакторинга.

---

## 11. Риски и меры

| # | Риск | Вероятность/эффект | Мера |
|---|---|---|---|
| R1 | Конфликт зависимостей: `whisperx` требует `torch~=2.8`, `transformers>=4.48`, `pyannote>=4.0`; GigaAM рекомендует конкретные версии | высокая / блокирует | отдельное venv, `requirements.lock`, спайк P0 **до** кода; при конфликте — два воркера (ASR и diarization) |
| R2 | Gated-модели (`community-1`, `segmentation-3.0`) без токена | высокая / задание падает | предзагрузка установщиком, `local_files_only`, понятная ошибка `hf_token_missing` |
| R3 | OOM на 12 ГБ (ASR + alignment + diarization) или ошибка DML-сессии | средняя / падение | последовательная загрузка, `batch_size` 4–8, CPU-fallback (`cuda_oom`/`dml_oom`), лимит `max_speakers` |
| R4 | Ожидание Vulkan для AMD | высокая / разочарование | явно в README/плане: ASR/диаризация — CUDA или **DirectML (AMD/Intel)** или CPU; Vulkan — только LLM в `llm_host` (или CPU). Подробности и ограничения — §4.7 |
| R5 | Нормализованный текст не выравнивается wav2vec2; aligner выдаёт «точные» таймкоды неверным словам | высокая / плохая атрибуция | `native` по умолчанию, `hybrid` через произнесённую форму/`rnnt`, карта `display_span`, флаги `low_confidence/failed`, fallback на границы сегмента |
| R6 | Hotwords: нет реального biasing / INT8 хуже fp16 / ложные вставки | высокая / качество | ворота G0–G3 (§4.2): без них `backend: none`; per-meeting глоссарий, `forbid`, кандидаты вместо безусловных замен |
| R7 | LLM «улучшает» текст и теряет смысл | средняя / доверие | только JSON-патчи, валидатор в Python, контекстные правила, лимит правок, журнал, off по умолчанию |
| R18 | VAD/поканальная обработка режет речь или ухудшает WER | средняя / качество | A/B в P0; VAD только в паузах, контекст 1–2 с; `channels: auto` с проверкой каналов; при отсутствии выигрыша — downmix |
| R19 | Кэш `transcript.json`/`diarization.json` устарел после смены модели/файла | средняя / неверный результат | ключ = sha256+size+mtime+duration+ревизия+параметры; инвалидация при смене; `hds check` показывает ключ; кэш можно отключить |
| R8 | Качество ниже текущей цепочки | средняя / приёмка | гейты §10, `engine: legacy`, сравнение на одном корпусе |
| R9 | Время на CPU неприемлемо | средняя / UX | CPU-профиль с ограничением длины/очереди; предупреждение в UI |
| R10 | Размер поставки/время установки | высокая / UX | предсобранный venv, отдельный `-SkipTranscribeStack`, кэш wheel |
| R11 | Лицензии: CC-BY-4.0 требует атрибуции | средняя / юридическая | `NOTICE.md`, ссылка в README, сохранить LICENSE модели |
| R12 | Windows-служба (LocalSystem) не видит HF-кэш пользователя | высокая / сервер | модели в `%ProgramData%`, `HDS_MODELS_DIR`, проверка в `hds check` |
| R13 | Утечки CUDA/ORT-сессий между файлами | средняя / падение | `max_requests`, перезапуск, `empty_cache` на CUDA / закрытие сессий на DML, замер RSS/VRAM в P0 |
| R14 | Модели обновятся и «поедет» качество | средняя / воспроизводимость | ревизии закреплены, автообновления нет |
| R15 | Длинные пути/кириллица (Windows) | низкая / падения | `\\?\` в Rust уже есть; в Python — `pathlib` + staging в ASCII-каталог (как `stage_ascii` сейчас) |
| R16 | `trust_remote_code=True` у GigaAM — исполнение кода с HF | средняя / безопасность | ревизия закреплена, код аудируется при обновлении, модели — локальный snapshot; в серверной поставке — только предзагруженные модели |
| R17 | Рефакторинг `hds_extract` на общий пакет ломает извлечение | средняя / регресс | делать первым коммитом P1, без изменения поведения; golden B4 + `worker_live` + `mock_worker` до и после; при расхождении — откат только этого коммита |
| R20 | Python-воркер держит acoustic-модели, когда вызывает LLM | средняя / OOM | postprocess — отдельный вызов после `del`/`empty_cache`; при `llm.enabled` — проверка `torch.cuda.memory_allocated()==0` (или порог) в stderr |
| R21 | DirectML: неподдерживаемые операторы GigaAM, падение сессии, fp16-несовместимость, `onnxruntime`/`onnxruntime-directml` конфликт, нет wheel под Python 3.12 | высокая / AMD-профиль не работает | P0-спайк на реальном Radeon; `enable_mem_pattern=False`, `SEQUENTIAL`; fp32 fallback; отдельный `venv-transcribe-dml`; фиксация `onnxruntime-directml==1.24.4`; проверить cp312 (иначе 3.11/3.13); если оператор не поддерживается — профиль понижается до CPU, а не «делает вид, что работает» |
| R22 | DirectML в maintenance mode, ORT DML EP — sustained engineering | средняя / долгосрочно | пин версий, изоляция в `hds_transcribe/asr_onnx.py`, тест на каждом релизе; при ухудшении — CPU-профиль; следить за WinML как заменой |

## 12. Откат и сосуществование

**Флаг:** `transcribe.engine: python | legacy` (по умолчанию `python` после P1,
`legacy` — на время приёмки).

* **Код:** старая ветка `llm_host`/`hds-diarize` остаётся в дереве до P6; в
  python-режиме не вызывается, в legacy — работает как раньше.
* **Данные:** схема БД не меняется; `.md`/`.orig`/`.speakers.json` совместимы;
  откат не требует миграции или переиндексации.
* **Модели:** старые (whisper GGML, Nemotron-3) и новые (GigaAM/pyannote/wav2vec2)
  могут лежать одновременно; место — единственная цена.
* **Конфиг:** при откате старые ключи уже на месте; новые игнорируются с
  предупреждением.
* **Пофайловый fallback:** если python-воркер упал с `model_missing`/`cuda_oom`/
  `dml_oom`
  и `transcribe.legacy_fallback: true`, задание **не** перезапускается старым
  путём автоматически (это скрыло бы проблему); пользователь видит ошибку и может
  переключить `engine`. Исключение — `fallback_cpu` внутри Python (та же логика,
  другое устройство).
* **Релиз:** минимум один минорный релиз с обоими путями; удаление legacy — только
  после двух успешных приёмок и отдельного решения владельца.

---

## 13. Открытые вопросы (с предлагаемым ответом)

| # | Вопрос | Предложение по умолчанию |
|---|---|---|
| 1 | Откуда брать HF-token на сервере? | установщик: интерактивно/`-HfToken`; в рантайме не нужен (`local_files_only`) |
| 2 | Кто и на чём делает эталонный корпус 30–60 мин? | владелец выделяет 3 файла и разметку спикеров; разметка текста — после первого прогона с ручной правкой |
| 3 | Известно ли число спикеров на типовых встречах? | по умолчанию `num_speakers: 0`, `min: 1`, `max: 20`; для известных проектов — точное число |
| 4 | Кто ведёт глоссарий? | файл `data/glossary.yaml` в проекте, правки — владельцем; на встречу можно переопределять |
| 5 | Нужен ли LLM-слой по умолчанию? | **нет**, `enabled: false`; включать для отдельных файлов/проектов |
| 6 | Оставлять ли предел 6 ч? | да, по умолчанию; `max_duration_hours` — явный ключ |
| 7 | Допустим ли отдельный `gigastt serve`? | только как эксперимент P3; если не проходит порог — не берём |
| 8 | Приоритет macOS/Metal? | бонус после Windows-приёмки; без блокировки релиза |
| 9 | Сколько релизов держать legacy? | минимум один минорный, затем — решение владельца |
| 10 | Переименовывать `whisper-check`? | да, `transcribe-check`, старый алиас сохранить |
| 11 | Есть ли живая Radeon/Intel для проверки DML? | нужен хотя бы один DX12-совместимый Radeon RX 6000/7000+ (или Intel Arc/iGPU); без него DML-профиль остаётся «код + CPU-проверка» и не объявляется приёмочным |
| 12 | Допустим ли Vulkan **только для LLM** на AMD? | да, это текущий компромисс; полный отказ от Vulkan = LLM на CPU (медленно) либо отдельный LLM-рантайм (WinML/ONNX GenAI/OpenCL) — вне MVP |

## 14. Отчёты Perplexity и ChatGPT: что принято, что скорректировано

**Принято полностью:**
* разделить **акустически привязанный** текст и **текст для чтения**; выравнивать
  произнесённую форму, а не нормализованную;
* WER 8,4 % относится к `rnnt`, а не к `e2e_rnnt`; сравнивать оба режима на своём
  корпусе, а не на всём архиве;
* hotwords — часть ASR-рантайма (`gigastt`), а не Python-постпроцессор; не включать
  общий список брендов, только словарь встречи; проверять ложные вставки;
* pyannote community-1: использовать exclusive-результат и хранить обычный для
  overlap; не фиксировать число спикеров «на глаз»;
* merge: максимальное пересечение + неоднозначность + overlap + паузы + короткие
  реплики; не сглаживать «да/нет/угу»;
* LLM: только проверяемые патчи, не переписывание текста; хранить журнал; не
  разрешать глобальные замены без акустического подтверждения;
* последовательная загрузка GPU-моделей, отдельное окружение/воркеры при конфликте
  зависимостей, фиксация версий/ревизий, маленький контрольный корпус.

**Скорректировано:**
* **Vulkan** не заявляем как backend для ASR/диаризации: у PyTorch/ONNX Runtime
  его нет. Вместо «бонуса» DirectML стал **полноценным профилем для AMD/Intel**
  (GigaAM ONNX); Vulkan остаётся только для LLM в `llm_host` (или CPU). Это
  уточнение заказчика от 22.10.2026 и одновременно ответ Perplexity/ChatGPT;
* **LLM-инференс** — в существующем `llm_host`, но **постобработка/валидатор — в Python**;
  вторая LLM не поднимается, Qwen2.5-7B — опциональная замена роли;
* **WhisperX** не обязателен для MVP: нативные таймкоды GigaAM дешевле; WhisperX —
  режим качества, решение по замеру, а не по вере в «forced alignment»;
* **`gigastt`** не становится основным ASR: INT8 и «pending» бенчмарки несовместимы
  с целью «максимальное качество»; только hotword-рантайм после порога;
* **6 ч** оставляем как текущий предел (Perplexity не рассматривал RAM на длинных
  файлах).

### 14.1 Отчёт ChatGPT (`EXAMINATION_OF_CHOOSEN_STACK_CHATGPT.md`)

**Принято (существенные улучшения):**
* **hotwords — не доказанная настройка GigaAM.** Лексикон `DEFAULT_HOTWORDS` и
  наличие `HotwordOverride` не равны совместимости с декодером; biasing — свойство
  `gigastt`, а не официальной модели. Введены ворота G0–G3 и запрет имитировать
  biasing текстовой заменой (§4.2).
* **Глоссарий — кандидаты, а не замены.** Добавлены `type/context/priority/
  allow_global/pronunciations`, правило «один эс» без контекста не меняется.
* **Alignment — отдельная задача с контролем качества.** Флаги
  `aligned/interpolated/low_confidence/failed`, `alignment_quality`, fallback на
  границы сегмента; WER aligner-модели не является метрикой выравнивания (§4.3).
* **Диаризация — независимый артефакт.** `diarization.json` + кэш; перекрытия
  сохраняются в JSON, не «схлопываются» (§4.4).
* **VAD — отдельный эксперимент**, а не обязательный проход; не резать речь на
  границах; сравнить с `transcribe_longform` (§4.5).
* **Многоканальное аудио** проверять до downmix: раздельные каналы могут быть
  готовой диаризацией (§4.5).
* **LLM-постобработка — Python**, инференс — `llm_host`; Qwen2.5-7B Q4_K_M как
  старт, Q5_K_M для сравнения; не держать LLM во время ASR (§4.6).
* **Матрица A/B/C/D и 1–2 ч корпуса**, четыре группы метрик: WER/CER, DER,
  ошибка таймкодов, RTF; плюс VRAM/CPU (§10).

**Скорректировано/уточнено:**
* «лёгкий специализированный aligner» — только как эксперимент P0, без обещания
  замены XLS-R: конкретной поддерживаемой модели ChatGPT не назвал;
* VAD не становится обязательным: если GigaAM на целых файлах не хуже, оставляем
  `longform` без отдельного VAD;
* hotwords не «включаем, если работает», а **закрываем** при непрохождении G0–G3;
* LLM не получает весь транскрипт и не «улучшает» его — только JSON-патчи по
  подозрительным фрагментам;
* **AMD/DirectML (уточнение заказчика):** ASR — GigaAM ONNX + `onnxruntime-directml`,
  pyannote — CPU, Vulkan — только LLM. Это не «бонус», а отдельный поддерживаемый
  профиль (§4.7); ChatGPT рассматривал только CUDA/CPU, поэтому здесь решение
  принято по первоисточникам ONNX Runtime/DirectML.

---

## 15. Ссылки (проверены 22.10.2026)

| Что | Ссылка |
|---|---|
| GigaAM-v3 (карточка, MIT, варианты) | https://huggingface.co/ai-sage/GigaAM-v3 |
| GigaAM (код, `transcribe_longform`, word timestamps) | https://github.com/salute-developers/GigaAM |
| GigaAM `model.py` / `vad_utils.py` (25 с, `segmentation-3.0`, `HF_TOKEN`) | https://github.com/salute-developers/GigaAM/blob/main/gigaam/model.py · `.../vad_utils.py` |
| gigastt-core (Rust, ONNX, hotwords-лексикон) | https://docs.rs/gigastt-core |
| gigastt (CLI/HTTP/Python, hotwords 64×64) | https://github.com/ekhodzitsky/gigastt · `docs/api.md` · `docs/quickstarts.md` |
| pyannote community-1 (CC-BY-4.0, gated) | https://huggingface.co/pyannote/speaker-diarization-community-1 |
| pyannote.audio 4.x (exclusive, GPU, hook) | https://github.com/pyannote/pyannote-audio |
| pyannote blog community-1 | https://www.pyannote.ai/blog/community-1 |
| WhisperX (3.8.7rc1, требования) | https://github.com/m-bain/whisperX · `pyproject.toml` |
| WhisperX `alignment.py` (`align`, модели `ru`) | https://github.com/m-bain/whisperX/blob/main/whisperx/alignment.py |
| wav2vec2-ru (Apache-2.0, 16 кГц) | https://huggingface.co/jonatasgrosman/wav2vec2-large-xlsr-53-russian |
| Qwen2.5-7B-Instruct (Apache-2.0, 128K) | https://huggingface.co/Qwen/Qwen2.5-7B-Instruct |
| ONNX Runtime DirectML EP (Windows) | https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html |
| `onnxruntime-directml` 1.24.4 | https://pypi.org/project/onnxruntime-directml/ |
| `torch-directml` (почему не используем) | https://pypi.org/project/torch-directml/ |
| DirectML (maintenance mode) | https://github.com/microsoft/DirectML |
| llama.cpp backends (нет DirectML) | https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md |
| GigaAM `onnx_utils.py` (`provider=`) | https://github.com/salute-developers/GigaAM/blob/main/gigaam/onnx_utils.py |
| Отчёт Perplexity (локально) | `C:\Users\Sasha\Downloads\EXAMINATION_OF_CHOOSEN_STACK.txt` |
| Отчёт ChatGPT (локально) | `C:\Users\Sasha\Downloads\EXAMINATION_OF_CHOOSEN_STACK_CHATGPT.md` |
| Текущие контракты проекта | `PLAN_AUTO_TRANSCRIBE.md`, `PLAN_DIARIZATION_NEMOTRON.md`, `PLAN_TRANSCRIBE_SERVICE.md`, `sidecar/README.md` |

---

## 16. Definition of Done

1. P0-отчёт с числами и решением по `variant`/`align.mode`/longform/VAD/каналам/
   hotwords (G0).
2. `sidecar/hds_sidecar` создан; `hds_extract` переведён на него **без изменения
   поведения** (golden B4 + `worker_live` до/после).
3. `sidecar/hds_transcribe` + `crates/hds-transcribe` + переключённый
   `TranscribeClient`; `engine: python` работает end-to-end.
4. В `transcript.json` есть `alignment`-флаги и `alignment_quality`; `failed`-слова
   не получают «точных» таймкодов; merge не двигает спикера по low-confidence.
5. Диаризация — независимый `diarization.json` с кэшем; перекрытия сохранены.
6. Hotwords: применимые G0–G3 пройдены **или** зафиксировано `backend: none`;
   текстовая замена не выдаётся за biasing.
7. `postprocess` в Python: глоссарий + (опционально) `llm_host`; JSON-патчи,
   валидатор, `edits.json`; повторный postprocess не запускает ASR.
8. Все контракты §8 проверены тестами и живым прогоном; UI не менялся.
9. Метрики §10 не хуже текущей цепочки; пик VRAM ≤ 10 ГБ без LLM.
10. Установка с нуля: `setup.cmd` → `hds check` → `transcribe-once` без ручных шагов.
11. `engine: legacy` работает; откат описан и проверен.
12. README/NOTICE/RELEASE_NOTES обновлены; лицензии и HF-token описаны.
13. `cargo test --workspace`, `clippy -D warnings`, `fmt` — зелёные; Python-тесты
    (`pytest`) — зелёные; артефакты замеров — в `tools/parity/out/py_transcribe/`.
14. **DML-профиль проверен на живой Radeon/Intel**: GigaAM ONNX-DML даёт WER ≤ +0,2 п.п.
    к PyTorch, RTF лучше CPU; pyannote работает на CPU; `hds check` показывает
    профиль/EP/вендор; при неподдерживаемом операторе — понятный откат на CPU.
15. Установщик сам выбирает профиль (NVIDIA → cuda, AMD/Intel → dml, нет → cpu);
    в архив кладётся ровно один venv; README описывает, что Vulkan для ASR/диаризации
    не нужен, а для LLM на AMD остаётся Vulkan или CPU.

---

> **Следующий шаг:** P0 — спайк окружения и замеры на 3 файлах, **включая DML-спайк
> на Radeon/Intel**. До его результатов код боевого пути не меняем: риск конфликта
> зависимостей, неверного выбора alignment и неподдержанных операторов DirectML
> слишком велик, чтобы «сначала написать, потом проверить».

