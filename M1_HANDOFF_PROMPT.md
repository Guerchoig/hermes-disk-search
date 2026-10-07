# M1_HANDOFF_PROMPT.md — промт для нового чата (исполнение M1 «Фундамент доступа»)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние,
> точка входа, где лежит детальный план, контракты, правила и грабли.
> Детали задач — `PLAN_M1_ACCESS_PROJECTS.md`; контекст проекта — `PLAN_SERVER_MULTIUSER.md`.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`** (от `w2-llm-host`).
Станция: Windows, RTX 3060 12 ГБ (+ AMD-карта), rustc/cargo 1.97.1. Ядро на **Rust**
(`crates/`), Python — только `sidecar`. Тесты: `cargo test --workspace`.

## Что делаем (M1)
Реализуем **milestone M1 «Фундамент доступа»**: понятие **«Проект»** (разграничение
доступа) + **идентичность** (логин/пароль → авто-выданный токен) + **фильтрация выдачи
MCP по проектам пользователя** + admin-гейтинг. Детальный план — **`PLAN_M1_ACCESS_PROJECTS.md`**
(читать целиком: §3 контракты, §4 треки T1–T6, §6 порядок, §5 приёмка).

## Точка входа (10 минут)
1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -6`
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`   # чисто
3. Прочитать **`PLAN_M1_ACCESS_PROJECTS.md`** (полностью) → затем `PLAN_SERVER_MULTIUSER.md`
   (§2 W2/W5, §5 Э2/Э3, §9).
4. Прочитать `tools/parity/HANDOFF_PROMPT.md` (грабли окружения — §«Грабли»).
5. `cargo build --workspace` и `cargo test --workspace` — убедиться, что база зелёная.

## Ключевые решения (заказчик, 07.10.2026)
- **Проект** — единица доступа; у корня в `index.roots` можно задать имя проекта; по
  умолчанию — имя младшей папки корня; **для корня диска — метка с двоеточием (`D:\`→`D:`)**.
- **Индексацией занимается только админ** (пользователи — никогда).
- **Токены** — авто-выдаваемые; у пользователя **логин+пароль** (выдаёт админ).
- Хранилище доступа — **YAML** (`data/access.yaml`).
- Схема индекса **меняется** — старую БД **дропаем и переиндексируем** (согласовано).

## Контракты (кратко; детали — план §3)
- `data/access.yaml`: `users[{name,login,password_hash,admin?,projects?,disabled?}]` +
  `tokens[{token_sha256,user,expires}]`. Пароли — Argon2id (фолбэк pbkdf2+sha2).
- Токен — opaque 256 бит, хранится хэшем sha256; сверка постоянным временем; TTL 90 дней.
- `POST /auth/login {login,password}` → `{token,expires,user,projects}` | `401`.
- `Authorization: Bearer <token>` проверяется на `hds-mcp` и `hds-ui` (в **server**-режиме).
- Поиск: `search(conn, emb, lem, cfg, query, kinds, projects: Option<&[String]>, limit)`;
  фильтр `f.project IN (…)` в FTS, vec и CLIP; `None` = без фильтра.
- Профиль: `profile: desktop|server` + `access.enabled` (дефолт `desktop`/`false`) —
  десктоп-поведение не ломаем.

## Порядок работы (план §6)
1. `hds-core::access` (T1) → 2. корни→проекты + схема БД (T2) → 3. фильтрация поиска (T3,
   тесты на утечку **до** auth) → 4. MCP auth + login (T4) → 5. UI+CLI (T5) → 6. миграция/доклад (T6).
По одному коммиту на шаг (`feat(access): …`, `feat(index): …`, `feat(search): …`, `feat(mcp): …`, `feat(cli): …`).

## Грабли окружения (кратко; полнее — tools/parity/HANDOFF_PROMPT.md)
- `git` — **всегда** `--no-pager`.
- PowerShell портит первый токен команды (повторить/начать с пробела); кириллица в
  `Select-String` **не ищется** — читать файл инструментом/ASCII-шаблоном.
- `config.yaml` — **в `.gitignore`**; правки боевого конфига/индекса — только по решению заказчика.
- Запущенный `llm_host.exe`/`hds mcp-http` держит `target\release\*.exe` → сборка падает
  «отказано в доступе»; на dev-машине их можно остановить (см. `cline-rules/dev-machine.md`).
- `HDS_CONFIG` персистентна в сессии PowerShell (изоляция тестов — через `HDS_CONFIG`/tempdir).

## Правила
- Сначала предложить план по файлам, при согласовании — код. После каждого шага — тесты + коммит.
- Не ломать десктоп-профиль (по умолчанию `access.enabled=false`).
- Не трогать боевые `PLAN_*.md`-решения; обновлять `PLAN_M1_ACCESS_PROJECTS.md` и `STATUS.md`.
- Свободный рефактор — только в рамках M1 (не расширять на Э1/Э6 и т.д.).
