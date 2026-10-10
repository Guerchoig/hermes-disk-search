# M7_HANDOFF_PROMPT.md — промт для нового чата (этап Э7/W10: управление службами и доступами из UI)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, правила и грабли окружения.
>
> **Этап начат.** M1–M6 закрыты: **M6/Э5** завершён (`ffc450c`) и прошёл код-ревью
> (`M6_CODE_REVIEW.md`, `4449fab`, принят). По дорожной карте `PLAN_SERVER_MULTIUSER.md` §5
> после Э5 (M6) и Э6 (M5) идёт **Э7** — «Управление службами и токенами из UI» (**W10**, §2).
> **M7: T0 (`14567a8`), T1 (`cd01065`), T2 (`112d6ee`) и T3 (`8980338`) закрыты, следующий шаг — T4.**
> Детальный план этапа — **`PLAN_M7_UI_CONTROL.md`** (треки T0–T6, контракты, приёмка;
> §4 T0–T3 помечены выполненными; §3.4 — UI-карточки «Службы»/«Доступ»).

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние на момент передачи: **HEAD `8980338`** (+ docs-коммит этого промта), дерево чистое;
боевой стенд поднят (резидент `llm_host`, `watch`/`mcp-http`/`transcribe-watch`, `index.pause` —
есть). Этап **M7/Э7** начат: **T0** (`14567a8`), **T1** (`cd01065`), **T2** (`112d6ee`),
**T3** (`8980338`) — закрыты, **следующий шаг — T4**. Предыдущий этап M6 закрыт: `ffc450c`
(T6-доклад), код-ревью — `4449fab`.

Станция: Windows, RTX 3060 12 ГБ (единственный дискретный GPU), AMD Radeon Graphics
(iGPU, UMA), Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1. Ядро на Rust (`crates/`),
Python — только sidecar.

## Объём M7 (что сделать) — кратко

1. **T0 (хвосты ревью M6) — ✅ сделано (`14567a8`):** M6-1 (`hds check` `gpu_roles` теперь
   читает `llm.<role>.device`), M6-4 (косметика `CUDA ,`), F-m6-t5-1 — решение «поведение
   не меняем» (ждёт владельца).
2. **T1 (общий модуль служб) — ✅ сделано (`cd01065`):** общая часть служб вынесена в
   `hds-core::service` (роли/имена, `sc_args(sub, role, hds_exe)`, `daemon_plist`, чтение
   состояния SCM/launchd, управление); `hds-cli::cmd::service` — тонкая обёртка; `service run`
   остался в CLI; `hds service …` — поведение без изменений.
3. **T2 (`/api/services`) — ✅ сделано (`112d6ee`):** `GET` — состояние 4 ролей
   (`watch|mcp-http|ui|llm-host`); `POST {role, action}` — `install|uninstall|start|stop|restart`
   (admin-гейт/CSRF/аудит `admin_op`); роли `ui`-restart — detached helper; честные ошибки прав
   и «не установлена»; desktop → пусто+hint.
4. **T3 (выдача токенов) — ✅ сделано (`8980338`):** CLI `hds token new --user <login> [--ttl-days N]`
   и UI `POST /api/access/token/new` → plain-токен **один раз** (в файл — хэш, в аудит `admin_op` —
   без токена); отказ для неизвестного логина.
5. **T4 (UI):** карточка «Службы» (состояние + кнопки, desktop-сообщение) и кнопка
   «Выдать токен» в карточке «Доступ» (`page.rs`).
6. **T5 (приёмка):** SCM-цикл по ролям (из-под администратора), выдача/отзыв токена живым
   запросом, desktop не изменён; артефакты — `tools/parity/out/m7_*`.
7. **T6 (доклад):** README/STATUS + статус-блок в `PLAN_SERVER_MULTIUSER.md` §9.

## Сделано в T0 (`14567a8`, 10.10.2026)

* `crates/hds-index/src/diag.rs::role_device_from_cfg` теперь читает карту роли так же, как
  рантайм `hds-llama::config::build_role`: `llm.<role>.device` **первым** → фолбэк
  `llm_server.<role>.device` → отрицательный индекс игнорируется (`filter(>=0)`) → плюс
  `gpu.devices[].roles`. До фикса роль, заданная ключом `llm.<role>.device`, «исчезала» из
  пункта `gpu_roles`, а двойное назначение с `gpu.devices` не ловилось (находка M6-1).
* `check_gpu_roles_live` больше не печатает `CUDA ,` при пустом `description` (M6-4).
* Новый юнит `diag::tests::role_device_prefers_llm_key` (4 сценария).
* **F-m6-t5-1 (M6-2):** поведение не менялось; ограничение «клиенты всегда шлют `gpu` →
  host-выбор карты под файл не срабатывает» уже задокументировано в
  `README_SERVER_MULTIUSER.md` §8b (M6 T6). Решение — за владельцем.
* Проверено: `cargo test --workspace` — **88 наборов / 0 failed**; `fmt`/`clippy -D warnings`
  чисто. Изменён только `crates/hds-index/src/diag.rs` (+55/−8).

## Сделано в T1 (`cd01065`, 10.10.2026)

* Создан **`crates/hds-core/src/service.rs`** — общий код служб (M7 T1, `PLAN_M7_UI_CONTROL.md`
  §3.1): `ROLES`, `service_name`, `daemon_label`, `parse_role`; `sc_args(sub, role, hds_exe: &Path)`
  (путь к `hds.exe` передаёт вызывающий — `current_exe()` в общем коде указал бы на UI),
  `daemon_plist`; чтение состояния `status(role) -> ServiceStatus` (SCM `sc query`/`sc qc`,
  launchd `launchctl print`); управление `control(action, role, hds_exe) -> ControlResult`
  (`install|uninstall|start|stop` через `sc.exe`/`launchctl`; `permission_denied()` — для T2).
  Контракт `ServiceStatus {role, kind, state, autostart, detail}` — под `GET /api/services`.
* `hds-cli/src/cmd/service.rs` стал тонкой обёрткой: `service run` (SCM-диспетчер
  `windows-service`, `run_llm_host`, `on_service_stop`) остался в CLI; `install…status`
  делегирует в `hds_core::service::control` (печать/exit-код без изменений); имена
  реэкспортированы (`pub use hds_core::service::{…}`) — внешний API `hds-cli::cmd::service`
  сохранён. `windows-service` в `hds-core` **не** тянется.
* Юнит-тесты: `roles_and_names`, `sc_install_args_quote_binpath`, `plist_has_required_keys`
  переехали в `hds-core::service::tests` (вызов `sc_args` — с путём); в CLI остался
  `role_from_args_reads_service_role`; добавлены чистые `parse_sc_query_state`/`parse_sc_qc_autostart`
  (работают и на linux-CI).
* Проверено: `cargo test --workspace` — **88 наборов / 0 failed**; `fmt`/`clippy --all-targets
  -D warnings` чисто; `hds check` (desktop) — ok. Регрессия `hds service status <role>` на стенде:
  `sc 1060` → exit 1; неизвестная подкоманда/нет роли → exit 2. Стенд останавливался и **возвращён**.

## Сделано в T2 (`112d6ee`, 10.10.2026)

* В `crates/hds-ui/src/lib.rs` добавлены `GET`/`POST /api/services` (контракт — §3.2):
  * `GET` → `{profile, services:[{role,kind,state,autostart,detail}], hint}`; роли из
    `hds_core::service::ROLES`, состояние — `hds_core::service::status(role)` (SCM/launchd);
    desktop → `services: []` + `hint`.
  * `POST {role, action}` → `{ok, msg, state, hint?}`; `install|uninstall|start|stop` через
    `hds_core::service::control(action, role, hds_exe())`; `restart = stop + start`.
  * Валидация: неизвестная роль/действие → `400 {error, hint}` (до профиля/SCM).
  * Ошибки: нет прав (`Access denied`/код 5) → hint «UI от администратора/LocalSystem»;
    не установлена (`sc 1060`) → `{ok:false, msg, hint, state}`; desktop → отказ с hint.
  * `ui` (UI сам себя): `restart` отвечает сразу, перезапуск — detached helper
    (`cmd` → пауза → `sc stop`/`start`); при сбое — честная ошибка с hint.
  * POST остаётся под штатным admin-гейтом/CSRF и аудитом `admin_op` (общий блок `is_admin_op`);
    старые `/api/*` не тронуты.
* Функции `services_json_profile`/`services_action_profile` вынесены `pub` — тесты без боевого
  `config.yaml`; `services_action_json` — разбор тела.
* Тесты (`tests/ui_core.rs`, +4): desktop (пусто+отказ), server (4 роли/поля), валидация (`400`),
  роутер GET. Живой прогон (песочница `HDS_CONFIG`): desktop `GET` пусто+hint/`POST` отказ;
  server `GET` 4 роли (`absent`)/`POST start` → `ok:false` «не установлена» + `state`.
* `cargo test --workspace` — **88 наборов / 0 failed**; `fmt`/`clippy --all-targets -D warnings`
  чисто. Артефакты — `tools/parity/out/m7_t2_live_{desktop,server}.txt`.

## Сделано в T3 (`8980338`, 10.10.2026)

* **CLI** (`crates/hds-cli/src/cmd/auth.rs`): `hds token new --user <login> [--ttl-days N]` →
  plain-токен печатается **один раз** + срок; TTL из `--ttl-days` (иначе `access.token_ttl_days`);
  отказ для несуществующего/отключённого логина (exit 1); аудит `admin_op`. Ядро —
  `token_new_issue(path, login, ttl, now) -> (plain, expires)` (в файл только `token_sha256`).
  Usage обновлён (`token list|new|revoke`).
* **UI** (`crates/hds-ui/src/lib.rs`): `POST /api/access/token/new {user, ttl_days?}` →
  `{token, expires, user}` (plain **один раз**); отказ для неизвестного/отключённого логина (`400`);
  под штатным admin-гейтом/CSRF и аудитом `admin_op` (токен в аудит не пишется).
* Тесты: CLI `token_new_issue` (хэш сохранён, plain резолвится, чужой логин — отказ); UI
  `access_token_new` (рабочий токен; `400`).
* Живой прогон (песочница `server` + `access.enabled`): CLI выдал токен, `ghost` → отказ (exit 1);
  admin Bearer `GET /api/services` → `200`, без токена → `401`; выданный через UI токен работает
  (`200`); отзыв по пользователю → последующий запрос `401`; аудит содержит `admin_op` (CLI и UI)
  и **не содержит** plain-токенов. Артефакты — `tools/parity/out/m7_t3_{cli_issue,live}.txt`,
  `m7_t3_audit.jsonl`.
* `cargo test --workspace` — **88 наборов / 0 failed**; `fmt`/`clippy --all-targets -D warnings`
  чисто.

## Следующий шаг — T4 (UI-карточки «Службы» и «Доступ»)

`crates/hds-ui/src/page.rs` (контракт — §3.4; без внешних ассетов, через существующий `act()`):

* новая карточка «Службы»: таблица 4 ролей (состояние/автозапуск), кнопки
  `install|uninstall|start|stop|restart`, автообновление статуса через `GET /api/services`;
  в desktop-профиле — сообщение «службы только в server-профиле» **без** кнопок.
* карточка «Доступ»: кнопка «Выдать токен» (пользователь + TTL) → `POST /api/access/token/new`,
  показ plain-токена **один раз** (копировать), как для секретов; отзыв — уже есть.
* Проверка: `hds_ui` собирается/открывается, карточки не ломают desktop; приёмка — T5.

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -8`  # HEAD — этот промт
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`     # чисто
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe`:
   либо остановить `hds watch`/`hds mcp-http`, либо `cargo test --workspace --exclude hds-cli`;
   после работ сервисы **вернуть**. Ожидание: **88 наборов, 0 failed**.
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
5. Прочитать: **`PLAN_M7_UI_CONTROL.md`** целиком (§3 — контракты, §4 — треки, §5 — приёмка)
   → `M6_CODE_REVIEW.md` §3 (хвосты T0) → `PLAN_SERVER_MULTIUSER.md` §2 W10 / §5 Э7 → `STATUS.md`.
6. Карта кода: `crates/hds-core/src/service.rs` (общий код служб, T1) →
   `crates/hds-cli/src/cmd/service.rs` (тонкая обёртка + `service run`), `crates/hds-ui/src/lib.rs`
   (`/api/*`, admin-гейт ~1424, `mcp_http_action` ~1237, `llm_host_restart` ~826),
   `crates/hds-ui/src/page.rs` (карточки), `crates/hds-core/src/access.rs` (`new_token` ~201),
   `crates/hds-mcp/src/auth.rs` (`login` ~96).

## Контракты (код — источник истины, `PLAN_M7_UI_CONTROL.md` §3)

* **Роль службы** = `watch | mcp-http | ui | llm-host`; имена `hermes-disk-search-<role>`
  (Windows SCM) / `com.hermes-disk-search.<role>` (macOS launchd). Источник — `hds_core::service`
  (общий модуль, T1); `hds-cli::cmd::service` реэкспортирует имена для совместимости.
* **`GET /api/services`** → `{profile, services:[{role,kind,state,autostart,detail}], hint}`;
  `kind = "service"|"absent"`, `state = "running"|"stopped"|"absent"|"unknown"`.
* **`POST /api/services {role, action}`** → `{ok, msg, state}`; действия
  `install|uninstall|start|stop|restart`; под admin-гейтом/CSRF/аудитом `admin_op`.
  Ошибки: нет прав (`sc.exe` код 5 / Access denied) → `{ok:false,msg,hint}`; не установлена;
  неподдерживаемая ОС; неизвестная роль/действие.
* **Токен:** `hds_core::access::new_token(user, ttl_days, now_unix)` → `(plain, Token)`
  (в файл — `token_sha256`). CLI `hds token new --user L [--ttl-days N]`;
  UI `POST /api/access/token/new {user,ttl_days?}` → `{token,expires,user}`. Plain-токен —
  **один раз**, не в логи/аудит. Отзыв уже есть (`/api/access/token/revoke`, `hds token revoke`).
* **Профили:** службы — только `profile: server` (ставит `installers/install_server.*`,
  от администратора/LocalSystem); desktop — `/api/services` сообщает «служб нет», действия
  отклоняются. Старые точки (`/api/watch/daemon`, `/api/mcp-http`, `/api/llm-host/restart`,
  `/api/transcribe/daemon`) — **не ломать**, `/api/services` — добавление.

## Правила и грабли окружения (важно, платил временем)

* **Резидент держит `target\release\*.exe`**, живые службы — `target\debug\hds.exe`:
  перед пересборкой/полным `cargo test` остановить и **вернуть** (`llm_host stop/run`,
  `hds watch`/`hds mcp-http`); иначе «отказано в доступе».
* **`HDS_CONFIG` (и др. env) переживают между элементами `run_commands`** — после песочных
  проб очищать переменную, иначе резидент поднимется на песочном конфиге.
* **Боевой `index.pause` не снимать** (пользовательская пауза) — только переиспользовать.
* Проверять **рантаймом** (живой запрос/SCM), а не только юнитами; секреты не печатать.
* Артефакты — в `tools/parity/out/` (гитигнор); после работ не оставлять мусор в репозитории.
* `read_files` иногда отвечает `[outdated - see the latest file content]` — перечитать тем же
  диапазоном; в PowerShell первый вывод команды может «прилипать» к приглашению — не пугаться.
* Коммиты — атомарные по трекам (`M7/T1: …`, `docs(m7): …`), как в M6; решения §5 дорожной
  карты не менять.

## Приёмка (кратко; подробно — `PLAN_M7_UI_CONTROL.md` §5)

`hds service …` без регрессии + `status` = живое состояние; `/api/services` GET/POST по 4 ролям
с честными ошибками прав/профиля; карточка «Службы» (desktop — сообщение); `hds token new` и
`/api/access/token/new` выдают рабочий токен (проверено живым запросом), plain не в логах,
отзыв → `401`; перезапуск роли `ui` не подвешивает страницу; desktop-профиль не изменён;
`cargo test --workspace` зелёный, fmt/clippy `-D warnings` чисто.