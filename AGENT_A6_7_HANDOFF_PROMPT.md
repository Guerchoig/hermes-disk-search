# AGENT_A6_7_HANDOFF_PROMPT.md — A6.7: приёмка A6 (подготовка без живого прогона)

> **Дата среза:** 21.11.2026. **Ветка:** `agent-privacy-zone`, **HEAD:** `beff5521`
> («A6.6 — эксплуатационные находки ревью»), дерево чистое, `origin` — ahead,
> **push не делался**.
> **Машина:** Windows 11, PowerShell 5.1, Rust 1.99, Node 24.18,
> Python 3.12 (`sidecar\venv-anonymize`).
>
> **Что это.** Контекст и готовый промт для нового чата: под-этап
> `AGENT_A6_STAGES.md` §2 — **A6.7 «Приёмка A6 и документация»**.
> **Ограничение владельца (21.11.2026):** в этом чате — **без живого прогона**:
> подготовить E2E-сценарий, отчёты и документацию; сам прогон стенда — отдельная
> задача. Отчёт не должен утверждать, что сценарий пройден.

## 0. Что уже закрыто (A6.1–A6.6)

| Под-этап | Коммит | Что сделано (снаружи) |
|---|---|---|
| A6.1 Диагностика | `018056d1` | `hds_index::diag::check_agent/check_agent_for` (12 пунктов `agent-*`), `hds agent check [--project P] [--json]` |
| A6.2 Автозапуск | `15b70e84` | `hds agent sync --all`, задача Планировщика `HermesDiskSearchAgentSync`, `installers/install_agent_sync_task.ps1` (`-Status/-Start/-Remove`, fallback «Автозагрузка» через `hidden_launch.vbs`) |
| A6.3 Горячий конфиг | `ac64a682` | `LiveConfig` (mtime+размер `config.yaml`): `agent.sync.*`, `index.max_file_mb`, `agent.enabled: false` → `stop reason=disabled`; смена `agent.root` — предупреждение; строка `config: …` в `sync.log` |
| A6.4 Откат/журнал/relink | `6dc86b2f` | `Mirror::{backups,restore,rebuild,relink}`, `.hds-agent\decisions.jsonl` (JSONL, ротация 1 МБ, `history clear` не трогает), CLI `hds agent result restore\|rebuild\|relink [--stamp] [--json]`, маршруты `GET /api/agent/backups`, `POST /api/agent/{restore,rebuild,relink,reveal}`, панель «Копии…»/«Вернуть»/«Пересоздать образ» |
| A6.5 Удаление проекта | `4c19a999` | `hds agent project rm <P> [--yes] [--force] [--purge-meta]` и кнопка «Удалить…»; отказ при живом `sync.lock`, `--force`, `--purge-meta`; удаляются `<P>\`, реестр, запись `projects.json`, `/projects/<P>` в `secrets.json`; папка проекта цела; повтор — 2 |
| A6.6 Находки ревью | `beff5521` | AG-23 (откат `decide` при сбое образа + `decision_failed`), AG-25 (кэш `ArtifactRegistry` в `AgentApi`, сброс перед `project rm`), AG-26 (относительные имена образов → пути оригиналов в ленте), AG-32 (`history: {session, uncleaned, hint}` в `state_json`, предупреждение в панели) |

**Прогоны на срезе `beff5521` (для сверки в A6.7):** `fmt` — 0; `clippy -D warnings` — 0;
`hds-agent` **180 lib** + наборы, 0 failed; `hds-cli` все наборы 0 failed (в т.ч.
`agent_project_rm` 5, `agent_run` 7, `agent_sync` 14); `hds-ui` **42 lib**, `agent_api` **11**,
`agent_labels/results/settings/ui_*` — ok; Python **335 OK** (skip 1); npm **23/23**.
Единственное падение полного `hds-ui` — **предсуществующий флейк**
`agent_chat::abort_cancels_pending_ask` (проверено `git stash` на базовом `0611159f` —
падает так же; к A6 не относится).

## 1. Задача этого чата и границы

**Сделать (A6.7, без живого прогона):**

1. E2E-сценарий `tools/parity/out/agent_a42/a6_e2e.ps1` (UTF-8 **с BOM**): установка/снятие
   задачи Планировщика → демон → горячая правка `config.yaml` → `restore`/`rebuild`/`relink` →
   `project rm`; режим **`-Preflight` без побочных эффектов** (проверка путей/бинарников/портов,
   печать плана) — он прогоняется в этом чате; полный прогон — **отдельно**.
2. `tools/parity/out/agent_a42/A6_E2E.md` — **каркас отчёта** со статусом «прогон не
   выполнялся», таблицей шагов/ожиданий и разделом «как прогнать»; колонки «результат» пустые.
3. `AGENT_A6_REPORT.md` — отчёт фазы A6 по фактически закрытому A6.1–A6.6 + явный раздел
   «A6.7 подготовлен, прогон отдельно».
4. `README.md`: новая глава **§21 «Эксплуатация зоны агента»**, «Что ещё почитать» → **§22**
   (обновить «Содержание» и заголовок).
5. `RELEASE_NOTES_0.27.0.md` — черновик заметок выпуска A6 (с пометкой о непройденной приёмке).
6. `STATUS.md` — компактный блок «Зона агента A0–A6» (дата, HEAD, что закрыто, тесты, что
   осталось), без переписывания старого содержимого (файл описывает прежние ветки).

**Не делать:** живой прогон E2E (регистрация задачи «по-настоящему», старт демона, `hds stop`,
`project rm` на стенде), A6.8 (K1), push. Не менять решения плана и `PLAN_*.md`.
Не выдавать подготовку за приёмку: критерий A6.7 («сценарий проходит целиком») остаётся
**открытым**, пока прогон не выполнен.

## 2. Контракты A6.7

| Что | Где | Что значит |
|---|---|---|
| Строка плана | `AGENT_A6_STAGES.md` §2, A6.7 | E2E `A6_E2E.md`: установка задачи → демон → правка конфига → restore/rebuild/relink → `project rm`; README §21, `RELEASE_NOTES_0.27.0.md`, `AGENT_A6_REPORT.md`; обновить `STATUS.md` |
| Критерий | там же | «сценарий проходит целиком; I1/I8 повторно подтверждены сканами новых журналов (`decisions.jsonl`, `sync.log`)» — **в этом чате не достигается** |
| Решение В-A6-8 | §3 | документация — главы `README.md` **§20/§21**; отдельного `README_AGENT.md` нет (фаза 2) |
| Решения §4 | там же | секреты не в журналах/ответах (I7/I8); `index.stop` не трогать (AG-27); реестр — источник истины; D20/D21 — маршруты под `gate()`/CSRF, `P`+4 цифры |
| Грабли §6 | там же | `HDS_ROOT/HDS_CONFIG` снимать перед тестами; PS 5.1 и BOM; порт 8787 занят (стенд — 8788/8799); `agent-state` в сканах I1 исключать; задача Планировщика в обычных прогонах не ставится |

**Тонкости, уже проверенные кодом (не переоткрывать):**

* `hds agent sync --all` — **один** процесс на все проекты, у каждого свой `sync.lock`,
  `sync.log`, реестр и воркеры; повторный запуск не конфликтует («already running» — пропуск).
* Горячее перечитывание пишет строку `config: reload interval=<N>s wait_stable=…` (A6.3);
  `agent.root` не переключается на живой сессии — только предупреждение `root ignored`.
* `hds stop` создаёт общий `index.stop`; **демон его не снимает** (AG-27) — снимает индексатор;
  сигналом считается только файл, изменённый после старта демона.
* `hds agent project rm` при живом `sync.lock` → код 1 и подсказка `hds stop`; `--force`
  снимает lock и удаляет каталог; повтор → 2; папка проекта не трогается.
* Журнал решений `.hds-agent\decisions.jsonl` — JSONL, ротация при 1 МБ, только пути/статусы.

## 3. Что именно подготовить

### 3.1 `tools/parity/out/agent_a42/a6_e2e.ps1` (сценарий, без прогона)

Опора — `a57_e2e.ps1` (там же, 39 880 байт) и `a46_e2e.ps1` (демон). Скрипт **не**
переписывает их, а повторяет приёмы: `param()` первой строкой, `$ErrorActionPreference='Stop'`,
`Log`/`Check`/`Save`, `ReadShared` (чтение занятого лога), `RunNative` с локальным
`$ErrorActionPreference='Continue'` (иначе `NativeCommandError`), `GetJson`/`PostJson`
(JSON читать из `RawContentStream`).

Обязательные свойства:

* файл — **UTF-8 с BOM** (PS 5.1 без BOM читает кириллицу как cp1251);
* `param([switch]$KeepRunning, [switch]$Preflight)`;
* **`-Preflight` — без побочных эффектов**: проверить `target\debug\hds.exe` (иначе
  `target\release\hds.exe`), `sidecar\hds_agent\dist\worker.mjs`, python воркера
  (`root\sidecar\venv-anonymize\Scripts\python.exe`), `a57_probe.py`, `config.yaml`,
  каталоги `project`/`zone`, свободные порты 8788/8799, напечатать план шагов и выйти
  0/1. Ни задач, ни процессов, ни правок файлов;
* уникальное имя задачи — `"HDS-A6-E2E-$PID"`; снятие задачи, остановка процессов,
  восстановление `config.yaml` и `finally` — **всегда**, даже при `-KeepRunning`
  (флаг оставляет только стенд для осмотра, но не задачу);
* все артефакты — `a6-*.log|json|txt` в каталоге стенда (`tools\parity\out` гитигнорится);
* никаких значений из документов: проверки печатают пути, статусы и числа (I8).

### 3.2 `tools/parity/out/agent_a42/A6_E2E.md` (каркас)

Шапка: дата, ветка, HEAD, стенд, **«Статус: подготовлен; прогон не выполнялся»**,
«Сценарий: `a6_e2e.ps1` (UTF-8 с BOM), режим `-Preflight` прогнан, полный — отдельно».
Далее:

1. «Что проверяет сценарий» — таблица `# | шаг | команда/маршрут | ожидаемо | результат`
   (колонка «результат» — **пустая**, её заполняет отдельный прогон).
2. «Артефакты прогона» — список `a6-*` файлов, которые создаст скрипт.
3. «Preflight» — фактический вывод `a6_e2e.ps1 -Preflight` этого чата (дата, что найдено).
4. «Как прогнать» — точная команда + `-KeepRunning`, требования (стенд, порт 8787 занят,
   `HDS_ROOT/HDS_CONFIG` только внутри процесса), время (~5–10 мин), что будет
   зарегистрировано и снято.
5. «Осталось до приёмки A6.7» — прогон, заполнение колонки «результат», сканы I1/I8
   новых журналов, обновление `AGENT_A6_REPORT.md`/`STATUS.md` по факту.

### 3.3 `AGENT_A6_REPORT.md` (отчёт фазы A6)

Опора по структуре — `AGENT_A4_REPORT.md`/`AGENT_A5_REPORT.md`. Разделы:

1. Шапка: дата, ветка, HEAD, ссылка на `AGENT_A6_STAGES.md`, стенд, **статус приёмки**.
2. «Итог фазы» — таблица A6.1–A6.6 (коммит, что «снаружи», чем проверено).
3. «Что проверено автотестами» — числа среза `beff5521` (см. §0) + известный флейк
   `agent_chat::abort_cancels_pending_ask` с доказательством, что он предсуществующий.
4. «Контракты и инварианты» — D20/D21, I1/I8, секреты («задан/нет»), `index.stop` (AG-27),
   `agent-state` в сканах I1.
5. «Эксплуатационные сценарии» — что подготовлено в `a6_e2e.ps1` и **что осталось
   прогнать**; не писать «пройдено».
6. «Что осталось» — A6.7-прогон, A6.8 (K1); долги фазы 2 — из плана §5.
7. Ссылки: `A6_E2E.md`, `RELEASE_NOTES_0.27.0.md`, README §20/§21.

### 3.4 `README.md`: §21 «Эксплуатация зоны агента»

Новая глава **перед** «Что ещё почитать»; «Что ещё почитать» становится **§22**
(обновить пункт 21 в «Содержании» — строка ~38 — и заголовок `## 21. Что ещё почитать`
— строка ~1980; других ссылок `§21` в репозитории нет, проверено).

Содержание §21 (кратко, без секретов и значений):

* **21.1 Что установлено и где** — `<agent.root>\P<NNNN>` (`workspace`, `agent-config`,
  `agent-state`), `data\agent\{projects.json,secrets.json,registry\<P>.db}`,
  `.hds-agent\{pending,base,backup,decisions.jsonl,messages.jsonl}`;
* **21.2 Автозапуск** — `installers\install_agent_sync_task.ps1` (`-Status`, `-Start`,
  `-Remove`), задача `HermesDiskSearchAgentSync` → `hds agent sync --all`, fallback
  «Автозагрузка» + `hidden_launch.vbs`, взаимное исключение task/ярлык;
* **21.3 Диагностика** — `hds check`, `hds agent check [--project P] [--json]`, группы
  `agent-*` (зона, Node/воркер, Python/база меток, `projects.json`, ключ/токен «задан/нет»,
  маршрут `/mcp/agent/<P>`, индекс, демон, `pending`);
* **21.4 Демон зеркала** — `--interval`, `sync.lock`, `hds stop` и общий `index.stop`
  (демон его не снимает), `sync.log` (start/pass/problem/error/stop/config), ротация 1 МБ,
  `decisions.jsonl` (JSONL, 1 МБ), `messages.jsonl`;
* **21.5 Горячее перечитывание `config.yaml`** — что применяется на ходу (`agent.sync.*`,
  `index.max_file_mb`, `agent.enabled:false`), строка `config: reload …`, что требует
  перезапуска (`agent.root`, `max_live_projects`), недописанный файл игнорируется;
* **21.6 Обслуживание** — «Копии…»/«Вернуть» (`restore [--stamp]`), `rebuild` для `orphan`,
  `relink` только по команде, `project rm` (что удаляется/остаётся, `--force`,
  `--purge-meta`), `hds agent init` после удаления;
* **21.7 Безопасность и ограничения** — server-профиль 403 (D20), секреты только
  «задан/нет», `agent-state` содержит историю до очистки (AG-32 — предупреждение),
  журналы без значений (I8).

### 3.5 `RELEASE_NOTES_0.27.0.md`

Заголовок «0.27.0 — эксплуатация зоны агента (фаза A6)», дата, база 0.26.0, разделы:
автозапуск (`--all`, задача), диагностика (`hds agent check`), горячий конфиг, откат и
журнал (`restore/rebuild/relink`, `decisions.jsonl`), удаление проекта, эксплуатационные
исправления (AG-23/25/26/32), документация (README §20/§21, `AGENT_A6_REPORT.md`).
В шапке — примечание: «черновик; приёмка A6.7 (живой E2E) — отдельный прогон».

### 3.6 `STATUS.md`

Файл описывает прежние ветки (Python-эпоха) и **не переписывается**. Добавить в начало
(сразу после заголовка/вводных цитат) компактный блок «Зона агента A0–A6 (21.11.2026)»:
HEAD `beff5521`, что закрыто (A0–A6.6, одной строкой на под-этап), тесты среза,
что осталось (A6.7-прогон + заполнение `A6_E2E.md`, A6.8/K1), что дальше — фаза 2 по
`PLAN_AGENT_PRIVACY_ZONE.md` §8. Без значений из меток.

## 4. Сценарий E2E детально (что должен делать `a6_e2e.ps1`)

Стенд (как в A5.7): `$stand = tools\parity\out\agent_a42`, `$env:HDS_CONFIG = $stand\config.yaml`,
`$env:HDS_ROOT = $stand\root`, проект `$stand\project`, зона `$stand\zone` (P0001),
`$hds = target\debug\hds.exe`, `$probe = $stand\a57_probe.py`,
`$forms = $stand\a57-forms.json`. Порты: UI стенда **8799**, MCP **8788** (8787 занят).
Внешний провайдер не нужен: ход агента в сценарий A6.7 не входит.

1. **Preflight (только в этом чате).** Проверить пути/порты, вывести план, `exit 0`.
2. **До прогона (в полном режиме).** `index.stop`: запомнить, был ли файл; удалить
   `agent-state\.cline\data\sessions`, `...\db`, `egress` (следы прежних прогонов,
   как `a57_clean.ps1`); `hds agent check --project P0001 --json` → `a6-check-before.json`;
   `hds agent project list --json` содержит P0001.
3. **Задача Планировщика.** `install_agent_sync_task.ps1 -TaskName $task -Status`
   (ожидаемо «[--] task is not registered»), затем регистрация (без `-Start`), `-Status`
   (ожидаемо «[ok] task exists» и действие `hds agent sync --all`), затем `-Remove`
   (ожидаемо «[ok] removed»). Если Планировщик недоступен — тот же цикл через
   `-StartupFolder` (ярлык «Автозагрузка») и `-Remove`; в отчёте отметить, какой путь сработал.
   `-Start` **не** вызывать: демон для следующего шага запускается отдельно и управляемо.
4. **Демон.** `Start-Process $hds -ArgumentList agent,sync,--project,P0001,--interval,1
   -PassThru`, ждать `sync.lock` ≤ 30 с; `hds agent check --project P0001 --json` →
   пункт демона `ok` (id пункта **взять из ответа**, не хардкодить). Правка файла проекта
   (дописать строку) → ждать в `sync.log` `pass … changes>=1` ≤ 120 с (первый проход
   поднимает воркер — до ~60 с) и обновлённый образ.
5. **Горячий конфиг (A6.3).** Сохранить копию `config.yaml`; заменить
   `agent.sync.interval_sec` на `2` (файл писать атомарно: temp + `Move-Item`), ждать
   `config: reload interval=2s` в `sync.log` ≤ 90 с. Вернуть исходный `config.yaml`,
   ждать `config: reload interval=<исходный>s`. Проверить, что `agent.root` не меняется
   на живой сессии (`root ignored` — только предупреждение).
6. **Остановка.** Записать `index.stop` (если его не было): `hds stop`; ждать выхода
   процесса демона ≤ 10 с, `sync.lock` снят; `index.stop` **остался** (AG-27: демон его
   не удаляет) — зафиксировать в отчёте; после сценария удалить только тот `index.stop`,
   который создал сам тест (если до него файла не было).
7. **Решения (A6.4).**
   * вариант: правка **образа** (`workspace\<…>`) → `hds agent sync --once` →
     `pending_accept` (`GET /api/agent/pending` или `hds agent result` без действия — код 2
     со списком);
   * `hds agent result accept --project P0001 --path <orig_rel>` → 0, `backup\<stamp>`
     появился, образ пересоздан, `decisions.jsonl` содержит `"action":"accept"`;
   * `hds agent result restore --project P0001 --path <orig_rel> [--stamp <stamp>]` → 0,
     sha оригинала = sha копии, образ пересоздан, журнал `"action":"restore"`;
   * `rebuild`: удалить образ → `hds agent sync --once` ставит `orphan` (ветка
     `row.anon_sha256_issued.is_some()`), затем `hds agent result rebuild …` → 0, статус
     `synced`, образ на месте, журнал `"action":"rebuild"`;
   * `relink`: на стенде поставить статус `error` **только в реестре стенда** (sqlite3 из
     python венва: `UPDATE artifacts SET status='error' WHERE orig_rel=?`), образ с
     собственным маркером `hds-agent/P0001/<id>` → `hds agent result relink …` → 0,
     `synced`; журнал `"action":"relink"`. Альтернатива без правки БД — зафиксировать
     отказ `NotApplicable` на `synced` и сослаться на unit-тест
     `relink_by_marker_only_on_command`;
   * проверить отказы: неизвестный `--stamp` → 1 с списком доступных; `restore` без копий —
     понятный текст; `rebuild` на `synced` — 409/1.
8. **Удаление проекта (A6.5).** Запустить демон снова (`--interval 1`), дождаться lock,
   `hds agent project rm P0001 --yes` → **1**, текст содержит «демон зеркала» и `hds stop`;
   остановить демон (`hds stop`, дождаться выхода); `hds agent project rm P0001 --yes`
   → **0**; повтор → **2**. Проверить: `<zone>\P0001` нет, `registry\P0001.db` нет,
   `projects.json` без записи, `secrets.json` без `/projects/P0001` (ключи провайдеров и
   `mcp` целы), **папка проекта цела** (sha всех файлов до/после), `index.stop` не тронут.
   `hds check --json` и `hds agent check --json` не падают без проекта.
9. **I1/I8-сканы (критерий A6.7).** `a57_probe.py scan`:
   * `<zone>` с `--skip=agent-state,agent-config` и `--nopath` — сравнить с базой A5.7
     (`a57-scan-zone-before.txt`: 22 попадания в 12 файлах — это **образы**, ожидаемо);
     новых попаданий быть не должно;
   * `decisions.jsonl`, `sync.log`, `.hds-agent\messages.jsonl`, `agent-state\egress`,
     `projects.json`, ответы `/api/agent/{state,files,pending,settings}` — `hits=0` (I8).
   Формы — из `a57-forms.json` (метки стенда; обновлять `forms` из `anonymize.db` не нужно,
   значения не печатать).
10. **Финал.** Снять задачу (`-Remove`), остановить все запущенные процессы, вернуть
    `config.yaml`, признать `a6-checks.txt` итогом (`OK/FAIL`), `-KeepRunning` — оставить
    стенд, но не задачу.

## 5. Методика «без живого прогона» (обязательна в этом чате)

1. Написать `a6_e2e.ps1` и **проверить синтаксис без запуска**:
   `powershell -NoProfile -Command "$e=$null; [void][System.Management.Automation.Language.Parser]::ParseFile('<путь>',[ref]$null,[ref]$e); $e.Count"` → `0`.
2. Реализовать и **прогнать только `-Preflight`** (он не трогает стенд): вывод записать в
   `A6_E2E.md` §3. Если preflight нашёл проблему (нет бинарника/воркера/портов), это
   фиксируется как «прогон пока невозможен» — скрипт всё равно сдаётся.
3. `A6_E2E.md` — только каркас: статус «не выполнялся», пустые «результат», никаких
   выдуманных чисел/OK.
4. `AGENT_A6_REPORT.md`: факты A6.1–A6.6 (коммиты и прогоны §0) — можно; про A6.7 —
   «сценарий и документы подготовлены, приёмка не пройдена».
5. `RELEASE_NOTES_0.27.0.md` — черновик с пометкой; не выпускать релиз.
6. Проверить документы: заголовки/нумерация (README §21/§22, «Содержание»), ссылки на
   существующие файлы (`Test-Path`), `git --no-pager diff --check`.

## 6. Грабли (оплачены, не наступать)

1. **PS 5.1 + кириллица:** `.ps1` — UTF-8 **с BOM** (a57_e2e.ps1 — образец); ASCII-only —
   только установщики (как `install_agent_sync_task.ps1`). `param()` — первой инструкцией.
2. **`& native 2>&1` при `$ErrorActionPreference='Stop'`** даёт `NativeCommandError` —
   в `RunNative` локально ставить `Continue` (приём `a57_e2e.ps1`).
3. **`Invoke-RestMethod` декодирует JSON как Latin-1** — читать
   `RawContentStream`/`[Text.Encoding]::UTF8.GetString` (a57 `GetJson`/`PostJson`).
4. **Порт 8787** занят службой машины — стенд 8788/8799; проверять `Get-NetTCPConnection`.
5. **`HDS_ROOT`/`HDS_CONFIG`** — только внутри процесса скрипта; перед `cargo test`
   снимать переменные (иначе тесты идут по стенду).
6. **Задача Планировщика** — уникальное имя и `-Remove` в `finally`; в обычных прогонах
   не регистрировать. При недоступном Планировщике срабатывает fallback «Автозагрузка»
   (`hidden_launch.vbs`) — это не ошибка, отметить в отчёте.
7. **`index.stop` — общий** (AG-27): демон его не снимает; `hds stop` пишет файл.
   Не удалять чужой: запомнить состояние до сценария и убирать только созданный тестом.
8. **`agent-state` — в зоне** (история SDK, `*.bak`, `.cline`): в сканах I1 исключать;
   до очистки история содержит открытые значения (AG-32) — это `warn`, не `fail`.
9. **Воркер анонимизации** — настоящий (`stand\root\sidecar\venv-anonymize`, GLiNER);
   первый проход поднимает его ~30–60 с. Если воркера нет — `-Preflight` это покажет,
   полный прогон не запускать.
10. **Флейк `agent_chat::abort_cancels_pending_ask`** — предсуществующий (падает и на
    `0611159f`); одиночным прогоном / с оговоркой в отчёте, не «лечить» в A6.7.
11. **`os error 1455`** при `cargo build/test` на Windows — повторить с `-j 2`.
12. **Секреты и значения** — в вывод, журналы и `A6_E2E.md` не попадают (I7/I8);
    `secrets.json` в отчёте — только «запись есть/нет».

## 7. Проверки и команды

| Что | Команда |
|---|---|
| Формат / линт | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` |
| Rust-тесты | `$env:HDS_ROOT=$null; $env:HDS_CONFIG=$null; cargo test -p hds-agent; cargo test -p hds-cli; cargo test -p hds-ui --no-fail-fast` (известный флейк — оговорка) |
| Python / Node | `sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar`; `npm --prefix sidecar/hds_agent test` |
| Скрипт | `[Parser]::ParseFile(...)` → 0 ошибок; `a6_e2e.ps1 -Preflight` → 0 (без изменений на стенде) |
| Документы | заголовки `^## ` в README, «Содержание», `Test-Path` для ссылок, `git --no-pager diff --check` |
| Срез | `git status --short` (чисто), `git log --oneline -1` |

**Коммит:** `docs(agent): A6.7 — сценарий E2E и документация A6 (без прогона)`.
Push не делать. Артефакты стенда (`a6-*`) не коммитить — `tools/parity/out` гитигнорится;
в репозиторий идут `AGENT_A6_7_HANDOFF_PROMPT.md` (этот файл уже в репозитории),
`AGENT_A6_REPORT.md`, `RELEASE_NOTES_0.27.0.md`, README, `STATUS.md`.

## 8. Промт для нового чата (копировать целиком)

```text
Ты — разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Задача — под-этап A6.7 плана AGENT_A6_STAGES.md: приёмка A6 и документация.
ВАЖНО (ограничение владельца): в этом чате — БЕЗ живого прогона E2E. Нужно
подготовить сценарий, каркас отчёта, документацию и черновик заметок выпуска;
сам прогон стенда — отдельная задача, критерий A6.7 («сценарий проходит целиком»)
остаётся открытым.

Сначала прочитай целиком AGENT_A6_7_HANDOFF_PROMPT.md (контекст, контракты,
сценарий, методика, грабли, проверки), затем AGENT_A6_STAGES.md (§2 A6.7,
§3 В-A6-8, §4 контракты, §6 грабли), AGENT_A5_REPORT.md и AGENT_A4_REPORT.md
(структура отчёта), tools/parity/out/agent_a42/a57_e2e.ps1 и A57_E2E.md
(образец E2E), a46_e2e.ps1/A46_E2E.md (демон), installers/install_agent_sync_task.ps1,
RELEASE_NOTES_0.26.0.md, README (§20, §21 «Что ещё почитать», «Содержание»)
и шапку STATUS.md. Срез: ветка agent-privacy-zone, HEAD beff5521
(A6.1–A6.6 закрыты), дерево чистое.

Сделай:
1. tools/parity/out/agent_a42/a6_e2e.ps1 — сценарий A6.7 (UTF-8 с BOM, PS 5.1):
   -Preflight без побочных эффектов; установка задачи Планировщика с уникальным
   именем и -Remove в finally (в т.ч. fallback «Автозагрузка»); демон
   `hds agent sync --project P0001 --interval 1`; горячая правка config.yaml
   (agent.sync.interval_sec=2) и строка `config: reload interval=2s` в sync.log;
   `hds stop` (index.stop не удалять — AG-27); restore/rebuild/relink через
   `hds agent result` и их отказы; `project rm` с живым демоном (код 1) и после
   стопа (0, повтор 2); сканы I1/I8 через a57_probe.py scan; артефакты a6-*.
   Полный сценарий НЕ запускать; прогнать только -Preflight и проверить синтаксис
   через [System.Management.Automation.Language.Parser]::ParseFile.
2. tools/parity/out/agent_a42/A6_E2E.md — каркас: статус «прогон не выполнялся»,
   таблица шагов/ожиданий с пустой колонкой «результат», артефакты, фактический
   вывод -Preflight, раздел «как прогнать», «осталось до приёмки».
3. AGENT_A6_REPORT.md — отчёт фазы A6 по A6.1–A6.6 (коммиты, что «снаружи»,
   тесты среза beff5521), раздел «A6.7 подготовлен, приёмка не пройдена».
4. README.md — новая глава §21 «Эксплуатация зоны агента» (см. §3.4 хендоффа),
   «Что ещё почитать» → §22, обновить «Содержание».
5. RELEASE_NOTES_0.27.0.md — черновик заметок A6 с пометкой о непройденной приёмке.
6. STATUS.md — компактный блок «Зона агента A0–A6» (HEAD, что закрыто, тесты,
   что осталось), старое содержимое не переписывать.

Не делать A6.8/K1, не выпускать релиз, не делать push, не менять решения плана.
Не выдавать подготовку за приёмку: в A6_E2E.md не должно быть выдуманных OK/чисел.
Секреты и значения из базы меток в документы, вывод и журналы не попадают (I7/I8).
Прогоны: cargo fmt --all -- --check; cargo clippy --workspace --all-targets --
-D warnings (при os error 1455 — -j 2); cargo test -p hds-agent / -p hds-cli /
-p hds-ui (известный предсуществующий флейк agent_chat::abort_cancels_pending_ask —
оговорка в отчёте); python и npm; проверка скрипта парсером и `-Preflight`;
`git --no-pager diff --check`.
Коммит: docs(agent): A6.7 — сценарий E2E и документация A6 (без прогона).
Push не делать.
```

**После отдельного прогона (другой чат):** выполнить `a6_e2e.ps1`, заполнить
`A6_E2E.md` (колонка «результат», итог `N/N ok`), обновить `AGENT_A6_REPORT.md`
(§ «Приёмка A6.7») и `STATUS.md`, подтвердить I1/I8 сканами `decisions.jsonl` и
`sync.log`, затем коммит `docs(agent): A6.7 — приёмка A6 (E2E пройден)`.
