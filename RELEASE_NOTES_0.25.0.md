# hermes-disk-search 0.25.0 — агент в анонимной зоне (фаза A3)

**Дата:** 19.11.2026 · **База:** 0.24.0 · **Ветка:** `agent-privacy-zone`

**Фаза A3** добавляет к программе ИИ-агента, который работает не с оригиналами документов, а
с их **обезличенными копиями**. Наружу уходят метки (`[PERSON_1]`), а не значения; реальный
профиль `%USERPROFILE%\.cline`, его плагины и оболочка агенту недоступны. Агент — функция
рабочего стола: серверный установщик ставит его только по флагу `-WithAgent`.

## Что нового

### Движок и воркер (TR-3.1…TR-3.10)

1. **Node-воркер `sidecar/hds_agent`** (TypeScript, `@cline/sdk` 0.2.0): собирается
   `esbuild` в `dist/worker.mjs` (ESM, `--packages=external`). Протокол §4.3 — JSON-RPC
   поверх stdio: `hello`/`ready`, `start`, `send`, `abort`, `stop`, `shutdown`; события
   `session`/`prompt`/`redacted`/`agent`/`turn_end`; `host_call` с префиксом `h` для
   `egress_check`, `office_*` и `ask_user`. stdout — только протокол, диагностика — в stderr.
2. **Политики агента** (TR-3.4): `run_commands` и `fetch_web_content` не выдаются; файловые
   инструменты ограничены `workspace`; `editor` по `docx`/`xlsx` отклоняется; `office_*`
   работают через `host_call`. Правила пишутся в `<P>\agent-config` и перечитываются каждым
   ходом.
3. **Хук `hds-guard`** (TR-3.4): матрица путей TR-3.10 — путь внутри `workspace` разрешён,
   выход за него и `..` отклоняются; хук не падает на несуществующем пути.
4. **`office_*`** (TR-3.5): чтение/правка DOCX, XLSX, PPTX, MPP через Python-воркер
   извлечения; результат — текст и путь к копии, оригинал не меняется.
5. **MCP агента** (TR-3.6): встроенные `disk-search` (маршрут `/mcp/agent/<P>`, токен
   проекта), `tavily` и `1c-buddy`; `ask_my_files` вырезан, в т.ч. с префиксом сервера.
   **Сверх встроенных** подключаются внешние серверы из общего блока `mcp.servers` в
   `data\agent\secrets.json` (stdio или http); конфликт имени решается в пользу встроенного.
   Готовый шаблон — `sidecar\mcp\mcp.example.json` и `sidecar\mcp\README.md`.
6. **Суб-агенты** (TR-3.7): `agent.subagents` (по умолчанию выключено); запросы суб-агента
   идут через тот же `egressFetch` и хук.
7. **Жизненный цикл** (TR-3.9): `AgentRegistry` с выгрузкой по простою
   (`max_live_projects=2`, `idle_unload_min=30`); `Gateway::session_id`, `RunOptions.session`,
   возобновление истории через `initialMessages`, `history clear` с бэкапами `.json.bak`.
8. **Тесты изоляции** (TR-3.10): mock-провайдер, MCP-заглушка, проверка «0 запросов при
   fail-closed», «секрет не ушёл», «профиль пользователя не изменён».

### Шлюз реплик и фильтр (TR-5.x, TR-6.x)

9. **Шлюз реплик** (TR-5.1…TR-5.3): `Gateway` запускает воркер, ведёт JSON-RPC, деанонимизирует
   события (`TurnReport`), отдаёт `TurnEnd` с `raw` (метки) и `text` (значения).
10. **Фильтр PII** (TR-6.1…TR-6.7): `Filter` на входе и выходе; снимок vault со сроком
    годности `agent.filter.max_stale_sec` (по умолчанию 300 с, fail-closed); неизвестные
    сущности — метка (`label`) или блокировка (`block`); ключ провайдера в тексте блокирует
    запрос (`privacy_filter_unavailable`); срабатывания пишутся в журнал `audit.enabled`
    (на desktop по умолчанию выключен).

### Установка и поставка

11. **`installers\build_agent_sidecar.ps1`** (A3.5): портативный Node из
    `runtime-manifests\node-manifest.json` (v24.18.0, sha256) → `sidecar\node`; `npm ci` по
    lock; сборка `dist\worker.mjs`; `npm prune --omit=dev` в дереве поставки; `-SelfTest`
    проверяет `hello` → `ready`, импорт `@cline/sdk` и наличие `tavily-mcp`; `-NodeSource`
    для офлайна; идемпотентность по маркерам.
12. **Релизная сборка**: `build_rust_release.ps1 -AgentSidecar none|portable` (по умолчанию
    `portable` для Windows-архива) собирает агентский воркер в `sidecar\`; на macOS шаг
    пропускается с явным сообщением в логе.
13. **Серверный установщик**: `install_server.ps1 -WithAgent` собирает агентский воркер и
    пишет `[ok] agent sidecar: …` в сводку; без флага — `[i] агент не ставится
    (desktop-функция, -WithAgent)`.

## Что качать

| Что | Файл | Зачем |
|---|---|---|
| Основная программа | `hds-0.25.0-windows-x64.zip` | Rust-ядро, страница управления, Python-воркеры, Node-воркер агента (`sidecar\hds_agent`), портативный Node, шаблон внешних MCP (`sidecar\mcp`) |
| Установщик | `setup.cmd` (внутри архива) | ставит зависимости и службы; агентский воркер уже в архиве |
| Заметки о выпуске | `RELEASE_NOTES_0.25.0.md` | этот файл |

Агентский воркер **не нужно** собирать отдельно на Windows: он лежит в архиве. Если архив
собран с `-AgentSidecar none`, воркер добавляется на месте:
`installers\build_agent_sidecar.ps1 -OutDir <папка установки>\sidecar -SelfTest`.

## Что и чем проверено

| Проверка | Файл / команда | Результат на срезе |
|---|---|---|
| Rust ↔ настоящий воркер | `crates/hds-agent/tests/gateway_worker.rs` | 56 тестов `cargo test -p hds-agent`; `egress_check` через настоящий `Filter`; fail-closed — 0 запросов к mock |
| Точка входа CLI | `crates/hds-cli/tests/agent_run.rs` | `project add/list`, `run --dry-run`, `run` на mock-провайдере, `--quiet`, `--json`, ключ не попадает в файлы агента |
| Жизненный цикл | `crates/hds-agent/src/registry.rs`, `lifecycle_worker.rs` | 68 тестов; переиспользование PID, выгрузка по простою, `history clear` с бэкапами |
| Установщик воркера | `installers/build_agent_sidecar.ps1 -SelfTest` | 4 прогона: репозиторий, дерево поставки, офлайн `-NodeSource`, `hds agent run` на установленном дереве |
| Приёмка e2e | `crates/hds-cli/tests/agent_acceptance.rs` | `office_read`, `disk-search` по маршруту A2, заглушка `tavily`, запись файла; I2/I7; профиль `.cline` не изменён |
| Внешние MCP | `crates/hds-cli/tests/agent_run.rs::external_mcp_from_delivery_template` | шаблон из поставки виден в `mcp.servers` плана; значения не уходят |
| Node-воркер | `sidecar\hds_agent`: `npm test` | 23/23 |
| Python-воркеры | `python -m unittest discover -s sidecar/tests -t sidecar` | 307+ тестов |
| Форматирование и линт | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` | чисто |

## Как включить агента

1. **Desktop-профиль.** Распакуйте архив и запустите `setup.cmd` — воркер и портативный Node
   уже на месте. В `config.yaml` задайте `agent.enabled: true` и `agent.root` (например
   `D:\hds-anon`), затем `hds agent init`.
2. **Серверный профиль.** Установите программу с флагом `-WithAgent`:
   `installers\install_server.ps1 ... -WithAgent`. Без флага агент не ставится (это
   desktop-функция); добавить позже можно тем же
   `installers\build_agent_sidecar.ps1 -OutDir <папка>\sidecar -SelfTest`.
3. **Ключ провайдера** — только в `data\agent\secrets.json` (в `config.yaml` и переменных
   окружения его быть не должно).
4. **Свои MCP-серверы** (например `1c-buddy`) — скопируйте блок `mcp` из
   `sidecar\mcp\mcp.example.json` в `data\agent\secrets.json`, подставьте путь к `.env`;
   токен в шаблон не входит. Проверка — `hds agent run --dry-run` (показывает имена
   серверов), затем обычный `hds agent run`.

## Ограничения и открытые вопросы

* **`ask_user` пока не поддерживается**: агент возвращает отказ до появления вкладки агента в
  UI (этап A5).
* **Реестр процессов к CLI не подключён**: `hds agent run` работает с одним проектом на
  процесс; `max_live_projects` и `idle_unload_min` заработают в службе/вкладке A5.
* **`hds check` не проверяет Node и `dist\worker.mjs`** — это задача этапа A6. До тех пор
  целостность воркера проверяет `build_agent_sidecar.ps1 -SelfTest`.
* **Аудит срабатываний фильтра выключен** (`audit.enabled: false`): включать явно, если нужен
  журнал меток.
* **Внешний MCP-сервер проверяется только настройками**: поставка несёт шаблон `1c-buddy`
  без токена и без бинаря; сам сервер и его `.env` администратор берёт у поставщика.
* **Открытые вопросы:** формат `mcp.servers` может потребовать валидации схемы и редактора в
  UI (этап A5); ACL каталога настроек MCP и `secrets.json` — этап A6 (В-1).


## Правки по итогам код-ревью A0–A3 (19.11.2026)

Ревью — `AGENT_A0_A3_CODE_REVIEW.md`; все находки закрыты. Что изменилось для пользователя:

* **Метки с падежом по TR-2.3.** Фильтр больше не выдаёт `[PERSON_1|nomn]`: именительный —
  `[PERSON_1]`, косвенные — `[PERSON_1|дат]` и т.д. Деанонимизация восстанавливает падеж
  (раньше подставлялся именительный), а метки, выпущенные прежней версией (`|nomn`), продолжают
  распознаваться.
* **Снимок меток обновляется по ходу сеанса.** Метка, созданная репликой (новое ФИО/организация),
  сразу деанонимизируется в ответе; долгий ход больше не упирается в `max_stale_sec`.
* **`hds agent token` сохраняет и хэш, и сам токен** — `disk-search` подключается к агенту без
  ручной правки `secrets.json`. Если plain-токена нет (старый файл), `hds agent run`
  предупреждает, что поиск отключён.
* **`mcp_in` фильтрует весь результат MCP** (включая `structuredContent`), а не только `output`.
* **Секреты в словаре — только секретные поля**: пути и аргументы MCP-команд больше не вызывают
  ложную блокировку; секрет внутри длинного токена, наоборот, блокируется.
* **Таймаут MCP** пишется ключом `timeout` (его читает SDK): `1c-buddy` — 120 с,
  `disk-search`/`tavily` — 60 с.
* **Приёмка стабильна на машине с запущенным Cline** (не сравнивает живые `sessions.db`), плюс
  проверяет, что в профиле владельца нет пути зоны агента.
* Мелочи: отставание индекса кэшируется на 30 с (поиск быстрее), `unknown` в событиях содержит
  только неразрешённые метки, выгрузка проекта ждёт не 5, а 2 с, `session_id` валидируется,
  `agent init` проверяет junction, `env::build` без мёртвого параметра, `config.example.yaml`
  описывает все ключи `agent:`.

