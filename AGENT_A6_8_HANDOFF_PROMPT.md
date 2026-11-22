# AGENT_A6_8_HANDOFF_PROMPT.md — A6.8 (K1): замер RAG «с disk-search / без»

> **Дата среза:** 22.11.2026. **Ветка:** `agent-privacy-zone`, **HEAD:** `920a054d`
> («A6.7 — приёмка A6 (E2E пройден)»), дерево чистое, `origin` — ahead,
> **push не делался**.
> **Машина:** Windows 11, PowerShell 5.1, Rust 1.99, Node 24.18, Python 3.12
> (`sidecar\venv-anonymize`).
>
> **Что это.** Контекст и готовый промт для нового чата: под-этап `A6.8` плана
> `AGENT_A6_STAGES.md` §2 — **K1, замер RAG (D23)**: сравнение (А) агента с
> `disk-search` и (Б) без него на 15–20 вопросах стенда, `recall@k`/MRR поиска,
> правило решения из §6 `PLAN_AGENT_PRIVACY_ZONE.md`, отчёт
> `tools/parity/AGENT_RAG_K1.md`; после решения — фиксация значения
> `agent.mcp.disk_search.enabled` по умолчанию.

## 0. Что уже закрыто (A0–A6.7)

| Этап | Коммит | Что «снаружи» |
|---|---|---|
| A0–A3 | — | SDK-агент, анонимная зона, маршрут `/mcp/agent/<P>`, фильтр на границе модели, стенд `agent_a42` |
| A4 | — | зеркало папки проекта, решения, демон `hds agent sync`, `A46_E2E.md` |
| A5 | — | вкладка «Агент», SSE, «Результаты», метки, настройки; живой сценарий `A57_E2E.md` (76/76) |
| A6.1 | `018056d1` | 12 пунктов `agent-*` в `hds check`, `hds agent check [--project P] [--json]` |
| A6.2 | `15b70e84` | `hds agent sync --all`, задача Планировщика, `installers/install_agent_sync_task.ps1`, fallback «Автозагрузка» |
| A6.3 | `ac64a682` | горячее перечитывание `config.yaml`, `config reload …`, `root ignored` |
| A6.4 | `6dc86b2f` | `restore`/`rebuild`/`relink`, `.hds-agent\decisions.jsonl` |
| A6.5 | `4c19a999` | `hds agent project rm`, кнопка «Удалить…» |
| A6.6 | `beff5521` | AG-23/25/26/32 |
| **A6.7** | **`920a054d`** | **приёмка A6:** `a6_e2e.ps1` — **75/75 ok, 0 fail** (1 мин 35 с); I1 23→23, I8 `hits=0` (`decisions.jsonl`, `sync.log`, `a6-sync-live.log` 497 строк, `messages.jsonl`, `projects.json`, `egress`), I6 0/35. Отчёты: `AGENT_A6_REPORT.md`, `A6_E2E.md` (стенд), README §21, `RELEASE_NOTES_0.27.0.md` |

**Прогоны на срезе `920a054d`:** `cargo fmt --all -- --check` — 0;
`cargo clippy --workspace --all-targets -- -D warnings` — 0;
`cargo test -p hds-agent` — 180 lib + наборы, 0 failed;
`cargo test -p hds-cli` — все наборы 0 failed (`agent_project_rm` 5, `agent_result` 6,
`agent_run` 7, `agent_sync` 14, `agent_acceptance` 1);
`cargo test -p hds-ui --no-fail-fast` — 42 lib, `agent_api` 11, `agent_chat` 9,
остальные ok, 0 failed; python **335 OK** (skip 1); npm **23/23**.
Флейк `agent_chat::abort_cancels_pending_ask` — предсуществующий (падает и на
`0611159f`), к A6 не относится.

**Известная находка (фаза 2, НЕ чинить в A6.8):** повторный `hds stop` не обновляет
mtime пустого `index.stop` (AG-27), поэтому демон, запущенный после предыдущей
остановки, сигнал игнорирует. Кандидат AG-34; обход — снять свой файл перед новым
запуском (README §21.4, `A6_E2E.md` §4 п.1).

## 1. Задача этого чата и границы

**Сделать (A6.8 / K1):**

1. **Качество самого поиска** на стенде `agent_a42`: 15–20 вопросов с эталонными
   источниками, `recall@1/5/10/30` и MRR по анонимным образам (инструмент —
   `tools/parity/agent_a0_rag_eval.py`, формат golden — как в A0(10)).
2. **Сравнение конфигураций (А) и (Б) на готовом агенте** по методике §6 плана:
   (А) с `search_local_files` и правилом TR-3.8a, (Б) без `disk-search`; одна
   модель, по 2 прогона на вопрос; метрики — полнота 0/1/2 против эталона, точность,
   входные/выходные токены и стоимость, число шагов, время до ответа.
3. **Отчёт `tools/parity/AGENT_RAG_K1.md`** с правилом решения §6, числами, выводом
   и решением владельца; результаты прогонов — в `tools/parity/out/` (в репозиторий
   не идут, данные вымышленные).
4. **Зафиксировать значение по умолчанию** `agent.mcp.disk_search.enabled` по
   итогам решения:
   * оставляем включённым — ничего в коде не меняется (сейчас default `true`),
     фиксируется записью в отчёте и `AGENT_A6_REPORT.md`/`STATUS.md`;
   * выключаем — правка default в `crates/hds-agent/src/settings.rs`
     (`bool_at(cfg, "agent.mcp.disk_search.enabled", true)` → `false`), тест на
     default, README §20/§21, `RELEASE_NOTES_0.27.0.md`.

**Не делать:** A6.8 не выпускает релиз и не делает push; не трогать решения плана
(§3 `AGENT_A6_STAGES.md`, §6 `PLAN_AGENT_PRIVACY_ZONE.md`); не «лечить» флейк
`agent_chat::abort_cancels_pending_ask` и AG-34 (это фаза 2); не менять поведение
маршрута `/mcp/agent/<P>` и фильтра (I5/I8).

**Блокер, который надо решить в начале:** на стенде **нет ключа облачного
провайдера** (`hds agent check` → `agent-provider` = `warn`, A5.7 шёл на
mock-провайдере). Для K1 по плану нужна «одна облачная модель». Варианты:
(а) владелец даёт ключ и модель для `P0001` — тогда A/B считается по `usage` из
воркера и стоимости провайдера; (б) прогон на mock — честно измеримы шаги, вызовы
инструментов, размеры запросов и токены, посчитанные по телам запросов
(`agent.egress_log: true` или журнал mock), но **не** реальная стоимость; в отчёте
это фиксируется как ограничение, и решение по умолчанию принимается с оговоркой.
Спросить владельца до прогонов.

## 2. Контракты и проверенные факты (не переоткрывать)

| Что | Где | Факт |
|---|---|---|
| Настройка | `crates/hds-agent/src/settings.rs:193` | `disk_search_enabled: bool_at(cfg, "agent.mcp.disk_search.enabled", true)`; default `true` (`:83`), поле `:59`; тест default `:304`, тест выключения `:345` |
| Где применяется | `crates/hds-agent/src/run.rs:374` | при `false` → `mcp.token = None`; сервер `disk-search` не попадает в `cline_mcp_settings.json` (это streamableHttp на `/mcp/agent/<P>` с Bearer-токеном, `crates/hds-agent/src/mcp.rs:9-10`) |
| Предупреждение CLI | `crates/hds-cli/src/cmd/agent_run.rs:715` | `disk_search_enabled`, а токена нет → сообщение в план/вывод |
| Панель | `crates/hds-ui/src/agent.rs:2004`, `:2097`; `page.rs:1586` | POST поля `disk_search_enabled`, поле в `state_json`, переключатель «MCP disk-search» |
| Токены | `sidecar/hds_agent/dist/worker.mjs:188-190`, `:238` | воркер шлёт событие `kind:"usage"` и кладёт `usage` в `turn_end` (`inputTokens`/`outputTokens`) |
| CLI `--json` | `crates/hds-cli/src/cmd/agent_run.rs:289`, `:341-349` | строки `{"kind":"agent","event":{...}}` (включая `kind:"usage"`) и `{"kind":"turn_end", ok, reason, text, error, tools}`; **usage в `turn_end` CLI не печатает** — суммировать из `usage`-событий |
| План запроса | `hds agent run --project P0001 --dry-run` | печатает план и список MCP-серверов: быстрый контроль, что в (Б) `disk-search` исчез |
| Оценка поиска | `tools/parity/agent_a0_rag_eval.py` | `hds search <q> --limit 30 --json` в цикле; `recall@1/5/10/30`, MRR, `byType`, `byFirstExpectedExt`, `labelSearch`; golden — JSONL `{id, type, question, expected:[имена файлов]}` |
| Стенд | `tools\parity\out\agent_a42` | `config.yaml` (`agent.root=…\zone`, `db_path=…\state\index.db`, `mcp_http.port: 8788`, `embedding.base_url http://127.0.0.1:8011/v1`, `dim: 1024`), `root` (венв воркера, `anonymize.db` — 47 меток, схема v2), `project` — 35 файлов, `zone\P0001\workspace` — образы, `state\index.db` — 4,4 МБ (построен в A5.7), `a57-forms.json` — формы для сканов |
| Порты | 8788 (MCP стенда), 8799 (UI стенда); **8787 занят** службой машины (4 процесса `hds.exe` с 16.11, без прав остановки — `A57_E2E.md` §3 п.6) |

**Эталон A0(10) — ориентир, не результат A6.8** (`tools\parity\out\agent_a0\s10\`,
16 вопросов): `recall@10 = 1.0`, MRR `0.906`, recall@1 `0.812`, среднее 1521,7 мс;
по типам fact 5 / semantic 5 / compare 2 / contradiction 2 / label 2; поиск по
метке слабый: `[PERSON_3]` — 5 из 10 выдач, `[ORG_21]` — 1 из 10 (факт для TR-4.6,
не дефект K1). Golden оттуда переносится почти без правок: 15 из 16 имён файлов
совпадают с `agent_a42`, проверить `ТЗ 1С ERP сводное (подписанное).pdf.md`.

## 3. Методика K1 (из `PLAN_AGENT_PRIVACY_ZONE.md` §6 — не менять)

* **Вопросы:** 15–20 типовых вопросов аналитика и РП с эталонным ответом и списком
  файлов-источников, составленных вручную: факт («какой срок сдачи этапа 2?»), смысл
  («где обсуждали интеграцию с банком?»), сверка («какие требования реестра не
  отражены в ТЗ?»), противоречия между документами, вопрос по метке («что поручено
  `[PERSON_3]`?»).
* **Конфигурации:** (А) агент с `search_local_files` и правилом TR-3.8a; (Б) агент
  без `disk-search` — только `search_codebase` и чтение файлов. Одна и та же модель,
  по **2 прогона** на вопрос.
* **Метрики:** полнота ответа против эталона (найдены ли все источники, 0/1/2),
  точность (нет выдуманного), входные и выходные токены и стоимость за вопрос, число
  шагов агента, время до ответа.
* **Правило решения (дословно):** «RAG оставляем включённым, если (А) не хуже (Б) по
  полноте и экономит ≥ 25 % токенов на вопрос **или** заметно выигрывает по полноте
  на вопросах „поиск по смыслу“. Иначе `agent.mcp.disk_search.enabled: false` по
  умолчанию (D23)». Решение фиксируется вместе с владельцем в
  `tools/parity/AGENT_RAG_K1.md`.
* **Данные:** только вымышленный проект стенда; результаты — в `tools/parity/out/`
  (в репозиторий не идут); значения из базы меток в отчёт не попадают (I7/I8).

## 4. Практический план (порядок работ)

1. **Спросить владельца про провайдера** (блокер §1): ключ + модель или согласие на
   mock с оговоркой. Выбор зафиксировать в отчёте.
2. **Golden для стенда:** взять `tools\parity\out\agent_a0\s10\golden.jsonl`,
   перенести в `tools\parity\out\agent_a42\golden-k1.jsonl`, сверить имена с файлами
   `project`, дополнить до 15–20 вопросов по типам §3 (эталон — имена файлов, без
   значений из базы меток).
3. **Индекс стенда:** проверить, что `state\index.db` соответствует 35 файлам (при
   расхождении — `hds index --full` / `hds agent init`; эмбеддер
   `http://127.0.0.1:8011/v1` должен быть жив, иначе каждый файл падает через 20 с —
   грабля A5.7 §3 п.7).
4. **Качество поиска:** прогнать `agent_a0_rag_eval.py` с конфигом стенда
   (`HDS_CONFIG=…\agent_a42\config.yaml`, `HDS_ROOT` снять) → `k1-search-eval.json`;
   записать recall@k/MRR общие и по типам, отдельно — метки.
5. **A/B агента:** для (А) поднять `hds mcp-http start` (порт 8788, `/health` = 200);
   для (Б) — копия конфига с `agent.mcp.disk_search.enabled: false` (`HDS_CONFIG` на
   копию), `--dry-run` подтверждает отсутствие `disk-search`. 15–20 вопросов × 2
   прогона × 2 конфигурации = 60–80 ходов; токены — сумма `usage`-событий,
   инструменты — `turn_end.tools`, время — по меткам лога, полнота 0/1/2 и точность —
   по чек-листу эталона.
6. **Свести и решить:** медианы по 2 прогонам, сравнение по правилу §3, решение
   владельца.
7. **Отчёт** `tools/parity/AGENT_RAG_K1.md`: методика (ссылка на §6), стенд и
   golden, числа по (А)/(Б), что показал поиск сам по себе, ограничения (mock/ключ,
   размер выборки), решение и следствия.
8. **Если решено выключить:** default в `settings.rs` → `false` + тест + README
   §20/§21 и `RELEASE_NOTES_0.27.0.md`; если оставить — только запись в отчёте,
   `AGENT_A6_REPORT.md` и `STATUS.md`.
9. **Проверки и коммит** (§7). Коммит: `feat(agent): A6.8 — K1, замер RAG (с
   disk-search и без)`; если default не меняется — `docs(agent): A6.8 — K1, замер
   RAG`. Push не делать.

**Оценка:** 1–2 дня (план §2). Основное время — прогоны: 60–80 ходов агента, каждый
с подъёмом воркера; при 15–30 с на ход это 30–40 минут машинного времени плюс разбор
ответов.

## 5. Что уже есть и чего нет

**Есть:**
* стенд `agent_a42` в рабочем состоянии (после A6.7): проект, зона, реестр, индекс
  4,4 МБ, `anonymize.db` схема v2, формы `a57-forms.json`;
* golden A0(10) на 16 вопросов (`tools\parity\out\agent_a0\s10\golden.jsonl`) и
  скрипт `tools/parity/agent_a0_rag_eval.py` (recall@k/MRR);
* mock-провайдер `tools\parity\out\agent_a42\a57_mock.mjs` (OpenAI-совместимый,
  A5.7) и `mock_provider.mjs` в фикстурах CLI;
* протокол A6.7 (`A6_E2E.md`) — как запускать стенд и что не трогать.

**Нет (создать в этом чате):**
* `tools/parity/agent_rag_eval.py` — сценарии A/B через `hds agent run` (план §6
  называет именно этот скрипт: прогон вопросов, сбор `usage`-событий,
  `turn_end.tools`, времени; сохранение `k1-<A|B>-q<id>-r<1|2>.jsonl` и сводки);
* golden для `agent_a42` (`golden-k1.jsonl`) — 15–20 вопросов, имена файлов сверены
  с `project`;
* ключа облачного провайдера на стенде (блокер §1);
* отчёта `tools/parity/AGENT_RAG_K1.md`.

## 6. Грабли (оплачены, не наступать)

1. **`HDS_ROOT`/`HDS_CONFIG`** — только внутри процесса прогона; перед `cargo test`
   снимать переменные, иначе тесты идут по стенду.
2. **Первый подъём воркера** — 30–60 с (GLiNER); таймаут хода
   `agent.request_timeout_sec` по умолчанию 120 с — для длинных вопросов с чтением
   нескольких документов может не хватить, проверять `turn_end.reason`.
3. **`hds agent run --json` смешивает stderr воркера** (pymorphy2, GLiNER) в общий
   поток: разбирать только строки, начинающиеся с `{`.
4. **MCP-сервер стенда — порт 8788** (8787 занят службой машины); для (А) нужен
   `hds mcp-http start` и `/health` = 200, иначе `search_local_files` не отвечает;
   для (Б) сервер не нужен. После прогонов — `hds mcp-http stop`.
5. **Индекс зоны требует живого эмбеддера** (`embedding.base_url` стенда —
   `http://127.0.0.1:8011/v1`): без него `hds index --full` падает на каждом файле
   через 20 с; перед замером проверить `hds search` на одном вопросе.
6. **`agent-state` — зона агента**: история SDK до очистки содержит открытые
   значения; в K1 её не сканировать и не выкладывать.
7. **Секреты и значения**: в отчёт, golden, логи и коммиты — только имена файлов,
   метки и числа (I7/I8); `secrets.json` — только «запись есть/нет».
8. **Mock ≠ провайдер**: на mock нет реальной стоимости и поведения модели; токены
   тогда считаются по телам запросов (egress/журнал mock) и помечаются как оценка.
   Не выдавать mock-прогон за замер D23 без оговорки.
9. **Одинаковые условия A/B**: одна модель, одни `agent.max_live_projects`,
   `request_timeout_sec`, `max-iterations`, один индекс и golden; менять только
   `agent.mcp.disk_search.enabled`.
10. **Демон и Планировщик в K1 не нужны**: `hds agent run` работает сам, задачу не
    ставить, `a6_e2e.ps1` не запускать (приёмка A6.7 уже пройдена).
11. **Мусор**: артефакты — в `tools\parity\out\agent_a42\k1-*` (каталог гитигнорится),
    в репозиторий идут только отчёт и (при решении выключить) правка default.

## 7. Проверки и команды

| Что | Команда |
|---|---|
| Формат / линт | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` (при os error 1455 — `-j 2`) |
| Rust-тесты | `$env:HDS_ROOT=$null; $env:HDS_CONFIG=$null; cargo test -p hds-agent; cargo test -p hds-cli; cargo test -p hds-ui --no-fail-fast` (флейк `agent_chat::abort_cancels_pending_ask` — оговорка) |
| Python / Node | `sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar`; `npm --prefix sidecar/hds_agent test` |
| Поиск | `hds search "<вопрос>" --limit 30 --json` с `HDS_CONFIG` стенда |
| Оценка поиска | `python tools\parity\agent_a0_rag_eval.py --hds target\debug\hds.exe --config tools\parity\out\agent_a42\config.yaml --golden tools\parity\out\agent_a42\golden-k1.jsonl --out tools\parity\out\agent_a42\k1-search-eval.json` |
| План агента | `hds agent run --project P0001 --dry-run` (список MCP-серверов: в (Б) `disk-search` нет) |
| Срез | `git status --short`, `git --no-pager diff --check` |

**Коммит:** один по итогам — `feat(agent): A6.8 — K1, замер RAG (с disk-search и
без)` или `docs(agent): A6.8 — K1, замер RAG` (если default не меняется). Push не
делать; релиз не выпускать.

## 8. Промт для нового чата (копировать целиком)

```text
Ты — разработчик проекта hermes-disk-search (Rust + Python/Node sidecar).
Задача — под-этап A6.8 плана AGENT_A6_STAGES.md: K1 — замер RAG, сравнение
агента (А) с disk-search и (Б) без него; отчёт tools/parity/AGENT_RAG_K1.md;
по итогам — значение по умолчанию agent.mcp.disk_search.enabled (D23,
PLAN_AGENT_PRIVACY_ZONE.md §6).

Сначала прочитай целиком AGENT_A6_8_HANDOFF_PROMPT.md (контракты, методика,
план, грабли, проверки), затем PLAN_AGENT_PRIVACY_ZONE.md §6 (K1 и правило
решения) и D23 в §3, AGENT_A6_STAGES.md (§2 A6.8, §5 долги), AGENT_A6_REPORT.md
и tools/parity/out/agent_a42/A6_E2E.md (состояние стенда и приёмка A6.7),
tools/parity/agent_a0_rag_eval.py и tools/parity/out/agent_a0/s10/golden.jsonl
(формат эталона и базовый замер A0), crates/hds-agent/src/settings.rs и run.rs
(где применяется disk_search_enabled), sidecar/hds_agent/dist/worker.mjs
(событие usage). Срез: ветка agent-privacy-zone, HEAD 920a054d, дерево чистое.

Сделай:
1. Спроси владельца про провайдера (на стенде ключа нет): ключ+модель для P0001
   или согласие на mock с оговоркой в отчёте. Без этого A/B не начинай.
2. Подготовь golden-k1.jsonl (15–20 вопросов; типы: факт, смысл, сверка,
   противоречия, метка; эталон — имена файлов) на стенде agent_a42.
3. Прогони оценку качества поиска agent_a0_rag_eval.py по конфигу стенда:
   recall@1/5/10/30, MRR, по типам, поиск по метке; сохрани k1-search-eval.json.
4. Создай tools/parity/agent_rag_eval.py: прогоны hds agent run --json по
   (А) и (Б) (15–20 вопросов × 2 прогона), суммирование usage-событий,
   turn_end.tools, время, сохранение сырых JSONL и сводки в out/agent_a42/k1-*.
   Для (А) подними hds mcp-http start (8788), для (Б) — конфиг с
   agent.mcp.disk_search.enabled: false и --dry-run без disk-search.
5. Сведи метрики (полнота 0/1/2, точность, токены, шаги, время), примени правило
   решения §6, зафиксируй решение владельца.
6. Напиши tools/parity/AGENT_RAG_K1.md: методика, стенд, golden, числа (А)/(Б),
   качество самого поиска, ограничения, решение и следствия.
7. Если решено выключить — поменяй default в settings.rs (bool_at ... true →
   false), добавь/поправь тест, обнови README §20/§21 и RELEASE_NOTES_0.27.0.md;
   если оставить — отрази это в AGENT_A6_REPORT.md и STATUS.md.
8. Прогоны: cargo fmt --all -- --check; cargo clippy --workspace --all-targets --
   -D warnings (при os error 1455 — -j 2); cargo test -p hds-agent / -p hds-cli /
   -p hds-ui (флейк agent_chat::abort_cancels_pending_ask — оговорка); python и
   npm; git --no-pager diff --check.
Коммит: feat(agent): A6.8 — K1, замер RAG (с disk-search и без) или
docs(agent): A6.8 — K1, замер RAG. Push не делать, релиз не выпускать.
Не трогать решения плана, не чинить AG-34 и флейк, не запускать a6_e2e.ps1.
```

**После A6.8:** фаза 2 (`PLAN_AGENT_PRIVACY_ZONE.md` §8) и решение владельца о
выпуске 0.27.0 (`RELEASE_NOTES_0.27.0.md` подготовлены, приёмка A6.7 пройдена).
