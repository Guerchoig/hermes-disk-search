# AGENT_A5_4_HANDOFF_PROMPT.md — контекст и промт для под-этапа A5.4 («Результаты» и решения)

> **Кому:** новый чат Cline в режиме Act. **Дата среза:** 20.11.2026.
> **Ветка:** `agent-privacy-zone`, **HEAD:** `fd2852b2` (`docs(agent): A5.3 закрыт`);
> ahead 76 + docs этого файла, дерево чистое, push не делался.
> **Что это:** самодостаточный контекст для продолжения **фазы A5** основного проекта
> `PLAN_AGENT_PRIVACY_ZONE.md` («ИИ-агент в анонимной зоне»). A0–A4 закрыты, A5 идёт
> под-этапами (`AGENT_A5_STAGES.md`), **A5.1, A5.2 и A5.3 закрыты**. Этот файл — A5.4:
> **панель «Результаты»** (варианты агента, отчёт валидатора, «Сравнить», «Принять»,
> «Отклонить», три исхода конфликта). Метки проекта — A5.5, настройки и «Очистить
> историю» — A5.6, живой сценарий §7 — A5.7.

## 0. Где мы (коротко)

Пользователь работает с документами в своей папке, агент — только с анонимными образами
в `<agent.root>\P<NNNN>\workspace`; исходные значения не покидают машину (I1), оригиналы
не меняются без решения пользователя (I6), в журналах нет значений (I8).

* **A4** — двустороннее зеркало `hds agent sync` — закрыта (`AGENT_A4_REPORT.md`):
  варианты в `<папка проекта>\.hds-agent\pending\`, отчёт валидатора рядом, решения
  (`hds agent result accept|reject|conflict`), backup, `_conflicts`, `messages.jsonl`.
* **A5.1** — каркас вкладки и API состояния (`/api/agent/projects|state|files`).
* **A5.2** — реплика: `preview` («что увидит модель»), `label`, `keep`, `attach`,
  `message`, `abort`, `stop`; `agent_session` (сессии, кольцо 500 событий, насос).
* **A5.3** — лента: SSE (`GET /api/agent/events`, `text/event-stream`), `ask_user`
  (`POST /api/agent/answer`), переключатель «как видит модель», деанонимизация вопроса
  в шлюзе (`A53_E2E.md`).

**Зачем A5.4.** Решения сейчас доступны только из CLI (`hds agent result …`): пользователь
видит в панели «Файлы проекта» статусы `pending_accept`/`conflict`, но не может ни
посмотреть вариант, ни сравнить, ни принять — и не видит отчёт валидатора (`verdict`,
`checks`), который зеркало уже пишет рядом с каждым вариантом. A5.4 закрывает это в
вкладке: панель «Результаты», `Mirror::compare` (долг A4 §4.2 п.17), маршруты
`/api/agent/pending|compare|decision`.

## 1. Состояние репозитория (проверено 20.11.2026)

| Что | Значение |
|---|---|
| Ветка / HEAD | `agent-privacy-zone`, `fd2852b2` (docs A5.3); коммиты A5.3: `a4cab75e` (feat), `fd2852b2` (docs) |
| Тесты | `cargo test --workspace -- --test-threads=4` — **125 наборов / 889 passed / 0 failed / 13 ignored**; python 328 OK (skip 1); `npm test` 23/23 |
| fmt / clippy | чисто (`cargo fmt --all --check`, `clippy --workspace --all-targets -- -D warnings`) |
| Новые наборы A5.3 | `hds-agent` unit (2 gateway + 1 deanon), `hds-ui` unit (4 `agent::`, 3 `agent_session::`), `hds-ui/tests/agent_chat.rs` (9, из них 4 новых) |
| Стенд | `tools\parity\out\agent_a42`: 34 записи реестра, `pending` пуст (A4 всё принял/отклонил); `config.yaml`, `project\`, `zone\` |
| Отчёты | `A51_E2E.md`, `A52_E2E.md`, `A53_E2E.md`; сценарии `a51_e2e.ps1`…`a53_e2e.ps1` |

**Известная флейка:** `hds-mcp/tests/quota_socket` под параллельной нагрузкой получает
503 (`max_conns` 32) — одиночный прогон зелёный, к правкам A5 не относится.

## 2. Обязательное чтение (по порядку)

1. `AGENT_A5_STAGES.md` — §2 (строка A5.4), §3.1–§3.3 (что уже сделано), §4.1 (решения
   В-A5-1…8, В-A5.2-1…6, В-A5.3-1…5), §4.2 (долги 1–23), §5 (контракты), §6 (грабли 1–32),
   §7 (окружение).
2. `PLAN_AGENT_PRIVACY_ZONE.md` — §5.7 (TR-7.1…7.9: реестр, варианты, четыре ситуации,
   «Сравнить», валидатор), §5.8 (TR-8.2 — панель «Результаты»), §5.9 (инварианты), §7
   (приёмка: «Принять» → backup).
3. `AGENT_A4_STAGES.md` — §3.4 (что сделано в решениях), §4.2 п.17–20 (долги: «Сравнить»,
   «Вернуть», журнал решений, конфликт без варианта), В-7 («Сравнить» docx), В-6
   (`messages.jsonl`).
4. `AGENT_A4_REPORT.md` — что «снаружи» уже умеет CLI и какие артефакты остались.
5. `crates/hds-agent/src/results.rs` — целиком: `Mirror::decide`, четыре ситуации,
   `DecisionReport`, `ResultsError` (готовые тексты для пользователя), `normalize_rel`.
6. `crates/hds-agent/src/validator.rs` — `Report`, `Check`, `Verdict`, `Report::read`.
7. `crates/hds-agent/src/mirror.rs` — `Mirror::new/from_config`, `Anonymizer`/`TextExtractor`,
   `LazyAnonymizer`/`LazyExtractor`, `pending_path`/`base_path` (сейчас `pub(crate)`),
   `sha256_file`, `join_segments`, сборка варианта и вызов валидатора (`:1435`).
8. `crates/hds-cli/src/cmd/agent_result.rs` — образец: как собрать `Mirror` по конфигу,
   как ошибки превращаются в текст, `--choice`.
9. `crates/hds-ui/src/agent.rs` и `agent_session.rs` — куда добавлять маршруты и состояние
   (A5.3: `answer_json`, `stream_events`, `AskState`); `crates/hds-ui/src/page.rs` — секции
   вкладки и `jget/jpost`.

## 3. Что уже готово — опора A5.4

| Что | Где | Чем пользоваться |
|---|---|---|
| **Решения** | `crates/hds-agent/src/results.rs` | `Mirror::decide(rel, Decision) -> Result<DecisionReport, ResultsError>`; `Decision::{Accept, Reject, Conflict(ConflictChoice)}`, `ConflictChoice::{Agent, New, Delegate}` (есть `parse`/`as_str`); `DecisionReport{orig_rel, anon_rel, status, backup, conflict_copy, message, warnings}`; `ResultsError` — **готовый текст для пользователя** (Display); `normalize_rel`; константы `SERVICE_DIR=".hds-agent"`, `BACKUP_SUBDIR="backup"`, `CONFLICTS_DIR="_conflicts"`, `MESSAGES_FILE="messages.jsonl"`, `CONFLICT_SUFFIX=" (вариант агента)"` |
| **Четыре ситуации** | там же | `accept` (sha256 оригинала = основание → backup → замена → `accepted` → пересоздать образ), `reject` (образ из оригинала + сообщение агенту), `conflict` с `agent`/`new`/`delegate`, `Busy` (os error 32) — вариант остаётся в `pending` |
| **Отчёт валидатора** | `crates/hds-agent/src/validator.rs` | `Report{rel, anon_rel, ts, rev, verdict, checks}`, `Check{name, verdict, detail}`, `Verdict::{Ok, Check}`, `Report::read(variant)`, `report_path(variant)` = `pending\<rel>.report.json`, имена проверок `CHECK_LABELS/CHECK_GRAMMAR/CHECK_IMAGES/CHECK_TEXT_LOST`; отчёт пишется при сборке варианта (`mirror.rs:1435`) |
| **Зеркало** | `crates/hds-agent/src/mirror.rs` | `Mirror::new(project_id, project_root, workspace, &registry, &mut anonymizer, &mut extractor, sync, max_file_mb)`, `Mirror::from_config(cfg, id, project_root, &registry, &mut anon, &mut ext)`; трейт `Anonymizer` — **8 методов без дефолтов** (`anonymize_path`, `deanonymize_path`, `anonymize_agent`, `text_image`, `read_marker`, `image_map`, `snapshot`, `deanonymize_agent`), трейт `TextExtractor{extract}`; `LazyAnonymizer`/`LazyExtractor` — готовые обёртки над фабриками; `sha256_file`, `join_segments(&ExtractResult)`; `pending_path`/`base_path` — **`pub(crate)`** (нужен публичный доступ для панели) |
| **Реестр** | `crates/hds-agent/src/registry_db.rs` | `ArtifactRegistry::{open, list, list_pending, list_by_status, by_orig, count, set_status}` — **`list_pending()` отдаёт только `PendingAccept`**, конфликты — `list_by_status(Status::Conflict)`; `Artifact{id, orig_rel, anon_rel, kind, orig_sha256, anon_sha256_issued, anon_sha256_seen, status, ts}`; `Status::{PendingAccept, Conflict, AgentChanged, Accepted, Orphan, …}`, `Kind` |
| **Воркеры** | `crates/hds-agent/src/factory.rs`, `mirror.rs` | `anonymizer_factory(cfg, root)`, `extractor_factory(root)`; **`hds_anonymize::Client` сам реализует `Anonymizer`** (`mirror.rs:353`) — отдельный адаптер не нужен: `&mut *client.lock()?`; в UI уже есть `AgentApi::client(cfg, state_dir) -> Arc<Mutex<Client>>`; `run::open_client_at` |
| **CLI-образец** | `crates/hds-cli/src/cmd/agent_result.rs` | порядок «реестр → LazyAnonymizer/LazyExtractor → `Mirror::from_config` → `decide`», `describe(err)` (занятый файл), `print_report` |
| **Маршруты вкладки** | `crates/hds-ui/src/{agent,lib}.rs` | `agent::gate()` (403 в server-профиле, D20), `err/ok/api_err`, CSRF-гейт, `is_project_id`, образец `files_json`/`events_json`; SSE-ветка в `run_http_listener` — **не ломать** |
| **Состояние вкладки** | `crates/hds-ui/src/agent_session.rs` | `AgentApi::{global, install_global, client, existing_turn, push, state, events_json}`, `Turn::{is_busy, wait_for_change}`, `ApiError{status, message}`; кольцо событий → лента (SSE) |
| **Вкладка** | `crates/hds-ui/src/page.rs` | секции «Проекты», «Реплика», «Состояние проекта», «Файлы проекта», «Новый проект»; `jget/jpost`, `esc`, `agLine`/`agentCopyPath` (A5.3) |
| **Стенд** | `tools\parity\out\agent_a42` | 34 записи реестра, `project\`, `zone\`, `config.yaml`, mock-провайдер, `a53_e2e.ps1`; рецепт варианта — `A44_E2E.md`: `office_edit` образа → `hds agent sync --project P0001 --once` → `pending_accept` + отчёт |

**Чего в A5.4 не делать:** «Метки проекта» и склейку (A5.5), настройки, «Очистить историю»,
уведомления фильтра как экран, доставку `messages.jsonl` в сессию, журнал решений и
«Вернуть» из backup (A5.6/A6), Word-compare, живой сценарий §7 (A5.7).

## 4. Задачи A5.4 с методикой

### 4.1. `Mirror::compare` в `hds-agent` (делать первым)

Почему: долг A4 §4.2 п.17 — `DecisionReport` отдаёт только пути, а TR-7.6/В-7 обещают
«данные для сравнения». Word-compare (COM) — не в MVP (В-7: «данные — в A4, запуск
Word-сравнения — A5»); в A5.4 нужен **минимальный объём** (В-A5-4): метаданные, отчёт
валидатора и текстовый diff по тому, что уже умеет `hds_extract`.

Добавить в `crates/hds-agent/src/results.rs` (рядом с `decide`, тот же `impl Mirror`):

```rust
/// Данные для «Сравнить» (TR-7.6, В-7): пути, размеры, sha256, отчёт валидатора и
/// текстовый diff. Запуск Word-сравнения — вне MVP (В-A5-4).
pub struct CompareReport { … }        // Serialize

impl<'a> Mirror<'a> {
    pub fn compare(&mut self, rel: &str) -> Result<CompareReport, ResultsError>;
}
```

Состав (имена — snake_case, как у остальных API ядра):

```
rel, anon_rel, status ("pending_accept" | "conflict"), kind
original { path, size, sha256, mtime }        // <папка проекта>\<rel>
variant  { path, size, sha256 } | null        // .hds-agent\pending\<rel>
base     { path, size, sha256 } | null        // .hds-agent\base\<rel>
image    { path, size, sha256 } | null        // <P>\workspace\<anon_rel>
conflict_copy { path, size } | null           // _conflicts\<имя> (вариант агента).<ext>
report   { verdict, checks[] } | null         // validator::Report::read(variant)
diff     { kind: "text", text: "…", lines: [{"op":" ","-","+"}], truncated: bool }
       | { kind: "counts", base_lines, variant_lines }
warnings [ "…" ]                              // без значений (I8)
```

Правила:

* пути — **только** через существующие хелперы зеркала: `pending_path`/`base_path` сейчас
  `pub(crate)` → добавить публичный метод (`Mirror::variant_path(rel)` / `Mirror::base_path_of(rel)`)
  или свободные функции в `results` (`variant_path(project_root, rel)`), но **не** склеивать
  `.hds-agent\pending\…` в UI — путей служебной папки не должно быть в двух местах;
* статусы, на которых «Сравнить» осмысленно: `PendingAccept`, `Conflict`, `AgentChanged`
  (вариант собирается — сравнение покажет основание/образ); иначе `ResultsError::WrongStatus`
  (`Orphan` — образа нет, сравнивать нечего);
* `NoVariant` — если варианта нет в `pending` (конфликт без собранного варианта, A4 §4.2 п.20):
  отдавать `ResultsError::NoVariant`, а `diff` не строить;
* diff: `TextExtractor::extract` даёт `String` (`join_segments`), diff — построчный.
  Зависимостей **не добавлять** (ни `similar`, ни `diff`): простой LCS/«общий префикс +
  общий суффикс» на строках, лимиты — 200 строк результата и 2 000 символов на строку,
  `truncated=true`, если обрезали. Для docx/xlsx `Segment{text, page, head}` — тоже строки:
  `head` (лист/страница) добавлять в префикс строки, чтобы diff был читаемым;
* `report` — как есть (`Verdict::Ok|Check`, `check.detail` уже без значений);
* если extractor недоступен/упал — не ошибка: `diff.kind="counts"` (число строк по
  размеру/сегментам) + `warnings` с причиной;
* тест (unit, `mod tests` в `results.rs`, стенд `mirror::tests::{Stand, FakeAnon, FakeExtractor}`):
  md/txt — изменённая строка видна как `-`/`+`; docx (FakeExtractor) — `counts` или
  `text`-diff; нет варианта → `NoVariant`; статус `synced` → `WrongStatus`; лимит строк
  (`truncated`).

### 4.2. `GET /api/agent/pending?project=` — что показывает панель

`crates/hds-ui/src/agent.rs`:

```rust
/// `GET /api/agent/pending?project=P0001` — варианты, ждущие решения (TR-7.6).
pub fn pending_json(cfg: &Config, project: &str) -> Reply;
```

Источник — **только реестр**: `ArtifactRegistry::open(&settings::registry_db(cfg, id))`
(нет БД — `200 {count:0, items:[]}`, как `files_json`), затем
`list_pending()` (**только `pending_accept`**) **плюс** `list_by_status(Status::Conflict)`.
Отсортировать по `orig_rel`. Для каждого:

```
rel, anon_rel, kind, status, ts
verdict ("ok" | "check" | null), checks_count
has_variant (pending\<rel> — файл), has_base, has_conflict_copy
```

`verdict/checks_count` — из `validator::Report::read(variant)`; отчёта нет (варианты A4
до валидатора) — `null`, панель пишет «отчёта нет». Значения не читаются: отчёт и статусы
значений не содержат (I8), `rel`/`anon_rel` — пути (зона пользователя).

Маршрут — в `route_full` рядом с `/api/agent/files`, под `agent::gate()`. Тест: пусто;
`pending_accept` + отчёт `check`; `conflict` попадает в список; незарегистрированный
проект — 404 (как `files_json`), без БД — нули.

### 4.3. `POST /api/agent/decision` — принять/отклонить/конфликт

Сначала в `results.rs` добавить разбор решения из строки (симметрично `ConflictChoice::parse`):

```rust
impl Decision {
    /// `accept` / `reject` / `conflict` (`choice` — отдельным полем запроса).
    pub fn parse(s: &str) -> Option<Decision>;
}
```

Маршрут: `POST /api/agent/decision {project, rel, decision, choice?}` — `choice` обязателен
для `conflict` (`agent`|`new`|`delegate`), иначе 400. Реализация — `agent::decision_json` +
`AgentApi::decide(cfg, project, rel, decision) -> Result<Value, ApiError>`:

1. `zone(cfg)`, `require_project`, `settings::registry_db` (нет — 503 «зона не настроена»);
2. `ArtifactRegistry::open(&db)` — на запрос (WAL, дешево; как `files_json`);
3. воркеры — **адаптер не нужен**: `hds_anonymize::Client` уже реализует трейт `Anonymizer`
   (`mirror.rs:353`). Берём живой клиент UI и отдаём его зеркалу по ссылке:
   ```rust
   let client = self.client(cfg, &state_dir)?;          // Arc<Mutex<Client>>
   let mut guard = client.lock().map_err(|_| ApiError::new(503, "воркер анонимизации занят"))?;
   let mut extractor = LazyExtractor::new(extractor_factory(&self.root));
   let mut mirror = Mirror::from_config(cfg, &rec.id, Path::new(&rec.path),
                                       &registry, &mut *guard, &mut extractor)?;
   ```
   Замок клиента держится на время решения (секунды у «Принять»); это осознанно: второй
   GLiNER не поднимается. `LazyExtractor` поднимает воркер извлечения лениво и только
   для чтения текста (docx/xlsx/pdf) при «Сравнить» (idle 600 с). `state_dir` —
   `settings::state_dir_for(cfg, &rec.id)`. Альтернатива без общего клиента —
   `LazyAnonymizer::new(anonymizer_factory(cfg, &self.root))` (как CLI), но это второй
   воркер (VRAM). Выбор зафиксировать в §3.4 и В-A5.4-7;
4. `mirror.decide(&rel, decision)` — **все проверки A4 остаются в ядре**: sha256 оригинала,
   `Kind`, наличие варианта, занятость файла, статус;
5. ответ: `{ok:true, report:{orig_rel, anon_rel, status, backup, conflict_copy, message, warnings}}`
   (пути — как есть; `warnings` без значений). Дополнительно — `notice` в кольцо хода
   (`turn.push("notice", …)`, A5.3), чтобы лента показала «документ принят/отклонён»;
6. ошибки — по таблице ниже. Текст `ResultsError` уже человеческий: отдавать его в
   `{"error": "…"}`, UI показывает как есть (не переписывать).

| `ResultsError` | HTTP | Почему |
|---|---|---|
| `NotFound` | 404 | нет записи реестра |
| `WrongStatus{status}` | 409 | решать нечего (повторное «Принять» после `accepted` — тоже сюда) |
| `NoVariant` | 409 | варианта нет в `pending` |
| `OriginalChanged` | 409 | оригинал изменился — только через конфликт (текст это и говорит) |
| `NothingToDelegate` | 409 | делегировать нечего |
| `Busy(path)` | 409 | «файл занят» — текст A4 как есть (В-A5.4-2) |
| `BadPath` | 400 | `rel` не относительный путь проекта |
| `Kind(kind)` | 409 | вид артефакта не допускает замены (текстовый образ) |
| `Io/Registry/Mirror` | 500 | сбой, не решение пользователя |

Тесты: accept (оригинал = вариант, backup есть, образ пересоздан, `pending` пуст,
`status=accepted`), reject (образ из оригинала, сообщение в `messages.jsonl`),
conflict×3, «файл занят» (держать `File` открытым → 409, вариант на месте, повтор после
закрытия — 200), повтор после accept → 409, без `choice` → 400, без CSRF → 403,
server-профиль → 403.

### 4.4. «Сравнить» в UI — минимальный объём

`GET /api/agent/compare?project=&rel=` → `mirror.compare`. Панель показывает:

* **метаданные** — пути (оригинал/вариант/основание/образ), размеры, sha256 (первые 12
  символов достаточно), статус, вид;
* **отчёт валидатора** — `verdict` бейджем («проверить» — жёлтым, `ok` — зелёным) и
  таблица `check.name / verdict / detail`;
* **diff** — блок `<pre>` с строками `-`/`+`/` ` (до 200 строк, `truncated` — подпись
  «показаны первые 200 строк»), для `counts` — «строк: было N, стало M»;
* кнопка «скопировать путь» — переиспользовать `agentCopyPath` (A5.3); UI не запускает
  Word и не открывает файлы (В-A5-7).

### 4.5. Панель «Результаты» во вкладке

Секция после «Файлы проекта» (`page.rs`): таблица `вариант | статус | отчёт | действия`.

* статус: `pending_accept` — «вариант», `conflict` — «конфликт» (красным), `agent_changed`
  — «собирается» (варианта ещё нет, кнопки решения неактивны);
* отчёт: «проверить» / «ok» / «отчёта нет»;
* действия: «Сравнить…» (раскрывает блок сравнения в строке), «Принять» (`confirm`:
  «Оригинал будет заменён. Копия сохранена в backup — продолжить?»), «Отклонить»;
  при `conflict` — три кнопки: «Оставить вариант агента» (`choice=agent`, с предупреждением
  «правки оригинала после основания будут потеряны»), «Взять новую версию» (`new`),
  «Поручить агенту» (`delegate`, подпись «правки уйдут в `_conflicts`, агент получит
  сообщение»);
* «файл занят» — текст ошибки в строке красным, вариант остаётся, повтор после закрытия
  файла проходит;
* после решения — обновить «Результаты», «Файлы проекта» и строку статуса; ошибки — текстом
  сервера;
* пусто — «вариантов, ждущих решения, нет» + подсказка «правки агента появятся здесь после
  `hds agent sync`».

### 4.6. Что НЕ делать в A5.4

* «Метки проекта», склейка/разъединение, `vault_project_labels` — A5.5.
* Настройки, ключи, «Очистить историю», уведомления фильтра как экран — A5.6.
* Доставка `messages.jsonl` в живую сессию — A5.6 (A5.4 только пишет очередь через
  `results::decide`); журнал решений и «Вернуть» из backup — A5.6/A6 (долги A4 п.18–19).
* Word-compare (COM) и ячеечный diff xlsx сверх сегментов extractor'а — фаза 2.
* Правки демона/зеркала (`sync_once`) и воркера (`sidecar/hds_agent`) — не нужны.

## 5. Порядок работ и критерии приёмки

Порядок: **4.1** (`Mirror::compare` + тест) → **4.2** (`/pending`) → **4.3** (`/decision`
со всеми кодами) → **4.4** (`/compare`) → **4.5** (панель). Каждый шаг — зелёный тест и
коммит (или один feat в конце, если шаги связаны).

Готово, когда:

1. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace -- --test-threads=4` зелёные; `npm test` 23/23; python 328 OK.
2. **Тесты A5.4**:
   * unit `hds-agent`: `compare` (md/txt diff, `counts` без extractor'а, `NoVariant`,
     `WrongStatus`, лимит строк), `Decision::parse`;
   * unit `hds-ui`: маппинг `ResultsError` → `ApiError` (все варианты таблицы 4.3);
   * интеграционные через реальный сокет (`run_http_listener` на порту 0, образец —
     `crates/hds-ui/tests/agent_chat.rs`): `pending` отдаёт вариант и `verdict`;
     `decision accept` → 200, оригинал заменён, backup есть, образ пересоздан, `pending`
     пуст; `reject`; `conflict` три исхода; «файл занят» → 409 и вариант на месте;
     повтор решения → 409; 403 без CSRF и в server-профиле;
   * проверка, что `hds agent result …` (CLI) продолжает работать — он использует те же
     функции, отдельный прогон `cargo test -p hds-cli` обязателен.
3. **E2E стенда** `agent_a42` (`a54_e2e.ps1`, отчёт `A54_E2E.md`):
   * вариант создаётся честно: mock-провайдер `office_edit` по образу → `hds agent sync
     --project P0001 --once` → `pending=1`, `status=pending_accept`, отчёт рядом;
   * `GET /api/agent/pending` — вариант, `verdict`, `checks` (если отчёт `check` — видно,
     какая проверка сработала);
   * `GET /api/agent/compare` — пути/размеры/sha, diff по md или docx;
   * `POST /api/agent/decision accept` — оригинал = вариант, backup в
     `.hds-agent\backup\…`, образ пересоздан, `status=accepted`, `pending` пуст,
     повторный `sync --once` — 0 изменений;
   * `reject` на втором документе — образ вернулся к оригиналу, агенту сообщение;
   * «файл занят» — открыть оригинал на запись в сценарии, `accept` → 409 и текст
     «файл занят», вариант на месте, после закрытия — 200 (или хотя бы 409 с тем же
     текстом и вариант на месте, если файл держать не удалось);
   * значений в служебных артефактах нет (лог провайдера, `sync.log`, `a54-e2e.log`);
     в панели/сравнении значения есть — это зона пользователя.
4. Ключ провайдера не появляется ни в одном ответе API и ни в одном артефакте стенда.

## 6. Грабли (оплачены в A0–A5.3 — не наступать снова)

1. **`list_pending()` — только `PendingAccept`.** Конфликты берутся `list_by_status(Status::Conflict)`,
   иначе панель их «не видит».
2. **`pending_path`/`base_path` — `pub(crate)`.** Публичный хелпер добавлять в `hds-agent`
   (`results`), а не склеивать `.hds-agent\pending\…` в UI: путь служебной папки должен
   жить в одном месте (A4 §5).
3. **`Mirror` держит `&mut dyn Anonymizer/TextExtractor`.** Собирать «реестр → воркеры →
   Mirror → decide» в одном скоупе на запрос; `Mirror` не кешировать и не хранить в `AgentApi`.
4. **Проверки решений — в ядре.** Не дублировать sha256/`Kind`/занятость файла в UI:
   `ResultsError` уже содержит человеческий текст, UI его показывает.
5. **«Файл занят» — os error 32**, а не «нет прав»: `ResultsError::Busy`. Тест держит
   `std::fs::File` открытым на запись (Windows), после `drop` повтор проходит.
6. **`rel` из реестра — с `\`.** `normalize_rel` принимает и `/`; в JSON отдавать как в
   реестре, в запросе принимать оба.
7. **После `accept` — `sync --once` даёт 0 изменений** (критерий A4.4). Если появились
   изменения — сломана пересборка образа, а не «нормально».
8. **Отчёта может не быть** (варианты, собранные до валидатора): `Report::read` → `None`,
   панель пишет «отчёта нет», не падает.
9. **Конфликт без варианта** (A4 §4.2 п.20): `NoVariant` — «Принять»/«Оставить вариант
   агента» недоступны, доступны «Взять новую версию»/«Поручить агенту».
10. **Пути и значения.** `DecisionReport`/`Report`/`warnings` — без значений (I8); пути
    оригинала/варианта — зона пользователя (можно в ответе и в панели); в журналы/аудит
    (`sync.log`, `messages.jsonl`, `a54-e2e.log`) — только пути и метки.
11. **Зависимостей не добавлять.** Diff — руками (LCS/префикс-суффикс) с лимитами; иначе
    `Cargo.lock` и лицензии поедут (NOTICE.md).
12. **Второй GLiNER.** `AgentApi::client` уже держит Python-воркер; `hds_anonymize::Client`
    реализует `Anonymizer` сам — берите `&mut *client.lock()?`, а не `LazyAnonymizer`
    (второй воркер = лишняя VRAM). Замок клиента держится на время решения — это ок,
    но не заворачивайте в него HTTP-ожидание: только `Mirror::from_config`/`decide`.
13. **`Artifact.ts` — f64**, не целое: в JSON отдавать как есть, не приводить к `u64`.
14. **SSE-ветку не ломать** (A5.3): `/api/agent/events` — отдельная ветка `run_http_listener`
    до `route_full`; новые маршруты идут обычным путём (CSRF + `agent::gate()`).
15. **`OnceLock` состояния вкладки в тестах**: сокет-тесты — `install_global` +
    `reset_for_tests()` в начале (A5.3 §6 п.32), порядок тестов не задан.
16. **Занятый файл не лечится** повтором в том же запросе: только сообщением пользователю;
    никаких «подождать и попробовать снова» с блокировкой HTTP-потока.
17. **`hds ui` не запускает процессы** (В-A5-7): «Открыть»/«Сравнить» — показ пути,
    копирование и текст; Word/проводник — A6.
18. **После решения панель и «Файлы проекта» расходятся**, если обновить только одну:
    `files_json` и `pending_json` читают реестр отдельно — обновлять оба.

## 7. Окружение и команды

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager log -3 --oneline; git --no-pager status --short --branch
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p hds-agent; cargo test -p hds-ui; cargo test -p hds-cli   # CLI-решения не сломать
cargo test --workspace -- --test-threads=4 *> tools\parity\out\a54-cargo-test.log
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

Стенд UI — тот же, что у A4/A5.3 (`tools\parity\out\agent_a42`):

```powershell
$env:HDS_CONFIG='<repo>\tools\parity\out\agent_a42\config.yaml'
$env:HDS_ROOT='<repo>\tools\parity\out\agent_a42\root'
target\debug\hds.exe ui --port 8799
# панель «Результаты» (значения в ответах — зона пользователя, кириллица: см. §6 п.15 A5.1)
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/pending?project=P0001'
# сравнение
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/compare?project=P0001&rel=<отн. путь>'
# решение (CSRF-заголовок обязателен; тело — файлом UTF-8, PS 5.1 портит кириллицу)
$h=@{'X-HDS-UI'='1';'Content-Type'='application/json'}
# Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/decision' -Method Post -Headers $h -Body (...)
Remove-Item Env:HDS_ROOT, Env:HDS_CONFIG -ErrorAction SilentlyContinue   # перед cargo test
```

**Как получить вариант на стенде** (без UI): mock правит образ инструментом `office_edit`,
затем зеркало собирает вариант:

```powershell
target\debug\hds.exe agent run --project P0001 --prompt "…"   # mock: office_edit(path=образ, ops=[{op:"replace_text",...}])
target\debug\hds.exe agent sync --project P0001 --once
# в реестре: pending_accept, в <папка проекта>\.hds-agent\pending\<rel> — вариант,
# рядом <rel>.report.json (валидатор), в base\ — снимок основания
```

Тесты A5.4 (быстрые, без воркеров и с ними):

```powershell
cargo test -p hds-agent results::          # compare + решения на FakeAnon/FakeExtractor
cargo test -p hds-agent mirror::tests      # стенд зеркала (вариант, отчёт)
cargo test -p hds-ui agent::               # маппинг ошибок/маршруты
cargo test -p hds-ui --test agent_chat     # сокет: pending/compare/decision
```

E2E под-этапа — новый сценарий `tools\parity\out\agent_a42\a54_e2e.ps1` (по образцу
`a53_e2e.ps1`: mock-провайдер из `crates\hds-cli\tests\fixtures\mock_provider.mjs`,
вымышленный ключ, бэкап `projects.json`/`secrets.json`, восстановление в `finally`,
логи только через `ReadShared`), отчёт — `A54_E2E.md` в каталоге стенда. Для шага с
правкой сценарий mock-провайдера:

```json
[{"toolCalls":[{"name":"office_edit","arguments":{"path":"<образ в workspace>",
  "ops":[{"op":"replace_text","find":"[ORG_2]","replace":"[ORG_2] (правка агента)"}]}}]},
 {"text":"Правка внесена"}]
```

## 8. Вопросы владельцу (предложены дефолты)

| # | Вопрос | Дефолт |
|---|---|---|
| В-A5.4-1 | Объём «Сравнить» | Метаданные + отчёт валидатора + текстовый diff (строки); Word-compare и ячейки xlsx — фаза 2 (продолжение В-A5-4/В-7) |
| В-A5.4-2 | Код «файл занят» | `409` с текстом A4 как есть (не 423): UI показывает сообщение, вариант остаётся |
| В-A5.4-3 | Конфликт без варианта | «Принять»/«Оставить вариант агента» недоступны; доступны «Взять новую версию» и «Поручить агенту» |
| В-A5.4-4 | Пределы вывода | 200 строк diff, 2 000 символов на строку, `truncated` в ответе; блок в панели — свёрнутый |
| В-A5.4-5 | Журнал решений и «Вернуть» из backup | Не в A5.4: след решения — `ts` реестра, backup и сообщение агенту (долги A4 п.18–19 → A5.6/A6) |
| В-A5.4-6 | Доставка `messages.jsonl` в сессию | Не в A5.4: A5.4 только пишет очередь (это делает `results::decide`), доставка — A5.6 |
| В-A5.4-7 | Кто собирает `Mirror` | Общий клиент UI: `Client` уже `impl Anonymizer`, адаптер не нужен — `Mirror::from_config(…, &mut *client.lock()?, &mut extractor)`. Замок держится на время решения; второй воркер (`LazyAnonymizer`) — запасной путь, только если замок мешает |

## 9. Срез на 20.11.2026 (для проверки в новом чате)

* Ветка `agent-privacy-zone`, HEAD `fd2852b2` (+docs этого файла), ahead 76, дерево чистое.
* `cargo test --workspace -- --test-threads=4` — 125 наборов / 889 passed / 0 failed /
  13 ignored; python 328 OK (skip 1); `npm test` 23/23; fmt/clippy чисто.
* Стенд `agent_a42`: 34 записи реестра, `pending` пуст, `A51_E2E.md`…`A53_E2E.md`,
  сценарии `a51_e2e.ps1`…`a53_e2e.ps1`.
* Вкладка умеет: проекты/состояние/файлы, реплику с предпросмотром и метками, вложения,
  отправку хода, «Стоп», ленту SSE с рассуждениями/инструментами/ответом, вопрос агента.
  Чего нет: панели «Результаты» (этот под-этап), меток проекта (A5.5), настроек и
  «Очистить историю» (A5.6).

## 10. Промт для нового чата (копировать целиком)

```text
Ты — ведущий разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Это продолжение основного проекта «ИИ-агент в анонимной зоне»
(PLAN_AGENT_PRIVACY_ZONE.md). Ветка agent-privacy-zone, HEAD — docs-коммит A5.3
(сверить git log -1 --oneline), дерево чистое, push не делался.

Фазы A0–A4 закрыты (A4 — двустороннее зеркало, отчёт AGENT_A4_REPORT.md).
Фаза A5 (вкладка «Агент») идёт под-этапами: план, решения (В-A5-1…8, В-A5.2-1…6,
В-A5.3-1…5), долги, контракты и грабли — AGENT_A5_STAGES.md (§1, §2, §3.1–§3.3,
§4–§7).
A5.1 закрыт: вкладка показывает проекты, состояние зеркала и файлы реестра.
A5.2 закрыт: реплика — предпросмотр «что увидит модель», «Скрыть ещё…»,
«Не скрывать», вложения и пути, отправка хода, «Стоп».
A5.3 закрыт: лента событий на SSE (рассуждения, вызовы инструментов и их вывод,
ответ, итог), переключатель «как видит модель», вопрос агента ask_user
(панель ответа, POST /api/agent/answer).
Контекст и методика этого под-этапа — AGENT_A5_4_HANDOFF_PROMPT.md (§3–§9),
этот промт — её §10.

Задача: под-этап A5.4 — панель «Результаты» и решения. Метки — A5.5, настройки и
«Очистить историю» — A5.6, живой сценарий §7 — A5.7.

1. Прочитай AGENT_A5_4_HANDOFF_PROMPT.md целиком, затем AGENT_A5_STAGES.md
   (§2, §3.1–§3.3, §4.1, §4.2, §5–§7) и PLAN_AGENT_PRIVACY_ZONE.md §5.7
   (TR-7.1…7.9), §5.8 (TR-8.2), §5.9 (инварианты), §7 (приёмка).
2. Mirror::compare в hds-agent (crates/hds-agent/src/results.rs): метаданные
   (пути, размеры, sha256), отчёт валидатора (validator::Report::read),
   текстовый diff по hds_extract (лимиты 200 строк, 2000 символов), без новых
   зависимостей; публичный доступ к pending/base (сейчас pub(crate)) — в ядре,
   не склейкой путей в UI.
3. GET /api/agent/pending?project= — варианты из реестра: list_pending()
   (только pending_accept) ПЛЮС list_by_status(Status::Conflict); для каждого
   rel, anon_rel, kind, status, ts, verdict/checks_count из отчёта, признаки
   варианта/основания/_conflicts.
4. GET /api/agent/compare?project=&rel= — Mirror::compare.
5. POST /api/agent/decision {project, rel, decision, choice?} поверх
   Mirror::decide (решения A4 не переписывать): accept|reject|conflict с
   choice=agent|new|delegate; Decision::parse в ядре; коды 400/404/409
   (файл занят — 409 с текстом A4)/500; после решения — notice в кольцо хода.
6. Панель «Результаты» в page.rs: таблица вариант | статус | отчёт | действия;
   «Сравнить…» (метаданные, отчёт, diff), «Принять» (confirm, backup),
   «Отклонить», три кнопки конфликта; «файл занят» — текст сервера, вариант на
   месте; после решения обновить обе панели (Результаты и Файлы проекта).
7. Не делать: метки проекта и склейку (A5.5), настройки, «Очистить историю»,
   доставку messages.jsonl в сессию, журнал решений и «Вернуть» из backup
   (A5.6/A6), Word-compare и ячеечный diff xlsx, правки демона/зеркала и
   sidecar/hds_agent.
8. Тесты: unit (compare, Decision::parse, маппинг ResultsError → ApiError);
   интеграционные через реальный сокет (run_http_listener на порту 0, образец
   crates/hds-ui/tests/agent_chat.rs, install_global + reset_for_tests):
   pending с вариантом и verdict, decision accept (оригинал заменён, backup,
   образ пересоздан, pending пуст), reject, conflict×3, «файл занят» → 409 и
   вариант на месте, повтор → 409, 403 без CSRF и в server-профиле; CLI
   hds agent result не сломан (cargo test -p hds-cli).
   Проверки: cargo fmt --all --check, clippy --workspace --all-targets --
   -D warnings, cargo test --workspace -- --test-threads=4, npm test,
   python unittest.
9. E2E стенда agent_a42 (mock-провайдер): a54_e2e.ps1 — office_edit образа →
   hds agent sync --once → pending=1 и отчёт валидатора; GET pending/compare;
   accept (оригинал = вариант, backup, образ пересоздан, повторный sync — 0
   изменений); reject; «файл занят»; значений в служебных артефактах нет;
   отчёт A54_E2E.md.
10. Коммиты: feat(agent): A5.4 — ...; docs(agent): A5.4 закрыт — ...
    (+ AGENT_A5_STAGES.md §2, §3.4, §4.1, §4.2, §6, §7; README §20 — про
    панель «Результаты» и решения). В сообщении — что проверено.

Проверка среза: ветка agent-privacy-zone, HEAD fd2852b2 (+docs этого файла),
ahead 76, дерево чистое; cargo test --workspace — 125 наборов / 889 passed /
0 failed / 13 ignored; python 328 OK (skip 1); npm test 23/23; стенд agent_a42 —
34 записи реестра, pending пуст, отчёты A51_E2E.md…A53_E2E.md.
```
