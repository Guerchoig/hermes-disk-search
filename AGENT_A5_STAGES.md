# AGENT_A5_STAGES.md — промежуточный план фазы A5 (вкладка «Агент»)

> **Зачем.** `PLAN_AGENT_PRIVACY_ZONE.md` §5.8 (TR-8.2), §5.5 (TR-5.1…5.3), §5.1 (TR-2.6),
> §5.6 (TR-6.6) и §6 задают требования A5. Этот файл режет фазу на **короткие
> под-этапы**: один под-этап — один чат — зелёный тест, видимый снаружи, — коммит.
> Карта требований остаётся в плане и хендоффах, здесь — порядок, границы, решения и
> состояние.
>
> **Дата среза:** 21.11.2026. **Ветка:** `agent-privacy-zone`, **HEAD:** docs-коммит A5.6
> + промт A5.7 (сверить `git log -1 --oneline`), дерево чистое, push не делался.
> **A5.1–A5.7 закрыты — фаза A5 закрыта** (`AGENT_A5_REPORT.md`, приёмка в README §20).
> Прогоны A5.7: `cargo test --workspace --no-fail-fast -- --test-threads=4` —
> **128 наборов / 918 passed / 13 ignored**, единственный упавший набор —
> `agent_acceptance` («профиль .cline изменился», известный флейк) — **одиночным
> прогоном зелёный** (`a57-cargo-test.log`, `a57-acceptance.log`); python 333 OK
> (skip 1); `npm test` 23/23; fmt/clippy чисто; сценарий `a57_e2e.ps1` — 76/76.
> **Машина:** Windows 11, PowerShell 5.1, Rust 1.99, Node 24.18, Python 3.12
> (`sidecar\venv-anonymize`).

## 1. Что уже готово (A0–A4) — опора A5

| Что | Где | Чем пользоваться в A5 |
|---|---|---|
| Шлюз реплик | `crates/hds-agent/src/gateway.rs` | `Gateway::spawn/open/send/abort/stop/next_event/clear_history/shutdown_with`, `GatewayEvent` (Ready, Redacted, Agent, Tool, TurnEnd, …), `GatewayHost` (egress/office/ask_user; `ask_user` сейчас `NoAskUser` — отказ) |
| Реестр живых сессий | `crates/hds-agent/src/registry.rs` | `AgentRegistry` (LRU по `max_live_projects`, выгрузка по `idle_unload_min`, `get_or_open`, `reap_idle`) — **никем пока не используется**, A5 — первый потребитель |
| Фильтр | `crates/hds-agent/src/filter.rs` | снимок vault по `vault_rev` (`RefreshState`), fail-closed, `take_redactions()` — уведомления в ленту |
| Деанонимизация | `crates/hds-agent/src/deanon.rs` | события и текст: метки → значения, пути workspace → папки проекта |
| Решения и отчёт | `crates/hds-agent/src/{results,validator}.rs` | `Mirror::decide`, `ResultsError::Busy`, `pending\<rel>.report.json` |
| Зеркало и реестр | `crates/hds-agent/src/{mirror,registry_db}.rs` | `Mirror::sync_once`, `list`, `list_pending`, `by_orig`, статусы |
| История | `crates/hds-agent/src/history.rs` | `clear`, `read_messages`, `latest_session` — «Очистить историю» (TR-6.6) |
| Клиент воркера | `crates/hds-anonymize/src/client.rs` | `anonymize_text`, `label_for_value`, `snapshot`, `merge_entities`, `keep_terms`, `add_keep_term`, `office_*`, `drop_project` |
| Python-воркер | `sidecar/hds_anonymize/handlers.py` | `anonymize_text`, `vault_snapshot`, `label_for_value`, `merge_entities`, `keep_terms_*` |
| Веб-интерфейс | `crates/hds-ui/src/{lib,page}.rs` | самописный HTTP: поток на соединение, `max_conns`, CSRF/loopback-гейт, `route_full`, вкладки `showTab` (Главная/Транскрибация/Анонимизация), тесты `ui_core`/`ui_auth`/`ui_user` |
| Образцы хода и прохода | `crates/hds-cli/src/cmd/{agent_run,agent_sync}.rs` | порядок «снимок vault → MCP → фильтр → Gateway → события»; ленивые фабрики воркеров (`anonymizer_factory`, `extractor_factory`) |
| Mock-провайдер | `crates/hds-agent/tests/gateway_worker.rs` | Node-скрипт `MOCK_JS` + `MOCK_SCRIPT`: OpenAI-совместимый ответ и журнал запросов — e2e UI без облака |

**Чего нет (создать в A5):** вкладки «Агент»; зависимости `hds-ui → hds-agent` (в
`Cargo.toml` её нет); сессий в процессе UI; ленты событий (SSE/стриминг); предпросмотра
реплики; панелей «Файлы проекта», «Результаты», «Метки проекта», «Настройки агента»;
`ask_user`; уведомлений фильтра; «Очистить историю».

**Открытая предпосылка плана.** K1 — оценка RAG «с / без» (§6 плана: «после A3, до A5»)
— **не выполнена**: `tools/parity/AGENT_RAG_K1.md` нет. Делать параллельно A5.1–A5.2
(1–2 дня), результат нужен до A5.7: иначе `agent.mcp.disk_search.enabled` остаётся
`true` по умолчанию (D23) без замера.


## 2. Под-этапы A5

| # | Под-этап | Результат «снаружи» (критерий) | Оценка | Состояние |
|---|---|---|---|---|
| **A5.1** | **Каркас вкладки и API состояния.** `hds-ui` зависит от `hds-agent`; перенос ленивых фабрик воркеров из CLI в `hds-agent`; вкладка «Агент»; `GET/POST /api/agent/projects`, `GET /api/agent/state?project=`, `GET /api/agent/files?project=`; в server-профиле — 403 (D20) | вкладка открывается, проекты читаются из `projects.json` и создаются, состояние показывает «демон зеркала: жив/нет» (по `sync.lock` + `lock_is_stale`) и последние строки `sync.log`, панель «Файлы проекта» показывает статусы реестра; CSRF-гейт работает; в ответах нет значений fixtures (I8) | 0,5–1 дн | **закрыт 20.11.2026** (§3.1) |
| **A5.2** | **Реплика: предпросмотр и отправка.** `POST /api/agent/preview`, `/label` («Скрыть ещё…»), `/keep` («Не скрывать»), `/attach` (вложения и пути, TR-5.2), `/message`, `/abort`, `/stop`; сессия через `AgentRegistry::get_or_open` | предпросмотр показывает ровно то, что увидит модель (метки подсвечены), «1С:ERP» не скрыт (D19), «Иванову Ивану» → `[PERSON_1\|дат]`; вложение из папки проекта даёт путь образа, файл вне папки — `workspace\_attachments`; один активный ход на проект (второй `message` — 409) | 1,5–2 дн | **закрыт 20.11.2026** (§3.2) |
| **A5.3** | **Лента событий, деанонимизация, `ask_user`.** pump-поток на сессию (`Gateway::next_event`), кольцевой буфер + `seq`; `GET /api/agent/events` (SSE, запасной вариант — long-poll `?after=`); стриминговый ответ в `run_http_listener`; `HostHandler` с вопросами; `POST /api/agent/answer`; переключатель «как видит модель» | в ленте видны рассуждения, вызовы инструментов и MCP с **настоящими** значениями, пути workspace переведены в пути папки проекта, есть ссылка «открыть»; «как видит модель» показывает метки; вопрос агента появляется в ленте и получает ответ, ход продолжается | 2 дн | **закрыт 20.11.2026** (§3.3, отчёт `A53_E2E.md`) |
| **A5.4** | **«Результаты» и решения.** `GET /api/agent/pending`, `/compare`, `POST /api/agent/decision` поверх `hds_agent::results`; `Mirror::compare` в `hds-agent`; панель «Результаты» | вариант в `pending`, отчёт валидатора (`verdict`, `checks`), «Открыть» (путь + копирование), «Сравнить» (минимальный объём), «Принять» (оригинал заменён, backup есть, образ пересоздан), «Отклонить», «Конфликт» с тремя исходами; «файл занят» — понятная ошибка, вариант на месте, повторный `sync` — 0 изменений | 1,5–2 дн | **закрыт 20.11.2026** (§3.4, отчёт `A54_E2E.md`) |
| **A5.5** | **«Метки проекта» (TR-2.6).** RPC `vault_project_labels`; `merge_log` + `unmerge_entities` (миграция схемы); «Объединить», «Разъединить», «Не скрывать»; после склейки — один проход `Mirror::sync_once` и сообщение агенту | таблица «метка → значение (формы) → где встречается»; склейка двух меток: образ содержит старшую, уже выпущенные файлы с `[PERSON_5]` деанонимизируются (`aliases`), агенту ушло сообщение; «Разъединить» возвращает формы склейки, записанной после A5.5 | 1,5–2 дн | **закрыт 20.11.2026** (§3.5, отчёт `A55_E2E.md`) |
| **A5.6** | **«Настройки агента», уведомления фильтра, «Очистить историю».** Панель настроек (провайдер, модель, ключи → `secrets.json`, MCP, суб-агенты); `take_redactions()` → уведомления в ленте; `Gateway::clear_history` по кнопке | ключ не попадает ни в `config.yaml`, ни в ответы API (маска); «Очистить историю»: стоп → значения заменены метками → сессия возобновлена, отчёт «файлов/замен»; в `agent-state` и журналах значений нет (I8) после очистки | 1 дн | **закрыт 20.11.2026** (§3.6, отчёт `A56_E2E.md`) |
| **A5.7** | **Живой сценарий §7, инварианты, отчёт.** Сценарий на вымышленных данных (реальный провайдер; без ключа — mock, и это явно в отчёте); I1/I2/I4/I8 на стенде; `AGENT_A5_REPORT.md`; README §20 — раздел про вкладку | шаги §7 проходят целиком: проект → образы; реплика с `[PERSON_1\|дат]` и «1С:ERP»; агент читает образы, ищет по проекту, пишет столбец и письмо; в ленте настоящие значения; письмо в папке проекта, вариант реестра в `pending`; «Принять» → backup; `egress_log` и `<agent.root>` без форм значений fixtures; `%USERPROFILE%\.cline` не изменён | 1,5 дн | **закрыт 21.11.2026** (§3.7, отчёт `A57_E2E.md`, приёмка — `AGENT_A5_REPORT.md`) |

**Зависимости:** A5.2 ← A5.1; A5.3 ← A5.2; A5.4 ← A5.1 (можно параллельно A5.3);
A5.5 ← A5.1; A5.6 ← A5.3 (уведомления — из событий); A5.7 — последним.

**Про оценку.** Сумма под-этапов — 9,5–11,5 дней, план §6 говорит 6–8: расхождение
дают три вещи, которых в плане не было видно — стриминговый транспорт SSE в
самописном HTTP-сервере (A5.3), `merge_log` и «Разъединить» (A5.5), минимальный
«Сравнить» (A5.4). Если день в день — урезать в порядке: «Разъединить» (A5.5),
«Сравнить» (A5.4), «Скрыть ещё…» в предпросмотре (A5.2) — каждый пункт оформляется
долгом §4.2, а не молчаливым «не сделано».

**Правило нарезки:** если под-этап разрастается — остановиться на ближайшем зелёном
тесте, закоммитить и вынести остаток в следующий под-этап; не тащить недоделанное
через два под-этапа.

## 3. Журнал под-этапов

| Под-этап | Состояние | Коммиты | Заметки |
|---|---|---|---|
| A5.1 | закрыт 20.11.2026 | `be398515` (feat) + docs | см. §3.1 |
| A5.2 | закрыт 20.11.2026 | feat + docs | см. §3.2 |
| A5.3 | закрыт 20.11.2026 | feat + docs (`fd2852b2`) | см. §3.3, отчёт `A53_E2E.md` |
| A5.4 | закрыт 20.11.2026 | `b5daab0b` (feat) + `fb699982` (docs) | см. §3.4, отчёт `A54_E2E.md` |
| A5.5 | закрыт 20.11.2026 | feat + docs | см. §3.5, отчёт `A55_E2E.md` |
| A5.6 | закрыт 20.11.2026 | feat + docs | см. §3.6, отчёт `A56_E2E.md` |
| A5.7 | закрыт 21.11.2026 | feat + docs | см. §3.7, отчёт `A57_E2E.md`, приёмка `AGENT_A5_REPORT.md` |

Факты по каждому под-этапу пишутся подразделами §3.1…§3.7 (как §3.1–3.6 в
`AGENT_A4_STAGES.md`): что сделано, тесты, стенд, осознанные отклонения.

### 3.1. A5.1: что именно сделано (факты)

* **Зависимость** `hds-ui → hds-agent` (`crates/hds-ui/Cargo.toml`).
* **Фабрики воркеров переехали** в `crates/hds-agent/src/factory.rs`
  (`anonymizer_factory`, `extractor_factory`, те же подмены `HDS_ANONYMIZE_*` /
  `HDS_EXTRACT_*`); в `crates/hds-cli/src/cmd/agent_sync.rs` — реэкспорт
  `pub(crate) use`, `agent_result.rs` и демон не менялись. Одно место поднятия
  Python на демон, решения и вкладку.
* **`crates/hds-ui/src/agent.rs`**: `gate()` (конфиг + 403 в server-профиле по
  D20), `projects_json`, `project_add_json` (id выдаётся автоматически, `P`+4
  цифры, отказы 400/409, папка внутри `agent.root` запрещена), `state_json`
  (демон по `sync.lock` + `lock_is_stale`, хвост `sync.log` — 20 строк, числа
  реестра), `files_json` (строки реестра: `orig`, `anon`, `kind`, `status`, `ts`),
  `tail_lines`, `is_project_id`, `next_id`. `DEFAULT_PROVIDER`/`DEFAULT_MODEL`
  переехали в `hds_agent::projects` (одна копия на CLI и UI).
* **Маршруты** (`lib.rs::route_full`): `GET /api/agent/projects|state|files`,
  `POST /api/agent/projects`; POST под общим CSRF-гейтом, `/api/agent/*` не входит
  в `is_user_route`.
* **Страница** (`page.rs`): вкладка «Агент» (`data-tab="agent"`), секции
  «Проекты», «Состояние проекта» (с журналом), «Файлы проекта», «Новый проект»;
  JS на существующих `jget`/`jpost`/`esc`, без внешних ресурсов. В server-профиле
  кнопка скрывается после ответа 403 (страница — константа, профиль зависит от
  конфига, поэтому решается в JS).
* **Тесты**: `crates/hds-ui/tests/agent_api.rs` — 4 теста (пустой список и
  валидация параметров; создание проекта, `state`, `files` с записью реестра,
  409 на дубль, 400 на относительный путь и папку в зоне; CSRF; 403 в
  server-профиле). Юнит-тесты `agent.rs` — 3 (`is_project_id`, `next_id`,
  `tail_lines`). Прогоны: `cargo test --workspace -- --test-threads=4` —
  **124 набора / 862 passed / 0 failed / 13 ignored** (`a51-cargo-test2.log`);
  python 328 OK (skip 1); `npm test` 23/23.
* **Стенд** (`A51_E2E.md`): `hds ui --port 8799` — проекты (`P0001`, 34 файла,
  0 в `pending`), состояние (`daemon=false`, журнал 12 строк, хвост
  `stop reason=once`), файлы (34: `synced` 31, `accepted` 2, `unsupported` 1),
  страница с вкладкой, создание `P0002` и откат `projects.json`.
* **Осознанные отклонения:** (1) вкладка не скрывается сервером — страница
  статична, профиль известен только API; (2) `hds ui` не кэширует реестр —
  открытие БД на запрос (WAL, дёшево), кэш появится вместе с сессиями в A5.2;
  (3) `hds agent project rm` в UI пока нет — удаление проекта остаётся CLI-долгом.
* **Известная флейка:** `agent_acceptance` (I4) сравнивает снимок
  `%USERPROFILE%\.cline` до и после хода; при параллельном прогоне всего
  workspace IDE пишет в профиль (кэш feature-flags, `session-search.db`) и тест
  падает «профиль .cline изменился». Одиночный прогон зелёный — это окружение,
  а не регрессия A5.1 (грабля §6 п.18).


### 3.2. A5.2: что именно сделано (факты)

* **План хода переехал в `hds_agent::run`** (`crates/hds-agent/src/run.rs`): `RunOptions`,
  `RunPlan` (поле `secrets` приватное, наружу — `secret_strings()`), `TurnReport`,
  помощники (`provider_kind`, `default_base_url`, `resolve_node/resolve_worker`,
  `system_values`, `mcp_port/mcp_base_path`, `tavily_script`), `plan/plan_with`,
  `plan_json` (без ключей), `prepare`, `build_filter`, `open_client/open_client_at`,
  `clear_history`. `crates/hds-cli/src/cmd/agent_run.rs` — тонкая обёртка (реэкспорт + разбор
  аргументов, чтение реплики, печать событий); тесты `agent_run`/`agent_acceptance` не менялись.
* **`GatewayError::is_timeout()` + `TIMEOUT_EVENT`** — насос отличает пустой poll от сбоя канала.
* **`crates/hds-ui/src/agent_session.rs`**: `AgentApi` (`OnceLock` в проде, `for_tests`/
  `for_tests_at(root)` — своё состояние и корень поставки), `Turn` (busy, кольцо 500 событий,
  `session_id`, насос), `events_json`, `preview`, `label`, `keep`, `attach`, `message`
  (plan→prepare→`Gateway::spawn|get_or_open`→`open`→`send_anonymized`), `abort`, `stop`,
  `reset_for_tests`. Один активный ход на проект (409), `busy` снимается насосом по
  `TurnEnd`/`Done`/ошибке; выгрузка по простою не рвёт кольцо (`session_id` возобновляется
  через `history::read_messages`).
* **Предпросмотр прогоняет словарный фильтр точки `model`** (`egress_preview`): `preview` и
  событие `prompt` совпадают с текстом, который получит воркер. Причина: NER не выделяет часть
  известных форм («Иванову» при метке на «Иван»), и без фильтра предпросмотр расходился с
  отправкой. Новые метки — тот же `label_for_value` (`OTHER`), своих регулярок меток в UI нет.
* **`attach`**: файл внутри папки проекта → путь образа по реестру (`kind=image`, `rel`,
  `anon`, `status`), нет записи/не зеркалирован → 409 с текстом «запустите hds agent sync»;
  файл вне папки → копия образа в `workspace\_attachments` (`kind=attachment`,
  `anon=_attachments\<имя>`). Оригинал не меняется.
* **Маршруты** (`lib.rs::route_full`): `GET /api/agent/events?project&after&limit`,
  `POST /api/agent/{preview,label,keep,attach,message,abort,stop}` — под общим CSRF-гейтом и
  `agent::gate()` (403 в server-профиле, D20). `/api/agent/*` в `is_user_route` не входит.
* **Вкладка** (`page.rs`): секция «Реплика» — textarea, «Что увидит модель» (подсветка меток
  токенами из ответа), «Скрыть ещё…» (по выделению), «Не скрывать», «Вложить файл…»,
  «Отправить» (требует предпросмотра), «Стоп», статус (`busy`, `session`, последние 20 строк
  событий), ошибки 400/403/409/503/500 текстом. Ленты/SSE и `ask_user` нет — A5.3.
* **Тесты**: unit в `hds-agent::run` (4) и `hds-ui::agent_session` (4: кольцо и нумерация
  `seq`, `after/limit`, лимит и пустой текст, `relative_to`); интеграционные
  `crates/hds-ui/tests/agent_chat.rs` (5) — живой `dist/worker.mjs`, mock-провайдер
  (`crates/hds-cli/tests/fixtures/mock_provider.mjs`), mock-воркер анонимизации (Python,
  `HDS_ANONYMIZE_WORKER`): preview с `[PERSON_1|дат]` и «1С:ERP» без метки, `paths` → образ,
  label/keep, attach (образ и `_attachments`, 409 на незеркалированный), message → 409 на
  второй ход → события (`prompt` раньше `agent`, метки в запросе к провайдеру, ключа нет) →
  abort → `busy=false`; маршруты: CSRF 403, server-профиль 403, пустой проект 400.
* **Прогоны**: `cargo test --workspace -- --test-threads=4` — **125 наборов / 875 passed /
  0 failed / 13 ignored** (`a52-cargo-test.log`); python 328 OK (skip 1); `npm test` 23/23;
  fmt/clippy чисто.
* **Стенд** (`A52_E2E.md`): mock-провайдер + настоящий анонимизатор; preview даёт
  `[PERSON_10|дат]` и `[PERSON_12]` (остаток «Иванову» добавлен «Скрыть ещё…»), «1С:ERP» не
  скрыт, keep «Ромашка» (в базе уже была — `added=false`), attach образа
  `01 Устав\… [ORG_1].docx` и вложения `_attachments\a52-outside.txt`, message → 14 событий
  (`prompt, session, agent×10, redacted, turn_end`), второй message — 409, abort → `busy=false`;
  ключа провайдера в артефактах нет.
* **Осознанные отклонения:** (1) предпросмотр делает дополнительный проход фильтра (см. выше) —
  это дороже одного `anonymize_text`, но сохраняет главный инвариант TR-5.1; (2) `label` без
  `type` — `OTHER` (как у фильтра исходящего); (3) вложение вне папки не пишется в реестр
  (долг §4.2 п.16); (4) «Отправить» требует предпросмотра — это и есть подтверждение отправки
  (В-A5.2-1).


### 3.3. A5.3: что именно сделано (факты)

* **Деанонимизация вопроса — в шлюзе** (`crates/hds-agent/src/gateway.rs`):
  `GatewayHost::deanon_question` разворачивает метки через `RefreshState.deanon` перед
  `AskUser::ask`; в egress-only (`refresh == None`) вопрос остаётся с метками (задокументировано).
  Тесты: `ask_user_question_is_deanonymized` (снимок `[PERSON_1]` → «Иванов Иван»),
  `ask_user_without_refresh_keeps_labels`.
* **`AskState` + `UiAskUser`** (`crates/hds-ui/src/agent_session.rs`): одно состояние на проект
  (`Turn::ask_state`), `post/pending/answer/cancel/wait`; `answer` проверяет номер
  (`AskError::None` — 404, `Stale` — 409), по таймауту и отмене вопрос снимается. `UiAskUser`
  кладёт в кольцо `ask`, `ask_timeout`, `ask_cancelled` и возвращает `{"answer": …}`.
  `abort`/`stop`/`reset_for_tests` снимают вопрос **до** команды воркеру (иначе поток чтения не
  прочитает `abort`). Таймаут — `agent.ask_timeout_sec` (новое поле `AgentSettings`, дефолт 600,
  ключ добавлен в `config.example.yaml`).
* **`POST /api/agent/answer`** `{project, ask_id, answer}`: 400 (пусто/длиннее 20 000), 404
  (вопроса нет), 409 (номер устарел); маршрут под общим CSRF-гейтом и `agent::gate()`.
* **`Turn` получил будильник**: общий счётчик `seq` (номер выдаёт кольцо) + `Condvar` в `push`;
  `wait_for_change(15 с)` — поток SSE отдаёт событие сразу, насос и кольцо не менялись.
* **SSE-транспорт** (`crates/hds-ui/src/{agent,lib}.rs`): ветка в `run_http_listener` **до**
  `route_full` — `GET /api/agent/events` при `Accept: text/event-stream`, те же гейты
  (`ui_auth_gate`, `agent::gate` → 403 в server-профиле); `ReqHeaders` получил `accept` и
  `last_event_id`; `sse_frame`/`sse_state_frame`/`stream_after`; заголовки без `Content-Length`,
  `retry: 1500`, `event: state`, кадры `id:`/`event: item`/`data:`, heartbeat `: ping` каждые
  15 с, закрытие по ошибке записи или через 300 с; `Last-Event-ID` важнее `?after=`. Поток
  открывается и **до** первого хода: ждёт появления хода (не закрывается); JSON-фолбэк
  (`?after=`) не изменился.
* **Лента вкладки** (`crates/hds-ui/src/page.rs`): `<pre id="agevents">` заменён на
  `<ul id="agfeed">` + панель `<div id="agask">`; блоки: рассуждение (`reasoning*`, свёрнуто),
  ответ (`text*`, дельты склеиваются, `text` заменяет накопленное), `tool_use` (вход до 500
  симв.), `tool_result` (вывод до 20 000, свёрнуто, ошибка — красным), `redacted`
  (предупреждение метками), `ask`/`ask_timeout`/`ask_cancelled`, `turn_end`/`done`/`error`;
  `usage`/`iteration`/`status`/`agent_done`/`ended` — в строку статуса; переключатель «как
  видит модель» перерисовывает ленту из `raw`; в DOM ≤ 500 узлов; `AG_SEQ` — из `id:` кадра
  (`e.lastEventId`); `EventSource` закрывается при смене проекта, уходе со вкладки
  (`visibilitychange`) и `beforeunload`; JSON-фолбэк — только если `EventSource` нет.
* **`DeAnonymizer::replace_paths`**: путь `workspace` пробуется в обоих разделителях (`/` и
  `\`) — конфиг стенда пишет `agent.root` через `/`, а модель присылает `\`, и путь зоны
  оставался в ленте (найдено на стенде). Тест `workspace_path_matches_any_separator`.
* **Mock-провайдер**: необязательное поле шага `reasoning` → поток `reasoning_content`
  (проверка ленты рассуждений без облака); без поля поведение прежнее.
* **Тесты**: unit — `hds-agent` gateway `ask_user_*` (2) и deanon
  `workspace_path_matches_any_separator`; `hds-ui` `agent::tests` (`sse_frame`,
  `sse_state_frame`, `wants_stream`, `stream_after`) и `agent_session::tests`
  (`ask_state_answer_by_id_only`, `ask_state_wait_returns_answer`,
  `ask_state_timeout_and_cancel`); интеграционные `crates/hds-ui/tests/agent_chat.rs` +4:
  `sse_stream_state_and_turn_events`, `ask_user_question_answer_and_turn_end`,
  `abort_cancels_pending_ask`, `sse_and_answer_gates` (реальный сокет на порту 0).
* **Стенд** (`A53_E2E.md`): 22/22 проверок — SSE с `event: state` и 45 кадрами, рассуждение,
  `tool_use`/`tool_result`, ответ, `turn_end`; путь в `text` — папка проекта, в `raw` — метки;
  вопрос «Сколько дней? Ивану» → ответ → ход завершён; в логе провайдера метки (`[PERSON_10]`
  ×53, `[PERSON_12]` ×40), значений и ключа нет.
* **Осознанные отклонения:** (1) `ask` не имеет `raw`: вопрос приходит в шлюз уже
  развёрнутым, «как видит модель» показывает его текст (метки видны в соседних `tool_use`);
  (2) таймаут вопроса в e2e вживую не ждали (600 с) — покрыт unit-тестом; (3) `ask_timeout`
  берётся из настроек первого хода проекта (шлюз переживает ходы) — смена ключа подхватится
  новым процессом воркера, это окно A5.6 (настройки).


### 3.4. A5.4: что именно сделано (факты)

* **`Mirror::compare`** (`crates/hds-agent/src/results.rs`) — `CompareReport`: `FileInfo`
  для оригинала, варианта, основания и образа (путь, наличие, размер, sha256, mtime),
  отчёт валидатора (`validator::Report::read`), `identical` (sha), построчный diff и
  `diff_truncated`/`diff_note`. Diff — LCS по строкам, без новых зависимостей: не больше
  `DIFF_MAX_LINES`=200 строк и `DIFF_MAX_CHARS`=2000 символов, каждая строка — до
  `DIFF_LINE_CHARS`=200, вход — до `DIFF_SCAN_LINES`=400 строк на сторону. Текст — через
  `validator::read_text`: md/txt/xml читаются напрямую, docx/xlsx/pdf/pptx — `hds_extract`.
* **Пути варианта — в ядре.** `results::VariantPaths` + `variant_paths(project_root, rel)`
  отдают `original`/`pending`/`base`/`report`; `Mirror::pending_path`/`base_path` теперь
  `pub` и делегируют туда же. UI (`/api/agent/pending`) не склеивает `.hds-agent\…` сам.
* **`Decision::parse(decision, choice)`** — `accept|reject` и `conflict` + `choice`
  (`agent|new|delegate`), плюс слитая форма `conflict:agent` (как в журнале). Неизвестное
  слово — `None` → 400, без догадок.
* **Маршруты** (`crates/hds-ui/src/agent.rs`, `lib.rs`): `GET /api/agent/pending?project=`
  (`list_pending()` + `list_by_status(Status::Conflict)`, сортировка по `ts`/`rel`;
  для каждого — `rel`, `anon_rel`, `kind`, `status`, `ts`, `conflict`, `variant`, `base`,
  `conflicts` (есть ли копии в `_conflicts`), `variant_path`, `report {verdict, checks, rev}`);
  `GET /api/agent/compare?project=&rel=`; `POST /api/agent/decision {project, rel, decision,
  choice?}` поверх `Mirror::decide` (решения A4 не переписывались).
* **Коды ответов решения:** 400 — тело/решение/путь (`BadPath`), 404 — проекта, записи
  реестра или варианта нет (`NotFound`/`NoVariant`), 409 — решать нечего (`WrongStatus`),
  оригинал изменился (`OriginalChanged`), нечего делегировать (`NothingToDelegate`),
  формат нельзя принять на месте (`Kind`), файл занят (`Busy`, текст A4 «закройте … вариант
  остался в pending, статус не изменён»), 503 — зона выключена, 500 — Io/реестр/воркер.
  Маппинг — `agent::results_err` с unit-тестом.
* **Notice в кольцо хода:** `AgentApi::notice(project, text)` кладёт событие `notice`
  («решение по «<rel>»: принято/отклонено/…», backup — `.hds-agent\backup`, копия — `_conflicts\…`)
  без содержимого документа; SSE отдаёт его как обычное событие.
* **Панель «Результаты»** (`page.rs`): таблица «вариант → статус → отчёт → действия»,
  «Открыть» (путь + копирование), «Сравнить…» (метаданные, отчёт по проверкам, diff),
  «Принять» (confirm + backup), «Отклонить», три кнопки конфликта; «файл занят» — текст
  сервера, вариант остаётся в таблице; после решения обновляются «Результаты», «Файлы
  проекта» и «Состояние проекта» (число «ждут решения»).
* **Unit:** `hds-agent` — `compare_reports_metadata_report_and_diff`,
  `compare_tolerates_missing_variant_and_unknown_rel`, `compare_diff_respects_limits`,
  `decision_parse_accepts_known_forms`; `hds-ui` — `results_errors_map_to_http_codes`.
* **Интеграционные** (`crates/hds-ui/tests/agent_results.rs`, реальный сокет `run_http_listener`
  на порту 0, `install_global` + `reset_for_tests`, mock-воркер анонимизации из
  `tests/fixtures/mock_anon.py`): pending с вариантом и отчётом, compare, accept (оригинал
  заменён, backup, образ пересоздан, pending пуст, повтор — 409), reject (оригинал цел,
  образ восстановлен, `document_rejected` в очереди), конфликт ×3 (agent/new/delegate),
  «файл занят» → 409 и вариант на месте (после закрытия — 200), 403 без CSRF и 403 в
  server-профиле, 400/404 на плохих входах, notice в кольце.
* **Фикстура:** mock-воркер вынесен в `crates/hds-ui/tests/fixtures/mock_anon.py` (общий с
  A5.2/A5.3), добавлены `anonymize_path`/`image_map`/`deanonymize_agent`; `agent_chat.rs`
  подключает её через `include_str!`.
* **E2E стенда** — `a54_e2e.ps1`, `A54_E2E.md`: 36/36 проверок (см. §4.2 п.24 — что нашлось).


### 3.5. A5.5: что именно сделано (факты)

* **Схема без повышения версии.** `vault.merge_log` создаётся в `_init_schema`
  как `CREATE TABLE IF NOT EXISTS` рядом с `aliases` (аддитивная миграция «на
  месте»): `SCHEMA_VERSION` остался **2**, потому что `_migrate_if_needed` при
  другой версии переименовал бы базу (D4) и потерял метки. Колонки: `id,
  project_id, ts, keep_type, keep_n, keep_entity, dropped, forms_before`
  (`dropped` — JSON `[{type, n, entity_id, forms}]`, `forms_before` — JSON форм
  старшей **до** склейки).
* **`Vault.merge_entities`** в той же транзакции пишет строку журнала и
  возвращает `merge_id` (`None`, если склеивать было нечего). Поведение
  (алиасы, перенос форм, отказ для разных типов) не менялось — на нём CLI.
* **`Vault.unmerge_entities(project_id, merge_id)`** (A5.5, В-A5-3): проверяет
  журнал, для каждой младшей метки снимает псевдоним, восстанавливает `labels`,
  убирает из старшей формы, которых не было в `forms_before`; удаляет запись
  журнала. Возвращает `{merge_id, keep, restored, forms_removed}`. Неизвестная
  склейка (в том числе любая до A5.5) и чужой проект — `VaultError` с понятным
  текстом (UI → 404/409). `drop_project` чистит `merge_log` вместе с
  `labels`/`aliases` (`merges` в ответе).
* **`Vault.project_labels(project_id)`** — `{rev, labels: [{label, type, forms,
  aliases}], merges: [{id, ts, keep, dropped, forms_moved}], keep}`. Псевдонимы
  собраны у **оставшейся** метки. Значения форм отдаются (зона пользователя,
  как `snapshot`), в журналы не пишутся (I8).
* **Разрешение псевдонимов при выдаче метки** (`_merged_target` в
  `_ensure_label`): без него следующий проход NER завёл бы младшей сущности
  **новый** номер (`norm_key` у неё свой, строка `labels` удалена) — образы
  после пересборки разошлись бы со вторым проходом, а `sync --once` дал бы
  изменения. Теперь номер берётся у старшей (цепочка `A → B → C` разрешается,
  цикл отсекается); после «Разъединить» запись журнала удалена — метка снова
  своя. Тест `test_merge_makes_ner_use_senior_label`.
* **RPC**: `vault_project_labels`, `unmerge_entities` — в `worker.py` (диспетчер
  + `hello.capabilities`) и `handlers.py`. Клиент `hds-anonymize`:
  `ProjectLabel`/`MergeRecord`/`ProjectLabels`/`MergeOutcome`/`UnmergeOutcome`,
  методы `project_labels`, `unmerge_entities`; `merge_entities` теперь отдаёт
  типизированный `MergeOutcome` с `merge_id` (CLI печатает «склейка №N»).
* **Ядро `crates/hds-agent/src/labels.rs`**: `LabelsReport` (`rev`, `labels`,
  `merges`, `keep`), строка `LabelRow` (метка, тип, формы, псевдонимы,
  `occurrences` — файлы реестра, где метка есть в образе, с числом вхождений),
  `report(registry, workspace, anonymizer, extractor, project_id)` и вход
  `Mirror::labels_report()`. Текст образа читается `validator::read_text`
  (md/txt/xml — напрямую, docx/xlsx/pdf/pptx — `hds_extract`), метки —
  `hds_core::labels::{find_labels, label_parts}`: своих регулярок в ядре и в
  UI нет (AG-1). Сравнение — по частям метки, поэтому `[PERSON_1|дат]` — та же
  сущность. Нечитаемый образ пропускается: панель подписана «по тексту
  документов (тело)» (A5.4 §6 п.24).
* **`Mirror::refresh_label_images(dropped)`** (`results.rs`) — пересборка
  образов, где встречаются младшие метки, через существующий
  `rebuild_image(row, Status::Synced)`; `RefreshReport {rebuilt, skipped}`.
  Пересобираются только `synced`/`accepted`; `pending_accept`/`conflict`/
  `agent_changed` попадают в `skipped` (пересборка потеряла бы правки агента,
  В-A5.5-5). После — **один** `sync_once` в маршруте.
* **Сообщения агенту**: `MessageKind::LabelsMerged` (`labels_merged`) и
  `LabelsUnmerged` (`labels_unmerged`), публичный
  `results::append_message(project_root, kind, rel, text)`; `Mirror::notify`
  делегирует ему. У меток `rel=""`, текст — только метки
  (`labels::merge_message`/`unmerge_message`), значений нет (I8).
* **Маршруты** (`crates/hds-ui/src/agent.rs`, `lib.rs`):
  `GET /api/agent/labels?project=`, `POST /api/agent/labels/merge
  {project, keep, drop}`, `POST /api/agent/labels/unmerge {project, merge_id}`;
  «Не скрывать» — существующий `/api/agent/keep`. Все под `agent::gate()` и
  CSRF-гейтом, в server-профиле 403. Python-воркер — **один на UI**
  (`AgentApi::client`), замок держится только на вызов vault и на
  пересборку+проход: `with_client_mirror` (новый вариант рядом с `with_mirror`
  A5.4, `LazyAnonymizer` не поднимается). Коды: 400 (тело, нет `keep`/`drop`,
  разные типы, одна метка, нет `merge_id`), 404 (проект/реестр/метка не
  проекта/склейка не найдена), 409 (метка уже псевдоним; склейка изменилась),
  503 (зона выключена), 500 (сбой). Проверка «метка есть в проекте / уже
  псевдоним» идёт по снимку `project_labels` **до** записи, а не по тексту
  ошибки Python. После склейки/разъединения — `notice` в ленту без значений.
* **Панель «Метки проекта»** (`page.rs`, между «Файлами проекта» и «Новым
  проектом»): таблица «метка | значение (формы) | где встречается», чекбоксы,
  «Объединить выбранные» (один тип, старшая — максимум файлов, при равенстве —
  первая отмеченная, `confirm()` с выбором), блок склеек с «Разъединить»
  (только `merge_log`), «Не скрывать» у каждой формы с подписью «действует во
  всех проектах», подпись про необратимость склеек до A5.5. После склейки и
  разъединения обновляются панель, «Файлы проекта» и «Состояние проекта».
* **Тесты.** Python: `SchemaTest` (`merge_log` в таблицах и колонках),
  `test_merge_writes_log`, `test_unmerge_restores`, `test_unmerge_unknown`,
  `test_project_labels_shape`. Клиент: mock-воркер отвечает
  `vault_project_labels`/`unmerge_entities`, в `tests/client.rs` — разбор
  `ProjectLabels`/`MergeOutcome`/`UnmergeOutcome`. `hds-agent`: `labels::tests`
  (вхождения по файлам, пропавший образ, тексты сообщений, отчёт через
  зеркало), `results::tests`
  (`refresh_rebuilds_synced_images_and_skips_variants`,
  `append_message_writes_labels_only_queue`, разбор новых `MessageKind`).
  `hds-ui`: unit `merge_request_accepts_one_type_and_rejects_bad_bodies`,
  `unmerge_merge_id_parses_number_and_string`; сокет
  `crates/hds-ui/tests/agent_labels.rs`
  (`labels_merge_unmerge_cycle_on_socket`,
  `labels_routes_validate_body_and_require_csrf`): GET labels, merge (образы
  пересобраны, `messages.jsonl`, журнал склеек, `notice`), 409 на повторную
  склейку псевдонима, 404, unmerge (метки вернулись, образы снова с младшей),
  keep, 400, 403 без CSRF и в server-профиле. `cargo test -p hds-cli` —
  `hds anon merge` не сломан (типизированный ответ).
* **Прогоны:** `cargo fmt --all --check` и
  `clippy --workspace --all-targets -- -D warnings` — чисто;
  `cargo test --workspace --no-fail-fast -- --test-threads=4` — **127 наборов /
  907 passed / 13 ignored** и два известных флейка под параллельной нагрузкой
  (`agent_acceptance::acceptance_a3_end_to_end` — IDE пишет в `%USERPROFILE%\.cline`;
  `quota_socket::unauthorized_heavy_rejected_before_queue` — `max_conns`/503);
  оба одиночным прогоном зелёные. Python — **333 OK (skip 1)**, `npm test` —
  **23/23**. Логи: `tools/parity/out/a55-cargo-test2.log`.
* **E2E стенда** — `a55_e2e.ps1` (+ `a55_probe.py`: текст образа, метки, значение
  метки, журнал склеек; `a55_db_state.py`): `GET labels` (46 меток), пара одного
  типа (`[MONEY_1]`/`[MONEY_2]`), склейка → 45 меток, образы пересобраны
  (младшей метки нет, старшая есть), `messages.jsonl` `labels_merged`, `sync --once`
  `changes=0`, деанонимизация младшей метки через псевдоним, разъединение → 46
  меток, `labels_unmerged`, `changes=0`, «Не скрывать» (вымышленный термин), I8 —
  значений в служебных артефактах нет. **30/30 проверок** (`a55-checks.txt`,
  отчёт `A55_E2E.md`). Бэкап `anonymize.db` — в `finally` (восстановление, если
  разъединение не прошло; на прогоне не понадобилось).
* **Осознанные отклонения:** (1) при разъединении образы пересобираются по
  **старшей** метке (`restored`-метки в образах уже нет — её заменяет NER);
  (2) `labels_err` отдаёт 500 на любую ошибку реестра/воркера — отдельного
  кода не заводили, причина для пользователя одна: «посмотрите `sync.log` и
  повторите»; (3) заголовки docx в «где встречается» не видны — ограничение
  `hds_extract` (A5.4 §6 п.24), а не панели.



### 3.6. A5.6: что именно сделано (факты)

* **`Secrets::save`** (`crates/hds-agent/src/secrets.rs`) — атомарная запись
  `data\agent\secrets.json` через `replace_file` с **слиянием** с тем, что лежит
  на диске: объекты сливаются рекурсивно, наши значения сильнее. Поэтому не
  теряются ни `mcp.servers`, ни ключи, появившиеся после `load` (CLI и вкладка
  правят один файл). Удалений слияние не умеет: `set_provider_base_url("", …)`
  пишет `null` («база API по умолчанию»). Добавлены `empty`, `load_or_empty`,
  `has_provider_key` (только признак), `provider_base_url`, `provider_names`,
  `set_provider_key`, `set_provider_base_url`; новая ошибка `SecretsError::Write`
  называет путь и причину, **не значение**. Тесты: `save_merges_and_keeps_unknown_keys`
  (ключ, `base_url`, `mcp.servers`, `projects`, `future_block` и ключ, дописанный
  на диск после `load`), `save_creates_missing_file_without_values` (нет файла и
  каталога; в тексте ошибки нет значения).
* **Редактор `config.yaml`** (`crates/hds-ui/src/config_edit.rs`): `apply_scalars`
  (строка-редактор без regex, значения — готовые YAML-литералы), `set_agent_values`
  (чтение файла → правка → `load_text` → проверка **каждого** пути через `dig`
  против разобранного литерала → `write_config` с `replace_file`). Вложенные
  ключи (`filter.unknown`, `mcp.tavily.enabled`, `network.proxy`) вставляются
  блоком; хвостовой комментарий строки сохраняется, отсутствующая секция
  дописывается. Тесты `agent_scalars_keep_comments_and_neighbours`,
  `agent_scalars_create_section_and_subsections`. `looks_abs` стал
  `pub(crate)` — им же проверяется абсолютность `agent.root`.
* **`AgentApi::unload` и `live`** (`agent_session.rs`): выгрузка шлюза по проекту
  (гасит насос, `opened=false`) и список живых процессов с простоем — для
  панели и для «Очистить историю». `Turn::set_busy_for_tests` (`#[doc(hidden)]`)
  позволяет проверить 409 без живого воркера.
* **Маршруты** (`agent.rs`, `lib.rs`, под `agent::gate` и CSRF):
  `GET /api/agent/settings?project=…` — проект (провайдер, модель, `key.set`,
  `base_url`), `providers` (имя и признак ключа), `config` (все ключи `agent:`),
  `live`, `config_path`, `secrets_path`; значения ключей не отдаются ни в каком
  виде (В-A5.6-2). `POST /api/agent/settings` — проверки (400) → `projects.json`
  (провайдер/модель) → `secrets.json` (ключ, только этот файл) → `config.yaml`
  (общие настройки) → `unload` + `notice`; в ответе то же тело плюс `applied`,
  `reopened`, `unloaded`, `warnings`. `POST /api/agent/history/clear` —
  `{project, session?}`: при `busy` сначала `stop` (не удалось — 409), затем
  `unload`, затем `hds_agent::run::clear_history` (**тот же путь, что у CLI**:
  словарь фильтра, бэкапы) и `notice`; ответ
  `{session, files, backups, replaced, files_count, backups_count}`
  (В-A5.6-4). Коды: 400 — тело/сессия, 404 — проект/состояние/сессия,
  409 — «ход идёт, остановить не удалось», 503 — зона выключена/словарь,
  500 — остальное; в server-профиле 403 (D20).
* **Панель** (`page.rs`): «Настройки агента» — таблица «параметр → значение →
  изменить» (провайдер, модель, ключ «задан/нет», база API, зона, пороги фильтра,
  `egress_log`, `preview_outgoing`, суб-агенты, лимиты, прокси, MCP и команды,
  живые процессы, пути файлов). Правка: `prompt()` для скаляров, `<select>` для
  `unknown`, `confirm()` для bool и для того, что перезапускает сессию
  (В-A5.3-3/В-A5.6-3); своих регулярок и разбора значений в JS нет (AG-1).
  Блок уведомлений фильтра — сводка по событиям `redacted` из кольца (число,
  метки, источник, без значений) и кнопка «Сбросить сводку»; там же
  «Очистить историю…» с `confirm()` и отчётом. Вторая такая же кнопка — в
  настройках; обе пишут отчёт в оба места.
* **Тесты.** `hds-agent`: `secrets::tests::save_*` (слияние, отсутствующий файл,
  отсутствие значения в тексте ошибки). `hds-ui` unit: `settings_request_validates_body`
  (поля, типы, границы 1…8 / ≥10 / ≥30 / ≥30, `label|block`, `provider` без
  слэшей, пустой ключ, `preview_outgoing` без `reopen`), `history_request_validates_session`
  (`..\..\x`, `a/b`, не строка), `config_edit::tests::agent_scalars_*`. Сокет
  `crates/hds-ui/tests/agent_settings.rs` (3 теста на реальном
  `run_http_listener`, mock-воркер `fixtures/mock_anon.py`, `config.yaml` с
  комментариями): GET без ключа и без `api_key` в тексте ответа → POST (модель,
  ключ, `ask_timeout_sec`, `unknown=block`, `egress_log`) → `projects.json`
  (модель, остальные поля), `secrets.json` (ключ, `mcp.servers`, `future_block`),
  `config.yaml` (комментарии в секции, хвостовой и отдельная строка;
  `ask_timeout_sec: 601`, `filter.unknown`, `egress_log`; соседние секции целы),
  `notice` в ленте; `history/clear` (2 файла, 2 бэкапа, `replaced ≥ 1`, метка
  вместо значения, бэкап — прежняя история, `notice`); 400/404/409 (ход идёт,
  шлюза нет)/403 без CSRF (ключ не записан)/403 в server-профиле. `cargo test
  -p hds-cli` — `hds agent history clear` не сломан.
* **Прогоны:** `cargo fmt --all --check` и
  `clippy --workspace --all-targets -- -D warnings` — чисто;
  `cargo test --workspace --no-fail-fast -- --test-threads=4` — **128 наборов /
  918 passed / 13 ignored, 0 failed**, известные флейки (`agent_acceptance` — IDE пишет в
  `.cline`; `quota_socket` — под нагрузкой) одиночным прогоном зелёные; Python —
  333 OK (skip 1), `npm test` — 23/23. Лог: `tools/parity/out/a56-cargo-test.log`.
* **E2E стенда** (`a56_e2e.ps1` + `a56_probe.py`, отчёт `A56_E2E.md`):
  комментарии в `config.yaml`; значение PERSON из базы стенда (probe через
  `Vault.project_labels`, без своих регулярок); mock-провайдер с `tool_call`, в
  аргументах которого **открытое** значение → 2 события `redacted`, в запросах к
  модели значения нет, метки есть (I2); GET settings (ключ только признаком,
  `ask_timeout_sec=600`, `unknown=label`); POST (модель `mock-model-2`,
  вымышленный ключ `sk-a56-e2e-secret`, `ask_timeout_sec=601`) — `reopened=true`,
  комментарии и соседи `config.yaml` целы, `mcp.servers`/`future_block` целы,
  ключа нет ни в `config.yaml`, ни в артефактах; `history/clear` — 2 файла,
  2 бэкапа, `replaced=8`, `notice`; I8-скан `agent-state` (без `*.bak`) — 0
  файлов со значением; повторный ход — сессия возобновлена (`resumed=true`);
  `sync --once` до и после — `changes=0`; 403/400/404. **49/49 проверок**
  (`a56-checks.txt`). Бэкап `projects.json`/`secrets.json`/`config.yaml` — в
  `finally`.
* **Осознанные отклонения:** (1) `reopened` — это «сессия будет открыта заново»
  (свойство запроса), а `unloaded` — был ли живой процесс; так панель честно
  пишет и тогда, когда процесс ещё не открывался; (2) `max_live_projects`
  пишется в конфиг, но реестр живых создаётся при старте `hds ui` — в ответе
  `warnings`; (3) пустой `base_url` пишется как `null` (слияние не удаляет
  ключи); (4) бэкапы истории (`*.json.bak`) остаются с открытыми значениями —
  это зона агента (I1), в ответах и логах их нет; «Вернуть» из бэкапа и его
  удаление — A6; (5) GET settings отдаёт пути `config.yaml`/`secrets.json` —
  зона пользователя, как `projects.json`.


### 3.7. A5.7: что именно сделано (факты)

* **Сценарий §7 плана — целиком** (`tools\parity\out\agent_a42\a57_e2e.ps1`,
  отчёт `A57_E2E.md`): проект → образы (34 записи реестра, 32 `synced`,
  2 `accepted`, 1 `unsupported`); предпросмотр «Сверь реестр требований с ТЗ…
  письмо [PERSON_10|дат]» → `[PERSON_10|дат]`, «1С:ERP» не скрыт; ход агента из 7 запросов
  (`office_read` вне workspace → отказ гуарда; `office_read` реестра `xlsx`;
  `read_files` образа ТЗ `*.pdf.md`; `disk-search__search_local_files`;
  `office_edit` — столбец «Покрытие 1С:ERP»; `office_create` — письмо;
  финальный текст); лента — 68 событий, `text` со значениями, `raw` с метками,
  9 событий `redacted`; `sync --once` → `agent_changed=1 variants=1 changes=3`;
  письмо `06 Переписка\Письмо 03 Вопросы по покрытию.docx` в папке проекта
  (адресат в дательном падеже), вариант реестра в `pending` (`pending_accept`,
  `report.verdict=ok`), `compare` — 4 проверки валидатора; `hds agent result
  accept` → backup `20261121-105319`, оригинал заменён, образ пересоздан,
  `pending` пуст. **76/76 проверок** (`a57-checks.txt`).
* **Mock-провайдер нового образца** (`a57_mock.mjs`): шаг `{fromTools: "…"}`
  ищет имя MCP-инструмента **в `tools` пришедшего запроса** и пишет факт в
  `MOCK_RESOLVE`; тела запросов — в `MOCK_REQUESTS` (это же проверка I2).
  Получилось `search_local_files → disk-search__search_local_files`,
  `turn_end.tools` = `read_files, search_codebase, editor,
  disk-search__search_local_files, disk-search__index_status, office_read,
  office_edit, office_create, ask_user` (без `run_commands`/`fetch_web_content`).
* **Пробник инвариантов** (`a57_probe.py`): формы — из `Vault.project_labels`
  (AG-1, `keep_terms` исключены); скан — **целыми словами** и самым длинным
  совпадением (семантика фильтра), с исключением маркера зеркала, путей
  (`--nopath`) и типов меток (`--types`); `shas`/`hashdir` для снимков I6/I4;
  `docx`/`xlsx` для артефактов. Иначе скан даёт ложные «FAIL»: слово-значение
  внутри другой словоформы того же корня, короткое имя внутри полного ФИО,
  служебное слово внутри маркера `hds-agent/…`.
* **Найден и исправлен дефект панели настроек** (`crates/hds-ui/src/config_edit.rs`):
  на `config.yaml` с CRLF `apply_scalars` не узнавал `agent:\r` заголовком
  секции, дописывал вторую секцию `agent:` и `POST /api/agent/settings`
  отвечал `500 duplicate entry with key "agent"`. Теперь `\r` снимается при
  чтении, стиль переводов строк сохраняется; тест
  `agent_scalars_keep_crlf_and_single_section`. Сценарий проверяет
  `egress_log=true/false` — оба 200.
* **Инварианты** (`A57_E2E.md` §5): I2 — 0 форм в 7 запросах к модели и в
  9 файлах `egress`; I3 — отказ гуарда вне `workspace`; I4 — снимок `.cline`
  (2153 хешируемых файла) без изменений + отдельный `agent_acceptance`; I6 —
  изменился только принятый реестр, добавлено письмо; I8 — 0 форм в `sync.log`,
  `messages.jsonl`, ответах API. **I1 — 22 попадания в 12 образах** (и свежий
  образ того же оригинала — 5): образ строится только NER, словарного прохода
  нет; на стенде в базе есть метки для общих слов и частей названий. Это
  остаточный риск §5.9 плана; в облако значения не уходят (I2).
* **Стенд:** чистка `agent-state` (сессии и `*.bak` A5.6), `hds agent token P0001`
  (токен → `secrets.json`, потом ключ mock-провайдера), индекс зоны
  (`hds index --full`, 34 файла, 0 ошибок) — для этого `embedding.base_url`
  стенда переведён на локальный фасад `127.0.0.1:8011` (`dim: 1024`);
  `mcp_http.port: 8788` (8787 занят службой машины); `a57_restore.ps1` вернул
  стенд перед повтором (реестр из backup, письмо удалено, один проход зеркала).
* **Отчёт фазы** — `AGENT_A5_REPORT.md` (итог, под-этапы, контракты, инварианты,
  живой сценарий, проверки, что осталось, долги); README §20 — подраздел
  «Приёмка фазы A5».
* **Прогоны:** `cargo fmt --all --check` — чисто; `cargo clippy --workspace
  --all-targets -- -D warnings` — чисто; `cargo test --workspace --no-fail-fast --
  --test-threads=4` — 128 наборов / 918 passed / 13 ignored, единственный упавший
  набор — `agent_acceptance` (флейк `.cline`, одиночным прогоном зелёный);
  python 333 OK (skip 1); `npm test` 23/23. Логи — `tools/parity/out/a57-*.log`.

Ниже — то, что меняет видимое поведение вкладки. Дефолт принят, пока владелец не
сказал иначе; ответы фиксируются здесь же.

| # | Вопрос | Предложенный дефолт |
|---|---|---|
| В-A5-1 | Транспорт ленты: SSE или long-poll? | Сначала SSE: сервер обслуживает соединение в своём потоке — стриминг возможен; если окажется, что стриминговый ответ мешает обычным (таймауты, `max_conns`), — long-poll `?after=<seq>` с интерфейсом «события с номером». Решение фиксируется по факту в A5.3 |
| В-A5-2 | «Число упоминаний» в панели «Метки проекта» | Считать по данным, которые есть: число форм и число файлов реестра, где метка встречается (по `anon_rel`/поиску); точный счётчик вхождений — фаза 2 (нужен инкремент в `vault._add_form`) |
| В-A5-3 | «Разъединить» (отмена склейки) | Делать, но только для склеек, записанных в `merge_log` (таблица + `user_version`, данные не меняются). Склейки до A5.5 необратимы — это честно пишется в интерфейсе |
| В-A5-4 | Объём «Сравнить» | `Mirror::compare`: пути, размеры, sha256; md/txt — построчный diff; docx/xlsx — число абзацев/строк и изменённые ячейки через `hds_extract`. Полноценный Word-compare — фаза 2 |
| В-A5-5 | Где живут сессии агента | В процессе `hds ui` (`AgentRegistry`, `max_live_projects`, `idle_unload_min`). Перезапуск UI сессии теряет, история остаётся на диске и возобновляется по `session_id` (`history::read_messages`) |
| В-A5-6 | `ask_user` | Реализуется в A5.3: вопрос в ленту, ответ `POST /api/agent/answer`, таймаут 5 мин; «отменить» = отказ агенту. Сейчас `NoAskUser` отказывает всегда. **Ответ A5.3:** сделано, таймаут 600 с (`agent.ask_timeout_sec`), вопрос деанонимизируется в шлюзе |
| В-A5-7 | «Открыть» файл/вариант | UI не запускает процессы: показывает путь и копирует его. Открытие проводником/офисом — A6 вместе с правами (или отдельное решение) |
| В-A5-8 | Вкладка в server-профиле | 403 и скрытая кнопка (D20); многопользовательская вкладка — фаза 2 |

**Ответы A5.2** (приняты при разработке, действуют до решения владельца):

| # | Вопрос | Решение |
|---|---|---|
| В-A5.2-1 | Отправлять исходный текст или предпросмотр? | **Исходный**: шлюз анонимизирует сам, предпросмотр — только подтверждение; «Отправить» требует свежего предпросмотра |
| В-A5.2-2 | «Не скрывать» пишет в глобальные `keep_terms` | Да (D19); в UI подписано «действует во всех проектах» |
| В-A5.2-3 | Второй ход при активном | 409 «ход уже идёт», кнопка «Стоп»; очередь — фаза 2 |
| В-A5.2-4 | Продолжать последнюю сессию? | Да: `latest_session` при первом сообщении проекта; «Новая сессия» — A5.3/A5.6 |
| В-A5.2-5 | Файл вне папки проекта | Копия образа в `workspace\_attachments` (В-A5.2-5); в реестр не пишется (долг §4.2 п.16) |
| В-A5.2-6 | Лимит реплики | 20 000 символов (400 сверх) |

**Ответы A5.3** (приняты при разработке, действуют до решения владельца):

| # | Вопрос | Решение |
|---|---|---|
| В-A5.3-1 | Транспорт ленты | **SSE** + JSON-фолбэк (`?after=`); long-poll не делали. Ветка `run_http_listener` до `route_full`, `Content-Length` не пишется |
| В-A5.3-2 | Таймаут вопроса агента | 600 с, ключ `agent.ask_timeout_sec`; по таймауту модели возвращается отказ (`tool result`), ход продолжается, в ленте `ask_timeout` |
| В-A5.3-3 | «Открыть» в ленте | Показ пути + копирование (В-A5-7, без запуска процессов); подпись «путь проекта» / «путь образа» |
| В-A5.3-4 | Подписок на вкладку | Одна; смена проекта/скрытие вкладки закрывает поток, `after` — из `AG_SEQ` (`id:` кадра), при переподключении `Last-Event-ID` |
| В-A5.3-5 | Сколько событий в DOM | 500 (как кольцо), старые вытесняются; переключатель «как видит модель» перерисовывает из буфера событий |

**Ответы A5.4** (приняты при разработке, действуют до решения владельца):

| # | Вопрос | Решение |
|---|---|---|
| В-A5.4-1 | Объём «Сравнить» | Метаданные (пути, размеры, sha256, mtime), отчёт валидатора целиком и построчный diff: 200 строк / 2 000 символов, вход — 400 строк на сторону. Word-compare и ячеечный diff xlsx — фаза 2 (В-A5-4) |
| В-A5.4-2 | «Открыть» вариант | Показ пути и копирование в буфер (В-A5-7); открывается `pending\<rel>`, не оригинал |
| В-A5.4-3 | Кто читает отчёт валидатора | `validator::Report::read` в ядре; `pending` отдаёт `verdict` и число проверок, `compare` — все проверки с деталями (детали без значений, I8) |
| В-A5.4-4 | Подтверждение «Принять» | `confirm()` в браузере с упоминанием backup; сервер не переспрашивает — решение пришло с CSRF-заголовком |
| В-A5.4-5 | Ошибки решения | 400/404/409/503/500 (§3.4); «файл занят» — 409 с текстом A4, вариант и статус не меняются; панель показывает текст сервера и не «притворяется», что решение прошло |
| В-A5.4-6 | Что после решения | Событие `notice` в кольцо ленты (путь и исход, без содержимого) и обновление «Результатов», «Файлов проекта» и «Состояния проекта» |

**Ответы A5.5** (приняты при разработке, действуют до решения владельца):

| # | Вопрос | Решение |
|---|---|---|
| В-A5.5-1 | Какая метка «старшая» | Метка с наибольшим числом файлов в `occurrences`; при равенстве — первая отмеченная. `confirm()` показывает выбор; ручной выбор «оставить» — фаза 2 |
| В-A5.5-2 | Область «Разъединить» | Только склейки из `merge_log` (A5.5+). Для старых — ошибка 404 с текстом «склейка N не найдена — «Разъединить» возможно только для склеек, записанных после A5.5» (В-A5-3) |
| В-A5.5-3 | «Число упоминаний» | Формы значения + число файлов реестра, где метка есть в образе, и вхождения в каждом (В-A5-2); точный счётчик — фаза 2 |
| В-A5.5-4 | Что уходит агенту | `messages.jsonl`: `labels_merged`/`labels_unmerged`, только метки, `rel=""`; `notice` в ленту. Доставка в сессию — A5.6 |
| В-A5.5-5 | Что делать с вариантами при склейке | `pending_accept`/`conflict`/`agent_changed` не пересобираются — попадают в `skipped`; решение — в «Результатах» (A5.4) |
| В-A5.5-6 | «Не скрывать» в панели | Глобальный `keep_terms` (D19), кнопка у каждой формы, подпись «действует во всех проектах»; метка не удаляется — новые значения метку не получат |

**Ответы A5.6** (приняты при разработке, действуют до решения владельца):

| # | Вопрос | Решение |
|---|---|---|
| В-A5.6-1 | Где живут настройки проекта | Провайдер и модель — в `projects.json` (рядом с проектом, TR-8.1); общие пороги, фильтр и MCP — в `config.yaml` (`agent:`); ключ — только `secrets.json` |
| В-A5.6-2 | Что показывать про ключ | Только «задан/нет» (`key.set`). Хвост и маска значения не отдаются; поле ввода пустое («заменить ключ»); тесты проверяют, что значения нет в тексте ответа |
| В-A5.6-3 | Применение настроек | Смена ключа/провайдера/модели/`egress_log`/`unknown`/MCP/таймаутов — `AgentApi::unload` и `reopened: true`; панель пишет «сессия откроется заново», `agent.root` — только через `confirm()` |
| В-A5.6-4 | Что показывает «Очистить историю» | `{session, files, backups, replaced}` (пути и числа) без значений; бэкапы `*.json.bak` рядом с файлами остаются — удаление и «Вернуть» в A6 |
| В-A5.6-5 | Когда чистить историю | Только по кнопке (TR-6.6); автопроверка перед завершением сессии — A6 |
| В-A5.6-6 | Уведомления фильтра | Сводка по событиям `redacted` из кольца (метки и источник, без значений) + кнопки «Сбросить сводку» и «Очистить историю» в том же блоке; аудит `agent_egress_redacted` — как был, по `audit.enabled` |

**Ответы A5.7** (приняты при прогоне, действуют до решения владельца):

| # | Вопрос | Решение |
|---|---|---|
| В-A5.7-1 | Живой провайдер или mock? | **Mock** (`a57_mock.mjs`, OpenAI-совместимый): ключа облачного провайдера на стенде нет. Сценарий от провайдера не зависит — с живым меняются `projects.json`, `secrets.json` и `base_url`; это прямо сказано в `AGENT_A5_REPORT.md` §1 и README §20 |
| В-A5.7-2 | Значения в `<P>\agent-state` против формулировки I1 | `agent-state` — **зона агента** (история SDK), I1/I8 проверяются по `<agent.root>` **без** `agent-state`/`*.bak`/имён файлов; в отчёте это зафиксировано. Перенос бэкапов в `data\agent\` и правка формулировки PLAN — A6 |
| В-A5.7-3 | K1 (RAG «с/без») не выполнен | Замер **не делался**: `tools/parity/AGENT_RAG_K1.md` нет; RAG-вопрос в сценарии не проверялся. Долг зафиксирован (§4.2 п.1, §8 отчёта) с явной пометкой «без замера» (D23) |
| В-A5.7-4 | Внешние MCP `tavily`, `1c-buddy` | В сценарии не вызывались (нет ключа `tavily`, нет exe `1c-buddy`). При отсутствии ключа/команды сервер просто не появляется в `tools` — ход не падает; проверено косвенно: в `turn_end.tools` их нет, а `disk-search` есть |
| В-A5.7-5 | «Принять» на стенде | Да: стенд вымышленный, оригинал реестра заменён вариантом, копия — в `.hds-agent\backup\20261121-105319\`; I6 проверен по остальным оригиналам (изменений нет) |
| В-A5.7-6 | Куда писать приёмку в README | §20, подраздел **«Приёмка фазы A5»** перед «Что уходит наружу»; текст самого раздела вкладки не переписывался. `STATUS.md` в A5.7 не трогали — подходящего раздела нет |

### 4.2. Долги и открытые места (передаются по под-этапам)

1. **K1 (RAG «с/без») не выполнен** — предпосылка плана «после A3, до A5». Параллельно
   A5.1–A5.2, отчёт `tools/parity/AGENT_RAG_K1.md`; без него A5.7 закрывает фазу с
   `disk_search` включённым «по умолчанию» без замера (D23).
2. **`AgentRegistry` не подключён ни к CLI, ни к UI.** A5.2 — первый потребитель;
   проверить `reap_idle` на живой сессии (выгрузка по простою не должна рвать ленту).
3. **`hds-ui` → `hds-agent` (закрыто в A5.1).** Зависимость добавлена, ленивые
   фабрики воркеров переехали в `hds_agent::factory`; CLI и UI поднимают Python
   одинаково. Новых копий фабрик не заводить.
4. **«Метки проекта» без RPC с counters/aliases.** *(Закрыто в A5.5:*
   `vault_project_labels` отдаёт метки с формами и псевдонимами, журнал склеек
   и `keep`; `unmerge_entities` — «Разъединить»; в панели — формы + число
   файлов, точный счётчик вхождений остаётся фазой 2.)*
5. **«Разъединить» потребует `merge_log`** *(Закрыто в A5.5: таблица
   `merge_log` создаётся аддитивно, `SCHEMA_VERSION` не повышался — иначе D4
   переименовал бы базу и потерял метки. Склейки до A5.5 необратимы, об этом
   сказано в панели и в тексте 404.)*
6. **Точный счётчик упоминаний метки** — фаза 2 (В-A5-2). В панели «Метки
   проекта» (A5.5) — формы значения, число файлов и вхождения в каждом файле
   по тексту образа.
7. **Полный Word-compare** — фаза 2 (В-A5-4).
8. **Открытие файлов из UI** — A6/отдельное решение (В-A5-7).
9. **Многопользовательская вкладка «Агент»** — фаза 2 (D20: сейчас 403).
10. **Долги A4, которые видны во вкладке:** «Сравнить» (**закрыто в A5.4** — минимальный
    объём: метаданные, отчёт, текстовый diff), `hds agent result report --json` (**закрыто
    в A5.4** — `GET /api/agent/pending`/`compare` читают отчёт напрямую), журнал решений
    (п.19 — A5.6 или A6), relink по маркеру (п.13 — A5.7/фаза 2), «восстановить образ»
    для `orphan` (п.15 — **не сделано**, «Пересоздать образ» — A5.7/A6), «Вернуть» из
    backup (**«Очистить историю» закрыта в A5.6; «Вернуть» из бэкапа — A6**).
11. **История в `agent-state` содержит значения** (это зона агента): «Очистить историю»
    (**закрыто в A5.6** — кнопка в ленте и в настройках, отчёт с бэкапами). Бэкапы
    `*.json.bak` остаются с открытыми значениями — их удаление и «Вернуть» из бэкапа A6;
    автопроверка истории перед завершением сессии — A6.
12. **`sync.log` и пути пользователя** (§4.2 п.28 журнала A4): вкладка показывает
    журнал, значит вопрос «пути в зоне агента» становится видимым — решать в A5.1
    (показывать последние строки как есть) и фиксировать решение.
13. **A5.1: вкладка не скрывается сервером.** Страница — константа, профиль
    известен только API: кнопка скрывается JS после ответа 403. Если нужен
    серверный вариант «страница без вкладки» — отдельная разметка по профилю (A6).
14. **A5.1: удаления проекта в UI нет.** `projects::remove` есть, команды
    `hds agent project rm` (TR-8.5) — нет; A5.7/A6 вместе с кнопкой «Удалить».
15. **A5.1: реестр открывается на каждый запрос.** Для панели нормально (WAL,
    34 строки); при больших реестрах и частом опросе — кэш вместе с сессиями
    (A5.2). *(A5.2: кэш клиента анонимизации есть, реестр файлов по-прежнему
    открывается на запрос — окно «no action» до A5.4.)*
16. **A5.2: вложение вне папки проекта не попадает в реестр.** В `_attachments` лежит
    образ, но записи `kind=Attachment` нет: `orig_rel` такого файла вне папки проекта, и
    запись сломала бы обратный путь зеркала (A4). Пока файл виден только в тексте реплики.
    Решение — A5.4/A6 вместе с очередью вложений (нужен `orig_rel`-пространство для
    внешних файлов).
17. **A5.2: предпросмотр дороже одного RPC.** `preview` = `anonymize_text` (NER) + снимок
    vault + проход словаря (`egress_check`-семантика). На слабой машине это заметно; если
    станет тесно — кэшировать снимок на 5–10 с в `AgentApi` (снимок всё равно обновляется по
    `vault_rev` перед каждым запросом шлюза).
18. **A5.2: `label` без типа — `OTHER`.** Для ФИО интерфейс передаёт `type=PERSON`
    («Скрыть ещё…» в A5.5 получит выбор типа); иначе метка будет `[OTHER_n]` и падеж может
    быть не определён (`confident=false`).
19. **A5.2: события — опрос, не SSE.** `GET /api/agent/events` отдаёт кольцо (500) по
    `after`; UI опрашивает раз в 1,2 с, пока `busy`. A5.3 переведёт тот же маршрут на
    `text/event-stream` (нужна отдельная ветка записи без `Content-Length` — грабля §6 п.1).
    *(A5.3: **закрыто** — SSE-ветка в `run_http_listener`, `Condvar` в `Turn`, JSON-фолбэк
    остался для тестов/`curl.exe`/страниц без `EventSource`.)*
20. **A5.3: вопрос агента живёт в памяти UI.** `AskState` — на проект в процессе `hds ui`;
    перезапуск UI висящий вопрос теряет (воркер получает ошибку по закрытию канала). Диалог
    вопросов между пользователями серверного профиля — не нужен (вкладка там 403, D20);
    «очередь вопросов» — фаза 2.
21. **A5.3: `ask_timeout` берётся из настроек первого хода проекта.** `GatewayHost` создаётся
    при открытии шлюза и переживает ходы; смена `agent.ask_timeout_sec` подхватится новым
    процессом воркера (выгрузка по простою или «Стоп» + следующий ход). Панель настроек —
    A5.6, там же можно пересоздавать шлюз.
22. **A5.3: SSE-поток не переживает выгрузку проекта из реестра** — он отдаёт кольцо из
    `AgentApi`, поэтому выгрузка шлюза (`reap_idle`) поток не рвёт; закрытие — по 300 с и
    переподключение. Одна подписка на вкладку (`EventSource` сам гасит дубли при смене
    проекта — `es.close()` в `agentStreamClose`).
23. **A5.3: «как видит модель» для `ask` и `prompt` без `raw`.** У `prompt` нет `raw` (реплика
    подтверждена в том виде, в каком уйдёт), у `ask` — вопрос приходит в шлюз уже
    деанонимизированным. Лента показывает `text`; метки видны в соседних `tool_use`/`tool_result`.
24. **A5.4: `hds_extract` не отдаёт заголовки docx как текст.** `extract_docx` использует
    абзацы-заголовки только как `head` пути сегмента, в `text` их нет: правка заголовка
    проходила (`office_edit applied=1`), а diff был пуст. Поэтому «Сравнить» для docx
    показывает изменения **тела**; заголовки в diff — долг (нужен флаг сегмента или
    включение заголовка в текст). Найдено на стенде (`A54_E2E.md` §4.1).
25. **A5.4: `/compare` для docx требует воркер `hds_extract`.** Нет воркера — сравнение
    отдаёт метаданные и отчёт валидатора с `diff_note` («… не прочитан»), а не 500:
    панель честно пишет, почему diff пуст.
26. **A5.4: реестр по-прежнему открывается на каждый запрос.** `pending`/`compare`/`decision`
    тоже открывают `ArtifactRegistry` (и поднимают ленивые воркеры решения). Для 34 записей
    нормально; кэш — вместе с сессиями в A5.6.
27. **A5.4: журнал решений и «Вернуть» из backup не сделаны.** `notice` о решении живёт в
    кольце ленты (память UI), backup — на диске, но кнопки восстановления нет: A5.6/A6
    (п.10). Копии `_conflicts` тоже без своего списка — видны только в `compare`/`notice`.
28. **A5.4: «Пересоздать образ» для `orphan` не сделана** (долг A4 п.15, п.10). Панель
    «Результаты» показывает только `pending_accept`/`conflict`; `orphan` — A5.7/A6.
29. **A5.4: `pending` не отдаёт детали проверок.** Чтобы не дублировать отчёт в списке, там
    только `verdict`, число проверок и `rev`; детали — в `/compare`. Если понадобится
    «проверить» без сравнения, добавить `checks` в `pending` (объём — 4 строки на вариант).
    Если понадобится и здесь «метки» — `AskUser::ask` должен получать оба вида (фаза 2).
30. **A5.6: реестр живых создаётся при старте `hds ui`.** `max_live_projects` из панели
    пишется в `config.yaml`, но действующее значение остаётся прежним до перезапуска —
    в ответе POST есть `warnings`. Пересоздавать реестр на ходу не стали: живые сессии
    пришлось бы выгружать без предупреждения (A6 — вместе с настройками UI).
31. **A5.6: слияние `secrets.json` не умеет удалять ключи.** Панель только задаёт
    значения; «снять» `base_url` пишет `null` (для `provider()` это «нет значения»).
    Удаление ключа — отдельное действие A6 (там же маска/хранение).
32. **A5.6: редактор `config.yaml` правит только `agent:`.** Другие секции (индекс,
    транскрибация) идут прежними редакторами (`config_edit::set_roots` и т.п.); общий
    редактор «любой ключ» не заводили — панель agent-настроек не должна трогать чужие
    разделы.
33. **A5.6: бэкапы истории остаются с открытыми значениями.** `*.json.bak` — копия
    прежней истории (В-A5.6-4), т.е. значения в `agent-state` остаются до удаления
    бэкапа; это зона агента (I1), но при проверке I8 файлы `*.bak` нужно исключать.
    Кнопки «удалить бэкапы» и «Вернуть» — A6.
34. **A5.6: панель не проверяет, что ключ рабочий.** `POST settings` только записывает
    значение; проверка «ключ принимается провайдером» — `hds agent check`/A6 (запрос к
    провайдеру из UI противоречил бы «UI не ходит в сеть»).

**A5.7: что закрыто, что перенесено в A6 и что нового (по итогам живого прогона).**

35. **Закрыто в A5.7 — панель настроек на `config.yaml` с CRLF.** `apply_scalars`
    не узнавал `agent:\r` заголовком секции, дописывал вторую секцию `agent:` и
    `POST /api/agent/settings` отвечал `500 duplicate entry with key "agent"`.
    Исправлено (`\r` снимается при чтении, стиль переводов строк сохраняется),
    тест `agent_scalars_keep_crlf_and_single_section`; сценарий A5.7 проверяет
    `egress_log=true/false` — оба 200. Грабли — §6 п.65.
36. **Новый долг (A6/фаза 2): образы содержат формы, которые NER не выделил.**
    I1 на стенде — 22 попадания в 12 образах из 34 (свежий образ того же
    оригинала — 5): `anonymize` в режиме `agent` строит образ **только NER**, без
    словарного прохода; метки на общих словах и частях названий
    (`[ORG_9]`, `[ORG_12]` — родовые слова, `[POSITION_5]` — должность общего
    вида, `[PASSPORT_1]` — строка теста прошлого прогона) добавляют попаданий.
    В облако значения не уходят (I2 = 0 в запросах и `egress`), но в самих
    образах они видны. Лечение: словарный проход при сборке образа **или**
    пересборка образов при появлении новой метки; гигиена меток (общие слова не
    должны становиться метками). Формулировку I1 в PLAN не меняли.
37. **Новый долг (A6): метка на общем слове портит пути в ленте.** `[POSITION_6]`
    (родовое английское слово) закрывает слово в пути зоны, и деанонимизация
    возвращает его с заглавной (`…\out\Agent_a42\zone\…`). Значения не утекают,
    но путь выглядит чужим; лечится п.36 (гигиена меток).
38. **Новый долг (A6): путь образа в результате `disk-search` остаётся путём
    зоны.** `mcp_in` закрывает системные строки метками (`[SYS_n]`), поэтому
    `deanon` не может сопоставить строку с `workspace` и не заменяет путь путём
    проекта. Безопасно (путь локальный), но в ленте лучше путь проекта.
39. **Открыто (A6), подтверждено прогоном:** «Вернуть» из backup и журнал
    решений (п.10, п.27), `orphan`/«Пересоздать образ» (п.28), автопроверка
    истории (п.11), удаление проекта в UI (п.14), кэш реестра (п.15, п.26),
    `hds agent check` и проверка ключа (п.34). Ничего из этого в A5.7 не
    реализовывалось — по границам под-этапа.


## 5. Контракты A5 (не ломать)

| Контракт | Правило |
|---|---|
| **Маршруты `/api/agent/*`** | loopback/CSRF-гейт как у остальных POST (`csrf_ok`, заголовок `X-HDS-UI`, `Content-Type: application/json`); в `is_user_route` **не** входят (это не клиентский маршрут); в server-профиле — 403 (D20) |
| **Значения в HTTP** | UI — зона пользователя: ответы вкладки могут содержать значения (лента, предпросмотр после подтверждения). Но предпросмотр и «как видит модель» — **метки**; журналы, аудит, `sync.log`, `messages.jsonl` — без значений (I8) |
| **Секреты** | только `data\agent\secrets.json` через `hds_agent::secrets`; в `config.yaml` — никогда; в ответах API — маска и признак «задан/нет», а не значение |
| **Решения** | только `hds_agent::results`; UI не заменяет оригиналы сам, «файл занят» — `ResultsError::Busy` |
| **Статусы файлов** | только `registry_db` (`list`, `list_pending`, `by_orig`); UI не вычисляет статус из наличия файла |
| **Воркер агента** | только через `Gateway` (процесс, `hello`, `send`, `next_event`, `abort`, `stop`, `shutdown_with`); один активный ход на проект; UI не читает stdout воркера напрямую |
| **Деанонимизация** | только `hds_agent::deanon` (единая реализация для событий и текста); своих регулярок меток в UI быть не должно |
| **Метки** | разбор — `hds_core::labels`; падеж — `gram_to_suffix`; в именах файлов суффикса нет |
| **Проекты** | `projects.json` (`hds_agent::projects`) — единственный список; `path` — абсолютная папка пользователя; `id` = `P`+4 цифры (D21) |
| **Процессы ОС** | UI не запускает процессы и не открывает файлы (В-A5-7); максимум — путь в буфер обмена |
| **Фильтр** | снимок по `vault_rev` перед каждым запросом (`RefreshState`); fail-closed `privacy_filter_unavailable`; срабатывания — `take_redactions()` в уведомления |
| **История** | `hds_agent::history`; «Очистить историю» = `Gateway::clear_history` (стоп → метки → возобновление), а не правка файлов вручную |

## 6. Грабли (оплачены в A0–A4; для A5 добавлены свежие)

1. **HTTP-сервер пишет `Content-Length` и `Connection: close`.** SSE потребует
   отдельной ветки записи (без длины, flush на событие); обычные ответы не трогать —
   их проверяют `ui_core`/`ui_auth`.
2. **Поток на соединение и `max_conns` (по умолчанию 32).** SSE занимает соединение;
   браузер держит 1–2 на вкладку — не открывать поток на каждый запрос и не плодить
   подписки на проект.
3. **`Gateway::next_event` блокирующий.** Только в pump-потоке; HTTP-обработчик не
   должен ждать ход, иначе вкладка «висит» (и `read_timeout` рвёт запрос).
4. **`route` в тестах вызывается без заголовков** (`ReqHeaders::default()`): любой POST
   `/api/agent/*` обязан требовать CSRF и отдавать 403 — это же и есть тест.
5. **`OnceLock`-состояние UI не сбрасывается между тестами.** Держать состояние в
   структуре с конструктором для тестов (как `bind_from_config`), а глобальный
   `OnceLock` — только обёртка в проде.
6. **`AgentRegistry` выгружает сессию по простою** (`idle_unload_min`): после выгрузки
   лента не должна «умирать» — `session_id` возобновляется через `history`.
7. **Значения в `agent-state` — норма** (история SDK), поэтому панель «Метки проекта»
   показывает **метки и формы**, не историю; и никакие значения не уходят в журналы.
8. **`HDS_ROOT`/`HDS_CONFIG` из env стенда ломают `cargo test`** (A4 §6 п.12): снимать
   перед прогоном; UI-тесты — со своим временным конфигом.
9. **PowerShell 5.1 портит кириллицу в командной строке** (A4 §6 п.2): пути и значения
   — из переменных/файлов, не литералами.
10. **`Invoke-RestMethod` не умеет SSE:** проверять ленту `curl.exe -N` или
    `StreamReader` из `System.Net.Http`.
11. **Mock-провайдер — Node-скрипт** (`crates/hds-agent/tests/gateway_worker.rs`,
    `MOCK_JS`): в e2e UI поднимать его в тесте, `base_url` — через `secrets.json`/
    `projects.json`, не через `config.yaml`.
12. **`hds ui` читает конфиг при старте** (`bind_from_config`, `agent.root`): смена
    проекта/настроек в UI не должна требовать перезапуска страницы, но перезапуск UI
    подхватывает конфиг — это документируется, не «лечится» на ходу.
13. **Правило «одна запись — одна строка»** (A4.6): уведомления и отчёты в UI тоже не
    должны склеиваться в одну строку журнала.
14. **Тесты идут параллельно в одном процессе** — временные каталоги с
    `AtomicU32`-счётчиком; env — под общим мьютексом (как `ENV_LOCK` в CLI-тестах).
15. **PowerShell 5.1 `Invoke-RestMethod` декодирует JSON как Latin-1** (в
    `Content-Type` нет `charset`): кириллица выглядит «мохнатой», хотя сервер
    отдаёт UTF-8. Проверять через `Invoke-WebRequest` +
    `[Text.Encoding]::UTF8.GetString($r.RawContentStream.ToArray())` (A5.1,
    `A51_E2E.md` §6); страницу смотреть в браузере.
16. **`insert_line` в `projects.rs` попал между `#[derive(...)]` и `struct`** —
    константы получили атрибут derive (`derive may only be applied to structs`).
    Константы вставлять до атрибутов (продолжение A4 §6 п.30).
17. **Страница `hds ui` статична**: профиль и конфиг на разметку не влияют,
    «скрыть в серверном профиле» можно только JS-ом (или отдельной страницей).
18. **`agent_acceptance` (I4) не любит параллельный прогон.** Он сравнивает снимок
    `%USERPROFILE%\.cline`; IDE/Cline во время теста пишет туда — «профиль .cline
    изменился». Это не регрессия кода: гонять `cargo test -p hds-cli --test
    agent_acceptance` отдельно или `--test-threads=4` на тихой машине.
19. **A5.2: `preview` без словарного фильтра расходится с отправкой.** `anonymize_text`
    выделяет не всё: «Иванову» при метке на «Иван» остаётся, а `egress_check` при отправке
    его заменяет. Поэтому `preview` и событие `prompt` прогоняют `Filter::redact(Point::Model)`
    с тем же `label_for_value`; без этого TR-5.1 нарушается (проверено на стенде — до правки
    в предпросмотре было видно «Иванову»).
20. **A5.2: папка проекта — системная строка.** Абсолютный путь файла проекта после фильтра
    становится `[SYS_n]\…`, поэтому `paths` в предпросмотре считаются по исходному тексту и
    отдают `anon` — путь образа относительно `workspace` (cwd воркера), а не абсолютный.
    «Вложить файл…» вставляет именно `anon`/`_attachments\<имя>`.
21. **A5.2: `Start-Process -RedirectStandardOutput` держит файл открытым.** Порт mock-провайдера
    читать с `[IO.FileShare]::ReadWrite` (`[IO.File]::Open(..., 'Read', 'ReadWrite')`), иначе
    «process cannot access the file… used by another process».
22. **A5.2: PS 5.1 теряет тело 4xx.** `Invoke-WebRequest` бросает исключение, а поток ответа уже
    прочитан — тело 409 брать из `$_.ErrorDetails.Message` (иначе артефакт пустой).
23. **A5.2: `ProxyLabeler`/`ClientLabeler` — не в UI.** Новые метки выдаёт `label_for_value`
    воркера (`OTHER`); в `agent_session` это `PreviewLabeler` (10 строк), а не своя регулярка.
24. **A5.2: насос и отправка делят замок реестра.** `next_event(500 мс)` держит
    `Mutex<AgentRegistry>` на время опроса — `message` ждёт не дольше периода опроса. Не
    увеличивать `POLL` без нужды и не звать `next_event` из HTTP-обработчика (§6 п.3).
25. **A5.3: поток открывается раньше первого хода.** `stream_events` не должен выходить на
    `existing_turn == None`: браузер подписывается до отправки реплики, и поток умирал
    (найдено на стенде — «sse state: True», а событий нет). Ждём хода (250 мс) с `: ping`
    каждые 15 с; закрываемся, только если ход был и исчез (сброс состояния), либо по 300 с.
26. **A5.3: `AskUser::ask` блокирует поток чтения воркера.** Поэтому `AskState` не берёт ни
    замок реестра, ни фильтр (только свой `Mutex`+`Condvar`), а `abort`/`stop` **сначала**
    снимают вопрос (`turn.cancel_ask()`), и только потом пишут команду воркеру.
27. **A5.3: вопрос и таймаут — разные вещи.** По таймауту `ask` возвращает `Err`, воркер
    получает отказ как `tool result` и продолжает ход; в ленте `ask_timeout`, а не `error`.
    По «Стоп» — `ask_cancelled`. По таймауту и отмене вопрос снимается, иначе поздний
    `POST /answer` «повис» бы в состоянии до следующего хода.
28. **A5.3: `Accept` и `Last-Event-ID` надо парсить в `read_request`.** `ReqHeaders` получил
    поля `accept`/`last_event_id`; `EventSource` при первом подключении `Last-Event-ID` **не**
    шлёт — `after` идёт в query, а на реконнекте сервер обязан принять заголовок (он важнее).
29. **A5.3: `curl.exe` из PowerShell.** `Start-Process -ArgumentList @('-H','Accept: text/event-stream')`
    разваливает заголовок на два аргумента (кавычки не ставятся) → сервер отдаёт JSON-фолбэк.
    Передавать аргументы одной строкой с кавычками и `-i`, если нужны заголовки в артефакте;
    `Invoke-WebRequest` на SSE «висит» (грабля §6 п.10).
30. **A5.3: живые логи нельзя читать `ReadAllText`.** UI/mock держат файл
    (`RedirectStandardOutput`) — только общий доступ (`FileShare.ReadWrite`), иначе
    «used by another process» и проверки сценария не выполняются (продолжение A5.2 §6 п.21).
31. **A5.3: путь `workspace` в конфиге может быть с `/`.** `DeAnonymizer::replace_paths`
    обязан пробовать оба разделителя — иначе в ленте остаётся путь зоны (стенд
    `agent.root: 'C:/…/zone'`, а модель присылает `C:\…\zone\…`).
32. **A5.3: `OnceLock` состояния вкладки в тестах.** Сокет-тесты ходят через `global(cfg)`;
    корень поставки у теста — репозиторий, а не `HDS_ROOT`, поэтому состояние ставится
    вручную (`agent_session::install_global`), иначе `plan_with` не найдёт `dist/worker.mjs`.
    Порядок тестов в бинаре не задан — каждый сокет-тест начинается с `reset_for_tests()`.
33. **A5.4: пути из реестра — с `\`, из JSON и маршрутов — с `/`.** `Mirror::decide`/`compare`
    нормализуют путь (`normalize_rel`), но тесты и сценарии сравнивают строки: приводить
    разделители (`-replace '[/\\]', '\'`), иначе «нет записи в реестре».
34. **A5.4: PowerShell 5.1 + `$ErrorActionPreference='Stop'` + `2>&1`.** Предупреждения Python
    в stderr (`pymorphy2`/`pkg_resources`) превращаются в завершающую ошибку `NativeCommandError`.
    В сценарии `SyncOnce` временно ставит `Continue` и возвращает прежнее значение.
35. **A5.4: `python -c` из PowerShell 5.1 теряет кавычки.** `print("\n".join(...))` приходит
    в Python без кавычек (`SyntaxError: unexpected character after line continuation`);
    в сценарии — `chr(10)` вместо строкового литерала с кавычками.
36. **A5.4: занятый файл откатывается ~1 с.** `replace_file` делает 20 попыток по 50 мс —
    тест «файл занят» идёт около секунды; это не таймаут и не повод уменьшать попытки
    (иначе Word, отпустивший файл через 200 мс, не даст сохранить).
37. **A5.4: mock-воркер анонимизации — общий фикстурный файл.**
    `crates/hds-ui/tests/fixtures/mock_anon.py` подключается через `include_str!` и из
    `agent_chat.rs`, и из `agent_results.rs`; второй копии скрипта не заводить. Для
    `Mirror` в нём нужны `anonymize_path`, `image_map`, `anonymize` и `vault_snapshot`.
38. **A5.4: `notice` заводит состояние хода.** `AgentApi::notice` создаёт `Turn` проекта,
    даже если чата не было: `busy=false`, событий — одно. Не считать это «ходом идёт».
39. **A5.5: `merge_log` создаётся аддитивно, `SCHEMA_VERSION` не повышается.**
    `Vault._migrate_if_needed` при другой версии **переименовывает** базу (D4) и
    теряет метки стенда. Новая таблица — `CREATE TABLE IF NOT EXISTS` в
    `_init_schema`, как `projects`/`aliases`.
40. **A5.5: пересборка после разъединения — по старшей метке.** `restored`-меток
    в образах уже нет (их заменил NER после склейки), поэтому
    `refresh_label_images(&[keep])`: образы пересобираются из оригиналов, и
    младшая метка возвращается сама. Искать `restored` в образах бесполезно —
    получится пустой `rebuilt`.
41. **A5.5: `drop` всегда приходит со страницы вместе со старшей меткой.**
    `merge_request` вычитает `keep` из `drop`, поэтому «одна метка» — это пустой
    `drop` **после** вычитания (400), а не пустой массив в теле.
42. **A5.5: значения форм — только в ответе панели.** `vault_project_labels`
    отдаёт формы значениями (зона пользователя, как `snapshot`), но
    `messages.jsonl`, `sync.log` и `notice` содержат только метки (I8). В
    сокет-тесте это проверяется явно (`!messages.contains("Петров")`).
43. **A5.5: не поднимать второй Python.** `with_mirror` (A5.4) создаёт
    `LazyAnonymizer` — для меток это второй процесс на тот же `anonymize.db`.
    `with_client_mirror` берёт уже открытый `AgentApi::client`; замок держится
    только на пересборку+проход, поэтому «Стоп» и реплика не ждут меток.
44. **A5.5: в E2E-сканировании I8 пути — не утечка.** `rebuilt[].rel/anon` —
    путь папки проекта и workspace; значение в имени файла там законно (на
    стенде «1С ERP» есть и в формах `[ORG_1]`, и в имени `.pdf.md`). Сценарий
    сканирует текст сообщения, метки, `sync.log` и `messages.jsonl`, а пути из
    проверки исключает. `forms_before` в `merge_log` — значения форм старшей, и
    это допустимо **только внутри `anonymize.db`** (I1); в логах и сообщениях
    их нет.
45. **A5.6: `secrets.json` пишется только слиянием.** Прямая запись `self.raw` поверх
    файла теряет `mcp.servers` и ключи, появившиеся после `load` (CLI правит тот же
    файл). `Secrets::save` сначала читает диск, рекурсивно сливает объекты и только
    потом `replace_file`; удалений нет — «снять» значение можно только `null`.
46. **A5.6: `config.yaml` — temp + `replace_file`, и проверка после правки.** Одного
    «файл разбирается» мало: `set_agent_values` сверяет **каждый** путь через `dig`
    с разобранным литералом, иначе опечатка в типе (`'601'` вместо `601`) оставила бы
    панель с одним значением, а конфиг с другим.
47. **A5.6: CSRF пропускает POST с `Content-Type: application/json` без `X-HDS-UI`.**
    Поэтому «403» проверяется запросом **без** Content-Type и без `X-HDS-UI`
    (в сокет-тесте — `ReqHeaders::default()`, в PowerShell — сырой `TcpClient`:
    `Invoke-WebRequest` сам добавляет Content-Type).
48. **A5.6: `api.stop` при закрытом шлюзе даёт 404.** Поэтому маршрут «Очистить
    историю» при `busy` и неудачном `stop` отдаёт **409** (а не чистит молча).
    Проверка 409 в тесте — `Turn::set_busy_for_tests` + отсутствие живого шлюза.
49. **A5.6: `agent-state` — в зоне, а не в `HDS_ROOT`.** `<agent.root>\P0001\agent-state`;
    на стенде это `zone\P0001\agent-state`. Сценарий, читающий `root\P0001\...`, падает
    на `ReadAllText` (A56, первая попытка).
50. **A5.6: `a56_e2e.ps1` — UTF-8 с BOM.** PowerShell 5.1 без BOM читает `.ps1` как
    cp1251: русские подписи проверок превращаются в мусор (а сравнения строк ломаются).
    Редактор сохраняет без BOM — перед запуском конвертировать
    (`[IO.File]::WriteAllText($p,$t,(New-Object Text.UTF8Encoding($true)))`).
51. **A5.6: бэкапы истории (`*.json.bak`) содержат значения.** I8-скан `agent-state`
    обязан их исключать (это копия прежней истории, В-A5.6-4); в ответах и логах
    значений нет.
52. **A5.6: после правки конфига его нужно читать заново.** `config::load()` кэша не
    держит (кэш — у `load_cached`), поэтому `settings_body` строится по свежему
    `config::load()`; иначе POST вернул бы старые значения.
53. **A5.7: `config.yaml` с CRLF ломал панель настроек.** `apply_scalars` делит текст по
    `\n`; у файла с CRLF первая строка — `agent:\r`, `is_section_header` её не узнаёт,
    редактор дописывает **вторую** секцию `agent:` и `POST settings` отдаёт
    `500 duplicate entry with key "agent"`. Лечение: снимать `\r` при чтении и
    сохранять стиль переводов строк (A5.7, тест `agent_scalars_keep_crlf_and_single_section`).
    Файлы стендов из старых прогонов — LF, поэтому дефект не проявлялся до тех пор,
    пока конфиг не записал редактор Windows.
54. **A5.7: имя MCP-инструмента не угадывать.** В запросе к модели MCP-инструменты
    называются `<сервер>__<инструмент>` (`disk-search__search_local_files`), и
    набор зависит от живого `hds mcp-http` и токена проекта. Mock-провайдер
    разрешает имя по суффиксу **в `tools` пришедшего запроса** (`fromTools` в
    `a57_mock.mjs`) и пишет факт в `MOCK_RESOLVE`; `turn_end.tools` — контроль.
55. **A5.7: аргументы `search_local_files` — `kinds` строкой.** Схема маршрута A2:
    `kinds` через запятую (`"docx,pdf,xlsx"`), `limit` — число. Массив `kinds`
    отклоняется валидацией.
56. **A5.7: скан инвариантов — семантика фильтра, иначе ложные «FAIL».** `value in text`
    ловит слово внутри другой словоформы того же корня, короткое имя внутри полного
    ФИО и служебное слово внутри маркера `hds-agent/<P>/<id>`. Правильно: границы
    слова, самое длинное совпадение, `ё→е`, исключение маркера, имён файлов
    (`*.bak`, `agent-state`), путей при скане журналов (`--nopath`) и типов меток
    (`--types`) — как в `hds-agent::filter`.
57. **A5.7: образы содержат формы, которые NER не выделил, — это не регрессия.** Режим
    `agent` строит образ только NER; словарного прохода нет. I1 в буквальном
    прочтении («`<agent.root>` — 0») на стенде не выполняется (22 попадания в
    12 образах; свежий образ того же оригинала — 5). Гарантия для облака — фильтр
    (I2 = 0 в запросах и `egress`, 9 событий `redacted`). Долг A6/фазы 2 —
    §4.2 п.36.
58. **A5.7: `hds mcp-http` требует подкоманду, и 8787 может быть занят.** Без
    подкоманды бинарь пишет «нужна подкоманда» и выходит; на этой машине порт 8787
    держит экземпляр службы (PID 4768, запуск 16.11, PID-файла в стенде нет,
    `taskkill` — «отказано в доступе»), и он отдаёт `{"error":"not found"}` на
    `/mcp/agent/P0001` (в его конфиге нет `agent.enabled`). Стенд получает свой
    порт (`mcp_http.port: 8788`) — иначе `disk-search` у агента не появится.
59. **A5.7: индекс зоны требует доступного эмбеддера.** С `embedding.base_url` на
    закрытый порт (`127.0.0.1:9`) каждый файл падает за ~20 с
    («Эмбеддинги недоступны») и `hds index --full` не заканчивается за разумное
    время. Для стенда — локальный фасад llm-host (`127.0.0.1:8011`, роль
    `embedding`, `dim: 1024`), иначе `disk-search` вернёт пусто.
60. **A5.7: `param()` в `.ps1` — первой инструкцией.** Скрипт с `$ErrorActionPreference`
    до `param(...)` падает на `param: не распознано как имя командлета`. Переводы
    строк — только после `param`; файл — UTF-8 с BOM (грабля 50).


## 7. Окружение и команды

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager status; git --no-pager log -3 --oneline
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p hds-ui
cargo test -p hds-agent
cargo test --workspace -- --test-threads=4 *> tools\parity\out\a5-cargo-test.log
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

Стенд UI — тот же, что у A4 (`tools\parity\out\agent_a42`, 34 записи реестра):

```powershell
$env:HDS_CONFIG='<repo>\tools\parity\out\agent_a42\config.yaml'
$env:HDS_ROOT='<repo>\tools\parity\out\agent_a42\root'
target\debug\hds.exe ui --port 8799
# вторая консоль: маршруты вкладки
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/projects'
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/state?project=P0001'
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/files?project=P0001'
$h=@{'X-HDS-UI'='1';'Content-Type'='application/json'}
# A5.2: Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/preview' -Method Post -Headers $h -Body (...)
# A5.2: события — GET /api/agent/events?project=P0001&after=<seq> (кольцо 500, JSON)
# A5.3: лента — curl.exe -N 'http://127.0.0.1:8799/api/agent/events?project=P0001&after=0'
# A5.3: поток SSE с заголовками (`-i` — чтобы видеть статус и Content-Type):
curl.exe -N -s -i -H 'Accept: text/event-stream' 'http://127.0.0.1:8799/api/agent/events?project=P0001&after=0'
# A5.3: ответ на вопрос агента (ask_id — из кадра `event: item` с `"kind":"ask"`):
# POST /api/agent/answer {"project":"P0001","ask_id":1,"answer":"..."} + X-HDS-UI: 1
# A5.4: варианты, ожидающие решения (pending_accept + conflict):
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/pending?project=P0001'
# A5.4: сравнение (rel — относительный путь оригинала, кодировать как Uri):
$rel=[Uri]::EscapeDataString('06 Переписка\Письмо 01 Согласование устава.md')
Invoke-RestMethod "http://127.0.0.1:8799/api/agent/compare?project=P0001&rel=$rel"
# A5.4: решение (тело — UTF-8 байтами; без X-HDS-UI/Origin — 403):
$body=@{project='P0001';rel='06 Переписка\Письмо 01 Согласование устава.md';decision='reject'}|ConvertTo-Json
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/decision' -Method Post -Headers $h `
    -Body ([Text.Encoding]::UTF8.GetBytes($body))
Remove-Item Env:HDS_ROOT, Env:HDS_CONFIG -ErrorAction SilentlyContinue   # перед cargo test
```

Тесты A5.2 (без воркеров и с ними):

```powershell
cargo test -p hds-agent run::                # помощники плана хода
cargo test -p hds-ui agent_session::         # кольцо, after/limit, лимит текста
cargo test -p hds-ui --test agent_chat       # живой воркер + mock-провайдер + mock-анонимайзер
```

Тесты A5.3 (то же + транспорт и вопрос):

```powershell
cargo test -p hds-agent gateway:: ask_user   # деанонимизация вопроса в шлюзе
cargo test -p hds-agent deanon::             # пути workspace в оба разделителя
cargo test -p hds-ui agent::                 # sse_frame, wants_stream, stream_after
cargo test -p hds-ui --test agent_chat       # SSE на сокете, ask_user, abort при вопросе
```

Тесты A5.4 (сравнение, разбор решения, маршруты на сокете):

```powershell
cargo test -p hds-agent results::            # compare, Decision::parse, решения A4
cargo test -p hds-ui agent::                 # results_err → коды 400/404/409/500
cargo test -p hds-ui --test agent_results    # pending/compare/decision, busy, CSRF, server
```

Тесты A5.5 (метки проекта, склейка, разъединение):

```powershell
cargo test -p hds-agent labels::             # report/occurrences, message, отчёт зеркала
cargo test -p hds-agent results::            # refresh_label_images, append_message, MessageKind
cargo test -p hds-anonymize --test client    # project_labels/unmerge на mock-воркере
cargo test -p hds-ui --test agent_labels     # сокет: GET labels, merge, unmerge, keep, 400/403
sidecar\venv-anonymize\Scripts\python.exe -m unittest sidecar.tests.test_anonymize_vault -v
```

Тесты A5.6 (настройки, «Очистить историю», редактор конфига):

```powershell
cargo test -p hds-agent secrets::            # save: слияние, отсутствующий файл
cargo test -p hds-ui config_edit::           # agent_scalars_*: комментарии, подсекции
cargo test -p hds-ui agent::                 # settings_request / history_request
cargo test -p hds-ui --test agent_settings   # сокет: GET/POST settings, history/clear, 400/404/409/403
cargo test -p hds-cli                        # hds agent history clear не сломан
```

Маршруты A5.5 на стенде (`$h` — `@{"X-HDS-UI"="1";"Content-Type"="application/json"}`):

```powershell
# A5.5: метки проекта (формы — значения, зона пользователя; кириллица в ответе — UTF-8)
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/labels?project=P0001'
# A5.5: склейка: тело — UTF-8 байтами, иначе кириллица меток ломается
$body=@{project='P0001';keep='[PERSON_1]';drop=@('[PERSON_2]')}|ConvertTo-Json
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/labels/merge' -Method Post -Headers $h `
    -Body ([Text.Encoding]::UTF8.GetBytes($body))
# A5.5: разъединение — только склейки из merge_log (номер из GET labels → merges[].id)
$body=@{project='P0001';merge_id=1}|ConvertTo-Json
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/labels/unmerge' -Method Post -Headers $h `
    -Body ([Text.Encoding]::UTF8.GetBytes($body))
```

Маршруты A5.6 на стенде (`$h` — те же заголовки; тела — UTF-8 байтами):

```powershell
# A5.6: настройки — ключ только признаком «задан/нет»
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/settings?project=P0001'
$body=@{project='P0001';model='mock-model-2';api_key='sk-test-a56';ask_timeout_sec=601}|ConvertTo-Json
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/settings' -Method Post -Headers $h `
    -Body ([Text.Encoding]::UTF8.GetBytes($body))
# A5.6: очистка истории (без session — последняя сессия проекта)
$body=@{project='P0001'}|ConvertTo-Json
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/history/clear' -Method Post -Headers $h `
    -Body ([Text.Encoding]::UTF8.GetBytes($body))
```

E2E вкладки по шагам под-этапа (артефакты — `A51_E2E.md` … `A57_E2E.md` в каталоге
стенда; сценарии — `a51_e2e.ps1` и далее):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a51_e2e.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a52_e2e.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a53_e2e.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a54_e2e.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a55_e2e.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a56_e2e.ps1
```

A5.7 — живой сценарий §7 плана (отчёт `A57_E2E.md`, приёмка `AGENT_A5_REPORT.md`):

```powershell
# индекс зоны (нужен доступный эмбеддер: на стенде 127.0.0.1:8011, dim 1024)
& $hds index --full
# токен проекта — ДО ключей (hds agent token перезаписывает secrets.json блоком projects)
& $hds agent token P0001
# свой MCP-сервер стенда (mcp_http.port: 8788 — 8787 занят службой машины)
& $hds mcp-http start
Invoke-RestMethod 'http://127.0.0.1:8788/health'
# сценарий: чистка agent-state, снимки, mock, ход, sync, pending, «Принять», сканы
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a57_e2e.ps1
# вернуть стенд перед повтором (реестр из backup, письмо удалить, один проход)
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a57_restore.ps1
# проверки фазы (логи a57-*.log)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast -- --test-threads=4
cargo test -p hds-cli --test agent_acceptance        # I4 — отдельным прогоном (флейк)
npm --prefix sidecar/hds_agent test
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
```

* **Mock-провайдер** без облака: `MOCK_JS` из `crates/hds-agent/tests/gateway_worker.rs`;
  `provider`/`model` — в `projects.json` (`hds agent project add`), ключ и `base_url` —
  в `data\agent\secrets.json` стенда.
* **Артефакты** — только `tools/parity/out/` (гитигнорится); отчёты стендов —
  `A51_E2E.md` … `A57_E2E.md` в каталоге стенда.
* **git-identity не настроена**: коммиты —
  `git -c user.name='Alex Guerchoig' -c user.email='Guerchoig@gmail.com' commit -F <файл UTF-8>`,
  пути — явным списком (кириллица в командной строке PS 5.1 портится).
* Машина разработческая: `hds ui`, `hds watch`, демон зеркала и `llm_host` можно
  останавливать и возвращать.


## 8. Контекст и промт для A5.1

### 8.1. Состояние на 20.11.2026 (сверить в новом чате)

* Ветка `agent-privacy-zone`, HEAD — docs-коммит A4.6 (`dbd30718`; сверить
  `git log -1 --oneline`), ahead 67, дерево чистое, push не делался.
* Фаза A4 закрыта (`AGENT_A4_REPORT.md`): 123 набора / 855 passed / 13 ignored,
  python 328 OK (skip 1), `npm test` 23/23, e2e `A46_E2E.md`.
* Опора A5 — §1 этого файла: `Gateway`, `AgentRegistry`, `filter`/`RefreshState`,
  `deanon`, `results`, `validator`, `mirror`, `registry_db`, `history`, клиент
  `hds-anonymize`, веб-сервер `hds-ui`, mock-провайдер.
* **Чего ещё нет:** зависимости `hds-ui → hds-agent`, сессий в UI, вкладки, ленты,
  предпросмотра, панелей «Результаты»/«Метки»/«Настройки», `ask_user`,
  «Очистить историю». K1 (RAG) не выполнен — параллельная задача, не часть A5.1.


### 8.2. Промт (копировать целиком)

```text
Ты — ведущий разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Это продолжение основного проекта «ИИ-агент в анонимной зоне»
(PLAN_AGENT_PRIVACY_ZONE.md). Ветка agent-privacy-zone, HEAD — docs-коммит A4.6
(сверить git log -1 --oneline), дерево чистое, push не делался.

Фаза A4 (демон двустороннего зеркала) закрыта: отчёт AGENT_A4_REPORT.md, журнал
AGENT_A4_STAGES.md, e2e A46_E2E.md. Фаза A5 — вкладка «Агент» — идёт под-этапами:
план, границы, решения (В-A5-1…8) и грабли — AGENT_A5_STAGES.md (§1, §2, §4.1,
§5–§7), этот промт — её §8.

Задача: под-этап A5.1 — каркас вкладки и API состояния.

1. Прочитай AGENT_A5_STAGES.md целиком, затем PLAN_AGENT_PRIVACY_ZONE.md §5.8
   (TR-8.2), §4.3 (канал hds ↔ агент), §4.4 (раскладка) и §6 (строка A5).
2. crates/hds-ui/Cargo.toml: зависимость hds-agent = { path = "../hds-agent" }.
3. Перенеси ленивые фабрики воркеров (anonymizer_factory, extractor_factory из
   crates/hds-cli/src/cmd/agent_sync.rs) в hds-agent — публичные функции с теми же
   env-подменами (HDS_ANONYMIZE_PYTHON/WORKER, HDS_EXTRACT_PYTHON/WORKER); CLI
   вызывает их оттуда, поведение и тесты не меняются.
4. crates/hds-ui/src/agent.rs: состояние вкладки — реестр сессий AgentRegistry (из
   settings::settings), список проектов; функции: проекты (projects.json), создание
   проекта (проверка пути матрицей hds_agent::paths), состояние проекта (демон
   зеркала жив/нет по sync.lock + watch::lock_is_stale, последние строки
   agent-state\sync.log, count/list_pending реестра), файлы проекта
   (registry_db::list). Состояние — в структуре с конструктором для тестов,
   глобальный OnceLock — тонкая обёртка.
5. Маршруты: GET/POST /api/agent/projects, GET /api/agent/state?project=,
   GET /api/agent/files?project=; POST — CSRF/loopback-гейт как у остальных
   (csrf_ok, X-HDS-UI); в server-профиле — 403 (D20); /api/agent/* не входит в
   is_user_route.
6. Вкладка «Агент» в page.rs: кнопка в ряду вкладок, секция tab-agent (список
   проектов с созданием, состояние выбранного, таблица «Файлы проекта»); JS — по
   образцу существующих вкладок, без фреймворков и внешних CDN.
7. Не делать в A5.1: предпросмотр и отправку реплики (A5.2), ленту событий (A5.3),
   решения и «Результаты» (A5.4), «Метки проекта» (A5.5), настройки и «Очистить
   историю» (A5.6).
8. Тесты: crates/hds-ui/tests/agent_api.rs — форма JSON, 403 без CSRF, 400 без
   проекта, 403 в server-профиле, отсутствие значений fixtures в ответах на копии
   стенда; cargo fmt --all --check, cargo clippy --workspace --all-targets --
   -D warnings, cargo test -p hds-ui, cargo test -p hds-cli, cargo test --workspace,
   npm test (sidecar/hds_agent), python unittest discover -s sidecar/tests -t
   sidecar.
9. Коммиты: feat(agent): A5.1 — ...; docs(agent): A5.1 закрыт — ... (+
   AGENT_A5_STAGES.md §2, §3.1, §4.2, §6, §7; README §20 — если появилось видимое
   поведение). В сообщении — что проверено.

Проверка среза: ветка agent-privacy-zone, HEAD dbd30718 (+docs этого файла), ahead 67,
дерево чистое; cargo test --workspace — 123 набора / 855 passed / 0 failed
(13 ignored); python 328 OK (skip 1); npm test 23/23; стенд agent_a42 — 34 записи
реестра, --once — 0 изменений, отчёт A46_E2E.md.
```


**Следующий под-этап:** `AGENT_A5_3_HANDOFF_PROMPT.md` — самодостаточный контекст и методика
A5.3 (SSE-ветка `GET /api/agent/events` без `Content-Length`, подписка на кольцо через
`Condvar`, `ask_user` с `AskState`/`UiAskUser`, `POST /api/agent/answer`, деанонимизация
вопроса в `GatewayHost`, лента с блоками рассуждений/инструментов и переключателем «как видит
модель»), критерии приёмки, грабли и промт (§10). Создан 20.11.2026 после закрытия A5.2; при
расхождении прав файл-хендофф.

**После A5.3:** A5.4 — «Результаты» и решения (`hds_agent::results`, `Mirror::compare`,
`GET /api/agent/pending|compare`, `POST /api/agent/decision`).

