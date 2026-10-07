# M2_CODE_REVIEW.md — код-ревью этапа M2 «Серверный профиль и службы»

> Дата: 07.10.2026 · Ветка: **`server-multiuser`** · HEAD: `5ecf39f`
> Объём: коммиты `243a028`…`5ecf39f` (T0–T5), изменения —
> `.gitignore`, `crates/hds-core/src/{config,access}.rs`, `crates/hds-ui/src/lib.rs`,
> `crates/hds-cli/src/{main.rs,cmd/{ui,mcp_http,auth,service}.rs}`, `crates/hds-index/src/diag.rs`,
> `crates/hds-llama/src/host.rs`, `installers/install_server.*`, `run_ui.ps1`,
> `config.example.yaml`, `README*.md`, `STATUS.md`.
> Метод: чтение кода + `cargo test --workspace`, `cargo fmt --all --check`,
> `cargo clippy --workspace --all-targets -- -D warnings`.

## 1. Итог

Этап выполнен добротно: все шесть треков (T0–T5) закрыты по одному коммиту на шаг,
десктоп-профиль не изменён, **`cargo test --workspace` — зелёный, `fmt`/`clippy`
(`-D warnings`) — чисто**. Дефекты M1 из прошлого ревью исправлены по-настоящему
(доказаны рантаймом + сокет-тест, который действительно ловит дефект). Ниже —
находки; критичных блокеров нет, но есть один явный баг (F1),
расхождения с контрактом плана (F3/F4) и набор рисков/мелочей.

## 2. Что сделано хорошо (подтверждено)

- **T0.1 (парсинг `Authorization` в UI).** `read_request` заполняет
  `ReqHeaders.authorization` (`crates/hds-ui/src/lib.rs:1597`); `run_http`
  разделён на `run_http`/`run_http_listener` ради теста; **сокет-тест**
  `ui_auth_socket_parses_authorization` (`crates/hds-ui/tests/ui_auth.rs:205`)
  биндит порт `0`, ходит сырым HTTP и проверяет 401 без токена / 200 с админ-Bearer
  и case-insensitive ключ — именно то, что чистая функция `route` поймать не могла.
- **T0.2.** `.gitignore`: `data/` → `data/*` + `!data/access.example.yaml`
  (ре-инклуд внутри исключённого каталога невозможен). `git ls-files data/` даёт
  `data/access.example.yaml`; живой `data/access.yaml` игнорируется.
- **T0.3.** `hds login` собирает тело через `serde_json::json!`
  (`crates/hds-cli/src/cmd/auth.rs:345`), есть юнит на `"`,`\`, переводы строк.
- **T1/T2/T3.** Резолверы `profile()`, `state_dir()`, `system_state_dir()`,
  `ensure_state_dir()` — маленькие, документированы; `HDS_ROOT` сильнее всего;
  `db_abs_path`/`access_path`/PID/логи/heartbeat сведены к `state_dir`; тесты
  профиля/путей; `hds check` показывает фактические пути.
- **T4.** `hds service run|install|…` с SCM-крейтом (спайк закрыт), graceful-фолбэк
  по коду, macOS LaunchDaemon, инсталляторы; `daemon_plist`/`sc_args` покрыты юнитами.
- **T5.** `run_ui.ps1` headless в server-профиле; README/STATUS/план обновлены.

## 3. Находки

### F1 — [Баг] `DisplayName` службы Windows — литерал `<role>` (виден в `services.msc`)

`crates/hds-cli/src/cmd/service.rs:239`:

```rust
"DisplayName= \"Hermes Disk Search: <role>\"".into(),
```

`<role>` — **строковый литерал**, а не подстановка. Все четыре службы получат
отображаемое имя «Hermes Disk Search: `<role>`». Форматируется только `binPath`
выше. Нужно `format!("DisplayName= \"Hermes Disk Search: {role}\"")`.
Тест `sc_install_args_quote_binpath` проверяет только `binPath`, поэтому дефект не
пойман. Рекомендация: проверять в тесте и `DisplayName`.

### F2 — [Риск/безопасность] `ui_auth_gate` — fail-open при нечитаемом конфиге

`crates/hds-ui/src/lib.rs:1295-1298`: если `load()` вернул ошибку — функция
отдаёт `Ok(false)` (гейт выключен). В server-поставке временно нечитаемый (или
битый по YAML) `config.yaml` **молча открывает UI без токена**. Для десктопа это
осознанный компромисс, но на сервере лучше fail-**closed** (например, если
присутствует системный конфиг/`profile: server`). Как минимум — предупредить в логе.

### F3 — [Несогласованность] `HDS_ROOT` не учитывается в `llm-host`

План M2 §3.2: «`HDS_ROOT` переопределяет всё». В `hds-core::config::state_dir`
(`config.rs:188`) это так. Но `hds-llama::host::resident_data_root` (`host.rs:105`)
и `HostConfig::default` (`host.rs:295+`) **`HDS_ROOT` не читают**: только
`profile: server` → системный каталог, иначе корень проекта. Следствия:
(а) тестовая изоляция через `HDS_ROOT` не переносит pid/лог/heartbeat резидента;
(б) при переносе установки через `HDS_ROOT` состояние **расщепляется** (БД/PID/access
идут за `HDS_ROOT`, резидент — за профилем). Тест `resident_data_root_follows_profile`
покрывает desktop/server/garbage, но не `HDS_ROOT`. Фикс — зеркально повторить
проверку `HDS_ROOT` в `host.rs`.

### F4 — [Несогласованность] `run_ui.ps1` жёстко фиксирует порт `8765`

`run_ui.ps1:8,48`: `$uiPort = 8765`, проверка доступности именно `8765` и запуск
`hds ui --port 8765` — это **отменяет T3**: при server-профиле с другим `ui.port`
скрипт поднимет UI на 8765, а не на сконфигурированном порту. T3 проверялся
запуском `hds ui` без флагов (там всё ок), но типовой путь «через `run_ui.ps1`»
порт из конфига игнорирует. Фикс: не передавать `--port`/распарсить `ui.port`.

### F5 — [Риск/остановка] Абруптный `std::process::exit(0)` в обработчике Stop

`crates/hds-cli/src/cmd/service.rs:268-278`: остановка службы завершает процесс
жёстко (роли — блокирующие циклы без канала остановки). Комментарий это признаёт,
но следствие: `mcp_http.pid` может остаться, сокеты/временные файлы не чистятся.
Для SQLite (коммит на файл) потери данных нет, но засорение PID-файлов возможно
(при старте логика должна их перезаписывать — проверить). Рекомендация: в M3
добавить удаление PID-файлов `mcp-http`/`ui` на выходе роли.

### F6 — [Мелочь/дублирование] Два экземпляра `system_state_dir`

`hds-core::config::system_state_dir` и `hds-llama::host::system_state_dir` —
дубликаты (обосновано: `hds-llama` не зависит от `hds-core`), плюс имя каталога
`"hermes-disk-search"` продублировано как литерал. Риск «расхождения при правке».
Допустимо; стоит вынести `STATE_DIR_NAME` в общий крейт или добавить тест-сверку.

### F7 — [Безопасность/UX] Пароль в аргументах `hds login --password`

`crates/hds-cli/src/cmd/auth.rs:352`: пароль идёт как аргумент CLI — виден в списке
процессов и истории шелла. Лучше `--password-stdin`/интерактивный ввод.

### F8 — [Риск/DDoS] Неограниченные тела запросов и поток-на-соединение

И в `hds-ui/src/lib.rs` (`read_request`, тело до `content_length`), и в
`hds-mcp/src/http.rs:130` ограничен только размер **заголовков** (1 МБ), а тело —
нет; серверы поднимают поток на каждое соединение. За reverse-proxy с лимитом тела
смягчается, но в M3 стоит (а) ограничить request-body, (б) не плодить потоки без
предела (или пул). Связано с rate-limit.

### F9 — [Наблюдение] `install_server.ps1` снимает задачу автотранскрибации без замены

`install_server.ps1:69`: удаляет в т.ч. `HermesDiskSearchTranscribe`, но среди
ролей-служб (`watch|mcp-http|ui|llm-host`) транскрибации нет — задача снимается и
**не** заменяется службой. Если в server-профиле нужна автотранскрибация, она тихо
отключится. Либо завести роль/службу для транскрибации, либо не снимать задачу
(или документировать явно). Проверить у заказчика.

### F10 — [Мелочи]

- **F10.1.** `config.example.yaml:139-140`: ключ `profile:` размещён под
  комментарием «# --- Доступ (M1) ---», хотя относится к секции M2.
- **F10.2.** macOS LaunchDaemon-плейст (`daemon_plist`) не задаёт
  `StandardOutPath`/`StandardErrorPath` — логи демона теряются (для Э10 желательно
  указать пути в `state_dir/data/logs`).
- **F10.3.** `current_service_role()` (`service.rs:258`) — эвристика поиска `"run"`
  в `argv`; тестом не покрыта (поведение корректно, но хрупко).
- **F10.4.** `access_path()` (`crates/hds-core/src/access.rs`) при каждой проверке
  читает и парсит `config.yaml` (`load()`) — лишняя работа на каждый вызов гейта.

## 4. Соответствие приёмке M2 (§5 плана)

| Пункт приёмки | Статус | Замечание |
|---|---|---|
| Службы стартуют без входа; `status` отражает SCM/launchd | ✅ | полный SCM-цикл — только из-под админа; юниты покрывают сборку аргументов |
| Пути (server) в ProgramData/Application Support; desktop без изменений | ✅ | рантайм-проверено в T2; см. F3 |
| UI (server): 200 с токеном / 401 без — рантайм | ✅ | сокет-тест T0.1 + рантайм; см. F2 |
| Порты: `hds ui` слушает `ui.host:ui.port` | ⚠️ | `hds ui` — да (T3); `run_ui.ps1` порт жёстко задаёт (F4) |
| Обратная совместимость, тесты, fmt/clippy | ✅ | подтверждено прогоном |
| M1-дефекты закрыты | ✅ | `access.example.yaml` в репо; `login` экранирует |

## 5. Рекомендации к следующему шагу

Быстрые фиксы (track T0 этапа M3): **F1** (DisplayName), **F4** (`run_ui.ps1` порт
из конфига), **F3** (`HDS_ROOT` в `llm-host`), **F8** (лимит тела — часть ложится
на периметр). Среднесрочно: **F2** (fail-closed для server), **F5** (очистка PID),
**F9** (транскрибация) — уточнить у заказчика. Косметика: **F6/F7/F10**.

Итоговая оценка: **принять с замечаниями**. Блокеров нет; F1 — единственный
однозначный дефект, F3/F4 — расхождения с заявленным контрактом, остальное — риски
и техдолг, часть закрывается треком T0 следующего этапа (`PLAN_M3_NETWORK_PERIMETER.md`).


