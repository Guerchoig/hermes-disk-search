# DIARIZATION_NEMOTRON_HANDOFF.md — промт для нового чата (диаризация: sortformer → Nemotron-3)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий/ветка/состояние, решения
> владельца, факты спайка с числами, целевая архитектура, треки работ, критерии приёмки и грабли.

---

## Что произошло (контекст из предыдущего чата)

Рассматривались варианты замены ASR/диаризации: (1) gigastt вместо whisper, (2) whisper+speakrs,
(3) **whisper (движок) + Nemotron-3-Diarization (Sortformer v3, до 8 спикеров)**.
Проведён спайк на реальных записях совещаний, отчёт: `tools/parity/out/diar_spike/REPORT.md`
(артефакты и скрипты — там же, каталог гитигнорится). **Владелец выбрал вариант 3** и попросил
перенести реализацию в новый чат.

Итог спайка одной строкой: диаризация Nemotron-3 **не хуже** текущего sortformer по качеству и
**заметно лучше по числу спикеров** (8 слотов вместо 4), работает на Windows (CPU и DirectML),
не имеет потолка длительности, но **память растёт с длиной файла ⇒ нужен чанкинг**. Дополнительно
найден критичный баг ASR: `whisper_language` в движок не передаётся (английские галлюцинации в начале).

## Репозиторий и состояние

* Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**,
  HEAD `0037f43` (2026-10-20), последний тег **`v0.20.6`**.
* Ядро — Rust (`crates/`: hds-core, hds-cli, hds-index, hds-llama, hds-mcp, hds-ui, hds-search,
  hds-extract, hds-clip). Python — только sidecar.
* Станция: Windows 11, RTX 3060 12 ГБ + AMD Radeon iGPU (UMA), rustc/cargo **1.97.1**,
  `ffmpeg 9.0.1` в PATH. Не админ. Это **dev-машина** (`cline-rules/dev-machine.md`): процессы
  можно останавливать/запускать, GPU/VRAM освобождать, `config.yaml`/`data/`/`index.db`
  перезаписывать, релиз пересобирать.
* Боевой стенд: резидент `llm_host` (`:8010/:8011/:8012`), `hds watch`, `hds mcp --http`, `hds ui`.
* Модели движка: `%APPDATA%\OpenResearchTools\models\{openresearchtools__whisper-large-v3-turbo-GGML,
  openresearchtools__diar_streaming_sortformer_4spk-v2.1-gguf}`; движок — `C:\ProgramData\hermes-disk-search\engine`.
* Артефакты спайка вне репозитория: `D:\diar-spike-models\nemotron3_diar_v3.onnx` (400.5 МБ),
  `D:\diar-spike-data\*.wav` (декодированные записи).

## Решения владельца (утверждены 20.10.2026 — менять нельзя)

1. **Диаризация — Nemotron-3-Diarization** через крейт `parakeet-rs` (ONNX Runtime), **не** sortformer.
   ASR остаётся прежним: whisper-large-v3-turbo через bridge-движка; gigastt **не** берём.
2. **Один проход декодирования (вариант A):** медиа декодируется один раз (ffmpeg) в WAV
   16 кГц mono; этот же WAV идёт и в ASR (в движок как WAV), и в диаризацию. Проверено: AAC-mp4 и
   mp3 одной встречи дают одинаковый таймлайн (lag 0 мс).
3. **Контракт `/internal/transcribe` и формат `.md` сохраняются:** `### SPEAKER_NN [HH:MM:SS - HH:MM:SS]`,
   литерал `UNASSIGNED`, sidecar `<stem>.speakers.json`, `<stem>.orig.md`. UI «Транскрибация»
   (`crates/hds-ui/src/transcribe.rs`) не переписывается.
4. **Backend-переключатель:** `diarization.backend: sortformer | nemotron3 | off`. sortformer остаётся
   рабочим fallback (движок + GGUF) до отдельного решения об удалении.
5. **`whisper_language` прокидывается в metadata** движка; значение берётся из конфига
   (`index.whisper_language`). Значение по умолчанию (`ru` или авто) — открытый вопрос №1.
   Плюс подавление галлюцинаций на тишине.
6. **Модель Nemotron-3 (ONNX, 400 МБ) ставится установщиком** в машинный каталог моделей (для служб —
   доступный LocalSystem), с проверкой sha256; в репозиторий модель не кладём.
7. Новые зависимости в продукте **разрешены только эти**: `parakeet-rs` (+ его `ort`/`ndarray`),
   декодер аудио. Способ декодирования фиксируется в W0 (рекомендация — **ffmpeg-сайдкар**).

## Факты спайка (числа воспроизводимы, см. REPORT.md)

**Материал:** `20260924_Запись ERP.SPS.Еженедельный статус.mp4` (1076 с, эталонная транскрипция —
98 реплик, 4 активных спикера + 3 одноразовых), `2026_0512_…mp4` (4619 с), `2026_0514_…mp4` (3363 с).

**Качество на 18-мин записи (эталон приблизительный — см. «Грабли»):**

| Метрика | sortformer 4spk-v2.1 | Nemotron-3 |
|---|---|---|
| спикеров | 4 (потолок) | **5** |
| атрибуция реплик: все / без филлеров | 56.1% / 71.8% | **58.2% / 74.4%** |
| DER~ без FA | **14.9%** | 19.7% |
| purity / F1 смен спикера | 0.937 / 0.403 | **0.944 / 0.431** |
| 77-мин запись: спикеров | 4 (потолок) | **8** (из них 5 ≥5% времени) |

**Скорость/память (эта машина):**

| Прогон | Время | RTFx | Пик RSS |
|---|---|---|---|
| sortformer + whisper (движок, CUDA), 18 мин | 63.5 с (whisper 52.7 + диар. 10.7) | диар. ≈100 | — |
| sortformer + whisper (движок), 77 мин | 224.9 с (whisper 180.3 + диар. 43.0) | диар. ≈107 | — |
| Nemotron-3, 18 мин, **CPU** | 42.6 с | **25.3** | 684 МБ |
| Nemotron-3, 18 мин, **DirectML** | **9.4 с** (результат побитово = CPU) | **114.6** | 494 МБ |
| Nemotron-3, 56 мин / 77 мин, CPU | 140.9 / 186.9 с | 23.9 / 24.7 | 1301 / 2073 МБ |
| Nemotron-3, клип 60 с | CPU 1.6 с / DirectML 1.9 с | 37.6 / 30.9 | ~510 МБ |

⇒ память ~25 МБ на минуту; на клипах <1–2 мин DirectML проигрывает CPU (оверхед сессии).

**Баг ASR (подтверждён экспериментом `asr_lang_probe`):**

| metadata | 60-с фрагмент (интро) |
|---|---|
| без ключа языка (как в проде) | «Obrigado.» |
| `"language": "ru"` / `"whisper_lang": "ru"` | «Obrigado.» (не помогают) |
| **`"whisper_language": "ru"`** | корректный русский |

На полном 18-мин файле: без ключа — **43× «Thank you.»** + 2× `It's the "Lukoyle".` (мусор ≈ первые
2.5 мин), остальное русский с ошибками; с `"ru"` — **0 английских вставок**, 39.3 с на файл вместо
52.7 с, но остаются 2× «Продолжение следует…» на тишине ⇒ нужен VAD/`no_speech`, не только язык.

## Целевая архитектура (вариант A)

```
медиа (mp4/mkv/mp3/…)
   └─ ffmpeg: -vn -ac 1 -ar 16000 -c:a pcm_s16le  →  <tmp>/input.wav      (ОДИН проход)
        ├─ llm-host → bridge движка (whisper-large-v3-turbo, mode: subtitle|speech,
        │              whisper_language из конфига)  → сегменты с таймкодами
        └─ hds-diarize → parakeet-rs (Nemotron-3 ONNX; EP: CPU/DirectML/CoreML)
                         блоки по 10 мин через feed()/flush()  → turns [start,end,speaker]
   └─ сшивка (середина сегмента × turn; сегмент на границе делится)
        └─ `.md` `### SPEAKER_NN [HH:MM:SS - HH:MM:SS]` (+ `UNASSIGNED`) → out_dir
           sidecar `<stem>.speakers.json`, `<stem>.orig.md`  (как сейчас)
```

Инварианты:
* внешний контракт `POST /internal/transcribe` (`path, mode, custom, gpu, diarization*,
  return_text`) и ответ (`segments[], out_ext, text, stats`) **не меняются**;
* `hds-index`/`hds-ui`/демон `transcribe-watch` не знают, кто именно диаризует;
* Nemotron-3 не занимает VRAM движка (CPU/DirectML), поэтому `prepare_demand`/арбитр для
  диаризации не нужен — он остаётся только для ASR-роли `whisper`.

## Конфиг (черновик)

```yaml
diarization:                 # новая секция; auto_transcribe.diarization* остаются для sortformer
  backend: nemotron3         # nemotron3 | sortformer | off
  required: true             # нет модели/сбой ⇒ задание падает (как сейчас)
  nemotron3:
    model: ''                # '' → <models>/nemotron3/nemotron3_diar_v3.onnx
    device: auto             # auto | cpu | directml | coreml
    chunk_min: 10            # блок feed()/flush() (память ~25 МБ/мин при diarize() целиком)
    min_audio_min: 2         # короче — CPU, даже если device: auto (оверхед DirectML)
    onset: 0.5               # DiarizationConfig::custom(onset, offset)
    offset: 0.5
index:
  whisper_language: ru       # ключ есть, но Rust-путь его ИГНОРИРУЕТ — починить (W1)
```

Модель: `altunenes/parakeet-rs` → `nemotron-3-diarization/nemotron3_diar_v3.onnx`,
size `400506656`, **sha256 `915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a`**
(пин по ревизии). Официальные файлы NVIDIA (`nvidia/Nemotron-3-Diarization`): `.nemo` 198.7 МБ,
`model.safetensors` 397 МБ, `Nemotron-3-Diarization.q8_0.gguf` **107 МБ** (для NeMo-Speech.cpp).

## Треки работ

### W0. Декодер (0.5 дня)
**ffmpeg-сайдкар**: `ffmpeg -nostdin -y -v error -i <src> -vn -ac 1 -ar 16000 -c:a pcm_s16le out.wav`.
`ffprobe` уже используется в `hds-index`; не тащим `symphonia/ryf/opus-rs`; одинаковый декод для
ASR и диаризации. Обязательно: положить ffmpeg в серверный пакет (`install_server.ps1`), иначе
службы на чистой машине не декодируют.
Приёмка: 77-мин mp4 → WAV < 30 с; mp4(AAC) vs mp3 одной встречи — lag 0 мс (проверено в спайке).

### W1. ASR: `whisper_language` + галлюцинации на тишине (1 день)
* `crates/hds-llama/src/whisper.rs::build_metadata` — добавить `"whisper_language": "<lang>"`
  (ключ подтверждён экспериментом; `language`/`whisper_lang` **не работают**).
* `crates/hds-llama/src/host.rs::internal_transcribe` — брать язык из тела/конфига и прокидывать.
* `crates/hds-index/src/transcribe.rs::TranscribeConfig::from_config` — читать `index.whisper_language`.
* Тишина: обрезать ведущую/хвостовую тишину в декодере (`silenceremove` или порог) — проверить,
  что «Продолжение следует…»/«Субтитры создавал…» исчезают.
* Тесты: unit на metadata (ключ есть/нет/пусто), integration на stub-сервере.
* Живая приёмка: 18-мин файл → 0 «Thank you»/«Obrigado»/«Lukoyle», русский текст с начала.

### W2. ONNX Runtime: одна версия на воркспейс (0.5 дня)
* `parakeet-rs 0.4.2` требует **Rust ≥ 1.92** (`ort 2.0.0-rc.15`, edition 2024) ⇒ поднять
  `rust-version` в `[workspace.package]` до `1.92` (CI на `stable` — ок).
* `hds-clip` зависит от `ort = "2.0.0-rc.10"` ⇒ cargo унифицирует до **rc.15**; проверить сборку
  `hds-clip` и починить API-дельты (`Session::builder`, `Tensor`, `inputs!`).
* **Один ORT-environment на процесс**: `parakeet_rs::ort::init().build()` создаётся один раз;
  если бинарь линкует и `hds-clip`, и `hds-diarize` — env общий (вынести инициализацию и заставить
  `hds-clip` принимать `&Environment`). Альтернатива без правки `hds-clip`: диаризация живёт
  **только** в `llm_host`, а `hds whisper-check` спрашивает хост по HTTP (он и так так делает).

### W3. Крейт `hds-diarize` (2 дня)
Новый член воркспейса `crates/hds-diarize`:
```toml
[dependencies]
parakeet-rs = { version = "0.4", default-features = false, features = ["sortformer", "cpu"] }
[target.'cfg(windows)'.dependencies]
parakeet-rs = { version = "0.4", default-features = false, features = ["sortformer", "cpu", "directml"] }
[target.'cfg(target_os = "macos")'.dependencies]
parakeet-rs = { version = "0.4", default-features = false, features = ["sortformer", "cpu", "coreml"] }
```
(точный набор фич проверить: без `ort-defaults` ort не скачивает бинарь — см. фичи `ort`.)

Публичный API (стиль проекта — русские докстринги):
```rust
pub struct Turn { pub start: f64, pub end: f64, pub speaker: u32 }
pub struct Diarizer { /* Sortformer + конфиг */ }
impl Diarizer {
    pub fn load(cfg: &NemotronConfig) -> Result<Self>;      // проверка sha256, выбор EP
    pub fn run_wav(&mut self, wav: &Path) -> Result<Vec<Turn>>;
}
```
* `run_wav`: если длительность ≤ `min_audio_min` → `diarize()` целиком; иначе **блоки
  `chunk_min`** через `feed(&[f32])` + `flush()`. **Проверить и покрыть тестом**, что `feed`
  отдаёт абсолютные таймкоды (в исходнике есть счётчик `elapsed_samples`) и что `speaker_id`
  стабилен между блоками (AOSC-кэш) — это критично для склейки.
* EP: `auto` → `DirectML` на Windows при наличии, иначе `Cpu`; `CoreMl` на macOS; явный конфиг
  сильнее авто.
* Модель: `<models>/nemotron3/nemotron3_diar_v3.onnx`; при `HDS_MODELS_DIR` — машинный каталог.
* Эталонный тест на синтетике: WAV 60 с (frag60 из спайка) → 2 спикера, 5 сегментов.

### W4. Интеграция в `llm-host` + форматтеры (2 дня)
* `crates/hds-llama/src/host.rs::internal_transcribe`:
  * `diarization.backend == nemotron3` → (1) декод в WAV (W0) **один раз**, (2) ASR на этих же
    байтах (движок получает WAV; `ffmpeg_convert` всё равно `true`, WAV декодируется без сдвига),
    (3) `Diarizer::run_wav`, (4) сшивка, (5) `.md`.
  * `backend == sortformer` — прежний путь без изменений (регресс-контроль).
* **Сшивка** (`merge_segments_with_turns`): для каждого ASR-сегмента берём середину; если середина
  вне turns → `UNASSIGNED`; если сегмент пересекает границу turn — режем на части по границе.
  Реализовать как чистую функцию + unit-тесты (граница ровно в середине, пустые turns, overlap).
* **Форматтеры** (`crates/hds-llama/src/format.rs` или `hds-diarize`): `render_md(turns, segments)`
  и `render_srt(...)`; `SPEAKER_NN` = `SPEAKER_{id:02}`; таймкод `HH:MM:SS` (как сейчас);
  порядок реплик по времени; `UNASSIGNED` — литерал.
* `stats` ответа: добавить `num_speakers`, `timings_sec {decode, asr, diar, total}` (диагностика).

### W5. Конфиг и диагностика (0.5 дня)
* `hds-core::config::dig` — чтение новых ключей (секция `diarization`, `diarization.nemotron3.*`),
  валидация: `backend` из списка, файл модели существует, устройство допустимо для платформы.
* `hds check` — пункт `diarization`: backend, путь модели, EP, наличие ffmpeg.
* `hds whisper-check` — печатать backend/EP/число спикеров последнего прогона.
* `config.example.yaml` — секция с комментариями; в `install_server`-конфиг — `backend: nemotron3`.

### W6. Установка и лицензии (1 день)
* `installers/fetch_nemotron3_model.ps1` (**ASCII-only**, PS 5.1): URL + `size` + `Get-FileHash`
  (sha256 выше), `-ModelsDir`/`HDS_MODELS_DIR`, идемпотентность, `-Force`.
* `setup.ps1`: шаг «Модель диаризации (Nemotron-3)»; `-SkipDiarization` — не качать.
* `installers/install_server.ps1` + `install_server.command`: скачать модель в машинный каталог
  (службы под LocalSystem должны её видеть) + проверить наличие `ffmpeg`.
* `NOTICE.md`: веса — **OpenMDW 1.1** (коммерческое использование разрешено, текст проверить),
  `parakeet-rs` — MIT OR Apache-2.0, ONNX Runtime — MIT; sortformer (если остаётся) — NVIDIA OML.

### W7. Тесты и приёмка (1 день) — см. следующий раздел.

### W8. macOS — best-effort (0.5 дня)
`coreml` EP, prebuilt-модель та же; CI-джоб macOS уже `continue-on-error`; проверить, что
`parakeet-rs` собирается под aarch64-apple-darwin и что модель грузится с CoreML (fallback на CPU
логируется). Приёмка — ручная, зафиксировать в отчёте.

### W9. Документация и релиз (0.5 дня)
`README.md` («Автотранскрибация», требования, модели), `STATUS.md`, `RELEASE_NOTES_v*.md`,
`NOTICE.md`; версия — по `RELEASING.md`.

## Критерии приёмки

**Обязательные прогоны:**
```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings     # 0/0
cargo test --workspace
```
**Живые прогоны (dev-стенд):**
1. `hds transcribe-watch` на тестовой inbox с 18-мин файлом: `.md` содержит `### SPEAKER_NN`,
   **5 спикеров**, `UNASSIGNED` только вне речей; `<stem>.orig.md` и `<stem>.speakers.json` на месте;
   UI «Транскрибация» показывает файл, переименование `SPEAKER_00 → Иван` работает (`.bak`, sidecar).
2. 77-мин файл: не падает, **RSS ≤ 1.5 ГБ** (с чанкингом), 8 спикеров, время ≤ 5 мин на CPU.
3. ASR: на 18-мин файле **0** «Thank you»/«Obrigado»/«Lukoyle»; язык — из конфига.
4. Регресс: `diarization.backend: sortformer` даёт прежний результат на тех же файлах
   (сравнение по `num_speaker_spans`/спикерам); паритеты индексации (`pipeline_parity`,
   `walk_parity`) — без изменений.
5. Качество: на 18-мин файле с эталоном (исключая короткие филлеры) атрибуция **не ниже 71.8%**
   (текущий sortformer) и спикеров не меньше 5; методика — скрипты спайка:
   `python flatten.py`, `python eval_diar.py <ref> <model>`.

**Порог производительности:** CPU RTFx ≥ 15 на файлах ≥10 мин; при `device: auto` на Windows
DirectML даёт ≥ 3× к CPU на файлах ≥10 мин (в спайке — 4.5×).

## Грабли и риски (из спайка, не переоткрывать)

1. **Эталон шумный.** В `20260924_Транскрипция …txt` только начала реплик, 20 из 98 — короче 8 символов
   («угу.», «ну.»), 9 «начинаются в тишине» (речь в пределах 0–1.04 с). Поэтому атрибуция у обеих
   моделей 56–58%, а без филлеров — 72–74%. **Метрики только относительные**; филлеры исключать,
   абсолютные DER по этому эталону не публиковать.
2. **`speaker_spans` движка перекрываются** (сумма 270% длины файла) — для любых сравнений
   обязателен `flatten.py` (одна метка на 10 мс), иначе метрики недостоверны.
3. **Память Nemotron растёт с длиной**: `diarize()` целиком — 684 МБ/18 мин, 1301 МБ/56 мин,
   2073 МБ/77 мин (~25 МБ/мин). Чанкинг через `feed()/flush()` обязателен; проверить, что
   спикеры не «перескакивают» на стыках.
4. **DirectML на коротких клипах медленнее CPU** (60-с клип: 1.9 с против 1.6 с) — включать по
   длительности (`min_audio_min`), а не безусловно.
5. **ORT**: `ort 2.0.0-rc.15` требует **Rust 1.92**, edition 2024; **один environment на процесс**;
   `hds-clip` соберётся против rc.15 — проверить API-дельты.
6. **`parakeet-rs` — молодой крейт** (0.4.2, API менялся между версиями): пинить версию, держать
   один интеграционный тест на реальном файле, при обновлении перечитывать `src/sortformer.rs`.
7. **Модель — сторонний ONNX-экспорт** (`altunenes/parakeet-rs`), официальный источник NVIDIA —
   `.nemo` / `.safetensors` / `.q8_0.gguf`. Пин по ревизии + sha256; при желании — свой экспорт из
   `.nemo` (`scripts/export_diar_sortformer.py`, нужен Python + NeMo).
8. **Ключ языка только один**: `whisper_language`; `language` и `whisper_lang` движок игнорирует.
9. **Галлюцинации на тишине остаются** даже с языком (русские «Продолжение следует…») — нужен VAD.
10. **Windows/сборка**: `target\release\llm_host.exe` держит Планировщик — перед `cargo build`
    остановить (`dev-machine.md`), после — вернуть.
11. **Не ломать существующие конфиги**: секция `diarization` опциональна; при отсутствии — если модель
    Nemotron есть, брать `nemotron3`, иначе `sortformer` с предупреждением в лог.
12. **Арбитр VRAM** для Nemotron не нужен (CPU/DirectML), но ASR-роль `whisper` работает на движке —
    `prepare_demand` для ASR-части не убирать.

## Артефакты спайка (переиспользовать)

```
tools/parity/out/diar_spike/REPORT.md        отчёт с таблицами (§11 — галлюцинации whisper)
tools/parity/out/diar_spike/out/*.json       результаты (sortformer/nemotron3, cpu/directml, flat)
tools/parity/out/diar_spike/parse_ref.py     эталонная транскрипция -> ref_*.json
tools/parity/out/diar_spike/eval_diar.py     attr/attr_main/der~/purity/F1 — методика приёмки
tools/parity/out/diar_spike/flatten.py       перекрытия -> одна метка на 10 мс
tools/parity/out/diar_spike/diag_ref.py      сдвиг эталона, точность по спикерам
tools/parity/out/diar_spike/speaker_stats.py доли спикеров, «реальные» (>=5%)
tools/parity/out/diar_spike/compare_pcm.py   lag между двумя WAV
tools/parity/out/diar_spike/crate/           diar_spike (Nemotron-3), asr_lang_probe (язык whisper)
D:\diar-spike-models\nemotron3_diar_v3.onnx  модель (400.5 МБ)
D:\diar-spike-data\*.wav                     декодированные записи (вне репозитория)
```

Проверка после реализации:
```powershell
ffmpeg -i "<mp4>" -vn -ac 1 -ar 16000 -c:a pcm_s16le D:\diar-spike-data\meet.wav
.\target\release\diar_probe.exe meet.wav <whisper.bin> <sortformer.gguf> 0 > probe.txt
diar_spike.exe D:\diar-spike-models\nemotron3_diar_v3.onnx meet.wav nem.json
python flatten.py nem.json nem_flat.json; python eval_diar.py ref.json nem_flat.json
```

## Открытые вопросы к владельцу (решить в начале нового чата)

1. **Дефолт `index.whisper_language`**: `ru` (продукт русскоязычный) или пусто = авто?
   (авто на интро даёт «Thank you»/«Obrigado»).
2. **Размер блока `chunk_min`**: 10 или 15 минут (память против риска стыков)?
3. **Поведение при апдейте**: включать `nemotron3` по умолчанию, если модель уже скачана?
4. **Экспорт ONNX**: остаёмся на стороннем файле (пин + sha256) или делаем свой из `.nemo`?
5. **Судьба sortformer**: оставляем fallback навсегда или удаляем после приёмки (вместе с
   `installers/fetch_diarization_model.ps1` и `auto_transcribe.diarization*`)?

---

## Промт для нового чата (скопировать целиком)

```text
Мы переводим диаризацию в проекте hermes-disk-search с sortformer (движок) на
Nemotron-3-Diarization (Sortformer v3, 8 спикеров) через крейт parakeet-rs + ONNX Runtime.
ASR остаётся прежним (whisper-large-v3-turbo через bridge движка). Спайк уже проведён,
решения приняты, числа измерены — всё в файле:

  C:\Users\Sasha\hermes-disk-search\DIARIZATION_NEMOTRON_HANDOFF.md

Прочитай его целиком перед началом (плюс tools/parity/out/diar_spike/REPORT.md — отчёт спайка
с таблицами и §11 про галлюцинации whisper). Ничего из «Решений владельца» не пересматривай.

Репозиторий: C:\Users\Sasha\hermes-disk-search, ветка server-multiuser, HEAD 0037f43 (v0.20.6).
Ядро — Rust (crates/), Python только sidecar. Это dev-машина (cline-rules/dev-machine.md):
процессы можно останавливать/запускать, GPU/VRAM освобождать, релиз пересобирать.
Правила проекта: комментарии и тексты по-русски; никаких новых зависимостей сверх списка из
решений; clippy -D warnings; каждый шаг — тесты + живой прогон; артефакты — в tools/parity/out/.

Порядок работы:
1) Создать PLAN_DIARIZATION_NEMOTRON.md (образец — PLAN_M9_CLIENT_PACKAGE.md) по трекам W0–W9
   из handoff-файла. Сначала задай мне 5 вопросов из §«Открытые вопросы» и дождись ответов.
2) W1: прокинуть whisper_language в bridge-движок (ключ только "whisper_language"!) + обрезка
   тишины; проверка на 18-мин файле: ноль «Thank you»/«Obrigado»/«Lukoyle».
3) W2: ort 2.0.0-rc.15 (MSRV 1.92, edition 2024), унификация с hds-clip, один ORT-environment
   на процесс.
4) W3–W5: крейт hds-diarize (Nemotron-3, EP по платформе, чанкинг feed()/flush()), интеграция
   в llm-host (internal_transcribe), сшивка сегментов с turns, формат .md/SRT как сейчас
   (### SPEAKER_NN [HH:MM:SS - HH:MM:SS], UNASSIGNED), конфиг diarization.backend.
5) W6–W9: установщик модели (sha256 в handoff), NOTICE (OpenMDW 1.1), тесты/приёмка по
   критериям из handoff, macOS best-effort, доки/релиз.

Приёмка — по §«Критерии приёмки»: cargo fmt/clippy/test зелёные; живые прогоны на 18/56/77-мин
записях; .md с 5 спикерами на 18-мин файле; RSS <= 1.5 ГБ на 77 мин; атрибуция не ниже 71.8%
на эталоне без филлеров; регресс sortformer и паритеты индексации не сломаны.

Начни с чтения handoff-файла, затем задай вопросы из §«Открытые вопросы».
```
