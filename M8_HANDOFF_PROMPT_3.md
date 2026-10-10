# M8_HANDOFF_PROMPT_3.md — промт для нового чата (этап M8/Э9 «клиентский пакет установки», трек T3)

> Скопируйте текст ниже в новый чат. Он самодостаточен: репозиторий, ветка, состояние, точка
> входа, что сделано, что осталось, контракты, правила и грабли окружения.
>
> **Актуальный промт.** Заменяет по состоянию `M8_HANDOFF_PROMPT_2.md` и `M8_HANDOFF_PROMPT.md`
> (оба закрыты: серверный пакет **Э8** выполнен). Следующий этап по дорожной карте
> `PLAN_SERVER_MULTIUSER.md` §5 — **Э9 «Клиентский пакет Win/macOS, Cline+Hermes» (W7, 3–5 дн)**;
> в текущем объёме остался **трек T3** из `PLAN_M8_INSTALL_PACKAGES.md` §4 (клиент — **Cline без
> Hermes**, решение владельца §7.4). Серверный пакет (Э8) закрыт: **T0/T1/T2/T4 ✅, T5 — приёмка
> (a)+(b) ✅ (живой SCM-прогон), T6 — доклад** (см. `PLAN_M8_INSTALL_PACKAGES.md` §4/§5).
> Детальный план клиентского пакета — **`PLAN_M9_CLIENT_PACKAGE.md`** (Э9/W7, треки T0–T5).

---

Репозиторий: `C:\Users\Sasha\hermes-disk-search`, ветка **`server-multiuser`**.
Состояние на момент передачи: **Э8 закрыт**; коды треков — `fd5af46` (T0), `44e20a8` (T1),
`6db2f69` (T2), `49e80e0` (T4); приёмка/доклад T5–T6 — docs-коммитами поверх. Боевой стенд поднят
(резидент `llm_host` из `target\release`; `hds watch`/`hds transcribe-watch`/`hds mcp --http`
из `target\debug`; `index.pause` — есть). M1–M8/Э8 закрыты (`M7_CODE_REVIEW.md`).

Станция: Windows, RTX 3060 12 ГБ, AMD Radeon iGPU (UMA), Ryzen 7 5700G, 64 ГБ RAM,
rustc/cargo 1.97.1. Ядро на Rust (`crates/`), Python — только sidecar. Сессия Cline — **не админ**.
Доступен **Git Bash** (`C:\Program Files\Git\bin\bash.exe`; WSL нет) — для `bash -n` и прогонов
`.command`. `tools/parity/out/` гитигнорится (артефакты — туда).

## Что осталось: Э9/T3 — тонкий клиентский пакет (Cline без Hermes)

Цель — **тонкий** клиентский пакет Win/macOS: только `hds(.exe)` + мастер «адрес сервера + токен»,
**без** движка/моделей/индекса/sidecar. Мастер поднимает Cline **проводкой** (MCP URL внешнего
сервера + Bearer), проверяет `GET /health` и просит перезапустить Cline.

Треки (по `PLAN_M8_INSTALL_PACKAGES.md` §4, T3):
1. **Расширить `hds cline-sync`** флагами `--server-url <url> --token <bearer> [--mcp-path /mcp]`
   (+ `--dry-run` уже есть). В этом режиме MCP-запись указывает на **внешний** URL с заголовком
   `Authorization: Bearer …`; окна моделей — без локального `llm_server` (или из ответа сервера);
   правило/скилл — как сейчас. Без флагов — прежнее локальное поведение (обратная совместимость).
   Точки кода: `crates/hds-core/src/cline.rs` (`mcp_entry(cfg)`, `sync(cfg, root, dry_run)`),
   `crates/hds-cli/src/cmd/cline_sync.rs` (`cmd_cline_sync(as_json, dry_run)`), `crates/hds-cli/src/main.rs`.
2. **Мастер `setup-client.ps1` / `install_client.command`:** параметры `-ServerUrl https://host/`
   `-Token <bearer>` (или интерактивно); шаги: `GET <ServerUrl>/health` → `hds cline-sync --server-url --token`
   → «перезапустите Cline». Никаких движка/моделей.
3. **Тонкий пакет:** только `hds(.exe)`; никаких движка/моделей/индекса/sidecar.
4. **Юниты:** формирование MCP-записи с внешним URL+Bearer (по образцу `hds_core::cline`;
   проверять отсутствие утечки токена в логи/аудит и корректную форму записи `streamableHttp`).

Контракты — `PLAN_M8_INSTALL_PACKAGES.md` §3 (клиентский мастер; health — существующие
`GET /health` (MCP) и/или `GET /api/status` (UI admin), новых серверных протоколов не вводить;
совместимость: desktop-профиль/установка не меняются).

## Точка входа (5 минут)

1. `git --no-pager -C C:\Users\Sasha\hermes-disk-search log --oneline -12`   # Э8 закрыт
2. `git --no-pager -C C:\Users\Sasha\hermes-disk-search status --short`      # чисто
3. `cargo test --workspace` — ⚠ живые сервисы держат `target\debug\hds.exe`: остановить
   `hds watch`/`hds transcribe-watch`/`hds mcp --http`, после — **вернуть**; либо
   `cargo test --workspace --exclude hds-cli`. Ожидание: **89 наборов, 0 failed**
   (в нагруженном прогоне возможен флейк `hds-mcp quota_socket` — перепроверить изолированно).
4. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`.
5. Прочитать: **`PLAN_M8_INSTALL_PACKAGES.md`** (§2 клиент, §3 контракты, §4 T3, §5 приёмка,
   §7 решения) → `PLAN_SERVER_MULTIUSER.md` §2 W7, §3, §5 Э9, §9 → `STATUS.md`.
6. Карта кода: `crates/hds-core/src/cline.rs`, `crates/hds-cli/src/cmd/cline_sync.rs`,
   `crates/hds-cli/src/main.rs`, `install_cline.ps1`, `installers/install_cline_macos.sh`,
   `installers/cline_mcp_merge.py` (устаревший, только совместимость), `installers/proxy/`.

## Правила и грабли окружения (важно, платил временем)

* **Резидент держит `target\release\*.exe`**, живые службы — `target\debug\hds.exe`: перед
  пересборкой/полным `cargo test` остановить и **вернуть** (`target\debug\hds.exe watch` /
  `… transcribe-watch` / `… mcp --http --host 127.0.0.1 --port 8787 --path /mcp`; резидент —
  `Start-Process target\release\llm_host.exe -ArgumentList run -WindowStyle Hidden`). Dev-машина
  (`cline-rules/dev-machine.md`) — останавливать/возвращать можно.
* **`HDS_CONFIG`/`HDS_ROOT` переживают между элементами `run_commands`** — для песочниц задавать
  и затем **очищать** (`Remove-Item Env:HDS_CONFIG; Remove-Item Env:HDS_ROOT`).
* **Боевой `index.pause` не снимать**; плейн-токены/секреты не печатать.
* **PowerShell 5.1:** `Invoke-WebRequest` без `-UseBasicParsing` зависает; кириллица в консоли
  искажается (читать файлы инструментом); первый вывод команды иногда «клеит» символ — писать
  вывод в файл (`Out-File -Encoding utf8`) и читать; `if/else` не ставить в конвейер напрямую.
* **`bash` — только Git Bash** (`C:\Program Files\Git\bin\bash.exe`); LF для `*.command`
  гарантируется `.gitattributes` (`text eol=lf`) — не ломать. `*.ps1` — **ASCII-only** (PS 5.1
  читает BOM-less .ps1 как ANSI).
* **Проводка Cline** — `crates/hds-core/src/cline.rs`: `cline_dir()` (`CLINE_DATA_DIR` → `~/.cline`),
  `mcp_entry(cfg)`, `sync(cfg, root, dry_run)`; запись MCP — в `cline_mcp_settings.json` и `mcp.json`
  (форма `transport: streamableHttp`). Токен в записи — только как заголовок `Authorization`.
* **Клиентские проверки `GET /health` с клиента** — требуют «машины с Cline/внешним сервером»
  (хвост; на dev-станции — юниты + локальная песочница). TLS-прокси — образцы `installers/proxy/`.
* Артефакты — в `tools/parity/out/` (гитигнор); коммиты — атомарные (`M8/T3: …`, `docs(m8): …`);
  решения §5 дорожной карты **не менять**; чужие незакоммиченные правки не трогать.

## Критерии приёмки (Э9/T3)

Тонкий клиентский пакет одной командой: мастер по «адрес сервера + токен» → `GET /health` ok →
Cline-конфиг с внешним MCP URL и `Authorization: Bearer` (без локальных движка/моделей/индекса);
поиск через внешний сервер работает; плейн-токен **не** в логах/аудите; `hds cline-sync` без флагов
сохраняет прежнее локальное поведение; desktop-профиль/установка не изменены; юниты на формирование
MCP-записи зелёные; `cargo test --workspace` зелёный, fmt/clippy `-D warnings` чисто. Живой цикл на
«машине с Cline/внешним сервером» — хвост. Детали — `PLAN_M8_INSTALL_PACKAGES.md` §3/§5.

