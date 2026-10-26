# ANONYMIZER_SIDECAR_HANDOFF_PROMPT.md — контекст и промт для нового чата

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать **§9 целиком**
> (он самодостаточен) и отправить первым сообщением. Этот документ — «карта» состояния:
> его можно читать целиком, но промт в §9 уже содержит главное.
>
> **Дата среза:** 26.10.2026. **Ветка:** `python-sidecar-transcribe`, HEAD `c832da5`,
> тег `v0.22.0`. **Код ещё не менялся.** План `PLAN_ANONYMIZER_SIDECAR.md` —
> **untracked** (`??` в git status), ~300 строк. Другое изменение в дереве —
> `M sidecar/hds_transcribe/glossary.example.yaml` — **чужое**, не трогать.

---

## 0. Что происходит

Функции полной/частичной анонимизации и деанонимизации документов из внешнего проекта
`C:\Test\anonymizer_proxy` (отменён как отдельный продукт) переносятся **в этот проект**
по проверенному паттерну «Python-sidecar + Rust-клиент»:

* **Python-sidecar** `sidecar/hds_anonymize/` — NER-движки и Office round-trip
  (копия из `anonymizer_proxy` без изменения алгоритмов), БД токенов;
* **Rust-клиент** `crates/hds-anonymize` — запуск воркера, протокол, lease VRAM;
* **CLI** `hds anon`; **UI** — карточка «Анонимизация» в `hds ui`.

**Не делаем** (решение заказчика): перехват команд агентов, OpenAI-прокси, OpenAI API.
Оркестратор пока не делаем — протокол готовит к нему (`inspect`), состояние потом будет
храниться на уровне оркестратора.

## 1. Обязательное чтение (в этом порядке)

1. `PLAN_ANONYMIZER_SIDECAR.md` — **весь**, особенно §1 (решения), §2.1 (оркестрация),
   §2.2 (GPU/lease), §3 (протокол), §4 (фазы), §5 (приёмка), §7 (решения/вопросы).
2. `sidecar/README.md` — контракт sidecar: stdio/NDJSON/JSON-RPC 2.0, `PROTOCOL 1`,
   изоляция fd1, UTF-8 stdin, idle-timeout.
3. `crates/hds-extract/src/{lib.rs,protocol.rs,worker.rs}` — `Worker`, `WorkerConfig`,
   `discover_python_with` — транспорт, который **переиспользуем**, а не копируем.
4. `crates/hds-transcribe/src/client.rs` — образец переиспользования `hds_extract::Worker`
   в другом воркере (то же сделаем в `hds-anonymize`).
5. `crates/hds-ui/src/transcribe.rs` + `page.rs` — образец задачи-карточки и страниц.
6. `installers/build_transcribe_sidecar.ps1`, `installers/sidecar_common.ps1`,
   `installers/build_sidecar.ps1` — образцы venv/профилей и упаковки sidecar.
7. Источник переноса: `C:\Test\anonymizer_proxy\anonymizer_proxy\` —
   `anonymizer/{gliner_engine,natasha_engine,ner_service,replacer,file_parser,mapping_store}.py`;
   `proxy/handlers.py` фрагменты 3360–3532, 2871–2919, 3095–3194, 90–140;
   `proxy/utils.py` строки 113, 202.

## 2. Что уже есть в проекте (не дублировать)

| Компонент | Что есть | Как использовать |
|---|---|---|
| `sidecar/hds_sidecar/` | общий пакет: `stdio` (fd1→stderr, UTF-8 stdin, NDJSON, `reply/ok/err`), `runtime` (reader-thread, idle-timeout, EOF), `progress`, `fs` | импортировать в воркере; `stdio.init()` — **до** тяжёлых импортов |
| `sidecar/hds_extract/worker.py` | образец воркера (`hello/extract/normalize/shutdown`) | структура цикла, ошибки, capabilities |
| `sidecar/hds_transcribe/` | тяжёлый воркер + свой venv (профили cuda/cpu/dml), README | образец venv/профилей/загрузки моделей |
| `sidecar/python/` | портативный CPython **3.12.14** (python-build-standalone) | база для `venv-anonymize` |
| `crates/hds-extract` | `Worker`, `WorkerConfig`, `discover_python_with`, `protocol` | **переиспользовать транспорт** |
| `crates/hds-transcribe/src/client.rs` | `use hds_extract::{Worker, WorkerConfig}` | образец клиента |
| `crates/hds-ui` | карточки задач (`transcribe.rs`), страницы (`page.rs`), конфиг, аудит `admin_op` в server-профиле | образец UI |
| `config.yaml` / `config.example.yaml` | секции `index`, `auto_transcribe`, `llm`, `gpu` | добавить секцию `anonymize` |
| `hds check` (`crates/hds-index/src/diag.rs`) | диагностика | добавить пункты venv/модели/БД/lease |
| `installers/` | `build_sidecar.ps1`, `build_transcribe_sidecar.ps1`, `sidecar_common.ps1`, `fetch_*` | образцы установщиков и манифестов |
| `crates/hds-llama` | `llm_host`, арбитр VRAM (chat:100 / embedding:40 / rerank:30 / whisper:20), `gate.rs` | расширение ролью `anon` (P3) |

## 3. Источник переноса — факты, сверенные с кодом

- **`gliner_engine.py`**: модель `knowledgator/gliner-pii-large-v1.0` (ONNX
  `onnx/model.onnx`); порог **0.3**; `max_input_chars 1500`; `chunk_overlap 200`;
  `timeout 300`; автопровайдеры CUDA → DirectML → CPU; torch-фоллбек;
  `_register_nvidia_pip_dlls`; загрузка `GLiNER.from_pretrained(..., local_files_only=True)`
  и при неудаче — сеть; чанкование с перекрытием.
- **`natasha_engine.py`**: Slovnet NER + navec + `NamesExtractor` (yargy + pymorphy2);
  `_ensure_downloaded` качает tar с Yandex Storage при отсутствии (в новом воркере —
  **offline-first**, без сети).
- **`ner_service.py`**: regex-слой (PASSPORT/PHONE/PERSON/EMAIL/WEB/INN/MONEY),
  GLiNER, Natasha, слияние сущностей, расширение всех вхождений, quoted cores;
  `NERUnavailableError` (отличает «NER упал» от «PII не найдено»).
- **`replacer.py`**: токены `[TYPE_N]`, `TextReplacer.anonymize` / `deanonymize` /
  `replace_by_value`, `split_entities_by_segments` (**строка 318**).
- **`file_parser.py`**: `SUPPORTED_EXTENSIONS = {".docx", ".xlsx", ".xml", ".txt", ".md"}`
  (**677**); `PARSER_VERSION = "3"` (**669**);
  `ANON_MARKER_PREFIX = "anonymizer_proxy/parser:"` (**670**); маркер пишется в
  `doc.core_properties.comments` DOCX (**1294**) и `wb.properties.keywords` XLSX
  (**1374**); `assemble(strip_hf_images=True, scrub_metadata=True)` (**1119–1126**);
  `_parse_xml` + автоопределение `xml_type = "msproject"` (**1024–1083**);
  рендер чисел «как в Excel» (`_render_numeric_cell`, **130**).
- **`mapping_store.py`**: `add_mapping` дедуп по **значению** (**241–244**),
  `_next_counter` **глобальный** (**202–211**), INSERT с ретраем при коллизии
  (**252–270**); сессии/TTL — **не переносим**.
- **`proxy/handlers.py`**: `handle_anonymize_file` (**3360–3532**) — ядро;
  `handle_deanonymize_file` (**2871–2919**); `_extra_anonymize_one_file`
  (**3095–3194**); `_atomic_write_bytes` / `_atomic_write_text` (**90 / 117**).
- **`proxy/utils.py`**: `PLACEHOLDER_TOKEN_RE = re.compile(r"\[[A-Z_]+_\d+\]")` (**113**);
  `_result_path_for` (**202**) — не нужен (комплекса файлов больше нет).
- **Тесты источника**: `tests/test_anonymize_file.py` (ждёт `.anonymized.docx` рядом
  с исходником, `[PERSON_1]`, review-md, маркер), `test_deanonymize_file.py`,
  `test_extra_commands.py`, `test_partial_deanon.py` — брать как **сценарии**, но
  пути/имена адаптировать (у нас `out_dir`, имена из §5).

## 4. Решения заказчика (не пересматривать без него)

| # | Решение | Следствие |
|---|---|---|
| 1 | Сессий нет — единичные акты | SQLite-сессии/TTL не переносим; только входной и выходной файлы |
| 2 | БД токенов — в sidecar, единый модуль | `mapping_store.py` (WAL, `data/anonymize.db`); Rust к БД не обращается |
| 3 | Схема токенов: **per-type счётчики + дедуп по (значение, тип)** | токены отличаются от `anonymizer_proxy` → golden сравнивает сущности |
| 4 | Маркер — как в оригинале + RPC `inspect` | повторный `anonymize` по маркеру/плейсхолдерам → **warning + обработка** |
| 5 | «Потерянные гарантии» частичных сценариев | констатация, не чиним |
| 6 | Только вход/выход; комплекс [оригинал, анон, результат] уходит | имена: `<stem>.anonymized.<ext>` / `<stem>.deanon.<ext>` |
| 7 | Модели NER не меняем | GLiNER PII + Natasha/Slovnet + pymorphy2 + regex; копия без правок алгоритмов; порог 0.3 |
| 8 | Форматы P1: DOCX + XLSX + TXT/MD; XML — P1b | XML-слой уже в копии `file_parser.py`; в P1b — fixture/тесты |
| 9 | Папка в UI: только верхний уровень | Rust перечисляет файлы (форматы из `hello.formats`) |
| 10 | UI: карточка в `hds ui`; вход — поле пути + `GET /api/anonymize/ls` | браузер не отдаёт абсолютные пути |
| 11 | GLiNER на GPU, приоритет как у chat | арбитр управляет только ролями движка → роль `anon` (100) + lease API (P3); fallback CPU |

## 5. Контракт sidecar (кратко; полностью — §3 плана)

| Метод | Запрос | Ответ |
|---|---|---|
| `hello` | — | `{protocol, pid, backend, providers[], model, formats[], capabilities}` |
| `warmup` | `{gpu_mem_limit_mb?}` | `{backend, providers[], load_ms}` |
| `inspect` | `{in_path}` | `{supported, marker_version, placeholders[], resolved[]}` — **без значений** |
| `anonymize` | `{in_path, out_dir}` | `{out_path, tokens[], warnings[], elapsed_ms}` |
| `anonymize_values` | `{in_path, out_dir, values[]}` | `{out_path, matched, unmatched, tokens, warnings[]}` |
| `deanonymize` | `{in_path, out_dir}` | `{out_path, restored, unmatched[], warnings[]}` |
| `deanonymize_values` | `{in_path, out_dir, tokens[]}` | `{out_path, restored[], unmatched[], warnings[]}` |
| `shutdown` | — | `{ok}` |

Правила: `in_path` — один файл; `out_dir` — только запись туда; атомарная запись
(temp + replace); ошибки `{code, message, hint, kind}` — `file_busy`, `unsupported`,
`db_missing`, `model_missing`, `out_of_dir`, `gpu_busy`, `internal`; progress-нотификации
без `id` (клиент пропускает); `config.yaml` воркер **не читает**.

## 6. Фазы и приёмка (полностью — §4–§5 плана)

| Фаза | Содержание |
|---|---|
| **P1** | sidecar: копии движков/парсера + `settings.py` + `mapping_store.py` + `worker.py` + оркестрация; fixtures + golden-сущности; тесты |
| **P1b** | XML (MS Project): fixture `.xml`, round-trip (текст + структура тегов) |
| **P2** | Rust-клиент `crates/hds-anonymize`: `client.rs` поверх `hds_extract::Worker` (+ пропуск progress-нотификаций), `settings.rs`, mock/live-тесты, пофайловый обход папки |
| **P3** | спайк + lease в `llm_host`: роль `anon` (100), `/internal/anon/enter|exit`, `gpu_mem_limit` в ORT, завершение sidecar после акта, fallback CPU |
| **P4** | CLI `hds anon` (`anonymize/values/deanon/deanon-values/inspect/warmup/check`) |
| **P5** | UI-карточка `anonymize.rs` + `page.rs` + `/api/anonymize/{run,status,ls}`, audit `admin_op`, пофайловый прогресс |
| **P6** | установщики: `build_anonymize_sidecar.ps1`, `fetch_anonymize_models.ps1` (+ манифест sha256), `build_sidecar.ps1`, `hds check`, `install_windows.ps1` |
| **P7** | приёмка + документация: README, `sidecar/README.md`, STATUS.md, RELEASE_NOTES, handoff |

Приёмка: `cargo test --workspace`, fmt, clippy `-D warnings`; sidecar-тесты P1/P1b;
golden-сущности (порог 0.3); round-trip по форматам (TXT/MD — байт-в-байт;
DOCX/XLSX — текст по сегментам + структура + маркер; XML — текст + теги);
GPU-сценарий P3; живой UI-сценарий; `hds check` — новые пункты.

## 7. Правила и грабли окружения (платил временем)

- `git` — всегда `--no-pager`; коммиты атомарные (`feat(anon): …`, `docs(anon): …`).
- PowerShell 5.1: `*.ps1` — **ASCII-only** (BOM-less читается как ANSI; `${var}`, не `$var:`);
  `Invoke-WebRequest` — с `-UseBasicParsing`; кириллица в консоли искажается — читать
  файлы инструментом; первый вывод иногда «клеит» символ — писать в файл
  (`Out-File -Encoding utf8`) и читать.
- `HDS_CONFIG`/`HDS_ROOT` переживают между командами — задавать и **очищать**.
- Резидент `llm_host` держит `target\release\*.exe`, живые службы — `target\debug\hds.exe`:
  перед полной пересборкой/`cargo test` остановить и **вернуть** (dev-машина позволяет),
  либо `--exclude hds-cli`.
- Боевой `index.pause` не снимать; плейн-токены/секреты/PII не печатать.
- Артефакты — `tools/parity/out/` (гитигнор). Чужие незакоммиченные правки не трогать
  (сейчас это `sidecar/hds_transcribe/glossary.example.yaml`); боевые `PLAN_*.md` не
  переписывать.
- Sidecar: воркер **не читает** `config.yaml` (всё из аргументов/RPC), **не ходит в сеть**
  (offline-first), stdout — только JSON-RPC (транспорт `hds_sidecar`), логи — stderr.
  `hds_sidecar` не импортирует тяжёлые библиотеки на уровне модуля (граница пакета).
- `onnxruntime` и `onnxruntime-gpu` — взаимоисключающие venv (как профили transcribe).
- Python-тесты sidecar — `unittest`: `python -m unittest discover -s sidecar/tests -v`.
- Git Bash: `C:\Program Files\Git\bin\bash.exe`; WSL нет.

## 8. Стартовое состояние (P0)

- **Есть:** план `PLAN_ANONYMIZER_SIDECAR.md` (untracked); `sidecar/hds_sidecar` +
  `hds_extract` + `hds_transcribe`; `crates/hds-extract` (Worker); клиент transcribe как
  образец; UI-конвенции; установщики sidecar.
- **Нет (создаётся в фазах):** `sidecar/hds_anonymize/`, `crates/hds-anonymize`,
  `venv-anonymize`, модели `<models>/anonymize/`, секция `anonymize` в `config.yaml`,
  карточка UI, скрипты установки, пункты `hds check`.
- **Кода по анонимизации в этом проекте ещё нет.**

---

## 9. Промт для нового чата (копировать целиком)

Ты — Cline в режиме Act. Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка
`python-sidecar-transcribe`, HEAD `c832da5` (тег v0.22.0). Ядро — Rust (`crates/`),
Python — только sidecar. План: `PLAN_ANONYMIZER_SIDECAR.md` (untracked) — прочитай
целиком перед работой; также `sidecar/README.md`, `crates/hds-extract/src/worker.rs`,
`crates/hds-transcribe/src/client.rs`, `crates/hds-ui/src/transcribe.rs`,
`installers/build_transcribe_sidecar.ps1`. В дереве есть чужое изменение
`sidecar/hds_transcribe/glossary.example.yaml` — не трогать.

Задача этапа **P1 — Python-sidecar анонимизации**:

1. Создать `sidecar/hds_anonymize/`:
   - скопировать из `C:\Test\anonymizer_proxy\anonymizer_proxy\anonymizer\` файлы
     `gliner_engine.py`, `natasha_engine.py`, `ner_service.py`, `replacer.py`,
     `file_parser.py` — **без изменения алгоритмов**; правки только импортов
     (`from ..config import …` → `from .settings import …`) и путей;
   - `settings.py` — замена `config.py`: `NER_ENGINE`/`PII_CATEGORIES` из аргументов
     воркера (`--model knowledgator/gliner-pii-large-v1.0`, `--threshold 0.3`,
     `--max-input-chars 1500`, `--chunk-overlap 200`, `--timeout 300`, `--natasha 1`,
     `--models-dir`, `--db`, `--device cpu|cuda`, `--gpu-mem-limit-mb`);
   - `mapping_store.py` — SQLite WAL (`data/anonymize.db`): `mappings(token PK, value,
     type, ts)` + `counters(type PK, last)`; **per-type счётчики + дедуп по
     (значение, тип)**; `busy_timeout` + `BEGIN IMMEDIATE` при выдаче номера;
   - `worker.py` — JSON-RPC 2.0 по stdio через `hds_sidecar` (PROTOCOL 1): `hello`
     (backend/providers/model/formats), `warmup`, `inspect`, `anonymize`,
     `anonymize_values`, `deanonymize`, `deanonymize_values`, `shutdown` +
     progress-нотификации без `id`; stdout — только протокол, логи — stderr;
   - оркестрация из `handlers.py`: parse → NER → split `\n` →
     `split_entities_by_segments` (`replacer.py:318`) → построчная замена → join →
     `FileAssembler.assemble` (анонимизация: `strip_hf_images=True,
     scrub_metadata=True` + маркер; деанонимизация: `False/False`) → атомарная
     запись (`_atomic_write_*`). **Не переносить** сессии, reuse-копии, чат-команды,
     review-md в хранилище.
2. **Offline-first**: воркер не ходит в сеть; веса — из `--models-dir`
   (`<models>/anonymize/hf-home`, `navec_*.tar`, `slovnet_ner_news_v1.tar`); нет
   весов → ошибка `kind: model_missing` с hint про установщик.
3. Fixtures + тесты (`unittest`, как `sidecar/tests`): DOCX/XLSX/TXT/MD;
   TXT/MD — байт-в-байт после «анон → деан»; DOCX/XLSX — текст по сегментам +
   структура + `ANON_MARKER`; частичные сценарии; сквозная нумерация; FILE_BUSY;
   дедуп; отсутствие PII в логах.
4. Проверки: `python sidecar/hds_anonymize/worker.py` с `hello`;
   `python -m unittest discover -s sidecar/tests -v`; `cargo test --workspace`
   (не ломать), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`.

Решения (не пересматривать): сессий нет; БД в sidecar; схема токенов per-type +
дедуп; маркер `anonymizer_proxy/parser:3` + `inspect`; только вход/выход; модели
не меняем; форматы P1: DOCX/XLSX/TXT/MD (XML — P1b); папку перечисляет Rust (P2);
UI — карточка (P5); GPU — lease у `llm_host` (P3).

Запрещено: перехват команд агентов, OpenAI-прокси/API, дублировать функции
sidecar в Rust, переписывать боевые `PLAN_*.md`, трогать чужие правки, печатать
PII/секреты.

Окружение: Windows, PowerShell 5.1 (`.ps1` — ASCII-only; `Invoke-WebRequest
-UseBasicParsing`; кириллица в консоли искажается — читать файлы инструментом);
`git` — всегда `--no-pager`; резидент `llm_host` держит `target\release\*.exe`,
службы — `target\debug\hds.exe` (перед полным `cargo test` остановить и вернуть);
артефакты — `tools/parity/out` (гитигнор); `HDS_CONFIG`/`HDS_ROOT` переживают между
командами — очищать.

Начни с: (1) прочитать план и перечисленные файлы; (2) `git status`;
(3) краткий план P1 (файлы/шаги) и сразу реализация; (4) тесты; (5) отчёт: что
сделано, чем проверено, что осталось. Вопросы — только если решение противоречит
плану.



