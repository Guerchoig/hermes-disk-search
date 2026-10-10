# PLAN_M9_CLIENT_PACKAGE.md — детальный план M9 «Клиентский пакет установки (Э9/W7)»

> Продолжает `PLAN_M8_INSTALL_PACKAGES.md`: этап **Э8 (серверный пакет)** закрыт 10.10.2026,
> а отложенный трек **T3 — клиентский пакет** вынесен сюда. Дополняет `PLAN_SERVER_MULTIUSER.md`
> §2 **W7**, §3.2 (состав клиентского пакета), §5 **Э9**, §9.
> **Статус:** запланирован 10.10.2026 (по итогам Э8). Ветка — **`server-multiuser`**.
> **Объём согласован владельцем (10.10.2026, `PLAN_M8_INSTALL_PACKAGES.md` §7):** клиент —
> **Cline, без Hermes** (п. 4); формат поставки — **zip + скрипты** (п. 1). Промт для старта —
> `M8_HANDOFF_PROMPT_3.md`.
>
> **Зависимости:** M1 (доступ/токены), M2 (профиль `server`, `GET /health` MCP), M3 (TLS-прокси,
> `trusted_proxy`), M7 (выдача токенов `hds token new`), Э8 (`install_server.*`, `update.ps1`).
> «Кирпичи» клиентской проводки уже есть: `hds cline-sync`, `crates/hds-core/src/cline.rs`,
> `install_cline.ps1`, `installers/install_cline_macos.sh`, `installers/cline_mcp_merge.py`.

## 1. Вводные (факты из кода, а не предположения)

| Факт | Где |
|---|---|
| **Проводка Cline уже есть, но локальная:** `hds cline-sync` (тот же код, что кнопка UI `POST /api/cline/sync`) пишет окна контекста в `models.json`, MCP-сервер `disk-search` в **оба** файла настроек, правило и скилл — **из локального `config.yaml`** | `crates/hds-core/src/cline.rs` (`sync`, `mcp_entry`, `mcp_url`), `crates/hds-cli/src/cmd/cline_sync.rs` |
| **Запись MCP — плоская (`type: streamableHttp`), без заголовков:** `{type, url, disabled, autoApprove, timeout}` — поля `headers` нет, т.е. **Bearer к внешнему серверу сейчас не выразить** | `cline.rs::mcp_entry` |
| **CLI `cline-sync` знает только `--json`/`--dry-run`:** нет `--server-url`/`--token` | `crates/hds-cli/src/main.rs` (~137) |
| **Каталог Cline:** `~/.cline` (`CLINE_DATA_DIR` переопределяет, корень = родитель `…/data`); MCP-файлы — `~/.cline/data/settings/cline_mcp_settings.json` и `~/.cline/mcp.json` | `cline.rs::cline_dir`, `sync` |
| **Клиентские установщики существуют, но «локальные»:** зовут `hds mcp-http restart` + `hds cline-sync`; сценария «внешний сервер + Bearer» нет | `install_cline.ps1`, `installers/install_cline_macos.sh` |
| **Cline валидирует файл настроек целиком:** одна неверная запись → теряются ВСЕ MCP-серверы (проверка — `cline config mcp --json`) | `install_cline.ps1` (комментарий + проверка), `RELEASE_NOTES_v0.11.0.md` |
| **Сборка релиза — только полный серверный пакет:** `bin\{hds,hds_mcp,llm_host}.exe`, `installers/`, `sidecar/`, `runtime-manifests/`, `hermes-skill/`, `cline-rules/`, `assets/`, docs. **Тонкого клиентского варианта нет** | `installers/build_rust_release.ps1` |
| **Health есть:** `GET /health` у MCP-сервера (отдаёт `app`, `client_ip`) — годится для «проверить соединение» | `crates/hds-mcp/src/http.rs` |
| **Токен при выдаче — plain один раз**, в файл — только `token_sha256`; в аудит — без токена | `crates/hds-cli/src/cmd/auth.rs::cmd_token` |

## 2. Область

**Входит (Э9/W7, клиент):**
- расширение `hds cline-sync` режимом «внешний сервер»: URL сервера + Bearer-токен → MCP-запись
  с заголовком `Authorization`; правило/скилл — как сейчас; без флагов — прежнее локальное поведение;
- мастер Win/macOS «адрес сервера + токен»: `GET /health` → `hds cline-sync --server-url --token`
  → «перезапустите Cline»; идемпотентно, без движка/моделей/индекса/sidecar;
- тонкая сборка/пакет: только `hds(.exe)` + правило/скилл + скрипты мастера + sha256;
- юниты на форму MCP-записи (внешний URL + Bearer), отсутствие утечки токена в логи/аудит.

**Не входит:** Hermes (решение владельца §7.4 — только Cline); изменения серверных протоколов
(используем существующий `GET /health`); изменения desktop/server-профилей; анонимизация (W9/Э6);
эксплуатация healthz/бэкап (Э10); нагрузочная приёмка (Э11).

## 3. Контракты (код — источник истины)

* **CLI `hds cline-sync` (расширение):** флаги `--server-url <url>` и `--token <bearer>` (опц.
  `--mcp-path /mcp`), поверх существующих `--dry-run`/`--json`. **Без `--server-url`** — прежнее
  локальное поведение (обратная совместимость: `config.yaml` → `mcp_url(cfg)`).
* **MCP-запись (клиентский режим):**
  `{ "type": "streamableHttp", "url": "<server-url><mcp-path>", "headers": {"Authorization": "Bearer <token>"},
  "disabled": false, "autoApprove": <TOOLS>, "timeout": <MCP_TIMEOUT_SECS> }` — в оба файла Cline
  (`~/.cline/data/settings/cline_mcp_settings.json`, `~/.cline/mcp.json`), слиянием
  (`merge_mcp_entry`, чужие серверы сохраняются). T0 обязан подтвердить, что Cline принимает
  `headers` у `streamableHttp` (иначе — способ по §7).
* **Окна моделей:** в клиентском режиме локального `llm_server` нет — шаг `models.json` либо
  пропускается, либо ставится из ответа сервера. Решение — §7 (по умолчанию: не трогать).
* **Правило/скилл:** из клиентского пакета (`cline-rules/disk-search.md` → `~/.cline/rules/`,
  `hermes-skill/disk-search.md` → `~/.cline/skills/disk-search/SKILL.md`); код — `cline::sync`.
* **Health:** `GET <server-url>/health` → `200` (поле `app`); новых серверных эндпоинтов не вводить.
* **Токен:** в Cline-конфиге — plain (как хранит сам Cline); не печатать в stdout/логи/аудит;
  `--dry-run`/`--json` не содержат токена.
* **Мастер:** `setup-client.ps1` + `setup-client.cmd` (Windows) и `install_client.command` (macOS) —
  параметры `-ServerUrl`/`--server-url`, `-Token`/`--token`, `-McpPath`/`--mcp-path`; шаги:
  health → `hds cline-sync --server-url --token` → «перезапустите Cline»; без прав админа/root.
* **Тонкий пакет:** только `bin\hds.exe` (`bin/hds`) + `cline-rules/` + `hermes-skill/` + скрипты
  мастера + `README`/`sha256.txt`. Без `hds_mcp`/`llm_host`, `sidecar/`, `runtime-manifests/`,
  моделей, индекса, `install_server.*`.
* **Совместимость:** desktop- и server-профили не изменяются; клиентский режим — аддитивно.

## 4. Треки T0–T5

### T0. Расширение `hds cline-sync` (внешний сервер + Bearer) — 1–1,5 дн
- [ ] `cline.rs`: режим-параметр (например `sync(cfg, root, dry_run, client: Option<ClientTarget>)`,
      `ClientTarget { server_url, token, mcp_path }`); `mcp_entry` — вариант с внешним URL и
      `headers`. Юниты: форма записи, слияние без потери чужих серверов, отсутствие токена в
      `Report`/`--json`.
- [ ] `cmd/cline_sync.rs` + `main.rs`: разбор `--server-url/--token/--mcp-path`; валидация
      (токен без URL — ошибка; предупреждение при `http://` вне localhost).
- [ ] Проверка совместимости с Cline: `headers` у `streamableHttp` принимается
      (`cline config mcp --json` не отбрасывает файл). Если нет — способ по §7.

### T1. Мастер Windows (`setup-client.ps1` + `setup-client.cmd`) — 0,5–1 дн
- [ ] Параметры `-ServerUrl -Token [-McpPath] [-DryRun]`; шаги: `GET /health` → `hds cline-sync
      --server-url --token` → «перезапустите Cline». Идемпотентно; без админа. ASCII-only (PS 5.1).

### T2. Мастер macOS (`install_client.command`) — 0,5 дн
- [ ] Паритет флагов (`--server-url/--token/--mcp-path`), `bash -n`, ad-hoc `codesign` для `hds`.

### T3. Тонкая сборка/пакет — 0,5–1 дн
- [ ] `installers/build_client_release.ps1` (+ `_macos.sh`): стейджит **только** `bin\hds(.exe)`,
      `cline-rules/`, `hermes-skill/`, скрипты мастера, `README`, `sha256.txt`; zip + `.sha256.txt`.
      Проверка: в архиве **нет** `hds_mcp`/`llm_host`/`sidecar`/`runtime-manifests`.

### T4. Приёмка — 1–1,5 дн
- [ ] Локальная песочница: `hds cline-sync --server-url http://127.0.0.1:<port>/ --token <t>`
      (фиктивный `CLINE_DATA_DIR`) → MCP-запись с URL+Bearer; `--dry-run` не пишет и не печатает
      токен; `--json` без токена; `GET /health` сервера — 200.
- [ ] Идемпотентность: повторный запуск не дублирует и не ломает чужие серверы в `mcpServers`.
- [ ] **Хвост (внешний стенд):** живой цикл «машина с Cline + внешний сервер» — `GET /health` с
      клиента, `cline config mcp --json` видит `disk-search`, поиск работает; **HTTPS через
      TLS-прокси** (образцы `installers/proxy/`, `trusted_proxy: true`) — на машине с сертификатом.
- [ ] Артефакты — `tools/parity/out/m9_*` (гитигнор); плейн-токен не печатать.

### T5. Доклад — 0,5 дн
- [ ] `README.md` (раздел «Клиентский пакет (M9)»), `README_SERVER_MULTIUSER.md` §8e,
      `STATUS.md` (блок M9), `PLAN_SERVER_MULTIUSER.md` §9 — статус M9; промт следующего этапа.

## 5. Приёмка

- [ ] Тонкий клиентский пакет одной командой: мастер по «адрес сервера + токен» → `GET /health` ok
      → Cline-конфиг с внешним MCP URL и `Authorization: Bearer`; без движка/моделей/индекса/sidecar.
- [ ] `hds cline-sync` без флагов сохраняет прежнее локальное поведение (регресс).
- [ ] Плейн-токен **не** в логах/аудите/`--json`; форма записи принимается Cline.
- [ ] Правило и скилл на месте; desktop/server-профили не изменены.
- [ ] Юниты зелёные; `cargo test --workspace` зелёный, fmt/clippy `-D warnings` чисто.
- [ ] (хвост) Живой цикл «внешний сервер + Cline» и HTTPS-прокси — на отдельной машине.

## 6. Риски

| Риск | Митигация |
|---|---|
| Cline отбрасывает файл настроек из-за одной записи | `merge_mcp_entry` + `cline config mcp --json`; юниты на форму |
| Плейн-токен в конфиге/логах | В конфиг — только как заголовок; не логировать; маскировать в отчёте |
| `headers` у `streamableHttp` не поддерживается | T0 подтверждает; альтернатива — §7 |
| Рассинхрон версии клиента и сервера (правило/скилл/схема) | Клиентский пакет несёт актуальные правило/скилл |
| Нет живой машины с внешним сервером | Приёмка юнитами + локальной песочницей; живой цикл — хвост |
| Токен не пробрасывается прокси | Проверить в образцах `installers/proxy/` проброс `Authorization` |

## 7. Решения владельца (открытые вопросы)

1. **Окна моделей у клиента:** не трогать `models.json` (чат-модель внешняя) — предлагается по
   умолчанию; либо ставить из ответа сервера (если он отдаёт окна).
2. **Способ передачи токена:** заголовок `Authorization: Bearer` в `headers` записи (предлагается)
   либо иной способ, если Cline не принимает `headers`.
3. **Формат `--server-url`:** `https://host/` + `--mcp-path /mcp` (предлагается) либо полный URL.
4. **Имена скриптов:** `setup-client.ps1`/`install_client.command` (предлагается).
5. **Hermes:** не включать (уже решено в §7.4 плана M8).

## 8. Ссылки
- `PLAN_M8_INSTALL_PACKAGES.md` — §2 (клиент), §3 (контракты), §4 T3, §7 (решения); Э8 закрыт.
- `PLAN_SERVER_MULTIUSER.md` — §2 **W7**, §3.2, §5 **Э9**, §9.
- Код: `crates/hds-core/src/cline.rs`, `crates/hds-cli/src/cmd/cline_sync.rs`,
  `crates/hds-cli/src/main.rs`, `install_cline.ps1`, `installers/install_cline_macos.sh`,
  `installers/build_rust_release.ps1`, `installers/proxy/`.
- Промт — `M8_HANDOFF_PROMPT_3.md`. Артефакты — `tools/parity/out/m9_*` (гитигнор).
