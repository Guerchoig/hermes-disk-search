# AGENT_A5_6_HANDOFF_PROMPT.md — контекст и промт для под-этапа A5.6 («Настройки агента», «Очистить историю»)

> **Кому:** новый чат Cline в режиме Act. **Дата среза:** 20.11.2026.
> **Ветка:** `agent-privacy-zone`, **HEAD:** `8d214fc6` (`docs(agent): A5.5 закрыт`)
> + docs этого файла, дерево чистое, push не делался.
> **Что это:** самодостаточный контекст для продолжения **фазы A5** основного проекта
> `PLAN_AGENT_PRIVACY_ZONE.md` («ИИ-агент в анонимной зоне»). A0–A4 закрыты, A5 идёт
> под-этапами (`AGENT_A5_STAGES.md`), **A5.1–A5.5 закрыты**. Этот файл — A5.6:
> **панель «Настройки агента»** (провайдер, модель, ключи, MCP, суб-агенты, фильтр),
> **уведомления фильтра** и кнопка **«Очистить историю»** (TR-6.4/6.6, TR-8.2/8.3).
> Живой сценарий §7 — A5.7.

## 0. Где мы (коротко)

Пользователь работает с документами в своей папке, агент — только с анонимными образами
в `<agent.root>\P<NNNN>\workspace`; исходные значения не покидают машину (I1), оригиналы
не меняются без решения пользователя (I6), в журналах нет значений (I8).

* **A4** — двустороннее зеркало `hds agent sync` — закрыта (`AGENT_A4_REPORT.md`).
* **A5.1** — каркас вкладки и API состояния (`/api/agent/projects|state|files`).
* **A5.2** — реплика: `preview`, `label`, `keep`, `attach`, `message`, `abort`, `stop`.
* **A5.3** — лента SSE, `ask_user` и `POST /api/agent/answer`, «как видит модель»,
  события `redacted` (срабатывания фильтра **уже приходят в ленту**).
* **A5.4** — панель «Результаты»: `pending/compare/decision`, решения A4 из вкладки,
  `notice` в ленту (`A54_E2E.md`).
* **A5.5** — панель «Метки проекта»: `GET /api/agent/labels`,
  `POST /api/agent/labels/merge|unmerge`, `merge_log` + `unmerge_entities`
  (`vault_project_labels`), пересборка образов и `labels_merged`/`labels_unmerged`
  в очередь агенту (`A55_E2E.md`).

**Зачем A5.6.** Во вкладке нельзя ни увидеть, ни изменить то, от чего зависит ход:
провайдер/модель проекта лежат в `projects.json`, ключи — в `data\agent\secrets.json`,
поведение фильтра и MCP — в секции `agent:` конфига, а «грязный хвост» (значение,
которое NER пропустил и которое фильтр починил только в исходящем запросе) остаётся
в файлах истории `<P>\agent-state`. Нужны: панель настроек (что меняется — куда
пишется), уведомления фильтра с понятным действием и кнопка «Очистить историю»
(TR-6.6: стоп → значения заменены метками → сессия возобновлена, отчёт «файлов/замен»).
Ключи при этом **никогда** не попадают в `config.yaml` и в ответы API (маска/признак).

## 1. Состояние репозитория (проверено 20.11.2026)

| Что | Значение |
|---|---|
| Ветка / HEAD | `agent-privacy-zone`, `8d214fc6`; коммиты A5.5: `6fd4e40b` (feat), `8d214fc6` (docs) |
| Тесты | `cargo test --workspace --no-fail-fast -- --test-threads=4` — **127 наборов / 907 passed / 13 ignored**; python 333 OK (skip 1); `npm test` 23/23 |
| fmt / clippy | чисто (`cargo fmt --all --check`, `clippy --workspace --all-targets -- -D warnings`) |
| Новые наборы A5.5 | `hds-agent` unit (`labels::*`, `results::refresh_*`, `append_message`, `MessageKind`), `hds-ui` unit (`merge_request_*`, `merge_id_of`), `hds-ui/tests/agent_labels.rs` (2, реальный сокет) |
| Стенд | `tools\parity\out\agent_a42`: 34 записи реестра, `pending` пуст; отчёты `A51_E2E.md`…`A55_E2E.md`, сценарии `a51_e2e.ps1`…`a55_e2e.ps1` |
| Метки стенда | `root\data\anonymize.db`, `meta.schema_version=2`, таблицы + `merge_log`; у `P0001` **46 меток**, `aliases` пуст, `merge_log` пуст, `keep_terms` 46 (в A5.5 добавлен вымышленный `СКЛАД-ТЕСТ-55`) |
| Документы A5.5 | `AGENT_A5_STAGES.md` §3.5 (факты), §4.1 (В-A5.5-1…6), §6 п.39–44, §7; README §20 |

**Известные флейки** (проверять одиночным прогоном, а не «лечить»):
`hds-cli/tests/agent_acceptance::acceptance_a3_end_to_end` (IDE пишет в
`%USERPROFILE%\.cline` во время параллельного прогона) и
`hds-mcp/tests/quota_socket::unauthorized_heavy_rejected_before_queue`
(`max_conns`/503 под нагрузкой). Оба одиночными прогонами зелёные.

## 2. Обязательное чтение (по порядку)

1. `AGENT_A5_STAGES.md` — §2 (строка A5.6), §3.5 (что стало после A5.5), §4.1 (ответы
   В-A5.5), §4.2 п.10–11 (долги «Вернуть» из backup и журнала решений), §5 (контракты
   A5), §6 (грабли 1–44), §7 (окружение и команды).
2. `PLAN_AGENT_PRIVACY_ZONE.md` — §5.6 (TR-6.4 аудит, TR-6.6 «Очистить историю»,
   TR-6.7 `egress_log`), §5.8 (TR-8.2 вкладка, TR-8.3 конфиг), §5.9 (инварианты,
   особенно I8), §3.1–3.2 (решения D19/D20), §7 (приёмка).
3. `README.md` §20 — «Вкладка „Агент“», «Что уходит наружу», «Ограничения» (аудит,
   ключи, таймауты) — там уже описано пользователю то, что A5.6 доводит до кнопок.
4. Код (в этом порядке; он и есть опора — см. §3):
   `crates/hds-agent/src/{run.rs, gateway.rs, history.rs, secrets.rs, settings.rs,
   projects.rs, filter.rs, registry.rs, mcp.rs}`,
   `crates/hds-ui/src/{agent.rs, agent_session.rs, config_edit.rs, page.rs, lib.rs}`,
   `crates/hds-cli/src/cmd/agent_run.rs` (там уже есть `history clear`).

## 3. Что уже готово — опора A5.6

| Что | Где | Чем пользоваться |
|---|---|---|
| **Очистка истории (ядро)** | `crates/hds-agent/src/run.rs::clear_history(cfg, project_id, session: Option<&str>, root) -> Result<(String, HistoryOutcome), String>` | Полный путь CLI: `session=None` → `history::latest_session`, сам собирает фильтр по снимку vault (`open_client_at` + `snapshot`), зовёт `history::clear`. **Это и есть то, что нужно маршруту** — не писать вторую реализацию |
| **Очистка истории (файлы)** | `history.rs::clear(state_dir, session_id, &Filter) -> Result<HistoryOutcome, HistoryError>` | `HistoryOutcome { files: Vec<PathBuf>, backups: Vec<PathBuf>, replaced: usize }`; `HistoryError::{Missing, Invalid, InvalidSession, NoDictionary, Io}`; `valid_session_id`, `session_dir`, `session_files`, `latest_session`, `read_messages` |
| **Очистка через шлюз** | `gateway.rs::Gateway::clear_history() -> Result<HistoryOutcome, GatewayError>` | Требует **открытой** сессии (`session_id`), иначе `GatewayError::History("сессия не открыта")`. Для маршрута проще `run::clear_history` (как CLI), а шлюз выгрузить перед очисткой |
| **Выгрузка шлюза** | `registry.rs::AgentRegistry::{get_mut, get_or_open, unload, reap_idle, from_settings, live}` | `unload(project)` освобождает процесс и файлы состояния — вызывать до очистки, чтобы SDK не держал сессию |
| **Срабатывания фильтра** | `filter.rs::{Filter, FilterOutcome, Hit, Point}`; `GatewayHost::take_redactions() -> Vec<Hit>` | `Hit { label, source }`, `Point::{Model, McpOut, McpIn}`; аудит `hds_core::audit`, событие `agent_egress_redacted` (`filter.rs::audit_hits`) — только метка/источник, без значения (TR-6.4) |
| **Очередь срабатываний** | `gateway.rs::Gateway::next_event` (~строки 711–720) | `take_redactions()` **дренится внутри** `next_event`, поэтому UI уже получает `GatewayEvent::Redacted { hits }` |
| **Лента** | `agent_session.rs` (pump, ~1445) | Событие `redacted` с `{hits: [{label, source}]}` уже кладётся в кольцо; A5.6 добавляет **сводку и действие**, а не второй канал |
| **Секреты** | `secrets.rs::Secrets` | `load(path)`, `provider_for(project)`, `provider(name)`, `project_model`, `project_provider`, `project_token`, `mcp_key`, `mcp_str`, `mcp_list`, `mcp_servers`, `secret_strings()`. **Сохранения нет** — A5.6 его добавляет (атомарно, слияние) |
| **Пути** | `settings.rs` | `secrets_file(cfg)` (`data\agent\secrets.json`), `state_dir_for(cfg, project)`, `workspace_dir(cfg, project)`, `registry_db(cfg, project)`, `projects_file(cfg)` |
| **Настройки `agent:`** | `settings.rs::AgentSettings` | Поля: `enabled, root, preview_outgoing, keep_terms, subagents, unknown (label\|block), proxy, egress_log, max_live_projects, idle_secs, request_timeout_secs, ask_timeout_secs, filter_max_stale_secs, node_path, disk_search_enabled, onec_enabled, tavily_enabled, onec_command, tavily_command`; ключи конфига — `agent.enabled`, `agent.root`, `agent.preview_outgoing`, `agent.subagents`, `agent.filter.unknown`, `agent.network.proxy`, `agent.egress_log`, `agent.max_live_projects`, `agent.idle_unload_min`, `agent.request_timeout_sec`, `agent.ask_timeout_sec`, `agent.filter.max_stale_sec`, `agent.node.path`, `agent.mcp.{disk_search,onec_buddy,tavily}.{enabled,command}`; дефолты — `Default`; чтение — `settings::settings(cfg)` через `dig` |
| **Проекты** | `projects.rs` | `Project { id, name, path, provider, model, created }`, `get/add/remove/load/save/upsert`, `DEFAULT_PROVIDER`/`DEFAULT_MODEL` (В-12) |
| **Правка `config.yaml`** | `crates/hds-ui/src/config_edit.rs` | `get_config()`, `set_roots`, `set_exclude_paths`, `set_transcribe_dirs`, `set_anonymize_out_dir`; внутри — строчный редактор `replace_scalar`/`replace_list` (сохраняет комментарии), атомарная запись `hds_core::config::replace_file`, проверка YAML после правки |
| **Маршруты UI** | `crates/hds-ui/src/{agent,lib}.rs` | `agent::gate()` (403 в server-профиле), общий CSRF-гейт для POST, `is_user_route` (агентские маршруты в него **не** входят); `GET /api/config`, `POST /api/config/*` — образец правки конфига |
| **Состояние вкладки** | `agent_session.rs::AgentApi` | `client(cfg, state_dir)` (один Python-воркер на UI), `notice(project, text)`, `turn/existing_turn`, `stop`, `answer`, `reset_for_tests`, `install_global`/`global` |
| **Стенд** | `tools\parity\out\agent_a42` | 34 записи реестра, 46 меток, `pending` пуст; `a55_e2e.ps1` — образец сценария с бэкапом/`finally` |
| **CLI-образец** | `crates/hds-cli/src/cmd/agent_run.rs::cmd_history` | `hds agent history clear --project <P> [--session ID]` — печатает «файлов/замен», работает без открытой сессии |

**Чего в A5.6 не делать:** живой сценарий §7 (A5.7); «Вернуть» из backup и журнал
решений (A6, §4.2 п.10); автоматическая автопроверка истории перед завершением сессии
(A6); Word-compare (фаза 2); правки демона/зеркала (`mirror.rs`), `sidecar/hds_agent`
(Node) и RPC-контрактов A5.5 (`vault_*`).


## 4. Задачи A5.6 с методикой

### 4.1. `GET /api/agent/settings` — что показать

Ответ — **без секретов**, значениями только то, что ключом не является:

```
{
  "ok": true,
  "project": {id, name, provider, model, key: {"set": bool}},
  "providers": [{name, key: {"set": bool}, base_url}],      // из secrets.json
  "config": {                                                // AgentSettings
    "enabled": bool, "root": "...", "preview_outgoing": bool,
    "subagents": bool, "unknown": "label|block", "egress_log": bool,
    "max_live_projects": 2, "idle_unload_min": 30, "request_timeout_sec": 120,
    "ask_timeout_sec": 600, "filter_max_stale_sec": 300,
    "node_path": "...", "mcp": {disk_search:{enabled}, onec_buddy:{enabled,command},
                                tavily:{enabled,command}}
  },
  "secrets_path": "...\\data\\agent\\secrets.json",
  "config_path": "...\\config.yaml",
  "live": [{id, seconds}]                                   // AgentRegistry::live()
}
```

* маска ключа — **признак** `set` (хвост значения по умолчанию не отдаём, В-A5.6-2);
* `live` — какие проекты держат открытый шлюз (`AgentRegistry::live()`), чтобы панель
  честно писала «изменится новым ходом»;
* `base_url` провайдера — не секрет, показывать можно (он уходит в SDK).

### 4.2. `POST /api/agent/settings` — куда что писать

| Поле | Куда | Как |
|---|---|---|
| `provider`, `model` проекта | `data\agent\projects.json` | `projects::get` + `Project {provider, model}` + `projects::save` (атомарно; `created`/`name` не терять) |
| `api_key`, `base_url` провайдера | `data\agent\secrets.json` | новый `Secrets::save`/`set_provider_*` (см. 4.3): писатель **только** этого файла |
| `enabled`, `root`, `preview_outgoing`, `subagents`, `egress_log`, таймауты, `unknown`, `proxy`, MCP-команды | `config.yaml`, секция `agent:` | редактор в духе `config_edit` (`replace_scalar`/`replace_list`, комментарии целы, `replace_file`, проверка YAML). `agent.root` — только с `confirm()` в странице и предупреждением, что образы/сессии старого корня останутся на месте |
| секреты | **никогда** в `config.yaml` | ключи и токены — только `secrets.json` (TR-8.3) |

* **400** — пустой/неизвестный провайдер, пустая модель, пустой ключ при попытке
  задать, порог вне диапазона (`max_live_projects` 0 или > 8, `request_timeout_sec`
  < 10), `unknown` не `label|block`;
* **404** — проекта нет; **503** — зона выключена/`agent.root` не задан;
* ответ — тот же `GET`-вид (странице не нужен второй запрос), плюс `applied` и
  `warnings` (что вступит в силу только со следующим ходом), `reopened`.


### 4.3. Секреты: запись и маска

* `Secrets::save(path, &Secrets)` (или узкие `set_provider_key`, `set_mcp_server`):
  - читать текущий файл **заново** (не из кэша), менять только переданное поле,
    остальное сохранять как есть (неизвестные ключи и блоки `mcp.servers` не терять);
  - запись атомарная: временный файл рядом + `hds_core::config::replace_file`;
  - после записи — **не логировать** ни значение, ни его часть; в журнал/аудит только
    «ключ провайдера X обновлён» — и это единственная строка, которую стоит писать;
  - ошибки — `SecretsError`: «secrets.json не разбирается/не записывается» + путь;
    в UI → 500 с понятной подсказкой.
* **Смена ключа и фильтр.** `Filter` собирается при открытии шлюза и берёт
  `secrets.secret_strings()`; после сохранения ключа старый шлюз живёт со старым
  словарём. Поэтому маршрут **выгружает** шлюз (`AgentRegistry::unload`), если менялся
  ключ/провайдер/модель/`egress_log`/`unknown`; это видно в ответе (`reopened: true`)
  и в `notice` («настройки применены; сессия откроется заново»).
* Проверка маски в тестах: `GET` не содержит ни значения ключа, ни его префикса, а
  `POST` с ключом возвращает `key.set = true`.

### 4.4. Уведомления фильтра (TR-6.4) и «Очистить историю» (TR-6.6)

Уведомления уже есть как события `redacted` в ленте (A5.3). A5.6 добавляет:

1. **Сводку в панели**: количество срабатываний за ход/за сессию и последние метки
   (`{label, source}`, без значений). Источник — те же события кольца
   (`GET /api/agent/events`), второго канала не заводить.
2. **Кнопку «Очистить историю»** (в блоке уведомлений и в «Состоянии проекта»):
   - `POST /api/agent/history/clear {project, session?}`;
   - последовательность: если ход идёт — `turn.cancel_ask()` + `gateway.stop()`
     (как в `AgentApi::stop`), затем `registry.unload(project)`, затем
     `run::clear_history(cfg, project, session, root)` (та же функция, что у CLI),
     затем `turn.opened = false`, `notice` в ленту и ответ;
   - ответ: `{ok, session, files, backups, replaced}` — **числа и пути**, без значений;
   - **400** — плохой `session` (`history::valid_session_id`), **404** — нет проекта/
     состояния/сессии, **409** — идёт ход и остановить не удалось, **503** — зона
     выключена, **500** — `HistoryError`/I/O;
   - после очистки в `<P>\agent-state` значений быть не должно (I8: поиск форм
     fixtures, как в E2E A4/A5).
3. **Следующий ход** возобновляет сессию по `latest_session`
   (`agent_session::open_session` → `history::read_messages`), отдельная кнопка
   «возобновить» не нужна; в `notice` пишем «сессия возобновится следующим ходом».

### 4.5. Панель «Настройки агента» в `page.rs`

Секция после «Меток проекта» (порядок не догма, существующие id/кнопки не ломать):

```html
<section><h2>Настройки агента</h2>
 <div class="row"><button onclick="loadAgentSettings()">Обновить</button>
  <span id="agsetmsg" class="muted"></span></div>
 <table class="tr"><thead><tr><th>Параметр</th><th>Значение</th><th></th></tr></thead>
  <tbody id="agsettings"></tbody></table>
 <div class="muted" style="margin-top:4px">Ключ провайдера хранится только в
  <code>data\agent\secrets.json</code>; в ответах — «задан/нет».</div>
 <div class="row" style="margin-top:10px">
  <button onclick="agentClearHistory()">Очистить историю</button>
  <span class="muted">стоп → значения заменяются метками → сессия возобновляется</span>
  <span id="aghistmsg" class="muted"></span></div></section>
```

* строки: провайдер, модель, ключ («задан/нет»), `agent.enabled`/`root`, пороги
  (`request_timeout_sec`, `ask_timeout_sec`, `filter.max_stale_sec`),
  `filter.unknown`, `egress_log`, `subagents`, MCP (`disk_search`, `onec_buddy`,
  `tavily`: признак и команда), `live` (какие проекты открыты);
* правка — кнопкой «Изменить» у строки (`prompt()` для скаляров, `select` для
  `unknown`, `confirm()` для `agent.root` и смены провайдера/модели);
* после `POST` — перерисовать панель и «Состояние проекта»; если `reopened` — строкой
  в `agsetmsg` «сессия откроется заново»;
* **«Очистить историю»** — `confirm()` («файлы истории будут перезаписаны, рядом
  останутся бэкапы; отчёт — сколько файлов и замен»), затем POST и текст отчёта в
  `aghistmsg`; при 404/409 — текст сервера, панель не «притворяется», что прошло;
* своих регулярок/разбора значений в JS нет (AG-1).


### 4.6. Тесты

| Уровень | Что | Где |
|---|---|---|
| `hds-agent` unit | `Secrets::save` (слияние с существующим файлом, атомарность, неизвестные ключи и `mcp.servers` не теряются), `secret_strings()` после записи | `secrets.rs` (`#[cfg(test)]`) |
| `hds-agent` unit | `run::clear_history` не сломан — при правках прогнать `cargo test -p hds-agent run::` | — |
| `hds-ui` unit | разбор тела `POST /api/agent/settings` (400: пустая модель, `unknown=foo`, `max_live=0`) и `history/clear` (400: плохой `session`) | `agent.rs` (`#[cfg(test)]`) |
| `hds-ui` сокет | `crates/hds-ui/tests/agent_settings.rs` (образец `agent_results.rs` / `agent_labels.rs`): `GET settings` (ключа нет в ответе), `POST settings` (projects.json/secrets.json изменились, config.yaml с комментариями), `history/clear` (отчёт, бэкапы, `notice`), 400/404/409, 403 без CSRF и в server-профиле | новый файл |
| CLI | `cargo test -p hds-cli` — `hds agent history clear` и `hds agent run` не сломаны | — |

**Проверки перед коммитом:** `cargo fmt --all --check`,
`clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace --no-fail-fast -- --test-threads=4`,
`python -m unittest discover -s sidecar/tests -t sidecar`,
`npm --prefix sidecar/hds_agent test`.

### 4.7. E2E стенда (`a56_e2e.ps1`, `A56_E2E.md`)

По образцу `a55_e2e.ps1` (ASCII-only код, русские тексты — `a56-requests.json`,
бэкап `projects.json`/`secrets.json`/`config.yaml`, `ReadShared`, `finally`):

1. `hds agent sync --project P0001 --once`; `hds ui --port 8799`.
2. `GET /api/agent/settings` → провайдер/модель стенда, ключ «задан/нет», пороги.
3. `POST /api/agent/settings` (модель `mock-model`, ключ вымышленный
   `sk-a56-e2e-secret`) → 200; в `projects.json` новая модель, в `secrets.json` ключ;
   **в ответе и в `a56-*.json` значения ключа нет**; `config.yaml` — комментарии целы.
4. Ход через mock-провайдер (`mock_provider.mjs`, как в A5.4): в ленте `redacted`
   (значение фильтр заменил меткой) — сводка в панели считает срабатывания.
5. `POST /api/agent/history/clear` → 200, `files > 0`, `replaced >= 1`; в
   `<P>\agent-state` форм fixtures нет (I8); рядом — бэкапы; в ленте `notice`.
6. Повторный ход — сессия возобновилась (в ленте `session`), ответ пришёл.
7. `sync --once` — `changes=0`; `GET /api/agent/files` — реестр не изменился.
8. 403 без CSRF и в server-профиле (`agent::gate`), 400 на мусор, 404 на чужой проект.
9. Восстановить `projects.json`/`secrets.json`/`config.yaml` в `finally`; отчёт
   `A56_E2E.md` с числами и найденными граблями; `a56-checks.txt`.

### 4.8. Документы и коммиты

* `AGENT_A5_STAGES.md`: §2 — A5.6 закрыт; новый §3.6 — факты; §4.1 — ответы
  В-A5.6-1…6; §4.2 — п.10/п.11 с пометкой, что закрыто, а что ушло в A6; §6 — новые
  грабли; §7 — маршруты, тесты, `a56_e2e.ps1`.
* `README.md` §20 — «Настройки агента» (что где хранится, маска ключа), «Уведомления
  фильтра», «Очистить историю» (+ маршруты и коды).
* Коммиты (`git-identity` не настроена — `git -c user.name=… -c user.email=… commit -F`):
  1. `feat(agent): A5.6 — настройки агента, уведомления фильтра и очистка истории`;
  2. `docs(agent): A5.6 закрыт — журнал под-этапов, README §20 и отчёт стенда`.
  В сообщениях — что проверено (fmt/clippy/тесты/python/npm/E2E).

## 5. Границы под-этапа

**Делать:** `GET/POST /api/agent/settings`, запись `secrets.json` (атомарно, слияние),
правка секции `agent:` в `config.yaml` без потери комментариев, маска ключей,
`POST /api/agent/history/clear` поверх `run::clear_history`, панель настроек, сводка и
действие по уведомлениям фильтра, тесты, E2E, документы.

**Не делать:**
* живой сценарий §7 и отчёт `AGENT_A5_REPORT.md` — A5.7;
* «Вернуть» из `.hds-agent\backup` и журнал решений — A6 (§4.2 п.10): в A5.6 только
  честная подпись в панели, что кнопки пока нет;
* автоматическая проверка «грязной» истории перед завершением сессии — A6;
* хранение ключей где-либо, кроме `data\agent\secrets.json` (ни `config.yaml`, ни
  `projects.json`, ни env);
* правки демона зеркала, `mirror.rs`, `results.rs` (A5.4/A5.5), `sidecar/hds_agent`;
* многопользовательская вкладка (D20: server-профиль — 403), очередь вопросов,
  Word-compare.


## 6. Ловушки (свежие, оплачены)

1. **`Gateway::clear_history` без открытой сессии — ошибка.** `session_id` живёт только
   в открытом шлюзе. Маршруту нужен `run::clear_history` (он сам берёт
   `latest_session`), а шлюз перед очисткой выгружается — иначе SDK держит файлы и
   после очистки может дописать их снова.
2. **`AgentRegistry::unload` — не «стоп».** `stop` оставляет шлюз и `session_id`,
   `unload` убирает процесс из реестра. Для смены ключа/модели нужен `unload`:
   следующий ход соберёт шлюз с новым фильтром и секретами (иначе новый ключ не
   попадёт в `secret_strings`).
3. **`take_redactions()` дренит очередь.** Читать её в маршруте нельзя — она уже
   вычитывается в `Gateway::next_event` и превращается в событие `redacted`. Сводка
   считается по кольцу (`GET /api/agent/events`).
4. **`turn.stop` сбрасывается только в `start_turn`.** После «Очистить историю» не
   выставляйте `stop` вручную: очистка — не «стоп», следующий ход должен пройти (если
   ход шёл — `stop`, как в `AgentApi::stop`, и это видно в ленте).
5. **Смена провайдера/модели — это `projects.json`, а ключ — `secrets.json`.**
   `Project` клонируется целиком: не потеряйте `created` и `name` при `projects::save`.
6. **`config_edit` — строчный редактор с наивным разбором `#`.** Значение с `#`
   потеряет хвост комментария (как в Python-редакторе UI). После правки обязательна
   проверка YAML (`load_from`) и `replace_file` — иначе половина конфига.
7. **`secrets.json` — единственное место ключей.** Не «удобно» писать ключ в
   `config.yaml`: это нарушение TR-8.3 и утечка в резервные копии.
8. **`dig` читает конфиг с кэшем по mtime.** После правки `config.yaml` перечитывать
   `config::load()` (UI так и делает на каждый запрос), не держать `AgentSettings` в
   статике.
9. **`OnceLock` состояния UI в тестах.** Сокет-тесты ходят через `global(cfg)`: ставить
   `agent_session::install_global(AgentApi::for_tests_at(root))` и `reset_for_tests()`
   в начале каждого теста (грабля §6 п.32).
10. **`HDS_CONFIG`/`HDS_ROOT` из env стенда ломают `cargo test`** — снимать перед
    прогоном (грабля §6 п.8); UI-тесты — со своим временным конфигом.
11. **PowerShell 5.1 портит кириллицу и снимает кавычки.** Пути/значения — из
    `a56-requests.json`; `python -c` с кавычками не работает (грабля §6 п.35) —
    вспомогательные пробы писать файлом (`a56_probe.py`).
12. **I8 после очистки проверяется поиском форм в `<P>\agent-state`.** В нём лежат
    файлы SDK (там и был «грязный хвост»), поэтому правило: после очистки ни одной
    формы fixtures в `agent-state`, `sync.log`, `messages.jsonl`, `a56-*.json`.
13. **Ключ в ответе API — только «задан/нет».** Соблазн показать «sk-…abcd» удобен, но
    это уже значение: по умолчанию хвост не отдаём (В-A5.6-2), а тест проверяет, что
    ответ не содержит ни ключа, ни его префикса.

## 7. Окружение и команды

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager status; git --no-pager log -3 --oneline
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p hds-agent secrets:: history:: run::
cargo test -p hds-ui agent::
cargo test -p hds-ui --test agent_settings          # новый сокет-тест
cargo test --workspace --no-fail-fast -- --test-threads=4 *> tools\parity\out\a56-cargo-test.log
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

Стенд UI — тот же, что у A4/A5 (`tools\parity\out\agent_a42`):

```powershell
$env:HDS_CONFIG='<repo>\tools\parity\out\agent_a42\config.yaml'
$env:HDS_ROOT='<repo>\tools\parity\out\agent_a42\root'
target\debug\hds.exe ui --port 8799
# настройки (в ответе ключа нет — только «задан/нет»)
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/settings?project=P0001'
# правка: тело — UTF-8 байтами, CSRF-заголовок обязателен
$h=@{'X-HDS-UI'='1';'Content-Type'='application/json'}
$body=@{project='P0001';model='mock-model'}|ConvertTo-Json
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/settings' -Method Post -Headers $h `
    -Body ([Text.Encoding]::UTF8.GetBytes($body))
# очистка истории: отчёт {session, files, backups, replaced}
$body=@{project='P0001'}|ConvertTo-Json
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/history/clear' -Method Post -Headers $h `
    -Body ([Text.Encoding]::UTF8.GetBytes($body))
Remove-Item Env:HDS_ROOT, Env:HDS_CONFIG -ErrorAction SilentlyContinue   # перед cargo test
```

CLI-путь (та же очистка, без UI — для сверки):

```powershell
target\debug\hds.exe agent history clear --project P0001
```

E2E: `powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a56_e2e.ps1`
(артефакты — только `tools/parity/out/`, он гитигнорится; отчёт — `A56_E2E.md` в
каталоге стенда).


## 8. Вопросы владельцу (предложены дефолты)

| # | Вопрос | Дефолт |
|---|---|---|
| В-A5.6-1 | Где живут настройки проекта: `projects.json` или `config.yaml`? | Провайдер и модель — в `projects.json` (рядом с проектом, TR-8.1); общие пороги, фильтр и MCP — в `config.yaml` (`agent:`) |
| В-A5.6-2 | Что показывать про ключ | Только «задан/нет» (`key.set`); хвост/маска значения не отдаются, поле ввода пустое («заменить ключ») |
| В-A5.6-3 | Применение настроек | Смена ключа/провайдера/модели/`egress_log`/`unknown` — `AgentRegistry::unload` → следующий ход откроет сессию заново; панель пишет «применится новым ходом», `agent.root` меняется только с `confirm()` |
| В-A5.6-4 | Что показывает «Очистить историю» | `{session, files, backups, replaced}` — числа и пути без значений; бэкапы рядом с файлами остаются (удаление — отдельное действие A6) |
| В-A5.6-5 | Когда чистить историю | Только по кнопке (TR-6.6); автопроверка перед завершением сессии — A6 |
| В-A5.6-6 | Уведомления фильтра | Сводка по событиям `redacted` в ленте (метки и источник, без значений) + кнопка очистки в том же блоке; аудит `agent_egress_redacted` — как и был, по `audit.enabled` |

## 9. Срез на 20.11.2026 (для проверки в новом чате)

* Ветка `agent-privacy-zone`, HEAD `8d214fc6` (+docs этого файла), дерево чистое.
* `cargo test --workspace --no-fail-fast -- --test-threads=4` — 127 наборов /
  907 passed / 13 ignored (+ два известных флейка, одиночными зелёные); python 333 OK
  (skip 1); `npm test` 23/23; fmt/clippy чисто.
* Стенд `agent_a42`: 34 записи реестра, `pending` пуст, отчёты `A51_E2E.md`…`A55_E2E.md`,
  сценарии `a51_e2e.ps1`…`a55_e2e.ps1`; `P0001` — 46 меток, `aliases`/`merge_log` пусты,
  `meta.schema_version=2`, `keep_terms` 46.
* Вкладка умеет: проекты/состояние/файлы, реплику с предпросмотром и метками, вложения,
  отправку хода, «Стоп», ленту SSE с рассуждениями/инструментами/ответом, вопрос агента,
  панель «Результаты» с решениями, панель «Метки проекта» со склейкой/разъединением.
  Чего нет: панели настроек, сводки/действия по уведомлениям фильтра и «Очистить
  историю» (этот под-этап); живой сценарий §7 — A5.7.


## 10. Промт (копировать целиком)

Ты — ведущий разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Это продолжение основного проекта «ИИ-агент в анонимной зоне»
(PLAN_AGENT_PRIVACY_ZONE.md). Ветка agent-privacy-zone, HEAD — docs-коммит A5.5
(сверить git log -1 --oneline), дерево чистое, push не делался.

Фазы A0–A4 закрыты (A4 — двустороннее зеркало, отчёт AGENT_A4_REPORT.md).
Фаза A5 (вкладка «Агент») идёт под-этапами: план, решения
(В-A5-1…8, В-A5.2-1…6, В-A5.3-1…5, В-A5.4-1…6, В-A5.5-1…6), долги, контракты и
грабли — AGENT_A5_STAGES.md (§1, §2, §3.1–§3.5, §4–§7).
A5.1 закрыт: проекты, состояние зеркала и файлы реестра.
A5.2 закрыт: реплика (предпросмотр «что увидит модель», «Скрыть ещё…»,
«Не скрывать», вложения, отправка хода, «Стоп»).
A5.3 закрыт: лента событий на SSE, переключатель «как видит модель»,
вопрос агента ask_user и POST /api/agent/answer.
A5.4 закрыт: панель «Результаты» — pending/compare/decision, отчёт
валидатора, «Сравнить», «Принять» (backup), «Отклонить», три исхода
конфликта, «файл занят»; A54_E2E.md.
A5.5 закрыт: панель «Метки проекта» — GET /api/agent/labels,
POST /api/agent/labels/merge|unmerge, merge_log + unmerge_entities
(vault_project_labels), пересборка образов, labels_merged/labels_unmerged
в очередь агенту; A55_E2E.md.
Контекст и методика этого под-этапа — AGENT_A5_6_HANDOFF_PROMPT.md (§3–§9),
этот промт — её §10.

Задача: под-этап A5.6 — панель «Настройки агента», уведомления фильтра и
«Очистить историю». Живой сценарий §7 — A5.7.

1. Прочитай AGENT_A5_6_HANDOFF_PROMPT.md целиком, затем AGENT_A5_STAGES.md
   (§2, §3.5, §4.1, §4.2, §5–§7) и PLAN_AGENT_PRIVACY_ZONE.md §5.6 (TR-6.4,
   TR-6.6, TR-6.7), §5.8 (TR-8.2, TR-8.3), §5.9 (инварианты), §7.
2. Ядро hds-agent: `Secrets::save` (слияние с существующим файлом, атомарная
   запись `replace_file`, неизвестные ключи и `mcp.servers` не теряются;
   значения не логируются) и тесты в `secrets.rs`; правки `settings.rs` —
   только если нужен новый ключ. `run::clear_history` и `history::clear` не
   переписывать — использовать как есть.
3. Маршруты hds-ui: `GET /api/agent/settings?project=`,
   `POST /api/agent/settings {project, provider?, model?, api_key?, …}`,
   `POST /api/agent/history/clear {project, session?}`. Под `agent::gate`
   и CSRF; в server-профиле 403. Ключ — только в `data\agent\secrets.json`;
   в ответах — «задан/нет» (маски значения нет). Правка `config.yaml` —
   редактором в духе `config_edit` (комментарии целы, `replace_file`,
   проверка YAML). Смена ключа/провайдера/модели — `AgentRegistry::unload`
   и `reopened: true` в ответе. Очистка: если ход идёт — `stop`, затем
   `unload`, затем `run::clear_history`, `notice` в ленту.
4. Панель в page.rs: таблица «параметр → значение → изменить», ключ
   «задан/нет», пороги, `filter.unknown`, `egress_log`, MCP; блок уведомлений
   фильтра (сводка по событиям `redacted`, без значений) и кнопка
   «Очистить историю» с `confirm()` и отчётом `{session, files, backups,
   replaced}`. Своих регулярок и разбора значений в JS нет (AG-1).
5. Не делать: живой сценарий §7 (A5.7), «Вернуть» из backup и журнал
   решений (A6), автопроверку истории перед завершением сессии (A6),
   Word-compare, правки демона/зеркала, sidecar/hds_agent, RPC-контрактов A5.5.
6. Тесты: unit hds-agent (`Secrets::save`, слияние, маска), unit hds-ui
   (разбор тела settings и history/clear → коды 400), интеграционные через
   реальный сокет (`run_http_listener` на порту 0, `install_global` +
   `reset_for_tests`, mock-воркер анонимизации из
   `crates/hds-ui/tests/fixtures/mock_anon.py`): GET settings (ключа нет),
   POST (projects.json/secrets.json/config.yaml изменились, комментарии
   целы), history/clear (отчёт, бэкапы, notice), 400/404/409, 403 без CSRF
   и в server-профиле. CLI `hds agent history clear` не сломан
   (cargo test -p hds-cli).
   Проверки: cargo fmt --all --check, clippy --workspace --all-targets --
   -D warnings, cargo test --workspace --no-fail-fast -- --test-threads=4,
   npm test, python unittest.
7. E2E стенда agent_a42: a56_e2e.ps1 — sync, GET settings, POST settings
   (модель + вымышленный ключ; в артефактах значения ключа нет; config.yaml
   с комментариями), ход через mock-провайдер с событием `redacted`,
   history/clear (files/replaced, бэкапы, notice, в agent-state форм
   fixtures нет), повторный ход — сессия возобновлена, sync changes=0,
   403/400/404; бэкап projects.json/secrets.json/config.yaml и
   восстановление в finally; отчёт A56_E2E.md.
8. Коммиты: feat(agent): A5.6 — ...; docs(agent): A5.6 закрыт — ...
   (+ AGENT_A5_STAGES.md §2, §3.6, §4.1, §4.2, §6, §7; README §20 — про
   настройки, уведомления фильтра и «Очистить историю»). В сообщении —
   что проверено.

