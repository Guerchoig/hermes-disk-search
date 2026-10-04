# PLAN_AUTO_TRANSCRIBE — автотранскрибация+диаризация «входная медиапапка → выходная» и UI присвоения имён спикерам

**Статус:** план **выполнен целиком**: **T0.1 ✅**, **T1 ✅**, **T2 ✅**, **T3 ✅**, **T4 ✅**,
**T5 ✅**. Формат вывода зафиксирован на реальном движке
(`tools/parity/T0_1_DIARIZATION_FORMAT.md`); конфиг `auto_transcribe.*` + контракт
`/internal/transcribe` + `gpu.priorities.transcribe`; клиент `transcribe_to_file`; демон
`hds transcribe-watch` с утилизацией исходника; арбитр VRAM (§6: 1 поиск/чат →
2 автотранскрибация → 3 индексация); UI (закладки, список/превью, присвоение имён,
«Перезапустить задание», правка папок); установка (fetch-скрипт модели, задача
автозапуска, пункты `hds check`/`whisper-check`) и доки (README/STATUS). Всё проверено
живыми прогонами на dev-машине; `cargo clippy --workspace --all-targets -- -D warnings`
— 0/0, `cargo test --workspace` — зелёные.
**Ветка/стек:** ядро — Rust (`crates/`), владелец GPU — резидент `llm-host`,
UI — `hds-ui` (плоский HTML, `TcpListener`), CLI — `hds`.
**Референс реализации транскрибации/диаризации:** `C:\Users\Sasha\transcribeoffline`
(тот же движок OpenResearchTools Engine; bridge-audio API).

---

## 0. Сценарий и цель

1. В `config.yaml` задаются **две директории**: `inbox_dir` (исходные медиа) и
   `out_dir` (транскрибированные/диаризованные результаты).
2. Пользователь кладёт медиафайл в `inbox_dir`. Демон, увидев **готовый** (целиком
   скопированный) файл, **немедленно** запускает подряд **транскрибацию и
   диаризацию** (одним вызовом движка, `mode: transcript`) и кладёт текстовый
   результат в `out_dir`. Реплики помечаются метками спикеров **в формате движка**
   (`SPEAKER_00`, `SPEAKER_01`, …) — как есть. Реплику, которую движок не смог
   отнести к конкретному спикеру, он помечает спецметкой `UNASSIGNED` (см. §5.4).
3. В UI (объединён с существующим): переход «Поиск ⇄ Транскрибация» — **закладками**.
   На странице транскрибации — **список файлов** `out_dir` и **окно превью**.
   Кнопка **«Присвоить имена спикерам»** открывает окно с таблицей **Спикер | Имя**:
   колонка *Спикер* заполняется автоматически из файла (`SPEAKER_00`, `SPEAKER_01`, …,
   включая `UNASSIGNED`),
   колонку *Имя* заполняет пользователь. По кнопке «Применить» имена
   подставляются в файл; **при частичном заполнении подставляются только
   заполненные** строки.

Дополнительно (из ТЗ): оценить объединение индексации и автотранскрибации (у папки
медиа — наивысший приоритет перед прочими файлами); чётко определить событие
«медиа помещено во входную папку»; ввести приоритеты ролей
**1) поиск и чат → 2) автотранскрибация → 3) индексация** (вытеснение из VRAM и др.).

**Зафиксированные решения (по итогам обсуждения):**
1. Формат меток спикеров — **как отдаёт движок** (`SPEAKER_00`, …), без преобразования (§5.4).
2. Формат выходного файла — **как отдаёт движок** (`.md`, без конвертации) (§2, §5.3).
3. Демон — **вариант A**: отдельная подкоманда `hds transcribe-watch` + своя задача автозапуска (§4).
4. Имена спикеров — **правка файла на месте** (+`.bak`), карта в `<stem>.speakers.json` (§8.3).
5. Индексация — `index_outputs: true`: `out_dir` индексируется, а **повторная ASR исходного медиа внутри индексации отключена** (§7).
6. Нет sortformer-модели при `diarization: true` — **жёсткий отказ** (задание падает), без деградации в `speech` (§2, §12).
7. Автозапуск демона — **да** (задача планировщика) (§9).

---

## 1. Инвентарь существующих точек расширения (по коду)

| Компонент | Файл / сущности | Роль в задаче |
|---|---|---|
| Watcher ФС | `crates/hds-index/src/watch.rs`: `WatchEvent`, `wait_stable`, `handle_event`, `run_watch`, `WatchLock`, `spawn_root_watcher` | детект появления медиа, стабилизация, очередь |
| Конвейер индексации | `crates/hds-index/src/pipeline.rs`: `process_file`, `run_index`, `media_will_process`, `index_filter` | индексный путь (низкий приоритет) |
| Медиа-ветка клиента | `crates/hds-index/src/transcribe.rs`: `MediaRouter`, `TranscribeClient`, `TranscribeConfig`, `lead_segment`, `resolve_whisper_model` | HTTP-клиент владельца GPU `/internal/transcribe` |
| Виды файлов | `crates/hds-index/src/kinds.rs`: `AUDIO_EXTS`, `VIDEO_EXTS`, `kind_of_path` | определение «это медиа» |
| HTTP владельца GPU | `crates/hds-llama/src/host.rs`: `internal_transcribe()` (~стр. 1343), `WhisperCell`, `whisper_device`, `whisper_idle_evict` | приём задания, ленивый транскрибатор |
| ASR-транскрибатор | `crates/hds-llama/src/whisper.rs`: `Whisper::transcribe_file`, `parse_srt`, `Transcript` | вызов bridge-audio, разбор вывода |
| Bridge-audio движка | `crates/hds-llama/src/bridge_audio.rs`: `BridgeAudio`, `Bridge::transcribe_raw`, `metadata_json` | FFI к `llama-server-bridge.dll` |
| Реестр ролей/приоритеты | `crates/hds-llama/src/config.rs`: `GpuConfig{priorities,policy}`, `RoleConfig`, `ROLES`, `build_role` | приоритеты, роли `chat/embedding/rerank/whisper` |
| Арбитр VRAM | `crates/hds-llama/src/dispatch.rs`: `plan_query`, `plan_indexing`, `eviction_order`, `eviction_order_for_indexing`, `idle_evictions`, `apply` | вытеснение, пауза индексации |
| Пауза индексации | `crates/hds-llama/src/pause.rs`: `IndexPause`, `PauseLease` (`index.pause`) | приоритет «запрос выше индексации» |
| CLI | `crates/hds-cli/src/main.rs`, `cmd/mod.rs`, `cmd/watch.rs`, `cmd/whisper_check.rs` | подкоманды демона/разовой обработки/проверки |
| UI | `crates/hds-ui/src/lib.rs` (`route`), `page.rs` (`PAGE`), `config_edit.rs` | маршруты, HTML, правка конфига |
| Автозапуск/установка | `install_autostart.ps1`, `installers/install_llm_host_task.ps1`, `installers/install_ui_task.ps1`, `installers/fetch_*.ps1` | задачи планировщика, доставка моделей |
| Модели движка | `%APPDATA%\OpenResearchTools\models\…` | whisper (есть), sortformer-диаризация (**есть**: `openresearchtools__diar_streaming_sortformer_4spk-v2.1-gguf`) |

**Факт из референса** (`transcribeoffline/src/main_backend_tail.rs`,
`settings.rs`): офлайн-диаризация включается `mode="transcript"` и metadata
`diarization_model_path`=`…/diar_streaming_sortformer_4spk-v2.1.gguf`,
`diarization_backend="sortformer"`, `diarization_feed_ms=10800001.0`; вывод —
текстовый файл (`.md`), реплики с меткой спикера вида `SPEAKER_00…`.
Пиксель-в-пиксель формат офлайн-вывода **требует спайка** (см. §12).

---

## 2. Конфигурация (схема)

Новая секция верхнего уровня `auto_transcribe` (отдельно от существующего ключа
`index.transcribe`, который управляет транскрибацией **внутри** индексации).
Правки — в `config.example.yaml`, боевом `config.yaml`, и в генераторе дефолтов
`hds/config.py` (Python-паритет); Rust читает конфиг как `serde_yaml::Value`
через `hds_core::config::dig` — новых структур не требуется.

```yaml
# --- Автотранскрибация + диаризация файлов из «входной» папки -----------------
auto_transcribe:
  enabled: true
  inbox_dir: 'D:\media_in'        # ВХОД:  исходные медиа (абсолютный путь)
  out_dir:   'D:\media_out'       # ВЫХОД: транскрибированные/диаризованные (абсолютный путь)
  recursive: false                # обходить подкаталоги inbox_dir
  # --- движок (перекрывают index.whisper_* только для этого конвейера) ---
  mode: transcript                # transcript = транскрибация+диаризация
  out_format: md                  # как отдаёт движок (.md) — без конвертации
  # метка «спикер не определён» (как её отдаёт движок; менять только по итогам спайка)
  unassigned_label: UNASSIGNED
  # --- диаризация (sortformer) ---
  diarization: true
  diarization_model: ''           # .gguf; '' → автопоиск %APPDATA%\OpenResearchTools\models\*sortformer*
  diarization_backend: sortformer
  diarization_feed_ms: 10800001
  diarization_required: true      # нет sortformer-модели ⇒ ЖЁСТКИЙ отказ задания (без fallback в speech)
  whisper_model: ''               # '' → index.whisper_model / автопоиск движка
  whisper_gpu: -1                 # -1 = как gpu.device_index; иначе индекс GPU
  # --- событие «файл помещён во входную папку» (см. §3) ---
  debounce_seconds: 8             # размер файла не меняется N сек ⇒ файл «готов»
  max_stable_wait: 120            # потолок ожидания стабилизации
  poll_seconds: 3                 # период reconcile-обхода инбокса (страховка от пропущенных событий)
  require_exclusive_read: true    # файл должен открываться на чтение (не занят писателем)
  extensions: []                  # [] = AUDIO_EXTS+VIDEO_EXTS (kinds.rs) + транзиентные исключения
  ignore_suffixes: ['.part', '.tmp', '.crdownload', '.!qb', '.bak']
  # --- очередь/идемпотентность ---
  queue_file: 'data/auto-transcribe-queue.jsonl'
  state_dir: 'data/auto-transcribe'
  overwrite_existing: false       # перетранскрибировать, если выход уже есть/свежее
  max_attempts: 3
  source_disposal: delete         # после УСПЕХА: delete (удалять исходник) | keep | move
  source_disposal_dir: ''         # для source_disposal: move — каталог архива (напр. D:\media_in\_done)
  # --- объединение с индексацией (§7) ---
  index_outputs: true             # индексировать out_dir; ASR исходного медиа в индексации ВЫКЛ (§7)
  pause_indexing: true            # на время задания ставить index.pause (приоритет выше индексации)
```

Приоритеты — расширение существующей секции `gpu`:

```yaml
gpu:
  priorities: { chat: 100, transcribe: 60, embedding: 40, rerank: 30, whisper: 20 }
  #             ^^^^ поиск/чат        ^^^^ автотранскрибация выше индексации
```

**Правила валидации (при старте демона и в `hds check`):**
- `inbox_dir` и `out_dir` заданы, существуют (или создаются демоном), различны;
- `out_dir` **не вложена** в `inbox_dir` и наоборот (иначе рекурсивная петля);
- `diarization: true` требует наличия sortformer-модели: **при её отсутствии — ЖЁСТКИЙ
  отказ** (валидация при старте демона + ошибка самого задания; без fallback в `speech`,
  §12) — аварийная ситуация не маскируется;
- `mode` ∈ {`transcript`,`speech`,`subtitle`}.

---

## 3. Чёткое определение события «медиа помещено во входную папку»

Проблема: производитель копирует файл **чанками** — ОС/`ReadDirectoryChangesW` шлёт
несколько `Modified`/`Created` до завершения копирования. Обработка «на первом
событии» даст битый/неполный файл. Правила (реализуются поверх
`hds-index::watch`):

1. **Фильтр по расширению.** Событие принимается только если
   `kinds::kind_of_path(path) == Some("media")` (`.mp3/.wav/.m4a/.flac/.ogg/.wma/.aac/.opus`
   + `.mp4/.avi/.mkv/.mov/.wmv/.flv/.webm/.mpg/.mpeg/.3gp/.mts`), с учётом
   `extensions` (если задан — только он).
2. **Отсечение транзиентных.** Имя с суффиксом из `ignore_suffixes`
   (`.part`, `.tmp`, `.crdownload`, `.!qb`, `.bak`) и с расширением `.tmp`
   — игнорируются (как сейчас `.tmp` в `handle_event`).
3. **Стабилизация (debounce).** Переиспользовать `watch::wait_stable(path, debounce, max_stable_wait)`
   — ждём, пока `len()` не меняется `debounce_seconds` (дефолт 8 c), потолок
   `max_stable_wait` (120 c). Это и есть точный момент «файл целиком записан».
   Отдельные значения дебаунса для инбокса — `auto_transcribe.debounce_seconds`.
4. **Проверка «не занят писателем»** (`require_exclusive_read: true`): пытаемся
   открыть файл на чтение (`std::fs::File::open`); при `PermissionDenied`
   (Windows-блокировка) — ждём и повторяем в пределах `max_stable_wait`.
5. **Дедуп/идемпотентность.** Ключ задания = абсолютный путь; журнал
   (`state_dir/manifest.jsonl`) хранит `(path, size, mtime, content_hash)`.
   Если выход есть и его mtime ≥ mtime входа и `overwrite_existing=false` — пропуск.
   Повторные события по тому же пути в пределах окна схлопываются (множество «в полёте»).
6. **Типы событий:**
   - `Modified`/`Moved(dst∈inbox)` → постановка в очередь (после пп. 1–5);
   - `Deleted` → снять из очереди (в т.ч. удаление источника **самим демоном** после
     успеха, §5.5 — не должно повторно ставить задание); выход **не трогаем**;
   - `Moved(dst∉inbox)` (файл унесли) → снять из очереди.
7. **Страховка от пропущенных событий.** Периодический `reconcile` (каждые
   `poll_seconds`) обходом `inbox_dir`: медиа без актуального выхода → в очередь
   (по образцу `watch.reconcile_on_start`). Это же покрывает рестарт демона.

Событие считается «медиа помещено» ⇔ **стабилизированный, читаемый, медиа‑файл,
для которого ещё нет актуального выхода**. Это определение тестируется без ФС
(чистая функция `should_enqueue(cfg, path, meta, manifest)`).


---

## 4. Демон автотранскрибации (архитектура, очередь, рестарт)

**Решение (зафиксировано): вариант A** — отдельный демон `hds transcribe-watch`
(безопасный и совместимый; не трогает боевой `hds watch`, независимый жизненный цикл,
своя задача автозапуска). Вариант B отклонён (оставлен ниже только как отвергнутая
альтернатива для истории).

### Вариант A (принят): отдельный демон `hds transcribe-watch`
Новый подкоманды-демон со своим `watch.lock`-подобным локом
(`transcribe.lock`), своим watcher'ом на `inbox_dir` и **своей** очередью.
Плюсы: не трогаем боевой `hds watch`, независимый жизненный цикл, отдельная
задача автозапуска; легко тестировать.

Компоненты (новый модуль `crates/hds-index/src/autotranscribe.rs`, pub-API через `lib.rs`):
- `AutoTranscribeConfig::from_config(cfg)` — разбор секции `auto_transcribe.*` (дефолты, валидация).
- `run_transcribe_watch(cfg, conn?, ...) -> Result<i32>` — цикл демона:
  1. захват лока (`transcribe.lock`, RAII, устаревший снимается) — как `WatchLock`;
  2. `spawn_root_watcher(inbox_dir, tx, stop)` (Windows `ReadDirectoryChangesW` / polling);
  3. поток-постановщик: `watch::recv` → `should_enqueue` (§3) → `queue.push`;
  4. **один** рабочий поток-исполнитель: берёт задания FIFO, зовёт
     `transcribe_client.transcribe_to_file(path)` (§5), пишет выход и sidecar (§5/§8),
     затем **утилизирует исходник** по `source_disposal` (§5.5);
  5. периодический `reconcile` (`poll_seconds`) на случай пропущенных событий;
  6. остановка — файл `transcribe.stop` (или Ctrl+C), `Drop` снимает лок.

Очередь и состояние:
- `queue_file` (`data/auto-transcribe-queue.jsonl`) — append-only JSONL:
  `{id, path, size, mtime, status, attempts, added_at, error?}`. Пишется **атомарно**
  (temp + `config.replace_file`), журнал компактится при старте.
- Идемпотентность и «уже обработано» — `state_dir/manifest.jsonl` (§3 п.5).
- **Рестарт:** при старте читаем очередь, отбрасываем `done`, восстанавливаем
  `pending`/`retry`; затем `reconcile` добавляет пропущенное. Двойной обработки нет
  благодаря манифесту + проверке выхода.

Одновременность: движок держит **один** bridge-audio (`WhisperCell`), поэтому
обработка **строго последовательная** (один исполнитель). Параллелить нельзя без
второго бриджа/второго GPU.

### Вариант B (отклонён): встроить в существующий `hds watch`
Тот же watcher, но два приоритетных потока очереди: **медиа‑инбокс (высокий)** и
**общая индексация (низкий)**. Усложнил бы боевой `watch` и его паритет с Python-веткой
— **не реализуется** (приоритет медиапапки обеспечен через `index.pause`, §6).

---

## 5. Транскрибация + диаризация (движок, метаданные, метки спикеров)

Движок выполняет транскрибацию и диаризацию **одним** вызовом bridge-audio
(`llama_server_bridge_audio_transcriptions_raw`), как в референсе. Отличие
текущего проекта: нет режима `transcript` и нет передачи диаризационных ключей.

### 5.1. Расширить HTTP-контракт `/internal/transcribe` (владелец GPU)
`crates/hds-llama/src/host.rs::internal_transcribe` — принимать дополнительные поля:
```json
{ "path": "...", "mode": "transcript", "custom": "auto", "gpu": 0, "model": "...",
  "diarization": true, "diarization_model": "...gguf",
  "diarization_backend": "sortformer", "diarization_feed_ms": 10800001,
  "return_text": true }
```
Возвращать `{segments:[...], text: "<полный текст вывода>", stats, ...}`.
Для `transcript` движок пишет `.md` — `text` вернём как содержимое этого файла
(расширить `Whisper::transcribe_file`, см. 5.2).

### 5.2. Расширить `crates/hds-llama/src/whisper.rs`
- `Whisper::transcribe_file(src, mode, custom)` — добавить параметр
  `diar: Option<&DiarizationParams>` и проброс в `metadata_json`:
  `diarization_model_path`, `diarization_backend`, `diarization_feed_ms`;
  при `gpu>=0` — `diarization_device` (имя устройства, как в референсе).
- `Transcript { json, segments, .. }` → добавить `raw_text: String` и `out_ext: String`
  (для `transcript`/`.md` — полный текст; для `subtitle`/`.srt` — как сейчас).
- Разбор `.srt` (`parse_srt`) сохранить; для `.md`/`.txt` — вернуть текст как есть.

### 5.3. Расширить клиент `crates/hds-index/src/transcribe.rs`
- `TranscribeConfig` + `TranscribeClient` — новые поля (`diarization*`, `out_dir`,
  `out_format`, `unassigned_label`) и метод
  `transcribe_to_file(path) -> Result<TranscribeOutcome>`:
  1. POST `/internal/transcribe` (`return_text: true`);
  2. **записать** полнотекстовый результат в `out_dir/<stem>.<ext>`
     (имя выхода = `file_stem` входа + `out_format`; коллизии — суффикс `-2`);
  3. **метки спикеров** — оставить как отдаёт движок (`SPEAKER_NN`), без преобразования (§5.4);
  4. записать sidecar `out_dir/<stem>.speakers.json` (карта плейсхолдер→имя, пустая)
     и `out_dir/<stem>.orig.md` (текст **до** подстановки имён — нужен для UI, §8);
  5. **утилизировать исходник** по `source_disposal` (§5.5) — только после успешной
     записи непустого выхода.
- Обратная совместимость: `MediaRouter` для индексации не меняется; новое —
  отдельный путь вызова демоном.

### 5.5. Утилизация исходного медиафайла (после успеха — удаление)
По ТЗ: **после завершения транскрибации/диаризации медиафайл удаляется**.
Реализуется как шаг исполнителя демона (вариант A) / ветки медиа‑очереди (вариант B)
с ключом `auto_transcribe.source_disposal`:

| Значение | Поведение |
|---|---|
| `delete` (**по умолчанию**) | удалить исходный медиафайл из `inbox_dir` |
| `keep` | оставить на месте (для отладки/повторной обработки) |
| `move` | переместить в `source_disposal_dir` (архивный каталог) |

Правила (безопасность):
- Удаляем **только при успехе**: выход записан, файл непустой, sidecar создан.
  При ошибке/таймауте/пустом выходе исходник **сохраняется** (для retry по
  `max_attempts`); окончательно «забракованные» (исчерпаны попытки) — остаются в
  `inbox_dir` и помечаются в манифесте (`status=failed`), чтобы `reconcile` их не
  зацикливал (пропуск по манифесту).
- Удаление — **после** атомарной записи выхода (порядок: выход+sidecar → `fsync` →
  `remove_file`). Если удаление не удалось (блокировка/права) — задание всё равно
  помечается `done`, в лог `warn`, исходник остаётся (не «теряем» данные).
- **Идемпотентность и reconcile:** удалённый файл больше не находится обходом — повторно
  не ставится. Манифест хранит исходный `path`, `size`, `mtime`, `content_hash` и
  `disposed: delete|move|keep` — для аудита и для корректной работы «переименовать
  заново» в UI (по `<stem>.speakers.json`, а не по исходнику).
- **Санкция `delete`:** тип деструктивной операции — логируется (путь + размер),
  дублируется в `data/logs/auto-transcribe.log`; при `source_disposal_dir` внутри
  `inbox_dir` — валидация запрещает (как и вложенность `out_dir`).

### 5.4. Формат меток спикеров в выводе
**Решение (зафиксировано): сохраняем метки движка «как есть» — `SPEAKER_00`,
`SPEAKER_01`, …** Никакого преобразования формата нет. Он проще, однозначен (ограничен
`SPEAKER_`+цифры) и совпадает с референсом — его `detect_speaker_slots`/
`apply_speaker_renames` работают именно с токенами `SPEAKER_NN`. Нумерация у движка
**с нуля** (`SPEAKER_00` — первый спикер).

**Неприсвоенные реплики.** Когда движок не может отнести фрагмент к конкретному
спикеру (голос/содержание похожи на нескольких), в референсе для этого есть спецметка
**`UNASSIGNED`** (`transcribeoffline/src/audio_assembler.rs`). Точную метку офлайн-вывода
подтверждает спайк (T0.1, §12). В плане она трактуется как отдельный «спикер»:
- детекция включает `unassigned_label` (`UNASSIGNED`) — и, на всякий случай,
  `UNKNOWN`/`SPEAKER_UNKNOWN` — отдельной строкой таблицы; пользователь может дать ей
  имя (напр. «Неизвестный») или оставить как есть;
- при переименовании `UNASSIGNED` заменяется так же, как `SPEAKER_NN` (непустым именем);
- метка **опциональна**: sortformer — модель на 4 голоса и обычно выбирает доминирующего
  спикера, поэтому в офлайне `UNASSIGNED` может не появляться; код работает в обоих случаях.

**Решение по детекции/подстановке токенов (зафиксировано): без `regex`, рукописный
сканер.** Обоснование:
- токены спикеров порождает **сам движок** в фиксированном формате `SPEAKER_NN` —
  матч однозначен, регулярные выражения не требуются;
- проект последовательно избегает новых зависимостей (`config_edit.rs` — «без regex»);
  `regex` (три крейта) добавил бы зависимость ради тривиального матчинга;
- `regex` при этом **доступен offline** (в локальном кэше, уже в дереве через
  `tokenizers`) — значит это **осознанный выбор стиля**, а не вынужденная мера
  (недоступны лишь крейты, которых нет в кэше, напр. `notify`).

Реализация (в `crates/hds-ui/src/transcribe.rs`, без новых крейтов):
- **сканер токенов:** `SPEAKER_<цифры>` (диалект движка) плюс литерал `unassigned_label`;
- `detect_speaker_slots(text) -> Vec<String>` — уникальные токены в порядке первого
  появления (колонка «Спикер»), включая `UNASSIGNED`, если он есть;
- `apply_renames(text, map) -> (String, usize)` — точная замена непустых имён
  (пустые/совпадающие с токеном пропускаются ⇒ частичное заполнение), счётчик замен.

Тот же сканер (в `hds-index`) применяется и при разборе вывода движка (при необходимости).
Легаси-диалекты (`UNKNOWN`, `Speaker NN`) добавляются в сканер по итогам спайка §12,
зависимости не вводятся.


---

## 6. Приоритеты и вытеснение из VRAM (1 поиск/чат → 2 автотранскрибация → 3 индексация)

Сейчас: `gpu.policy: query_priority`, `priorities: {chat:100, embedding:40, rerank:30, whisper:20}`;
`dispatch::eviction_order` сортирует по возрастанию приоритета (вытесняются первыми
роли с меньшим числом). Запрос (`plan_query`) при нехватке ставит `index.pause` и
вытесняет `whisper→rerank→embedding`; индексация (`plan_indexing`) вытесняет
резидентный `chat` последней.

**Целевая иерархия:**
1. **Поиск и чат** (`chat`, роль поиска) — высший.
2. **Автотранскрибация** (bridge-audio `whisper` **под заданием демона**) — середина.
3. **Индексация** (`embedding`/`rerank`, работа конвейера `hds index`/`watch`) — низший.

Правки (минимальные, поверх существующих «чистых» функций арбитра):
- `crates/hds-llama/src/config.rs`: добавить ключ `gpu.priorities.transcribe`
  (дефолт **60**), не меняя дефолт `whisper` (20). Роль-имя `transcribe` —
  «логическая» роль задания автотранскрибации; физически это тот же bridge-audio.
- `crates/hds-llama/src/host.rs::internal_transcribe`: при вызове **демоном** —
  брать аренду `index.pause` (`self.prepare("transcribe")`, как остальные роли) на
  время задания и снять по завершении. Это реализует «2 > 3»: пока идёт
  автотранскрибация, индексация на паузе.
- `crates/hds-llama/src/dispatch.rs`:
  - новый `plan_transcribe(gpu, instances, demand)` — как `plan_query`, но
    `protect_role="transcribe"`, при нехватке вытесняет **индексные** роли
    (`embedding`/`rerank`) первыми; `chat` — только если совсем не хватает
    (и то по политике `fixed` лучше честный «не хватает VRAM»-отчёт, без деградации);
  - в `plan_indexing` и `eviction_order_for_indexing` — **не вытеснять** инстанс,
    помеченный «активно задание автотранскрибации» (иначе индексация, дойдя до
    медиа, выбьет собственный транскрибатор);
  - `idle_evictions` — существующий `whisper_idle_evict` (ARB-5) сохранить:
    по простою bridge-audio выгружается, VRAM возвращается.
- `crates/hds-llama/src/config.rs`: `ROLES`/`role_defaults` — учесть `transcribe`
  в отчётах `status` (строка роли), чтобы приоритет был видим в `/internal/status`.

**Итог для пользователя:** запрос чата/поиска вытесняет автотранскрибацию и
индексацию; автотранскрибация вытесняет/паузит индексацию; индексация уступает
обоим. Всё — через `index.pause`-аренды (уже есть механика `pause.rs`) и `dispatch`.

---

## 7. Объединение индексации и автотранскрибации

**Решение (зафиксировано): объединяем через общую инфраструктуру и приоритеты**
(вариант A, §4), а не общий поток — полностью «сливать» конвейеры рискованно
(боевой `watch`/`index` имеют Python-паритет).

- **Событие и очередь — общие механизмы.** Демон автотранскрибации и индексатор
  используют один watcher (`hds-index::watch`) и общий арбитр VRAM.
- **Наивысший приоритет у медиапапки.** Задание автотранскрибации ставит `index.pause`
  (§6), т.е. идёт «вперёд» любых индексных файлов.
- **`index_outputs: true` (принято).** `out_dir` индексируется как обычный текст
  (`.md` ∈ `TEXT_EXTS`) — файлы из `out_dir` попадают в обход индексатора.
- **Повторная ASR исходного медиа внутри индексации — ОТКЛЮЧЕНА (принято).** Чтобы не
  транскрибировать дважды: при активной автотранскрибации медиа-ветка `MediaRouter` в
  конвейере индексации не запускает ASR — исходное медиа попадает в индекс **только**
  ведущим сегментом-метаданными (`lead_segment`, ffprobe). Ключ `index.transcribe`
  остаётся legacy для корней **вне** автотранскрибации.


---

## 8. UI: закладки, список, превью, окно имён спикеров

UI — одностраничный HTML (`crates/hds-ui/src/page.rs`, константа `PAGE`) + маршруты
(`crates/hds-ui/src/lib.rs::route`). `hds-ui` имеет `#![forbid(unsafe_code)]`; правки —
чистый Rust + JS в строке.

### 8.1. Закладки
- В `<main>` добавить навигацию `<nav class="tabs">` с кнопками **«Поиск»** (текущие
  секции статуса/поиска/ask/дерево/настройки — обернуть в `<div id="tab-search">`)
  и **«Транскрибация»** (`<div id="tab-transcribe">`).
- JS: `showTab('search'|'transcribe')` переключает видимость и подсветку; активная
  закладка запоминается в `localStorage`. Существующие функции (`refresh`, `loadDiag`, …)
  продолжают работать без изменений.

### 8.2. Страница «Транскрибация»
- **Список файлов** `out_dir` (`GET /api/transcribe/list`): имя, размер, изменён,
  число распознанных спикеров, признак «имена присвоены» (по sidecar), статус.
  Клик по строке → **превью** (`<pre>`, read-only).
- **Окно превью** — блок с текстом выбранного файла и подписью пути (полный путь —
  в `title`).
- Кнопка **«Присвоить имена спикерам»** (активна при выбранном файле) → **модальное
  окно** с таблицей **Спикер | Имя**:
  - строки из детектированных спикеров файла (`GET /api/transcribe/file` возвращает
    `speakers: ["SPEAKER_00", "SPEAKER_01", …]`, включая `UNASSIGNED`);
  - колонка *Спикер* — только чтение (моно-шрифт), колонка *Имя* — `<input>`
    («Имя или метка»);
  - кнопки «Применить» и «Закрыть».

### 8.3. Новые маршруты (`hds-ui`) — новый модуль `src/transcribe.rs`
| Метод | Путь | Тело/ответ |
|---|---|---|
| GET | `/api/transcribe/list` | `{dir, files:[{name,size,mtime,speakers,renamed}]}` |
| GET | `/api/transcribe/file?name=…` | `{name,path,text,speakers:[…],renamed}` |
| POST | `/api/transcribe/speakers` | `{name, names:{"SPEAKER_00":"Иван",…}}` → `{ok,replaced,saved}` |
| POST | `/api/transcribe/apply` | (опц.) повтор/перезапуск задания для файла |

Правила:
- **Безопасность путей:** `name` разрешается строго внутри `out_dir`; запрещены `..`,
  абсолютные пути, разделители каталогов, уход по репарс-точкам (дисциплина как в
  `db_move`/`tree`).
- POST — через существующую CSRF-проверку `csrf_ok` (Origin loopback +
  `application/json`/`X-HDS-UI: 1`).
- **Подстановка (частичная):** для каждой строки, если `Имя` непустое и не равно
  метке — заменить токен (`SPEAKER_NN` или `UNASSIGNED`) на имя. Пустые/
  совпадающие — пропускаются ⇒ «подставляются только заполненные». Запись **атомарная**
  (temp + replace); перед заменой — копия `.bak` (одна, перезаписываемая).
- Sidecar `<stem>.speakers.json` обновляется `{placeholder: name}`; повторное открытие
  окна подставляет уже сохранённые имена (аналог `previous` в референсе).

### 8.4. Настройки в UI (опция)
- На закладке «Транскрибация» — блок «Папки»: `inbox_dir` / `out_dir` редактируются
  и сохраняются в `config.yaml` тем же строчным редактором, что `config_edit.rs`
  (без regex); применяется к новым запускам.


---

## 9. Установка, автозапуск, модели

- **Модель диаризации.** Новый `installers/fetch_diarization_model.ps1` —
  скачивание `diar_streaming_sortformer_4spk-v2.1.gguf` (≈471 МБ) в
  `%APPDATA%\OpenResearchTools\models\openresearchtools__diar_streaming_sortformer_4spk-v2.1-gguf\`
  (URL/размер — как в референсе: `transcribeoffline/src/main.rs` константы
  `DIARIZATION_MODEL_URL`, `DIARIZATION_MODEL_SIZE_BYTES`). На текущей машине модель
  **уже есть** — доставка нужна для «чистых» установок; добавить проверку в `hds check`
  и `hds whisper-check` (показ наличия sortformer).
- **Задача автозапуска демона (принято: да).** Расширить `install_autostart.ps1` третьей
  задачей `HermesDiskSearchTranscribe` → `<hds.exe> transcribe-watch` (по образцу
  `HermesDiskSearchWatch`). Учесть `-StartupFolder`-фолбэк и `hidden_launch.vbs`.
- **setup.ps1**: вызвать `fetch_diarization_model.ps1` (best-effort, как остальные
  `fetch_*`), зарегистрировать задачу демона.
- **Документация:** `config.example.yaml` (секция §2), `README.md` (раздел
  «Автотранскрибация»), `STATUS.md`, `RELEASE_NOTES_*`.

---

## 10. Разбиение на этапы (задачи)

**T0. Спайки (до кода):**
- T0.1 Прогнать движок на фикстуре с `mode: "transcript"` + diarization metadata →
  зафиксировать **точный формат офлайн-вывода** (метка спикера, расширение,
  наличие таймкодов), путь/имя выходного файла.
- T0.2 Замерить VRAM/время диаризации (sortformer) с whisper-large-v3-turbo на RTX 3060.
- T0.3 Проверить `diarization_feed_ms`/`custom` и поведение на длинных файлах
  (cut/окно), ограничения `max_media_mb`.

**T1. Конфиг + контракт движка:**
- T1.1 Схема `auto_transcribe.*` в `config.example.yaml`/`config.yaml`/`hds/config.py`.
- T1.2 `whisper.rs`: `transcribe_file` c `DiarizationParams`, `raw_text`/`out_ext`.
- T1.3 `host.rs::internal_transcribe`: приём `diarization*`, `return_text`.
- T1.4 `hds-llama::config`: `gpu.priorities.transcribe`, строка роли в status.

**T2. Клиент и конвейер демона:**
- T2.1 `transcribe.rs`: `transcribe_to_file` (POST + запись выхода + sidecar).
- T2.2 `autotranscribe.rs`: `should_enqueue`, очередь JSONL, манифест, reconcile,
  `run_transcribe_watch` (вариант A).
- T2.3 `hds-cli`: подкоманда `transcribe-watch` и разовая `transcribe-once <file>`;
  регистрация в `main.rs`+`cmd/mod.rs`.
- T2.4 **Утилизация исходника** (`source_disposal`: delete/keep/move) — только после
  успешной записи выхода; обработка ошибок удаления; запись `disposed` в манифест (§5.5).

**T3. Приоритеты/арбитр:**
- T3.1 `dispatch::plan_transcribe` + защита транскрибатора в `plan_indexing`.
- T3.2 `internal_transcribe` — `prepare("transcribe")` (пауза индексации).
- T3.3 Тесты арбитра (расширить `tests/arbiter.rs`).

**T4. UI:**
- T4.1 `hds-ui/src/transcribe.rs`: list/read/detect_speakers/apply_renames (path-safety,
  токены спикеров — рукописным сканером, без `regex`).
- T4.2 Маршруты в `route()` (+CSRF).
- T4.3 `page.rs`: закладки, страница «Транскрибация», превью, модалка «Спикер|Имя».

**T5. Установка/доки/тесты:** `fetch_diarization_model.ps1`, `install_autostart.ps1`,
`setup.ps1`, README/STATUS/RELEASE_NOTES, интеграционные тесты (§11).

---

## 11. Тесты и критерии приёмки

**Юнит-тесты (без GPU/ФС):**
- `should_enqueue`: медиа/не-медиа, транзиентные суффиксы, уже готовый выход,
  повтор в окне (дедуп), файл занят.
- Метки спикеров: детекция `SPEAKER_00/01/…` в порядке первого появления; `UNASSIGNED`
  попадает в список (если присутствует); идемпотентность.
- `apply_renames`: частичное заполнение (только непустые), совпадение с меткой
  пропускается, счётчик замен, атомарность+`.bak`.
- Path-safety UI: `../`, абсолютные, вложенность — отклоняются.
- Очередь: перезагрузка из JSONL, retry при ошибке (`max_attempts`).
- Утилизация (§5.5): `delete` срабатывает **только при успехе**, при ошибке исходник
  сохраняется; `move` переносит в `source_disposal_dir`; неудача удаления → `done`+`warn`.

**Интеграционные:**
- `hds transcribe-once <fixture.wav>` при поднятом `llm-host` → в `out_dir` появляется
  файл с метками `SPEAKER_NN` (как отдаёт движок); sidecar `*.speakers.json` создан.
- E2E демона: положить файл в `inbox_dir` → (без ручных действий) появился выход,
  а **исходник удалён** из `inbox_dir`; повторное событие/`reconcile` не дублирует;
  при смоделированной ошибке транскрибации исходник остаётся.
- Арбитр: во время задания чат-запрос вытесняет транскрибацию; индексация ставится
  на паузу; после завершения пауза снимается.

**Критерии приёмки (из ТЗ):**
1. Две директории в конфиге; демон кладёт результат из `inbox` в `out_dir`.
2. Результат — текстовый файл с метками спикеров **в формате движка** (`SPEAKER_00`,
   `SPEAKER_01`, …); неприсвоенные реплики — `UNASSIGNED` (если движок их отдаёт).
3. UI: закладки «Поиск ⇄ Транскрибация»; список файлов `out_dir`; окно превью.
4. Кнопка «Присвоить имена спикерам» → таблица Спикер|Имя (Спикер авто, Имя ввод);
   после «Применить» имена подставляются; частичное заполнение учтено.
5. Приоритеты 1) поиск/чат 2) автотранскрибация 3) индексация соблюдены.
6. **После успешной транскрибации/диаризации исходный медиафайл удаляется**
   (`source_disposal: delete`, по умолчанию); при ошибке — сохраняется (§5.5).
7. `cargo clippy --workspace --all-targets -- -D warnings` — 0/0; тесты зелёные.

---

## 12. Риски и открытые вопросы

**Риски:**
- **Формат офлайн-диаризации движка не подтверждён** (T0.1). Если движок не отдаёт
  «чистый» transcript-файл, а только сегменты — потребуется собственная сборка
  реплик (как `transcribeoffline::audio_assembler`), это существенно больше работы.
- **Одна модель/один bridge** ⇒ строгая последовательность заданий; длинные медиа
  блокируют очередь (приемлемо, но нужно UX-индикация).
- **Windows-блокировки/не-ASCII пути** — референс делает ASCII-стейджинг; у нас
  стейджинг уже есть в `whisper.rs` (`%TEMP%\hds_whisper`), сохранить при расширении.
- **Новые зависимости:** crates.io offline; крейты из **локального кэша** добавлять
  можно (напр. `regex` — уже в дереве через `tokenizers`), реально недоступны лишь
  отсутствующие в кэше (`notify`). Для этой фичи новые зависимости **не вводим**:
  детекция/подстановка токенов спикеров — рукописным сканером (§5.4).
- **Рефактор `hds-ui`** (обёртка HTML в закладки) может затронуть существующие
  функции JS — покрыть ручной проверкой всех секций.

**Открытых вопросов нет** — все решения зафиксированы в §0. Оставшаяся неопределённость
(точный формат офлайн-вывода движка) снимается спайком T0.1, а не решением заказчика.


---

## 13. Контекст для продолжения (handoff)

**Статус:** план готов, **реализация НЕ начата**.

**Задача (кратко).** Автотранскрибация+диаризация: конфиг с двумя папками
(`inbox_dir` → `out_dir`); демон видит новый медиафайл, прогоняет транскрибацию+
диаризацию движком (владелец GPU — `llm-host`), пишет текстовый файл с метками
спикеров `SPEAKER_00…` (и `UNASSIGNED`) в `out_dir`, удаляет исходник; UI с
закладками «Поиск ⇄ Транскрибация»: список файлов, окно превью, кнопка «Присвоить
имена спикерам» (таблица Спикер|Имя, частичная подстановка).

**Зафиксированные решения:** см. §0 (7 пунктов). Открытых вопросов нет (§12).

**Первые шаги (порядок работ):**
- **T0.1 (спайк, делать первым).** ✅ **Выполнен** — реальный вывод движка
  зафиксирован в `tools/parity/T0_1_DIARIZATION_FORMAT.md` (метка `SPEAKER_NN` +
  `UNASSIGNED`, `.md`, таймкоды `HH:MM:SS`). Спайк идёт через **нашу** пробу
  `crates/hds-llama/src/bin/diar_probe.rs` (стоковый `example-cli.exe` несовместим
  с патченными DLL движка), обвязка — `tools/parity/spike7_diarization.py`.
- **T1.** ✅ **Выполнен** — `auto_transcribe.*` в `config.example.yaml`/`config.yaml`/
  `hds/config.py`; `whisper.rs` (`DiarizationParams`, `raw_text`/`out_ext`);
  `host.rs::internal_transcribe` (`diarization*`, `return_text`, автопоиск
  sortformer, жёсткий отказ без модели); `gpu.priorities.transcribe = 60`
  (`config.rs`, `TRANSCRIBE_ROLE`).
- **T2.** ✅ **Выполнен** — `transcribe.rs::transcribe_to_file` (+`detect_speaker_slots`,
  sidecar `*.speakers.json`/`*.orig.<ext>`, коллизии `-2`), новый модуль
  `crates/hds-index/src/autotranscribe.rs` (конфиг+валидация, `should_enqueue`,
  манифест/очередь JSONL, reconcile, демон, утилизация §5.5, шлюз `index.pause`),
  CLI `hds transcribe-watch|transcribe-once|transcribe-stop`. **Живой E2E пройден**
  (dev-машина): файл в `inbox_dir` → `.md` c `### SPEAKER_00` в `out_dir` +
  `*.speakers.json` + `*.orig.md`, исходник удалён, манифест `done/delete`,
- **T3.** ✅ **Выполнен** — `dispatch::plan_transcribe` (+`eviction_order_multi`,
  защита носителя `whisper`; индексные роли уступают первыми, `chat` — последним),
  поле/билдер `InstanceUse.transcribe_active` (индексация и предохранитель простоя
  не выбивают транскрибатор; запрос чата может — 1 > 2), `plan_indexing` поясняет
  защиту; `host.rs::prepare_demand` + `TranscribeGuard` + `transcribe_need_mib`
  (в `internal_transcribe` — пауза индексации и вытеснение под задание, §6).
  Живой прогон: в логе резидента `вердикт fits: модель+KV 2254` (= whisper 1549 +
  sortformer 449 + буфер 256), `обеспечить загрузку роли 'whisper'`,
  `приоритет роли 'transcribe' = 60`. Тесты арбитра — 7 новых (24 всего).
- **T4.** ✅ **Выполнен** — новый модуль `crates/hds-ui/src/transcribe.rs`
  (`list_json`/`file_json`/`speakers_json`, `replace_speaker_labels`, `rename_pairs`,
  path-safety `is_safe_name`/`resolve_out_file` с `canonicalize`, `.bak` + атомарная
  запись, sidecar-мерж); маршруты `/api/transcribe/{list,file,speakers}` (POST — под
  CSRF); `page.rs`: закладки «Поиск ⇄ Транскрибация» (`localStorage`), список файлов
  `out_dir`, превью, модальное окно «Спикер | Имя» с частичной подстановкой.
  Живой прогон: список из 3 файлов, `rename1 SPEAKER_00→Иван` (1 замена), затем
  `rename2 Иван→Пётр` (1 замена), `.bak` создан, sidecar `{"SPEAKER_00":"Пётр"}`,
  файл содержит `### Пётр [00:00:01 - 00:00:11]`. Тесты: +5 unit, +5 integration.
- **T5.** ✅ **Выполнен** — установка и доки:
  * `installers/fetch_diarization_model.ps1` (URL/размер из референса: 471 107 712 Б);
  * `install_autostart.ps1` — третья задача `HermesDiskSearchTranscribe` (`hds transcribe-watch`),
    + фолбэк в Startup и `hidden_launch.vbs`; `setup.ps1` — вызов fetch-скрипта;
  * `hds check` — пункт `diarization`; `hds whisper-check` — строка о sortformer;
  * UI: **§8.4** (`config_edit::replace_scalar`/`set_transcribe_dirs`, маршрут
    `/api/config/transcribe-dirs`, блок «Папки конвейера») и **`POST /api/transcribe/apply`**
    (кнопка «Перезапустить задание»; задание кладётся в журнал очереди, демон подхватывает
    его периодическим `merge_external_jobs`);
  * UI: **управление демоном** — `GET/POST /api/transcribe/daemon` (`status|start|stop|restart`),
    блок «Демон автотранскрибации» (состояние по `transcribe.lock`+`lock_is_stale`,
    кнопки Запустить/Остановить/Перезапустить; запуск detached, стоп — `transcribe.stop`);
  * README (раздел «Автотранскрибация»), STATUS.md (сводка 04.10.2026).
  Живые проверки: `hds check` → `[ok] модель диаризации (sortformer)`, `whisper-check` → путь
  к .gguf; UI — `transcribe-dirs` сохранил папки (боевой `config.yaml` не тронут),
  `apply` поставил задание в очередь (`daemon:false`, честное сообщение).
  Тесты: 8 unit + 12 integration в `hds-ui`, +1 в `hds-index` (merge внешних заданий).

**Ключевые точки кода:**
`crates/hds-index/src/{watch,pipeline,transcribe,kinds}.rs`;
`crates/hds-llama/src/{host,whisper,bridge_audio,config,dispatch,pause}.rs`;
`crates/hds-cli/src/{main.rs,cmd/}`;
`crates/hds-ui/src/{lib.rs,page.rs}` (+ новый `transcribe.rs`);
`installers/*`, `install_autostart.ps1`, `config.example.yaml`, `config.yaml`,
`hds/config.py`.

**Референс (тот же движок):** `C:\Users\Sasha\transcribeoffline` —
`main_backend_tail.rs` (офлайн-диаризация: `mode:"transcript"`, metadata
`diarization_model_path`/`diarization_backend:"sortformer"`/`diarization_feed_ms`);
`audio_assembler.rs` (`SPEAKER_00`, `UNASSIGNED`); `main.rs`
(`detect_speaker_slots`, `apply_speaker_renames`).

**Инфраструктура (проверено на этой машине):** sortformer-модель уже установлена —
`%APPDATA%\OpenResearchTools\models\openresearchtools__diar_streaming_sortformer_4spk-v2.1-gguf\
diar_streaming_sortformer_4spk-v2.1.gguf`; движок —
`%APPDATA%\OpenResearchTools\TranscribeOffline\Engine`; `regex` доступен offline
(через `tokenizers`), но **не используется** (рукописный сканер).

**Правила проекта:** новые зависимости не вводим; не ломать Python-паритет боевых
`watch`/`index`; комментарии/тексты — по-русски; стиль — как в существующих `PLAN_*.md`;
прогон `cargo clippy --workspace --all-targets -- -D warnings` — 0/0.

