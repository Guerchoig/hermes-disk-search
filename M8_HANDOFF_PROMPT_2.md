# M8_HANDOFF_PROMPT_2.md — промт для нового чата (этап M8/Э8 «серверный пакет установки»: осталось T5–T6)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние, точка
> входа, что сделано, что осталось, контракты, правила и грабли окружения.
>
> **Актуальный промт.** Заменяет по состоянию `M8_HANDOFF_PROMPT.md` (тот писался до T1/T2/T4).
> Этап **M8/Э8** «Серверный пакет установки Win/Apple Silicon» (W6) — из дорожной карты
> `PLAN_SERVER_MULTIUSER.md` §5. **Согласованный объём:** Э8 первым, минимальный
> (**T0 → T1 → T2 → T4 → T5 → T6**), формат zip+скрипты, прокси только образцом; **Э9 (клиент) и
> его T3 отложены** в следующий этап. Детальный план — **`PLAN_M8_INSTALL_PACKAGES.md`**
> (§3 контракты, §4 треки, §5 приёмка, §7 «Решения владельца»).

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние на момент передачи: **HEAD `2a55440`**, дерево чистое. Боевой стенд поднят
(резидент `llm_host` `target\release`, `hds watch`/`hds mcp-http`/`hds transcribe-watch` из
`target\debug`, `index.pause` — есть). M1–M7 закрыты и прошли код-ревью (`M6_CODE_REVIEW.md`,
`M7_CODE_REVIEW.md`).

Станция: Windows, RTX 3060 12 ГБ, AMD Radeon iGraphics (iGPU/UMA), Ryzen 7 5700G, 64 ГБ RAM,
rustc/cargo 1.97.1. Ядро на Rust (`crates/`), Python — только sidecar. Сессия Cline — **не админ**.
Доступен **Git Bash** (`C:\Program Files\Git\bin\bash.exe`; WSL-дистрибутива нет) — для `bash -n`
и прогонов `.command` в песочнице.

## Что УЖЕ сделано в M8 (перепроверить, не «переоткрывать»)

* **T0 (хвосты ревью M7) — `fd5af46`** (`crates/hds-core/src/service.rs`, `crates/hds-ui/src/lib.rs`):
  **M7-1** `ControlResult::permission_denied` — Unicode `to_lowercase()` + `"failed 5"`/`"ошибка: 5"`
  (юнит `permission_denied_matches_localized_denied`); **M7-2** `services_restart` — `wait_stopped()`
  (bounded 10 с) перед `start`; **M7-4** macOS `uninstall` сохраняет код `launchctl bootout`.
  Хелпер приёмки `tools/parity/out/m8_t0_scm.ps1` (гитигнор, нужен UAC).
* **T1 (unattended серверный установщик Windows) — `44e20a8`** (`installers/install_server.ps1`,
  `crates/hds-cli/tests/install_server_parity.rs`): параметры
  `-StateDir/-McpHost/-McpPort/-UiHost/-UiPort/-Parallel/-GpuDevice/-GpuPreset/-Roots/-EnableAccess/
  -AdminLogin/-AdminPassword/-Fetch/-NoFetch/-WhatIf`; `-WhatIf` (без изменений/админа),
  `-Roles ""` (config-only); генерация server-`config.yaml` (не затирает существующий); админ через
  `hds user add --admin` (пароль — только хэш); оркестрация `fetch_*` (в них **нет** `Read-Host`).
* **T2 (серверный пакет macOS) — `6db2f69`** (`installers/install_server.command`,
  parity-тест): тот же набор bash-флагами (`--statedir/--mcp-*/--ui-*/--parallel/--gpu-*/--roots/
  --enable-access/--admin-*/--fetch/--no-fetch/--whatif`), `gen_config` зеркалит `.ps1`, `pwsh`-fetch,
  `codesign --force -s -`, `xattr -dr com.apple.quarantine`, позиционные `[root_dir] [roles]`.
  **Исправлен баг:** `printf '%s'` без перевода строки терял последний корень в `--roots`.
* **T4 (обновление/откат) — `49e80e0`** (`installers/update.ps1`, `install_server_parity.rs`):
  `-Profile desktop|server` (server — стоп SCM-служб `ui→mcp-http→watch→llm-host` с ожиданием
  `STOPPED`, установка, старт `llm-host→watch→mcp-http→ui`, health `/health`), `-RollbackTo`,
  `-ListVersions`, `-WhatIf`; **фикс** `-Force:$Force` (передавался строкой во внешний
  `powershell` и падал). Окно обслуживания (занятые `*.exe`) — стоп всех держателей до подмены.
* **Доклад-коммиты:** `976d7da` (T0), `89865b3` (T1), `bbcb1b9` (T2), `a95018c` (T4), `2a55440`
  (сужение T5 до серверной приёмки). План/решения — `b5c0310`/`a07747f`.
* **Проверки (независимо):** `cargo test --workspace` — **89 наборов / 0 failed**
  (88 + новый таргет `install_server_parity` с 3 тестами); `fmt --check`, clippy `-D warnings` —
  чисто. Живые песочницы: T1 config-only (валидный YAML + хэш-админ, `tools/parity/out/m8_t1_check.py`),
  T2 `--whatif` (`tools/parity/out/m8_t2_check.py`), T4 реальные откат+обновление (junction
  `app\current`). Артефакты — `tools/parity/out/m8_*` (гитигнор).

## Что ОСТАЛОСЬ: T5 (приёмка) → T6 (доклад)

### T5. Приёмка: чистый сервер (Э8; клиент — в Э9) — 1–2 дн
Цель — подтвердить «одной командой» серверный пакет на живом стенде. **Требует администратора
(SCM) и сети (fetch)**, поэтому разделяйте на две части:

**(a) без админа — сделать сразу и зафиксировать:**
- `install_server.ps1 -WhatIf` (полный набор параметров) и `install_server.command --whatif` —
  план + тело конфига (Git Bash, песочница);
- config-only установка в песочницу (`-StateDir <temp> -Roles "" -NoFetch -KeepAtLogOn
  -EnableAccess -AdminLogin admin -AdminPassword <sandbox pw>`), проверить YAML + `access.yaml`
  (только хэш) — переиспользовать `tools/parity/out/m8_t1_check.py`;
- `update.ps1 -ListVersions` / `-WhatIf` / реальные откат+обновление в песочнице (junction);
- `cargo test --workspace` (89/0), fmt/clippy.

**(b) с админом (хвост, воспроизводимо через UAC-хелпер, образец `tools/parity/out/m7_t5_scm.ps1`):**
- полный цикл: `install_server.ps1` (admin, реальный `bin\hds.exe`) → 4 службы SCM →
  `GET /api/services` = `running/auto` ↔ `sc query`; включённый доступ + админ;
- MCP за TLS-proxy (образцы `installers/proxy/`, `trusted_proxy: true`): с токеном `200`, без — `401`;
- обновление `update.ps1 -Profile server` (без «отказано в доступе») + `-RollbackTo`;
- desktop-профиль не сломан (регресс).
- новый SCM-хелпер — `tools/parity/out/m8_t5_scm.ps1` (артефакт, гитигнор), отчёт — `m8_t5_*.txt`.

**Клиентские проверки (Cline-конфиг, `GET /health` с клиента) — отложены вместе с Э9/T3.**

### T6. Доклад — 0,5 дн
- `README_SERVER_MULTIUSER.md` §8d «Установка сервер/клиент» (+ ссылка из §6), `README.md`
  (раздел про M8), `STATUS.md` (блок M8), `PLAN_SERVER_MULTIUSER.md` §9 — статус-блок M8
  (решения §5 **не менять**).
- Отметить этап **M8/Э8 закрытым**; обновить/закрыть `M8_HANDOFF_PROMPT*.md`; при необходимости —
  промт для следующего этапа (**Э9/T3** — клиентский пакет, Cline без Hermes).

## Точка входа (5 минут)

1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -16`   # HEAD 2a55440
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`      # чисто
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe`: остановить
   `hds watch`/`hds mcp-http`/`hds transcribe-watch`, после — **вернуть**; либо
   `cargo test --workspace --exclude hds-cli`. Ожидание: **89 наборов, 0 failed**.
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
5. Прочитать: **`PLAN_M8_INSTALL_PACKAGES.md`** (§4 — журнал T0/T1/T2/T4, §5 приёмка, §7 решения) →
   `M7_CODE_REVIEW.md` (хвосты, закрытые в T0) → `PLAN_SERVER_MULTIUSER.md` §2 W6/W7, §3, §5 Э8/Э9,
   §9 → `STATUS.md`.
6. Карта кода: `installers/{install_server.ps1,install_server.command,update.ps1,
   install_app_version.ps1,fetch_*.ps1,proxy/}`,
   `crates/hds-cli/tests/install_server_parity.rs`, `crates/hds-core/src/service.rs`,
   `crates/hds-ui/src/lib.rs` (`/api/services`), `crates/hds-cli/src/cmd/{service,auth}.rs`.

## Контракты (код — источник истины, §3 плана)

* Серверный установщик (§3): параметры/`-WhatIf`/`-Roles ""`; без новых параметров — прежнее
  поведение M2. Генерация server-`config.yaml` (не затирает существующий): `profile: server`,
  `access.enabled`, `llm_server.parallel`, `gpu.device_index`/`gpu.preset`, `index.roots`,
  `mcp_http`/`ui`.
* Первичный доступ — через `hds-core::access` (пароль хэшируется; plain — не в логи/аудит).
* Обновление server-профиля: стоп `hermes-disk-search-<role>` (4 роли, ожидание STOPPED) →
  подмена `app\current` → старт → health; откат — `-RollbackTo <ver>` прошлой версии.
* Десктоп-профиль и desktop-установка **не изменяются**.

## Правила и грабли окружения (важно, платил временем)

* **Резидент держит `target\release\*.exe`**, живые службы — `target\debug\hds.exe`: перед
  пересборкой/полным `cargo test` остановить и **вернуть** (`target\debug\hds.exe watch` /
  `… transcribe-watch` / `… mcp --http --host 127.0.0.1 --port 8787 --path /mcp`; резидент —
  `Start-Process target\release\llm_host.exe -ArgumentList run -WindowStyle Hidden`). Dev-машина
  (`cline-rules/dev-machine.md`) — останавливать/возвращать можно.
* **`HDS_CONFIG`/`HDS_ROOT` переживают между элементами `run_commands`** — для песочниц задавать
  и затем **очищать** (`Remove-Item Env:HDS_CONFIG; Remove-Item Env:HDS_ROOT`).
* **Боевой `index.pause` не снимать** (пользовательская пауза). Плейн-токены/секреты не печатать.
* **Полный SCM-цикл требует администратора**; сессия Cline — не админ; вариант без перезапуска
  VS Code — UAC-хелпер через `Start-Process -Verb RunAs` (образцы `tools/parity/out/m7_t5_scm.ps1`,
  `m8_t0_scm.ps1`).
* **`bash` — только Git Bash** (`C:\Program Files\Git\bin\bash.exe`); WSL-дистрибутива нет.
  LF для `*.command` гарантируется `.gitattributes` (`text eol=lf`) — не ломать.
* PowerShell 5.1: `Invoke-WebRequest` без `-UseBasicParsing` зависает; кириллица в консоли
  искажается (читать файлы инструментом); первый вывод команды иногда «клеит» символ — ставить
  заглушку-префикс (`$x=0; …`).
* `read_files` иногда отвечает `[outdated - see the latest file content]` — перечитать диапазон.
* Артефакты — в `tools/parity/out/` (гитигнор); коммиты — атомарные по трекам (`M8/TN: …`,
  `docs(m8): …`); решения §5 дорожной карты **не менять**; чужие незакоммиченные правки не трогать.

## Критерии приёмки (кратко; подробно — `PLAN_M8_INSTALL_PACKAGES.md` §5)

Серверный пакет (Win) одной командой: 4 службы (SCM, `/api/services` = running/auto), `hds check`
ok, доступ включён, админ создан; рантайм/модели скачиваются unattended; MCP за TLS-прокси —
`200`/`401`; обновление без «отказано в доступе» + откат; macOS — `bash -n` + parity-тест
(+ живой хвост «машина с Mac»); desktop не изменён; `cargo test --workspace` зелёный, fmt/clippy
`-D warnings` чисто. Клиентские проверки — в Э9.

