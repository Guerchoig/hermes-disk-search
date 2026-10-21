# PLAN_DIARIZATION_NEMOTRON.md — диаризация Nemotron-3 (Sortformer v3) вместо sortformer

> Продолжает `DIARIZATION_NEMOTRON_HANDOFF.md` (решения владельца 20.10.2026) и спайк
> `tools/parity/out/diar_spike/REPORT.md` (числа, §11 — галлюцинации whisper). Дополняет
> `PLAN_AUTO_TRANSCRIBE.md` §5 (диаризация, формат `.md`, sidecar) и `PLAN_SERVER_MULTIUSER.md`
> (роль `transcribe`); контракты `PLAN_AUTO_TRANSCRIBE.md` §5 и формат вывода **не меняются**.
> **Статус:** в работе с 21.10.2026; **W0–W9 закрыты по коду и документации** (ветка
> `server-multiuser`; тег `v0.21.0` — после решения владельца по RSS: на 77 мин 1.56 ГБ против
> порога 1.5 ГБ, пик даёт движок whisper, а не диаризация — см. W7).
> **Уточнение владельца 21.10.2026 (W4):** backend диаризации выбирается телом
> `diarization_backend` → конфигом `diarization.backend` → авто (модель+sha256 ⇒ `nemotron3`, иначе
> `sortformer`); `auto_transcribe.diarization_backend` переключаем на `nemotron3` в W5.
> **Согласовано владельцем 21.10.2026:** ответы на 5 открытых вопросов (§7) и **вариант A**
> обрезки тишины — диаризация идёт по **полному** WAV (как в спайке), обрезка — только для ASR
> (§3.4, §6). ASR остаётся прежним (whisper-large-v3-turbo через bridge движка), gigastt **не**
> берём; sortformer — рабочий fallback до отдельного решения (§7 п. 5).

## 1. Вводные (факты из кода и спайка, а не предположения)

| Факт | Где |
|---|---|
| Диаризация сегодня делает **только движок**: `mode: transcript` + `DiarizationParams{model_path, backend, feed_ms, device}`; ключи metadata `diarization_model_path`/`diarization_backend`/`diarization_feed_ms`/`diarization_device` | `crates/hds-llama/src/whisper.rs` (`DiarizationParams` 45–73, `build_metadata` 260–291) |
| **`whisper_language` в metadata не передаётся вообще**; читается только Python-путём (`hds/extract_av.py`), Rust-путь язык не знает | `whisper.rs::build_metadata`; `crates/hds-index/src/transcribe.rs::from_config` (138–166 — только `index.*` без языка) |
| Настройки диаризации автотранскрибации: `auto_transcribe.diarization`, `.diarization_model`, `.diarization_backend`, `.diarization_feed_ms`, `.diarization_required` (по умолчанию `enabled: true`, `required: true`, `feed_ms: 10800001`) | `crates/hds-index/src/transcribe.rs::DiarizationSettings` (60–83), `from_auto_transcribe` (174–231) |
| Без sortformer-модели задание **падает** (жёсткий отказ, деградации в `speech` нет) | `crates/hds-llama/src/host.rs` 2325–2346 |
| Текст `.md` пишет **не движок**, а `hds-index`: host возвращает `text`+`out_ext`, `transcribe_to_file` кладёт их в `out_dir/<stem>.<out_format>` и создаёт `.orig.md`/`.speakers.json` | `crates/hds-index/src/transcribe.rs` (417+), `crates/hds-llama/src/host.rs` 2397–2410 |
| Для sortformer нужен **CUDA-резерв**: `transcribe_need_mib` + `prepare_demand(TRANSCRIBE_ROLE)` | `host.rs` 2379–2385 |
| Воркспейс: `rust-version = "1.80"`, `edition = "2021"`, члены `crates/*` (крейта `hds-diarize` нет) | `Cargo.toml` |
| **Единственный ORT-потребитель — `hds-clip`**: `ort = "2.0.0-rc.10"`; `hds` (CLI) зависит от `hds-clip`, а `llm_host` живёт в `hds-llama` (без `hds-clip`) ⇒ в бинаре `llm_host` ORT-окружение будет **одно** | `crates/hds-clip/Cargo.toml`, `crates/hds-cli/Cargo.toml`, `crates/hds-llama/Cargo.toml` (`[[bin]] llm_host`) |
| `ffmpeg` ищется по `index.ffmpeg_cmd` → `PATH` → стандартные каталоги (в Rust-медиа-пути сейчас декодирует движок) | `crates/hds-index/src/diag.rs` 726+ |
| Формат вывода зафиксирован: `### SPEAKER_NN [HH:MM:SS - HH:MM:SS]`, литерал `UNASSIGNED`, sidecar `<stem>.speakers.json`, `.orig.md`, служебные файлы в `auto_transcribe.state_dir` | `PLAN_AUTO_TRANSCRIBE.md` §5.3, `crates/hds-ui/src/transcribe.rs`, `tools/parity/T0_1_DIARIZATION_FORMAT.md` |
| Модель sortformer ставит `installers/fetch_diarization_model.ps1` (471 107 712 байт, проверка только по размеру) в `%APPDATA%\OpenResearchTools\models\openresearchtools__diar_streaming_sortformer_4spk-v2.1-gguf` | `installers/fetch_diarization_model.ps1` |
| API `parakeet-rs`, проверенный спайком: `ort::init().build()` → `ExecutionConfig::new().with_environment(env).with_execution_provider(Cpu|DirectML)` → `Sortformer::with_config(model, Some(exec), DiarizationConfig::custom(0.5, 0.5))` → `diarize(samples_f32, rate, channels)` → сегменты `{speaker_id, start, end}` **в сэмплах** | `tools/parity/out/diar_spike/crate/src/main.rs` |
| Модель Nemotron-3: `altunenes/parakeet-rs` → `nemotron-3-diarization/nemotron3_diar_v3.onnx`, size **400 506 656**, sha256 **`915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a`**, веса — OpenMDW 1.1 | `DIARIZATION_NEMOTRON_HANDOFF.md` (Конфиг, W6) |

**Числа спайка (воспроизведены на `D:\diar-spike-data\*.wav`, см. `REPORT.md` §3–§7):**

| Метрика | sortformer 4spk-v2.1 (движок) | Nemotron-3 (parakeet-rs) |
|---|---|---|
| спикеров на 18-мин файле / на 77-мин | 4 (потолок) / 4 (потолок) | **5** / **8** (5 из них ≥5 % времени) |
| атрибуция реплик: все 98 / без филлеров (78) | 56.1 % / **71.8 %** | **58.2 % / 74.4 %** |
| DER~ без FA / purity / F1 смен | **14.9 %** / 0.937 / 0.403 | 19.7 % / **0.944** / **0.431** |
| CPU RTFx / пик RSS | ≈100 (CUDA, движок) | 25.3 / 684 МБ (18 мин); 23.9 → 2073 МБ (77 мин, `diarize()` целиком) |
| DirectML (iGPU) | — | 114.6 RTFx, побитово = CPU, 494 МБ |

**Обрезка тишины — измерения 21.10.2026 (кадр 30 мс, шумовой пол p10 = −75.0 dBFS):**

| Файл | Ведущая тишина @ −50 dBFS | Первая «устойчивая» речь | Хвост |
|---|---|---|---|
| `meet0924.wav` (18 мин) | 13.38 с | 20.2 с (приветствие 16.0–20.0 с, пик **−34.8 dBFS**) | 0.23 с |
| `meet0512.wav` (77 мин) | 8.6 с | 8.2 с | — |
| `meet0514.wav` (56 мин) | 0.0 с (тишины нет) | 0.0 с | 0.05 с |

* Порог −40/−35 dBFS сдвигает точку обрезки на **16.7 с**, то есть **режет внутрь** первой реплики
  «Добрый день» ⇒ порог только `-50 dBFS` (запас 25 дБ над шумом).
* Обе модели и так не выдают ни одного turn'а в 0–35 с (первый turn: Nemotron 35.95 с, sortformer
  35.76 с) ⇒ на этом материале обрезка не меняет разметку — только сдвигает таймкоды.
* 13.38 с — **одиночный кадр шума**: правило «устойчивой речи» (окно 200 мс, доля ≥50 %) даёт точку
  16.58 с, минус lead-in 0.3 с ⇒ **16.28 с** (замерено `diar_wav_probe`, W0). Тихая первая реплика
  остаётся: в срезе первые 4 с содержат речь (12 % кадров, пик −31.9 dBFS).
* Некомпенсированный сдвиг на реальную величину обрезки (16.28 с) роняет приёмочную атрибуцию
  **74.4 % → 52.6 %** (эталон без филлеров; для 13.4 с было бы 57.7 %) ⇒ компенсация offset —
  обязательный инвариант (§3.4).

## 2. Область

**Входит (W0–W9):**
- W0: единый проход декодирования (ffmpeg-сайдкар) + обрезка ведущей тишины **для ASR** (вариант A);
- W1: `whisper_language` в metadata движка + `index.whisper_language` в Rust-пути + энергетический
  постфильтр ASR-сегментов (галлюцинации на тишине);
- W2: `ort 2.0.0-rc.15` на весь воркспейс (MSRV 1.92), унификация с `hds-clip`, один
  ORT-environment на процесс;
- W3: крейт `hds-diarize` (Nemotron-3 через `parakeet-rs`: EP по платформе, чанкинг `feed()`/`flush()`);
- W4: интеграция в `llm_host::internal_transcribe`, сшивка сегментов с turns, форматтеры `.md`/`.srt`;
- W5: конфиг `diarization.*`, `hds check`, `hds whisper-check`, образцы конфигов;
- W6: установщик модели (`fetch_nemotron3_model.ps1`, sha256), `setup.ps1`, `install_server.*`,
  `NOTICE.md` (OpenMDW 1.1 / MIT / Apache-2.0);
- W7: тесты и живая приёмка (18/56/77 мин, эталон, регресс sortformer, паритеты индексации);
- W8: macOS best-effort (CoreML EP);
- W9: документация и релиз.
- Зависимости: **только** `parakeet-rs` (+ его транзитивные `ort`/`ndarray`), как разрешено решением
  владельца (handoff, «Решения владельца» п. 7); всё остальное (`sha2`, `serde_json`, `serde_yaml`) уже
  в `[workspace.dependencies]`.

**Не входит:**
- удаление sortformer, `installers/fetch_diarization_model.ps1`, `auto_transcribe.diarization*`
  (отдельное решение после приёмки, §7 п. 5);
- собственный экспорт ONNX из `.nemo` (§7 п. 4);
- VAD **внутри** движка whisper (`no_speech_threshold`/`suppress_blank` не подтверждены) — отдельный
  трек, если понадобится;
- WER-оценка ASR (эталон шумный, см. `REPORT.md` §11.3);
- анонимизация; арбитр VRAM для диаризации (Nemotron не занимает VRAM движка);
- изменения UI «Транскрибация» (`crates/hds-ui/src/transcribe.rs`) и формата `.md`.

## 3. Контракты (код — источник истины)

### 3.1. `POST /internal/transcribe` — **не меняется**

Тело: `path, mode, custom, model, gpu, diarization, diarization_model, diarization_backend,
diarization_feed_ms, diarization_device, return_text`. Ответ: `path, mode, custom, out_ext,
diarization{enabled}, segments[{text,t_start,t_end}], stats, text` — те же ключи.

Бэкенд выбирает **хост** по конфигу (`diarization.backend`), а не клиент:

| `diarization.backend` | `diarization: true` | Что делает хост |
|---|---|---|
| `sortformer` | да | **прежний путь без изменений**: `mode: transcript` + metadata `diarization*` (регресс) |
| `nemotron3` | да | (1) ffmpeg → WAV 16к mono **один раз**; (2) ASR по **срезу** WAV (`mode: subtitle` внутри — нужны таймкоды); (3) диаризация по **полному** WAV; (4) `+trim_offset` к таймкодам ASR; (5) сшивка; (6) `render_md` → `text` + `out_ext: "md"`; `stats` дополняется |
| `off` | да | `diarization{enabled: false}`, обычный ASR (как `diarization: false`) |

Добавления (аддитивные, обратно совместимые): необязательное поле тела `language` (переопределяет
`index.whisper_language` на один запрос) и ключи `stats.num_speakers`,
`stats.timings_sec{decode, asr, diar, total}`, `stats.filtered_segments`. Старые клиенты
(`hds-index`, UI) о них не знают и работают как раньше: текст `.md` по-прежнему приходит в `text`.

### 3.2. Язык ASR — только ключ `whisper_language`

`build_metadata` кладёт `"whisper_language": "<lang>"` **только** если язык задан и не пуст (ключи
`language`/`whisper_lang` движок игнорирует — подтверждено спайком §8). Источник: тело `language` →
`index.whisper_language` → дефолт `ru` (§7 п. 1). Пустая строка = авто-детекция (осознанный выбор).
`whisper_word_time_offset_sec: 0.73` остаётся как есть и **не** используется для компенсации trim.

### 3.3. Формат вывода — как сейчас

`### SPEAKER_NN [HH:MM:SS - HH:MM:SS]` (время абсолютное, от начала исходного файла), литерал
`UNASSIGNED` для реплик без turns, порядок по времени; в SRT — `HH:MM:SS,mmm` и та же метка
префиксом. `SPEAKER_NN` = `SPEAKER_{id:02}`. Sidecar `.speakers.json`, `.orig.md`, `.bak` и
переименование имён в UI делает `hds-index`/`hds-ui` — здесь ничего не меняется.

### 3.4. Инварианты таймлайна (вариант A)

1. **Единая шкала — абсолютные времена исходного медиа.** Эталон — полный WAV после единственного
   декодирования.
2. **Диаризация всегда по полному WAV.** Она не видит ни обрезки, ни её последствий ⇒ метрики спайка
   воспроизводимы 1:1, границы блоков `feed()`/`flush()` не смещаются.
3. **Обрезка — только вход ASR**: PCM-срез полного WAV (копия сэмплов, **второго декодирования нет**);
   ведущая тишина, порог `-50 dBFS`, кадр 20 мс, требование ≥200 мс речи над порогом, lead-in 0.3 с.
   Внутренние паузы **не режутся** (`stop_periods` не используется), хвост не режется (эффекта нет:
   0.05–0.23 с).
4. **Offset применяется ровно один раз** — к таймкодам сегментов ASR, до сшивки. `turns`, `.md`,
   `.srt`, `segments[]` ответа и `.speakers.json` — сразу в абсолютной шкале. Тест-инвариант:
   `merge(shift(segments, +off), turns) == merge(segments_abs, turns)`.
5. **Энергетический постфильтр ASR** (лечит галлюцинации в паузах): сегмент отбрасывается, если в его
   окне `[t_start, t_end]` доля кадров выше `-50 dBFS` < 5 % **и** суммарная речь < 150 мс. Считается
   по **полному** WAV (та же шкала), диаризацию не касается; отброшенное — в лог и
   `stats.filtered_segments`. Порог «тихие, но настоящие» реплики не режет: в эталоне 9 реплик с
   речью −17…−19 дБ (спайк §11.2) должны пройти.

### 3.5. Конфиг (новые ключи)

```yaml
diarization:                 # новая секция; auto_transcribe.diarization* остаются для sortformer
  backend: nemotron3         # nemotron3 | sortformer | off
  required: true             # нет модели/сбой ⇒ задание падает (как сейчас)
  nemotron3:
    model: ''                # '' → <models>/nemotron3/nemotron3_diar_v3.onnx
    device: auto             # auto | cpu | directml | coreml
    chunk_min: 10            # блок feed()/flush() (память ~25 МБ/мин при diarize() целиком)
    min_audio_min: 2         # короче — CPU даже при device: auto (оверхед DirectML)
    onset: 0.5               # DiarizationConfig::custom(onset, offset)
    offset: 0.5
  asr:                       # обрезка/фильтр входа ASR (вариант A)
    trim_leading_silence: true
    silence_threshold_db: -50
    trim_lead_in_sec: 0.3
    drop_silent_segments: true
index:
  whisper_language: ru       # "" = авто; Rust-путь читает с W1
```

Валидация (`hds check`, при старте роли): `backend` из списка; файл модели существует и проходит
sha256 (кэш результата, чтобы не хешировать 400 МБ на каждом старте); `device` допустим для платформы
(`coreml` только macOS, `directml` только Windows); `chunk_min ∈ 2..=30`.

## 4. Треки W0–W9

> Порядок: W0 → W1 (можно параллельно) → W2 → W3 → W4 → W5 → W6 → W7 → W8 → W9.
> Каждый трек закрывается **юнитами + живым прогоном** (`cline-rules/dev-machine.md`: процессы
> останавливаем/перезапускаем сами). Артефакты прогонов — `tools/parity/out/diar_nemotron/` (гитигнор).

### W0. Декодер и обрезка тишины (0.5 дня) — ✅ **выполнено** (21.10.2026)

- [x] **Скелет `crates/hds-diarize`** (член воркспейса) с модулями `wav.rs`/`vad.rs`/`decode.rs` — чтобы
      PCM-утилиты не плодились в `hds-llama` и были тестируемы без ONNX (W3 добавляет `sortformer.rs`).
- [x] `hds-core::tools::{which, find_ffmpeg}` — единый поиск (`index.ffmpeg_cmd` → `PATH` → стандартные
      каталоги); `hds-index::diag::which/ffmpeg_exe` и `hds-index::transcribe::which` переведены на него
      (поведение и порядок поиска не изменились, дубли удалены).
- [x] `hds-diarize::wav`: разбор RIFF (pcm_s16le, 16 кГц, mono; `hound` **не** подключён),
      `encode_wav`/`write_wav`/`slice_from`, `Wav::to_f32` (вход Sortformer).
- [x] `hds-diarize::vad::{leading_silence_samples, speech_stats, TrimConfig}` — чистые функции, offset
      в сэмплах; правило «окно 200 мс, доля кадров выше −50 dBFS ≥ 50 %», ошибка — в сторону сохранения
      речи (до 100 мс раньше атаки).
- [x] `hds-diarize::decode_to_wav(ffmpeg, src, dst)` — `ffmpeg -nostdin -y -v error -i … -vn -ac 1
      -ar 16000 -c:a pcm_s16le`, `CREATE_NO_WINDOW` на Windows, хвост stderr в ошибке.
- [x] Тесты (13 в крейте): тишина −80 dBFS (режется целиком, offset = длина), тихий тон −36 dBFS (не
      режется — регресс ловушки «Добрый день»), шум −75 dBFS + речь, одиночный выброс 40 мс не
      останавливает обрезку, нет тишины (offset 0), WAV roundtrip/битые заголовки, декод WAV через
      `ffmpeg` (тест пропускается, если ffmpeg нет в `PATH`).
- [x] Проба `diar_wav_probe` (bin крейта): медиа/WAV → trim offset, доля речи, срез, JSON.
- [ ] `hds-llama` использует декодер в `internal_transcribe` (полный WAV → диаризация, срез → ASR) —
      **входит в W4**; на W0 проверено пробой.
- [x] Живой прогон: 77-мин mp4 → WAV **2.49 с** (< 30 с); mp4(AAC) vs mp3 той же встречи — **lag +0 мс**
      (`compare_pcm.py`: 17 219 920 / 17 219 925 сэмплов, mean|diff| 96 LSB — как в спайке).

**Результаты W0 (живой прогон 21.10.2026, `target\release\diar_wav_probe.exe`; исходники —
`D:\САША\ИРИС\ИИ\Тестовые записи совещаний\Новый каталог`):**

| Источник | Длительность | Декод | trim offset | Доля речи | Примечание |
|---|---|---|---|---|---|
| `…0924…статус.mp4` (18 мин) | 1076.24 с | 0.67 с | **16.280 с** | 81.2 % | — |
| `…0924…mn7i8.mp3` (та же встреча) | 1076.25 с | 0.80 с | 16.280 с | 81.0 % | lag 0 мс к mp4 |
| `2026_0512_ERP.STS…mp4` (77 мин) | 4619.99 с | **2.49 с** | 8.220 с | 73.2 % | критерий < 30 с |
| `2026_0514_ERP.SPS…mp4` (56 мин) | 3363.11 с | — | **0.000 с** | 79.6 % | ведущей тишины нет (no-op) |
| `meet0924.wav` + срез | 1076.24 / 1059.96 с | — | 16.280 с | 82.5 % после среза | первая реплика в срезе: 12 % кадров, пик −31.9 dBFS |

Артефакты: `tools/parity/out/diar_nemotron/w0_*.json`, `w0_0924_mp4.wav`, `w0_0924_mp3.wav`,
`w0_meet0924_trim.wav`.

**Приёмка W0 — ✅ выполнена (21.10.2026):** тесты 13/13 зелёные; 77-мин mp4 → WAV за **2.49 с**;
mp4↔mp3 lag **+0 мс**; первая реплика не потеряна при offset 16.28 с (в срезе первые 4 с — речь,
пик −31.9 dBFS; проверка на уровне ASR — в W1); хвост не обрезан (срез 1059.96 с = 1076.24 − 16.28);
`fmt`/`clippy --workspace --all-targets -D warnings` — 0; живой `hds check` — `[ok] ffmpeg найден`.

### W1. ASR: `whisper_language` + галлюцинации на тишине (1 день) — ✅ **выполнено** (21.10.2026)

- [x] `whisper.rs::build_metadata` — параметр `language: Option<&str>`, ключ **только**
      `"whisper_language"`, добавляется лишь при непустом значении; `language`/`whisper_lang` не
      используются (спайк §8). Плюс `language_from_body`: нет поля ⇒ `ru`, `""` ⇒ авто.
- [x] `Whisper::transcribe_file(.., language: Option<&str>)`; `host::internal_transcribe` берёт
      `language_from_body` и пишет в лог `[transcribe] язык ASR: …`; пробы (`audio_probe`,
      `asr_lang_probe`) обновлены.
- [x] `TranscribeConfig`: поле `language: Option<String>` (`from_config` и `from_auto_transcribe`
      читают `index.whisper_language`; `None` ≠ `Some("")`); поле уходит в тело запроса на обоих путях
      (`transcribe`, `transcribe_to_file`).
- [x] Постфильтр §3.4 п. 5 — `hds_llama::asr_filter::drop_silent_segments` (по полному WAV, offset один
      раз, «отбрасываем, если доля < 5 % **и** речь < 150 мс»). Подключение в `internal_transcribe` —
      W4 (там же появляется полный WAV).
- [x] `config.example.yaml`: `whisper_language: "ru"` + комментарий; `server_config.example.yaml` —
      `whisper_language: 'ru'`.
- [x] Юниты (всего +8): metadata с языком/без/пустым; `language_from_body`; `from_config`/
      `from_auto_transcribe` (нет ключа / `''` / `' ru '`); фильтр (тишина, речь, тихая −31 dBFS,
      пограничный обрывок 9 % речи, вырожденные окна).
- [x] Интеграционный тест на stub-сервере: тело запроса содержит `"language":"ru"`, `"language":""`,
      и **не** содержит поля при `None`.

**Результаты W1 (живой прогон 21.10.2026, `asr_lang_probe`, 18-мин файл, GPU, `mode: subtitle`):**

| Прогон | Сегментов | оставлено | «Thank you» | «Obrigado» | «Lukoyle» | «Продолжение» | Время |
|---|---|---|---|---|---|---|---|
| без языка, без обрезки (как в проде до W1) | 241 | 240 | **42** | 0 | **67** | 0 | 51.5 с |
| `ru`, без обрезки | 245 | 244 | **0** | **0** | **0** | 1 | 43.5 с |
| без языка + обрезка | 246 | 243 | **0** | **0** | **0** | 0 | 44.4 с |
| `ru` + обрезка (боевой путь) | 246 | 243 | **0** | **0** | **0** | 0 | 44.0 с |

* «Lukoyle» в прогоне без языка — это транслит «Лукоил» (компания обсуждается всю встречу), а не только
  галлюцинация; с `ru` текст кириллический, вхождений 0.
* Постфильтр (боевой прогон) отбросил 3 сегмента: «добрый» [28.64–33.63] (доля речи **0.000**),
  «Продолжение» [1076.82–1093.46] и «следует.» [1096.19–1106.55] — **за концом файла** (1076.24 с).
  «Планировать их» [974.25–975.87] сначала отбрасывался — исправлено на семантику плана (keep при
  доле ≥ 5 %: у него 8.6 % речи, пик −25.2 dB) и покрыто тестом.
* Обрезка улучшает и содержимое начала: без неё whisper покрывает 15.84–45.84 с фразами «следует…»/
  «Продолжение», и приветствие теряется; с обрезкой первые реплики — «Ольга,» (17.42), «коллеги,»
  (21.95), «день. Прошу прощения…» (36.63).

**Приёмка W1 — ✅ выполнена:** **0** вхождений «Thank you»/«Obrigado»/«Lukoyle»; сегментов, целиком
лежащих в тишине, **0** (3 отброшено); русский текст с начала файла. По строгой методике спайка
(«начинается в тишине» = <20 % речи за 2.5 с) остаётся 3 реплики, но это **реальная тихая речь**:
«Ольга,» и «коллеги,» (тихое приветствие, ради которого и выбран порог −50 dBFS) и обрывок
«планировать их»; фантомов в тишине нет. Время ASR не выросло (44.0 с против 51.5 с без языка).

### W2. ONNX Runtime: одна версия на воркспейс (0.5 дня) — ✅ **выполнено** (21.10.2026)

- [x] `[workspace.package] rust-version = "1.92"` (локально 1.97.1). MSRV включил новые clippy-линты
      по всему воркспейсу — исправлено: `hds-core/src/roots.rs` (`map_or` → `is_none_or`),
      `hds-mcp/tests/client_package.rs` (`repeat().take()` → `repeat_n`).
- [x] `hds-clip`: `ort = "=2.0.0-rc.15"`. API-дельта оказалась одна: **`Session::builder(&Environment)`**
      (в rc.13 аргумента не было). Введён `hds_clip::env::environment()` — `OnceLock<Result<Environment>>`
      на процесс (ошибка кэшируется), и `Clip::load_with_env` / `VisionEncoder::load_with_env` /
      `TextEncoder::load_with_env` для случая, когда ORT уже инициализирован другим крейтом.
- [x] `hds-diarize::ort_env()` — `OnceLock` поверх `parakeet_rs::ort::init().build()` + `ort_env_clone()`
      (`Environment` — `Arc`, клон бесплатный); тест `env_is_singleton` (повторный вызов — тот же
      указатель). `parakeet-rs = "=0.4.2"`, фичи: дефолтные (`cpu` + `ort-defaults` + `api-28`) +
      `sortformer`, на Windows ещё `directml`, на macOS — `coreml`.
      **Уточнение к плану:** `default-features = false` не годится — `ort-defaults` включает
      `download-binaries`/`copy-dylibs`; без него ORT не подхватывается.
- [x] `hds-clip` теперь в одном бинаре с `hds-diarize` **уже сейчас**: `hds` → `hds-ui` → `hds-llama`
      → `hds-diarize` (плюс `hds-clip`). Коллизии нет, потому что CLI не инициализирует диаризацию;
      правило «один ORT-environment на процесс» зафиксировано в doc-комментариях обоих крейтов,
      а `Clip::load_with_env` готов принять общий env (W4).
- [x] Контроль: `cargo tree -i ort` — **одна** версия (`2.0.0-rc.15`); `cargo tree -i parakeet-rs` —
      одна (`0.4.2`).

**Приёмка W2 — ✅ выполнена:**
- `cargo build --release` целиком — ок (собран в `target\w2`, потому что службы держат
  `target\release\*.exe`, а останавливать их без админа нельзя; на артефакт это не влияет).
- Паритет CLIP на rc.15 (`cargo test -p hds-clip --test clip_parity -- --ignored`): vision
  **cos 0.999976 / 0.999950**, text **1.000000** — численно то же, что на rc.13.
- Живой `hds clip-index` на песочнице (`HDS_ROOT`=репо, `HDS_CONFIG`=песочница): `hds index` —
  `[1] накладная_с_exif.jpg -> indexed(1 чанков)`, `clip-index` — «картинок без векторов: 0»,
  в БД `images_vec` 1 строка (vision-сессия rc.15 отработала).
- `llm_host.exe` (пересобранный, ORT-linked) — `status` отвечает, `DirectML.dll` скопирован рядом;
  `onnxruntime*.dll` рядом нет — ORT линкуется статически (для упаковки: тащить DLL не нужно,
  `DirectML.dll` — нужен; учесть в W6).
- `cargo fmt --all -- --check` — 0; `cargo clippy --workspace --all-targets -D warnings` — 0;
  `cargo test --workspace` — зелёный.

### W3. Крейт `hds-diarize` (2 дня) — ✅ **выполнено** (21.10.2026)

- [x] `crates/hds-diarize/Cargo.toml`:
      ```toml
      [dependencies]
      parakeet-rs = { version = "=0.4.2", features = ["sortformer"] }
      [target.'cfg(windows)'.dependencies]
      parakeet-rs = { version = "=0.4.2", features = ["sortformer", "directml"] }
      [target.'cfg(target_os = "macos")'.dependencies]
      parakeet-rs = { version = "=0.4.2", features = ["sortformer", "coreml"] }
      ```
      Версию пиним **точно** (`=0.4.2`), как требует «Грабли» п. 6 handoff; дефолтные фичи
      **оставляем** (см. W2: `ort-defaults` тянет `download-binaries`/`copy-dylibs`). Уже добавлено
      в W2, здесь — только использование.
- [x] Публичный API (русские докстринги, стиль проекта):
      ```rust
      pub struct Turn { pub start: f64, pub end: f64, pub speaker: u32 }
      pub struct NemotronConfig { model, device, chunk_min, min_audio_min, onset, offset }
      pub struct Diarizer { /* Sortformer + конфиг + кэш sha256 */ }
      impl Diarizer {
          pub fn load(cfg: &NemotronConfig) -> Result<Self>;   // sha256, выбор EP, загрузка модели
          pub fn run_wav(&mut self, wav: &Path) -> Result<Vec<Turn>>;
      }
      pub fn merge_segments_with_turns(segments: &[WhisperSegment], turns: &[Turn]) -> Vec<Block>;
      pub fn render_md(blocks: &[Block], unassigned: &str) -> String;
      pub fn render_srt(blocks: &[Block], unassigned: &str) -> String;
      ```
- [x] `run_wav`/`run_i16`: длительность ≤ `min_audio_min` → `diarize()` целиком; иначе блоки по
      `chunk_min` через `feed(&[f32])` + `flush()`. Проверено: `feed()`/`flush()` отдают абсолютные
      таймкоды (`elapsed_samples`), `speaker_id` стабилен между блоками — чанкинг 5 и 10 мин дают
      **побитово одинаковый** результат (326 turns, 0 отличий границ); склейка-эвристика не понадобилась.
- [x] EP: `auto` → DirectML (Windows) / CoreML (macOS) только для файлов ≥ `min_audio_min`, иначе CPU;
      явный `device` сильнее; GPU-сессия не поднялась ⇒ лог + fallback на CPU.
      **Найден и исправлен баг:** `min_audio_min` (минуты) сравнивался с секундами — 60-с клип уходил на
      DirectML, где он медленнее; покрыто тестом `auto_device_threshold_is_in_minutes`.
- [x] sha256 модели: `verify_model` (size-pre-check → хеш, кэш по `(путь, размер, mtime)`), хешер `sha2`;
      константы `NEMOTRON_SIZE`/`NEMOTRON_SHA256` в коде; `default_model_path` (`HDS_MODELS_DIR` →
      `%APPDATA%\OpenResearchTools\models\nemotron3\`).
- [x] `merge_segments_with_turns` — чистая функция: середина сегмента; вне turns → `None`
      (`UNASSIGNED`); длинный сегмент (≥3 с) режется по границе turn'а (текст — по словам
      пропорционально), части <1 с присоединяются к соседу; перекрытие turns → большее пересечение,
      при равенстве — меньший `speaker`.
- [x] `render_md`/`render_srt` — формат движка (`### SPEAKER_NN [HH:MM:SS - HH:MM:SS]`, `UNASSIGNED`,
      пустая строка между репликами; SRT с миллисекундами).
- [x] Юниты: 29 в крейте (wav/vad/merge/render/sortformer/sha256/конфиг/EP) + интеграционный
      `tests/nemotron_live.rs` (`#[ignore]`, модель и WAV из env/артефактов спайка).

**Результаты W3 (живой прогон 21.10.2026, `target\w2\release\diar_nemotron.exe`):**

| Прогон | Устройство | Время | RTFx | Turns | Спикеры |
|---|---|---|---|---|---|
| `frag60` (60 с) | CPU (auto: <2 мин) | 4.68 с (вкл. сессию) | 12.8 | **5** | **2** — как в спайке |
| 18 мин | DirectML (auto) | 12.93 с | 83.2 | 326 | **5** |
| 18 мин, `chunk_min: 5` | DirectML | 12.77 с | 84.3 | 326 | 5, **побитово = chunk 10** |
| 18 мин, `--device cpu` | CPU | 47.47 с | **22.7** (≥20) | 326 | 5, **побитово = DirectML** |

Качество (flatten + `eval_diar.py` спайка, тот же эталон): атрибуция **58.2 %** (57/98), по активным
спикерам **58.5 %**, F1 смен **0.431** — точь-в-точь числа спайка; **без филлеров 74.4 % (58/78)**
(порог приёмки 71.8 %). Доли спикеров 49.0 / 48.5 / 1.9 / 0.2 / 0.2 % против спайковых
49.8 / 47.9 / 2.0 / 0.2 / 0.2 %. Модель проверена по sha256 (`400 506 656` /
`915e4fa2…be03a`).

**Приёмка W3 — ✅ выполнена:** набор и число спикеров совпали со спайком, чанкинг не меняет результат,
CPU RTFx 22.7, интеграционный тест `frag60` зелёный, `fmt`/`clippy -D warnings`/`test --workspace` — 0.
Замечание: среда ассистента выставляет `HDS_ROOT=<repo>`, из-за чего 2 теста `hds-llama`
(`host_resident`) падают — тесты прогнаны с очищенной переменной (код не менялся).

### W4. Интеграция в `llm_host` + форматтеры (2 дня) — ✅ **выполнено** (21.10.2026)

- [x] `host::internal_transcribe`: ветвление по backend. Приоритет (решение владельца 21.10.2026):
      тело `diarization_backend` → конфиг `diarization.backend` → авто (§7 п. 3: модель Nemotron есть
      и проходит sha256 → `nemotron3`, иначе `sortformer` + предупреждение в лог). Путь `nemotron3`:
      `[decode]` (ffmpeg, один проход) → обрезка ведущей тишины (PCM-срез только для ASR) → `[asr]`
      (`mode: subtitle` внутри, язык из §3.2) → `[diar]` (`run_wav` по полному WAV) → `+offset`
      (один раз) → постфильтр → `merge` → `render_md` (`out_ext: "md"`). Код:
      `crates/hds-llama/src/diar.rs` (разбор секции, выбор backend, `find_ffmpeg`, временный каталог
      задания, перевод шкал, `stats`) + ветка в `host.rs`; ORT-сессия живёт в
      `ClusterBackend::nemotron` и переиспользуется между заданиями.
- [x] `stats`: `num_speakers`, `num_turns`, `filtered_segments`, `timings_sec{decode, asr, diar, total}`
      поверх статистики движка (`num_segments`, `num_words`, …); в лог — одна строка на этап.
- [x] Для `nemotron3` VRAM под модель диаризации не запрашивается: `transcribe_need_mib` видит только
      whisper (`diar = None`), `prepare_demand(TRANSCRIBE_ROLE)` остаётся (ASR на движке).
- [x] Сбой диаризации: `required: true` → задание падает; `false` → `.md` без меток +
      `diarization{enabled:false, error}` и предупреждение в лог.
- [x] Путь `sortformer` не менялся (в лог — `backend=sortformer (legacy)`); `off` отдаёт обычный ASR
      (для `mode: transcript` движку передаётся `speech`, иначе он требует sortformer-модель).
- [x] Юниты +12 (`hds-llama::diar`): разбор секции и предупреждения, `pick_backend` (тело сильнее
      конфига), инвариант «offset один раз» (`merge_absolute` ≡ сшивка уже сдвинутых сегментов),
      подсчёт спикеров, `stats` поверх ключей движка, поиск `ffmpeg`, уникальность/уборка `TempDir`.
- [x] Живой прогон `hds transcribe-once` (18-мин файл, песочница `llm_host` на 8110,
      `target\w2\release`): **197 реплик, 5 спикеров** (`SPEAKER_00…04`, 34 `UNASSIGNED`),
      `<stem>.orig.md` и `<stem>.speakers.json` созданы (`w4_out/`, `w4_state/`).
- [x] Прямой `/internal/transcribe` (точные таймкоды, `w4_resp.json`): `out_ext: "md"`,
      `diarization{backend: nemotron3, enabled: true, num_speakers: 5}`, 310 сегментов, первый
      сегмент **17.42 с** (= 16.28 offset + 1.14), последний 1076.57 с.
- [x] Регресс sortformer тем же файлом (тело переопределило конфиг — проверка приоритета):
      `backend=sortformer`, `enabled: true`, 4 спикера + `UNASSIGNED`, 0 «Thank you»/«Obrigado»/
      «Lukoyle», статистика движка (`num_speaker_spans: 629`, `num_speaker_segments: 53`).

**Результаты W4 (живой прогон 21.10.2026, 18-мин файл, DirectML):**

| Этап | Время | Факт |
|---|---|---|
| декод ffmpeg (один проход) | 0.62–0.76 с | 1076.2 с аудио, `ffmpeg 9.0.1` (winget) |
| обрезка ASR | 0.07 с | **16.280 с** ведущей тишины (как в W0), срез 1060.0 с |
| ASR (движок, `whisper_language: ru`) | 43.7–53.2 с | 313 сегментов, **3 отброшено** постфильтром |
| диаризация Nemotron-3 (DirectML) | 9.8–13.7 с | 326 turns, **5 спикеров** (первый прогон — с сессией) |
| итого задание | 56.7–67.9 с | RTFx ≈ 15.9–19.0 к длительности файла |

Контроль качества: **0** «Thank you»/«Obrigado»/«Lukoyle»; по **точным** таймкодам сегментов —
**0** реплик, целиком лежащих в тишине (методика §3.4 п. 5: доля речи < 5 % И речь < 150 мс;
отброшены «добрый» на 28.6–33.6 с и два сегмента за концом файла). Проверка `.md` по округлённым до
секунды заголовкам даёт 2 ложных срабатывания (967–968 с и 1007–1008 с): точные сегменты там 56 % и
70 % речи — учитывать в W7 (`w4_check.py` для `.md` vs `w4_verify.py` для `segments[]`).

Память: прирост RSS `llm_host` за задание — **+780 МБ** (пик на диаризации 18 мин). Полный RSS
процесса (~8.5 ГБ) включает предзагруженный чат-инстанс движка (`KEEP_LOADED`, прежнее поведение),
поэтому критерий §7 «≤ 1.5 ГБ на 77 мин» в W7 измеряем **дельтой** на этой же песочнице
(`llm.chat.retention: load_on_demand` стартовую загрузку чата не отменяет — так устроен хост).

Артефакты: `tools/parity/out/diar_nemotron/{w4_host.yaml, host_w4.log, w4_resp.json,
w4_resp_sortformer.json, w4_check.json, w4_out/*.md, w4_state/*, w4_post.py, w4_check.py,
w4_verify.py}` (каталог гитигнорится).

**Приёмка W4 — ✅ выполнена:** `.md` содержит `### SPEAKER_NN` и 5 спикеров; `segments[]` и `.md`
согласованы (197 блоков сшивки из 310 сегментов); таймкоды абсолютные, offset применён один раз;
sidecar и `.orig.md` на месте, формат не менялся (переименование имён — существующий путь UI, покрыт
`cargo test -p hds-ui`); sortformer-регресс, `fmt`/`clippy -D warnings`/`test` — зелёные.

### W5. Конфиг и диагностика (0.5 дня) — ✅ **выполнено** (21.10.2026)

- [x] Чтение `diarization.*` и авто-выбор backend — сделаны в W4 (`hds-llama::diar`, решение №10).
- [x] Дефолт `auto_transcribe.diarization_backend` → **`nemotron3`** (`DEFAULT_DIARIZATION_BACKEND`
      в `hds-index::transcribe`, также `DiarizationSettings::default`); явное значение в конфиге
      по-прежнему сильнее (регресс sortformer покрыт тестом). Причина: тело запроса сильнее конфига
      (решение №13), при дефолте `sortformer` установки оставались на legacy-пути.
- [x] `hds-index` зависит от `hds-diarize` (константы `NEMOTRON_SIZE`/`NEMOTRON_SHA256`/`NEMOTRON_FILE`,
      `verify_model`) — один источник истины с `llm_host`. **Следствие для W6:** `hds_mcp.exe` и
      `hds-search` теперь тоже линкуют ORT ⇒ `DirectML.dll` нужен рядом со всеми бинарями пакета
      (а не только `hds`/`llm_host`).
- [x] Поиск модели: `hds_diarize::models_roots()` — единый порядок `HDS_MODELS_DIR` →
      `%ProgramData%\hermes-disk-search\models` → `%APPDATA%\OpenResearchTools\models`; дубль из
      `hds-index` удалён, **исправлен дефект W4**: `default_model_path` не смотрел `%ProgramData%`,
      т.е. служба под `LocalSystem` не нашла бы модель, поставленную машинно. Новое:
      `diarization.nemotron3.model` → автопоиск (`autotranscribe::nemotron_model_for`,
      явный путь возвращается как есть — диагностика видит «файла нет»).
- [x] Валидация ключей: `diag::diarization_summary` повторяет правило хоста «конфиг → авто» и собирает
      предупреждения (неизвестный `backend`/`device`, `chunk_min` вне 2..=30, EP чужой платформы);
      жёстких падений нет — как и у `DiarSettings::from_yaml` (W4).
- [x] `hds check` → пункт `diarization`: backend (конфиг/авто), путь модели + **sha256 ok**, EP
      (`device` + что значит `auto` на платформе), `chunk_min`, наличие `ffmpeg`. `fail` — если
      `nemotron3` без модели/с несошедшимся sha256/без `ffmpeg`; `warn` — авто-fallback на
      `sortformer` (причина + команда установщика); `ok` — `off` и рабочий legacy. Данные, которые
      нужно видеть сразу, вынесены в заголовок (у `ok` `hds check` печатает только его).
- [x] `hds whisper-check`: backend, EP, путь/размер модели + sha256, для legacy — «sortformer
      (legacy)»; в `--json` **аддитивные** ключи `diarization_backend`, `diarization_configured`,
      `diarization_device`, `diarization_device_auto`, `nemotron_model`, `nemotron_ok`,
      `nemotron_error`, `diarization_warnings` (ключ `diarization_model` = legacy сохранён).
      Уточнение к плану: «число спикеров последнего прогона» не печатаем — по handoff W4 п. 5 нужны
      путь/размер модели; спикеры видны в `stats` ответа и логе хоста (`[diar] … N спикеров`).
- [x] Образцы: `config.example.yaml` (секция `diarization:` с комментариями + `diarization_backend:
      nemotron3`), `server_config.example.yaml` (его рендерит `hds config init`: `diarization.backend:
      'nemotron3'`, `auto_transcribe.diarization_backend: 'nemotron3'`), Python-шаблон `hds/config.py`
      синхронизирован.

**Результаты W5 (живой прогон 21.10.2026, `target\w2\release\hds.exe`):**

| Прогон | Конфиг | Пункт `diarization` | Код |
|---|---|---|---|
| dev `config.yaml` (секция `diarization` отсутствует, Nemotron-модели нет) | — | `[--] диаризация: sortformer (legacy) — сработал автоподбор backend` + путь sortformer-модели (449 МБ), EP auto → directml | 0 |
| песочница `w5_check.yaml` (`backend: nemotron3`, модель `D:\diar-spike-models\…`) | явный путь | `[ok] … nemotron3 (Sortformer v3), EP auto (auto → directml) — D:\diar-spike-models\nemotron3_diar_v3.onnx (382 МБ, sha256 ok), ffmpeg найден` | 0 |
| песочница `w5_check_missing.yaml` (`backend: nemotron3`, модели нет) | автопоиск | `[!!] диаризация: модель Nemotron-3 не найдена` + `installers\fetch_nemotron3_model.ps1` | 1 |

- `hds whisper-check` (песочница с моделью): `[ok] диаризация: nemotron3 (Sortformer v3), EP auto
  (auto → directml)` + `модель: … (382 МБ, sha256 ok)`; `--json` — новые ключи, `nemotron_ok: true`.
- `hds config init --profile server --print --template server_config.example.yaml` — валидный YAML:
  `diarization.backend=nemotron3`, `auto_transcribe.diarization_backend=nemotron3`.
- Время `hds check` (весь набор): 0.5 с без модели, 0.7–1.5 с с моделью (sha256 400 МБ кэшируется
  по `(путь, размер, mtime)`).
- Тесты: +4 (`transcribe::auto_transcribe_defaults_to_nemotron3`, `diag::diarization_verdict_statuses`,
  `diag::diarization_summary_reads_config`, `config_init::template_renders_nemotron3_diarization`);
  `cargo fmt --check`, `clippy --workspace --all-targets -D warnings`, `cargo test --workspace` —
  зелёные. Артефакты — `tools/parity/out/diar_nemotron/{w5_check.yaml, w5_check_missing.yaml,
  w5_whisper_check.txt, w5_whisper_check.json, w5_server_config.yaml}`.

**Приёмка W5 — ✅ выполнена:** `hds check` показывает все пункты зелёными на песочнице с моделью; без
модели — понятная ошибка с командой установщика и код 1; `config init` даёт валидный YAML (тест на
рендер); дефолт клиента — `nemotron3`.

### W6. Установка модели и лицензии (1 день) — ✅ **выполнено** (21.10.2026)

- [x] `installers/fetch_nemotron3_model.ps1` (**ASCII-only**, PS 5.1): `-ModelsDir`, `-Force`,
      `-Repo`/`-Revision`/`-File`; ревизия **`4d2a8bc71f5c896ec40faa59732e6716295edaf2`**
      (`altunenes/parakeet-rs`, `nemotron-3-diarization/nemotron3_diar_v3.onnx`) зафиксирована в
      шапке и в параметре — `main` не используется; проверки size **400 506 656** и
      `Get-FileHash -Algorithm SHA256` (совпадает с LFS-oid HF); каталог `<ModelsDir>/nemotron3/`;
      скачивание в `.part` → проверка → `Move-Item`; идемпотентность (верный файл — выход 0),
      `-Force` — перекачать; каталоги: `HDS_MODELS_DIR` → `%ProgramData%\hermes-disk-search\models`
      (если записываем) → `%APPDATA%\OpenResearchTools\models`; macOS — `~/Library/Application Support`.
      Попутно исправлено: `$File?download` PowerShell разбирал как переменную `$File?download`
      (URL получался `…/=true`) — теперь `${File}?download=true`.
- [x] `setup.ps1`: шаг «Diarization models (auto-transcription)» качает **оба** (Nemotron-3 + Sortformer),
      флаг `-SkipDiarization` пропускает оба, подсказки `-SkipModels` и список моделей автотранскрибации
      обновлены.
- [x] `installers/install_server.ps1`: `fetch_nemotron3_model.ps1` в `$fetches` (`-ModelsDir
      $modelsDir` = машинный `HDS_MODELS_DIR`); текст про `ffmpeg` исправлен — он **обязателен**
      для `nemotron3` (декод WAV), без него `[!!]` с командой winget.
      `installers/install_server.command`: тот же fetch через `pwsh` + тот же текст про `ffmpeg`.
- [x] `installers/build_rust_release.ps1`: `DirectML.dll` копируется в `bin\` (из `target\release`,
      иначе поиск по дереву; нет DLL — `throw`, а не молчаливый пакет без GPU); перед упаковкой
      проверка «в stage нет `*.onnx`/`*.gguf`/`*.onnx.data`» (`throw` со списком). Скрипт fetch
      попадает в архив вместе с каталогом `installers\`.
- [x] `NOTICE.md`: новый раздел «Диаризация: Nemotron-3 (Sortformer v3), `hds-diarize`» — веса
      **OpenMDW 1.1** (текст проверен по `nemotron-3-diarization/LICENSE`: сохранение текста и
      уведомлений о происхождении, на **выходы** ограничений нет), `parakeet-rs` 0.4.2 —
      MIT OR Apache-2.0, ONNX Runtime 2.0.0-rc.15 — MIT, `DirectML.dll` — лицензия Microsoft
      «DirectML» (распространение в составе приложения для Windows/Xbox ⇒ только Windows-пакет),
      sortformer 4spk-v2.1 — NVIDIA Open Model License (fallback); отдельно: модель в репозиторий
      и архив не входит.
- [x] `.gitignore`: `*.onnx`, `*.onnx.data`, `*.onnx.part`, `*.gguf` (в git отслеживаемых весов не было
      и нет: `git ls-files '*.onnx' '*.gguf'` = 0).

**Результаты W6 (живой прогон 21.10.2026):**

| Проверка | Факт |
|---|---|
| чистая загрузка (`-ModelsDir D:\diar-spike-models`) | 400 506 656 байт, sha256 совпал, **52.7 с** (382 МБ) |
| повторный запуск (файл на месте) | `[ok] … already present`, **0.7 с**, выход 0 |
| `-Force` | перекачал и проверил sha256, **40.3 с**, выход 0 |
| битый ответ (подставлен LICENSE вместо модели) | `[!!] wrong size: expected 400506656, got 2660`, `.part` удалён, выход 1 |
| `hds check` c `HDS_MODELS_DIR=D:\diar-spike-models` | `[ok] … nemotron3 … D:\diar-spike-models\nemotron3\nemotron3_diar_v3.onnx (382 МБ, sha256 ok), ffmpeg найден` — канонический путь `<root>/nemotron3/` совпадает с установщиком |
| `build_rust_release.ps1 -Version 0.21.0-w6test -SkipBuild -SkipZip` | `bin\` = `hds.exe`, `hds_mcp.exe`, `llm_host.exe`, **`DirectML.dll`**; `*.onnx`/`*.gguf` в stage — **0**; `installers\fetch_nemotron3_model.ps1` в архиве |

Тесты: +2 (`install_server_parity::nemotron3_fetch_script_is_pinned_and_verified`,
`release_package_ships_directml_and_no_weights`; 17 в файле) — зелёные. Артефакты:
`tools/parity/out/diar_nemotron/w6_package.txt` (состав пакета; `dist/` после проверки удалён).

**Приёмка W6 — ✅ выполнена:** установщик качает 400.5 МБ, sha256 совпадает, повторный запуск
пропускает файл, `-Force` перекачивает; `hds check` видит модель в каноническом каталоге;
LICENSE-атрибуция на месте; в пакете `DirectML.dll` и нет весов.

### W7. Тесты и приёмка (1 день) — ✅ **выполнено** (21.10.2026), RSS — с оговоркой

- [x] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace` — зелёные (после `Remove-Item Env:\HDS_ROOT`).
- [x] Живые прогоны 18 / 56 / 77 мин через песочницу `llm_host` (`w7_host.yaml`, порт-база 8110) и
      диаризация отдельно (`diar_nemotron`); артефакты — `tools/parity/out/diar_nemotron/`
      (`w7_*`, `*.rss.csv`, `host_w7*.log`), сводка — `report.md`.
- [x] Диаризация отдельно: 18 мин — DirectML **12.75 с (RTFx 84.4)**, CPU **51.11 с (RTFx 21.1)**
      ⇒ DirectML **4.0×** CPU; 77 мин — DirectML **45.07 с (RTFx 102.4)**, **8 спикеров**;
      `chunk_min` 5 vs 10 — turns **побитово равны**; изолированный пик RSS на 77 мин — **633 МБ**.
- [x] Сквозной путь (nemotron3): 18 мин — 72.5 с (decode 0.89 / ASR 58.1 / diar 13.3), 5 спикеров;
      56 мин — 177.7 с (2.2 / 145.5 / 29.7), 8 спикеров; 77 мин — 217.5 с (2.6 / 171.7 / 42.5),
      **8 спикеров**; **0** реплик целиком в тишине и **0** «Thank you»/«Obrigado»/«Lukoyle» на всех
      трёх; offset один раз (первый сегмент 18 мин = 17.42 с).
- [x] RSS 77 мин: **+1557 МБ** от свежего хоста (было +1704 до оптимизации). Диаризация в хосте
      добавляет **≈0.40 ГБ** (633 МБ в изоляции), а пик задания — фаза ASR: контрольный прогон
      `backend: off` (только whisper) пикует **выше** (9519 против 9252 МБ абсолютных).
      **Критерий «≤ 1.5 ГБ» буквально не выполнен на 4 %**; причина — память движка whisper (путь
      W1, не менялся), а не диаризация. Попутная оптимизация W7 в `host.rs`: полный WAV не держится
      на время ASR и диаризация получает сэмплы (`run_i16`) вместо второго чтения (−147 МБ, результат
      тот же). **Требуется решение владельца:** принять 1.56 ГБ или отдельный трек по памяти ASR.
- [x] Качество: `flatten.py` + `w3_eval_filtered.py` на `ref_20260924__ERP.SPS._.json` — атрибуция
      без филлеров **74.4 % (58/78)** при пороге 71.8 %, 5 спикеров (как в W3); оговорка §11.3
      спайка зафиксирована в отчёте (метрики относительные, абсолютный DER не публикуем).
- [x] Регресс `diarization.backend: sortformer`: 18 мин — 312 сегментов, 2342 слова, 4 спикера +
      `UNASSIGNED`, формат прежний; 77 мин — 1368 сегментов, 7774 слова, 4 спикера + `UNASSIGNED`.
      `num_speaker_spans` на 18 мин гуляет между запусками (629 в W4 / 571 / 523) при одинаковых
      сегментах и словах — недетерминизм потокового диаризатора движка, legacy-код не менялся.
      Паритеты `pipeline_parity`, `walk_parity`, `chunker_parity`, `hash_parity` — зелёные.
- [x] Отчёт `tools/parity/out/diar_nemotron/report.md` (таблицы 18/56/77, CPU/DML, attr, RSS с
      разбором фаз, паритеты, `chunk_min` 5 vs 10, регресс sortformer).


### W8. macOS — best-effort (0.5 дня) — ⚠️ **код готов, вживую НЕ проверено** (машины macOS нет)

- [x] `parakeet-rs` с фичей `coreml` включён для `cfg(target_os = "macos")`
      (`crates/hds-diarize/Cargo.toml`); CI-джоб `build-macos` (`release.yml`) остаётся
      `continue-on-error` и соберёт darwin-ветку с этой фичей (как только появится раннер).
- [x] `device: auto` на macOS → CoreML; если CoreML-сессия не поднялась — `Diarizer::create`
      пишет `[diar] CoreMl недоступен (…); перехожу на CPU` и работает на CPU (не падает).
      Явный `device: directml` вне Windows и `coreml` вне macOS отвергаются понятной ошибкой —
      покрыто тестом `sortformer::tests::foreign_execution_provider_is_rejected` (работает на
      любой платформе). `hds check` показывает «auto → coreml» на macOS —
      `diag::tests::platform_auto_ep_matches_platform`.
- [x] Каталог модели на macOS — `~/Library/Application Support/OpenResearchTools/models/nemotron3/`
      (`hds_diarize::models_roots`); установщик `installers/install_server.command` качает модель
      тем же `fetch_nemotron3_model.ps1` через `pwsh` (W6). `ffmpeg` для macOS-installer —
      `brew install ffmpeg` с `[!!]`-предупреждением, если его нет (W6).
- [ ] Ручная проверка на живой машине — **не выполнялась**: macOS-машины нет.
      Кросс-проверка с Windows невозможна: `cargo check --target aarch64-apple-darwin` падает
      в `onig_sys` (`tokenizers`, зависимость `parakeet-rs`) — нужен macOS SDK/C-компилятор
      (проверено 21.10.2026, `target\maccheck` удалён, цель rustup снята).
      **Статус в документации: «macOS: не проверено»** (README — W9).

**Приёмка W8 — ⚠️ частично:** гарды и fallback покрыты тестами, установка/поиск модели готовы;
живой прогон CoreML не выполнялся — так и фиксируем в README/STATUS (W9).

### W9. Документация и релиз (0.5 дня) — 📝 **документация готова**, тег — после решения владельца

- [x] `README.md`: глава 8 «Расшифровка видео/аудио» — секция `diarization` в примере настроек, один
      декод/срез тишины/до 8 говорящих, модель Nemotron-3 (400 506 656 байт, OpenMDW 1.1,
      `installers\fetch_nemotron3_model.ps1`, каталоги моделей), `ffmpeg` (winget/brew), прежний
      sortformer как fallback, `index.whisper_language` (`ru`), macOS «не проверено»; в главе команд —
      `fetch_nemotron3_model.ps1`.
- [x] `README_SERVER_MULTIUSER.md`: машинный каталог модели (`%ProgramData%\hermes-disk-search\models\
      nemotron3`, на Mac — `/Library/Application Support/...`), `-Fetch`/`--fetch` качает Nemotron-3,
      `-SkipDiarization`, `ffmpeg` обязателен, секция `diarization` в примере настроек, формат
      `SPEAKER_00`…`SPEAKER_07`, macOS «не проверено».
- [x] `STATUS.md`: блок «Диаризация Nemotron-3» (треки W0–W8, числа W7, открытый вопрос по RSS).
- [x] `NOTICE.md` (W6), `RELEASE_NOTES_v0.21.0.md` — по формату `RELEASING.md` §7 (что скачивать,
      что нового, требования к обновлению включая модель и `ffmpeg`, «проверено/не проверено»).
- [x] Версия по `RELEASING.md`: `__version__ = "0.21.0"` (`hds/__init__.py`; джоба `check-version`
      сверит её с тегом), крейты воркспейса остаются `0.1.0`.
- [ ] **Тег `v0.21.0` и `release.yml`** — не сделаны осознанно: по `RELEASING.md` тег ставится после
      зелёной приёмки, а по RSS на 77 мин нужно решение владельца (см. W7: 1.56 ГБ против 1.5 ГБ).
      Ветка `server-multiuser` локально впереди `origin` — сначала её пуш, затем
      `git tag v0.21.0 && git push origin v0.21.0` (или `gh workflow run release.yml --ref
      server-multiuser -f version=v0.21.0`).
- [x] Обновлён статус в этом файле (W0–W9 отмечены, числа записаны); `DIARIZATION_NEMOTRON_HANDOFF.md`
      (исходный спайк) остаётся историей, актуальный план — этот файл.


## 5. Приёмка

**Обязательные проверки:**
```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings     # 0/0
cargo test --workspace
```
- [ ] fmt/clippy/test зелёные; `cargo tree -i ort` — одна версия (`2.0.0-rc.15`).

**Живые прогоны (dev-стенд):**
1. [ ] 18-мин файл через `hds transcribe-once` (и `transcribe-watch` на тестовой inbox): `.md` содержит
   `### SPEAKER_NN`, **5 спикеров**, `UNASSIGNED` только вне речей; `<stem>.orig.md` и
   `<stem>.speakers.json` на месте; UI «Транскрибация» открывает файл, переименование
   `SPEAKER_00 → Иван` работает (`.bak`, sidecar).
2. [ ] 56-мин файл: не падает, спикеров ≥ 5, `stats.timings_sec` заполнен; обрезка ведущей тишины —
   no-op (файл без тишины, offset 0).
3. [ ] 77-мин файл: не падает, **RSS ≤ 1.5 ГБ** (чанкинг), **8 спикеров**, диаризация ≤ 5 мин на CPU.
4. [ ] ASR (18-мин): **0** «Thank you»/«Obrigado»/«Lukoyle»; язык — из конфига; **0** реплик,
   начинающихся в тишине; «Добрый день» присутствует (обрезка не съела тихую первую реплику).
5. [ ] Регресс: `diarization.backend: sortformer` даёт прежний результат на тех же файлах (сравнение по
   `num_speaker_spans`/спикерам); паритеты индексации (`pipeline_parity`, `walk_parity`) — без изменений.
6. [ ] Качество: на 18-мин файле (эталон без коротких филлеров, 78 реплик) атрибуция **≥ 71.8 %**
   (текущий sortformer) и спикеров **≥ 5**; методика — `flatten.py` + `eval_diar.py` спайка.
7. [ ] Таймлайн: таймкоды `.md`/`segments[]` — абсолютные (выборочная сверка 10 реплик с эталоном);
   trim-offset применён ровно один раз (нет двойного сдвига на 13.38 с).

**Порог производительности:** CPU RTFx ≥ 15 на файлах ≥ 10 мин; при `device: auto` на Windows DirectML
даёт ≥ 3× к CPU на файлах ≥ 10 мин (в спайке — 4.5×).

## 6. Риски

| Риск | Митигация |
|---|---|
| **Рассинхрон таймлайнов** (trim применён к ASR, но не скомпенсирован) — атрибуция 74.4 % → 57.7 % | Инвариант §3.4 п. 4, тест `merge(shift(segments,+off))==merge(abs)`, живая сверка таймкодов (приёмка п. 7) |
| Агрессивный порог обрезки съедает тихую первую реплику (в эталоне 9 реплик «из тишины», «Добрый день» −34.8 dBFS) | Порог только `-50 dBFS` (+25 дБ над шумом), lead-in 0.3 с, юнит-тест «тихий тон −36 dBFS», живая проверка |
| `parakeet-rs` — молодой крейт, API менялся между версиями | Пин точной версии, интеграционный тест на `frag60`, при обновлении перечитать `src/sortformer.rs` |
| `feed()`/`flush()`: относительные таймкоды или переиндексация спикеров между блоками | Тест чанкинга (2 блока), при расхождении — склейка по перекрытию 5 с, `chunking_report.md` |
| Память растёт с длиной (~25 МБ/мин) | Чанкинг `chunk_min: 10` + замер RSS на 77 мин (критерий ≤ 1.5 ГБ) |
| DirectML проигрывает на коротких клипах | `min_audio_min: 2`, `device: auto` |
| `ort 2.0.0-rc.15` ломает `hds-clip` | Отдельный шаг W2 с живым `hds clip-index`; откат на rc.10 невозможен (cargo унифицирует версию) ⇒ правки API делаем сразу |
| Две ORT-версии / два environment в процессе | `hds-diarize::ort_env()` (OnceLock), `cargo tree -i ort`, `hds-clip` принимает `&Environment` |
| Эталон шумный (только начала реплик, 20 филлеров) | Метрики относительные, филлеры исключаем, абсолютный DER не публикуем (`REPORT.md` §11.3) |
| Модель — сторонний ONNX-экспорт | Пин ревизии + sha256 + size в установщике и в `Diarizer::load` |
| Галлюцинации в паузах остаются (русские) | Энергетический постфильтр §3.4 п. 5, `stats.filtered_segments`, живая проверка |
| Службы не видят модель/`ffmpeg` | Машинный каталог моделей (`HDS_MODELS_DIR`) в `install_server.*`, проверка `ffmpeg` |
| Сборка падает: `target\release\llm_host.exe` занят Планировщиком | Остановить резидента (`dev-machine.md`), собрать, вернуть задачу |
| macOS/CoreML не проверен | Best-effort W8, статус «не проверено» в отчёте и `README` |
| Конфиги без секции `diarization` | Авто-выбор по §7 п. 3 + предупреждение в лог (конфиги не перезаписываем) |

## 7. Решения владельца (утверждены 20–21.10.2026 — не пересматривать)

1. **Дефолт `index.whisper_language` — `ru`** (продукт русскоязычный); пустая строка = авто-детекция
   (осознанный выбор), поле `language` в теле `/internal/transcribe` переопределяет конфиг на один запрос.
2. **`chunk_min = 10` минут** (память против риска стыков); `min_audio_min = 2`.
3. **Поведение при апдейте:** если секции `diarization` нет — `nemotron3`, когда файл модели есть и
   проходит sha256, иначе `sortformer` + предупреждение в лог. `install_server`/`setup` пишут
   `backend: nemotron3` явно; существующие конфиги принудительно не перезаписываются.
4. **Экспорт ONNX:** остаёмся на стороннем файле `altunenes/parakeet-rs` (пин по ревизии + sha256);
   свой экспорт из `.nemo` — отдельным необязательным треком вне этого плана; модель в репозитории не
   хранится.
5. **Sortformer остаётся рабочим fallback** (`diarization.backend: sortformer`,
   `auto_transcribe.diarization*`, `installers/fetch_diarization_model.ps1` не трогаем). Удаление —
   отдельным решением после приёмки и одного релиза с Nemotron.
6. **Обрезка тишины — вариант A** (решение владельца 21.10.2026): диаризация по полному WAV, обрезка
   только для ASR, offset применяется один раз, порог `-50 dBFS`, внутренние паузы и хвост не режутся;
   галлюцинации в паузах лечит энергетический постфильтр сегментов (§3.4 п. 5).
7. **Версия релиза — `v0.21.0`** (новая функциональность + MSRV 1.92), тег после зелёной приёмки по
   `RELEASING.md`.

## 8. Ссылки

- `DIARIZATION_NEMOTRON_HANDOFF.md` — промт, решения, треки, sha256 модели, грабли.
- `tools/parity/out/diar_spike/REPORT.md` — спайк: числа, §11 (галлюцинации whisper), §11.3 (эталон).
- `tools/parity/out/diar_spike/{flatten,eval_diar,diag_ref,speaker_stats,compare_pcm}.py`,
  `crate/src/main.rs` (API `parakeet-rs`), `out/*.json` (эталонные прогоны).
- `PLAN_AUTO_TRANSCRIBE.md` §5 (диаризация, `.md`, sidecar), `PLAN_SERVER_MULTIUSER.md` (роль
  `transcribe`), `RELEASING.md` (версия/релиз), `cline-rules/dev-machine.md` (правила dev-машины).
- Код: `crates/hds-llama/src/{whisper,host}.rs`, `crates/hds-index/src/{transcribe,autotranscribe,diag}.rs`,
  `crates/hds-cli/src/cmd/{check,whisper_check}.rs`, `crates/hds-ui/src/transcribe.rs`,
  `installers/{fetch_diarization_model,install_server,setup}.ps1*`, `NOTICE.md`.
- Артефакты прогонов — `tools/parity/out/diar_nemotron/` (гитигнорится).
