# AGENT_A5_2_HANDOFF_PROMPT.md — контекст и промт для под-этапа A5.2 (реплика: предпросмотр и отправка)

> **Кому:** новый чат Cline в режиме Act. **Дата среза:** 20.11.2026.
> **Ветка:** `agent-privacy-zone`, **HEAD:** `cd398c76` (`docs(agent): A5.1 закрыт`);
> ahead 70, дерево чистое, push не делался.
> **Что это:** самодостаточный контекст для продолжения **фазы A5** основного проекта
> `PLAN_AGENT_PRIVACY_ZONE.md` («ИИ-агент в анонимной зоне»). A0–A4 закрыты, A5 разбит
> на семь под-этапов (`AGENT_A5_STAGES.md`), **A5.1 закрыт** (вкладка «Агент»: проекты,
> состояние зеркала, файлы). Этот файл — A5.2: **реплика пользователя** — предпросмотр
> «что увидит модель», «Скрыть ещё…», «Не скрывать», вложения и пути, отправка хода.
> Лента событий с SSE и `ask_user` — A5.3, решения — A5.4, метки — A5.5, настройки и
> «Очистить историю» — A5.6, живой сценарий §7 — A5.7.

## 0. Где мы (коротко)

Пользователь работает с документами в своей папке, агент — только с анонимными образами
в `<agent.root>\P<NNNN>\workspace`; исходные значения не покидают машину (I1), оригиналы
не меняются без решения пользователя (I6), в журналах нет значений (I8).

* **A4** — двустороннее зеркало `hds agent sync` (реестр, прямой и обратный путь,
  решения, валидатор, демон и журнал) — закрыта, отчёт `AGENT_A4_REPORT.md`.
* **A5** — вкладка «Агент» в `hds ui`. План под-этапов, решения (В-A5-1…8), долги,
  контракты и грабли — `AGENT_A5_STAGES.md`.
* **A5.1** — каркас вкладки и API состояния: `GET/POST /api/agent/projects`,
  `GET /api/agent/state|files`, 403 в server-профиле (D20), перенос ленивых фабрик
  воркеров в `hds-agent`, панель «Файлы проекта» по реестру.

**Зачем A5.2.** Сейчас вкладка только показывает состояние: чтобы поговорить с агентом,
пользователь идёт в CLI (`hds agent run --prompt …`). A5.2 делает реплику главным
действием вкладки и держит главный инвариант TR-5.1: **пользователь видит ровно тот
текст, который уйдёт модели** (метки вместо значений) и может поправить это до отправки.

## 1. Состояние репозитория (проверено 20.11.2026)

| Что | Значение |
|---|---|
| Ветка / HEAD | `agent-privacy-zone`, `cd398c76` (docs A5.1); коммиты A5: `4bc3ba21` (план), `be398515` (A5.1 feat), `cd398c76` (A5.1 docs) |
| Тесты | `cargo test --workspace -- --test-threads=4` — **124 набора / 862 passed / 0 failed / 13 ignored**; python 328 OK (skip 1); `npm test` 23/23 |
| fmt / clippy | чисто (`cargo fmt --all --check`, `clippy --workspace --all-targets -- -D warnings`) |
| Стенд | `tools\parity\out\agent_a42`: 34 записи реестра (31 `synced`, 2 `accepted`, 1 `unsupported`), `pending` — 0; `HDS_ROOT=…\root`, `HDS_CONFIG=…\config.yaml` |
| Отчёт A5.1 | `tools\parity\out\agent_a42\A51_E2E.md` (проекты/состояние/файлы, 34 файла, `daemon=false`, журнал 12 строк) |

## 2. Обязательное чтение (по порядку)

1. `AGENT_A5_STAGES.md` — целиком; особенно §1 (опора), §2 (A5.2/A5.3), §4.1 (решения
   В-A5-1…8), §5 (контракты), §6 (грабли), §7 (окружение), §8 (промт A5.1 — образец).
2. `PLAN_AGENT_PRIVACY_ZONE.md` — §5.5 (TR-5.1…5.3), §5.6 (TR-6.1…6.7), §4.3 (канал
   `hds ui` ↔ агент), §4.4 (раскладка), §5.8 (TR-8.2), §6 (строка A5), §7 (приёмка).
3. `AGENT_A4_STAGES.md` §4.3, §5–§7 — демон, журнал, контракты и грабли, которые
   действуют и в UI.
4. Этот файл — §3–§9; промт — §10.

## 3. Что уже готово — опора A5.2

| Что | Где | Чем пользоваться |
|---|---|---|
| Шлюз | `crates/hds-agent/src/gateway.rs` | `Gateway::spawn(spec, host, project, workspace, state_dir, filter, deanon, client)`, `open(provider, history, session_id, max_iterations, subagents)`, `send(text)`, `send_anonymized(text)`, `abort()`, `stop()`, `next_event(timeout)`, `clear_history()`, `shutdown_with` |
| События | там же | `GatewayEvent::Ready{tools}`, `Agent{raw, text, unknown}`, `TurnEnd{ok, reason, text, error, tools, raw}`, `Redacted{hits}`, `Done{ok, result, error}`. **`text` уже деанонимизирован**, `raw` — «как видит модель» |
| Обработчик `host_call` | там же | `GatewayHost::new(project, workspace, filter, client, ask, egress_log)`, `.with_refresh(RefreshState{system, projects, secrets, workspace, project_root, deanon})`; `NoAskUser` пока отказывает (A5.3) |
| Реестр сессий | `crates/hds-agent/src/registry.rs` | `AgentRegistry::from_settings(&settings)`, `get_or_open(project, now: Instant, \|\| -> Result<Gateway, GatewayError>) -> Result<&mut Gateway, _>`, `reap_idle`, `unload` |
| Фильтр | `crates/hds-agent/src/filter.rs` | `Filter::new(project, keep_terms, unknown, max_stale, audit)`, `set_snapshot(snapshot, system, project_names, secrets)`, `take_redactions()` |
| Деанонимизация | `crates/hds-agent/src/deanon.rs` | `DeAnonymizer::new(&snapshot, &sys_values, &workspace, &project_root)` |
| Клиент анонимизации | `crates/hds-anonymize/src/client.rs` | `anonymize_text(project, text) -> TextResult{text, labels: Vec<TokenCount{token, entity_type, count}>, project_id, warnings}`; `label_for_value(project, value, type) -> LabelForValue{label, value_type, kept, confident}`; `keep_terms()`, `add_keep_term(value) -> bool`; `snapshot(project)`; `anonymize_agent_to(...)` |
| Секреты | `crates/hds-agent/src/secrets.rs` | `Secrets::load(path)`, `provider_for(project)`, `project_provider/model/token`, `secret_strings()`, `mcp_servers()` |
| План хода (сейчас в CLI) | `crates/hds-cli/src/cmd/agent_run.rs` | `RunOptions`, `RunPlan{project, provider, workspace, config_dir, state_dir, node, worker, spec, system, project_names, settings, mcp, audit, secrets}`, `plan_with(cfg, id, opts, sidecar_root)`, `prepare(plan)`, `open_client(cfg, plan, root)`, `open_client_at(cfg, state_dir, root)`, `build_filter(...)`, `run(plan, client, opts, out, err)` |
| UI | `crates/hds-ui/src/{agent,lib,page}.rs` | `agent::gate()` (403 в server), `projects_json/project_add_json/state_json/files_json`, `route_full`, вкладка `tab-agent`, `jget/jpost/esc` |

**Чего в A5.2 не делать:** SSE и лента вкладки (A5.3), `ask_user` (A5.3), панель
«Результаты» и решения (A5.4), «Метки проекта» (A5.5), настройки и «Очистить историю»
(A5.6). Но **pump-поток и кольцевой буфер событий** нужны уже в A5.2: без них
`Gateway::send`/`next_event` нельзя звать из HTTP-обработчика (§6 п.1), а A5.3 добавит
только транспорт (SSE) и разметку ленты.


## 4. Задачи A5.2 с методикой

### 4.1. Перенос плана хода в `hds-agent` (делать первым)

**Почему.** `hds-cli` зависит от `hds-ui` (CLI запускает веб-сервер), значит `hds-ui`
не может зависеть от `hds-cli`. А весь план хода — `RunPlan`, `plan_with`, `prepare`,
`open_client`, `build_filter` и помощники — сейчас в `crates/hds-cli/src/cmd/agent_run.rs`.
Это тот же случай, что фабрики воркеров в A5.1: код переезжает в `hds-agent`, CLI
остаётся тонкой обёрткой (реэкспорт), тесты не меняются.

**Что перенести** (модуль `crates/hds-agent/src/run.rs`, `pub mod run;` в `lib.rs`):

* `RunOptions` (промпт можно оставить в CLI, но `session`, `max_iterations`, `subagents`
  нужны и UI — проще перенести структуру целиком);
* `RunPlan` и `TurnReport`. Поле `secrets: Secrets` сделать приватным, доступ — методами
  (`plan.secrets()`), чтобы случайно не сериализовать ключи;
* `provider_kind`, `default_base_url`, `resolve_node`, `resolve_worker`,
  `system_values`, `sys_values`, `mcp_port`, `mcp_base_path`, `tavily_script`;
* `plan_with` / `plan`, `prepare`, `build_filter`, `open_client`, `open_client_at`;
* `run(plan, client, opts, out, err)` можно оставить в CLI (он печатает в stdout) —
  в `hds-agent` нужны только сборка плана, подготовка каталогов и клиент.

**Правило:** `plan_json` по-прежнему без секретов; `hds agent run --dry-run` печатает то
же; `crates/hds-cli/tests/agent_run.rs` и `agent_acceptance.rs` проходят без правок
(импорт меняется только внутри CLI).

### 4.2. Состояние вкладки: сессии, pump-поток, буфер событий

Новый модуль `crates/hds-ui/src/agent_session.rs` (или расширить `agent.rs`):

```rust
pub struct AgentApi {
    registry: Mutex<AgentRegistry>,          // LRU: max_live_projects, idle_unload_min
    turns: Mutex<HashMap<String, Arc<Turn>>>, // по проекту; заводится при первом сообщении
    client: Mutex<Option<Arc<Mutex<Client>>>>, // один Python-воркер на процесс UI
    seq: AtomicU64,                           // сквозной номер события
}

pub struct Turn {
    busy: AtomicBool,               // один активный ход на проект
    events: Mutex<VecDeque<Item>>,  // кольцо на 500 событий
    session: Mutex<Option<String>>, // session_id воркера
}

pub struct Item { pub seq: u64, pub kind: String, pub ts: f64,
                  pub text: Option<String>, pub raw: Option<Value> }
```

* `OnceLock<AgentApi>` в проде + `AgentApi::for_tests()` (структура с конструктором, как
  `agent::gate()`; глобальное состояние между тестами не сбрасывается — §6 п.8).
* **Pump-поток**: заводится при `open`; в цикле `next_event(500 ms)`:
  `Agent{raw, text}` → `Item{kind:"agent", text, raw}`; `Redacted{hits}` →
  `kind:"redacted"` (только метки); `TurnEnd`/`Done` → `kind:"turn_end"` и `busy=false`;
  ошибка → `kind:"error"` и `busy=false`. Поток завершается по `stop`/`shutdown`.
* Буфер обрезается до 500; `seq` не переиспользуется — UI просит `after=<seq>`.
* `reap_idle` вызывать при обращении к API (простой → выгрузка) и **не рвать** буфер:
  события остаются, сессия возобновляется по `session_id` (`history::read_messages`).


### 4.3. Предпросмотр реплики (TR-5.1)

`POST /api/agent/preview` `{project, text}` →

```json
{ "text": "Подготовь письмо [PERSON_1|дат] …",
  "labels": [{"token":"[PERSON_1|дат]","type":"PERSON","count":1}],
  "warnings": [],
  "changed": true }
```

* только `Client::anonymize_text(project, text)` — NER + regex + словарь форм проекта +
  `keep_terms`; своих регулярок меток в UI быть не должно (AG-1);
* лимит текста — 20 000 символов (иначе 400); пустой текст — 400;
* `changed` = текст изменился; `warnings` воркера — как есть (без значений);
* `agent.preview_outgoing` — ключ CLI-вывода (`hds agent run`), в UI предпросмотр показ
  всегда: это и есть подтверждение отправки.

### 4.4. «Скрыть ещё…» и «Не скрывать»

* `POST /api/agent/label` `{project, value, type?}` → `label_for_value` →
  `{label, type, kept, confident}`. `kept=true` — значение уже в `keep_terms`, метка не
  выдаётся (D19): UI так и пишет. `label=null` — не удалось разобрать, UI предлагает
  другое выделение. После ответа UI повторяет `preview`.
* `POST /api/agent/keep` `{project, value}` → `add_keep_term` → `{added}`; затем
  повторный `preview`. **`keep_terms` — база, а не проект** (вопрос В-A5.2-2).

### 4.5. Вложения и пути (TR-5.2)

`POST /api/agent/attach` `{project, path}`:

* путь **внутри** папки проекта → относительный путь → `ArtifactRegistry::by_orig(rel)`:
  * запись есть → `{kind:"image", path:"<P>\\workspace\\<anon_rel>", rel}`;
  * записи нет (`unsupported`/`too_big`/ещё не зеркалирован) → 409 с текстом
    «файл не зеркалирован — запустите hds agent sync»;
* путь **вне** папки проекта → разовая копия в `workspace\_attachments\<имя>` через
  `Client::anonymize_agent_to` (или `anonymize` с `mode:"agent"`); ответ
  `{kind:"attachment", path}`; в реестр — `Kind::Attachment`, если это не ломает
  прямой путь (иначе без записи, с долгом в журнале);
* в тексте реплики абсолютные пути Windows/UNC распознаются регуляркой и заменяются
  путём образа или меткой `[PATH_n]`; соответствия возвращаются в `preview`
  (`paths:[{from,to}]`), чтобы UI показал замену.

### 4.6. Отправка, «Стоп», один активный ход

`POST /api/agent/message` `{project, text, session?}`:

1. 400 — пустой текст/нет проекта; 409 — `busy` («ход уже идёт, дождитесь или нажмите
   Стоп»); 503 — `agent.enabled: false` или `agent.root` не задан;
2. `run::plan_with(cfg, project, opts, root)` → `run::prepare(&plan)` → клиент из кэша
   (`run::open_client_at`) → снимок vault → `Filter`/`DeAnonymizer`/`GatewayHost
   +with_refresh` → `Gateway::spawn` (или `registry.get_or_open`);
3. `gateway.open(provider, history, session, max_iterations, subagents)` — один раз на
   сессию; `history` — `history::read_messages(state_dir, session)` для возобновления;
4. `gateway.send(&text)` — **исходный** текст: шлюз анонимизирует его сам (В-A5.2-1);
5. ответ `{ok:true, session_id, seq}`; дальше — события из буфера.

`POST /api/agent/abort` / `POST /api/agent/stop` `{project}` → `Gateway::abort/stop`;
404, если сессии нет. `busy` снимается по `TurnEnd`/`Done`/ошибке, не по таймауту.

### 4.7. События без SSE (транспорт A5.3 не трогаем)

`GET /api/agent/events?project=<P>&after=<seq>&limit=200` →

```json
{ "busy": true, "session": "…", "seq": 42,
  "events": [{"seq":41,"kind":"agent","ts":1795172999.1,"text":{…},"raw":{…}}] }
```

Кольцо 500, `after` по умолчанию 0, `limit` ≤ 500. Это даёт A5.2 проверяемый снаружи
результат (curl/PS) и не мешает A5.3 добавить `text/event-stream` тем же маршрутом.

### 4.8. UI вкладки

Секция «Реплика» на `tab-agent`:

* `textarea` + блок «Что увидит модель» (результат preview, метки подсвечены);
* кнопки «Скрыть ещё…» (по выделению → `/label` → повторный preview), «Не скрывать»
  (`/keep`), «Вложить файл…» (поле пути → `/attach`), «Отправить», «Стоп»;
* статус: `busy`, `session_id`, последние 5 событий текстом (полная лента — A5.3);
* ошибки 400/403/409/503/500 — текстом рядом с кнопками; в server-профиле вкладки нет.


## 5. Порядок работ и критерии приёмки

Порядок: **4.1** (перенос плана) → 4.2 (сессии/буфер) → 4.3 (preview) → 4.6 (отправка)
→ 4.4 (label/keep) → 4.5 (вложения) → 4.7 (события) → 4.8 (UI). Каждый шаг — зелёный
тест и коммит, как в A4/A5.1.

Готово, когда:

1. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` — зелёные; `npm test` 23/23; python 328 OK (skip 1).
2. **Тесты A5.2**: unit — лимит и пустой текст, `after/limit` в событиях, обрезка буфера
   до 500, `is_project_id`-стиль валидация; интеграционные (mock-воркер
   `HDS_ANONYMIZE_WORKER`, mock-провайдер `MOCK_JS`) — preview даёт метку, label/keep,
   409 на второй ход, abort снимает `busy`, 403 в server-профиле.
3. **E2E стенда** `agent_a42` (mock-провайдер, вымышленные данные, отчёт `A52_E2E.md`):
   * `preview` на «Подготовь письмо Иванову Ивану» → в тексте `[PERSON_1|дат]`, «1С:ERP»
     не скрыт (D19), `changed=true`;
   * `label` для «Петрова» → `[PERSON_2]`; `keep` для «Ромашка» → `added=true`, повторный
     preview без метки;
   * `attach` файла из папки проекта → путь образа `workspace\…[ORG_1].docx`;
     файл вне папки → `_attachments`;
   * `message` → 200, `busy=true`, события приходят (agent/turn_end), второй `message` →
     409, `abort` → `busy=false`;
   * в ответах API нет ключа провайдера и значений из `keep`-тестов (I8).
4. Значения в журналах/аудите не появляются; `sync.log` не трогаем.

## 6. Грабли (оплачены в A0–A5.1 — не наступать снова)

1. **`Gateway::send` и `next_event` — блокирующие.** Только из pump-потока/фонового
   потока; из HTTP-обработчика нельзя: `read_timeout` UI (30 с) оборвёт запрос, а вкладка
   «зависнет». `send` возвращается по ответу воркера, события идут после него.
2. **`Gateway` не `Sync`** — только внутри `Mutex` (`AgentRegistry` это уже делает:
   `get_or_open` отдаёт `&mut Gateway`). Замок держать минимально: `send`/`open` под ним,
   а `next_event` — в отдельном потоке.
3. **`AgentRegistry` выгружает по простою** (`idle_unload_min`) и при `max_live_projects`:
   после выгрузки буфер событий не терять, `session_id` возобновлять через
   `history::read_messages` + `open(..., session_id)`.
4. **`plan_with` требует `agent.root`** и зарегистрированный проект: 503/404 с понятным
   текстом, а не паника.
5. **Секреты.** `RunPlan.secrets` не сериализовать; `plan_json` — без ключей; в ответах
   API ключа нет. `api_key` — только в памяти.
6. **`keep_terms` — глобальная база**, «Не скрывать» влияет на все проекты (В-A5.2-2):
   так и написать в UI.
7. **Двойная анонимизация.** Отправлять исходный текст (`send`), превью — только показ:
   `send(preview.text)` даст метки внутри меток.
8. **Состояние между тестами.** `OnceLock` не сбрасывается: `AgentApi::for_tests()` с
   собственным состоянием; env (`HDS_CONFIG`/`HDS_ROOT`) — под общим мьютексом.
9. **PS 5.1 `Invoke-RestMethod` декодирует JSON как Latin-1** (нет `charset` в
   `Content-Type`): кириллица «мохнатая». Проверять через `Invoke-WebRequest` +
   `[Text.Encoding]::UTF8.GetString($r.RawContentStream.ToArray())`.
10. **`HDS_ROOT`/`HDS_CONFIG` из env стенда ломают `cargo test`** — снимать перед
    прогоном; `HDS_ANONYMIZE_PYTHON`/`HDS_ANONYMIZE_WORKER` — только для тестов/e2e.
11. **`insert_line` в конец файла** легко попадает внутрь последнего теста — после
    вставки проверять структуру (`cargo fmt --check` и число тестов).
12. **`agent_acceptance` (I4) флейкует** под параллельной нагрузкой (`~/.cline` пишет
    IDE) — гонять отдельно: `cargo test -p hds-cli --test agent_acceptance`.
13. **Mock-провайдер** — `MOCK_JS` из `crates/hds-agent/tests/gateway_worker.rs`
    (OpenAI-совместимый, слушает `127.0.0.1:0`, пишет журнал запросов); `provider` в
    `projects.json` — `openai-compatible`, `base_url` — в `secrets.json` стенда.


## 7. Окружение и команды

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager status; git --no-pager log -3 --oneline
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p hds-agent
cargo test -p hds-ui
cargo test -p hds-cli --test agent_run
cargo test --workspace -- --test-threads=4 *> tools\parity\out\a52-cargo-test.log
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

Стенд UI (`tools\parity\out\agent_a42`) — как в A5.1:

```powershell
$env:HDS_CONFIG='<repo>\tools\parity\out\agent_a42\config.yaml'
$env:HDS_ROOT='<repo>\tools\parity\out\agent_a42\root'
target\debug\hds.exe ui --port 8799
# preview (POST с CSRF-заголовком)
$h=@{'X-HDS-UI'='1';'Content-Type'='application/json'}
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/preview' -Method Post -Headers $h `
  -Body (@{project='P0001';text='...'} | ConvertTo-Json)
# события (A5.2 — обычный JSON):
Invoke-RestMethod 'http://127.0.0.1:8799/api/agent/events?project=P0001&after=0'
Remove-Item Env:HDS_ROOT, Env:HDS_CONFIG -ErrorAction SilentlyContinue   # перед cargo test
```

* **Mock-провайдер**: возьмите `MOCK_JS` из `crates/hds-agent/tests/gateway_worker.rs`,
  положите `mock_provider.mjs` в каталог стенда, запустите `node mock_provider.mjs` с
  `MOCK_SCRIPT`/`MOCK_REQUESTS`; в `secrets.json` стенда — `providers.openai-compatible.base_url`
  на его порт, `projects.json` — `provider: "openai-compatible"`.
* **Артефакты** — только `tools/parity/out/` (гитигнорится); отчёт — `A52_E2E.md` в
  каталоге стенда; сценарий — `a52_e2e.ps1` (ASCII-only, PS 5.1).
* **git-identity не настроена**: `git -c user.name='Alex Guerchoig'
  -c user.email='Guerchoig@gmail.com' commit -F <файл UTF-8>`, пути — явным списком.
* Машина разработческая: `hds ui`, демон зеркала, `llm_host` можно останавливать.

## 8. Вопросы владельцу (предложены дефолты)

| # | Вопрос | Предложенный дефолт |
|---|---|---|
| В-A5.2-1 | Отправлять исходный текст или показанный предпросмотр? | **Исходный**: `Gateway::send` анонимизирует сам; `send(preview.text)` дал бы метки внутри меток. Предпросмотр — только подтверждение |
| В-A5.2-2 | «Не скрывать» пишет в глобальные `keep_terms` | Да, так устроена база (D19); в UI подписать «не скрывать это значение во всех проектах» |
| В-A5.2-3 | Второй ход при активном | 409 «ход уже идёт», кнопка «Стоп»; очередь — фаза 2 |
| В-A5.2-4 | Продолжать последнюю сессию? | Да: `latest_session` при первом сообщении проекта; кнопка «Новая сессия» — A5.3/A5.6 |
| В-A5.2-5 | Файл вне папки проекта | Копия в `workspace\_attachments` (анонимизированная), чистка — A5.6 |
| В-A5.2-6 | Лимит реплики | 20 000 символов (400 сверх) |

## 9. Срез на 20.11.2026 (для проверки в новом чате)

* Ветка `agent-privacy-zone`, HEAD `cd398c76`, ahead 70, дерево чистое; push не делался.
* `cargo test --workspace -- --test-threads=4` — 124 набора / 862 passed / 0 failed /
  13 ignored; python 328 OK (skip 1); `npm test` 23/23; fmt/clippy чисто.
* Стенд `agent_a42`: 34 записи (31 `synced`, 2 `accepted`, 1 `unsupported`), `pending` 0;
  `A51_E2E.md` — вкладка, проекты, состояние, файлы.
* A5.1 закрыт: `be398515` (feat) + `cd398c76` (docs); план A5 — `4bc3ba21`.


## 10. Промт для нового чата (копировать целиком)

```text
Ты — ведущий разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Это продолжение основного проекта «ИИ-агент в анонимной зоне»
(PLAN_AGENT_PRIVACY_ZONE.md). Ветка agent-privacy-zone, HEAD — docs-коммит A5.1
(сверить git log -1 --oneline), дерево чистое, push не делался.

Фазы A0–A4 закрыты (A4 — демон двустороннего зеркала, отчёт AGENT_A4_REPORT.md).
Фаза A5 (вкладка «Агент») разбита на под-этапы, план — AGENT_A5_STAGES.md.
A5.1 закрыт: вкладка показывает проекты, состояние зеркала и файлы реестра
(crates/hds-ui/src/agent.rs, маршруты /api/agent/*). Контекст и методика этого
под-этапа — AGENT_A5_2_HANDOFF_PROMPT.md (§3–§9), этот промт — её §10.

Задача: под-этап A5.2 — реплика пользователя: предпросмотр, «Скрыть ещё…»,
«Не скрывать», вложения и пути, отправка хода. Лента с SSE и ask_user — A5.3,
решения — A5.4, метки — A5.5, настройки — A5.6.

1. Прочитай AGENT_A5_2_HANDOFF_PROMPT.md целиком, затем AGENT_A5_STAGES.md (§1, §2,
   §4.1, §5–§7) и PLAN_AGENT_PRIVACY_ZONE.md §5.5 (TR-5.1…5.3), §4.3, §5.8 (TR-8.2).
2. Перенеси план хода из CLI в hds-agent (модуль run): RunOptions, RunPlan, TurnReport,
   plan_with/plan, prepare, open_client/open_client_at, build_filter и помощники
   (provider_kind, default_base_url, resolve_node/resolve_worker, system_values,
   sys_values, mcp_port/mcp_base_path, tavily_script). CLI
   (crates/hds-cli/src/cmd/agent_run.rs) — тонкая обёртка/реэкспорт; поле secrets
   приватное, plan_json без ключей; тесты agent_run и agent_acceptance не меняются.
3. Состояние вкладки: AgentApi (OnceLock + for_tests): Mutex<AgentRegistry>,
   Mutex<HashMap<project, Arc<Turn>>>, кэш клиента анонимизации, seq; Turn { busy,
   кольцо событий на 500, session_id }; pump-поток next_event(500 мс) после open;
   busy снимается по TurnEnd/Done/ошибке, не по таймауту.
4. Маршруты (POST — под общим CSRF-гейтом, в server-профиле 403 через agent::gate):
   preview {project,text} → Client::anonymize_text (лимит 20 000, пустой — 400);
   label {project,value,type?} → label_for_value; keep {project,value} →
   add_keep_term; attach {project,path} → путь образа по реестру или _attachments;
   message {project,text,session?} → plan/prepare/open_client/Gateway::spawn или
   get_or_open → open(provider, history, session, …) → send(исходный текст); второй
   message — 409; abort/stop; events?after=&limit= → {busy, session, seq, events}.
5. Вкладка: секция «Реплика» — textarea, блок «Что увидит модель» (метки подсвечены),
   «Скрыть ещё…» (по выделению), «Не скрывать», «Вложить файл…», «Отправить», «Стоп»,
   статус (busy, session, последние 5 событий), ошибки 400/403/409/503 текстом.
   Ленту, SSE и ask_user не делать — A5.3.
6. Тесты: unit (лимит/пустой текст, after/limit, обрезка кольца, валидация) и
   интеграционные с mock-воркером (HDS_ANONYMIZE_WORKER) и mock-провайдером (MOCK_JS
   из crates/hds-agent/tests/gateway_worker.rs): preview → метка, label/keep, 409 на
   второй ход, abort, 403 в server-профиле.
7. Проверки: cargo fmt --all --check, clippy --workspace --all-targets -- -D warnings,
   cargo test --workspace -- --test-threads=4, npm test, python unittest; e2e стенда
   agent_a42 (mock-провайдер): preview «Подготовь письмо Иванову Ивану» →
   [PERSON_1|дат] и «1С:ERP» не скрыт; label → [PERSON_2]; keep; attach; message →
   события, 409, abort; отчёт A52_E2E.md.
8. Коммиты: feat(agent): A5.2 — ...; docs(agent): A5.2 закрыт — ... (+
   AGENT_A5_STAGES.md §2, §3.2, §4.2, §6, §7; README §20 — про реплику). В сообщении —
   что проверено.

Проверка среза: ветка agent-privacy-zone, HEAD cd398c76 (+docs этого файла), ahead 70,
дерево чистое; cargo test --workspace — 124 набора / 862 passed / 0 failed
(13 ignored); python 328 OK (skip 1); npm test 23/23; стенд agent_a42 — 34 записи
реестра, отчёт A51_E2E.md.
```

