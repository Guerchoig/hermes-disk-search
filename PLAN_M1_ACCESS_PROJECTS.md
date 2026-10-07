# PLAN_M1_ACCESS_PROJECTS.md — детальный план M1 («Фундамент доступа»)

> Дополняет `PLAN_SERVER_MULTIUSER.md` §9 (milestone M1) и §2 W2/W5. Структура — по
> образцу `PLAN_W2_LLM_HOST.md`. Промт для нового чата — `M1_HANDOFF_PROMPT.md`.
>
> **Статус:** детализация готова, к исполнению. Ветка — **`server-multiuser`**.
> **Основание:** решения заказчика 07.10.2026 (шапка `PLAN_SERVER_MULTIUSER.md`):
> «Проекты» как единица доступа; **логин/пароль + авто-токены**; индексация — **только
> админ**; хранилище доступа — **YAML** (`data/access.yaml`).

## 1. Вводные (факты из кода, а не предположения)

| Факт | Где |
|---|---|
| `dig/load/Config`, `project_root()`, `db_abs_path` | `crates/hds-core/src/config.rs` |
| `index.roots` — **список строк**; читается `string_list(cfg,"index.roots")`; walker `WalkConfig::from_parts(roots, exclude_dirs, exclude_paths)` | `crates/hds-index/src/pipeline.rs`, `walk.rs` |
| Схема БД `files(id,path,ext,kind,size,mtime,content_hash,status,error,chunk_count,indexed_at)`; PRAGMA **уже** `journal_mode=WAL` + `busy_timeout=30000` | `crates/hds-core/src/db.rs` |
| Поиск `search(conn, emb, lem, cfg, query, kinds, limit)`; ветки **FTS** (`fts_search_ids`), **vec** (`chunks_vec` JOIN files), **CLIP** (`images_vec`); слияние RRF | `crates/hds-search/src/lib.rs`, `fts.rs` |
| MCP-http `route(method,path,body,mcp_path)` читает **только** method/path/body (заголовков нет); `server::handle(&msg)`, `schema::call(name,args)` | `crates/hds-mcp/src/{http,server,schema,tools}.rs` |
| UI: `run_http`, `ReqHeaders{origin,content_type,x_hds_ui}`, `csrf_ok` (loopback) | `crates/hds-ui/src/lib.rs`, `page.rs`, `assets/ui.html` |
| CLI: свой разбор argv; команды в `crates/hds-cli/src/cmd/*` | `crates/hds-cli/src/main.rs` |
| Ключи конфига: `mcp_http.*`, `db_path`, `embedding.dim`; `profile:` пока **никем не читается** (вводим) | `config.example.yaml` |

## 2. Границы M1

**Входит:**
- Понятие **«Проект»** в данных и в поиске (W5).
- **Идентичность:** пользователи (логин/пароль), авто-выдача токенов, проверка `Bearer`, admin-гейтинг.
- **Фильтрация выдачи** MCP по проектам токена (все ветки).
- **Админ-часть:** имена проектов корням, пользователи/доступы (CLI + минимальный UI).
- **Тесты**, включая утечку между проектами.

**Не входит (следующие этапы):** серверный профиль/службы (Э1); TLS/прокси, аудит,
rate-limit (сетевая часть Э2); внешняя чат-модель (Э6); клиентские/серверные пакеты
(Э8/Э9). **Десктоп-профиль сохраняет нынешнее поведение** (все проекты, без auth).

## 3. Технические решения (контракты)

### 3.1. Понятие «Проект»
- **Проект = строка** (имя), единица доступа и фильтра.
- `index.roots` — элемент: строка **или** объект `{path, project}`.
  ```yaml
  index:
    roots:
      - "E:\\Archive"                                   # project = "Archive"
      - { path: "D:\\Work\\Sales", project: "Sales" }    # явно
      - { path: "F:\\Reports",     project: "Sales" }    # тот же проект
  ```
- **Проект по умолчанию** = имя младшей папки корня; для **корня диска** — **метка
  диска с двоеточием** (`D:\` → `D:`). Пустой массив roots → нет проектов.
- Один файл принадлежит **одному** проекту: корень определяется по **наибольшему
  совпадающему префиксу** пути (как `exclude_paths` сейчас).
- Обратная совместимость: элемент-строка = проект по умолчанию.

### 3.2. Хранилище доступа — `data/access.yaml` (YAML, выбран)
```yaml
users:
  - name: admin
    login: admin
    password_hash: "$argon2id$..."   # Argon2id (или pbkdf2-sha256 как фолбэк)
    admin: true
  - name: alice
    login: alice
    password_hash: "$argon2id$..."
    projects: [Sales, Docs]
    disabled: false
tokens:                                # авто-выданные, хранятся ХЭШЕМ (sha256)
  - { token_sha256: "<hex>", user: alice, expires: 1798761600 }
```
- Файл **в `.gitignore`** (боевой). Рядом — `data/access.example.yaml` (в репо).
- Атомарная запись — через `hds_core::config::replace_file`.

### 3.3. Токен-флоу (login/password → token)
- `POST /auth/login` `{login, password}` → `200 {token, expires, user, projects}` или `401`.
- Токен — **opaque 256 бит** (32 случайных байта, hex/base64url). Хранится **хэш sha256**.
- Сверка пароля и токена — **постоянным временем** (`subtle`/сравнение без раннего выхода).
- TTL — из `access.token_ttl_days` (дефолт 90). Истёкший/отозванный → `401`.
- Приложение проверяет `Authorization: Bearer <token>` на `hds-mcp` и `hds-ui`.
- **Роли:** `admin: true` → доступ к UI и к управляющим инструментам; обычный
  пользователь → только `search_local_files`/`ask_my_files`/`index_status` (по своим проектам).

### 3.4. Схема индекса (формат меняется — БД дропаем)
- В `SCHEMA` (`db.rs`) у `files` добавить `project TEXT` + `CREATE INDEX idx_files_project ON files(project)`.
- Опц. таблица `projects(name TEXT PRIMARY KEY, roots_json TEXT)` — для админ-списка и
  обратного соответствия «проект → корни».
- **Совместимость не требуется** — старая БД удаляется и переиндексируется (согласовано).
- `db::stats` дополнить разбивкой по проектам (для админ-UI).

### 3.5. Присвоение проекта при индексации
- Новый хелпер (`hds-core`/`hds-index`): `roots_map(cfg) -> Vec<Root{path, project}>`
  + `project_for_path(&[Root], path) -> Option<String>` (наибольший совпадающий префикс).
- `walk`: каждый файл получает `project` из своего корня; для файла вне корней — `None`
  (в server-режиме такие невидимы; локально — видны всем).
- `pipeline`/`db` upsert — принять и записать `project`.
- `diag::check_roots` — вывести «корень → проект», предупредить о дублях/пустых именах.

### 3.6. Фильтрация поиска по проектам
- Расширить сигнатуру:
  `search(conn, emb, lem, cfg, query, kinds, projects: Option<&[String]>, limit)`.
  `None` = без фильтра (десктоп/CLI); `Some(&["Sales"])` = только эти проекты.
- Фильтр — в **каждой** ветке, единообразно:
  - **FTS:** JOIN `files` + `f.project IN (…)` (в `fts.rs`);
  - **vec:** в существующий JOIN `files` добавить `AND f.project IN (…)` (`lib.rs`);
  - **CLIP:** JOIN `files` (через `images_vec`→file) + `f.project IN (…)`.
- RAG (`rag::ask`) получает уже отфильтрованные результаты (контекст и **цитаты** — из них).
- **Единая точка:** список проектов собирается один раз и передаётся в ветки — чтобы
  «забытый JOIN» был невозможен (закрывается тестом на утечку).

### 3.7. Профиль и обратная совместимость
- Новый ключ `profile: desktop | server` (дефолт `desktop`) + `access.enabled` (дефолт `false`).
- **desktop:** auth выключен, фильтр проектов выключен (как сейчас) — UI/MCP без токена.
- **server:** MCP/UI требуют токен; MCP фильтрует по проектам пользователя; управляющие
  инструменты — только admin.

## 4. Задачи (треки)

### T1. Модуль доступа `hds-core::access` (1–2 дня)
- [ ] `crates/hds-core/src/access.rs`: типы `User`, `Token`, `Access`; `load/save`
      (`data/access.yaml`, атомарно через `replace_file`), `access_path(root)`.
- [ ] Хэш пароля: **Argon2id** (`argon2` crate, RustCrypto, чисто Rust) — `hash_password`
      / `verify_password`; фолбэк на `pbkdf2` + `sha2`, если `argon2` недоступен offline.
- [ ] Токены: `issue_token(user) -> (plain, Token)` (32 случайных байта через `getrandom`/`rand`),
      `resolve_token(plain) -> Option<AuthCtx>` (sha256 + constant-time), `revoke_token`,
      проверка `expires`.
- [ ] `AuthCtx { user: String, admin: bool, projects: Vec<String> }`.
- [ ] Тесты: логин успех/фейл, истечение, отзыв, admin-флаг, константное время.

### T2. Корни → проекты, схема БД (1–2 дня)
- [ ] `roots_map(cfg)` + `project_for_path` (default-имя, `D:\`→`D:`); поддержка
      элемента-строки и `{path, project}`.
- [ ] `db.rs`: `project` в `files` + индекс; `upsert/add_file` принимает `project`;
      `stats` по проектам.
- [ ] Индексатор (`hds-index`): проставляет проект при записи файла.
- [ ] `config.example.yaml` — пример формы корней.
- [ ] Тесты: парсинг обоих форм, default-имена (в т.ч. диск с двоеточием), выбор корня по префиксу.

### T3. Фильтрация поиска (2–3 дня)
- [ ] `hds-search::search(..., projects: Option<&[String]>, ...)` — фильтр в FTS/vec/CLIP.
- [ ] Обновить вызовы: `hds-cli` (search/ask — `None`), `hds-ui` (по контексту), `hds-mcp` (из токена).
- [ ] Тесты на утечку: файлы проекта A не видны при `projects=["B"]` (FTS, vec, CLIP, RAG-цитаты).

### T4. MCP: аутентификация и авторизация (2–3 дня)
- [ ] `hds-mcp::http`: `read_request` возвращает и **заголовки**; `route(..., headers)`.
- [ ] `server::handle(msg, auth)`; `schema::call(name, args, auth)`; `tools::*` принимают
      `projects`.
- [ ] Правила: `access.enabled=false` (desktop) → как сейчас; `true` → `401` без валидного
      `Bearer`; управляющие инструменты — `admin` только.
- [ ] `POST /auth/login` — эндпоинт в MCP-http (или отдельный мини-сервер); `GET /health`
      остаётся публичным.
- [ ] Stdio-режим: токен не нужен (процесс локальный) — десктоп-поведение сохраняется.
- [ ] Тесты: `401`/`200`, admin-гейтинг, фильтр по проектам токена.

### T5. UI (админ) и CLI (2–3 дня)
- [ ] UI (`hds-ui`): `ReqHeaders.authorization`; в server-режиме — только админ-токен;
      `csrf_ok` для server-режима = валидный админ-токен (loopback-проверку снять/ослабить).
- [ ] UI-страница: карточка **«Проекты»** — список корней с полем имени проекта
      (опц.), список проектов; карточка **«Пользователи»** — создание (login/password/projects),
      отзыв токенов.
- [ ] CLI (`hds-cli`): `hds user add|list|del|passwd|grant|revoke`, `hds token list|revoke`,
      `hds login [--url ...]` (клиентский помощник: login/password → токен).
- [ ] Тесты CLI: создание пользователя, выдача/отзыв токена, `login`.

### T6. Миграция/доклад (0,5 дня)
- [ ] `data/access.example.yaml` в репо; `data/access.yaml` — в `.gitignore`.
- [ ] Пометка «переиндексация обязательна» в `README`/`hds check`.
- [ ] Обновить `PLAN_SERVER_MULTIUSER.md` §9 (статус M1), `STATUS.md`.

## 5. Приёмка M1 (черновик)

- **Утечка:** при `projects=["Sales"]` выдача **не** содержит файлов других проектов —
  во всех ветках (юнит-тесты на FTS/vec/CLIP + интеграционный RAG-ответ с цитатами).
- **Доступ:** без токена — `401`; с токеном пользователя — только его проекты; с токеном
  admin — всё + управляющие инструменты. UI открывается только с админ-токеном.
- **Проекты:** корень `D:\` → проект `D:`; корень-строка → имя младшей папки; два корня —
  один проект; файл вне корней не виден в server-режиме.
- **Обратная совместимость:** `profile: desktop` (дефолт) — нынешнее поведение без токена
  и без фильтра; `cargo test --workspace` зелёный; `cargo clippy -- -D warnings` и `fmt` — чисто.
- **Токены:** логин/пароль → токен; истечение и отзыв → `401`; пароль хранится хэшем.

## 6. Порядок работ и коммиты

Порядок (по зависимостям), по одному коммиту на шаг:
1. `hds-core::access` (T1) — изолированно, тесты.
2. Корни→проекты + схема (T2) — с тестами парсинга/префикса.
3. Фильтрация поиска (T3) — тесты на утечку **до** подключения auth.
4. MCP auth + login (T4).
5. UI + CLI (T5).
6. Миграция/доклад (T6).

Коммиты — в стиле репозитория: `feat(access): ...`, `feat(index): project field and roots->project`,
`feat(search): project filter in all branches`, `feat(mcp): bearer auth and project scoping`,
`feat(cli): user/token/login commands`. После каждого шага — тесты + обновление журнала.

**Рекомендованный порядок в чате:** начать с T1+T2 (данные), затем T3 (фильтр) — это уже
проверяемо без auth, затем T4 (auth поверх фильтра), затем T5, T6.

## 7. Риски
| Риск | Митигация |
|---|---|
| Забытая ветка без фильтра (утечка) | единая точка сбора проектов; тесты на каждую ветку |
| `argon2`/`getrandom` недоступны offline | фолбэк `pbkdf2`+`sha2`; проверка `cargo fetch` заранее |
| Ломается десктоп-поведение | `profile: desktop` по умолчанию, `access.enabled=false`; регресс-тесты |
| Пустые/дублирующиеся имена проектов | `diag::check_roots` предупреждает; default-имя всегда непусто |
| Смена схемы БД | согласованный дроп + переиндексация; явная пометка в диагностике |
| Токен в конфиге Cline виден локально | TLS на прокси (Э2); в server-режиме только HTTPS |

## 8. Точки входа по файлам (шпаргалка)
- `crates/hds-core/src/{access.rs(new),config.rs,db.rs,lib.rs}`
- `crates/hds-index/src/{pipeline.rs,walk.rs,diag.rs}`
- `crates/hds-search/src/{lib.rs,fts.rs,rag.rs}`
- `crates/hds-mcp/src/{http.rs,server.rs,schema.rs,tools.rs}`
- `crates/hds-ui/src/{lib.rs,page.rs}`, `assets/ui.html`
- `crates/hds-cli/src/{main.rs,cmd/auth.rs(new)}`
- `config.example.yaml`, `data/access.example.yaml(new)`, `.gitignore`

## 9. Ссылки
- `PLAN_SERVER_MULTIUSER.md` — §2 W2/W5, §5 Э2/Э3, §9 (M1).
- `M1_HANDOFF_PROMPT.md` — промт для нового чата.
- `MIGRATION_PLAN_RUST.md` — карта кода крейтов.
