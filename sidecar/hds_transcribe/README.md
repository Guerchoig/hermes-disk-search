# hds_transcribe — sidecar транскрибации и диаризации (P1, P2)

Отдельный процесс (не метод `hds_extract`): 30-минутная транскрибация не должна
блокировать извлечение документов, а CUDA/DML-контекст не нужен процессу,
который запускается ради каждого PDF. Транспорт и изоляция вывода — общий пакет
`sidecar/hds_sidecar` (`PLAN_PYTHON_SIDECAR_TRANSCRIBE.md` §5).

## Стек (решения P0, подтверждены владельцем)

| Компонент | Решение |
|---|---|
| ASR | GigaAM-v3 **`e2e_rnnt`**, PyTorch fp16 на CUDA; ONNX (EP `cuda\|dml\|cpu`) — для профилей без CUDA |
| Нарезка | **Silero VAD**, окна ≤ 25 с, границы только в паузах; `transcribe_longform` не используется |
| Alignment | **native** (`word_timestamps` GigaAM) — default; **`whisperx`** — выравнивание произнесённой формы по сегментам ASR (wav2vec2 XLSR-53-ru); **`hybrid`** — native + whisperx при расхождении > `align.max_shift_sec` или не-aligned слове (P2) |
| Диаризация | **pyannote community-1 4.0.0**, `exclusive` + `regular`, блоки **15–30 мин** с перекрытием, кэш `diarization.json` |
| Merge | max-overlap; `interpolated/low_confidence/failed` не двигают спикера; `overlap`/`ambiguous` в JSON |
| Hotwords | `none` — ворота G1/G2 P3 не пройдены (числа — в `P3_REPORT.md`, `tools/parity/out/…`, не коммитится) |
| Глоссарий | `glossary.py`: детерминированные замены по словам (`variants` — кандидаты, нужен `context`/`allow_global`/`priority: high`), `edits.json` |
| LLM-постобработка | `enabled: false` (P4) |

Профили: `cuda` — основной; `cpu` — fallback и правильный выбор для iGPU;
`dml` — код-путь/fallback (**не** ускорение: на встроенной AMD iGPU медленнее CPU
в 2,2–2,5×). Профиль задаётся аргументом `--profile` / `HDS_TRANSCRIBE_PROFILE`.

## Протокол (stdio + JSON-RPC 2.0, NDJSON)

| Метод | Что |
|---|---|
| `hello` | `{protocol: 1, python, pid, profile, capabilities, versions, devices}` — без импорта torch |
| `probe` | тяжёлая диагностика: torch/CUDA, ORT EP, ffmpeg, кэши моделей (для `hds transcribe-check`) |
| `transcribe` | полный конвейер → `transcript.json`, блоки, спикеры, `alignment_quality`, `warnings` |
| `postprocess` | читает `transcript.json`, применяет глоссарий (P3) → `<stem>.edits.json` + `needs_review` в транскрипте; ASR не пересчитывается; LLM — P4 |
| `cancel` | ставит флаг отмены для активного `job_id` (второй способ — файл `stop_file`) |
| `shutdown` | штатный выход; EOF на stdin делает то же |

**Параметры приходят из Rust** — `config.yaml` воркер не читает (§5.3). Все пути
(`asr.cache_dir`, `asr.onnx_dir`, `diarization.model`, `ffmpeg/ffprobe`) —
абсолютные или из переменных окружения поставки.

Прогресс — только в stderr: `[stage] asr 42% chunks=17 of=42`. В stdout —
исключительно JSON-RPC (иначе `parse_response` по `id` сломается).

Ошибки: числовой JSON-RPC `code` + машинный `kind` (`model_missing`,
`hf_token_missing`, `ffmpeg_missing`, `cuda_oom`, `dml_oom`, `onnx_error`,
`diarization_failed`, `cancelled`, `bad_audio`, `asr_failed`, `align_failed`,
`bad_request`).
При `fallback_cpu: true` стадия один раз повторяется на CPU после
`cuda_oom`/`dml_oom`/`onnx_error`.

`hello.capabilities` содержит `align:native`, `quality` и — если в venv есть
whisperx — `align:whisperx`. `align.mode: whisperx` без пакета даёт `align_failed`
(явный запрос не подменяется молча), `hybrid` — warning и нативные таймкоды.

## Артефакты

* `state_dir/<stem>.transcript.json` — блоки, слова с флагами (`aligned`,
  `interpolated`, `low_confidence`, `failed`), `alignment_reason`
  (`short_duration`/`timing_overlap`/`gap`/`empty`/`no_timestamps`/`non_monotonic`),
  `alignment_source` (`whisperx` у слов, взятых из aligner'а), `display_span`
  (смещения слова в тексте сегмента), `alignment_quality`, параметры,
  пути/план каналов; нужен для повторного `postprocess` без ASR;

Диагностика флагов **без UI** (решение владельца 22.10.2026):
`hds transcribe-check` печатает настройки alignment с порогами и агрегат
последних транскрипций из `transcribe.state_dir` (`mode`, `aligned_ratio`,
`low/failed`, `reasons`, `rtf_total`, `vram`), то же — в `--json`
(`alignment`, `transcripts[]`); в журнал задания (`transcribe-worker.log`)
пишется `[stage] done … mode=… aligned=… low_conf=… reasons=…`.
Слова с `alignment_reason`/`aligner_score`/`display_span` остаются в
`transcript.json` (word-level UI — отдельная задача после P5).

`alignment_quality` (P2): `aligned_ratio`, `low_confidence_ratio`,
`failed_ratio`, `unreliable_ratio` (сумма двух последних — гейт ≤ 2 %),
`max_gap_sec` считается **внутри фразы** (блоки режутся по паузе > 0,8 с, поэтому
паузы между чанками VAD больше не дают 21 с), `gap_flags`, `reasons` (из чего
сложились флаги) и `mode_requested`/`mode_used`/`max_shift_sec`/`whisperx_share`/
`mean_shift_ms`. `stats` дополнен `align_sec` и `rtf_total` (ASR+align+диаризация);
`rtf` оставлен как в P1 для сравнимости.
* `state_dir/<stem>.diarization.json` — независимый артефакт: `exclusive`,
  `regular`, спикеры, блоки, ключ кэша, ошибка (если была). Пересчитывается
  только при изменении файла/модели/числа спикеров; ASR/глоссарий на ключ не
  влияют.

Ключ кэша: размер + mtime + первые/последние 1 МБ файла + модель + число
спикеров + размер блока. Полный sha256 длинного видео не считаем — стоимость
не оправдана (документировано в `cache.py`).

`.md` рендерит **Rust**: Python отдаёт структуру, формат
`### SPEAKER_NN [HH:MM:SS - HH:MM:SS]` и литерал `UNASSIGNED` остаются в Rust
(контракт §8).

## Запуск вручную (разработка)

```powershell
# venv P0 (CUDA/DML) уже собраны; профиль обязан совпадать с venv
'{"jsonrpc":"2.0","id":1,"method":"hello","params":{}}' |
  D:\hds-py-transcribe\venv-cuda\Scripts\python.exe sidecar\hds_transcribe\worker.py --profile cuda
```

Установка окружения — `requirements.lock` (CUDA/CPU) и `requirements-dml.lock`
(DML: только `onnxruntime-directml`, `gigaam` — `--no-deps`).

## Грабли, учтённые в коде

* **torchcodec 0.7.0 + FFmpeg 9** — не грузится; аудио декодирует ffmpeg CLI в
  массив, pyannote получает `{"waveform": tensor, "sample_rate": 16000}`;
* **pyannote 4.0 + torch 2.8** — `torch.serialization.add_safe_globals([...])`
  до `Pipeline.from_pretrained` (иначе `UnpicklingError: Specifications`);
* **gated HF-модели** — нужны принятые условия; при `local_files_only` модель
  обязана быть в кэше (`hf_token_missing` с подсказкой до старта задания), в
  рантайме сеть выключена (`HF_HUB_OFFLINE`);
* **DML** — `ORT_SEQUENTIAL`, `enable_mem_pattern=False`, `intra_op=1`,
  закреплённый `device_id`; фактический EP пишется в прогресс;
* **pyannote: батчи 4/4 обязательны.** `segmentation_batch_size` и
  `embedding_batch_size` ≥ 8 у community-1 аллоцируют ~9–10 ГБ VRAM транзиентно
  **даже на 60-секундном файле** (P1-замер: batch 4 — 0,24 ГБ и быстрее, batch 8 —
  8,9 ГБ). Свойства выставляются **после** `pipeline.to(device)`: перенос
  устройства их сбрасывает;
* **длинные файлы: блоки + глобальная сшивка голосов.** Локальные метки pyannote в
  каждом блоке несогласованы (на 77 мин выходило 16–17 «спикеров»). Сшивка: по
  эмбеддингам `pipeline._embedding` (центроиды на спикера блока) + union-find по
  косинусу `EMBED_CLUSTER_MIN = 0.6`. Итог на 77 мин: **10 спикеров** против 9 у
  whole-file P0, VRAM 5,8 ГБ, диаризация 149 с (RTF 0,032);
* **VRAM мерим своим сэмплером** (`_VramSampler`, опрос `memory_allocated()` раз
  в 200 мс): pyannote сбрасывает `torch.cuda.max_memory_allocated()` внутри
  пайплайна, и пик ASR терялся (0,7 ГБ вместо 6,8 ГБ);
* **CPU** — батчи и `gc.collect()` после каждого (fp32 на 18 мин дал RSS 15,4 ГБ);
* **каналы** — `auto` проверяет корреляцию L/R: дубль-стерео → downmix,
  реально разные каналы → поканально (канал = спикер, pyannote не нужен);
* **WhisperX — только «произнесённая форма»** (`spoken.py`): у `e2e_rnnt`
  текст нормализован («1С», «25 %», «МЧД», «Excel»), а словарь wav2vec2 —
  строчная кириллица. Без конверсии CTC «плывёт» и теряет именно эти слова.
  Таймкод переносится только при полном совпадении токенов (`difflib`), иначе
  слово остаётся нативным — наследования по позиции нет;
* **`low_confidence` не «залипает»** (P2.1): `prev_end` обновляется для каждого
  слова с конечным `end`; в P1 один разрыв > 1,5 с помечал весь остаток чанка
  (270 слов = 11,3 % на `meet0924`);
* **`timing_overlap` ≠ перекрытая речь.** Причина `timing_overlap` — конфликт
  таймкодов двух слов (допуск `align.quality.overlap_tolerance_sec`, 0,12 с =
  3 кадра); перекрытие речи двух спикеров — это `Word.overlap` из `merge`
  (пересечение с turns ≥ 2 спикеров). Имена разные намеренно.

## Не входит в P2

LLM-патчи (P4), установщики/упаковка (P5), удаление legacy-цепочки (P6).
`decode.channels: split` проверен на **синтетической** двухдорожечной
записи (код-путь, не приёмка — реальных таких записей в корпусе нет).

## P3: глоссарий и ворота hotwords

* `glossary.py` — загрузка/валидация `data/glossary.yaml` (пример формата —
  `glossary.example.yaml` рядом с модулем), нормализация (регистр, `ё/е`, `1C`/`1С`,
  пунктуация по краям), окно контекста ±6 слов, приоритеты, `allow_global`,
  `forbid`, `case_sensitive`; `variants` — **кандидаты**, неоднозначное уходит
  в `needs_review`;
* `postprocess` меняет только `words[].text` и пересобирает `block.text`
  (`merge.join_words`); таймкоды/спикеры/`alignment`/`word_id` не трогаются,
  правки — `<stem>.edits.json`; `needs_review` дублируется в `transcript.json`
  (метка на слове + секция `postprocess`), текст ASR в файле не переписывается;
* пустой/отсутствующий глоссарий → результат идентичен P2;
* hotwords: biasing у gigastt (G0 сработал), но G1 (recall сущностей INT8 ниже
  fp16) не пройден ⇒ `hotwords.backend: none`; числа и команды —
  `tools/parity/out/py_transcribe/P3_REPORT.md`.

## Эксплуатационные детали (P4-фиксы по ревью)

* **Один воркер на процесс-владелец.** Rust запускает `worker.py` лениво и
  переиспользует его между файлами: после стадии модели выгружаются, процесс
  остаётся. `idle_unload_sec`/`max_requests` действуют на уровне транспорта, а
  не на каждый файл.
* **`cancel` работает во время `transcribe`.** Запрос обрабатывается в
  reader-потоке (`hds_sidecar.runtime.serve(..., control=...)`) и ставит флаг
  отмены; последовательный `handle` до очереди во время долгой стадии не дойдёт.
  Второй путь — файл `transcribe.stop` (только демон; `transcribe-once` его не
  слушает, иначе оставшийся файл отменял бы задание).
* **Модели и токен:** `transcribe.models_dir` → env воркера `HDS_MODELS_DIR`,
  HF-кэш — `<models_dir>/hf-home` (если `HF_HOME` не задан вручную);
  `transcribe.hf_token_env` задаёт имя переменной с токеном.
* **Ревизия ASR:** `transcribe.asr.revision` — локальный `.ckpt` или каталог с
  `<variant>.ckpt` (фиксация весов); HF-revision к этому рантайму не применим.
* **Частота:** декодирование всегда 16 кГц; `decode.sample_rate` с другим
  значением игнорируется с notice — ASR/VAD/диаризация на другой частоте не
  поддерживаются.
* **ONNX (DML/CPU) без word timestamps:** слова получают `interpolated`, но при
  полном отсутствии `aligned`-слов merge атрибутирует их по пересечению
  ASR-сегмента с turns (`speaker_source="segment"`), а не отдаёт всё в
  `UNASSIGNED`.
* **Заголовки `.md`:** Rust рисует их только при `diarization.status = ok|split`
  из результата воркера; `disabled/failed` → текст без меток (не «### UNASSIGNED»).
* **WhisperX** остаётся опциональным: в `requirements.lock` он закомментирован
  (упаковка — P5). Для `align.mode: hybrid` установите `whisperx` вручную, иначе
  будет warning и нативные таймкоды; явный `whisperx` без пакета — `align_failed`.

## Тесты

```powershell
python -m pytest sidecar\tests -q       # или: python -m unittest discover sidecar\tests
```

Покрыто без моделей: разбор параметров, merge/quality, план блоков диаризации,
склейка меток между блоками, корреляция каналов, протокол (runtime/stdio).