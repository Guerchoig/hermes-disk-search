# M2_HANDOFF_PROMPT.md — промт для нового чата (исполнение M2 «Серверный профиль и службы»)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где лежит детальный план, контракты, правила и грабли.
> Детали задач — `PLAN_M2_SERVER_PROFILE.md`; контекст проекта — `PLAN_SERVER_MULTIUSER.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`** (в ней уже
сделан M1 — `feat(access)` … `docs(migration)`). Станция: Windows, RTX 3060 12 ГБ
(+ AMD-карта), rustc/cargo 1.97.1. Ядро на **Rust** (`crates/`), Python — только `sidecar`.
Тесты: `cargo test --workspace`.

## Что делаем (M2, этап Э1)
Реализуем milestone **M2 «Серверный профиль и службы»**: чтение `profile: desktop|server`,
**серверные пути** состояния (ProgramData / Application Support), **привязка хостов/портов
из конфига** (`ui.*`), **службы Windows (SCM/AtStartup) и macOS (LaunchDaemon)**, **headless
админ-UI**, и — первым делом — **исправление дефектов M1** из код-ревью (трек T0).
Детальный план — **`PLAN_M2_SERVER_PROFILE.md`** (читать целиком: §3 контракты, §4 треки
T0–T5, §6 порядок, §5 приёмка).

## Точка входа (10 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -12`
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`   # должно быть чисто
3. Прочитать **`PLAN_M2_SERVER_PROFILE.md`** (полностью) → затем `PLAN_SERVER_MULTIUSER.md`
   (§2 W1/W2/W10, §5 Э1, §9) и `PLAN_M1_ACCESS_PROJECTS.md` (что уже сделано).
4. Прочитать `README_SERVER_MULTIUSER.md` (сценарии доступа) и
   `tools/parity/HANDOFF_PROMPT.md` (грабли окружения — §«Грабли»).
5. `cargo build --workspace` и `cargo test --workspace` — убедиться, что база зелёная.

## Дефекты M1, которые надо закрыть (T0) — найдены код-ревью 07.10.2026
1. **UI не парсит `Authorization`.** `crates/hds-ui/src/lib.rs::read_request` заполняет
   только `origin/content-type/x-hds-ui/content-length`, а `ui_auth_gate` читает
   `ReqHeaders.authorization` → в server-режиме (`access.enabled: true`) **любой** запрос к
   UI, включая `GET /`, отдаёт `401` даже с валидным админ-токеном (проверено рантаймом:
   запуск `hds ui` + `curl -H "Authorization: Bearer <admin>"` → 401). Юнит-тесты не ловят:
   они конструируют `ReqHeaders` руками. Починить парсинг + добавить тест **через реальный
   сокет** (`TcpListener` + `route`).
2. **`data/access.example.yaml` не в git.** `.gitignore` содержит `data/` и затем
   `!data/access.example.yaml`; ре-инклуд файла внутри исключённого каталога **не работает**
   (`git ls-files data/` пуст, `git check-ignore -v` матчит `data/`). Исправить: `data/*` +
   `!data/access.example.yaml`.
3. **`hds login` собирает JSON через `format!`** (`crates/hds-cli/src/cmd/auth.rs::cmd_login`) —
   пароль с `"`/`\` ломает тело запроса. Перейти на `serde_json::json!`.

## Ключевые решения (заказчик, 07.10.2026; полнее — `PLAN_SERVER_MULTIUSER.md`)
- Периметр — **reverse-proxy с TLS**; наружу только `/mcp` и `/ui`; фасад `:8010–8012` — localhost.
- **UI — только админ**; пользователи ходят через Cline/Hermes (MCP).
- Профиль **`desktop | server`**: server — службы без входа, серверные пути; desktop не ломаем.
- **Службами** управляем из UI (W10/Э7) — но сам M2 делает только службы и CLI.
- Не путать: **M1 = «Проекты» + токены** (сделано), **M2 = Э1 (профиль/службы)**.

## Контракты (кратко; детали — план §3)
- `profile`: `desktop` (дефолт) | `server`; резолвер `config::profile(cfg)`.
- `state_dir(cfg)`: `HDS_ROOT` → иначе `server` = `%ProgramData%\hermes-disk-search` /
  `/Library/Application Support/hermes-disk-search`, `desktop` = корень проекта.
- Секция `ui: { host, port }`; `hds ui`/`hds_ui` читают её (CLI-флаги — переопределение).
- `hds service run <role>` + `hds service install|uninstall|start|stop|status [role]`;
  Windows — SCM или AtStartup-задача (спайк), macOS — LaunchDaemon.
- Автозапуск M1: Планировщик **AtLogOn** (`install_autostart.ps1`) — desktop сохраняем.

## Порядок работы (план §6)
T0 (hotfix) → T1 профиль → T2 пути → T3 порты → T4 службы → T5 headless-UI+доклад.
По одному коммиту на шаг (`fix(ui): …`, `chore(git): …`, `fix(cli): …`, `feat(config): …`,
`feat(paths): …`, `feat(service): …`, `docs(server): …`). После каждого шага — тесты.

## Грабли окружения (кратко; полнее — tools/parity/HANDOFF_PROMPT.md)
- `git` — **всегда** `--no-pager`. PowerShell портит первый токен команды (повторить/
  начать с пробела); кириллица в `Select-String` **не ищется** — читать файл инструментом.
- `config.yaml` — **в `.gitignore`**; правки боевого конфига/индекса — только по решению.
- Запущенный `llm_host.exe`/`hds mcp-http`/`hds watch` держит `target\release\*.exe` →
  сборка падает «отказано в доступе»; на dev-машине их можно остановить
  (см. `cline-rules/dev-machine.md`), кроме чужих процессов (не трогать).
- `HDS_CONFIG`/`HDS_ROOT` персистентны в сессии PowerShell — изоляция тестов через них.

## Правила
- Сначала предложить план по файлам, при согласовании — код. После каждого шага — тесты + коммит.
- Не ломать десктоп-профиль (`profile: desktop` по умолчанию).
- Не трогать чужие незакоммиченные изменения и боевые `PLAN_*.md`-решения; обновлять
  `PLAN_M2_SERVER_PROFILE.md` и `STATUS.md`.
- Обязательно проверять изменения **в рантайме** (особенно auth/пути), а не только юнит-тестами.
