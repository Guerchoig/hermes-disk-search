# AGENT_A5_5_HANDOFF_PROMPT.md — контекст и промт для под-этапа A5.5 («Метки проекта»)

> **Кому:** новый чат Cline в режиме Act. **Дата среза:** 20.11.2026.
> **Ветка:** `agent-privacy-zone`, **HEAD:** `fb699982` (`docs(agent): A5.4 закрыт`);
> ahead 447, дерево чистое, push не делался.
> **Что это:** самодостаточный контекст для продолжения **фазы A5** основного проекта
> `PLAN_AGENT_PRIVACY_ZONE.md` («ИИ-агент в анонимной зоне»). A0–A4 закрыты, A5 идёт
> под-этапами (`AGENT_A5_STAGES.md`), **A5.1–A5.4 закрыты**. Этот файл — A5.5:
> **панель «Метки проекта»** (TR-2.6): таблица «метка → значение (формы) → где
> встречается», «Объединить», «Разъединить», «Не скрывать»; после склейки образы
> пересоздаются, агенту уходит служебное сообщение. Настройки и «Очистить историю» —
> A5.6, живой сценарий §7 — A5.7.

## 0. Где мы (коротко)

Пользователь работает с документами в своей папке, агент — только с анонимными образами
в `<agent.root>\P<NNNN>\workspace`; исходные значения не покидают машину (I1), оригиналы
не меняются без решения пользователя (I6), в журналах нет значений (I8).

* **A4** — двустороннее зеркало `hds agent sync` — закрыта (`AGENT_A4_REPORT.md`).
* **A5.1** — каркас вкладки и API состояния (`/api/agent/projects|state|files`).
* **A5.2** — реплика: `preview`, `label`, `keep`, `attach`, `message`, `abort`, `stop`.
* **A5.3** — лента SSE (`/api/agent/events`, `text/event-stream`), `ask_user` и
  `POST /api/agent/answer`, переключатель «как видит модель».
* **A5.4** — панель «Результаты»: `Mirror::compare`, `GET /api/agent/pending|compare`,
  `POST /api/agent/decision`, решения A4 (`accept|reject|conflict`) из вкладки,
  `notice` в ленту (`A54_E2E.md`).

**Зачем A5.5.** NER не склеивает частичные упоминания: «Иванов», «Иванов Иван Иванович»,
«И. И. Иванов» — три сущности и три метки (`[PERSON_3]`, `[PERSON_5]`, `[PERSON_8]`).
Агент видит трёх разных людей, деанонимизация даёт три разных значения. Склейку делает
**пользователь** в панели «Метки проекта» (TR-2.6, решение О2): отметил строки →
«Объединить» (это не команда чата). В базе формы младших переезжают в старшую, младшие
номера становятся псевдонимами (`aliases`) — уже выпущенные файлы с `[PERSON_5]`
по-прежнему деанонимизируются; образы пересоздаются с одной меткой; агенту уходит
сообщение «метки … теперь обозначаются …». «Разъединить» отменяет **ошибочную** склейку —
но только ту, что записана в `merge_log` (склейки до A5.5 необратимы, это честно пишется
в интерфейсе; решение В-A5-3).

## 1. Состояние репозитория (проверено 20.11.2026)

| Что | Значение |
|---|---|
| Ветка / HEAD | `agent-privacy-zone`, `fb699982`; коммиты A5.4: `b5daab0b` (feat), `fb699982` (docs) |
| Тесты | `cargo test --workspace -- --test-threads=4` — **126 наборов / 899 passed / 0 failed / 13 ignored**; python 328 OK (skip 1); `npm test` 23/23 |
| fmt / clippy | чисто (`cargo fmt --all --check`, `clippy --workspace --all-targets -- -D warnings`) |
| Новые наборы A5.4 | `hds-agent` unit (compare ×3, `Decision::parse`), `hds-ui` unit (`results_err`), `hds-ui/tests/agent_results.rs` (5, реальный сокет) |
| Стенд | `tools\parity\out\agent_a42`: 34 записи реестра, `pending` пуст; отчёты `A51_E2E.md`…`A54_E2E.md`, сценарии `a51_e2e.ps1`…`a54_e2e.ps1` |
| Метки стенда | `root\data\anonymize.db`, `meta.schema_version=2`; у `P0001` **46 меток**, в workspace встречаются **34**; `aliases` пуст |

**Известная флейка:** `hds-mcp/tests/quota_socket` под параллельной нагрузкой получает
503 (`max_conns` 32) — одиночный прогон зелёный, к правкам A5 не относится.

## 2. Обязательное чтение (по порядку)

1. `AGENT_A5_STAGES.md` — §2 (строка A5.5), §3.1–§3.4 (что уже сделано), §4.1 (решения
   В-A5-1…8, В-A5.2-1…6, В-A5.3-1…5, В-A5.4-1…6; особенно **В-A5-2** «число упоминаний»
   и **В-A5-3** «Разъединить»), §4.2 (долги 1–29; п.4–6 — RPC меток, `merge_log`,
   точный счётчик), §5 (контракты), §6 (грабли 1–38), §7 (окружение).
2. `PLAN_AGENT_PRIVACY_ZONE.md` — §5.2 (TR-1.5: метки проектов не связываются, TR-1.6:
   снимок `{rev, labels, others, keep}`, TR-2.2 формы/падежи, TR-2.6 «Объединить метки»),
   §5.8 (TR-8.2 — панель «Метки проекта»), §5.9 (инварианты I1/I5/I8), §7 (приёмка).
3. `AGENT_A5_4_HANDOFF_PROMPT.md` — §3–§9 как методика под-этапа (структура «ядро → маршруты
   → панель → тесты → E2E → документы»); `A54_E2E.md` — как выглядит отчёт стенда.
4. `sidecar/hds_anonymize/vault.py` — целиком: `SCHEMA_VERSION=2`, `_migrate_if_needed`
   (v1 переименовывается, метки не переносятся, D4), `_init_schema`, `aliases`,
   `_project_entity` (метки → псевдонимы), `project_value`, `project_mappings`,
   `snapshot`, **`merge_entities`** (формы копируются в старшую, `labels` младшей
   удаляется, `aliases` пишется; формы младшей **остаются**), `drop_project`, `stats`.
5. `sidecar/hds_anonymize/{worker,handlers}.py` — диспетчер методов (`vault_snapshot`,
   `merge_entities`, `keep_terms`, `vault_stats`; строки ~359–425 в `worker.py`),
   `handlers.vault_snapshot/merge_entities/keep_term_add`; `schemas.py` — JSON-схемы,
   если они валидируют параметры.
6. `crates/hds-anonymize/src/{client,protocol}.rs` — `snapshot`, `vault_rev`,
   `merge_entities` (возвращает `serde_json::Value`), `keep_terms`, `add_keep_term`;
   `Snapshot{rev, labels[{label, entity_type, forms[{value, gram, origin}]}], others, keep}`.
7. `crates/hds-anonymize/tests/{mock_worker.py,client.rs}` — mock-воркер отвечает на
   `merge_entities`; новый RPC придётся добавить и туда (иначе клиентский тест не пройдёт).
8. `sidecar/tests/test_anonymize_vault.py` — `SchemaTest`: **точный список таблиц**
   (`meta, entities, forms, global_counters, projects, labels, counters, aliases, keep_terms`)
   и `test_merge_entities_keeps_alias`; добавление `merge_log` меняет этот тест.
9. `crates/hds-agent/src/{mirror,results,validator,factory}.rs` — трейт `Anonymizer`
   (8 методов, `snapshot`), `Mirror::from_config`, `LazyAnonymizer`/`LazyExtractor`,
   `results::rebuild_image` (сейчас private, ~578), `set_baseline`, `MessageKind`
   (+`Message`), `MESSAGES_FILE`, `notify` (private, ~695), `validator::read_text`.
10. `crates/hds-ui/src/{agent,agent_session,page,lib}.rs` — `agent::gate`, `with_mirror`
    (~471), `pending_json`/`decision_json`, `AgentApi::client` (один Python-воркер на UI),
    `jget/jpost/esc`, секции вкладки; `lib.rs` — GET/POST-маршруты `/api/agent/*`.
11. `crates/hds-cli/src/cmd/anonymize.rs` — `cmd_merge` (`hds anon merge <проект> <метка>
    <метка…>`, ~586): CLI-путь склейки; **не сломать**, он должен получить `merge_log`
    и «Разъединить» вместе с UI.
12. `crates/hds-ui/tests/agent_results.rs` — образец сокет-тестов A5.4 (Stand, mock-воркер
    `tests/fixtures/mock_anon.py`, `install_global`, `reset_for_tests`).


## 3. Что уже готово — опора A5.5

| Что | Где | Чем пользоваться |
|---|---|---|
| **База меток** | `sidecar/hds_anonymize/vault.py` | `SCHEMA_VERSION=2`; таблицы `meta/entities/forms/global_counters/projects/labels/counters/aliases/keep_terms`; `_init_schema` идемпотентна (`CREATE TABLE IF NOT EXISTS`); `_migrate_if_needed` **переименовывает** базу при другой версии (D4) |
| **Склейка** | там же, `merge_entities(project_id, keep, drop)` | проверяет тип, копирует формы младших в старшую, удаляет `labels` младших, пишет `aliases`; возвращает `{keep, dropped, forms_moved}`; **формы младшей сущности остаются** (на этом можно построить «Разъединить») |
| **Псевдонимы** | `aliases(project_id, type, n, entity_id)`; `_project_entity` | деанонимизация уже выпущенных файлов: `project_value` ищет `labels`, затем `aliases` |
| **Снимок** | `vault.snapshot(project_id)` | `{rev, labels:[{label,type,forms:[{value,gram,origin}]}], others, keep}` — **без** aliases/merges и без числа файлов |
| **RPC** | `sidecar/hds_anonymize/worker.py` (~359–425), `handlers.py` | методы `vault_snapshot`, `vault_rev`, `merge_entities`, `vault_drop_project`, `keep_terms`, `vault_stats`; список `capabilities` в `hello` (~275–287 и ~457–461) |
| **Клиент** | `crates/hds-anonymize/src/client.rs`, `protocol.rs` | `snapshot`, `vault_rev`, `label_for_value`, `merge_entities -> serde_json::Value`, `drop_project`, `keep_terms`, `add_keep_term`; `Snapshot/SnapshotLabel/SnapshotForm` |
| **Mock-воркер** | `crates/hds-anonymize/tests/mock_worker.py` | отвечает на `merge_entities`, `keep_terms`; новый RPC добавлять сюда же; тест `tests/client.rs` |
| **CLI** | `crates/hds-cli/src/cmd/anonymize.rs` (~586) | `hds anon merge <P> <keep> <drop…>` и `hds anon keep list|add` — не ломать, они ходят через тот же `vault.merge_entities` |
| **Ядро зеркала** | `crates/hds-agent/src/mirror.rs` | трейт `Anonymizer` (8 методов, `snapshot`), `Mirror::from_config`, `LazyAnonymizer`/`LazyExtractor`, `sha256_file`, `kind_for`, `rel_to_path` |
| **Пересборка образа** | `crates/hds-agent/src/results.rs` (~578) | `rebuild_image(row, status)` — private, внутри `impl Mirror`: пишет образ из **текущего** оригинала и `set_baseline`; нужен публичный вход (метод рядом, не копия) |
| **Очередь агенту** | `results.rs` | `MessageKind::{DocumentRejected, ConflictDelegated}`, `Message{ts,kind,rel,text}`, `MESSAGES_FILE`, `notify` (private, ~695) — писать строкой JSON, без значений (I8) |
| **Чтение текста** | `crates/hds-agent/src/validator.rs` | `pub(crate) read_text(path, &mut dyn TextExtractor)`: md/txt/xml — напрямую, docx/xlsx/pdf/pptx — `hds_extract` |
| **Метки в тексте** | `hds_core::labels` | `find_labels(text)`, `label_parts(token)`, `gram_to_suffix` — своих регулярок меток нет (AG-1) |
| **Маршруты вкладки** | `crates/hds-ui/src/{agent,lib}.rs` | `agent::gate` (403 в server-профиле), `err/ok`, `with_mirror` (~471), `pending_json`/`decision_json`, CSRF-гейт, `is_project_id` |
| **Состояние вкладки** | `crates/hds-ui/src/agent_session.rs` | `AgentApi::client(cfg, state_dir) -> Arc<Mutex<Client>>` — **один** Python-воркер на процесс UI; `notice` (A5.4), кольцо/SSE |
| **Панель** | `crates/hds-ui/src/page.rs` | секции «Проекты», «Реплика», «Состояние проекта», «Результаты», «Файлы проекта», «Новый проект»; `jget/jpost/esc`, `agentMsg`, `agentCopyPath` |
| **Стенд** | `tools\parity\out\agent_a42` | 46 меток `P0001`, `aliases` пуст; в workspace 34 метки; `[PERSON_1]` и `[PERSON_2]` встречаются совместно в 2 образах; `anonymize.db` — `root\data\anonymize.db` |
| **E2E-образец** | `a54_e2e.ps1`, `A54_E2E.md` | mock-провайдер, `ReadShared`, бэкап/восстановление `projects.json`/`secrets.json`, ASCII-only + русские тексты в JSON |

**Чего в A5.5 не делать:** автоматическая склейка частичных упоминаний (фаза 2), точный
счётчик вхождений (В-A5-2: формы + число файлов), «Разъединить» для склеек **до** A5.5
(их нет в `merge_log`), настройки и «Очистить историю» (A5.6), доставка `messages.jsonl`
в сессию, правки демона/зеркала (`sync_once` не меняем), Word-compare, правки
`sidecar/hds_agent`.


## 4. Задачи A5.5 с методикой

### 4.1. RPC `vault_project_labels` (Python + клиент)

Почему: `snapshot` отдаёт только метки с формами — панели нужны ещё псевдонимы,
журнал склеек и (в связке с реестром) «где встречается». Расширять `snapshot` нельзя:
это горячий путь фильтра (TR-1.6), его форма зафиксирована тестами и Rust-типами.

`vault.py`, новый метод рядом со `snapshot`:

```python
def project_labels(self, project_id: str) -> dict:
    """Данные панели «Метки проекта» (TR-2.6): метки, псевдонимы, склейки, keep."""
    # labels:  [{label, type, forms: [{value, gram, origin}]}]  — как в snapshot
    # aliases: [{label, type, n}] — младшие номера, ставшие псевдонимами
    # merges:  [{id, ts, keep, dropped: [label], forms_moved}]   — из merge_log
    # keep:    [значения D19]
    # rev:     self.rev()
```

* `aliases` — строки `aliases` проекта, сгруппированные по `entity_id` старшей метки;
  метка старшей берётся из `labels` (нет её — не отдавать).
* `merges` — из `merge_log` (§4.2); `dropped` — метки, `forms_moved` — счётчик.
* Значения форм отдаются (зона пользователя, как `snapshot`), но **не логируются** и в
  `sync.log`/аудит не попадают (I8).

`worker.py`/`handlers.py`: метод `vault_project_labels`, строка в `capabilities` `hello`,
`handlers.vault_project_labels(project_id) -> self.store.project_labels(project_id)`.
`schemas.py` — если параметры валидируются схемой, добавить `{project_id}`.

`crates/hds-anonymize`:

```rust
// protocol.rs
pub struct ProjectLabel { pub label: String, pub entity_type: String,
                          pub forms: Vec<SnapshotForm>, pub aliases: Vec<String> }
pub struct MergeRecord  { pub id: i64, pub ts: f64, pub keep: String,
                          pub dropped: Vec<String>, pub forms_moved: u64 }
pub struct ProjectLabels { pub rev: i64, pub labels: Vec<ProjectLabel>,
                           pub merges: Vec<MergeRecord>, pub keep: Vec<String> }

// client.rs
pub fn project_labels(&mut self, project_id: &str) -> Result<ProjectLabels, AnonymizeError>;
```

Mock-воркер (`tests/mock_worker.py`) отвечает `vault_project_labels` фикстурой
(2 метки, 1 склейка); в `tests/client.rs` — проверка разбора (значения не печатать).

### 4.2. `merge_log` и `unmerge_entities` (Python)

Почему: `aliases` помнят **факт** склейки, но не помнят, **что** склеили и какие формы
перенесли. Без этого «Разъединить» невозможно (В-A5-3: только склейки, записанные в
`merge_log`; до-A5.5 необратимы — так и написать в интерфейсе).

Схема — **аддитивно**, в `_init_schema` рядом с `aliases`:

```sql
CREATE TABLE IF NOT EXISTS merge_log (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   TEXT NOT NULL,
    ts           REAL NOT NULL,
    keep_type    TEXT NOT NULL,
    keep_n       INTEGER NOT NULL,
    keep_entity  INTEGER NOT NULL,
    dropped      TEXT NOT NULL,   -- JSON [{type, n, entity_id}]
    forms_before TEXT NOT NULL    -- JSON [значение…]: формы старшей ДО склейки
);
```

* **`SCHEMA_VERSION` не повышать.** `_migrate_if_needed` при другой версии переименует
  базу и **потеряет метки** (D4). Таблица создаётся `IF NOT EXISTS` — это миграция
  «на месте», как `projects`/`aliases`.
* `merge_entities`: в той же транзакции (`_write()`) писать строку `merge_log`
  (`dropped` — то, что реально переехало, `forms_before` — формы старшей до переноса)
  и вернуть `merge_id` в ответе. Поведение (алиасы, копирование форм, отказ для разных
  типов) **не менять** — на нём CLI и тесты.
* `unmerge_entities(project_id, merge_id)`: проверить проект и запись; для каждой
  младшей метки: удалить `aliases`-строку, вернуть `labels`-строку (сущность и её формы
  никуда не удалялись), из старшей удалить формы, которых **не было** в `forms_before`
  и которые есть у младшей сущности (формы младшей не удалялись — их можно перечитать),
  удалить `merge_log`-строку; вернуть `{restored: [label], forms_removed: N}`.
* Ошибки: неизвестный `merge_id`/чужой проект — `VaultError`; метка уже склеена заново
  (нет `aliases`) — `VaultError` с понятным текстом (UI → 409).
* `drop_project` чистит `merge_log` проекта вместе с `labels`/`aliases`.
* Тесты в `sidecar/tests/test_anonymize_vault.py`: обновить `SchemaTest` (таблицы и
  `PRAGMA table_info(merge_log)`), `test_merge_writes_log`, `test_unmerge_restores`
  (после unmerge `project_value(drop)` даёт **своё** значение, старшая метка не
  потеряла собственных форм, `project_mappings` снова содержит обе), `test_unmerge_unknown`.


### 4.3. Ядро: «где встречается», пересборка образов, сообщение агенту

**Новый модуль `crates/hds-agent/src/labels.rs`** (не в UI: реестр, workspace и
`hds_extract` — зона ядра):

```rust
pub struct LabelOccurrence { pub rel: String, pub anon_rel: String, pub count: usize }
pub struct LabelRow {
    pub label: String, pub entity_type: String,
    pub forms: Vec<hds_anonymize::protocol::SnapshotForm>,
    pub aliases: Vec<String>,          // младшие метки, ставшие псевдонимами
    pub occurrences: Vec<LabelOccurrence>, // файлы реестра, где метка есть в образе
}
pub struct LabelsReport {
    pub rev: i64, pub labels: Vec<LabelRow>,
    pub merges: Vec<hds_anonymize::protocol::MergeRecord>, pub keep: Vec<String>,
}

/// Снимок `vault_project_labels` + проход по образам реестра.
pub fn report(
    registry: &ArtifactRegistry,
    workspace: &Path,
    anonymizer: &mut dyn Anonymizer,
    extractor: &mut dyn TextExtractor,
    project_id: &str,
) -> Result<LabelsReport, LabelsError>;
```

* `occurrences` — по `registry.list()`: только записи с непустым `anon_rel`; текст
  образа через `validator::read_text` (md/txt — напрямую, docx/xlsx — `hds_extract`);
  вхождения — `hds_core::labels::find_labels` (своих регулярок нет, AG-1); `count` —
  число вхождений, `rel`/`anon_rel` — пути из реестра.
* Заголовки docx в текст не попадают (A5.4 §6 п.33) — в панели подпись «по тексту
  документов (тело)».
* `LabelsError` — свой enum (`Snapshot`, `Registry`, `Anonymizer`, `Extractor`, `Io`)
  с `Display` для HTTP-текста; значения в текст ошибки не попадают.

**Пересборка после склейки** — метод в `results.rs` (там же, где `rebuild_image`):

```rust
pub struct RebuiltImage { pub rel: String, pub anon_rel: String, pub status: Status }
pub struct RefreshReport { pub rebuilt: Vec<RebuiltImage>, pub skipped: Vec<String> }

impl<'a> Mirror<'a> {
    /// Образы, где встречаются `dropped`, пересоздаются из текущего оригинала
    /// (после склейки NER отдаёт старшую метку). Варианты и конфликты не трогаем.
    pub fn refresh_label_images(&mut self, dropped: &[String])
        -> Result<RefreshReport, ResultsError>;
}
```

* Условия: `status` из `{Synced, Accepted}`; текст образа содержит хотя бы одну
  `dropped`-метку; иначе — пропуск (`skipped`: `pending_accept`, `conflict`,
  `agent_changed`, `orphan` — там есть правки/вариант, пересборка их потеряет).
* Внутри — существующий `rebuild_image(row, Status::Synced)`, не копия кода;
  `rebuild_image` можно оставить private (метод в том же модуле).
* После пересборки — **один** `sync_once()` и проверка, что изменений нет
  (критерий TR-2.6/§2: «после склейки — один проход `Mirror::sync_once`»).

**Сообщение агенту** — в `results.rs`:

* `MessageKind::LabelsMerged` (`"labels_merged"`) и `MessageKind::LabelsUnmerged`
  (`"labels_unmerged"`) + `as_str`/`parse` (тест `choices_and_message_kinds_parse_back`
  расширить, старые значения не менять).
* Публичный `pub fn append_message(project_root: &Path, kind: MessageKind, rel: &str,
  text: String) -> Result<Message, ResultsError>`; `Mirror::notify` делегирует ему.
  Для меток `rel = ""`, текст — только метки: «метки [PERSON_5], [PERSON_8] теперь
  обозначаются [PERSON_3]» / «склейка меток отменена: … снова самостоятельные» (I8).
* Порядок: склейка → пересборка → сообщение → `notice` в ленту («метки склеены: …»).

### 4.4. Маршруты вкладки

| Маршрут | Тело/query | Ответ | Коды |
|---|---|---|---|
| `GET /api/agent/labels` | `project` | `{rev, labels:[{label, type, forms, aliases, occurrences:[{rel, anon_rel, count}], files, forms_count}], merges:[{id, ts, keep, dropped, forms_moved}], keep}` | 400 (нет project), 404 (проект/реестр), 500 |
| `POST /api/agent/labels/merge` | `{project, keep, drop:[…]}` | `{ok, keep, dropped, merge_id, rebuilt:[…], skipped:[…], message}` | 400 (пусто/одна метка/разные типы), 404 (проект, метка не проекта), 409 (метка уже псевдоним), 500 |
| `POST /api/agent/labels/unmerge` | `{project, merge_id}` | `{ok, restored:[…], rebuilt:[…], skipped:[…], message}` | 400 (нет merge_id), 404 (нет склейки), 409 (метка склеена заново), 500 |
| `POST /api/agent/keep` | `{project, value}` | уже есть (A5.2) — переиспользовать для «Не скрывать» | 200/400/404 |

* Все — под `agent::gate()` и общим CSRF-гейтом (как A5.4); в server-профиле 403.
* Python-воркер — **один на UI**: `AgentApi::client(cfg, state_dir)`; маршрут склейки:
  `client.merge_entities(...)` → отпустить замок → `with_client_mirror(...)` для
  `refresh_label_images` + `sync_once`; `LazyAnonymizer` не поднимать (второй воркер,
  A5.4 §6 п.12).
* `with_mirror` A5.4 создаёт `LazyAnonymizer` — для A5.5 добавить вариант
  `with_client_mirror`, который берёт `&mut *client.lock()` (образец — `preview`/`attach`
  в `agent_session.rs`). Замок держится только на пересборку+sync.
* `notice` после решения — через `AgentApi::notice` (A5.4): текст без значений.


### 4.5. Панель «Метки проекта»

Секция в `page.rs` (после «Файлов проекта», перед «Новым проектом») — порядок вкладки
не догма, но не ломать существующие id и кнопки:

```html
<section><h2>Метки проекта</h2>
 <div class="row"><button onclick="loadAgentLabels()">Обновить</button>
  <span id="aglabelsmsg" class="muted"></span></div>
 <table class="tr"><thead><tr><th></th><th>Метка</th><th>Значение (формы)</th>
  <th>Где встречается</th></tr></thead><tbody id="aglabels"></tbody></table>
 <div class="row" style="margin-top:6px">
  <button onclick="agentMerge()">Объединить выбранные</button>
  <span class="muted">старшая — метка с наибольшим числом файлов</span></div>
 <div class="muted" style="margin-top:4px">«Разъединить» отменяет склейку, записанную в
  merge_log; склейки до A5.5 необратимы.</div>
 <table class="tr"><thead><tr><th>Когда</th><th>Оставлено</th><th>Склеено</th><th></th></tr></thead>
  <tbody id="agmerges"></tbody></table></section>
```

* Строка: checkbox (`data-label`), метка (`<b>`), тип, формы значениями (зона
  пользователя), «где встречается» — число файлов + первые 3 пути из `occurrences`
  («ещё N»); пусто — «в образе не встречается».
* «Не скрывать» — кнопка у каждой формы: `POST /api/agent/keep {project, value}`,
  затем `loadAgentLabels()`; подпись «действует во всех проектах» (D19).
* «Объединить»: выбрано ≥ 2 метки; проверить один тип (иначе сообщение, запрос не
  отправлять); `keep` — максимум `files`, при равенстве — первая выбранная; `confirm()`
  с текстом «старшая [PERSON_1], младшие [PERSON_2], [PERSON_3]»; после 200 —
  `loadAgentLabels(); loadAgentFiles();` (+ `loadAgentState()` — если показывается сводка).
* «Разъединить» в блоке склеек: `confirm()`, затем
  `POST /api/agent/labels/unmerge {project, merge_id}`; ошибка сервера — текстом в
  `aglabelsmsg`, таблица не меняется.
* Своих регулярок меток и разбора значений в JS нет (AG-1): всё, что показывается,
  пришло из ответа сервера.
* `notice` («метки склеены/разъединены») приходит в ленту сам (A5.4) — дублировать
  строку в панели не нужно.

### 4.6. Тесты

| Уровень | Что | Где |
|---|---|---|
| Python | `SchemaTest` (таблица `merge_log` + колонки), `test_merge_writes_log`, `test_unmerge_restores`, `test_unmerge_unknown`, `test_project_labels_shape` (метки/формы/aliases/merges/keep), отказ при разных типах — уже есть | `sidecar/tests/test_anonymize_vault.py` |
| Python | диспетчер `vault_project_labels` в worker/handlers (если `test_anonymize_worker.py` перечисляет методы — добавить) | `sidecar/tests/test_anonymize_worker.py` |
| Rust-клиент | разбор `ProjectLabels`/`MergeRecord` на mock-воркере | `crates/hds-anonymize/tests/{mock_worker.py,client.rs}` |
| `hds-agent` unit | `labels::report` (2 метки, occurrences по FakeAnon/FakeExtractor), `refresh_label_images` (Synced пересобран, PendingAccept — в `skipped`), `append_message` + `MessageKind::parse` | `crates/hds-agent/src/labels.rs`, `results.rs` |
| `hds-ui` unit | разбор тела merge/unmerge (пусто/один тип/нет merge_id) → коды 400 | `agent.rs` (`#[cfg(test)]`) |
| `hds-ui` сокет | `crates/hds-ui/tests/agent_labels.rs` (образец `agent_results.rs`): GET labels с метками и merges; merge → `rebuilt` непуст, образ содержит старшую метку, `messages.jsonl` + `labels_merged`; unmerge → метки вернулись, `labels_unmerged`; keep; 400/404/409; 403 без CSRF и в server-профиле | новый файл |
| CLI | `cargo test -p hds-cli` — `hds anon merge` и его тесты не сломаны | — |

**Проверки перед коммитом:** `cargo fmt --all --check`, `clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace -- --test-threads=4`,
`python -m unittest discover -s sidecar/tests -t sidecar`, `npm --prefix sidecar/hds_agent test`.


### 4.7. E2E стенда (`a55_e2e.ps1`, `A55_E2E.md`)

Сценарий — по образцу `a54_e2e.ps1` (ASCII-only, русские тексты — `a55-requests.json`,
бэкап `projects.json`/`secrets.json`, `ReadShared`, `finally`).

1. **Бэкап базы меток**: копия `root\data\anonymize.db` (+`-wal`/`-shm`) в
   `a55-anonymize.backup.db`; в `finally` — восстановить, если «Разъединить» не прошло.
2. `target\debug\hds.exe agent sync --project P0001 --once`; `hds ui --port 8799`.
3. `GET /api/agent/labels?project=P0001` → 46 меток; выбрать пару **одного типа** с
   вхождениями: `keep` — максимум `files`, `drop` — следующая того же типа
   (на стенде подходит `[PERSON_1]`/`[PERSON_2]` — совместно в 2 образах; выбор
   записать в `a55-choice.json`).
4. Запомнить `occurrences` младшей метки (файлы образа) и их содержимое до склейки.
5. `POST /api/agent/labels/merge {project, keep, drop}` → 200: проверить `merge_id`,
   `dropped`, непустой `rebuilt`, `message`.
6. Проверки после склейки:
   * `GET /api/agent/labels` — `drop` нет среди `labels`, есть в `merges`;
   * в файлах из п.4 нет `drop`, есть `keep` (docx — распаковать `word/document.xml`,
     md/txt — прочитать как UTF-8);
   * `messages.jsonl` — строка `labels_merged` с обеими метками, значений нет;
   * `hds agent sync --once` — `changes=0` (один проход, без петель);
   * деанонимизация старых меток: `Vault.project_value('P0001', drop)` возвращает
     значение старшей (aliases), проверка — `a55_probe.py` (stdlib + `hds_anonymize.vault`).
7. `POST /api/agent/labels/unmerge {project, merge_id}` → 200: `drop` снова в `labels`,
   `merges` пуст, образы снова содержат `drop`, `messages.jsonl` += `labels_unmerged`,
   `sync --once` — `changes=0`.
8. **«Не скрывать»**: `POST /api/agent/keep {project, value:"СКЛАД-ТЕСТ-55"}` (значение
   вымышленное, в документах не встречается) → `added=true`, `GET /api/agent/labels` —
   `keep` содержит его; в отчёте отметить, что стенд пополнился одним keep-термином.
9. **I8**: сканировать `a55-e2e.log`, `a55-ui.log/err`, `sync.log`, `messages.jsonl`,
   отчёты валидатора — значений из `a55-requests.json` нет; `merge_log`/`aliases` — только
   метки и id.
10. `a55-checks.txt` (все проверки `= ok`) и отчёт `A55_E2E.md`: шаги, числа, что
    изменилось в стенде, найденные грабли.

### 4.8. Документы и коммиты

* `AGENT_A5_STAGES.md`: §2 — A5.5 закрыт; новый §3.5 — факты (RPC, `merge_log`,
  пересборка, сообщение, маршруты, панель, тесты, E2E); §4.1 — ответы В-A5.5-1…6;
  §4.2 — п.4 (RPC меток) и п.5 (`merge_log`) **закрыть**, п.6 (точный счётчик) оставить
  с пометкой «фаза 2, в панели — формы + файлы»; §6 — новые грабли; §7 — команды
  маршрутов, тестов и `a55_e2e.ps1`.
* `README.md` §20 — пункт «Метки проекта»: таблица, «Объединить» (старшая метка,
  псевдонимы, пересборка образов, сообщение агенту), «Разъединить» (только `merge_log`),
  «Не скрывать»; маршруты `GET /api/agent/labels`, `POST /api/agent/labels/merge|unmerge`
  и коды.
* Коммиты (git-identity не настроена — `git -c user.name=… -c user.email=… commit -F`):
  1. `feat(agent): A5.5 — метки проекта, склейка и разъединение`;
  2. `docs(agent): A5.5 закрыт — журнал под-этапов, README §20 и отчёт стенда`.
  В сообщениях — что проверено (fmt/clippy/тесты/python/npm/E2E).


## 5. Границы под-этапа

**Делать:** `merge_log`, `unmerge_entities`, `vault_project_labels`, `labels::report`,
 `refresh_label_images`, `MessageKind::{LabelsMerged, LabelsUnmerged}` + `append_message`,
 маршруты `/api/agent/labels*`, панель «Метки проекта», тесты и E2E.

**Не делать:**
* автоматическую склейку частичных упоминаний (план: фаза 2) — только ручная;
* точный счётчик вхождений (В-A5-2) — формы + число файлов; счётчик — фаза 2;
* «Разъединить» для склеек, сделанных **до** A5.5 (нет `merge_log`) — в интерфейсе
  честный текст, не «тихая» ошибка;
* журнал решений, «Вернуть» из backup, настройки агента, «Очистить историю» (A5.6);
* доставку `messages.jsonl` в сессию агента (A5.6) — A5.5 только пишет очередь;
* правки `Mirror::sync_once`/демона и `sidecar/hds_agent` (Node);
* Word-compare и ячеечный diff (A5.4 долг), живой сценарий §7 (A5.7).

## 6. Ловушки (свежие, оплачены)

1. **Не повышать `SCHEMA_VERSION`.** `_migrate_if_needed` при другой версии
   **переименует** базу и создаст новую — метки пропадут (D4). Новая таблица —
   `CREATE TABLE IF NOT EXISTS` в `_init_schema`, версия остаётся `2`.
2. **`merge_entities` не удаляет формы младшей сущности** — только `labels`-строку.
   На этом стоит «Разъединить»: после unmerge формы уже на месте, удалять надо лишь
   **скопированные** в старшую (и не входившие в её `forms_before`).
3. **`aliases` — проектные** (`project_id`), не глобальные. Деанонимизация старой
   метки работает через `_project_entity`; `snapshot` aliases **не отдаёт** — не
   рассчитывать на них в фильтре.
4. **Типы не смешивать.** `PERSON` и `SURNAME` — разные типы (`label_of`), merge
   отвергает. UI обязан проверить до запроса, иначе пользователь получит VaultError.
5. **Один Python-воркер на UI.** `with_mirror` A5.4 поднимает `LazyAnonymizer`; для
   склейки нужен `AgentApi::client` (иначе второй GLiNER и лишняя VRAM, A5.4 §6 п.12).
   Замок клиента — только на `merge_entities` / пересборку, не на HTTP-ожидание.
6. **`rebuild_image` — private.** Добавлять `refresh_label_images` в `results.rs`
   (тот же модуль), а не копировать код и не открывать внутренний метод наружу.
7. **Варианты не пересобирать.** У `pending_accept`/`conflict`/`agent_changed` в
   `pending` лежит правка агента — пересборка её потеряет. Такие строки — в `skipped`.
8. **Метки — только `hds_core::labels`** (`find_labels`, `label_parts`). Своя
   регулярка меток в UI/ядре запрещена (AG-1, контракт §5).
9. **docx-заголовки не извлекаются** (`hds_extract`, A5.4 §6 п.33): «где встречается»
   считается по телу; в панели это подписано, а не выдаётся за полный счётчик.
10. **`messages.jsonl`: одна запись — одна строка** (A4.6). Текст сообщения — только
    метки и пути; ни значений, ни их фрагментов (I8). `rel` для меток — пустая строка.
11. **Стенд меняет vault.** E2E обязан либо сделать `unmerge`, либо восстановить
    `anonymize.db` из бэкапа в `finally`; иначе следующий прогон не найдёт пару меток.
12. **`keep` не удаляется через API** (только add/list). Для E2E — вымышленный термин,
    не из документов; иначе стенд «загрязнится» реальным значением.
13. **PowerShell 5.1**: `2>&1` при `$ErrorActionPreference='Stop'` превращает
    предупреждения Python в ошибку (A5.4 §6 п.34); `python -c` теряет кавычки (п.35);
    кириллица в командной строке портится — тексты в JSON-файле сценария.
14. **Разделители путей:** реестр — `\`, JSON/маршруты — `/` (A5.4 §6 п.33).
15. **Сокет-тесты:** `install_global` + `reset_for_tests()` в начале, порядок тестов в
    бинаре не задан (A5.3 §6 п.32).
16. **`GET /api/agent/labels` с 46 метками и 34 образами** — это чтение + извлечение
    текста; для 34 файлов терпимо, но не дёргать маршрут на каждое событие SSE.
17. **Ответы API содержат значения** (формы меток) — это зона пользователя (так же
    `snapshot`/`preview`); в журналы, аудит и `sync.log` они не попадают.


## 7. Окружение и команды

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager log -3 --oneline; git --no-pager status --short --branch
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p hds-anonymize; cargo test -p hds-agent; cargo test -p hds-ui; cargo test -p hds-cli
cargo test --workspace -- --test-threads=4 *> tools\parity\out\a55-cargo-test.log
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

Стенд UI — тот же, что у A4/A5.4 (`tools\parity\out\agent_a42`):

```powershell
$env:HDS_CONFIG='<repo>\tools\parity\out\agent_a42\config.yaml'
$env:HDS_ROOT='<repo>\tools\parity\out\agent_a42\root'
target\debug\hds.exe ui --port 8799
# метки проекта (формы — значения, зона пользователя; кириллица в ответе — UTF-8)
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/labels?project=P0001'
# склейка/разъединение: CSRF-заголовок обязателен, тело — UTF-8 байтами
$h=@{'X-HDS-UI'='1';'Content-Type'='application/json'}
$body=@{project='P0001';keep='[PERSON_1]';drop=@('[PERSON_2]')}|ConvertTo-Json
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/labels/merge' -Method Post -Headers $h `
    -Body ([Text.Encoding]::UTF8.GetBytes($body))
Remove-Item Env:HDS_ROOT, Env:HDS_CONFIG -ErrorAction SilentlyContinue   # перед cargo test
```

CLI-путь (тот же `vault.merge_entities`; после A5.5 появится `merge_log`):

```powershell
target\debug\hds.exe anon merge P0001 "[PERSON_1]" "[PERSON_2]"
# «Разъединить» из CLI — отдельная команда не обязательна, но vault-метод должен быть
```

Тесты A5.5:

```powershell
cargo test -p hds-agent labels::            # report/occurrences, refresh, message
cargo test -p hds-agent results::           # MessageKind, append_message
cargo test -p hds-anonymize --test client   # project_labels на mock-воркере
cargo test -p hds-ui --test agent_labels    # сокет: GET labels, merge, unmerge, keep
sidecar\venv-anonymize\Scripts\python.exe -m unittest sidecar.tests.test_anonymize_vault -v
```

E2E — `tools\parity\out\agent_a42\a55_e2e.ps1` (по образцу `a54_e2e.ps1`), отчёт
`A55_E2E.md` в каталоге стенда. Вспомогательный `a55_probe.py` (stdlib): чтение текста
docx без python-docx (`zipfile` + `word/document.xml`) и `Vault.project_value` для
проверки псевдонимов. Черновики проб уже лежат в каталоге стенда:
`a55_probe_labels.py` (46 меток `P0001`, формы) и `a55_probe_pairs.py` (какие метки
встречаются совместно в образах) — их можно взять за основу.

## 8. Вопросы владельцу (предложены дефолты)

| # | Вопрос | Дефолт |
|---|---|---|
| В-A5.5-1 | Какая метка «старшая» при склейке | Метка с наибольшим числом файлов в `occurrences`; при равенстве — первая выбранная. `confirm()` показывает выбор; ручной выбор «оставить» — фаза 2 |
| В-A5.5-2 | Область «Разъединить» | Только склейки из `merge_log` (A5.5+). Для старых — надпись «склейка до A5.5 необратима» (В-A5-3) |
| В-A5.5-3 | «Число упоминаний» | Формы значения + число файлов реестра, где метка есть в образе (В-A5-2); точный счётчик вхождений — фаза 2 |
| В-A5.5-4 | Что уходит агенту | `messages.jsonl`: `labels_merged`/`labels_unmerged`, только метки; `notice` в ленту. Доставка в сессию — A5.6 |
| В-A5.5-5 | Что делать с вариантами при склейке | Не пересобирать `pending_accept`/`conflict`/`agent_changed` — вернуть их в `skipped`; пользователь решает по «Результатам» (A5.4) |
| В-A5.5-6 | «Не скрывать» в панели | Глобальный `keep_terms` (D19), кнопка у каждой формы, подпись «действует во всех проектах»; метка при этом не удаляется (новые значения метку не получат) |

## 9. Срез на 20.11.2026 (для проверки в новом чате)

* Ветка `agent-privacy-zone`, HEAD `fb699982` (+docs этого файла), ahead 447, дерево чистое.
* `cargo test --workspace -- --test-threads=4` — 126 наборов / 899 passed / 0 failed /
  13 ignored; python 328 OK (skip 1); `npm test` 23/23; fmt/clippy чисто.
* Стенд `agent_a42`: 34 записи реестра, `pending` пуст, `A51_E2E.md`…`A54_E2E.md`,
  сценарии `a51_e2e.ps1`…`a54_e2e.ps1`; `P0001` — 46 меток, `aliases` пуст.
* Вкладка умеет: проекты/состояние/файлы, реплику с предпросмотром и метками, вложения,
  отправку хода, «Стоп», ленту SSE с рассуждениями/инструментами/ответом, вопрос агента,
  панель «Результаты» с решениями. Чего нет: панели «Метки проекта» (этот под-этап),
  настроек и «Очистить историю» (A5.6).


## 10. Промт (копировать целиком)

```text
Ты — ведущий разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Это продолжение основного проекта «ИИ-агент в анонимной зоне»
(PLAN_AGENT_PRIVACY_ZONE.md). Ветка agent-privacy-zone, HEAD — docs-коммит A5.4
(сверить git log -1 --oneline), дерево чистое, push не делался.

Фазы A0–A4 закрыты (A4 — двустороннее зеркало, отчёт AGENT_A4_REPORT.md).
Фаза A5 (вкладка «Агент») идёт под-этапами: план, решения
(В-A5-1…8, В-A5.2-1…6, В-A5.3-1…5, В-A5.4-1…6), долги, контракты и грабли —
AGENT_A5_STAGES.md (§1, §2, §3.1–§3.4, §4–§7).
A5.1 закрыт: проекты, состояние зеркала и файлы реестра.
A5.2 закрыт: реплика (предпросмотр «что увидит модель», «Скрыть ещё…»,
«Не скрывать», вложения, отправка хода, «Стоп»).
A5.3 закрыт: лента событий на SSE, переключатель «как видит модель»,
вопрос агента ask_user и POST /api/agent/answer.
A5.4 закрыт: панель «Результаты» — pending/compare/decision, отчёт
валидатора, «Сравнить», «Принять» (backup), «Отклонить», три исхода
конфликта, «файл занят»; A54_E2E.md.
Контекст и методика этого под-этапа — AGENT_A5_5_HANDOFF_PROMPT.md (§3–§9),
этот промт — её §10.

Задача: под-этап A5.5 — панель «Метки проекта» и склейка меток. Настройки и
«Очистить историю» — A5.6, живой сценарий §7 — A5.7.

1. Прочитай AGENT_A5_5_HANDOFF_PROMPT.md целиком, затем AGENT_A5_STAGES.md
   (§2, §3.1–§3.4, §4.1, §4.2, §5–§7) и PLAN_AGENT_PRIVACY_ZONE.md §5.2
   (TR-1.5, TR-1.6, TR-2.2, TR-2.6), §5.8 (TR-8.2), §5.9 (инварианты), §7.
2. Python (sidecar/hds_anonymize): таблица merge_log (аддитивно, IF NOT EXISTS,
   SCHEMA_VERSION не повышать — иначе D4 переименует базу и потеряет метки);
   merge_entities пишет merge_log (dropped + forms_before) и возвращает
   merge_id; unmerge_entities(project_id, merge_id) возвращает младшие метки и
   удаляет только перенесённые формы старшей; RPC vault_project_labels
   (метки с формами, aliases, merges, keep) в vault.py/handlers.py/worker.py
   (+ capabilities); тесты в sidecar/tests/test_anonymize_vault.py.
3. Клиент hds-anonymize: protocol ProjectLabel/MergeRecord/ProjectLabels,
   client.project_labels; mock-воркер и клиентский тест — обновить.
4. Ядро hds-agent: модуль labels.rs — LabelsReport (метки, forms, aliases,
   occurrences: файлы реестра, где метка есть в образе; текст через
   validator::read_text, метки через hds_core::labels); в results.rs —
   Mirror::refresh_label_images(dropped) поверх rebuild_image (только
   Synced/Accepted, остальные в skipped) и MessageKind::LabelsMerged/
   LabelsUnmerged + публичный append_message (messages.jsonl, только метки).
5. Маршруты hds-ui: GET /api/agent/labels?project=,
   POST /api/agent/labels/merge {project, keep, drop},
   POST /api/agent/labels/unmerge {project, merge_id}; «Не скрывать» —
   существующий /api/agent/keep. Под agent::gate и CSRF; в server-профиле 403;
   после склейки — пересборка, один Mirror::sync_once, сообщение и notice в
   ленту. Один Python-воркер на UI — AgentApi::client, не LazyAnonymizer.
6. Панель «Метки проекта» в page.rs: таблица «метка | значение (формы) |
   где встречается», «Не скрывать» у формы, «Объединить выбранные» (один тип,
   старшая — максимум файлов, confirm), блок склеек с «Разъединить»
   (только merge_log). После склейки/разъединения обновить панель и «Файлы».
7. Не делать: автоматическую склейку (фаза 2), точный счётчик вхождений,
   «Разъединить» для склеек до A5.5, настройки и «Очистить историю» (A5.6),
   доставку messages.jsonl в сессию, правки демона/зеркала и sidecar/hds_agent,
   Word-compare.
8. Тесты: python (schema/merge_log/unmerge/project_labels), клиент на
   mock-воркере, unit hds-agent (labels::report, refresh, append_message,
   MessageKind), unit hds-ui (разбор тела merge/unmerge → коды), интеграционные
   через реальный сокет (run_http_listener на порту 0, образец
   crates/hds-ui/tests/agent_results.rs, install_global + reset_for_tests):
   GET labels, merge (образ пересобран, messages.jsonl, merges), unmerge,
   keep, 400/404/409, 403 без CSRF и в server-профиле; CLI hds anon merge не
   сломан (cargo test -p hds-cli).
   Проверки: cargo fmt --all --check, clippy --workspace --all-targets --
   -D warnings, cargo test --workspace -- --test-threads=4, npm test,
   python unittest.
9. E2E стенда agent_a42: a55_e2e.ps1 — sync, GET labels, выбор пары одного
   типа (keep = максимум файлов), merge → образы пересозданы (младшей метки
   нет, старшая есть), messages.jsonl labels_merged, повторный sync —
   changes=0, деанонимизация старой метки через aliases; unmerge → метки
   вернулись, labels_unmerged, sync changes=0; «Не скрывать» (вымышленный
   термин); значений в служебных артефактах нет; бэкап anonymize.db и
   восстановление в finally; отчёт A55_E2E.md.
10. Коммиты: feat(agent): A5.5 — ...; docs(agent): A5.5 закрыт — ...
    (+ AGENT_A5_STAGES.md §2, §3.5, §4.1, §4.2, §6, §7; README §20 — про
    панель «Метки проекта»). В сообщении — что проверено.

Проверка среза: ветка agent-privacy-zone, HEAD fb699982 (+docs этого файла),
ahead 447, дерево чистое; cargo test --workspace — 126 наборов / 899 passed /
0 failed / 13 ignored; python 328 OK (skip 1); npm test 23/23; стенд agent_a42 —
34 записи реестра, pending пуст, отчёты A51_E2E.md…A54_E2E.md, 46 меток P0001.
```

