# AGENT_A4_HANDOFF_PROMPT.md — контекст и промт для фазы A4 (демон `hds agent-sync`)

> **Кому:** новый чат Cline в режиме Act. **Дата среза:** 19.11.2026.
> **Ветка:** `agent-privacy-zone`. **HEAD:** последний docs-коммит ветки (`git log -1` —
> этот файл и `AGENT_A0_A3_CODE_REVIEW.md`); код правок по ревью — `e161012e`;
> фазы A0–A3 — `3f34d51d`…`cb635027`.
> **Что это:** самодостаточный контекст для продолжения плана
> `PLAN_AGENT_PRIVACY_ZONE.md`. Прочитайте §0–§2, затем обязательное чтение §3.

## 0. Где мы (коротко)

План «ИИ-агент в анонимной зоне» выполнен по фазы **A0–A3** и принят код-ревью
(`AGENT_A0_A3_CODE_REVIEW.md`). Работает end-to-end:

* **A0** — спайки SDK 0.2.0, стенд, методика RAG (`tools/parity/AGENT_SPIKES.md`);
* **A1** — база меток v2 (`anonymize.db`), морфология/падежи, `keep_terms`, режим
  `mode:"agent"` (образы docx/xlsx с `[IMAGE_n]`, маркер `hds-agent/<P>/<artifact_id>`),
  текстовые образы `pdf/pptx/mpp` → `*.pdf.md`/`*.pptx.md`/`*.mpp.md`, `office_read/edit/create`;
* **A2** — индекс только по `<agent.root>\P*\workspace`, маршрут `/mcp/agent/<P>`
  (свой токен, фильтр проекта, без `ask_my_files`), метки одним токеном FTS,
  отставание индекса по проекту;
* **A3** — Node-воркер `sidecar/hds_agent` (@cline/sdk 0.2.0), крейт `hds-agent`
  (процесс, NDJSON §4.3, `host_call`, словарный фильтр, деанонимизация событий,
  реестр живых процессов), CLI `hds agent project|run|history`, установщик
  `installers/build_agent_sidecar.ps1`, приёмка e2e.

Ревью закрыло 19 находок (AG-1…AG-19): единое пространство граммем и метки
`[PERSON_1|дат]`, обновление снимка vault по `vault_rev`, plain-токен для
`disk-search`, `mcp_in` по всему результату, секреты только из секретных полей,
`timeout` в MCP-настройках, стабильная приёмка и др. **Не переделывайте это** —
используйте как контракт (§4).

**Следующая фаза — A4: демон `hds agent-sync`** (двустороннее зеркало папки проекта
и `<P>\workspace`), реестр артефактов, снимок основания, четыре ситуации «Принять/
Отклонить/конфликт», оформление правок `pdf/pptx/mpp` по TR-7.3a, валидатор
результата. Оценка плана — **5–6 дней**. UI вкладки «Агент» — это A5, в A4 только
ядро и CLI/API.

## 1. Состояние репозитория (проверено 19.11.2026)

| Параметр | Значение |
|---|---|
| Ветка | `agent-privacy-zone` (от `anonymizer-sidecar`), push не делался |
| Коммиты A0–A3 | `3f34d51d` … `cb635027` (47 коммитов) |
| Ревью A0–A3 | `AGENT_A0_A3_CODE_REVIEW.md` — все AG-1…AG-19 закрыты |
| Правки по ревью | `e161012e` (32 файла, +1025/−180, новый `crates/hds-agent/tests/refresh.rs`) |
| Docs этого среза | последний коммит ветки: `AGENT_A0_A3_CODE_REVIEW.md` + `AGENT_A4_HANDOFF_PROMPT.md` |
| Проверки на срезе | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` — чисто; `cargo test --workspace` — **121 набор / 767 тестов**, включая приёмку A3; `npm test` в `sidecar/hds_agent` — **23/23**; python `unittest discover -s sidecar/tests -t sidecar` — **307 OK** (1 skip) |
| Машина | Windows 11, PowerShell 5.1, Rust 1.99, Node 24.18, Python 3.12 в `sidecar\venv-anonymize`, RTX 3060 12 ГБ, `llm_host` резидент |

**Что уже лежит в коде и можно переиспользовать (не писать заново):**

| Компонент | Где | Что даёт |
|---|---|---|
| Клиент анонимизации | `crates/hds-anonymize/src/client.rs` | `anonymize` (mode `agent`, `images_mode`, `artifact_id`), `anonymize_text`, `text_image`, `office_read/edit/create`, `snapshot`, `vault_rev`, `label_for_value`, `merge_entities`, `drop_project` |
| Метки и граммемы | `crates/hds-core/src/labels.rs` | `label_parts`, `gram_to_suffix`, `is_label`, `split_labels`, `find_labels` — **единственный** разбор меток |
| Фильтр/деанон/шлюз | `crates/hds-agent/src/{filter,deanon,gateway,worker,paths,env,mcp,projects,settings,registry,history,rules,secrets}.rs` | словарь TR-6.x с `RefreshState` (TR-6.5), деанонимизация событий, матрица опасных путей, окружение процесса, `projects.json`, реестр процессов |
| CLI агента | `crates/hds-cli/src/cmd/agent.rs`, `agent_run.rs` | `init`, `token`, `project add|list`, `run`, `history clear`; шаблон для новых подкоманд |
| Индексация | `crates/hds-index` (`watch.rs`, `pipeline.rs`, `lag.rs`) | `hds watch`, heartbeat, `implicit_excludes` (`.hds-agent`, `_conflicts`, `agent-config`, `agent-state`), кэш отставания 30 с |
| Маршрут MCP | `crates/hds-mcp/src/{agent,http,schema,tools}.rs` | `/mcp/agent/<P>`, `search_local_files`/`index_status`, фильтр проекта |
| Разбор/сборка Office | `sidecar/hds_anonymize/{file_parser,office_ops,text_image}.py` | маркер `hds-agent/<P>/<id>`, `strip_body_images`, `read_anon_marker`, `is_text_image`, `build_text_image` |
| Конфиг | `crates/hds-core/src/config.rs`, `crates/hds-agent/src/settings.rs` | `dig`, `replace_file`, `state_dir`, `db_abs_path`, секция `agent:` (пример — `config.example.yaml`) |

## 2. Обязательное чтение (по порядку)

1. `PLAN_AGENT_PRIVACY_ZONE.md` — **§5.7 (TR-7.1…7.9), §5.8 (TR-8.1/8.3), §6 (строка A4),
   §7 (приёмка), §5.9 (I1–I8), §3.1 (D2, D5, D18, D22)**.
2. `AGENT_A0_A3_CODE_REVIEW.md` — §3 (находки) и **§8 (что уже исправлено: контракты
   меток, `RefreshState`, plain-токен, `mcp_in`, секреты, `timeout`)**.
3. `AGENT_A3_STAGES.md` — §4 «Известные долги и находки» (пп.1–39) и §6 «Грабли».
4. `AGENT_A3_HANDOFF_PROMPT.md` — §2.3 «Контракты, которые нельзя ломать», §4 «Порядок
   работ», §6 «Грабли».
5. `AGENT_A2_HANDOFF_PROMPT.md` — §8 (решения В-7…В-11), `README.md` глава «Анонимная
   зона агента» (§15), `README_ANONYMIZATION.md`, `PLAN_ANONYMIZER_SIDECAR.md` (§2.6,
   режим `agent`).
6. Код: `crates/hds-anonymize/src/client.rs`, `sidecar/hds_anonymize/handlers.py`
   (`anonymize(mode="agent")`, `text_image`, `office_*`), `sidecar/hds_anonymize/file_parser.py`
   (`read_anon_marker`, `strip_body_images`), `crates/hds-agent/src/{paths,filter,deanon,projects,settings}.rs`,
   `crates/hds-index/src/{watch,pipeline}.rs`.

## 3. Контракты, которые нельзя ломать

| Контракт | Где | Правило |
|---|---|---|
| **Метки** | `hds_core::labels` | `[TYPE_n]` / `[TYPE_n|русский падеж]` (TR-1.4/2.3); `gram` базы — код pymorphy, перевод только через `gram_to_suffix`; разбор — только `label_parts`, второй регулярки быть не должно. В **именах файлов** `|` запрещён Windows: имя получает метку **без** падежного суффикса (`Встреча с [PERSON_3].docx`) |
| **Режим `agent`** | `hds_anonymize` | `mode:"agent"` + `project_id`; имя файла сохраняется, маркер `hds-agent/<P>/<artifact_id>` в свойствах; `anonymize.agent.images: strip` (по умолчанию) заменяет картинки тела `[IMAGE_n]`; оригинал не меняется |
| **Текстовые образы** | `text_image.py` | `<имя>.<ext>.md`, только чтение; `editor`/`apply_patch` по ним блокирует `hds-guard` (`READONLY_IMAGE`); правки — новым документом (TR-7.3a/D22) |
| **Опасные пути** | `crates/hds-agent/src/paths.rs` + `sidecar/hds_agent/src/guard.ts` | матрица одна: UNC, `\\?\`, `\\.\`, ADS, устройства, `..`, junction; `office_*` проверяются в Rust повторно |
| **Фильтр** | `crates/hds-agent/src/filter.rs` | fail-closed `privacy_filter_unavailable`; снимок сверяется по `vault_rev` перед каждым запросом (`RefreshState`); секреты блокируют `model`/`mcp_out`; `keep_terms` не входят в словарь |
| **Токены** | `data/agent/secrets.json` | `projects.<P>.token` (plain, для `Authorization`) + `token_sha256` (маршрут) + `expires`; `mcp.servers` — дополнительные MCP; ACL «только владелец» best-effort (В-1) |
| **Проекты** | `projects.json` | `path` — **абсолютная папка пользователя** (не workspace); `id` = `P` + 4 цифры (D21) |
| **Индекс** | `hds-core::roots` | при `agent.enabled` корни — только `<agent.root>\P*\workspace`; `agent-config`/`agent-state`/`.hds-agent`/`_conflicts` исключены неявно |
| **Маршрут MCP** | `hds-mcp::agent` | `/mcp/agent/<P>`: только `search_local_files` и `index_status`, `projects=[P]` всегда, чужой/просроченный токен → 401 |
| **Жизненный цикл** | `hds-agent::registry` | `max_live_projects` (2), `idle_unload_min` (30), LRU; CLI — один проект на процесс |
| **Журналы** | `hds_core::audit` | событие `agent_egress_redacted` — метка/источник, **без значения**; секреты и значения в логи не пишутся (I8) |

## 4. Задачи A4 (TR-7.1…7.9) с методикой

### 4.1 TR-7.1 — структура зеркала и имена

* Папка проекта и `<P>\workspace` — одно дерево в двух видах; относительные пути совпадают,
  **имена** файлов и папок анонимизированы. Обход рекурсивный.
* Пропускать: `.hds-agent` (служебная папка проекта), `~$*`, `*.tmp`, `Thumbs.db`.
  Служебные папки workspace `_attachments\` и `_conflicts\` **не выпускать** в папку проекта.
* **Анонимизация имён.** В имени нельзя `|` (Windows), поэтому метка без падежного
  суффикса: `Протоколы\Встреча с Ивановым.docx` → `Протоколы\Встреча с [PERSON_3].docx`.
  Рекомендация: добавить в Python-воркер RPC `anonymize_path(project_id, path, suffix=false)`
  (NER по компонентам + `project_label_ex(suffix=False)`), а не склеивать из `anonymize_text`:
  на короткой строке NER слабее, а суффиксы пришлось бы вырезать регуляркой.
* Санитайзинг: убрать из имён `<>:"/\|?*` и управляющие символы (после подстановки меток),
  схлопнуть пробелы; длинное имя (> 120 символов) обрезать и добавить короткий хеш
  (8 hex от sha256 исходного имени), чтобы не упереться в `MAX_PATH`.
* Коллизии имён (разные оригиналы → одно имя): суффикс ` (2)`, ` (3)`; пара «оригинал ↔
  образ» хранится в реестре, а не вычисляется из имени.
* Переименование/перемещение оригинала → переименовать образ (и запись реестра), не
  пересоздавать.

### 4.2 TR-7.2 — реестр `data\agent\registry\<P>.db`

SQLite (WAL), пишут **только** демон и кнопки UI:

```sql
artifacts(
  id INTEGER PRIMARY KEY,           -- artifact_id для маркера hds-agent/<P>/<id>
  orig_rel TEXT NOT NULL UNIQUE,    -- относительный путь в папке проекта
  anon_rel TEXT NOT NULL,           -- относительный путь в workspace
  kind TEXT NOT NULL,               -- image | text_image | agent_new | attachment
  orig_sha256 TEXT,
  anon_sha256_issued TEXT,          -- что выпустил демон
  anon_sha256_seen TEXT,            -- что видели последним (TR-7.7)
  status TEXT NOT NULL,             -- synced|unsupported|too_big|agent_new|agent_changed|
                                    -- pending_accept|accepted|conflict|orphan|error
  ts REAL NOT NULL
)
```

* `artifact_id` выделяет демон (автоинкремент) и передаёт его в
  `anonymize {project_id, mode:"agent", artifact_id}` — маркер в файле связывает образ с
  реестром даже после потери БД.
* `orig_sha256` — от оригинала; `anon_sha256_issued` — от того, что записали в workspace;
  `anon_sha256_seen` обновляется при обходе. Изменение агентом = `seen != issued`.
* API реестра — отдельный модуль (`crates/hds-agent/src/registry_db.rs` или
  `crates/hds-index`?): `open`, `upsert`, `by_orig`, `by_anon`, `set_status`, `mark_seen`,
  `list_pending`. Ошибка БД — `error` в панели, но демон продолжает (не падает).

### 4.3 TR-7.3 — прямой путь (пользователь → анонимная зона)

1. `wait_stable`: размер и mtime не меняются **2 с** (опрос 0,5 с, максимум 30 с), затем
   обработка. Иначе файл может быть ещё копируется.
2. `docx`, `xlsx`, `txt`, `md`, `xml` → `Client::anonymize(mode:"agent")`; образ того же
   формата; для docx/xlsx картинки вырезаются (`anonymize.agent.images: strip`).
3. `pdf`, `pptx`, `mpp` → текстовый образ `<имя>.<ext>.md` (D18): текст берёт `hds_extract`
   (страницы/слайды/задачи; для `mpp` — mpxj, только через `hds_extract`), затем
   `Client::text_image(text=...)` (D18/D22).
4. Прочие форматы и файлы больше `max_file_mb` в workspace **не попадают**:
   `unsupported`/`too_big` в реестре (в панели «Файлы проекта» — A5).
   Решение по лимиту: переиспользовать `index.max_file_mb` (по умолчанию 200) — см. §9.
5. Удалённый оригинал → удалить образ, если агент его не менял (`seen == issued`), иначе
   `orphan` (оригинал не удаляется, UI предложит — A5).
6. Изменённый оригинал, у которого есть непринятый вариант (`pending_accept`), образ
   **не перезаписывать**: `conflict` (TR-7.6, случай 2).
7. После записи образа в workspace индексирование делает существующий `hds watch` — не
   дублировать очередь; отставание видно агенту (TR-4.7).

### 4.4 TR-7.4/7.3a — маркер, снимок основания, правки pdf/pptx/mpp

* Маркер `hds-agent/<P>/<artifact_id>` пишется Python-воркером в свойства файла; демон
  читает его (`read_anon_marker`) при обходе и сверяет с реестром.
* **Снимок основания**: при первом обнаружении изменения образа агентом скопировать
  текущий оригинал в `.hds-agent\base\<отн. путь>`, если sha256 оригинала ещё равен
  основанию (`orig_sha256`). Снимок нужен для восстановления `docProps` и картинок и для
  трёхстороннего «Сравнить». Удаляется вместе с вариантом.
* `pdf/pptx/mpp`: агент **не правит** оригинал и текстовый образ (guard уже блокирует
  `editor` по `*.pdf.md`/`*.pptx.md`/`*.mpp.md`); он создаёт новый документ:
  `<имя> — замечания.docx`, `<имя> — правки.docx`, `<имя> — изменения плана.xlsx`
  (TR-7.3a). Это `agent_new` → выпускается в папку проекта (TR-7.5).

### 4.5 TR-7.5 — обратный путь (анонимная зона → пользователь)

* **Новый файл агента** (`agent_new`, маркера нет или он новый) → деанонимизировать имя и
  содержимое → положить **на соответствующее место** в папке проекта (D5,
  автоматически). Если файл с таким именем есть — суффикс ` (агент)`.
  Пару «оригинал ↔ образ» записать в реестр, чтобы демон не сделал обратного образа
  (TR-7.7).
* **Изменённый образ существующего документа** (`agent_changed`: `seen != issued`) →
  деанонимизировать с восстановлением из основания:
  * `docProps` (автор, организация, заголовок) — из `.hds-agent\base\<rel>`;
  * картинки тела: `[IMAGE_n]` заменяются медиа из основания (порядок `strip_body_images`
    детерминирован; для XLSX — по якорям/ячейкам). **Нужна новая операция Python-воркера**
    (например `deanonymize(..., restore_from=<base>, images=true)`), возвращающая карту
    `[IMAGE_n] → media` — см. §9, вопрос 4.
  * результат — в `<папка проекта>\.hds-agent\pending\<отн. путь>`, статус `pending_accept`;
    **оригинал не трогается** (I6).
* **Удалённый агентом образ** → оригинал не удаляется; статус/предложение «удалить
  оригинал?» — A5.

### 4.6 TR-7.6 — четыре ситуации «Принять/Отклонить/Конфликт»

В A4 — **ядро** (библиотека + CLI-подкоманды), UI-кнопки — A5. Общие правила:

* **Принять, оригинал не менялся** (`sha256(оригинал) == основание`):
  backup оригинала в `.hds-agent\backup\<дата-время>\<rel>` → атомарная замена оригинала
  вариантом (`hds_core::config::replace_file`) → статус `accepted` → пересоздать образ из
  нового оригинала → удалить вариант из `pending` и снимок основания.
* **Конфликт** (оригинал изменился): автоматически ничего. Варианты:
  * «Оставить вариант агента» — как «Принять», с предупреждением о потере правок
    оригинала (backup обязателен);
  * «Взять новую версию» — вариант удалить, образ пересоздать из нового оригинала;
  * «Поручить агенту» — образ пересоздать из нового оригинала, анонимную копию прежнего
    варианта положить в `<P>\workspace\_conflicts\<имя> (вариант агента).docx`, агенту
    поставить сообщение (очередь, A5 доставит в сессию) → новый вариант с новым основанием.
* **Файл занят** (Word/Excel держит оригинал): замена невозможна — понятная ошибка
  «файл занят: закройте и повторите», backup не создаётся, вариант остаётся в `pending`.
* **Отклонить**: удалить вариант из `pending`; пересоздать образ из оригинала (иначе агент
  продолжит работать с отвергнутой правкой); поставить агенту сообщение «изменения
  отклонены; образ восстановлен».
* **Сравнить**: A4 отдаёт данные (пути основания/оригинала/варианта, для xlsx — список
  изменённых ячеек, для md/txt — текстовый diff). Запуск сравнения Word — A5.

### 4.7 TR-7.7/7.8 — петли и валидатор

* **Петли**: демон не реагирует на собственную запись — `anon_sha256_issued` (что выпустил)
  vs `anon_sha256_seen` (что видят); для выпущенных в папку проекта файлов хранится
  `orig_sha256` выпуска. Принятый вариант порождает новый образ, но не новый «результат».
* **Валидатор** (отчёт к каждому выпуску, не блокирует):
  * метки, которых нет в проекте (сверить со `snapshot`/`label_parts`; агент выдумал);
  * метки с несогласованным падежом (`|дат` при отсутствии формы — уже даёт
    `DeAnonymizer::unknown_labels`/предупреждения Python);
  * непомещённые `[IMAGE_n]` (метка осталась в тексте варианта);
  * доля текста оригинала, пропавшая в варианте (сравнить нормализованный текст;
    порог, например, > 40 % — «документ пересобран с нуля»).
  Отчёт — рядом с вариантом (`pending\<rel>.report.json`) и/или в реестре; UI помечает
  «проверить» (A5).

### 4.8 TR-7.9 — без «Входящих»

Отдельной папки нет: новые файлы агента сразу на своём месте; в `.hds-agent\pending\` —
только варианты изменённых документов (до «Принять»).

## 5. Порядок работ (под-этапы A4)

Дробить так же, как A3: каждый под-этап — один чат, заканчивается **зелёным тестом,
видимым снаружи**, и коммитом.

| Под-этап | Содержание | Готово, когда |
|---|---|---|
| **A4.1** | Реестр `data\agent\registry\<P>.db`: схема, API, статусы, `mark_seen`, `list_pending`, тесты (в т.ч. конкурентная запись из двух потоков) | тесты реестра; `hds agent sync --once` на пустой папке создаёт реестр и ничего не ломает |
| **A4.2** | Прямой путь: обход, `wait_stable`, анонимизация имён (RPC `anonymize_path`), docx/xlsx/txt/md/xml → образ, pdf/pptx/mpp → текстовый образ (через `hds_extract` + `text_image`), `unsupported`/`too_big`, удаление/переименование, `_attachments`/`_conflicts` | e2e на стенде (30–50 документов): структура повторена, имена с метками, текстовые образы читаются, повторный прогон — 0 изменений (идемпотентность) |
| **A4.3** | Обратный путь: `agent_new` → папка проекта (D5), `agent_changed` → вариант в `pending` с восстановлением `docProps`/картинок из основания, маркер/`artifact_id`, защита от петель | e2e: правка `office_edit` → вариант в `pending`, оригинал не изменился (I6); новый файл агента появился в папке; повторный обход не создаёт петель |
| **A4.4** | Решения TR-7.6: принять/отклонить/конфликт (3 варианта), backup, «файл занят», сравнение (данные), сообщения агенту (очередь) | тесты на копиях файлов: все четыре ситуации + занятый файл (Windows-only сценарий с открытым handle); backup и восстановление |
| **A4.5** | Валидатор TR-7.8 (отчёт к выпуску), порог «текст пропал», непомещённые `[IMAGE_n]`, метки вне проекта | тест: подделанный вариант ловится валидатором; нормальный — нет |
| **A4.6** | `hds agent sync [--once] [--interval SEC]`, остановка по `index.stop`/Ctrl+C, журнал в `agent-state\sync.log` (без значений), документация (`README.md` глава, `sidecar/README.md` при необходимости), отчёт `tools/parity/AGENT_A4_REPORT.md` | полный набор проверок §6; отчёт с фактами и командами |

Зависимости: A4.2 ← A4.1; A4.3 ← A4.2; A4.4 ← A4.3; A4.5 — параллельно A4.4; A4.6 —
последним.

## 6. Критерии готовности A4 (приёмка)

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` — зелёные; `npm test` в `sidecar/hds_agent`; python
   `unittest discover -s sidecar/tests -t sidecar`.
2. **E2E на стенде** (вымышленный проект внедрения 1С, 30–50 документов; генератор —
   `tools/parity/agent_a0_gen_project.py`, fixtures — `tools/parity/fixtures/`):
   * оригинал → образ (структура и имена), `pdf/pptx/mpp` → `*.md`;
   * агент правит docx через `office_edit` → вариант в `.hds-agent\pending`;
     `sha256` оригиналов до/после без «Принять» равны (**I6**);
   * «Принять» → оригинал заменён, backup есть, образ пересоздан;
   * новый файл агента (`office_create`) → появился в папке проекта;
   * конфликт: пользователь меняет оригинал, пока есть вариант → `conflict`, ничего не
     потеряно; все три исхода;
   * повторный обход — **нет петель** (число записей/образов не растёт);
   * `_attachments`/`_conflicts` не выпущены в папку проекта.
3. **I1/I2**: в `<agent.root>` и в журнале исходящего нет ни одной формы значений fixtures;
   в `data\logs`/аудите — тоже (**I8**).
4. **I4**: `%USERPROFILE%\.cline` до/после не изменился (приёмка A3 уже умеет это
   проверять — переиспользовать `snapshot_dir`/`profile_mentions`).
5. `hds agent sync --once` идемпотентен; повторный запуск не пересоздаёт образы и не
   плодит варианты.

## 7. Грабли (оплачены в A0–A3 и ревью — не наступать снова)

1. **Метки.** `forms.gram` — код pymorphy; в метку переводить только `gram_to_suffix`
   (`datv`→`дат`), именительный — без суффикса. В **именах** `|` запрещён — только
   `[PERSON_3]`.
2. **`canonicalize` на Windows** даёт `\\?\C:\…`, Node такое не переваривает: перед
   передачей пути в Node/реестр снимать префикс `\\?\` (A3 §4 п.13).
3. **PowerShell 5.1**: `.ps1` — только ASCII без BOM; стенды с кириллицей — UTF-8 **с BOM**;
   файлы править `[IO.File]::ReadAllText/WriteAllText` с `UTF8Encoding`, иначе кириллица
   портится.
4. **Тесты идут параллельно в одном процессе**: временные каталоги — с `AtomicU32`-счётчиком,
   не только `process::id()`.
5. **`git` без `--no-pager` вешает терминал**; длинные прогоны — `Start-Process`/`Start-Job`
   с логом в `tools/parity/out/`.
6. **`hds watch` держит `index.db`**: не удалять/не пересоздавать базу под живым watcher'ом
   (`hds agent init` умеет его останавливать); демон зеркала пишет только файлы workspace —
   индексирует существующий watcher.
7. **Снимок фильтра** обновляется по `vault_rev` (`RefreshState`) — не кэшировать снимок
   надолго и не держать замок словаря на время IPC.
8. **Занятый файл (Word/Excel)**: `os error 32`/`PermissionDenied` при замене — это ситуация 3
   TR-7.6, а не ошибка демона; backup не создавать, вариант оставить в `pending`.
9. **Атомарная замена** — `hds_core::config::replace_file`; `rename` между дисками падает —
   нужен fallback copy+replace.
10. **Длинные пути**: `LongPathsEnabled=0`; имена ≤ 120 символов, иначе `MAX_PATH`; в релизной
    сборке уже был случай с `venv-anonymize` (не копировать профильные venv).
11. **Junction/симлинк**: перед проверкой вложенности — `canonicalize` (иначе
    `remove_dir_all` может уйти за пределы каталога, см. AG-11 ревью).
12. **Исключения индекса** по именам (`.hds-agent`, `_conflicts`) уже заданы в
    `hds-index::pipeline::index_filter`; новые служебные папки добавлять туда же.
13. **`index.max_file_mb`** — общий лимит; `too_big` для зеркала считать по нему (см. §9).
14. **Секреты**: в словарь — только секретные поля; plain-токен проекта лежит в
    `secrets.json`; в MCP-настройках таймаут — ключ `timeout`.
15. **Логи и аудит без значений** (I8): в `sync.log` — пути/метки/статусы, не содержимое.
16. **Кэш отставания индекса** — 30 с (`project_lag_cached`); в тестах, где важна точность,
    звать `project_lag`.
17. **Реестр процессов** ждёт выхода 2 с (`shutdown_with`); не увеличивать без нужды.

## 8. Окружение и команды

```powershell
# из корня репозитория
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=4

# приёмка A3 (нужны node, sidecar\hds_agent\dist\worker.mjs, fixtures, venv)
cargo test -p hds-cli --test agent_acceptance -- --test-threads=1

# Node-воркер
cd sidecar\hds_agent; npm run typecheck; npm run build; npm test; cd ..\..

# Python-воркер
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar

# ручные прогоны агента
hds agent init --yes
hds agent token P0001
hds agent project add P0001 --name "Пилот" --path "D:\Проекты\Внедрение ERP"
hds agent run --project P0001 --dry-run
hds agent run --project P0001 --prompt "..."
```

Переменные: `HDS_CONFIG`, `HDS_ROOT`, `HDS_AGENT_SECRETS`, `HDS_ANONYMIZE_PYTHON`,
`HDS_ANONYMIZE_WORKER`, `HDS_EXTRACT_PYTHON`. Артефакты — только `tools/parity/out/`
(гитигнорится). Машина — разработческая: процессы `llm_host`/`hds *` можно останавливать и
возвращать; GPU-аренда `anon` — через `llm_host` (см. правила среды).

## 9. Вопросы владельцу (предложены решения — можно двигаться с дефолтом)

| # | Вопрос | Предложение (если владелец не ответит) |
|---|---|---|
| 1 | Режим `hds agent sync`: только `--once` или демон с интервалом? | A4: `--once` + `--interval` (по умолчанию 5 с), автозапуск службой — A6 |
| 2 | `wait_stable`: сколько ждать стабильности? | размер+mtime не меняются 2 с, опрос 0,5 с, максимум 30 с |
| 3 | Лимит `too_big` для зеркала | `index.max_file_mb` (200 МБ), отдельный ключ не вводить |
| 4 | Восстановление картинок XLSX | на A4.3 расширить `strip_body_images`/`_strip_xlsx_images` возвращать карту `[IMAGE_n] → якорь/медиа` и сохранять её в `.hds-agent\base\<rel>.images.json`; docx восстанавливать по порядку |
| 5 | Имена: коллизии и длина | суффикс ` (2)`, длинные — обрезка + 8 hex sha256; метки без падежа |
| 6 | Сообщения агенту при «Отклонить/Поручить» | очередь `.hds-agent\messages.jsonl`; A5 доставляет в сессию, A4 только пишет |
| 7 | «Сравнить» docx | A4 отдаёт пути/данные; запуск Word-сравнения — A5 |

## 10. Промт для нового чата (копировать целиком)

```text
Ты — ведущий разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Ветка agent-privacy-zone, фазы A0–A3 плана PLAN_AGENT_PRIVACY_ZONE.md закрыты и приняты
код-ревью (AGENT_A0_A3_CODE_REVIEW.md); правки по ревью — коммит e161012e.

Задача: выполнить фазу A4 — демон hds agent-sync (двустороннее зеркало папки проекта и
<P>\workspace), реестр артефактов, снимок основания, четыре ситуации TR-7.6, оформление
правок pdf/pptx/mpp по TR-7.3a, валидатор результата.

1. Прочитай целиком AGENT_A4_HANDOFF_PROMPT.md (это контекст и методика) и обязательное
   чтение из его §2: PLAN_AGENT_PRIVACY_ZONE.md §5.7–5.9, §6 (A4), §7;
   AGENT_A0_A3_CODE_REVIEW.md §3 и §8; AGENT_A3_STAGES.md §4 и §6;
   AGENT_A3_HANDOFF_PROMPT.md §2.3/§4/§6; AGENT_A2_HANDOFF_PROMPT.md §8. Затем код:
   crates/hds-anonymize/src/client.rs,
   sidecar/hds_anonymize/{handlers,file_parser,text_image,office_ops}.py,
   crates/hds-agent/src/{paths,filter,deanon,projects,settings}.rs,
   crates/hds-index/src/{watch,pipeline}.rs.

2. Работай по под-этапам A4.1…A4.6 из §5: один под-этап — один чат, в конце зелёный тест,
   видимый снаружи, и коммит. Начни с A4.1 (реестр data\agent\registry\<P>.db + тесты).
   Перед каждым под-этапом кратко покажи план и затронутые файлы.

3. Не ломай контракты §3 (метки hds_core::labels, режим agent, guard путей, фильтр с
   RefreshState, токены, projects.json, корни индекса, маршрут /mcp/agent, жизненный цикл,
   журналы без значений). Правки по ревью AG-1…AG-19 уже в коде — не переделывай.

4. Грабли §7 обязательны: \\?\ после canonicalize, PowerShell 5.1 и кодировки, уникальные
   temp-каталоги в тестах, занятый файл = ситуация 3 TR-7.6, атомарная замена через
   hds_core::config::replace_file, длинные имена, junction, логи без значений.

5. Проверки после каждого под-этапа: cargo fmt --check, cargo clippy --workspace
   --all-targets -- -D warnings, cargo test --workspace, npm test в sidecar/hds_agent,
   python unittest discover -s sidecar/tests -t sidecar. Артефакты — в tools/parity/out/.

6. Вопросы из §9 задай владельцу сразу; если ответа нет — работай по предложенному
   дефолту и зафиксируй решение в AGENT_A4_STAGES.md (создай по образцу AGENT_A3_STAGES.md).

7. Коммиты: feat(agent): A4.N — ...; docs(agent): A4.N закрыт — ...; в сообщении — что
   проверено (тесты/факты). В конце фазы — AGENT_A4_REPORT.md и обновление
   README/RELEASE_NOTES.
```
