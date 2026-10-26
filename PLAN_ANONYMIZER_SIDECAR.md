# ПЛАН: анонимизация/деанонимизация файлов в этом проекте (Python-sidecar + Rust)

**Статус:** черновик к согласованию (26.10.2026).
**Основание:** решение заказчика — перенести наработки `C:\Test\anonymizer_proxy` сюда.
Закрывает пункт «Решения заказчика» `MIGRATION_PLAN_RUST.md` («anonymizer_proxy — ждём
переезда»): переезд происходит **сюда** (sidecar + Rust), сам `MIGRATION_PLAN_RUST.md`
не правим, факт фиксируем в `RELEASE_NOTES.md`.
**Связанные документы:** `sidecar/README.md` (контракт sidecar), `PLAN_W2_LLM_HOST.md`
§4 (арбитр VRAM/роли), `PLAN_AUTO_TRANSCRIBE.md` §8 (образец задачи-карточки UI),
`PLAN_M9_CLIENT_PACKAGE.md` (образец клиента sidecar).

## 0. Цель

Функции полной/частичной анонимизации и деанонимизации документов (DOCX, XLSX, TXT/MD;
XML/MS Project — фаза P1b) в этом проекте: Python-sidecar `sidecar/hds_anonymize/`
(JSON-RPC по stdio, как `hds_extract`), Rust-клиент `crates/hds-anonymize`, карточка
«Анонимизация» в `hds ui`, CLI `hds anon`. **Не делаем** (решение заказчика): перехват
команд агентов, OpenAI-прокси и OpenAI API. Оркестратор пока не делаем: протокол
готовит к нему (`inspect`), состояние потом будет храниться на уровне оркестратора.

## 1. Зафиксированные решения (не пересматривать без заказчика)

| # | Решение | Следствие |
|---|---|---|
| 1 | Сессий нет — анонимизация/деанонимизация единичные акты | SQLite-сессии/TTL из `anonymizer_proxy` не переносим; в акте только входной и выходной файлы |
| 2 | БД токенов остаётся в sidecar, **единый модуль доступа** | `mapping_store.py` (SQLite WAL, `data/anonymize.db`) — Rust к БД не обращается; нумерация сквозная, схема — **per-type счётчики + дедуп по (значение, тип)** (решение заказчика, §2.1) |
| 3 | Маркер анонимизации — как в `anonymizer_proxy` + RPC `inspect` | повторный NER по анонимизированной копии — warning; `inspect` — функция для будущего оркестратора |
| 4 | «Потерянные гарантии» частичных сценариев | констатация, не чиним |
| 5 | Только входной/выходной файл; комплекс [оригинал, анонимизированный, результат] уходит | имена выходов: `<stem>.anonymized.<ext>` / `<stem>.deanon.<ext>` |
| 6 | Модели NER не меняем | GLiNER PII (ONNX) + Natasha/Slovnet + pymorphy2 + regex-слой; перенос без правок алгоритмов; golden-паритет в приёмке |
| 7 | Форматы P1: DOCX + XLSX + TXT/MD | XML (MS Project) — фаза **P1b** (XML-слой уже в копии `file_parser.py`: `.xml` в `SUPPORTED_EXTENSIONS`, `xml_type = "msproject"`); в P1b — fixture и тесты |
| 8 | Папка в UI: только верхний уровень | как `auto_transcribe.inbox_dir: recursive: false` |
| 9 | UI: карточка «Анонимизация» в существующем `hds ui` | по образцу `crates/hds-ui/src/transcribe.rs` |
| 10 | GLiNER на **GPU**, приоритет как у чата; существующий балансировщик VRAM | проверено: арбитр управляет только ролями движка → расширение llm_host ролью `anon` (приоритет 100) + lease API (§2.2); авто-fallback CPU |

## 2. Архитектура

### 2.1 Sidecar `sidecar/hds_anonymize/` (перенос из `anonymizer_proxy`)

- `worker.py` — JSON-RPC 2.0 (транспорт `hds_sidecar`, PROTOCOL 1): `hello` (backend,
  providers, model, formats), `warmup`, `inspect`, `anonymize`, `anonymize_values`,
  `deanonymize`, `deanonymize_values`, `shutdown`; плюс progress-нотификации без `id`;
  NER-модели грузятся лениво (первый вызов); idle-таймаут 60 с (как `hds_extract`).
- `mapping_store.py` — **единый модуль БД токенов** (SQLite WAL, `busy_timeout`,
  выдача номера в `BEGIN IMMEDIATE`): `mappings(token PK, value, type, ts)` +
  `counters(type PK, last)`. **Решение заказчика**: счётчики **per-type** (`[PERSON_1]`,
  `[ORG_1]`, …) и дедуп по **(значение, тип)** (уник. индекс). Следствие: токены
  отличаются от `anonymizer_proxy` (там глобальный счётчик и дедуп по значению) →
  golden-паритет сравнивает **сущности/спаны**, а не текст с токенами (§5). БД не
  чистим (строка на уникальную пару); удаление БД = потеря деанонимизации старых
  файлов; БД `anonymizer_proxy` не мигрируется (токены сессий могут конфликтовать).
- `gliner_engine.py`, `natasha_engine.py`, `ner_service.py`, `replacer.py`,
  `file_parser.py` — копия из `anonymizer_proxy` **без изменения алгоритмов**
  (правки импортов, env → параметры RPC). `file_parser.py` (~70 КБ) с насаженными
  багфиксами (числовые ячейки, вложенные таблицы) — копия без правок.
- `office_ops.py` — не переносим (CLI-инструменты для агента, без прокси не нужны).
- `settings.py` — локальная замена `config.py` (модули копируются с правкой импортов
  `from ..config import …` → `from .settings import …`): `NER_ENGINE`/`PII_CATEGORIES`
  из аргументов воркера (`--model`, `--threshold`, `--max-input-chars`,
  `--chunk-overlap`, `--timeout`, `--natasha`, `--models-dir`, `--db`, `--device`);
  `config.yaml` воркер не читает.
- **Оркестрация файловых актов** (переносится из `proxy/handlers.py`, адаптируется под
  stateless; `split_entities_by_segments` уже в `replacer.py:318`):
  `handle_anonymize_file` (parse → NER по `parsed.text` → split `\n` →
  `split_entities_by_segments` → построчный `TextReplacer.anonymize` → join →
  `FileAssembler.assemble(strip_hf_images=True, scrub_metadata=True)` → маркер),
  `handle_deanonymize_file` (assemble с `strip_hf_images=False, scrub_metadata=False`),
  `_extra_anonymize_one_file` (детерминированный поиск значений: регистронезависимо,
  границы слова, числовые варианты с пробелами/запятой, маска существующих
  плейсхолдеров `PLACEHOLDER_TOKEN_RE` = `\[[A-Z_]+_\d+\]`), атомарная запись
  (`_atomic_write_*`, `handlers.py:90/117`). **Не переносим**: reuse-копии
  (`_reusable_copy`, проверка маркера для reuse), `store.register_file_session`,
  сохранение review-md в сессию, всё, что завязано на `ChatCompletionRequest`.
- **Модели — offline-first**: воркер не ходит в сеть (в `anonymizer_proxy`
  `GLiNER.from_pretrained` и `natasha._ensure_downloaded` докачивали веса при
  отсутствии). Раскладка `models_dir` (из Rust):
  `<models>/anonymize/hf-home/…` (HF-кэш GLiNER: config.json, tokenizer, `onnx/model.onnx`),
  `<models>/anonymize/navec_news_v1_1B_250K_300d_100q.tar`,
  `<models>/anonymize/slovnet_ner_news_v1.tar`; env воркера `HF_HOME`,
  `HF_HUB_OFFLINE=1`, `TRANSFORMERS_OFFLINE=1`. Нет весов → `kind: model_missing`
  (+hint про `installers/fetch_anonymize_models.ps1`).
- `file_parser.py`: `PARSER_VERSION="3"`, `ANON_MARKER_PREFIX="anonymizer_proxy/parser:"`
  — **не переименовываем** (иначе `inspect` не узнает ранее анонимизированные файлы);
  маркер пишется только в DOCX (`docProps/comments`) и XLSX (`properties/keywords`);
  для TXT/MD/XML маркера нет — `inspect`/warning определяет по плейсхолдерам.
- `requirements.lock` (пин версий): `gliner`, `torch` (gliner импортирует его даже для
  ONNX), `natasha`, `navec`, `pymorphy2` (`setuptools<81`), `python-docx`, `openpyxl`,
  `lxml`, `numpy`, `onnxruntime` (профиль gpu: `onnxruntime-gpu` + CUDA-DLL, спайк P3).

### 2.2 VRAM-балансировщик (проверка решения №10)

Проверено: арбитр `hds-llama/src/dispatch.rs` управляет **ролями движка** (chat:100 /
embedding:40 / rerank:30 / whisper:20 — whisper bridge, не инстанс); посторонние
GPU-процессы он не видит, GLiNER — не роль движка. Для решения №10 нужно малое
расширение `llm_host`:

- новая роль `anon` с приоритетом **100 (= chat)** и lease API
  (`POST /internal/anon/enter|exit`, только loopback): аренда — **shared-держатель**
  (как chat/embedding/rerank в `gate.rs`); вход: пауза индексации (`index.pause`),
  выгрузка ролей <100 (embedding/rerank/whisper), при **busy-сеансе chat** — ожидание
  его окончания с бюджетом (`anonymize.lease_timeout_sec`, дефолт 120 с), затем
  `gpu_busy` (503 + hint); при idle chat — выгрузка (единственный владелец GPU) и
  резерв NVML ≈ 2 ГБ (GLiNER ONNX ≈1 ГБ + активации); `exit` → освобождение, роли
  вернутся по потребности;
- **резерв передаётся в sidecar**: CLI/RPC-параметр `--gpu-mem-limit-mb` →
  `onnxruntime` CUDA EP `gpu_mem_limit` (иначе ORT аллоцирует по своему аренарному
  росту и резерв не соблюсти); в GPU-режиме sidecar **завершается после акта**
  (lease → spawn → act → shutdown → exit), чтобы VRAM освобождался сразу; в CPU-режиме —
  idle-таймаут 60 с;
- клиент lease — Rust `hds-anonymize/src/lease.rs`: аренда берётся **до** spawn/RPC
  и держится, пока идёт NER; sidecar остаётся «глупым»;
- fallback: llm_host недоступен/lease не прошёл → CPU (onnxruntime без CUDA),
  предупреждение в отчёт; `anonymize.gpu: auto|off`;
- спайк P3: GLiNER на RTX 3060 12 ГБ рядом с chat (Q6_K) — замер VRAM, поведение
  арбитра; источник CUDA-DLL для onnxruntime-gpu: `torch/lib` (torch будет в venv)
  через `os.add_dll_directory` до импорта ORT, либо `nvidia-*-cu12` pip-пакеты
  (в `gliner_engine._register_nvidia_pip_dlls` уже есть логика).

### 2.3 Rust-клиент `crates/hds-anonymize/`

Транспорт **переиспользуем** из `hds-extract` (`Worker`, `WorkerConfig`,
`discover_python_with`) — как это делает `hds-transcribe` (`client.rs`), без копии
300-строчного `worker.rs`. Состав крейта: `lib.rs`, `client.rs` (типизированные методы),
`protocol.rs` (типы/константы), `lease.rs` (§2.2), `settings.rs` (разрешение профиля по
наличию venv — как `hds-transcribe/src/settings.rs`), `tests/` (mock-worker + live
`#[ignore]`). Интерпретатор:
`discover_python_with(root, "HDS_ANONYMIZE_PYTHON", Some("venv-anonymize"))`.
В `Worker::request` — одна аддитивная доработка: строки без `id` (progress-нотификации,
§3) пропускать, а не считать «неожиданным id».

### 2.4 CLI `hds anon`

`hds anon anonymize <path>`, `values <path> <values…>`, `deanon <path>`,
`deanon-values <path> <tokens…>`, `inspect <path>`, `warmup` (прогрев моделей, печатает
backend/providers), `check` (venv/модели/БД/lease) — отладка без UI. Папка: Rust
перечисляет файлы верхнего уровня (форматы — из `hello.formats`) и вызывает RPC по
одному файлу (§3), поэтому прогресс и ошибки — пофайлово.

### 2.5 UI-карточка «Анонимизация» (`crates/hds-ui/src/anonymize.rs`)

По образцу `transcribe.rs`: поле входа (файл или папка), выходной каталог
(`anonymize.output_dir` из конфига; правится в карточке и сохраняется). Браузер не
отдаёт абсолютный путь из `<input type=file>`, поэтому: текстовое поле пути +
серверный «обзор» `GET /api/anonymize/ls?path=…` (листинг каталогов; только в
admin-профиле UI). Каталог выхода исключается из индексации **неявно в рантайме**,
как `auto_transcribe.*_dir` (не через `exclude_paths`). Textarea «значения» /
«токены», 4 кнопки: «Скрой все» / «Скрой указанное» / «Покажи все» / «Покажи
указанное»; одна активная пачка (кнопки блокируются), прогресс — пофайлово (поллинг
`/api/anonymize/status`), отчёт (вход → выход, токены, warnings, kind+hint).
Path-safety: выходной путь строит Rust (out_dir + имя), валидация как в
транскрибации (`is_safe_name`/canonicalize); запись — только sidecar. POST-эндпоинты
в server-профиле аудируются автоматически как `admin_op` (существующий механизм
`hds-ui`); PII-значения в лог/аудит не пишем (только счётчики).

### 2.6 Конфиг `config.yaml` (секция `anonymize`)

```yaml
anonymize:
  output_dir: data/anonymized
  db_path: data/anonymize.db
  models_dir: <models>/anonymize
  model: knowledgator/gliner-pii-large-v1.0   # как в anonymizer_proxy
  threshold: 0.3            # как в anonymizer_proxy (НЕ 0.5)
  max_input_chars: 1500     # окно модели; длинный текст — чанки с перекрытием
  chunk_overlap: 200
  timeout_sec: 300          # таймаут NER-вызова
  natasha: true
  max_file_mb: 200          # пропуск с warning (защита от гигантов)
  review_md: false          # .md-предпросмотр в out_dir (опционально)
  gpu: auto                 # auto = lease у llm_host, иначе CPU
  lease_timeout_sec: 120    # ожидание busy-сеанса chat
  request_timeout_sec: 600  # RPC на файл (WorkerConfig)
```

### 2.7 Установщики

- `build_anonymize_sidecar.ps1` — venv-anonymize из портативного CPython, профили
  cpu/gpu (по образцу `build_transcribe_sidecar.ps1`, ASCII-only);
- `fetch_anonymize_models.ps1` — GLiNER PII ONNX (~1 ГБ), navec (~50 МБ), slovnet;
- `build_sidecar.ps1` += `hds_anonymize`; `hds check` += пункты venv/модели/БД/lease;
- `install_windows.ps1` += блок анонимизации (сборка sidecar, модели, cargo build).

## 3. Протокол sidecar (JSON-RPC 2.0, NDJSON, PROTOCOL 1)

| Метод | Запрос | Ответ |
|---|---|---|
| `hello` | — | `{protocol, pid, backend, providers[], model, formats[], capabilities}` |
| `warmup` | `{gpu_mem_limit_mb?}` | `{backend, providers[], load_ms}` |
| `inspect` | `{in_path}` | `{supported, marker_version, placeholders: [{token, type, count}], resolved: [{token, in_db}]}` — **без значений** (PII не отдаём) |
| `anonymize` | `{in_path, out_dir}` | `{out_path, tokens: [{token, type, count}], warnings[], elapsed_ms}` |
| `anonymize_values` | `{in_path, out_dir, values[]}` | `{out_path, matched, unmatched, tokens, warnings[], elapsed_ms}` |
| `deanonymize` | `{in_path, out_dir}` | `{out_path, restored, unmatched[], warnings[], elapsed_ms}` |
| `deanonymize_values` | `{in_path, out_dir, tokens[]}` | `{out_path, restored[], unmatched[], warnings[], elapsed_ms}` |
| `shutdown` | — | `{ok}` |

- `in_path` — **один файл**; папку перечисляет Rust (форматы из `hello.formats`) и
  вызывает RPC по одному файлу — так есть пофайловый прогресс/ошибки и не нужен
  долгий RPC на сотни файлов;
- `out_dir` — выходной каталог; Rust строит выходной путь; sidecar пишет только в
  `out_dir`; запись атомарная (temp + replace), существующий выход перезаписывается;
- имена: `<stem>.anonymized.<ext>` (скрыть), `<stem>.deanon.<ext>` (показать; если
  вход уже `<stem>.anonymized.<ext>` — суффикс `.anonymized` отбрасывается:
  `a.anonymized.docx` → `a.deanon.docx`) — подтвердить в P1 (§7);
- прогресс: воркер шлёт NDJSON-нотификации без `id`
  (`{"jsonrpc":"2.0","method":"progress","params":{"file","phase","done","total"}}`);
  клиент их пропускает (§2.3);
- ошибки `{code, message, hint, kind}`: `file_busy` (Word держит файл), `unsupported`,
  `db_missing`, `model_missing`, `out_of_dir`, `gpu_busy`, `internal`;
- маркер: для DOCX/XLSX — как сейчас (`ANON_MARKER`, префикс не переименовываем);
  для TXT/MD/XML маркера нет → `inspect`/warning по плейсхолдерам;
- уже анонимизированный файл + `anonymize` → **warning и обработка** (решение
  заказчика; «Скрой указанное» на таком файле — штатный сценарий);
- `config.yaml` воркером НЕ читается (§5.2 контракта sidecar) — все параметры из RPC.

## 4. Фазы

- **P1 — sidecar**: перенос движков и оркестрации (§2.1), `settings.py`,
  `mapping_store.py` (per-type счётчики, дедуп по (значение, тип)), `worker.py`;
  fixtures `sidecar/tests/fixtures/anonymize/` (DOCX/XLSX/TXT/MD) и golden-набор
  сущностей (сгенерировать один раз из `anonymizer_proxy` и закоммитить); тесты:
  round-trip по форматам (TXT/MD — байт-в-байт; DOCX/XLSX — текст по сегментам +
  структура + маркер), частичные сценарии, сквозная нумерация, атомарная запись,
  FILE_BUSY, дедуп/счётчик, reject, отсутствие PII в логах.
- **P1b — формат XML (MS Project)**: XML-слой входит в копию `file_parser.py` уже в P1
  (`.xml` в `SUPPORTED_EXTENSIONS`, `xml_type = "msproject"`); в P1b — **приёмка**:
  fixture `.xml` (MS Project), тесты round-trip (текст + структура тегов) и частичных
  сценариев; UI/CLI ограничений не требуют. Выполняется сразу после P1; можно
  параллельно с P2.
- **P2 — Rust-клиент**: `client.rs` поверх `hds_extract::Worker` (+ пропуск
  progress-нотификаций в `Worker::request`), `settings.rs` (профиль по venv),
  mock/live-тесты, пофайловый обход папки (форматы из `hello.formats`) — без lease, CPU.
- **P3 — спайк + lease в llm_host**: роль `anon` (100) + `/internal/anon/enter|exit`,
  GLiNER на GPU рядом с chat (3060 12 ГБ): замер VRAM, поведение арбитра (выгрузка
  embedding/rerank/whisper, ожидание busy-сеанса chat), `gpu_mem_limit` в ORT,
  завершение sidecar после акта, фолбэк CPU.
- **P4 — CLI**: `hds anon` (+lease).
- **P5 — UI-карточка**: `anonymize.rs`, `page.rs`, `/api/anonymize/{run,status,ls}`,
  блокировка второй пачки, пофайловый прогресс, audit `admin_op` (авто в server-профиле),
  тесты.
- **P6 — установщики**: `build_anonymize_sidecar.ps1` (venv-anonymize, профили cpu/gpu;
  по образцу `build_transcribe_sidecar.ps1`), `fetch_anonymize_models.ps1` (HF-кэш
  GLiNER + navec + slovnet, манифест `anonymize-models.json` с sha256),
  `build_sidecar.ps1` += `hds_anonymize`, `hds check` (venv/модели/БД/lease),
  `install_windows.ps1`.
- **P7 — приёмка + документация**: README «Анонимизация», `sidecar/README.md` (состав
  воркера), STATUS.md, RELEASE_NOTES, handoff.md.

## 5. Приёмка

- `cargo test --workspace`, `cargo fmt --check`, `cargo clippy -- -D warnings` зелёные.
- Sidecar-тесты P1 (все перечисленные сценарии) и P1b (XML) зелёные.
- **Golden-паритет NER**: из-за выбранной схемы токенов (per-type + дедуп, §2.1)
  сравниваем **сущности** (значение/тип/спан) на fixtures с эталоном из
  `anonymizer_proxy`, а не текст с токенами; текстовый golden — с нормализацией
  токенов (тип+порядок). Порог 0.3, модели/версии/regex — как в оригинале (реш. №6).
- **Round-trip по форматам**: TXT/MD — байт-в-байт после «анон → деан»; DOCX/XLSX —
  деанонимизированный ТЕКСТ (по сегментам) == исходному, структура (таблицы/стили/
  числа) сохранена, в анонимизированном — `ANON_MARKER`; XML — текст и структура
  тегов идентичны. Байт-в-байт для DOCX/XLSX недостижим (маркер + `scrub_metadata`) —
  это не дефект.
- GPU-сценарий P3: lease активен → chat-сеанс дождался/низкоприоритетные выгружены,
  роли вернулись после `exit`; VRAM освобождён после завершения sidecar; fallback
  CPU — предупреждение в отчёте.
- Живой UI-сценарий: вход (файл + папка через `/api/anonymize/ls`) → 4 кнопки →
  выходной файл(ы) в out_dir, отчёт по файлам; `hds check` — новые пункты зелёные.

## 6. Трудности и риски

| Риск | Митигация |
|---|---|
| Паритет NER (модели/regex) | перенос без правок + golden-сущности; версии пин в `requirements.lock`; порог 0.3 |
| Схема токенов отличается от оригинала | решение заказчика (§2.1); golden сравнивает сущности, не текст |
| `file_parser.py` ~70 КБ с багфиксами | копия почти без правок (только импорты) |
| Балансировщик: GLiNER не роль движка | расширение llm_host ролью `anon` + lease (спайк P3); авто-fallback CPU |
| ORT не освобождает VRAM | GPU-режим: sidecar завершается после акта; `gpu_mem_limit` из lease |
| Два процесса (UI+CLI) на одной БД | WAL + `busy_timeout` + `BEGIN IMMEDIATE` при выдаче номера |
| PII в логах/`inspect` | `inspect` без значений; в логи — только токены/счётчики |
| Старые файлы из `anonymizer_proxy` | БД не мигрируется (конфликт токенов) — старые файлы не деанонимизируются; констатация (реш. №4) |
| Server-профиль: сервисный аккаунт | входные пути должны быть доступны сервису; иначе permission-ошибка с hint |
| venv-anonymize тяжёлый (~2,5–6 ГБ) | как venv-transcribe: cpu/gpu профили, сборка установщиком |
| FILE_BUSY (Word держит файл) | kind: `file_busy` + hint |
| Маппинги между актами — ответственность оркестратора | констатация (реш. №4); `inspect` как подсказка |
| Сквозная нумерация при папке/серии актов | per-type счётчики в БД (единый модуль, реш. №2) |

## 7. Решения и открытые вопросы

**Решено в этом раунде (заказчик, 26.10.2026):**
1. Схема токенов — **per-type счётчики + дедуп по (значение, тип)** (осознанный отход
   от `anonymizer_proxy`: там глобальный счётчик и дедуп по значению) → golden-тесты
   сравнивают сущности, а не текст с токенами (§5).
2. `anonymize` на уже анонимизированном файле — **warning + обработка** (идемпотентно).
3. Выбор входа в UI — **текстовое поле пути + серверный обзор `/api/anonymize/ls`**.

**Осталось (мелочи, решаются в P1/P3):**
- точные имена выходов (`.anonymized` / `.deanon`, отбрасывание `.anonymized` у входа)
  — подтвердить на живом сценарии;
- CUDA-DLL для onnxruntime-gpu в venv-anonymize (спайк P3).

