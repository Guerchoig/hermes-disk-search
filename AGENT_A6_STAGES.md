# AGENT_A6_STAGES.md — план фазы A6 (автозапуск, диагностика, эксплуатационные долги)

> **Дата среза:** 21.11.2026. **Ветка:** `agent-privacy-zone`, **HEAD:** `93c82867`
> («A4–A5 code review — fixes applied»), дерево чистое, `origin/agent-privacy-zone` — **ahead 94**,
> push не делался. **Машина:** Windows 11, PowerShell 5.1, Rust 1.99, Node 24.18,
> Python 3.12 (`sidecar\venv-anonymize`).
>
> **Что это.** План следующего этапа основного проекта `PLAN_AGENT_PRIVACY_ZONE.md`.
> A0–A5 закрыты (`AGENT_A4_REPORT.md`, `AGENT_A5_REPORT.md`), ревью A4–A5 — `AGENT_A4_A5_CODE_REVIEW.md`
> (AG-20…AG-33; AG-20…AG-28/AG-30/AG-33 исправлены коммитом `b0da9de8`, AG-29 снята).
> **A6 — не новые функции предметной области**, а перевод зоны агента в эксплуатацию:
> автозапуск зеркала, диагностика `hds check`, горячее перечитывание конфига, откат/журнал
> решений, удаление проекта, закрытие эксплуатационных находок ревью.
>
> **Статус:** план, **не исполнялся**; решения владельца по В-A6-1…В-A6-10 **согласованы
> 21.11.2026** (§3) — вопросы закрыты, под-этапы приведены к принятым решениям.
> Один под-этап = один чат = зелёный тест + коммит.

## 1. Что уже готово (A0–A5) — опора A6

| Что | Где | Чем можно пользоваться в A6 |
|---|---|---|
| Зеркало: прямой/обратный путь, реестр | `crates/hds-agent/src/{mirror,registry_db}.rs` | `Mirror::{sync_once,compare,decide,labels_report,refresh_label_images}`, `rebuild_image`, статусы TR-7.2 |
| Решения и очередь | `crates/hds-agent/src/results.rs` | `Decision::{Accept,Reject,Conflict}`, `backup_path`, `read_queue`/`mark_delivered`/`queue_prompt` (AG-20), `append_message` |
| Демон зеркала | `crates/hds-cli/src/cmd/agent_sync.rs` | `run_loop`, `StopSignal` (AG-27: `index.stop` не снимается, устаревший не считается), `SyncLog` |
| Диагностика | `crates/hds-index/src/diag.rs::run_checks` (`Check{id,status,title,msg,fix}`), `crates/hds-cli/src/cmd/check.rs::cmd_check(json)` | куда добавлять `agent-*` проверки |
| Пути и настройки | `crates/hds-agent/src/{settings,projects,paths}.rs` | `zone_root`, `workspace_dir`, `state_dir_for`, `registry_db`, `sync_settings`, `projects.json` |
| План сессии | `crates/hds-agent/src/run.rs` | `plan_with`, `resolve_node`, `resolve_worker`, `open_client_at`, `system_values`, `clear_history` |
| Вкладка «Агент» | `crates/hds-ui/src/{agent,agent_session,page}.rs` | маршруты `/api/agent/*`, SSE, `AgentApi::{client,reap_idle,unload}`, кольцо событий |
| Маршрут `disk-search` | `crates/hds-mcp/src/agent.rs` | `project_route`, `token_ok`, `project_ctx` (I5) |
| Воркеры | `crates/hds-anonymize`, `sidecar/hds_anonymize` | `drop_project`, `vault_project_labels`, `merge/unmerge`, `office_*`, словарный проход (AG-21) |
| Образец задачи Планировщика | `installers/install_ui_task.ps1` | `New-ScheduledTaskAction/Trigger/Settings`, `-Status/-Remove`, fallback `hidden_launch.vbs` |
| Приёмка и стенд | `tools/parity/out/agent_a42` (34 оригинала, 35 записей реестра, `anonymize.db` 47 меток), `A57_E2E.md`, `a57_probe.py` | сценарии, сканы I1/I2/I8 |

**Проверки на входе A6** (срез `93c82867`): `cargo fmt`/`clippy` чисто, полный `cargo test` —
единственный сбой известный флейк `quota_socket::unauthorized_heavy_rejected_before_queue`
(одиночным прогоном зелёный), python **335 OK** (skip 1), npm 23/23.

## 2. Под-этапы A6

| # | Под-этап | Результат «снаружи» (критерий) | Оценка |
|---|---|---|---|
| **A6.1** | **Диагностика.** `agent-*` проверки в `hds check` (`hds_index::diag`), `hds agent check [--project P] [--json]`: Node + `dist/worker.mjs`, Python-воркер и модели, `anonymize.db` схема v2, `projects.json`, ключ провайдера («задан/нет»), plain-токен `disk-search`, маршрут `/mcp/agent/<P>` (health/401/200), индекс зоны, демон зеркала (`sync.lock`/`sync.log`), `pending` (информационно) | `hds check` печатает `agent-*`; при выключенном агенте — одна строка `--`, при живых сервисах — 0 `fail`; `hds agent check --json` — тот же набор; нет Node → `fail` с `fix` | 1–1,5 дн |
| **A6.2** | **Автозапуск зеркала.** `installers/install_agent_sync_task.ps1` (задача на вход в систему, `-Status/-Remove`, fallback Startup), `hds agent sync --all` (цикл по `projects.json`, свой `sync.lock` на проект) | после входа в систему/`Start-ScheduledTask` демон жив и пишет `start` в `sync.log`; повторный запуск — «уже запущен»; `-Remove` снимает; `--all` обходит все проекты, недоступная папка — пропуск с `problem` | 1,5–2 дн |
| **A6.3** | **Горячее перечитывание конфига.** Демон сверяет mtime `config.yaml` между проходами: `agent.sync.*`, `index.max_file_mb`, `agent.enabled`; `agent.root` на лету не меняется (предупреждение) | правка `interval_sec` подхватывается без перезапуска (строка `config` в журнале); `agent.enabled: false` → остановка с причиной `disabled`; смена `root` → `[!]`, работа на прежнем | 1 дн |
| **A6.4** | **Откат, журнал решений, relink.** «Вернуть» из `.hds-agent\backup\<stamp>\<rel>` — **выбор штампа** (`--stamp`) и список копий в панели; журнал `.hds-agent\decisions.jsonl` (JSONL, ротация при 1 МБ, `history clear` его не трогает); relink по маркеру — **только по команде/кнопке**; «Пересоздать образ» для `orphan`; «Показать в Проводнике» для файлов панели (явное согласие, без запуска приложений); CLI `hds agent result restore\|rebuild\|relink`, маршруты `POST /api/agent/restore\|rebuild\|relink` | восстановление выбранного штампа даёт sha этой копии, образ пересоздан; журнал ротируется и не чистится `history clear`; relink восстанавливает запись только по команде (в проходе не срабатывает); `orphan` пересобирается; журнал решений — только пути/статусы (I8) | 2–3 дн |
| **A6.5** | **Удаление проекта.** `hds agent project rm <P> [--yes] [--force] [--purge-meta]` и кнопка в UI; при живом демоне — **отказ** с подсказкой, принудительно только `--force`; удаляются `<P>\` (workspace/agent-config/agent-state), `registry\<P>.db`, метки (`vault_drop_project`), запись `projects.json`, токен `secrets.json`; папка проекта не трогается | после `rm` нет каталога зоны/реестра/записи/токена, папка проекта цела; при живом `sync.lock` — отказ без `--force`; повторный `rm` — код 2; UI требует подтверждения и обновляет список | 1–1,5 дн |
| **A6.6** | **Эксплуатационные находки ревью.** AG-23 (атомарность `decide`: откат из backup при сбое `rebuild_image` + сообщение), AG-25 (кэш `ArtifactRegistry` в `AgentApi`), AG-26 (относительные пути образа → путь проекта), AG-32 (предупреждение в `state_json`, если история сессии не очищена) | на каждый пункт тест; `agent_*` наборы зелёные; поведение пользователя описано в README | 2 дн |
| **A6.7** | **Приёмка A6 и документация.** E2E `A6_E2E.md`: установка задачи → демон → правка конфига → restore/rebuild/relink → `project rm`; README §21, `RELEASE_NOTES_0.27.0.md`, `AGENT_A6_REPORT.md`; обновить `STATUS.md` | сценарий проходит целиком; I1/I8 повторно подтверждены сканами новых журналов (`decisions.jsonl`, `sync.log`) | 1,5 дн |
| **A6.8** | **K1 — замер RAG (D23).** Сравнение (А) с `disk-search` и (Б) без него на 15–20 вопросах стенда; `recall@k`/MRR поиска; правило решения из §6 плана | отчёт `tools/parity/AGENT_RAG_K1.md`, зафиксировано `agent.mcp.disk_search.enabled` по умолчанию | 1–2 дн (параллельно) |

**Итого:** ~10–13 рабочих дней. Критический путь: A6.1 → A6.2 → A6.3; A6.4/A6.5 — параллельно,
A6.6 — по одной правке с тестом, A6.7 — в конце, A6.8 — параллельно.

## 3. Решения владельца (приняты 21.11.2026)

Владелец согласился со всеми рекомендациями — вопросы **закрыты**, план приведён к решениям.
Менять их можно только новым решением (в журнале под-этапа, с обоснованием).

| # | Принято | Следствие в плане |
|---|---|---|
| **В-A6-1** | Автозапуск — **задача Планировщика на вход в систему**, fallback — «Автозагрузка»; служба Windows — фаза 2 (вместе с отдельной учётной записью агента) | A6.2: `installers/install_agent_sync_task.ps1`, `-Status/-Remove`, `hidden_launch.vbs` |
| **В-A6-2** | **Один демон на все проекты**: `hds agent sync --all` в одном процессе, свой `sync.lock` и воркеры на проект | A6.2: `--all` — часть критерия, не отдельная задача |
| **В-A6-3** | `project rm` при живом демоне — **отказ** с подсказкой; принудительно только с `--force` | A6.5: критерий «отказ при живом `sync.lock`» + `--force` |
| **В-A6-4** | «Вернуть»: **выбор штампа** (`--stamp`) и **список копий** в панели | A6.4: CLI `--stamp`, UI — список `.hds-agent\backup\*` |
| **В-A6-5** | relink по маркеру — **только по команде/кнопке**, в проходе не выполняется | A6.4: отдельная команда и маршрут, автоrelink исключён |
| **В-A6-6** | Журнал решений — **JSONL, ротация при 1 МБ**; `history clear` его не трогает | A6.4: `.hds-agent\decisions.jsonl` + ротация |
| **В-A6-7** | «Открыть» — **«показать в Проводнике»** отдельной кнопкой с явным согласием; запуск приложений не делаем; копирование пути остаётся | A6.4: кнопка в «Результатах»/«Файлах» |
| **В-A6-8** | Документация — **главы `README.md` §20/§21**; `README_AGENT.md` — при передаче заказчику (фаза 2) | A6.7: без отдельного файла |
| **В-A6-9** | K1 — **в A6**, параллельно A6.2–A6.4 | A6.8 остаётся в плане фазы |
| **В-A6-10** | Неочищенная история сессии — **`warn` в `state_json`**, без блокировки и автоочистки | A6.6 (AG-32) |

**Что это меняет по сравнению с черновиком:** relink перестал быть частью прохода (только
команда), «Открыть» ограничен Проводником, служба Windows и `README_AGENT.md` ушли в фазу 2,
а K1 и `--all` остались внутри A6 (без переноса).

## 4. Контракты A6 (не ломать)

* **Маршрут и токен.** `POST /mcp/agent/<P>` — только `project_route`/`token_ok`/`project_ctx`
  (I5); `ask_my_files` и управляющие инструменты отсутствуют (TR-4.4). Диагностика ходит в
  `/health` и `/mcp/agent/<P>` **без** выдачи токена в логи.
* **`index.stop` (AG-27).** Демон зеркала файл не снимает, устаревший сигнал (старше старта)
  не считается. Новые остановки (`agent.enabled: false`, `project rm`) общий файл не трогают —
  только строка причины в `sync.log`.
* **Секреты.** `secrets.json` пишется только со слиянием (`Secrets::save`, `save_at` — AG-22);
  ключи и токены не попадают в `config.yaml`, журналы, события и ответы API. В диагностике —
  только «задан/нет».
* **Реестр.** `ArtifactRegistry` (`status`, `issued`/`seen`, `orig_sha256`) — единственный
  источник истины о связи «оригинал ↔ образ». Новые операции (restore/rebuild/relink) проходят
  через `set_baseline`/`upsert`, не «руками».
* **Очередь сообщений (AG-20).** `.hds-agent\messages.jsonl` читается при открытии сессии и
  переезжает в `messages.delivered.jsonl`. Действия пользователя (restore) в очередь не пишутся:
  результат виден в UI.
* **I8.** `decisions.jsonl`, `sync.log`, `messages.jsonl`, `egress\*` — только пути, метки,
  статусы и числа; скан `a57_probe.py` по ним — часть приёмки.
* **D20/D21.** Новые маршруты вкладки — под `agent::gate` (403 в server-профиле) и CSRF-гейтом;
  идентификатор проекта — только `P` + 4 цифры (`is_agent_project`) в командах и путях.

## 5. Долги: что в A6, а что остаётся в фазе 2

**В A6:** `--all` по проектам (A6.2), перечитывание конфига (A6.3), «Вернуть» с выбором штампа,
журнал решений с ротацией, relink только по команде, «Показать в Проводнике», `orphan` (A6.4),
`project rm` с отказом при живом демоне (A6.5), AG-23/25/26/32 (A6.6), K1 (A6.8).

**Остаётся в фазе 2** (`PLAN_AGENT_PRIVACY_ZONE.md` §8, `AGENT_A5_REPORT.md` §7):
подписка на изменения ФС вместо опроса; настоящие образы PDF/PPTX и OCR; Word-compare и
ячеечный diff xlsx (В-A5-4); многопользовательская (server) вкладка; настраиваемые корни
индекса; отдельная учётная запись Windows + брандмауэр; `README_AGENT.md` (если В-A6-8 = (б));
Pillow для пакетного восстановления картинок xlsx; кэш sha по (size, mtime); очередь реплик и
очередь вопросов агента; точный счётчик вхождений метки (В-A5-2).

## 6. Грабли (оплачены, не наступать)

1. **`HDS_ROOT`/`HDS_CONFIG` в окружении** — тесты идут по стенду (A4 §6 п.12): перед
   `cargo test` снимать переменные.
2. **PowerShell 5.1.** `.ps1` с кириллицей — UTF-8 **с BOM** (иначе cp1251 и «мусор»);
   `param()` — первой инструкцией; тела POST — UTF-8 байтами; `Invoke-RestMethod` декодирует
   JSON как Latin-1 (читать `RawContentStream`); `& native 2>&1` при `$ErrorActionPreference='Stop'`
   даёт `NativeCommandError` (локально `Continue`, как `RunNative` в `a57_e2e.ps1`).
3. **Консоль врёт про кириллицу** — проверять файлами (`read_files`), а не выводом терминала.
4. **Порт 8787** занят службой машины (PID без PID-файла в стенде): стенд — **8788**.
5. **Индекс зоны требует эмбеддера** (`embedding.base_url` жив, `dim` совпадает): без него
   `hds index --full` падает на каждом файле.
6. **`agent-state` — в зоне**, `*.bak` и `.cline` — зона агента: в сканах I1 исключать;
   история SDK до очистки содержит открытые значения (AG-32) — это `warn`, не `fail`.
7. **Флейки** — `agent_acceptance` (`.cline` пишет IDE) и
   `quota_socket::unauthorized_heavy_rejected_before_queue`: одиночным прогоном, не «лечить».
8. **`index.stop`** — общий файл (AG-27): не удалять, устаревший сигнал игнорировать.
9. **`mark_delivered`** перемещает файл: на Windows при открытом хендле возможен
   `PermissionDenied` — ошибка не валит старт сессии, сообщение придёт снова.
10. **Задача Планировщика в тестах** — не регистрировать в обычных прогонах: E2E A6 ставит
    задачу с уникальным именем и снимает её в `finally`.

## 7. Проверки и команды

| Что | Команда |
|---|---|
| Формат / линт | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust-тесты | `$env:HDS_ROOT=$null; $env:HDS_CONFIG=$null; cargo test --workspace --no-fail-fast -- --test-threads=4` |
| Приёмка одиночным | `cargo test -p hds-cli --test agent_acceptance` |
| Целевые A6 | `cargo test -p hds-index diag`; `cargo test -p hds-cli --test agent_sync`; `cargo test -p hds-cli --test agent_result`; `cargo test -p hds-ui --test agent_api` |
| Python | `sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar` |
| Node | `npm --prefix sidecar/hds_agent test` |
| Диагностика | `hds check`; `hds check --json`; `hds agent check --project P0001 --json` |
| Демон | `hds agent sync --project P0001 --interval 1` + правка `config.yaml` + `hds stop` |
| E2E A6 | `tools\parity\out\agent_a42\a6_e2e.ps1` (UTF-8 BOM), отчёт `A6_E2E.md` |
| Инварианты | `a57_probe.py forms|scan` по `zone`, `sync.log`, `decisions.jsonl`, `egress\*` |
| Задача | `powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_agent_sync_task.ps1 -Status` / `-Remove` |

## 8. Промт для A6.1 (копировать целиком)

```text
Ты — разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Задача — под-этап A6.1 плана AGENT_A6_STAGES.md: диагностика зоны агента.

Сначала прочитай целиком AGENT_A6_STAGES.md (§1–§7 — контекст, решения, контракты, грабли,
проверки) и §5.8–5.9 плана PLAN_AGENT_PRIVACY_ZONE.md (TR-8.5, инварианты). Код:
crates/hds-index/src/diag.rs (Check, run_checks), crates/hds-cli/src/cmd/check.rs (cmd_check),
crates/hds-cli/src/cmd/agent.rs (usage, cmd_agent_cfg), crates/hds-agent/src/{settings,run,secrets}.rs,
crates/hds-mcp/src/agent.rs, crates/hds-ui/src/agent.rs (state_json).
Срез: ветка agent-privacy-zone, HEAD 93c82867.

Сделай:
1. agent-* проверки в hds_index::diag (новая функция check_agent(cfg) -> Vec<Check>, вызов из
   run_checks): agent.enabled/root (задан, абсолютный, не общий каталог), Node (resolve_node) и
   dist/worker.mjs (resolve_worker), Python-воркер (discover_python) и anonymize.db (схема v2),
   projects.json (читается, пути существуют), ключ провайдера проекта («задан/нет», без значения),
   plain-токен disk-search (предупреждение, если только хэш), маршрут /mcp/agent/<P> (health,
   401 без токена, 200 с токеном — только если mcp_http.port слушает), индекс зоны (db_path,
   projects=[P]), демон зеркала (sync.lock + lock_is_stale, последняя строка sync.log), pending.
   При agent.enabled: false — одна строка [--] «агент выключен», остальное не проверяется.
2. hds agent check [--project P] [--json]: тот же набор, отфильтрованный по проекту; коды 0/1.
3. Тесты: unit на check_agent с временным конфигом (нет root → fail; нет Node → fail;
   нет plain-токена → warn; выключенный агент → одна строка); сокет-тест маршрута не обязателен,
   если он уже есть в hds-mcp/.

Не меняй поведение существующих проверок и не печатай значений секретов. Прогоны: fmt, clippy
-D warnings, cargo test -p hds-index, cargo test -p hds-cli, python и npm — зелёные (известные
флейки — одиночным прогоном). Коммит: feat(agent): A6.1 — диагностика зоны агента (hds check,
hds agent check). Push не делать.
```
