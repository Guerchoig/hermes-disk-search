# AGENT_A5_7_HANDOFF_PROMPT.md — контекст и промт для под-этапа A5.7 (живой сценарий §7, инварианты, отчёт фазы A5)

> **Кому:** новый чат Cline в режиме Act. **Дата среза:** 20.11.2026.
> **Ветка:** `agent-privacy-zone`, **HEAD:** `82f07587` (`docs(agent): A5.6 закрыт`),
> дерево чистое, push не делался (ahead 85).
> **Что это:** самодостаточный контекст для **последнего** под-этапа фазы A5 основного
> проекта `PLAN_AGENT_PRIVACY_ZONE.md` («ИИ-агент в анонимной зоне»). A0–A4 закрыты,
> A5.1–A5.6 закрыты (`AGENT_A5_STAGES.md` §3.1–§3.6). Этот файл — A5.7: **живой сценарий
> §7 плана** (проект → образы → реплика → агент читает/ищет/пишет → «Принять»), проверка
> инвариантов I1/I2/I4/I6/I8 на стенде, `AGENT_A5_REPORT.md` (отчёт фазы) и README §20.

## 0. Где мы (коротко)

Пользователь работает с документами в своей папке, агент — только с анонимными образами
в `<agent.root>\P<NNNN>\workspace`; исходные значения не покидают машину (I1), оригиналы
не меняются без решения пользователя (I6), в журналах нет значений (I8).

* **A4** — двустороннее зеркало `hds agent sync` — закрыта (`AGENT_A4_REPORT.md`).
* **A5.1** — каркас вкладки, `GET /api/agent/projects|state|files`.
* **A5.2** — реплика: `preview`, `label`, `keep`, `attach`, `message`, `abort`, `stop`.
* **A5.3** — лента SSE, `ask_user` + `POST /api/agent/answer`, «как видит модель»,
  события `redacted`.
* **A5.4** — «Результаты»: `pending`/`compare`/`decision`, «Принять» (backup),
  «Отклонить», три исхода конфликта, «файл занят» (`A54_E2E.md`).
* **A5.5** — «Метки проекта»: `GET /api/agent/labels`, `POST /api/agent/labels/merge|unmerge`,
  `merge_log` + `unmerge_entities`, `labels_merged`/`labels_unmerged` (`A55_E2E.md`).
* **A5.6** — «Настройки агента» (`GET/POST /api/agent/settings`), уведомления фильтра,
  «Очистить историю» (`POST /api/agent/history/clear`), `Secrets::save`, редактор
  `config.yaml` (`A56_E2E.md`).

**Зачем A5.7.** Всё по частям проверено, но **целиком** сценарий плана §7 ещё не проходили:
проект → образы; реплика «Сверь реестр требований с ТЗ…»; агент читает образы, ищет по
проекту через `disk-search`, правит столбец (`office_edit`) и создаёт письмо
(`office_create`); в ленте настоящие значения; письмо в папке проекта, вариант реестра в
`pending`; «Принять» → backup; `egress_log` и `<agent.root>` без форм значений;
`%USERPROFILE%\.cline` не изменён. Это и есть **приёмка MVP** — после A5.7 фаза A5
закрывается отчётом `AGENT_A5_REPORT.md`.

## 1. Состояние репозитория (проверено 20.11.2026)

| Что | Значение |
|---|---|
| Ветка / HEAD | `agent-privacy-zone`, `82f07587`; коммиты A5.6: `bc790bb4` (feat), `82f07587` (docs) |
| Тесты | `cargo test --workspace --no-fail-fast -- --test-threads=4` — **128 наборов / 918 passed / 0 failed / 13 ignored**; python **333 OK (skip 1)**; `npm test` **23/23** |
| fmt / clippy | чисто (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`) |
| Новые наборы A5.6 | `hds-agent` — `secrets::save_*` (слияние, нет файла, нет значений в ошибке); `hds-ui` unit — `settings_request_validates_body`, `history_request_validates_session`, `config_edit::agent_scalars_*`; сокет `crates/hds-ui/tests/agent_settings.rs` (3) |
| Стенд | `tools\parity\out\agent_a42`: 34 записи реестра (**32 synced, 1 accepted, 1 unsupported**), `pending` пуст; отчёты `A51_E2E.md`…`A56_E2E.md`, сценарии `a51_e2e.ps1`…`a56_e2e.ps1` |
| Метки стенда | `root\data\anonymize.db`, `meta.schema_version=2`; у `P0001` **46 меток**, `aliases` пуст, `merge_log` пуст, `keep_terms` **46** |
| Индекс стенда | **не построен**: `state\index.db` отсутствует (`db_path` в `config.yaml` указывает туда) |
| `secrets.json` стенда | **нет** (сценарий A5.6 вернул исходное состояние); `projects.json` — `openrouter` / `~deepseek/deepseek-flash-latest` |
| Зона стенда | `zone\P0001\agent-state` содержит сессию `1795180092124_k6lyi` и **2 файла `*.json.bak`** (в них значения — см. §6 п.54) |
| Документы A5.6 | `AGENT_A5_STAGES.md` §3.6 (факты), §4.1 (В-A5.6-1…6), §6 п.45–52, §7; README §20 |

**Известные флейки** (проверять одиночным прогоном, а не «лечить»): `agent_acceptance`
(сравнивает снимок `%USERPROFILE%\.cline` — IDE/Cline во время теста пишет туда) и
`quota_socket::unauthorized_heavy_rejected_before_queue` (под параллельной нагрузкой).

## 2. Обязательное чтение (по порядку)

1. `PLAN_AGENT_PRIVACY_ZONE.md` — §0 (парадигма), §4.2 (потоки), §5.3 (воркер),
   §5.5 (вкладка и шлюз), §5.6 (фильтр, TR-6.1…6.7), §5.7 (зеркало), **§7 (приёмка MVP)**,
   §5.8 (TR-8.2/8.3), §5.9 (инварианты I1–I8).
2. `AGENT_A5_STAGES.md` — §1 (опора A0–A4), §2 (таблица под-этапов, строка A5.7), §3.1–§3.6
   (что уже сделано), **§5 (контракты A5 — не ломать)**, **§6 (грабли 1–52)**, §7 (окружение).
3. `AGENT_A4_REPORT.md` — структура отчёта фазы (разделы 1–8): A5.7 пишет такой же
   `AGENT_A5_REPORT.md`.
4. `A54_E2E.md`, `A55_E2E.md`, `A56_E2E.md` (в каталоге стенда) — как оформляются живые
   прогоны и что уже проверено на стенде.
5. `README.md` §20 — раздел про вкладку (после A5.6), его и дополнять.

## 3. Что уже готово — опора A5.7

| Блок | Где | Что есть (использовать как есть) |
|---|---|---|
| CLI агента | `crates/hds-cli/src/cmd/agent*.rs` | `hds agent project add|list`, `sync --project P [--once|--interval S]`, `run --project P --prompt T`, `result accept|reject|conflict --project P --path REL`, `history clear --project P [--session ID]`, `token P [--ttl-days N]` |
| Шлюз и реестр | `crates/hds-agent/src/{run,gateway,registry}.rs` | `run::plan_with/prepare/build_filter/clear_history`; `Gateway::{open,send,send_anonymized,next_event,abort,stop,shutdown_with}`; `AgentRegistry` (LRU, `reap_idle`, `unload`) |
| Зеркало | `crates/hds-agent/src/{mirror,registry_db}.rs` | `Mirror::sync_once/compare/decide/labels_report/refresh_label_images`; реестр `<state>\data\agent\registry\<P>.db` |
| Решения | `crates/hds-agent/src/results.rs` | `pending`, `compare`, `decide` (`accept|reject|conflict`), backup в `.hds-agent\backup\`, `ResultsError::Busy` |
| Метки | `crates/hds-agent/src/labels.rs` | `report`, `occurrences`; RPC `vault_project_labels`/`unmerge_entities` |
| Настройки | `crates/hds-agent/src/{settings,secrets}.rs`, `crates/hds-ui/src/config_edit.rs` | `settings::settings`, `Secrets::{load,save,has_provider_key,…}`, `config_edit::set_agent_values` |
| История | `crates/hds-agent/src/history.rs`, `run::clear_history` | очистка с бэкапами `*.json.bak`, `latest_session`, `valid_session_id` |
| Вкладка | `crates/hds-ui/src/{agent,agent_session,page}.rs` | маршруты ниже; лента SSE; вопрос агента; «Результаты»; «Метки»; «Настройки»; «Очистить историю» |
| Воркер | `sidecar/hds_agent/src/{worker,tools,hooks}.ts` | инструменты **`read_files`, `search_codebase`, `editor`, `office_read`, `office_edit`, `office_create`, `ask_user`** + MCP из `cline_mcp_settings.json` |
| MCP | `crates/hds-agent/src/mcp.rs` | `disk-search` — `http://127.0.0.1:<mcp_http.port><base>/agent/<P>` с `Bearer` токеном проекта (нужен `projects.<P>.token` в `secrets.json` **и** живой `hds mcp-http`); `tavily` — `node_modules/tavily-mcp` (нужен ключ `mcp.tavily.api_key`); `1c-buddy` — `agent.mcp.onec_buddy.command` или `mcp.onec_buddy.command` в секретах |
| Фильтр | `crates/hds-agent/src/filter.rs`, `run::build_filter` | fail-closed `privacy_filter_unavailable`; `egress_log: true` → тела запросов в `<P>\agent-state\egress\`; события `redacted` (метки и источник) |
| Настройки вкладки | `GET/POST /api/agent/settings`, `POST /api/agent/history/clear` | ключ только «задан/нет»; `reopened`/`unloaded`; очистка с отчётом `{session, files, backups, replaced}` |
| Стенд | `tools\parity\out\agent_a42` | проект-фикстура: 21 docx, 14 md, 2 xlsx, 1 pdf, `project.json` (unsupported — норма); реестр 34 записи; `anonymize.db` 46 меток |

Маршруты вкладки (все под `agent::gate` + CSRF; в server-профиле 403, D20):
`projects` (GET/POST), `state`, `files`, `preview`, `label`, `keep`, `attach`, `message`,
`abort`, `stop`, `answer`, `pending`, `compare`, `decision`, `labels`,
`labels/merge`, `labels/unmerge`, `events` (SSE + JSON-фолбэк), `settings` (GET/POST),
`history/clear`.

## 4. Задачи A5.7 с методикой

### 4.1. Стенд: что подготовить до сценария

Стенд — тот же (`tools\parity\out\agent_a42`), данные вымышленные. Порядок:

1. **Окружение** (обе консоли): `HDS_CONFIG=<stand>\config.yaml`, `HDS_ROOT=<stand>\root`.
   Канонические пути данных — `root\data\agent\projects.json|secrets.json`,
   `root\data\agent\registry\P0001.db`, `root\data\anonymize.db`; зона — `<stand>\zone`
   (значение `agent.root` в `config.yaml`).
2. **Почистить следы A5.6 в зоне**, иначе I1/I8 не проверить: удалить
   `zone\P0001\agent-state\.cline\data\sessions\*` (там `*.json.bak` со значениями) и
   `zone\P0001\agent-state\egress\*`, если есть. Реестр, образы и оригиналы не трогать —
   это стенд A4 (34 записи, из них 1 `accepted`).
3. **Секреты стенда** — собрать `root\data\agent\secrets.json`. **Порядок важен:**
   сначала `hds agent token P0001` (он пишет файл типизированной структурой `AgentSecrets`
   — только блок `projects`, см. §6 п.63), и только **потом** добавить
   `providers.openai-compatible = {api_key, base_url}` для mock-провайдера **или**
   `providers.openrouter = {api_key}` при живом прогоне, `mcp.tavily.api_key` — если есть.
   Ключи удобнее писать через `POST /api/agent/settings` (слияние `Secrets::save`).
   Ключ в `config.yaml` не писать никогда (TR-8.3).
4. **Индекс анонимной зоны**: `hds index --full`. При `agent.enabled: true` корни берутся
   из `agent_roots` — `<agent.root>\P*\workspace`, а `agent-config`, `agent-state`,
   `.hds-agent`, `_conflicts` исключаются автоматически. Проверить `hds status` и поиск по
   образу (`hds search --query '[PERSON_'` или поиск во вкладке).
5. **MCP `disk-search`**: поднять `hds mcp-http` (порт `mcp_http.port`, по умолчанию 8788) и
   убедиться, что `GET /api/mcp-http` в `hds ui` отвечает. Без живого сервера (или без
   `projects.<P>.token`) инструментов `disk-search` у агента не будет — см. §6 п.56.
6. **Демон зеркала**: `hds agent sync --project P0001` (без `--once`), чтобы созданное
   агентом письмо появилось в папке проекта во время сценария; `--once` — контрольные
   проходы.
7. **UI**: `hds ui --port 8799`, вкладка «Агент».
8. **Снимки до сценария**: `sha256` всех оригиналов папки проекта (I6), хеши файлов
   `%USERPROFILE%\.cline\` (I4), список файлов `<agent.root>` (I1), `GET /api/agent/files`
   и `GET /api/agent/pending` (пусто).

### 4.2. Живой сценарий §7 плана — по шагам

Сценарий — отдельный `a57_e2e.ps1` на стенде (как a51…a56), артефакты `a57-*`,
отчёт `A57_E2E.md` в каталоге стенда; итоговая приёмка — `AGENT_A5_REPORT.md` в корне.

0. **Провайдер.** Ключа на стенде нет: по умолчанию — **mock** (`mock_provider.mjs`,
   `MOCK_SCRIPT`/`MOCK_REQUESTS`; `provider=openai-compatible`, `base_url` — в
   `secrets.json`). Это **явно** пишется в отчёте («живой провайдер не проверялся, ключа
   нет»; при наличии ключа владельца — прогнать тем же сценарием на `openrouter`).
1. **Проект → образы.** `hds agent sync --once --project P0001` → 34 записи реестра,
   структура папок повторена, имена — с метками (`01 Устав\Устав … 1С ERP.docx` и т.п.).
   В UI — «Файлы проекта»: статусы `synced` (1 запись `accepted` из A5.4 — норма).
2. **Реплика и предпросмотр.** `POST /api/agent/preview` с текстом плана: «Сверь реестр
   требований с ТЗ, найди требования без покрытия в типовой 1С:ERP, подготовь письмо
   Иванову с вопросами» → в ответе **метки**, в т.ч. `[PERSON_1|дат]`, «1С:ERP» **не
   скрыт** (D19, keep), путь папки — `[SYS_n]`.
3. **Ход агента** (`POST /api/agent/message`, затем `GET /api/agent/events` / SSE):
   * ветка mock: `MOCK_SCRIPT` из 4–5 шагов — `office_read` (реестр `xlsx` и ТЗ `pdf`;
     пути брать из `GET /api/agent/files` — имена образов с метками, **не угадывать**),
     `office_edit` (xlsx: `add_column` «Покрытие 1С:ERP» + значения), `office_create`
     (письмо `06 Переписка\Письмо 03 Вопросы по покрытию.docx`, `kind=docx`),
     при поднятом `mcp-http` — вызов инструмента `disk-search` (имя — из `tools` первого
     запроса в `MOCK_REQUESTS`), финальный `text`;
   * ветка живого провайдера: тот же ход, но модель сама выбирает инструменты; в отчёте —
     скриншот/дамп ленты (пути и метки), без значений в тексте отчёта.
4. **Лента.** В ленте — рассуждения, `tool_use`/`tool_result` с **настоящими** значениями
   (деанонимизация шлюза), пути — папки проекта; переключатель «как видит модель» —
   метки. События `redacted` — если фильтр что-то починил (в отчёте: было/нет).
5. **Результат.** `hds agent sync --once` (или демон): письмо появилось в папке проекта
   (имя деанонимизировано, «Иванову» в дательном или пометка «проверить»), вариант
   реестра — в `pending` с метаданными и логотипом оригинала (`GET /api/agent/pending`,
   `GET /api/agent/compare?rel=…`).
6. **Решение.** «Принять» — `POST /api/agent/decision {decision:'accept'}` (или
   `hds agent result accept --project P0001 --path …` — то же ядро): оригинал заменён
   вариантом, копия — в `.hds-agent\backup\<дата-время>\`, образ пересоздан,
   `pending` пуст. **Проверить I6:** sha256 остальных оригиналов не изменились.
7. **Уборка истории (по желанию, A5.6).** Если в истории остались значения —
   `POST /api/agent/history/clear`; в отчёте — отчёт очистки (файлы/бэкапы/замены).

### 4.3. Инварианты на стенде (I1, I2, I4, I6, I8)

Формы значений fixtures брать **из базы** (`Vault.project_labels` / probe как в a56),
своих списков не писать (AG-1).

* **I1 (значения только в зоне пользователя/базе/памяти).** Скан `<agent.root>` на формы:
  исключить `agent-state` (история SDK — зона агента, решение В-A5.7-2), `*.json.bak`
  (копия истории, В-A5.6-4) и **имена файлов** (значение в имени образа — зона
  workspace/пользователя: A55_E2E §3 п.2, A5.5 §4.2 п.44). Остальное — 0 совпадений.
* **I2 (в облако не уходит известное).** `MOCK_REQUESTS` (или лог провайдера): ни одной
  формы fixtures; метки `[PERSON_n]`/`[ORG_n]` есть. При `egress_log: true` — то же по
  `<P>\agent-state\egress\*` (это проверяемая проекция «что ушло»).
* **I4 (программирующий Cline не затронут).** Хеши файлов `%USERPROFILE%\.cline\` до и
  после сценария равны; дополнительно — `cargo test -p hds-cli --test agent_acceptance`
  **одиночным** прогоном (флейк под параллелью).
* **I6 (оригиналы без «Принять» не меняются).** `sha256` всех оригиналов до сценария и
  после; после «Принять» изменился только принятый файл (и его образ), остальные — те же.
* **I8 (значений нет в журналах и аудите).** `sync.log`, `.hds-agent\messages.jsonl`,
  ответы API, `data\logs` (если пишутся) — без форм; в ленте значения **есть** (зона
  пользователя, A5.3) — это не нарушение, но в тексте `AGENT_A5_REPORT.md` значения
  fixtures не цитировать.
* **I3 (файловые инструменты только в workspace) и I7 (секрет в тексте блокирует
  запрос)** — уже покрыты A3; в A5.7 достаточно одной проверки «путь вне workspace —
  ошибка» и «ключ в реплике — отказ» (или ссылка на существующие тесты).

### 4.4. `egress_log` — проверяемая проекция (§7 п.3 плана)

Включить через панель настроек (A5.6): `POST /api/agent/settings {egress_log: true}`
(`reopened: true`), пройти сценарий, затем просканировать `<P>\agent-state\egress\*` на
формы fixtures (0 совпадений) и выключить обратно. В отчёте — число файлов и результат
скана. Журнал включается только на прогон: тела запросов — зона агента, но в git и в
отчёт их не выкладывать.

### 4.5. Отчёт `AGENT_A5_REPORT.md` (корень репозитория)

Структура — как `AGENT_A4_REPORT.md`:
1. **Итог фазы A5** — что теперь умеет вкладка, что проверено живьём, чем (mock/реальный
   провайдер), что осталось в фазе 2.
2. **Что сделано по под-этапам** A5.1–A5.7 (по 5–8 строк: результат снаружи, ключевые
   файлы, тесты).
3. **Контракты фазы** — маршруты `/api/agent/*`, секреты, решения, статусы, воркер,
   деанонимизация, метки, проекты, фильтр, история (сводка §5 `AGENT_A5_STAGES.md`).
4. **Инварианты на стенде** — таблица I1…I8: чем проверено, результат (как в A4 §4).
5. **Живой сценарий** — шаги §7 плана, артефакты (`a57-*`, `A57_E2E.md`), числа.
6. **Проверки** — команды и цифры (cargo/python/npm, что игнорируется и почему).
7. **Что осталось** — долги фазы 2/A6 (Word-compare, «Открыть» файл, автопроверка
   истории, «Вернуть» из бэкапа, удаление проекта, многопользовательская вкладка, K1…).
8. **Долги §4.2** — актуальный список с пометками «закрыто в A5.x».

### 4.6. Документы

* `AGENT_A5_STAGES.md`: §2 — A5.7 закрыт; §3.7 — факты (что прогнали, что нашли);
  §3 (журнал) — строка A5.7; §4.1 — ответы В-A5.7-1…6; §4.2 — что закрыто/что в A6;
  §6 — новые грабли; §7 — команды A5.7.
* `README.md` §20 — подраздел **«Приёмка фазы A5»**: что проверено на вымышленном
  проекте, чем (mock/реальный провайдер), какие инварианты и как, где отчёт; ссылка на
  `AGENT_A5_REPORT.md`. Существующий текст «Вкладка «Агент»» не переписывать.
* `STATUS.md` — по желанию: строка про готовность агента (если найдёте подходящий раздел;
  боевые `PLAN_*.md` не трогать).

### 4.7. Проверки

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast -- --test-threads=4 *> tools\parity\out\a57-cargo-test.log
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
cargo test -p hds-cli --test agent_acceptance      # I4 — отдельно (флейк под параллелью)
```

Плюс сам сценарий `a57_e2e.ps1` (49+ проверок по образцу a56) и скан инвариантов.

## 5. Границы под-этапа

**Делать:** живой сценарий §7 на стенде, проверки инвариантов, `AGENT_A5_REPORT.md`,
README §20 (приёмка), §3.7 и закрытие A5.7 в журнале.

**Не делать:** Word-compare и ячеечный diff (фаза 2), «Открыть» файл/вариант процессами
(В-A5-7), «Вернуть» из бэкапа и журнал решений (A6), автопроверку истории перед
завершением сессии (A6), удаление проекта в UI (A6), многопользовательскую вкладку
(D20), правки RPC-контрактов A5.5, правки демона/зеркала и sidecar/hds_agent, переписывание
`PLAN_AGENT_PRIVACY_ZONE.md` и чужих незакоммиченных изменений. Ключ провайдера — только в
`data\agent\secrets.json`; в `config.yaml` его нет никогда.

## 6. Ловушки (свежие, оплачены)

53. **`state\data\agent\...` — след старого прогона.** На стенде есть
    `tools\parity\out\agent_a42\state\data\agent\registry\P0001.db` (**1 запись**). При
    `HDS_ROOT=root` канонические данные — `root\data\agent\...` (34 записи). Не путать и не
    «чинить» второй реестр; при сомнении — сравнивать по `projects.json` и `GET /api/agent/files`.
54. **`agent-state` — в зоне, а не в `HDS_ROOT`.** `<agent.root>\P0001\agent-state`
    (на стенде `zone\P0001\agent-state`). После A5.6 там лежит сессия с `*.json.bak`, а в
    бэкапах — **открытые значения** (В-A5.6-4). Для I1/I8 либо чистить сессии перед
    прогоном, либо явно исключать `agent-state`/`*.bak` и писать это в отчёте.
55. **Индекс зоны может быть не построен.** `state\index.db` на стенде отсутствует →
    `disk-search` вернёт пусто, и шаг «агент ищет по проекту» провалится не по вине агента.
    Перед сценарием: `hds index --full` (при `agent.enabled: true` корни — `<agent.root>\P*\workspace`,
    `agent-config`/`agent-state`/`.hds-agent`/`_conflicts` исключены).
56. **`disk-search` появляется в инструментах не всегда.** Нужны одновременно:
    `projects.<P>.token` в `secrets.json`, `agent.mcp.disk_search.enabled: true` и живой
    `hds mcp-http` на порту `mcp_http.port` с base `/mcp`. В A5.4 первый запрос к модели
    содержал только 7 инструментов воркера — MCP там не было. Имена MCP-инструментов
    **брать из `tools` первого запроса** (`MOCK_REQUESTS`), а не угадывать.
57. **Mock-провайдер: `MOCK_SCRIPT` — JSON-массив шагов.** Шаг: `{text}` |
    `{toolCalls:[{name, arguments}]}` | `{reasoning}`; запросы сверх сценария повторяют
    последний шаг. Тела пишутся в `MOCK_REQUESTS` (JSONL) — это же и есть проверка I2.
58. **`office_create` не перезаписывает существующий файл**, а путь — внутри `workspace`
    (имя образа). В папке проекта письмо появится только после прохода зеркала; имя
    деанонимизируется (`[PERSON_1]` → «Иванов» и т.п.).
59. **«Принять» из сценария удобнее CLI.** `hds agent result accept --project P0001 --path
    <отн. путь>` — то же ядро, что кнопка (A5.4); путь — от оригинала, разделители
    нормализует ядро (`normalize_rel`), но в PowerShell сравнивать строки с `\`.
60. **PowerShell 5.1.** `.ps1` с кириллицей — UTF-8 **с BOM** (иначе cp1251 и мусор);
    тела POST — UTF-8 байтами (`[Text.Encoding]::UTF8.GetBytes`); `Invoke-RestMethod`
    декодирует JSON как Latin-1 — читать `RawContentStream`; 403 проверять запросом **без**
    `Content-Type` и `X-HDS-UI` (сырой `TcpClient`), иначе `Invoke-WebRequest` добавит
    Content-Type и CSRF пропустит.
61. **Снимок `%USERPROFILE%\.cline` портит IDE.** Пока идёт сценарий/тест, VS Code и Cline
    пишут в этот каталог: снимать хеши до/после в тихий момент, а `agent_acceptance` гонять
    одиночным прогоном.
62. **Значение в имени образа — не утечка.** `… 1С ERP ….pdf.md`, `[ORG_1]` в имени файла —
    зона workspace/пользователя (A55_E2E §3 п.2). Сканы I1/I8 по `<agent.root>` должны
    исключать имена файлов (или проверять только текст), иначе будет ложное «FAIL».
63. **`hds agent token` перезаписывает `secrets.json` только блоком `projects`.**
    `hds_mcp::agent::AgentSecrets` — типизированная структура (`projects` + `token_sha256`),
    неизвестные ключи при чтении отбрасываются, а `save_at` пишет лишь `projects`. Если
    сначала записать `providers`/`mcp`, а потом выдать токен — ключи провайдера и MCP
    **пропадут**. Порядок: токен → ключи (руками или `POST /api/agent/settings`, он
    сохраняет слиянием). Постоянное лечение (сохранять неизвестные ключи в
    `hds_mcp::agent`) — A6.
64. **`hds ui` и `hds mcp-http` — разные процессы.** Вкладка не поднимает MCP-сервер:
    без отдельного `hds mcp-http` URL `http://127.0.0.1:<port>/mcp/agent/P` не отвечает, и
    `disk-search` у агента не появится (ход при этом не падает — инструмента просто нет).

## 7. Окружение и команды

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager log -2 --oneline; git --no-pager status
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast -- --test-threads=4 *> tools\parity\out\a57-cargo-test.log
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

Стенд `tools\parity\out\agent_a42` (34 записи реестра, 46 меток):

```powershell
$env:HDS_CONFIG='<repo>\tools\parity\out\agent_a42\config.yaml'
$env:HDS_ROOT='<repo>\tools\parity\out\agent_a42\root'
$hds='<repo>\target\debug\hds.exe'

# индекс анонимной зоны (agent.enabled: true → <agent.root>\P*\workspace)
& $hds index --full
# токен проекта для disk-search (пишется в data\agent\secrets.json).
# ВАЖНО: вызывать ДО записи ключей — token перезаписывает файл только блоком projects (§6 п.63).
& $hds agent token P0001
# MCP-сервер (порт mcp_http.port, по умолчанию 8788) — без него disk-search у агента нет
Start-Process $hds -ArgumentList 'mcp-http'
Invoke-RestMethod 'http://127.0.0.1:8799/api/mcp-http'    # из hds ui: жив ли
# демон зеркала (для появления письма в папке проекта «на ходу»)
& $hds agent sync --project P0001
# контрольный проход
& $hds agent sync --once --project P0001

# UI вкладки
& $hds ui --port 8799
$h=@{'X-HDS-UI'='1';'Content-Type'='application/json'}
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/projects'
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/pending?project=P0001'
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/files?project=P0001'
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/settings?project=P0001'
# решение: тело — UTF-8 байтами
$body=@{project='P0001';rel='05 План-график\Реестр требований.xlsx';decision='accept'}|ConvertTo-Json
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/decision' -Method Post -Headers $h `
    -Body ([Text.Encoding]::UTF8.GetBytes($body))
# CLI-эквивалент «Принять»
& $hds agent result accept --project P0001 --path '05 План-график\Реестр требований.xlsx'
# отчёт очистки истории (A5.6)
$body=@{project='P0001'}|ConvertTo-Json
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/history/clear' -Method Post -Headers $h `
    -Body ([Text.Encoding]::UTF8.GetBytes($body))

Remove-Item Env:HDS_ROOT, Env:HDS_CONFIG -ErrorAction SilentlyContinue   # перед cargo test
```

Сценарий A5.7 (свой `a57_e2e.ps1`; образец — `a56_e2e.ps1`, он же показывает mock-провайдер,
мок-воркер анонимизации не нужен — на стенде настоящий):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a57_e2e.ps1
```

* **Mock-провайдер** — `crates\hds-cli\tests\fixtures\mock_provider.mjs`
  (`MOCK_SCRIPT`/`MOCK_REQUESTS`); `provider`/`model` — в `projects.json`, ключ и
  `base_url` — в `data\agent\secrets.json`.
* **`hds agent check` нет** (TR-8.5 — долг A6): проверка готовности — `hds check`
  (секции агента в нём пока нет), `GET /api/agent/state`, `GET /api/mcp-http`.
* **Артефакты** — только `tools/parity/out/` (гитигнорится); отчёты — `A57_E2E.md`,
  итоговый — `AGENT_A5_REPORT.md` в корне.
* **git-identity не настроена**: коммиты —
  `git -c user.name='Alex Guerchoig' -c user.email='Guerchoig@gmail.com' commit -F <файл UTF-8>`,
  пути — явным списком (кириллица в командной строке PS 5.1 портится).
* Машина разработческая: `hds ui`, `hds mcp-http`, `hds agent sync`, `llm_host` можно
  останавливать и возвращать.

## 8. Вопросы владельцу (предложены дефолты)

| # | Вопрос | Предложенный дефолт |
|---|---|---|
| В-A5.7-1 | Живой провайдер или mock? | Ключа на стенде нет → **mock**; это явно в отчёте. Если владелец даст ключ `openrouter` — тот же сценарий на живом провайдере, отчёт дополняется |
| В-A5.7-2 | Значения в `<P>\agent-state` (и `*.bak`) против формулировки I1 | `agent-state` — **зона агента** (история SDK, решение A5.6); I1 проверяется по `<agent.root>` **без** `agent-state`/`*.bak`, в отчёте это фиксируется. Перенос бэкапов в `data\agent\` и правка формулировки PLAN — A6, `PLAN_*.md` в A5.7 не трогаем |
| В-A5.7-3 | K1 (RAG «с/без», `tools/parity/AGENT_RAG_K1.md`) не выполнен | Сделать минимальный замер в A5.7 (1 вопрос, 2 прогона: `disk_search` вкл/выкл) **или** зафиксировать долг в отчёте с явной пометкой «без замера» (D23) |
| В-A5.7-4 | Внешние MCP: `tavily` (нет ключа), `1c-buddy` (нет exe) | В сценарии не вызывать; в отчёте — «не проверено вживую, нет ключа/программы»; проверить, что при отсутствии ключа серверы просто не появляются в `tools`, а ход не падает |
| В-A5.7-5 | «Принять» на стенде | Да: стенд вымышленный, оригинал реестра меняется вариантом, копия — в `.hds-agent\backup\`; I6 проверяется по остальным оригиналам |
| В-A5.7-6 | Куда писать приёмку в README | Дополнить §20 подразделом «Приёмка фазы A5» + ссылка на `AGENT_A5_REPORT.md`; `STATUS.md` — только если найдётся подходящий раздел |

## 9. Срез на 20.11.2026 (для проверки в новом чате)

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager log -1 --oneline          # 82f07587 docs(agent): A5.6 закрыт
git --no-pager status --porcelain -uall  # пусто
(Select-String -Path tools\parity\out\a56-cargo-test.log -Pattern 'test result:').Count  # 128
Select-String -Path tools\parity\out\a56-cargo-test.log -Pattern 'test result:.*FAILED'  # пусто
Test-Path tools\parity\out\agent_a42\root\data\anonymize.db      # True
Test-Path tools\parity\out\agent_a42\root\data\agent\secrets.json # False
Test-Path tools\parity\out\agent_a42\state\index.db               # False (индекс не построен)
Get-ChildItem tools\parity\out\agent_a42\zone\P0001\agent-state -Recurse -Filter '*.bak'  # 2 (след A5.6)
```

| Что | Ожидание |
|---|---|
| HEAD | `82f07587`, ветка `agent-privacy-zone`, ahead 85, дерево чистое |
| Тесты | 128 наборов / 918 passed / 0 failed / 13 ignored; python 333 OK (skip 1); npm 23/23 |
| Реестр `root\data\agent\registry\P0001.db` | 34 записи: 32 `synced`, 1 `accepted` (`06 Переписка\Письмо 02 …md`), 1 `unsupported` (`project.json`) |
| `anonymize.db` | `schema_version=2`, `labels P0001=46`, `aliases=0`, `merge_log=0`, `keep_terms=46` |
| `projects.json` | `P0001` — `openrouter` / `~deepseek/deepseek-flash-latest`, путь — `<stand>\project` |
| Зона | `zone\P0001\agent-state` — сессия `1795180092124_k6lyi` + 2 `*.json.bak` (значения внутри) |
| Стенд-файлы | `A51_E2E.md`…`A56_E2E.md`, `a51_e2e.ps1`…`a56_e2e.ps1`, `a56_probe.py`, `a56-checks.txt` (49/49) |
| `tools\parity\AGENT_RAG_K1.md` | **нет** (K1 не выполнен — В-A5.7-3) |

## 10. Промт (копировать целиком)

```text
Ты — ведущий разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Это продолжение основного проекта «ИИ-агент в анонимной зоне»
(PLAN_AGENT_PRIVACY_ZONE.md). Ветка agent-privacy-zone, HEAD — docs-коммит A5.6
(сверить git log -1 --oneline), дерево чистое, push не делался.

Фазы A0–A4 закрыты (A4 — двустороннее зеркало, отчёт AGENT_A4_REPORT.md).
Фаза A5 (вкладка «Агент») идёт под-этапами: план, решения (В-A5-1…8,
В-A5.2-1…6, В-A5.3-1…5, В-A5.4-1…6, В-A5.5-1…6, В-A5.6-1…6), долги, контракты и
грабли — AGENT_A5_STAGES.md (§1, §2, §3.1–§3.6, §4–§7).
A5.1–A5.6 закрыты: каркас и API; реплика; лента SSE и ask_user; «Результаты»;
«Метки проекта»; «Настройки агента», уведомления фильтра и «Очистить историю»
(A56_E2E.md, 49/49).
Контекст и методика этого под-этапа — AGENT_A5_7_HANDOFF_PROMPT.md (§3–§9),
этот промт — её §10.

Задача: под-этап A5.7 — живой сценарий §7 плана (проект → образы → реплика →
агент читает образы, ищет по проекту, правит столбец и создаёт письмо → «Принять»),
проверка инвариантов I1/I2/I4/I6/I8 на стенде, отчёт AGENT_A5_REPORT.md и приёмка
фазы A5 в README §20.

1. Прочитай AGENT_A5_7_HANDOFF_PROMPT.md целиком, затем AGENT_A5_STAGES.md (§1, §2,
   §3.6, §5–§7) и PLAN_AGENT_PRIVACY_ZONE.md §5.3, §5.5–§5.7, §5.9 и §7.
2. Подготовь стенд по §4.1: почисти `zone\P0001\agent-state` (сессии и .bak —
   след A5.6), собери `root\data\agent\secrets.json` (токен проекта, ключ mock-
   провайдера), построй индекс зоны (`hds index --full`), подними `hds mcp-http`,
   демон `hds agent sync --project P0001`, UI на 8799; сними снимки до сценария:
   sha256 оригиналов (I6), хеши `%USERPROFILE%\.cline` (I4), файлы `<agent.root>` (I1).
3. Прогони сценарий §7 плана (сценарий a57_e2e.ps1 на стенде, отчёт A57_E2E.md):
   sync → образы; preview реплики (метки, `[PERSON_1|дат]`, «1С:ERP» не скрыт);
   ход через mock-провайдер: `office_read` (xlsx реестра и pdf ТЗ), `office_edit`
   (столбец покрытия), `office_create` (письмо в `06 Переписка`), при поднятом
   mcp-http — вызов disk-search (имя инструмента — из `tools` первого запроса);
   лента с настоящими значениями; sync → письмо в папке проекта, вариант реестра в
   `pending`; «Принять» (decision/CLI) → backup и пересозданный образ, `pending` пуст.
4. Проверь инварианты по §4.3 и egress_log по §4.4: I1 — `<agent.root>` без форм
   (кроме `agent-state`/`*.bak` и имён файлов, см. В-A5.7-2); I2 — в запросах к
   модели и в `egress` нет форм, метки есть; I4 — хеши `.cline` равны и
   `cargo test -p hds-cli --test agent_acceptance` одиночным прогоном; I6 — sha256
   остальных оригиналов не изменились; I8 — журналы и ответы API без значений.
5. Напиши `AGENT_A5_REPORT.md` по структуре §4.5 (итог фазы, под-этапы A5.1–A5.7,
   контракты, инварианты, живой сценарий, проверки, что осталось, долги) и
   добавь в README §20 подраздел «Приёмка фазы A5» (что проверено, чем — mock или
   реальный провайдер, где отчёт). Существующий текст вкладки не переписывать.
6. Обнови AGENT_A5_STAGES.md (§2 — A5.7 закрыт, §3.7 — факты, журнал §3,
   §4.1 — ответы В-A5.7-1…6, §4.2 — что закрыто и что в A6, §6 — новые грабли,
   §7 — команды A5.7).
7. Не делать: Word-compare и ячеечный diff, «Открыть» файл процессами, «Вернуть»
   из бэкапа и журнал решений (A6), автопроверку истории перед завершением сессии
   (A6), удаление проекта, многопользовательскую вкладку, правки демона/зеркала,
   sidecar/hds_agent и RPC-контрактов A5.5, переписывание PLAN_*.md.
8. Проверки: cargo fmt --all --check, clippy --workspace --all-targets -- -D warnings,
   cargo test --workspace --no-fail-fast -- --test-threads=4, npm test,
   python unittest, плюс сценарий a57_e2e.ps1 и скан инвариантов. Числа — в отчёт.
9. Коммиты: feat(agent): A5.7 — ...; docs(agent): A5.7 закрыт — ... (+ отчёт фазы
   AGENT_A5_REPORT.md, журнал, README). В сообщении — что проверено и какими
   командами; push не делать.
```







