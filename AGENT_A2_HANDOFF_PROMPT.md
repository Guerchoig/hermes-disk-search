# AGENT_A2_HANDOFF_PROMPT.md — контекст и промт для фазы A2 (индекс и `disk-search` по анонимной зоне)

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать **§9 целиком** (он
> самодостаточен) и отправить первым сообщением. Разделы §0–§8 — справочная «карта»: промт
> в §9 ссылается на них, исполнитель читает этот файл целиком.
>
> **Дата среза:** 17.11.2026. **Ветка:** `agent-privacy-zone`, HEAD `c626a089` (push не делался,
> рабочее дерево чистое).
> **План:** `PLAN_AGENT_PRIVACY_ZONE.md`, редакция 3.1. Правки плана по итогам A0 (П-1…П-14)
> действуют, владельцем в план ещё не внесены — они в `tools/parity/AGENT_SPIKES.md` §3.11.
> **Отчёт A0:** `tools/parity/AGENT_SPIKES.md`. **Итог A1:** §2 этого файла,
> `RELEASE_NOTES_0.24.0.md`, `README_ANONYMIZATION.md`.

---

## 0. Что происходит (коротко)

`hermes-disk-search` (**hds**) — локальный поиск и ИИ-инструменты для аналитиков и менеджеров
проектов внедрения 1С (проектные документы, не код). Функция по плану: **ИИ-агент в анонимной
зоне** — headless-ядро Cline (`@cline/sdk`) в процессе `hds_agent`, который работает только с
анонимизированными копиями документов (метки `[PERSON_3]` вместо ФИО), а всё исходящее проходит
фильтр hds.

| Фаза | Состояние |
|---|---|
| **A0 — спайки** | закрыт, отчёт `AGENT_SPIKES.md` |
| **A1 — база меток v2** | закрыт: `vault.py`, морфология, `keep_terms`, режим agent, образы, office; 6 коммитов (§2.1) |
| **A2 — индекс и `disk-search`** | **эта фаза** |
| A3 — агент (`hds_agent`) | дальше |
| A4 — демон `hds agent-sync` | дальше |
| A5 — вкладка «Агент» в UI | дальше |
| A6 — выпуск | дальше |

**Фаза A2 (эта карта)** — индекс и маршрут `disk-search` по анонимной зоне (D9):

1. при `agent.enabled: true` индексатор игнорирует `index.roots` и индексирует один корень —
   `<agent.root>`; каждый `P*\workspace` становится проектом индекса `P0001` (TR-4.1);
2. `hds agent init` — сброс индекса и первичное построение (TR-4.2);
3. `POST /mcp/agent/<P>` — свой токен проекта, **всегда** фильтр `projects=[P]`, `401` на
   чужой/просроченный токен или чужой `P`, без инструментов управления индексацией (TR-4.3);
4. на маршруте агента публикуется только поиск: `search_local_files` + `index_status`,
   `ask_my_files` **нет** (TR-4.4);
5. метка `[PERSON_3]` ищется как одно слово — нормализация в FTS при индексации и в запросе
   (TR-4.6);
6. отставание индекса: `pending`-файлы, `index_status` по проекту, предупреждение в выдаче
   (TR-4.7).

**Что НЕ входит в A2:** процесс `hds_agent` и фильтр исходящего (A3), демон зеркалирования
`hds agent-sync` и реестр артефактов целиком (A4), вкладка «Агент» (A5), ключи конфига и
`hds check` для агента (A6). Контракт «что уже есть в sidecar» — §2.2–2.4.

## 1. Обязательное чтение (по порядку)

1. **`PLAN_AGENT_PRIVACY_ZONE.md`**: §3 (D3, D4, D9, D21, D22, D23), §4.2 (потоки), §4.4
   (раскладка `agent.root`, `data\agent\`, `.hds-agent\`), **§5.4 (TR-4.1…TR-4.7)**, §5.9
   (инварианты I1–I8), §6 (таблица фаз: строка A2 — критерий «готово»), §9 (риски).
2. **Этот файл §2** — состояние репозитория и что именно оставил A1.
3. **Код индекса и корней** (читать до правок):
   * `crates/hds-core/src/roots.rs` — `Root { path, project }`, `roots_from_cfg` (принимает и
     строки, и объекты `{path, project}`), `roots_paths`, `project_for_path` (длиннейший префикс);
   * `crates/hds-index/src/pipeline.rs` — `implicit_excludes(cfg)`, `effective_roots`,
     `collect_files`, `count_files`, присвоение проекта файлу через `project_for_path`;
   * `crates/hds-index/src/walk.rs` (обход), `hash.rs` (**`content_hash(path, size)`** —
     blake2b-16; этим же отпечатком TR-4.7 проверяет «проиндексировано»);
   * `crates/hds-search/src/lib.rs::search(conn, emb, lem, cfg, query, kinds, projects, limit)` —
     **фильтр `projects` уже есть**; `fts.rs` (`fts_tokens`, `fts_query`, `fts_search_ids`);
   * схема `files` (`path, ext, kind, size, mtime, content_hash, status, error, chunk_count,
     indexed_at, project`) — `CREATE TABLE IF NOT EXISTS files` в `hds-index`.
4. **Код MCP** (`crates/hds-mcp/src/`): `http.rs::route` (сейчас один маршрут `mp == mcp_path`),
   `server.rs::handle(msg, auth)`, `schema.rs::tool_list()`, `tools.rs`
   (`search_local_files`, `ask_my_files`, `index_status`, `start_indexing`, …), `auth.rs`
   (`AuthEnv`, `resolve_bearer`, `project_filter`). Плюс `crates/hds-core/src/access.rs`
   (`AuthCtx`, `token_sha256_hex`) и `crates/hds-cli/src/cmd/{mcp_http.rs, stop.rs, index.rs}`.
5. **Контракт A1** (то, на что опираются A3/A4 и что нельзя ломать):
   `sidecar/hds_anonymize/vault.py`, `handlers.py`, `morph.py`, `office_ops.py`,
   `text_image.py`; `crates/hds-anonymize/src/{protocol,client,settings}.rs`;
   `crates/hds-cli/src/cmd/anonymize.rs`; `README_ANONYMIZATION.md` (§4–§6, §9–§10);
   `sidecar/README.md` (раздел «Анонимизация»).
6. **Стенд и замер**: `tools/parity/agent_a1_stand.py` (переанонимизация стенда новой базой),
   `tools/parity/agent_a0_rag_eval.py` (recall@k, MRR), `tools/parity/agent_a0_gen_project.py`
   (генератор стенда), `tools/parity/out/agent_a1/` (готовые образы и конфиг).

## 2. Состояние репозитория

### 2.1 git

* ветка `agent-privacy-zone`, HEAD **`c626a089`**, дерево **чистое**, **push не делался**;
* коммиты A1 (чужие правки из другого чата были закоммичены владельцем до начала A1 как
  `d36ddbb3 Small_changes`):
  * `ae9d07b8` — sidecar: `vault.py` (схема v2), морфология, `keep_terms`, режим agent, образы, office;
  * `102152b0` — Rust: `Mode::Agent`, `LABEL_PATTERN/label_parts`, новые RPC клиента/настройки, CLI;
  * `a7a6e98a` — документация A1 + `tools/parity/agent_a1_stand.py`;
  * `98abc394` — `agent.images: keep`;
  * `90927de2` — `keep_terms`: согласованный состав продуктов 1С;
  * `c626a089` — картинки листов XLSX → `[IMAGE_n]`, очистка метаданных XLSX.
* `target/release/hds.exe` пересобран в A1: `hds anon agent|text-image|snapshot|keep|merge`
  доступны в release.

### 2.2 Что сделано в A1 (по файлам)

| Область | Файлы | Итог |
|---|---|---|
| База меток v2 | `sidecar/hds_anonymize/vault.py` (вместо удалённого `mapping_store.py`) | схема ровно TR-1.2: `meta`(schema_version=2, rev), `entities/forms/global_counters` (уровень 0), `projects/labels/counters` (уровень 1), `aliases`, `keep_terms`; WAL + busy_timeout; v1 → `anonymize.db.v1-<YYYYMMDD-HHMMSS>.bak` |
| Морфология | `morph.py` | `norm_key` (П-2: Surn-разбор, дефисные фамилии, инициалы «и.»), генерация падежей, суффикс только при однозначности (П-12), обратная подстановка, `,мн` |
| `keep_terms` | `vault.DEFAULT_KEEP_TERMS`, `keep_terms.example.yaml` | 44 записи: продукты 1С (полные + короткие), `cs:ДО`/`cs:КА`, `re:REQ-\d+`, компоненты составных названий |
| Режим agent | `handlers.anonymize(project_id, mode="agent", artifact_id, images_mode)`, `file_parser.strip_body_images` | метки проекта с суффиксами, `[IMAGE_n]` в DOCX (тело/таблицы) и XLSX (листы, уровень пакета без Pillow), маркер `hds-agent/<P>/<id>`, scrub DOCX+XLSX, оригинал не меняется |
| Образы D18 | `text_image.py`, RPC `text_image` | pdf/pptx (PyMuPDF/python-pptx или портативный CPython), mpp — текст от `hds_extract`; `<имя>.<ext>.md` только для чтения |
| Office | `office_ops.py`, RPC `office_read/edit/create` | docx/xlsx/pptx; формулы, форматы, стили сохраняются; pptx в MVP — только `replace_text` |
| RPC | `worker.py`, `handlers.py` | `anonymize_text`, `text_image`, `label_for_value`, `vault_snapshot`, `vault_rev`, `merge_entities`, `vault_drop_project`, `keep_terms`, `office_*`; `hello`: `schema_version: 2`, capabilities |
| Rust | `crates/hds-anonymize/src/{protocol,client,settings}.rs` | `Mode::Agent`, `LABEL_PATTERN`/`label_parts`/`is_label` (TR-1.4), типы снимка/текста/образа/office, `anonymize.keep_terms_file`, `anonymize.agent.images` |
| CLI | `crates/hds-cli/src/cmd/anonymize.rs` | `agent`, `text-image`, `snapshot`, `keep`, `merge`, `deanon --project` |
| Тесты | `sidecar/tests/test_anonymize_{vault,morphology,agent_mode,office,text_image}.py` | 307 python-тестов (1 skip — нет `python-pptx`), rust: `protocol`/`client`/`settings`/CLI |

### 2.3 Чего A1 НЕ создавал (нужно A2)

* каталогов `<agent.root>` (по умолчанию `D:\hds-anon`) и проектов `P0001\workspace`;
* `data\agent\projects.json` (реестр проектов: имя, путь папки, модель);
* `data\agent\secrets.json` (ключи провайдеров/MCP и **токены `/mcp/agent/<P>`**, ACL — только владелец);
* `data\agent\registry\<P>.db` (реестр артефактов, TR-7.2 — целиком в A4);
* секции `agent:` в `config.example.yaml` (TR-8.3 — A6, но ключи `agent.enabled`, `agent.root`,
  `agent.mcp.*` нужны уже A2);
* маршрута `/mcp/agent/<P>`, команды `hds agent init`, выдачи токенов проекта;
* нормализации меток в FTS и `index_status` по проекту;
* пункта `agent` в `hds check` (A6).

### 2.4 Факты A1, на которые опираться

* Метки: уровень 0 — `[PERSON_7]` (карточка/CLI), уровень 1 — метки проекта, при однозначном
  падеже с русским суффиксом (`[PERSON_3|дат]`, `[PERSON_2|род,мн]`). Регулярка одна:
  `\[([A-Z_]+)_(\d+)(?:\|([а-яё,]+))?\]` (`vault.LABEL_RE`, `protocol.LABEL_PATTERN`).
* `keep_terms` (D19) — 44 записи, включая `cs:`-регистр и `re:REQ-\d+`; поиск по продуктам 1С
  должен остаться осмысленным (продукты в образах **не** закрыты метками).
* `[IMAGE_n]` — синтаксически метка, но **тип `IMAGE`**: фильтр исходящего (A3) обязан его
  пропускать, а индексация — сохранять как текст (это важно для поиска по образу).
* Стенд и цифры A1: 32 файла, 0 ошибок, `rev=116`, 41 метка проекта, `keep=44`;
  **recall@10 = 1,0**, recall@5 = 1,0, MRR 0,906; поиск по `[PERSON_3]` — 7 из 10 выдач.
* Артефакты: `tools/parity/out/agent_a1/report.json`, `s10/{anon,config.yaml,index.db,eval.json}`;
  `s10/config.yaml` — **временный** конфиг стенда (индекс и образы только в `out/`).
* Python-тесты гоняются интерпретатором `sidecar\venv-anonymize\Scripts\python.exe`;
  rust — `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` (при полной параллельности два тайминг-теста `hds-core::queue`
  флейкуют — гонять `-- --test-threads=4`).

## 3. Задачи с методикой

### 3.1. TR-4.1 — корень индекса = `agent.root` (игнорировать `index.roots`)

**Что сделать.**

1. В `hds_core::roots` добавить функцию вида `effective_roots(cfg) -> Vec<Root>`:
   * `agent.enabled: true` → корни **синтезируются**: `<agent.root>\P*\workspace`, `project` =
     имя каталога проекта (`P0001`), только каталоги, где есть `workspace`;
   * иначе — `roots_from_cfg(cfg)` (прежнее поведение, ничего не ломать).
2. Перевести на неё индексатор: `pipeline::effective_roots` (сейчас читает `index.roots`),
   `collect_files`/`count_files`, `hds watch` (`WalkOptions::roots`), показ корней в настройках UI.
   Явные `--roots` у `hds index` при `agent.enabled` **игнорировать** (или отклонять с сообщением).
3. `implicit_excludes(cfg)` дополнить: `<agent.root>\P*\agent-config`, `<agent.root>\P*\agent-state`,
   `.hds-agent` (в папке проекта, вне agent.root — но пусть будет), `_conflicts` внутри workspace.
   `_attachments` **индексируется** (это вложения из чата, агент должен их находить).
4. Настройки UI: при `agent.enabled` вкладка «Настройки» показывает «корни заданы агентом» и не
   даёт их менять (минимальная правка существующей страницы; вкладка «Агент» — A5).
5. `config.example.yaml`: добавить секцию `agent:` с `enabled: false`, `root: 'D:\hds-anon'`
   (комментарий — вне профиля пользователя), и заготовку `mcp` (ключи — A6, но типы нужны).

**Критерий.** Тест `hds-core`/`hds-index`: временное дерево `P0001\workspace`, `P0002\workspace`,
`P0003\agent-config`; при `agent.enabled: true` `effective_roots` даёт два корня с проектами
`P0001`/`P0002`, `index.roots` не учитываются; файл в `agent-config` не попадает в обход;
`project_for_path` возвращает проект для файла workspace.

### 3.2. TR-4.2 — `hds agent init`

**Что сделать.** Подкоманда CLI `hds agent init [--yes] [--root DIR] [--no-index]`
(точка входа — разбор argv в `crates/hds-cli/src/main.rs`, новый модуль `cmd/agent.rs`):

1. остановить `hds watch`: создать сигнал `index.stop` (как `cmd_stop`) и дождаться завершения
   (PID/файл-сигнал — посмотреть, как это делает `watch.rs`/`service`); если watch не запущен —
   просто идти дальше;
2. удалить `index.db` вместе с `-wal`/`-shm` (путь — `hds_core::config::db_abs_path(cfg)`);
3. удалить `data/anonymized` (`anonymize.output_dir`, путь — `hds_anonymize::settings` или
   `settings.output_dir_abs`; в CLI это уже считается);
4. создать пустую базу со схемой (открыть соединение и вызвать инициализацию схемы так же, как
   `hds index` на пустом каталоге);
5. запустить индексацию (`pipeline` напрямую или породить `hds index`; в тестах — `--no-index`);
6. идемпотентность: повторный запуск не падает; без `--yes` в неинтерактивном режиме —
   подтверждение невозможно → код 2 и подсказка про `--yes`.

**Критерий.** Тест CLI на временном конфиге (`HDS_CONFIG`) и tmp `agent.root` с двумя
проектами: до init в индексе мусор, после — база создана заново, `data/anonymized` удалён,
`files` содержит образы обоих проектов с правильными `project`; повторный запуск — 0 и без
дублей.

### 3.3. TR-4.3 / TR-4.4 — маршрут `POST /mcp/agent/<P>`

**Что сделать.**

1. `http::route`: распознавать `<mcp_path>/agent/<P>` (`P` строго `^P\d{4}$`); прежний
   `<mcp_path>` не меняется. Обработчик — новый, чтобы не размазывать условия по старому коду.
2. Токен проекта: `data\agent\secrets.json`, структура вида
   `{"projects": {"P0001": {"token_sha256": "<hex>", "expires": 0}}}`
   (`hds_core::access::token_sha256_hex`, сравнение постоянным временем — как в `access.rs`);
   выдача — `hds agent token <P> [--ttl-days]`, plain печатается один раз (В-9).
   Проверки: нет заголовка/неверный токен/просрочен/`P` не тот → **401** (не 403, TR-4.3).
3. Фильтр: на этом маршруте `projects` **всегда** `[P]`, независимо от `access.enabled`;
   ветку `ask_my_files` и инструменты управления (`start_indexing`, `stop_indexing`,
   `reindex_path`, `forget`) не публиковать вообще (ни в `tools/list`, ни в `tools/call`).
   `search_local_files` и `index_status` — публикуются, `index_status` отвечает по проекту.
4. `server::handle`/`schema::tool_list`/`tools.rs` должны принимать «скоуп маршрута»
   (проект + сокращённый набор инструментов) — не хардкодить в `tools.rs`.
5. Аудит (если включён): событие `tool_call` с проектом маршрута, без значений PII.

**Критерий (сокет-тест).** Поднять `hds mcp-http` на свободном порту с временным конфигом и
индексом из двух проектов:
* токен `P0001` → `search_local_files` возвращает только фрагменты `P0001` (проверить по путям
  `<agent.root>\P0001\...`, включая ветку векторов — фильтр `projects` уже есть в `search`);
* тот же токен на `/mcp/agent/P0002` → 401;
* без токена/мусорный токен → 401;
* `tools/list` на маршруте агента: есть `search_local_files`, `index_status`; **нет**
  `ask_my_files` и управляющих;
* `tools/call` с `ask_my_files` → ошибка «unknown tool» (не выполняется).

### 3.4. TR-4.5 — пути в выдаче

Пути в результатах — абсолютные, внутри `<agent.root>` (это уже так: `files.path` — абсолютный).
Проверить тестом, что выдача `search_local_files` на маршруте агента содержит пути
`...\P0001\workspace\...` и не содержит путей других проектов; при необходимости — не резать
пути до относительных.

### 3.5. TR-4.6 — метка как одно слово в FTS

**Проблема.** Стандартный токенизатор FTS5 режет `[PERSON_3]` на `person` и `3` — запрос
«[PERSON_3]» находил бы любой фрагмент со словом «person» и цифрой 3 (и метки `[PERSON_30]`,
`[ORG_3]` тоже).

**Что сделать.**

1. Одна функция нормализации меток, например `hds_search::labels::normalize_labels(text)`:
   `\[([A-Z_]+)_(\d+)(?:\|([а-яё,]+))?\]` → `xperson3x`, `xperson3x` для `|дат` (суффикс **не**
   должен менять токен: «передать [PERSON_3|дат]» и «[PERSON_3]» — одна сущность), `[IMAGE_1]` →
   `ximage1x`.
2. Применить её:
   * в индексации — к тексту чанка **до** вставки в `chunks_fts` (искать точку в
     `hds-index/src/pipeline.rs`/`chunker.rs`, где формируется FTS-текст; лемматизация — после
     нормализации);
   * в запросе — в `fts::fts_tokens`/`fts_query` (нормализовать до лемматизации и в кавычки).
3. Векторная ветка получает текст как есть (не нормализовать) — TR-4.6.
4. Ничего не менять в sidecar: `[IMAGE_n]`/`[PERSON_n]` в образах остаются текстом.

**Критерий.** Тест на временной базе: три файла — `[PERSON_3]`, `[PERSON_30]`, `[ORG_3]`;
поиск «[PERSON_3]» находит только первый; «[PERSON_3|дат]» находит тот же файл; «[PERSON_30]» —
только второй. Плюс регресс: обычные запросы (`этап 2`, `бюджет`) работают как раньше.

### 3.6. TR-4.7 — отставание индекса

**Что сделать.**

1. Критерий «проиндексировано» — запись в `files` с тем же `path`, тем же `content_hash`
   (`hds_index::hash::content_hash(path, size)`) и успешным `status`.
2. `pending` — файлы `<agent.root>\<P>\workspace\**`, для которых такого совпадения нет.
   **Решение по объёму:** полный реестр артефактов (TR-7.2) — A4; в A2 достаточно считать
   `pending` по файловой системе + `index.db` (только чтение, без записи). Уточнить у владельца
   (В-7 в §8), зафиксировать решение в документации.
3. `index_status` на маршруте агента: `{project, total, done, pending, errors, building}`;
   `building` — первичное построение (`files` проекта пуст и/или индексация идёт).
   На общем маршруте `<mcp_path>` поведение прежнее.
4. `search_local_files` на маршруте агента: если `pending > 0` — добавить к тексту выдачи
   предупреждение «N файлов проекта ещё не проиндексированы (в т.ч. недавно изменённые: …);
   для них используйте `search_codebase` или чтение файла», список — до 10 путей.
5. Панель «Файлы проекта» (A5) — отдельно; в A2 достаточно меток в ответе `index_status`.

**Критерий.** Тест: в workspace появляется новый файл → `index_status.pending = 1`,
`search_local_files` содержит предупреждение и путь файла; после `hds index` →
`pending = 0`, предупреждения нет.

## 4. Порядок работ и стоп-точки

1. **Стоп-точка 0.** `git status`, ветка `agent-privacy-zone`, HEAD `c626a089`, дерево чистое.
   Если появились чужие незакоммиченные правки — **не трогать**, согласовать с владельцем.
2. **TR-4.1** — `effective_roots` + исключения + тесты; коммит.
3. **TR-4.2** — `hds agent init` + тесты; коммит.
4. **TR-4.6** — нормализация меток в FTS + тест (нужна до теста маршрута: критерий A2 «поиск
   `[PERSON_3]` находит только эту метку» проверяется через поиск); коммит.
5. **TR-4.3/4.4** — маршрут `/mcp/agent/<P>` + сокет-тест; коммит.
6. **TR-4.5/4.7** — пути и отставание индекса + тесты; коммит.
7. **Приёмка и документация.** Пересобрать release, замерить поиск на стенде A1 (см. §5),
   обновить `config.example.yaml`, `README.md` (раздел MCP/индекс), `RELEASE_NOTES_0.24.0.md`
   (или новый файл), при необходимости — `AGENT_A2_HANDOFF_PROMPT.md` §7.

**Стоп-точки (остановиться и согласовать):**
* фильтр `projects` не применяется к векторной ветке или к `snippet` (риск утечки между
  проектами) — это инвариант I5;
* нормализация меток ломает обычный поиск (регресс recall@10 на стенде) — откатить и согласовать
  формат токена;
* для `pending` требуется полноценный реестр (если владелец выберет вариант «реестр уже в A2»);
* маршрут на одном `mcp-http` конфликтует с текущей проверкой пути (`mp == mcp_path`) —
  проверить, что старый маршрут и `/health`, `/auth/login` не сломаны.

## 5. Критерии готовности A2 (план §6 + уточнения)

- [ ] `agent.enabled: true` → индексатор игнорирует `index.roots` и индексирует
      `<agent.root>\P*\workspace`; `agent-config`/`agent-state` не индексируются; настройки UI
      показывают «корни заданы агентом».
- [ ] `hds agent init` идемпотентен: останавливает watch, удаляет `index.db` (+`-wal`/`-shm`) и
      `data/anonymized`, создаёт пустую базу и запускает индексацию.
- [ ] `POST /mcp/agent/<P>`: свой токен → только фрагменты `P`; чужой/просроченный токен или
      чужой `P` → 401; `ask_my_files` и управляющих инструментов нет.
- [ ] Поиск `[PERSON_3]` находит только эту метку (не `[PERSON_30]`, не `[ORG_3]`).
- [ ] `index_status` по проекту: всего/проиндексировано/в очереди/ошибки + признак построения;
      предупреждение `pending` появляется и исчезает после индексации.
- [ ] `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace` (с `--test-threads=4`), python-тесты — зелёные.
- [ ] **Приёмочный замер:** стенд A1 (`tools/parity/out/agent_a1/s10/anon`, 32 файла) индексируется
      при `agent.enabled: true`, `hds search` по проекту `P0001` даёт `recall@10 ≥ 0,9`
      (`tools/parity/agent_a0_rag_eval.py` с тем же `golden.jsonl`), поиск `[PERSON_3]`
      возвращает фрагменты с этой меткой.

**Как повторить замер (A1 → A2):**

```powershell
# 1) временный конфиг (боевой config.yaml не трогать): HDS_CONFIG на копию,
#    index.roots убрать/оставить, agent.enabled: true, agent.root = ...\agent_a2\root,
#    db_path = ...\agent_a2\index.db
# 2) каталоги проектов: <agent.root>\P0001\workspace — копия anon-стенда A1
# 3) hds agent init --yes   (или hds index при готовом конфиге)
# 4) python tools\parity\agent_a0_rag_eval.py --hds target\release\hds.exe ^
#      --config <временный> --golden tools\parity\out\agent_a0\s10\golden.jsonl ^
#      --out tools\parity\out\agent_a2\eval.json
```

## 6. Грабли (A0/A1 + специфика A2)

* **FTS5 режет метки.** `[PERSON_3]` → `person` + `3`; нормализация нужна на **обеих** сторонах
  (индексация и запрос), иначе критерий TR-4.6 не выполнится. Суффикс `|дат` не должен менять
  токен.
* **Регулярка метки одна.** `\[([A-Z_]+)_(\d+)(?:\|([а-яё,]+))?\]` — `vault.LABEL_RE` (Python),
  `protocol::LABEL_PATTERN/label_parts` (Rust). Не писать третий вариант.
* **`[IMAGE_n]` — тип `IMAGE`.** Для индексации это обычный текст (файл должен находиться), для
  фильтра исходящего (A3) — пропуск. Не считать метку картинки секретом.
* **`projects` фильтр уже есть** в `hds_search::search`/`fts_search_ids`/`vec_search_ids` — A2 не
  пишет поиск заново, а **гарантирует** `projects=[P]` на маршруте агента и покрывает тестом
  (и FTS, и векторы).
* **`files.project`** заполняется из `project_for_path(&roots_from_cfg(cfg), path)` — после
  TR-4.1 там должны быть синтезированные корни, иначе весь фильтр по проекту «не увидит» файлы.
* **`hds stop` — это файл `index.stop`**, а не сигнал процессу; `init` обязан дождаться
  завершения watch, иначе удаление `index.db` упадёт на занятом файле (Windows).
* **PowerShell 5.1**: UTF-8 без BOM читается как ANSI; вывод `hds.exe` в консоли — кракозябры
  (разбирать `--json` или через файл + `Get-Content`). Пути с кириллицей — в кавычках.
* **Env живут между командами** в одной сессии (`HDS_CONFIG`, `HDS_MODELS_DIR`,
  `HDS_ANONYMIZE_PYTHON`) — задавать и снимать внутри одной команды.
* **`target\release\*.exe` занят** запущенным процессом — перед пересборкой остановить
  (`hds stop`, `hds mcp-http stop`, службы).
* **Секреты**: токен проекта в `secrets.json` — только sha256; plain-токен печатается один раз;
  ACL на `data\agent\secrets.json` — как у документа (В-1), в логи/аудит токен не писать.
* **Тайминг-тесты `hds-core::queue`** флейкуют при полной параллельности — гонять
  `cargo test --workspace -- --test-threads=4`.
* **Стенд**: образы A1 — в `tools/parity/out/agent_a1/s10/anon`; конфиг стенда — там же
  (`config.yaml`), он **временный**; боевые `index.db`, `config.yaml`, `data/anonymize.db` не
  трогать.
* **`agent.root` по умолчанию `D:\hds-anon`** — вне профиля пользователя; в тестах подменять
  временным каталогом и **не** создавать `D:\hds-anon` на машине без нужды.
* **keep_terms расширен (44)**: продукты 1С в образах не закрыты — поиск по «1С:ERP»/«ЗУП»
  должен работать; не добавлять их в стоп-слова FTS.

## 7. Правила коммита

* ветка `agent-privacy-zone`; **явный список путей** в `git add` (никаких `git add -A`);
* коммит по смыслу: roots/init/labels/route/lag/docs — раздельно, сообщение на русском в стиле
  проекта (`feat(agent): …`, `docs(agent): …`);
* **push — только по просьбе владельца**;
* артефакты экспериментов — только `tools/parity/out/**` (гитигнорится);
* не трогать боевые `config.yaml`, `index.db`, `data/anonymize.db`, `PLAN_*.md`;
* перед каждым коммитом: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace -- --test-threads=4`, python-тесты.

## 8. Вопросы к владельцу (нужны до/во время A2)

| # | Вопрос | Рекомендация |
|---|---|---|
| **В-7** | Отставание индекса (TR-4.7): минимальный реестр артефактов уже в A2 или `pending` по ФС + `index.db`, а полный реестр (TR-7.2) — A4? | `pending` по ФС + `index.db` (только чтение) в A2; реестр целиком — A4 |
| **В-8** | Список проектов в A2: каталоги `P*` в `agent.root` или `data\agent\projects.json` уже сейчас? | Каталоги `P*` (источник корней индекса); `projects.json` — A4 |
| **В-9** | Выдача токенов `/mcp/agent/<P>`: команда `hds agent token <P> [--ttl-days]` (sha256 в `secrets.json`, plain один раз)? | Да — иначе маршрут нечем проверить; UI-выдача — A5 |
| **В-10** | Блокировка правки корней в «Настройках» UI — A2 или A5? | Минимально в A2 (требует TR-4.1), полноценная вкладка — A5 |
| **В-11** | Формат нормализованной метки в FTS: `xperson3x`? | `xperson3x`; альтернатива — `person_3` в кавычках FTS |

## 9. Промт (скопировать целиком в новый чат, режим Act)

Ты — ведущий разработчик проекта hermes-disk-search (рабочая папка
`C:\Users\Sasha\hermes-disk-search`, Windows, PowerShell 5.1). Отвечай по-русски.

ЗАДАЧА: выполнить фазу A2 плана `PLAN_AGENT_PRIVACY_ZONE.md` — индекс и `disk-search`
по анонимной зоне (D9): корень индекса = `agent.root` (TR-4.1), `hds agent init`
(TR-4.2), маршрут `POST /mcp/agent/<P>` с токеном проекта и обязательным фильтром
`projects=[P]`, без `ask_my_files` и управляющих инструментов (TR-4.3/4.4), метки как
одно слово в FTS (TR-4.6), отставание индекса (TR-4.7). Фазы A0 и A1 закрыты — их
результаты обязательны к учёту.

СНАЧАЛА ПРОЧИТАЙ ЦЕЛИКОМ:
1. `AGENT_A2_HANDOFF_PROMPT.md` — карта фазы: состояние репозитория (§2), задачи с
   методикой (§3), порядок работ и стоп-точки (§4), критерии готовности (§5), грабли
   (§6), правила коммита (§7), вопросы владельцу (§8).
2. `PLAN_AGENT_PRIVACY_ZONE.md` — §3 (D3, D4, D9, D21, D23), §4.4 (раскладка), §5.4
   (TR-4.1…TR-4.7), §5.9 (I1–I8), §6 (строка A2), §9 (риски).
3. `tools/parity/AGENT_SPIKES.md` — §3.9 (морфология), §3.10 (стенд S10), §3.11 (правки
   плана), §5 (решения В-1…В-6).
4. Код: `crates/hds-core/src/roots.rs`; `crates/hds-index/src/{pipeline,walk,hash}.rs`;
   `crates/hds-search/src/{lib,fts}.rs`; `crates/hds-mcp/src/{http,server,schema,tools,auth}.rs`;
   `crates/hds-core/src/access.rs`; `crates/hds-cli/src/cmd/{mcp_http,index,stop}.rs`;
   `config.example.yaml`.
5. Контракт A1: `sidecar/hds_anonymize/{vault,handlers,file_parser}.py`,
   `crates/hds-anonymize/src/{protocol,client}.rs`, `crates/hds-cli/src/cmd/anonymize.rs`,
   `README_ANONYMIZATION.md`, `sidecar/README.md`, `RELEASE_NOTES_0.24.0.md`.
Затем составь и покажи короткий план (что, в каком порядке, стоп-точки) и сразу начинай.

ГЛАВНОЕ:
* Перед правками проверь `git status` и ветку: ожидается `agent-privacy-zone`, HEAD
  `c626a089`, дерево чистое. Чужие незакоммиченные правки **не откатывать, не стейджить,
  не переписывать**; если они появились и мешают — остановись и согласуй с владельцем.
* Коммить явным списком путей, разбивая по смыслу (roots → init → labels → route → lag →
  docs). Push — только по просьбе владельца.
* Инвариант I5: агент проекта P видит **только** фрагменты P. Фильтр `projects=[P]` на
  маршруте `/mcp/agent/<P>` обязателен и должен быть покрыт тестом и для FTS, и для
  векторной ветки. Чужой/просроченный токен или чужой P → **401**.
* `ask_my_files` и инструменты управления индексацией (`start_indexing`, `stop_indexing`,
  `reindex_path`, `forget`) на маршруте агента **не публикуются**.
* Метка `[PERSON_3]` должна искаться как одно слово: нормализация на индексации **и** в
  запросе; суффикс `|дат` токен не меняет; векторная ветка получает текст как есть.
* Отставание индекса — по критерию «запись в `files` с тем же путём и `content_hash` и
  успешным статусом»; `pending` в A2 считается по ФС + `index.db` без записи (полный реестр
  TR-7.2 — A4, см. вопрос В-7).
* Продуктовый код меняется только в `crates/` (и, при необходимости, в `config.example.yaml`
  и существующей странице настроек UI). Боевые `config.yaml`, `index.db`,
  `data/anonymize.db` не трогать; все временные базы, индексы и стенды — в
  `tools/parity/out/`.
* Контракт A1 не ломать: `vault.py` — единственный доступ к базе меток, RPC и CLI
  анонимизации остаются рабочими; `[IMAGE_n]` — тип `IMAGE`.

ПРОВЕРКИ И ЗАВЕРШЕНИЕ:
* `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace -- --test-threads=4`; python-тесты —
  `sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar`.
* Сокет-тест маршрута: токен P0001 → только P0001; чужой P/токен → 401; в `tools/list` нет
  `ask_my_files` и управления индексацией.
* Приёмочный замер: образы стенда A1 (`tools/parity/out/agent_a1/s10/anon`) положить в
  `<agent.root>\P0001\workspace`, `agent.enabled: true` на временном конфиге, `hds agent init`,
  затем `tools/parity/agent_a0_rag_eval.py` с тем же `golden.jsonl` — `recall@10 ≥ 0,9`
  (в A1 было 1,0); поиск `[PERSON_3]` возвращает фрагменты с этой меткой.
* Обнови `config.example.yaml` (секция `agent:`), `README.md` (MCP/индекс, корни агента),
  `RELEASE_NOTES_0.24.0.md` (или новый файл) и, при необходимости, §2–§5 этого файла.
* В конце дай сводку: что сделано, какие тесты/замеры, какие решения приняты, какие вопросы
  к владельцу (В-7…В-11).
