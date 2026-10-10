# M7_CODE_REVIEW_PROMPT.md — промт для нового чата (ревью этапа M7, затем Э8/Э9 по дорожной карте)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, объём ревью, где детали, правила и грабли окружения.
>
> **Этап M7/Э7 «Управление службами и токенами из UI» закрыт (T0–T6, 10.10.2026).**
> **Задача 1 — независимое код-ревью этапа** → создать **`M7_CODE_REVIEW.md`** по образцу
> `M6_CODE_REVIEW.md` (тот же формат: шапка с HEAD/объёмом/методом, §1 итог-вердикт,
> §2 таблица «что проверено», §3 находки со severity, §4/§5 хвосты). M6 прошёл так же
> (код-ревью `M6_CODE_REVIEW.md`, коммит `4449fab`, принят).
> **Задача 2 (после ревью) — далее по дорожной карте** `PLAN_SERVER_MULTIUSER.md` §5:
> **Э8** «Серверный пакет установки Win/Apple Silicon» (W6, 5–8 дн) и **Э9** «Клиентский пакет
> Win/macOS, Cline+Hermes» (W7, 3–5 дн). По образцу предыдущих этапов — завести план этапа
> (`PLAN_M8_*.md`) с треками T0–T6, контрактами и приёмкой, согласовать объём с владельцем.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние на момент передачи: **HEAD `94d3469`**, дерево чистое; боевой стенд поднят
(резидент `llm_host`, `watch`/`mcp-http`/`transcribe-watch`, `index.pause` — есть).
M1–M7 закрыты; M6 прошёл код-ревью (`M6_CODE_REVIEW.md`, принят). Предыдущий этап M6 закрыт
`ffc450c` (T6-доклад); M7 — коммиты ниже.

Станция: Windows, RTX 3060 12 ГБ (дискретный GPU), AMD Radeon iGraphics (iGPU, UMA),
Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo 1.97.1. Ядро на Rust (`crates/`), Python — только sidecar.

## Задача 1 — объём код-ревью M7

Коммиты этапа (код):
* **T0** `14567a8` — `crates/hds-index/src/diag.rs` (M6-1/M6-4: `gpu_roles` читает
  `llm.<role>.device` первым; нет `CUDA ,`).
* **T1** `cd01065` — новый `crates/hds-core/src/service.rs`; `crates/hds-core/src/lib.rs`;
  `crates/hds-cli/src/cmd/service.rs` (тонкая обёртка, `service run` остался в CLI).
* **T2** `112d6ee` — `crates/hds-ui/src/lib.rs` (`GET/POST /api/services`),
  `crates/hds-ui/tests/ui_core.rs`.
* **T3** `8980338` — `crates/hds-cli/src/cmd/auth.rs`, `crates/hds-cli/src/main.rs`,
  `crates/hds-ui/src/lib.rs` (`POST /api/access/token/new`), тесты.
* **T4** `5592f2f` — `crates/hds-ui/src/page.rs` (карточки «Службы»/«Доступ»),
  `crates/hds-ui/tests/ui_core.rs`.
* **T5** `d8dc1eb` — `crates/hds-core/src/service.rs` (2 фикса SCM: `raw_arg`, парсеры по значениям).

Документы этапа: `PLAN_M7_UI_CONTROL.md` (§3 контракты, §4 треки T0–T6, §5 приёмка),
`M7_HANDOFF_PROMPT.md`, `STATUS.md` (блок M7), `PLAN_SERVER_MULTIUSER.md` §9 (статус M7),
`README_SERVER_MULTIUSER.md` §8c, `README.md`.

Что проверять (метод M6-ревью — чтение кода + независимые прогоны, не доверяя журналу):
* **`hds-core::service`** — роли/имена, `sc_args`/`sc_install_raw` (`raw_arg`!), `daemon_plist`,
  `status` (парсеры `parse_sc_state`/`parse_sc_autostart` — по значениям, не меткам),
  `control` (коды/`permission_denied`), `ServiceStatus`/`ControlResult`; нет ли протечки
  `windows-service` в `hds-core` (не должно быть).
* **`/api/services`** — контракт §3.2; профиль/права (§3.5); аудит `admin_op` (общий блок
  `is_admin_op`); валидация роль/действие → `400`; `ui`-restart (detached helper) — не виснет.
* **Выдача токенов** — `token_new_issue` (CLI) и `access_token_new` (UI): plain-токен один раз,
  в файл только `token_sha256`, **нет plain в логах/аудите**, отказ для отсутствующего/`disabled`.
* **`page.rs`** — карточки, восстановленные `accessList/Add/Del`, escape/CSRF-инварианты.
* **Десктоп-профиль не изменён** (службы/токены — server-only; `hds check` ok).

## Что уже зафиксировано (перепроверить, не «переоткрывать»)

* `cargo test --workspace` — **88 наборов / 0 failed**; `fmt`/`clippy --all-targets -D warnings` — чисто.
* Живой SCM-цикл **под администратором** (UAC-хелпер `tools/parity/out/m7_t5_scm.ps1`):
  `install→start→status→stop→uninstall` по 4 ролям; `GET /api/services` == `sc query`
  (`service/stopped|running|absent`, `autostart=True`). Отчёт — `tools/parity/out/m7_t5_scm.txt`.
  Для роли `llm-host` фаза `start` не поднимается как LocalSystem (debug `llm_host` без
  GPU-контекста) — остальные фазы цикла отработали.
* Токен: живой прогон (песочница server+access) — выдача UI/CLI, Bearer `200`, без токена `401`,
  отзыв → `401`, аудит `admin_op` без plain-токена. Артефакты — `m7_t3_{cli_issue,live}.txt`,
  `m7_t3_audit.jsonl`.
* Desktop: `/api/services` пусто+hint, кнопок нет; страница отдаётся с новыми контролами
  (артефакт `m7_t4_live.txt`).
* **Уже исправлены в T5** (не считать новыми находками, но проверить корректность фиксов):
  (1) `sc.exe create` отвергал параметры при передаче `binPath` через argv (Rust перекавычивает)
  → опции `create` идут `raw_arg` (`sc_install_raw`); (2) `autostart` всегда был `false` из-за
  локализации `sc qc` (`Тип_запуска`) → парсеры читают значения, не метки.
* **Открытый хвост M7:** живой прогон перезапуска роли `ui` из самого UI (код — detached helper,
  ответ до перезапуска) на живом SCM-стенде не делали.

## Точка входа (5 минут)

1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -16`  # HEAD — этот промт
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`     # чисто
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe`: остановить
   `hds watch`/`hds mcp-http`/`hds transcribe-watch`, после — **вернуть**; либо
   `cargo test --workspace --exclude hds-cli`. Ожидание: **88 наборов, 0 failed**.
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
5. Прочитать: **`PLAN_M7_UI_CONTROL.md`** → `M7_HANDOFF_PROMPT.md` → `M6_CODE_REVIEW.md`
   (образец ревью) → `PLAN_SERVER_MULTIUSER.md` §2 W10/§5 (дорожная карта)/§9 → `STATUS.md` (блок M7).
6. Карта кода: `crates/hds-core/src/service.rs`, `crates/hds-cli/src/cmd/{service,auth}.rs`,
   `crates/hds-ui/src/{lib,page}.rs`, `crates/hds-ui/tests/ui_core.rs`.

## Правила и грабли окружения (важно, платил временем)

* **Резидент держит `target\release\*.exe`**, живые службы — `target\debug\hds.exe`: перед
  пересборкой/полным `cargo test` остановить и **вернуть**. Резидент запускается вручную
  (`Start-Process target\release\llm_host.exe -ArgumentList run -WindowStyle Hidden`); задачи
  Планировщика у него нет (проверено).
* **`HDS_CONFIG`/`HDS_ROOT` переживают между элементами `run_commands`** — для песочниц задавать
  и затем **очищать** (`Remove-Item Env:HDS_CONFIG; Remove-Item Env:HDS_ROOT`); иначе процессы
  поднимутся на песочном конфиге.
* **Боевой `index.pause` не снимать** (пользовательская пауза) — только переиспользовать.
  Плейн-токены/секреты не печатать в ответах и логах.
* **Полный SCM-цикл требует администратора**, а сессия Cline — обычно **не** админ
  (`IsInRole(Administrator)` = False). Вариант без перезапуска VS Code: helper через
  `Start-Process -Verb RunAs` (владелец подтверждает UAC) — так сделан T5
  (`tools/parity/out/m7_t5_scm.ps1`).
* PowerShell 5.1: `Invoke-WebRequest` **без** `-UseBasicParsing` зависает на интерактивном
  запросе (использовать `Invoke-RestMethod` или `-UseBasicParsing`); кириллица в консоли
  искажается (читать файлы инструментом); первый токен команды иногда «клеит» лишний символ —
  ставить заглушку-префикс (`$x=0; …`).
* `read_files` иногда отвечает `[outdated - see the latest file content]` — перечитать диапазон.
* Артефакты — в `tools/parity/out/` (гитигнор); коммиты — атомарные по трекам (`M7/TN: …`,
  `docs(m7): …`); решения §5 дорожной карты **не менять**.

## Критерии вердикта ревью

* Контракты §3 M7 реализованы; дефектов уровня «ломает прод» нет (иначе — severity + хвост).
* Находки оформить как `M7-N` (medium/low) с файлом/строкой и рекомендацией; подтверждать
  кодом и, где можно, тестом — как в `M6_CODE_REVIEW.md`.
* Десктоп-профиль не изменён; plain-токен нигде не утекает; права/профиль — честные ошибки.

## Задача 2 — далее по дорожной карте (`PLAN_SERVER_MULTIUSER.md` §5)

* **Э8 — серверный пакет установки Win/Apple Silicon (W6, 5–8 дн):** `installers/install_server.*`,
  службы SCM/LaunchDaemon, headless-UI, без локальных моделей у клиента. Опирается на M2 (службы,
  `hds service`) и M7 (управление из UI).
* **Э9 — клиентский пакет Win/macOS, Cline+Hermes (W7, 3–5 дн):** мастер, Cline-конфиг
  (`hds cline-sync`), подключение MCP-клиента к серверу.
* Порядок/объём и открытые вопросы — `PLAN_SERVER_MULTIUSER.md` §7 (согласовать с владельцем).
  Завести план этапа по образцу `PLAN_M6_*`/`PLAN_M7_*` (треки T0–T6, контракты, приёмка) и
  handoff-промт для передачи.

