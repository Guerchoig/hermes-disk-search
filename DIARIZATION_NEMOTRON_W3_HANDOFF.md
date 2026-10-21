# DIARIZATION_NEMOTRON_W3_HANDOFF.md — продолжение работ (после W0–W3)

> Скопируйте §10 (промт) в новый чат. Документ самодостаточен: состояние репозитория,
> решения владельца, что уже сделано (с числами), где именно в коде менять W4–W9,
> грабли окружения и критерии приёмки.
>
> Предыдущий handoff: `DIARIZATION_NEMOTRON_HANDOFF.md` (спайк, решения, треки целиком).
> План: `PLAN_DIARIZATION_NEMOTRON.md` (W0–W3 отмечены ✅ с числами).

## 1. Где мы находимся

* Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
* Коммиты на 21.10.2026 (рабочее дерево чистое):
  * `968c2ab` — W0: декодер ffmpeg, обрезка тишины, `hds-core::tools`;
  * `5305fd1` — docs: W0 закрыт;
  * `f367642` — W1: `whisper_language` + постфильтр «реплик из тишины»;
  * `db9c632` — W2: ORT `2.0.0-rc.15` на воркспейс, одно окружение;
  * `6ceafe7` — **HEAD**: W3: Nemotron-3 в крейте `hds-diarize`.
* Ветка **не запушена** (`origin/server-multiuser` = `0037f43`, v0.20.6).
* Сделано: **W0, W1, W2, W3**. Осталось: **W4–W9**.

## 2. Решения владельца (не пересматривать)

1. Диаризация — **Nemotron-3-Diarization** (Sortformer v3, 8 спикеров) через `parakeet-rs` +
   ONNX Runtime; ASR — прежний whisper-large-v3-turbo через bridge движка; gigastt не берём.
2. Один проход декодирования (вариант A): ffmpeg → WAV 16 кГц mono; этот же WAV — в ASR и диаризацию.
3. Контракт `/internal/transcribe` и формат `.md` (`### SPEAKER_NN [HH:MM:SS - HH:MM:SS]`,
   литерал `UNASSIGNED`, `<stem>.speakers.json`, `<stem>.orig.md`) **не меняются**; UI не переписываем.
4. `diarization.backend: nemotron3 | sortformer | off`; sortformer остаётся fallback.
5. `whisper_language` из `index.whisper_language`, дефолт **`ru`**, `""` = авто; плюс подавление
   галлюцинаций на тишине.
6. Модель ставит установщик в машинный каталог + sha256; в репозиторий не кладём.
7. Новые зависимости — только `parakeet-rs` (+ его `ort`/`ndarray`/`hound`/`realfft`/`tokenizers`).
8. **Обрезка тишины — вариант A** (21.10.2026): диаризация по **полному** WAV, обрезка только для ASR
   (PCM-срез), offset применяется **один раз**, порог `-50 dBFS`, внутренние паузы и хвост не режутся;
   галлюцинации в паузах лечит энергетический постфильтр.
9. `chunk_min = 10`, `min_audio_min = 2`.
10. Апдейт: нет секции `diarization` → `nemotron3`, если модель есть и проходит sha256, иначе
    `sortformer` + предупреждение в лог.
11. Модель — сторонний ONNX `altunenes/parakeet-rs`, пин ревизии + sha256; свой экспорт из
    `.nemo` — не в этом плане.
12. Релиз — `v0.21.0` (MSRV 1.92, новая функциональность), тег после приёмки.

## 3. Что уже сделано и где лежит

### W0 — декодер и обрезка (коммит `968c2ab`)

* `crates/hds-diarize/src/{wav.rs,vad.rs,decode.rs}`, `src/bin/diar_wav_probe.rs`.
* `hds-core::tools::{which, find_ffmpeg}`; `hds-index::diag`/`transcribe` переведены на них (−117 строк).
* Живая приёмка: 77-мин mp4 → WAV **2.49 с**; mp4↔mp3 **lag +0 мс**; trim offset: 18 мин —
  **16.280 с**, 77 мин — 8.220 с, 56 мин — **0.000 с**; тихая первая реплика сохранена (в срезе
  первые 4 с — речь, пик −31.9 dBFS); срез 1059.96 с = 1076.24 − 16.28.

### W1 — язык ASR и постфильтр (коммит `f367642`)

* `hds-llama/src/whisper.rs`: `build_metadata(.., language)` кладёт **только** `"whisper_language"`;
  `language_from_body` (нет поля ⇒ `"ru"`, `""` ⇒ авто); `DEFAULT_WHISPER_LANGUAGE = "ru"`;
  `transcribe_file(.., language)`.
* `hds-llama/src/asr_filter.rs`: `drop_silent_segments(segments, wav: &Wav, offset_sec, cfg)` —
  отбрасываем, если доля речи < 5 % **и** суммарная речь < 150 мс (по полному WAV, offset один раз).
* `hds-index/src/transcribe.rs`: `TranscribeConfig.language: Option<String>` (`None` ≠ `Some("")`),
  уходит в тело на обоих путях (`transcribe`, `transcribe_to_file`).
* `config.example.yaml` и `server_config.example.yaml`: `whisper_language: "ru"`.
* Живой A/B 18-мин (GPU, `mode: subtitle`): без языка — **42× «Thank you», 67× «Lukoyle»**;
  с `ru` — **0/0/0**; с `ru` + обрезка — 246 сегментов, 243 оставлено, 3 отброшено
  («добрый» в полной тишине, 2 сегмента за концом файла), 44.0 с.

### W2 — ONNX Runtime, одна версия (коммит `db9c632`)

* `[workspace.package] rust-version = "1.92"`; MSRV включил 2 новых clippy-линта — поправлены
  (`hds-core/src/roots.rs` → `is_none_or`, `hds-mcp/tests/client_package.rs` → `repeat_n`).
* `hds-clip`: `ort = "=2.0.0-rc.15"`; API-дельта одна — **`Session::builder(&Environment)`**;
  добавлены `hds_clip::env::environment()` (OnceLock) и `load_with_env` у `Clip`/энкодеров.
* `hds-diarize`: `parakeet-rs = "=0.4.2"` (**дефолтные фичи**, иначе нет `ort-defaults` →
  `download-binaries`), `ort_env()`/`ort_env_clone()` (OnceLock), тест `env_is_singleton`.
* `cargo tree -i ort` → одна версия; паритет CLIP на rc.15: vision cos 0.999976/0.999950, text 1.000000;
  живой `hds clip-index` в песочнице → `images_vec` 1 строка; `llm_host status` отвечает.
* **ORT линкуется статически** (`onnxruntime*.dll` рядом с exe нет), но `DirectML.dll` копируется —
  для упаковки (W6/W9) нужен только `DirectML.dll`.

### W3 — Nemotron-3 в крейте (коммит `6ceafe7`)

* `crates/hds-diarize/src/sortformer.rs` — `Turn{start,end,speaker}`, `Device{Auto,Cpu,DirectMl,CoreMl}`,
  `NemotronConfig{model,device,chunk_min,min_audio_min,onset,offset}`, `Diarizer::{load, load_with_env,
  run_wav, run_samples, run_i16, device, config}`, `segments_to_turns`, `verify_model`, `sha256_file`,
  `default_model_path`; константы `NEMOTRON_SIZE=400_506_656`,
  `NEMOTRON_SHA256=915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a`,
  `NEMOTRON_FILE="nemotron3_diar_v3.onnx"`.
* `crates/hds-diarize/src/merge.rs` — `AsrSpan{start,end,text}`, `Block{start,end,speaker:Option<u32>,text}`,
  `merge_segments_with_turns(&[AsrSpan], &[Turn]) -> Vec<Block>`, `render_md(&[Block], unassigned)`,
  `render_srt`, `hhmmss`/`srt_time`; константы `SPLIT_MIN_SEC=3.0`, `MIN_PART_SEC=1.0`.
* `src/bin/diar_nemotron.rs` — проба (JSON совместим со спайком); `tests/nemotron_live.rs` (`#[ignore]`).
* Живой прогон 18-мин: DirectML 12.93 с (RTFx 83.2), CPU 47.47 с (**RTFx 22.7**), **5 спикеров**,
  326 turns; `chunk_min 5` == `chunk 10` == CPU == DirectML **побитово**; `frag60` → 2 спикера / 5 turns.
* Качество (методика спайка): attr **58.2 %** (57/98), attr_main 58.5 %, F1 смен **0.431**,
  **без филлеров 74.4 %** (58/78); доли спикеров 49.0/48.5/1.9/0.2/0.2 %.

## 4. Ключевые API для W4

```rust
// hds-diarize
use hds_diarize::{AsrSpan, Diarizer, NemotronConfig, TrimConfig};
use hds_diarize::{decode_to_wav, leading_silence_samples, read_wav, write_wav};
let cfg = NemotronConfig::new(model_path); // Auto, chunk 10 мин, min_audio 2 мин, onset/offset 0.5
let mut diar = Diarizer::load(&cfg)?;      // sha256+размер; ORT-сессия — лениво при первом run
let turns = diar.run_wav(&full_wav)?;      // Vec<Turn>, абсолютные секунды
let md = hds_diarize::render_md(&blocks, "UNASSIGNED");

// hds-llama
use hds_llama::asr_filter::{drop_silent_segments, FilterConfig};
use hds_llama::whisper::{language_from_body, DEFAULT_WHISPER_LANGUAGE};
let filtered = drop_silent_segments(&segments, &full_wav, trim_offset, &FilterConfig::default());
// filtered.kept / filtered.dropped / filtered.filtered_count()

// hds-core
hds_core::tools::find_ffmpeg(&cfg);        // Option<PathBuf>
```

`WhisperSegment { text, t_start, t_end }` — в `hds_llama::whisper`; для `merge` переводится в
`AsrSpan`, прибавляя `trim_offset` **ровно один раз**.

## 5. Что осталось (W4–W9)

### W4. Интеграция в `llm_host` — главный трек

Файл: `crates/hds-llama/src/host.rs`, `fn internal_transcribe` (≈ строка 2237).

Сейчас: `diarization: true` ⇒ `mode: transcript` + metadata `diarization*` (sortformer движка).
Нужно ветвление по **конфигу хоста** `diarization.backend`:

1. **`sortformer`** — прежний путь без изменений (регресс; лог `backend=sortformer (legacy)`).
2. **`nemotron3`**:
   * `[decode]` `hds_core::tools::find_ffmpeg(&cfg)` → `hds_diarize::decode_to_wav(ffmpeg, src, tmp/full.wav)`
     — **один проход** (tmp: `%TEMP%/hds_<pid>/`, чистить после);
   * `[trim]` `read_wav(full)` → `leading_silence_samples(&w.samples, w.sample_rate,
     &TrimConfig::new(-50.0, 0.3))` → `write_wav(trim.wav, w.slice_from(cut), sr)`;
     `trim_offset = cut as f64 / sr as f64`;
   * `[asr]` движку — **срез** (`trim.wav`), `mode: subtitle` (нужны таймкоды), язык из W1
     (`language_from_body`); metadata **без** `diarization*`;
   * `[diar]` `Diarizer::load(&NemotronConfig{..})` + `run_wav(&full_wav)` — **полный** WAV (вариант A);
   * `+trim_offset` к `t_start/t_end` сегментов ASR (один раз, до сшивки);
   * `[filter]` `drop_silent_segments(&segments, &full_wav, trim_offset, &FilterConfig::default())`;
   * `[merge]` `AsrSpan` из `kept` → `merge_segments_with_turns(&spans, &turns)`;
   * `render_md(&blocks, "UNASSIGNED")` → `text`, `out_ext: "md"`;
   * `stats`: `num_speakers`, `num_turns`, `filtered_segments`,
     `timings_sec{decode, asr, diar, total}`; в лог — строка на этап
     (`[diar] nemotron3: EP=DirectML, 5 спикеров, 326 turn'ов, 12.9 с`).
3. **`off`** — как `diarization: false`.

Детали, чтобы не изобретать:
* конфиг хоста: `HostConfig.config: PathBuf` (`hds-llama` читает YAML через `serde_yaml`, зависимость
  уже есть). **`hds-llama` не зависит от `hds-core`** — не тянуть его ради `dig`.
* `diarization.required: true` и сбой диаризации ⇒ задание падает; `false` ⇒ `.md` без меток +
  `diarization{enabled:false}` и предупреждение в лог.
* `transcribe_need_mib`/`prepare_demand(TRANSCRIBE_ROLE)` — только под whisper; модель Nemotron
  VRAM движка не занимает (CPU/DirectML).
* фильтр и обрезку применять **только** на nemotron3-пути (регресс sortformer не трогать).
* `hds-llama` уже зависит от `hds-diarize` — новых зависимостей не нужно.

### W5. Конфиг и диагностика

* `config.example.yaml` / `server_config.example.yaml` / `hds config init`: секция `diarization:`
  (черновик — `PLAN_DIARIZATION_NEMOTRON.md` §3.5; в серверном шаблоне `backend: nemotron3`).
* `hds check`: пункт `diarization` — backend, путь модели + sha256, EP, наличие `ffmpeg`
  (`hds_diarize::verify_model`, `default_model_path`, `hds_core::tools::find_ffmpeg`).
* `hds whisper-check`: backend/EP/число спикеров последнего прогона.
* валидация: `backend` из списка; `chunk_min ∈ 2..=30`; `device` допустим для платформы.

### W6. Установщик модели и лицензии

* `installers/fetch_nemotron3_model.ps1` (ASCII-only, PS 5.1): URL + size **400 506 656** +
  `Get-FileHash` sha256 **`915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a`**;
  каталог `<ModelsDir>/nemotron3/`; `.part` → проверка → `Move-Item`; идемпотентность; `-Force`;
  `HDS_MODELS_DIR`. URL/ревизию зафиксировать в шапке скрипта (источник `altunenes/parakeet-rs`,
  файл `nemotron-3-diarization/nemotron3_diar_v3.onnx`).
* `setup.ps1` — шаг «Модель диаризации (Nemotron-3)» + `-SkipDiarization`;
  `install_server.ps1`/`install_server.command` — машинный каталог + проверка `ffmpeg`.
* `NOTICE.md`: веса — OpenMDW 1.1 (проверить текст), `parakeet-rs` MIT OR Apache-2.0, ONNX Runtime MIT,
  `DirectML.dll` — Microsoft, sortformer — NVIDIA OML.
* упаковка: `DirectML.dll` рядом с `llm_host.exe`; `onnxruntime.dll` не нужен (ORT статичен);
  модель в архив не кладём.

### W7. Тесты и приёмка (критерии — §7)

### W8. macOS best-effort (CoreML): сборка под `aarch64-apple-darwin`, загрузка модели,
`device: auto` → CoreML с fallback на CPU; иначе статус «не проверено» в отчёте и README.

### W9. Документация и релиз: `README.md`, `README_SERVER_MULTIUSER.md`, `STATUS.md`,
`RELEASE_NOTES_v0.21.0.md`, `NOTICE.md`, версия по `RELEASING.md`, тег `v0.21.0`.

## 6. Грабли окружения (проверено, не переоткрывать)

1. **Службы держат `target\release\*.exe`.** `hds watch/mcp/ui` (службы `hermes-disk-search-*`) и
   `llm_host` (задача `HermesDiskSearchLlmHost`) запущены; без админа их не остановить ⇒ пересборка
   релиза падает «отказано в доступе». Решение: `cargo build --release --target-dir target\w2 …`;
   боевой стенд остаётся на v0.20.6.
2. **`HDS_ROOT` выставляет среда ассистента** (`<repo>`), из-за чего 2 теста `hds-llama`
   (`host_resident`) падают. Перед тестами: `Remove-Item Env:\HDS_ROOT` в **той же** команде.
3. **PowerShell искажает кириллицу** в stdout и при `>`: проверки делать через `--json`-опции
   (пробы, `eval_diar.py --json`) или читать файлы Python-ом.
4. **`Select-Object -First N`** обрывает пайплайн → процесс получает exit 1; длинные прогоны писать в файл.
5. **`Engine::activate()` меняет CWD** процесса: в пробах пути делать абсолютными **до** активации
   (`std::path::absolute`) — на этом уже потеряли `--json`.
6. **`ort 2.0.0-rc.15`**: `Session::builder(&Environment)`; один `Environment` на процесс
   (`hds_clip::env::environment`, `hds_diarize::ort_env`); ORT статичен; `DirectML.dll` копируется рядом.
7. **`parakeet-rs`**: пин `=0.4.2`; дефолтные фичи обязательны (`ort-defaults` → `download-binaries`);
   `feed()`/`flush()` дают абсолютные таймкоды.
8. **Единицы**: `min_audio_min` — минуты (баг W3 был именно здесь).
9. **Эталон шумный** (`REPORT.md` §11.3): метрики только относительные, филлеры исключать,
   абсолютный DER не публиковать.
10. **Материалы**: модель `D:\diar-spike-models\nemotron3_diar_v3.onnx`; исходники —
    `D:\САША\ИРИС\ИИ\Тестовые записи совещаний\Новый каталог`; WAV — `D:\diar-spike-data\`.

## 7. Критерии приёмки

```powershell
Remove-Item Env:\HDS_ROOT -ErrorAction SilentlyContinue
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings     # 0/0
cargo test --workspace
```
1. 18-мин файл через `hds transcribe-once`/`transcribe-watch`: `.md` с `### SPEAKER_NN`, **5 спикеров**,
   `UNASSIGNED` только вне речей; `<stem>.orig.md` и `<stem>.speakers.json` на месте; UI открывает файл
   и переименовывает `SPEAKER_00 → Иван` (`.bak`).
2. 56 мин: не падает, спикеров ≥ 5, `stats.timings_sec` заполнен (обрезка no-op).
3. 77 мин: не падает, **RSS ≤ 1.5 ГБ** (чанкинг), **8 спикеров**, диаризация ≤ 5 мин на CPU.
4. ASR: **0** «Thank you»/«Obrigado»/«Lukoyle», 0 реплик в тишине, «Добрый день» на месте.
5. Регресс `diarization.backend: sortformer` (спаны/спикеры как раньше) + паритеты `pipeline_parity`,
   `walk_parity`.
6. Качество на 18-мин (эталон без филлеров, 78 реплик): атрибуция **≥ 71.8 %**, спикеров ≥ 5
   (на уровне крейта уже 74.4 %).
7. Таймкоды абсолютные; trim-offset применён ровно один раз.

Производительность: CPU RTFx ≥ 15 (сейчас 22.7), DirectML ≥ 3× CPU на файлах ≥ 10 мин (сейчас 3.7×).

## 8. Артефакты (гитигнор, `tools/parity/out/`)

* `diar_nemotron/w0_*.json|wav` — декод/обрезка (18/56/77 мин, mp4/mp3).
* `diar_nemotron/w1_asr_*.json|log`, `w1_stats.py`, `w1_starts.py` — язык ASR и постфильтр.
* `diar_nemotron/w3_*.json` (`frag60`, 18 мин DML/CPU/chunk5), `w3_flat.json`, `w3_eval.json`,
  `w3_eval_filtered.py` — прогоны крейта и метрики.
* `w2_clip/{sandbox,check_db.py}` — живая проверка CLIP на rc.15.
* `diar_spike/*` — спайк: `REPORT.md`, скрипты (`flatten.py`, `eval_diar.py`, …), эталонные JSON.

## 9. Быстрые команды проверки

```powershell
Remove-Item Env:\HDS_ROOT -ErrorAction SilentlyContinue
.\target\w2\release\diar_nemotron.exe D:\diar-spike-models\nemotron3_diar_v3.onnx `
    D:\diar-spike-data\meet0924.wav --json tools\parity\out\diar_nemotron\check.json
cd tools\parity\out\diar_spike
python flatten.py ..\diar_nemotron\check.json ..\diar_nemotron\check_flat.json
python eval_diar.py ref_20260924__ERP.SPS._.json ..\diar_nemotron\check_flat.json --json ..\diar_nemotron\check_eval.json
cd ..\..\..
python tools\parity\out\diar_nemotron\w3_eval_filtered.py `
    tools\parity\out\diar_spike\ref_20260924__ERP.SPS._.json tools\parity\out\diar_nemotron\check_flat.json
```

## 10. Промт для нового чата

```text
Продолжаем перевод диаризации в hermes-disk-search с sortformer на Nemotron-3 (Sortformer v3,
8 спикеров) через parakeet-rs + ONNX Runtime. Этапы W0–W3 выполнены и закоммичены, дальше — W4–W9.

Сначала прочитай целиком:
  C:\Users\Sasha\hermes-disk-search\DIARIZATION_NEMOTRON_W3_HANDOFF.md    (состояние, решения, что
      сделано с числами, где менять код, грабли окружения, критерии приёмки, промт)
  C:\Users\Sasha\hermes-disk-search\PLAN_DIARIZATION_NEMOTRON.md          (план W0–W9, §3 контракты)
  C:\Users\Sasha\hermes-disk-search\DIARIZATION_NEMOTRON_HANDOFF.md       (исходный спайк и решения)
  C:\Users\Sasha\hermes-disk-search\tools\parity\out\diar_spike\REPORT.md  (числа спайка, §11)

Репозиторий: C:\Users\Sasha\hermes-disk-search, ветка server-multiuser, HEAD 6ceafe7 (W3), дерево чистое.
Ядро — Rust (crates/), Python только sidecar. Это dev-машина (cline-rules/dev-machine.md): процессы можно
останавливать/запускать, GPU/VRAM освобождать, релиз пересобирать. Правила проекта: комментарии и тексты
по-русски; никаких новых зависимостей сверх списка из решений (только parakeet-rs и его транзитивные);
clippy -D warnings; каждый шаг — тесты + живой прогон; артефакты — в tools/parity/out/.
Ничего из «Решений владельца» (§2 handoff) не пересматривай.

Сделано: W0 (декодер ffmpeg + обрезка ведущей тишины, hds-core::tools), W1 (whisper_language и
постфильтр «реплик из тишины»), W2 (ort 2.0.0-rc.15, одно ORT-окружение), W3 (крейт hds-diarize:
Diarizer, чанкинг feed()/flush(), merge/render_md/render_srt, проба diar_nemotron). Числа:
18-мин файл — 5 спикеров, DirectML 12.9 с (RTFx 83), CPU 47.5 с (RTFx 22.7), attr 58.2 %,
без филлеров 74.4 %; chunk 5 == chunk 10 == CPU == DirectML побитово.

Порядок работы:
1) W4 — главный трек: ветка nemotron3 в crates/hds-llama/src/host.rs::internal_transcribe по конфигу
   diarization.backend: decode (ffmpeg, один проход) -> trim (PCM-срез только для ASR, offset один раз)
   -> ASR mode: subtitle с языком -> Diarizer::run_wav по полному WAV -> drop_silent_segments ->
   merge_segments_with_turns -> render_md в text + out_ext "md"; stats num_speakers/timings_sec;
   путь sortformer не меняется (регресс). Живой прогон 18-мин файла: .md с 5 спикерами,
   sidecar/.orig.md, переименование спикеров в UI; проверить 0 «Thank you»/«Obrigado»/«Lukoyle»
   и 0 реплик, целиком лежащих в тишине.
2) W5 — конфиг diarization.*, hds check, hds whisper-check, образцы конфигов.
3) W6 — installers/fetch_nemotron3_model.ps1 (sha256 915e4fa2…be03a, size 400506656), setup.ps1
   (-SkipDiarization), install_server.*, NOTICE (OpenMDW 1.1), DirectML.dll в пакет.
4) W7 — приёмка по §7 handoff (18/56/77 мин, RSS ≤ 1.5 ГБ, 8 спикеров, attr ≥ 71.8 %, регресс sortformer,
   паритеты индексации), отчёт tools/parity/out/diar_nemotron/report.md.
5) W8 — macOS best-effort (CoreML); W9 — доки и релиз v0.21.0.

Каждый трек: юниты + живой прогон, обновить PLAN_DIARIZATION_NEMOTRON.md (чек-боксы и фактические числа),
коммит с русским сообщением (стиль `feat(diarize): W4 — …`). Перед cargo test в этой среде вычищай
HDS_ROOT: `Remove-Item Env:\HDS_ROOT`; релиз собирай в target\w2 (службы держат target\release).
Начни с чтения §3–§6 handoff, затем задай уточняющие вопросы и приступай к W4.
```
