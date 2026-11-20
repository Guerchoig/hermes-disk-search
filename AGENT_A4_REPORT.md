# AGENT_A4_REPORT.md — отчёт фазы A4 (двустороннее зеркало папки проекта)

> **Дата:** 20.11.2026. **Ветка:** `agent-privacy-zone`. **Журнал фазы:**
> `AGENT_A4_STAGES.md` (под-этапы, решения, долги, грабли). **Стенд:**
> `tools\parity\out\agent_a42` (34 записи реестра: 33 образа + 1 `unsupported`).
> **Отчёты стендов:** `A43_E2E.md`, `A44_E2E.md`, `A45_E2E.md`, `A46_E2E.md`
> (в каталоге стенда, он гитигнорится).
>
> **Фаза A4** — зеркало `hds agent sync`: папка проекта пользователя и
> `<agent.root>\P<NNNN>\workspace` — одно дерево в двух видах. Пользователь
> работает с оригиналами, агент — только с обезличенными образами; оригиналы не
> меняются без решения пользователя.

## 1. Итог фазы

| Под-этап | Коммит | Что «снаружи» |
|---|---|---|
| **A4.1** Реестр артефактов `data\agent\registry\<P>.db` (TR-7.2) + минимальный `sync --once` | `00da7c54` | реестр создаётся, `--once` на пустой папке ничего не ломает, повторный прогон идемпотентен, `--project` не идентификатор (D21) — код 2 |
| **A4.2** Прямой путь (TR-7.1/7.3) | `81961936` | структура папок повторена, имена с метками без падежа, docx/xlsx/txt/md/xml → образ, pdf/pptx/mpp → `*.pdf.md`, `unsupported`/`too_big`, удаление и переименование; повторный проход — 0 изменений |
| **A4.3** Обратный путь (TR-7.5/7.7) | `d84571ee` | `agent_new` → папка проекта, `agent_changed` → вариант в `.hds-agent\pending` + снимок основания, маркер `hds-agent/<P>/<id>`, защита от петель `issued`/`seen` |
| **A4.4** Решения (TR-7.6) | `43305867` | «Принять» (backup → замена → образ), «Отклонить» (сообщение агенту), три исхода конфликта, «файл занят» без потерь, `messages.jsonl` |
| **A4.5** Валидатор (TR-7.8) | `e676bb89` | `pending\<rel>.report.json`: метки вне проекта, падеж, `[IMAGE_n]`, «текст пропал» > 40 %; `verdict: check` не блокирует; отчёт удаляется решением |
| **A4.6** Демон, CLI, документация | feat + docs этого этапа | `--interval SEC` (дефолт `agent.sync.interval_sec` = 5 с), остановка `hds stop`/Ctrl+C, `sync.lock`, журнал `agent-state\sync.log`; README §15/§20, этот отчёт, `RELEASE_NOTES_0.26.0.md` |

Критерии приёмки A4 (хендофф §6) выполнены: см. §4 (e2e) и §5 (проверки).


## 2. Что сделано по под-этапам

### A4.1 — реестр

* `crates/hds-agent/src/registry_db.rs`: SQLite (WAL, `busy_timeout`),
  `user_version`; записи `Artifact` (`orig_rel`, `anon_rel`, `kind`, `status`,
  `artifact_id`, `orig_sha256`, `anon_sha256_issued`, `anon_sha256_seen`);
  статусы `synced`/`pending_accept`/`conflict`/`unsupported`/`too_big`/`orphan`/
  `accepted`; `insert`/`upsert`/`mark_seen`/`set_issued`/`set_baseline`/
  `list_pending`/`by_orig`; конкурентная запись из двух потоков.
* CLI `hds agent sync --project <P> --once`: проверка `agent.enabled`, проекта в
  `projects.json`, идентификатора (D21), открытие реестра.

### A4.2 — прямой путь

* `crates/hds-agent/src/mirror.rs`: `walk_files` (детерминированно, junction не
  обходится, `.hds-agent`/`_attachments`/`_conflicts`/`~$*`/`*.tmp`/`Thumbs.db`
  пропускаются), `kind_for`, `wait_stable_at` (2 с / 0,5 с / 30 с +
  `settle_age_sec`), `fit_component` (≤ 120 символов + 8 hex sha256),
  `unique_name` (` (2)`, ` (3)`), `sha256_file`.
* Идемпотентность по sha256: `orig_sha256` (оригинал) и
  `anon_sha256_issued`/`seen` (образ); совпало — `mark_seen` только при
  изменении `seen`.
* RPC `anonymize_path(project_id, path, suffix=false)` — NER по компонентам,
  `project_label_ex(suffix=False)`, санитайзинг; `suffix=true` — отказ (в имени
  Windows нельзя `|`).
* Ленивые воркеры `LazyAnonymizer`/`LazyExtractor` — Python поднимается только
  при первом файле, которому он нужен.

### A4.3 — обратный путь

* Маркер `hds-agent/<P>/<id>`: в `docProps` (docx/xlsx) и **первой строкой**
  текстовых образов (`<!-- hds-agent/P/id -->`), при деанонимизации снимается.
* `agent_new` → файл в папке проекта (`free_project_name`, суффикс
  ` (агент)` при занятом имени), маркер не уезжает.
* `agent_changed` → вариант в `.hds-agent\pending\<rel>`: деанонимизация
  (`deanonymize(restore_from=<base>, images=true)`), восстановление `docProps` и
  картинок из `.hds-agent\base`, `…images.json`.
* Защита от петель: `issued` (что выпустили) против `seen` (что видели); наш
  маркер с неизвестным `artifact_id` не выпускается повторно (в `problems`).

### A4.4 — решения

* `hds_agent::results` + `hds agent result accept|reject|conflict`:
  * `accept`: backup в `.hds-agent\backup\<stamp>\` → `replace_file` →
    `accepted` → образ из нового оригинала (`set_baseline`) → очистка
    `pending`/`base`;
  * `reject`: образ восстанавливается из оригинала, в `messages.jsonl` —
    `document_rejected`;
  * `conflict --choice agent|new|delegate`: три исхода, `delegate` копирует
    образ в `<P>\workspace\_conflicts\<имя> (вариант агента)<ext>`;
  * занятый файл (`PermissionDenied`/`os error 32`) → `ResultsError::Busy`,
    ничего не меняется, backup не создаётся, вариант остаётся в `pending`.

### A4.5 — валидатор

* `hds_agent::validator`: `Report {rel, anon_rel, ts, rev, verdict, checks}`
  рядом с вариантом; четыре проверки — `labels_outside_project`,
  `label_grammar`, `unplaced_images`, `text_lost` (> 40 %);
* пишется после `set_issued(seen, PendingAccept)`, **ничего не блокирует**;
  сбой валидатора → `problems` + `verdict: check`; удаляется вместе с вариантом
  (`results::cleanup_decision`), перезаписывается при пересборке.

### A4.6 — демон, CLI, документация

* `crates/hds-cli/src/cmd/agent_sync.rs`:
  * `parse_args` — `--project <P>`/`--project=<P>`, `--once`, `--interval SEC`/
    `--interval=SEC`; `0`, отрицательное, не число, `--interval` без значения и
    `--once` + `--interval` → код 2;
  * `SyncLog` — `<agent.root>\P<NNNN>\agent-state\sync.log`; строки `start`
    (режим, проект, интервал, папки), `pass` (все счётчики `SyncReport` +
    `changes` + `problems`), `problem` (не больше 10 + «ещё N»), `error`, `stop`;
    ротация > 1 МБ в `sync.log.1` при старте; пишется и в `--once`;
  * `run_loop` — проверка сигнала → проход → сон интервала шагами ≤ 500 мс;
    `max_passes` для тестов; `PassResult::Failed` не убивает демон;
  * lock демона — `WatchLock::acquire_named(<P>\agent-state, "sync.lock")`:
    занят → «уже запущен», код 0; каталог `agent-state` создаётся **до** lock
    (`create_new` на отсутствующем каталоге неотличим от «занято»);
  * порядок остановки: цикл → `drop` воркеров (Python) → `sync.lock` →
    снять `index.stop` → строка `stop`.
* Тесты: 7 unit (`cmd::agent_sync::tests`/`loop_tests`) + 4 интеграционных
  (`crates/hds-cli/tests/agent_sync.rs`): аргументы, `max_passes=2` (две строки
  `pass` и `stop`, не висит), сигнал посреди сна 60 с (остановка за мгновение),
  ошибка прохода не убивает цикл, ротация, «одна запись — одна строка»,
  ≤ 10 проблем, занятый `sync.lock`, демон до `index.stop`, журнал без значений
  на mock-воркере.
* README: §15 — команды `sync` и `result`; §20 — «Зеркало папки проекта»,
  «Вариант, отчёт и решение», «Режим демона», таблица `agent.sync`,
  «Что не зеркалится». `RELEASE_NOTES_0.26.0.md` — короткая запись фазы.

## 3. Контракты фазы (не ломать)

Полные формулировки — `AGENT_A4_STAGES.md` §5. Коротко:

| Контракт | Правило |
|---|---|
| Метки | разбор только `hds_core::labels`; в именах файлов `|` запрещён → метка **без** падежного суффикса |
| Имена файлов | только RPC `anonymize_path(..., suffix=false)`; Rust режет до 120 символов + 8 hex sha256 и разводит коллизии ` (2)` |
| Режим `agent` | `mode:"agent"` + `project_id` (+ `artifact_id`), маркер `hds-agent/<P>/<id>`, картинки тела `strip` → `[IMAGE_n]`, оригинал не меняется |
| Текстовые образы | `<имя>.<ext>.md`, только чтение (`hds-guard`), маркер первой строкой, правка — новым документом (TR-7.3a/D22) |
| Обратный путь | `read_marker`, `deanonymize_path`, `image_map`, `deanonymize(restore_from=<base>, images=true)`; маркер своего проекта повторно не выпускается |
| Решения | `Mirror::decide`: backup → `replace_file` → `accepted` → `set_baseline`; занятый файл — `ResultsError::Busy`, ничего не меняется |
| Валидатор | `pending\<rel>.report.json` пишется после `set_issued`; ничего не блокирует; удаляется `cleanup_decision` |
| Сообщения агенту | `.hds-agent\messages.jsonl`, только метки и пути |
| Журналы | `agent_egress_redacted` и `sync.log` — пути/метки/статусы, без значений (I8) |
| Дата-время | одна реализация — `hds_core::timefmt` |
| Атомарная замена | `hds_core::config::replace_file` (ретраи), между дисками — copy+replace |

## 4. Инварианты на стенде

| Инвариант | Как проверен |
|---|---|
| **I1/I2** — значения не покидают машину | в `<agent.root>` только метки (образы); варианты с значениями лежат в папке проекта (зона пользователя, `.hds-agent`), а не в зоне; `sync.log` и `messages.jsonl` значений не содержат. A4.6: `Select-String` по журналу демона — 0 совпадений текста правок; CLI-тест `sync_journal_has_paths_but_no_values` (mock-воркер метит «Иванов Иван», в журнале его нет) |
| **I4** — профиль пользователя не меняется | зеркало не запускает Node и агента (это `hds agent run`); `%USERPROFILE%\.cline` не трогается — приёмка A3 (снимок каталога) остаётся в силе |
| **I6** — оригинал без решения не меняется | A4.3/A4.4 e2e: sha256 оригинала до и после правки образа равны; `accept` меняет оригинал только после копии в `backup`; занятый файл — ничего не меняется (`accept_on_busy_file_keeps_everything_as_is`) |
| **I8** — в журналах нет значений | `sync.log`: пути, метки, статусы, числа; `messages.jsonl`: метки и пути; `validator` `detail` — метки; проверено тестами и e2e (A45/A46) |

## 5. E2E на стенде `agent_a42`

| Отчёт | Что проверено | Результат |
|---|---|---|
| `A43_E2E.md` | обратный путь: 32 образа, `agent_new` → папка проекта, `agent_changed` md/docx → `pending` + `base`, повторные проходы — 0 изменений, `_attachments`/`_conflicts`/`.hds-agent` не выпущены | пройдено |
| `A44_E2E.md` | «Принять» (backup, замена, образ пересоздан), «Отклонить» (образ = оригинал, `document_rejected`), «файл занят» (код 1, вариант на месте, backup не создан), повторный проход — 0 | пройдено; три исхода конфликта — тесты `results::tests` (на стенде требуют двух версий документа) |
| `A45_E2E.md` | `pending\<rel>.report.json`, `verdict: ok` и `check`, `check` не блокирует, отчёт удаляется решением, в отчёте нет текста документа | пройдено |
| `A46_E2E.md` | демон `--interval 1` без перезапуска подхватывает правку файла (14,45 с — холодный старт GLiNER) и правку образа (1,03 с); `hds stop` — 1,35 с, `index.stop`/`sync.lock` сняты; журнал 9 строк без значений; повторный `--once` — 0 изменений | пройдено |


## 6. Проверки (A4.6)

| Что | Команда | Результат |
|---|---|---|
| Формат | `cargo fmt --all -- --check` | чисто |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` | чисто, `exit=0` |
| Тесты Rust | `cargo test --workspace` | **123 набора / 855 passed / 0 failed / 13 ignored** (срез A4.5 — 844: +11 тестов A4.6) |
| Тесты зеркала | `cargo test -p hds-cli --test agent_sync` | 10/10 |
| Node-воркер | `npm --prefix sidecar/hds_agent test` | **23/23** (прогон параллельно с `cargo test` один раз упёрся в таймаут `abort.test.mjs` — нагрузка машины; отдельный прогон 23/23) |
| Python | `sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar` | **328 OK** (skip 1) |
| E2E | `tools\parity\out\agent_a42\a46_e2e.ps1` | см. §5 и `A46_E2E.md` |

Логи: `tools/parity/out/a46-cargo-test.log`, `a46-npm-test.log`,
`a46-python-test.log`; артефакты e2e — в каталоге стенда (`a46-*.log`,
`A46_E2E.md`).

## 7. Что осталось

**A5 (вкладка «Агент»)** — кнопки «Принять/Отклонить/Конфликт» поверх
`hds_agent::results`, показ `pending` и `report.json`, доставка `messages.jsonl`
в сессию, «Сравнить», «Вернуть» из `backup`, журнал решений,
`hds agent result report --json`, relink по маркеру, «восстановить образ» для
`orphan`, обрезка длинных имён и в папке проекта.

**A6 (автозапуск)** — служба/Планировщик для `hds agent sync --interval`,
проверка Node в `hds check`, решение по `stop` при Ctrl+C (§4.2 п.29),
перечитывание конфига без перезапуска (§4.2 п.30).

**Фаза 2 плана** (`PLAN_AGENT_PRIVACY_ZONE.md` §8) — подписка на изменения ФС
вместо опроса, кэш sha по (size, mtime), кэш извлечения текста в валидаторе,
`--all` по проектам, Pillow/пакетное восстановление картинок xlsx.

## 8. Долги фазы (§4.2 журнала)

* **A4.1**: проект обязателен, `--all` нет — п.2.
* **A4.2**: Pillow для xlsx-картинок (п.10); смена расширения — не переименование
  (п.9); правила обхода зеркала отдельно от `index_filter` (п.7); путь к sidecar
  от `project_root()` (п.6).
* **A4.3**: маркер у образов, созданных до A4.3 (п.16); `agent_new` текстового
  образа (п.12); маркер без записи реестра → relink (п.13); длинные имена
  в проекте (п.14); `orphan` без «лечения» (п.15).
* **A4.4**: «Сравнить» (п.17); «Вернуть» из `backup` (п.18); журнал решений
  (п.19); конфликт без собранного варианта (п.20); приём текстовых образов
  запрещён (п.21); вторая копия `civil_from_days` в `hds-ui` (п.22).
* **A4.5**: `hds agent result report` (п.23); `Snapshot.others` в проверках не
  используется (п.24); `text_lost` по множествам слов (п.25); mock-воркер и
  `[PERSON_2]` (п.26); три извлечения текста на вариант (п.27).
* **A4.6**: журнал в зоне агента и пути пользователя (п.28); Ctrl+C без строки
  `stop` (п.29); конфиг не перечитывается (п.30); полный обход по таймеру и
  глубина ротации журнала (п.31); общий `index.stop` с индексатором (п.32).

Закрыто в A4.6: п.1 (`--interval`, цикл и журнал). Ссылки на A4.6 в п.8, 16, 27
перенесены на фазу 2 — в A4.6 они не делались. Грабли, оплаченные этой
доработкой, — §6 журнала, пп.29–32.

