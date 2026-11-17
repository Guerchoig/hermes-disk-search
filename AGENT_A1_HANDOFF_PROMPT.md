# AGENT_A1_HANDOFF_PROMPT.md — контекст и промт для фазы A1 (база меток v2)

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать **§9 целиком** (он
> самодостаточен) и отправить первым сообщением. Разделы §0–§8 — справочная «карта»: промт
> в §9 ссылается на них, исполнитель читает этот файл целиком.
>
> **Дата среза:** 17.11.2026. **Ветка:** `agent-privacy-zone`, HEAD `3f34d51d` (push не делался).
> **План:** `PLAN_AGENT_PRIVACY_ZONE.md`, редакция 3.1. **Правки плана по итогам A0** ещё НЕ внесены
> владельцем — они лежат в `tools/parity/AGENT_SPIKES.md` §3.11 (П-1…П-14) и **считаются
> действующими**.
> **Отчёт A0:** `tools/parity/AGENT_SPIKES.md` (10 спайков, стоп-точки пройдены, вопросы В-1…В-6
> закрыты).

---

## 0. Что происходит (коротко)

Проект `hermes-disk-search` (**hds**) — локальный поиск и ИИ-инструменты для аналитиков и
менеджеров проектов внедрения 1С (проектные документы, не код). Новая функция по плану: **ИИ-агент
в анонимной зоне** — headless-ядро Cline (`@cline/sdk` 0.2.0) в отдельном процессе `hds_agent`,
который работает только с анонимизированными копиями документов (метки `[PERSON_3]` вместо ФИО),
а всё исходящее проходит фильтр hds.

**A0 закрыт** (см. `tools/parity/AGENT_SPIKES.md`): изоляция SDK, `fetch`, хуки, политики
инструментов, суб-агенты, формат истории, MCP, скорость анонимизации, морфология, стенд и оценка
поиска по анонимным образам.

**Фаза A1 (эта карта) — база меток v2.** Это первая фаза, где **меняется продуктовый код**
(`sidecar/`, `crates/`). Задачи:

1. `sidecar/hds_anonymize/vault.py` вместо `mapping_store.py` — схема v2: **одна база, два уровня
   меток** (D3): уровень 0 — сущности (исходные значения живут только здесь), уровень 1 — метки
   проекта, связанные с сущностями. Миграции с v1 нет (D4), старый файл переименовывается.
2. Морфология (TR-2.1…TR-2.4): `norm_key` для склейки словоформ, генерация падежей, падежный суффикс
   `[PERSON_3|дат]`, обратная подстановка.
3. `keep_terms` (D19) — список «не анонимизировать» (термины 1С).
4. Режим `mode: "agent"` в файловых актах (TR-7.4): метки проекта с суффиксами, картинки → `[IMAGE_n]`,
   метаданные вычищаются, «слипшиеся» ФИО правятся **только в образе**.
5. Текстовые образы `pdf`/`pptx`/`mpp` (D18): `hds_extract` → `anonymize_text` → `<имя>.<ext>.md`,
   только для чтения.
6. `office_read` / `office_edit` / `office_create` (TR-3.5) — операции с docx/xlsx/pptx через Python.

**Что НЕ входит в A1:** индекс и маршрут `/mcp/agent/<P>` (A2), процесс `hds_agent` и фильтр
исходящего (A3), демон зеркалирования `hds agent-sync` (A4), вкладка «Агент» в UI (A5).

## 1. Обязательное чтение (в этом порядке)

1. `tools/parity/AGENT_SPIKES.md` — **§3.9** (морфология: что реально работает, что нет),
   **§3.10** (стенд и эталон, `recall@10 = 1,0`), **§3.11** (правки плана П-1…П-14),
   **§5** (решения владельца В-1…В-6), **§4** (API SDK 0.2.0 — понадобится в A3).
2. `PLAN_AGENT_PRIVACY_ZONE.md` — целиком, но особенно:
   * §1 (принципы П1–П6), §3 (D3, D4, D7, D19, D22),
   * §4.4 (раскладка каталогов), §4.5 (два Cline),
   * §5.1 (TR-1.1…TR-1.7 — схема базы и RPC), §5.2 (TR-2.1…TR-2.6 — морфология),
   * §5.3 (TR-3.5 office-инструменты, TR-3.8 правила), §5.7 (TR-7.3a, TR-7.4 режим `agent`),
   * §6 (таблица фаз: строка A1 — что считается «готово»), §9 (риски).
3. `sidecar/README.md` — контракт воркеров (stdio, JSON-RPC 2.0, NDJSON, `--root`, progress без `id`).
4. Код, который будет меняться:
   * `sidecar/hds_anonymize/mapping_store.py` — текущая схема v1 (`mappings`, `counters`), заменяется;
   * `sidecar/hds_anonymize/handlers.py` — `AnonymizeHandlers.anonymize/deanonymize/inspect`,
     `PLACEHOLDER_TOKEN_RE`, `_atomic_write_bytes/_atomic_write_text`, `fix_glued_full_names`;
   * `sidecar/hds_anonymize/replacer.py` — `TextReplacer`, `StreamDeAnonymizer`, `MessageAnonymizer`;
   * `sidecar/hds_anonymize/file_parser.py` — `FileParser`/`FileAssembler`, `_scrub_docx_metadata`,
     `_strip_hf_images`, `read_anon_marker`;
   * `sidecar/hds_anonymize/ner_service.py`, `natasha_engine.py`, `gliner_engine.py` — NER и сущности;
   * `sidecar/hds_anonymize/worker.py` — RPC-dispatch и параметры (`--db`, `--device`, `--hide-*`);
   * `sidecar/hds_anonymize/settings.py` — `NER_ENGINE`, `PII_CATEGORIES`, `configure()`;
   * `crates/hds-anonymize/src/{lib,client,protocol,settings,folder}.rs` — Rust-клиент воркера;
   * `crates/hds-cli/src/cmd/anonymize.rs`, `crates/hds-ui/src/anonymize.rs` — CLI и карточка.
5. Тесты: `sidecar/tests/test_anonymize_{store,roundtrip,worker,xml,name_fixer,optional_categories}.py`
   — это база для новых тестов; запуск: `python -m unittest discover -s sidecar/tests -t sidecar -v`.
6. `cline-rules/dev-machine.md` — что разрешено на этой машине.

## 2. Факты о состоянии (проверено 17.11.2026)

| Факт | Следствие для A1 |
|---|---|
| Ветка `agent-privacy-zone`, HEAD `3f34d51d`, push не делался | работать в этой ветке; push — только по просьбе владельца |
| `sidecar/venv-anonymize` **собран** (профиль `cuda`, self-test OK, CUDA-провайдер загружен) | не пересобирать; Python — `sidecar\venv-anonymize\Scripts\python.exe` |
| Модели NER — `C:\ProgramData\hermes-disk-search\models\anonymize` (GLiNER PII, navec, slovnet) | для тестов модели не нужны; для ручных прогонов `--models-dir` = этот каталог |
| Тесты базы меток зелёные: `test_anonymize_store.py` — 7 тестов, OK | новые тесты — рядом, в том же стиле (`unittest`, без моделей) |
| **Незакоммиченные правки другого чата**: `README_ANONYMIZATION.md`, `config.example.yaml`, `crates/hds-anonymize/src/settings.rs`, `crates/hds-anonymize/tests/settings.rs`, `sidecar/hds_anonymize/{gliner_engine,handlers,ner_service,settings,worker}.py`, `sidecar/hds_transcribe/glossary.example.yaml`; untracked `sidecar/hds_anonymize/name_fixer.py`, `sidecar/tests/test_anonymize_{name_fixer,optional_categories}.py`, `tools/md2docx.ps1`, `Анализ_рефакторинга_анонимизации.md` | **не трогать, не откатывать, не стейджить.** A1 пересекается с этими файлами (`handlers.py`, `worker.py`, `settings.py`): либо владелец сначала коммитит их (рекомендуется), либо A1 правит поверх и коммитит **только свои** изменения явным списком путей |
| `PLAN_AGENT_PRIVACY_ZONE.md` ещё в редакции 3.1 (правки A0 не внесены) | правки П-1…П-14 из `AGENT_SPIKES.md` §3.11 считать обязательными: П-1 (SDK 0.2.0 — для A3), П-2 (правила `norm_key`), П-12 (суффикс только при однозначном падеже), П-14 (термины `keep_terms`) |
| Решения владельца (`AGENT_SPIKES.md` §5): В-1 ACL как у `secrets.json`; В-3 всегда GPU, CPU — аварийный режим; В-4 свой токен `/mcp/agent/<P>`; В-5 `.pptx`/`.mpp` — в A4; В-6 `venv-anonymize` в `.gitignore` | в A1 влияет В-3: `mode:"agent"` и `anonymize_text` считаем GPU-first; CPU-путь не оптимизируем |
| Боевая база `data/anonymize.db` — в работе владельца | **не трогать**. Тесты и прогоны — только с временной БД (`--db <temp>`), как делалось в A0 |
| Боевой индекс `D:\hermes-disk-search-db\index.db`, `config.yaml` | **не трогать**; в A1 индекс не нужен |
| Стенд S10 и эталон: `tools/parity/out/agent_a0/s10/{project,anon,golden.jsonl,eval.json}` (не в git) | переиспользовать для приёмочного прогона: переанонимизировать 32 документа новой базой и повторить `agent_a0_rag_eval.py` |
| `hds_anonymize` уже умеет: `hello`, `warmup`, `inspect`, `anonymize`, `anonymize_values`, `deanonymize`, `deanonymize_values`, `shutdown`; форматы `docx/xlsx/txt/md/xml` | новые RPC добавлять аддитивно; `hello.capabilities` расширять, `protocol` не ломать |
| Поведение SDK/анонимизации из A0, важное здесь: обработчик **кэширует результат по содержимому файла**; `anonymize_text` пока не существует | при замерах использовать уникальные тексты; `anonymize_text` — новый RPC (TR-1.7) |
| PowerShell 5.1: UTF-8 без BOM читается как ANSI; env-переменные живут между командами; вывод `hds.exe` в консоли — кракозябры | скрипты — только ASCII в сообщениях либо с BOM; `HDS_CONFIG` задавать и снимать в одной команде; для разбора — `--json` |


## 3. Задачи A1 (с методикой и критериями приёмки)

### 3.1. `sidecar/hds_anonymize/vault.py` — база меток v2 (TR-1.1…TR-1.7, D3)

**Новый модуль вместо `mapping_store.py`** (старый удаляется или остаётся тонким фасадом до A1-финала —
решить по объёму правок, но доступ к БД должен быть **только** через `vault.py`, TR-1.1).

Схема — ровно как в TR-1.2 (SQLite WAL, `busy_timeout`, выдача номеров в `BEGIN IMMEDIATE`):

```sql
meta(key PK, value)               -- schema_version=2; rev — растёт при любой записи
entities(id PK, type, n, norm_key, ts, UNIQUE(type,n), UNIQUE(type,norm_key))
forms(id PK, entity_id → entities, value, gram NULL, origin CHECK IN ('seen','derived'),
      UNIQUE(entity_id, value))
global_counters(type PK, last)
projects(id TEXT PK, ts)
labels(project_id, entity_id, type, n, ts, PRIMARY KEY(project_id, entity_id),
       UNIQUE(project_id, type, n))
counters(project_id, type, last INTEGER, PRIMARY KEY(project_id, type))
aliases(project_id TEXT NULL, type, n, entity_id)     -- след «Объединить метки» (TR-2.6)
keep_terms(value TEXT PK)                             -- «не анонимизировать» (D19)
```

**Обязательные свойства (тесты на каждое):**
1. **Глобальные метки уровня 0** — прежнее поведение карточки и `hds anon`: `[PERSON_1]`, `[ORG_1]`
   с per-type счётчиками и дедупом по `(type, norm_key)` (не по точной строке — это и есть D7).
2. **Метки проекта (уровень 1)** — `labels[project_id]` с **своими** счётчиками: в `P0001` и `P0002`
   одна и та же сущность получает независимые номера (TR-1.5). Тест: одна сущность в двух проектах —
   разные метки; номера не выводятся из глобального.
3. **`rev`** инкрементируется при любой записи; `vault_rev` дешёвый (TR-1.6).
4. **Снимок для Rust** — RPC `vault_snapshot {project_id}` →
   `{rev, labels:[{label,type,forms:[{value,gram,origin}]}], others:[value…], keep:[value…]}`.
   Ответ содержит исходные значения: Rust держит его только в памяти (TR-1.6).
5. **Новые RPC** (TR-1.7), объявляются в `hello.capabilities`:
   * `anonymize_text {project_id, text}` → `{text, labels:[{label,type,form,gram}]}` — NER + regex +
     словарь известных форм проекта + `keep_terms`; **никаких файловых операций**;
   * `label_for_value {project_id, value, type}` — «Скрыть ещё…»;
   * `merge_entities {project_id, keep, drop}` — объединение меток (псевдоним в `aliases`);
   * `vault_drop_project {project_id}` — удалить проект и его метки (глобальные сущности остаются);
   * файловые акты получают `project_id` и `mode: "agent"` (см. §3.4).
6. **Форма значения** (`forms`) пишется с граммемами (`gram`) и `origin` (`seen`/`derived`):
   `seen` — встретилось в тексте, `derived` — сгенерировано pymorphy (TR-2.2).

**Критерий.** Новый `sidecar/tests/test_anonymize_vault.py`: несвязываемость проектов, `rev`,
снимок, `merge_entities`, `vault_drop_project`, `keep_terms`, round-trip метки с суффиксом.

### 3.2. Морфология (TR-2.1…TR-2.4, D7) — с учётом результатов A0

В A0 проверено (`AGENT_SPIKES.md` §3.9): генерация падежей **98,5 %**, `norm_key` **90,9 %**,
однозначное определение падежа — **19,7 %**. Отсюда обязательные правила:

**`norm_key` (TR-2.1 + П-2):**
* `PERSON`: Natasha `NamesExtractor` (конструктор `NamesExtractor(MorphVocab())`, вызов `names(text)`,
  не `.extract`), поля `first/last/middle`; `last` приходит **в исходной форме** («Иванову») — лемматизировать;
  инициалы приходят без точки («И») — приводить к `и.`; **дефисную фамилию `NamesExtractor` режет на два
  совпадения** — склеивать по дефису; прилагательные фамилии («Толстой») приводить к мужскому
  именительному, иначе ключ в косвенных падежах расходится («толстый» ≠ «толстой»).
* `ORG`/`LOC`: леммы pymorphy; текст в кавычках и латиница не лемматизируются; **учесть дубли**
  «Северный ветер» ↔ «ООО «Северный ветер»» (A0: ключи разные) — либо правило нормализации ORG,
  либо оставить на «Объединить метки» (TR-2.6) и зафиксировать это в документации.
* Шаблонные типы (`PHONE/EMAIL/INN/KPP/MONEY/DATE`, номера) — каноническая запись, словоформ нет.

**Падежный суффикс (TR-2.3 + П-12):** ставить `|род|дат|вин|твор|пред` только если падеж исходной
формы определён **однозначно** (в A0 — только 19,7 % при разборе одного слова; контекст ФИО повышает,
но это улучшение, а не условие MVP). Иначе — суффикса нет и пометка «проверить» (TR-2.4). В именах
файлов и папок суффикса нет всегда (символ `|` в Windows запрещён).

**Обратная подстановка (TR-2.4):** `[T_n|g]` → `seen`-форма с граммемами `g` → `derived` с `g`
(пометка «проверить») → именительный падеж; терпимый разбор `[person_3]`, `[PERSON 3]`, `[PERSON-3]`.

**Критерий.** `sidecar/tests/test_anonymize_morphology.py` на фикстурах из A0 (8 персон + 3 ORG/LOC,
те же, что в `morph_spike.py`): `norm_key` одинаков для всех падежей, генерация совпадает с ожидаемой
таблицей, суффикс ставится только при однозначности, обратная подстановка даёт исходную форму.


### 3.3. `keep_terms` — «не анонимизировать» (D19, П-14)

Таблица `keep_terms(value TEXT PK)` + загрузка стартового списка. По результатам A0 (§3.9) NER
**фактически** помечает как `ORG`/`LOC`: `ТЗ`, `ЗУП`, `1С`, `Документооборот`, `ДиректБанк`, `ГИС МТ`,
«банком/банка», «Направляем»; коды `REQ-001…REQ-020` попадают в `INN`/`PASSPORT`, а имя человека —
в `EMAIL`. Поэтому стартовый список (файл `sidecar/hds_anonymize/keep_terms.example.yaml`, копия в
`data/` при первом запуске) должен содержать минимум:

* продукты и конфигурации 1С: `1С`, `1С:ERP`, `1С:Бухгалтерия`, `1С:ЗУП`, `ЗУП`, `1С:Документооборот`,
  `Документооборот`, `1С:УТ`, `УТ 11`, `1С:ERP 2.5`, `ДиректБанк`, `ГИС МТ`;
* общие термины проекта: `ТЗ`, `ТЭО`, `ФТТ`, `HTTP-сервис`, `RLS`, `регламентное задание`;
* фирма `1С`;
* коды требований: шаблон `REQ-\d+` (не значение, а **регулярка** — решить, хранить ли шаблоны
  отдельной таблицей `keep_patterns` или расширить `keep_terms` до `(value, is_regex)`; предпочтительно
  второе, чтобы не плодить сущности).

**Как применять:** `keep_terms` проверяется (а) перед выдачей метки в NER/regex-пайплайне, (б) при
создании сущности (не создавать), (в) в снимке (`keep`) — фильтр исходящего не должен считать эти
слова секретами (TR-6.2).

**Критерий.** Тест: текст с `1С:ERP`, `ЗУП`, `REQ-003`, `ТЗ` → этих меток нет; при этом ФИО и
организация в том же тексте маскируются. Плюс тест «keep_terms не ломает существующие метки»:
значение, которое уже есть в базе и попало в `keep_terms`, не пере-нумеруется, а исключается из выдачи.

### 3.4. Режим `mode: "agent"` в файловых актах (TR-7.4)

Файловые RPC (`anonymize`) получают два новых параметра: `project_id` и `mode: "agent" | "classic"`
(по умолчанию `classic` — прежнее поведение, чтобы карточка и `hds anon` не изменились).

В режиме `agent`:
* метки — **уровня 1** (`labels[project_id]`), с падежными суффиксами (§3.2);
* `fix_glued_full_names` (в A0-коде — `name_fixer.py` от другого чата + `handlers.py`) применяется
  **только к образу**; оригинал демон не меняет никогда;
* картинки в теле документа по умолчанию вырезаются (`agent.images: strip`), вместо картинки —
  метка `[IMAGE_n]` (счётчик в пределах документа);
* метаданные чистятся (`scrub_metadata`), автор/организация из `docProps` не остаются;
* в файл пишется маркер `hds-agent/<P>/<artifact_id>` (уже есть `read_anon_marker` — расширить формат);
* имена файлов и папок — **без суффиксов** (TR-2.3), сам файл образа сохраняется под тем же именем.

**Критерий.** Тест на docx с картинкой в теле и в колонтитуле, автором в `docProps.xml` и ФИО в
разных падежах: в образе нет исходных значений, есть `[PERSON_n|пад]` и `[IMAGE_1]`, картинка
колонтитула удалена, картинка тела заменена меткой, маркер на месте; оригинальный файл не изменён
(sha256 до/после равны).

### 3.5. Текстовые образы `pdf` / `pptx` / `mpp` (D18)

* извлечение: `hds_extract` (уже умеет PDF/PPTX/MPP; `.mpp` — через mpxj/Java, JDK 21 на машине);
* текст → `anonymize_text {project_id, text}` → md с разметкой страниц/слайдов/задач:
  `<!-- страница N -->`, `## Слайд N`, `## Задача <ID>`;
* результат — `<имя>.<ext>.md` в workspace (только для чтения; запись блокирует будущий `hds-guard`, A3).

**Критерий.** На стенде S10 (`out/agent_a0/s10/project`) есть `ТЗ 1С ERP сводное (подписанное).pdf`;
сделать по нему `.pdf.md` и убедиться, что (а) текст читается поиском, (б) ФИО/организации заменены,
(в) номера страниц сохранены. `.pptx`/`.mpp` по решению владельца (В-5) проверяются в A4 — в A1
достаточно кода и юнит-теста на синтетическом тексте.

### 3.6. `office_read` / `office_edit` / `office_create` (TR-3.5)

Порт `office_ops.py` из прежнего проекта `anonymizer_proxy` (если исходник недоступен — реализовать
по TR-3.5). Живут в Python-воркере, вызываются Rust'ом через `host_call` (в A1 — просто RPC, канал
`host_call` появится в A3):

| Операция | Что делает |
|---|---|
| `office_read(path)` | абзацы с номерами, таблицы в markdown, листы и ячейки с адресами |
| `office_edit(path, ops[])` | `replace_text`, `set_cell`, `add_row`, `add_column`, `insert_paragraph`, `delete_paragraph`; форматирование, числа, даты и формулы сохраняются |
| `office_create(path, kind, content)` | новый docx (из md), xlsx (из таблиц), pptx (слайды «заголовок + тезисы»); `python-pptx` добавить в `requirements.lock` венва анонимизации |

**Критерий.** Тесты на временных файлах: правка ячейки xlsx сохраняет формулу и формат; правка абзаца
docx не теряет стили; `office_create` создаёт читаемый файл, который открывается `python-docx`/`openpyxl`.

### 3.7. Совместимость: карточка «Анонимизация» и `hds anon` на новой базе

* глобальные метки уровня 0 — прежний вид (`[PERSON_1]`, без суффиксов), чтобы карточка и CLI не
  изменились визуально;
* `inspect` показывает метки/маркер **без значений** (как сейчас);
* `hds anon anonymize/deanonymize/check/warmup` работают без новых флагов; новые флаги — аддитивно
  (`--project`, `--mode agent`);
* `crates/hds-anonymize/src/settings.rs` — новые поля `anonymize.keep_terms_file`, `anonymize.agent.images`
  (появятся в TR-8.3, но тип нужен уже сейчас).

### 3.8. Миграция v1 → v2 (TR-1.3, D4)

При первом открытии базы: если `schema_version` отсутствует или ≠ 2 — файл `anonymize.db`
переименовывается в `anonymize.db.v1-<YYYYMMDD-HHMMSS>.bak`, создаётся новая база. Метки **не
переносятся** (D4). Факт фиксируется в `RELEASE_NOTES.md` и в `README_ANONYMIZATION.md`.
Тест: подсунуть v1-файл, открыть, проверить бэкап и пустую v2.


## 4. Порядок работ

1. **Стоп-точка 0 (согласование, 5 минут).** Проверить `git status`: если незакоммиченные правки
   другого чата на месте — согласовать с владельцем один из двух режимов (он коммитит их / A1 работает
   поверх). Без этого не начинать правки в `handlers.py`, `worker.py`, `settings.py`.
2. **`vault.py`** со схемой v2 + тесты (`test_anonymize_vault.py`), позже — переключение всех вызовов
   на него (одним коммитом, чтобы не было двух источников истины).
3. **Морфология** (`norm_key`, генерация, суффикс, подстановка) + тесты на фикстурах A0.
4. **`keep_terms`** + тесты.
5. **`mode: "agent"`** в файловых актах + тест на docx с картинками и метаданными.
6. **Образы D18** (`pdf`, затем `pptx`/`mpp`-код) + тест на PDF из стенда S10.
7. **`office_*`** + тесты; `python-pptx` в `requirements.lock`.
8. **Миграция v1→v2** + тест; обновление `README_ANONYMIZATION.md` и `RELEASE_NOTES.md`.
9. **Приёмочный прогон**: переанонимизировать стенд S10 новой базой (`out/agent_a0/s10/project` →
   новый `anon2/`), собрать индекс на временном конфиге и повторить `tools/parity/agent_a0_rag_eval.py`
   с тем же `golden.jsonl`. Ожидание: `recall@10` не ниже **0,9** (в A0 было 1,0 на старой
   анонимизации; падение сильнее 0,1 — сигнал, что морфология/`keep_terms` ухудшили образы).
10. Финальные проверки: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
    `cargo test --workspace`, `python -m unittest discover -s sidecar/tests -t sidecar`.

## 5. Критерии готовности A1 (из плана §6 + правки A0)

* [ ] `vault.py` реализует TR-1.1…TR-1.7; доступ к БД только через него.
* [ ] Тесты: несвязываемость проектов, морфология (`norm_key`, падежи, суффикс, подстановка),
      round-trip с суффиксами, `keep_terms`, `merge_entities`, `vault_drop_project`, миграция v1→v2.
* [ ] Карточка «Анонимизация» и `hds anon` работают на новой базе (ручная проверка на 2–3 файлах
      стенда, глобальные метки — как раньше).
* [ ] `mode: "agent"`: метки проекта с суффиксами, `[IMAGE_n]`, метаданные вычищены, оригинал не изменён.
* [ ] `office_read/edit/create` работают и покрыты тестами.
* [ ] `pdf` из стенда даёт читаемый `.pdf.md` с метками.
* [ ] `cargo fmt/clippy/test` и python-тесты зелёные.
* [ ] Приёмочный замер S10: `recall@10 ≥ 0,9`.
* [ ] `README_ANONYMIZATION.md`, `RELEASE_NOTES.md`, `sidecar/README.md` (новые RPC) обновлены.

## 6. Грабли (из A0 и специфики этой фазы)

* **Чужие правки.** `handlers.py`, `worker.py`, `settings.py`, `ner_service.py`, `gliner_engine.py`
  изменены другим чатом и **не закоммичены**. Не откатывать, не стейджить, не переписывать целиком —
  только аддитивные правки и явный список путей в `git add`.
* **PowerShell 5.1.** UTF-8 без BOM читается как ANSI (кириллица ломается в скриптах и в `.ps1`);
  вывод `node`/`hds.exe` в консоли — кракозябры (разбирать `--json` или через файл + `Get-Content`).
* **Env-переменные живут между командами** в одной сессии (`HDS_CONFIG`, `HDS_MODELS_DIR`,
  `HDS_ANONYMIZE_PYTHON`). Задавать и снимать внутри одной команды.
* **`hds anon` обходит только верхний уровень папки** — по подпапкам вызывать отдельно.
* **Кэш обработчика по содержимому**: повторный акт с тем же текстом возвращается мгновенно (в A0 это
  дало ложные 3 мс). Для тестов производительности — уникальные тексты.
* **pymorphy2 0.9.1 на Python 3.12**: работает, но тянет `pkg_resources` с `UserWarning` — не считать
  это ошибкой; `for`-цикл по `parses[:3]` для определения падежа (в A0 — 19,7 % однозначных).
* **`NamesExtractor`**: вызывать `names(text)` (объект вызываемый), конструктор —
  `NamesExtractor(MorphVocab())`; `.extract` не существует; `NewsNgramTagger` в natasha 1.6 нет.
* **`|` в именах файлов Windows запрещён** — суффикс падежа только в тексте, не в путях.
* **WAL и два процесса**: карточка UI и CLI могут писать одновременно — все операции через
  `BEGIN IMMEDIATE` + `busy_timeout` (как в v1).
* **Тесты не должны требовать моделей NER**: юнит-тесты `vault`/морфологии/`keep_terms` — только
  pymorphy/natasha; файловые акты с NER — отдельный маркер `@unittest.skipUnless` по наличию весов.
* **Не оставлять мусор**: временные БД, `.bak`, `anon2/`, `index2.db` — только в `tools/parity/out/`
  (игнорируется git).

## 7. Коммиты и документация

* Ветка — `agent-privacy-zone`; коммитить **явным списком путей** (никаких `git add -A`/`git add .`).
* Разбивка по смыслу: (1) `vault.py` + тесты + переключение вызовов; (2) морфология + тесты;
  (3) `keep_terms`; (4) `mode:"agent"`; (5) образы D18; (6) `office_*`; (7) миграция + документация.
* Сообщения — в стиле репозитория: `feat(anonymize): …`, `fix(anonymize): …`, `docs(anonymize): …`.
* Push — только по просьбе владельца.
* Перед каждым коммитом: python-тесты (`unittest discover`) и, если менялся Rust, `cargo test` по
  затронутым крейтам.

## 8. Что НЕ делать в A1

* Не трогать `config.yaml`, `D:\hermes-disk-search-db\index.db`, боевую `data/anonymize.db`.
* Не реализовывать маршрут `/mcp/agent/<P>`, индекс по анонимной зоне и `ask_my_files` (A2).
* Не писать процесс `hds_agent`, фильтр исходящего и деанонимизацию событий (A3).
* Не делать демон зеркалирования и «Принять/Отклонить» (A4), вкладку «Агент» (A5).
* Не менять `PLAN_AGENT_PRIVACY_ZONE.md` — предложения по правкам писать в отчёт/сообщение владельцу.
* Не «улучшать» чужие незакоммиченные изменения и не переформатировать файлы целиком.


## 9. Промт для нового чата (скопировать целиком)

```text
Ты — ведущий разработчик проекта hermes-disk-search (рабочая папка
C:\Users\Sasha\hermes-disk-search, Windows, PowerShell 5.1). Отвечай по-русски.

ЗАДАЧА: выполнить фазу A1 плана PLAN_AGENT_PRIVACY_ZONE.md — базу меток v2:
python-модуль vault.py со схемой «одна база, два уровня меток», морфологию
(склейка словоформ, падежи, суффикс |падеж), keep_terms, режим mode:"agent"
для файловых актов, текстовые образы pdf/pptx/mpp и office_read/edit/create.
Фаза A0 закрыта, её результаты и правки плана обязательны к учёту.

СНАЧАЛА ПРОЧИТАЙ ЦЕЛИКОМ:
1. AGENT_A1_HANDOFF_PROMPT.md — карта фазы: состояние репозитория (§2),
   задачи с методикой (§3), порядок работ (§4), критерии готовности (§5),
   грабли (§6), правила коммита (§7).
2. tools/parity/AGENT_SPIKES.md — отчёт A0: §3.9 (морфология: что реально
   работает), §3.10 (стенд S10 и recall@10 = 1,0), §3.11 (правки плана
   П-1…П-14), §5 (решения владельца В-1…В-6).
3. PLAN_AGENT_PRIVACY_ZONE.md — весь: §1 (принципы), §3 (D3, D4, D7, D19),
   §4.4 (раскладка), §5.1 (TR-1.1…1.7), §5.2 (TR-2.1…2.6), §5.3 (TR-3.5),
   §5.7 (TR-7.3a, TR-7.4), §6 (строка A1 — критерий «готово»).
4. sidecar/README.md и код: sidecar/hds_anonymize/{mapping_store,handlers,
   replacer,file_parser,ner_service,natasha_engine,settings,worker}.py;
   crates/hds-anonymize/src/{lib,client,protocol,settings,folder}.rs;
   crates/hds-cli/src/cmd/anonymize.rs; sidecar/tests/test_anonymize_*.py.
Затем составь и покажи короткий план (что, в каком порядке, какие стоп-точки)
и сразу начинай работу.

ГЛАВНОЕ:
* Перед правками проверь `git status`: в рабочем дереве есть незакоммиченные
  правки другого чата (README_ANONYMIZATION.md, config.example.yaml,
  crates/hds-anonymize/src/settings.rs, crates/hds-anonymize/tests/settings.rs,
  sidecar/hds_anonymize/{gliner_engine,handlers,ner_service,settings,worker}.py,
  sidecar/hds_transcribe/glossary.example.yaml; untracked name_fixer.py,
  test_anonymize_{name_fixer,optional_categories}.py). Их НЕЛЬЗЯ откатывать,
  стейджить или переписывать. Если они мешают — остановись и согласуй с
  владельцем (он коммитит их / работаем поверх).
* Работай в ветке agent-privacy-zone (HEAD 3f34d51d). Коммить явным списком
  путей, разбивая по смыслу. Push — только по просьбе владельца.
* Доступ к базе меток — только через sidecar/hds_anonymize/vault.py (TR-1.1).
  Схема — ровно TR-1.2. Миграции с v1 нет: файл переименовать в
  anonymize.db.v1-<дата>.bak, создать новую базу (TR-1.3, D4).
* Морфология — с учётом A0: norm_key по правилам П-2 (NamesExtractor(...)
  вызывать как names(text); фамилия приходит в исходной форме; инициалы без
  точки; дефисная фамилия режется на два совпадения; прилагательные фамилии
  приводить к мужскому именительному). Падежный суффикс ставить ТОЛЬКО при
  однозначном определении падежа (в A0 — 19,7 %), иначе без суффикса и
  пометка «проверить» (TR-2.4, правка П-12). В именах файлов суффикса нет.
* keep_terms (D19): стартовый список терминов 1С из §3.3 карты; проверить
  фактом A0, что NER иначе метит ТЗ/ЗУП/1С/Документооборот/ДиректБанк/ГИС МТ
  и коды REQ-…
* mode:"agent" (TR-7.4): метки проекта с суффиксами, картинки → [IMAGE_n],
  scrub_metadata, «слипшиеся» ФИО править только в образе, маркер
  hds-agent/<P>/<artifact_id>, оригинал не меняется.
* Образы pdf/pptx/mpp (D18): hds_extract → anonymize_text → <имя>.<ext>.md,
  только для чтения. office_read/edit/create — по TR-3.5.
* Продуктовый код меняется только в sidecar/ и crates/ (анонимизация, CLI,
  UI-карточка). config.yaml, боевые index.db и data/anonymize.db — не трогать.
* Все временные БД, .bak, образы и индексы — только в tools/parity/out/.

ПРОВЕРКИ И ЗАВЕРШЕНИЕ:
* Тесты: `python -m unittest discover -s sidecar/tests -t sidecar` (интерпретатор
  sidecar\venv-anonymize\Scripts\python.exe) + `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
* Приёмочный прогон: переанонимизировать стенд tools/parity/out/agent_a0/s10/
  project новой базой, собрать отдельный индекс на временном конфиге и
  повторить tools/parity/agent_a0_rag_eval.py с тем же golden.jsonl:
  recall@10 должен остаться ≥ 0,9 (в A0 было 1,0).
* Обнови README_ANONYMIZATION.md, RELEASE_NOTES.md и sidecar/README.md
  (новые RPC и режим agent).
* В конце дай сводку: что сделано, какие тесты/замеры, какие решения приняты,
  какие вопросы к владельцу.
```

