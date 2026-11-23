# AGENT_A6_FIXES_HANDOFF_PROMPT.md — продолжение исправлений A6 (три B-варианта)

> **Дата:** 23.11.2026. **Ветка:** `agent-privacy-zone`. **HEAD:** `6193863a` (промт-док),
> продуктовый срез — `6f0070b7`, тег `v0.27.0` (`b8967937`). **Коммит с правками НЕ создан, push
> не делался** — все изменения лежат в рабочем дереве и считаются «своими» (не откатывать).
> **Ревью и текущее состояние:** `AGENT_A6_CODE_REVIEW.md` (там же §8 «что исправлено» и §9
> «решения владельца»). **Отчёт фазы:** `AGENT_A6_REPORT.md`; **K1:** `tools/parity/AGENT_RAG_K1.md`.
>
> **Что это.** Контекст и готовый промт для нового чата: реализовать три варианта, выбранные
> владельцем 23.11.2026 (AG6-02 → B, AG6-15(а) → B, AG6-16(б) → B) поверх уже сделанных
> исправлений ревью A6.
>
> **Состояние на выходе (23.11.2026):** все три задачи §3–§5 **выполнены**; что сделано, какие
> тесты добавлены и результаты прогонов — `AGENT_A6_CODE_REVIEW.md` §10/§10.1. Коммит не создан,
> push не делался; стенд `tools\parity\out\agent_a42` не менялся (`a6_e2e.ps1 -Preflight` — 0).

## 1. Состояние рабочего дерева на входе (проверено)

Уже сделано (ревью A6, **не переделывать и не откатывать**; всё проверено прогонами):

* **AG6-03** `restore`: копия текущей версии в `backup\<штамп>`, откат при занятом файле/сбое
  образа, `decision_failed` (`crates/hds-agent/src/results.rs`).
* **AG6-01/AG-34** `hds stop` явно ставит mtime `index.stop` (`crates/hds-cli/src/cmd/stop.rs`
  + юнит-тест).
* **AG6-04** ротация `decisions.jsonl` под lock-файлом; **AG6-05** `k1-times.json` +
  `--score-only` → `k1-summary-score.json`; **AG6-06** `--once` занимает `sync.lock`;
  **AG6-07** guard «папка проекта вне `agent.root`» в `projects::validate`; **AG6-08**
  `remove_project_checked` (повторная проверка lock); **AG6-12** проверка индекса по всем путям;
  **AG6-13** `disk-search` из `mcp.servers` не обходит выключенную настройку; **AG6-15(б)**
  `--all` при всех недоступных папках → код 0; **AG6-16(а)** `reap_idle` в `/api/agent/events`.
* **AG6-10** `-Hidden` у задачи Планировщика; **AG6-14** `setup-node` + `npm ci && npm run build`
  в Windows-джобах `rust.yml`/`release.yml`.
* **AG6-09/AG6-11** формулировки: `README.md` §20/§21, `RELEASING.md`,
  `RELEASE_NOTES_0.27.0.md`, `tools/parity/AGENT_RAG_K1.md` §7/§9.

Проверки этого среза: `fmt` 0, `clippy -D warnings` 0, `hds-agent` **185** lib (0 failed),
`hds-index` 51 (0 failed), `hds-cli` все наборы (0 failed), `hds-ui` все наборы (одиночный прогон
известного флейка `abort_cancels_pending_ask` — зелёный), python **335 OK** (skip 1), npm
**23/23**. Стенд `tools\parity\out\agent_a42` восстановлен (`k1-summary.json` не тронут,
`index.stop` убран).

## 2. Решения владельца (23.11.2026) — обязательны к исполнению

| # | Решение |
|---|---|
| **AG6-02** | **B.** Отмена **между файлами**: `Mirror::sync_once_cancellable(cancel)`, `sync_once()` — обёртка `\|\| false`; демон передаёт `stop.triggered()`. Текущий файл дорабатывается полностью. |
| **AG6-15(а)** | **B.** Супервизор `--all`: перечитывать `projects.json`, добирать новые проекты, перезапускать поток при смене пути, останавливать исчезнувшие; пер-проектные флаги; кулдаун для занятого `sync.lock`. |
| **AG6-16(б)** | **B.** `hds agent init` останавливает зеркальный демон и ждёт снятия `sync.lock` (таймаут — честный отказ), плюс строка «демон зеркала будет остановлен» в списке подтверждения. |
| **AG6-09** | Поправить формулировки — **выполнено**, повторно не трогать. |
| **Коммит** | После реализации: `fix(agent): A6 review fixes — restore backup/rollback, stop signal, ops races`. Push не делать, релиз не выпускать. |

## 3. Задача 1 — AG6-02: отмена прохода между файлами

**Файлы:** `crates/hds-agent/src/mirror.rs`, `crates/hds-cli/src/cmd/agent_sync.rs`, тесты.

**Точки правки (срез `6193863a`, номера строк до правок):**
* `mirror.rs:771` `pub fn sync_once` → оставить обёрткой:
  `self.sync_once_cancellable(&|| false)`.
* Новый `pub fn sync_once_cancellable(&mut self, cancel: &dyn Fn() -> bool) -> Result<SyncReport, MirrorError>`,
  который вызывает `sync_forward(cancel)` и `sync_reverse(cancel, &mut report)`.
* Прямой путь `sync_forward` (`:778`): проверка `if cancel() { break; }` — в начале **обоих**
  циклов по файлам (`:790` — статусы/хеши/`wait_stable`, `:854` — `publish`/`rename`).
* Обратный путь `sync_reverse` (`:1252`): та же проверка в цикле по файлам (`:1268`).
* `run_project` (`agent_sync.rs:264`, замыкание прохода в `run_loop` `:373-437`):
  `mirror.sync_once_cancellable(&|| stop.triggered())`.
  `run_loop` уже проверяет `should_stop()` перед сном — после отчёта цикл сразу выйдет с
  `reason=index.stop`, а строки `pass`/`stop` в журнал попадут.

**Требования:**
* не прерывать анонимизацию текущего файла (только «перед следующим»), иначе возможен
  недописанный образ;
* `wait_stable` (до `agent.sync.wait_max_sec` = 30 с на файл) остаётся непрерываемым —
  допустимо: верхняя граница задержки = один файл; зафиксировать это в отчёте по этапу;
* `--once` и UI не меняются (`sync_once()` сохраняет поведение).

**Тест:** `mirror::tests::sync_once_cancellable_stops_between_files` — папка из 3–5 файлов,
`cancel` возвращает `true` после первого вызова; проверить, что обработана часть, `report`
согласован, повторный проход без отмены доводит остальное.

**Критерий приёмки:** `hds stop` во время прохода на папке в сотни файлов завершает демон
в пределах обработки одного файла + ≤0,5 с; в `sync.log` есть `pass …` и `stop reason=index.stop`;
`sync.lock` снят.

## 4. Задача 2 — AG6-15(а): супервизор `--all` с перечитыванием `projects.json`

**Файл:** `crates/hds-cli/src/cmd/agent_sync.rs` (режим демона `sync_all`, `:490-567`), тесты
`crates/hds-cli/tests/agent_sync.rs`.

**Что сейчас:** список читается один раз, на проект — поток с общим `StopSignal`, затем `join`
до общей остановки. Новый проект/новый путь подхватываются только перезапуском демона.

**Целевая схема:**
* константы: `SUPERVISOR_TICK = 5s` (проверка списка), `BUSY_RETRY = 60s` (кулдаун для занятого
  `sync.lock`), сон внутри тика шагами ≤500 мс с проверкой `stop.triggered()`;
* состояние: `HashMap<String, Worker { flag: Arc<AtomicBool>, handle: Option<JoinHandle<ProjectOutcome>>,
  path: String, busy_until: Option<Instant> }>`;
* `run_project` получает четвёртым/дополнительным параметром локальный флаг остановки
  (`Option<&AtomicBool>`); в режиме демона `should_stop = || stop.triggered() || local.load(Relaxed)`;
  `--project` и `--once` передают `None`;
* тик супервизора:
  1. если mtime `projects.json` изменился — `projects::load(cfg)`;
  2. для каждого проекта: нет в карте → запустить поток (если не активен `busy_until`);
     есть, но `path` другой → поднять флаг, дождаться потока, запустить заново;
  3. проект исчез из файла → поднять флаг и дождаться (удаление через `project rm` при живом
     демоне и так запрещено — это сценарий ручной правки файла);
  4. поток завершился сам (busy/ошибка) → `handle=None`; если причина busy — `busy_until = now+60s`,
     чтобы не перезапускать каждые 5 секунд;
* при `stop.triggered()` — поднять все флаги, дождаться всех, напечатать прежнюю сводку
  (`запущено/уже работали/пропущено/отказов`) и вернуть код как сейчас;
* `--all --once` не меняется (последовательный обход, без потоков).

**Подводные камни:** не держать Mutex карты во время `join`; не плодить дубли потоков; не
пересоздавать поток на каждый тик из-за того, что он ещё стартует (запись в карту — до `spawn`);
не менять формат `sync.log` и коды возврата.

**Тесты:**
* `sync_all_daemon_picks_up_new_project` — старт `--all` с P0001, дождаться `pass`; дописать
  P0002 в `projects.json`; за ≤15 с появиться `P0002\agent-state\sync.lock` и строка `start`/`pass`
  в его журнале; `hds stop` — оба потока завершились, lock сняты;
* (по возможности) `sync_all_daemon_restarts_project_on_path_change` — сменить `path` у P0001 на
  вторую папку, дождаться `start` с новым `source=…` в журнале.

**Критерий приёмки:** «добавил проект — заработал без перезапуска»; смена пути применяется;
`hds stop` останавливает всё; наборы `agent_sync`/`agent_acceptance` зелёные.

## 5. Задача 3 — AG6-16(б): `agent init` останавливает зеркало

**Файл:** `crates/hds-cli/src/cmd/agent.rs` (`cmd_init`, `:329-506`), тесты в том же файле.

**Что сейчас:** `stop_watch` (пишет `index.stop` и ждёт до 60 с `watch.lock`), затем удаление
`index.db`, создание пустой базы и **безусловное** удаление `<root>\index.stop` (`:464`).
Если в этот момент работает зеркальный демон, сигнал он либо увидит (остановится), либо нет
(mtime-правило) — поведение лотерейное; при этом `index.db` уже снесён.

**Целевая схема:**
* новый helper (тестируемый, без долгих ожиданий):
  `fn stop_mirror_daemons(cfg: &Config, ids: &[&str], wait_secs: u64) -> Result<usize, String>`
  (или `(root, &[PathBuf])` — как удобнее): для каждого id берёт
  `settings::state_dir_for(cfg, id).join("sync.lock")`; если lock есть и не устарел
  (`hds_index::watch::lock_is_stale`), один раз вызывает `crate::cmd::stop::stop(root)`
  (уже с явным mtime) и опрашивает все lock до исчезновения/устаревания; по таймауту —
  `Err` с подсказкой `hds stop`. Список проектов у `cmd_init` уже есть:
  `hds_core::roots::agent_roots(&cfg)` → `entry.project`;
* в `cmd_init` после `stop_watch`: вызвать helper (те же 60 с, что у watch); при `Err` —
  напечатать и **вернуть 1** (не удалять базу под живым демоном);
* в списке «Будет очищено» добавить строку «демон зеркала: запущен — будет остановлен»
  (по аналогии с существующей строкой про демон индексации);
* только после этого удалять `index.db`, затем — `index.stop` (watch требует отсутствия файла).

**Тесты (unit):** live lock (PID текущего процесса) + `wait_secs=0` → `Err`, зона цела;
stale lock (PID 0) → `Ok(0)`, ничего не ждём; без lock → `Ok(0)`; при живом lock проверено,
что `index.stop` создан (mtime свежий).

**Критерий приёмки:** `hds agent init` при работающем зеркале детерминированно останавливает
его (или честно отказывает по таймауту с кодом 1); после `init` в живых нет ни `watch.lock`,
ни `sync.lock`; `index.stop` удалён; `hds agent sync --all` после `init` стартует нормально.

## 6. Проверки (обязательные для каждого шага)

```powershell
# env стенда снять, иначе тесты пойдут по стенду
$env:HDS_ROOT=$null; $env:HDS_CONFIG=$null; $env:CARGO_INCREMENTAL='0'

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -j 2 -- -D warnings
cargo test -p hds-agent -j 2
cargo test -p hds-cli -j 2 --no-fail-fast
cargo test -p hds-ui  -j 2 --no-fail-fast
cargo test -p hds-index -j 2
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

Стенд (`tools\parity\out\agent_a42`, `HDS_CONFIG`/`HDS_ROOT` — его):

```powershell
target\debug\hds.exe agent check --project P0001 --json      # 12 пунктов, все ok
target\debug\hds.exe agent run --project P0001 --dry-run --json
target\debug\hds.exe agent sync --all --once                 # последовательный обход
# проверка отмены (AG6-02): демон --interval 1 на папке с несколькими файлами + hds stop
# проверка супервизора (AG6-15): дописать проект в projects.json при живом --all
powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\out\agent_a42\a6_e2e.ps1 -Preflight
```

`a6_e2e.ps1` **целиком не запускать** без разрешения (меняет стенд); `-Preflight` безопасен.
Полный e2e после трёх правок — по решению владельца.

**Известный флейк:** `agent_chat::abort_cancels_pending_ask` — предсуществующий, под нагрузкой в
полном прогоне `hds-ui` падает, одиночным зелёный. Не «лечить», одиночный прогон для подтверждения:
`cargo test -p hds-ui --test agent_chat abort_cancels_pending_ask -j 1`.

## 7. Границы и грабли

* **Не откатывать** уже сделанные правки ревью A6 (список — §1 и `AGENT_A6_CODE_REVIEW.md` §8).
* **Push не делать**, релиз/тег не трогать; коммит — только после подтверждения владельца
  (сообщение в §2).
* Не менять решения плана (`PLAN_AGENT_PRIVACY_ZONE.md`, `AGENT_A6_STAGES.md` §3) и формат
  журналов `sync.log`/`decisions.jsonl`, коды возврата CLI.
* `HDS_ROOT`/`HDS_CONFIG` в окружении ломают тесты — снимать перед `cargo test` (грабля проекта).
* PowerShell 5.1: `.ps1` с кириллицей — UTF-8 **с BOM**, тела POST — байтами UTF-8; читать файлы
  в репозитории лучше инструментом чтения, а не `Get-Content` (кодировка консоли врёт).
  `Set-Content -Encoding utf8` в PS 5.1 пишет BOM — для правок файлов репозитория использовать
  инструмент редактирования, а не скрипты PowerShell.
* `target\debug\hds.exe` может быть залочен запущенным `hds sync`/`hds ui` — остановить перед сборкой.
* Значения из базы меток, ключи и токены в отчёты/логи не печатать (I8); артефакты проб — только
  в `tools/parity/out/` (гитигнорится).
* Существующие тесты, которые завязаны на старую семантику: `agent_sync` (14 интеграционных),
  `agent_sync::loop_tests` (22 юнита), `agent_acceptance`; после AG6-15/16 проверить, что
  «повторный запуск — уже запущен, код 0» и «недоступная папка — problem» не сломаны.

## 8. Промт для нового чата (копировать целиком)

```text
Ты — разработчик проекта hermes-disk-search (Rust + Python/Node sidecar). Продолжаешь
исправления по ревью фазы A6 — три варианта, выбранные владельцем 23.11.2026.

Сначала прочитай целиком:
1) AGENT_A6_FIXES_HANDOFF_PROMPT.md (этот файл: состояние, решения, задачи, проверки, границы);
2) AGENT_A6_CODE_REVIEW.md §8 (что уже исправлено) и §9 (решения владельца);
3) README.md §20/§21 (эксплуатация зоны агента), AGENT_A6_STAGES.md §4/§6 (контракты и грабли).

Срез: ветка agent-privacy-zone, HEAD 6193863a, тег v0.27.0. В рабочем дереве уже лежат
незакоммиченные исправления ревью A6 (20 файлов + два новых документа: AGENT_A6_CODE_REVIEW.md
и AGENT_A6_FIXES_HANDOFF_PROMPT.md) — они «свои», не откатывать, коммит не создавать без
подтверждения.

Сделай три задачи строго по §3–§5 хендоффа:
A. AG6-02 (вариант B): Mirror::sync_once_cancellable(cancel) с проверками перед каждым файлом
   (прямой и обратный путь), sync_once() — обёртка, демон передаёт stop.triggered();
   тест «отмена в середине прохода».
B. AG6-15(а) (вариант B): супервизор hds agent sync --all — перечитывать projects.json раз в
   5 с, добирать новые проекты, перезапускать поток при смене path, останавливать исчезнувшие;
   пер-проектные флаги остановки, кулдаун 60 с для занятого sync.lock; тесты «добавили проект —
   заработал», «сменили путь — перезапустился».
C. AG6-16(б) (вариант B): hds agent init останавливает зеркальный демон (сигнал с явным mtime +
   ожидание снятия/устаревания sync.lock, таймаут — код 1 с подсказкой hds stop) и печатает
   строку про демон зеркала в подтверждении; unit-тесты helper'а с wait_secs=0.

Требования: не менять формат журналов и коды возврата; текущий файл при отмене дорабатывается
полностью; значения из базы меток, ключи и токены не печатать. После каждой задачи —
fmt, clippy -D warnings, целевые тесты; в конце — полный набор (§6 хендоффа), python и npm.
Стенд: tools\parity\out\agent_a42, a6_e2e.ps1 только -Preflight. Известный флейк
agent_chat::abort_cancels_pending_ask не лечить (одиночным прогоном зелёный).
Push не делать, релиз не выпускать. Итог — короткий отчёт: что сделано, какие тесты добавлены,
результаты прогонов и что осталось.
```
