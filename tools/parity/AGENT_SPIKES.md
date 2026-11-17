# A0 — спайки плана «ИИ-агент в анонимной зоне»: результаты

> Отчёт фазы **A0** плана `PLAN_AGENT_PRIVACY_ZONE.md` (редакция 3.1).
> Продуктовый код (`crates/`, `sidecar/`, `installers/`) в этой фазе **не менялся**.

## 1. Шапка

| Параметр | Значение |
|---|---|
| Дата | 17.11.2026 |
| Ветка | `agent-privacy-zone` (от `anonymizer-sidecar`, HEAD `43d3c98`) |
| Машина | Windows 11, PowerShell 5.1, Node **24.18.0**, npm 11.16.0, GPU NVIDIA RTX 3060 12 ГБ |
| `@cline/sdk` | **0.2.0** (план писал 0.0.90 — API отличается, см. §4) |
| `@cline/core` / `@cline/shared` / `@cline/llms` | 0.2.0 (ставятся транзитивно) |
| `tavily-mcp` | **0.2.22** (закреплён, не `npx @latest`) |
| `undici` | 8.12.4 (поставлен, но не понадобился: опция `fetch` принимается напрямую) |
| venv анонимизации | `sidecar/venv-anonymize`, Python 3.12.14, `onnxruntime-gpu` 1.24.4 (CUDA), torch 2.13.0, natasha 1.6.0, pymorphy2 0.9.1 |
| Модели анонимизации | `C:\ProgramData\hermes-disk-search\models\anonymize` (GLiNER PII, navec, slovnet) |
| Артефакты спайков | `tools/parity/out/agent_a0/` (не в git) |

## 2. Сводная таблица S1–S10

| Пункт §6 плана | Вопрос | Ответ | Ключевая цифра | Раздел |
|---|---|---|---|---|
| (1) S1 | SDK в `backendMode: local` с подменёнными `HOME`/`USERPROFILE`/`APPDATA` не трогает `~/.cline`, не поднимает hub, не ходит в сеть | **да** | 0 обращений к реальному профилю, 0 внешних соединений, снимок профиля до/после — 0 изменений | §3.1 |
| (2) S2 | все HTTP-запросы провайдеров идут через переданный `fetch` | **да** | 9 запросов на mock = 9 вызовов `fetch`; 3 провайдера; повторы после 429 — тоже через `fetch`; при отказе `fetch` — 0 запросов | §3.2 |
| (3) S3 | `beforeTool` блокирует/подменяет input (встроенные и MCP), `afterTool` подменяет result, fail-closed | **да** | 7 прогонов: skip/input/result/MCP — работают; исключение в хуке → прогон падает (fail-closed); таймаута 3 с нет | §3.3 |
| (4) S4 | `run_commands` и `fetch_web_content` отключаются политикой и не попадают в `tools` | **да** | 5 → 3 инструмента в теле запроса к модели; `'*': {enabled:false}` работает | §3.4 |
| (5) S5 | суб-агенты наследуют `fetch`, хуки, cwd, MCP; события видны | **да** | 4/4 запроса через `fetch`, хук сработал на инструменте суб-агента | §3.5 |
| (6) S6 | формат истории в `agent-state`; можно переписать и возобновить | **частично** | формат снят; `start({config.sessionId})` историю **не** грузит — нужен `initialMessages`; `sessions.db` текстов не содержит | §3.6 |
| (7) S7 | MCP (`1c-buddy`, `tavily`, `disk-search`) подключается из ядра | **частично** | tavily 5 инструментов + реальный вызов; 1c-buddy 8 инструментов + реальный вызов; disk-search: транспорт отвечает, но токен из настроек владельца → **401** | §3.7 |
| (8) S8 | скорость `anonymize_text` на GPU/CPU | **да** (CPU — на грани) | GPU: 0,34–0,39 с на 2 000 символов; CPU: 4,76–4,9 с (цель ≤ 5 с); warmup 11–30 с | §3.8 |
| (9) S9 | морфология: `norm_key`, падежи, ложные склейки | **частично** | `norm_key` 90,9 %, генерация падежей 98,5 %, однозначное определение падежа 19,7 % | §3.9 |
| (10) S10 | стенд и эталон; `recall@k` поиска по анонимным образам | **да** | `recall@10 = 1,0`, MRR 0,906, среднее 1,52 с/запрос; поиск по метке `[PERSON_3]` — 5 из 10 выдач «в тему» | §3.10 |

**Стоп-точки (1) и (2) пройдены: оба ответа «да».** Архитектура D1/D13 (headless-ядро SDK + фильтр в `fetch`, без прокси) подтверждена экспериментом.

## 3. Спайки подробно

Общий стенд S1–S7 — Node-проект `tools/parity/agent_a0_sdk/` (исходники и `package-lock.json` в git;
рабочая копия с `node_modules` — `tools/parity/out/agent_a0/sdk/`). Mock-провайдер
(`mock_provider.mjs`) отвечает в формате OpenAI Chat Completions и Anthropic Messages (стрим и JSON),
поддерживает сценарии ошибок (`{"status": 429, "retryAfter": 1}`) и пишет журнал всех запросов.

### 3.1. S1 — изоляция конфигурации (стоп-точка) — **ДА**

**Что сделано.** `s1_isolation.mjs` + `fs_trace.mjs` (обёртка `fs`/`net` через `node --import`) +
`s1_run.ps1` (снимки `%USERPROFILE%\.cline` и каталога расширения Cline до/после, опрос
`Win32_Process` и `Get-NetTCPConnection` каждые 250 мс).

**Наблюдения.**
* `ClineCore.create({ backendMode: "local" })` при подменённых `USERPROFILE`/`HOME`/`APPDATA`/`LOCALAPPDATA`/`TEMP`:
  * `resolveDefaultMcpSettingsPath()` → `<sandbox>\home\.cline\data\settings\cline_mcp_settings.json` — **внутри** песочницы;
  * сессии и SQLite-база создаются в `<sandbox>\home\.cline\data\...` (см. §3.6);
  * ни одного обращения к реальному `C:\Users\Sasha\.cline` или `%APPDATA%` — 0 в журнале трассировки;
  * ни одного внешнего соединения: единственный TCP — `127.0.0.1:<port>` mock-провайдера;
  * телеметрия/feature-flags: по умолчанию no-op (в `ClineCoreOptions.telemetry`/`featureFlags`
    не переданы) — сетевых обращений не было, включая `models.dev` (каталог моделей при локальном
    runtime не запрашивался).
* Дочерние процессы: `node.exe` (наш mock), `cmd.exe`/`conhost.exe`, `git.exe`. Hub-демона нет.
  `git.exe` SDK запускает сам: собирает `url`/`branch` репозитория в метаданные сессии (в проде cwd —
  `workspace` без `.git`, вызов будет безвредным, но его надо учесть в замере старта).
* Снимок профиля вокруг прогона (`s1/profile-before.json` → `s1/profile-after.json`): **0 добавленных,
  0 удалённых, 0 изменённых** файлов в `~/.cline` и в каталоге расширения.

**Цифры.** fs-событий в трассировке 1 491 (1 489 — код SDK и зависимости в `node_modules`, 2 — песочница);
внешних соединений 0; обращений к реальному профилю 0; снимков профиля 2, различий 0.

**Вывод.** D15 (переопределение `HOME`/`USERPROFILE` = изоляция) работает. Агент не увидит ни MCP,
ни правила, ни ключи программирующего Cline. Hub не поднимается — `backendMode: "local"` достаточен.

**Ограничения методики.** Process Monitor на машине не установлен, поэтому чтения ловятся только
частично (обёртка `fs` видит не все вызовы ESM-импортов ядра). Доказательство «нет утечки» держится на
трёх независимых сигналах: (а) снимки профиля без изменений; (б) `cline_mcp_settings.json` резолвится
внутрь песочницы; (в) в запросе к модели нет ни одного MCP-инструмента владельца (только 5 встроенных).

### 3.2. S2 — все запросы провайдеров через наш `fetch` (стоп-точка) — **ДА**

**Что сделано.** `s2_fetch.mjs`: 8 прогонов — `openrouter`, `openai-compatible` с произвольным
`baseUrl`, `anthropic`; отдельно повторы после 429/500 и «fail-closed» (наш `fetch` бросает исключение).

| Случай | mock получил | наш `fetch` вызван | Совпало |
|---|---|---|---|
| openrouter (`baseUrl = mock/v1`) | 1 | 1 | да |
| openai-compatible (`baseUrl = mock/v1`) | 1 | 1 | да |
| anthropic (`baseUrl = mock`, путь `/messages`) | 1 | 1 | да |
| openrouter, первый ответ 429 | 1 | 1 | да |
| openai-compatible, первый ответ 500 | 1 | 1 | да |
| anthropic, первый ответ 500 | 1 | 1 | да |
| openrouter, три 429 подряд (retry-after 1 с) | 3 (интервал ≈1,03 с) | 3 | да |
| openrouter, `fetch` бросает исключение (fail-closed) | **0** | 1 | да |

**Вывод.** Провайдеры не имеют «второго» HTTP-клиента: повторы провайдера тоже идут через наш `fetch`,
а при отказе фильтра запрос не уходит вовсе (это и есть механика TR-6.5).

**Как задаётся `baseUrl` в 0.2.0** (важно для A3): `config.baseUrl` — для `openrouter` и
`openai-compatible` это база **с** `/v1` (SDK зовёт `<baseUrl>/chat/completions`), для `anthropic` —
база **без** `/v1` (SDK зовёт `<baseUrl>/messages`).

**Побочное наблюдение (риск).** Если провайдер ответил 429/500 и повторы исчерпаны, прогон всё равно
завершается событием `done` с `reason: "completed"` и **пустым** текстом. В A3 нельзя считать успехом
только `ended.reason === "completed"` — надо проверять текст/usage (см. §5).


### 3.3. S3 — хуки `beforeTool`/`afterTool` и fail-closed — **ДА**

**Что сделано.** `s3_hooks.mjs`: 7 прогонов против mock-провайдера + прототип проверки путей
(`safeResolve`, будущий `hds-guard`). Модель «просит» `read_files`, по второму запросу видно, что
именно получил агент.

| Случай | Что вернул хук | Что увидела модель |
|---|---|---|
| baseline | — | содержимое `allowed.txt` (чтение выполнено) |
| путь вне workspace (`..\..\outside\secret.txt`) | `{skip: true, reason: "hds-guard: outside workspace"}` | `{"error":"hds-guard: outside workspace"}`, файл не прочитан |
| подмена input (`missing.txt` → `allowed.txt`) | `{input: {files: [{path: "allowed.txt"}]}}` | содержимое `allowed.txt` |
| подмена результата | `afterTool → {result: {output: {replaced:true, text:"REDACTED-BY-AFTERTOOL"}}}` | `REDACTED-BY-AFTERTOOL` |
| хук бросает исключение, `hookErrorMode: "ignore"` | throw | прогон **упал**, инструмент не выполнен |
| хук бросает исключение, `hookErrorMode: "throw"` | throw | прогон **упал**, инструмент не выполнен |
| хук спит 5 с | — | инструмент выполнен после 5 с: таймаута хука **нет** |

**Матрица путей (`safeResolve`, прототип TR-3.4).** Все запрещённые варианты отклонены:
`..\..`, абсолютный путь вне workspace, junction внутри workspace наружу, UNC `\\server\share`,
`\\?\C:\...`, альтернативный поток `allowed.txt:hidden`, имя устройства `NUL`. Абсолютный путь
**внутри** workspace разрешён.

**Выводы.**
1. Отказ инструмента доходит до модели текстом причины — это работает как «обучение модели»
   (TR-7.3a опирается на это).
2. Fail-closed есть по факту: исключение в хуке не даёт инструменту выполниться. Но цена —
   падение всего прогона (в обоих режимах `hookErrorMode`), поэтому хук должен быть безотказным.
3. **Таймаута хука 3 с в 0.2.0 нет** (хук на 5 с прошёл). Если guard может зависнуть — таймаут
   надо делать самим внутри хука.
4. `read_files` возвращает ошибку как `{"error": ...}` без обёртки `files` — при разборе в UI это надо
   учитывать.

**Находка, влияющая на архитектуру.** Встроенные файловые инструменты резолвят **относительные**
пути от `cwd процесса`, а не от `config.cwd`: при запуске из каталога SDK `read_files` искал
`...\agent_a0\sdk\allowed.txt` вместо `...\workspace\allowed.txt`. Для `hds_agent` это значит:
процесс обязательно запускать с рабочим каталогом `<P>\workspace` (правка плана — §3.11, П-3).

### 3.4. S4 — отключение `run_commands` и `fetch_web_content` — **ДА**

| Прогон | Политика | Инструменты в теле запроса к модели |
|---|---|---|
| baseline | — | `read_files, search_codebase, run_commands, fetch_web_content, editor` (5) |
| без терминала и сети | `run_commands: {enabled:false}`, `fetch_web_content: {enabled:false}` | `read_files, search_codebase, editor` (3) |
| без чтения и редактора | `read_files/editor: {enabled:false}` | `search_codebase, run_commands, fetch_web_content` (3) |
| только редактор | `'*': {enabled:false}`, `editor: {enabled:true}` | `editor` (1) |
| попытка включить `apply_patch`, `ask_question`, `submit_and_exit` | `{enabled:true}` | те же 5 — **включить нельзя** |
| `mode: "plan"` | — | `read_files, search_codebase, run_commands, fetch_web_content` (без `editor`) |

**Вывод.** `toolPolicies` убирает инструмент из списка `tools` (не «требует подтверждения»), маска
`'*'` работает. Набор инструментов задаётся пресетом режима; `apply_patch`, `ask_question` и
`submit_and_exit` в headless-наборе недоступны (у `ask_question` нет интерактивных capabilities).
Правка плана — §3.11, П-4.

### 3.5. S5 — суб-агенты — **ДА** (можно включать)

`enableSpawnAgent: true`, модель вызывает `spawn_agent {systemPrompt, task}`, суб-агент читает файл.

* 4 запроса к модели — 4 вызова нашего `fetch` (включая запросы суб-агента);
* system prompt суб-агента — свой (`«ты субагент»`), у родителя — свой;
* `beforeTool` вызван для инструментов суб-агента (`read_files`), подмена/блокировка действует;
* суб-агент видит тот же `cwd` (прочитал `allowed.txt`);
* события суб-агента приходят в тот же `subscribe` (вложенные `iteration_start`/`content_*`);
* `toolPolicies`/`hookErrorMode` передаются в суб-агента (видно и в коде ядра:
  `Zs({.. hookErrorMode: e.hookErrorMode, toolPolicies: e.toolPolicies ..})`).

**Вывод.** Условие D17/TR-3.7 выполнено — суб-агентов можно разрешать настройкой (по умолчанию всё
равно оставить `false` до A3, как в плане).


### 3.6. S6 — формат истории и «Очистить историю» (TR-6.6) — **ЧАСТИЧНО** (механизм найден)

**Где лежит история** (всё внутри подменённого `HOME`):

```
<HOME>/.cline/data/db/sessions.db                       SQLite: ТОЛЬКО метаданные сессий
<HOME>/.cline/data/logs/hooks.jsonl                      журнал хуков
<HOME>/.cline/data/sessions/<id>/<id>.json               манифест сессии (snake_case)
<HOME>/.cline/data/sessions/<id>/<id>.messages.json      сообщения (вот здесь текст!)
```

**Формат `<id>.messages.json`** (снят дословно):

```json
{
  "version": 1,
  "updated_at": "2026-11-17T14:39:18.359Z",
  "agent": "lead",
  "sessionId": "1794926357710_73r8c",
  "origin": { "source": "core", "mode": "user", "sessionId": "..." },
  "messages": [
    { "id": "uuid|msg_xxx", "role": "user", "content": [{ "type": "text", "text": "<user_input mode=\"act\">…</user_input>" }], "ts": 1794926358253 },
    { "role": "assistant", "content": [{ "type": "tool_use", "id": "call_1_0", "name": "read_files", "input": {…} }], "modelInfo": {…}, "metrics": {…} },
    { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "call_1_0", "name": "read_files", "content": [ … ] }] }
  ],
  "system_prompt": "…"
}
```

**Что проверено.**
1. Маркер `Иванов` есть в `.messages.json` и в манифесте (4 вхождения в каждом), в `sessions.db` —
   **нет** (там только метаданные). Значит «чистить» достаточно файлы сессии.
2. Перезапись при остановленной сессии работает: `str.replace` + атомарная замена файла; после
   перезаписи маркера в файле нет, метки `[PERSON_1]` на месте.
3. `start({ config: { sessionId: <прежний id> } })` **историю не загружает**: модель видит 2 сообщения
   (system + новая реплика), id при этом сохраняется.
4. `start({ config: {…, sessionId: <прежний id>}, initialMessages })` — рабочий вариант: модель видит
   4 сообщения, в первом запросе уже `[PERSON_1]`, маркера нет.

**Вывод для TR-6.6.** «Очистить историю» = остановить сессию → прогнать `<id>.messages.json` и
`<id>.json` через словарь (значения → метки) → запустить `start` с **тем же** `sessionId` и
`initialMessages` из перезаписанного массива. `sessionId`-only resume не годится.

**Ограничение.** Бэкап-копий истории SDK не делает; перезапись — наша ответственность (в A3: сначала
копия файла, потом замена, как в `_atomic_write_*` у Python-воркера).

### 3.7. S7 — MCP из ядра — **ЧАСТИЧНО**

**Как задаётся список MCP (0.2.0).** Программной опции «вот серверы» в `ClineCore.create`/`start` нет:
ядро читает **файл настроек** по пути `resolveDefaultMcpSettingsPath()` =
`<HOME>/.cline/data/settings/cline_mcp_settings.json`. Это ровно то, что делает D15: файл лежит в
`agent-config` и пишется hds.

Формат (нативный для SDK; классический Cline-формат `{type, url, headers}` тоже принимается):

```json
{ "mcpServers": {
  "echo": { "transport": { "type": "stdio", "command": "…\\node.exe", "args": ["…\\mcp_echo_server.mjs"], "env": {"K":"V"} },
            "disabled": false, "timeoutSeconds": 30 },
  "disk-search": { "transport": { "type": "streamableHttp", "url": "http://127.0.0.1:8787/mcp", "headers": { "Authorization": "Bearer …" } } }
} }
```

**Имя инструмента в запросе к модели: `<server>__<tool>`** (проверено: `echo__echo`, `1c-buddy__ask_1c_ai`).

| Сервер | Инструментов | Реальный вызов | Замечания |
|---|---|---|---|
| собственный echo (stdio) | 3 | `ECHO:…` вернулся | `beforeTool` подменил input, `skip` заблокировал с причиной |
| `tavily` (0.2.22 из `node_modules`) | 5 | `tavily_search` по «1С:ERP 2.5 управление холдингом» — ответ получен | ключ передан только env процесса |
| `1c-buddy` (`…\.venv\Scripts\1c-buddy-mcp.exe --env-file …\.env`) | 8 | `ask_1c_ai` с обезличенным вопросом — ответ получен | старт ~5–15 с |
| `disk-search` (`http://127.0.0.1:8787/mcp`, Bearer из настроек владельца) | 0 | — | транспорт отвечает, но **401** `{"error":"unauthorized","hint":"POST /auth/login"}` |

**Запуск stdio-сервера: command не квотируется.** `command: "C:\Program Files\nodejs\node.exe"`
разваливается (`Process exited code=1`, stderr «C:\Program не является…»). Работают три варианта:
копия `node.exe` в путь без пробелов, `command` в кавычках, `command: "node"` (разрешается по PATH).

**Секреты.** Bearer-токен `disk-search` попадает в открытом виде в `cline_mcp_settings.json` внутри
`agent-config`. Для A3 нужно либо передавать секрет через `transport.env` (и подставлять в заголовок
на стороне SDK нельзя) — либо ставить на файл ACL как у `secrets.json`. **Требуется решение владельца**
(вопрос В-1, §5).

**Вывод.** Ядро подключает MCP так, как нужно плану (файл в `agent-config`, хуки и политики действуют
на MCP-инструменты). Не проверено end-to-end только `disk-search`: текущий токен владельца не
принимается (сервер требует `POST /auth/login`), в A2 будет собственный токен маршрута `/mcp/agent/<P>`.


### 3.8. S8 — скорость анонимизации (прототип `anonymize_text`) — **ДА** (CPU на грани)

**Методика.** Долгоживущий воркер `hds_anonymize` (полный NER), временная база в
`out/agent_a0/s8/<label>/anonymize.db`, файлы `.txt` 500/2 000/5 000 символов (вымышленный протокол
совещания 1С с ФИО, организациями, ИНН, телефонами, суммами). Первый акт каждой серии — отдельно,
warmup — отдельно. Каждый акт получает уникальный хвост: обработчик кэширует результат по содержимому,
и повтор одного и того же текста даёт ложные 3–4 мс.

| Размер | GPU (RTX 3060, аренда `anon`) | CPU |
|---|---|---|
| 500 символов | 78–102 мс | 1 007–1 015 мс |
| 2 000 символов | 338–372 мс | 4 752–4 906 мс |
| 5 000 символов | 966–989 мс | 12 490–12 828 мс |
| warmup (загрузка моделей) | 11,5 с (тёплый кэш ОС) / 30,3 с (холодный) | 10,5 с |

Цели плана (TR-8.4): ≤ 2 с на 2 000 символов GPU — **выполнено с 5-кратным запасом**;
≤ 5 с CPU — **на грани** (4,75–4,91 с, запас 2–5 %).

**Предложения (правки плана — §3.11, П-11).**
1. CPU: ограничить длину реплики 1 500 символами (тогда ≈ 3,6 с) либо ставить акты в очередь с
   прогрессом; «≤ 5 с на CPU» на более слабом ноутбуке не выполнится.
2. Убрать кэш по содержимому из замера производительности (не из продукта): в A1 для `anonymize_text`
   кэш допустим (реплика может повториться), но метрики снимать на уникальных текстах.
3. Warmup 11–30 с — учитывать в TR-3.9: вкладка открывает проект до первой реплики, воркер надо
   прогревать заранее (это уже в плане).

### 3.9. S9 — морфология (TR-2.1…2.4) — **ЧАСТИЧНО**

**Что сделано.** `morph_spike.py` (venv-anonymize): 8 персон (мужские/женские/несклоняемые/дефисная/
прилагательные/с инициалами) и 3 однословных ORG/LOC, у каждого — ожидаемые формы во всех шести падежах.

| Проверка | Результат | Комментарий |
|---|---|---|
| (а) `norm_key` одинаков для всех падежных форм одной сущности | **10 из 11 (90,9 %)** | падение на «Толстой Лев Николаевич»: прилагательная фамилия в косвенных падежах лемматизируется в «толстый», в именительном — «толстой» |
| (б) генерация всех падежей | **65 из 66 (98,5 %)** | единственная ошибка: «Орёл» (город) в винительном — «Орла» (pymorphy считает одушевлённым) |
| (в) ложные склейки | **0** | «Иванов» ↔ «Иванова», «Иванов» ↔ «Иванов И. И.», «Белый»(человек) ↔ «Орёл»(город) — не склеиваются |
| (в′) обратная проблема — дубли одной сущности | 1 | «Северный ветер» и «ООО «Северный ветер»» дают **разные** `norm_key` (текст в кавычках по TR-2.1 не лемматизируется) → нужны правила нормализации ORG или «Объединить метки» (TR-2.6) |
| (г) определение падежа исходной формы | 41 из 66 (62,1 %); **строго однозначно — 13 из 66 (19,7 %)** | `accs`/`gent` у одушевлённых неразличимы, несклоняемые (Шевченко, Дюма) всегда «nomn», «Тверь» datv/loct → gent |

**Выводы.**
1. Генерация падежей пригодна (98,5 %) — суффикс `|дат` можно ставить, но только при **однозначном**
   определении падежа; иначе — без суффикса и пометка «проверить» (это и есть TR-2.4).
2. 19,7 % — оценка «в лоб» по одному слову. Контекст (ФИО целиком + известный род + согласование
   отчества) поднимет её, но это работа A1; в MVP поведение по умолчанию — «нет однозначности → нет
   суффикса».
3. `norm_key` требует двух правил, которых в TR-2.1 нет: (а) прилагательные фамилии приводить к
   мужскому именительному; (б) дефисные фамилии `NamesExtractor` режет на два совпадения — склеивать
   по дефису. Плюс: `NamesExtractor` отдаёт фамилию **в исходной форме** («Иванову»), лемматизировать
   надо отдельно, а инициалы приходят без точки («И») — приводить к «и.».
4. Побочная находка для D19: NER на стенде пометил как ORG `ТЗ`, `ЗУП`, `1С`, `Документооборот`,
   `ДиректБанк`, `ГИС МТ`, «банком/банка», коды `REQ-001…REQ-020` — как `INN`/`PASSPORT`, почту —
   именем человека. `keep_terms` и настройка regex-категорий обязательны, иначе агент теряет предмет
   анализа (подтверждение D19 фактом).


### 3.10. S10 — стенд, эталон, `recall@k` поиска по анонимным образам — **ДА**

**Стенд.** `tools/parity/agent_a0_gen_project.py` (в git) генерирует **32 документа** в 6 подпапках
(`01 Устав`, `02 Обследование`, `03 ТЗ`, `04 Протоколы`, `05 План-график`, `06 Переписка`): 12 docx,
2 xlsx (реестр требований на 20 позиций + план-график), 1 pdf (текстовый, PyMuPDF), 10 md-писем.
Все ФИО/организации/ИНН/телефоны/суммы вымышлены; ФИО — в 5 падежах и в виде инициалов; заказчик
встречается в трёх написаниях; термины 1С (`1С:ERP`, `ЗУП`, `Документооборот`) присутствуют.

**Анонимные образы.** `s10_anonymize.py`: 32 файла через воркер (полный NER, CUDA), `.pdf` → текст
PyMuPDF → `<имя>.pdf.md` (прототип D18). Временная база `s10/anonymize.db` (боевая не тронута),
0 ошибок, 32/32 файла; ~70 меток, из них 14 `PERSON`, 9 `SURNAME`, 21 `ORG`, 4 `MONEY`, 13 `INN`,
2 `LOC`.

**Индекс.** Отдельная база `s10/index.db`, временный `s10/config.yaml` (копия боевого с заменой
`index.roots` на `s10/anon`, своей `db_path` и `ocr: false`), `HDS_CONFIG=…\s10\config.yaml`,
`hds.exe index`: 32 файла, 0 ошибок, 10 с.

**Эталон.** `s10/golden.jsonl` — 16 вопросов: 5 «факт», 5 «по смыслу», 2 «сверка», 2 «противоречие»,
2 «по метке», у каждого — список файлов-источников.

**Замер** (`tools/parity/agent_a0_rag_eval.py`, `hds search <вопрос> --limit 30 --json`):

| Метрика | Значение |
|---|---|
| `recall@1` | 0,812 |
| `recall@5` | **1,000** |
| `recall@10` | **1,000** (ориентир плана ≥ 0,7 — выполнен) |
| `recall@30` | 1,000 |
| MRR | 0,906 |
| среднее время запроса | 1,52 с |
| по типам (`recall@10` / MRR) | факт 1,0 / 0,90; смысл 1,0 / 0,90; сверка 1,0 / 0,75; противоречие 1,0 / 1,0; метка 1,0 / 1,0 |

**Поиск по метке (факт для TR-4.6).** Запрос `[PERSON_3]` вернул 10 фрагментов, из них **5** содержат
именно эту метку; `[ORG_21]` — **1 из 10**. Стандартный токенизатор FTS5 действительно режет метку и
подмешивает посторонние фрагменты. Нормализация метки в единый токен (TR-4.6) **нужна**; после неё
замер надо повторить в A2.

**Вывод.** Поиск по анонимным образам работает: метки вместо имён не ломают ни FTS, ни векторный
поиск, ни переранжирование. Стенд и эталон переиспользуются в контрольной точке K1.


### 3.11. Предлагаемые правки плана (в `PLAN_AGENT_PRIVACY_ZONE.md` не вносились)

| # | Требование / раздел | Было | Стало (формулировка для владельца) |
|---|---|---|---|
| П-1 | §6 A0(1), риск «SDK 0.0.x» | `@cline/sdk` 0.0.90 | `@cline/sdk` **0.2.0**: API другой (`start({config})`, события `agent_event`/`ended`, MCP через файл настроек). Закрепить 0.2.0 в `sidecar/hds_agent/package.json` |
| П-2 | TR-2.1 | «`PERSON` — разбор Natasha `NamesExtractor` → фамилия\|имя\|отчество в именительном падеже» | добавить: «`NamesExtractor` возвращает фамилию в исходной форме — лемматизировать отдельно; инициалы приходят без точки — приводить к `и.`; дефисную фамилию он режет на два совпадения — склеивать по дефису; прилагательные фамилии («Толстой») приводить к мужскому именительному, иначе `norm_key` расходится (спайк: 10/11)» |
| П-3 | TR-3.2 / TR-3.4 | «`cwd` — рабочая папка агента» | добавить: «процесс `hds_agent` запускается с рабочим каталогом `<P>\workspace`: встроенные файловые инструменты 0.2.0 резолвят относительные пути от `cwd` процесса, а не от `config.cwd`»; в TR-3.10 добавить пункт «(ж) относительный путь в `read_files` попадает в workspace» |
| П-4 | TR-3.4 | включены `read_files`, `search_codebase`, `editor`, `apply_patch`, `ask_question`, `submit_and_exit` | «в headless-наборе 0.2.0 доступны `read_files`, `search_codebase`, `run_commands`, `fetch_web_content`, `editor`; `apply_patch`, `ask_question`, `submit_and_exit` недоступны и политикой не включаются; вопросы пользователю — только через собственный инструмент на `host_call ask_user`» |
| П-5 | TR-3.4 (fail-closed) | «точное значение `hookErrorMode` — по итогам A0» | «`hookErrorMode: "throw"`; исключение в хуке блокирует вызов, но **валит весь прогон** (в `ignore` тоже) — хук обязан быть безотказным и с собственным таймаутом: встроенного таймаута хука в 0.2.0 **нет**» |
| П-6 | TR-3.6 | «hds сам пишет список MCP в `<P>\agent-config`» | уточнить: «файл `<P>\agent-config\.cline\data\settings\cline_mcp_settings.json`, формат `{"mcpServers": {name: {transport: {type, command, args, env} или {type: "streamableHttp", url, headers}}, timeoutSeconds}}`; имя инструмента для модели — `<server>__<tool>`; `command` с пробелом в пути не экранируется — путь без пробелов или кавычки» |
| П-7 | TR-3.6 / TR-8.1 | «ключи и токены в `config.yaml` не пишутся — только в `secrets.json`» | добавить: «токен MCP-сервера не должен попадать в `agent-config` в открытом виде: либо ACL на файл настроек как у `secrets.json`, либо подстановка из env при старте (решение — В-1)» |
| П-8 | TR-6.6 | «формат файлов истории уточняется в A0» | «история — `<P>\agent-state\.cline\data\sessions\<id>\<id>.messages.json` (JSON, `messages[]` с `content[]`-частями) + `<id>.json`; `sessions.db` текстов не содержит; «Очистить историю» = перезаписать оба файла словарём и запустить `start` с **тем же** `sessionId` и `initialMessages` (resume по одному `sessionId` историю не загружает)» |
| П-9 | TR-3.3 | «провайдер: `baseUrl`» | уточнить: «`openrouter` и `openai-compatible` — `baseUrl` **с** `/v1`; `anthropic` — **без** `/v1` (SDK зовёт `/messages`); все три идут через переданный `fetch`, включая повторы» |
| П-10 | TR-3.7 / D17 | «включаются, если A0 подтвердит» | «подтверждено: суб-агенты наследуют `fetch`, хуки, `toolPolicies`, `hookErrorMode`, `cwd`; их события видны в `subscribe`. Включать настройкой `agent.subagents` (по умолчанию `false`)» |
| П-11 | TR-8.4 | «≤ 2 с GPU, ≤ 5 с CPU на 2 000 символов» | «GPU — выполнено (0,34–0,39 с). CPU — 4,75–4,91 с, запас 2–5 %: для CPU ограничить реплику 1 500 символами (~3,6 с) либо ставить акты в очередь с прогрессом» |
| П-12 | TR-2.3 / TR-2.4 | «метка получает суффикс по граммемам исходной формы» | добавить: «суффикс ставится только при однозначном определении падежа (спайк: 19,7 % форм при разборе одного слова; контекст ФИО повышает, но требует работы A1); иначе — без суффикса и пометка «проверить»» |
| П-13 | TR-4.6 | «метка нормализуется в единый токен» | подтверждено фактом: без нормализации по `[PERSON_3]` только 5 из 10 выдач «в тему», по `[ORG_21]` — 1 из 10; добавить в A2 повторный замер после нормализации |
| П-14 | D19 / A1 | «`keep_terms` — термины 1С» | добавить обязательные пункты: `ТЗ`, `ЗУП`, `1С`, `Документооборот`, `ДиректБанк`, `ГИС МТ`, коды `REQ-…`, `HTTP-сервис`; отдельно — уточнить regex-категории `INN`/`PASSPORT` (на стенде они «поймали» `REQ-001…REQ-020`) и не помечать почтой имя человека |


## 4. API `@cline/sdk` 0.2.0, которое понадобится в A3

Все имена — из установленного пакета (`.d.ts`), не из документации.

| Что | Точное имя и путь `.d.ts` |
|---|---|
| Точка входа | `import { ClineCore } from "@cline/sdk"` → `@cline/sdk/dist/index.d.ts` (реэкспорт `@cline/core`) |
| Создание | `ClineCore.create(options: ClineCoreOptions)` — `@cline/core/dist/ClineCore.d.ts` |
| Опции | `ClineCoreOptions`: `clientName`, `distinctId`, `backendMode: "auto"\|"local"\|"hub"\|"remote"`, `hub`, `remote`, **`fetch?: typeof fetch`**, `capabilities?: RuntimeCapabilities`, `telemetry?` (по умолчанию no-op), `featureFlags?` (no-op), `logger?`, `toolPolicies?`, `automation?`, `sessionService?`, `prepare?` — `@cline/core/dist/cline-core/types.d.ts` |
| Каталоги/состояние | отдельных опций нет: всё под `<HOME>/.cline/data`; `resolveDefaultMcpSettingsPath()` → `<HOME>/.cline/data/settings/cline_mcp_settings.json` |
| Старт сессии | `cline.start(input: StartSessionInput \| ClineCoreStartInput)` — `@cline/core/dist/runtime/host/runtime-host.d.ts` (`StartSessionInput`), `@cline/core/dist/types/config.d.ts` (`CoreSessionConfig`, `ClineCoreStartConfig`) |
| Конфиг сессии | `config: { providerId, modelId, apiKey, baseUrl, headers?, providerConfig?, cwd, workspaceRoot, systemPrompt, enableTools, enableSpawnAgent, enableAgentTeams, maxIterations, mode?, hooks?, hookErrorMode?, extraTools?, extensions?, toolPolicies? }`; `localRuntime: { hooks, extraTools, extensions, checkpoint, compaction, … }` для локального хоста |
| Отправка реплики | `cline.send({ sessionId, prompt, mode?, delivery? })` (`RuntimeHost["runTurn"]`) — возвращает `AgentResult \| undefined`; **в локальном runtime обычно `undefined`**, результат берётся из событий |
| События | `cline.subscribe(listener, { sessionId })` → `CoreSessionEvent`: `chunk`, `agent_event` (`iteration_start`, `content_start`, `usage`, `content_end`, `iteration_end`, `done`), `hook`, `status`, `session_snapshot`, `ended` (`payload.reason: completed\|error`) — `@cline/core/dist/types/events.d.ts` |
| Стоп/пауза | `cline.stop(sessionId)`, `cline.abort(sessionId)`, `cline.dispose()` |
| История | `cline.readMessages(sessionId)`, `readDisplayMessages`, `readLiveMessages`, `restore({sessionId, checkpointRunCount, start, restore})`, `compareCheckpoint` |
| Расход | `cline.getAccumulatedUsage(sessionId)` → `{ usage, aggregateUsage }` (токены/стоимость) |
| Хуки | `AgentRuntimeHooks`: `beforeRun`, `afterRun`, `beforeModel`, `afterModel`, `beforeTool`, `afterTool`, `onEvent` — `@cline/shared/dist/agent.d.ts` (строки 375–385); контексты `AgentBeforeToolContext{tool, toolCall, input, snapshot}`, результаты `AgentBeforeToolResult{skip?, stop?, reason?, input?, policy?, appendContext?}`, `AgentAfterToolResult{result?, stop?, reason?, appendContext?}` |
| Ошибки хуков | `HookErrorMode = "ignore" \| "throw"` — `@cline/shared/dist/agents/types.d.ts:217` |
| Политики | `ToolPolicy { enabled?: boolean; autoApprove?: boolean }` — `@cline/shared/dist/llms/tools.d.ts` |
| Встроенные инструменты | `getCoreHeadlessToolNames()`, `getCoreDefaultEnabledToolIds()`, `getCoreBuiltinToolCatalog()`, `createDefaultToolsWithPreset`, `ToolPresets` — `@cline/core/dist/extensions/tools/{runtime,presets}.d.ts` |
| MCP | `McpServerRegistration{transport: McpStdioTransportConfig\|McpSseTransportConfig\|McpStreamableHttpTransportConfig, disabled?, timeoutSeconds?, oauth?}`; `InMemoryMcpManager`, `registerMcpServersFromSettingsFile`, `createMcpTools`, имя инструмента `serverName__toolName` — `@cline/core/dist/extensions/mcp/{types,manager,config-loader,tools}.d.ts` |
| Суб-агенты | `enableSpawnAgent` → инструмент `spawn_agent {systemPrompt, task}`; `AgentRuntimePlugin`/`plugins` — `@cline/shared/dist/agent.d.ts` |
| Чекпойнты | `checkpoint: { enabled?, createCheckpoint? }` — по умолчанию **выключены** |

**Отличия от 0.0.90, на которые стоит обратить внимание при реализации A3:**
1. `start` принимает `{ config: {…} }` (а не `{ providerId, modelId }` в корне); хуки можно класть и в
   `config.hooks`, и в `localRuntime.hooks`.
2. Результат прогона — не возвращаемое значение `send`, а события (`agent_event.done` / `ended`).
3. MCP задаётся **только** файлом настроек (нет опции «передать серверы»).
4. `toolPolicies` не расширяет набор инструментов, а только выключает/подтверждает существующие.
5. `checkpoint` по умолчанию выключен; SDK всё равно запускает `git` для метаданных сессии.


## 5. Риски и открытые вопросы

**Риски, подтверждённые экспериментом**

1. **«Успех» при пустом ответе модели.** Если провайдер вернул 429/500 и повторы исчерпаны, прогон
   завершается `ended.reason = "completed"` с пустым текстом. A3 должен проверять наличие текста и
   `usage`, иначе UI покажет «готово» без ответа (и агент продолжит работу с пустым контекстом).
2. **Падение прогона из-за хука.** Fail-closed реализован жёстко: любое исключение в `beforeTool`
   (в обоих режимах `hookErrorMode`) прекращает прогон. `hds-guard` должен быть написан так, чтобы
   не бросать: все ошибки — в `{skip: true, reason}`.
3. **Секрет в `agent-config`.** Bearer-токен `disk-search` при передаче через `headers` попадает в
   `cline_mcp_settings.json` открытым текстом (в A0 подтверждено: файл содержит токен).
4. **Зависимость от NER для `keep_terms`.** Без списка терминов NER метит `ТЗ`, `ЗУП`, `1С`,
   `Документооборот`, `ДиректБанк`, коды `REQ-…` — агент теряет предметную область.
5. **CPU-производительность анонимизации** на грани цели; на слабом ноутбуке реплика 2 000 символов
   не уложится в 5 с.
6. **Кэш обработчика по содержимому**: повторный акт с тем же текстом возвращается за 3 мс — при
   замерах это вводит в заблуждение (в A1 помнить при профилировании).

**Открытые вопросы к владельцу**

| # | Вопрос | Варианты |
|---|---|---|
| В-1 | Куда класть токен MCP `disk-search`? Сейчас файл настроек MCP внутри `agent-config` содержит его открытым текстом | (а) ACL на файл как у `secrets.json`; (б) не писать токен в файл, а подставлять из env в момент запуска (hds пишет плейсхолдер и передаёт значение в окружение процесса агента); (в) свой HTTP-шлюз для MCP |
| В-2 | Дымовой прогон на **реальном** OpenRouter (один запрос «скажи привет») — нужен ли? Ключ не запрашивался и никуда не писался | (а) не нужен, mock достаточен; (б) сделать, ключ дам в env |
| В-3 | CPU-цель `anonymize_text`: ужесточить до 1 500 символов или оставить 2 000 с очередью? | (а) лимит 1 500; (б) очередь; (в) как есть |
| В-4 | `disk-search` для агента: подтвердить, что в A2 токен проекта будет свой (`/mcp/agent/<P>`), а текущий `/mcp` с логином владельца не используется | (а) да; (б) обсудить |
| В-5 | `.mpp` в стенде не генерировался (COM MS Project не использовался): достаточно ли текстовых образов pdf/pptx/mpp в A4 проверять отдельно? | (а) да, отдельная проверка в A4; (б) нужен и в A0 |
| В-6 | `sidecar\venv-anonymize\` (≈6 ГБ, собран для S8–S10) **не покрыт** `.gitignore` — в `git status` он висит как untracked. Добавить строку `sidecar/venv-anonymize/` в `.gitignore`? Сам `.gitignore` я не менял: он не входит в список файлов фазы | (а) добавьте сами; (б) внесите в следующий коммит A1 |

## 6. Как воспроизвести

**Окружение.** Node 24.18, npm 11.16; `sidecar\venv-anonymize` (собран
`installers\build_anonymize_sidecar.ps1 -Profile cuda -SelfTest`); модели в
`C:\ProgramData\hermes-disk-search\models\anonymize`; службы `llm_host` (:8010/:8011/:8012) и
`hds mcp-http` (:8787) запущены. Переменные окружения, нужные спайкам, задаются **внутри одной
команды** и снимаются сразу (`HDS_TRACE_LOG`, `HDS_CONFIG`, `A0_REPO_ROOT`).

**S1–S7 (Node-стенд).**

```powershell
# исходники в git: tools\parity\agent_a0_sdk\ (package.json + package-lock.json)
# рабочая копия (не в git): tools\parity\out\agent_a0\sdk\
Copy-Item tools\parity\agent_a0_sdk\* -Destination tools\parity\out\agent_a0\sdk\ -Force
cd tools\parity\out\agent_a0\sdk
npm install --save-exact --no-audit --no-fund @cline/sdk@0.2.0 tavily-mcp@0.2.22 undici

$env:A0_REPO_ROOT='C:\Users\Sasha\hermes-disk-search'; powershell -NoProfile -ExecutionPolicy Bypass -File .\s1_run.ps1; Remove-Item Env:A0_REPO_ROOT
node s2_fetch.mjs; node s3_hooks.mjs; node s4_toolpolicies.mjs; node s5_subagents.mjs; node s6_history.mjs
node s7_mcp.mjs        # собственный echo-MCP: hooks/policies на MCP
node s7_remote.mjs     # tavily + 1c-buddy + disk-search (ключи берутся из настроек владельца в память)
```

**S8 (скорость).**

```powershell
sidecar\venv-anonymize\Scripts\python.exe tools\parity\out\agent_a0\anonymize_bench.py --device cuda --label gpu --repeat 3
sidecar\venv-anonymize\Scripts\python.exe tools\parity\out\agent_a0\anonymize_bench.py --device cpu  --label cpu --repeat 2
```

**S9 (морфология).**

```powershell
sidecar\venv-anonymize\Scripts\python.exe tools\parity\out\agent_a0\morph_spike.py   # → s9\result.json
```

**S10 (стенд → образы → индекс → замер).**

```powershell
$py = 'sidecar\python\cpython-3.12.14-windows-x86_64-none\python.exe'
& $py tools\parity\agent_a0_gen_project.py                 # 32 документа в out\agent_a0\s10\project
sidecar\venv-anonymize\Scripts\python.exe tools\parity\out\agent_a0\s10_anonymize.py --device cuda
& $py tools\parity\out\agent_a0\make_s10_config.py         # временный s10\config.yaml
$env:HDS_CONFIG='C:\Users\Sasha\hermes-disk-search\tools\parity\out\agent_a0\s10\config.yaml'
.\target\release\hds.exe index --quiet
Remove-Item Env:HDS_CONFIG
& $py tools\parity\agent_a0_rag_eval.py `
    --config tools\parity\out\agent_a0\s10\config.yaml `
    --golden tools\parity\out\agent_a0\s10\golden.jsonl `
    --out tools\parity\out\agent_a0\s10\eval.json
```

**Снимки профиля (критерий §7).**

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\agent_a0_sdk\snapshot_profile.ps1 -OutFile <до.json>
# … прогоны …
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\agent_a0_sdk\snapshot_profile.ps1 -OutFile <после.json>
& $py tools\parity\out\agent_a0\snapshot_diff.py <до.json> <после.json>
```

**Данные, которые не в git** (`tools\parity\out\agent_a0\`): `sdk\node_modules`, журналы mock/egress,
`s1\…`, `s3\…`, `s7\…`, `s8\…`, `s9\result.json`, `s10\{project,anon,index.db,anonymize.db,golden.jsonl,eval.json}`,
`snapshots\{before,after}.json`.


## 7. Критерии готовности (§7 карты фазы) — состояние

| Критерий | Состояние |
|---|---|
| Сводная таблица по всем 10 пунктам, у каждого — вывод и правки плана | выполнено (§2, §3.1–3.11) |
| S1 и S2 — однозначное «да» (стоп-точки) | **да**, оба (§3.1, §3.2) |
| В отчёте — точные API SDK 0.2.0 для A3 | выполнено (§4) |
| В `%USERPROFILE%\.cline` и каталоге расширения Cline ничего не изменилось | снимок вокруг прогона S1: **0 различий** (3 229 файлов, sha256); снимок за всю сессию: 4 изменения — файлы текущего чата владельца (`data\db\sessions.db`, `sessions.db-wal` и две записи сессии `1794925343101_lh7d7`), это активность программирующего Cline, а не агента: в трассировке S1 — 0 обращений к реальному профилю |
| Боевые `index.db`, `data/anonymize.db`, `config.yaml` не менялись | время изменения: `D:\hermes-disk-search-db\index.db` — 24.10.2026, `data\anonymize.db` — 26.10.2026, `config.yaml` — 05.10.2026 (все старше даты работ); использовались только временные копии в `out\agent_a0\s10\` |
| В git не попали ключи, токены, тексты ответов `1c-buddy`/`tavily`, `node_modules` | проверено: в коммит идут только `AGENT_SPIKES.md`, два скрипта `tools/parity/agent_a0_*.py`, исходники `tools/parity/agent_a0_sdk/**` и `package-lock.json`; секретов в отчёте нет (в §3.7 — только имена инструментов и факт «ответ получен»); `node_modules` — в `out/` (игнорируется git) |
| Временные переменные окружения сняты | `HDS_CONFIG`, `HDS_TRACE_LOG`, `A0_REPO_ROOT` сняты (проверено `Get-ChildItem Env:HDS_*` — остались только предустановленные `HDS_ENGINE_DIR`, `HDS_EXTRACT_PYTHON`) |
| Службы в исходном состоянии | `llm_host` (:8010/:8011/:8012), `hds mcp-http` (:8787), `hds ui` (:8765) работают; ни одна не останавливалась и не перезапускалась; GPU-аренда `anon` не требовалась (модели влезли рядом с резидентом) |

**Что осталось за рамками A0 (осознанно).**
* Реальный облачный провайдер не вызывался (только mock) — ждёт согласия владельца (В-2).
* `disk-search` MCP не проверен end-to-end: токен владельца не принимается, нужен токен проекта (A2).
* `.mpp` в стенде не генерировался: COM MS Project не использовался; текстовые образы pdf/pptx/mpp
  проверяются в A4 (В-5).
* Вторая ступень методики RAG (сравнение агента «с RAG / без RAG», K1) — после A3, как и планировалось.
* Process Monitor не устанавливался; чтения профиля доказываются косвенно (см. §3.1, «Ограничения методики»).

