# M8_HANDOFF_PROMPT.md — промт для нового чата (этап M8/Э8+Э9: пакеты установки сервер/клиент)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние, точка
> входа, объём, контракты, где детали, правила и грабли окружения.
>
> **Этап M7/Э7 закрыт (T0–T6) и прошёл независимое код-ревью** (`M7_CODE_REVIEW.md`, коммит
> `345a0df`, принят; главная находка **M7-1** — medium, локализация отказа SCM). По дорожной
> карте `PLAN_SERVER_MULTIUSER.md` §5 следующий этап — **M8/Э8 «Серверный пакет установки
> Win/Apple Silicon»** (W6, 5–8 дн); **Э9 (клиентский пакет) отложен** в следующий этап.
> **Объём согласован владельцем (10.10.2026):** Э8 первым, минимальный объём
> (**T0 → T1 → T2 → T4 → T5 → T6**), формат zip+скрипты, прокси только образцом.
> Детальный план — **`PLAN_M8_INSTALL_PACKAGES.md`** (§3 контракты, §4 треки, §5 приёмка,
> §7 «Решения владельца»).

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние на момент передачи: **HEAD — верх `git log`** после docs-коммитов M8 (план + промт);
ранее — ревью `345a0df`, промт ревью `77634a7`, этап M7 закрыт на `94d3469`. Дерево чистое.
Боевой стенд поднят (резидент `llm_host` `target\release`, `hds watch`/`hds mcp-http`/
`hds transcribe-watch` из `target\debug`, `index.pause` — есть). M1–M7 закрыты; M6/M7 прошли
код-ревью (`M6_CODE_REVIEW.md`, `M7_CODE_REVIEW.md`).

Станция: Windows, RTX 3060 12 ГБ, AMD Radeon iGraphics (iGPU, UMA), Ryzen 7 5700G, 64 ГБ RAM,
rustc/cargo 1.97.1. Ядро на Rust (`crates/`), Python — только sidecar.

## Объём M8 (кратко; подробно — `PLAN_M8_INSTALL_PACKAGES.md`)

1. **T0 — хвосты ревью M7 — ✅ сделано 10.10.2026 (`fd5af46`; следующий шаг — T1):**
   **M7-1** (`permission_denied`: Unicode `to_lowercase()` + проверки `"failed 5"`/`"ошибка: 5"`;
   юнит на рус. текст), **M7-2** (`restart` ролей кроме `ui`: `wait_stopped()` перед `start`),
   **M7-4** (macOS `uninstall` сохраняет код `bootout`).
2. **T1 — серверный установщик unattended (Windows) — ✅ сделано 10.10.2026 (`44e20a8`;
   следующий шаг — T2):** параметры `install_server.ps1` (`-StateDir/-Parallel/-Gpu*/-Roots/
   -EnableAccess/-Admin*/-Fetch/-NoFetch/-WhatIf`), генерация server-`config.yaml`, админ
   (хэш), unattended-вызов `fetch_*` (они уже без `Read-Host`), parity-тест.
3. **T2 — серверный пакет macOS/Apple Silicon — ✅ сделано 10.10.2026 (`6db2f69`;
   следующий шаг — T4):** `install_server.command` с тем же unattended-набором (bash-флаги),
   генерация конфига, `pwsh`-fetch, `codesign`/`xattr`, parity-тест (macOS больше не `#[ignore]`).
4. **T3 — клиентский пакет (Win/macOS):** ⏸ **отложен вместе с Э9** (следующий этап) —
   `hds cline-sync --server-url --token`, мастер «адрес+токен → `GET /health` → Cline-конфиг»,
   Cline без Hermes; без локальных моделей/индекса.
5. **T4 — обновление/откат серверного профиля:** `update.ps1` с остановом SCM-служб (окно
   обслуживания), откат `app\current`.
6. **T5 — приёмка:** чистый сервер (одна команда → 4 службы → `/api/services` → MCP за TLS) +
   тонкий клиент (одна команда → рабочий поиск, локально пусто); обновление с откатом.
7. **T6 — доклад:** README/`README_SERVER_MULTIUSER.md` §8d/`STATUS.md`/§9 дорожной карты.

**Решения владельца зафиксированы (10.10.2026, §7 плана):** формат — zip+скрипты; порядок —
Э8 первым; прокси — только образцом; клиент (Э9) отложен, Cline без Hermes; первый админ —
параметры `-AdminLogin/-AdminPassword` в T1.

## Сделано в T0 (`fd5af46`, 10.10.2026)

* `crates/hds-core/src/service.rs`:
  * `ControlResult::permission_denied` — `to_lowercase()` (Unicode) вместо `to_ascii_lowercase()`
    + `"failed 5"`/`"ошибка: 5"`; юнит `permission_denied_matches_localized_denied`
    (рус. «Отказано в доступе.» / англ. / 1060-негатив) — **зелёный**;
  * macOS `control("uninstall")` — код `launchctl bootout` сохранён (в сообщении), успех — по
    удалению плейста (идемпотентно).
* `crates/hds-ui/src/lib.rs`: `services_restart` — `wait_stopped()` (bounded 10 с, шаг 400 мс)
  перед `start`; таймаут → `hint` (роль `ui` — прежний detached helper).
* Приёмочный SCM-хелпер `tools/parity/out/m8_t0_scm.ps1` (гитигнор; нужен админ/UAC) — фаза
  `restart` через `POST /api/services`.
* Регресс: `cargo test --workspace` — **88 наборов / 0 failed**; `fmt --check`/clippy `-D warnings`
  чисто. **Хвост:** живой SCM-прогон хелпера — при админ-сессии.

## Сделано в T1 (`44e20a8`, 10.10.2026)

* `installers/install_server.ps1` переписан под unattended (обратная совместимость: без новых
  параметров — прежнее поведение M2): параметры `-StateDir/-McpHost/-McpPort/-UiHost/-UiPort/
  -Parallel/-GpuDevice/-GpuPreset/-Roots/-EnableAccess/-AdminLogin/-AdminPassword/-Fetch/
  -NoFetch/-WhatIf`; `-WhatIf` — план + тело конфига **без изменений и без админа**; `-Roles ""`
  — config-only. Генерация `config.yaml`: `profile: server`, `access.enabled`,
  `llm_server.parallel`, `gpu.device_index`/`gpu.preset`, `index.roots`, `mcp_http`/`ui`
  (существующий конфиг не затирается). Админ — `hds user add --admin` (пароль только хэш,
  идемпотентно `user passwd`), хинт `token new --user`. **Уточнение:** `fetch_*` уже без
  `Read-Host` — установщик их оркестрирует (идемпотентно, перехват ошибок).
* Паритет-тест `crates/hds-cli/tests/install_server_parity.rs` (`install_server_ps1_has_unattended_params`;
  macOS-паритет — `#[ignore]` до T2).
* Проверка: `-WhatIf` без админа; песочница config-only (валидный YAML + хэшированный админ,
  `tools/parity/out/m8_t1_check.py`); `cargo test --workspace` — **89 наборов / 0 failed**,
  fmt/clippy чисто. **Хвост:** живой прогон с `-Fetch` (сеть) и установка служб — на админ-стенде.

## Сделано в T2 (`6db2f69`, 10.10.2026)

* `installers/install_server.command` (macOS) переписан под тот же unattended-набор: bash-флаги
  `--statedir/--mcp-host/--mcp-port/--ui-host/--ui-port/--parallel/--gpu-device/--gpu-preset/
  --roots/--enable-access/--admin-login/--admin-password/--fetch/--no-fetch/--whatif` +
  обратная совместимость с позиционными `[root_dir] [roles]`. `gen_config` зеркалит
  `New-ServerConfig` из `.ps1`; `--whatif` без root/изменений; `--roles ""` — config-only.
* **Найден и исправлен баг:** `printf '%s'` без перевода строки — `while read` терял последний
  корень из `--roots` (только macOS-ветка; в PowerShell `-split` корректен).
* Fetch: вызывает `fetch_*.ps1` через `pwsh` (без `pwsh` — понятный пропуск). Ad-hoc подпись
  (`codesign --force -s -`) + снятие карантина (`xattr -dr com.apple.quarantine`). LF-инвариант
  обеспечивает `.gitattributes` (`*.command text eol=lf`).
* Parity-тест: `install_server_command_has_unattended_params` (`#[ignore]` снят). Проверка:
  `bash -n` (Git Bash) — ok; `--whatif` → YAML-валидный конфиг (`tools/parity/out/m8_t2_check.py`);
  `cargo test --workspace` — **89 наборов / 0 failed**, fmt/clippy чисто.
  **Хвост:** живой цикл на реальном Mac (LaunchDaemon/codesign/`spec`).

## Контракты (код — источник истины, §3 плана)

* Серверный установщик — параметры (`-StateDir`, `-McpHost/-McpPort`, `-UiHost/-UiPort`,
  `-Parallel`, `-GpuDevice/-GpuPreset`, `-Roots`, `-Roles`, `-AdminLogin/-AdminPassword`
  или `-AdminToken`, `-EnableAccess`, `-Proxy caddy|nginx|none`, `-NoFetch`, `-WhatIf`);
  без параметров — прежнее поведение.
* Первичный доступ — через `hds-core::access` (пароль хэшируется; plain — не в логи/аудит).
* Клиент: `setup-client.ps1`/`install_client.command` (`-ServerUrl`, `-Token`) → `GET /health` →
  `hds cline-sync --server-url --token`; MCP-запись с внешним URL + заголовком `Authorization`.
* Обновление server-профиля: стоп `hermes-disk-search-<role>` (4 роли) → подмена `app\current`
  → старт → `hds check`/`GET /api/services`; откат — прежний `app\current`.
* Десктоп-профиль и desktop-установка **не изменяются**.

## Точка входа (5 минут)

1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -16`
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`     # чисто
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe`: остановить
   `hds watch`/`hds mcp-http`/`hds transcribe-watch`, после — **вернуть**; либо
   `cargo test --workspace --exclude hds-cli`. Ожидание: **88 наборов, 0 failed**.
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
5. Прочитать: **`PLAN_M8_INSTALL_PACKAGES.md`** → `M7_CODE_REVIEW.md` §3 (хвосты T0) →
   `PLAN_SERVER_MULTIUSER.md` §2 W6/W7, §3, §5 Э8/Э9, §6 рисков, §7 (закрытые вопросы) →
   `PLAN_M2_SERVER_PROFILE.md` §4 (службы/`install_server.*`) → `STATUS.md`.
6. Карта кода: `installers/{install_server.ps1,install_server.command,update.ps1,
   install_app_version.ps1,cline_mcp_merge.py,fetch_*.ps1,build_rust_release.*}`,
   `crates/hds-cli/src/cmd/{cline_sync,service,auth}.rs`, `crates/hds-core/src/{cline,service,access}.rs`,
   `crates/hds-ui/src/lib.rs` (`/api/services`, `/api/cline/sync`), `installers/proxy/`.

## Правила и грабли окружения (важно, платил временем)

* **Резидент держит `target\release\*.exe`**, живые службы — `target\debug\hds.exe`: перед
  пересборкой/полным `cargo test` остановить и **вернуть** (команды: `target\debug\hds.exe
  watch` / `… transcribe-watch` / `… mcp --http --host 127.0.0.1 --port 8787 --path /mcp`;
  резидент — `Start-Process target\release\llm_host.exe -ArgumentList run -WindowStyle Hidden`).
  Это dev-машина (правило `cline-rules/dev-machine.md`) — останавливать/возвращать можно.
* **`HDS_CONFIG`/`HDS_ROOT` переживают между элементами `run_commands`** — для песочниц задавать
  и затем **очищать** (`Remove-Item Env:HDS_CONFIG; Remove-Item Env:HDS_ROOT`).
* **Боевой `index.pause` не снимать** (пользовательская пауза). Плейн-токены/секреты не печатать.
* **Полный SCM-цикл требует администратора**; сессия Cline — обычно **не** админ; вариант без
  перезапуска VS Code — UAC-хелпер через `Start-Process -Verb RunAs` (образец
  `tools/parity/out/m7_t5_scm.ps1`).
* PowerShell 5.1: `Invoke-WebRequest` без `-UseBasicParsing` зависает; кириллица в консоли
  искажается (читать файлы инструментом); первый вывод команды иногда «клеит» символ — ставить
  заглушку-префикс (`$x=0; …`).
* `read_files` иногда отвечает `[outdated - see the latest file content]` — перечитать диапазон.
* Артефакты — в `tools/parity/out/` (гитигнор); коммиты — атомарные по трекам (`M8/TN: …`,
  `docs(m8): …`); решения §5 дорожной карты **не менять**; чужие незакоммиченные правки не трогать.

## Приёмка (кратко; подробно — `PLAN_M8_INSTALL_PACKAGES.md` §5)

Серверный пакет (Win) одной командой: 4 службы (SCM, `/api/services` = running/auto), `hds check`
ok, доступ включён, админ создан; рантайм/модели скачиваются unattended; MCP за TLS-прокси —
`200`/`401`; клиентский мастер поднимает Cline проводкой (без локальных моделей), `GET /health`
ok, поиск работает; обновление без «отказано в доступе» + откат; macOS — юниты (+ хвост «машина с
Mac»); desktop не изменён; `cargo test --workspace` зелёный, fmt/clippy `-D warnings` чисто.
