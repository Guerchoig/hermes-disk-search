# ANONYMIZER_SIDECAR_HANDOFF_PROMPT.md — контекст и промт для нового чата

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать **§9 целиком**
> (он самодостаточен) и отправить первым сообщением. Этот документ — «карта» состояния:
> его можно читать целиком, но промт в §9 уже содержит главное.
>
> **Дата среза:** 26.10.2026. **Ветка:** `anonymizer-sidecar`, HEAD `60755a8`
> (этот handoff-коммит; последний код-коммит — `5ba55d3`).
> Сделано и закоммичено: **P1, P1b, P2, P3, P4** (план `PLAN_ANONYMIZER_SIDECAR.md`,
> tracked). Осталось: **P5 (UI), P6 (установщики+веса), P7 (приёмка/доки)**.
> Другое изменение в дереве — `M sidecar/hds_transcribe/glossary.example.yaml` —
> **чужое**, не трогать.
>
> Проверки на срезе: `cargo test --workspace` 110 ok / 0 FAILED; `cargo fmt --check`,
> `cargo clippy --workspace --all-targets -- -D warnings` — 0; sidecar-тесты
> (`test_anonymize_*`) 28/28 OK.

---

## 0. Что происходит

Функции полной/частичной анонимизации и деанонимизации документов из внешнего проекта
`C:\Test\anonymizer_proxy` (отменён как отдельный продукт) переносятся **в этот проект**
по проверенному паттерну «Python-sidecar + Rust-клиент»:

* **Python-sidecar** `sidecar/hds_anonymize/` — NER-движки и Office round-trip
  (копия из `anonymizer_proxy` без изменения алгоритмов), БД токенов;
* **Rust-клиент** `crates/hds-anonymize` — запуск воркера, типизированный протокол,
  ошибки `kind`, lease VRAM, обход папки;
* **CLI** `hds anon` (сделан); **UI** — карточка «Анонимизация» в `hds ui` (P5).

**Не делаем** (решение заказчика): перехват команд агентов, OpenAI-прокси, OpenAI API.
Оркестратор пока не делаем — протокол готовит к нему (`inspect`), состояние потом будет
храниться на уровне оркестратора.

## 1. Обязательное чтение (в этом порядке)

1. `PLAN_ANONYMIZER_SIDECAR.md` — **весь**: §1 (решения), §2.1 (sidecar), §2.2 (GPU/lease),
   §2.3 (Rust-клиент), §2.4 (CLI), §2.5 (UI), §2.6 (конфиг `anonymize.*`), §2.7
   (установщики), §3 (протокол), §4 (фазы), §5 (приёмка), §6 (риски), §7 (решения).
2. `sidecar/hds_anonymize/{worker,handlers,mapping_store,settings,schemas}.py` —
   текущий воркер (см. §2 ниже).
3. `crates/hds-anonymize/src/{client,protocol,error,settings,folder,lease}.rs`.
4. `crates/hds-llama/src/{dispatch,host,facade}.rs` — фрагменты lease: `plan_anon_enter`,
   `internal_anon_enter/exit`, маршруты `/internal/anon/*` (см. §2, P3).
5. Образцы для P5: `crates/hds-ui/src/transcribe.rs` (модуль карточки) + `page.rs`
   (страницы) + **`crates/hds-ui/src/lib.rs`** — маршруты `/api/transcribe/*`,
   аудит `admin_op`; новый `/api/anonymize/*` регистрируется там же.
6. Источник переноса (если нужен оригинал): `C:\Test\anonymizer_proxy\anonymizer_proxy\`.

## 2. Что уже сделано (не переделывать)

### P1 — sidecar `sidecar/hds_anonymize/` (коммит 6e4038e)

* Копии из `anonymizer_proxy` **без правок алгоритмов**: `gliner_engine.py`,
  `natasha_engine.py`, `ner_service.py`, `replacer.py`, `file_parser.py`.
  Правки: импорты (`..config` → `.settings`), имена логгеров (`hds_anonymize.*`),
  **offline-first** (сетевые фоллбэки удалены: natasha `_ensure_downloaded` не качает,
  gliner только `local_files_only=True`).
* `schemas.py` — `Entity`/`MappingEntry` как dataclasses (pydantic в окружении
  `sidecar/python` НЕТ).
* `settings.py` — замена `config.py`: `NER_ENGINE`/`PII_CATEGORIES`; `configure()`
  из аргументов воркера (`--model --threshold --max-input-chars --chunk-overlap
  --timeout --natasha --models-dir --db --device --gpu-mem-limit-mb`); `config.yaml`
  воркер НЕ читает.
* `mapping_store.py` — SQLite WAL (`data/anonymize.db`): `mappings(token PK, value,
  type, ts)` + `counters(type PK, last)`, **per-type счётчики + дедуп по
  (значение, тип)**, `busy_timeout=5000` + выдача номера в `BEGIN IMMEDIATE`.
* `worker.py` — JSON-RPC 2.0 через `hds_sidecar` (PROTOCOL 1): `hello`
  (`backend/providers/model/formats=["docx","xlsx","txt","md","xml"]/capabilities`),
  `warmup`, `inspect`, `anonymize`, `anonymize_values`, `deanonymize`,
  `deanonymize_values`, `shutdown`; progress-нотификации без `id`; env
  `HF_HOME=<models-dir>/hf-home`, `HF_HUB_OFFLINE=1`; нет весов → `kind: model_missing`
  + hint про `installers/fetch_anonymize_models.ps1`.
* `handlers.py` — оркестрация: parse → NER (или детерминированный список `values`) →
  split `\n` → `split_entities_by_segments` → построчный `TextReplacer.anonymize` →
  join → `FileAssembler.assemble` (анон: `strip_hf_images=True, scrub_metadata=True`;
  деанон: `False/False`) → `_atomic_write_bytes` (создаёт родительский каталог;
  `PermissionError` → `FileBusyError`).
* Имена выходов строит **Rust**, но воркер умеет и сам: `<stem>.anonymized.<ext>` /
  `<stem>.deanon.<ext>` (суффикс `.anonymized` у входа отбрасывается); `..` в
  out_dir/out_path → `kind: out_of_dir`.

### P1b — XML (входит в 23943f1)

* `hello.formats` включает `xml`; fixture `sidecar/tests/fixtures/anonymize/msproject.xml`;
  тесты `test_anonymize_xml.py` (round-trip текст/теги идентичны).
* **Исправлен баг оригинала**: `_assemble_xml` замена ПОЗИЦИОННАЯ, а не по `path`
  (пути в structure неуникальны — `Task/Name` коллидировали).

### P2 — Rust-клиент `crates/hds-anonymize` (23943f1)

* `hds-extract`: `Worker::request` **пропускает** progress-нотификации без `id`
  (общий дедлайн `request_timeout`), `protocol::is_notification`.
* Крейт: `protocol.rs` (`HelloInfo/TokenCount/AnonymizeResult/DeanonymizeResult/
  InspectResult/WarmupResult`, `out_path()` — схема имён), `error.rs`
  (`kind: model_missing|file_busy|unsupported|db_missing|out_of_dir|gpu_busy`),
  `settings.rs` (§2.6: `output_dir, db_path, models_dir(HDS_MODELS_DIR/anonymize),
  model, threshold 0.3, max_input_chars 1500, chunk_overlap 200, timeout_sec 300,
  natasha, max_file_mb, gpu, llm_host_url, gpu_reserve_mb, lease_timeout_sec,
  request_timeout_sec 600, idle_timeout_sec 60, notices` → `worker_args[_for]`),
  `folder.rs` (`list_files` — верхний уровень по `hello.formats`),
  `client.rs` (`Client::spawn` проверяет cap `anonymize`).
* ВАЖНО: `values=Some` → отдельный RPC `anonymize_values` (не `anonymize` с values!),
  `tokens=Some` → `deanonymize_values` — воркер различает методы.

### P3 — GPU-lease (5ba55d3)

* `llm_host` (`hds-llama`): роль `anon` (приоритет 100 = chat, инстанса НЕТ);
  `dispatch::plan_anon_enter` — `index.pause` + вытеснение всех незанятых
  (idle-chat тоже); занятые защищены; маршруты `POST /internal/anon/enter|exit`
  (enter: 200 `{granted, gpu_mem_limit_mb}`, 503 `gpu_busy`, 501 если backend
  не умеет; exit идемпотентен); `gpu_mem_limit_mb` = free_after − `gpu.reserve_mb`
  (0 — замера нет); на время lease `prepare_demand` отказывает новым ролям;
  тесты `tests/anon_lease.rs`, `tests/facade_http.rs::anon_lease_routes_codes_and_body`.
* `hds-anonymize/src/lease.rs` — loopback-клиент (`hds_core::http::request_url`),
  `LeaseGuard` (Drop → exit), 503 → `GpuBusy`, недоступность → `Unavailable`
  (fallback CPU); тесты на фейк-TCP-сервере.
* CLI: `anonymize.gpu: auto` → lease → spawn `--device cuda --gpu-mem-limit-mb N`
  → акт → shutdown → exit; `off` — CPU; недоступен llm_host → `[warn]` + CPU fallback;
  `gpu_busy` → акт отклонён.
* `gliner_engine._load_onnx`: `--device cpu|cuda` (cuda без CUDA EP → warning +
  CPU), `gpu_mem_limit` в provider_options CUDA EP.

### P4 — CLI `hds anon` (dafe04a)

* `hds-cli/src/cmd/anonymize.rs`: `anonymize|values|deanon|deanon-values|inspect|
  warmup|check`; папку перечисляет Rust; PII в вывод НЕ печатаются (токены/счётчики);
  `check` — python/веса/БД; ошибки пофайлово `[!!] … (kind)`.

### Отступления от плана, найденные при реализации (это не «откаты»)

* `_atomic_write_bytes` создаёт родительский каталог выхода (`data/anonymized`
  при первом акте).
* `FileNotFoundError` → `model_missing` **только** если речь о весах/моделях
  (нет файла/каталога данных — без kind).
* `inspect` отсутствующего файла — ошибка, а не `supported=true`.
* `hint` RPC-ошибки не дублируется в тексте (поле `hint` всегда заполнено).

## 3. Решения заказчика (не пересматривать без него)

| # | Решение | Следствие |
|---|---|---|
| 1 | Сессий нет — единичные акты | SQLite-сессии/TTL не перенесены; только вход/выход |
| 2 | БД токенов — в sidecar, единый модуль | `mapping_store.py` (WAL); Rust к БД не обращается |
| 3 | Токены: per-type счётчики + дедуп по (значение, тип) | golden сравнивает СУЩНОСТИ, не текст с токенами |
| 4 | Маркер `anonymizer_proxy/parser:3` + `inspect` | повторный anon — warning + обработка (TXT/MD/XML — по плейсхолдерам) |
| 5 | «Потерянные гарантии» частичных сценариев | констатация, не чиним |
| 6 | Только вход/выход | имена `<stem>.anonymized.<ext>` / `<stem>.deanon.<ext>` |
| 7 | Модели NER не меняем | порог 0.3; копии без правок алгоритмов |
| 8 | Форматы: DOCX/XLSX/TXT/MD + XML (P1b сделан) | XML/MS Project |
| 9 | Папку перечисляет Rust (верхний уровень, форматы из hello) | сделано в P4 |
| 10 | GPU — lease у `llm_host` (P3 сделан) | `anonymize.gpu: auto\|off` |

Запрещено: перехват команд агентов, OpenAI-прокси/API, дублировать функции sidecar
в Rust, переписывать боевые `PLAN_*.md`, трогать чужие правки, печатать PII/секреты
(в логи/ответы — только токены и счётчики).

## 4. Что осталось (фазы P5–P7)

* **P5 — UI-карточка** `crates/hds-ui/src/anonymize.rs` (+ страница в `page.rs`):
  по образцу `transcribe.rs`; вход — текстовое поле пути + серверный обзор
  **`/api/anonymize/ls`** (верхний уровень, фильтр по `hello.formats`); 4 кнопки
  (анон / деанон / частичная анонимизация списком значений / частичная
  деанонимизация токенами); выходные файлы в `anonymize.output_dir`; отчёт по
  файлам (токены, matched/unmatched, warnings); прогресс пофайловый; PII в UI/лог
  не выводить. Server-профиль: POST-эндпоинты аудируются автоматически (`admin_op`).
* **P6 — установщики**: `installers/build_anonymize_sidecar.ps1` (venv-anonymize из
  `sidecar/python`, профили cpu/gpu, по образцу `build_transcribe_sidecar.ps1`,
  ASCII-only; `requirements.lock` уже есть в `sidecar/hds_anonymize/`);
  `installers/fetch_anonymize_models.ps1` (GLiNER PII ONNX ~1 ГБ →
  `<models>/anonymize/hf-home`, `navec_news_v1_1B_250K_300d_100q.tar`,
  `slovnet_ner_news_v1.tar`, манифест с sha256 — по образцу
  `fetch_transcribe_models.ps1`); `build_sidecar.ps1` += `hds_anonymize`;
  `hds check` += пункты venv/моделей/БД (+ lease, только если хост доступен):
  логика — `hds_index::diag::run_checks` (общий код с `GET /api/diagnostics`),
  форматирование — `crates/hds-cli/src/cmd/check.rs`; `hds-index` уже зависит от
  `hds-transcribe` — по той же схеме добавить `hds-anonymize` (для проверки
  `discover_python`/весов); быстрая заготовка тех же проверок уже есть в
  `hds anon check`.
* **P7 — приёмка + документация**: раздел «Анонимизация» в `README.md`,
  `sidecar/README.md` (состав воркера), `STATUS.md`, новый
  `RELEASE_NOTES_vX.Y.Z.md` (по образцу `RELEASE_NOTES_v0.22.0.md`; факт
  переезда из anonymizer_proxy; `MIGRATION_PLAN_RUST.md` не править);
  обновить этот handoff; живой сценарий:
  UI → файлы → отчёт; **golden-паритет NER** (сущности/тип/спан против
  `anonymizer_proxy`, порог 0.3) — ждёт весов из P6; полный `anonymize` по NER
  и живой GPU-спайк (lease + RTX 3060, замер VRAM, источник CUDA-DLL) — тоже
  после P6.
* Мелочи из §7 плана: имена выходов подтверждены (`a.anonymized.docx` →
  `a.deanon.docx`).

## 5. Как проверять (точные команды)

```powershell
# Python-тесты sidecar (портативный CPython; НЕ системный python!)
& 'C:\Users\Sasha\hermes-disk-search\sidecar\python\cpython-3.12.14-windows-x86_64-none\python.exe' `
  -m unittest discover -s sidecar/tests -t sidecar -p 'test_anonymize_*.py'
# ВАЖНО: нужен -t sidecar (импорты hds_*). В ПОЛНОМ наборе 4 предсуществующих
# import-error (test_decode/diar_blocks/llm_patch/postprocess — нужен numpy/torch,
# это venv-transcribe) — не наши, не «чинить» через смену окружения.

# Rust
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace        # ~6-8 мин; журнал в tools/parity/out (гитигнор)

# Живой smoke (веса не нужны)
cargo run -q -p hds-cli -- anon check
cargo run -q -p hds-cli -- anon values <файл> 'Значение' --out-dir <каталог>
cargo run -q -p hds-cli -- anon inspect <файл>
cargo run -q -p hds-cli -- anon deanon-values <файл> '[PERSON_1]' --out-dir <каталог>
```

## 6. Окружение и грабли (проверено на предыдущем чате)

* Windows, PowerShell 5.1; кириллица в консоли искажается — **читать/писать файлы
  инструментом (editor), НЕ `Get-Content|Set-Content`**: перезапись PS 5.1 портит
  кириллицу двойным энкодингом (однажды испортили `tests/facade_http.rs` и
  восстанавливали из git).
* `git` — всегда `--no-pager`; чужое `M sidecar/hds_transcribe/glossary.example.yaml`
  не трогать; коммиты только своих файлов; вслед за изменением крейтов не забыть
  `Cargo.lock`.
* `HDS_CONFIG`/`HDS_ROOT` переживают между командами — очищать
  (`$env:HDS_CONFIG=$null; $env:HDS_ROOT=$null`).
* Тесты `unittest discover` требуют `-t sidecar`; воркер-тесты поднимают
  подпроцессы (sys.executable).
* Резидент `llm_host` держит `target\release\*.exe`; службы —
  `target\debug\hds.exe`; `cargo test` в debug обычно проходит без остановки
  (линк тест-бинарей), но при «отказано в доступе» — освободить бинарь.
* Портативный Python: `sidecar\python\cpython-3.12.14-windows-x86_64-none\python.exe`
  (есть docx/openpyxl/lxml/sqlite3; НЕТ pydantic/numpy/torch/gliner).
* Веса NER не установлены (`models\anonymize` пуст): полный `anonymize` и warmup
  честно отвечают `kind: model_missing` — это ожидаемо до P6.

## 7. Карта новых файлов

```
sidecar/hds_anonymize/            воркер (P1/P1b/P3-правки)
  __init__.py schemas.py settings.py mapping_store.py handlers.py worker.py
  gliner_engine.py natasha_engine.py ner_service.py replacer.py file_parser.py
  requirements.lock
sidecar/tests/test_anonymize_{store,roundtrip,worker,xml}.py
sidecar/tests/fixtures/anonymize/msproject.xml
crates/hds-anonymize/             Rust-клиент (P2/P3)
  src/{lib,client,protocol,error,settings,folder,lease}.rs
  tests/{client,settings}.rs tests/mock_worker.py
crates/hds-cli/src/cmd/anonymize.rs      P4
crates/hds-llama: src/{dispatch,host,facade,config}.rs — lease 'anon' (P3)
  tests/anon_lease.rs; tests/facade_http.rs (+anon-тест)
crates/hds-extract/src/{worker,protocol}.rs — пропуск нотификаций (P2)
```

## 8. Коммиты ветки `anonymizer-sidecar`

```
60755a8 docs(anonymize): handoff-промпт обновлён под срез P1-P4
5ba55d3 feat(anonymize): P3 — GPU-lease 'anon' у llm_host + --gpu-mem-limit-mb в ORT
dafe04a feat(anonymize): P4 — CLI hds anon + доводка kind-ошибок
23943f1 feat(anonymize): P1b — XML (MS Project) + P2 — Rust-клиент hds-anonymize
c4f4bc5 docs(anonymize): план P1-P7 и handoff-промпт переноса anonymizer_proxy в sidecar
6e4038e feat(anonymize): P1 — Python-sidecar анонимизации (перенос из anonymizer_proxy)
```

## 9. Промт для нового чата (скопировать отсюда до конца файла целиком)

Ты — Cline в режиме Act. Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка
`anonymizer-sidecar`, HEAD `60755a8` (последний код-коммит — `5ba55d3`).
Ядро — Rust (`crates/`), Python — только
sidecar. План: `PLAN_ANONYMIZER_SIDECAR.md` (tracked) — прочитай целиком перед
работой; также `ANONYMIZER_SIDECAR_HANDOFF_PROMPT.md` §§0–8 (карта состояния) и
файлы, перечисленные там в §1. В дереве есть чужое изменение
`sidecar/hds_transcribe/glossary.example.yaml` — не трогать.

Сделано и закоммичено (не переделывать): **P1** — sidecar
`sidecar/hds_anonymize/` (worker.py/handlers.py/mapping_store.py/settings.py/
schemas.py + копии движков из `C:\Test\anonymizer_proxy` без правок алгоритмов,
offline-first); **P1b** — XML/MS Project (позиционная сборка `_assemble_xml`);
**P2** — крейт `crates/hds-anonymize` (client/protocol/error/settings/folder) +
`Worker::request` пропускает progress-нотификации без id; **P3** — GPU-lease:
роль `anon` в `llm_host` (`dispatch::plan_anon_enter`, `POST /internal/anon/
enter|exit`, 503 gpu_busy / 501 not_implemented), `hds-anonymize/src/lease.rs`,
`--device cuda --gpu-mem-limit-mb` в ORT; **P4** — CLI `hds anon
{anonymize,values,deanon,deanon-values,inspect,warmup,check}`. RPC воркера:
hello/warmup/inspect/anonymize/anonymize_values/deanonymize/deanonymize_values/
shutdown; kind-ошибки: model_missing/file_busy/unsupported/db_missing/out_of_dir/
gpu_busy. Имена выходов: `<stem>.anonymized.<ext>` / `<stem>.deanon.<ext>`
(суффикс `.anonymized` у входа отбрасывается). Веса NER не установлены —
полный `anonymize` честно отвечает `kind: model_missing` (это норма до P6).

Задача этого чата — этап **P5 — UI-карточка «Анонимизация» в `hds ui`**, затем,
если останется ресурс, **P6 — установщики**:

1. P5 (`crates/hds-ui/src/anonymize.rs` + страница по образцу `transcribe.rs`
   и `page.rs`):
   - маршруты — в `crates/hds-ui/src/lib.rs` (рядом с `/api/transcribe/*`,
     аудит `admin_op`), модуль карточки — новый `anonymize.rs`;
   - серверный обзор **`/api/anonymize/ls`**: путь из запроса → файлы верхнего
     уровня, отфильтрованные по форматам из `hello.formats` воркера
     (`hds_anonymize::list_files`, `hds_anonymize::discover_python`,
     `Client::hello`);
   - карточка: текстовое поле пути + список найденных файлов; 4 кнопки:
     «Скрыть» (RPC anonymize), «Показать» (deanonymize), «Скрыть указанное»
     (values списком), «Показать указанное» (tokens списком); выходной каталог —
     `anonymize.output_dir` (§2.6), флаг `--out-dir` не обязателен;
   - прогон: spawn клиента (`ClientConfig::from_settings`; при
     `anonymize.gpu: auto` — lease из `hds_anonymize::lease`, как в CLI
     `hds-cli/src/cmd/anonymize.rs`), пофайлово RPC, отчёт по файлам: out_path,
     токены (token/type/count), matched/unmatched, warnings; PII-ЗНАЧЕНИЯ в
     UI/лог не выводить (inspect/values — только токены);
   - прогресс пофайловый (как в карточке транскрибации); server-профиль:
     POST-эндпоинты аудируются автоматически (`admin_op`).
2. P6 (если P5 готова и проверена):
   - `installers/build_anonymize_sidecar.ps1` — venv-anonymize из
     `sidecar/python` (портативный CPython 3.12.14), профили cpu/gpu,
     `requirements.lock` из `sidecar/hds_anonymize/`, ASCII-only, идемпотентно
     по маркеру — по образцу `build_transcribe_sidecar.ps1` + `sidecar_common.ps1`;
   - `installers/fetch_anonymize_models.ps1` — GLiNER PII ONNX (модель
     `knowledgator/gliner-pii-large-v1.0`) в `<models>/anonymize/hf-home`
     (структура HF-кэша), `navec_news_v1_1B_250K_300d_100q.tar`,
     `slovnet_ner_news_v1.tar` рядом; манифест `<models>/anonymize-models.json`
     с ревизиями и sha256 — по образцу `fetch_transcribe_models.ps1`;
     загрузчик НЕ должен требовать HF-токен, если модель публична;
   - `build_sidecar.ps1` += копирование `hds_anonymize` в поставку;
   - `hds check` += пункты venv-anonymize, веса (`models\anonymize\...`), БД
     `data/anonymize.db`, lease (`/internal/anon/enter` доступен) — логика в
     `hds_index::diag::run_checks` (общий код с `GET /api/diagnostics`), вывод —
     `crates/hds-cli/src/cmd/check.rs`; зависимость `hds-anonymize` в
     `hds-index` добавить по образцу `hds-transcribe`; заготовка — `hds anon check`.
3. После P6 (если получится и есть GPU): golden-паритет NER (сущности/тип/спан
   против `anonymizer_proxy` на тех же файлах, порог 0.3) и живой GPU-спайк
   (lease → chat выгружен/дождался → NER на GPU → exit → роли вернулись);
   иначе — зафиксировать в отчёте, что это следующий шаг.

Решения (не пересматривать): сессий нет; БД в sidecar (Rust к ней не обращается);
токены per-type + дедуп по (значение, тип); маркер `anonymizer_proxy/parser:3` +
`inspect`; только вход/выход; модели NER не меняем; папку перечисляет Rust;
UI — карточка; GPU — lease у `llm_host`. Форматы: docx/xlsx/txt/md/xml.

Запрещено: перехват команд агентов, OpenAI-прокси/API, дублировать функции
sidecar в Rust, переписывать боевые `PLAN_*.md`, трогать чужие правки
(`sidecar/hds_transcribe/glossary.example.yaml`), печатать PII/секреты.

Окружение: Windows, PowerShell 5.1 (`.ps1` — ASCII-only; `Invoke-WebRequest
-UseBasicParsing`); кириллицу в файлах читать/писать ТОЛЬКО инструментом
редактора — перезапись через PowerShell портит кириллицу; `git` — всегда
`--no-pager`; резидент `llm_host` держит `target\release\*.exe`, службы —
`target\debug\hds.exe`; артефакты — `tools/parity/out` (гитигнор);
`HDS_CONFIG`/`HDS_ROOT` очищать между командами. Python для sidecar-тестов —
портативный `sidecar\python\cpython-3.12.14-windows-x86_64-none\python.exe`;
unittest строго с `-t sidecar`.

Проверки перед отчётом: sidecar-тесты `python -m unittest discover -s
sidecar/tests -t sidecar -p 'test_anonymize_*.py'` (28 тестов); `cargo fmt
--check`; `cargo clippy --workspace --all-targets -- -D warnings`;
`cargo test --workspace` (не ломать; 110 ok на срезе); живой smoke UI:
карточка → обзор папки → акт на TXT → отчёт с токенами без PII.

Начни с: (1) прочитать план и handoff §§0–8; (2) `git status` и `git --no-pager
log --oneline -6`; (3) краткий план P5 (файлы/эндпоинты/страницы) и сразу
реализация; (4) тесты и smoke; (5) отчёт: что сделано, чем проверено, что
осталось. Вопросы — только если решение противоречит плану.
