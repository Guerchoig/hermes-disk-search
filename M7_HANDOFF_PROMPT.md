# M7_HANDOFF_PROMPT.md — промт для нового чата (этап Э7/W10: управление службами и доступами из UI)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где детали, контракты, правила и грабли окружения.
>
> **Новый этап.** M1–M6 закрыты: **M6/Э5** (GPU-раскладка) завершён (`ffc450c`) и прошёл
> код-ревью (`M6_CODE_REVIEW.md`, `4449fab`, принят, находки некритичные). По дорожной
> карте `PLAN_SERVER_MULTIUSER.md` §5 после Э5 (M6) и Э6 (M5) идёт **Э7** — «Управление
> службами и токенами из UI» (**W10**, §2).
> Детальный план этапа — **`PLAN_M7_UI_CONTROL.md`** (треки T0–T6, контракты, приёмка).

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние на момент передачи: **HEAD `4449fab`**, дерево чистое; боевой стенд поднят
(резидент `llm_host`, `watch`/`mcp-http`, `index.pause` — есть).
Этап **M7/Э7** — код не начат. Предыдущий этап M6 закрыт: `ffc450c` (T6-доклад),
`M6_CODE_REVIEW.md` — принят (`4449fab`).

Станция: Windows, RTX 3060 12 ГБ (единственный дискретный GPU), AMD Radeon Graphics
(iGPU, UMA), Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1. Ядро на Rust (`crates/`),
Python — только sidecar.

## Объём M7 (что сделать) — кратко

1. **T0 (хвосты ревью M6):** M6-1 (`hds check` `gpu_roles` не читает `llm.<role>.device`),
   M6-4 (косметика `CUDA ,`), зафиксировать решение по F-m6-t5-1 (клиенты всегда шлют `gpu`).
2. **T1 (общий модуль служб):** вынести общую часть служб из `hds-cli` в `hds-core::service`
   (роли/имена, `sc_args`, `daemon_plist`, чтение состояния SCM/launchd, управление) — UI не
   может импортировать `hds-cli` (там цикл: `hds-cli` зависит от `hds-ui`). `hds service …` —
   поведение без изменений.
3. **T2 (`/api/services`):** `GET` — состояние 4 ролей (`watch|mcp-http|ui|llm-host`),
   `POST {role, action}` — `install|uninstall|start|stop|restart` (admin-гейт/CSRF/аудит
   `admin_op`); особые случаи: перезапуск роли `ui` (сам себя), нет прав, desktop-профиль.
4. **T3 (выдача токенов):** CLI `hds token new --user <login> [--ttl-days N]` и UI
   `POST /api/access/token/new` → plain-токен **один раз** (в файл — хэш, в аудит — без токена).
5. **T4 (UI):** карточка «Службы» (состояние + кнопки, desktop-сообщение) и кнопка
   «Выдать токен» в карточке «Доступ» (`page.rs`).
6. **T5 (приёмка):** SCM-цикл по ролям (из-под администратора), выдача/отзыв токена живым
   запросом, desktop не изменён; артефакты — `tools/parity/out/m7_*`.
7. **T6 (доклад):** README/STATUS + статус-блок в `PLAN_SERVER_MULTIUSER.md` §9.

## Точка входа (5 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -8`  # HEAD — этот промт
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`     # чисто
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe`:
   либо остановить `hds watch`/`hds mcp-http`, либо `cargo test --workspace --exclude hds-cli`;
   после работ сервисы **вернуть**. Ожидание: **88 наборов, 0 failed**.
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
5. Прочитать: **`PLAN_M7_UI_CONTROL.md`** целиком (§3 — контракты, §4 — треки, §5 — приёмка)
   → `M6_CODE_REVIEW.md` §3 (хвосты T0) → `PLAN_SERVER_MULTIUSER.md` §2 W10 / §5 Э7 → `STATUS.md`.
6. Карта кода: `crates/hds-cli/src/cmd/service.rs` (службы), `crates/hds-ui/src/lib.rs`
   (`/api/*`, admin-гейт ~1424, `mcp_http_action` ~1237, `llm_host_restart` ~826),
   `crates/hds-ui/src/page.rs` (карточки), `crates/hds-core/src/access.rs` (`new_token` ~201),
   `crates/hds-mcp/src/auth.rs` (`login` ~96).

## Контракты (код — источник истины, `PLAN_M7_UI_CONTROL.md` §3)

* **Роль службы** = `watch | mcp-http | ui | llm-host`; имена `hermes-disk-search-<role>`
  (Windows SCM) / `com.hermes-disk-search.<role>` (macOS launchd). Источник —
  `hds-cli::cmd::service` (переносится в общий модуль, T1).
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