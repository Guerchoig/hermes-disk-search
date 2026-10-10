# PLAN_M7_UI_CONTROL.md — детальный план M7 «Управление службами и доступами из UI» (Э7/W10)

> Дополняет `PLAN_SERVER_MULTIUSER.md` §2 (**W10**), §5 (**этап Э7**), §9.
> Структура — по образцу `PLAN_M6_GPU_MULTIVENDOR.md`; код-ревью предыдущего этапа —
> `M6_CODE_REVIEW.md`.
>
> **Основание:** M1–M6 закрыты. По дорожной карте §5 после **Э5** (M6) и **Э6** (M5)
> идёт **Э7** «Управление службами и токенами из UI (W10)». M6 прошёл код-ревью
> (`M6_CODE_REVIEW.md`, принят; в хвостах — M6-1 и решение по F-m6-t5-1).
> **Статус:** запланирован 10.10.2026. Ветка — **`server-multiuser`**.
>
> **Зависимости:** M1 (доступ: `data/access.yaml`, токены, `/api/access/*`, `hds user|token`),
> M2 (`hds service` + каталог состояния server), M3 (аудит `admin_op`, CSRF на POST UI,
> `{error,hint}`), M4 (`/api/llm-host/restart`, `/api/mcp-http`), M6 (ревью-хвосты).

## 1. Вводные (факты из кода, а не предположения)

| Факт | Где |
|---|---|
| **Службы уже есть, но только в CLI:** `hds service run|install|uninstall|start|stop|status <role>`, роли `watch|mcp-http|ui|llm-host`; Windows — SCM через `windows-service` (+ `sc.exe` для управления, автостарт/авторестарт), macOS — plist LaunchDaemon + `launchctl`; имена `hermes-disk-search-<role>` / `com.hermes-disk-search.<role>` | `crates/hds-cli/src/cmd/service.rs` (527 строк) |
| **UI не имеет доступа к коду служб:** `hds-ui` зависит от `hds-core/index/search/extract/llama`, но **не** от `hds-cli`; наоборот, `hds-cli` зависит от `hds-ui`. Прямой вызов `hds_cli::cmd::service::*` из UI дал бы цикл | `crates/hds-ui/Cargo.toml`, `crates/hds-cli/Cargo.toml` |
| **UI управляет демонами, а не службами:** `POST /api/watch/daemon` (detached `hds watch` + `watch.lock`/`index.stop`), `POST /api/mcp-http` (через CLI `hds mcp-http <action>`), `POST /api/llm-host/restart` (`/internal/stop` + повторный spawn), `POST /api/transcribe/daemon` | `crates/hds-ui/src/lib.rs` (`watch_json` ~680, `mcp_http_action` ~1237, `llm_host_restart` ~826, `transcribe::daemon_json`) |
| **Состояние MCP/watch читается из локальных признаков:** проба `/health` порта MCP, `data/mcp_http.pid` (+ `process_alive`), `watch.lock`; **состояние OS-службы (SCM/launchd) не читается вовсе** | `crates/hds-ui/src/lib.rs` (`mcp_http_status` ~1201, `watch_status` ~606) |
| **Доступ в UI — пользователи/проекты/отзыв, но нет выдачи токена:** `GET /api/access/users|projects`, `POST /api/access/user/{add,del,passwd,grant}`, `POST /api/access/token/revoke`; выдачи токена из UI **нет** | `crates/hds-ui/src/lib.rs` (~1497, ~2027–2191) |
| **Выдача токена есть только через логин и внутри ядра:** `POST /auth/login` → `authenticate` → `new_token` (в файл кладётся хэш); CLI `hds token` умеет только `list|revoke` (нет `new`) | `crates/hds-mcp/src/auth.rs::login` (~96), `crates/hds-cli/src/cmd/auth.rs::cmd_token` (~264), `crates/hds-core/src/access.rs::new_token` (~201) |
| **Управляющие POST UI уже под admin-гейтом и CSRF:** `ui_auth_gate` (server-режим: Bearer admin), `is_admin_op = POST /api/*`, аудит `admin_op` | `crates/hds-ui/src/lib.rs` (~1424–1431), `crates/hds-core/src/audit.rs` |
| **Профиль:** службы — только `profile: server` (ставит `installers/install_server.ps1`, запускается от администратора/LocalSystem); desktop-профиль — автозапуск Планировщиком, служб нет | `crates/hds-core/src/config.rs` (`Profile`, ~210), `installers/install_server.{ps1,command}`, `PLAN_M2_SERVER_PROFILE.md` §4 |
| **Права:** управление SCM требует прав администратора; UI-служба идёт от LocalSystem (права есть), `hds ui` в desktop/пользовательском режиме — прав нет (нужна честная ошибка, а не падение) | `PLAN_M2_SERVER_PROFILE.md`, `hds-cli::cmd::service::scm` |
| **Ревью-хвосты M6 в зоне досягаемости:** M6-1 (`hds check` `gpu_roles` не читает `llm.<role>.device`), M6-4 (косметика `CUDA ,`), F-m6-t5-1 (клиенты всегда шлют `gpu`) | `M6_CODE_REVIEW.md` §3 |

## 2. Границы M7

**Входит:**
- единый модуль управления службами, доступный и CLI, и UI (без цикла зависимостей);
- API `/api/services` в UI: состояние + `install|uninstall|start|stop|restart` для ролей
  `watch|mcp-http|ui|llm-host`;
- выдача токенов: CLI `hds token new` и UI `POST /api/access/token/new` (плюс отзыв уже есть);
- UI-карточка «Службы» и доработка карточки «Доступ» (выдать токен);
- честные ошибки прав/профиля (desktop без служб; не-админ — `403`-подобный отказ с hint);
- хвосты код-ревью M6, критичные для эксплуатации (T0).

**Не входит (следующие этапы):** пакеты установки Win/macOS (Э8/Э9/W6/W7);
эксплуатация `healthz`/бэкап/обновление (Э10/W8); нагрузочная приёмка на 1/2 картах +
Apple Silicon (Э11); анонимизация внешнего API (W9); новые роли `llm-host`
(раскладка — M6, не трогаем).

**Обязательно:** **десктоп-профиль не меняется** (служб нет — карточка «Службы» честно
об этом сообщает и не предлагает действий); существующие `/api/watch/daemon`,
`/api/mcp-http`, `/api/llm-host/restart`, `/api/transcribe/daemon` **сохраняются**
(`/api/services` — добавление, а не замена; при совпадении действий UI может
делегировать, но контракты старых точек не ломаем).

## 3. Технические решения (контракты)

### 3.1. Общий модуль служб (единый код CLI ↔ UI)

Код служб сейчас в `hds-cli`, а UI не может его импортировать (цикл). Решение — вынести
**общую (непривилегированную) часть** в нижний крейт. Предлагается `hds-core::service`
(`hds-core` уже держит `config`/`state_dir`/`access`/`audit` и его импортируют все):

* переносится из `hds-cli/src/cmd/service.rs`: `ROLES`, `service_name`, `daemon_label`,
  `parse_role`, сборка аргументов `sc.exe` (`sc_args`), генерация `daemon_plist`, а также
  **чтение состояния** службы (SCM query через `sc query <name>` / `launchctl print
  <label>`) и **управление** (`install|uninstall|start|stop` — вызов `sc.exe`/`launchctl`);
* `hds-cli::cmd::service` остаётся тонкой обёрткой (разбор argv, exit-коды, `service run`
  с SCM-диспетчером и `run_llm_host` — это по-прежнему в CLI, т.к. тянет роли);
* юнит-тесты `service.rs` (`roles_and_names`, `sc_install_args_quote_binpath`,
  `plist_has_required_keys`, `role_from_args_reads_service_role`) переносятся к общему коду;
* контракт роли: `{ role, installed: bool, state: "running|stopped|absent|unknown",
  autostart: bool, detail: String }`;
* **тонкость:** `sc_args("install")` сейчас берёт `std::env::current_exe()` для `binPath` —
  при выносе это стало бы exe **вызывающего** (UI), а не `hds.exe`. Общий код обязан
  принимать путь к `hds.exe` параметром (`sc_args(sub, role, hds_exe: &Path)`); как искать
  `hds.exe` из UI — переиспользовать существующий `hds-ui::hds_exe()` (рядом/`bin`/`target/…`).

> Альтернатива (если перенос `windows-service` в `hds-core` нежелателен) — отдельный
> крейт `hds-service` в workspace. Выбор фиксируется в T1.

### 3.2. API `/api/services` (UI)

* `GET /api/services` → `{ "profile": "desktop|server", "services": [ {role, kind,
  state, autostart, detail}, … ], "hint": "…" }`, где `kind` — `"service"` (SCM/launchd)
  или `"absent"`; в desktop-профиле `services` пуст, `hint` объясняет почему.
* `POST /api/services {"role": "<…>", "action": "install|uninstall|start|stop|restart"}`
  → `{ "ok": bool, "msg": "…", "state": <как в GET> }`; под admin-гейтом/CSRF/аудитом
  `admin_op` (как остальные управляющие POST).
* `restart = stop + start`; **перезапуск роли `ui`** (UI сам себя) — отдельный путь:
  ответ отдаётся, затем перезапуск выполняется отложенно (detached helper) **или**
  делегируется SCM-авторестарту; если прав нет — честная ошибка с hint, без падения UI.
* Ошибки: нет прав (`sc.exe` → 5 / `Access is denied`) → `{ok:false, msg, hint:"запустите
  UI от администратора или из службы LocalSystem"}`; служба не установлена →
  `{ok:false, msg:"служба не установлена — сначала install"}`; неизвестная роль/
  действие → `400`-подобный `{error,hint}`.
* На macOS — `launchctl`; на прочих ОС (linux/dev) — `state:"unknown"` с честным
  сообщением (службы не поддерживаются, как в `hds service`).

### 3.3. Выдача токенов (CLI + UI)

* ядро уже умеет: `hds_core::access::new_token(user, ttl_days, now_unix)` →
  `(plain, Token)` (в файл — только хэш); TTL — `token_ttl_days(cfg)`;
* **CLI:** `hds token new --user <login> [--ttl-days N]` → печатает **plain-токен один раз**
  + срок; аудит `admin_op` (как `token revoke`); отказ, если пользователя нет;
* **UI:** `POST /api/access/token/new {"user": "<login>", "ttl_days": N?}` →
  `{ "token": "<plain>", "expires": <unix>, "user": "…" }`; под admin-гейтом/CSRF; аудит
  `admin_op`; plain-токен **не** пишется в лог/аудит (в аудите — только факт и пользователь);
* отзыв — уже есть (`/api/access/token/revoke`, `hds token revoke`), не трогаем.

### 3.4. UI: карточки «Службы» и «Доступ»

* новая карточка (вкладка/блок) «Службы» в `page.rs`: таблица ролей (состояние, автозапуск),
  кнопки `install/uninstall/start/stop/restart`, автообновление статуса; в desktop-профиле —
  сообщение «службы только в server-профиле» без кнопок;
* карточка «Доступ»: кнопка «Выдать токен» (пользователь + TTL) с показом plain-токена
  **один раз** (копировать), как это делается для секретов; кнопка отзыва — уже есть;
* все POST — через существующий `act()` (CSRF/token), стилистика и раскладка — как в
  текущей странице (без внешних ассетов).

### 3.5. Профили и права (единые правила)

* server-профиль + UI-служба (LocalSystem) — управление службами разрешено;
* server-профиль, но UI запущен от непривилегированного пользователя — управление
  возвращает честную ошибку прав (не падает, не «делает вид»);
* desktop-профиль — `/api/services` отвечает «служб нет», действия отклоняются;
* выдача/отзыв токенов — всегда под admin-гейтом (server-режим), в desktop — как раньше
  (локально, без доступа); секреты не печатаются в лог/аудит/`hds check`.

## 4. Треки

### T0. Хвосты код-ревью M6 — 0,5–1 дн — ✅ **выполнено 10.10.2026 (`14567a8`)**
- [x] **M6-1**: `crates/hds-index/src/diag.rs::role_device_from_cfg` читает
      `llm.<role>.device` **первым**, затем `llm_server.<role>.device` (зеркально
      `config.rs::build_role`); юнит `role_device_prefers_llm_key` (карта показывается,
      фолбэк сохранён, двойное назначение = 2 источника, отрицательный индекс игнорируется).
- [x] **M6-4**: `CUDA ,` при пустом `description` убран (склейка backend/desc non-empty).
- [x] **F-m6-t5-1** (M6-2): решение — **поведение не меняем** (ждёт владельца), ограничение
      «транскрибация шлёт `gpu` → host-выбор карты не срабатывает» уже зафиксировано в
      `README_SERVER_MULTIUSER.md` §8b (M6 T6); код `transcribe.rs` не тронут.
- [x] `cargo test --workspace` — **88 наборов / 0 failed**; `fmt`/`clippy -D warnings` чисто.
- **Следующий шаг — T1.** Боевой стенд (`watch`/`mcp-http`/`transcribe-watch`) останавливался
  на время тестов и **возвращён**; `index.pause` цел, резидент `llm_host` жив.

### T1. Общий модуль служб (рефакторинг без смены поведения) — 1–1,5 дн — ✅ **выполнено 10.10.2026**
- [x] Общая часть служб вынесена в **`hds-core::service`** (§3.1): роли/имена (`ROLES`,
      `service_name`, `daemon_label`, `parse_role`), `sc_args`, `daemon_plist`, чтение состояния
      (`status` — SCM `sc query`/`sc qc`, launchd `launchctl print`) и управление
      (`control` — `sc.exe`/`launchctl`); `hds-cli::cmd::service` — тонкая обёртка (реэкспорт
      имён для совместимости). Ключевое: `sc_args(sub, role, hds_exe: &Path)` принимает путь к
      `hds.exe` параметром (в general-коде `current_exe()` указал бы на exe вызывающего — UI).
- [x] `service run` (SCM-диспетчер `windows-service`, `run_llm_host`) остался в CLI;
      `windows-service` в `hds-core` не тянется (управление идёт через `sc.exe`).
- [x] Юнит-тесты сохранены/перенесены: `roles_and_names`, `sc_install_args_quote_binpath`,
      `plist_has_required_keys` — в core; `role_from_args_reads_service_role` — в CLI. Добавлены
      чистые кросс-платформенные `parse_sc_query_state`/`parse_sc_qc_autostart`.
- [x] Контракты для T2/T3: `ServiceStatus` (`role/kind/state/autostart/detail`) и
      `ControlResult` (код + раздельные потоки + `permission_denied`).
- [x] Поведение `hds service …` без изменений (регрессия): `status <role>` — живое состояние
      (`sc 1060` → exit 1), неизвестная подкоманда/нет роли — exit 2 (проверено на стенде).
- [x] `cargo test --workspace` — **88 наборов / 0 failed**; `fmt`/`clippy --all-targets -D warnings`
      чисто; `hds check` (desktop) — ok. Стенд (`watch`/`mcp-http`/`transcribe-watch`)
      останавливался на тесты и **возвращён**; `index.pause` цел, резидент `llm_host` жив.
- **Следующий шаг — T2** (`/api/services` в UI).

### T2. `/api/services` в UI — 1,5–2 дн — ✅ **выполнено 10.10.2026 (`112d6ee`)**
- [x] `GET /api/services` (§3.2): профиль + 4 роли (`role/kind/state/autostart/detail` из
      `hds_core::service::status`), `hint`; desktop → пустой список + объяснение.
- [x] `POST /api/services {role, action}`: `install|uninstall|start|stop|restart` через
      `hds_core::service::control` (путь `hds.exe` — существующий `hds_ui::hds_exe()`);
      под admin-гейтом/CSRF и аудитом `admin_op` (штатный блок `is_admin_op`, без правок).
- [x] Валидация входа: неизвестная роль/действие → `400 {error, hint}`. Ошибки: нет прав
      (`Access denied`/код 5) → hint «запустите UI от администратора/LocalSystem»;
      служба не установлена (`sc 1060`) → `{ok:false, msg, hint, state}`.
- [x] Особый случай `ui` (UI сам себя): `restart` отвечает сразу, затем перезапуск —
      detached helper (`cmd` → пауза → `sc stop`/`start`, ~2 c), страница не виснет; при
      сбое запуска helper — честная ошибка с hint.
- [x] Юнит-тесты (`tests/ui_core.rs`): desktop (пусто + отказ, без SCM), server (4 роли/поля),
      валидация (`400` с hint), роутер `GET /api/services`.
- [x] Живой прогон в песочнице (`HDS_CONFIG`): desktop — `GET` пусто+hint, `POST` отклонён;
      server — `GET` 4 роли (`absent`), `POST start` → `ok:false` «не установлена» + `state`.
      `cargo test --workspace` — **88 наборов / 0 failed**; `fmt`/`clippy --all-targets -D warnings`
      чисто. Артефакты — `tools/parity/out/m7_t2_live_{desktop,server}.txt`.
- **Следующий шаг — T3** (выдача токенов: CLI `hds token new` + UI `/api/access/token/new`).

### T3. Выдача токенов (CLI + UI) — 1–1,5 дн — ✅ **выполнено 10.10.2026 (`8980338`)**
- [x] CLI `hds token new --user <login> [--ttl-days N]` (§3.3): plain-токен печатается
      **один раз** + срок; TTL из `--ttl-days` либо `access.token_ttl_days`; отказ для
      несуществующего/отключённого логина (exit 1); аудит `admin_op`. Ядро — `token_new_issue`
      (в файл только `token_sha256`).
- [x] UI `POST /api/access/token/new {user, ttl_days?}` → `{token, expires, user}`; под штатным
      admin-гейтом/CSRF и аудитом `admin_op` (без токена); отказ для неизвестного логина (`400`).
- [x] Проверено живым прогоном (песочница `server` + `access.enabled`): CLI выдал токен, `ghost`
      → отказ (exit 1); admin Bearer `GET /api/services` → `200`, без токена → `401`; выданный
      через UI токен работает (`200`), отзыв по пользователю → последующий запрос `401`; аудит
      содержит `admin_op` (CLI `hds token new` и UI `/api/access/token/new`) и **не содержит**
      plain-токенов. Артефакты — `tools/parity/out/m7_t3_{cli_issue,live}.txt`, `m7_t3_audit.jsonl`.
- [x] Юниты: CLI `token_new_issue` (хэш сохранён, plain резолвится, чужой логин — отказ);
      UI `access_token_new` (рабочий токен; `400` на неизвестного). `cargo test --workspace` —
      **88 наборов / 0 failed**; `fmt`/`clippy --all-targets -D warnings` чисто.
- **Следующий шаг — T4** (UI-карточки «Службы» и «Доступ» в `page.rs`).

### T4. UI-карточка «Службы» + доработка «Доступа» — 1–1,5 дн — ✅ **выполнено 10.10.2026 (`5592f2f`)**
- [x] `page.rs`: карточка «Службы» — таблица 4 ролей (состояние/автозапуск) и кнопки
      `install|uninstall|start|stop|restart` через `POST /api/services`; в desktop-профиле —
      сообщение «службы только в server-профиле» без кнопок; обновление по кнопке и после действия.
- [x] Карточка «Доступ»: строка «Выдать токен» (логин + TTL) → `POST /api/access/token/new`,
      plain-токен показывается **один раз** в `<pre>`; отзыв (`token revoke`) оставлен.
- [x] **Попутно исправлен существовавший дефект:** разметка «Доступа» вызывала `accessList()/
      accessAdd()/accessDel()`, которых в JS **не было** (`refresh()` падал ReferenceError каждый
      тик). Реализованы поверх существующих `/api/access/user(s)`.
- [x] Всё через существующий `act()`/`jpost`; без внешних ассетов; `hds_ui` собирается и
      отдаёт страницу. Юнит `page_has_services_and_token_controls` (наличие контролов в `PAGE`).
      Живой прогон (песочница): страница содержит «Службы»/«Выдать токен», `/api/services`
      (desktop) — пусто+hint. `cargo test --workspace` — **88 наборов / 0 failed**;
      `fmt`/`clippy --all-targets -D warnings` чисто. Артефакт — `tools/parity/out/m7_t4_live.txt`.
- **Следующий шаг — T5** (рантайм-приёмка: SCM-цикл под администратором, токен+Bearer, desktop).

### T5. Рантайм-приёмка — 1–1,5 дн
- [ ] Server-профиль (из-под администратора): SCM-цикл по всем 4 ролям через UI/API
      (install→start→status→stop→uninstall), состояния совпадают с `sc query`.
- [ ] Выдача токена из UI → клиент с Bearer получает 200 `search`/`ask`; отзыв → `401`;
      аудит содержит `admin_op` (и не содержит plain-токен).
- [ ] Desktop-профиль: `/api/services` — «служб нет», кнопки не появляются; поведение не
      изменилось (боевой `hds check` ok).
- [ ] Артефакты — `tools/parity/out/m7_*` (гитигнор); боевой стенд вернуть.

### T6. Доклад — 0,5 дн
- [ ] `README_SERVER_MULTIUSER.md` — сценарий «Управление службами и токенами из UI»;
      `README.md`/`STATUS.md` — короткие блоки; статус в `PLAN_SERVER_MULTIUSER.md` §9
      (по образцу M6 — только блок статуса, решения §5 не меняем).

## 5. Приёмка

- [ ] `hds service …` работает как раньше (регрессия) и `status` показывает живое состояние.
- [ ] `GET /api/services` — все 4 роли с состоянием/автозапуском; `POST` — install/
      uninstall/start/stop/restart, с числами/деталями; ошибки прав/профиля честные.
- [ ] UI-карточка «Службы» управляет службами (server-профиль); в desktop — сообщение.
- [ ] `hds token new` и `POST /api/access/token/new` выдают рабочий токен (проверено
      живым запросом); plain-токен не в логах/аудите; отзыв работает (`401`).
- [ ] Перезапуск роли `ui` из UI не «подвешивает» страницу (ответ получен, служба
      поднялась) — либо честная ошибка с hint.
- [ ] Десктоп-профиль не изменён; `hds check` в бою — ok.
- [ ] `cargo test --workspace` зелёный; `cargo fmt --all --check`,
      `cargo clippy --workspace --all-targets -- -D warnings` — чисто.
- [ ] Рантайм-проверки (не только юниты): SCM-цикл, выдача/отзыв токена, UI-карточки —
      на живом стенде по `cline-rules/dev-machine.md` (резидент/порты вернуть,
      артефакты — в `tools/parity/out/`).

## 6. Риски

| Риск | Митигация |
|---|---|
| Нет прав на управление SCM (UI от пользователя) | Честная ошибка с hint; приёмка — из-под администратора/LocalSystem |
| Перезапуск UI «сам себя» подвешивает страницу | Ответ до перезапуска + detached helper или делегирование SCM-авторестарту; иначе отказ с hint |
| Перенос служб в `hds-core` тянет `windows-service` в общий крейт | Если нежелательно — отдельный крейт `hds-service`; решение в T1 |
| Раскол управления: старые `/api/*` и новый `/api/services` расходятся | `/api/services` — добавление; старые контракты не ломаем, поведение сверяем числами |
| Утечка plain-токена (лог/аудит/HTML) | Писать только хэш; показ один раз; аудит без токена; тест-проверка |
| macOS-путь не проверяется на этой станции | Юниты на plist/`launchctl`-аргументы; живой цикл — хвост «машина с Mac» |
| Рефакторинг служб ломает установку на сервере | Регрессия CLI (тексты/exit-коды) + SCM-цикл на стенде |

## 7. Ссылки
- `PLAN_SERVER_MULTIUSER.md` — §2 **W10**, §5 **Э7**, §9 (статусы).
- `PLAN_M6_GPU_MULTIVENDOR.md` + `M6_CODE_REVIEW.md` — предыдущий этап и его ревью (T0).
- `PLAN_M2_SERVER_PROFILE.md` §4 (службы), `PLAN_M1_ACCESS_PROJECTS.md` (токены/UI-доступ),
  `PLAN_M3_NETWORK_PERIMETER.md` (аудит/CSRF).
- Код: `crates/hds-cli/src/cmd/service.rs`, `crates/hds-ui/src/lib.rs`, `page.rs`,
  `crates/hds-core/src/{access,audit,config}.rs`, `crates/hds-mcp/src/auth.rs`.
- `tools/parity/out/` — артефакты (гитигнор).