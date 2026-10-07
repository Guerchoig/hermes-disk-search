# PLAN_M2_SERVER_PROFILE.md — детальный план M2 («Серверный профиль и службы», Э1)

> Дополняет `PLAN_SERVER_MULTIUSER.md` §5 (этап **Э1**), §2 (**W1**; частично W2/W10).
> Структура — по образцу `PLAN_M1_ACCESS_PROJECTS.md`. Промт для нового чата —
> `M2_HANDOFF_PROMPT.md`.
>
> **Статус:** в работе — **T0 (дефекты M1) выполнен** 07.10.2026 (см. §4 T0); дальше
> T1 (профиль) → T5. Ветка-продолжение — **`server-multiuser`** (база после M1).
> **Основание:** решения заказчика 07.10.2026 (`PLAN_SERVER_MULTIUSER.md`, шапка):
> периметр — **reverse-proxy с TLS**; **UI — только админ**; **службы управляются из UI**
> (W10, будущий Э7); профиль `desktop | server` — «разделить интерактивный десктоп и
> серверный режим, не ломая одиночных пользователей» (W1).
>
> **Зависимость:** M2 включает исправление дефектов M1 (трек **T0**): без парсинга
> заголовка `Authorization` в UI админ-гейт неработоспособен в рантайме, а headless-UI —
> часть M2.

## 1. Вводные (факты из кода, а не предположения)

| Факт | Где |
|---|---|
| `profile: desktop \| server` есть в `config.example.yaml`, но **никем не читается** (в коде читается только `access.enabled`) | `config.example.yaml`; `crates/hds-mcp/src/auth.rs::auth_enabled`, `crates/hds-ui/src/lib.rs::ui_auth_gate` |
| `hds ui` принимает `--host/--port`, дефолт `127.0.0.1:8765`; **конфиг не читает** | `crates/hds-cli/src/cmd/ui.rs`, `crates/hds-ui/src/bin/hds_ui.rs` |
| `hds mcp-http` берёт `mcp_http.host/port/path` из конфига | `crates/hds-cli/src/cmd/mcp_http.rs`, `crates/hds-mcp/src/http.rs` |
| Корень проекта `project_root()`: `HDS_ROOT` → рядом с `hds.exe` → build-time; данные — `<root>/data`, БД — `db_abs_path` (дефолт `index.db`) | `crates/hds-core/src/config.rs` |
| Автозапуск Windows — **Планировщик, триггер `AtLogOn`** (Watch/Mcp/Transcribe) + опц. UI; llm-host — своя задача | `install_autostart.ps1`, `installers/install_ui_task.ps1`, `installers/install_llm_host_task.ps1` |
| macOS-инсталлятор ставит **`~/Library/LaunchAgents`** (вход пользователя) | `installers/install_macos.command` |
| Windows-служба (WinSW/NSSM/крейт `windows-service`) **не реализована** | — (в репо упоминаний нет) |
| Админ-гейт UI `ui_auth_gate` читает `ReqHeaders.authorization`, но `read_request` **не парсит** этот заголовок → 401 даже с валидным токеном (подтверждено рантаймом 07.10.2026) | `crates/hds-ui/src/lib.rs` (`read_request`, `ui_auth_gate`) |
| `.gitignore`: `data/` затем `!data/access.example.yaml` — **ре-инклуд не работает** (нельзя вернуть файл внутри исключённого каталога); `git ls-files data/` пуст | `.gitignore`, `data/access.example.yaml` |
| `hds login` собирает JSON через `format!` без экранирования логина/пароля | `crates/hds-cli/src/cmd/auth.rs::cmd_login` |

## 2. Границы M2

**Входит:**
- Чтение профиля `profile: desktop | server` (дефолт `desktop`) и его влияние на поведение.
- **Серверные пути** состояния (индекс, логи, PID, `access.yaml`) в системный каталог.
- **Привязка хостов/портов из конфига** (секция `ui.*`; CLI — переопределение).
- **Сервисные обёртки Windows и macOS** + команды установки/управления; автозапуск
  Windows — **`AtStartup`** (не `AtLogOn`).
- **Headless админ-UI** (без ярлыков и авто-открытия браузера).
- **Исправление дефектов M1** из код-ревью (T0).
- Тесты, обновление `STATUS.md` и `README` (серверный профиль).

**Не входит (следующие этапы):** TLS/прокси, аудит, rate-limit (сетевая часть Э2);
«Проекты» (сделано в M1); конкурентность под SLA (Э4/W3); GPU-раскладка по картам
(Э5/W4); внешняя чат-модель (Э6/W9); **управление службами из UI** (Э7/W10 — M2 лишь
делает службы и CLI, UI-карточку добавляет Э7); пакеты установки Win/macOS (Э8/Э9);
эксплуатация `healthz`/бэкап (Э10). **Десктоп-профиль сохраняет нынешнее поведение.**

## 3. Технические решения (контракты)

### 3.1. Профиль
- `profile` из `config.yaml`: **`desktop`** (дефолт, если ключа нет) или **`server`**.
- Резолвер в `hds-core`: `config::profile(cfg)` (enum `Profile`).
- `server` влияет на: **корни путей** (T2), **headless-UI** (T5), **установку служб** (T4)
  и выдаёт **рекомендации** в `hds check` (`access.enabled: true`, задан `ui.*`).
  Гейт доступа по-прежнему определяется **только** `access.enabled`: `profile: server`
  без `access.enabled` не включает принудительно auth — только предупреждает.

### 3.2. Серверные пути (state dir)
- Windows: **`%ProgramData%\hermes-disk-search`**; macOS:
  **`/Library/Application Support/hermes-disk-search`**.
- Содержимое: `config.yaml`, `index.db`, `data/` (`access.yaml`, `logs/`, `*.pid`).
- Новый резолвер `state_dir(cfg)`: `HDS_ROOT` переопределяет всё; `desktop` — нынешнее
  поведение (корень проекта); `server` — системный каталог. БД/PID/логи/`access.yaml`
  читаются через этот резолвер (`db_abs_path` учитывает `state_dir`).

### 3.3. Хосты и порты
- Новая секция `ui: { host, port }` (`config.example.yaml`); `hds ui` (и бинарь `hds_ui`)
  читает её, флаги `--host/--port` — переопределение.
- `server`: за reverse-proxy → `host: 127.0.0.1` (наружу только прокси; фасад
  `:8010–8012` **не** публикуем). `0.0.0.0` — только если прокси на другой машине.

### 3.4. Службы
- Единая точка входа в бинаре `hds`: **`hds service run <role>`** (`watch` | `mcp-http` |
  `ui` | `llm-host`) — сервис-хостинг для SCM/launchd.
- Управление: **`hds service install | uninstall | start | stop | status [role]`**.
- Windows: интеграция со SCM. Вариант A — крейт **`windows-service`** (проверить
  offline-доступность, как в спайках крейтов); Вариант B — **WinSW/NSSM** внешним бинарём
  в комплекте; worst case — **AtStartup**-задача Планировщика вместо службы (спайк в T4).
- macOS: **LaunchDaemon** plist в `/Library/LaunchDaemons` (root), `RunAtLoad` +
  `KeepAlive`; ставится скриптом `install_server.command` (вместо LaunchAgent-ов входа).
- Десктоп: прежний Планировщик `AtLogOn` **не трогаем**.

### 3.5. Headless UI
- `server`: UI — только админ, без ярлыков и авто-открытия браузера (`run_ui.ps1`,
  `shortcuts/*` — только desktop); bind из `ui.*`; вход строго по админ-токену.
- Требует закрытия дефекта **T0** (парсинг `Authorization`) — иначе UI недоступен.

## 4. Треки

### T0. Устранение дефектов M1 (код-ревью) — 0,5 дн — **выполнен 07.10.2026**
- [x] **UI:** `read_request` (`crates/hds-ui/src/lib.rs`) парсит заголовок
      `authorization:` в `ReqHeaders.authorization`; тест через **реальный сокет**
      (`TcpListener` + `route`), т.к. чистая функция `route` дефект не ловит.
      → `243a028` `fix(ui)`: парсинг + `run_http_listener` (публичный, для тестов);
      сокет-тест `ui_auth_socket_parses_authorization` (сырой HTTP, 401 без токена /
      200 с админ-токеном / case-insensitive ключ); проверено рантаймом: `hds ui` +
      curl → 401 без токена, **200** с Bearer (до фикса — 401). Негативная проверка:
      без парсинга сокет-тест падает (дефект действительно ловится).
- [x] **Git:** `.gitignore` — `data/` → `data/*` + `!data/access.example.yaml`.
      → `f62cbc1` `chore(git)`: `git ls-files data/` = `data/access.example.yaml`;
      `git check-ignore data/access.yaml` — по-прежнему игнорируется.
- [x] **CLI:** `hds login` — тело запроса через `serde_json::json!`, не `format!`.
      → `0362fef` `fix(cli)`: `login_body()` + юнит-тест на `"`, `\`, `\n`;
      рантайм: пользователь с паролем `p"a\ss` логинится живым `/auth/login`.

### T1. Профиль конфига — 1 дн
- [ ] `config::profile(cfg)` (`desktop` по умолчанию); показ профиля в `hds check`.
- [ ] Предупреждения для `server`: `access.enabled`, наличие `ui.*`.
- [ ] `config.example.yaml`: секция `ui: { host, port }` + комментарии.
- [ ] Тесты парсинга/дефолтов профиля.

### T2. Серверные пути (state dir) — 1,5 дн
- [ ] `state_dir(cfg)`: `HDS_ROOT` → иначе `server` = ProgramData / Application Support,
      `desktop` = корень проекта.
- [ ] БД, PID, логи, `access.yaml` — через резолвер; `db_abs_path` учитывает `state_dir`.
- [ ] Инициализация каталога при первом запуске службы.
- [ ] `hds check`/`status` показывает фактические пути; тесты.

### T3. Хосты/порты из конфига — 0,5 дн
- [ ] `hds ui` и бинарь `hds_ui` читают `ui.host/port` (CLI-флаги — переопределение).
- [ ] Тесты.

### T4. Службы Windows/macOS — 2,5–3 дн
- [ ] `hds service run <role>` — единый сервис-хостинг (`watch`/`mcp-http`/`ui`/`llm-host`).
- [ ] Windows: `install | uninstall | start | stop | status`; **спайк** выбора
      (`windows-service` vs WinSW/NSSM vs AtStartup-задача).
- [ ] macOS: генерация/установка LaunchDaemon plist (`install_server.command`).
- [ ] `installers/install_server.ps1` (Windows) и `installers/install_server.command` (macOS).
- [ ] Десктопный автозапуск (`install_autostart.ps1`, AtLogOn) сохранить без изменений.

### T5. Headless UI + доклад — 1 дн
- [ ] `server`: UI без ярлыков/авто-браузера; bind из `ui.*`.
- [ ] `README.md` (раздел «серверный профиль»), `STATUS.md`, `PLAN_SERVER_MULTIUSER.md` §9
      (статус M2).

## 5. Приёмка M2 (черновик)

- **Службы:** `watch`/`mcp-http`/`ui`/`llm-host` стартуют при загрузке **без интерактивного
  входа**; `hds service status` отражает реальное состояние SCM/launchd.
- **Пути (server):** `index.db`, `data/` (`access.yaml`, логи, PID) — в ProgramData /
  Application Support; **desktop — без изменений**.
- **UI (server):** страница и API открываются с валидным админ-токеном, без токена — `401`
  (дефект T0 закрыт) — **рантайм-тест**, а не только юнит.
- **Порты:** `hds ui` слушает `ui.host:ui.port` из конфига.
- **Обратная совместимость:** `profile: desktop` (дефолт) — нынешнее поведение и
  автозапуск; `cargo test --workspace` зелёный; `clippy -D warnings` и `fmt` — чисто.
- **M1-дефекты закрыты:** `git ls-files data/` содержит `access.example.yaml`;
  `hds login` экранирует логин/пароль.


## 6. Порядок работ и коммиты

Порядок (по зависимостям), по одному коммиту на шаг:
1. **T0** (hotfix отдельными коммитами): `fix(ui): parse Authorization header`,
   `chore(git): track data/access.example.yaml`, `fix(cli): escape login JSON`.
2. **T1** — `feat(config): read profile (desktop|server)`.
3. **T2** — `feat(paths): server state dir (ProgramData/Application Support)`.
4. **T3** — `feat(ui): host/port from config`.
5. **T4** — `feat(service): windows/macos service wrappers`.
6. **T5** — `docs(server): server profile and services`.

После каждого шага — тесты + обновление журнала/статуса. Порядок в чате: начать с **T0**
(он разблокирует рантайм-проверку UI), затем T1→T2→T3→T4→T5.

## 7. Риски

| Риск | Митигация |
|---|---|
| `windows-service`/NSSM недоступны offline | **спайк** в T4; фолбэк — WinSW в комплекте; worst case — AtStartup-задача |
| Серверные пути ломают desktop | `profile: desktop` по умолчанию; регресс-тесты резолвера путей |
| Служба держит `*.exe` при обновлении | известный прецедент (резидент `llm_host.exe`); окно обслуживания |
| macOS LaunchDaemon не проверен | best-effort как весь macOS-стек; чек-лист `MAC_CHECKLIST.md` |
| Дефект T0 блокирует headless-UI | T0 — первым треком |
| Дублирование автозапуска (служба + задача Планировщика) | server-инсталлятор снимает AtLogOn-задачи ролей |

## 8. Точки входа по файлам (шпаргалка)

- `crates/hds-core/src/config.rs` (profile, state_dir, db_abs_path)
- `crates/hds-cli/src/main.rs`, `crates/hds-cli/src/cmd/{ui.rs,mcp_http.rs,watch.rs,service.rs(new)}`
- `crates/hds-ui/src/lib.rs` (read_request, ui_auth_gate), `crates/hds-ui/src/bin/hds_ui.rs`
- `crates/hds-mcp/src/http.rs` (host/port)
- `installers/{install_server.ps1(new),install_server.command(new)}`, `install_autostart.ps1`,
  `installers/install_ui_task.ps1`, `run_ui.ps1`
- `config.example.yaml`, `.gitignore`

## 9. Ссылки

- `PLAN_SERVER_MULTIUSER.md` — §2 W1/W2/W10, §5 Э1, §9.
- `PLAN_M1_ACCESS_PROJECTS.md` — фундамент M1 (проекты, токены, фильтр).
- `README_SERVER_MULTIUSER.md` — сценарии подсистемы доступа.
- `M2_HANDOFF_PROMPT.md` — промт для нового чата.


