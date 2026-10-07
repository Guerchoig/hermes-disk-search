# PLAN_M3_NETWORK_PERIMETER.md — детальный план M3 («Сетевой периметр: TLS-прокси, аудит, rate-limit», Э2-сеть)

> Дополняет `PLAN_SERVER_MULTIUSER.md` §5 (этап **Э2**, сетевая часть), §2 (**W2**).
> Структура — по образцу `PLAN_M2_SERVER_PROFILE.md`. Промт для нового чата —
> `M3_HANDOFF_PROMPT.md`. Код-ревью предыдущего этапа — `M2_CODE_REVIEW.md`.
>
> **Основание:** решения заказчика 07.10.2026 (`PLAN_SERVER_MULTIUSER.md`, шапка):
> периметр — **reverse-proxy с TLS** (наружу только `/mcp` и `/ui`); **UI — только
> админ**; идентификация — локальные токены. M1 сделал «приложение»-часть Э2
> (логин/пароль → авто-токены, Bearer, admin-гейтинг), M2 — серверный профиль и
> службы (Э1). Осталась **сетевая часть Э2**: TLS/прокси, аудит, rate-limit
> (`README_SERVER_MULTIUSER.md`: «TLS/reverse-proxy, аудит и rate-limit — следующий
> этап (Э2); здесь сервис считается закрытым в доверенной сети»).
>
> **Статус:** запланирован 07.10.2026. Ветка — **`server-multiuser`**.
>
> **Зависимости:** M1 (токены/Bearer/admin — основа для аудита и rate-limit по
> токену) и M2 (профиль `server`, state dir, службы — куда пишется аудит-лог).
> Включает трек **T0** — быстрые фиксы дефектов M2 (`M2_CODE_REVIEW.md` §5).

## 1. Вводные (факты из кода, а не предположения)

| Факт | Где |
|---|---|
| MCP-сервер (`hds mcp-http`) — свой минимальный HTTP/1.1-сервер, свой `read_request`/`route`; авторизация по Bearer вынесена в `auth::resolve_bearer` | `crates/hds-mcp/src/{http.rs,auth.rs}` |
| UI (`hds ui`) — тоже свой HTTP-сервер (`read_request`/`route_with_access`/`ui_auth_gate`) | `crates/hds-ui/src/lib.rs` |
| Логирования событий доступа **нет** (ни в MCP, ни в UI, ни в `hds-core::access`); есть только `eprintln!`/лог процессов | — (поиск `audit` по `crates/` пуст) |
| Rate-limit / ограничения частоты **нет**; на каждый запрос — поток (`std::thread::spawn`), тела запроса не ограничены (только заголовки ≤1 МБ) | `hds-mcp/src/http.rs:130`, `hds-ui/src/lib.rs:1560` |
| Bind host/port из конфига уже есть (`ui.*`, `mcp_http.*`), дефолт `127.0.0.1` — под прокси | `crates/hds-ui/src/lib.rs::bind_from_config`, `crates/hds-cli/src/cmd/{ui,mcp_http}.rs` |
| `X-Forwarded-For`/`X-Forwarded-Proto`/`Host` **не читаются** — реальный IP клиента за прокси недоступен (нужно для аудита/лимитов) | `read_request` в обоих серверах |
| CSRF в UI: в server-режиме origin не проверяется (за прокси origin не-loopback) — уже так | `crates/hds-ui/src/lib.rs::csrf_ok` |
| Каталог состояния (server) и `data/logs/` уже резолвятся (`state_dir`/`ensure_state_dir`) — аудит-лог ляжет туда | `crates/hds-core/src/config.rs`, `crates/hds-index/src/diag.rs` (`data/logs`) |
| Инсталлятор server-профиля уже есть (`installers/install_server.*`) — добавить туда шаг «поставить/подсказать прокси» | `installers/install_server.ps1/.command` |
| Эскиз Caddy для прокси описан в плане, файла в репо нет | `PLAN_SERVER_MULTIUSER.md` §8.4 |

## 2. Границы M3

**Входит:**
- **TLS / reverse-proxy:** поставочные шаблоны прокси (Caddy + nginx) и шаг
  установки; обработка заголовков прокси (`X-Forwarded-For/Proto`, `Host`) в
  MCP/UI; явная документация «наружу только `/mcp` и `/ui`».
- **Аудит:** структурированный append-only лог событий доступа/действий
  (JSONL) в `state_dir/data/logs/audit.jsonl`, с ротацией по размеру; запись
  логина (успех/отказ), выдачи/отзыва токена, вызовов инструментов (кто/какие
  проекты/что), админ-операций; конфиг `audit: {...}`.
- **Rate-limit:** лимиты по токену/по IP на MCP (`/mcp`, `/auth/login`) и на UI;
  ответ `429` с ретрай-заголовком; конфиг `rate_limit: {...}`; учёт `X-Forwarded-For`.
- **Сетевая обвязка/hardening:** лимит размера тела запроса, ограничение
  числа одновременных соединений (или пул), полезные заголовки безопасности
  (не раскрывать внутренности), единый формат `401/429`.
- **T0 (быстрые фиксы из ревью M2):** `DisplayName` службы (F1), порт из конфига
  в `run_ui.ps1` (F4), `HDS_ROOT` в `llm-host` (F3).
- Тесты (юнит + сокет), обновление `README`/`README_SERVER_MULTIUSER.md`,
  `STATUS.md`, `PLAN_SERVER_MULTIUSER.md` §9.

**Не входит (следующие этапы):** управление службами/токенами из UI (Э7/W10);
GPU-раскладка по картам (Э5/W4); внешняя чат-модель (Э6/W9); пакеты установки
Win/macOS (Э8/Э9); эксплуатация `healthz`/бэкап (Э10); нагрузочная приёмка
(Э11). **Десктоп-профиль сохраняет нынешнее поведение** (без прокси, без
аудита/лимитов по умолчанию — или аудит выключен, см. §3.2).


## 3. Технические решения (контракты)

### 3.1. Reverse-proxy и TLS
- Прокси — **внешний** (Caddy/nginx), приложение слушает `127.0.0.1` (уже так —
  `ui.*`/`mcp_http.*`). В репозиторий кладём **образцы**: `installers/proxy/Caddyfile`
  и `installers/proxy/nginx.conf` (наружу только `/mcp*` → `:8787`, `/ui*` → `:8765`;
  фасад `:8010–8012` — только localhost; TLS Let's Encrypt/свой сертификат).
- `installers/install_server.*` получает **опциональный** шаг: если найден
  `caddy`/`nginx` — записать образец и подсказать команду; иначе — только сообщение
  с путями к образцам (не падать).
- Приложение **учитывает заголовки прокси**: при `trusted_proxy: true`
  (конфиг) реальный клиент берётся из `X-Forwarded-For` (последний доверенный хоп),
  схема — из `X-Forwarded-Proto`; иначе — `remote_addr`. Заголовки читаются в
  `read_request` (оба сервера) и кладутся в контекст запроса.
- Безопасность по умолчанию: без `trusted_proxy` заголовки прокси **игнорируются**
  (нельзя подделать IP, сидя напрямую).

### 3.2. Аудит
- Формат — **JSONL** (`одно событие = одна строка`), append-only, файл
  `state_dir/data/logs/audit.jsonl` (server — ProgramData/Application Support;
  desktop — корень проекта). Ротация по размеру (`audit.max_mb`, дефолт 50 →
  `audit.jsonl.1`), без внешних крейтов (пишем сами через `serde_json`, уже в зависимостях).
- Схема события: `{ ts, event, user|null, ip, method, path, tool|null, projects|null,
  status, ms }`. События: `login_ok`/`login_fail`, `token_issue`/`token_revoke`,
  `tool_call` (имя инструмента + проекты), `admin_op`, `unauthorized`.
- Конфиг `audit: { enabled: bool, path?: str, max_mb?: int }`. **Дефолт:** в
  `profile: server` — включён; в `desktop` — выключен (не шумим локально).
- Единая точка записи — новый модуль `hds-core::audit` (`AuditEvent`, `record`),
  вызываемый из `hds-mcp` (route/tools) и `hds-ui` (gate/api). Писатель —
  потокобезопасный (файл с `OpenOptions::append`, короткая строка).
- **Не логируем** тела запросов и токены (только усечённый префикс sha при нужде).

### 3.3. Rate-limit
- In-process token-bucket (без новых крейтов): ключ — **токен** (если есть) иначе
  **IP**; два лимита — «частота запросов» и «параллелизм» (одновременные `ask_my_files`).
  Конфиг `rate_limit: { enabled, requests_per_min, burst, login_per_min, max_inflight }`.
- При превышении — `429 Too Many Requests` + `Retry-After` (и `X-RateLimit-*`).
  Для `/auth/login` — отдельный, более жёсткий лимит (антибрутфорс, по IP логина).
- Учёт за прокси — по `X-Forwarded-For` при `trusted_proxy: true`.
- **Дефолт:** `enabled: false` в desktop; в `server` — разумные значения по умолчанию
  (`requests_per_min` ~ 120, `login_per_min` ~ 10, `max_inflight` по числу слотов движка).
- Состояние лимитера — в памяти процесса; при нескольких процессах (`mcp-http`+`ui`)
  лимиты независимы (документируем; общий лимит — задача прокси).

### 3.4. Сетевая обвязка
- **Лимит тела запроса** (`http.max_body_kb`, дефолт напр. 512): превышение →
  `413`; читаем не более лимита (защита от аллокаций, ревью M2 F8).
- **Ограничение одновременных соединений** (`http.max_conns`): сверх лимита —
  короткий `503`/`429` без спавна неограниченных потоков (простой счётчик в `Arc`).
- Заголовки ответа: не раскрывать версии/внутренние пути в ошибках; единый JSON
  тела ошибок (`{error, hint}`).

### 3.5. Профиль/десктоп
- Всё новое — **по конфигу** и с дефолтами, сохраняющими десктоп: аудит и
  rate-limit выключены, заголовки прокси игнорируются. `profile: server` меняет
  только **дефолты** (аудит вкл.), поведение остаётся управляемым ключами.

## 4. Треки

### T0. Быстрые фиксы дефектов M2 (код-ревью `M2_CODE_REVIEW.md`) — 0,5 дн
- [x] **F1:** `sc_args` — `DisplayName` через `format!("… {role}")`
      (`crates/hds-cli/src/cmd/service.rs:239`); тест проверяет DisplayName.
- [x] **F4:** `run_ui.ps1` берёт `ui.host/port` из конфига (не жёсткий 8765);
      тест/ручная проверка: `ui.port: <не 8765>` → скрипт слушает этот порт.
- [x] **F3:** `hds-llama::host::resident_data_root`/`HostConfig::default` учитывают
      `HDS_ROOT` (зеркально `state_dir`); тест на `HDS_ROOT` добавлен.

### T1. Reverse-proxy + TLS (шаблоны и учёт заголовков) — 1,5–2 дн
- [x] `installers/proxy/{Caddyfile,nginx.conf}` — образцы (только `/mcp*`,`/ui*`).
- [x] `installers/install_server.*` — опциональный шаг прокси (не фатальный).
- [x] `read_request` (MCP/UI) читает `X-Forwarded-For`/`X-Forwarded-Proto`/`Host`;
      при `trusted_proxy` реальный IP идёт в контекст (аудит/лимиты). Тесты.
- [x] README: «наружу только `/mcp` и `/ui`; прокси — TLS; фасад не публикуем».

### T2. Аудит — 1,5–2 дн
- [x] `hds-core::audit` (`AuditEvent`, `record`, ротация) + конфиг `audit`.
- [x] Вызовы: `login_ok/fail`, `token_issue/revoke`, `tool_call`, `admin_op`,
      `unauthorized` — из `hds-mcp`/`hds-ui`/CLI. Тесты (JSONL, ротация,
      desktop → выключен, server → включён).
- [x] `hds check`/UI: пункт о состоянии аудита (путь, размер, вкл/выкл).

### T3. Rate-limit — 1,5–2 дн
- [x] Token-bucket (частота + параллелизм), ключ токен/IP; конфиг `rate_limit`.
- [x] MCP (`/mcp`, `/auth/login`) и UI: `429` + `Retry-After`; `/auth/login`
      жёстче (антибрутфорс). Учёт `X-Forwarded-For` при `trusted_proxy`. Тесты.

### T4. Сетевая обвязка/hardening — 1 дн
- [x] Лимит тела (`http.max_body_kb` → `413`) в обоих серверах (ревью F8).
- [x] Ограничение одновременных соединений (`http.max_conns`).
- [x] Единый формат ошибок `{error, hint}`; не раскрывать внутренности.
- [x] (Опц.) F2: fail-closed `ui_auth_gate` при нечитаемом конфиге в server-профиле.

### T5. Доклад — 0,5 дн
- [x] `README.md`/`README_SERVER_MULTIUSER.md` (прокси, аудит, лимиты), `STATUS.md`,
      `PLAN_SERVER_MULTIUSER.md` §9 (статус M3).

## 5. Приёмка M3 (черновик)

- **Прокси/TLS:** внешний доступ только по HTTPS к `/mcp` и `/ui`; прямой запрос
  к `:8787`/`:8765` снаружи невозможен; фасад `:8010–8012` не опубликован.
- **Аудит:** события доступа пишутся в `state_dir/data/logs/audit.jsonl`
  (login ok/fail, tool_call с проектами, admin_op); ротация работает;
  desktop-профиль по умолчанию **не** пишет аудит.
- **Rate-limit:** превышение частоты → `429` + `Retry-After`; `/auth/login`
  ограничен жёстче; параллелизм `ask_my_files` ограничен; за прокси IP
  определяется по `X-Forwarded-For` (и не подделывается без `trusted_proxy`).
- **Hardening:** тело больше лимита → `413`; сверх `max_conns` — отказ без
  неограниченного спавна потоков; ошибки не раскрывают внутренние пути/версии.
- **Обратная совместимость:** `profile: desktop` (дефолт) — поведение как прежде;
  `cargo test --workspace` зелёный; `clippy -D warnings` и `fmt` — чисто.
- **Дефекты M2 закрыты:** DisplayName службы корректен; `run_ui.ps1` слушает
  порт из конфига; `HDS_ROOT` переносит состояние резидента.

## 6. Порядок работ и коммиты

По зависимостям, один коммит на шаг:
1. **T0** — `fix(service): DisplayName`, `fix(ui): run_ui.ps1 port from config`,
   `fix(paths): honor HDS_ROOT in llm-host`.
2. **T1** — `feat(proxy): TLS reverse-proxy samples + forwarded headers`.
3. **T2** — `feat(audit): access audit log (JSONL, rotation)`.
4. **T3** — `feat(ratelimit): per-token/IP limits on MCP/UI`.
5. **T4** — `feat(http): body limit + connection cap`.
6. **T5** — `docs(server): network perimeter (M3)`.

После каждого шага — тесты + обновление журнала/статуса. Начать с **T0** (быстрые
дефекты), затем T1→T2→T3→T4→T5.

## 7. Риски

| Риск | Митигация |
|---|---|
| Прокси-заголовки подделываются напрямую | игнорировать XFF без `trusted_proxy`; bind только `127.0.0.1` |
| Аудит тормозит горячий путь | короткая append-запись, без блокировок на весь запрос; ротация по размеру |
| Rate-limit по IP за прокси ломает всех (общий IP) | ключ — сначала токен; XFF — последний доверенный хоп; лимиты настраиваемы |
| Лимиты памяти/соединений ломают легитимную нагрузку | дефолты под 5 пользователей; параметры в конфиге; тесты границ |
| Десктоп ломается новыми ключами | дефолты выключены, профиль меняет только дефолты; регресс-тесты |
| Прокси-образец ведёт к ложной уверенности в TLS | в README — явно: TLS терминирует прокси, сертификаты настраивает оператор |

## 8. Точки входа по файлам (шпаргалка)

- `crates/hds-core/src/audit.rs` (новый), `crates/hds-core/src/config.rs` (ключи конфига)
- `crates/hds-mcp/src/{http.rs,auth.rs}` — forwarded-заголовки, лимит тела, лимиты, аудит
- `crates/hds-ui/src/lib.rs` — то же; `ui_auth_gate`/`csrf_ok` под прокси
- `crates/hds-cli/src/cmd/{mcp_http.rs,ui.rs}` — прокинуть конфиг лимитов/прокси
- `installers/proxy/{Caddyfile,nginx.conf}` (новые), `installers/install_server.*`
- `run_ui.ps1`, `crates/hds-llama/src/host.rs`, `crates/hds-cli/src/cmd/service.rs`
- `config.example.yaml`, `README.md`, `README_SERVER_MULTIUSER.md`, `STATUS.md`

## 8a. Журнал исполнения (07.10.2026)

Все треки T0–T5 закрыты, по коммиту на шаг; после каждого — `cargo test
--workspace`, `fmt`, `clippy -D warnings` (зелёные на финальном HEAD).

| Шаг | Коммит | Что сделано / рантайм-проверки |
|---|---|---|
| T0.1 F1 | `156f2c5` `fix(service)` | `DisplayName` через `format!`; тест ловит литерал `<role>` |
| T0.2 F4 | `561d0bd` `fix(ui)` | `run_ui.ps1` читает `ui.host/port` из конфига, без `--port`; **e2e в песочнице**: конфиг `ui.port: 18790` → UI слушает 18790, 8765 молчит |
| T0.3 F3 | `3cf7f8c` `fix(paths)` | `HDS_ROOT` сильнее `profile: server` в `resident_data_root` (тест + **рантайм**: pid/лог ушли в `$HDS_ROOT\data`, не в ProgramData) |
| T1 | `bef86b0` `feat(proxy)` | образцы `installers/proxy/{Caddyfile,nginx.conf}`; `hds-core::net::{trusted_proxy,real_client_ip,request_scheme}`; XFF в `read_request` обоих серверов; `client_ip` в `/health`; шаг прокси в `install_server.*`; **рантайм**: trusted=true → `client_ip=10.0.0.2` (правая запись XFF), trusted=false → `127.0.0.1` |
| T2 | `5aa2ee6` `feat(audit)` | `hds-core::audit` (JSONL, ротация `audit.jsonl` → `.1`); события login_ok/fail, unauthorized, tool_call, admin_op, token_revoke; пункт `audit` в `hds check`/`/api/diagnostics`; **рантайм**: живой логин/отказ/401 → три корректные строки JSONL при `profile: server` (аудит включён по умолчанию) |
| T3 | `f1a7dad` `feat(ratelimit)` | `hds-core::ratelimit` (token-bucket, login-лимит, max_inflight); 429 + `Retry-After` + `X-RateLimit-Limit` на MCP и UI; **рантайм**: burst 5 → 5×200, 6-й → `429` c `Retry-After: 1` |
| T4 | `55f4d24` `feat(http)` | `http.max_body_kb` → 413 без чтения тела; `http.max_conns` → 503 без спавна (RAII-декремент); единый `{error,hint}`; F2 fail-closed UI-гейта; **рантайм**: тело 5 КБ при лимите 1 КиБ → `413 {"error":"payload too large",...}` |
| T5 | (этот коммит) `docs(server)` | README/README_SERVER_MULTIUSER §11, STATUS.md, PLAN_SERVER_MULTIUSER §9, этот журнал; в репозиторий добавлены `PLAN_M3_NETWORK_PERIMETER.md`, `M2_CODE_REVIEW.md`, `M3_HANDOFF_PROMPT.md` |

Приёмка (§5): всё закрыто, кроме полного внешнего HTTPS-обхода (нужен домен и
сертификаты — вне компа разработки; образцы и заголовки проверены сокетами).
`cargo test --workspace` зелёный; fmt/clippy `-D warnings` — чисто.


## 9. Ссылки

- `PLAN_SERVER_MULTIUSER.md` — §2 W2, §5 Э2, §8.4 (эскиз Caddy), §9.
- `PLAN_M2_SERVER_PROFILE.md` — предыдущий этап (профиль/службы).
- `M2_CODE_REVIEW.md` — находки F1–F10 (источник трека T0).
- `README_SERVER_MULTIUSER.md` — сценарии доступа.
- `M3_HANDOFF_PROMPT.md` — промт для нового чата.

