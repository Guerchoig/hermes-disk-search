# AGENT_A0_A3_CODE_REVIEW.md — код-ревью фаз A0–A3 плана «ИИ-агент в анонимной зоне»

> Дата: 19.11.2026 · Ветка: **`agent-privacy-zone`** · HEAD: `cb635027` («A3.7 closed - phase A3»),
> дерево чистое, push не делался.
> Объём: коммиты `3f34d51d`…`cb635027` (A0–A3, 47 коммитов от `origin/anonymizer-sidecar`),
> 155 файлов, +41 503/−460 — включая документы фаз, `package-lock.json` и `AGENT_A3_STAGES.md`;
> продуктовый код — `crates/hds-agent`, `crates/hds-anonymize`, `crates/hds-core`,
> `crates/hds-index`, `crates/hds-mcp`, `crates/hds-search`, `crates/hds-cli`, `crates/hds-ui`,
> `sidecar/hds_anonymize`, `sidecar/hds_agent`, `installers/`.
> Метод: чтение полного итогового кода и документов фаз (`PLAN_AGENT_PRIVACY_ZONE.md`,
> `AGENT_A*_HANDOFF_PROMPT.md`, `AGENT_A3_STAGES.md`, `tools/parity/AGENT_SPIKES.md`),
> **независимые прогоны** тестов/линтера, **живые пробы** (реальный vault, схема MCP-настроек
> SDK 0.2.0) и воспроизведение найденных дефектов. Артефакты проб — `tools/parity/out/review-*`
> (каталог гитигнорится).

## 1. Итог

| Фаза | Вердикт | Обоснование |
|---|---|---|
| **A0** (спайки, стенд, методика RAG) | **Принята** | Отчёт `AGENT_SPIKES.md` самодостаточен, стоп-точки (1) и (2) закрыты экспериментом, продуктовый код не менялся; дефекты стенда (Р-1…Р-4) исправлены в самой фазе. Находок по A0 нет. |
| **A1** (база меток v2, морфология, образы, Office) | **Принята с замечанием** | Схема v2, миграция без переноса, `norm_key`, keep_terms, office-операции, текстовые образы — реализованы и покрыты (307 python-тестов OK). Но контракт падежного суффикса **не выдержан на границе Python→Rust** — см. **AG-1 (high)**. |
| **A2** (корни, маршрут `/mcp/agent/<P>`, метки в FTS, отставание) | **Принята** | Изоляция проектов проверена сокет-тестом (401, чужой проект, отсутствие `ask_my_files`), инвариант I5 обеспечен `Scope`+`project_ctx` независимо от `access.enabled`. Замечания — **AG-8** (стоимость `lag` на каждый поиск), **AG-11** (удаление выхода через junction). |
| **A3** (Node-воркер, крейт `hds-agent`, CLI, установщик, приёмка, документация) | **Принята с обязательными доработками** | Архитектура D1/D13 подтверждена: единственный сетевой выход — `egressFetch`, fail-closed, guard, политика плагинов, MCP-фильтр, реестр процессов, установщик с sha256. Но три дефекта ломают пользовательское поведение: **AG-1** (метки с латинской граммемой), **AG-2** (снимок vault не обновляется, TR-6.5), **AG-3** (токен проекта хранится только хэшем → `disk-search` молча не подключается). Их нужно закрыть **до вкладки A5**, иначе UI покажет пользователю сырые метки вместо значений и останется без RAG. |

**Обязательно до A4/A5:** AG-1, AG-2, AG-3. **Желательно до A5:** AG-4, AG-5, AG-6, AG-7.
**Хвост (A6):** AG-8…AG-19.

> **Статус на 19.11.2026:** все находки AG-1…AG-19 **исправлены** в ветке
> `agent-privacy-zone` (рабочее дерево, коммит не делался) — что именно изменено и чем
> проверено, см. §8.

## 2. Независимые проверки (не по отчётам фаз)

| Проверка | Как | Результат |
|---|---|---|
| Формат | `cargo fmt --all --check` | чисто ✅ |
| Линт | `cargo clippy --workspace --all-targets -- -D warnings` | чисто ✅ (лог пуст) |
| Rust-тесты | `cargo test --workspace -- --test-threads=4 --skip acceptance_a3_end_to_end` | **120 наборов, 755 passed, 0 failed** ✅ |
| Приёмка A3 как есть | `cargo test --workspace` (полный, с приёмкой) | `acceptance_a3_end_to_end` **FAILED**: `agent_acceptance.rs:818` «профиль .cline изменился» — воспроизведена нестабильность приёмки **AG-7**; остальные наборы зелёные |
| Node-воркер | `npm test` в `sidecar/hds_agent` | **23/23 pass**, 45,9 с ✅ |
| Python-воркер | `sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar` | **Ran 307, OK (skipped=1)** ✅ |
| Реальный снимок vault | проба `Vault.project_label_ex(...)` + `snapshot()` на временной БД | `forms[].gram` = `nomn`, `gent`, `datv`, `nomn,мн`, … — **коды pymorphy**, метка — `|дат` (**AG-1**) |
| Синтаксис метки в Rust | временный тест `hds-agent` (создан, прогнан, удалён; исходник — §7) | фильтр выдаёт `[PERSON_1|nomn]`; `is_label` = **false** для `|nomn` и `|datv`; деанонимизатор оставляет метку как есть; для `|дат` подставляет **именительный** вместо дательного (**AG-1**) |
| Схема MCP-настроек SDK 0.2.0 | чтение бандла `@cline/core/dist/hub/daemon/entry.js` + `node -e "m.resolveMcpTimeoutSeconds(undefined)"` | принимаются **оба** формата (`{transport:{…}}` и плоский `{type,command/url}`); поле таймаута — **`timeout`**, по умолчанию 60 с; `timeoutSeconds` **отбрасывается** (**AG-6**) |
| Вызовы `Gateway::refresh` | `Select-String` по `crates/` и `tests/` | **0 вызовов** (только определение) — TR-6.5 не выполняется (**AG-2**) |

## 3. Находки

Уровни: **high** — ломает пользовательское поведение или требование плана; **medium** —
заметный дефект/риск, обходится; **low** — качество, документация, производительность;
**info** — наблюдение.

### AG-1 (high). Метки теряют падеж: две несовместимые системы граммем на границе Python→Rust

**Файлы:** `sidecar/hds_anonymize/vault.py:823` (`snapshot`), `sidecar/hds_anonymize/morph.py:615`
(`gram_from_suffix`, там же прямо написано «это разные пространства, и путать их нельзя»),
`crates/hds-agent/src/filter.rs:246` (`label_with_gram`), `crates/hds-agent/src/deanon.rs:101`
(`resolve`), `crates/hds-core/src/labels.rs:29` (`label_parts`).

**Что не так.** В базе `forms.gram` хранятся **коды pymorphy** (`nomn`, `gent`, `datv`, …),
а в метке по TR-2.3 стоит **русский суффикс** (`|дат`, `|род,мн`). Снимок отдаёт `gram` как есть,
и дальше:

1. `filter::label_with_gram` подставляет `gram` **в метку**: фильтр выдаёт
   `[PERSON_1|nomn]`, `[PERSON_1|datv]`, `[ORG_2|gent,мн]`. Это нарушает TR-1.4/TR-2.3
   (именительный ед. ч. — без суффикса; суффикс — русский).
2. `hds_core::labels::label_parts` принимает суффикс только из **не-ASCII** букв и запятой →
   `is_label("[PERSON_1|nomn]") == false`. Следствия:
   * деанонимизатор **не подставляет значение** — пользователь видит в ответе сырой
     `[PERSON_1|nomn]`/`[PERSON_1|datv]`;
   * `DeAnonymizer::text` не считает её неизвестной (`unknown` пуст) — предупреждения нет;
   * если такая метка попадёт в файл образа, `split_labels` не выдаст `xperson1x` — поиск по
     метке (TR-4.6) этот фрагмент не найдёт.
3. `deanon::resolve` сравнивает суффикс метки с `form.gram` — пространства не совпадают никогда,
   поэтому **падежная подстановка не работает даже с правильным `|дат`**: проба даёт именительный
   «Иванов Иван» вместо дательного «Иванову Ивану».
4. Запасной путь в `resolve` ищет форму с `gram` = пусто/`None`; в реальном снимке такой формы
   нет (у встреченной формы `gram` = её падеж), поэтому берётся **`forms.first()`** — первая
   встреченная форма, которая может быть косвенным падежом. По TR-2.4 без суффикса обязательна
   именительная форма.

**Почему тесты зелёные.** `filter.rs` и `deanon.rs` строят фикстуры вручную и пишут в `gram`
русские суффиксы (`"дат"`), т.е. проверяется не тот формат, который отдаёт `vault.snapshot`.
`agent_run.rs` (тест истории) и `AGENT_A3_STAGES.md` п.27 уже **зафиксировали** `[PERSON_1|nomn]`
как ожидаемое поведение — это решение противоречит плану и его надо пересмотреть.

**Воспроизведение:** §7, проба `zz_review_probe` (вывод приложен).

**Рекомендация.** Одно пространство на всю систему:
* в `vault.snapshot` отдавать готовый **суффикс** (`morph.suffix_for(case, plural)`, пусто для
  именительного ед. ч.) — тогда `filter::label_with_gram` и `deanon::resolve` совпадают без
  второй таблицы; либо
* оставить коды, но конвертировать их в Rust (`nomn`→"", `gent`→"род", …) в одном месте
  (`hds_core::labels`), а `label_with_gram` не добавлять суффикс для `nomn` без `,мн`;
* в `deanon::resolve` запасной путь — искать `nomn` (и `nomn,мн` при множественном числе), и
  только затем первую форму;
* тест на **реальном** снимке (`Vault` → `snapshot` → `Filter`/`DeAnonymizer`) вместо
  рукописных `gram`; проверять `is_label` для всех веток.

### AG-2 (high). Снимок vault не обновляется: TR-6.5 не выполнен, новые метки не деанонимизируются

**Файлы:** `crates/hds-agent/src/filter.rs:343,371` (проверка только `built_at`),
`crates/hds-agent/src/gateway.rs:694,719` (`refresh`/`refresh_from_snapshot` есть, но **не
вызываются**), `crates/hds-cli/src/cmd/agent_run.rs:505-540` (снимок берётся один раз до хода),
`crates/hds-cli/tests/agent_acceptance.rs:611-616` (обход: метка заводится заранее).

**Что не так.** TR-6.5 требует: «Перед каждым запросом снимок сверяется по `vault_rev`».
Фактически словарь и деанонимизатор строятся **один раз** в начале сеанса, ревизия не
проверяется, `refresh` никто не зовёт. Два следствия:

1. **Первый ход на чистом проекте показывает пользователю сырые метки.** `Gateway::send`
   анонимизирует реплику через `anonymize_text`, который **создаёт новые метки** в базе. Этих
   меток нет в карте деанонимизатора, поэтому когда модель (по правилам `privacy.md` — «пиши
   метки как есть») вернёт `[PERSON_1]`, пользователь увидит `[PERSON_1]`, а не значение.
   Приёмка A3.6 этого не видит, потому что перед ходом специально заводит метку через
   `label_for_value` («чтобы снимок фильтра знал его форму»); в документированном сценарии
   (README §15: `agent init` → `project add` → `agent run`) метки нет.
2. **Ход длиннее `agent.filter.max_stale_sec` (300 с) обрывается.** После истечения
   `max_stale` каждый `egress_check` возвращает `privacy_filter_unavailable` — это корректный
   fail-closed, но без обновления снимка сеанс восстановить нельзя (UI/CLI не имеют точки
   обновления). Для 8–12 итераций агента с Office-операциями 5 минут — реальный сценарий.

**Рекомендация.** Подключить уже написанный `Gateway::refresh`:
* после `Gateway::send` (анонимизация реплики могла создать метки) и/или
* в обработчике `egress_check model` перед проверкой словаря: дешёвый `vault_rev` (RPC
  `vault_rev`) → при смене пересобрать снимок (снимок — тысячи строк, но это не NER);
* для A5 — тик по `max_stale`.
Тест: пустой vault → реплика с новым ФИО → ответ модели с меткой → пользователь получает
значение; и тест «ход длиннее `max_stale`» (подменить `built_at`/`max_stale`).

### AG-3 (medium-high). Токен проекта хранится только хэшем — `disk-search` молча не подключается

**Файлы:** `crates/hds-cli/src/cmd/agent.rs:171-232` (`agent token` пишет `token_sha256`),
`crates/hds-agent/src/secrets.rs:137` (`project_token` читает `projects.<P>.token`),
`crates/hds-agent/src/mcp.rs:67` (`McpSpec.token = secrets.project_token(...)`),
`crates/hds-cli/src/cmd/agent_run.rs:357-367`, `README.md:899` (шаг 3) и `README.md:1420-1438`
(шаги 3–4), `crates/hds-cli/tests/agent_init.rs:215` («в файл пишется только хэш»).

**Что не так.** После `hds agent token P0001` в `data/agent/secrets.json` есть только
`token_sha256`/`expires` — этого достаточно маршруту `/mcp/agent/<P>` (A2). Но воркеру нужен
**plain-токен**, чтобы положить `Authorization: Bearer …` в `cline_mcp_settings.json`; он
читается из `projects.<P>.token`, которого никто не пишет. Итог: `McpSpec.token = None` →
сервер `disk-search` **не попадает** в настройки MCP → у агента нет инструмента поиска по
проекту, при этом ошибки нет (тихое отсутствие RAG). Приёмка A3.6 это маскирует: стенд пишет
оба поля руками (`agent_acceptance.rs:245-258`), а README шаг 3-4 про plain-токен молчит.

**Рекомендация.** Один из вариантов:
* `hds agent token` пишет **и** `token` (plain), и `token_sha256` (файл и так под ACL, а
  plain-токен нужен локальному агенту по плану §4.4 «secrets.json … токены /mcp/agent»);
* либо `hds agent run` при отсутствии `projects.<P>.token` печатает явное предупреждение
  «disk-search отключён: нет plain-токена» и подсказку, как его добавить;
* в README — шаг «положите выданный токен в `projects.P0001.token`».
Тест: `agent token` → `agent run --dry-run` → в `mcp.servers` есть `disk-search`.

### AG-4 (medium). `mcp_in` фильтрует только `result.output`, а не весь результат

**Файл:** `sidecar/hds_agent/src/hooks.ts:146-162`.

`afterTool` отправляет в `egress_check {kind:"mcp_in"}` только `result.output` и заменяет только
его (`{...result, output: checked.body}`). TR-6.3 говорит о подмене **`result`**. Всё остальное —
`structuredContent`, `metadata`, вложенные поля, которые SDK кладёт в результат MCP, — уходит
модели без фильтра. Для текущих серверов (`disk-search` — текст, `tavily` — текст) это не
проявляется, но `mcp.servers` (A3.7) разрешает любой сторонний сервер, в т.ч. возвращающий
структурированные данные.

**Рекомендация:** фильтровать весь объект `result` (кроме служебных `isError`/`_meta`, если они
не строковые), т.е. `filter::redact` по нему целиком; тест: MCP-заглушка возвращает значение в
`structuredContent` → модель видит метку.

### AG-5 (medium). Секреты в словарь собираются с перебором; граница слова ослабляет блокировку

**Файлы:** `crates/hds-agent/src/secrets.rs:191-232` (`secret_strings`,
`collect_secret_strings`), `crates/hds-agent/src/filter.rs:518` (граница слова).

1. `collect_secret_strings` обходит **все** строки блоков `providers` и `mcp` (включая
   `mcp.servers`) длиной ≥ 8: в «секреты» попадают `base_url` провайдера, `command`
   (`C:\tools\1c-buddy.exe`), `args` (`--env-file`), `url` сторонних серверов. Любое появление
   такой строки в теле запроса к модели или в аргументах MCP → `FilterError::Secret` →
   `privacy_filter_unavailable` (fail-closed, но ложное срабатывание: агент «ломается» на
   безобидном тексте).
2. Проверка секрета идёт через ту же «границу слова», что и значения: секрет, вклеенный в
   более длинный буквенно-цифровой токен (`…0123abc`, `xsk-…`), **не** будет найден. Для
   секретов это направление fail-open.

**Рекомендация:** собирать только значения секретных ключей (`api_key`, `token`, `password`,
`authorization`, `headers.*`, `env.*`), не обходить `command`/`args`/`url`/`base_url`; для
секретов не применять правую границу слова (или применять отдельное правило — блокировать
вхождение подстроки). Тест: `--env-file` в реплике не блокирует запрос; `sk-…x` блокирует.

### AG-6 (medium). Таймаут MCP в `cline_mcp_settings.json` — мёртвое поле

**Файл:** `crates/hds-agent/src/mcp.rs:99,114,132`.

`to_json` пишет `"timeoutSeconds": 60|120`, но схема SDK 0.2.0 (`@cline/core`, разбор
`cline_mcp_settings.json`) знает только **`timeout`** (секунды, 1–3600, по умолчанию 60), а
неизвестные ключи отбрасывает. Итог: `1c-buddy` получает 60 с вместо 120, `disk-search`/`tavily`
— дефолт (60), т.е. заявленные таймауты не действуют. Проверено чтением бандла и пробой
`resolveMcpTimeoutSeconds(undefined) → 60`.

**Рекомендация:** писать `"timeout": 60|120` (плоская схема SDK его принимает в обоих форматах —
и `{transport}`, и `{type,command/url}`); добавить assert в тест `mcp::tests`.

### AG-7 (medium). Приёмка A3 нестабильна на машине разработчика (проверка профиля `~/.cline`)

**Файл:** `crates/hds-cli/tests/agent_acceptance.rs:84-113,810-820`.

`snapshot_dir(~/.cline)` сравнивает **все** файлы профиля по (путь, размер, mtime) до и после
прогона. На машине, где параллельно работает Cline (IDE пишет `data\db\sessions.db`, `-wal`,
файлы сессий), проверка I1 падает ложно. Воспроизведено в этом ревью: полный
`cargo test --workspace` → `acceptance_a3_end_to_end` FAILED ровно на строке 818, при этом все
остальные 755 тестов зелёные; повтор с `--skip acceptance_a3_end_to_end` — 0 падений.

**Рекомендация:** проверять инвариант, а не всё дерево:
* в снимок включать только элементы, чей mtime **старше** начала теста, и/или исключить
  `sessions.db*`, `tasks*`, `*.log`;
* дополнительно проверять содержательно: в `~/.cline` нет путей workspace агента и значений
  из теста;
* либо вынести проверку в CI-джобу без запущенного Cline (и пометить тест `#[ignore]` с
  запуском по флагу).

### AG-8 (low). Отставание индекса считается обходом всего дерева на каждый поиск

**Файл:** `crates/hds-index/src/lag.rs:67-142`.

`project_lag` вызывается из `search_local_files` (маршрут агента) и `index_status` **на каждый
вызов**: полный `walk_files` корня проекта + `metadata` по каждому файлу; для изменившихся —
`content_hash` (до 512 КБ чтения). На проекте в тысячи файлов это секунды, добавленные к каждому
поиску (агент вызывает поиск несколько раз за ход). Плюс `index_status` в `lag` ходит в heartbeat.

**Рекомендация:** считать `pending` по `files` (статусы) и хранить отметку «сверено на такой-то
mtime/rev», либо кэшировать результат на N секунд; полный обход оставить диагностике
(`index_status`), а в выдачу поиска отдавать последнее известное значение.

### AG-9 (low). `GatewayEvent::Agent.unknown` — это все метки события, а не неизвестные

**Файлы:** `crates/hds-agent/src/gateway.rs:68,645,754-777`, `crates/hds-agent/src/deanon.rs:52`.

Документация поля говорит «неизвестные метки», но `collect_unknown` собирает **каждую** метку из
`raw`-события. `DeAnonymizer::text` уже вычисляет настоящий список неразрешённых меток и выбрасывает
его. UI (A5) покажет «не подставлено: [PERSON_1]» для успешно подставленной метки.

**Рекомендация:** отдавать `unknown` из результата деанонимизации; если нужен полный список —
назвать поле `labels` и добавить отдельное `unknown`.

### AG-10 (low). `Filter::redact` держит блокировку словаря на время IPC в Python

**Файл:** `crates/hds-agent/src/filter.rs:358-405` (mutex на весь проход), `ClientLabeler`
(`crates/hds-agent/src/gateway.rs:151-161`).

Каждое неизвестное значение (`others` с политикой `label`) — синхронный вызов
`label_for_value` (Python, 0.3–5 с) **под замком словаря**; вдобавок `GatewayHost::egress` держит
замок клиента. Взаимной блокировки в текущем графе вызовов нет, но все `egress_check`
сериализуются, а длинный запрос к модели может «залипнуть» на минуты. Рекомендация: собрать
уникальные неизвестные значения, отпустить замок, разрешить метки одним пачкой, затем применить
замены.

### AG-11 (low). `agent init` удаляет выход анонимизации без разрешения junction/симлинков

**Файл:** `crates/hds-cli/src/cmd/agent.rs:135-165,402-408`.

Ограждение В-10 проверяет вложенность **лексически** (`std::path::absolute`), а удаление —
`remove_dir_all`. На Windows junction не считается символической ссылкой, и `remove_dir_all`
может обойти его и удалить содержимое цели. Сценарий требует, чтобы junction лежал внутри
`<state>\data` или `<agent.root>` и указывал наружу, но предохранитель должен это ловить.
Рекомендация: перед проверкой вложенности канонизировать путь (разрешая ссылки) и отказывать,
если разрешённый путь не совпал с лексическим.

### AG-12 (low). `session_id` не валидируется перед построением пути истории

**Файл:** `crates/hds-agent/src/history.rs:60-75,79-115,121`.

`--session <ID>` подставляется в `session_dir` без проверки: `ID` с `\`, `/` или `..` выводит
чтение/перезапись за пределы каталога сессий (локальный CLI, владелец — тот же пользователь;
модель к этому пути доступа не имеет). Рекомендация: принимать только `[A-Za-z0-9_-]{1,64}`,
иначе — ошибка использования.

### AG-13 (low). Поздний `done` после таймаута команды попадает в поток событий

**Файл:** `crates/hds-agent/src/worker.rs:243-275,299-325`.

После `RecvTimeoutError::Timeout` ожидание снимается; пришедший позже `done` с тем же `id` не
находит отправителя и уходит в канал событий (`events.send(up)`). `Gateway::map_up` превратит его
в `GatewayEvent::Done`, который `agent_run::print_event` игнорирует, но UI может принять за
завершение текущего хода. Рекомендация: помнить id снятых ожиданий и отбрасывать их ответы
(или логировать в stderr).

### AG-14 (low). `env::build` принимает `cfg` и не использует

**Файл:** `crates/hds-agent/src/env.rs:58-65` (`let _ = cfg;`).

Прокси приходит аргументом, конфиг не нужен. Убрать параметр (и поправить вызовы) или читать
`agent.network.proxy` внутри — сейчас мёртвый аргумент в публичном API.

### AG-15 (low, документация/контракт). Секция `agent:` в `config.example.yaml` описана не полностью

**Файлы:** `config.example.yaml:68-70` (`enabled`, `root`), `crates/hds-agent/src/settings.rs:120-150`.

`settings()` читает ещё `preview_outgoing`, `subagents`, `keep_terms`, `filter.unknown`,
`filter.max_stale_sec`, `network.proxy`, `egress_log`, `max_live_projects`, `idle_unload_min`,
`request_timeout_sec`, `node.path`, `mcp.{disk_search,tavily,onec_buddy}.{enabled,command}`,
`mcp.tavily.command`. Ни один из них (кроме `root`/`enabled`) не виден в примере конфига —
найти их можно только в коде. Дополнительно TR-8.3 называет ключи `agent.images` и
`agent.keep_terms_file`, а реализация читает `anonymize.agent.images` и
`anonymize.keep_terms_file` (`hds-anonymize/src/settings.rs`), и не поддерживает `args` у
встроенных MCP. Рекомендация: привести пример конфига и план к фактическим ключам (или
реализовать указанные), чтобы контракт TR-8.3 был проверяем.

### AG-16 (low, UX/документация). Пример `agent project add --path` указывает workspace, а не папку проекта

**Файлы:** `README.md:1437` (пример), `crates/hds-agent/src/projects.rs:21-32`,
`crates/hds-agent/src/deanon.rs:131-140`.

`Project.path` — путь **папки проекта пользователя** (TR-8.1), по нему `DeAnonymizer` заменяет
`<P>\workspace` на путь пользователя, а `system_values` маскирует его как `[SYS_n]`. Пример в
README передаёт `D:\hds-anon\P0001\workspace`, т.е. анонимный путь: подстановка путей не работает,
пользователь видит пути зоны. Рекомендация: в примере — реальная папка проекта; проверять, что
`path` абсолютный; в справке команды описать смысл поля.

### AG-17 (info). `privacy.md` упоминает метку `[PATH_3]`, которой не существует

**Файл:** `crates/hds-agent/src/rules.rs:11-13`.

A1 не выпускает сущностей типа `PATH`; агент получает `[PERSON_n]`, `[ORG_n]`, `[SYS_n]`,
`[IMAGE_n]`, `[OTHER]`. Лишний пример в правилах не вреден, но может провоцировать модель
«изобретать» такие метки. Заменить на реальный тип.

### AG-18 (info). `hds agent run` готовит проект дважды

**Файлы:** `crates/hds-cli/src/cmd/agent_run.rs:560` (`prepare` в `run_cli`), `:505` (`prepare` в
`run`).

Правила и `cline_mcp_settings.json` пишутся дважды, `icacls` вызывается дважды. Безвредно, но
лишний I/O и два места, где можно разойтись. Оставить один вызов (в `run`), либо сделать
`prepare` идемпотентным по маркеру.

### AG-19 (info/medium для A5). Выгрузка проекта в реестре блокирует вызывающего до 5 с

**Файл:** `crates/hds-agent/src/registry.rs:142-200`.

`get_or_open` при переполнении вызывает `evict_lru` → `stop` + `shutdown` (ожидание выхода до
5 с) **в вызывающем потоке**; `Drop` реестра — то же по всем проектам. Для CLI/тестов нормально;
во вкладке A5 переключение на третий проект может «заморозить» UI до 5 с. Рекомендация: выгрузку
в фоне (очередь на shutdown) или уменьшить ожидание для LRU (после `stop` процесс обычно выходит
быстро).

## 4. Что сделано хорошо (чтобы не потерять при доработках)

* **Единственный сетевой выход.** `egressFetch` — обязательный `fetch` у `ClineCore`; любой отказ
  (нет снимка, ошибка фильтра, таймаут, канал закрыт, неизвестный тип тела) — `privacy_filter_unavailable`
  и **ноль** запросов к провайдеру. Повторы 429 и суб-агенты идут через тот же `fetch` (A0 §3.2/3.5,
  A3.1 тесты).
* **Политика инструментов + страховки.** `toolPolicies` (run_commands/fetch_web_content — off),
  `beforeModel` вырезает запрещённые инструменты из запроса, guard в Node **и** повторная проверка
  путей в Rust для `office_*`, отказ неизвестным инструментам, отключение плагинов политикой
  (`plugins: {'*': {enabled:false}}`) — найдено и закрыто в A3.1 (плагин из `<HOME>\.cline\plugins`,
  Agent Plugin, хеш-суффикс в имени `ask_my_files`).
* **Изоляция профиля.** `env_clear` + белый список переменных, подменённые `HOME/APPDATA/TEMP`,
  отдельные `CLINE_DATA_DIR`/`CLINE_MCP_SETTINGS_PATH`; `~/.cline` владельца не читается.
* **Изоляция проекта в поиске.** `Scope::agent` + `project_ctx` (admin=false) + `project_filter` в
  FTS/векторе/CLIP; `ask_my_files` и управление индексацией на маршруте отсутствуют; токен проекта
  сверяется по sha256 в постоянное время; 401 на чужой/просроченный токен — проверено сокет-тестом.
* **Метка — одно слово в FTS** на индексации и в запросе (`split_labels`/`fts_token`), включая
  `reindex-fts`; `[PERSON_3]` и `[PERSON_30]` различаются.
* **Отставание индекса** видно агенту: `index_status` по проекту (JSON) и предупреждение в выдаче
  с путями; сбой подсчёта не превращает поиск в ошибку.
* **Жизненный цикл**: реестр живых процессов, LRU, простой, переоткрытие упавшего воркера,
  возобновление `session_id + initialMessages`, «Очистить историю» с бэкапами.
* **Установщик A3.5**: портативный Node из манифеста, sha256 **до** распаковки, `npm ci` по lock,
  `npm prune --omit=dev` в дереве поставки, self-test импортирует SDK и проверяет путь
  `tavily-mcp`; `-WithAgent` не ставит агент на сервере без явного флага.
* **Документация фаз** (`AGENT_SPIKES.md`, `AGENT_A*_HANDOFF_PROMPT.md`, `AGENT_A3_STAGES.md`) —
  редкий случай, когда «грабли» и найденные дефекты описаны вместе с фактами и командами; ревью
  смогло опереться на них и проверить независимо.

## 5. Соответствие плану (A0–A3)

| Требование | Состояние | Комментарий |
|---|---|---|
| A0 §6 (1)–(10): спайки SDK, egress, хуки, политики, суб-агенты, история, MCP, скорость | **выполнено** | Отчёт `tools/parity/AGENT_SPIKES.md`; стоп-точки 1–2 закрыты; S6/S7/S9 — «частично» с планом проверки в следующих фазах (закрыты в A2/A3) |
| TR-1.1…1.7 (база v2, ревизия, метки проектов, миграция, снимок, операции) | **выполнено** | `vault.py`, `test_anonymize_vault.py` (435 строк тестов) |
| TR-2.1…2.6 (морфология, `norm_key`, суффикс, обратная подстановка, keep_terms, слияние) | **выполнено в Python; нарушено на границе Rust** | Python-часть корректна (`gram_from_suffix`); Rust путает пространства — **AG-1** |
| TR-3.1…3.10 (рантайм, окружение, провайдер, файлы/guard, Office, MCP, суб-агенты, правила, жизненный цикл, тесты изоляции) | **выполнено**, кроме оговорённых долгов | 1c-buddy под подменённым HOME не проверялся в приёмке (сервер выключен) — перенесено осознанно |
| TR-4.1…4.7 (корни, сброс, маршрут, только поиск, пути, метки в FTS, отставание) | **выполнено** | Изоляция и 401 подтверждены тестом; AG-8 — стоимость, не корректность |
| TR-5.1…5.3 (реплики, анонимизация, события, деанонимизация) | **выполнено**, с оговоркой | Деанонимизация работает, но падеж/неизвестные метки — **AG-1**, **AG-9** |
| TR-6.1…6.7 (точки, словарь, `mcp_in`, аудит, fail-closed, очистка истории, журнал) | **выполнено частично** | TR-6.5 (сверка `vault_rev`) — **AG-2**; TR-6.3 (весь `result`) — **AG-4**; TR-6.2 г — **AG-5**; остальное выполнено |
| TR-8.1, TR-8.3 (реестр проектов, секция `agent:`) | **выполнено с отклонениями** | **AG-15** (ключи конфига), **AG-16** (смысл `path`), **AG-3** (plain-токен) |
| TR-8.4 (производительность шлюза, GPU-аренда) | **выполнено** | Клиент анонимизации один на сеанс; замеры A0 (0,34–0,39 с GPU) |
| TR-8.5 (установка и диагностика) | **частично** | Установщик есть; `hds agent check`, `hds agent sync` — решениями перенесены (A6/A4); `hds agent project rm` из плана не реализован (CLI: `add|list`) |
| TR-8.6 (документация) | **частично** | README/README_ANONYMIZATION/sidecar README/RELEASE_NOTES есть; отдельного `README_AGENT.md` нет; STATUS.md не обновлён этой фазой |

## 6. Порядок закрытия

**До A4 (зеркало) — потому что A4 будет массово создавать метки и образы:**
1. **AG-1** — единое пространство граммем, тест на реальном снимке. Иначе каждый
   образ и ответ будут содержать метки, которые деанонимизатор не понимает.
2. **AG-3** — токен для `disk-search` (иначе A4 зеркалит, а агент всё равно не ищет).

**До A5 (вкладка) — потому что UI делает поведение видимым и сеансы становятся длинными:**
3. **AG-2** — обновление снимка по `vault_rev`/таймауту (TR-6.5).
4. **AG-4** (`mcp_in` целиком), **AG-9** (`unknown`), **AG-19** (выгрузка не в потоке UI).
5. **AG-7** — стабилизировать приёмку (иначе CI/локальные прогоны красные).

**Хвост / A6:**
6. AG-5, AG-6, AG-8, AG-10…AG-18 — по одной правке с тестом.
7. Привести `config.example.yaml` (AG-15) и README (AG-16) к фактическим ключам — это дёшево и
   снимает вопросы при передаче заказчику.

## 7. Приложение: воспроизведение

### 7.1 Проба метки/деанонимизации (создана, прогнана, удалена)

```rust
// crates/hds-agent/tests/zz_review_probe.rs (временный, после прогона удалён)
use std::time::Duration;
use serde_json::json;
use hds_agent::deanon::DeAnonymizer;
use hds_agent::filter::{Filter, NoLabeler, Point};
use hds_anonymize::protocol::{Snapshot, SnapshotForm, SnapshotLabel};

fn snapshot() -> Snapshot {
    Snapshot { rev: 1, others: vec![], keep: vec![], labels: vec![SnapshotLabel {
        label: "[PERSON_1]".into(), entity_type: "PERSON".into(),
        forms: vec![
            SnapshotForm { value: "Иванов Иван".into(), gram: Some("nomn".into()), origin: "seen".into() },
            SnapshotForm { value: "Иванову Ивану".into(), gram: Some("datv".into()), origin: "derived".into() },
        ],
    }]}
}

#[test]
fn probe() {
    let f = Filter::new("P0001", vec![], hds_agent::settings::UnknownPolicy::Label,
                        Duration::from_secs(60), None);
    f.set_snapshot(&snapshot(), &[], &[], &[]);
    let out = f.redact(Point::Model, &json!("Иванов Иван передал"), &mut NoLabeler).unwrap();
    let text = out.payload.as_str().unwrap().to_string();
    println!("FILTER_OUT: {text}");
    println!("IS_LABEL_NOMN: {}", hds_core::labels::is_label("[PERSON_1|nomn]"));
    println!("IS_LABEL_DATV: {}", hds_core::labels::is_label("[PERSON_1|datv]"));
    let d = DeAnonymizer::new(&snapshot(), &[],
        std::path::Path::new("C:\\anon\\P0001\\workspace"), std::path::Path::new("C:\\proj"));
    let bare = d.text(&text);
    println!("DEANON_BARE_LABEL: {} (unknown={:?})", bare.text, bare.unknown);
    let datv = d.text("Передал [PERSON_1|datv]");
    println!("DEANON_DATV: {} (unknown={:?})", datv.text, datv.unknown);
    let ru = d.text("Передал [PERSON_1|дат]");
    println!("DEANON_RU_SUFFIX: {} (unknown={:?})", ru.text, ru.unknown);
}
```

Вывод (`cargo test -p hds-agent --test zz_review_probe -- --nocapture`; кириллица в консоли
Windows показана в кодировке консоли, значения — в UTF-8):

```
FILTER_OUT: [PERSON_1|nomn] передал
IS_LABEL_NOMN: false
IS_LABEL_DATV: false
DEANON_BARE_LABEL: [PERSON_1|nomn] передал (unknown=[])
DEANON_DATV: Передал [PERSON_1|datv] (unknown=[])
DEANON_RU_SUFFIX: Передал Иванов Иван (unknown=[])
```

То есть: (а) фильтр выдаёт латинскую граммему; (б) такая метка не распознаётся и не
деанонимизируется; (в) даже русский суффикс `|дат` даёт **именительный** падеж.

### 7.2 Реальный снимок vault (коды pymorphy)

```powershell
$env:PYTHONPATH='sidecar'
sidecar\venv-anonymize\Scripts\python.exe <probe.py>   # Vault -> project_label_ex -> snapshot
```

```
label: [PERSON_1] confident: False
{"rev": 2, "labels": [{"label": "[PERSON_1]", "forms": [
  {"value": "Иванов Иване Ивановичу", "gram": null, "origin": "seen"},
  {"value": "Иванов Иван Иванович", "gram": "nomn", "origin": "derived"},
  {"value": "Иванова Ивана Ивановича", "gram": "gent", ...},
  {"value": "Иванову Ивану Ивановичу", "gram": "datv", ...},
  {"value": "Ивановы Иваны Ивановичи", "gram": "nomn,мн", ...}, ...]}], ...}
```

### 7.3 Схема MCP-настроек SDK 0.2.0

Бандл `sidecar/hds_agent/node_modules/@cline/core/dist/hub/daemon/entry.js`:
`wj = z.union([Sj, vj, Tj])`, где `Sj = z.object({transport, disabled, timeout, …})` (форма
`{transport:{…}}`), `vj`/`Tj` — плоские `{type, command|url, timeout}`, `timeout` проходит через
`resolveMcpTimeoutSeconds` (1–3600, по умолчанию 60). `timeoutSeconds` в схемах нет — ключ
отбрасывается.

```powershell
cd sidecar\hds_agent
node -e "import('@cline/shared').then(m=>console.log(m.resolveMcpTimeoutSeconds(undefined)))"
# -> 60
```

### 7.4 Тесты ревью (логи в `tools/parity/out/`, каталог гитигнорится)

| Лог | Что |
|---|---|
| `review-test2.log` | `cargo test --workspace -- --test-threads=4 --skip acceptance_a3_end_to_end` → 120 наборов, 755 passed, 0 failed |
| `review-test.log` | полный прогон: единственное падение — `acceptance_a3_end_to_end` (профиль `~/.cline`), репро **AG-7** |
| `review-npm.log` | `npm test` → 23 pass, 0 fail |
| `review-py.log` | `unittest discover` → Ran 307, OK (skipped=1) |
| `review-clippy.log`/`.err` | clippy чистый |

## 8. Исправления после ревью (19.11.2026)

Все находки закрыты; правки — коммит `e161012e` (ветка `agent-privacy-zone`). Ниже — что
именно изменено и чем проверено.

| № | Что сделано | Проверка |
|---|---|---|
| **AG-1** | `hds_core::labels`: добавлены `CASE_SUFFIX`, `gram_to_suffix` (`datv`→`дат`, `nomn`→``, `nomn,мн`→`,мн`) и терпимый разбор уже выпущенных меток (`[PERSON_1|nomn]`, `|datv`). `filter::label_with_gram` переводит граммему в русский суффикс и не ставит его для именительного ед. ч. `deanon::resolve` сравнивает падеж в одном пространстве (`gram_to_suffix` обеих сторон) и без суффикса предпочитает именительную форму, а не первую встреченную | `hds-core::labels` тесты (`gram_to_suffix_*`, `latin_gram_suffix_is_tolerated`); `filter::pymorphy_gram_becomes_russian_suffix`; `deanon::pymorphy_gram_and_russian_suffix_match`, `bare_label_prefers_nominative`; проба §7.1 теперь даёт `[PERSON_1]`/`[PERSON_1|дат]` и «Иванову Ивану» |
| **AG-2** | `GatewayHost` получил `RefreshState` и `with_refresh`; в `egress_check` **перед каждым запросом** вызывается `vault_rev`, при смене ревизии пересобираются словарь и деанонимизатор (TR-6.5). CLI `agent run` включает это состояние. `Filter` хранит словарь в `Arc`, поэтому обновление не ждёт длинный проход | новый интеграционный тест `crates/hds-agent/tests/refresh.rs` на mock-воркере: метка, выданная после старта, попадает в деанонимизатор (`filter.rev() == 8`, `[PERSON_2]` → «Петров Пётр»); mock-воркер стал stateful (`vault_rev`/`vault_snapshot` учитывают выданные метки) |
| **AG-3** | `AgentToken` хранит и `token_sha256`, и plain-токен; `hds agent token` пишет оба (файл под ACL). `hds agent run` предупреждает, если plain-токена нет (старые файлы) и `disk-search` из-за этого отключён. README шаг 3 обновлён | `agent_init::token_is_hashed_and_stored` проверяет, что plain-токен сохранён и его хэш совпадает; `agent_route` тесты по-прежнему 401/200 |
| **AG-4** | `hooks.ts::afterTool` отправляет в `mcp_in` **весь** `result` и подменяет его целиком (TR-6.3), а не только `output` | Node-тест: заглушка возвращает значение в `structuredContent`; проверяется, что `mcp_in` получил весь результат и значение не ушло модели |
| **AG-5** | `Secrets::secret_strings` собирает только значения секретных полей (`api_key`/`token`/`password`/`authorization`, все значения `headers`/`env`) и токены проектов; `command`/`args`/`url`/`base_url` больше не «секреты». В фильтре для секретов отключена проверка границы слова — секрет внутри длинного токена блокирует запрос | `secrets::non_secret_fields_are_not_in_dictionary` (пути/аргументы/адреса не попадают, `Bearer …` и сам токен попадают); `filter::secret_inside_longer_token_blocks` |
| **AG-6** | `McpSpec::to_json` пишет `"timeout": 60/120` (ключ, который читает SDK 0.2.0) вместо игнорируемого `timeoutSeconds`; тестовые фикстуры обновлены | `mcp::disk_search_route_has_project_and_token` проверяет `timeout == 60` и отсутствие `timeoutSeconds` |
| **AG-7** | Приёмка больше не сравнивает «горячие» файлы IDE (`sessions.db*`, `tasks.db*`, `*.log`, `*.jsonl`); добавлена содержательная проверка: в `~/.cline` не должно быть пути зоны агента | полный `cargo test --workspace` (включая приёмку) на машине с запущенным Cline — **зелёный** |
| **AG-8** | `hds_index::lag::project_lag_cached` — кэш отставания на 30 с для выдачи `search_local_files`; `index_status` остаётся точным (без кэша) | поиск/`index_status` на маршруте агента — тесты `agent_route` и приёмка зелёные |
| **AG-9** | `DeAnonymizer::unknown_labels` отдаёт только неразрешённые метки; шлюз больше не собирает «все метки» | `deanon::unknown_labels_lists_only_unresolved` |
| **AG-10** | `Filter` хранит словарь как `Arc<Dictionary>`: замок держится только на клонирование, IPC `label_for_value` идёт без него | все тесты фильтра/шлюза; рефакторинг без изменения поведения |
| **AG-11** | `hds agent init` проверяет вложенность выхода анонимизации по **разрешённым** путям (`canonicalize`), junction/симлинк наружу больше не проходит предохранитель В-10 | существующие тесты `agent_init` (отказ вне разрешённых корней, `--force`) |
| **AG-12** | `history::valid_session_id` (буквы/цифры/`-`/`_`, ≤128); `clear` и `read_messages` отказывают до обращения к ФС | `history::session_id_is_validated`, `traversal_session_is_rejected_before_touching_files`; CLI-тест «нет такой сессии» переведён на валидный формат |
| **AG-13** | `Worker` запоминает id команд, ожидание которых снято по таймауту, и отбрасывает их поздний `done` вместо отправки в поток событий | транспортные тесты `worker` (эхо/таймаут) — зелёные |
| **AG-14** | `env::build` больше не принимает неиспользуемый `cfg` | компиляция + тесты `env` |
| **AG-15** | `config.example.yaml`: секция `agent:` описана полностью (preview, subagents, keep_terms, filter, egress_log, лимиты, proxy, node, mcp) с дефолтами в комментариях | YAML разбирается, `agent.keys() == ['enabled','root']` (остальное — комментарии) |
| **AG-16** | `projects::validate` требует абсолютный путь папки проекта; README-пример `project add --path` переведён на папку пользователя с пояснением | `projects::validate_requires_absolute_path_and_pnnnn` |
| **AG-17** | `privacy.md`: `[PATH_3]` заменён на реальный `[IMAGE_3]` | тест правил (`rules_do_not_contain_user_paths`) |
| **AG-18** | `run_cli` больше не вызывает `prepare` второй раз (это делает `run`) | CLI-тесты `agent_run` |
| **AG-19** | `Worker::shutdown_with(grace)`, `Gateway::shutdown_with`; реестр ждёт выхода не 5, а 2 с — выгрузка LRU не «морозит» UI надолго | тесты реестра (LRU, простой, падение) |

### 8.1 Итоговые прогоны после исправлений

| Проверка | Результат |
|---|---|
| `cargo fmt --all --check` | чисто |
| `cargo clippy --workspace --all-targets -- -D warnings` | чисто |
| `cargo test --workspace -- --test-threads=4` | **121 набор, 767 passed, 0 failed** (включая `acceptance_a3_end_to_end` — 29,4 с) |
| `sidecar/hds_agent`: `npm run typecheck`, `npm run build`, `npm test` | сборка ок, **23/23 pass** |
| python: `unittest discover -s sidecar/tests -t sidecar` | **Ran 307, OK (skipped=1)** |
| Проба AG-1 (реальный снимок → фильтр/деанон) | `[PERSON_1]` / `[PERSON_1|дат]`, дательный падеж восстанавливается |
| Проба AG-2 (`refresh.rs`) | снимок пересобирается по `vault_rev`, новая метка деанонимизируется |

**Наблюдение (не относится к A0–A3).** Один из полных прогонов дал транзиентный сбой
`hds-mcp/tests/quota_socket.rs::unauthorized_heavy_rejected_before_queue` (503 вместо ожидания
в очереди) под параллельной нагрузкой; отдельно и в повторном полном прогоне тест зелёный.
Это тест M4 (очередь/квоты), к изменениям A0–A3 отношения не имеет, но его стоит сделать
менее чувствительным к нагрузке (таймауты/ожидание вместо фиксированных пауз).
