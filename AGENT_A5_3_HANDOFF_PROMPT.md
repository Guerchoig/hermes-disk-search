# AGENT_A5_3_HANDOFF_PROMPT.md — контекст и промт для под-этапа A5.3 (лента событий с SSE и `ask_user`)

> **Кому:** новый чат Cline в режиме Act. **Дата среза:** 20.11.2026.
> **Ветка:** `agent-privacy-zone`, **HEAD:** `144cdd33` (`docs(agent): A5.2 закрыт`);
> ahead 73, дерево чистое, push не делался.
> **Что это:** самодостаточный контекст для продолжения **фазы A5** основного проекта
> `PLAN_AGENT_PRIVACY_ZONE.md` («ИИ-агент в анонимной зоне»). A0–A4 закрыты, A5 разбит на
> семь под-этапов (`AGENT_A5_STAGES.md`), **A5.1 и A5.2 закрыты**. Этот файл — A5.3:
> **лента событий** (стриминг, деанонимизация в ленте, переключатель «как видит модель») и
> **вопрос агента пользователю** (`ask_user`). Решения — A5.4, метки — A5.5, настройки и
> «Очистить историю» — A5.6, живой сценарий §7 — A5.7.

## 0. Где мы (коротко)

Пользователь работает с документами в своей папке, агент — только с анонимными образами
в `<agent.root>\P<NNNN>\workspace`; исходные значения не покидают машину (I1), оригиналы
не меняются без решения пользователя (I6), в журналах нет значений (I8).

* **A4** — двустороннее зеркало `hds agent sync` — закрыта, отчёт `AGENT_A4_REPORT.md`.
* **A5** — вкладка «Агент» в `hds ui`. План под-этапов, решения (В-A5-1…8, В-A5.2-1…6),
  долги, контракты и грабли — `AGENT_A5_STAGES.md`.
* **A5.1** — каркас вкладки и API состояния: `/api/agent/projects|state|files`, 403 в
  server-профиле (D20), панель «Файлы проекта».
* **A5.2** — реплика: `preview` («что увидит модель»), `label` («Скрыть ещё…»), `keep`
  («Не скрывать»), `attach` (образ по реестру / `_attachments`), `message`, `abort`, `stop`,
  `GET /api/agent/events` (JSON). План хода переехал в `hds_agent::run`; в UI появились
  `agent_session` (сессии, кольцо 500 событий, `seq`, насос) и секция «Реплика».

**Зачем A5.3.** Сейчас вкладка показывает только **последние 20 строк** событий,
собранных опросом раз в 1,2 с: не видно рассуждений, вызовов инструментов и их вывода,
а вопрос агента (`ask_user`) получает отказ — `NoAskUser`. A5.3 делает ленту главным
экраном хода: поток событий (SSE), настоящие значения в ленте (метки — по переключателю),
и даёт агенту возможность спросить пользователя, не выходя из вкладки.

**Важно (отклонение от плана, зафиксировано в §3.2 `AGENT_A5_STAGES.md`).** Часть работ
A5.3 сделана в A5.2, потому что без неё нельзя было отправлять ход:
pump-поток (`next_event(500 мс)`), кольцо на 500 событий, сквозной `seq`, маршрут
`GET /api/agent/events` в JSON и опрос в UI. A5.3 добавляет **транспорт SSE**, **разметку
ленты** (склейка дельт, инструменты, служебные события), **`ask_user`** и
**деанонимизацию вопроса**. Переделывать кольцо и насос не нужно.

## 1. Состояние репозитория (проверено 20.11.2026)

| Что | Значение |
|---|---|
| Ветка / HEAD | `agent-privacy-zone`, `144cdd33` (docs A5.2); коммиты A5.2: `ec37004c` (feat), `144cdd33` (docs) |
| Тесты | `cargo test --workspace -- --test-threads=4` — **125 наборов / 875 passed / 0 failed / 13 ignored**; python 328 OK (skip 1); `npm test` 23/23 |
| fmt / clippy | чисто (`cargo fmt --all --check`, `clippy --workspace --all-targets -- -D warnings`) |
| Новые наборы A5.2 | `hds-agent` unit (4, `run::`), `hds-ui` unit (4, `agent_session::`), `hds-ui/tests/agent_chat.rs` (5) |
| Стенд | `tools\parity\out\agent_a42`: 34 записи реестра; `HDS_ROOT=…\root`, `HDS_CONFIG=…\config.yaml` |
| Отчёт A5.2 | `tools\parity\out\agent_a42\A52_E2E.md` (preview/label/keep/attach/message/409/abort, 14 событий, ключа нет) |

## 2. Обязательное чтение (по порядку)

1. `AGENT_A5_STAGES.md` — целиком; особенно §2 (строка A5.3), §3.2 (что уже сделано в A5.2),
   §4.1 (решения В-A5-1…8 и ответы В-A5.2-1…6), §4.2 (долги, п.19 — события на опросе),
   §5 (контракты), §6 (грабли, п.1–3 и 19–24), §7 (окружение).
2. `PLAN_AGENT_PRIVACY_ZONE.md` — §5.5 (TR-5.1…5.3), §5.6 (TR-6.x), §4.3 (канал `hds ui` ↔
   агент, `ask_user`), §4.4 (раскладка), §5.8 (TR-8.2), §6 (строка A5), §7 (приёмка).
3. `AGENT_A5_2_HANDOFF_PROMPT.md` §3–§9 — контекст и методика предыдущего под-этапа
   (структура этого файла повторяет её).
4. `crates/hds-ui/src/agent_session.rs` — целиком: насос, кольцо, `events_json`, `message`.
5. `crates/hds-ui/src/lib.rs` — `write_reply_with`, `run_http_listener`, `ui_auth_gate`,
   `route_full` (ветка `/api/agent/*`).
6. `sidecar/hds_agent/src/{tools,events,egress}.ts` — инструмент `ask_user`, виды событий,
   `egress_check` на каждом запросе к провайдеру.

## 3. Что уже готово — опора A5.3

| Что | Где | Чем пользоваться |
|---|---|---|
| Шлюз | `crates/hds-agent/src/gateway.rs` | `Gateway::spawn/open/send/send_anonymized/abort/stop/next_event/clear_history/shutdown_with`; `GatewayError::is_timeout()` + `TIMEOUT_EVENT` (A5.2) |
| События шлюза | там же | `Ready{tools}`, `Agent{raw, text, unknown}`, `TurnEnd{…}`, `Redacted{hits}`, `Done{…}`; **`text` уже деанонимизирован**, `raw` — «как видит модель» |
| **Вопрос пользователю** | `gateway.rs:47` | `trait AskUser { fn ask(&self, question: &str) -> Result<Value, String> }`; `HostCall::AskUser{question}` → `self.ask.ask(&question)` (`gateway.rs:442`); заглушка `NoAskUser` (`gateway.rs:54`) отказывает |
| Обработчик `host_call` | `gateway.rs:186` | `GatewayHost::new(project, workspace, filter, client, ask, egress_log).with_refresh(RefreshState{system, projects, secrets, workspace, project_root, deanon})` |
| Деанонимизация | `crates/hds-agent/src/deanon.rs` | `DeAnonymizer::new(&snapshot, &sys_values, &workspace, &project_root)`; `text()` — метки → значения, пути `workspace` → пути папки проекта |
| Состояние вкладки | `crates/hds-ui/src/agent_session.rs` | `RING=500` (49), `POLL=500 мс` (52), `Turn{busy, opened, stop, events, session, pump}` (105), `events_json` (247), `message` (682), `start_turn` (708), `finish_turn` (885), `abort` (979), `stop` (1004), `pump_loop` (1081) |
| Кольцо событий | там же | `Item{seq, kind, ts, text: Option<Value>, raw: Option<Value>}`; виды, которые уже кладёт насос: `prompt`, `session`, `agent`, `redacted`, `turn_end`, `done`, `notice`, `error` |
| Маршруты вкладки | `crates/hds-ui/src/lib.rs` | `GET /api/agent/events?project&after&limit` (JSON), `POST /api/agent/{preview,label,keep,attach,message,abort,stop}` — CSRF + `agent::gate()` |
| HTTP-сервер | `lib.rs` | `write_reply_with` (2497: `Content-Length` + `Connection: close`), `run_http_listener` (2540: поток на соединение, `ConnGuard`, `http.max_conns` 32, `http.read_timeout_sec` 30), `read_request` (2420), гейты `ui_auth_gate` (1758) / `ui_user_gate` (1791), `route_full` |
| Вкладка | `crates/hds-ui/src/page.rs` | секция «Реплика» (304), `<pre id="agevents">` (322), `agentPoll()` (716, опрос 1,2 с, `AG_SEQ`, `AG_LINES`), `agentEventText` |
| Воркер: `ask_user` | `sidecar/hds_agent/src/tools.ts` | инструмент вызывает `channel.host('ask_user', {question}, 30 мин)` и возвращает модели `text(result?.answer ?? result)` — **ожидаемая форма ответа `{"answer": "…"}`** |
| Воркер: виды событий | `sidecar/hds_agent/src/events.ts` | `text_delta`, `reasoning_delta`, `tool_use`, `text`, `reasoning`, `tool_result`, `iteration`, `usage`, `notice`, `error`, `agent_done`, `turn_end`; вывод инструментов клипован 20 000 символов |
| Воркер: egress | `sidecar/hds_agent/src/egress.ts` | **каждый** запрос к провайдеру проходит `host_call egress_check {kind:"model", body}`; сюда попадает и результат `ask_user` ⇒ ответ пользователя фильтруется до модели |
| Mock-провайдер | `crates/hds-cli/tests/fixtures/mock_provider.mjs` | шаг сценария `{toolCalls:[{name, arguments}]}` — так заказывается вызов `ask_user` без облака |

**Чего в A5.3 не делать:** решения и «Результаты» (A5.4), «Метки проекта» (A5.5), настройки,
уведомления фильтра как отдельная панель и «Очистить историю» (A5.6). Уведомления `redacted`
в ленте показывать **можно** (они уже приходят), но без панели настроек.

## 4. Задачи A5.3 с методикой

### 4.1. SSE-транспорт `GET /api/agent/events` (делать после 4.4/4.3)

**Почему отдельная ветка.** `write_reply_with` всегда пишет `Content-Length` и
`Connection: close` — поток так отдать нельзя. Обычные ответы трогать нельзя: их проверяют
`ui_core`/`ui_auth`.

**Где резать.** В `run_http_listener`, внутри потока-обработчика, **до** `route_full`:

```rust
// псевдокод; p/q/h/b — уже разобранные запрос
if m == "GET" && p == "/api/agent/events" && agent::wants_stream(&h) {
    // 1. те же гейты, что в route_full: в server-профиле без токена — 401/403
    if let Err(reply) = ui_auth_gate(&h, None, &m, &p, &q) { write_reply(&mut stream, reply); return; }
    // 2. вкладка недоступна в серверном профиле (D20) — 403, как у остальных маршрутов
    let cfg = match agent::gate() { Ok(cfg) => cfg, Err(reply) => { write_reply(&mut stream, reply); return; } };
    // 3. поток: заголовки без Content-Length, затем события до отключения клиента
    agent::stream_events(&mut stream, &cfg, &q);
    return;
}
```

**Формат** (одна функция `agent::sse_frame(seq: u64, item: &Value) -> String` — её и тестировать):

```
HTTP/1.1 200 OK
Content-Type: text/event-stream; charset=utf-8
Cache-Control: no-cache
Connection: close
X-Accel-Buffering: no
```
далее
`retry: 1500` → пустая строка → первое событие `event: state` с `data: {"busy":…,"session":…,"seq":…}`
→ на каждое новое: `id: <seq>`, `event: item`, `data: <json одного Item>`, пустая строка;
каждые 15 с без событий — комментарий `: ping`; закрытие — по ошибке записи, по
`agent.sse_max_sec` (300 с) или когда проект выгружен из реестра.

**Клиент (page.rs):** `new EventSource('/api/agent/events?project=P&after=<AG_SEQ>&limit=500')`.
`EventSource` сам переподключается и присылает `Last-Event-ID` — сервер обязан принимать
этот заголовок как `after` (при первом подключении `after` из query). Закрывать поток при
смене проекта, `visibilitychange` (скрыта — закрыть, показана — открыть) и `beforeunload`.
JSON-фолбэк (`?after=` без `Accept: text/event-stream`) **оставить** — им пользуются тесты,
`curl.exe`, e2e и страница, если `EventSource` недоступен.

**Пределы.** `http.max_conns` = 32 на процесс: одна подписка на вкладку, не на проект.
`http.read_timeout_sec` к потоку **не применять** (он про чтение запроса), но поставить
`stream.set_write_timeout(Some(5 с))`, чтобы мёртвый клиент не держал поток вечно.

### 4.2. Подписка на кольцо: без опроса

Кольцо уже есть; нужен «звоночек», чтобы поток отдавал событие сразу, а не через 1,2 с.

```rust
pub struct Turn {
    // … как сейчас (busy, opened, stop, events, session, pump, ask — см. 4.3)
    notify: Arc<(Mutex<u64>, Condvar)>,   // счётчик + будильник
}

// push(): после вставки в кольцо
{ let (lock, cv) = &*self.notify; *lock.lock().unwrap() += 1; cv.notify_all(); }

// stream_events(): ждём нового или 15 с (heartbeat), затем читаем кольцо по `after`
let (lock, cv) = &*turn.notify;
let mut seen = lock.lock().unwrap();
let _ = cv.wait_timeout(&mut seen, Duration::from_secs(15));
```

`events_json(project, after, limit)` остаётся единственным источником данных (тесты и
JSON-фолбэк ничего не заметят). `Condvar` — из `std::sync`, без новых зависимостей.
Дешёвая альтернатива, если `Condvar` окажется неудобен в тестах: опрос 250 мс внутри потока —
но тогда не плодить по потоку на клиента сверх одного.

**«Как видит модель».** У `Item` уже два поля: `text` (деанонимизировано) и `raw` (метки).
Переключатель в ленте — выбор, какое поле рендерить; сервер ничего не меняет. Для
`prompt` (введённая реплика) `raw` не заполняется — показываем `text` и подпись
«исходная реплика».

### 4.3. `ask_user`: вопрос из ленты, ответ из вкладки

`AskUser::ask` вызывается **в потоке чтения воркера** и блокирует его до ответа. Значит:

* реализация не должна держать замки `Filter`/`registry` (только свой `AskState`);
* пока вопрос висит, события этого хода не идут — это норма (модель ждёт);
* `abort`/`stop` обязаны **сначала** снять вопрос, иначе `done` на `abort` не будет прочитан.

```rust
pub struct AskState {
    id: AtomicU64,                       // номер вопроса в ходе
    question: Mutex<Option<(u64, String)>>,
    answer: Mutex<Option<String>>,
    cv: Condvar,
}

pub struct UiAskUser { turn: Arc<Turn>, ask: Arc<AskState> }

impl AskUser for UiAskUser {
    fn ask(&self, question: &str) -> Result<Value, String> {
        let id = self.ask.id.fetch_add(1, SeqCst) + 1;
        *self.ask.question.lock()? = Some((id, question.to_string()));
        self.push_event(&self.turn, "ask", json!({"ask_id": id, "question": question}));
        let deadline = Instant::now() + ask_timeout;          // agent.ask_timeout_sec, дефолт 600
        loop {
            if let Some(text) = self.ask.answer.lock()?.take() { return Ok(json!({"answer": text})); }
            if self.turn.stop.load(SeqCst) { return Err("вопрос отменён (стоп)".into()); }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() { self.push_event(…, "ask_timeout", …); return Err("вопрос без ответа: таймаут".into()); }
            self.ask.cv.wait_timeout(…, left);
        }
    }
}
```

* `start_turn` передаёт `Arc::new(UiAskUser::new(Arc::clone(turn)))` в `GatewayHost::new`
  вместо `Arc::new(NoAskUser)`; в `Turn` хранится `ask: Mutex<Option<Arc<AskState>>>`.
* `POST /api/agent/answer {project, ask_id, answer}` → `AgentApi::answer(cfg, project, ask_id,
  answer)`: 400 без ответа, 404 «нет вопроса», 409 «вопрос устарел» (id не совпал);
  ответ кладётся в `answer`, `cv.notify_all()`. Ответ — **исходный текст пользователя**:
  в модель он уйдёт как результат инструмента и будет заменён фильтром в `egress_check`.
* `abort`/`stop`: `turn.ask.cancel()` (Err в `ask`) **до** `gateway.abort()`/`stop()`; в ленту —
  событие `ask_cancelled`.
* Пустой ответ — 400; ответ обрезать по 20 000 символов (как реплику).

### 4.4. Деанонимизация вопроса — в шлюзе (правка `hds-agent`)

Вопрос формирует модель, значит он приходит **с метками**: `[PERSON_1], сколько дней?`.
Пользователь должен видеть значение, а UI не имеет права разбирать метки (AG-1), поэтому
деанонимизируем в `GatewayHost::handle`:

```rust
HostCall::AskUser { question } => {
    let question = self.refresh.as_ref()
        .and_then(|state| state.deanon.lock().ok().map(|d| d.text(&question).text))
        .unwrap_or(question);
    self.ask.ask(&question)
}
```

`refresh` есть всегда, когда шлюз собран через `run.rs`/`agent_session` (вкладка, CLI);
в egress-only тестах `refresh` может быть `None` — тогда вопрос уходит как есть (метки),
это ожидаемо и документируется. Тест: `hds-agent` с `FakeAsk` (собирает вопросы) и снимком,
где `[PERSON_1]` = «Иванов Иван» → в `ask` приходит «Иванов Иван».

### 4.5. Лента вкладки: разметка и склейка

Заменить `<pre id="agevents">` на `<ul id="agfeed">` + панель вопроса
`<div id="agask" hidden>` (вопрос, `textarea`, «Ответить»).

| Событие (`kind`/`text.kind`) | Как показывать |
|---|---|
| `prompt` | пузырь пользователя, «→ модели:» с метками (это подтверждённый текст) |
| `session` | строка «сессия N (возобновлена)», без пузыря |
| `agent` + `reasoning`/`reasoning_delta` | серый блок «рассуждение» (свёрнут по умолчанию), дельты **дописывать** в текущий блок |
| `agent` + `text`/`text_delta` | пузырь ответа; `text` заменяет накопленные дельты (не дублировать) |
| `agent` + `tool_use` | строка «инструмент `office_read` ← путь» (вход — клип 500 симв.) |
| `agent` + `tool_result` | вывод инструмента свёрнутым блоком (до 20 000 симв., как клип воркера) + «ошибка», если есть |
| `agent` + `usage`/`iteration`/`status`/`agent_done`/`ended` | только в строку статуса (`ход: идёт · итерация 3 · токены …`) |
| `redacted` | предупреждение «фильтр заменил: `[PERSON_1]` (значение)» — метки, без значений |
| `ask` / `ask_timeout` / `ask_cancelled` | панель вопроса / строка «вопрос без ответа» / «вопрос снят» |
| `turn_end`, `done`, `error`, `notice` | итог хода (ok/сбой, причина), ошибки — красным |

Правила:

* **пути:** `text` уже с путями папки проекта (деанонимизация в шлюзе). Ссылка «открыть» —
  это **показ пути + копирование в буфер** (В-A5-7: UI не запускает процессы и не открывает
  файлы); подпись — «путь образа» / «путь проекта» по тому, что пришло;
* **«как видит модель»** — переключатель над лентой: рендерить `raw` вместо `text`; у пузыря
  ответа — пометка «метки»;
* дельты склеивать по `call_id`/`index` (у `text_delta` его нет — склеивать до ближайшего
  `text`/`tool_use`); в DOM держать не больше 500 узлов (как кольцо), старые — «показать ещё»;
* блоки больше 2 000 символов сворачивать (`<details>`), ошибки — не сворачивать;
* `AG_SEQ` обновлять из `id:` каждого кадра (а не из поля `seq` ответа) — так переподключение
  не теряет и не дублирует события.

### 4.6. Что НЕ делать в A5.3

* «Результаты», `pending`, решения, сравнение — A5.4.
* «Метки проекта», склейка/разъединение, `vault_project_labels` — A5.5.
* Панель настроек, ключи, «Очистить историю», уведомления фильтра как отдельный экран — A5.6.
* Правки воркера (`sidecar/hds_agent`) — не нужны: `ask_user` уже есть, виды событий есть.
* Свои регулярки меток/путей в UI — запрещено (AG-1, §5 контракты).

## 5. Порядок работ и критерии приёмки

Порядок: **4.4** (деанон вопроса в шлюзе + тест) → 4.3 (`AskState`/`UiAskUser`,
`POST /api/agent/answer`, отмена) → 4.1 (SSE-ветка и `sse_frame`) → 4.2 (`Condvar`
в кольце) → 4.5 (лента и панель вопроса). Каждый шаг — зелёный тест и коммит.

Готово, когда:

1. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace -- --test-threads=4` зелёные; `npm test` 23/23; python 328 OK.
2. **Тесты A5.3**:
   * unit: `sse_frame` (id/event/data/пустая строка), `wants_stream` (Accept), разбор
     `Last-Event-ID`, `AskState` (ответ по id, чужой id — отказ, таймаут);
   * интеграционный через реальный сокет (`run_http_listener` на порту 0, образец —
     `crates/hds-ui/tests/ui_auth.rs`): подключиться с `Accept: text/event-stream`, получить
     `event: state`, отправить `message`, прочитать `data:` с событием хода, отключиться —
     сервер не падает и не течёт;
   * `ask_user` end-to-end на mock-провайдере: сценарий `{toolCalls:[{name:"ask_user",
     arguments:{question:"Сколько дней? [PERSON_1]"}}]}` → в SSE приходит `ask` с
     **значением** (деанон), `POST /api/agent/answer` → ход продолжается до `turn_end`, в
     логе mock-провайдера ответ пользователя без исходных значений (фильтр сработал);
   * `abort` при висящем вопросе: `ask` получает отказ, `busy` снимается, сессия жива;
   * 403 в server-профиле и без CSRF — и для SSE, и для `/answer`.
3. **E2E стенда** `agent_a42` (`a53_e2e.ps1`, отчёт `A53_E2E.md`):
   * `curl.exe -N` (или PowerShell `StreamReader`) видит `event: state` и поток событий;
   * в ленте: рассуждение, `tool_use`/`tool_result`, ответ, `turn_end`; пути —
     папки проекта, значения настоящие;
   * переключатель «как видит модель» — метки (`[PERSON_…]`), а не значения;
   * вопрос агента → ответ → ход завершён; в `<agent.root>` и журналах значений нет (I8).
4. Ключ провайдера не появляется ни в одном ответе API и ни в одном артефакте стенда.

## 6. Грабли (оплачены в A0–A5.2 — не наступать снова)

1. **`Content-Length` + `Connection: close` в `write_reply_with`.** SSE — отдельная ветка
   записи; обычные ответы не трогать: их проверяют `ui_core`/`ui_auth`.
2. **Поток на соединение, `http.max_conns` = 32.** SSE занимает соединение: одна подписка на
   вкладку, не на каждый запрос и не на каждый проект.
3. **`Gateway::next_event` блокирующий** — только насос (A5.2). Стрим читает **кольцо**, а не шлюз.
4. **SSE идёт мимо `route_full`** ⇒ гейты (`ui_auth_gate`, `agent::gate`) вызвать вручную,
   иначе в server-профиле поток отдал бы данные без 403 (D20).
5. **`AskUser::ask` блокирует поток чтения воркера.** `abort`/`stop` сначала снимают вопрос,
   иначе `done` на `abort` не будет прочитан и насос не опустит `busy`.
6. **Таймаут вопроса ≠ таймаут хода.** По таймауту модели возвращается отказ (tool result),
   ход продолжается; в ленте — `ask_timeout`, а не общий сбой.
7. **Ответ пользователя — это tool result** и уходит модели через `egress_check`: значения в
   нём заменяются метками, поэтому проверять надо лог провайдера, а не только ленту.
8. **Деанонимизация вопроса — в шлюзе** (`GatewayHost`), не в UI (AG-1); в egress-only
   (`refresh == None`) вопрос остаётся с метками — задокументировать.
9. **`EventSource` при первом подключении** не шлёт `Last-Event-ID` — `after` передавать в query;
   на реконнекте сервер обязан `Last-Event-ID` принять.
10. **`curl.exe -N` для SSE** (A4 §6 п.10): `Invoke-WebRequest`/`Invoke-RestMethod` читают поток
    до конца и «висят».
11. **Кириллица в PowerShell 5.1** (A5.2 §6 п.9/15/22): тексты — файлами UTF-8, тело 4xx —
    из `$_.ErrorDetails.Message`, `Invoke-RestMethod` не использовать для JSON.
12. **Кольцо 500 вытесняет начало** при длинном ходе с дельтами: поток обязан успевать читать
    по `after`; `AG_SEQ` брать из `id:` кадра.
13. **`usage`/`iteration`/`status` — не текст модели**: в пузырь ответа не попадают.
14. **Одна подписка — один `EventSource`**: при смене проекта старый закрывать
    (`es.close()`), иначе в server-профиле упрёшься в `max_conns`.
15. **`reset_for_tests`/`reap_idle` не должны рвать поток**: `None` из `get_mut` — это конец
    потока (закрыть с `event: state` и пустым телом), а не паника.

## 7. Окружение и команды

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager log -3 --oneline
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p hds-agent; cargo test -p hds-ui
cargo test --workspace -- --test-threads=4 *> tools\parity\out\a53-cargo-test.log
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test

$env:HDS_CONFIG='<repo>\tools\parity\out\agent_a42\config.yaml'
$env:HDS_ROOT='<repo>\tools\parity\out\agent_a42\root'
target\debug\hds.exe ui --port 8799
# вторая консоль: поток событий (JSON-фолбэк и SSE)
curl.exe -s 'http://127.0.0.1:8799/api/agent/events?project=P0001&after=0'
curl.exe -N -H 'Accept: text/event-stream' 'http://127.0.0.1:8799/api/agent/events?project=P0001&after=0'
Remove-Item Env:HDS_ROOT, Env:HDS_CONFIG -ErrorAction SilentlyContinue   # перед cargo test
```

E2E под-этапа — новый сценарий `tools\parity\out\agent_a42\a53_e2e.ps1` (по образцу
`a52_e2e.ps1`: mock-провайдер из `crates\hds-cli\tests\fixtures\mock_provider.mjs`,
вымышленный ключ, бэкап `projects.json`/`secrets.json`, восстановление в `finally`), отчёт —
`A53_E2E.md` в каталоге стенда. Для шага с вопросом сценарий mock-провайдера:
`[{"toolCalls":[{"name":"ask_user","arguments":{"question":"Сколько дней? [PERSON_1]"}}]},
  {"text":"Принято"}]`

## 8. Вопросы владельцу (предложены дефолты)

| # | Вопрос | Дефолт |
|---|---|---|
| В-A5.3-1 | Транспорт ленты | SSE + JSON-фолбэк (`?after=`), как в A5.2; long-poll не делать |
| В-A5.3-2 | Таймаут вопроса агента | 600 с, ключ `agent.ask_timeout_sec`; по таймауту — отказ модели |
| В-A5.3-3 | «Открыть» в ленте | показ пути + копирование (В-A5-7: без запуска процессов) |
| В-A5.3-4 | Подписок на вкладку | одна; смена проекта закрывает поток, `after` — с 0 |
| В-A5.3-5 | Сколько событий в DOM | 500 (как кольцо), старые за «показать ещё» |

## 9. Срез на 20.11.2026 (для проверки в новом чате)

* Ветка `agent-privacy-zone`, HEAD `144cdd33` (+ docs этого файла), ahead 73, дерево чистое.
* `cargo test --workspace -- --test-threads=4` — 125 наборов / 875 passed / 0 failed /
  13 ignored; python 328 OK (skip 1); `npm test` 23/23; fmt/clippy чисто.
* Стенд `agent_a42`: 34 записи реестра, `A52_E2E.md`, `a52_e2e.ps1`, артефакты `a52-*.json`.
* Вкладка умеет: проекты/состояние/файлы, реплику с предпросмотром, метки, вложения,
  отправку, «Стоп», события опросом (последние 20 строк). Чего нет: SSE, ленты, `ask_user`.

## 10. Промт для нового чата (копировать целиком)

```text
Ты — ведущий разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Это продолжение основного проекта «ИИ-агент в анонимной зоне»
(PLAN_AGENT_PRIVACY_ZONE.md). Ветка agent-privacy-zone, HEAD — docs-коммит A5.2
(сверить git log -1 --oneline), дерево чистое, push не делался.

Фазы A0–A4 закрыты (A4 — демон двустороннего зеркала, отчёт AGENT_A4_REPORT.md).
Фаза A5 (вкладка «Агент») идёт под-этапами: план, решения (В-A5-1…8, В-A5.2-1…6),
долги, контракты и грабли — AGENT_A5_STAGES.md (§1, §2, §3.2, §4–§7).
A5.1 закрыт: вкладка показывает проекты, состояние зеркала и файлы реестра.
A5.2 закрыт: реплика — предпросмотр «что увидит модель», «Скрыть ещё…»,
«Не скрывать», вложения и пути, отправка хода, «Стоп» (crates/hds-ui/src/agent.rs,
crates/hds-ui/src/agent_session.rs, маршруты /api/agent/*).
Контекст и методика этого под-этапа — AGENT_A5_3_HANDOFF_PROMPT.md (§3–§9),
этот промт — её §10.

Задача: под-этап A5.3 — лента событий с SSE и ask_user. Решения — A5.4, метки —
A5.5, настройки и «Очистить историю» — A5.6, живой сценарий §7 — A5.7.

1. Прочитай AGENT_A5_3_HANDOFF_PROMPT.md целиком, затем AGENT_A5_STAGES.md
   (§2, §3.2, §4.1, §4.2, §5–§7) и PLAN_AGENT_PRIVACY_ZONE.md §5.5 (TR-5.1…5.3),
   §4.3 (канал hds ↔ агент, ask_user), §5.8 (TR-8.2).
2. Деанонимизация вопроса агента — в шлюзе: GatewayHost::handle, ветка
   HostCall::AskUser деанонимизирует question через refresh.deanon (UI метки не
   разбирает, AG-1); в egress-only (refresh == None) вопрос остаётся с метками —
   задокументировать. Тест: FakeAsk собирает вопросы, снимок [PERSON_1] →
   «Иванов Иван».
3. ask_user в вкладке: AskState {id, question, answer, cv} + UiAskUser (AskUser),
   событие ask/ask_timeout/ask_cancelled в кольцо, POST /api/agent/answer
   {project, ask_id, answer} (400/404/409), agent.ask_timeout_sec (дефолт 600);
   abort/stop сначала снимают вопрос (иначе поток чтения воркера заблокирован
   в ask и done на abort не прочитается), затем gateway.abort()/stop().
4. SSE: в run_http_listener до route_full — ветка GET /api/agent/events при
   Accept: text/event-stream; те же гейты (ui_auth_gate, agent::gate — 403 в
   server-профиле, D20); заголовки без Content-Length, retry: 1500, первое
   event: state {busy, session, seq}, затем id: <seq> / event: item / data: <json>,
   heartbeat «: ping» каждые 15 с, закрытие по ошибке записи или 300 с;
   Last-Event-ID как after; JSON-фолбэк (?after=) сохранить для тестов/curl/e2e.
5. Подписка на кольцо: Condvar в Turn (счётчик + будильник из push), поток ждёт
   до 15 с и читает events_json(project, after, 500) — насос и кольцо не менять.
6. Лента вкладки: вместо <pre id="agevents"> — <ul id="agfeed"> с блоками:
   рассуждение (reasoning/reasoning_delta), ответ (text/text_delta — дельты
   склеивать, text заменяет накопленное), tool_use/tool_result (вывод до 20 000
   символов, свёрнутый), redacted — предупреждение метками, usage/iteration/
   status/agent_done/ended — в строку статуса, turn_end/done/error — итог;
   переключатель «как видит модель» (raw вместо text); «открыть» = показ пути +
   копирование (В-A5-7, без запуска процессов); событий в DOM ≤ 500; AG_SEQ из
   id: кадра. EventSource закрывать при смене проекта/скрытии вкладки.
7. Не делать: решения и «Результаты» (A5.4), «Метки проекта» (A5.5), настройки,
   «Очистить историю» (A5.6), правки sidecar/hds_agent.
8. Тесты: unit (sse_frame, wants_stream/Accept, Last-Event-ID, AskState —
   ответ по id, чужой id, таймаут); интеграционные через реальный сокет
   (run_http_listener на порту 0, образец crates/hds-ui/tests/ui_auth.rs):
   SSE отдаёт event: state и события хода; ask_user на mock-провайдере
   ({toolCalls:[{name:"ask_user", arguments:{question:"… [PERSON_1]"}}]}) —
   вопрос с значением, ответ через /answer, ход до turn_end, в логе провайдера
   ответ без исходных значений; abort при висящем вопросе; 403 в server-профиле
   и без CSRF для SSE и /answer. Проверки: cargo fmt --all --check,
   clippy --workspace --all-targets -- -D warnings, cargo test --workspace --
   --test-threads=4, npm test, python unittest.
9. E2E стенда agent_a42 (mock-провайдер): a53_e2e.ps1 — curl.exe -N видит
   event: state и поток; лента: рассуждение, tool_use/tool_result, ответ,
   turn_end; пути — папки проекта; «как видит модель» — метки; вопрос агента →
   ответ → ход завершён; значений в артефактах нет; отчёт A53_E2E.md.
10. Коммиты: feat(agent): A5.3 — ...; docs(agent): A5.3 закрыт — ...
    (+ AGENT_A5_STAGES.md §2, §3.3, §4.1, §4.2, §6, §7; README §20 — про ленту
    и вопрос агента). В сообщении — что проверено.

Проверка среза: ветка agent-privacy-zone, HEAD 144cdd33 (+docs этого файла),
ahead 73, дерево чистое; cargo test --workspace — 125 наборов / 875 passed /
0 failed / 13 ignored; python 328 OK (skip 1); npm test 23/23; стенд agent_a42 —
34 записи реестра, отчёт A52_E2E.md, артефакты a52-*.json.
```





