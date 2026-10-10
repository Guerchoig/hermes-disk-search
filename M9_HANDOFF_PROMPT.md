# M9_HANDOFF_PROMPT.md — промт для нового чата (следующий этап: Э10/W8 «Эксплуатация»)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние, точка
> входа, объём, контракты, правила и грабли окружения.
>
> **Этап M9/Э9 «Клиентский пакет установки» ЗАКРЫТ (10.10.2026):** тонкий клиент (Cline без
> Hermes) — `hds cline-sync --server-url/--token`, мастера `setup-client.*`/`install_client.command`,
> тонкая сборка `build_client_release.*`. Детали — `PLAN_M9_CLIENT_PACKAGE.md` (§4 — треки, §5 —
> приёмка). Хвост: живой цикл «клиент + внешний сервер» и HTTPS через TLS-прокси (машина с
> сертификатом).
>
> Следующий этап по дорожной карте `PLAN_SERVER_MULTIUSER.md` §5 — **Э10 «Эксплуатация: healthz,
> бэкап, диагностика, обновление» (W8, 3–5 дн)**. Детального плана для него ещё нет — **задача 1:
> завести план этапа** (`PLAN_M10_OPS.md` по образцу `PLAN_M9_CLIENT_PACKAGE.md`), согласовать
> объём, затем реализовать.

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние: **M1–M9 закрыты** (Э8 серверный пакет и Э9 клиентский пакет — готовы). Боевой стенд
поднят: резидент `llm_host` (`target\release`), `hds watch` / `hds transcribe-watch` / `hds mcp
--http` (`target\debug`), `index.pause` — есть.

Станция: Windows, RTX 3060 12 ГБ, AMD Radeon iGPU (UMA), Ryzen 7 5700G, 64 ГБ RAM, rustc/cargo
1.97.1. Ядро на Rust (`crates/`), Python — только sidecar. Сессия Cline — **не админ**. Доступен
**Git Bash** (`C:\Program Files\Git\bin\bash.exe`; WSL нет). Артефакты — `tools/parity/out/` (гитигнор).

## Объём Э10/W8 (из `PLAN_SERVER_MULTIUSER.md` §2 W8)

- **Логи:** единый формат, ротация (`data/logs` уже есть — см. аудит M3), уровни.
- **Health/metrics:** `GET /health` у MCP/UI уже есть; **агрегированный `/healthz`** — состояние
  `llm-host` + БД + watcher + свободное место на дисках (задел: `/api/diagnostics` UI, `hds check`).
- **Бэкап `index.db`** через SQLite `.backup` (при WAL) и восстановление (задел: `hds db-move`
  использует `VACUUM INTO`).
- **Обновление сервера с малым простоем + откат** (задел: `installers/update.ps1 -Profile server`
  + `-RollbackTo`, окно обслуживания — из M8/Э8).
- **Админ-диагностика:** расширить `hds check` (службы, карты/VRAM, число слотов, очереди, число
  активных пользователей).

## Точка входа (5 минут)

1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -14`   # M9 закрыт
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`      # чисто
3. `cargo test --workspace` — ⚠ живые службы держат `target\debug\hds.exe`: остановить
   `hds watch`/`transcribe-watch`/`mcp --http`, после — **вернуть**; либо `--exclude hds-cli`.
   Ожидание: **89 наборов, 0 failed** (в нагруженном прогоне возможен флейк `hds-mcp quota_socket`).
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
5. Прочитать: **`PLAN_SERVER_MULTIUSER.md`** §2 **W8**, §3, §5 **Э10**, §9 →
   `PLAN_M9_CLIENT_PACKAGE.md` (образец плана этапа) → `STATUS.md`.
6. Карта кода: `crates/hds-index/src/diag.rs` (`hds check`), `crates/hds-mcp/src/http.rs` (`/health`),
   `crates/hds-ui/src/lib.rs` (`/api/diagnostics`, `/api/services`), `crates/hds-core/src/audit.rs`
   (логи/ротация), `crates/hds-cli/src/cmd/{db_move,status,check}.rs`, `installers/update.ps1`.

## Правила и грабли окружения (важно, платил временем)

* **Резидент держит `target\release\*.exe`**, живые службы — `target\debug\hds.exe`: перед
  пересборкой/полным `cargo test` остановить и **вернуть** (`… watch` / `… transcribe-watch` /
  `… mcp --http --host 127.0.0.1 --port 8787 --path /mcp`; резидент —
  `Start-Process target\release\llm_host.exe -ArgumentList run -WindowStyle Hidden`).
  Dev-машина (`cline-rules/dev-machine.md`) — можно.
* **`HDS_CONFIG`/`HDS_ROOT` переживают между элементами `run_commands`** — задавать и **очищать**.
* **Боевой `index.pause` не снимать**; плейн-токены/секреты не печатать.
* **PowerShell 5.1:** `Invoke-WebRequest` без `-UseBasicParsing` зависает; кириллица в консоли
  искажается (читать файлы инструментом); первый вывод команды иногда «клеит» символ — писать в
  файл (`Out-File -Encoding utf8`) и читать; `if/else` не ставить в конвейер напрямую.
* **`bash` — только Git Bash**; LF для `*.command` — `.gitattributes` (`text eol=lf`). `*.ps1` —
  **ASCII-only** (PS 5.1 читает BOM-less .ps1 как ANSI).
* Артефакты — `tools/parity/out/` (гитигнор); коммиты атомарные (`M10/TN: …`, `docs(m10): …`);
  решения §5 дорожной карты **не менять**; чужие незакоммиченные правки не трогать.

## Контракты (черновик, уточнить в плане)

* **`/healthz`** — агрегированный health (llm-host, БД, watcher, диски): единый JSON, `200`/`503`;
  новых секретов не печатать; не ломать существующие `GET /health` (MCP) и `/api/diagnostics` (UI).
* **Бэкап** — `hds backup <path>` (SQLite `.backup` при WAL) + восстановление; не мешать живому
  писателю БД (WAL + `db.read_only_conns`).
* **`hds check`** — расширить пунктами (службы/VRAM/слоты/очереди/пользователи) без смены формата
  существующих пунктов.
* **Обновление** — переиспользовать `update.ps1 -Profile server`/`-RollbackTo`; малое окно простоя.
* Совместимость: desktop-профиль не изменяется.

## Критерии приёмки (черновик)

`/healthz` отражает реальное состояние (юниты + живой стенд); бэкап/восстановление проверены на
песочнице; `hds check` показывает новые пункты; обновление сервера с откатом без «отказано в
доступе»; `cargo test --workspace` зелёный, fmt/clippy `-D warnings` чисто; desktop не изменён.
