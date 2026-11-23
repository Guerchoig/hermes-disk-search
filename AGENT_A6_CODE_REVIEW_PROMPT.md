# AGENT_A6_CODE_REVIEW_PROMPT.md — код-ревью фазы A6 и выпуска 0.27.0

> **Дата среза:** 22.11.2026. **Ветка:** `agent-privacy-zone`. **HEAD:** `6f0070b7`
> («docs: релиз 0.27.0 выпущен»), **тег:** `v0.27.0` (`b8967937`), ветка синхронизирована
> с `origin`. **Релиз выпущен:** https://github.com/Guerchoig/hermes-disk-search/releases/tag/v0.27.0
> (Windows-only, macOS не поставляется).
>
> **Что это.** Контекст и готовый промт для нового чата: независимое код-ревью фазы A6
> (`A6.1`–`A6.8`) и изменений выпуска 0.27.0. Ревью — **только чтение и проверки**;
> правки кода — после отчёта и решения владельца.

## 0. Задача ревью

Проверить, что A6 действительно даёт то, что обещано в отчётах, и что выпуск 0.27.0
не принёс регрессий:

1. **A6.1–A6.6** — код эксплуатации зоны агента (диагностика, автозапуск, горячий
   конфиг, restore/rebuild/relink, `project rm`, AG-23/25/26/32).
2. **A6.7** — приёмочный сценарий и документация (сам сценарий — в каталоге стенда,
   в git его нет).
3. **A6.8 (K1)** — методика замера RAG, скрипт `tools/parity/agent_rag_eval.py`,
   решение `agent.mcp.disk_search.enabled: false` и его последствия.
4. **Выпуск 0.27.0** — Windows-only workflows, удаление mac-джоб/ассетов, зеркало
   рантайма движка, правки CI/тестов, версия и заметки.

**Не задачи ревью:** пересматривать решения плана (`PLAN_AGENT_PRIVACY_ZONE.md` §3,
§6; `AGENT_A6_STAGES.md` §3), чинить AG-34 и известный флейк, менять продуктовое
поведение «по вкусу», переписывать отчёты.

## 1. Состояние среза (проверено 22.11.2026)

| Что | Факт |
|---|---|
| Релиз | `v0.27.0` опубликован, не draft/prerelease; ассеты Windows-only |
| Ассеты | `hds-0.27.0-windows-x64.zip` + sha, клиент + sha, `client-install.ps1`, CUDA/Vulkan рантаймы, исходники Windows |
| macOS | джоб и ассетов нет; `installers/*_macos*`, `*.command` — legacy в исходниках |
| Рантайм движка | upstream v1.15 отдаёт 404; манифест указывает на зеркало `engine-runtime-v1.15`, sha256 закреплены |
| RAG по умолчанию | `agent.mcp.disk_search.enabled: false` (решение владельца по K1) |
| CI тега | Release run `45055365847` — success; Rust CI — success (ubuntu + Windows) |
| Тесты A6.8 | `hds-agent` 181 lib + наборы; `hds-cli` наборы 0 failed (на Windows; см. §10 про профиль Cline); `hds-ui` 42 lib, `agent_api` 11, `agent_chat` 9; python **335 OK** (skip 1); npm **23/23**; fmt/clippy — 0 |
| K1 | 20 вопросов × 2 прогона × 2 конфигурации, `~deepseek/deepseek-flash-latest`: полнота 1,775 против 0,325; semantic 2,0 против 0,333; токены 130,9 тыс. против 91,1 тыс.; сбои 4 против 23 |

Артефакты стенда (гитигнорятся): `tools/parity/out/agent_a42/` — `A6_E2E.md`,
`a6_e2e.ps1`, `k1-*`, `golden-k1.jsonl`, `k1-search-eval.json`, `k1-summary.json`,
`k1-analysis.json`.

## 2. Объём ревью

**Диапазон:** `75912a0e..HEAD` (закрытие A5.7 → выпуск 0.27.0), 61 файл,
~11,4 тыс. строк. Смотреть в таком порядке:

| Группа | Коммиты | Что |
|---|---|---|
| A6.1 | `018056d1` | `hds-index/src/diag.rs`, `hds-cli/src/cmd/check.rs`, CLI usage |
| A6.2 | `15b70e84` | `hds-cli/src/cmd/agent_sync.rs`, `installers/install_agent_sync_task.ps1` |
| A6.3 | `ac64a682` | горячий `LiveConfig` в `agent_sync.rs` |
| A6.4 | `6dc86b2f` | `hds-agent/src/results.rs`, `registry.rs`, `hds-cli/src/cmd/agent_result.rs`, UI-маршруты |
| A6.5 | `4c19a999` | `hds-agent/src/remove.rs`, `agent.rs` (project rm), UI |
| A6.6 | `beff5521` | AG-23/25/26/32: `results.rs`, `hds-ui/src/agent.rs`, `page.rs` |
| A6.7 | `920a054d` | сценарий/протокол (стенд), README §21, заметки |
| A6.8 | `cebe7884` | `settings.rs`, тесты default/enable, `agent_rag_eval.py`, отчёт, README/RELEASING/заметки |
| Выпуск | `e9fb550b..b8967937` | Windows-only workflows, зеркало рантайма, CI/тест-правки, версия |

**Не входит:** A1–A5 как продуктовая функциональность (ревью было отдельно:
`AGENT_A4_A5_CODE_REVIEW.md`, `AGENT_A0_A3_CODE_REVIEW.md`); здесь — только их
регрессии, вызванные A6/выпуском.

## 3. Обязательное чтение (по порядку)

1. `PLAN_AGENT_PRIVACY_ZONE.md` §3 (D19, D23), §5.8–5.9, §6 (K1 и правило решения), §7 (приёмка).
2. `AGENT_A6_STAGES.md` §2 (A6.1–A6.8), §3 (решения В-A6-1…10), §4 (контракты), §5 (долги), §6 (грабли).
3. `AGENT_A6_REPORT.md` — что заявлено и чем проверено.
4. `tools/parity/out/agent_a42/A6_E2E.md` — протокол 75/75 и находки (AG-27/AG-34).
5. `tools/parity/AGENT_RAG_K1.md` — методика, числа, ограничения, решение.
6. `RELEASE_NOTES_0.27.0.md`, `RELEASING.md`, `README.md` §20/§21, `STATUS.md`.
7. `.github/workflows/release.yml`, `.github/workflows/rust.yml`,
   `runtime-manifests/engine-manifest.json`, `engine-manifest-sources.json`.
8. Прошлые ревью: `AGENT_A4_A5_CODE_REVIEW.md` (AG-20…AG-33 — проверить, что закрыты
   и не вернулись), `AGENT_A0_A3_CODE_REVIEW.md` (AG-1…AG-19).

## 4. Карта кода (что читать в первую очередь)

### 4.1 Диагностика и CLI

| Файл | Что смотреть |
|---|---|
| `crates/hds-index/src/diag.rs` | `check_agent`, `check_agent_for`, `agent_*_check` (12 пунктов), маршрут `/mcp/agent/<P>` (health/401/200), формат `Check`, отсутствие значений секретов |
| `crates/hds-cli/src/cmd/check.rs` | печать `agent-*`, коды 0/1, `--json` |
| `crates/hds-cli/src/cmd/agent.rs` | usage, `agent check`, `project add/list/rm`, коды возврата |
| `crates/hds-cli/src/cmd/agent_run.rs` | `--dry-run`, `--json`, предупреждение `disk_search_enabled && token.is_none()`, `max-iterations` |

### 4.2 Демон зеркала

| Файл | Что смотреть |
|---|---|
| `crates/hds-cli/src/cmd/agent_sync.rs` | `--all`, один процесс на проекты, `sync.lock` на проект, `LiveConfig` (mtime+размер), `config reload`, `root ignored`, `stop reason=…`, `index.stop` (AG-27/AG-34), интервалы и `wait_stable` |
| `installers/install_agent_sync_task.ps1` | задача Планировщика/ярлык «Автозагрузка», `-TaskName/-Start/-Status/-Remove`, идемпотентность, кавычки аргументов |

### 4.3 Решения и реестр

| Файл | Что смотреть |
|---|---|
| `crates/hds-agent/src/results.rs` | `accept/reject/conflict`, `backup`/`pending`/`base`, штампы, атомарность AG-23, занятый файл, `decisions.jsonl` + ротация, `--json`-отчёт |
| `crates/hds-agent/src/registry.rs` | статусы и переходы, `set_baseline`/`upsert`, кэш в UI и его сброс |
| `crates/hds-agent/src/validator.rs` | 4 проверки варианта (`labels_outside_project`, `label_grammar`, `unplaced_images`, `text_lost`), `verdict` не блокирует |
| `crates/hds-cli/src/cmd/agent_result.rs` | CLI restore/rebuild/relink, коды 0/1, `--stamp`, понятные ошибки |
| `crates/hds-agent/src/remove.rs` | `project rm`: проверка `P` + 4 цифры, guard зоны, живой lock, порядок удаления, `secrets` merge, папка проекта не трогается, повтор → код 2 |

### 4.4 UI и маршруты

| Файл | Что смотреть |
|---|---|
| `crates/hds-ui/src/agent.rs` | `AgentApi`, `agent::gate` (403 server), CSRF, `state_json`, `history.uncleaned`, настройки `disk_search_enabled`, кэш реестра, SSE |
| `crates/hds-ui/src/agent_session.rs` | процесс агента, выгрузка/перезапуск, очередь реплик, abort |
| `crates/hds-ui/src/page.rs` | вкладка «Агент», «Результаты», «Настройки агента», переключатель «MCP disk-search» |
| `crates/hds-ui/src/lib.rs` | маршруты `/api/agent/*`, `is_user_route` (агентские туда не входят) |
| `crates/hds-mcp/src/agent.rs` | `/mcp/agent/<P>`: `project_ctx`, `token_ok`, отсутствие `ask_my_files`/управляющих инструментов, фильтр |
| `sidecar/hds_anonymize/handlers.py` | режим `mode:"agent"`, метки/суффиксы, `office_*`, если A6 их касался |

### 4.5 Замер и выпуск

| Файл | Что смотреть |
|---|---|
| `crates/hds-agent/src/settings.rs` | `disk_search_enabled` default `false`, `bool_at`, тесты дефолта/включения |
| `crates/hds-agent/src/run.rs` | `if !agent.disk_search_enabled { mcp.token = None }`, сборка плана, `open_client` |
| `tools/parity/agent_rag_eval.py` | прогоны A/B, usage/шаги/время, scoring, транзиентные повторы, `--score-only`, отсутствие утечек значений |
| `.github/workflows/release.yml` | Windows-only джобы, mandatory-ассеты, клиент, source archive, публикация |
| `.github/workflows/rust.yml` | ubuntu: fmt/clippy/`cargo check`; Windows: `cargo test`; ветка `agent-privacy-zone` |
| `runtime-manifests/engine-manifest*.json` | зеркало рантайма, sha256, отсутствие mac-ассета |
| `RELEASING.md` | правила соответствуют workflow и фактическому выпуску |

## 5. Чеклист по под-этапам

### A6.1 — диагностика

- [ ] 12 пунктов `agent-*` покрывают заявленное; при `agent.enabled: false` — одна строка.
- [ ] В пунктах нет значений ключей/токенов/паролей; `agent-provider` — только «задан/нет».
- [ ] `agent-route` не падает, если порт не слушает; 401 без токена и 200 с токеном проверяются без печати токена.
- [ ] `agent-index` считает проекты/файлы по зоне, а не по обычным корням.
- [ ] `hds agent check --json` — одна строка, стабильный набор полей; коды 0/1.
- [ ] `hds check` не дублирует пункты и корректно ведёт себя без Node/venv.

### A6.2 — автозапуск

- [ ] `sync --all` держит по `sync.lock` на проект; второй запуск — «уже запущен», код 0, без цикла.
- [ ] Недоступная папка — `problem`, остальные проекты обрабатываются.
- [ ] `hds stop`/`index.stop` завершает все проекты, lock снимаются, файл сигнала не удаляется (AG-27).
- [ ] Установщик задачи идемпотентен, `-Remove` снимает и задачу, и ярлык; аргументы с пробелами не ломаются.
- [ ] Fallback «Автозагрузка» не оставляет мусора и не запускает второй экземпляр.

### A6.3 — горячий конфиг

- [ ] Изменение `agent.sync.*`/`index.max_file_mb` подхватывается без перезапуска; `config reload` пишется только при изменении.
- [ ] Правка того же размера в пределах разрешения mtime — обнаруживается (или ограничение честно описано).
- [ ] `agent.root` на лету не меняется: `[!]` + `config root ignored`, работа на прежнем.
- [ ] Недописанный/битый YAML — строка `error`, работа на прежних настройках, без паники.
- [ ] `agent.enabled: false` — остановка `disabled`, без удаления `index.stop`/чужих файлов.

### A6.4 — откат, журнал, relink

- [ ] `restore`: выбор штампа, неизвестный штамп — список доступных, занятый файл — код 1 без изменения оригинала.
- [ ] `rebuild` — только `orphan`; `relink` — только по команде, маркер принадлежит проекту и записи реестра.
- [ ] «Принять»: копия → замена оригинала → пересборка образа; при сбое — откат (AG-23), `decision_failed`.
- [ ] `decisions.jsonl`: только пути/статусы/числа, ротация, отсутствие гонок при двух операциях.
- [ ] Пути: `..`, абсолютные, junction/симлинки, UNC — отказ; штампы не сталкиваются.

### A6.5 — удаление проекта

- [ ] `P` + 4 цифры; `..`/чужая зона/симлинк наружу — отказ.
- [ ] Живой демон — отказ с подсказкой `hds stop`; `--force` — осознанно; TOCTOU между проверкой lock и удалением.
- [ ] Удаляются зона, реестр, запись `projects.json`, `/projects/<P>` в `secrets.json`; **другие ключи secrets сохраняются**; папка проекта цела.
- [ ] `--purge-meta` не трогает метки других проектов; повтор — код 2; UI-коды 200/409/404.

### A6.6 — находки ревью

- [ ] AG-23: откат `decide` действительно атомарен (backup + registry + файлы).
- [ ] AG-25: кэш реестра сбрасывается после `rm` и после внешних изменений.
- [ ] AG-26: относительные имена образов превращаются в пути проекта без выхода за корень.
- [ ] AG-32: `history.uncleaned` не раскрывает содержимое и не мешает работе.

### A6.8 — K1 и значение по умолчанию

- [ ] `disk_search_enabled` default `false` и в `Default`, и в `settings()`; тесты проверяют оба пути.
- [ ] При `false` MCP-сервер не попадает в `cline_mcp_settings.json`; при `true` — попадает с Bearer.
- [ ] `agent-token` warn при выключенном поиске — ожидаем и не ломает `hds check`/UI.
- [ ] `agent_rag_eval.py`: prompt одинаков для (А)/(Б); usage суммируется из событий; `turn_end.tools` не путается с числом шагов; время измеряется; TLS-повторы не искажают числа; `--score-only` воспроизводит сводку.
- [ ] Полнота/точность считаются по эталонным именам и деанонимизированным путям; выдуманные имена ловятся; значений из vault в отчёте нет.
- [ ] Числа в `AGENT_RAG_K1.md` воспроизводятся из `k1-summary.json`/`k1-analysis.json` (без новых платных прогонов).

### Выпуск 0.27.0

- [ ] `release.yml`: только Windows; mandatory-ассеты соответствуют `RELEASING.md`; клиент вложен; source archive один; публикация идемпотентна; заметки находятся.
- [ ] `rust.yml`: ubuntu — компиляция всех целей, Windows — тесты; ветка релиза в списке.
- [ ] Правки тестов при выпуске (`pymorphy2`, путь глоссария от `CARGO_MANIFEST_DIR`, `mcp` внутри `agent`, ожидание `pass`) не маскируют дефекты и не делают тесты бессмысленными.
- [ ] Зеркало рантайма: URL и sha256 согласованы; `fetch_engine_runtime.ps1` скачивает и проверяет; mac-ассета нет.
- [ ] Версия `hds/__init__.py` = тег; `version.txt` в архиве = 0.27.0; заметки соответствуют ассетам.

## 6. Сквозные инварианты и контракты

| Инвариант | Где искать | Что проверить |
|---|---|---|
| **I1** оригиналы не меняются без решения | `mirror.rs`, `results.rs`, `agent_sync.rs` | прямое зеркало не пишет в папку проекта; `accept` — единственный путь замены; `restore` — из backup |
| **I2** значения не уходят модели | `hds-ui/src/agent.rs`, `hds-agent/src/filter.rs`, `hds-mcp/src/agent.rs` | фильтр на каждом запросе, fail-closed, обновление снимка по `vault_rev` |
| **I4** профиль Cline не тронут | `hds-agent/src/env.rs` | `HOME`/`APPDATA`/`TEMP` подменены; MCP/сессии не пишут в `%USERPROFILE%\.cline` |
| **I5** маршрут только свой проект | `hds-mcp/src/agent.rs` | `project_ctx`, `token_ok`, `ask_my_files` и управляющих инструментов нет |
| **I6** оригиналы не меняются операциями A6 | `results.rs`, `remove.rs` | `project rm` не трогает папку проекта; restore/rebuild пишут только оригинал по решению |
| **I7** ключи не в логах | `secrets.rs`, `diag.rs`, `agent*.rs` | ключи/токены только «задан/нет»; `--json`, `sync.log`, `decisions.jsonl` без значений |
| **I8** журналы без значений | `agent_sync.rs`, `results.rs`, `agent.rs` | только пути, метки, статусы, числа; проверить `decisions.jsonl`, `sync.log`, `messages.jsonl`, `egress\*` |
| Секреты | `secrets.rs` | merge при записи, права, `remove_project` не теряет чужие ключи |
| Атомарность | `results.rs`, `remove.rs`, `registry.rs` | temp+rename, откат при сбое, порядок «копия → замена → образ» |
| Блокировки | `agent_sync.rs`, `results.rs` | `sync.lock` на проект, занятый файл → код 1, без «полузаписанного» состояния |
| Windows-пути | `paths.rs`, `remove.rs`, UI | `..`, UNC, `\\?\`, ADS, junction — отказ; длинные пути |
| Коды ошибок | CLI | 0/1/2 и тексты соответствуют README/RELEASING; повторные операции идемпотентны |

**Не «улучшать» молча:** формат `sync.log`/`decisions.jsonl`, имена статусов реестра,
маршрут `/mcp/agent/<P>`, формат `state_json`, поведение `agent-token` warn, значение
`disk_search` по умолчанию (это решение владельца, D23/K1).

## 7. Гипотезы: что проверять в первую очередь

1. **A6.2 + AG-34.** `index.stop` общий для всех проектов и не обновляет mtime при
   повторном `hds stop`; демон, запущенный после остановки, сигнал игнорирует. Проверить:
   нет ли пути, где `sync --all` удаляет/перезаписывает чужой `index.stop`, и не
   «залипает» ли остановка на нескольких проектах.
2. **A6.3 mtime+size.** Правка файла того же размера в пределах разрешения mtime может
   не подхватиться. Оценить риск для `config.yaml` (пользователь правит значение на
   равное по длине) и честность документации.
3. **A6.4 restore/accept.** Гонка между проходом зеркала и `accept/restore`: не может ли
   образ пересобраться из старого оригинала, не остаётся ли `pending` без `base`,
   не теряется ли backup при сбое посередине.
4. **A6.5 remove.** TOCTOU: lock проверен, демон стартовал, удаление пошло; guard зоны
   против junction/симлинка наружу; `--purge-meta` не задевает метки других проектов;
   `secrets.json` merge сохраняет `providers` и `mcp`.
5. **A6.6 AG-23.** Откат `decide` при сбое `rebuild_image`: восстанавливаются ли файл,
   registry и `pending` согласованно; нет ли «осиротевших» записей.
6. **A6.1 diag.** `agent-route` ходит по HTTP с токеном: не печатает ли токен в ошибке,
   не делает ли лишних запросов, корректно ли различает «не слушает» и «401/500».
7. **A6.8 K1.** Справедливо ли сравнение: одинаковый prompt/лимиты/индекс; не считаются
   ли неудачные ходы дважды; не завышает ли scoring полноту за счёт имён в путях
   инструментов; воспроизводятся ли числа из сырых JSONL.
8. **Выпуск.** Не осталось ли в `release.yml` mac-путей/ассетов; не попали ли в архив
   venv/секреты; соответствует ли `RELEASING.md` фактическим mandatory-ассетам;
   не маскируют ли правки тестов (pymorphy2, cwd, YAML, ожидание pass) реальные дефекты.

## 8. Независимые проверки (не верить отчётам)

Выполнять **только чтение**; команды, меняющие стенд, — с оговоркой в отчёте.

```powershell
# Срез и объём
git status -sb
git --no-pager log --oneline 75912a0e..HEAD
git --no-pager diff --stat 75912a0e..HEAD

# Формат/линт/тесты (env стенда снять!)
$env:HDS_ROOT=$null; $env:HDS_CONFIG=$null
$env:CARGO_INCREMENTAL='0'
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -j 2 -- -D warnings
cargo check --workspace --all-targets          # то, что делает ubuntu-джоба
cargo test -p hds-agent -j 2
cargo test -p hds-cli -j 2 --no-fail-fast
cargo test -p hds-ui -j 2 --no-fail-fast
sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar
npm --prefix sidecar/hds_agent test
```

Стенд (если есть `tools\parity\out\agent_a42`):

```powershell
$env:HDS_CONFIG='C:\Users\Sasha\hermes-disk-search\tools\parity\out\agent_a42\config.yaml'
$env:HDS_ROOT  ='C:\Users\Sasha\hermes-disk-search\tools\parity\out\agent_a42\root'
target\debug\hds.exe agent check --project P0001 --json
target\debug\hds.exe agent run --project P0001 --dry-run
target\debug\hds.exe agent project list --json
# оценка K1 без платных прогонов:
sidecar\venv-anonymize\Scripts\python.exe tools\parity\agent_rag_eval.py `
  --config-a tools\parity\out\agent_a42\config.yaml `
  --config-b tools\parity\out\agent_a42\k1-config-B.yaml `
  --root tools\parity\out\agent_a42\root `
  --workspace tools\parity\out\agent_a42\zone\P0001\workspace `
  --golden tools\parity\out\agent_a42\golden-k1.jsonl `
  --out-dir tools\parity\out\agent_a42 --score-only
```

`a6_e2e.ps1` меняет стенд (демоны, restore, `project rm`) — **только `-Preflight`**
без разрешения владельца; полный прогон не запускать.

Релиз (без изменений): `gh release view v0.27.0 --json assets`;
`gh run view 45055365847 --json jobs` — зелёный Release.

Проверки «на утечки» и «на остатки»:

- `rg -n "disk_search_enabled: true|macos-latest|build-macos|fetch-engine-runtime-macos"` —
  не должно остаться в workflows/коде, кроме тестов и legacy-документации;
- `rg -n "sk-|api_key|token" crates/hds-*/src` — ключи только в `secrets.rs` и «задан/нет» в диагностике;
- в `sync.log`, `decisions.jsonl`, `messages.jsonl`, `egress\*` — нет значений из
  фикстур (искать 2–3 тестовых значения, не печатая их в отчёт);
- в архиве релиза нет `sidecar\venv-*`, mac-клиента (проверить листинг zip).

## 9. Формат отчёта `AGENT_A6_CODE_REVIEW.md`

```markdown
# AGENT_A6_CODE_REVIEW.md — код-ревью A6 и выпуска 0.27.0

> Дата, ветка, HEAD/тег, чем проверено (команды, прогоны), что не проверено.

## 1. Вердикт
Готово к эксплуатации / готово с замечаниями / не готово. 3–6 предложений.

## 2. Что проверено и как
Таблица: область → команда/чтение → результат (без пересказа отчётов).

## 3. Находки
| ID | Severity | Где (file:line) | Проблема | Почему это важно | Как исправить | Тест |
|---|---|---|---|---|---|---|
| AG6-01 | blocker/major/minor/nit | ... | ... | ... | ... | ... |

## 4. Регрессии A4–A5
AG-20…AG-33: закрыты / вернулись / не проверялись.

## 5. Выпуск 0.27.0
Windows-only, ассеты, зеркало, default disk_search, CI, версия.

## 6. Ограничения ревью
Что не запускалось (платные A/B, полный E2E, чистая установка) и почему.

## 7. План исправлений
Сначала blocker/major, с тестами; отдельно — долги фазы 2 (AG-34, флейк и т.п.).
```

**Severity:**
- **blocker** — утечка значений/секретов, потеря данных, сломанный выпуск/маршрут;
- **major** — неверное поведение эксплуатации, регрессия инварианта, ложная документация;
- **minor** — edge case, неудобство, неточность без потери данных;
- **nit** — стиль/текст.

## 10. Границы ревью

- **Не менять код** в процессе ревью: только отчёт. Патчи — после решения владельца.
- **Не менять решения плана** (`В-A6-*`, D19/D23), даже если кажутся спорными: предложение — в отчёт.
- **Не чинить** AG-34, флейк `agent_chat::abort_cancels_pending_ask` и тестовые
  «шумы» без отдельного решения.
- **Не запускать платные A/B** на ключе владельца без явного согласия; `--score-only` — можно.
- **Не трогать** релизы/теги, не делать push, не выпускать новый релиз.
- **Не использовать реальные документы** заказчиков; стенд — вымышленный.
- Чужие незакоммиченные изменения не откатывать.

## 11. Окружение и грабли

1. `HDS_ROOT`/`HDS_CONFIG` — только для команд стенда; перед `cargo test` снимать, иначе тесты идут по стенду.
2. Диск C: был переполнен (LNK1180); при нехватке — `target\debug\incremental`/`target\release`, `CARGO_INCREMENTAL=0`, `-j 2`.
3. `target\debug\hds.exe` может быть залочен запущенным `hds mcp-http`/`hds ui` — остановить перед сборкой.
4. `agent_acceptance::acceptance_a3_end_to_end` проверяет реальный `%USERPROFILE%\.cline`;
   при **активной сессии Cline** файлы `data\sessions\**` меняются и тест падает — это
   окружение, а не регресс; одиночный прогон без активной сессии проходит.
5. На ubuntu Windows-only тесты (`C:\…`) падают — поэтому тесты гоняются на Windows,
   а ubuntu проверяет `cargo check --all-targets`; не «чинить» это возвратом тестов на Linux.
6. Ключ провайдера на стенде — только в `tools\parity\out\agent_a42\root\data\agent\secrets.json`
   (в git нет); не печатать, не коммитить.
7. `a6_e2e.ps1` — только стенд и только по разрешению; `-Preflight` безопасен.
8. Сборка/тесты долгие: первый прогон 10–20 мин; не считать «зависанием».

## 12. Промт для нового чата (копировать целиком)

```text
Ты — независимый ревьюер проекта hermes-disk-search (Rust + Python/Node sidecar).
Задача — код-ревью фазы A6 (A6.1–A6.8) и выпуска 0.27.0. Ничего не исправляй:
только чтение, проверки и отчёт AGENT_A6_CODE_REVIEW.md.

Срез: ветка agent-privacy-zone, HEAD 6f0070b7, тег v0.27.0 (b8967937),
релиз выпущен, Windows-only, macOS не поставляется. Дерево чистое.

Сначала прочитай целиком AGENT_A6_CODE_REVIEW_PROMPT.md (контекст, карта кода,
чеклисты, гипотезы, формат отчёта, границы), затем обязательное чтение из §3:
PLAN_AGENT_PRIVACY_ZONE.md §3/§5.8–5.9/§6/§7; AGENT_A6_STAGES.md §2–§6;
AGENT_A6_REPORT.md; tools/parity/out/agent_a42/A6_E2E.md;
tools/parity/AGENT_RAG_K1.md; RELEASE_NOTES_0.27.0.md; RELEASING.md;
README.md §20/§21; .github/workflows/{release,rust}.yml;
runtime-manifests/engine-manifest*.json; AGENT_A4_A5_CODE_REVIEW.md.

Объём: git diff 75912a0e..HEAD (A6.1–A6.8 + выпуск). Особое внимание:
1) A6.2 --all и index.stop (AG-34), изоляция проектов и остановка;
2) A6.3 mtime+size и битый YAML;
3) A6.4 restore/accept: атомарность, гонки с зеркалом, backup/pending/base;
4) A6.5 project rm: guard зоны, junction/симлинк, TOCTOU с живым lock, merge secrets;
5) A6.6 AG-23/25/26/32 — закрыты и не вернулись ли;
6) A6.8 default disk_search=false, тесты, agent_rag_eval.py, воспроизводимость K1;
7) выпуск: Windows-only workflows, mandatory-ассеты, зеркало рантайма, правки тестов.

Проверки: fmt/clippy/cargo check на ubuntu-профиле, тесты на Windows (env стенда снять),
python/npm; на стенде — hds agent check, dry-run, --score-only K1 (без платных прогонов);
a6_e2e.ps1 только -Preflight; релиз читать через gh release view, не менять.

Формат отчёта — §9 файла-контекста: вердикт, что проверено и как, таблица находок
(ID, severity blocker/major/minor/nit, file:line, проблема, почему, исправление, тест),
регрессии A4–A5, выпуск, ограничения, план исправлений. Не меняй код, решения плана,
не чини AG-34/флейк, не делай push и не выпускай релиз. Значения из документов,
ключи и токены в отчёт не включай.
```
