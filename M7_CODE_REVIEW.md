# M7_CODE_REVIEW.md — код-ревью этапа M7 «Управление службами и токенами из UI» (Э7/W10)

> Дата: 10.10.2026 · Ветка: **`server-multiuser`** · HEAD (на момент ревью): `77634a7`
> (docs-промт поверх `94d3469` — T6-доклад, этап закрыт), дерево чистое.
> Объём: коммиты `14567a8`…`94d3469` (код T0–T5 + docs T6) + docs `888f438`/`77634a7`.
> Код: `crates/hds-core/src/{service,lib}.rs`, `crates/hds-cli/src/cmd/{service,auth}.rs`,
> `crates/hds-cli/src/main.rs`, `crates/hds-ui/src/{lib,page}.rs`,
> `crates/hds-index/src/diag.rs`, `crates/hds-ui/tests/ui_core.rs`
> (15 файлов изменено, +1687/−369).
> Метод: чтение кода (полные диффы M7 и итоговое состояние) + **независимые проверки дерева**:
> `cargo test --workspace` — **88 наборов / 0 failed**, `cargo fmt --all --check` и
> `cargo clippy --workspace --all-targets -- -D warnings` — чисто. Живые SCM/токен-стенды
> в этой сессии **не перезапускал** (сессия не админ; артефакты `tools/parity/out/m7_*`),
> их утверждения сверял **по коду и артефактам** — и независимо воспроизвёл локальный дефект
> локализации SCM (**M7-1**) на этой станции.

## 1. Итог

Код M7 **принят** (findings некритичные, переносятся в хвосты). Контракты §3 плана
реализованы и согласованно разделены между `hds-core`, `hds-cli` и `hds-ui` без цикла
зависимостей:

* **общий модуль служб** `hds_core::service` (T1): роли `watch|mcp-http|ui|llm-host`, имена
  `hermes-disk-search-<role>` / `com.hermes-disk-search.<role>`, `sc_args`/`sc_install_raw`
  (опции `create` — через `raw_arg`, фикс T5), `daemon_plist`, `status`
  (`parse_sc_state`/`parse_sc_autostart` — по **значениям**, не меткам), `control`,
  `ServiceStatus`/`ControlResult`. `windows-service` в `hds-core` **не протекает**
  (проверено `Cargo.toml`: зависимости нет; `service run` остался в CLI);
* **`GET/POST /api/services`** (T2): `GET` — 4 роли с живым состоянием
  (`kind = service|absent`, `state = running|stopped|absent|unknown`, `autostart`, `detail`),
  desktop → пусто + `hint`; `POST {role, action}` — `install|uninstall|start|stop|restart`,
  валидация роль/действие до профиля → `400 {error, hint}`, профиль/права — честные ответы
  `{ok:false, msg, hint}`, `ui`-restart — отложенный detached helper (ответ до перезапуска);
  POST идёт под admin-гейтом / CSRF / аудитом `admin_op` (запись **после** маршрута, с
  фактическим статусом — фикс M3-F2 сохранён);
* **выдача токенов** (T3): CLI `hds token new --user L [--ttl-days N]` и UI
  `POST /api/access/token/new {user, ttl_days?}` → plain-токен **один раз** в ответе,
  в `data/access.yaml` — только `token_sha256`, в аудит `admin_op` — **без токена**;
  отказ для отсутствующего/`disabled` логина;
* **UI** (T4): карточка «Службы (server-профиль)» (роли + кнопки + desktop-сообщение) и
  «Выдать токен» в карточке «Доступ» (plain показывается один раз через `<pre>`); попутно
  восстановлены обработчики `accessList/Add/Del`, на которые ссылалась разметка;
* **десктоп-профиль не изменён** (`config.rs` вне диффа M7); старые точки
  (`/api/watch/daemon`, `/api/mcp-http`, `/api/llm-host/restart`, `/api/transcribe/daemon`)
  не тронуты.

Незакрытых дефектов уровня «ломает прод» не найдено. Найдены **4 некритичные находки**
(§3) плюс 2 наблюдения; главная — **M7-1 (medium)**: на русской Windows проверка прав SCM
(`permission_denied`) не срабатывает из-за `to_ascii_lowercase` (кириллица не понижается),
поэтому UI не показывает hint «запустите от администратора» (частичный пробел п. §3.5 плана
и приёмки §5 «ошибки прав честные»).

## 2. Что проверено (независимо и по коду)

| Проверка | Как проверено | Результат |
|---|---|---|
| Полный набор тестов | `cargo test --workspace --no-fail-fast` (независимо; службы остановлены и возвращены) | **88 наборов / 0 failed** ✅ (совпадает с журналом M7) |
| Формат/линт | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` | чисто ✅ |
| Общий модуль служб (T1) | код `hds-core/src/service.rs`; `hds-cli/src/cmd/service.rs` — тонкая обёртка + `pub use` + `service run` в CLI | соответствует §3.1 ✅ |
| Нет протечки `windows-service` | `crates/hds-core/Cargo.toml` — зависимости нет; упоминания только в doc-комментариях `service.rs` | ✅ |
| `sc create` через `raw_arg` | код `control` (#148) → `.raw_arg(sc_install_raw(role, exe))`, тест `sc_install_raw_quotes_binpath`; артефакт T5 — install всех 4 ролей прошёл | ✅ (фикс T5 верен) |
| Локализация `sc qc` | `parse_sc_state` читает `RUNNING`/`STOPPED`, `parse_sc_autostart` — `AUTO_START`; в артефакте T5 (ru-Windows) значения есть (`Тип_запуска : 2 AUTO_START`) | ✅ (фикс T5 верен) |
| Контракт `GET /api/services` | код `services_json_profile`/`services_json`/`service_state_json`; тесты `services_server_lists_four_roles`, `services_desktop_reports_none_and_rejects_actions`, `services_route_get_is_json` | соответствует §3.2 ✅ |
| Валидация `POST /api/services` | код `services_action_json` (`400` до профиля) + `SERVICE_ACTIONS`; тест `services_post_validates_role_and_action`; артефакты `m7_t2_live_*` | ✅ |
| Профиль/права | `services_action_profile`: не-server → `{ok:false, msg, hint}`; нет `hds.exe` → честный `{ok:false}`; разбор прав — `control_hint` → **см. M7-1** | ⚠️ hint прав не срабатывает на ru-Windows (M7-1) |
| Аудит `admin_op` | код `route_with_access` (`is_admin_op = server_mode && POST && /api/`), запись **после** маршрута `status: reply.0`; артефакт `m7_t3_audit.jsonl` (200 для `token/new`, 401 для GET без токена) | соответствует M3 ✅ |
| CSRF на POST | POST-ветка роутера: `if !csrf_ok(h, server_mode)` → `403` **до** `match p` (покрывает `/api/services` и `/api/access/token/new`) | ✅ |
| `ui`-restart (detached) | код `services_restart_ui`/`spawn_deferred_restart` (Windows: `DETACHED_PROCESS`, паузы `ping`, `sc stop`/`start`); иначе — честный отказ | ✅ (по коду; живой прогон — хвост) |
| Выдача токена — CLI | код `token_new_issue` + `cmd_token("new")`; тест `token_new_issue_saves_hash_and_refuses_unknown_user`; живой артефакт `m7_t3_cli_issue.txt` | соответствует §3.3 ✅ |
| Выдача токена — UI | код `access_token_new` (хэш в файл, plain в тело, `ttl_days`/дефолт, отказ для `disabled`); тест `access_token_new_issues_working_token`; аудит без токена | ✅ |
| `page.rs` карточки | код секций «Службы»/«Доступ» + JS `loadServices`/`svcAct`/`accessTokenNew`; тест `page_has_services_and_token_controls`; артефакт `m7_t4_live.txt` | соответствует §3.4 ✅ |
| T0 — хвосты M6 | `diag.rs::role_device_from_cfg` читает `llm.<role>.device` первым + фолбэк + `filter(>=0)`; `check_gpu_roles_live` не печатает `CUDA ,`; тест `role_device_prefers_llm_key` | **M6-1 / M6-4 закрыты** ✅ |
| Десктоп-профиль не изменён | `git diff --stat 14567a8^..94d3469` — `config.rs`/installers не затронуты; desktop-ветки `services_*` возвращают «служб нет» | ✅ |
| Регрессия CLI служб | `hds-cli::cmd::service` реэкспортирует имена, `service_control` печатает потоки и возвращает прежний exit-код; тест `role_from_args_reads_service_role` | ✅ (тексты/коды сохранены) |

## 3. Находки

**M7-1 (medium) — проверка прав SCM не срабатывает на локализованной Windows.**
`crates/hds-core/src/service.rs:96` (`ControlResult::permission_denied`) понижает регистр через
`to_ascii_lowercase()` и ищет `"access is denied"`, `"отказано в доступе"`, `"code 5 "`,
`"код 5 "`. `to_ascii_lowercase` понижает **только ASCII**, поэтому кириллица не меняется:
живое сообщение `sc.exe` на этой станции (`sc.exe stop Dnscache`, сессия без админа) —
`[SC] OpenService: ошибка: 5:\n\nОтказано в доступе.` — содержит «Отказано» с **заглавной**
«О», и `contains("отказано в доступе")` даёт `false` (проверено сниппетом на `rustc`:
`ru_lower_contains=false`, `ascii_denied=false`, `code5=false`, `failed5=true`). Следствие:
в `hds-ui/src/lib.rs::control_hint` ветка «нужны права администратора» **не срабатывает** на
русской Windows (T5 гонялся из-под администратора — этим путём не ходили), и вместо hint
пользователь видит только сырой вывод `sc`. Частичный пробел §3.2/§3.5 («ошибки прав —
честные») и приёмки §5. Рекомендация: `to_lowercase()` (Unicode) вместо `to_ascii_lowercase()`
+ добавить `"ошибка: 5"`/`"failed 5"`/`"код ошибки 5"`; юнит на русский текст.

**M7-2 (medium/low) — `restart` для ролей кроме `ui` — `stop`+`start` без ожидания STOPPED.**
`crates/hds-ui/src/lib.rs:1495` (`services_restart`) делает синхронный `control("stop")` и сразу
`control("start")`. `sc.exe stop` асинхронен (не дожидается `STOPPED`), поэтому `start` может
вернуть 1053/1062 («служба не в том состоянии»), и карточка покажет ошибку при фактически
удавшемся останове. Для роли `ui` сделан отложенный helper с паузами (`ping`), для остальных
ролей пауз/поллинга нет — асимметрия. Приёмка T5 (`m7_t5_scm.ps1`) прогоняла
`install→start→status→stop→uninstall`, **`restart` не проверялся вовсе**. Рекомендация:
опрашивать `status()` до `stopped` (bounded timeout) или переиспользовать паузу-подход;
добавить `restart` в приёмочный SCM-хелпер (M8 T-приёмка).

**M7-3 (low) — `kind:"absent"` при ошибке чтения SCM, не связанной с отсутствием (non-1060).**
`crates/hds-core/src/service.rs:229` (`status`): при неуспешном `sc query` и коде ≠ 1060
(например, отказ в доступе к диспетчеру) возвращается `(installed=false, state="unknown")`,
и наружу уходит `kind = "absent"`. Контракт §3.1/§3.2 закрепляет `kind ∈ {service, absent}`,
поэтому «unknown» там не выразить; но «отказ в доступе» ≠ «не установлена». Карточка при этом
покажет «неизвестно» (по `state`), так что влияние мало. Рекомендация: оставить контракт, но
в `detail` явно отличать «не установлена» от «не удалось прочитать состояние».

**M7-4 (low, macOS-only) — `uninstall` жёстко ставит `code = 0`.**
`crates/hds-core/src/service.rs:306` (`control`, macOS): после `launchctl bootout` код
безусловно обнуляется, а ошибка `bootout` теряется в `stdout`. Идемпотентность «удалить plist»
понятна, но неудачная выгрузка демона отрапортуется успехом. Рекомендация: сохранять код
`bootout` (или отделять «plist удалён» от «демон выгружен»).

**Наблюдения (не находки, вне регресса M7):**
1. `page.rs:233` `esc()` экранирует только `& < >` (без кавычек); `esc(s.role)` попадает в
   `onclick`-атрибут, но `role` приходит из серверной константы `ROLES` — инъекции нет.
   Стоит держать в уме при добавлении произвольных строк в атрибуты.
2. Карточка «Службы» загружается на старте и после действий, но не в 3-секундном
   `setInterval(refresh, …)` (намеренно: `GET /api/services` = 8 процессов `sc.exe` за тик).
   Приёмку §3.4 «автообновление статуса» формулировка выполняет лишь частично.

## 4. Сверка приёмки §5 плана с фактом

| Пункт §5 | Факт | Статус |
|---|---|---|
| `hds service …` без регрессии, `status` — живое состояние, exit-коды сохранены | код-обёртка + `hds-core::service::status`; тест CLI; SCM-цикл `m7_t5_scm.txt` | ✅ |
| Полный SCM-цикл под администратором (4 роли) | `m7_t5_scm.ps1` (UAC), `GET /api/services` ↔ `sc query` совпадает (`service/stopped|running|absent`, `autostart=True`) | ✅ (по артефакту; `llm-host.start` не поднялся как LocalSystem — задокументировано) |
| `GET /api/services`: 4 роли/состояние/автозапуск; `POST`: install/uninstall/start/stop/restart | код + тесты (`ui_core.rs`); артефакты `m7_t2_live_*`; `restart` живьём не гонялся (**M7-2**) | ⚠️ кроме `restart` |
| Карточка «Службы»; desktop — сообщение | `page.rs` + тест `page_has_services_and_token_controls`; `m7_t4_live.txt` | ✅ |
| `hds token new` / `POST /api/access/token/new` выдают рабочий токен | тесты + живой прогон (`m7_t3_live.txt`: Bearer `200`, без токена `401`, отзыв → `401`); в файл — хэш; plain не в аудите (`m7_t3_audit.jsonl`) | ✅ |
| Перезапуск роли `ui` не виснет (detached helper) | код (`services_restart_ui`/`spawn_deferred_restart`), ответ до перезапуска | ⚠️ живой прогон на установленной службе UI — хвост (как в T5) |
| Десктоп-профиль не изменён; `hds check` ok | `config.rs` вне диффа; desktop-ветки → «служб нет»/отказ | ✅ |
| `cargo test`/`fmt`/`clippy` | независимо: 88/0, fmt чисто, clippy `-D warnings` чисто | ✅ |
| Рантайм-проверки (SCM, токен, карточки) на живом стенде | артефакты `tools/parity/out/m7_*`; стенд возвращён | ✅ (по артефактам; см. §2 про перепроверку) |

## 5. Статус хвостов после M7

| Хвост | Статус |
|---|---|
| Перезапуск роли `ui` из самого UI на живом SCM-стенде | ⏸ открыт (T5-хвост) |
| `restart` для ролей кроме `ui` (порядок stop/start) | ⏸ новый — **M7-2** |
| Хинт «нет прав» на локализованной Windows | ⏸ новый — **M7-1** |
| M7-3 (kind при ошибке чтения SCM), M7-4 (macOS `uninstall`) | ⏸ новые — низкие (§3) |
| **M6-1** (`gpu_roles` не читал `llm.<role>.device`) | ✅ закрыт в T0 (юнит `role_device_prefers_llm_key`) |
| **M6-4** (`CUDA ,`) | ✅ закрыт в T0 |
| **F-m6-t5-1 / M6-2** (клиенты всегда шлют `gpu`) | ⏸ без изменений — ждёт решения владельца |
| M6-3/M6-5/M6-6 | ⏸ без изменений (низкие, вне зоны M7) |
| macOS-путь (launchd): живой цикл | ⏸ без изменений — юниты только (нет Mac) |
| Хвосты M5 (P5/рост fit-шага, анонимизация W9, HTTPS-чек-лист, M5-1…M5-3) | ⏸ без изменений |

## 6. Рекомендации для следующего этапа (Э8/Э9)

1. Починить **M7-1** (одна строка + юнит): это дёшево и возвращает «честные ошибки прав» на
   локализованной Windows; попутно — **M7-2** (поллинг `stopped` перед `start`), т.к. Э8 будет
   ставить службы и дергать `restart` из установщика/UI.
2. Э8 (серверный пакет) опирается на уже готовые `hds service …` и `/api/services`: контракты
   служб/установки зафиксированы M2/M7 — переиспользовать `hds-core::service` в установщике,
   не дублировать имена/роли.
3. Учесть хвост M7 «занятые `*.exe`» (резидент/службы держат бинарники) — в Э8 запланировать
   окно обновления с перезапуском служб (риск §6 `PLAN_SERVER_MULTIUSER.md`).
4. Э9 (клиент) — тонкая проводка `hds cline-sync`; `installers/cline_mcp_merge.py` уже есть,
   обобщить на «URL сервера + токен + health-чек».
5. Низкие M7-3/M7-4 и M6-3/M6-5/M6-6 — в контекст эксплуатационной доводки (Э10), не блокируют.

## 7. Как воспроизведены проверки

```powershell
# независимая проверка дерева (эта сессия; живые debug-службы остановлены и возвращены)
cargo test --workspace --no-fail-fast        # 88 наборов / 0 failed
cargo fmt --all --check                       # чисто
cargo clippy --workspace --all-targets -- -D warnings   # чисто

# независимое воспроизведение M7-1 (сессия без админа)
[Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent() `
  .IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)   # False
sc.exe stop Dnscache 2>&1                                             # "...ошибка: 5:\n\nОтказано в доступе."
# `to_ascii_lowercase()` не понижает кириллицу → ru_lower_contains=false (сниппет rustc)
```

> Живые SCM/токен-прогоны этапа (`m7_t5_scm.txt`, `m7_t2_live_*`, `m7_t3_*`, `m7_t4_live.txt`)
> в этой сессии **не перезапускались**: сессия Cline — не администратор, а UAC-хелпер T5
> требует подтверждения владельца. Их утверждения сверены с кодом и артефактами (§2);
> расхождений, кроме M7-1/M7-2, не найдено. Артефакты ревью — `tools/parity/out/m7_review_*`
> (гитигнор).
