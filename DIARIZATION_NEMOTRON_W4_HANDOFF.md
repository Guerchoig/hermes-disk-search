# DIARIZATION_NEMOTRON_W4_HANDOFF.md — продолжение работ (после W0–W4)

> Скопируйте §8 (промт) в новый чат. Документ самодостаточен: состояние репозитория,
> решения владельца, что уже сделано (с числами), где именно в коде менять W5–W9,
> грабли окружения, критерии приёмки, воспроизведение живого прогона.
>
> Предыдущие документы: `DIARIZATION_NEMOTRON_W3_HANDOFF.md` (состояние после W3, W4–W9
> целиком), `DIARIZATION_NEMOTRON_HANDOFF.md` (спайк и решения), `PLAN_DIARIZATION_NEMOTRON.md`
> (план W0–W9, §3 контракты; W0–W4 отмечены ✅ с фактическими числами).

## 1. Где мы находимся

* Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
* Коммиты (рабочее дерево чистое):
  * `968c2ab` — W0: декодер ffmpeg, обрезка тишины, `hds-core::tools`;
  * `5305fd1` — docs: W0 закрыт;
  * `f367642` — W1: `whisper_language` + постфильтр «реплик из тишины»;
  * `db9c632` — W2: ORT `2.0.0-rc.15` на воркспейс, одно окружение;
  * `6ceafe7` — W3: Nemotron-3 в крейте `hds-diarize`;
  * `2b703da` — docs: handoff после W0–W3;
  * `a5cbc9e` — W4: ветка nemotron3 в `llm_host`;
  * последний коммит — docs: этот handoff (хеш не фиксируем, он меняется при правках).
* `origin/server-multiuser` = `0037f43` (v0.20.6); локально **впереди на 8** коммитов, ветка
  не запушена.
* Сделано: **W0, W1, W2, W3, W4**. Осталось: **W5–W9**.
* Модель Nemotron-3 на dev-машине: `D:\diar-spike-models\nemotron3_diar_v3.onnx`
  (400 506 656 байт, sha256 `915e4fa2…be03a`); в машинные каталоги её ставит W6.
* Декодированные WAV спайка: `D:\diar-spike-data\{meet0924.wav, meet0514.wav, meet0512.wav,
  frag60.wav, meet0924_mp3.wav}` — нужны для приёмки без повторного декодирования.
* Артефакты W4: `tools/parity/out/diar_nemotron/` (гитигнорится) — см. §7.

## 2. Решения владельца (не пересматривать)

1. Диаризация — **Nemotron-3-Diarization** (Sortformer v3, 8 спикеров) через `parakeet-rs` +
   ONNX Runtime; ASR — прежний whisper-large-v3-turbo через bridge движка; gigastt не берём.
2. Один проход декодирования (вариант A): ffmpeg → WAV 16 кГц mono; этот же WAV — в ASR и
   диаризацию (ASR получает PCM-срез ведущей тишины).
3. Контракт `/internal/transcribe` и формат `.md` (`### SPEAKER_NN [HH:MM:SS - HH:MM:SS]`,
   литерал `UNASSIGNED`, `<stem>.speakers.json`, `<stem>.orig.md`) **не меняются**; UI не
   переписываем. Новые ключи ответа — только аддитивные.
4. `diarization.backend: nemotron3 | sortformer | off`; sortformer остаётся fallback.
5. `whisper_language` из `index.whisper_language`, дефолт **`ru`**, `""` = авто; плюс подавление
   галлюцинаций на тишине.
6. Модель Nemotron-3 ставит установщик в машинный каталог + sha256; в репозиторий не кладём.
7. Новые зависимости — только `parakeet-rs` (+ его `ort`/`ndarray`/`hound`/`realfft`/`tokenizers`).
8. **Обрезка тишины — вариант A**: диаризация по **полному** WAV, обрезка только для ASR
   (PCM-срез), offset применяется **один раз**, порог `-50 dBFS`, внутренние паузы и хвост не
   режутся; галлюцинации в паузах лечит энергетический постфильтр.
9. `chunk_min = 10`, `min_audio_min = 2`.
10. Апдейт: нет секции `diarization` → `nemotron3`, если модель есть и проходит sha256, иначе
    `sortformer` + предупреждение в лог.
11. Модель — сторонний ONNX `altunenes/parakeet-rs`, пин ревизии + sha256; свой экспорт из
    `.nemo` — не в этом плане.
12. Релиз — `v0.21.0` (MSRV 1.92, новая функциональность), тег после приёмки.
13. **Приоритет backend (21.10.2026, W4):** тело `diarization_backend` → конфиг
    `diarization.backend` → авто (решение №10). Поэтому `auto_transcribe.diarization_backend`
    (клиент всегда шлёт его в теле) **переключаем на `nemotron3` в W5** — иначе установки
    останутся на legacy-пути.


## 3. Что уже сделано и где лежит

### W0 — декодер и обрезка (коммиты `968c2ab`, `5305fd1`)

* `crates/hds-diarize/src/{wav.rs,vad.rs,decode.rs}`, проба `src/bin/diar_wav_probe.rs`.
* `hds-core::tools::{which, find_ffmpeg}`; `hds-index::{diag,transcribe}` переведены на них.
* Живая приёмка: 77-мин mp4 → WAV **2.49 с** (< 30 с); mp4↔mp3 **lag +0 мс**; trim offset:
  18 мин — **16.280 с**, 77 мин — 8.220 с, 56 мин — **0.000 с**; первая тихая реплика сохранена
  (в срезе первые 4 с — речь, пик −31.9 dBFS); срез 1059.96 с = 1076.24 − 16.28.

### W1 — язык ASR и постфильтр (коммит `f367642`)

* `hds-llama/src/whisper.rs`: `build_metadata(.., language)` кладёт **только** `"whisper_language"`;
  `language_from_body` (нет поля ⇒ `"ru"`, `""` ⇒ авто); `DEFAULT_WHISPER_LANGUAGE = "ru"`;
  `transcribe_file(.., language)`.
* `hds-llama/src/asr_filter.rs`: `drop_silent_segments(segments, wav: &Wav, offset_sec, cfg)` —
  отбрасываем, если доля речи < 5 % **и** суммарная речь < 150 мс (по полному WAV, offset один раз).
* `hds-index/src/transcribe.rs`: `TranscribeConfig.language: Option<String>` на обоих путях
  (`transcribe`, `transcribe_to_file`).
* Живой A/B 18-мин: без языка — **42× «Thank you», 67× «Lukoyle»**; с `ru` — **0/0/0**; с `ru` +
  обрезка — 246 сегментов, 243 оставлено, **3 отброшено**.

### W2 — ONNX Runtime (коммит `db9c632`)

* `ort = "=2.0.0-rc.15"` (транзитивно из `parakeet-rs =0.4.2`), одно ORT-окружение
  (`hds_diarize::ort_env()` — `OnceLock`), `hds-clip` переведён на rc.15 (паритет cos 0.999976,
  text 1.000000).
* `DirectML.dll` появляется рядом с бинарниками (`ort` → `copy-dylibs`) — **нужен в пакете** (W6);
  `onnxruntime*.dll` не нужны (ORT линкуется статически).

### W3 — крейт `hds-diarize` (коммит `6ceafe7`)

* `sortformer.rs`: `Diarizer::load`, `run_wav`/`run_i16`, чанкинг `feed()`/`flush()`, EP по
  платформе (`auto` — GPU только для файлов ≥ `min_audio_min`), `verify_model` (size → sha256,
  кэш), `default_model_path` (`HDS_MODELS_DIR` → `%APPDATA%\OpenResearchTools\models\nemotron3\`).
* `merge.rs`: `merge_segments_with_turns`, `render_md`, `render_srt` (формат движка), `hhmmss`.
* Проба `src/bin/diar_nemotron.rs` (модель + WAV → JSON turns).
* Числа: 60 с — CPU 4.68 с (RTFx 12.8), 2 спикера; 18 мин — DirectML **12.93 с (RTFx 83.2)**,
  CPU 47.47 с (**RTFx 22.7**), 326 turns, **5 спикеров**; `chunk_min` 5 == 10 == CPU == DirectML
  **побитово**; атрибуция 58.2 % (без филлеров **74.4 %**, порог 71.8 %); 29 тестов крейта.

### W4 — интеграция в `llm_host` (коммит `a5cbc9e`)

Код (1206 вставок, 36 удалений):
* **`crates/hds-llama/src/diar.rs` (новый, 763 строки)** — разбор `diarization.*`
  (`DiarSettings{nemotron3, asr, required, backend}`, предупреждения вместо падения),
  `Backend::{Nemotron3,Sortformer,Off}`, `pick_backend` (тело → конфиг → авто),
  `DiarSettings::{nemotron_config, trim_config, filter_config}`,
  `merge_absolute` (offset один раз), `speaker_count`, `NemotronStats/NemotronTimings::to_json`,
  `configured_ffmpeg`/`find_ffmpeg`/`which` (порядок как в `hds-core::tools`), `TempDir`.
* **`host.rs` (+378)**: `ClusterBackend` получил поля `diar`, `diar_auto: OnceLock<DiarBackend>`,
  `nemotron: Mutex<Option<Diarizer>>`, `ffmpeg`; методы `pick_diar_backend` (2499),
  `resolved_diar_backend` (2527, авто + лог), `run_nemotron` (2558, кэш ORT-сессии),
  `transcribe_nemotron3` (2592). В `internal_transcribe`: ветка nemotron3 идёт **после**
  `prepare_demand` (VRAM считает только whisper), `off` превращает `transcript` → `speech`,
  sortformer-ветка не тронута (`backend=sortformer (legacy)` в логе).
* **`lib.rs`**: `pub mod diar;` + `pub use diar::{Backend as DiarBackend, DiarSettings};`.
* Юниты **+12** (`diar::tests`): секция/предупреждения, `pick_backend`, offset-инвариант,
  `speaker_count`, `stats` поверх ключей движка, `find_ffmpeg`, `TempDir`, `nemotron_config`.

Живой прогон 18-мин файла (песочница `llm_host` на :8110, `target\w2\release`):

| Этап | Время | Факт |
|---|---|---|
| декод ffmpeg | 0.62–0.76 с | 1076.2 с аудио (winget `ffmpeg 9.0.1`) |
| обрезка ASR | 0.07 с | **16.280 с**, срез 1060.0 с |
| ASR (движок, `ru`) | 43.7–53.2 с | 313 сегментов, **3 отброшено** постфильтром |
| диаризация DirectML | 9.8–13.7 с | 326 turns, 5 спикеров |
| итого | 56.7–67.9 с | RTFx ≈ 15.9–19.0 |

* `hds transcribe-once`: **197 реплик**, `SPEAKER_00…04` + 34 `UNASSIGNED`, созданы
  `<stem>.orig.md` и `<stem>.speakers.json` (5 слотов + UNASSIGNED).
* `/internal/transcribe` напрямую: `out_ext: "md"`, `diarization{backend: nemotron3,
  enabled: true, num_speakers: 5}`, 310 сегментов, первый сегмент **17.42 с**
  (= 16.28 offset + 1.14), последний 1076.57 с.
* **0** «Thank you»/«Obrigado»/«Lukoyle»; по **точным** таймкодам — **0** реплик, целиком в тишине.
* Регресс sortformer тем же файлом (тело переопределило конфиг): 4 спикера + `UNASSIGNED`,
  `num_speaker_spans: 629`, формат прежний, 0 галлюцинаций.
* Память: прирост RSS `llm_host` за задание — **+780 МБ** (пик). Полный RSS ~8.5 ГБ — в нём
  предзагруженный чат-инстанс движка (`KEEP_LOADED`, прежнее поведение).
* `cargo fmt --check` / `clippy --workspace --all-targets -D warnings` / `test --workspace` —
  зелёные; `cargo test -p hds-ui` (переименование спикеров, sidecar, `.bak`) — зелёный.

## 4. Где именно менять код (W5–W9)

### W5 — конфиг и диагностика

1. **Дефолт backend у клиента**: `crates/hds-index/src/transcribe.rs::from_auto_transcribe`
   (184–245) — `backend: ... .unwrap_or_else(|| "sortformer".to_string())` (234–235) заменить
   на `"nemotron3"` + комментарий про приоритет тела (решение №13). Тесты — `hds-index`.
2. **Доступ к модели из `hds-index`**: добавить в `crates/hds-index/Cargo.toml`
   `hds-diarize = { path = "../hds-diarize" }` — в бинарь `hds.exe` ORT уже входит
   (`hds-ui → hds-llama → hds-diarize`, проверено `cargo tree -i hds-llama`), новых внешних
   зависимостей нет. Тогда доступны `hds_diarize::{default_model_path, verify_model,
   NEMOTRON_FILE, NEMOTRON_SHA256, NEMOTRON_SIZE}`. Альтернатива (если не хочется связывать
   индексный слой с ONNX) — продублировать поиск пути по каталогам в `hds-index`
   (`autotranscribe::models_roots`, 578), но константы sha256/размера всё равно держать в одном
   месте.
3. **Поиск модели**: рядом с `diarization_model_for` (632) добавить
   `nemotron_model_for(cfg)` — `diarization.nemotron3.model` → `HDS_MODELS_DIR` →
   `%ProgramData%\hermes-disk-search\models\nemotron3\` → `%APPDATA%\OpenResearchTools\models\nemotron3\`
   (имя файла `nemotron3_diar_v3.onnx`).
4. **`hds check`**: `crates/hds-index/src/diag.rs::check_diarization` (883) — пункт `diarization`
   должен показывать: backend (тело→конфиг→авто считать как в `host::pick_diar_backend`; для
   диагностики достаточно `diarization.backend` + «авто»), путь модели, **sha256 ok**
   (`verify_model`), EP (`diarization.nemotron3.device`), наличие `ffmpeg` (готовый
   `check_ffmpeg`, 716). Испускать `fail`, если `backend=nemotron3` и модели нет; `warn` — если
   секции нет и сработал legacy-fallback. `run_checks` (1945) — порядок не менять.
5. **`hds whisper-check`**: `crates/hds-cli/src/cmd/whisper_check.rs` — `report_static` (119) и
   `print_static` (149): печатать `diarization.backend`, EP, путь/размер Nemotron-модели,
   а для legacy — пометку «sortformer (legacy)». JSON-ключи только добавлять.
6. **Образцы конфигов**: `config.example.yaml` (350–380) — секция
   ```yaml
   diarization:
     backend: nemotron3        # nemotron3 | sortformer | off
     required: true
     nemotron3: { model: '', device: auto, chunk_min: 10, min_audio_min: 2, onset: 0.5, offset: 0.5 }
     asr: { trim_leading_silence: true, silence_threshold_db: -50, trim_lead_in_sec: 0.3,
            drop_silent_segments: true }
   ```
   и `auto_transcribe.diarization_backend: nemotron3`; `server_config.example.yaml` (49–57) —
   то же с плейсхолдерами; `hds/config.py` (130–140, Python-шаблон) — синхронизировать текст.
   Серверный `hds config init` рендерит `server_config.example.yaml`
   (`crates/hds-cli/src/cmd/config.rs::render_server_config`, 309).
7. **Валидация**: `hds check` + стартовый лог хоста уже пишут предупреждения
   (`diar::DiarSettings::from_yaml`); жёсткие ошибки (модель не найдена при явном `nemotron3`)
   — `fail` в `hds check`, но не отказ старта хоста.

### W6 — установка модели и лицензии

1. **Новый `installers/fetch_nemotron3_model.ps1`** (образец —
   `installers/fetch_diarization_model.ps1`: ASCII-only, PS 5.1, `-ModelsDir`, `-Force`,
   `HDS_MODELS_DIR` → `%ProgramData%\hermes-disk-search\models` → `%APPDATA%\OpenResearchTools\models`):
   каталог `<ModelsDir>/nemotron3/`, файл `nemotron3_diar_v3.onnx`; скачивание в `.part` →
   проверка размера **400 506 656** и `Get-FileHash -Algorithm SHA256` =
   **`915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a`** → `Move-Item`
   (идемпотентно: существующий файл с верным размером/хешем пропускается, `-Force` качает заново).
   URL и **ревизию** Hugging Face зафиксировать и записать в шапку скрипта (источник —
   `altunenes/parakeet-rs`, файл `nemotron-3-diarization/nemotron3_diar_v3.onnx`; каноническая
   форма `https://huggingface.co/altunenes/parakeet-rs/resolve/<REVISION>/nemotron-3-diarization/nemotron3_diar_v3.onnx`
   — `<REVISION>` определить при реализации и пиновать, не `main`).
2. **`setup.ps1`** (корень): `param` уже содержит `-SkipDiarization` (строка 25) — добавить
   вызов нового скрипта рядом с `fetch_diarization_model.ps1` и гейт `-not $SkipDiarization`;
   текст шага обновить («Sortformer GGUF + Nemotron-3 ONNX»).
3. **`installers/install_server.ps1`**: в список `fetch_*` (рядом с
   `fetch_diarization_model.ps1`) добавить `fetch_nemotron3_model.ps1`; в `install_server.command`
   (macOS) — если модель поддерживается на macOS, иначе только Windows-путь + предупреждение.
4. **`installers/build_rust_release.ps1`**: (а) бинарники — список на строке 41 (`hds`,
   `hds_mcp`, `llm_host`); (б) **скопировать `DirectML.dll`** из `target\release` в `bin\`
   (появляется там от `ort` `copy-dylibs`; без него Diarizer/DirectML на машине без SDK не
   поднимется); (в) убедиться, что `*.onnx` не попадает в архив.
5. **`NOTICE.md`** (§ «Модели», строка 57): добавить Nemotron-3-Diarization (OpenMDW 1.1 —
   проверить текст лицензии и записать вывод), `parakeet-rs` (MIT OR Apache-2.0), ONNX Runtime
   (MIT), DirectML (Microsoft, поставляется пакетом `ort`), sortformer 4spk-v2.1 (NVIDIA Open
   Model License — пока остаётся); отдельной строкой: модель в репозиторий не входит.
6. **`.gitignore`**: `*.onnx` не должен попадать в git и в релизный zip (проверить
   `git check-ignore`).


### W7 — тесты и приёмка

1. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` — зелёные (после `Remove-Item Env:\HDS_ROOT`).
2. Живые прогоны на 18 / 56 / 77 мин (файлы — `D:\САША\ИРИС\ИИ\Тестовые записи совещаний\
   Новый каталог\`; WAV — `D:\diar-spike-data\`). Диаризация отдельно:
   `target\w2\release\diar_nemotron.exe <model.onnx> <wav> <out.json>`; сквозной путь — песочница
   `llm_host` + `hds transcribe-once` / POST `/internal/transcribe` (§7).
3. **RSS ≤ 1.5 ГБ — мерить дельтой**: хост на старте грузит чат-инстанс (`KEEP_LOADED`,
   `host.rs` ~3337), поэтому абсолютный RSS ~8.5 ГБ. Рецепт: снять `WorkingSet64` до задания,
   семплировать каждые ~1 с во время прогона, в отчёт — `peak − base` (W4 на 18 мин: **+780 МБ**).
4. Качество: `python tools\parity\out\diar_spike\flatten.py <out.json> <flat.json>` +
   `python tools\parity\out\diar_spike\eval_diar.py
   tools\parity\out\diar_spike\ref_20260924__ERP.SPS._.json <flat.json> --json <eval.json>`;
   атрибуция **≥ 71.8 %** на эталоне без филлеров (W3: 74.4 %); в отчёт — оговорку `REPORT.md`
   §11.3 (метрики относительные, абсолютный DER не публикуем).
5. Производительность: CPU RTFx ≥ 15 на ≥10 мин; `device: auto` на Windows — DirectML ≥ 3× CPU.
6. Регресс: `sortformer` на 18/77 мин (тело `diarization_backend: "sortformer"`) — прежние
   спаны/спикеры; паритеты индексации `pipeline_parity`, `walk_parity` — без изменений;
   `cargo test` по `hds-index`.
7. Отчёт `tools/parity/out/diar_nemotron/report.md` со сводными таблицами (18/56/77, CPU/DML,
   attr, RSS, паритеты, `chunk_min` 5 vs 10).

### W8 — macOS best-effort

* `crates/hds-diarize/src/sortformer.rs`: `Diarizer::load` (250) и `choose_device` (277) —
  `Device::CoreMl` с fallback на CPU (лог причины); фичи `parakeet-rs` для macOS
  (`coreml`) уже прописаны в `Cargo.toml`.
* CI-джоб macOS уже `continue-on-error`; при недоступной машине — статус «не проверено» в отчёте
  и `README` (не выдавать за проверенное).

### W9 — документация и релиз

* `README.md` (раздел «Автотранскрибация»): модель Nemotron-3 (размер, лицензия, установщик),
  требования (`ffmpeg`), `diarization.backend`, «8 спикеров вместо 4», язык ASR.
* `README_SERVER_MULTIUSER.md`: машинный каталог модели, установка на сервере, `-SkipDiarization`.
* `STATUS.md`: блок «Диаризация Nemotron-3» (треки, числа приёмки).
* `NOTICE.md` (из W6), `RELEASE_NOTES_v0.21.0.md`; версия по `RELEASING.md` (версия продукта
  передаётся в `build_rust_release.ps1`, крейты воркспейса остаются `0.1.0`); тег — после
  зелёной приёмки; обновить `PLAN_DIARIZATION_NEMOTRON.md` и этот handoff.

## 5. Грабли окружения (проверено на этой машине)

1. **`HDS_ROOT`**: среда ассистента выставляет `HDS_ROOT=<repo>`; перед `cargo test` —
   `Remove-Item Env:\HDS_ROOT`, иначе 2 теста `hds-llama::host_resident` падают (код не при чём).
2. **`target\release` занят** резидентом/службами: релиз собирать в отдельный каталог,
   например `cargo build --release --target-dir target\w2 …` (так сделаны W3/W4).
3. **`--port-base N` — это БАЗА, не смещение**: чат получает порт `N`, embedding `N+1`,
   rerank `N+2` (на `--port-base 100` хост слушал 100/101/102, а не 8110). Для песочницы брать
   `--port-base 8110` и такой же `index.transcribe_url`.
4. **Хост всегда грузит чат-инстанс на старте** (`KEEP_LOADED`, `host.rs` ~3337), независимо от
   `llm.chat.retention` (допустимые значения — `keep_loaded` / `load_on_demand`, значение
   `on_demand` даёт предупреждение и дефолт). Следствие: RSS процесса ~8 ГБ; память диаризации
   мерить **дельтой**.
5. **`.md` округляет таймкоды до секунды вниз** — проверка «реплика в тишине» по заголовкам даёт
   ложные срабатывания (W4: 2 из 197). Проверять по `segments[]` ответа (`w4_verify.py`), а
   `.md`-проверка (`w4_check.py`) — только как грубый фильтр.
6. **Тело `diarization_backend` сильнее конфига** (решение №13), а `hds-index` всегда шлёт его из
   `auto_transcribe.diarization_backend` (дефолт `sortformer`) ⇒ без переключения дефолта в W5
   установки останутся на legacy.
7. **Модель** на dev-машине лежит в `D:\diar-spike-models\nemotron3_diar_v3.onnx`; автопоиск
   смотрит `HDS_MODELS_DIR` → `%ProgramData%\hermes-disk-search\models\nemotron3\` →
   `%APPDATA%\OpenResearchTools\models\nemotron3\`. Для песочницы путь задавать явно.
8. **Движок**: `--engine-dir C:\ProgramData\hermes-disk-search\engine`; `ffmpeg` из winget
   (`…\Gyan.FFmpeg_…\bin\ffmpeg.exe`) находится `diar::find_ffmpeg` без настройки.
9. **`DirectML.dll`** лежит рядом с собранными бинарниками (`target\w2\release\DirectML.dll`) —
   в W6 его надо положить в `bin\` пакета; `onnxruntime*.dll` не нужны (ORT статический).
10. **Версии**: `parakeet-rs = "=0.4.2"` (точно), `ort = "=2.0.0-rc.15"` (транзитивно);
    обновление — отдельным решением (ломает API/паритеты).
11. **PowerShell-скрипты установщиков — ASCII-only** (PS 5.1 читает BOM-less `.ps1` как ANSI).
12. **`hds transcribe-once` требует `inbox_dir`/`out_dir`** (валидация `AutoTranscribeConfig`),
    даже при работе «здесь и сейчас»: в песочнице задать оба + `state_dir`.
13. **Остановка песочницы**: `Stop-Process -Id (Get-Content host_w4.pid) -Force` (или
    `/internal/stop`); после работ не оставлять процессов (они держат ~8 ГБ RAM).


## 6. Критерии приёмки (W7)

* `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` — зелёные.
* Живые прогоны **18 / 56 / 77 мин**: `.md` с `### SPEAKER_NN`, на 18 мин — **5 спикеров**;
  на 77 мин — до **8** (в W3 ровно 8, из них 5 с долей ≥ 5 %).
* **RSS ≤ 1.5 ГБ** на 77 мин — прирост за задание (абсолютный RSS хоста включает чат-модель).
* Атрибуция на эталоне без филлеров — **≥ 71.8 %** (W3: 74.4 %), спикеров ≥ 5.
* CPU RTFx ≥ 15 (≥ 10 мин), DirectML ≥ 3× CPU (W3: 22.7 и 83.2).
* Регресс sortformer (18/77 мин) и паритеты индексации (`pipeline_parity`, `walk_parity`).
* 0 «Thank you»/«Obrigado»/«Lukoyle»; 0 реплик, целиком лежащих в тишине (по точным таймкодам).
* Отчёт `tools/parity/out/diar_nemotron/report.md` с числами и оговоркой §11.3 спайка.

## 7. Артефакты и воспроизведение W4

`tools/parity/out/diar_nemotron/` (гитигнорится):
* `w4_host.yaml` — песочница `llm_host` (порт 8110, `diarization.backend: nemotron3`,
  `auto_transcribe.diarization_backend: nemotron3`, свои `inbox/out/state_dir`);
* `host_w4.log` (+ ротации `.1/.2/.3`) — лог песочницы (строки `[diar]`/`[transcribe]`);
* `w4_resp.json` / `w4_resp_sortformer.json` — ответы `/internal/transcribe` (nemotron3/sortformer),
  рядом `.md`;
* `w4_out/*.md`, `w4_state/*.orig.md`, `w4_state/*.speakers.json` — результат `hds transcribe-once`;
* `w4_check.json`, `w4_post.py`, `w4_check.py`, `w4_verify.py` — проверки (метрики/тишина).

```powershell
# 0) окружение
Remove-Item Env:\HDS_ROOT -ErrorAction SilentlyContinue
cd C:\Users\Sasha\hermes-disk-search

# 1) релизные бинарники в отдельный каталог (target\release держат службы)
cargo build --release --target-dir target\w2 -p hds-llama --bin llm_host
cargo build --release --target-dir target\w2 -p hds-cli --bin hds

# 2) песочница llm_host (порт = база 8110)
$log = 'C:\Users\Sasha\hermes-disk-search\tools\parity\out\diar_nemotron\host_w4.log'
$p = Start-Process -FilePath 'target\w2\release\llm_host.exe' -ArgumentList @(
  'run','--config','tools\parity\out\diar_nemotron\w4_host.yaml','--port-base','8110',
  '--no-residency','--log',$log,'--pause-dir','tools\parity\out\diar_nemotron\w4_state',
  '--engine-dir','C:\ProgramData\hermes-disk-search\engine') -PassThru -WindowStyle Hidden
$p.Id | Set-Content tools\parity\out\diar_nemotron\host_w4.pid
Start-Sleep 30   # движок + стартовая загрузка чат-инстанса

# 3) сквозной путь (sidecars .orig.md/.speakers.json): hds-index -> хост
$env:HDS_CONFIG = 'C:\Users\Sasha\hermes-disk-search\tools\parity\out\diar_nemotron\w4_host.yaml'
target\w2\release\hds.exe transcribe-once "<18-мин mp4>"

# 4) прямой ответ с ТОЧНЫМИ таймкодами (для проверки тишины/offset)
python tools\parity\out\diar_nemotron\w4_post.py http://127.0.0.1:8110 "<18-мин mp4>" `
  tools\parity\out\diar_nemotron\w4_resp.json nemotron3
python tools\parity\out\diar_nemotron\w4_verify.py `
  tools\parity\out\diar_nemotron\w4_resp.json D:\diar-spike-data\meet0924.wav

# 5) замер памяти (дельта; хост уже держит чат-модель)
$hostPid = [int](Get-Content tools\parity\out\diar_nemotron\host_w4.pid)
$base = (Get-Process -Id $hostPid).WorkingSet64; $max = 0
$job = Start-Job { Set-Location 'C:\Users\Sasha\hermes-disk-search';
  python tools\parity\out\diar_nemotron\w4_post.py http://127.0.0.1:8110 "<mp4>" `
    tools\parity\out\diar_nemotron\w4_resp.json nemotron3 }
while ($job.State -eq 'Running') { $w = (Get-Process -Id $hostPid).WorkingSet64
  if ($w -gt $max) { $max = $w }; Start-Sleep -Milliseconds 1000 }
"delta_MB=$([math]::Round(($max-$base)/1MB))"

# 6) стоп песочницы
Stop-Process -Id $hostPid -Force
```


## 8. Промт для нового чата

```text
Продолжаем перевод диаризации в hermes-disk-search с sortformer на Nemotron-3 (Sortformer v3,
8 спикеров) через parakeet-rs + ONNX Runtime. Этапы W0–W4 выполнены и закоммичены, дальше — W5–W9.

Сначала прочитай целиком:
  C:\Users\Sasha\hermes-disk-search\DIARIZATION_NEMOTRON_W4_HANDOFF.md  (состояние, решения, что
      сделано с числами, где менять код W5–W9, грабли окружения, критерии приёмки, промт)
  C:\Users\Sasha\hermes-disk-search\PLAN_DIARIZATION_NEMOTRON.md        (план W0–W9, §3 контракты,
      W0–W4 отмечены ✅ с числами)
  C:\Users\Sasha\hermes-disk-search\DIARIZATION_NEMOTRON_HANDOFF.md     (исходный спайк и решения)
  C:\Users\Sasha\hermes-disk-search\tools\parity\out\diar_spike\REPORT.md (§11 — галлюцинации whisper)

Репозиторий: C:\Users\Sasha\hermes-disk-search, ветка server-multiuser, последний код-коммит
a5cbc9e (W4), дерево чистое, локально впереди origin на 8 коммитов.
Ядро — Rust (crates/), Python только sidecar.
Это dev-машина (cline-rules/dev-machine.md): процессы можно останавливать/запускать, GPU/VRAM
освобождать, релиз пересобирать. Правила проекта: комментарии и тексты по-русски; никаких новых
внешних зависимостей сверх списка из решений (только parakeet-rs и его транзитивные); clippy
-D warnings; каждый шаг — тесты + живой прогон; артефакты — в tools/parity/out/. Ничего из
«Решений владельца» (§2 handoff) не пересматривай.

Сделано: W0 (декодер ffmpeg + обрезка ведущей тишины), W1 (whisper_language и постфильтр «реплик из
тишины»), W2 (ort 2.0.0-rc.15, одно ORT-окружение), W3 (крейт hds-diarize: Diarizer, чанкинг
feed()/flush(), merge/render_md/render_srt), W4 (ветка nemotron3 в llm_host: decode один раз ->
trim только для ASR -> subtitle-ASR -> Diarizer::run_wav по полному WAV -> постфильтр -> merge ->
.md; diarization.backend и приоритет «тело -> конфиг -> авто»). Числа W4 (18 мин): 5 спикеров,
197 реплик, 0 «Thank you»/«Obrigado»/«Lukoyle», 0 реплик в тишине по точным таймкодам, декод
0.6 с / ASR 44–53 с / диаризация DirectML 10–14 с, RSS +780 МБ; sortformer-регресс тем же файлом
(4 спикера + UNASSIGNED).

Порядок работы:
1) W5 — конфиг и диагностика: переключить дефолт auto_transcribe.diarization_backend на nemotron3
   (иначе тело запроса всегда просит sortformer — §4 handoff, п. 1); hds check (пункт diarization:
   backend, путь модели + sha256, EP, ffmpeg) и hds whisper-check; образцы конфигов
   (config.example.yaml, server_config.example.yaml, hds config init/шаблон) и валидация ключей.
2) W6 — installers/fetch_nemotron3_model.ps1 (ASCII-only; size 400506656, sha256 915e4fa2…be03a,
   каталог <ModelsDir>/nemotron3/, .part -> проверка -> Move-Item), setup.ps1 (-SkipDiarization уже
   есть — добавить новый fetch), install_server.*, DirectML.dll в пакет (build_rust_release.ps1),
   NOTICE (OpenMDW 1.1, parakeet-rs, ORT/DirectML), *.onnx вне git и архива.
3) W7 — приёмка по §6 handoff: 18/56/77 мин, RSS ≤ 1.5 ГБ (мерить ДЕЛЬТОЙ: хост держит чат-модель),
   8 спикеров на 77 мин, attr ≥ 71.8 % без филлеров, CPU RTFx ≥ 15 / DirectML ≥ 3× CPU, регресс
   sortformer, паритеты индексации; отчёт tools/parity/out/diar_nemotron/report.md.
4) W8 — macOS best-effort (CoreML, fallback CPU; статус «не проверено», если машины нет).
5) W9 — доки (README, README_SERVER_MULTIUSER, STATUS, NOTICE), RELEASE_NOTES_v0.21.0.md и релиз
   v0.21.0 по RELEASING.md; тег — после зелёной приёмки.

Каждый трек: юниты + живой прогон, обновить PLAN_DIARIZATION_NEMOTRON.md (чек-боксы и фактические
числа), коммит с русским сообщением (стиль `feat(diarize): W5 — …`). Перед cargo test вычищай
HDS_ROOT: `Remove-Item Env:\HDS_ROOT`; релиз собирай в target\w2 (службы держат target\release);
песочница llm_host — `--port-base 8110` (это БАЗА портов, не смещение), после работ останови процесс.

Начни с чтения §3–§5 handoff (что сделано, где менять код, грабли), затем задай уточняющие
вопросы и приступай к W5.
```

