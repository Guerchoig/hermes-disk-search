# AGENT_A4_6_HANDOFF_PROMPT.md — контекст и промт для под-этапа A4.6 (демон, CLI, документация)

> **Кому:** новый чат Cline в режиме Act. **Дата среза:** 20.11.2026.
> **Ветка:** `agent-privacy-zone`, **HEAD кода:** `b2ae8552` (`docs(agent): A4.5
> закрыт`); сам этот файл — docs-коммит поверх (сверить `git log -1 --oneline`),
> `origin/agent-privacy-zone` ahead 65, дерево чистое, push не делался.
> **Что это:** самодостаточный контекст для продолжения **фазы A4** основного
> проекта `PLAN_AGENT_PRIVACY_ZONE.md` («ИИ-агент в анонимной зоне»). A4.1–A4.5
> закрыты, **следующий и последний — A4.6: циклический режим `hds agent sync`,
> остановка, журнал, README, отчёт фазы**. Кнопки вкладки «Агент» — фаза **A5**,
> автозапуск демона — **A6**, подписка на изменения ФС — **фаза 2** плана (§8).

## 0. Где мы (коротко)

Пользователь работает с документами в своей папке, агент — только с анонимными
образами в `<agent.root>\P<NNNN>\workspace`; исходные значения не покидают машину
(I1), оригиналы не меняются без решения пользователя (I6), в журналах нет
значений (I8).

Фазы: **A0** спайки/методика, **A1** база меток v2 и `mode:"agent"`, **A2** индекс
и MCP-маршрут анонимной зоны, **A3** Node-воркер/крейт `hds-agent`/CLI агента —
приняты (`AGENT_A0_A3_CODE_REVIEW.md`). **A4** (текущая) — зеркало папки проекта
и workspace:

| Под-этап | Что делает | Состояние |
|---|---|---|
| A4.1 | Реестр артефактов `data\agent\registry\<P>.db` (TR-7.2) + `hds agent sync --once` | закрыт 19.11.2026 (`00da7c54`) |
| A4.2 | Прямой путь: обход, `wait_stable`, образы docx/xlsx/txt/md/xml, текстовые образы pdf/pptx/mpp, `unsupported`/`too_big`, удаление/переименование | закрыт 20.11.2026 (`81961936`) |
| A4.3 | Обратный путь: `agent_new` → проект, `agent_changed` → `.hds-agent\pending` + снимок основания, маркер, защита от петель `issued`/`seen` | закрыт 20.11.2026 (`d84571ee`) |
| A4.4 | Решения TR-7.6: «Принять», «Отклонить», три исхода конфликта, «файл занят», очередь `.hds-agent\messages.jsonl` | закрыт 20.11.2026 (`43305867`) |
| A4.5 | Валидатор TR-7.8: отчёт `pending\<rel>.report.json` — метки вне проекта, падеж, `[IMAGE_n]`, «текст пропал» > 40 % | закрыт 20.11.2026 (`e676bb89`) |
| **A4.6** | **Демон: `--interval` (дефолт `agent.sync.interval_sec` = 5 с), остановка по `index.stop`/Ctrl+C, журнал `agent-state\sync.log`, README с таблицей настроек, `AGENT_A4_REPORT.md`, `RELEASE_NOTES`** | **эта задача** |

Что уже работает end-to-end (контракт, не переделывать):

* `hds agent sync --project P --once` — один проход = прямой путь + обратный;
  повторный прогон по неизменённой папке — **0 изменений**;
* образы сохраняют структуру папок и получают имена с метками
  (`Протоколы\Встреча с [PERSON_3].docx`), `pdf/pptx/mpp` → `*.pdf.md`;
* правка образа агентом → вариант `.hds-agent\pending\<rel>` (деанонимизированный,
  с восстановленными `docProps`/картинками из `.hds-agent\base\<rel>`) + **отчёт
  валидатора** `<rel>.report.json` и статус `pending_accept`; оригинал не
  меняется (I6);
* решения: `hds agent result accept|reject|conflict --project P --path <rel>
  [--choice agent|new|delegate]` — ядро `hds_agent::results`; отчёт удаляется
  вместе с вариантом;
* уведомления агенту — `.hds-agent\messages.jsonl` (`document_rejected`,
  `conflict_delegated`), только метки и пути (I8); доставка в сессию — A5.

**Зачем A4.6.** Сейчас зеркало запускается только вручную: пользователь должен
помнить про `hds agent sync --once`. Демон делает зеркало фоновым — сохранил
документ, через несколько секунд образ в анонимной зоне; агент правит образ —
вариант и отчёт в `pending` появляются сами. Журнал `sync.log` — то, что читают
при разборе «почему образ не обновился» (и что покажет вкладка «Агент» в A5).
README — единственное место, где объяснён смысл настроек `agent.sync.*` (В-2), а
`AGENT_A4_REPORT.md` закрывает фазу A4 целиком.

**Границы A4.6.** Только опрос по таймеру (В-1: подписка на ФС — фаза 2 плана §8).
Автозапуск (Планировщик/служба) — фаза A6: демон запускается вручную и живёт в
терминале. UI — A5. Валидатор A4.5 и решения A4.4 по поведению **не меняются**.

## 1. Состояние репозитория (проверено 20.11.2026)

| Параметр | Значение |
|---|---|
| Ветка | `agent-privacy-zone`, HEAD кода `b2ae8552` (+ docs этого файла), ahead 65, дерево чистое, push не делался |
| Коммиты A4 | A4.1 `00da7c54`, A4.2 `81961936`, A4.3 `d84571ee`, A4.4 `43305867`, A4.5 `e676bb89` (+ docs после каждого) |
| Docs среза | `AGENT_A4_STAGES.md` (§2, §3.1–3.5, §4.2 п.17–27, §4.3, §5–§7, §8), `AGENT_A4_5_HANDOFF_PROMPT.md`, этот файл |
| Проверки A4.5 | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` — чисто; `cargo test --workspace` — **123 набора / 844 passed / 0 failed** (13 ignored); python `unittest discover -s sidecar/tests -t sidecar` — **328 OK** (skip 1); `npm --prefix sidecar/hds_agent test` — **23/23** |
| Стенд | `tools\parity\out\agent_a42` (32 документа): `pending`/`base` **пусты**, `sync` — `34 файла, 33 без изменений, 1 unsupported, 0 изменений`; в реестре 34 записи (часть `accepted`), в `messages.jsonl` два `document_rejected`; отчёты `A43_E2E.md`, `A44_E2E.md`, `A45_E2E.md`, логи `a45-*.log` |
| Машина | Windows 11, PowerShell 5.1, Rust 1.99, Node 24.18, Python 3.12 (`sidecar\venv-anonymize`), `llm_host` резидент |

Известный флейк: `agent_acceptance::acceptance_a3_end_to_end` снимает mtime
`~/.cline`, а IDE туда пишет — отдельный прогон зелёный; не пугаться. Второй
флейк, оплаченный в A4.5: сравнение `f64` после `serde_json` даёт расхождение на
1 ULP (грабля 26 в `AGENT_A4_STAGES.md` §6) — `ts` сравнивать с допуском.

## 2. Обязательное чтение (по порядку)

1. `AGENT_A4_STAGES.md` — **целиком**: §2 (карта под-этапов), §3.1–3.5 (что
   сделано и какие приняты отклонения), §4.2 (долги, особенно пп.17–27),
   §4.3 (**таблица смысла настроек `agent.sync`** — она переезжает в README),
   §5 (контракты), §6 (грабли 1–28), §7 (команды и стенд), §8 (этот контекст).
2. `AGENT_A4_HANDOFF_PROMPT.md` — §4.1 (структура зеркала и имена), §4.3/§4.4
   (прямой путь, маркер), §4.8 (без «Входящих»), §6 (**критерии готовности A4** —
   их закрывает A4.6), §7 (грабли фазы), §9 (вопросы В-1…В-11).
3. `PLAN_AGENT_PRIVACY_ZONE.md` — §5.7 (зеркало, четыре ситуации, валидатор),
   §7 (приёмка MVP: живой сценарий), **§8** (фаза 2: подписка на ФС — не сейчас).
4. Код: `crates/hds-cli/src/cmd/agent_sync.rs` (точка входа и `print_report`),
   `crates/hds-cli/src/cmd/agent.rs` (`stop_watch`, `init` — там же снимается
   `index.stop`), `crates/hds-index/src/watch.rs` (`run_watch`, `WatchLock`),
   `crates/hds-index/src/pipeline.rs` (стр. 928: кто снимает `index.stop`),
   `crates/hds-agent/src/mirror.rs` (`Mirror::sync_once`, `SyncReport`,
   `LazyAnonymizer`/`LazyExtractor`), `crates/hds-agent/src/settings.rs`
   (`sync_settings`, `state_dir_for`), `crates/hds-extract/src/worker.rs`
   (`request(retry_dead)` — перезапуск мёртвого воркера), `config.example.yaml`
   (блок `agent.sync`), `README.md` §15 и §20.

## 3. Что уже есть в коде — опора A4.6

| Что нужно демону | Где лежит | Как использовать |
|---|---|---|
| Точка входа | `crates/hds-cli/src/cmd/agent_sync.rs::cmd_sync` | сейчас: проверки `--project`/`agent.enabled`/папки, `ArtifactRegistry::open`, `Mirror::from_config`, **один** `sync_once()`, `print_report`, код 1 при `errors > 0` |
| Флаги | `unknown_flag` + `usage` там же | `--once` уже известен; `--interval` (со значением и `--interval=SEC`) надо добавить в `unknown_flag`, иначе код 2 |
| Один проход | `Mirror::sync_once(&mut self) -> Result<SyncReport, MirrorError>` | `Mirror` держит `&ArtifactRegistry` и `&mut dyn Anonymizer/TextExtractor` — создаётся **один раз** на демон, проходы идут через `&mut` |
| Реестр | `ArtifactRegistry::open(&db_path)` | открыть один раз; WAL + `busy_timeout` 30 с настроены (два писателя переживут) |
| Воркеры | `LazyAnonymizer` / `LazyExtractor`, фабрики `anonymizer_factory`/`extractor_factory` (`pub(crate)`) | Python поднимается лениво и живёт между проходами; после idle-выхода воркера (`anonymize.idle_timeout_sec`, по умолчанию 60 с) транспорт `hds_extract::Worker::request(..., retry_dead=true)` перезапускает процесс и повторяет запрос один раз — **проверить на стенде паузой > 60 с** |
| Счётчики | `SyncReport` (`scanned/created/updated/unchanged/unsupported/too_big/deferred/removed/renamed/orphans/agent_changed/conflicts/errors/emitted/variants/problems`), `changes()` | те же числа, что печатает `print_report`; `problems` — «файл: причина» без значений (I8) |
| Интервал | `settings::sync_settings(cfg).interval_sec` (u64) | дефолт 5 с, `0`/отрицательное → дефолт (A4.2); в A4.2 только читался, теперь используется |
| Каталог журнала | `settings::state_dir_for(cfg, project)` → `<agent.root>\P<NNNN>\agent-state` | создать `create_dir_all` при старте; каталог уже исключён из индекса (A2) и не зеркалится |
| Остановка | `<root>\index.stop` (`hds_core::config::project_root()`); `hds stop` создаёт его | образец — `run_watch` (опрос 500 мс, `stop_file.exists()` → break); `run_index` **снимает** файл после кооперативной остановки (стр. 928) — «сигнал отработан» |
| Lock демона | `hds_index::watch::WatchLock::acquire_named(root, "sync.lock", attempts)` | RAII, снимает устаревший lock по PID; так уже живёт `transcribe.lock` — два демона не мешают друг другу |
| Ctrl+C | в workspace нет `ctrlc`/`signal-hook` | `hds watch` на Ctrl+C просто завершается (деструкторы не отрабатывают); для зеркала так же: мягкая остановка — `index.stop`, Ctrl+C — жёсткая |
| README | `README.md`: §15 «Перечень команд» (стр. 1027), §20 «ИИ-агент в анонимной зоне» (стр. 1390) | глава A3 описывает зону, `secrets.json`, MCP, но `agent sync`/`agent result`/`.hds-agent`/`agent.sync` в ней **нет** (проверено `Select-String`) |
| Отчёт фазы | `AGENT_A4_REPORT.md` — **создать**; последний по времени — `RELEASE_NOTES_0.25.0.md` | материал: `A43_E2E.md`, `A44_E2E.md`, `A45_E2E.md`, §3 журнала; версия пакета `0.1.0`, имя нового файла — `RELEASE_NOTES_0.26.0.md` |

Важные следствия для реализации:

* **Один `Mirror` на демон.** Пересоздавать его каждый проход нельзя: `Mirror`
  владеет ссылкой на реестр и воркерами, а `from_config` каждый раз поднимал бы
  Python заново. Но если папка проекта исчезла/переехала, `sync_once` вернёт
  `Err` (`walk_files` падает без каталога — грабля 16) — цикл обязан **не

## 4. Задача A4.6 — что и как делать

### 4.1. CLI: `--interval` и цикл

Синтаксис: `hds agent sync --project <P> [--once | --interval SEC]`.

Правила (сверить с `usage()` и `unknown_flag`):

* **без флагов — один проход**, как сейчас: скрипты, тесты и `hds agent init`
  рассчитывают на завершение процесса. Демон включается **только** явным
  `--interval SEC` (ошибка «повисший процесс» дороже, чем лишний флаг);
* `--once` — один проход (совместимо с A4.1–A4.5);
* `--once` вместе с `--interval` — код 2 и понятный текст («или один проход, или
  цикл»);
* `--interval` без значения, `--interval 0`, отрицательное, не число — код 2;
  подсказка: «период по умолчанию — `agent.sync.interval_sec` (5 с), значение
  задаётся секундами»; форму `--interval=SEC` поддержать (как `--project=`);
* дефолт интервала — `settings::sync_settings(cfg).interval_sec`; печатать его
  при старте, чтобы пользователь видел, откуда взялось 5 с.

Структура кода:

* цикл — отдельная функция (`run_daemon`/`run_loop`) с инъекцией проверки
  остановки (`should_stop: impl Fn() -> bool`) и, для тестов, ограничением числа
  проходов (`max_passes: Option<usize>`). Иначе тест демона либо повиснет, либо
  потребует потоков и env-локов (CLI-тесты держат `ENV_LOCK`);
* `Mirror`, `ArtifactRegistry`, `LazyAnonymizer`, `LazyExtractor` создаются
  **один раз** до цикла и живут до остановки;
* проход: `sync_once()` → печать (тот же `print_report`, что и сейчас) → строка в
  журнал → сон интервала **дробными шагами** (200–500 мс), чтобы `index.stop`
  замечался быстро и не ждал полный интервал;
* `Err` прохода (папка исчезла, реестр недоступен, SQL) — строка `error …` в
  журнал + интервал, цикл продолжается: демон не должен умирать от одного
  сбоя (TR-7.2: «ошибка БД не валит демон»);
* если на старте уже нет папки проекта / не открылся реестр — код 1 и выход (как
  сейчас), без цикла;
* коды: **0** — штатная остановка по `index.stop`; **1** — фатальный сбой старта
  или ошибки при `--once`; **2** — аргументы.

### 4.2. Остановка и lock

Порядок остановки (важен для воркеров и для следующего запуска):

1. цикл вышел (увидел `index.stop`);
2. `drop` зеркала и воркеров — `LazyAnonymizer`/`LazyExtractor` в `Drop` зовут
   `shutdown()` Python (`hds_extract::Worker::shutdown`);
3. снять `sync.lock` (`WatchLock` — RAII, снимается сам на `Drop`);
4. **снять `index.stop`** (решение В-14, дефолт «снимать»): `run_index` делает
   так же — «сигнал отработан». Иначе следующий `--once`/демон встанет сразу;
5. записать в журнал `stop reason=index.stop passes=<n> changes=<m>`.

Ctrl+C: обработчика в проекте нет — процесс завершается сразу, деструкторы не
выполняются (так же ведёт себя `hds watch`). В README написать честно: мягкая
остановка — `hds stop` (или `index.stop`), Ctrl+C — жёсткая; Python-воркеры
добиваются по своему idle timeout, реестр (WAL) не портится.

Lock демона: `WatchLock::acquire_named(&state_dir, "sync.lock", 3)` —
`<agent.root>\P<NNNN>\agent-state\sync.lock`, чтобы два проекта не мешали друг
другу. `None` — «демон зеркала уже запущен (sync.lock)», код 0, как у
`run_watch`. Устаревший lock (процесс умер) снимается автоматически по PID.

### 4.3. Журнал `<P>\agent-state\sync.log`

Путь: `settings::state_dir_for(cfg, project)` + `sync.log`; каталог создать при
старте. Формат — текст UTF-8, по строке на событие, время —
`hds_core::timefmt::fmt_local_datetime(now_epoch())`:

```text
2026-11-20 14:02:11 start project=P0001 root=D:\Проекты\Внедрение ERP interval=5s mode=daemon
2026-11-20 14:02:11 pass files=34 created=0 updated=0 unchanged=33 unsupported=1 too_big=0 deferred=0 removed=0 renamed=0 orphans=0 agent_changed=0 conflicts=0 emitted=0 variants=0 errors=0 changes=0 problems=0
2026-11-20 14:02:16 pass files=34 created=0 updated=1 unchanged=32 unsupported=1 too_big=0 deferred=0 removed=0 renamed=0 orphans=0 agent_changed=0 conflicts=0 emitted=0 variants=0 errors=0 changes=1 problems=0
2026-11-20 14:02:21 problem rel=Протоколы\Встреча.md reason=вариант не собран — …
2026-11-20 14:03:02 stop reason=index.stop passes=11 changes=1
```

Правила:

* **только пути, метки, статусы и числа** (I8). Содержимое документов, значения
  сущностей, ключи — никогда. Готовые строки проблем уже безопасны
  (`SyncReport::problems` — «файл: причина»);
* проблем на проход — не больше 10 (как `print_report`), остальные — счётчиком
  `problems=<n>`;
* при `errors > 0` и при `Err` прохода писать `error …`, но **не** завершать
  демон;
* журнал пишется и в режиме `--once` (дефолт: да — иначе разбор «почему образ не
  обновился» после разового прогона невозможен); решение зафиксировать в README;
* ротация (В-13): при старте, если файл больше 1 МБ, переименовать в
  `sync.log.1` (старый `.1` перезаписывается) — журнал не должен расти годами;
* тест: в журнале нет ни одной формы значения из fixtures и нет текста
  документа; строка `pass` содержит все счётчики; остановка оставляет `stop`.

### 4.4. README: глава про зеркало и таблица настроек

`README.md` — единственная документация для пользователя, и сейчас про зеркало
в ней нет ничего (проверено `Select-String`: ни `agent sync`, ни `agent result`,
ни `.hds-agent`). Дописать:

1. **§15 «Перечень команд»** (стр. 1027): строки `hds agent sync [--once |
   --interval SEC]`, `hds agent result accept|reject|conflict --project P --path
   <rel> [--choice agent|new|delegate]`, `hds agent project add|list`.
2. **§20 «ИИ-агент в анонимной зоне»** (стр. 1390) — новые подразделы:
   * **«Зеркало папки проекта»** — что происходит: оригиналы превращаются в
     образы с метками, структура папок сохраняется, `pdf/pptx/mpp` →
     `*.pdf.md`; что лежит в `.hds-agent` (`pending` — варианты до решения,
     `base` — снимок основания и карта картинок, `backup` — копии прежних
     версий, `messages.jsonl` — очередь сообщений агенту) и что `.hds-agent`
     **не** индексируется;
   * **«Вариант, отчёт и решение»** — правка образа даёт вариант в `pending`,
     рядом `<rel>.report.json` с четырьмя проверками (`labels_outside_project`,
     `label_grammar`, `unplaced_images`, `text_lost`, порог 40 %); `verdict:
     "check"` — пометка «проверить», а не запрет; «Принять» делает backup и
     заменяет оригинал, «Отклонить» возвращает образ, конфликт — три исхода;
     файл, открытый в Word/Excel, — «файл занят» (закрыть и повторить);
   * **«Режим демона»** — `--interval SEC` (по умолчанию
     `agent.sync.interval_sec` = 5 с), пауза **между** проходами, а не «раз в N
     секунд от старта»; остановка — `hds stop` (или `index.stop`), Ctrl+C —
     жёсткая; журнал `<agent.root>\P0001\agent-state\sync.log` — пути, метки и
     счётчики без содержимого документов; два демона на проект не запускаются
     (`sync.lock`);
   * **таблица настроек `agent.sync`** — перенести из `AGENT_A4_STAGES.md` §4.3
     (`interval_sec`, `wait_stable_sec`, `wait_poll_ms`, `wait_max_sec`,
     `settle_age_sec`) вместе со смыслом и дефолтами; в `config.example.yaml`
     эти ключи уже есть — сверить значения;
   * **«Что не зеркалится»**: `.hds-agent`, `_attachments`, `_conflicts`,
     `~$*`, `*.tmp`, `Thumbs.db`, junction/симлинки, файлы больше
     `index.max_file_mb` (`too_big`), неподдерживаемые форматы (`unsupported`).
3. Проверить, что README не обещает лишнего: «проверить» ничего не блокирует;
   автозапуска нет (это A6); подписки на изменения ФС нет (это фаза 2).

### 4.5. `AGENT_A4_REPORT.md` и RELEASE_NOTES

`AGENT_A4_REPORT.md` (корень репозитория) — отчёт фазы A4:

* таблица под-этапов A4.1–A4.6: что сделано, коммит, чем проверено;
* контракты, которые нельзя ломать (`AGENT_A4_STAGES.md` §5) — свести в список;
* что осталось: A5 (UI-кнопки, доставка `messages.jsonl` в сессию, «Сравнить»),
  A6 (автозапуск, `hds check` с Node), фаза 2 (подписка на ФС, настоящие образы
  PDF/PPTX); долги §4.2 (пп.1–27) — сгруппировать «блокирует A5 / можно потом»;
* e2e-свидетельства: `A43_E2E.md`, `A44_E2E.md`, `A45_E2E.md` (+ новый
  `A46_E2E.md`), сводные числа прогонов;
* инварианты: как проверяли I1/I2/I4/I6/I8 и что получилось.

`RELEASE_NOTES_0.26.0.md` — короткий раздел «Анонимная зона: зеркало папки
проекта» (что появилось для пользователя, какие команды, где настройки), ссылка
на README §20 и на `AGENT_A4_REPORT.md`. Имя файла сверить с текущим порядком
(последний — `RELEASE_NOTES_0.25.0.md`); если версия в `Cargo.toml` меняется
отдельным этапом — файл назвать по фактической версии, а не «на глаз».

### 4.6. Приёмка фазы A4 (её закрывает A4.6)

Критерии — `AGENT_A4_HANDOFF_PROMPT.md` §6. A4.2–A4.5 уже приложили свои
свидетельства; A4.6 добавляет **сводку** и проверяет то, что видно только с
демоном:

1. `hds agent sync --once` идемпотентен: повторный прогон — 0 изменений (есть).
2. Демон: правка файла в папке проекта подхватывается **без перезапуска** (в
   пределах интервала + `wait_stable`), правка образа даёт вариант и отчёт.
3. `hds stop` (он же `index.stop`) останавливает демон; после остановки
   `index.stop` снят, `sync.lock` снят, в журнале `stop reason=index.stop`;
   повторный старт не требует уборки руками.
4. Петли: за 3–5 проходов число записей реестра и файлов в `pending` не растёт
   (кроме реальных изменений); принятый вариант не порождает нового варианта.
5. I1/I2/I8: в `<agent.root>` нет ни одной формы значений fixtures; в
   `agent-state\sync.log`, `data\logs` и аудите — тоже; `%USERPROFILE%\.cline`
   до/после не изменился (переиспользовать `snapshot_dir`/`profile_mentions` из
   `agent_acceptance`).
6. `_attachments`/`_conflicts`/`.hds-agent` не выпущены в папку проекта.

## 5. Порядок работ и границы

Порядок (каждый шаг заканчивается зелёным прогоном):

1. **Флаги и их разбор** (без цикла): `--interval SEC`/`--interval=SEC`,
   `unknown_flag`, `usage()`, дефолт из `agent.sync.interval_sec`, коды ошибок;
   тесты аргументов рядом с существующими (`agent_sync.rs`).
2. **Журнал** отдельными функциями (открытие/ротация/строки `start`, `pass`,
   `problem`, `error`, `stop`): тесты на временном каталоге, без Python.
3. **Цикл** `run_loop` с инъекцией `should_stop` и `max_passes`; lock; порядок
   остановки (§4.2); тест «два прохода + стоп по флагу → код 0 и `stop` в
   журнале».
4. **Подключение к `cmd_sync`**: без флагов и с `--once` — прежнее поведение
   (та же функция проводки), с `--interval` — демон.
5. **Стенд** `agent_a42`: демон, правки, остановка, повторный `--once`;
   `A46_E2E.md` + логи `a46-*.log`.
6. **README** (§15 и §20, таблица настроек).
7. **`AGENT_A4_REPORT.md`**, `RELEASE_NOTES_0.26.0.md`.
8. **Журнал** `AGENT_A4_STAGES.md`: §2 (статус A4.6 и A4 целиком), §3.6, §4.2
   (закрытые/новые долги), §6 (новые грабли), §7 (команды демона), §8 (контекст
   A5).

Не делать в A4.6:

* автозапуск (Планировщик/служба), `hds check` с Node — **A6**;
* подписка на изменения ФС (`ReadDirectoryChangesW`) — **фаза 2** плана §8;
* вкладка «Агент», доставка `messages.jsonl` в сессию, «Сравнить» — **A5**;
* менять поведение валидатора A4.5, решений A4.4, формата отчёта и статусов;
* `hds agent result report` (долг §4.2 п.23) — только если останется время и
  не в ущерб приёмке.

## 6. Критерии готовности A4.6

1. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
   warnings`, `cargo test --workspace` — зелёные; `npm test` 23/23; python
   `unittest discover -s sidecar/tests -t sidecar` — OK.
2. **Тесты аргументов**: `--interval` без значения, `0`, отрицательное, не
   число → код 2; `--once --interval 5` → код 2; без флагов → один проход
   (неизменённая папка — 0 изменений); `--interval=7` — принимается.
3. **Тест цикла**: `run_loop` с `max_passes = 2` пишет две строки `pass` и одну
   `stop`, возвращает 0 и не висит; журнал создаётся и в режиме `--once`; в
   журнале нет ни одной формы значения fixtures и нет текста документа.
4. **Lock**: второй процесс/вызов при занятом `sync.lock` — сообщение «уже
   запущен», код 0, состояние не меняется.
5. **Стенд** `agent_a42`: `--interval 1` подхватывает правку файла (образ) и
   правку образа (вариант + отчёт) без перезапуска; `hds stop` останавливает
   ≤ 2 с, `index.stop` и `sync.lock` сняты, в журнале `stop reason=index.stop`;
   повторный `--once` — 0 изменений; отчёт `A46_E2E.md` с логами.
6. **README**: таблица `agent.sync` совпадает с `config.example.yaml`; команды
   `sync`/`result` описаны; обещаний сверх сделанного нет (автозапуск, подписка
   на ФС — явно «нет»).
7. **Документы фазы**: `AGENT_A4_REPORT.md` и `RELEASE_NOTES_0.26.0.md` есть;
   в отчёте — e2e-свидетельства и проверка I1/I2/I4/I6/I8.
8. **Журнал** обновлён (§2, §3.6, §4.2, §6, §7, §8), фаза A4 закрыта.

## 7. Грабли (отобрано для A4.6; полный список — `AGENT_A4_STAGES.md` §6)

1. **Ctrl+C не даёт выполнить деструкторы.** Обработчика сигнала в проекте нет
   (нет `ctrlc`/`signal-hook`), `hds watch` ведёт себя так же. Значит
   `shutdown()` Python-воркеров и снятие lock случатся только при мягкой
   остановке — по `index.stop`. В README так и написать.
2. **`index.stop` — общий сигнал** с `hds watch`/`hds index`/`hds stop`. Свой
   файл не заводить. Если работают оба демона, один может снять сигнал раньше
   второго — в README: «останавливайте по очереди» (или повторите `hds stop`).
3. **`sync.lock` не путать с `watch.lock` и `index.stop`**: lock — в
   `<P>\agent-state` и снимается RAII, `index.stop` — в каталоге состояния
   приложения и означает «остановись».
4. **Сон интервала — шагами ≤ 500 мс**, иначе демон реагирует на `hds stop`
   только через `interval` секунд (при 5 с это выглядит как «завис»).
5. **`Mirror` создаётся один раз.** Пересоздание каждый проход поднимает Python
   заново и теряет ленивых воркеров; при этом `Err` прохода — не повод
   пересоздавать мир: лог + интервал.
6. **Без флагов — один проход.** Нельзя делать «по умолчанию демон»: скрипты,
   тесты и `hds agent init` ждут завершения процесса.
7. **Тест демона не должен висеть.** Инъекция `should_stop`/`max_passes`
   обязательна; CLI-тесты держат `ENV_LOCK`, поэтому «по потоку на тест» —
   плохая идея.
8. **Журнал без значений (I8).** Берите готовые строки `SyncReport::problems`;
   не добавляйте «полезных» деталей вида фрагментов текста или имён сущностей.
9. **`state_dir_for` может вернуть `None`** (проект не `P`+4 цифры или
   `agent.enabled: false`) — тогда журнал писать некуда: сообщение и код 1, не
   паника.
10. **Ротация не должна терять текущую строку**: переименование — один раз при
    старте, до открытия на запись.
11. **PowerShell 5.1**: `*>` пишет UTF-16LE (читать `-Encoding Unicode`),
    кириллица в командной строке портится (пути — из `Get-ChildItem`), длинные
    прогоны — в файл в `tools/parity/out/`.
12. **`git` без `--no-pager` вешает терминал**; коммиты — `commit -F` с файлом
    UTF-8, путь — явным списком.
13. **Флейки**: `agent_acceptance::acceptance_a3_end_to_end` (mtime `~/.cline`) и
    сравнение `f64` после `serde_json` (1 ULP). Не «чинить» их в A4.6.

## 8. Окружение и команды

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager status; git --no-pager log -3 --oneline
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p hds-cli --test agent_sync
cargo test --workspace -- --test-threads=4 *> tools\parity\out\a46-cargo-test.log
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

Стенд `agent_a42` (демон; правки — из `Get-ChildItem` в переменные, без
кириллицы в командной строке):

```powershell
$env:HDS_ROOT='<repo>\tools\parity\out\agent_a42\root'
$env:HDS_CONFIG='<repo>\tools\parity\out\agent_a42\config.yaml'
& '<repo>\target\debug\hds.exe' agent sync --project P0001 --interval 1
#   -> [i] демон зеркала: интервал 1 с; строка pass каждую секунду в консоли
# правка файла в папке проекта -> новый/обновлённый образ сам
# правка образа агентом -> variant + report.json в .hds-agent\pending
$log='<repo>\tools\parity\out\agent_a42\zone\P0001\agent-state\sync.log'
[IO.File]::ReadAllText($log,[Text.Encoding]::UTF8)
& '<repo>\target\debug\hds.exe' stop   # index.stop: демон выходит, сигнал и lock сняты
& '<repo>\target\debug\hds.exe' agent sync --project P0001 --once   # 0 изменений
Remove-Item Env:HDS_ROOT, Env:HDS_CONFIG -ErrorAction SilentlyContinue
```

* `HDS_ANONYMIZE_PYTHON`/`HDS_ANONYMIZE_WORKER`/`HDS_EXTRACT_PYTHON`/
  `HDS_EXTRACT_WORKER` — подмена воркеров (mock) для CLI-тестов.
* `llm_host` на машине резидент; для прогонов демона он не нужен (GPU не
  занимается).

## 9. Вопросы владельцу (дефолты предложены — можно двигаться без ответа)

Нумерация — **фазы A4** (В-1…В-11 разобраны в `AGENT_A4_STAGES.md` §4.1; в A3
те же номера означают другие вопросы — не путать).

| # | Вопрос | Дефолт | Альтернатива |
|---|---|---|---|
| В-12 | Защита от второго демона зеркала | **да**: `sync.lock` в `<P>\agent-state` через `WatchLock::acquire_named`, «уже запущен» → код 0 | без lock: WAL выдержит, но варианты могут продублироваться |
| В-13 | Рост журнала `sync.log` | **да**: при старте > 1 МБ → `sync.log.1` | без ротации — журнал растёт годами и ест диск |
| В-14 | Снимать `index.stop` после остановки зеркала | **да** (как `run_index`: «сигнал отработан») | оставлять — следующий запуск встанет сразу |
| В-15 | Журнал в режиме `--once` | **да**: единое поведение и разбор инцидентов | только в режиме демона (меньше файлов в зоне) |
| В-16 | `--interval` без значения | **ошибка 2** с подсказкой про `agent.sync.interval_sec` | брать дефолт из конфига молча |

## 10. Промт для нового чата (копировать целиком, режим Act)

```text
Ты — ведущий разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Это продолжение основного проекта «ИИ-агент в анонимной зоне»
(PLAN_AGENT_PRIVACY_ZONE.md). Ветка agent-privacy-zone, HEAD — docs-коммит A4.6
(сверить git log -1 --oneline), код A4.5 — b2ae8552, дерево чистое, push не
делался.

Фаза A4 (демон двустороннего зеркала hds agent sync) идёт под-этапами:
A4.1 (реестр) — 00da7c54, A4.2 (прямой путь) — 81961936, A4.3 (обратный путь,
TR-7.5/7.7) — d84571ee, A4.4 (решения TR-7.6) — 43305867, A4.5 (валидатор
TR-7.8) — e676bb89. Журнал, решения и долги — AGENT_A4_STAGES.md (§2, §3.1–3.5,
§4.2 п.17–27, §4.3 — таблица настроек agent.sync, §5–§7); контекст и методика
этого под-этапа — AGENT_A4_6_HANDOFF_PROMPT.md (§3–§9), этот промт — её §10.

Задача: под-этап A4.6 — демон, CLI и документация (последний под-этап фазы A4).
Автозапуск — A6, подписка на изменения ФС — фаза 2, вкладка «Агент» — A5:
в A4.6 только опрос по таймеру, журнал и документация.

1. Прочитай AGENT_A4_6_HANDOFF_PROMPT.md целиком, затем AGENT_A4_STAGES.md (§2,
   §3.5, §4.2 — особенно 17–27, §4.3, §5–§7) и AGENT_A4_HANDOFF_PROMPT.md §4.1,
   §4.8, §6 (критерии приёмки A4), §7.
2. CLI: hds agent sync --project <P> [--once | --interval SEC].
   * без флагов и с --once — один проход, как сейчас (совместимость);
   * --interval SEC — демон: проход Mirror::sync_once, сон интервала шагами
     не больше 500 мс, остановка по <root>\index.stop и Ctrl+C; дефолт
     интервала — agent.sync.interval_sec (5 с); --interval 0/отрицательное/без
     значения и --once+--interval — код 2;
   * Mirror, ArtifactRegistry, LazyAnonymizer/LazyExtractor создаются один раз
     до цикла; Err прохода — строка в журнал и продолжение, демон не умирает;
   * lock демона — sync.lock в <P>\agent-state через
     hds_index::watch::WatchLock::acquire_named; занят — «уже запущен», код 0;
   * порядок остановки: цикл -> drop воркеров (shutdown Python) -> lock ->
     снять index.stop -> строка stop в журнал.
3. Журнал <agent.root>\P<NNNN>\agent-state\sync.log (settings::state_dir_for):
   строки start / pass (все счётчики SyncReport + changes + problems) /
   problem (не больше 10) / error / stop; только пути, метки, статусы и числа
   (I8), без содержимого документов; ротация при старте больше 1 МБ в
   sync.log.1; пишется и в режиме --once.
4. README.md: §15 «Перечень команд» — sync и result; §20 «ИИ-агент в анонимной
   зоне» — подразделы «Зеркало папки проекта» (что в .hds-agent: pending, base,
   backup, messages.jsonl), «Вариант, отчёт и решение» (report.json, четыре
   проверки, verdict check не блокирует, «Принять/Отклонить/Конфликт», «файл
   занят»), «Режим демона» (интервал, hds stop/index.stop/Ctrl+C, журнал,
   sync.lock), таблица настроек agent.sync (перенести из §4.3 журнала),
   «Что не зеркалится».
5. AGENT_A4_REPORT.md — отчёт фазы A4 (под-этапы, контракты §5, что осталось
   A5/A6/фаза 2, долги §4.2, e2e A43/A44/A45/A46, инварианты I1/I2/I4/I6/I8);
   RELEASE_NOTES_0.26.0.md — короткий раздел про зеркало.
6. Проверки: cargo fmt --check, clippy -D warnings, cargo test --workspace,
   npm test, python unittest; тесты: аргументы (--interval 0/-5/abc/без
   значения, --once+--interval), цикл с max_passes=2 (две строки pass и stop,
   код 0, не висит), журнал без значений fixtures, занятый sync.lock; e2e стенда
   agent_a42: --interval 1 подхватывает правку файла и правку образа без
   перезапуска, hds stop останавливает не дольше 2 с (index.stop и sync.lock
   сняты), повторный --once — 0 изменений; отчёт A46_E2E.md.
7. Коммиты: feat(agent): A4.6 — ...; docs(agent): A4.6 закрыт — ... (+ README,
   AGENT_A4_REPORT.md, RELEASE_NOTES); в сообщении — что проверено. Журнал
   AGENT_A4_STAGES.md — §2 (A4.6 и A4 целиком), §3.6, §4.2, §6, §7, §8 (контекст
   A5).
```

**Проверка среза:** ветка `agent-privacy-zone`, HEAD кода `b2ae8552` (+ docs этого
файла), ahead 65, дерево чистое; `cargo test --workspace` — 123 набора / 844 passed
/ 0 failed (13 ignored); python 328 OK (skip 1); `npm test` 23/23; стенд
`agent_a42` — `pending`/`base` пусты, `sync` — 0 изменений, отчёт `A45_E2E.md`.
