# DIARIZATION_NEMOTRON_CODE_REVIEW.md — код-ревью доработки Nemotron-3

> **Дата:** 21.10.2026 · **Ветка:** `server-multiuser` · **Ревьюер:** Cline (агент)
> **Предмет:** изменения по `PLAN_DIARIZATION_NEMOTRON.md` (W0–W9), коммиты
> `968c2ab..abda621` — 15 коммитов, 58 файлов, +7581/−226.
> **Рабочее дерево на момент ревью:** чистое (`git status` пуст), ничего не изменялось;
> для проверки гипотезы о панике временно создавался тест, после прогона удалён.
> **Связанные документы:** `PLAN_DIARIZATION_NEMOTRON.md`, `DIARIZATION_NEMOTRON_HANDOFF.md`,
> `DIARIZATION_NEMOTRON_W3_HANDOFF.md`, `DIARIZATION_NEMOTRON_W4_HANDOFF.md`,
> `RELEASE_NOTES_v0.21.0.md`, `PLAN_AUTO_TRANSCRIBE.md`.
> **План исправлений:** `PLAN_DIARIZATION_NEMOTRON_FIXES.md` (задачи F1–F16).
> **Промт для нового чата:** `DIARIZATION_NEMOTRON_FIXES_HANDOFF_PROMPT.md`.

## 1. Резюме

Доработка в целом соответствует плану: один проход декодирования, обрезка ведущей тишины
только для ASR, `trim_offset` применяется ровно один раз, чанкинг `feed()/flush()`, пин
модели по ревизии HF + размеру + sha256, аккуратные деградации (`required: false`),
обновлённая диагностика и установщики. Тесты содержательные, `fmt`/`clippy`/`test`
по затронутым крейтам зелёные.

При этом найдено **2 дефекта уровня «не выпускать»** (один — паника на данных, один —
рассинхрон выбора backend/диагностики/документации), **1 дефект пути обновления**
(обновлённая установка остаётся без модели и теряет автотранскрибацию), **6 средних** и
**7 низких** замечаний (техдолг, лишняя работа, мёртвые ключи/код, ограничения WAV).

| Уровень | Кол-во | Кратко |
|---|---|---|
| Блокирующий | 3 | паника `split_text`; дефолт клиента обходит выбор backend и ломает fallback/диагностику; `update` не качает модель |
| Средний | 6 | ложное предупреждение демона; лишнее хеширование 400 МБ; кэш только успешных проверок; два ORT-окружения в одном бинаре; предел RIFF/u32 (≈6.7 ч); мёртвый `diarization_required` |
| Низкий | 7 | `mode` в ответе не соответствует факту; мёртвый/дублированный код; нет валидации `onset/offset`; dev-пути в live-тесте; устаревшие комментарии; гонка `hds_whisper/input.*`; транзиентная память WAV |

**Вывод:** после исправления F1–F3 (см. план) доработку можно считать готовой к тегу
`v0.21.0`; F4–F9 желательно включить в тот же релиз, F10–F16 — отдельной задачей
(техдолг), не блокируют.

## 2. Методика и выполненные проверки

Читался код (не только diff): `crates/hds-diarize/**`, ветка `nemotron3` в
`crates/hds-llama/src/{host,diar,asr_filter,whisper}.rs`, `crates/hds-index/src/{transcribe,
autotranscribe,diag}.rs`, `crates/hds-cli/src/cmd/{check,whisper_check,transcribe}.rs`,
`crates/hds-clip/src/{env,lib}.rs`, конфиги, установщики, релизные документы; сверка с
контрактами `PLAN_AUTO_TRANSCRIBE.md` §5 и `PLAN_DIARIZATION_NEMOTRON.md` §3/§7.

| Проверка | Команда | Результат |
|---|---|---|
| Форматирование | `cargo fmt --all -- --check` | ✅ exit 0 |
| Линтер (затронутые крейты) | `cargo clippy -p hds-diarize -p hds-llama --all-targets -- -D warnings` | ✅ exit 0 |
| Юниты диаризации/хоста | `cargo test -p hds-diarize -p hds-llama --lib` | ✅ 30 + 77 passed, 2 ignored |
| Тесты индекса/диагностики/клиента | `cargo test -p hds-index` | ✅ 12 наборов, 0 падений |
| Воспроизведение паники | временный `tests/_review_panic.rs`, затем удалён | ❌ паника подтверждена (`min > max. min = 1, max = 0`, `merge.rs:101`) |

Ограничения ревью (честно): полный `cargo test --workspace` (99 наборов, как в W7) не
перезапускался — прогонялись затронутые крейты; живые прогоны 18/56/77 мин на GPU не
повторялись (использованы числа и артефакты W7); macOS/CoreML не проверялся (как и в плане).
## 3. Блокирующие дефекты

### B1. Паника в `split_text` на реальных данных

**Где:** `crates/hds-diarize/src/merge.rs:99-104` (функция приватная, вызывается из
`merge_segments_with_turns`), срабатывание — строка 101.

```rust
let want = ((d / total) * n as f64).round() as usize;
let want = want.clamp(1, n.saturating_sub(used));   // clamp(1, 0) → panic
counts.push(want);
used += want;
```

**Причина:** если к не последней части все слова уже израсходованы (`used == n`),
`n.saturating_sub(used) == 0`, а `usize::clamp(1, 0)` в Rust паникует
(`assert!(min <= max)`). Последняя часть защищена (`counts.push(n - used)`), промежуточные — нет.

**Воспроизведение (проверено):** сегмент ASR 0…5 с, текст из **двух слов**; turns
`0–1.5 / 1.5–2.5 / 2.5–3.5 / 3.5–5` (четыре части ≥ `MIN_PART_SEC`, разные спикеры):

```
thread 'few_words_many_speakers_does_not_panic' panicked at crates\hds-diarize\src\merge.rs:101:29:
min > max. min = 1, max = 0
```

Достаточно и трёх групп при одном слове. На реальной записи достижимо: whisper-сегмент
≥ 3 с с 1–2 словами («Секундочку…», «Да-а-а.») и частая смена говорящих (до 8 спикеров).

**Последствие:** паника рабочего потока `internal_transcribe` → клиент получает обрыв
соединения вместо JSON-ошибки; задание уходит в ретраи (`auto_transcribe.max_attempts: 3`)
и попадает в «забракованные». Процесс `llm_host` не падает (panic=unwind), но задача
теряется, а в логе остаётся только стек.

**Исправление:** не использовать `clamp` с динамическим минимумом; добавить регрессионные
тесты (задача F1 плана):

```rust
let room = n - used;                       // инвариант used <= n
let want = if room == 0 { 0 } else { want.max(1).min(room) };
counts.push(want);
used += want;
```

### B2. Дефолт клиента `nemotron3` обходит правило выбора backend, ломает fallback и диагностику

**Где:**
`crates/hds-index/src/transcribe.rs:58` (`DEFAULT_DIARIZATION_BACKEND = "nemotron3"`),
`:248-249` (чтение `auto_transcribe.diarization_backend` с этим дефолтом),
`:481-489` (клиент **всегда** отправляет `diarization_backend` в теле),
`crates/hds-llama/src/host.rs:2499-2519` (тело сильнее конфига),
`host.rs:2527-2551` (конфиг → авто, решение №10/№3),
`crates/hds-index/src/diag.rs:954-1032` (сводка строится **только** по хостовому конфигу).

**Суть:** `auto_transcribe.diarization` по умолчанию `true`, поэтому клиент всегда кладёт
`diarization_backend` в тело. Тело сильнее конфига хоста, значит:

1. **Решение №3 плана** («секции `diarization` нет → nemotron3, если модель есть и прошла
   sha256, иначе sortformer + предупреждение») для автотранскрибации **не применяется**:
   тело диктует `nemotron3` даже без модели.
2. `diarization.backend: sortformer` в конфиге хоста **не переключает конвейер** назад —
   а именно это советуют `README.md:616-617` и `RELEASE_NOTES_v0.21.0.md:80-82`
   («поставьте `diarization.backend: sortformer`»). Работает только
   `auto_transcribe.diarization_backend: sortformer`.
3. Диагностика расходится с фактом: `hds check`, `hds whisper-check` и
   `hds transcribe-watch --dry-run` (`crates/hds-cli/src/cmd/transcribe.rs:85`) показывают
   backend по хостовому конфигу (возможен `[ok] sortformer` или `[ok] off`), а задание
   попросит `nemotron3` и упадёт на отсутствии модели.

**Последствие:** после обновления без модели (см. B3) либо при осознанном возврате на
sortformer конвейер ведёт себя не так, как написано в документации и показывает
диагностика; ошибка воспроизводится только в момент задания.

**Рекомендация:** клиент отправляет `diarization_backend` **только если ключ явно задан**
(отсутствует → поле не отправляется, хост применяет правило №3). Дополнительно:
`transcribe-watch --dry-run` должен показывать фактический запрос конвейера, а не только
хостовый конфиг. Альтернатива (если владелец настаивает на «всегда nemotron3») — явно
задокументировать, что `diarization.backend` для автотранскрибации не действует, и научить
диагностику это подписывать; но это не лечит п.1.

### B3. Путь обновления не скачивает модель Nemotron-3, хотя релизные заметки утверждают обратное

**Где:** `RELEASE_NOTES_v0.21.0.md:74` («при обновлении её скачает `update.cmd`/
`install-server.cmd -Fetch`»); фактически `fetch_nemotron3_model.ps1` вызывается только из
`setup.ps1:355-365` и `installers/install_server.ps1:397-404`; в
`installers/update.ps1` нет ни одного `fetch_*` (`update.cmd` лишь запускает `update.ps1`).

**Последствие:** существующая установка, обновлённая через `update.cmd`/
`installers/update.command`, получает новый дефолт `nemotron3` + `diarization.required:
true`, но без модели. Каждое задание автотранскрибации падает 3 раза и «забраковывается»
(`max_attempts`), пока администратор вручную не запустит установщик модели. `hds check`
это покажет (fail), но после обновления ожидание «всё работает» нарушено.

**Исправление:** добавить идемпотентную докачку в `update.ps1` (профили server/desktop,
с `-NoModels` для отказа), либо исправить релизные заметки/README и добавить явный шаг
обновления. Желательно — и то, и другое (задача F3).
## 4. Средние замечания

### M1. Ложное предупреждение демона при дефолтном `nemotron3`

**Где:** `crates/hds-index/src/autotranscribe.rs:417-424` (`warnings()`), текст — строка 421.

```rust
if self.transcribe.diarization.enabled && self.transcribe.diarization.model.is_none() {
    w.push("diarization_model не задан: владелец ищет sortformer сам; если модели нет — задание упадёт ...");
}
```

`diarization_model` — это legacy-sortformer `.gguf`, и он пуст по умолчанию. Значит при
дефолтном `nemotron3` демон на **каждом** старте печатает предупреждение про sortformer и
«жёсткий отказ», хотя выбран другой путь. Условие должно учитывать backend: для
`nemotron3` проверять `diarization.nemotron3.model`/автопоиск и `ffmpeg`, для `sortformer` —
прежнюю модель.

### M2. `hds check`/`host` хешируют 400 МБ, даже когда модель не нужна

**Где:** `crates/hds-index/src/diag.rs:978-980` (verify_model до выбора backend),
`crates/hds-llama/src/host.rs:3534` (`resolved_diar_backend()` вызывается на старте хоста).

`diarization_summary` вызывает `verify_model` **до** `parsed.unwrap_or(auto)`, поэтому при
`backend: off` или явном `sortformer` `hds check` всё равно читает и хеширует 400 МБ
(≈1–2 с на вызов; кэш — только на процесс). Аналогично хост на каждом старте (даже с
выключенной автотранскрибацией) делает полный проход по модели, чтобы залогировать выбор
backend.

**Рекомендация:** проверять модель только когда backend действительно `nemotron3`
(явно или авто); при `off`/`sortformer` писать `[--] не проверялась`. В хосте — перенести
`resolved_diar_backend()` на первое задание (или на первый запрос статуса), на старте
оставить дешёвую проверку `is_file`/размера.

### M3. Неудачи `verify_model` не кэшируются

**Где:** `crates/hds-diarize/src/sortformer.rs:231-296`; кэш наполняется только при успехе
(строка 249 и ниже).

Если файл битый (обрезанная закачка, подменённый LICENSE), `Diarizer::load` падает,
`run_nemotron` оставляет `guard = None`, и **каждое** следующее задание снова хеширует
400 МБ. При `diarization.required: false` это тихая постоянная просадка времени задания.
Кэшировать и отрицательный результат по `(path, size, mtime)`.

### M4. Два независимых ORT-окружения в одном бинаре

**Где:** `crates/hds-clip/src/env.rs:20` (`static ENV: OnceLock<...>`) и
`crates/hds-diarize/src/ort_env.rs:21` (такой же `static ENV`).

`hds` уже линкует оба крейта: `hds-index → hds-clip` и `hds-index → hds-diarize`
(добавлено в W5). Коллизии нет только потому, что CLI не создаёт `Diarizer`; это записано
в комментариях, но не защищено кодом. Любой будущий вызов диаризации из процесса `hds`
(например, десктопный прогон из UI) даст `only one environment is allowed per process`.

**Рекомендация:** один общий синглтон окружения — новый тонкий крейт `hds-ort`
(зависимость только `ort = "=2.0.0-rc.15"`), который используют `hds-clip` и `hds-diarize`;
`parakeet-rs` пинится на ту же версию, поэтому тип `Environment` совпадает. Плюс
интеграционный тест «оба API дают одно окружение в одном процессе».

### M5. Предел RIFF/u32: записи длиннее ≈6 ч 40 мин

**Где:** `crates/hds-diarize/src/wav.rs:113-114` (`data_len`/RIFF-размеры в `u32`),
`parse_wav` (строки 49-83) понимает только RIFF, не RF64.

Для 16 кГц mono предел `u32` — ≈6.7 ч; `ffmpeg` для >4 ГБ пишет RF64, который `parse_wav`
отвергнет («нет data-чанка»). В `transcribe_nemotron3` нет проверки длительности, поэтому
на очень длинной записи пользователь получит невнятную ошибку после часов обработки.
Добавить явный guard по длительности с понятным текстом (или поддержать RF64/стриминг).

### M6. `auto_transcribe.diarization_required` — мёртвый ключ

**Где:** читается `crates/hds-index/src/transcribe.rs:253`, печатается в
`--dry-run` (`crates/hds-cli/src/cmd/transcribe.rs:95-100`), но **не отправляется** в тело
(`transcribe_to_file`, строки 481-489). Хост использует свой `diarization.required`.

Пользователь ставит `diarization_required: false`, ожидая деградацию вместо отказа, —
эффекта нет. Либо передавать поле (`diarization_required` — аддитивный ключ тела) и
учитывать его на хосте с приоритетом «тело → конфиг», либо убрать ключ из конфига и
документации, чтобы не создавать ложных ожиданий.
## 5. Низкие замечания и техдолг

### L1. `mode` в ответе nemotron-ветки не соответствует факту

**Где:** `crates/hds-llama/src/host.rs:2769` (`"mode": mode`), при этом ASR всегда идёт
`subtitle` (`host.rs:2662`), а `out_ext` всегда `"md"` (`host.rs:2764`).

При `auto_transcribe.mode: speech` legacy-backend возвращает плоский текст, а nemotron3 —
`.md` со спикерами, но в ответе всё равно `mode: "speech"`. Потребитель API не может
отличить фактический формат от запрошенного. Варианты: возвращать фактический режим
(`"transcript"`), либо явно отклонять комбинацию `speech + nemotron3`, либо
задокументировать, что диаризация всегда даёт `.md`.

### L2. Мёртвый и дублированный код

* `crates/hds-llama/src/diar.rs:279` — `pick_backend` используется только в тесте
  (`body_backend_overrides_config`); хост реализует то же правило в
  `ClusterBackend::pick_diar_backend`.
* `crates/hds-diarize/src/merge.rs:164` — `render_srt` не вызывается в продукте (только тест;
  хост всегда отдаёт `.md`).
* `DiarBackend` продублирован: `crates/hds-index/src/diag.rs:886` и
  `crates/hds-llama/src/diar.rs:37` — вместе с логикой «конфиг → авто».
* Поиск `ffmpeg` продублирован трижды: `crates/hds-core/src/tools.rs:118`,
  `crates/hds-llama/src/diar.rs:377`, `crates/hds-diarize/src/bin/diar_wav_probe.rs:25`.

Именно дубль логики выбора backend уже привёл к расхождению B2. Свести в один источник
(`hds-diarize` для backend-правила; отдельный тонкий `hds-tools` или `hds-diarize` для
поиска ffmpeg) — задача F11.

### L3. Нет валидации `onset/offset`

**Где:** `crates/hds-llama/src/diar.rs:209-214` (чтение без проверки диапазона),
`crates/hds-diarize/src/sortformer.rs:135-160` (`NemotronConfig::validate` проверяет
`chunk_min`/`min_audio_min`/путь, но не пороги).

Значение `onset: 5` или `offset: -1` тихо уйдёт в модель. Добавить проверку `0.0..=1.0` с
предупреждением в `from_yaml` и ошибкой в `validate` (как у `chunk_min`).

### L4. Dev-пути в live-тесте

**Где:** `crates/hds-diarize/tests/nemotron_live.rs:21-28` — зашиты
`D:\diar-spike-models\nemotron3_diar_v3.onnx` и `D:\diar-spike-data\frag60.wav`.

На другой машине `cargo test ... -- --ignored` «зелёный», ничего не проверив (печатает
`[skip]` и `return`). Лучше только `HDS_DIAR_MODEL`/`HDS_TEST_DIAR_WAV` и явное сообщение;
если переменные заданы, но файлов нет — падать с понятной ошибкой (как в `decode.rs`
тест пропускается только при отсутствии `ffmpeg` в `PATH`).

### L5. Устаревшие комментарии про каталоги sortformer

**Где:** `config.example.yaml:367`, `hds/config.py:137` — «`''` → автопоиск
`%APPDATA%\OpenResearchTools\models\*sortformer*`». После W5 порядок —
`HDS_MODELS_DIR → %ProgramData%\hermes-disk-search\models → %APPDATA%\OpenResearchTools\models`
(`hds_diarize::models_roots`). Синхронизировать текст.

### L6. Гонка в `hds_whisper/input.<ext>` (не внесена этой доработкой)

**Где:** `crates/hds-llama/src/whisper.rs::stage_ascii` (ASCII-стейджинг в общий
`%TEMP%/hds_whisper`) и `build_metadata` с тем же `out_dir`.

Файл входа/выхода общий на процесс, а клетки whisper — на карту, т.е. параллельные файлы
на разных GPU перезаписывают друг друга. Проблема существовала и раньше, но nemotron-ветка
добавила второго вызывающего и всегда стейджит WAV (срез). Исправить: уникальный подкаталог
на запрос (`TempDir`), уборка после.

### L7. Транзиентная память при записи/чтении WAV

**Где:** `crates/hds-diarize/src/wav.rs:113` (`Vec<u8>` на весь файл),
`crates/hds-diarize/src/wav.rs:136-139` + `parse_wav` (двойное чтение: `fs::read` + `Vec<i16>`).

Для 77 мин это ≈300 МБ пик на `write_wav` и ≈300 МБ на `read_wav` (файл + сэмплы). Замеры
W7 это учитывают и не блокируют, но при желании снизить: потоковая запись `BufWriter` и
разбор `data` сразу в `Vec<i16>` без промежуточной копии.

## 6. Что сделано хорошо (не потерять при исправлениях)

* Инвариант таймлайна соблюдён и покрыт тестом: `offset_is_applied_once`; постфильтр и
  `merge_absolute` сдвигают шкалу ровно один раз.
* Один декод (вариант A), обрезка только входа ASR, `drop(wav)` перед ASR — осмысленная
  работа с памятью; чанкинг `feed()/flush()` побитово стабилен (W3/W7).
* Пин модели на трёх уровнях: константы в коде, установщик (ревизия HF + размер + sha256),
  диагностика; отрицательный размер отсекается до хеширования.
* Деградации аккуратные: `required: false` → `.md` без меток и `diarization.enabled: false`;
  legacy-путь sortformer не тронут и покрыт тестом.
* Диагностика и установщики обновлены вместе с кодом; есть тесты на состав пакета
  (`DirectML.dll`, отсутствие весов, пин ревизии).
* Документация и handoff-файлы подробные, числа приёмки воспроизводимы
  (`tools/parity/out/diar_nemotron/`).
## 7. Ограничения ревью

* Не перезапускался полный `cargo test --workspace` (99 наборов по W7) — прогонялись
  затронутые крейты; полный прогон входит в финальную проверку плана исправлений.
* Живые прогоны 18/56/77 мин на GPU не повторялись; использованы артефакты и числа W7.
* macOS/CoreML не проверялся (нет машины) — как и заявлено в W8/релизных заметках.
* B2/B3 подтверждены чтением кода и существующим тестом `body_backend_overrides_config`;
  живой прогон `transcribe-watch` без модели не выполнялся (это проверка плана F2/F3).

## Приложение A. Воспроизведение B1 (тест, которым подтверждена паника)

```rust
use hds_diarize::{merge_segments_with_turns, AsrSpan, Turn};

#[test]
fn few_words_many_speakers_does_not_panic() {
    // 5-секундный сегмент ASR с ДВУМЯ словами; turns делят его на четыре части.
    let turns = [
        Turn { start: 0.0, end: 1.5, speaker: 0 },
        Turn { start: 1.5, end: 2.5, speaker: 1 },
        Turn { start: 2.5, end: 3.5, speaker: 2 },
        Turn { start: 3.5, end: 5.0, speaker: 3 },
    ];
    let segs = [AsrSpan::new(0.0, 5.0, "да нет")];
    let blocks = merge_segments_with_turns(&segs, &turns);
    let words: usize = blocks.iter().map(|b| b.text.split_whitespace().count()).sum();
    assert_eq!(words, 2, "слова не потеряны");
}
```

До исправления: `panicked at merge.rs:101: min > max. min = 1, max = 0`.
После исправления тест должен быть добавлен в `crates/hds-diarize/src/merge.rs`
(`#[cfg(test)]`) — см. задачу F1 плана.




