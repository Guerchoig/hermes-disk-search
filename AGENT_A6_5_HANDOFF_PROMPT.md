# AGENT_A6_5_HANDOFF_PROMPT.md — A6.5: удаление проекта

> **Дата среза:** 21.11.2026. **Ветка:** `agent-privacy-zone`, **HEAD:** `6dc86b2f`
> («A6.4 — откат, журнал решений и relink»), дерево чистое, `origin/agent-privacy-zone`
> — ahead, **push не делался**.
> **Машина:** Windows 11, PowerShell 5.1, Rust 1.99, Node 24.18,
> Python 3.12 (`sidecar\venv-anonymize`).
>
> **Что это.** Контекст и готовый промт для нового чата: следующий под-этап
> `AGENT_A6_STAGES.md` §2 — **A6.5 «Удаление проекта»** (1–1,5 дн).
> Правило плана: один под-этап = один чат = зелёный тест + коммит.

## 0. Что уже закрыто (A6.1–A6.4)

| Под-этап | Коммит | Что сделано (снаружи) |
|---|---|---|
| A6.1 Диагностика | `018056d1` | `hds_index::diag::check_agent/check_agent_for` (12 пунктов `agent-*`), `hds agent check [--project P] [--json]`; `hds-index` зависит от `hds-agent` (цикла нет) |
| A6.2 Автозапуск | `15b70e84` | `hds agent sync --all`, задача Планировщика `HermesDiskSearchAgentSync`, `installers/install_agent_sync_task.ps1` (`-Status/-Remove`, fallback «Автозагрузка») |
| A6.3 Горячий конфиг | `ac64a682` | `LiveConfig` (mtime+размер): `agent.sync.*`, `index.max_file_mb`, `agent.enabled: false` → `stop reason=disabled`; смена `agent.root` — предупреждение |
| A6.4 Откат/журнал/relink | `6dc86b2f` | `Mirror::{backups,restore,rebuild,relink}`, `.hds-agent\decisions.jsonl` (JSONL, ротация 1 МБ, `history clear` не трогает), CLI `hds agent result restore\|rebuild\|relink [--stamp] [--json]`, маршруты `GET /api/agent/backups`, `POST /api/agent/{restore,rebuild,relink,reveal}`, панель «Копии…»/«Вернуть»/«Пересоздать образ» |

**Прогоны на срезе:** `fmt` — 0; `clippy -D warnings` — 0; `cargo test -p hds-agent` —
166 lib + наборы, 0 failed; `-p hds-cli` — 22 lib + все наборы (`agent_result` 6),
0 failed; `-p hds-ui` — 40 lib + все наборы, 0 failed; Python — 335 OK (skip 1);
npm — 23/23. В A6.4 заодно обезврежен предсуществующий флейк
`results::tests::queue_is_read_and_moved_to_delivered` (сравнение f64 `ts` из JSON —
теперь с допуском).

**Что осталось в A6:** A6.5 (этот этап), A6.6 эксплуатационные находки
(AG-23/25/26/32), A6.7 приёмка + документация, A6.8 K1 (RAG).

## 1. Контракты, на которые опирается A6.5

| Что | Где | Чем пользоваться |
|---|---|---|
| Строка плана | `AGENT_A6_STAGES.md` §2, A6.5 | `hds agent project rm <P> [--yes] [--force] [--purge-meta]` + кнопка в UI; удаляются `<P>\`, `registry\<P>.db`, метки (`vault_drop_project`), запись `projects.json`, токен `secrets.json`; папка проекта **не трогается**; критерий — «после `rm` нет каталога зоны/реестра/записи/токена, папка проекта цела; при живом `sync.lock` — отказ без `--force`; повторный `rm` — код 2; UI требует подтверждения и обновляет список» |
| Решение | `AGENT_A6_STAGES.md` §3, **В-A6-3** | при живом демоне — **отказ** с подсказкой; принудительно только `--force` |
| Контракты §4 | там же | секреты не в журналах/ответах; `index.stop` не трогать (AG-27); реестр — единственный источник истины; D20/D21: маршруты под `agent::gate()` + CSRF, `P` + 4 цифры |
| Пути зоны | `crates/hds-agent/src/settings.rs` | `project_dir` (207), `workspace_dir` (212), `config_dir` (217), `state_dir_for` (222), `projects_file` (227), `registry_db` (254), `secrets_file` (268) |
| Реестр проектов | `crates/hds-agent/src/projects.rs` | `load_at` (74), `save_at` (84, атомарно), `Projects::remove` (66), `add` (135), `remove` (152) |
| Секреты — **два вида одного файла** | `crates/hds-agent/src/secrets.rs` | `Secrets { path, raw }` (76–79): `providers.*`, `mcp.*`, `projects.<P>.{provider,model,token,token_sha256}`; **`Secrets::save` (249) сливается с диском и не умеет удалять** (AG-22) |
| Токен проекта | `crates/hds-mcp/src/agent.rs` | `AgentToken` (35), `AgentSecrets` (49), `secrets_path_from` (58), `load_at` (71), **`save_at` (86) — тоже merge, удалений нет**; `merge_json` (115) |
| Метки проекта | `crates/hds-anonymize/src/client.rs` | `Client::drop_project(project_id)` (474) → `vault_drop_project`; mock-воркер поддерживает (`crates/hds-anonymize/tests/mock_worker.py:307`) |
| Воркер анонимизации | `crates/hds-agent/src/factory.rs` | `anonymizer_factory(cfg, &root)()` → `hds_anonymize::Client` (CLI); в UI — общий клиент `AgentApi::client(cfg, state_dir)` (`crates/hds-ui/src/agent_session.rs:691`) |
| Живой демон | `crates/hds-index/src/watch.rs` | `lock_is_stale(lock)` (227), `WatchLock::acquire_named` (179); lock — `<P>\agent-state\sync.lock`, fresh = `is_file() && !lock_is_stale` |
| CLI-образец | `crates/hds-cli/src/cmd/agent_run.rs` | `cmd_project` (359), `project_add` (374), `project_list` (421); флаги — `crates/hds-cli/src/cmd/agent.rs`: `has` (107), `flag_val` (112), подтверждение с `--yes`/`IsTerminal` (408–420), `usage` (66) |
| UI-образец | `crates/hds-ui/src/agent.rs` | `project_add_json` (153), `history_clear_json` (2099: стоп → `unload` → операция → `notice`), `gate()` (74); `AgentApi::{existing_turn,is_busy,stop,unload,live,client}` |
| Маршруты | `crates/hds-ui/src/lib.rs` | GET `/api/agent/*` ~1939, POST ~2127 (все под `agent::gate()` и CSRF) |
| Панель | `crates/hds-ui/src/page.rs` | таблица `agrows` (~642) с `onclick="selectAgentProject(this)"`, форма `agname`/`agpath`/`agaddmsg` |
| Тесты | `crates/hds-cli/tests/agent_init.rs`, `crates/hds-ui/tests/agent_api.rs` | стенды с `projects.json`, токеном (`token_is_hashed_and_stored`), маршрутами проектов и CSRF/403 |

**Важно про зависимости.** `hds-index` зависит от `hds-agent` (A6.1), а не наоборот:
`hds-agent` **не может** звать `hds_index::watch::lock_is_stale`. Поэтому «демон жив»
считает вызывающий (CLI/UI), а ядру передаётся готовый факт (`daemon_alive: bool`),
иначе получился бы цикл или дубль проверки PID.

**`--purge-meta` — трактовка (не менять решения плана).** Критерий A6.5 перечисляет
только каталог зоны, реестр, запись `projects.json` и токен; метки в критерий не входят.
Рекомендуемое прочтение: обычный `rm` метки в `anonymize.db` **не трогает** (проект
можно завести снова), `--purge-meta` дополнительно вызывает `vault_drop_project`.
В отчёте всегда видно, удалены ли метки (`labels: true/false`).

## 2. Границы этапа (не расширять)

* **Не делать** A6.6 (AG-23 атомарность `decide`, AG-25 кэш реестра в UI, AG-26
  относительные пути образа, AG-32 `warn` о неочищенной истории), A6.7 (E2E/отчёт/
  `RELEASE_NOTES`) и A6.8 (K1).
* Решения плана **не менять**: В-A6-3 (живой демон — отказ, `--force`), D20/D21,
  I8, AG-27 (`index.stop` не трогать).
* Индекс зоны после удаления **не переиндексируется** и `hds agent init` не
  вызывается: это отдельное действие пользователя (A6.7/фаза 2). В README —
  одна строка «после `rm` запустите `hds agent init`, если индекс должен
  отражать новую зону».
* Папка проекта не удаляется, не переименовывается и не чистится ни при каких
  флагах.

## 3. Методика (что именно сделать)

### 3.1 `hds-agent` — удаление (новый модуль или `projects.rs`)

1. **Хирургическое удаление из `secrets.json`.** `Secrets::save` и
   `hds_mcp::agent::save_at` — merge с диском (AG-22), удалять ключи ими нельзя:
   токен «воскреснет» из файла. Добавить свободную функцию
   `pub fn remove_project_at(path: &Path, project: &str) -> Result<bool, SecretsError>`
   (в `crates/hds-agent/src/secrets.rs`): прочитать **весь** JSON (BOM срезать),
   удалить `/projects/<P>`, записать pretty-JSON через temp + `replace_file`;
   отсутствие файла — `Ok(false)`; чужие блоки (`providers`, `mcp`, другие проекты)
   не трогать; ошибки — путь и причина, **без содержимого**. Этого достаточно и для
   токена (`token`, `token_sha256`), и для `provider`/`model` проекта.
2. **Ядро удаления** — `pub fn remove_project(cfg, id, options, purge) -> Result<RemoveReport, RemoveError>`
   (рекомендуется новый `crates/hds-agent/src/remove.rs`):
   ```rust
   pub struct RemoveOptions { pub force: bool, pub daemon_alive: bool, pub purge_labels: bool }
   pub struct RemoveReport {
       pub project: String,
       pub zone: Option<PathBuf>,        // удалённый <P>\
       pub registry: Vec<PathBuf>,       // .db, -wal, -shm
       pub token: bool,                  // запись projects.<P> была и удалена
       pub record: bool,                 // запись в projects.json
       pub labels: Option<bool>,         // Some — попытка была
       pub left: Vec<PathBuf>,           // что удалить не удалось (пути, без значений)
   }
   pub enum RemoveError {
       NotRegistered(String),            // повторный rm (CLI → код 2, UI → 404)
       DaemonAlive(PathBuf),             // живой sync.lock без --force (CLI/UI → 409)
       Busy(PathBuf),                    // os error 32 на Windows
       Io(PathBuf, String),
       Worker { report: RemoveReport, message: String }, // метки не удалились
   }
   pub fn remove_project(
       cfg: &Config,
       id: &str,
       options: RemoveOptions,
       purge: Option<&mut dyn FnMut(&str) -> Result<(), String>>,
   ) -> Result<RemoveReport, RemoveError>;
   ```
   `purge` — внедряемый вызов `vault_drop_project` (в CLI/UI это
   `hds_anonymize::Client::drop_project`; в unit-тестах — замыкание-счётчик, без
   Python). Так ядро не создаёт воркер само и остаётся тестируемым.
3. **Порядок удаления** (детерминированный и с понятным частичным отказом):
   1) проверки: `is_agent_project` (D21), запись в `projects.json` есть (иначе
      `NotRegistered`), `daemon_alive && !force` → `DaemonAlive`;
   2) каталог зоны `<P>\` (`workspace`, `agent-config`, `agent-state`) — первый:
      если он занят (демон/`hds agent run`), ничего ещё не снято, пользователь
      закрывает и повторяет; при `--force` сначала снять `sync.lock`, затем удалять;
   3) реестр: `<P>.db`, `<P>.db-wal`, `<P>.db-shm`;
   4) `remove_project_at(secrets_file, id)`;
   5) `projects::remove(cfg, id)`;
   6) `purge_labels` → `purge(id)`.
   Частичный отказ (`Busy`/`Io`) возвращать с уже собранным `RemoveReport`
   (в `RemoveError::Worker` поле `report`; для `Busy`/`Io` — либо отдельный вариант
   с отчётом, либо `report` в ошибке) — CLI обязан напечатать, что уже удалено и
   что осталось.
4. **Безопасность путей.** Перед `remove_dir_all` канонизировать `<agent.root>` и
   `<P>` (AG-11), требовать `canonical(P) == canonical(root)/P` и `starts_with(root)`;
   если `<P>` — симлинк/junction, удалять **ссылку**, не цель
   (`symlink_metadata().file_type().is_symlink()`). Никогда не удалять сам
   `agent.root`, `data\agent`, `secrets.json` целиком.
5. **Секреты/I8.** В `RemoveReport` — только пути и `bool`. Токен (plain и хэш) и
   значения из базы меток не печатать и не логировать; ошибки воркера — текст
   без значений.


### 3.2 CLI — `hds agent project rm` (`crates/hds-cli/src/cmd/agent_run.rs`)

* `hds agent project rm <P> [--yes] [--force] [--purge-meta]`; `cmd_project`
  (359) добавляет ветку `rm`; `usage` в `crates/hds-cli/src/cmd/agent.rs`.
* До удаления **перечислить**, что будет удалено: `<P>\` (workspace/agent-config/
  agent-state), пути `<P>.db` (+wal/shm), «токен: да/нет», «метки: с `--purge-meta`»,
  и отдельной строкой «папка проекта не трогается: <path>».
* Без `--yes` — подтверждение как у `agent init` (образец 408–420: `IsTerminal`,
  неинтерактивный запуск → отказ с кодом 2).
* «Демон жив»: `<P>\agent-state\sync.lock` есть и `!hds_index::watch::lock_is_stale`
  → без `--force` код 1 с подсказкой «остановите: `hds stop` (или снимите задачу
  `installers\install_agent_sync_task.ps1 -Remove`) либо повторите с `--force`».
* `--purge-meta`: `anonymizer_factory(cfg, &project_root)()` →
  `Client::drop_project(&id)` (это делает ядро через `purge`); ошибка воркера —
  код 1, но в отчёте видно, что каталог/реестр/запись/токен уже удалены,
  а метки остались.
* Коды: 0 — удалено; 1 — отказ/частичный отказ (живой демон, busy, воркер);
  2 — аргументы и повторный `rm` (проект не зарегистрирован).
* `--json` не требуется планом; если добавлять — по образцу A6.4 (`ok/action/...`),
  только пути и `bool`.

### 3.3 UI — маршрут и кнопка

* `POST /api/agent/project/rm {project, force?, purge_meta?}` в POST-блоке
  `crates/hds-ui/src/lib.rs` (~2127) под `agent::gate()` + CSRF (в server-профиле —
  403). Ответ: `{ok, project, removed:{zone, registry, token, record, labels},
  left:[...]}` — пути и `bool` (I8). Коды: 400 — нет `project`/id не по D21,
  404 — проект не зарегистрирован (повтор), 409 — живой `sync.lock` без `force`,
  500 — IO/воркер (в тексте — что осталось).
* Перед удалением: как `history_clear_json` (2099) — если ход идёт, `api.stop`,
  затем `api.unload(&project)`; после успеха — обновить список проектов, сбросить
  `AG_ID` (выбрать следующий проект или пусто), закрыть панели «Копии…»/«Сравнить».
  `notice` в ленту удаляемого проекта не нужен (кольцо уходит вместе с сессией).
* `AgentApi::client(cfg, state_dir)` — общий клиент для `--purge-meta`
  (как `with_client_mirror`); `daemon_alive` считать через
  `hds_index::watch::lock_is_stale` (в UI `hds-index` уже в зависимостях).
* Панель (`page.rs`): в строке таблицы `agrows` — кнопка «Удалить…» с
  `event.stopPropagation()` (строка кликабельна — `selectAgentProject`), подтверждение
  в `confirm` с перечислением того, что уйдёт, отдельной фразой «папка проекта
  останется». После 200 — `loadAgentProjects()`; после 409 — сообщение с подсказкой
  «остановите демон (`hds stop`) и повторите» (UI процессов не запускает).

## 4. Критерии приёмки (из `AGENT_A6_STAGES.md` §2, строка A6.5)

1. После `rm` нет каталога зоны `<agent.root>\<P>` (workspace/agent-config/
   agent-state), нет `registry\<P>.db` (+`-wal`/`-shm`), нет записи в
   `projects.json`, нет проекта в `secrets.json`; **папка проекта и её файлы целы**.
2. При живом `sync.lock` без `--force` — отказ (код 1 / HTTP 409), ничего не
   удалено; с `--force` — удалено.
3. Повторный `rm` — код 2 (CLI) / 404 (UI), не 500.
4. UI: кнопка с подтверждением, после удаления список проектов обновлён,
   проект исчез из вкладки.
5. `--purge-meta` вызывает `vault_drop_project`; в отчёте `labels: true`;
   без флага метки остаются (`labels: false`/`None`).
6. Секретов и значений в выводе/ответах/журналах нет (I8): в отчёте — только
   пути и «да/нет».

## 5. Тесты (минимум)

* `hds-agent` unit: `secrets::remove_project_at` (удаляет только цель; `providers`,
  `mcp`, соседние проекты целы; нет файла → `false`; ошибки без значений);
  `remove_project` на временном конфиге: все артефакты исчезли, отчёт корректен,
  `NotRegistered` на повтор, `DaemonAlive` при `daemon_alive && !force`, `force`
  проходит; `purge`-замыкание вызывается только с `purge_labels`; busy-файл
  (Windows, `share_mode`) → `Busy`, состояние прежнее; симлинк `<P>` не уводит
  удаление за корень (можно `#[cfg(windows)]` или проверить защиту пути напрямую).
* `hds-cli` новый `tests/agent_project_rm.rs` (mock-воркер
  `crates/hds-anonymize/tests/mock_worker.py`): регистрация + `<P>\…` + `<P>.db`
  + токен (`hds agent token`) → `project rm P --yes` код 0, всё перечисленное
  удалено, папка проекта цела; повтор → 2; живой lock (`std::process::id()` в
  `<P>\agent-state\sync.lock`) → без `--force` 1 и проект на месте, с `--force` 0;
  `--purge-meta` → 0 (mock отвечает на `vault_drop_project`); без `--yes` и без
  терминала → 2.
* `hds-ui` `tests/agent_api.rs`: `POST /api/agent/project/rm` 200 (всё удалено,
  папка проекта цела), 409 при живом lock, 404 повтор, 403 без CSRF, 403 в
  server-профиле; тело ответа — только пути/`bool`.
* Опционально: unit-тест `page::PAGE` содержит кнопку удаления (строковая
  проверка), чтобы UI-кнопка не потерялась.

## 6. Прогоны

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings        # при `os error 1455` — `-j 2`
$env:HDS_ROOT=$null; $env:HDS_CONFIG=$null
cargo test -p hds-agent
cargo test -p hds-cli
cargo test -p hds-ui
cargo test -p hds-cli --test agent_project_rm
cargo test -p hds-ui --test agent_api
.\sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```


## 7. Грабли (оплачены — не наступать)

1. `HDS_ROOT`/`HDS_CONFIG` в окружении — снимать перед `cargo test` (§6 п.1 плана).
2. **AG-22/merge:** `hds_agent::secrets::Secrets::save` и `hds_mcp::agent::save_at`
   сливаются с диском и **не умеют удалять**. Наивное `projects.remove(P); save_at()`
   вернёт токен из файла. Удаление — только точечной функцией чтения всего JSON.
3. Один и тот же `data\agent\secrets.json` читают два API (`hds_agent::secrets` —
   providers/mcp/projects; `hds_mcp::agent` — токены). Удалять из общего файла,
   сохраняя чужие блоки; ничего не перезаписывать целиком из структуры.
4. Реестр — SQLite WAL: удалять `<P>.db`, `<P>.db-wal`, `<P>.db-shm`. При живом
   демоне файлы заняты (`os error 32`) — это и есть причина отказа без `--force`,
   а не «ошибка логики».
5. `hds-index` зависит от `hds-agent` — в `hds-agent` нельзя звать
   `hds_index::watch::lock_is_stale`. `daemon_alive` считает CLI/UI и передаёт
   в ядро; в unit-тестах — просто `true/false`.
6. `--force` не «магия»: живой демон может держать файлы и **пересоздать**
   каталог `<P>` после удаления. В сообщении — подсказка `hds stop` / снять задачу
   `installers\install_agent_sync_task.ps1 -Remove`; после `--force` в отчёте
   честно писать «демон мог пересоздать каталог — остановите его».
7. `index.stop` не трогать (AG-27): rm не создаёт и не удаляет общий сигнал.
8. Папка проекта неприкосновенна; перед рекурсивным удалением — канонизация и
   проверка `starts_with(<agent.root>)` (AG-11), симлинк/junction — удалять ссылку.
9. Секреты и значения базы меток не попадают в вывод, ответы API и журналы (I8):
   в отчёте только `token: true/false`, `labels: true/false`, пути.
10. Маршруты вкладки — только `agent::gate()` + CSRF; `P` — `P` + 4 цифры (D21);
    в server-профиле 403 (D20).
11. Повторный `rm` — код 2 (CLI) и 404 (UI); не превращать в 500 и не удалять
    «остатки» молча.
12. Тесты не запускают настоящий демон и не ставят задачу Планировщика: живой lock
    имитируется записью собственного PID (`std::process::id()`).
13. `hds check` / `hds agent check` после `rm` не должны падать: `projects.json`
    без записи, зоны `<P>` нет (диагностика это уже переживает).
14. Индекс зоны после `rm` не чистится/не перестраивается автоматически — это
    отдельное действие (`hds agent init`), упомянуть в README и отчёте.
15. `--purge-meta` идёт через Python-воркер: в тестах — mock
    (`crates/hds-anonymize/tests/mock_worker.py`, `vault_drop_project` уже отвечает);
    в unit-тестах ядра — замыкание, без Python.

## 8. Промт для нового чата (копировать целиком)

```text
Ты — разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Задача — под-этап A6.5 плана AGENT_A6_STAGES.md: удаление проекта.

Сначала прочитай AGENT_A6_STAGES.md (§2 строка A6.5, §3 решение В-A6-3, §4 контракты,
§6 грабли), AGENT_A6_5_HANDOFF_PROMPT.md (контекст, контракты, методика, грабли) и код:
crates/hds-agent/src/{projects,settings,secrets,factory}.rs,
crates/hds-mcp/src/agent.rs (AgentSecrets/save_at — merge),
crates/hds-anonymize/src/client.rs (drop_project), crates/hds-index/src/watch.rs
(lock_is_stale), crates/hds-cli/src/cmd/{agent,agent_run}.rs,
crates/hds-ui/src/{agent,agent_session,lib,page}.rs,
тесты crates/hds-cli/tests/agent_init.rs и crates/hds-ui/tests/agent_api.rs.
Срез: ветка agent-privacy-zone, HEAD 6dc86b2f (A6.1–A6.4 закрыты, дерево чистое).

Сделай:
1. hds_agent: хирургическое удаление проекта из secrets.json
   (remove_project_at: весь JSON, убрать /projects/<P>, temp+replace_file; providers/mcp
   и другие проекты целы; AG-22 — save_at/save сливаются и удалять не умеют);
   remove_project(cfg, id, RemoveOptions{force, daemon_alive, purge_labels}, purge)
   -> RemoveReport/RemoveError: каталог <P>\, реестр <P>.db(+wal/shm), токен+provider/model,
   запись projects.json; purge — внедряемый вызов vault_drop_project (Client::drop_project);
   проверки D21, NotRegistered на повтор, DaemonAlive при живом sync.lock без --force,
   Busy при os error 32, безопасность путей (канонизация, симлинк, не трогать папку проекта).
2. CLI hds agent project rm <P> [--yes] [--force] [--purge-meta]: перечислить удаляемое
   до удаления, подтверждение как у agent init, коды 0/1/2 (повтор — 2), подсказка
   «hds stop» при живом демоне, --purge-meta через anonymizer_factory.
3. UI: POST /api/agent/project/rm {project, force?, purge_meta?} под agent::gate() и CSRF
   (server-профиль 403); перед удалением stop/unload сессий, после — обновить список
   проектов; в панели проектов кнопка «Удалить…» с подтверждением (stopPropagation),
   после успеха список и выбранный проект обновляются.
4. Тесты: unit на remove_project_at/remove_project (порядок, NotRegistered, DaemonAlive,
   force, purge, Busy, симлинк, I8); CLI-тесты кодов и следов (папка проекта цела,
   повтор 2, живой lock 1/0, --purge-meta с mock); сокет-тесты UI (200/404/409/403 CSRF,
   server 403).
5. README: §15 (команда), §20 «Вкладка «Агент»» и новый подраздел «Удаление проекта»:
   что удаляется, что остаётся, отказ при живом демоне, --force, --purge-meta,
   после rm — hds agent init при необходимости.

Не делать A6.6/A6.7/A6.8 и не менять решения плана. Секреты и значения из базы меток
в вывод, ответы API и журналы не попадают. Прогоны: cargo fmt, clippy -D warnings
(при os error 1455 — -j 2), cargo test -p hds-agent / -p hds-cli / -p hds-ui, python
и npm — зелёные. Коммит: feat(agent): A6.5 — удаление проекта (hds agent project rm,
кнопка во вкладке). Push не делать.
```

