# T0.1 — формат офлайн-вывода движка в режиме `mode: transcript` (диаризация)

**Фича:** `PLAN_AUTO_TRANSCRIBE` (автотранскрибация+диаризация медиа).
**Статус:** ✅ спайк выполнен на реальном движке; формат зафиксирован.
**Дата:** 04.10.2026, RTX 3060 (12 288 МиБ), whisper-large-v3-turbo (`GGML .bin`) +
sortformer `diar_streaming_sortformer_4spk-v2.1.gguf` (449 МБ).
**Артефакты:** `out/spike7_*.log`, `out/spike7_*.md`, `out/spike7_diarization.json`;
проба `crates/hds-llama/src/bin/diar_probe.rs`; запуск — `spike7_diarization.py`.

---

## 1. Зачем и как

План требовал до кода подтвердить, **что именно** отдаёт движок в режиме
`transcript` (он же «транскрибация + диаризация одним вызовом»): метку спикера,
метку неприсвоенной реплики, расширение/имя файла, наличие таймкодов — от этого
зависят §5.3 (клиент) и §5.4 (метки спикеров, сканер без `regex`).

**Метод.** Вызов bridge-audio движка ровно тем же путём, что и боевой `llm-host`:
`llama_server_bridge_audio_transcriptions_raw` с `metadata_json`. Реализовано
разовой пробой `diar_probe` (публичный API `Engine` + `BridgeAudio` + `Bridge`),
metadata:

```json
{
  "mode": "transcript",
  "custom": "auto",
  "whisper_model": "…\\whisper-large-v3-turbo-GGML.bin",
  "whisper_gpu_device": 0,
  "output_dir": "<ASCII-стейджинг %TEMP%\\hds_diar>",
  "audio_source_path": "<ASCII-стейджинг \\input.wav>",
  "diarization_model_path": "…\\diar_streaming_sortformer_4spk-v2.1.gguf",
  "diarization_backend": "sortformer",
  "diarization_feed_ms": 10800001.0,
  "diarization_device": "CUDA0"
}
```

> **Почему не `example-cli.exe`** (как спайки 5–6). Стоковый CLI несовместим с
> патченным рантаймом движка: с нашими DLL — `0xC0000005`, со стоковыми `*.orig` —
> `0xC06D007E` (см. `engine-patch/README.md`; патч меняет раскладки SDK-структур).
> Наша проба собрана против патченных заголовков и работает — это и есть боевой путь.

**Фикстуры:** `test_data/jfk.wav` (11 с, 1 спикер); синтетическая двухспикерная
(склейка EN-jfk + RU-60сек, 71 с); `tools/parity/fixtures/русская_речь_60сек.wav`
(1 спикер); `test_data/speech_2min.wav` (132,7 с, повтор jfk → провоцирует
`UNASSIGNED`).

---

## 2. Результат: точный формат выходного файла

Расширение — **`.md`** (`output.format = "md"`), имя — **`<stem>.md`** (стем
*самого аудио*, как его видит движок). Формат реплик (реальный вывод,
`out/spike7_two.md`):

```md
### SPEAKER_00 [00:00:01 - 00:00:11]
And so, my fellow Americans, ask not what your country can do for you, ask what you can do for your country.

### SPEAKER_01 [00:00:31 - 00:01:12]
рассказывать. Я предприниматель уже с пятилетним стажем, то есть почти пять лет я занимаюсь только интернет-бизнесом. …
```

Правила формата (подтверждены на всех прогонах):

| Свойство | Значение |
|---|---|
| Заголовок реплики | `### <SPEAKER> [<start> - <end>]` |
| Метка спикера | `SPEAKER_NN` — **как отдаёт движок**, нумерация **с нуля** (`SPEAKER_00`) |
| Неприсвоенная реплика | литерал **`UNASSIGNED`** (см. §3) |
| Таймкод | `HH:MM:SS` (часы:минуты:секунды, **без миллисекунд**), разделитель ` - ` |
| Разделитель реплик | пустая строка между блоками |
| Конец файла | перевод строки + пустая строка (`…\n\n`) |
| Порядок реплик | по времени появления; разные спикеры чередуются как в записи |
| Кодировка | UTF-8 |

Формат **совпал** с демо-ассетом движка
(`%APPDATA%…\transcribeoffline\assets\demo\Demo.md`) — он и есть эталон.

---

## 3. `UNASSIGNED` — подтверждён реально

Прогон `speech_2min.wav` (в аудио есть «хвост» без уверенного владельца):

```md
### SPEAKER_00 [00:00:01 - 00:02:12]
And so, my fellow Americans, … (повтор)

### UNASSIGNED [00:02:12 - 00:02:13]
And so, my fellow Americans, ask not what your country can do for you, ask what you can do for your country.
```

⇒ метка `UNASSIGNED` **есть** и приходит тем же форматом заголовка; сканер §5.4
обязан ловить и её. Других диалектов (`UNKNOWN`, `Speaker NN`) движок не отдаёт —
легаси-варианты добавлять не нужно.

**Важно (влияет на §5.3/§5.4):** в том же прогоне массив `speaker_spans` содержал
только `SPEAKER_00` — записи со `speaker="UNASSIGNED"` там **нет**. То есть
авторитетный источник меток — **текст `.md`**, а не `speaker_spans`. Парсить
спикеров надо сканированием текста (как и решено в §5.4 — без `regex`).

---

## 4. JSON-ответ движка (ключевые поля)

```json
{
  "mode": "transcript",
  "custom": "auto",
  "diarization": {
    "enabled": true, "backend": "sortformer", "runtime_backend": "CUDA0",
    "model_path": "…\\diar_streaming_sortformer_4spk-v2.1.gguf",
    "feed_ms": 10800001.0, "speaker_count": "model_default"
  },
  "output": { "path": "…\\input.md", "format": "md", "speaker_turns": true },
  "stats": {
    "num_words": 22, "num_whisper_pieces": 1, "num_segments": 3,
    "num_speaker_spans": 8, "num_speaker_segments": 1, "source_audio_seconds": 11.13
  },
  "speaker_spans": [
    { "speaker_id": 0, "speaker": "SPEAKER_00",
      "start_sec": 0.32, "end_sec": 2.32,
      "start_hms": "00:00:00", "end_hms": "00:00:02" } ],
  "timings_sec": { "total": 2.63, "whisper": 1.86, "diarization": 0.77 }
}
```

Полезно для реализации:

* `output.path` — **абсолютный путь** реально записанного `.md`; его и надо читать
  (`Whisper::transcribe_file` уже так делает);
* `output.speaker_turns = true` — признак диаризованного вывода;
* `stats.num_speaker_spans` / `num_speaker_segments` — счётчики для диагностики;
* `speaker_spans` — **готовые интервалы** (`speaker_id`, `speaker`, `start_sec`,
  `end_sec`) — можно использовать для превью/будущих фич (в §5.4 берём текст, а не
  интервалы);
* `timings_sec` — постадийные замеры.

---

## 5. Жёсткий отказ без sortformer-модели (подтверждение решения §0 п.6)

Если `diarization_model_path`/`diarization_models_dir` не заданы, движок отвечает
`ok=0, status=400`:

```json
{"error":{"code":400,
          "message":"Missing diarization model source. Provide diarization_model_path or diarization_models_dir.",
          "type":"invalid_request_error"}}
```

⇒ «нет модели ⇒ отказ задания» — **поведение движка по умолчанию**, наш код лишь
обязан не подменять его деградацией в `speech` (§12 плана).

---

## 6. Производительность и VRAM (зацепка T0.2)

| Аудио | Длительность | total, с | whisper, с | diarization, с |
|---|---|---|---|---|
| jfk.wav | 11,1 с | 2,29 | 1,90 | 0,39 |
| two_speakers.wav (EN+RU) | 71,0 с | 7,93 | 6,76 | 1,16 |
| русская_речь_60сек.wav | 60,0 с | 5,08 | 3,99 | 1,08 |
| speech_2min.wav (повтор jfk) | 132,7 с | 6,56 | 4,86 | 1,69 |

* Движок укладывается в **≈0,11× реального времени** (71 с → 8 с) при
  whisper-turbo + sortformer на GPU.
* Пиковая VRAM по `nvidia-smi`: база **8 517 МиБ** (резидент `llm-host` с чатом) →
  пик **9 374 МиБ**, т.е. транскрибация+диаризация вместе берут **≈0,86 ГБ** VRAM.
  Это подтверждает: ротация «1 поиск/чат → 2 автотранскрибация → 3 индексация»
  реализуема вытеснением (модели не сосуществуют одновременно).

---

## 7. Выводы для реализации (§5.3–5.4 и далее)

1. **§5.3** — записываем **как есть**: `out_dir/<stem>.<out_format>` = `<stem>.md`,
   содержимое — из `output.path` (ASCII-стейджинг движка), без конвертации.
   Оригинальный стем берём из исходного медиа (движок назовёт файл по своему
   стейджингу — `input.md`), поэтому имя формируем **мы**, а не движок.
2. **§5.4** — сканер токенов: `SPEAKER_<цифры>` + литерал `UNASSIGNED`; рукописный,
   без `regex`; порядок по первому появлению. Регистр — как отдаёт движок
   (`SPEAKER_00`, `UNASSIGNED`) — «нормализовать» токены **не нужно**.
3. **Таймкоды** — в тексте `HH:MM:SS` (без мс), парсить их не требуется (UI/замена
   имён работают с токенами спикеров, а не с временем).
4. **Проверка модели** — валидация «`diarization: true` без sortformer ⇒ отказ»
   дублирует поведение движка; сообщение движка можно пробрасывать в лог заданий.
5. **Пробу `diar_probe` оставляем** как инструмент T0.2/T0.3 и приёмочных тестов
   (единственный способ прогнать диаризацию вне `llm-host`).

## 8. Осталось за спайками (не блокирует T1)

* **T0.2** — замер VRAM/времени «в чистом поле» (без резидента) и на длинных
  файлах; здесь получена только дельта к базовому уровню.
* **T0.3** — поведение `custom`/`diarization_feed_ms` на длинных файлах
  (окно/cut) и связь с `max_media_mb`.
