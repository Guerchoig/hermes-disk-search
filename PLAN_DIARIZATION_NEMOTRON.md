# PLAN_DIARIZATION_NEMOTRON.md — диаризация Nemotron-3 (Sortformer v3) вместо sortformer

> Продолжает `DIARIZATION_NEMOTRON_HANDOFF.md` (решения владельца 20.10.2026) и спайк
> `tools/parity/out/diar_spike/REPORT.md` (числа, §11 — галлюцинации whisper). Дополняет
> `PLAN_AUTO_TRANSCRIBE.md` §5 (диаризация, формат `.md`, sidecar) и `PLAN_SERVER_MULTIUSER.md`
> (роль `transcribe`); контракты `PLAN_AUTO_TRANSCRIBE.md` §5 и формат вывода **не меняются**.
> **Статус:** запланирован 21.10.2026. Ветка **`server-multiuser`**, HEAD `0037f43` (тег `v0.20.6`).
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

### W3. Крейт `hds-diarize` (2 дня)

- [ ] `crates/hds-diarize/Cargo.toml`:
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
- [ ] Публичный API (русские докстринги, стиль проекта):
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
- [ ] `run_wav`: длительность ≤ `min_audio_min` → `diarize()` целиком; иначе блоки по `chunk_min` через
      `feed(&[f32])` + `flush()`. **Проверить и покрыть тестом** (критично для склейки): `feed()` отдаёт
      **абсолютные** таймкоды (счётчик `elapsed_samples` в `parakeet-rs::sortformer`) и `speaker_id`
      стабилен между блоками (AOSC-кэш). Если не стабилен — склейка по перекрытию соседних блоков
      (5 с) + отчёт в `tools/parity/out/diar_nemotron/chunking_report.md`.
- [ ] EP: `auto` → `DirectML` на Windows при успешной сессии, иначе `Cpu` (с логом причины); `CoreMl` на
      macOS; явный `device` сильнее авто.
- [ ] sha256 модели: проверка при `load` (size быстрый pre-check → хеш, результат кэшируется по
      `(путь, размер, mtime)` в памяти процесса); хешер — `sha2` (уже в `[workspace.dependencies]`,
      новых зависимостей **нет**).
- [ ] `merge_segments_with_turns` — чистая функция: середина сегмента; середина вне turns →
      `UNASSIGNED`; сегмент через границу turn'а режется на части; пустые turns → всё `UNASSIGNED`;
      перекрытие turns → побеждает turn с большим пересечением (детерминированно при равенстве — меньший
      `speaker`).
- [ ] Юниты: `wav`/`vad`/`merge`/`render` (формат строки `### SPEAKER_00 [00:00:16 - 00:00:22]`,
      `UNASSIGNED`, `HH:MM:SS` от 0, порядок, режутся границы, сегмент короче 1 мс).
- [ ] Интеграционные (по умолчанию `#[ignore]`, включаются `HDS_TEST_DIAR=1` + пути к WAV/модели):
      `frag60` → 2 спикера / 5 сегментов (эталон спайка); 18-мин → 5 спикеров.

**Приёмка W3:** совпадение с `tools/parity/out/diar_spike/out/nemotron3_meet0924.json` (±1 кадр по
границам, тот же набор спикеров, немонотонности нет); чанкинг на 18-мин файле (`chunk_min: 5`) даёт те
же 5 спикеров и границы ±0.5 с; CPU RTFx ≥ 20.

### W4. Интеграция в `llm_host` + форматтеры (2 дня)

- [ ] `host::internal_transcribe`: ветвление по `diarization.backend` (§3.1). Путь `nemotron3`:
      `[decode]` (W0) → `[asr]` (`mode: subtitle` внутри, язык из §3.2) → `[diar]` (`run_wav` по полному
      WAV) → `+offset` → постфильтр → `merge` → `render_md`.
- [ ] `stats`: `num_speakers`, `num_turns`, `filtered_segments`, `timings_sec{decode, asr, diar, total}`;
      в лог — одна строка на этап (`[diar] nemotron3: EP=DirectML, 5 спикеров, 315 turn'ов, 9.4 с`).
- [ ] Для `nemotron3` **не** запрашивать VRAM под модель диаризации: `transcribe_need_mib` учитывает
      только whisper; `prepare_demand(TRANSCRIBE_ROLE)` остаётся (ASR на движке) — «Грабли» п. 12.
- [ ] Сбой диаризации при `diarization.required: true` → задание падает (жёсткий отказ, как сейчас);
      при `false` → `.md` без меток + предупреждение в лог (`diarization{enabled:false}` в ответе).
- [ ] Путь `sortformer` — байт-в-байт прежний код (метка в логе `backend=sortformer (legacy)`).
- [ ] Живой прогон `hds transcribe-once` на 18-мин файле: `.md` с 5 спикерами, `UNASSIGNED` только вне
      речей, `<stem>.orig.md` и `<stem>.speakers.json` на месте, UI «Транскрибация» открывает файл и
      переименовывает `SPEAKER_00 → Иван` (`.bak` создаётся).
- [ ] Регресс: тот же файл с `diarization.backend: sortformer` → прежние `num_speaker_spans`/спикеры
      (сравнение с `tools/parity/out/diar_spike/out/baseline_sortformer.json` по числу спанов и
      набору спикеров).

**Приёмка W4:** `.md` содержит `### SPEAKER_NN`, 5 спикеров; таймкоды совпадают с ASR-разметкой
(выборочно 10 реплик вручную); `segments[]` и `.md` согласованы; sidecar/UI работают.

### W5. Конфиг и диагностика (0.5 дня)

- [ ] `hds-core::config` — чтение `diarization.*` (секция опциональна; нет секции ⇒ авто-выбор по §7 п. 3:
      модель Nemotron есть → `nemotron3`, иначе `sortformer` + предупреждение в лог).
- [ ] Валидация ключей (§3.5) в `hds check` и при старте `llm_host` (ошибка конфига — понятное
      сообщение, без паники).
- [ ] `hds check`: пункт `diarization` — backend, путь модели + sha256 ok, EP, наличие `ffmpeg`.
- [ ] `hds whisper-check`: печатать `diarization.backend`, EP и число спикеров последнего прогона
      (из лога/статуса; без запуска модели).
- [ ] `config.example.yaml`, `server_config.example.yaml`, `hds config init` — секция с комментариями,
      в серверном шаблоне `backend: nemotron3`.

**Приёмка W5:** `hds check` на dev-машине показывает все пункты зелёными; при отсутствующей модели —
понятная ошибка с командой установщика; `config init` даёт валидный YAML (`cargo test` на рендер).

### W6. Установка модели и лицензии (1 день)

- [ ] `installers/fetch_nemotron3_model.ps1` (**ASCII-only**, PowerShell 5.1): параметры
      `-ModelsDir`, `-Force`; URL и ревизия — **зафиксировать и записать в шапку скрипта** (источник —
      `altunenes/parakeet-rs`, файл `nemotron-3-diarization/nemotron3_diar_v3.onnx`); проверки
      size **400 506 656** и `Get-FileHash -Algorithm SHA256` =
      **`915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a`**; каталог
      `<ModelsDir>/nemotron3/`; скачивание в `.part` → проверка → `Move-Item`; идемпотентность (файл с
      верным хешем — выход `0`), `-Force` — перекачать; `HDS_MODELS_DIR` учитывается (службы).
- [ ] `setup.ps1`: шаг «Модель диаризации (Nemotron-3)» + флаг `-SkipDiarization` (не качать).
- [ ] `installers/install_server.ps1` и `install_server.command`: модель в **машинный** каталог (видят
      службы под LocalSystem) + проверка наличия `ffmpeg` (иначе предупреждение с инструкцией —
      без `ffmpeg` сервер не декодирует медиа, W0).
- [ ] `installers/build_rust_release.ps1`: включить новый скрипт в серверный пакет; модель в архив
      **не** кладём.
- [ ] `NOTICE.md`: блок «Диаризация» — веса **Nemotron-3-Diarization** (OpenMDW 1.1; проверить текст
      лицензии и записать вывод), `parakeet-rs` — MIT OR Apache-2.0, ONNX Runtime — MIT (DirectML —
      Microsoft, поставляется пакетом `ort`), sortformer 4spk-v2.1 (пока остаётся) — NVIDIA Open Model
      License; отдельно: модель не входит в репозиторий.
- [ ] `.gitignore`/пакет: убедиться, что `*.onnx` не попадёт в git и в релизный zip.

**Приёмка W6:** на чистом профиле установщик качает 400.5 МБ, sha256 совпадает, повторный запуск
пропускает файл, `-Force` перекачивает; `hds check` видит модель; LICENSE-атрибуция на месте.

### W7. Тесты и приёмка (1 день)

- [ ] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace` — зелёные (каждый трек проверялся отдельно).
- [ ] Живые прогоны по §5 (18/56/77 мин), артефакты — `tools/parity/out/diar_nemotron/`:
      `.md`, `probe.json`, `rss.txt`, `eval_*.json`, `chunking_report.md`, `report.md`.
- [ ] Замер RSS 77-мин прогона: `llm_host` с чанкингом (`chunk_min: 10`) — **≤ 1.5 ГБ** (сейчас
      `diarize()` целиком даёт 2073 МБ ⇒ это и есть проверка чанкинга).
- [ ] Метрики качества: `python flatten.py`, `python eval_diar.py <ref> <model>` на эталоне
      `ref_20260924__ERP.SPS._.json` без филлеров (78 реплик) — атрибуция **≥ 71.8 %**, спикеров ≥ 5.
      В отчёт — оговорка `REPORT.md` §11.3 (метрики относительные, абсолютный DER не публикуем).
- [ ] Производительность: CPU RTFx ≥ 15 на ≥10 мин; при `device: auto` на Windows DirectML ≥ 3× CPU.
- [ ] Регресс: `diarization.backend: sortformer` (18/77 мин) — прежние спаны/спикеры; паритеты
      индексации `pipeline_parity`, `walk_parity` — без изменений; `cargo test` по `hds-index`.
- [ ] Отчёт `tools/parity/out/diar_nemotron/report.md` со сводными таблицами.

### W8. macOS — best-effort (0.5 дня)

- [ ] `parakeet-rs` с фичей `coreml` собирается под `aarch64-apple-darwin` (CI-джоб macOS уже
      `continue-on-error`); модель та же (sha256 совпадает).
- [ ] `device: auto` на macOS → CoreML с fallback на CPU (лог причины); при недоступном CoreML
      диаризация не падает.
- [ ] Ручная проверка на живой машине (если доступна) — зафиксировать в отчёте; иначе — статус
      «не проверено» и заметка в `README`.

### W9. Документация и релиз (0.5 дня)

- [ ] `README.md`: раздел «Автотранскрибация» — модель Nemotron-3 (размер, лицензия, установщик),
      требования (`ffmpeg` в PATH/пакете), `diarization.backend`, «8 спикеров вместо 4», язык ASR.
- [ ] `README_SERVER_MULTIUSER.md`: машинный каталог модели, установка на сервере, `-SkipDiarization`.
- [ ] `STATUS.md`: блок «Диаризация Nemotron-3» со статусом треков и числами приёмки.
- [ ] `NOTICE.md` (из W6), `RELEASE_NOTES_v0.21.0.md` (новая минорная версия: новая функциональность,
      MSRV 1.92, дефолт языка `ru`, sortformer как fallback).
- [ ] Версия по `RELEASING.md`: поднять до `v0.21.0`, тег после зелёной приёмки.
- [ ] Обновить статус в этом файле (треки → `[x]`, приёмка → `[x]`) и в `DIARIZATION_NEMOTRON_HANDOFF.md`
      сослаться на этот план.

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
