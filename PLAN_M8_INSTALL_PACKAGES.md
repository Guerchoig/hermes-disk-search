# PLAN_M8_INSTALL_PACKAGES.md — детальный план M8 «Пакеты установки: сервер (Э8/W6) и клиент (Э9/W7)»

> Дополняет `PLAN_SERVER_MULTIUSER.md` §2 (**W6/W7**), §3 (состав пакетов), §5 (**Э8/Э9**), §9.
> Структура — по образцу `PLAN_M7_UI_CONTROL.md`; код-ревью предыдущего этапа — `M7_CODE_REVIEW.md`.
>
> **Основание:** M1–M7 закрыты; **M7 прошёл код-ревью** (`M7_CODE_REVIEW.md`, коммит `345a0df`,
> принят; хвост «ui-restart на живом SCM» + находки M7-1…M7-4). По дорожной карте §5 после
> **Э7** (M7) идут **Э8** «Серверный пакет установки Win/Apple Silicon» (W6, 5–8 дн) и
> **Э9** «Клиентский пакет Win/macOS, Cline+Hermes» (W7, 3–5 дн).
> **Статус:** запланирован 10.10.2026. Ветка — **`server-multiuser`**.
> **Объём согласован владельцем (10.10.2026):** делаем **Э8 (серверный пакет) первым**,
> минимальный объём — **T0 → T1 → T2 → T4 → T5 → T6**; **Э9 (клиент) и его T3 отложены** в
> следующий этап (формат — zip+скрипты; прокси только образцом; клиент — Cline без Hermes).
> См. §7 «Решения владельца».
>
> **Зависимости:** M2 (профиль `server`, каталог состояния, службы `hds service` +
> `installers/install_server.*`, headless-UI), M3 (reverse-proxy образцы, аудит), M7 (управление
> службами и токенами из UI, `hds-core::service`). Инсталляционные «кирпичи» — `setup.ps1`/
> `setup.cmd`, `install_macos.command`, `build_rust_release.*`, `build_sidecar.*`, `fetch_*`,
> `update.ps1`.

## 1. Вводные (факты из кода, а не предположения)

| Факт | Где |
|---|---|
| **Серверный установщик есть, но «голый»:** ставит 4 роли через `hds service install`, кладёт шаблон `config.yaml` в каталог состояния, удаляет desktop-задачи Планировщика, копирует образец прокси. Нет скачивания рантайма/моделей, нет раскладки ролей, нет включения доступа/админа | `installers/install_server.ps1`, `installers/install_server.command` |
| **Скачивание рантайма/моделей интерактивно:** `fetch_engine_runtime.ps1`, `fetch_llm_models.ps1`, `fetch_whisper_model.ps1`, `fetch_clip_models.ps1` используют `Read-Host`/приглашения — в unattended-пакет так не годится (W6 требует параметры) | `installers/fetch_*.ps1` |
| **Обновление не знает про службы:** `update.ps1` останавливает задачи Планировщика и резидент `llm_host`, но **не** SCM-службы/LaunchDaemon → на сервере занятые `*.exe` дадут «отказано в доступе» при подмене | `installers/update.ps1`, `installers/install_app_version.ps1` |
| **Клиентская проводка уже есть, но локальная:** `hds cline-sync` (тот же код, что кнопка UI `POST /api/cline/sync`) пишет MCP-сервер, окна контекста моделей, правило и скилл **из локального `config.yaml`**; сценарий «внешний сервер + Bearer» не выделен | `crates/hds-cli/src/cmd/cline_sync.rs`, `crates/hds-core/src/cline.rs`, `installers/cline_mcp_merge.py`, `install_cline.ps1`, `installers/install_cline_macos.sh` |
| **Hermes-установка есть** (опционально) | `installers/install_hermes_macos.sh`, `install_cline_macos.sh` |
| **Формат поставки — zip + скрипты,** MSI/`.pkg` нет; сборка релиза | `build_rust_release.ps1`/`.macos.sh`, `build_sidecar.*`, CI `.github/workflows/release.yml` |
| **Профиль/службы/права:** службы только в `profile: server`; состояние `%ProgramData%\hermes-disk-search` / `/Library/Application Support/...`; управление SCM из UI уже есть | M2, M7 |
| **Раскладка GPU и `parallel` — конфигом** (`gpu.devices`/`gpu.preset`, `llm_server.parallel`), `hds check` валидирует | M5/M6 |
| **Health-эндпоинты есть:** `/health` у MCP-сервера и UI (клиент может «проверить соединение») | `crates/hds-mcp/src/http.rs`, `crates/hds-ui/src/lib.rs` |

## 2. Область

**Входит (Э8/W6, сервер):**
- unattended-режим серверного установщика (параметры вместо `Read-Host`) Win/macOS;
- генерация `config.yaml` серверного профиля «из коробки»: `profile: server`, bind за прокси,
  `parallel`, раскладка ролей (`gpu.devices`/`gpu.preset`), корни индексации, включение доступа
  и первичный админ;
- скачивание рантайма движка/моделей без интеракции (по манифестам);
- службы SCM/LaunchDaemon + старт, образец reverse-proxy с TLS;
- обновление серверного профиля с окном обслуживания (останов служб → подмена → старт), откат.

**Не входит:** анонимизация (W9, отдельный шаг), нагрузочная приёмка на 1/2 картах/Apple
Silicon (Э11), эксплуатация healthz/бэкап (Э10), изменения схемы индекса/поиска.

**Клиент (Э9/W7) — ⏸ отложен в следующий этап (решение владельца 10.10.2026):** тонкий пакет
Win/macOS — `hds` + мастер «адрес сервера + токен» → `GET /health` → запись Cline-конфига
(MCP URL + Bearer + правило + скилл), **Cline без Hermes**; **без** движка/моделей/индекса/sidecar.
Трек T3 остаётся в этом плане как задел для следующего этапа.

## 3. Контракты (код — источник истины)

* **Серверный установщик (unattended):** `install_server.ps1`/`install_server.command` принимают
  параметры (пример): `-StateDir`, `-McpHost/-McpPort`, `-UiHost/-UiPort`, `-Parallel`,
  `-GpuDevice/-GpuPreset`, `-Roots`, `-Roles`, `-AdminLogin/-AdminPassword` (или `-AdminToken`),
  `-EnableAccess`, `-Proxy caddy|nginx|none`, `-NoFetch`, `-WhatIf`. Без параметров — прежнее
  поведение (обратная совместимость).
* **Первичная выдача доступа:** после установки `access.enabled: true` и админ-запись в
  `data/access.yaml` (пароль хэшируется; plain-пароль/токен — не в логах), затем
  `hds token new --user <admin>` при необходимости — **переиспользовать** `hds-core::access`.
* **Клиентский мастер:** `setup-client.ps1`/`install_client.command` параметрами
  `-ServerUrl https://host/` `-Token <bearer>` (или интерактивно); шаги: `GET <ServerUrl>/health`
  → `hds cline-sync --server-url --token` → «перезапустите Cline». Никаких движка/моделей.
* **`hds cline-sync` (расширение):** новые флаги `--server-url <url> --token <bearer>
  [--mcp-path /mcp]`; в этом режиме MCP-запись указывает на **внешний** URL с заголовком
  `Authorization`, окна моделей — без локального `llm_server` (или из ответа сервера), правило/
  скилл — как сейчас. Без флагов — прежнее локальное поведение.
* **Health:** клиент использует существующий `GET /health` (MCP) и/или `GET /api/status` (UI
  admin) — новых серверных протоколов не вводить.
* **Обновление серверного профиля:** останов `hermes-disk-search-<role>` (все 4) → подмена
  `app\current` → старт служб → `hds check`/`GET /api/services` (health-проверка). Откат —
  прежний `app\current`. Десктопный `update.ps1` (задачи Планировщика) не ломать.
* **Совместимость:** desktop-профиль и desktop-установка не изменяются; `config.yaml`
  расширяется аддитивно.

## 4. Треки T0–T6

### T0. Хвосты ревью M7 + регресс-проверка — 0,5–1 дн — ✅ **выполнено 10.10.2026** (`fd5af46`)
- [x] **M7-1** (medium): `ControlResult::permission_denied` — `to_lowercase()` (Unicode) вместо
      `to_ascii_lowercase()` + проверки `"failed 5"`/`"ошибка: 5"`; юнит
      `permission_denied_matches_localized_denied` (рус. + англ. + 1060-негатив) — зелёный.
- [x] **M7-2** (medium/low): `restart` ролей кроме `ui` — `wait_stopped()` (bounded 10 с, шаг
      400 мс) перед `start`, диагностика таймаута в `hint`; приёмочный SCM-хелпер с фазой
      `restart` через `POST /api/services` — `tools/parity/out/m8_t0_scm.ps1` (артефакт, UAC).
- [x] **M7-4** (low): macOS `uninstall` — код `launchctl bootout` сохранён (в сообщении),
      успех определяется по удалению плейста (идемпотентно; логика проверена изолированным
      сниппетом — macOS-таргет на станции недоступен).
- [x] Регресс: `cargo test --workspace` — **88 наборов / 0 failed**; `fmt --check`, clippy
      `-D warnings` — чисто (живые debug-службы остановлены и возвращены).
- **Хвост T0:** живой SCM-прогон хелпера `m8_t0_scm.ps1` (нужен администратор/UAC) — при
  следующей админ-сессии; на этой станции проверены код+юнит+изолированный сниппет.
- **Следующий шаг — T1** (unattended серверный установщик, Windows).

### T1. Серверный установщик, unattended (Windows) — 2–3 дн — ✅ **выполнено 10.10.2026** (`44e20a8`)
- [x] Параметризовать `install_server.ps1` (§3): `-StateDir/-McpHost/-McpPort/-UiHost/-UiPort/
      -Parallel/-GpuDevice/-GpuPreset/-Roots/-EnableAccess/-AdminLogin/-AdminPassword/-Fetch/
      -NoFetch/-WhatIf`; `-WhatIf` — печать плана + тела конфига **без изменений и без админа**;
      `-Roles ""` — config-only (без SCM/админа). Без новых параметров — прежнее поведение.
- [x] Unattended-загрузка: установщик вызывает `fetch_engine_runtime.ps1`/`fetch_llm_models.ps1`/
      `fetch_whisper_model.ps1`/`fetch_clip_models.ps1` (параметрами, идемпотентно). **Уточнение к
      §1 плана:** `Read-Host` в `fetch_*` нет (проверено grep) — они уже unattended, правок не
      потребовалось; установщик лишь оркестрирует и перехватывает ошибки.
- [x] Генерация `config.yaml` серверного профиля: `profile: server`, `access.enabled`,
      `llm_server.parallel`, `gpu.device_index`/`gpu.preset`, `index.roots`, `mcp_http`/`ui`;
      существующий конфиг **не затирается** (идемпотентно).
- [x] Админ: `hds user add --admin` (пароль — **только хэш** в `data/access.yaml`, идемпотентно
      через `user passwd`); хинт `token new --user`.
- [x] Юнит паритета `install_server_ps1_has_unattended_params` (набор параметров + секции +
      fetch-хуки + хинт токена); macOS-паритет — плейсхолдер `#[ignore]` до T2.
- Проверка: `-WhatIf` без админа; песочница config-only (валидный YAML, хэшированный админ,
      `tools/parity/out/m8_t1_check.py`); `cargo test --workspace` — **89 наборов / 0 failed**,
      fmt/clippy чисто. **Следующий шаг — T2** (macOS-аналог `.command`).

### T2. Серверный пакет macOS/Apple Silicon — 1,5–2 дн — ✅ **выполнено 10.10.2026** (`6db2f69`)
- [x] `install_server.command`: тот же unattended-набор (`--statedir/--mcp-*/--ui-*/--parallel/
      --gpu-*/--roots/--enable-access/--admin-*/--fetch/--no-fetch/--whatif`), обратная
      совместимость с позиционными `[root_dir] [roles]`; LaunchDaemon — через `hds service install`
      (как было); `--whatif` без root/изменений; `--roles ""` — config-only.
- [x] Генерация конфига зеркалит `install_server.ps1` (`New-ServerConfig` ↔ `gen_config`);
      **найден и исправлен баг** `printf '%s'` без перевода строки — `while read` терял
      последний корень из `--roots` (macOS-only).
- [x] Metal-рантайм/модели: `install_server.command` вызывает `fetch_*.ps1` через `pwsh`
      (macos-ветки внутри скриптов); без `pwsh` — понятный пропуск с подсказкой.
- [x] Ad-hoc подпись (`codesign --force -s -`) по каждому бинарю в `bin/`; снятие
      Gatekeeper-карантина (`xattr -dr com.apple.quarantine`); `chmod +x` — best effort.
- [x] Образцы прокси Caddyfile/nginx.conf копируются как и раньше (`trusted_proxy: true` — в
      подсказке/доке, не автоправка конфига).
- [x] Тесты: `install_server_command_has_unattended_params` — `#[ignore]` снят, проверяет флаги +
      секции конфига + `launchctl`/`codesign`/`xattr`/sudo-гейт. Синтаксис — `bash -n` (Git Bash),
      `--whatif` выведен и YAML-проверен (`tools/parity/out/m8_t2_check.py`).
- **Хвост T2:** живой цикл на реальном Mac (LaunchDaemon/`spec`/codesign) — как в M6/M7.
- **Следующий шаг — T4** (обновление/откат серверного профиля).

### T3. Клиентский пакет (Win/macOS, Cline+Hermes) — 2–3 дн — ⏸ **отложен (Э9, следующий этап)**
- [ ] Расширить `hds cline-sync` флагами `--server-url/--token/--mcp-path` (§3); `--dry-run`.
- [ ] Мастер `setup-client.ps1`/`install_client.command`: адрес+токен → `GET /health` →
      запись конфига → «перезапустите Cline»; опционально Hermes.
- [ ] Тонкий пакет: только `hds(.exe)`; никаких движка/моделей/индекса/sidecar.
- [ ] Юниты: формирование MCP-записи с внешним URL+Bearer (по образцу `hds_core::cline`).

### T4. Обновление/откат серверного профиля — 1–2 дн — ✅ **выполнено 10.10.2026** (`49e80e0`)
- [x] `update.ps1` получил `-Profile desktop|server`. **server:** останавливает SCM-службы
      `hermes-disk-search-<role>` в порядке зависимостей (`ui→mcp-http→watch→llm-host`) с ожиданием
      `STOPPED` (bounded 30 с), затем `install_app_version.ps1`, затем старт
      (`llm-host→watch→mcp-http→ui`, ожидание `RUNNING` 60 с) и health-чек
      `http://127.0.0.1:8787/health` (переопределяется `-HealthUrl`). Требует администратора.
      **desktop** — прежний путь (задачи Планировщика + резидент).
- [x] Откат: `-RollbackTo <ver>` — стоп → переключение junction `app\current` → `app\<ver>` →
      старт; `-ListVersions` — список версий (`*` — текущая). `-WhatIf` — план без изменений.
- [x] **Попутный фикс (был и в прежней desktop-версии):** `-Force:$Force` при вызове внешнего
      `powershell` передавался строкой и падал на `[switch]`; теперь аргументы собираются явным
      массивом (`-Force` добавляется только при `$Force`).
- [x] Юнит `update_ps1_is_server_profile_aware` (серверный профиль/службы/откат/листинг + прежняя
      desktop-ветка). Проверка: `-ListVersions`/`-WhatIf`(server)/`-WhatIf`(rollback) + **реальные**
      откат и обновление на песочнице (`app\current` переключается); `cargo test --workspace` —
      **89 наборов / 0 failed**, fmt/clippy чисто.
- **Окно обслуживания (занятые `*.exe`):** до подмены `app\current` останавливаются **все** держатели
  бинарей — SCM-службы (server) либо задачи Планировщика + резидент (desktop); это снимает
  «отказано в доступе» при обновлении. Файлы прошлых версий сохраняются, откат — секунды.
- **Хвост T4:** живое обновление на реальных SCM-службах (нужен админ) — при админ-сессии.
- **Следующий шаг — T5** (приёмка: чистый сервер + тонкий клиент).

### T5. Приёмка: чистый сервер (Э8; клиент — в Э9) — 1–2 дн — ◐ **частично (a) 10.10.2026**
- [x] **(a) без администратора** — выполнено в этой сессии, артефакты `tools/parity/out/m8_t5_*`:
      `install_server.ps1 -WhatIf` (полный набор параметров) и `install_server.command --whatif`
      (Git Bash; `bash -n` — чисто) — план + тело конфига, оба YAML-проверены (`m8_t2_check.py`);
      config-only установка в песочницу (`-Roles "" -NoFetch -KeepAtLogOn -EnableAccess
      -AdminLogin admin -AdminPassword <sandbox>`): `m8_t1_check.py` — YAML ok, админ только
      хэшем (argon2id), plain не утёк; `update.ps1 -ListVersions`/`-WhatIf` (server + откат) и
      **реальные** обновление и откат в песочнице (`app\current` junction переключается,
      health `/health` → 200). `cargo test --workspace` — **89/0** (повторный прогон; в первом —
      флейк сокет-теста `quota_socket::unauthorized_heavy_rejected_before_queue`, изолированно
      зелёный), fmt/clippy `-D warnings` чисто.
- [ ] **(b) с администратором** — хвост; воспроизводимо через UAC-хелпер
      `tools/parity/out/m8_t5_scm.ps1` (отчёт `m8_t5_scm.txt`): `install_server.ps1` (admin,
      реальный `bin\hds.exe`, layout `app\current`) → 4 службы SCM → `GET /api/services` =
      running/auto ↔ `sc query`; доступ включён + админ; MCP-гейт (без токена `401`, с токеном
      `200`); обновление `update.ps1 -Profile server` без «отказано в доступе» + `-RollbackTo`;
      desktop не сломан. **TLS-прокси** (Caddy/nginx) — вручную (образцы `installers/proxy/`,
      `trusted_proxy: true`); на станции Caddy/nginx не установлены.
- [ ] **Клиентские проверки (Cline-конфиг, `GET /health` с клиента) — отложены вместе с Э9** (T3).
- [x] Артефакты — `tools/parity/out/m8_t5_*` (гитигнор); стенд возвращён (3 debug-процесса
      `hds watch` / `transcribe-watch` / `mcp --http` + резидент `llm_host`), `index.pause` не снимался.

### T6. Доклад — 0,5 дн — ✅ **выполнено 10.10.2026**
- [x] `README_SERVER_MULTIUSER.md` §8d «Установка серверного пакета (M8)» (+ ссылка из §6),
      `README.md` (раздел «Установка серверного пакета (M8)»), `STATUS.md` (блок M8),
      `PLAN_SERVER_MULTIUSER.md` §9 — статус-блок M8 (решения §5 не менялись).
- [x] Промт для следующего этапа — `M8_HANDOFF_PROMPT_3.md` (Э9/T3, клиентский пакет);
      `M8_HANDOFF_PROMPT*.md` помечены закрытыми по состоянию Э8.

## 5. Приёмка

> Статус на 10.10.2026: **(a)** без администратора — выполнено (см. T5); **(b)** с администратором
> (SCM/TLS) — хвост, воспроизводится UAC-хелпером `tools/parity/out/m8_t5_scm.ps1`.

- [ ] (b) Серверный пакет (Win) на «чистой» машине: одна команда → службы (SCM), `/api/services`
      = running/auto для 4 ролей, `hds check` ok; доступ включён, админ создан. — ◐ (a) config/план ok;
      SCM-цикл — хелпер `m8_t5_scm.ps1`
- [x] Рантайм/модели скачиваются **unattended** (без `Read-Host`), по манифестам. — `fetch_*`
      уже без `Read-Host`; оркестрация — в `install_server.*` (`-Fetch`/`-NoFetch`)
- [ ] (b) MCP за TLS-прокси: с токеном — `200`, без токена — `401`; UI только админ. — ◐ гейт
      `401`/`200` проверяется хелпером по HTTP; TLS-прокси (Caddy/nginx) — вручную
- [ ] Клиентский пакет: мастер поднимает Cline только проводкой (без локальных моделей);
      `GET /health` ok, поиск работает; плейн-токен не в логах. — ⏸ **отложено вместе с Э9/T3**
- [x] Обновление серверного профиля без «отказано в доступе»; откат проверен. — ◐ реальные
      обновление/откат в песочнице (junction, health 200); живой SCM — хелпер `m8_t5_scm.ps1`
- [x] macOS-ветки: юниты зелёные; живой цикл — хвост «машина с Mac». — `bash -n` + parity-тест
- [x] Десктоп-профиль/установка не изменены; `cargo test --workspace` зелёный, fmt/clippy
      `-D warnings` чисто. — 89/0, fmt/clippy чисто
- [x] Рантайм-проверки на живом стенде (не только юниты). — ◐ (a) песочницы; (b) — хелпер

## 6. Риски

| Риск | Митигация |
|---|---|
| Занятые `*.exe` при обновлении (резидент/службы) | Окно обслуживания: стоп служб → подмена → старт (T4); версионирование + откат |
| Интерактивные `fetch_*` ломают unattended | Обёртки с параметрами; `-NoFetch` для офлайн-монтажа |
| macOS-путь не проверяется на этой станции | Юниты + хвост «машина с Mac» (как M6/M7) |
| Плейн-админ-пароль/токен в логах установщика | Хэшировать, не печатать; аудит без секретов (как M7) |
| Клиент и сервер рассинхронизированы (правило/скилл/схема инструментов) | Версия клиента согласуется с сервером; `hds cline-sync` идемпотентен |
| Смешение с desktop-установкой (`config.yaml`) | Параметр `-StateDir`; desktop-пути не трогать; регресс |
| Формат пакета (zip vs MSI/`.pkg`) | ✅ решено 10.10.2026: **zip + скрипты** (§7) |

## 7. Решения владельца (согласовано 10.10.2026)

1. **Формат пакета:** **zip + скрипты** (`install_server.*`/`setup-client.*`) — MSI/`.pkg` не делаем.
2. **Порядок:** **Э8 (сервер) первым**, минимальный объём; **Э9 (клиент) отложен** в следующий
   этап (план сохраняется для него).
3. **Reverse-proxy:** только **образцы** (`installers/proxy/`) — автоустановка Caddy/nginx не входит.
4. **Клиент (Э9, отложен):** Cline, **без Hermes** (в отложенном объёме — только Cline).
5. **Первый админ:** согласуется в T1 (по умолчанию — параметры `-AdminLogin/-AdminPassword`
   при установке; токен при необходимости выписывается `hds token new`).

**Следствие для треков:** в текущем этапе выполняются **T0 → T1 → T2 → T4 → T5 → T6**;
**T3 (клиент) переносится** вместе с Э9 в следующий этап.

## 8. Ссылки
- `PLAN_SERVER_MULTIUSER.md` — §2 **W6/W7**, §3 (состав), §5 **Э8/Э9**, §6 (риски), §9 (статусы).
- `M7_CODE_REVIEW.md` — ревью предыдущего этапа (находки M7-1…M7-4 → T0).
- `PLAN_M2_SERVER_PROFILE.md` §4 (службы/`install_server.*`), `PLAN_M3_NETWORK_PERIMETER.md`
  (прокси/аудит), `PLAN_M7_UI_CONTROL.md` (`/api/services`, токены).
- Код: `crates/hds-cli/src/cmd/{cline_sync,service,auth}.rs`, `crates/hds-core/src/{cline,service}.rs`,
  `crates/hds-ui/src/lib.rs`, `installers/*`.
- Артефакты — `tools/parity/out/` (гитигнор).
